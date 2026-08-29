#![cfg(all(windows, target_arch = "x86"))]

use dogmos_byond::DogmosClient;
use dogmos_protocol::{
	encode_gas_metadata_batch, encode_lifecycle_batch, BuildIdentity, CapacityLimits,
	GasMetadataRegistration, HandshakePayload, LifecycleAction, LifecycleMutation,
	MixtureCommandRequest, MixtureCommandResponse, MixtureSnapshot, MixtureSnapshotRequest,
	OperationKind, ScalarValue, WireGasFireRole, WireHandle, DOGMOS_ABI_VERSION,
	DOGMOS_PROTOCOL_VERSION, MAX_CONTROL_PAYLOAD, MIXTURE_COMMAND_RESPONSE_LEN,
	MIXTURE_SNAPSHOT_LEN,
};
use std::{
	io::Write,
	process::{Child, Command, Stdio},
	time::{Duration, SystemTime, UNIX_EPOCH},
};

const LEGACY_TRANSCRIPT: &str =
	include_str!("../../dogmos-core/tests/fixtures/legacy_mixture_transcript_v1.txt");
const TRANSCRIPT_HEADER: &str = "DOGMOS_LEGACY_MIXTURE_TRANSCRIPT_V1";

struct ChildGuard(Child);

impl Drop for ChildGuard {
	fn drop(&mut self) {
		let _ = self.0.kill();
		let _ = self.0.wait();
	}
}

#[derive(Debug)]
struct CapturedStep {
	name: String,
	result_kind: String,
	result_value: f64,
	mixtures: [[f64; 4]; 3],
}

#[derive(Clone, Copy, Debug)]
enum LegacyResult {
	Null,
	Number(f64),
	Mixture,
}

fn handle(slot: u32) -> WireHandle {
	WireHandle {
		slot,
		generation: 1,
	}
}

fn handshake(service_path: &std::path::Path) -> HandshakePayload {
	HandshakePayload {
		auth_token: [0x74; 32],
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
		world_nonce: 0x7465_7374_2d76_3031,
	}
}

fn start_service() -> (ChildGuard, DogmosClient) {
	let unique = SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.unwrap()
		.as_nanos();
	let endpoint = format!("dogmos-world-transcript-{}-{unique}", std::process::id());
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
	let client = DogmosClient::connect(&endpoint, expected, Duration::from_secs(5)).unwrap();
	(service, client)
}

fn gas(id: u16, key: &str) -> GasMetadataRegistration {
	GasMetadataRegistration {
		id,
		key: key.into(),
		name: key.into(),
		flags: 0,
		specific_heat: ScalarValue(20.0),
		fusion_power: ScalarValue(0.0),
		moles_visible: None,
		enthalpy: ScalarValue(0.0),
		fire_radiation_released: ScalarValue(0.0),
		fire_role: WireGasFireRole::None,
		fire_products: None,
	}
}

fn parse_transcript() -> Vec<CapturedStep> {
	let mut lines = LEGACY_TRANSCRIPT.lines();
	assert_eq!(lines.next(), Some(TRANSCRIPT_HEADER));
	lines
		.map(|line| {
			let fields = line.split('|').collect::<Vec<_>>();
			assert_eq!(fields.len(), 15, "malformed transcript row: {line}");
			let mut values = [0.0_f64; 12];
			for (index, value) in fields[3..].iter().enumerate() {
				values[index] = value
					.parse::<f64>()
					.unwrap_or_else(|_| panic!("invalid transcript scalar {value} in {line}"));
			}
			CapturedStep {
				name: fields[0].to_owned(),
				result_kind: fields[1].to_owned(),
				result_value: fields[2].parse().unwrap(),
				mixtures: [
					values[0..4].try_into().unwrap(),
					values[4..8].try_into().unwrap(),
					values[8..12].try_into().unwrap(),
				],
			}
		})
		.collect()
}

