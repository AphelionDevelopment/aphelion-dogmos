use dogmos_core::numerics::{
	conduction::{
		conduction_step, heat_row_weight, ConductionError, BASE_HEAT_STEP_SECONDS,
		BYOND_INFINITY_THRESHOLD,
	},
	diffusion::{
		diffusion_self_weight, diffusion_step, diffusion_step_into,
		diffusion_step_into_cancellable, upsert_graph_node, validate_graph, DiffusionError,
		DirectedEdge, GraphNode, GraphValidationError, MixtureHandle, NodeHandle, NodeUpsert,
		GAS_DIFFUSION_CONSTANT, MAX_CARDINAL_NEIGHBORS,
	},
};
use proptest::prelude::*;
use std::mem::{offset_of, size_of};

fn mixture_handle(slot: u32, generation: u32) -> MixtureHandle {
	MixtureHandle { slot, generation }
}

fn node(handle: u32) -> GraphNode {
	GraphNode {
		handle: NodeHandle(handle),
		generation: 1,
		mixture: Some(mixture_handle(handle, 1)),
	}
}

#[test]
fn mixture_handle_is_generation_checked_and_cross_bitness_stable() {
	assert_eq!(size_of::<MixtureHandle>(), 8);
	assert_eq!(offset_of!(MixtureHandle, slot), 0);
	assert_eq!(offset_of!(MixtureHandle, generation), 4);
	assert_ne!(mixture_handle(4, 1), mixture_handle(4, 2));
}

fn reciprocal_edges(pairs: &[(u32, u32)]) -> Vec<DirectedEdge> {
	pairs
		.iter()
		.flat_map(|&(first, second)| {
			[
				DirectedEdge {
					from: NodeHandle(first),
					to: NodeHandle(second),
				},
				DirectedEdge {
					from: NodeHandle(second),
					to: NodeHandle(first),
				},
			]
		})
		.collect()
}

#[test]
fn graph_accepts_reciprocal_cardinal_degrees_zero_through_six() {
	for degree in 0..=MAX_CARDINAL_NEIGHBORS {
		let nodes = (0..=degree).map(node).collect::<Vec<_>>();
		let pairs = (1..=degree)
			.map(|neighbor| (0, neighbor))
			.collect::<Vec<_>>();
		let graph = validate_graph(&nodes, &reciprocal_edges(&pairs)).unwrap();
		assert_eq!(graph.degree(NodeHandle(0)), Some(degree));
		assert_eq!(
			diffusion_self_weight(degree).unwrap(),
			1.0 - degree as f32 / 8.0
		);
	}
}

#[test]
fn graph_rejects_invalid_topology_before_processing() {
	let nodes = [node(0), node(1)];
	assert!(matches!(
		validate_graph(&[node(0), node(0)], &[]),
		Err(GraphValidationError::DuplicateNode(NodeHandle(0)))
	));
	assert_eq!(
		validate_graph(
			&nodes,
			&[DirectedEdge {
				from: NodeHandle(0),
				to: NodeHandle(1),
			}],
		),
		Err(GraphValidationError::MissingReciprocalEdge {
			from: NodeHandle(0),
			to: NodeHandle(1),
		})
	);
	assert!(matches!(
		validate_graph(&nodes, &reciprocal_edges(&[(0, 1), (0, 1)]),),
		Err(GraphValidationError::DuplicateEdge { .. })
	));
	assert!(matches!(
		validate_graph(
			&nodes,
			&[DirectedEdge {
				from: NodeHandle(0),
				to: NodeHandle(0),
			}],
		),
		Err(GraphValidationError::SelfEdge(NodeHandle(0)))
	));
	assert!(matches!(
		validate_graph(&nodes, &reciprocal_edges(&[(0, 2)]),),
		Err(GraphValidationError::UnknownNode(NodeHandle(2)))
	));

	let mut missing_mixture = node(0);
	missing_mixture.mixture = None;
	assert!(matches!(
		validate_graph(&[missing_mixture], &[]),
		Err(GraphValidationError::MissingMixture(NodeHandle(0)))
	));

	let degree_seven_nodes = (0..=7).map(node).collect::<Vec<_>>();
	let degree_seven_pairs = (1..=7).map(|neighbor| (0, neighbor)).collect::<Vec<_>>();
	assert!(matches!(
		validate_graph(&degree_seven_nodes, &reciprocal_edges(&degree_seven_pairs)),
		Err(GraphValidationError::DegreeExceeded {
			handle: NodeHandle(0),
			degree: 7,
		})
	));
}

