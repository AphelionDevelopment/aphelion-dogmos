use dogmos_byond::{
	decode_production_callback_batch, decode_production_continuation_token,
	decode_production_mixture_snapshot, decode_production_simulation_stage,
	encode_production_continuation_adjust_multiple, encode_production_continuation_resume,
	encode_production_gas_metadata, encode_production_mixture_adjust_multiple,
	encode_production_mixture_command, encode_production_mixture_lifecycle_batch,
	encode_production_mixture_state_batch, encode_production_reaction_metadata,
	encode_production_simulation_stage, encode_production_turf_adjacency_batch,
	encode_production_turf_lifecycle_batch, ClientError, DogmosClient,
};
use dogmos_protocol::{
	encode_lifecycle_batch, BuildIdentity, CallbackBatchRequest, CapacityLimits, HandshakePayload,
	LifecycleAction, LifecycleMutation, MixtureCommandRequest, MixtureCommandResponse,
	MixtureSnapshot, MixtureSnapshotRequest, OperationKind, ScalarValue, ServiceTelemetry,
	SimulationStageResponse, WireHandle, CALLBACK_BATCH_HEADER_LEN, CALLBACK_EVENT_LEN,
	DOGMOS_ABI_VERSION, DOGMOS_PROTOCOL_VERSION, MAX_CONTROL_PAYLOAD, MAX_GAS_SLOTS,
	MIXTURE_COMMAND_RESPONSE_LEN, MIXTURE_SNAPSHOT_LEN, SERVICE_TELEMETRY_LEN,
	SIMULATION_STAGE_RESPONSE_LEN,
};
use std::{
	error::Error,
	io::Write,
	process::{Child, Command, Stdio},
	time::{Duration, SystemTime, UNIX_EPOCH},
};

const LEGACY_MIXTURE_TRANSCRIPT: &str =
	include_str!("../../dogmos-core/tests/fixtures/legacy_mixture_transcript_v1.txt");
const LEGACY_TRANSCRIPT_HEADER: &str = "DOGMOS_LEGACY_MIXTURE_TRANSCRIPT_V1";
const LEGACY_TRANSCRIPT_HANDLE_BASE: u32 = 100;

struct ChildGuard(Child);

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

impl Drop for ChildGuard {
	fn drop(&mut self) {
		let _ = self.0.kill();
		let _ = self.0.wait();
	}
}

