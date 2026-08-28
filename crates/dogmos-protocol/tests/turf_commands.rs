use dogmos_protocol::{
	decode_turf_adjacency_batch, decode_turf_heat_adjacency_batch, decode_turf_heat_batch,
	decode_turf_lifecycle_batch, encode_turf_adjacency_batch, encode_turf_heat_adjacency_batch,
	encode_turf_heat_batch, encode_turf_lifecycle_batch, LifecycleAction, ProtocolError,
	ScalarValue, TurfAdjacencyMutation, TurfHeatAdjacencyMutation, TurfHeatMutation,
	TurfLifecycleMutation, WireHandle, TURF_ADJACENCY_MUTATION_LEN,
	TURF_HEAT_ADJACENCY_MUTATION_LEN, TURF_HEAT_MUTATION_LEN, TURF_LIFECYCLE_MUTATION_LEN,
};

fn handle(slot: u32, generation: u32) -> WireHandle {
	WireHandle { slot, generation }
}

#[test]
fn turf_lifecycle_batch_has_a_fixed_width_golden_layout() {
	let entries = [
		TurfLifecycleMutation {
			action: LifecycleAction::Register,
			turf: handle(1, 2),
			mixture: Some(handle(3, 4)),
		},
		TurfLifecycleMutation {
			action: LifecycleAction::Unregister,
			turf: handle(5, 6),
			mixture: None,
		},
	];
	let mut bytes = Vec::new();
	encode_turf_lifecycle_batch(&entries, &mut bytes).unwrap();
	assert_eq!(bytes.len(), 4 + 2 * TURF_LIFECYCLE_MUTATION_LEN);
	assert_eq!(&bytes[0..4], &2_u32.to_le_bytes());
	assert_eq!(
		&bytes[4..8],
		&(LifecycleAction::Register as u32).to_le_bytes()
	);
	assert_eq!(&bytes[8..16], &handle(1, 2).encode());
	assert_eq!(&bytes[16..20], &1_u32.to_le_bytes());
	assert_eq!(&bytes[20..28], &handle(3, 4).encode());
	assert_eq!(decode_turf_lifecycle_batch(&bytes, 2).unwrap(), entries);
}

#[test]
fn turf_topology_batch_validates_boolean_and_firelock_flags() {
	let entry = TurfAdjacencyMutation {
		left: handle(1, 2),
		right: handle(3, 4),
		connected: true,
		firelock: true,
	};
	let mut bytes = Vec::new();
	encode_turf_adjacency_batch(&[entry], &mut bytes).unwrap();
	assert_eq!(bytes.len(), 4 + TURF_ADJACENCY_MUTATION_LEN);
	assert_eq!(decode_turf_adjacency_batch(&bytes, 1), Ok(vec![entry]));

	bytes[20..24].copy_from_slice(&2_u32.to_le_bytes());
	assert_eq!(
		decode_turf_adjacency_batch(&bytes, 1),
		Err(ProtocolError::InvalidBoolean(2))
	);
	bytes[20..24].copy_from_slice(&0_u32.to_le_bytes());
	bytes[24..28].copy_from_slice(&1_u32.to_le_bytes());
	assert_eq!(
		decode_turf_adjacency_batch(&bytes, 1),
		Err(ProtocolError::FirelockOnDisconnectedEdge)
	);
}

#[test]
fn turf_topology_batch_rejects_duplicate_undirected_edges() {
	let entries = [
		TurfAdjacencyMutation {
			left: handle(1, 2),
			right: handle(3, 4),
			connected: true,
			firelock: false,
		},
		TurfAdjacencyMutation {
			left: handle(3, 4),
			right: handle(1, 2),
			connected: false,
			firelock: false,
		},
	];
	let mut bytes = Vec::new();
	assert_eq!(
		encode_turf_adjacency_batch(&entries, &mut bytes),
		Err(ProtocolError::DuplicateTurfAdjacency { left: 1, right: 3 })
	);

	encode_turf_adjacency_batch(&entries[..1], &mut bytes).unwrap();
	bytes[0..4].copy_from_slice(&2_u32.to_le_bytes());
	bytes.extend_from_slice(&handle(3, 4).encode());
	bytes.extend_from_slice(&handle(1, 2).encode());
	bytes.extend_from_slice(&0_u32.to_le_bytes());
	bytes.extend_from_slice(&0_u32.to_le_bytes());
	assert_eq!(
		decode_turf_adjacency_batch(&bytes, 2),
		Err(ProtocolError::DuplicateTurfAdjacency { left: 1, right: 3 })
	);
}

#[test]
fn turf_heat_batch_is_fixed_width_and_rejects_reserved_bits() {
	let entries = [
		TurfHeatMutation {
			turf: handle(7, 8),
			state: Some(dogmos_protocol::TurfHeatState {
				temperature: ScalarValue(700.0),
				thermal_conductivity: ScalarValue(0.4),
				heat_capacity: ScalarValue(2500.0),
				adjacent_to_space: true,
			}),
		},
		TurfHeatMutation {
			turf: handle(9, 10),
			state: None,
		},
	];
	let mut bytes = Vec::new();
	encode_turf_heat_batch(&entries, &mut bytes).unwrap();
	assert_eq!(bytes.len(), 4 + 2 * TURF_HEAT_MUTATION_LEN);
	assert_eq!(decode_turf_heat_batch(&bytes, 2).unwrap(), entries);

	bytes[12..16].copy_from_slice(&4_u32.to_le_bytes());
	assert_eq!(
		decode_turf_heat_batch(&bytes, 2),
		Err(ProtocolError::UnknownTurfHeatFlags(4))
	);
}

#[test]
fn turf_heat_adjacency_batch_has_a_fixed_width_layout() {
	let entry = TurfHeatAdjacencyMutation {
		left: handle(11, 12),
		right: handle(13, 14),
		connected: true,
	};
	let mut bytes = Vec::new();
	encode_turf_heat_adjacency_batch(&[entry], &mut bytes).unwrap();
	assert_eq!(bytes.len(), 4 + TURF_HEAT_ADJACENCY_MUTATION_LEN);
	assert_eq!(decode_turf_heat_adjacency_batch(&bytes, 1), Ok(vec![entry]));
	bytes[20..24].copy_from_slice(&3_u32.to_le_bytes());
	assert_eq!(
		decode_turf_heat_adjacency_batch(&bytes, 1),
		Err(ProtocolError::InvalidBoolean(3))
	);
}