#[test]
fn diffusion_bounds_each_disconnected_component_independently() {
	let graph = validate_graph(
		&[node(0), node(1), node(2), node(3)],
		&reciprocal_edges(&[(0, 1), (2, 3)]),
	)
	.unwrap();
	let result = diffusion_step(&graph, 1, &[0.0, 10.0, 100.0, 200.0]).unwrap();
	assert!(result[0..2]
		.iter()
		.all(|&value| (0.0..=10.0).contains(&value)));
	assert!(result[2..4]
		.iter()
		.all(|&value| (100.0..=200.0).contains(&value)));
}

#[test]
fn diffusion_relaxation_budget_controls_work_without_becoming_time() {
	let graph = validate_graph(&[node(0), node(1)], &reciprocal_edges(&[(0, 1)])).unwrap();
	let initial = [0.0, 100.0];
	let one_iteration = diffusion_step(&graph, 1, &initial).unwrap();
	let two_iterations = diffusion_step(&graph, 1, &one_iteration).unwrap();
	assert_ne!(one_iteration, two_iterations);
	assert!(two_iterations
		.iter()
		.all(|value| value.is_finite() && *value >= 0.0));
	assert_eq!(one_iteration.iter().sum::<f32>(), initial.iter().sum());
	assert_eq!(two_iterations.iter().sum::<f32>(), initial.iter().sum());
}

#[test]
fn diffusion_can_reuse_a_caller_owned_output_buffer() {
	let graph = validate_graph(&[node(0), node(1)], &reciprocal_edges(&[(0, 1)])).unwrap();
	let state = [0.0, 100.0, 50.0, 25.0];
	let expected = diffusion_step(&graph, 2, &state).unwrap();
	let mut output = vec![f32::NAN; state.len()];
	let allocation = output.as_ptr();
	diffusion_step_into(&graph, 2, &state, &mut output).unwrap();
	assert_eq!(output, expected);
	assert_eq!(output.as_ptr(), allocation);
}

#[test]
fn diffusion_cancellation_stops_before_committing_more_work() {
	let graph = validate_graph(&[node(0), node(1)], &reciprocal_edges(&[(0, 1)])).unwrap();
	let state = [0.0, 100.0];
	let mut output = [f32::NAN; 2];
	assert_eq!(
		diffusion_step_into_cancellable(&graph, 1, &state, &mut output, || true),
		Err(DiffusionError::Cancelled)
	);
	assert!(output.iter().all(|value| value.is_nan()));
}

#[test]
fn graph_allows_disconnected_components_and_generation_replacement() {
	let nodes = [node(0), node(1), node(2), node(3)];
	let graph = validate_graph(&nodes, &reciprocal_edges(&[(0, 1), (2, 3)])).unwrap();
	assert_eq!(graph.node_count(), 4);

	let mut replaceable = vec![node(7)];
	let replacement = GraphNode {
		handle: NodeHandle(7),
		generation: 2,
		mixture: Some(mixture_handle(99, 2)),
	};
	assert_eq!(
		upsert_graph_node(&mut replaceable, replacement),
		NodeUpsert::Replaced {
			previous_generation: 1,
		}
	);
	assert_eq!(replaceable[0], replacement);
	assert_eq!(
		upsert_graph_node(&mut replaceable, node(8)),
		NodeUpsert::Inserted
	);
	let stale = GraphNode {
		handle: NodeHandle(7),
		generation: 1,
		mixture: Some(mixture_handle(4, 1)),
	};
	assert_eq!(
		upsert_graph_node(&mut replaceable, stale),
		NodeUpsert::IgnoredStale {
			current_generation: 2,
		}
	);
}