fn main() -> Result<(), Box<dyn Error>> {
	let mut arguments = std::env::args().skip(1);
	let service_path = arguments
		.next()
		.ok_or("usage: cross_bitness_probe <dogmosd-path>")?;
	let diagnostic_bytes = arguments
		.next()
		.map(|value| value.parse::<u64>())
		.transpose()?
		.unwrap_or(0);
	let hold_milliseconds = arguments
		.next()
		.map(|value| value.parse::<u64>())
		.transpose()?
		.unwrap_or(0);
	let pressure_cycles = arguments
		.next()
		.map(|value| value.parse::<u32>())
		.transpose()?
		.unwrap_or(0);
	let pressure_hold_milliseconds = arguments
		.next()
		.map(|value| value.parse::<u64>())
		.transpose()?
		.unwrap_or(0);
	if arguments.next().is_some() {
		return Err("cross-bitness probe received too many arguments".into());
	}
	let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
	let endpoint = format!(
		"dogmos-cross-bitness-{pid}-{unique}",
		pid = std::process::id()
	);
	let service_digest = dogmos_identity::sha256_file(std::path::Path::new(&service_path))?;
	let handshake = test_handshake(service_digest)?;
	let mut service = ChildGuard(
		Command::new(service_path)
			.arg("--echo-server")
			.arg(&endpoint)
			.stdin(Stdio::piped())
			.stdout(Stdio::null())
			.stderr(Stdio::inherit())
			.spawn()?,
	);
	service
		.0
		.stdin
		.take()
		.ok_or("dogmosd stdin was not piped")?
		.write_all(&handshake.encode())?;

	let mut client = DogmosClient::connect(&endpoint, handshake, Duration::from_secs(5))?;
	let service_pid = client.peer().process_id;
	if diagnostic_bytes != 0 {
		println!(
			"isolation_baseline,shim_pid={},service_pid={service_pid}",
			std::process::id()
		);
		std::io::stdout().flush()?;
		std::thread::sleep(Duration::from_millis(hold_milliseconds));
		let allocated = client.allocate_diagnostic(diagnostic_bytes)?;
		println!(
			"isolation_allocated,shim_pid={},service_pid={service_pid},bytes={allocated}",
			std::process::id()
		);
		std::io::stdout().flush()?;
		std::thread::sleep(Duration::from_millis(hold_milliseconds));
		client.allocate_diagnostic(0)?;
	}
	if pressure_cycles != 0 {
		run_callback_pressure(
			&mut client,
			service_pid,
			pressure_cycles,
			pressure_hold_milliseconds,
		)?;
	}
	if client.echo(b"i686-to-x64")? != b"i686-to-x64" {
		return Err("cross-bitness echo payload changed".into());
	}
	let mut gas_metadata_fields = Vec::new();
	gas_metadata_fields.extend([
		0.0, 0.0, 0.0, 20.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
	]);
	gas_metadata_fields.extend([
		1.0, 0.0, 0.0, 20.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
	]);
	let mut metadata_request = encode_production_gas_metadata(
		&gas_metadata_fields,
		&["o2".to_owned(), "n2".to_owned()],
		&["Oxygen".to_owned(), "Nitrogen".to_owned()],
		&[],
	)?;
	let mut processed = [0_u8; 4];
	client.round_trip_into(
		OperationKind::GasMetadataInstall,
		&metadata_request,
		&mut processed,
	)?;
	if u32::from_le_bytes(processed) != 2 {
		return Err("cross-bitness gas metadata install count changed".into());
	}
	metadata_request = encode_production_reaction_metadata(
		&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
		&["dm_probe".to_owned()],
		&[0.0, 0.0, 1.0],
	)?;
	client.round_trip_into(
		OperationKind::ReactionMetadataInstall,
		&metadata_request,
		&mut processed,
	)?;
	verify_legacy_mixture_transcript(&mut client)?;
	let first_mixture = WireHandle {
		slot: 0,
		generation: 1,
	};
	let second_mixture = WireHandle {
		slot: 1,
		generation: 1,
	};
	let lifecycle_request =
		encode_production_mixture_lifecycle_batch(&[1.0, 0.0, 1.0, 1.0, 1.0, 1.0])?;
	client.round_trip_into(
		OperationKind::MixtureLifecycleBatch,
		&lifecycle_request,
		&mut processed,
	)?;
	let mut gases = [ScalarValue(0.0); MAX_GAS_SLOTS];
	gases[0] = ScalarValue(21.0);
	let mut state_fields = vec![0.0, 1.0, 0.0, 0.0, 293.15, 2500.0];
	state_fields.extend(gases.into_iter().map(|gas| gas.0 as f32));
	state_fields.extend([1.0, 1.0, 0.0, 0.0, 293.15, 2500.0]);
	state_fields.extend([0.0; MAX_GAS_SLOTS]);
	let state_request = encode_production_mixture_state_batch(&state_fields)?;
	client.round_trip_into(
		OperationKind::MixtureStateBatch,
		&state_request,
		&mut processed,
	)?;
	let mut snapshot = [0_u8; MIXTURE_SNAPSHOT_LEN];
	client.round_trip_into(
		OperationKind::MixtureSnapshot,
		&MixtureSnapshotRequest {
			handle: first_mixture,
		}
		.encode(),
		&mut snapshot,
	)?;
	let production_snapshot = decode_production_mixture_snapshot(&snapshot)?;
	if production_snapshot[0] != 1.0
		|| production_snapshot[1] != 0.0
		|| production_snapshot[7] != 21.0
	{
		return Err("cross-bitness production snapshot ABI changed".into());
	}
	let initial_snapshot = MixtureSnapshot::decode(&snapshot)?;
	if initial_snapshot.revision != 1 || initial_snapshot.gases[0] != ScalarValue(21.0) {
		return Err("cross-bitness mixture state changed".into());
	}

	let turf_lifecycle_request = encode_production_turf_lifecycle_batch(&[
		1.0, 10.0, 1.0, 1.0, 0.0, 1.0, 1.0, 11.0, 1.0, 1.0, 1.0, 1.0,
	])?;
	client.round_trip_into(
		OperationKind::TurfLifecycleBatch,
		&turf_lifecycle_request,
		&mut processed,
	)?;
	let turf_adjacency_request =
		encode_production_turf_adjacency_batch(&[10.0, 1.0, 11.0, 1.0, 1.0, 0.0])?;
	client.round_trip_into(
		OperationKind::TurfAdjacencyBatch,
		&turf_adjacency_request,
		&mut processed,
	)?;
	let stage_request = encode_production_simulation_stage([4.0, 0.5])?;
	let mut stage_response = [0_u8; SIMULATION_STAGE_RESPONSE_LEN];
	client.round_trip_into(
		OperationKind::SimulationStage,
		&stage_request,
		&mut stage_response,
	)?;
	let diffusion_stage_response = SimulationStageResponse::decode(&stage_response)?;
	if decode_production_simulation_stage(&stage_response)? != [2.0, 0.0, 0.0, 0.0] {
		return Err("cross-bitness production stage response ABI changed".into());
	}
	if diffusion_stage_response.work_items != 2 || diffusion_stage_response.callback_events != 0 {
		return Err("cross-bitness turf stage result changed".into());
	}
	client.round_trip_into(
		OperationKind::MixtureSnapshot,
		&MixtureSnapshotRequest {
			handle: first_mixture,
		}
		.encode(),
		&mut snapshot,
	)?;
	let first_after = MixtureSnapshot::decode(&snapshot)?;
	client.round_trip_into(
		OperationKind::MixtureSnapshot,
		&MixtureSnapshotRequest {
			handle: second_mixture,
		}
		.encode(),
		&mut snapshot,
	)?;
	let second_after = MixtureSnapshot::decode(&snapshot)?;
	if first_after.revision != 2
		|| second_after.revision != 2
		|| first_after.gases[0] != ScalarValue(18.375)
		|| second_after.gases[0] != ScalarValue(2.625)
	{
		return Err("cross-bitness turf diffusion transcript changed".into());
	}
	let mut command_response = [0_u8; MIXTURE_COMMAND_RESPONSE_LEN];
	client.round_trip_into(
		OperationKind::MixtureCommand,
		&encode_production_mixture_command([
			4.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
		])?,
		&mut command_response,
	)?;
	let MixtureCommandResponse::Scalar(ScalarValue(moles)) =
		MixtureCommandResponse::decode(&command_response)?
	else {
		return Err("cross-bitness mixture query returned the wrong result kind".into());
	};
	if (moles - 18.375).abs() > 0.0001 {
		return Err("cross-bitness mixture query changed".into());
	}
	client.round_trip_into(
		OperationKind::MixtureCommand,
		&MixtureCommandRequest::SetVolume {
			handle: first_mixture,
			volume: ScalarValue(1000.0),
		}
		.encode()?,
		&mut command_response,
	)?;
	if MixtureCommandResponse::decode(&command_response)?
		!= (MixtureCommandResponse::Applied { updated: 1 })
	{
		return Err("cross-bitness mixture mutation changed".into());
	}
	let mut adjust_multiple_request =
		encode_production_mixture_adjust_multiple(&[0.0, 1.0, 0.0, 1.0, 0.0, -0.5])?;
	client.round_trip_into(
		OperationKind::MixtureAdjustMultiple,
		&adjust_multiple_request,
		&mut command_response,
	)?;
	if MixtureCommandResponse::decode(&command_response)?
		!= (MixtureCommandResponse::Applied { updated: 1 })
	{
		return Err("cross-bitness adjust-multiple mutation changed".into());
	}
	client.round_trip_into(
		OperationKind::MixtureCommand,
		&MixtureCommandRequest::GetMoles {
			handle: first_mixture,
			gas_id: 0,
		}
		.encode()?,
		&mut command_response,
	)?;
	if MixtureCommandResponse::decode(&command_response)?
		!= MixtureCommandResponse::Scalar(ScalarValue(18.875))
	{
		return Err("cross-bitness adjust-multiple transcript changed".into());
	}
	let reaction_stage_request = encode_production_simulation_stage([5.0, 0.5])?;
	client.round_trip_into(
		OperationKind::SimulationStage,
		&reaction_stage_request,
		&mut stage_response,
	)?;
	if SimulationStageResponse::decode(&stage_response)?.callback_events != 1 {
		return Err("cross-bitness DM reaction did not issue one continuation".into());
	}
	let mut callback_response = [0_u8; CALLBACK_BATCH_HEADER_LEN + CALLBACK_EVENT_LEN];
	client.round_trip_into(
		OperationKind::CallbackBatch,
		&CallbackBatchRequest { max_events: 1 }.encode(),
		&mut callback_response,
	)?;
	let callback_fields = decode_production_callback_batch(&callback_response, 1)?;
	let continuation_fields = &callback_fields[33..43];
	let mut continuation_adjust_fields = continuation_fields.to_vec();
	continuation_adjust_fields.extend([0.0, 1.0, 0.0, -0.125]);
	adjust_multiple_request =
		encode_production_continuation_adjust_multiple(&continuation_adjust_fields)?;
	client.round_trip_into(
		OperationKind::ContinuationAdjustMultiple,
		&adjust_multiple_request,
		&mut command_response,
	)?;
	client.round_trip_into(
		OperationKind::ContinuationResume,
		&encode_production_continuation_resume(
			&continuation_fields
				.iter()
				.copied()
				.chain(std::iter::once(1.0))
				.collect::<Vec<_>>(),
		)?,
		&mut command_response,
	)?;

	client.round_trip_into(
		OperationKind::SimulationStage,
		&reaction_stage_request,
		&mut stage_response,
	)?;
	client.round_trip_into(
		OperationKind::CallbackBatch,
		&CallbackBatchRequest { max_events: 1 }.encode(),
		&mut callback_response,
	)?;
	let cancelled_fields = decode_production_callback_batch(&callback_response, 1)?;
	let cancelled = decode_production_continuation_token(&cancelled_fields[33..43])?;
	assert_eq!(
		client.round_trip_into(
			OperationKind::ContinuationCancel,
			&cancelled.encode()?,
			&mut command_response,
		)?,
		0
	);
	if !matches!(
		DogmosClient::connect(&endpoint, handshake, Duration::from_secs(1)),
		Err(ClientError::ServerBusy)
	) {
		return Err("dogmosd accepted a second concurrent client".into());
	}
	client.shutdown()?;
	if !service.0.wait()?.success() {
		return Err("dogmosd did not shut down cleanly".into());
	}
	println!(
		"cross-bitness IPC passed: shim_pid={} service_pid={service_pid}",
		std::process::id()
	);
	Ok(())
}

