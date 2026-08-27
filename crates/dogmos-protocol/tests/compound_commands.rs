use dogmos_protocol::{
	decode_adjacency_batch, decode_lifecycle_batch, decode_mixture_state_batch,
	encode_mixture_state_batch, AdjacencyMutation, LifecycleAction, LifecycleMutation,
	MixtureSnapshot, MixtureSnapshotRequest, MixtureStateMutation, OperationKind, ProtocolError,
	ScalarValue, SimulationStage, SimulationStageRequest, SimulationStageResponse, WireHandle,
	ADJACENCY_MUTATION_LEN, LIFECYCLE_MUTATION_LEN, MAX_GAS_SLOTS, MIXTURE_SNAPSHOT_LEN,
	MIXTURE_STATE_MUTATION_LEN, SIMULATION_STAGE_REQUEST_LEN, SIMULATION_STAGE_RESPONSE_LEN,
};

fn handle(slot: u32, generation: u32) -> WireHandle {
	WireHandle { slot, generation }
}

fn decode_hex_fixture(input: &str) -> Vec<u8> {
	let digits = input
		.bytes()
		.filter(|byte| !byte.is_ascii_whitespace())
		.collect::<Vec<_>>();
	let (pairs, remainder) = digits.as_chunks::<2>();
	assert!(remainder.is_empty());
	pairs
		.iter()
		.map(|pair| {
			let digit = |value| match value {
				b'0'..=b'9' => value - b'0',
				b'a'..=b'f' => value - b'a' + 10,
				_ => panic!("invalid hex fixture"),
			};
			(digit(pair[0]) << 4) | digit(pair[1])
		})
		.collect()
}

#[test]
fn compound_operation_ids_are_stable() {
	assert_eq!(OperationKind::MixtureSnapshot as u16, 18);
	assert_eq!(OperationKind::MixtureLifecycleBatch as u16, 19);
	assert_eq!(OperationKind::AdjacencyBatch as u16, 20);
	assert_eq!(OperationKind::SimulationStage as u16, 21);
	assert_eq!(OperationKind::MixtureStateBatch as u16, 23);
	assert_eq!(
		OperationKind::try_from(18),
		Ok(OperationKind::MixtureSnapshot)
	);
	assert_eq!(
		OperationKind::try_from(21),
		Ok(OperationKind::SimulationStage)
	);
}

#[test]
fn mixture_state_batch_round_trips_exact_fixed_records() {
	let mut gases = [ScalarValue(0.0); MAX_GAS_SLOTS];
	gases[0] = ScalarValue(12.5);
	gases[31] = ScalarValue(0.25);
	let entries = [MixtureStateMutation {
		handle: handle(7, 11),
		expected_revision: 4,
		temperature: ScalarValue(293.15),
		volume: ScalarValue(2500.0),
		gases,
	}];
	let mut bytes = Vec::new();
	encode_mixture_state_batch(&entries, &mut bytes).unwrap();
	assert_eq!(bytes.len(), 4 + MIXTURE_STATE_MUTATION_LEN);
	assert_eq!(&bytes[4..12], &handle(7, 11).encode());
	assert_eq!(&bytes[12..16], &4_u32.to_le_bytes());
	assert_eq!(&bytes[16..20], &[0; 4]);
	assert_eq!(decode_mixture_state_batch(&bytes, 8).unwrap(), entries);

	bytes.push(0);
	assert!(matches!(
		decode_mixture_state_batch(&bytes, 8),
		Err(ProtocolError::InvalidPayloadLength { .. })
	));
}

