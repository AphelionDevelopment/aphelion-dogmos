#![cfg(all(windows, target_arch = "x86"))]

use dogmos_byond::{ClientError, DogmosClient};
use dogmos_protocol::{
	encode_adjacency_batch, encode_gas_metadata_batch, encode_lifecycle_batch,
	encode_mixture_state_batch, encode_turf_heat_batch, encode_turf_lifecycle_batch,
	read_frame_into, write_frame, AdjacencyMutation, BuildIdentity, CallbackBatchHeader,
	CallbackBatchRequest, CallbackEvent, CallbackScope, CapacityLimits, FrontierBeginRequest,
	FrontierCommitRequest, GasMetadataRegistration, HandshakePayload, LifecycleAction,
	LifecycleMutation, MixtureSnapshot, MixtureSnapshotRequest, MixtureStateMutation,
	MixtureStateUploadAbortRequest, MixtureStateUploadAppendRequest,
	MixtureStateUploadBeginRequest, MixtureStateUploadBeginResponse,
	MixtureStateUploadCommitRequest, OperationKind, ProtocolHeader, ScalarValue, ServiceErrorCode,
	ServiceTelemetry, SimulationStage, SimulationStageRequest, TurfHeatMutation, TurfHeatSnapshot,
	TurfHeatSnapshotRequest, TurfHeatState, TurfLifecycleMutation, WireGasFireRole, WireHandle,
	CALLBACK_BATCH_HEADER_LEN, CALLBACK_EVENT_LEN, DOGMOS_ABI_VERSION, DOGMOS_PROTOCOL_VERSION,
	FLAG_ERROR, HANDSHAKE_PAYLOAD_LEN, MAX_CONTROL_PAYLOAD, MAX_GAS_SLOTS, MIXTURE_SNAPSHOT_LEN,
	SERVICE_TELEMETRY_LEN, SIMULATION_STAGE_RESPONSE_LEN, TURF_HEAT_SNAPSHOT_LEN,
};
use interprocess::local_socket::{prelude::*, ConnectOptions, GenericNamespaced, Stream};
use std::{
	io::{self, Write},
	process::{Child, Command, Stdio},
	time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct ChildGuard(Child);

impl Drop for ChildGuard {
	fn drop(&mut self) {
		let _ = self.0.kill();
		let _ = self.0.wait();
	}
}

fn handshake(service_path: &std::path::Path) -> HandshakePayload {
	HandshakePayload {
		auth_token: [0x5a; 32],
		identity: BuildIdentity {
			abi_version: DOGMOS_ABI_VERSION,
			protocol_version: DOGMOS_PROTOCOL_VERSION,
			source_revision: [0x11; 20],
			feature_fingerprint: [0x22; 32],
			executable_digest: dogmos_identity::sha256_file(service_path).unwrap(),
		},
		capacities: CapacityLimits {
			max_control_payload: MAX_CONTROL_PAYLOAD,
			max_batch_operations: 4096,
			max_callback_events: 1024,
			max_pending_continuations: 1024,
			max_frontier_handles: 4096,
			max_stage_work_items: 4096,
			max_reaction_transactions: 1024,
			reserved: 0,
			max_world_bytes: 8 * 1024 * 1024 * 1024,
		},
		process_id: std::process::id(),
		world_generation: 1,
		world_nonce: 0x1234_5678_90ab_cdef,
	}
}

fn gas_metadata(count: usize) -> Vec<GasMetadataRegistration> {
	(0..count)
		.map(|id| GasMetadataRegistration {
			id: id as u16,
			key: format!("gas_{id}"),
			name: format!("Gas {id}"),
			flags: 0,
			specific_heat: ScalarValue(20.0 + id as f64),
			fusion_power: ScalarValue(0.0),
			moles_visible: None,
			enthalpy: ScalarValue(0.0),
			fire_radiation_released: ScalarValue(0.0),
			fire_role: WireGasFireRole::None,
			fire_products: None,
		})
		.collect()
}

fn connect_raw(endpoint: &str, timeout: Duration) -> Stream {
	let name = endpoint.to_ns_name::<GenericNamespaced>().unwrap();
	let deadline = Instant::now() + timeout;
	loop {
		match ConnectOptions::new().name(name.clone()).connect_sync() {
			Ok(stream) => return stream,
			Err(error)
				if Instant::now() < deadline
					&& matches!(
						error.kind(),
						io::ErrorKind::NotFound
							| io::ErrorKind::ConnectionRefused
							| io::ErrorKind::WouldBlock
					) =>
			{
				std::thread::sleep(Duration::from_millis(5));
			}
			Err(error) => panic!("failed to connect to dogmosd: {error}"),
		}
	}
}

fn raw_round_trip(
	stream: &mut Stream,
	expected: HandshakePayload,
	operation: OperationKind,
	request_id: u64,
	payload: &[u8],
) -> (ProtocolHeader, Vec<u8>) {
	let request = ProtocolHeader::request(
		operation,
		request_id,
		expected.world_generation,
		expected.world_nonce,
		payload.len() as u32,
		0,
	);
	write_frame(stream, request, payload).unwrap();
	let mut response_payload = vec![0_u8; MAX_CONTROL_PAYLOAD as usize];
	let (response, response_len) = read_frame_into(stream, &mut response_payload).unwrap();
	response.validate_response_to(&request).unwrap();
	response_payload.truncate(response_len);
	(response, response_payload)
}

#[test]
fn service_rejects_duplicate_and_decreasing_request_ids_without_losing_the_session() {
	let unique = SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.unwrap()
		.as_nanos();
	let endpoint = format!("dogmos-sequence-{pid}-{unique}", pid = std::process::id());
	let service_path = std::path::Path::new(env!("CARGO_BIN_EXE_dogmosd"));
	let expected = handshake(service_path);
	let mut service = ChildGuard(
		Command::new(service_path)
			.arg("--echo-server")
			.arg(&endpoint)
			.stdin(Stdio::piped())
			.stdout(Stdio::null())
			.stderr(Stdio::inherit())
			.spawn()
			.unwrap(),
	);
	service
		.0
		.stdin
		.take()
		.unwrap()
		.write_all(&expected.encode())
		.unwrap();
	let mut stream = connect_raw(&endpoint, Duration::from_secs(5));

	let (response, payload) = raw_round_trip(
		&mut stream,
		expected,
		OperationKind::Handshake,
		10,
		&expected.encode(),
	);
	assert_eq!(response.flags & FLAG_ERROR, 0);
	assert_eq!(payload.len(), HANDSHAKE_PAYLOAD_LEN);

	let (response, payload) =
		raw_round_trip(&mut stream, expected, OperationKind::Echo, 11, b"first");
	assert_eq!(response.flags & FLAG_ERROR, 0);
	assert_eq!(payload, b"first");

	for rejected_id in [11, 10, 1] {
		let (response, payload) = raw_round_trip(
			&mut stream,
			expected,
			OperationKind::Echo,
			rejected_id,
			b"replay",
		);
		assert_ne!(response.flags & FLAG_ERROR, 0);
		assert_eq!(
			ServiceErrorCode::decode(&payload).unwrap(),
			ServiceErrorCode::InvalidRequest
		);
	}

	let (response, payload) = raw_round_trip(
		&mut stream,
		expected,
		OperationKind::Echo,
		12,
		b"still live",
	);
	assert_eq!(response.flags & FLAG_ERROR, 0);
	assert_eq!(payload, b"still live");

	let (response, payload) =
		raw_round_trip(&mut stream, expected, OperationKind::Shutdown, 13, &[]);
	assert_eq!(response.flags & FLAG_ERROR, 0);
	assert!(payload.is_empty());
	assert!(service.0.wait().unwrap().success());
}

#[test]
fn cross_process_handshake_echo_single_client_and_shutdown() {
	let unique = SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.unwrap()
		.as_nanos();
	let endpoint = format!("dogmos-{pid}-{unique}", pid = std::process::id());
	let service_path = std::path::Path::new(env!("CARGO_BIN_EXE_dogmosd"));
	let expected = handshake(service_path);
	let mut service = ChildGuard(
		Command::new(service_path)
			.arg("--echo-server")
			.arg(&endpoint)
			.stdin(Stdio::piped())
			.stdout(Stdio::null())
			.stderr(Stdio::inherit())
			.spawn()
			.unwrap(),
	);
	service
		.0
		.stdin
		.take()
		.unwrap()
		.write_all(&expected.encode())
		.unwrap();
	let mut wrong_token = expected;
	wrong_token.auth_token[0] ^= 1;
	assert!(matches!(
		DogmosClient::connect(&endpoint, wrong_token, Duration::from_secs(5)),
		Err(ClientError::Server(
			dogmos_protocol::ServiceErrorCode::AuthenticationFailed
		))
	));

	let mut client = DogmosClient::connect(&endpoint, expected, Duration::from_secs(5)).unwrap();
	assert_ne!(client.peer().process_id, expected.process_id);
	assert_eq!(
		client.echo(b"cross-bitness dogmos").unwrap(),
		b"cross-bitness dogmos"
	);
	let mut scalar = [0_u8; 8];
	assert_eq!(
		client
			.round_trip_into(OperationKind::ScalarGet, &[0; 8], &mut scalar)
			.unwrap(),
		8
	);
	let mut no_response = [];
	assert_eq!(
		client
			.round_trip_into(OperationKind::ScalarSet, &[0; 24], &mut no_response)
			.unwrap(),
		0
	);
	let mut gas_vector = [0_u8; 260];
	assert_eq!(
		client
			.round_trip_into(OperationKind::GasVector, &[0; 8], &mut gas_vector)
			.unwrap(),
		260
	);
	let enqueue_request = CallbackBatchRequest {
		max_events: 1024,
		scope: CallbackScope::General,
		transaction_id: 0,
	}
	.encode()
	.unwrap();
	let mut accepted = [0_u8; 4];
	assert_eq!(
		client
			.round_trip_into(
				OperationKind::DiagnosticCallbackEnqueue,
				&enqueue_request,
				&mut accepted,
			)
			.unwrap(),
		4
	);
	assert_eq!(u32::from_le_bytes(accepted), 1024);
	assert!(matches!(
		client.round_trip_into(
			OperationKind::DiagnosticCallbackEnqueue,
			&CallbackBatchRequest {
				max_events: 1,
				scope: CallbackScope::General,
				transaction_id: 0,
			}
			.encode()
			.unwrap(),
			&mut accepted,
		),
		Err(ClientError::Server(ServiceErrorCode::CallbackBackpressure))
	));
	let callback_request = CallbackBatchRequest {
		max_events: 64,
		scope: CallbackScope::General,
		transaction_id: 0,
	}
	.encode()
	.unwrap();
	let mut callbacks = [0_u8; CALLBACK_BATCH_HEADER_LEN + 64 * CALLBACK_EVENT_LEN];
	assert_eq!(
		client
			.round_trip_into(
				OperationKind::CallbackBatch,
				&callback_request,
				&mut callbacks,
			)
			.unwrap(),
		callbacks.len()
	);
	let callback_header =
		CallbackBatchHeader::decode(&callbacks[..CALLBACK_BATCH_HEADER_LEN]).unwrap();
	assert_eq!(callback_header.returned, 64);
	assert_eq!(callback_header.remaining, 960);
	assert_eq!(callback_header.capacity, 1024);
	assert_eq!(callback_header.high_water, 1024);
	assert_eq!(callback_header.rejected, 1);
	assert_eq!(
		CallbackEvent::decode(
			&callbacks[CALLBACK_BATCH_HEADER_LEN..CALLBACK_BATCH_HEADER_LEN + CALLBACK_EVENT_LEN]
		)
		.unwrap()
		.scope_sequence,
		1
	);
	assert_eq!(
		client.allocate_diagnostic(1024 * 1024).unwrap(),
		1024 * 1024
	);
	assert_eq!(client.allocate_diagnostic(0).unwrap(), 0);
	let mut gas_metadata_request = Vec::new();
	encode_gas_metadata_batch(&gas_metadata(MAX_GAS_SLOTS), &mut gas_metadata_request).unwrap();
	let mut processed = [0_u8; 4];
	client
		.round_trip_into(
			OperationKind::GasMetadataInstall,
			&gas_metadata_request,
			&mut processed,
		)
		.unwrap();
	assert_eq!(u32::from_le_bytes(processed), MAX_GAS_SLOTS as u32);

	let lifecycle = (0..64)
		.map(|slot| LifecycleMutation {
			action: LifecycleAction::Register,
			handle: WireHandle {
				slot,
				generation: 1,
			},
		})
		.collect::<Vec<_>>();
	let mut lifecycle_request = Vec::new();
	encode_lifecycle_batch(&lifecycle, &mut lifecycle_request).unwrap();
	client
		.round_trip_into(
			OperationKind::MixtureLifecycleBatch,
			&lifecycle_request,
			&mut processed,
		)
		.unwrap();
	assert_eq!(u32::from_le_bytes(processed), 64);

	let mixture_states = (0..64)
		.map(|slot| {
			let mut gases = [ScalarValue(0.0); MAX_GAS_SLOTS];
			gases[0] = ScalarValue(f64::from(slot + 1));
			MixtureStateMutation {
				handle: WireHandle {
					slot,
					generation: 1,
				},
				expected_revision: 0,
				temperature: ScalarValue(293.15),
				volume: ScalarValue(2500.0),
				gases,
			}
		})
		.collect::<Vec<_>>();
	let mut mixture_state_request = Vec::new();
	encode_mixture_state_batch(&mixture_states, &mut mixture_state_request).unwrap();
	client
		.round_trip_into(
			OperationKind::MixtureStateBatch,
			&mixture_state_request,
			&mut processed,
		)
		.unwrap();
	assert_eq!(u32::from_le_bytes(processed), 64);
	assert!(matches!(
		client.round_trip_into(
			OperationKind::MixtureStateBatch,
			&mixture_state_request,
			&mut processed,
		),
		Err(ClientError::Server(ServiceErrorCode::RevisionMismatch))
	));

	let upload_mutations = [
		MixtureStateMutation {
			handle: WireHandle {
				slot: 0,
				generation: 1,
			},
			expected_revision: 1,
			temperature: ScalarValue(300.0),
			volume: ScalarValue(2500.0),
			gases: [ScalarValue(0.0); MAX_GAS_SLOTS],
		},
		MixtureStateMutation {
			handle: WireHandle {
				slot: 1,
				generation: 1,
			},
			expected_revision: 1,
			temperature: ScalarValue(301.0),
			volume: ScalarValue(2500.0),
			gases: [ScalarValue(0.0); MAX_GAS_SLOTS],
		},
	];
	let mut upload_response = [0_u8; 8];
	client
		.round_trip_into(
			OperationKind::MixtureStateUploadBegin,
			&MixtureStateUploadBeginRequest { expected_count: 2 }
				.encode()
				.unwrap(),
			&mut upload_response,
		)
		.unwrap();
	let upload_id = MixtureStateUploadBeginResponse::decode(&upload_response)
		.unwrap()
		.upload_id;
	let append = MixtureStateUploadAppendRequest {
		upload_id,
		offset: 0,
		mutations: vec![upload_mutations[0]],
	}
	.encode()
	.unwrap();
	client
		.round_trip_into(
			OperationKind::MixtureStateUploadAppend,
			&append,
			&mut processed,
		)
		.unwrap();
	let mut first_snapshot = [0_u8; MIXTURE_SNAPSHOT_LEN];
	client
		.round_trip_into(
			OperationKind::MixtureSnapshot,
			&MixtureSnapshotRequest {
				handle: upload_mutations[0].handle,
			}
			.encode(),
			&mut first_snapshot,
		)
		.unwrap();
	assert_eq!(
		MixtureSnapshot::decode(&first_snapshot).unwrap().revision,
		1
	);
	assert!(matches!(
		client.round_trip_into(
			OperationKind::MixtureStateUploadCommit,
			&MixtureStateUploadCommitRequest { upload_id }.encode(),
			&mut processed,
		),
		Err(ClientError::Server(
			ServiceErrorCode::MixtureStateUploadIncomplete
		))
	));
	let append = MixtureStateUploadAppendRequest {
		upload_id,
		offset: 1,
		mutations: vec![upload_mutations[1]],
	}
	.encode()
	.unwrap();
	client
		.round_trip_into(
			OperationKind::MixtureStateUploadAppend,
			&append,
			&mut processed,
		)
		.unwrap();
	client
		.round_trip_into(
			OperationKind::MixtureStateUploadCommit,
			&MixtureStateUploadCommitRequest { upload_id }.encode(),
			&mut processed,
		)
		.unwrap();
	assert_eq!(u32::from_le_bytes(processed), 2);

	client
		.round_trip_into(
			OperationKind::MixtureStateUploadBegin,
			&MixtureStateUploadBeginRequest { expected_count: 2 }
				.encode()
				.unwrap(),
			&mut upload_response,
		)
		.unwrap();
	let upload_id = MixtureStateUploadBeginResponse::decode(&upload_response)
		.unwrap()
		.upload_id;
	let append = MixtureStateUploadAppendRequest {
		upload_id,
		offset: 0,
		mutations: vec![MixtureStateMutation {
			expected_revision: 2,
			temperature: ScalarValue(500.0),
			..upload_mutations[0]
		}],
	}
	.encode()
	.unwrap();
	client
		.round_trip_into(
			OperationKind::MixtureStateUploadAppend,
			&append,
			&mut processed,
		)
		.unwrap();
	client
		.round_trip_into(
			OperationKind::MixtureStateUploadAbort,
			&MixtureStateUploadAbortRequest { upload_id }.encode(),
			&mut [],
		)
		.unwrap();
	client
		.round_trip_into(
			OperationKind::MixtureSnapshot,
			&MixtureSnapshotRequest {
				handle: upload_mutations[0].handle,
			}
			.encode(),
			&mut first_snapshot,
		)
		.unwrap();
	let first_snapshot = MixtureSnapshot::decode(&first_snapshot).unwrap();
	assert_eq!(first_snapshot.revision, 2);
	assert_eq!(first_snapshot.temperature, ScalarValue(300.0));

	let snapshot_request = MixtureSnapshotRequest {
		handle: WireHandle {
			slot: 7,
			generation: 1,
		},
	};
	let mut snapshot_bytes = [0_u8; MIXTURE_SNAPSHOT_LEN];
	assert_eq!(
		client
			.round_trip_into(
				OperationKind::MixtureSnapshot,
				&snapshot_request.encode(),
				&mut snapshot_bytes,
			)
			.unwrap(),
		MIXTURE_SNAPSHOT_LEN
	);
	let snapshot = MixtureSnapshot::decode(&snapshot_bytes).unwrap();
	assert_eq!(snapshot.gas_count, 32);
	assert_eq!(snapshot.revision, 1);
	assert_eq!(snapshot.gases[0], ScalarValue(8.0));

	let turf = WireHandle {
		slot: 7,
		generation: 1,
	};
	let mut turf_lifecycle_request = Vec::new();
	encode_turf_lifecycle_batch(
		&[TurfLifecycleMutation {
			action: LifecycleAction::Register,
			turf,
			mixture: Some(snapshot_request.handle),
		}],
		&mut turf_lifecycle_request,
	)
	.unwrap();
	client
		.round_trip_into(
			OperationKind::TurfLifecycleBatch,
			&turf_lifecycle_request,
			&mut processed,
		)
		.unwrap();
	let mut turf_heat_request = Vec::new();
	encode_turf_heat_batch(
		&[TurfHeatMutation {
			turf,
			state: Some(TurfHeatState {
				temperature: ScalarValue(700.0),
				thermal_conductivity: ScalarValue(0.4),
				heat_capacity: ScalarValue(2500.0),
				adjacent_to_space: true,
			}),
		}],
		&mut turf_heat_request,
	)
	.unwrap();
	client
		.round_trip_into(
			OperationKind::TurfHeatBatch,
			&turf_heat_request,
			&mut processed,
		)
		.unwrap();
	let mut turf_heat_snapshot = [0_u8; TURF_HEAT_SNAPSHOT_LEN];
	client
		.round_trip_into(
			OperationKind::TurfHeatSnapshot,
			&TurfHeatSnapshotRequest { turf }.encode(),
			&mut turf_heat_snapshot,
		)
		.unwrap();
	assert_eq!(
		TurfHeatSnapshot::decode(&turf_heat_snapshot).unwrap().state,
		Some(TurfHeatState {
			temperature: ScalarValue(700.0),
			thermal_conductivity: ScalarValue(f64::from(0.4_f32)),
			heat_capacity: ScalarValue(2500.0),
			adjacent_to_space: true,
		})
	);

	let adjacency = (0..64)
		.map(|slot| AdjacencyMutation {
			left: WireHandle {
				slot,
				generation: 1,
			},
			right: WireHandle {
				slot: (slot + 1) % 64,
				generation: 1,
			},
			conductivity: ScalarValue(0.75),
		})
		.collect::<Vec<_>>();
	let mut adjacency_request = Vec::new();
	encode_adjacency_batch(&adjacency, &mut adjacency_request).unwrap();
	client
		.round_trip_into(
			OperationKind::AdjacencyBatch,
			&adjacency_request,
			&mut processed,
		)
		.unwrap();
	assert_eq!(u32::from_le_bytes(processed), 64);

	let degree_seven = (2..=6)
		.map(|right| AdjacencyMutation {
			left: WireHandle {
				slot: 0,
				generation: 1,
			},
			right: WireHandle {
				slot: right,
				generation: 1,
			},
			conductivity: ScalarValue(0.75),
		})
		.collect::<Vec<_>>();
	let mut invalid_adjacency_request = Vec::new();
	encode_adjacency_batch(&degree_seven, &mut invalid_adjacency_request).unwrap();
	assert!(matches!(
		client.round_trip_into(
			OperationKind::AdjacencyBatch,
			&invalid_adjacency_request,
			&mut processed,
		),
		Err(ClientError::Server(ServiceErrorCode::InvalidGraph))
	));

	client
		.round_trip_into(
			OperationKind::FrontierBegin,
			&FrontierBeginRequest {
				epoch: 1,
				expected_count: 0,
			}
			.encode(),
			&mut [0_u8; 8],
		)
		.unwrap();
	client
		.round_trip_into(
			OperationKind::FrontierCommit,
			&FrontierCommitRequest { epoch: 1 }.encode(),
			&mut [0_u8; 16],
		)
		.unwrap();
	let stage = SimulationStageRequest {
		stage: SimulationStage::ProcessTurfs,
		frontier_epoch: 1,
		stage_epoch: 1,
		work_limit: 4096,
		seconds_per_tick: ScalarValue(0.5),
	}
	.encode()
	.unwrap();
	let mut stage_result = [0_u8; SIMULATION_STAGE_RESPONSE_LEN];
	assert!(matches!(
		client.round_trip_into_with_deadline(
			OperationKind::SimulationStage,
			&stage,
			&mut stage_result,
			1,
		),
		Err(ClientError::Server(ServiceErrorCode::DeadlineExceeded))
	));
	client
		.round_trip_into(
			OperationKind::MixtureSnapshot,
			&snapshot_request.encode(),
			&mut snapshot_bytes,
		)
		.unwrap();
	assert_eq!(
		MixtureSnapshot::decode(&snapshot_bytes).unwrap().revision,
		1
	);
	assert_eq!(
		client
			.round_trip_into(OperationKind::SimulationStage, &stage, &mut stage_result,)
			.unwrap(),
		SIMULATION_STAGE_RESPONSE_LEN
	);

	let malformed_lifecycle = [2_u8, 0, 0, 0];
	assert!(matches!(
		client.round_trip_into(
			OperationKind::MixtureLifecycleBatch,
			&malformed_lifecycle,
			&mut processed,
		),
		Err(ClientError::Server(ServiceErrorCode::InvalidRequest))
	));
	assert_eq!(client.echo(b"still connected").unwrap(), b"still connected");
	let mut telemetry_bytes = [0_u8; SERVICE_TELEMETRY_LEN];
	assert_eq!(telemetry_bytes.len(), SERVICE_TELEMETRY_LEN);
	assert_eq!(
		client
			.round_trip_into(OperationKind::ServiceTelemetry, &[], &mut telemetry_bytes)
			.unwrap(),
		SERVICE_TELEMETRY_LEN
	);
	let telemetry = ServiceTelemetry::decode(&telemetry_bytes).unwrap();
	assert_eq!(telemetry.callback_depth, 960);
	assert_eq!(telemetry.callback_capacity, 1024);
	assert_eq!(telemetry.callback_high_water, 1024);
	assert_eq!(telemetry.callback_enqueued, 1024);
	assert_eq!(telemetry.callback_drained, 64);
	assert_eq!(telemetry.callback_rejected, 1);
	assert_eq!(telemetry.callback_enqueued_by_kind[0], 1024);
	assert_eq!(telemetry.callback_drained_by_kind[0], 64);
	assert_eq!(telemetry.callback_rejected_by_kind[0], 1);
	assert_eq!(telemetry.request_timeouts, 1);
	assert_eq!(telemetry.protocol_errors, 1);

	assert!(matches!(
		DogmosClient::connect(&endpoint, expected, Duration::from_secs(1)),
		Err(ClientError::ServerBusy)
	));

	client.shutdown().unwrap();
	let status = service.0.wait().unwrap();
	assert!(status.success());
}

#[test]
fn service_exits_when_the_authenticated_client_disconnects() {
	let unique = SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.unwrap()
		.as_nanos();
	let endpoint = format!("dogmos-disconnect-{pid}-{unique}", pid = std::process::id());
	let service_path = std::path::Path::new(env!("CARGO_BIN_EXE_dogmosd"));
	let expected = handshake(service_path);
	let mut service = ChildGuard(
		Command::new(service_path)
			.arg("--echo-server")
			.arg(&endpoint)
			.stdin(Stdio::piped())
			.stdout(Stdio::null())
			.stderr(Stdio::null())
			.spawn()
			.unwrap(),
	);
	service
		.0
		.stdin
		.take()
		.unwrap()
		.write_all(&expected.encode())
		.unwrap();
	let mut stream = connect_raw(&endpoint, Duration::from_secs(5));
	let (response, _) = raw_round_trip(
		&mut stream,
		expected,
		OperationKind::Handshake,
		1,
		&expected.encode(),
	);
	assert_eq!(response.flags & FLAG_ERROR, 0);
	drop(stream);

	let deadline = Instant::now() + Duration::from_secs(5);
	let status = loop {
		if let Some(status) = service.0.try_wait().unwrap() {
			break status;
		}
		assert!(
			Instant::now() < deadline,
			"dogmosd survived its authenticated client"
		);
		std::thread::sleep(Duration::from_millis(5));
	};
	assert!(!status.success());
}

#[test]
fn service_rejects_a_parent_supplied_digest_that_does_not_match_its_executable() {
	let service_path = std::path::Path::new(env!("CARGO_BIN_EXE_dogmosd"));
	let mut expected = handshake(service_path);
	expected.identity.executable_digest[0] ^= 1;
	let endpoint = format!("dogmos-wrong-digest-{}", std::process::id());
	let mut service = Command::new(service_path)
		.arg("--echo-server")
		.arg(endpoint)
		.stdin(Stdio::piped())
		.stdout(Stdio::null())
		.stderr(Stdio::null())
		.spawn()
		.unwrap();
	service
		.stdin
		.take()
		.unwrap()
		.write_all(&expected.encode())
		.unwrap();
	assert!(!service.wait().unwrap().success());
}