fn legacy_transcript_handle(slot: u32) -> WireHandle {
	WireHandle {
		slot: LEGACY_TRANSCRIPT_HANDLE_BASE + slot,
		generation: 1,
	}
}

fn parse_legacy_mixture_transcript() -> Result<Vec<CapturedStep>, Box<dyn Error>> {
	let mut lines = LEGACY_MIXTURE_TRANSCRIPT.lines();
	if lines.next() != Some(LEGACY_TRANSCRIPT_HEADER) {
		return Err("legacy mixture transcript header changed".into());
	}
	lines
		.map(|line| {
			let fields = line.split('|').collect::<Vec<_>>();
			if fields.len() != 15 {
				return Err(format!("malformed legacy mixture transcript row: {line}").into());
			}
			let mut values = [0.0_f64; 12];
			for (index, value) in fields[3..].iter().enumerate() {
				values[index] = value.parse()?;
			}
			Ok(CapturedStep {
				name: fields[0].to_owned(),
				result_kind: fields[1].to_owned(),
				result_value: fields[2].parse()?,
				mixtures: [
					values[0..4].try_into()?,
					values[4..8].try_into()?,
					values[8..12].try_into()?,
				],
			})
		})
		.collect()
}

fn legacy_mixture_command(
	client: &mut DogmosClient,
	command: MixtureCommandRequest,
) -> Result<MixtureCommandResponse, Box<dyn Error>> {
	let mut response = [0_u8; MIXTURE_COMMAND_RESPONSE_LEN];
	let response_len = client.round_trip_into(
		OperationKind::MixtureCommand,
		&command.encode()?,
		&mut response,
	)?;
	if response_len != MIXTURE_COMMAND_RESPONSE_LEN {
		return Err("legacy mixture command response length changed".into());
	}
	Ok(MixtureCommandResponse::decode(&response)?)
}