#[test]
fn mixture_state_batch_matches_complete_protocol_v4_golden_bytes() {
	const GOLDEN_HEX: &str = concat!(
		"01000000070000000b0000000400000000000000000000000000104000000000",
		"00002040000000000000f03f0000000000000000000000000000000000000000",
		"0000000000000000000000000000000000000000000000000000000000000000",
		"0000000000000000000000000000000000000000000000000000000000000000",
		"0000000000000000000000000000000000000000000000000000000000000000",
		"0000000000000000000000000000000000000000000000000000000000000000",
		"0000000000000000000000000000000000000000000000000000000000000000",
		"0000000000000000000000000000000000000000000000000000000000000000",
		"0000000000000000000000000000000000000000000000000000000000000000",
		"00000040",
	);
	let mut gases = [ScalarValue(0.0); MAX_GAS_SLOTS];
	gases[0] = ScalarValue(1.0);
	gases[31] = ScalarValue(2.0);
	let entry = MixtureStateMutation {
		handle: handle(7, 11),
		expected_revision: 4,
		temperature: ScalarValue(4.0),
		volume: ScalarValue(8.0),
		gases,
	};
	let mut encoded = Vec::new();
	encode_mixture_state_batch(&[entry], &mut encoded).unwrap();
	let golden = decode_hex_fixture(GOLDEN_HEX);
	assert_eq!(golden.len(), 4 + MIXTURE_STATE_MUTATION_LEN);
	assert_eq!(encoded, golden);
	assert_eq!(decode_mixture_state_batch(&golden, 1).unwrap(), [entry]);
}

#[test]
fn mixture_state_batch_rejects_reserved_and_non_finite_values() {
	let entry = MixtureStateMutation {
		handle: handle(1, 2),
		expected_revision: 0,
		temperature: ScalarValue(293.15),
		volume: ScalarValue(2500.0),
		gases: [ScalarValue(0.0); MAX_GAS_SLOTS],
	};
	let mut bytes = Vec::new();
	encode_mixture_state_batch(&[entry], &mut bytes).unwrap();
	bytes[16..20].copy_from_slice(&1_u32.to_le_bytes());
	assert_eq!(
		decode_mixture_state_batch(&bytes, 1),
		Err(ProtocolError::ReservedMixtureStateField(1))
	);

	bytes[16..20].fill(0);
	bytes[20..28].copy_from_slice(&f64::NAN.to_bits().to_le_bytes());
	assert_eq!(
		decode_mixture_state_batch(&bytes, 1),
		Err(ProtocolError::NonFiniteScalar)
	);
}

#[test]
fn mixture_snapshot_request_requires_one_exact_handle() {
	let request = MixtureSnapshotRequest {
		handle: handle(7, 11),
	};
	let bytes = request.encode();
	assert_eq!(MixtureSnapshotRequest::decode(&bytes), Ok(request));
	assert!(matches!(
		MixtureSnapshotRequest::decode(&bytes[..7]),
		Err(ProtocolError::InvalidPayloadLength { .. })
	));
	let mut trailing = bytes.to_vec();
	trailing.push(0);
	assert!(matches!(
		MixtureSnapshotRequest::decode(&trailing),
		Err(ProtocolError::InvalidPayloadLength { .. })
	));
}

#[test]
fn mixture_snapshot_has_a_fixed_cross_bitness_layout() {
	let mut gases = [ScalarValue(0.0); MAX_GAS_SLOTS];
	gases[0] = ScalarValue(4.5);
	gases[1] = ScalarValue(9.25);
	let snapshot = MixtureSnapshot {
		revision: 0x1122_3344,
		gas_count: 2,
		temperature: ScalarValue(293.15),
		volume: ScalarValue(2500.0),
		gases,
	};
	let bytes = snapshot.encode().unwrap();
	assert_eq!(bytes.len(), MIXTURE_SNAPSHOT_LEN);
	assert_eq!(&bytes[0..4], &0x1122_3344_u32.to_le_bytes());
	assert_eq!(&bytes[4..8], &2_u32.to_le_bytes());
	assert_eq!(MixtureSnapshot::decode(&bytes), Ok(snapshot));
}

#[test]
fn mixture_snapshot_rejects_invalid_gas_counts_and_non_finite_values() {
	let mut bytes = [0_u8; MIXTURE_SNAPSHOT_LEN];
	bytes[4..8].copy_from_slice(&((MAX_GAS_SLOTS as u32) + 1).to_le_bytes());
	assert!(matches!(
		MixtureSnapshot::decode(&bytes),
		Err(ProtocolError::GasCountExceeded { .. })
	));
	bytes[4..8].copy_from_slice(&1_u32.to_le_bytes());
	bytes[8..16].copy_from_slice(&f64::NAN.to_bits().to_le_bytes());
	assert_eq!(
		MixtureSnapshot::decode(&bytes),
		Err(ProtocolError::NonFiniteScalar)
	);
}

