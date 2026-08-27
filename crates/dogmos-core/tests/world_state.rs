use dogmos_core::world::{
	AdjacencyMutation, DogmosWorld, LifecycleAction, LifecycleMutation, MixtureStateMutation,
	WorldError, WorldStage,
};
use dogmos_core::{MixtureHandle, MAX_GAS_SLOTS};

fn handle(slot: u32, generation: u32) -> MixtureHandle {
	MixtureHandle { slot, generation }
}

fn state(handle: MixtureHandle, expected_revision: u32, oxygen: f32) -> MixtureStateMutation {
	let mut gases = [0.0; MAX_GAS_SLOTS];
	gases[0] = oxygen;
	MixtureStateMutation {
		handle,
		expected_revision,
		temperature: 293.15,
		volume: 2500.0,
		gases,
	}
}

#[test]
fn mixture_state_batches_are_revision_checked_and_atomic() {
	let mut world = DogmosWorld::new(1024 * 1024);
	world
		.apply_lifecycle(&[
			LifecycleMutation {
				action: LifecycleAction::Register,
				handle: handle(0, 1),
			},
			LifecycleMutation {
				action: LifecycleAction::Register,
				handle: handle(1, 1),
			},
		])
		.unwrap();
	assert_eq!(
		world
			.apply_mixture_state(&[state(handle(0, 1), 0, 10.0), state(handle(1, 1), 0, 20.0)])
			.unwrap(),
		2
	);
	assert_eq!(world.snapshot(handle(0, 1)).unwrap().revision, 1);
	assert_eq!(world.snapshot(handle(0, 1)).unwrap().gases[0], 10.0);

	assert!(matches!(
		world.apply_mixture_state(&[state(handle(0, 1), 1, 30.0), state(handle(1, 1), 0, 40.0),]),
		Err(WorldError::RevisionMismatch { .. })
	));
	assert_eq!(world.snapshot(handle(0, 1)).unwrap().revision, 1);
	assert_eq!(world.snapshot(handle(0, 1)).unwrap().gases[0], 10.0);
}

#[test]
fn mixture_state_batches_reject_invalid_physical_values() {
	let mut world = DogmosWorld::new(1024 * 1024);
	world
		.apply_lifecycle(&[LifecycleMutation {
			action: LifecycleAction::Register,
			handle: handle(0, 1),
		}])
		.unwrap();
	let mut invalid = state(handle(0, 1), 0, 10.0);
	invalid.gases[3] = -1.0;
	assert_eq!(
		world.apply_mixture_state(&[invalid]),
		Err(WorldError::InvalidMixtureState)
	);
	assert_eq!(world.snapshot(handle(0, 1)).unwrap().revision, 0);
}

#[test]
fn mixture_state_matches_legacy_temperature_and_volume_bounds() {
	let mut world = DogmosWorld::new(1024 * 1024);
	world
		.apply_lifecycle(&[LifecycleMutation {
			action: LifecycleAction::Register,
			handle: handle(0, 1),
		}])
		.unwrap();
	let mut below_cosmic_background = state(handle(0, 1), 0, 10.0);
	below_cosmic_background.temperature = 2.6;
	assert_eq!(
		world.apply_mixture_state(&[below_cosmic_background]),
		Err(WorldError::InvalidMixtureState)
	);

	let mut zero_volume = state(handle(0, 1), 0, 10.0);
	zero_volume.volume = 0.0;
	assert_eq!(world.apply_mixture_state(&[zero_volume]).unwrap(), 1);
	assert_eq!(world.snapshot(handle(0, 1)).unwrap().volume, 0.0);
}

#[test]
fn world_owns_generation_checked_mixtures_adjacency_and_stage_state() {
	let mut world = DogmosWorld::new(1024 * 1024);
	assert_eq!(
		world
			.apply_lifecycle(&[
				LifecycleMutation {
					action: LifecycleAction::Register,
					handle: handle(0, 1),
				},
				LifecycleMutation {
					action: LifecycleAction::Register,
					handle: handle(1, 1),
				},
			])
			.unwrap(),
		2
	);
	let empty = world.snapshot(handle(0, 1)).unwrap();
	assert_eq!(empty.temperature, 2.7);
	assert_eq!(empty.volume, 0.0);
	assert!(empty.gases.iter().all(|amount| *amount == 0.0));
	assert_eq!(
		world
			.apply_adjacency(&[AdjacencyMutation {
				left: handle(0, 1),
				right: handle(1, 1),
				conductivity: 0.75,
			}])
			.unwrap(),
		1
	);
	let result = world
		.process_stage_cancellable(WorldStage::ProcessTurfs, 0.5, || false)
		.unwrap();
	assert_eq!(result.work_items, 2);
	assert_eq!(world.snapshot(handle(0, 1)).unwrap().revision, 1);
}

