use crate::turfs::{
	capture_two_turf_heat_trace,
	katmos::{capture_two_turf_equalize_trace, LegacyStageTrace},
	processing::capture_two_turf_diffusion_trace,
};
use dogmos_core::{
	metadata::{GasFireRole, GasId, GasMetadata, TurfHandle},
	world::{
		DogmosWorld, LifecycleAction, LifecycleMutation, MixtureStateMutation,
		TurfAdjacencyMutation, TurfHeatAdjacencyMutation, TurfHeatMutation, TurfHeatState,
		TurfLifecycleMutation, WorldEvent, WorldStage,
	},
	MixtureHandle, MAX_GAS_SLOTS,
};
use std::collections::BTreeMap;

const LEGACY_STAGE_TRANSCRIPT: &str = include_str!("fixtures/legacy_stage_transcript_v1.txt");
const TRANSCRIPT_HEADER: &str = "DOGMOS_LEGACY_STAGE_TRANSCRIPT_V1";

fn mixture(slot: u32) -> MixtureHandle {
	MixtureHandle {
		slot,
		generation: 1,
	}
}

fn turf(slot: u32) -> TurfHandle {
	TurfHandle {
		slot,
		generation: 1,
	}
}

fn oxygen() -> GasMetadata {
	GasMetadata {
		id: GasId(0),
		key: "o2".into(),
		name: "o2".into(),
		flags: 0,
		specific_heat: 20.0,
		fusion_power: 0.0,
		moles_visible: None,
		enthalpy: 0.0,
		fire_radiation_released: 0.0,
		fire_role: GasFireRole::None,
		fire_products: None,
	}
}

fn parse_legacy_stage_fixture(
	captured: &BTreeMap<&str, LegacyStageTrace>,
) -> BTreeMap<String, LegacyStageTrace> {
	let mut lines = LEGACY_STAGE_TRANSCRIPT.lines();
	assert_eq!(lines.next(), Some(TRANSCRIPT_HEADER));
	let rows = lines.collect::<Vec<_>>();
	assert_eq!(
		rows.len(),
		3,
		"expected three legacy stage transcript rows; captured {captured:?}"
	);
	rows.into_iter()
		.map(|row| {
			let fields = row.split('|').collect::<Vec<_>>();
			assert_eq!(fields.len(), 10, "malformed legacy stage transcript row");
			let pressure_events = match fields[4] {
				"none" => Vec::new(),
				"pressure_difference" => vec![(
					fields[9].parse().unwrap(),
					fields[5].parse().unwrap(),
					fields[6].parse().unwrap(),
					fields[7].parse().unwrap(),
					fields[8].parse().unwrap(),
				)],
				kind => panic!("unknown legacy stage event kind {kind}"),
			};
			(
				fields[0].to_owned(),
				LegacyStageTrace {
					work_items: fields[1].parse().unwrap(),
					left_value: fields[2].parse().unwrap(),
					right_value: fields[3].parse().unwrap(),
					pressure_events,
				},
			)
		})
		.collect()
}