proptest! {
	#[test]
	fn diffusion_conserves_and_obeys_the_maximum_principle(
		degree in 0_u32..=6,
		values in prop::collection::vec(0.0_f32..1.0e6, 7),
	) {
		let nodes = (0..=degree).map(node).collect::<Vec<_>>();
		let pairs = (1..=degree).map(|neighbor| (0, neighbor)).collect::<Vec<_>>();
		let graph = validate_graph(&nodes, &reciprocal_edges(&pairs)).unwrap();
		let before = values[..nodes.len()].to_vec();
		let after = diffusion_step(&graph, 1, &before).unwrap();
		let before_total = before.iter().map(|&value| f64::from(value)).sum::<f64>();
		let after_total = after.iter().map(|&value| f64::from(value)).sum::<f64>();
		let tolerance = 1.0e-5_f64.max(before_total * 1.0e-5);
		prop_assert!((after_total - before_total).abs() <= tolerance);
		let minimum = before.iter().copied().fold(f32::INFINITY, f32::min);
		let maximum = before.iter().copied().fold(f32::NEG_INFINITY, f32::max);
		prop_assert!(after.iter().all(|&value| value >= minimum && value <= maximum));
	}

	#[test]
	fn conduction_conserves_finite_energy_and_stays_bounded(
		temperatures in prop::collection::vec(1.0_f32..10_000.0, 4),
		conductivities in prop::collection::vec(0.0_f32..10.0, 4),
		capacities in prop::collection::vec(1.0_f32..1.0e6, 4),
		seconds_per_tick in 0.0_f32..2.0,
	) {
		let initial = temperatures.clone();
		let mut result = temperatures;
		let edges = [(0, 1), (1, 2), (1, 3)];
		conduction_step(
			&mut result,
			&conductivities,
			&capacities,
			&edges,
			seconds_per_tick,
		).unwrap();
		let minimum = initial.iter().copied().fold(f32::INFINITY, f32::min);
		let maximum = initial.iter().copied().fold(f32::NEG_INFINITY, f32::max);
		prop_assert!(result.iter().all(|value| value.is_finite()));
		prop_assert!(result.iter().all(|&value| value >= minimum && value <= maximum));
		let initial_energy = initial
			.iter()
			.zip(&capacities)
			.map(|(&temperature, &capacity)| f64::from(temperature) * f64::from(capacity))
			.sum::<f64>();
		let result_energy = result
			.iter()
			.zip(&capacities)
			.map(|(&temperature, &capacity)| f64::from(temperature) * f64::from(capacity))
			.sum::<f64>();
		prop_assert!((result_energy - initial_energy).abs() / initial_energy < 1.0e-4);
	}
}

#[test]
fn diffusion_coefficient_preserves_a_non_negative_self_weight() {
	assert_eq!(GAS_DIFFUSION_CONSTANT, 0.125);
	assert_eq!(diffusion_self_weight(6).unwrap(), 0.25);
	assert!(diffusion_self_weight(7).is_err());
}

fn star_edges(degree: u32) -> Vec<(u32, u32)> {
	(1..=degree).map(|neighbor| (0, neighbor)).collect()
}

fn legacy_simultaneous_step(
	temperatures: &[f32],
	conductivities: &[f32],
	capacities: &[f32],
	edges: &[(u32, u32)],
) -> Vec<f32> {
	let mut deltas = vec![0.0; temperatures.len()];
	for &(first, second) in edges {
		let first = first as usize;
		let second = second as usize;
		let harmonic =
			capacities[first] * capacities[second] / (capacities[first] + capacities[second]);
		let energy = conductivities[first].min(conductivities[second])
			* (temperatures[second] - temperatures[first])
			* harmonic;
		deltas[first] += energy / capacities[first];
		deltas[second] -= energy / capacities[second];
	}
	temperatures
		.iter()
		.zip(deltas)
		.map(|(&temperature, delta)| temperature + delta)
		.collect()
}