fn apply_command(
	client: &mut DogmosClient,
	command: MixtureCommandRequest,
) -> MixtureCommandResponse {
	let mut response = [0_u8; MIXTURE_COMMAND_RESPONSE_LEN];
	let response_len = client
		.round_trip_into(
			OperationKind::MixtureCommand,
			&command.encode().unwrap(),
			&mut response,
		)
		.unwrap();
	assert_eq!(response_len, MIXTURE_COMMAND_RESPONSE_LEN);
	MixtureCommandResponse::decode(&response).unwrap()
}

fn snapshot(client: &mut DogmosClient, handle: WireHandle) -> MixtureSnapshot {
	let mut response = [0_u8; MIXTURE_SNAPSHOT_LEN];
	let response_len = client
		.round_trip_into(
			OperationKind::MixtureSnapshot,
			&MixtureSnapshotRequest { handle }.encode(),
			&mut response,
		)
		.unwrap();
	assert_eq!(response_len, MIXTURE_SNAPSHOT_LEN);
	MixtureSnapshot::decode(&response).unwrap()
}

fn assert_applied(response: MixtureCommandResponse) {
	assert!(matches!(response, MixtureCommandResponse::Applied { .. }));
}

fn assert_close(actual: f64, expected: f64, step: &str, field: &str) {
	let tolerance = 0.000_1_f64.max(expected.abs() * 0.000_01);
	assert!(
		(actual - expected).abs() <= tolerance,
		"{step} {field}: expected {expected}, got {actual}, tolerance {tolerance}"
	);
}

fn assert_result(step: &CapturedStep, actual: LegacyResult) {
	let (kind, value) = match actual {
		LegacyResult::Null => ("null", 0.0),
		LegacyResult::Number(value) => ("number", value),
		LegacyResult::Mixture => ("mixture", 1.0),
	};
	assert_eq!(kind, step.result_kind, "{} result kind", step.name);
	assert_close(value, step.result_value, &step.name, "result");
}

fn assert_state(client: &mut DogmosClient, step: &CapturedStep) {
	for (slot, expected) in step.mixtures.iter().enumerate() {
		let actual = snapshot(client, handle(slot as u32));
		let values = [
			actual.temperature.0,
			actual.volume.0,
			actual.gases[0].0,
			actual.gases[1].0,
		];
		for (field, (actual, expected)) in ["temperature", "volume", "o2", "n2"]
			.into_iter()
			.zip(values.into_iter().zip(expected.iter().copied()))
		{
			assert_close(actual, expected, &step.name, field);
		}
	}
}