fn legacy_mixture_snapshot(
	client: &mut DogmosClient,
	handle: WireHandle,
) -> Result<MixtureSnapshot, Box<dyn Error>> {
	let mut response = [0_u8; MIXTURE_SNAPSHOT_LEN];
	let response_len = client.round_trip_into(
		OperationKind::MixtureSnapshot,
		&MixtureSnapshotRequest { handle }.encode(),
		&mut response,
	)?;
	if response_len != MIXTURE_SNAPSHOT_LEN {
		return Err("legacy mixture snapshot response length changed".into());
	}
	Ok(MixtureSnapshot::decode(&response)?)
}

fn require_applied(response: MixtureCommandResponse) -> Result<(), Box<dyn Error>> {
	if !matches!(response, MixtureCommandResponse::Applied { .. }) {
		return Err("legacy mixture mutation returned the wrong result kind".into());
	}
	Ok(())
}

fn verify_close(actual: f64, expected: f64, step: &str, field: &str) -> Result<(), Box<dyn Error>> {
	let tolerance = 0.000_1_f64.max(expected.abs() * 0.000_01);
	if (actual - expected).abs() > tolerance {
		return Err(format!(
			"legacy transcript {step} {field}: expected {expected}, got {actual}, tolerance {tolerance}"
		)
		.into());
	}
	Ok(())
}