#[test]
fn old_six_neighbor_simultaneous_update_can_leave_pre_step_extrema() {
	let temperatures = vec![1000.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
	let conductivities = vec![0.4; temperatures.len()];
	let capacities = vec![100.0; temperatures.len()];
	let old = legacy_simultaneous_step(&temperatures, &conductivities, &capacities, &star_edges(6));
	assert!(old[0] < 0.0);
}

#[test]
fn stable_conduction_handles_four_and_six_neighbors_and_high_conductivity() {
	for degree in [4, 6] {
		let mut temperatures = vec![1000.0];
		temperatures.resize(degree + 1, 0.0);
		let conductivities = vec![4.0; temperatures.len()];
		let capacities = vec![100.0; temperatures.len()];
		let initial_energy = temperatures
			.iter()
			.zip(&capacities)
			.map(|(&temperature, &capacity)| f64::from(temperature * capacity))
			.sum::<f64>();
		let stats = conduction_step(
			&mut temperatures,
			&conductivities,
			&capacities,
			&star_edges(degree as u32),
			BASE_HEAT_STEP_SECONDS,
		)
		.unwrap();
		assert!(stats.substeps > 1);
		assert!(temperatures
			.iter()
			.all(|&temperature| (0.0..=1000.0).contains(&temperature)));
		let final_energy = temperatures
			.iter()
			.zip(&capacities)
			.map(|(&temperature, &capacity)| f64::from(temperature * capacity))
			.sum::<f64>();
		assert!((final_energy - initial_energy).abs() / initial_energy < 1.0e-4);
	}
}

#[test]
fn conduction_handles_unequal_and_effectively_infinite_capacities() {
	let weight = heat_row_weight(0.4, 0.2, 100.0, 300.0).unwrap();
	assert!((weight - 0.15).abs() < 1.0e-6);

	let mut temperatures = [300.0, 1000.0];
	conduction_step(
		&mut temperatures,
		&[1.0, 1.0],
		&[100.0, BYOND_INFINITY_THRESHOLD],
		&[(0, 1)],
		BASE_HEAT_STEP_SECONDS,
	)
	.unwrap();
	assert_eq!(temperatures, [1000.0, 1000.0]);

	let mut two_infinite = [300.0, 1000.0];
	conduction_step(
		&mut two_infinite,
		&[1.0, 1.0],
		&[BYOND_INFINITY_THRESHOLD, BYOND_INFINITY_THRESHOLD],
		&[(0, 1)],
		BASE_HEAT_STEP_SECONDS,
	)
	.unwrap();
	assert_eq!(two_infinite, [300.0, 1000.0]);
}

#[test]
fn conduction_rejects_zero_or_invalid_capacity() {
	for capacity in [0.0, -1.0, f32::NAN, f32::INFINITY] {
		let mut temperatures = [300.0, 1000.0];
		assert!(matches!(
			conduction_step(
				&mut temperatures,
				&[1.0, 1.0],
				&[capacity, 100.0],
				&[(0, 1)],
				BASE_HEAT_STEP_SECONDS,
			),
			Err(ConductionError::InvalidCapacity { index: 0 })
		));
	}
}

#[test]
fn conduction_is_deterministic_and_elapsed_time_is_explicit() {
	let conductivities = [0.4, 0.4, 0.4];
	let capacities = [100.0, 200.0, 300.0];
	let edges = [(0, 1), (1, 2)];
	let mut one_second = [1000.0, 300.0, 0.0];
	let mut repeated = one_second;
	let first_stats =
		conduction_step(&mut one_second, &conductivities, &capacities, &edges, 1.0).unwrap();
	let repeated_stats =
		conduction_step(&mut repeated, &conductivities, &capacities, &edges, 1.0).unwrap();
	assert_eq!(one_second, repeated);
	assert_eq!(first_stats, repeated_stats);

	let mut two_half_seconds = [1000.0, 300.0, 0.0];
	conduction_step(
		&mut two_half_seconds,
		&conductivities,
		&capacities,
		&edges,
		0.5,
	)
	.unwrap();
	conduction_step(
		&mut two_half_seconds,
		&conductivities,
		&capacities,
		&edges,
		0.5,
	)
	.unwrap();
	for values in [one_second, two_half_seconds] {
		assert!(values
			.iter()
			.all(|&temperature| (0.0..=1000.0).contains(&temperature)));
		let energy = values
			.iter()
			.zip(capacities)
			.map(|(&temperature, capacity)| f64::from(temperature * capacity))
			.sum::<f64>();
		assert!((energy - 160_000.0).abs() / 160_000.0 < 1.0e-4);
	}
}
