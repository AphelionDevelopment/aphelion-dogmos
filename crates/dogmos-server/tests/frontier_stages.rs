#![cfg(all(windows, target_arch = "x86"))]

mod common;

use dogmos_byond::ClientError;
use dogmos_protocol::{
	encode_turf_adjacency_batch, encode_turf_lifecycle_batch, FrontierAppendRequest,
	FrontierBeginRequest, FrontierCommitRequest, LifecycleAction, OperationKind, ScalarValue,
	ServiceErrorCode, SimulationStage, SimulationStageRequest, SimulationStageResponse,
	TurfAdjacencyMutation, TurfLifecycleMutation, WireHandle, SIMULATION_STAGE_RESPONSE_LEN,
};

fn turf(slot: u32) -> WireHandle {
	WireHandle {
		slot,
		generation: 1,
	}
}

#[test]
fn frontier_dispatch_is_atomic_chunked_and_blocks_topology_mutation() {
	let mut service = common::start(16, 4, 4);
	let mut request = Vec::new();
	encode_turf_lifecycle_batch(
		&[
			TurfLifecycleMutation {
				action: LifecycleAction::Register,
				turf: turf(0),
				mixture: None,
			},
			TurfLifecycleMutation {
				action: LifecycleAction::Register,
				turf: turf(1),
				mixture: None,
			},
		],
		&mut request,
	)
	.unwrap();
	service
		.client
		.round_trip_into(OperationKind::TurfLifecycleBatch, &request, &mut [0_u8; 4])
		.unwrap();

	service
		.client
		.round_trip_into(
			OperationKind::FrontierBegin,
			&FrontierBeginRequest {
				epoch: 1,
				expected_count: 2,
			}
			.encode(),
			&mut [0_u8; 8],
		)
		.unwrap();
	service
		.client
		.round_trip_into(
			OperationKind::FrontierAppend,
			&FrontierAppendRequest {
				epoch: 1,
				offset: 0,
				handles: vec![turf(0), turf(1)],
			}
			.encode()
			.unwrap(),
			&mut [0_u8; 4],
		)
		.unwrap();
	service
		.client
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
		work_limit: 1,
		seconds_per_tick: ScalarValue(0.5),
	}
	.encode()
	.unwrap();
	let mut response = [0_u8; SIMULATION_STAGE_RESPONSE_LEN];
	service
		.client
		.round_trip_into(OperationKind::SimulationStage, &stage, &mut response)
		.unwrap();
	let first = SimulationStageResponse::decode(&response).unwrap();
	assert_eq!(first.work_items, 1);
	assert!(first.pending);

	request.clear();
	encode_turf_adjacency_batch(
		&[TurfAdjacencyMutation {
			left: turf(0),
			right: turf(1),
			connected: true,
			firelock: false,
		}],
		&mut request,
	)
	.unwrap();
	assert!(matches!(
		service
			.client
			.round_trip_into(OperationKind::TurfAdjacencyBatch, &request, &mut [0_u8; 4]),
		Err(ClientError::Server(ServiceErrorCode::StageConflict))
	));

	service
		.client
		.round_trip_into(OperationKind::SimulationStage, &stage, &mut response)
		.unwrap();
	let second = SimulationStageResponse::decode(&response).unwrap();
	assert_eq!(second.work_items, 1);
	assert!(!second.pending);

	assert!(matches!(
		service.client.round_trip_into(
			OperationKind::FrontierBegin,
			&FrontierBeginRequest {
				epoch: 1,
				expected_count: 0,
			}
			.encode(),
			&mut [0_u8; 8]
		),
		Err(ClientError::Server(ServiceErrorCode::FrontierConflict))
	));
	service
		.client
		.round_trip_into(
			OperationKind::FrontierBegin,
			&FrontierBeginRequest {
				epoch: 2,
				expected_count: 1,
			}
			.encode(),
			&mut [0_u8; 8],
		)
		.unwrap();
	assert!(matches!(
		service.client.round_trip_into(
			OperationKind::FrontierCommit,
			&FrontierCommitRequest { epoch: 2 }.encode(),
			&mut [0_u8; 16]
		),
		Err(ClientError::Server(ServiceErrorCode::FrontierIncomplete))
	));
}
