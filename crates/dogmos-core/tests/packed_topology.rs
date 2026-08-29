use dogmos_core::{
	metadata::TurfHandle,
	topology::{PackedTopology, TopologyError},
};

fn turf(slot: u32) -> TurfHandle {
	TurfHandle {
		slot,
		generation: 1,
	}
}

#[test]
fn gas_neighbors_are_bounded_sorted_replaceable_and_removable() {
	let mut topology = PackedTopology::default();
	for slot in (1..=6).rev() {
		topology.connect_gas(turf(0), turf(slot)).unwrap();
	}
	assert_eq!(
		topology
			.gas_neighbors(turf(0))
			.map(|neighbor| neighbor.handle.slot)
			.collect::<Vec<_>>(),
		vec![1, 2, 3, 4, 5, 6]
	);
	assert_eq!(
		topology.connect_gas(turf(0), turf(7)),
		Err(TopologyError::DegreeExceeded(turf(0)))
	);
	assert!(!topology.connect_gas(turf(0), turf(1)).unwrap());
	assert!(topology.disconnect_gas(turf(0), turf(3)));
	assert!(topology.connect_gas(turf(0), turf(7)).unwrap());
	assert_eq!(topology.gas_edge_count(), 6);
}

#[test]
fn heat_edges_and_firelock_metadata_are_independent() {
	let mut topology = PackedTopology::default();
	topology.connect_gas(turf(0), turf(1)).unwrap();
	topology.connect_heat(turf(0), turf(2)).unwrap();
	topology.set_firelock(turf(0), turf(1), true).unwrap();
	assert!(topology.gas_neighbors(turf(0)).next().unwrap().firelock);
	assert_eq!(
		topology.heat_neighbors(turf(0)).next().unwrap().handle,
		turf(2)
	);
	assert_eq!(
		topology.set_firelock(turf(0), turf(2), true),
		Err(TopologyError::MissingGasEdge)
	);
	topology.remove_turf(turf(0));
	assert_eq!(topology.gas_edge_count(), 0);
	assert_eq!(topology.heat_edge_count(), 0);
}

#[test]
fn every_effective_mutation_advances_revision_once() {
	let mut topology = PackedTopology::default();
	assert_eq!(topology.revision(), 0);
	assert!(topology.connect_gas(turf(0), turf(1)).unwrap());
	assert_eq!(topology.revision(), 1);
	assert!(!topology.connect_gas(turf(0), turf(1)).unwrap());
	assert_eq!(topology.revision(), 1);
	topology.set_firelock(turf(0), turf(1), true).unwrap();
	assert_eq!(topology.revision(), 2);
}