#[test]
fn lifecycle_batch_round_trips_and_checks_exact_counted_length() {
	let entries = [
		LifecycleMutation {
			action: LifecycleAction::Register,
			handle: handle(1, 2),
		},
		LifecycleMutation {
			action: LifecycleAction::Unregister,
			handle: handle(3, 4),
		},
	];
	let mut bytes = Vec::new();
	dogmos_protocol::encode_lifecycle_batch(&entries, &mut bytes).unwrap();
	assert_eq!(bytes.len(), 4 + entries.len() * LIFECYCLE_MUTATION_LEN);
	assert_eq!(decode_lifecycle_batch(&bytes, 8).unwrap(), entries);
	bytes.push(0);
	assert!(matches!(
		decode_lifecycle_batch(&bytes, 8),
		Err(ProtocolError::InvalidPayloadLength { .. })
	));
}

#[test]
fn lifecycle_batch_rejects_unknown_actions_and_capacity_overruns() {
	let mut bytes = vec![0_u8; 4 + LIFECYCLE_MUTATION_LEN];
	bytes[0..4].copy_from_slice(&1_u32.to_le_bytes());
	bytes[4..8].copy_from_slice(&99_u32.to_le_bytes());
	assert_eq!(
		decode_lifecycle_batch(&bytes, 1),
		Err(ProtocolError::UnknownLifecycleAction(99))
	);
	bytes[0..4].copy_from_slice(&2_u32.to_le_bytes());
	assert!(matches!(
		decode_lifecycle_batch(&bytes, 1),
		Err(ProtocolError::OperationCountExceeded { .. })
	));
}

#[test]
fn adjacency_batch_round_trips_and_rejects_non_finite_conductivity() {
	let entries = [AdjacencyMutation {
		left: handle(1, 2),
		right: handle(3, 4),
		conductivity: ScalarValue(0.75),
	}];
	let mut bytes = Vec::new();
	dogmos_protocol::encode_adjacency_batch(&entries, &mut bytes).unwrap();
	assert_eq!(bytes.len(), 4 + ADJACENCY_MUTATION_LEN);
	assert_eq!(decode_adjacency_batch(&bytes, 4).unwrap(), entries);
	bytes[20..28].copy_from_slice(&f64::INFINITY.to_bits().to_le_bytes());
	assert_eq!(
		decode_adjacency_batch(&bytes, 4),
		Err(ProtocolError::NonFiniteScalar)
	);
}

#[test]
fn simulation_stage_request_is_fixed_width_and_validated() {
	let request = SimulationStageRequest {
		stage: SimulationStage::ProcessTurfHeat,
		seconds_per_tick: ScalarValue(0.5),
	};
	let bytes = request.encode().unwrap();
	assert_eq!(bytes.len(), SIMULATION_STAGE_REQUEST_LEN);
	assert_eq!(SimulationStageRequest::decode(&bytes), Ok(request));
	let mut unknown = bytes;
	unknown[0..4].copy_from_slice(&99_u32.to_le_bytes());
	assert_eq!(
		SimulationStageRequest::decode(&unknown),
		Err(ProtocolError::UnknownSimulationStage(99))
	);
}

#[test]
fn simulation_stage_response_is_fixed_width() {
	let response = SimulationStageResponse {
		work_items: 64,
		callback_events: 3,
	};
	let bytes = response.encode();
	assert_eq!(bytes.len(), SIMULATION_STAGE_RESPONSE_LEN);
	assert_eq!(SimulationStageResponse::decode(&bytes), Ok(response));
	assert!(matches!(
		SimulationStageResponse::decode(&bytes[..7]),
		Err(ProtocolError::InvalidPayloadLength { .. })
	));
}
