#![cfg(all(windows, target_arch = "x86"))]

mod common;

use dogmos_byond::ClientError;
use dogmos_protocol::{
	encode_gas_metadata_batch, encode_lifecycle_batch, encode_reaction_metadata_batch,
	CallbackBatchRequest, CallbackEvent, CallbackEventKind, CallbackScope, GasMetadataRegistration,
	LifecycleAction, LifecycleMutation, MixtureCommandRequest, MixtureCommandResponse,
	OperationKind, ReactionMetadataRegistration, ScalarValue, ServiceErrorCode, WireGasFireRole,
	WireHandle, WireReactionExecution, CALLBACK_BATCH_HEADER_LEN, CALLBACK_EVENT_LEN,
	MIXTURE_COMMAND_RESPONSE_LEN,
};

fn handle(slot: u32) -> WireHandle {
	WireHandle {
		slot,
		generation: 1,
	}
}

fn direct_reaction(
	client: &mut dogmos_byond::DogmosClient,
	mixture: WireHandle,
	target: WireHandle,
) -> u64 {
	let mut response = [0_u8; MIXTURE_COMMAND_RESPONSE_LEN];
	client
		.round_trip_into(
			OperationKind::MixtureCommand,
			&MixtureCommandRequest::React {
				handle: mixture,
				target,
				reaction_profile_threshold_ms: None,
			}
			.encode()
			.unwrap(),
			&mut response,
		)
		.unwrap();
	let MixtureCommandResponse::ReactionProgress {
		pending: true,
		transaction_id,
		..
	} = MixtureCommandResponse::decode(&response).unwrap()
	else {
		panic!("direct DM reaction did not return pending progress");
	};
	transaction_id
}

fn drain(
	client: &mut dogmos_byond::DogmosClient,
	scope: CallbackScope,
	transaction_id: u64,
) -> CallbackEvent {
	let mut response = [0_u8; CALLBACK_BATCH_HEADER_LEN + CALLBACK_EVENT_LEN];
	client
		.round_trip_into(
			OperationKind::CallbackBatch,
			&CallbackBatchRequest {
				max_events: 1,
				scope,
				transaction_id,
			}
			.encode()
			.unwrap(),
			&mut response,
		)
		.unwrap();
	CallbackEvent::decode(&response[CALLBACK_BATCH_HEADER_LEN..]).unwrap()
}

#[test]
fn direct_reactions_and_general_callbacks_remain_transaction_isolated() {
	let mut service = common::start(3, 2, 2);
	let mut request = Vec::new();
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
	)
	.unwrap();
	service
		.client
		.round_trip_into(OperationKind::GasMetadataInstall, &request, &mut [0_u8; 4])
		.unwrap();
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
	)
	.unwrap();
	service
		.client
		.round_trip_into(
			OperationKind::ReactionMetadataInstall,
			&request,
			&mut [0_u8; 4],
		)
		.unwrap();
	encode_lifecycle_batch(
		&[
			LifecycleMutation {
				action: LifecycleAction::Register,
				handle: handle(0),
			},
			LifecycleMutation {
				action: LifecycleAction::Register,
				handle: handle(1),
			},
		],
		&mut request,
	)
	.unwrap();
	service
		.client
		.round_trip_into(
			OperationKind::MixtureLifecycleBatch,
			&request,
			&mut [0_u8; 4],
		)
		.unwrap();

	let general_request = CallbackBatchRequest {
		max_events: 1,
		scope: CallbackScope::General,
		transaction_id: 0,
	}
	.encode()
	.unwrap();
	service
		.client
		.round_trip_into(
			OperationKind::DiagnosticCallbackEnqueue,
			&general_request,
			&mut [0_u8; 4],
		)
		.unwrap();
	let first_transaction = direct_reaction(&mut service.client, handle(0), handle(100));
	let second_transaction = direct_reaction(&mut service.client, handle(1), handle(101));
	assert_ne!(first_transaction, second_transaction);

	let mut rejected_response = [0_u8; MIXTURE_COMMAND_RESPONSE_LEN];
	assert!(matches!(
		service.client.round_trip_into(
			OperationKind::MixtureCommand,
			&MixtureCommandRequest::React {
				handle: handle(0),
				target: handle(102),
				reaction_profile_threshold_ms: None,
			}
			.encode()
			.unwrap(),
			&mut rejected_response
		),
		Err(ClientError::Server(ServiceErrorCode::CallbackBackpressure))
	));

	let general = drain(&mut service.client, CallbackScope::General, 0);
	assert_eq!(general.kind, CallbackEventKind::Diagnostic);
	assert_eq!(general.scope_sequence, 1);
	assert_eq!(general.transaction_id, 0);

	let second = drain(
		&mut service.client,
		CallbackScope::Reaction,
		second_transaction,
	);
	assert_eq!(second.subject, handle(1));
	assert_eq!(second.scope_sequence, 1);
	assert_eq!(second.transaction_id, second_transaction);

	let first = drain(
		&mut service.client,
		CallbackScope::Reaction,
		first_transaction,
	);
	assert_eq!(first.subject, handle(0));
	assert_eq!(first.scope_sequence, 1);
	assert_eq!(first.transaction_id, first_transaction);

	service
		.client
		.round_trip_into(
			OperationKind::ContinuationCancel,
			&first.continuation.unwrap().encode().unwrap(),
			&mut [],
		)
		.unwrap();
	service
		.client
		.round_trip_into(
			OperationKind::ContinuationCancel,
			&second.continuation.unwrap().encode().unwrap(),
			&mut [],
		)
		.unwrap();
}