fn verify_legacy_result(step: &CapturedStep, actual: LegacyResult) -> Result<(), Box<dyn Error>> {
	let (kind, value) = match actual {
		LegacyResult::Null => ("null", 0.0),
		LegacyResult::Number(value) => ("number", value),
		LegacyResult::Mixture => ("mixture", 1.0),
	};
	if kind != step.result_kind {
		return Err(format!(
			"legacy transcript {} result kind: expected {}, got {kind}",
			step.name, step.result_kind
		)
		.into());
	}
	verify_close(value, step.result_value, &step.name, "result")
}

fn verify_legacy_state(
	client: &mut DogmosClient,
	step: &CapturedStep,
) -> Result<(), Box<dyn Error>> {
	for (slot, expected) in step.mixtures.iter().enumerate() {
		let actual = legacy_mixture_snapshot(client, legacy_transcript_handle(slot as u32))?;
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
			verify_close(actual, expected, &step.name, field)?;
		}
	}
	Ok(())
}

fn verify_legacy_mixture_transcript(client: &mut DogmosClient) -> Result<(), Box<dyn Error>> {
	let captured = parse_legacy_mixture_transcript()?;
	if captured.len() != 12 {
		return Err("legacy mixture transcript step count changed".into());
	}

	let mut request = Vec::new();
	encode_lifecycle_batch(
		&(0..4)
			.map(|slot| LifecycleMutation {
				action: LifecycleAction::Register,
				handle: legacy_transcript_handle(slot),
			})
			.collect::<Vec<_>>(),
		&mut request,
	)?;
	let mut processed = [0_u8; 4];
	client.round_trip_into(
		OperationKind::MixtureLifecycleBatch,
		&request,
		&mut processed,
	)?;
	if u32::from_le_bytes(processed) != 4 {
		return Err("legacy mixture transcript registration count changed".into());
	}

	for step in &captured {
		let result = match step.name.as_str() {
			"set_o2" => {
				require_applied(legacy_mixture_command(
					client,
					MixtureCommandRequest::SetMoles {
						handle: legacy_transcript_handle(0),
						gas_id: 0,
						amount: ScalarValue(100.0),
					},
				)?)?;
				LegacyResult::Null
			}
			"set_n2" => {
				require_applied(legacy_mixture_command(
					client,
					MixtureCommandRequest::SetMoles {
						handle: legacy_transcript_handle(0),
						gas_id: 1,
						amount: ScalarValue(50.0),
					},
				)?)?;
				LegacyResult::Null
			}
			"adjust_o2" => {
				require_applied(legacy_mixture_command(
					client,
					MixtureCommandRequest::AdjustMoles {
						handle: legacy_transcript_handle(0),
						gas_id: 0,
						delta: ScalarValue(-25.0),
					},
				)?)?;
				LegacyResult::Null
			}
			"set_temperature" => {
				require_applied(legacy_mixture_command(
					client,
					MixtureCommandRequest::SetTemperature {
						handle: legacy_transcript_handle(0),
						temperature: ScalarValue(400.0),
					},
				)?)?;
				LegacyResult::Null
			}
			"set_volume" => {
				require_applied(legacy_mixture_command(
					client,
					MixtureCommandRequest::SetVolume {
						handle: legacy_transcript_handle(0),
						volume: ScalarValue(2000.0),
					},
				)?)?;
				LegacyResult::Null
			}
			"seed_b" => {
				require_applied(legacy_mixture_command(
					client,
					MixtureCommandRequest::SetMoles {
						handle: legacy_transcript_handle(1),
						gas_id: 0,
						amount: ScalarValue(20.0),
					},
				)?)?;
				LegacyResult::Null
			}
			"temperature_b" => {
				require_applied(legacy_mixture_command(
					client,
					MixtureCommandRequest::SetTemperature {
						handle: legacy_transcript_handle(1),
						temperature: ScalarValue(300.0),
					},
				)?)?;
				LegacyResult::Null
			}
			"merge" => {
				let response = legacy_mixture_command(
					client,
					MixtureCommandRequest::Merge {
						receiver: legacy_transcript_handle(0),
						giver: legacy_transcript_handle(1),
					},
				)?;
				if response != (MixtureCommandResponse::Applied { updated: 1 }) {
					return Err("legacy mixture merge result changed".into());
				}
				LegacyResult::Number(1.0)
			}
			"remove_ratio" => {
				let source_volume =
					legacy_mixture_snapshot(client, legacy_transcript_handle(0))?.volume;
				require_applied(legacy_mixture_command(
					client,
					MixtureCommandRequest::SetVolume {
						handle: legacy_transcript_handle(2),
						volume: source_volume,
					},
				)?)?;
				require_applied(legacy_mixture_command(
					client,
					MixtureCommandRequest::RemoveRatioInto {
						source: legacy_transcript_handle(0),
						destination: legacy_transcript_handle(2),
						ratio: ScalarValue(0.25),
					},
				)?)?;
				LegacyResult::Mixture
			}
			"transfer_amount" => {
				require_applied(legacy_mixture_command(
					client,
					MixtureCommandRequest::TransferAmount {
						source: legacy_transcript_handle(0),
						destination: legacy_transcript_handle(1),
						amount: ScalarValue(10.0),
					},
				)?)?;
				LegacyResult::Null
			}
			"equalize" => {
				let total_volume =
					legacy_mixture_snapshot(client, legacy_transcript_handle(0))?
						.volume
						.0 + legacy_mixture_snapshot(client, legacy_transcript_handle(1))?
						.volume
						.0;
				require_applied(legacy_mixture_command(
					client,
					MixtureCommandRequest::Clear {
						handle: legacy_transcript_handle(3),
					},
				)?)?;
				require_applied(legacy_mixture_command(
					client,
					MixtureCommandRequest::SetVolume {
						handle: legacy_transcript_handle(3),
						volume: ScalarValue(total_volume),
					},
				)?)?;
				for giver in [legacy_transcript_handle(0), legacy_transcript_handle(1)] {
					require_applied(legacy_mixture_command(
						client,
						MixtureCommandRequest::Merge {
							receiver: legacy_transcript_handle(3),
							giver,
						},
					)?)?;
				}
				for receiver in [legacy_transcript_handle(0), legacy_transcript_handle(1)] {
					require_applied(legacy_mixture_command(
						client,
						MixtureCommandRequest::EqualizeWith {
							receiver,
							total: legacy_transcript_handle(3),
						},
					)?)?;
				}
				LegacyResult::Null
			}
			"immutable_write" => {
				require_applied(legacy_mixture_command(
					client,
					MixtureCommandRequest::MarkImmutable {
						handle: legacy_transcript_handle(2),
					},
				)?)?;
				let response = legacy_mixture_command(
					client,
					MixtureCommandRequest::SetMoles {
						handle: legacy_transcript_handle(2),
						gas_id: 0,
						amount: ScalarValue(999.0),
					},
				)?;
				if response != (MixtureCommandResponse::Applied { updated: 0 }) {
					return Err("legacy immutable mixture write result changed".into());
				}
				LegacyResult::Null
			}
			other => return Err(format!("unknown captured legacy transcript step {other}").into()),
		};
		verify_legacy_result(step, result)?;
		verify_legacy_state(client, step)?;
	}

	encode_lifecycle_batch(
		&(0..4)
			.map(|slot| LifecycleMutation {
				action: LifecycleAction::Unregister,
				handle: legacy_transcript_handle(slot),
			})
			.collect::<Vec<_>>(),
		&mut request,
	)?;
	client.round_trip_into(
		OperationKind::MixtureLifecycleBatch,
		&request,
		&mut processed,
	)?;
	if u32::from_le_bytes(processed) != 4 {
		return Err("legacy mixture transcript cleanup count changed".into());
	}
	Ok(())
}

