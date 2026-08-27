#![cfg(all(windows, target_arch = "x86"))]

use dogmos_byond::{ClientError, DogmosClient};
use dogmos_protocol::{
	encode_adjacency_batch, encode_lifecycle_batch, encode_mixture_state_batch, AdjacencyMutation,
	BuildIdentity, CallbackBatchHeader, CallbackBatchRequest, CallbackEvent, CapacityLimits,
	HandshakePayload, LifecycleAction, LifecycleMutation, MixtureSnapshot, MixtureSnapshotRequest,
	MixtureStateMutation, OperationKind, ScalarValue, ServiceErrorCode, SimulationStage,
	SimulationStageRequest, WireHandle, CALLBACK_BATCH_HEADER_LEN, CALLBACK_EVENT_LEN,
	DOGMOS_ABI_VERSION, DOGMOS_PROTOCOL_VERSION, MAX_CONTROL_PAYLOAD, MAX_GAS_SLOTS,
	MIXTURE_SNAPSHOT_LEN,
};
use std::{
	io::Write,
	process::{Child, Command, Stdio},
	time::{Duration, SystemTime, UNIX_EPOCH},
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
			reserved: 0,
			max_world_bytes: 8 * 1024 * 1024 * 1024,
		},
		process_id: std::process::id(),
		world_generation: 1,
		world_nonce: 0x1234_5678_90ab_cdef,
	}
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
	let enqueue_request = CallbackBatchRequest { max_events: 1024 }.encode();
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
			&CallbackBatchRequest { max_events: 1 }.encode(),
			&mut accepted,
		),
		Err(ClientError::Server(ServiceErrorCode::CallbackBackpressure))
	));
	let callback_request = CallbackBatchRequest { max_events: 64 }.encode();
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
		.sequence,
		1
	);
	assert_eq!(
		client.allocate_diagnostic(1024 * 1024).unwrap(),
		1024 * 1024
	);
	assert_eq!(client.allocate_diagnostic(0).unwrap(), 0);

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
	let mut processed = [0_u8; 4];
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

	let stage = SimulationStageRequest {
		stage: SimulationStage::ProcessTurfs,
		seconds_per_tick: ScalarValue(0.5),
	}
	.encode()
	.unwrap();
	let mut stage_result = [0_u8; 8];
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
		8
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

	assert!(matches!(
		DogmosClient::connect(&endpoint, expected, Duration::from_secs(1)),
		Err(ClientError::ServerBusy)
	));

	client.shutdown().unwrap();
	let status = service.0.wait().unwrap();
	assert!(status.success());
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