#[test]
fn legacy_mixture_transcript_replays_through_service_dispatcher() {
	let (mut service, mut client) = start_service();
	let captured = parse_transcript();
	assert_eq!(captured.len(), 12);

	let mut request = Vec::new();
	encode_gas_metadata_batch(&[gas(0, "o2"), gas(1, "n2")], &mut request).unwrap();
	let mut processed = [0_u8; 4];
	client
		.round_trip_into(OperationKind::GasMetadataInstall, &request, &mut processed)
		.unwrap();
	assert_eq!(u32::from_le_bytes(processed), 2);

	encode_lifecycle_batch(
		&(0..4)
			.map(|slot| LifecycleMutation {
				action: LifecycleAction::Register,
				handle: handle(slot),
			})
			.collect::<Vec<_>>(),
		&mut request,
	)
	.unwrap();
	client
		.round_trip_into(
			OperationKind::MixtureLifecycleBatch,
			&request,
			&mut processed,
		)
		.unwrap();
	assert_eq!(u32::from_le_bytes(processed), 4);

	for step in &captured {
		let result = match step.name.as_str() {
			"set_o2" => {
				assert_applied(apply_command(
					&mut client,
					MixtureCommandRequest::SetMoles {
						handle: handle(0),
						gas_id: 0,
						amount: ScalarValue(100.0),
					},
				));
				LegacyResult::Null
			}
			"set_n2" => {
				assert_applied(apply_command(
					&mut client,
					MixtureCommandRequest::SetMoles {
						handle: handle(0),
						gas_id: 1,
						amount: ScalarValue(50.0),
					},
				));
				LegacyResult::Null
			}
			"adjust_o2" => {
				assert_applied(apply_command(
					&mut client,
					MixtureCommandRequest::AdjustMoles {
						handle: handle(0),
						gas_id: 0,
						delta: ScalarValue(-25.0),
					},
				));
				LegacyResult::Null
			}
			"set_temperature" => {
				assert_applied(apply_command(
					&mut client,
					MixtureCommandRequest::SetTemperature {
						handle: handle(0),
						temperature: ScalarValue(400.0),
					},
				));
				LegacyResult::Null
			}
			"set_volume" => {
				assert_applied(apply_command(
					&mut client,
					MixtureCommandRequest::SetVolume {
						handle: handle(0),
						volume: ScalarValue(2000.0),
					},
				));
				LegacyResult::Null
			}
			"seed_b" => {
				assert_applied(apply_command(
					&mut client,
					MixtureCommandRequest::SetMoles {
						handle: handle(1),
						gas_id: 0,
						amount: ScalarValue(20.0),
					},
				));
				LegacyResult::Null
			}
			"temperature_b" => {
				assert_applied(apply_command(
					&mut client,
					MixtureCommandRequest::SetTemperature {
						handle: handle(1),
						temperature: ScalarValue(300.0),
					},
				));
				LegacyResult::Null
			}
			"merge" => {
				let response = apply_command(
					&mut client,
					MixtureCommandRequest::Merge {
						receiver: handle(0),
						giver: handle(1),
					},
				);
				assert_eq!(response, MixtureCommandResponse::Applied { updated: 1 });
				LegacyResult::Number(1.0)
			}
			"remove_ratio" => {
				let source_volume = snapshot(&mut client, handle(0)).volume;
				assert_applied(apply_command(
					&mut client,
					MixtureCommandRequest::SetVolume {
						handle: handle(2),
						volume: source_volume,
					},
				));
				assert_applied(apply_command(
					&mut client,
					MixtureCommandRequest::RemoveRatioInto {
						source: handle(0),
						destination: handle(2),
						ratio: ScalarValue(0.25),
					},
				));
				LegacyResult::Mixture
			}
			"transfer_amount" => {
				assert_applied(apply_command(
					&mut client,
					MixtureCommandRequest::TransferAmount {
						source: handle(0),
						destination: handle(1),
						amount: ScalarValue(10.0),
					},
				));
				LegacyResult::Null
			}
			"equalize" => {
				let total_volume = snapshot(&mut client, handle(0)).volume.0
					+ snapshot(&mut client, handle(1)).volume.0;
				assert_applied(apply_command(
					&mut client,
					MixtureCommandRequest::Clear { handle: handle(3) },
				));
				assert_applied(apply_command(
					&mut client,
					MixtureCommandRequest::SetVolume {
						handle: handle(3),
						volume: ScalarValue(total_volume),
					},
				));
				for giver in [handle(0), handle(1)] {
					assert_applied(apply_command(
						&mut client,
						MixtureCommandRequest::Merge {
							receiver: handle(3),
							giver,
						},
					));
				}
				for receiver in [handle(0), handle(1)] {
					assert_applied(apply_command(
						&mut client,
						MixtureCommandRequest::EqualizeWith {
							receiver,
							total: handle(3),
						},
					));
				}
				LegacyResult::Null
			}
			"immutable_write" => {
				assert_applied(apply_command(
					&mut client,
					MixtureCommandRequest::MarkImmutable { handle: handle(2) },
				));
				assert_eq!(
					apply_command(
						&mut client,
						MixtureCommandRequest::SetMoles {
							handle: handle(2),
							gas_id: 0,
							amount: ScalarValue(999.0),
						},
					),
					MixtureCommandResponse::Applied { updated: 0 }
				);
				LegacyResult::Null
			}
			other => panic!("unknown captured step {other}"),
		};
		assert_result(step, result);
		assert_state(&mut client, step);
	}

	client.shutdown().unwrap();
	assert!(service.0.wait().unwrap().success());
}