fn capture_core_gas_stage(stage: WorldStage) -> LegacyStageTrace {
	let left_mixture = mixture(0);
	let right_mixture = mixture(1);
	let left_turf = turf(10);
	let right_turf = turf(11);
	let mut world = DogmosWorld::new_with_event_capacity(1024 * 1024, 8);
	world.install_gases(vec![oxygen()]).unwrap();
	world
		.apply_lifecycle(&[
			LifecycleMutation {
				action: LifecycleAction::Register,
				handle: left_mixture,
			},
			LifecycleMutation {
				action: LifecycleAction::Register,
				handle: right_mixture,
			},
		])
		.unwrap();
	let mut left_gases = [0.0; MAX_GAS_SLOTS];
	left_gases[0] = 100.0;
	world
		.apply_mixture_state(&[
			MixtureStateMutation {
				handle: left_mixture,
				expected_revision: 0,
				temperature: 2.7,
				volume: 2500.0,
				gases: left_gases,
			},
			MixtureStateMutation {
				handle: right_mixture,
				expected_revision: 0,
				temperature: 2.7,
				volume: 2500.0,
				gases: [0.0; MAX_GAS_SLOTS],
			},
		])
		.unwrap();
	world
		.apply_turf_lifecycle(&[
			TurfLifecycleMutation::Register {
				handle: left_turf,
				mixture: Some(left_mixture),
			},
			TurfLifecycleMutation::Register {
				handle: right_turf,
				mixture: Some(right_mixture),
			},
		])
		.unwrap();
	world
		.apply_turf_adjacency(&[TurfAdjacencyMutation {
			left: left_turf,
			right: right_turf,
			connected: true,
		}])
		.unwrap();
	let work_items = world
		.process_stage_cancellable(stage, 0.5, || false)
		.unwrap()
		.work_items;
	let mut events = Vec::new();
	world.drain_events_into(8, &mut events);
	let pressure_events = events
		.into_iter()
		.map(|event| match event {
			WorldEvent::PressureDifference {
				source,
				target,
				moles,
			} => (
				moles,
				source.slot,
				source.generation,
				target.slot,
				target.generation,
			),
			other => panic!("unexpected core equalize event {other:?}"),
		})
		.collect();
	LegacyStageTrace {
		work_items,
		left_value: world.snapshot(left_mixture).unwrap().gases[0],
		right_value: world.snapshot(right_mixture).unwrap().gases[0],
		pressure_events,
	}
}

fn capture_core_heat_trace() -> LegacyStageTrace {
	let left_turf = turf(10);
	let right_turf = turf(11);
	let mut world = DogmosWorld::new_with_event_capacity(1024 * 1024, 8);
	world
		.apply_turf_lifecycle(&[
			TurfLifecycleMutation::Register {
				handle: left_turf,
				mixture: None,
			},
			TurfLifecycleMutation::Register {
				handle: right_turf,
				mixture: None,
			},
		])
		.unwrap();
	world
		.apply_turf_heat(&[
			TurfHeatMutation {
				handle: left_turf,
				state: Some(TurfHeatState {
					temperature: 1000.0,
					thermal_conductivity: 0.05,
					heat_capacity: 100.0,
					adjacent_to_space: false,
				}),
			},
			TurfHeatMutation {
				handle: right_turf,
				state: Some(TurfHeatState {
					temperature: 300.0,
					thermal_conductivity: 0.05,
					heat_capacity: 200.0,
					adjacent_to_space: false,
				}),
			},
		])
		.unwrap();
	world
		.apply_turf_heat_adjacency(&[TurfHeatAdjacencyMutation {
			left: left_turf,
			right: right_turf,
			connected: true,
		}])
		.unwrap();
	let work_items = world
		.process_stage_cancellable(WorldStage::TurfHeat, 0.5, || false)
		.unwrap()
		.work_items;
	LegacyStageTrace {
		work_items,
		left_value: world.turf_heat(left_turf).unwrap().unwrap().temperature,
		right_value: world.turf_heat(right_turf).unwrap().unwrap().temperature,
		pressure_events: Vec::new(),
	}
}

#[test]
fn legacy_stage_and_event_traces_match_process_neutral_core() {
	let legacy = BTreeMap::from([
		("process_turfs", capture_two_turf_diffusion_trace()),
		("equalize", capture_two_turf_equalize_trace()),
		("turf_heat", capture_two_turf_heat_trace()),
	]);
	let fixture = parse_legacy_stage_fixture(&legacy);
	for (stage, trace) in &legacy {
		assert_eq!(trace, &fixture[*stage]);
	}
	assert_eq!(
		capture_core_gas_stage(WorldStage::ProcessTurfs),
		fixture["process_turfs"]
	);
	assert_eq!(
		capture_core_gas_stage(WorldStage::Equalize),
		fixture["equalize"]
	);
	assert_eq!(capture_core_heat_trace(), fixture["turf_heat"]);
}