fn run_callback_pressure(
	client: &mut DogmosClient,
	service_pid: u32,
	cycles: u32,
	hold_milliseconds: u64,
) -> Result<(), Box<dyn Error>> {
	if cycles < 100 {
		return Err("callback pressure requires at least 100 cycles".into());
	}
	let enqueue_request = CallbackBatchRequest { max_events: 1024 }.encode();
	let drain_request = CallbackBatchRequest { max_events: 1024 }.encode();
	let mut accepted = [0_u8; 4];
	let mut drained = vec![0_u8; CALLBACK_BATCH_HEADER_LEN + 1024 * CALLBACK_EVENT_LEN];
	let mut telemetry_bytes = [0_u8; SERVICE_TELEMETRY_LEN];
	let checkpoints = [
		((cycles / 10).max(1), "warmup"),
		(cycles / 4, "quarter"),
		(cycles / 2, "midpoint"),
		(cycles.saturating_mul(3) / 4, "three_quarter"),
		(cycles, "complete"),
	];
	for cycle in 1..=cycles {
		client.round_trip_into(
			OperationKind::DiagnosticCallbackEnqueue,
			&enqueue_request,
			&mut accepted,
		)?;
		if u32::from_le_bytes(accepted) != 1024 {
			return Err("callback pressure enqueue count changed".into());
		}
		let response_len =
			client.round_trip_into(OperationKind::CallbackBatch, &drain_request, &mut drained)?;
		if response_len != drained.len() {
			return Err("callback pressure drain length changed".into());
		}
		if let Some((_, phase)) = checkpoints
			.iter()
			.find(|(checkpoint, _)| *checkpoint == cycle)
		{
			client.round_trip_into(OperationKind::ServiceTelemetry, &[], &mut telemetry_bytes)?;
			let telemetry = ServiceTelemetry::decode(&telemetry_bytes)?;
			let expected_callbacks = u64::from(cycle) * 1024;
			if telemetry.callback_depth != 0
				|| telemetry.callback_high_water != 1024
				|| telemetry.callback_enqueued != expected_callbacks
				|| telemetry.callback_drained != expected_callbacks
				|| telemetry.callback_rejected != 0
			{
				return Err(
					format!("callback pressure telemetry diverged at cycle {cycle}").into(),
				);
			}
			println!(
				"callback_pressure_{phase},shim_pid={},service_pid={service_pid},cycles={cycle},callback_depth={},callback_high_water={},callback_enqueued={},callback_drained={}",
				std::process::id(),
				telemetry.callback_depth,
				telemetry.callback_high_water,
				telemetry.callback_enqueued,
				telemetry.callback_drained,
			);
			std::io::stdout().flush()?;
			std::thread::sleep(Duration::from_millis(hold_milliseconds));
		}
	}
	Ok(())
}

fn test_handshake(service_digest: [u8; 32]) -> Result<HandshakePayload, Box<dyn Error>> {
	let build_metadata = dogmos_identity::BuildMetadata::from_compile_environment()?;
	Ok(HandshakePayload {
		auth_token: [0x6b; 32],
		identity: BuildIdentity {
			abi_version: DOGMOS_ABI_VERSION,
			protocol_version: DOGMOS_PROTOCOL_VERSION,
			source_revision: build_metadata.source_revision,
			feature_fingerprint: build_metadata.feature_fingerprint,
			executable_digest: service_digest,
		},
		capacities: CapacityLimits {
			max_control_payload: MAX_CONTROL_PAYLOAD,
			max_batch_operations: 4096,
			max_callback_events: 1024,
			max_pending_continuations: 1024,
			max_world_bytes: 8 * 1024 * 1024 * 1024,
		},
		process_id: std::process::id(),
		world_generation: 1,
		world_nonce: 0x2233_4455_6677_8899,
	})
}
