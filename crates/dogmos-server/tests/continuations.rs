#![cfg(all(windows, target_arch = "x86"))]

use dogmos_byond::{ClientError, DogmosClient};
use dogmos_protocol::{
	encode_gas_metadata_batch, encode_lifecycle_batch, encode_reaction_metadata_batch,
	encode_turf_lifecycle_batch, BuildIdentity, CallbackBatchRequest, CallbackEvent,
	CapacityLimits, ContinuationCommandRequest, ContinuationResumeRequest, GasMetadataRegistration,
	HandshakePayload, LifecycleAction, LifecycleMutation, MixtureCommandRequest,
	MixtureCommandResponse, OperationKind, ReactionMetadataRegistration, ScalarValue,
	ServiceErrorCode, SimulationStage, SimulationStageRequest, TurfLifecycleMutation,
	WireGasFireRole, WireHandle, WireReactionExecution, CALLBACK_BATCH_HEADER_LEN,
	CALLBACK_EVENT_LEN, DOGMOS_ABI_VERSION, DOGMOS_PROTOCOL_VERSION, MAX_CONTROL_PAYLOAD,
	MIXTURE_COMMAND_RESPONSE_LEN,
};
use std::{
	error::Error,
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

#[test]
fn dm_reaction_continuation_allows_nested_commands_then_fails_closed_on_service_death(
) -> Result<(), Box<dyn Error>> {
	let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
	let endpoint = format!("dogmos-continuation-{}-{unique}", std::process::id());
	let service_path = std::path::Path::new(env!("CARGO_BIN_EXE_dogmosd"));
	let handshake = HandshakePayload {
		auth_token: [0x7a; 32],
		identity: BuildIdentity {
			abi_version: DOGMOS_ABI_VERSION,
			protocol_version: DOGMOS_PROTOCOL_VERSION,
			source_revision: [0x11; 20],
			feature_fingerprint: [0x22; 32],
			executable_digest: dogmos_identity::sha256_file(service_path)?,
		},
		capacities: CapacityLimits {
			max_control_payload: MAX_CONTROL_PAYLOAD,
			max_batch_operations: 4096,
			max_callback_events: 8,
			max_pending_continuations: 1,
			max_world_bytes: 1024 * 1024,
		},
		process_id: std::process::id(),
		world_generation: 7,
		world_nonce: 0x1234_5678_90ab_cdef,
	};
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
	let mut request = Vec::new();
	let mut count_response = [0_u8; 4];
	encode_gas_metadata_batch(
		&[GasMetadataRegistration {
			id: 0,
			key: "o2".into(),
			name: "Oxygen".into(),
			flags: 0,
			specific_heat: ScalarValue(20.0),
			fusion_power: ScalarValue(0.0),
			moles_visible: None,
			enthalpy: ScalarValue(0.0),
			fire_radiation_released: ScalarValue(0.0),
			fire_role: WireGasFireRole::None,
			fire_products: None,
		}],
		&mut request,
	)?;
	client.round_trip_into(
		OperationKind::GasMetadataInstall,
		&request,
		&mut count_response,
	)?;
	encode_reaction_metadata_batch(
		&[ReactionMetadataRegistration {
			id: 0,
			key: "dm".into(),
			priority: ScalarValue(1.0),
			minimum_temperature: None,
			maximum_temperature: None,
			minimum_energy: None,
			minimum_fire_reagents: None,
			gas_requirements: Vec::new(),
			execution: WireReactionExecution::Dm,
		}],
		&mut request,
	)?;
	client.round_trip_into(
		OperationKind::ReactionMetadataInstall,
		&request,
		&mut count_response,
	)?;
	let mixture = WireHandle {
		slot: 0,
		generation: 1,
	};
	encode_lifecycle_batch(
		&[LifecycleMutation {
			action: LifecycleAction::Register,
			handle: mixture,
		}],
		&mut request,
	)?;
	client.round_trip_into(
		OperationKind::MixtureLifecycleBatch,
		&request,
		&mut count_response,
	)?;
	encode_turf_lifecycle_batch(
		&[TurfLifecycleMutation {
			action: LifecycleAction::Register,
			turf: WireHandle {
				slot: 0,
				generation: 1,
			},
			mixture: Some(mixture),
		}],
		&mut request,
	)?;
	client.round_trip_into(
		OperationKind::TurfLifecycleBatch,
		&request,
		&mut count_response,
	)?;

	let stage = SimulationStageRequest {
		stage: SimulationStage::ProcessReactions,
		seconds_per_tick: ScalarValue(0.5),
	}
	.encode()?;
	let mut stage_response = [0_u8; 8];
	let mut callback_response = [0_u8; CALLBACK_BATCH_HEADER_LEN + CALLBACK_EVENT_LEN];
	let mut command_response = [0_u8; MIXTURE_COMMAND_RESPONSE_LEN];
	client.round_trip_into(OperationKind::SimulationStage, &stage, &mut stage_response)?;
	client.round_trip_into(
		OperationKind::CallbackBatch,
		&CallbackBatchRequest { max_events: 1 }.encode(),
		&mut callback_response,
	)?;
	let token = CallbackEvent::decode(&callback_response[CALLBACK_BATCH_HEADER_LEN..])?
		.continuation
		.ok_or("DM reaction callback omitted its continuation")?;
	let nested = ContinuationCommandRequest {
		token,
		command: MixtureCommandRequest::SetMoles {
			handle: mixture,
			gas_id: 0,
			amount: ScalarValue(4.0),
		},
	}
	.encode()?;
	client.round_trip_into(
		OperationKind::ContinuationCommand,
		&nested,
		&mut command_response,
	)?;
	assert_eq!(
		MixtureCommandResponse::decode(&command_response)?,
		MixtureCommandResponse::Applied { updated: 1 }
	);
	let resume = ContinuationResumeRequest {
		token,
		reaction_result: 1,
	}
	.encode()?;
	client.round_trip_into(
		OperationKind::ContinuationResume,
		&resume,
		&mut command_response,
	)?;
	assert_eq!(
		MixtureCommandResponse::decode(&command_response)?,
		MixtureCommandResponse::ReactionProgress {
			flags: 1,
			work_items: 0,
			pending: false,
		}
	);
	assert!(matches!(
		client.round_trip_into(
			OperationKind::ContinuationResume,
			&resume,
			&mut command_response,
		),
		Err(ClientError::Server(ServiceErrorCode::UnknownContinuation))
	));

	client.round_trip_into(OperationKind::SimulationStage, &stage, &mut stage_response)?;
	client.round_trip_into(
		OperationKind::CallbackBatch,
		&CallbackBatchRequest { max_events: 1 }.encode(),
		&mut callback_response,
	)?;
	let cancel_token = CallbackEvent::decode(&callback_response[CALLBACK_BATCH_HEADER_LEN..])?
		.continuation
		.ok_or("DM reaction callback omitted its cancellation token")?;
	assert_eq!(
		client.round_trip_into(
			OperationKind::ContinuationCancel,
			&cancel_token.encode()?,
			&mut command_response,
		)?,
		0
	);
	assert!(matches!(
		client.round_trip_into(
			OperationKind::ContinuationCancel,
			&cancel_token.encode()?,
			&mut command_response,
		),
		Err(ClientError::Server(ServiceErrorCode::UnknownContinuation))
	));

	client.round_trip_into(OperationKind::SimulationStage, &stage, &mut stage_response)?;
	client.round_trip_into(
		OperationKind::CallbackBatch,
		&CallbackBatchRequest { max_events: 1 }.encode(),
		&mut callback_response,
	)?;
	let lost_token = CallbackEvent::decode(&callback_response[CALLBACK_BATCH_HEADER_LEN..])?
		.continuation
		.ok_or("DM reaction callback omitted its service-death token")?;
	service.0.kill()?;
	service.0.wait()?;
	let lost_resume = ContinuationResumeRequest {
		token: lost_token,
		reaction_result: 1,
	}
	.encode()?;
	assert!(matches!(
		client.round_trip_into(
			OperationKind::ContinuationResume,
			&lost_resume,
			&mut command_response,
		),
		Err(ClientError::Io(_) | ClientError::Transport(_))
	));
	Ok(())
}