#[test]
fn stale_lifecycle_and_cancelled_stage_leave_world_state_unchanged() {
	let mut world = DogmosWorld::new(1024 * 1024);
	world
		.apply_lifecycle(&[LifecycleMutation {
			action: LifecycleAction::Register,
			handle: handle(0, 2),
		}])
		.unwrap();
	assert!(matches!(
		world.snapshot(handle(0, 1)),
		Err(WorldError::StaleHandle { .. })
	));
	assert_eq!(
		world.process_stage_cancellable(WorldStage::ProcessTurfs, 0.5, || true),
		Err(WorldError::Cancelled)
	);
	assert_eq!(world.snapshot(handle(0, 2)).unwrap().revision, 0);
}

#[test]
fn unregister_releases_mixture_and_incident_edges() {
	let mut world = DogmosWorld::new(1024 * 1024);
	world
		.apply_lifecycle(&[
			LifecycleMutation {
				action: LifecycleAction::Register,
				handle: handle(0, 1),
			},
			LifecycleMutation {
				action: LifecycleAction::Register,
				handle: handle(1, 1),
			},
		])
		.unwrap();
	world
		.apply_adjacency(&[AdjacencyMutation {
			left: handle(0, 1),
			right: handle(1, 1),
			conductivity: 1.0,
		}])
		.unwrap();
	world
		.apply_lifecycle(&[LifecycleMutation {
			action: LifecycleAction::Unregister,
			handle: handle(0, 1),
		}])
		.unwrap();
	assert!(matches!(
		world.snapshot(handle(0, 1)),
		Err(WorldError::UnknownHandle(_))
	));
	assert_eq!(world.edge_count(), 0);
}

#[test]
fn unregister_preserves_generation_tombstone_against_aba_reuse() {
	let mut world = DogmosWorld::new(1024 * 1024);
	world
		.apply_lifecycle(&[
			LifecycleMutation {
				action: LifecycleAction::Register,
				handle: handle(0, 2),
			},
			LifecycleMutation {
				action: LifecycleAction::Unregister,
				handle: handle(0, 2),
			},
		])
		.unwrap();

	assert!(matches!(
		world.apply_lifecycle(&[LifecycleMutation {
			action: LifecycleAction::Register,
			handle: handle(0, 1),
		}]),
		Err(WorldError::StaleHandle { current: 2, .. })
	));
	assert!(matches!(
		world.apply_lifecycle(&[LifecycleMutation {
			action: LifecycleAction::Register,
			handle: handle(0, 2),
		}]),
		Err(WorldError::StaleHandle { current: 2, .. })
	));
	assert_eq!(
		world
			.apply_lifecycle(&[LifecycleMutation {
				action: LifecycleAction::Register,
				handle: handle(0, 3),
			}])
			.unwrap(),
		1
	);
	assert_eq!(world.snapshot(handle(0, 3)).unwrap().revision, 0);
}

#[test]
fn invalid_generation_reuse_rolls_back_the_entire_lifecycle_batch() {
	let mut world = DogmosWorld::new(1024 * 1024);
	world
		.apply_lifecycle(&[LifecycleMutation {
			action: LifecycleAction::Register,
			handle: handle(0, 2),
		}])
		.unwrap();

	assert!(matches!(
		world.apply_lifecycle(&[
			LifecycleMutation {
				action: LifecycleAction::Unregister,
				handle: handle(0, 2),
			},
			LifecycleMutation {
				action: LifecycleAction::Register,
				handle: handle(0, 1),
			},
		]),
		Err(WorldError::StaleHandle { current: 2, .. })
	));
	assert_eq!(world.snapshot(handle(0, 2)).unwrap().revision, 0);
}
