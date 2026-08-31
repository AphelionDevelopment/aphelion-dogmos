use dogmos_core::{
	metadata::{GasFireRole, GasId, GasMetadata, TurfHandle},
	world::{
		Command, DogmosWorld, FrontierError, LifecycleAction, LifecycleMutation,
		MixtureStateMutation, StageChunkRequest, TurfAdjacencyMutation, TurfHeatAdjacencyMutation,
		TurfHeatMutation, TurfHeatState, TurfLifecycleMutation, WorldError, WorldStage,
	},
	MixtureHandle, MAX_GAS_SLOTS,
};

fn turf(slot: u32, generation: u32) -> TurfHandle {
	TurfHandle { slot, generation }
}

fn register_turfs(world: &mut DogmosWorld, handles: &[TurfHandle]) {
	let mutations = handles
		.iter()
		.map(|handle| TurfLifecycleMutation::Register {
			handle: *handle,
			mixture: None,
		})
		.collect::<Vec<_>>();
	assert_eq!(
		world.apply_turf_lifecycle(&mutations),
		Ok(handles.len() as u32)
	);
}

fn mixture(slot: u32) -> MixtureHandle {
	MixtureHandle {
		slot,
		generation: 1,
	}
}

fn oxygen() -> GasMetadata {
	GasMetadata {
		id: GasId(0),
		key: "o2".into(),
		name: "Oxygen".into(),
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

fn diffusion_pair(
	left_moles: f32,
	left_temperature: f32,
	right_moles: f32,
	right_temperature: f32,
) -> (DogmosWorld, [TurfHandle; 2], [MixtureHandle; 2]) {
	let turfs = [turf(0, 1), turf(1, 1)];
	let mixtures = [mixture(0), mixture(1)];
	let mut world = DogmosWorld::new(1024 * 1024);
	world.install_gases(vec![oxygen()]).unwrap();
	world
		.apply_lifecycle(&mixtures.map(|handle| LifecycleMutation {
			action: LifecycleAction::Register,
			handle,
		}))
		.unwrap();
	for (handle, moles, temperature) in [
		(mixtures[0], left_moles, left_temperature),
		(mixtures[1], right_moles, right_temperature),
	] {
		let mut gases = [0.0; MAX_GAS_SLOTS];
		gases[0] = moles;
		world
			.apply_mixture_state(&[MixtureStateMutation {
				handle,
				expected_revision: 0,
				temperature,
				volume: 2500.0,
				gases,
			}])
			.unwrap();
	}
	world
		.apply_turf_lifecycle(&[
			TurfLifecycleMutation::Register {
				handle: turfs[0],
				mixture: Some(mixtures[0]),
			},
			TurfLifecycleMutation::Register {
				handle: turfs[1],
				mixture: Some(mixtures[1]),
			},
		])
		.unwrap();
	world
		.apply_turf_adjacency(&[TurfAdjacencyMutation {
			left: turfs[0],
			right: turfs[1],
			connected: true,
		}])
		.unwrap();
	(world, turfs, mixtures)
}

fn run_diffusion_stage(world: &mut DogmosWorld, frontier: &[TurfHandle]) {
	world.begin_frontier(1, frontier.len() as u32).unwrap();
	world.append_frontier(1, 0, frontier).unwrap();
	world.commit_frontier(1).unwrap();
	let request = StageChunkRequest {
		stage: WorldStage::ProcessTurfs,
		frontier_epoch: 1,
		stage_epoch: 1,
		work_limit: 16,
		seconds_per_tick: 0.5,
	};
	for _ in 0..4 {
		if !world
			.process_stage_chunk_cancellable(request, || false)
			.unwrap()
			.pending
		{
			return;
		}
	}
	panic!("diffusion stage did not complete within four chunks");
}

#[test]
fn frontier_commit_atomically_replaces_the_previous_snapshot() {
	let handles = [turf(0, 1), turf(1, 1), turf(2, 1)];
	let mut world = DogmosWorld::new(1024 * 1024);
	register_turfs(&mut world, &handles);

	assert_eq!(world.begin_frontier(1, 2), Ok(()));
	assert_eq!(world.append_frontier(1, 0, &handles[..2]), Ok(2));
	assert_eq!(world.commit_frontier(1), Ok(2));
	assert_eq!(world.committed_frontier_epoch(), Some(1));
	assert_eq!(world.committed_frontier(), &handles[..2]);

	assert_eq!(world.begin_frontier(2, 1), Ok(()));
	assert_eq!(world.append_frontier(2, 0, &handles[2..]), Ok(1));
	assert_eq!(world.committed_frontier_epoch(), Some(1));
	assert_eq!(world.committed_frontier(), &handles[..2]);
	assert_eq!(world.commit_frontier(2), Ok(1));
	assert_eq!(world.committed_frontier_epoch(), Some(2));
	assert_eq!(world.committed_frontier(), &handles[2..]);
}

#[test]
fn incremental_add_and_remove_mutate_the_committed_frontier_without_a_full_reupload() {
	let handles = [turf(0, 1), turf(1, 1), turf(2, 1), turf(3, 1)];
	let mut world = DogmosWorld::new(1024 * 1024);
	register_turfs(&mut world, &handles);

	// Bootstrap via the existing two-phase path, exactly as DM does for the first-ever publish.
	world.begin_frontier(1, 2).unwrap();
	world.append_frontier(1, 0, &handles[..2]).unwrap();
	world.commit_frontier(1).unwrap();
	assert_eq!(world.committed_frontier(), &handles[..2]);

	// Steady-state sync: add one, without touching the two already committed.
	assert_eq!(world.add_frontier(2, &handles[2..3]), Ok(1));
	assert_eq!(world.committed_frontier_epoch(), Some(2));
	assert_eq!(
		world.committed_frontier().iter().collect::<Vec<_>>(),
		vec![&handles[0], &handles[1], &handles[2]]
	);

	// Remove one of the originally-committed turfs, leaving the newly added one alone.
	assert_eq!(world.remove_frontier(3, &handles[..1]), Ok(1));
	assert_eq!(world.committed_frontier_epoch(), Some(3));
	assert_eq!(
		world.committed_frontier().iter().collect::<Vec<_>>(),
		vec![&handles[1], &handles[2]]
	);

	// Removing a handle that isn't present is a no-op, not an error - DM's diff is computed
	// against its own last-known snapshot and can legitimately be stale by one partial sync.
	assert_eq!(world.remove_frontier(4, &handles[3..]), Ok(0));
	assert_eq!(world.committed_frontier_epoch(), Some(4));
	assert_eq!(
		world.committed_frontier().iter().collect::<Vec<_>>(),
		vec![&handles[1], &handles[2]]
	);

	// Adding a handle already present is rejected rather than silently ignored, since DM's diff
	// should never legitimately re-add something it already knows is committed.
	assert_eq!(
		world.add_frontier(5, &handles[1..2]),
		Err(WorldError::Frontier(FrontierError::DuplicateHandle(
			handles[1]
		)))
	);
	// A rejected add must not have mutated the committed epoch or set.
	assert_eq!(world.committed_frontier_epoch(), Some(4));
	assert_eq!(
		world.committed_frontier().iter().collect::<Vec<_>>(),
		vec![&handles[1], &handles[2]]
	);

	// A non-increasing epoch is rejected exactly as the two-phase path rejects it.
	assert_eq!(
		world.add_frontier(4, &handles[3..]),
		Err(WorldError::Frontier(FrontierError::EpochConflict {
			committed: Some(4),
			uploading: None,
			requested: 4,
		}))
	);
}

#[test]
fn committed_frontier_storage_is_separate_from_upload_scratch() {
	let handle = turf(0, 1);
	let mut world = DogmosWorld::new(1024 * 1024);
	register_turfs(&mut world, &[handle]);
	world.add_frontier(1, &[handle]).unwrap();

	assert_eq!(world.frontier_upload_bytes(), 0);
	assert_eq!(world.committed_frontier(), &[handle]);
	let (committed_capacity, membership_capacity) = world.frontier_committed_capacities();
	assert!(committed_capacity >= 1);
	assert!(membership_capacity >= 1);
	assert_eq!(
		world.frontier_committed_storage_bytes_lower_bound(),
		((committed_capacity + membership_capacity) * std::mem::size_of::<TurfHandle>()) as u64
	);
}

#[test]
fn newer_begin_cancels_only_the_partial_upload() {
	let handles = [turf(0, 1), turf(1, 1)];
	let mut world = DogmosWorld::new(1024 * 1024);
	register_turfs(&mut world, &handles);
	world.begin_frontier(1, 1).unwrap();
	world.append_frontier(1, 0, &handles[..1]).unwrap();
	world.commit_frontier(1).unwrap();

	world.begin_frontier(2, 2).unwrap();
	world.append_frontier(2, 0, &handles[..1]).unwrap();
	assert_eq!(world.committed_frontier(), &handles[..1]);

	world.begin_frontier(3, 1).unwrap();
	assert_eq!(world.committed_frontier_epoch(), Some(1));
	assert_eq!(world.committed_frontier(), &handles[..1]);
	world.append_frontier(3, 0, &handles[1..]).unwrap();
	world.commit_frontier(3).unwrap();
	assert_eq!(world.committed_frontier(), &handles[1..]);
}

#[test]
fn frontier_upload_rejects_missing_overlapping_wrong_epoch_and_out_of_range_chunks() {
	let handles = [turf(0, 1), turf(1, 1)];
	let mut world = DogmosWorld::new(1024 * 1024);
	register_turfs(&mut world, &handles);
	world.begin_frontier(1, 2).unwrap();
	world.append_frontier(1, 0, &handles[..1]).unwrap();

	assert_eq!(
		world.commit_frontier(1),
		Err(WorldError::Frontier(FrontierError::Incomplete {
			epoch: 1,
			expected: 2,
			received: 1,
		}))
	);
	assert!(matches!(
		world.append_frontier(1, 0, &handles[..1]),
		Err(WorldError::Frontier(
			FrontierError::RangeAlreadyReceived { .. }
		))
	));
	assert!(matches!(
		world.append_frontier(2, 1, &handles[1..]),
		Err(WorldError::Frontier(FrontierError::EpochConflict { .. }))
	));
	assert!(matches!(
		world.append_frontier(1, 2, &handles[1..]),
		Err(WorldError::Frontier(FrontierError::RangeOutOfBounds { .. }))
	));
	assert_eq!(world.append_frontier(1, 1, &handles[1..]), Ok(1));
	assert_eq!(world.commit_frontier(1), Ok(2));
}

#[test]
fn frontier_commit_rejects_duplicate_unknown_and_stale_handles() {
	let first = turf(0, 1);
	let second = turf(1, 1);
	let mut world = DogmosWorld::new(1024 * 1024);
	register_turfs(&mut world, &[first, second]);

	world.begin_frontier(1, 2).unwrap();
	assert_eq!(
		world.append_frontier(1, 0, &[first, first]),
		Err(WorldError::Frontier(FrontierError::DuplicateHandle(first)))
	);
	assert_eq!(world.append_frontier(1, 0, &[first, second]), Ok(2));
	assert_eq!(world.commit_frontier(1), Ok(2));

	world.begin_frontier(2, 1).unwrap();
	world.append_frontier(2, 0, &[turf(9, 1)]).unwrap();
	assert_eq!(
		world.commit_frontier(2),
		Err(WorldError::UnknownTurfHandle(turf(9, 1)))
	);

	world
		.apply_turf_lifecycle(&[TurfLifecycleMutation::Unregister { handle: first }])
		.unwrap();
	let replacement = turf(0, 2);
	register_turfs(&mut world, &[replacement]);
	world.begin_frontier(3, 1).unwrap();
	world.append_frontier(3, 0, &[first]).unwrap();
	assert_eq!(
		world.commit_frontier(3),
		Err(WorldError::StaleTurfHandle {
			requested: first,
			current: 2,
		})
	);
}

#[test]
fn frontier_upload_rejects_cross_chunk_duplicates_without_consuming_the_range() {
	let handles = [turf(0, 1), turf(1, 1), turf(2, 1)];
	let mut world = DogmosWorld::new(1024 * 1024);
	register_turfs(&mut world, &handles);

	world.begin_frontier(1, 3).unwrap();
	assert_eq!(world.append_frontier(1, 2, &handles[2..]), Ok(1));
	assert_eq!(world.append_frontier(1, 0, &handles[..1]), Ok(1));
	assert_eq!(
		world.append_frontier(1, 1, &handles[..1]),
		Err(WorldError::Frontier(FrontierError::DuplicateHandle(
			handles[0]
		)))
	);
	assert_eq!(
		world.commit_frontier(1),
		Err(WorldError::Frontier(FrontierError::Incomplete {
			epoch: 1,
			expected: 3,
			received: 2,
		}))
	);
	assert_eq!(world.append_frontier(1, 1, &handles[1..2]), Ok(1));
	assert_eq!(world.commit_frontier(1), Ok(3));
	assert_eq!(world.committed_frontier(), &handles);
}

#[test]
fn empty_frontier_commits_and_removed_turfs_remain_tombstoned_in_snapshot() {
	let first = turf(0, 1);
	let mut world = DogmosWorld::new(1024 * 1024);
	register_turfs(&mut world, &[first]);
	world.begin_frontier(1, 1).unwrap();
	world.append_frontier(1, 0, &[first]).unwrap();
	world.commit_frontier(1).unwrap();
	world
		.apply_turf_lifecycle(&[TurfLifecycleMutation::Unregister { handle: first }])
		.unwrap();
	assert_eq!(world.committed_frontier(), &[first]);

	world.begin_frontier(2, 0).unwrap();
	assert_eq!(world.commit_frontier(2), Ok(0));
	assert_eq!(world.committed_frontier_epoch(), Some(2));
	assert!(world.committed_frontier().is_empty());
}

#[test]
fn frontier_count_is_bounded_by_allocated_turf_slots() {
	let mut world = DogmosWorld::new(1024 * 1024);
	register_turfs(&mut world, &[turf(0, 1)]);
	assert_eq!(
		world.begin_frontier(1, 2),
		Err(WorldError::Frontier(FrontierError::CountExceeded {
			actual: 2,
			maximum: 1,
		}))
	);
}

#[test]
fn stage_chunks_resume_only_an_identical_request_and_skip_tombstones() {
	let handles = [turf(0, 1), turf(1, 1), turf(2, 1)];
	let mut world = DogmosWorld::new(1024 * 1024);
	register_turfs(&mut world, &handles);
	world.begin_frontier(1, 3).unwrap();
	world.append_frontier(1, 0, &handles).unwrap();
	world.commit_frontier(1).unwrap();
	world
		.apply_turf_lifecycle(&[TurfLifecycleMutation::Unregister { handle: handles[1] }])
		.unwrap();
	let request = StageChunkRequest {
		stage: WorldStage::ProcessTurfs,
		frontier_epoch: 1,
		stage_epoch: 7,
		work_limit: 1,
		seconds_per_tick: 0.5,
	};
	let first = world
		.process_stage_chunk_cancellable(request, || false)
		.unwrap();
	assert!(first.pending);
	assert_eq!(first.work_items, 1);
	assert_eq!(first.remaining_estimate, 2);

	let changed = StageChunkRequest {
		stage_epoch: 8,
		..request
	};
	assert_eq!(
		world.process_stage_chunk_cancellable(changed, || false),
		Err(WorldError::StageConflict)
	);
	let mut completed = false;
	for _ in 0..4 {
		if !world
			.process_stage_chunk_cancellable(request, || false)
			.unwrap()
			.pending
		{
			completed = true;
			break;
		}
	}
	assert!(completed);
	assert_eq!(world.pending_stage_epoch(), None);
}

#[test]
fn stage_chunk_validates_frontier_epoch_and_work_limit() {
	let mut world = DogmosWorld::new(1024 * 1024);
	register_turfs(&mut world, &[turf(0, 1)]);
	world.begin_frontier(1, 1).unwrap();
	world.append_frontier(1, 0, &[turf(0, 1)]).unwrap();
	world.commit_frontier(1).unwrap();
	let request = StageChunkRequest {
		stage: WorldStage::ProcessTurfs,
		frontier_epoch: 2,
		stage_epoch: 1,
		work_limit: 1,
		seconds_per_tick: 0.5,
	};
	assert_eq!(
		world.process_stage_chunk_cancellable(request, || false),
		Err(WorldError::StageConflict)
	);
	assert_eq!(
		world.process_stage_chunk_cancellable(
			StageChunkRequest {
				frontier_epoch: 1,
				work_limit: 0,
				..request
			},
			|| false,
		),
		Err(WorldError::InvalidStageWorkLimit(0))
	);
}

#[test]
fn rejected_component_stage_aborts_and_retries_cleanly() {
	let handle = turf(0, 1);
	let mixture = mixture(0);
	let mut world = DogmosWorld::new(1024 * 1024);
	world.install_gases(vec![oxygen()]).unwrap();
	world
		.apply_lifecycle(&[LifecycleMutation {
			action: LifecycleAction::Register,
			handle: mixture,
		}])
		.unwrap();
	let mut gases = [0.0; MAX_GAS_SLOTS];
	gases[0] = 10.0;
	world
		.apply_mixture_state(&[MixtureStateMutation {
			handle: mixture,
			expected_revision: 0,
			temperature: 293.15,
			volume: 2500.0,
			gases,
		}])
		.unwrap();
	world
		.apply_turf_lifecycle(&[TurfLifecycleMutation::Register {
			handle,
			mixture: Some(mixture),
		}])
		.unwrap();
	world.begin_frontier(1, 1).unwrap();
	world.append_frontier(1, 0, &[handle]).unwrap();
	world.commit_frontier(1).unwrap();
	let request = StageChunkRequest {
		stage: WorldStage::Equalize,
		frontier_epoch: 1,
		stage_epoch: 1,
		work_limit: 1,
		seconds_per_tick: 0.5,
	};
	assert!(
		world
			.process_stage_chunk_cancellable(request, || false)
			.unwrap()
			.pending
	);
	let retry = StageChunkRequest {
		stage_epoch: 2,
		..request
	};
	assert_eq!(
		world.process_stage_chunk_cancellable(retry, || false),
		Err(WorldError::StageConflict)
	);
	assert_eq!(world.pending_stage_epoch(), None);
	for _ in 0..8 {
		if !world
			.process_stage_chunk_cancellable(retry, || false)
			.unwrap()
			.pending
		{
			break;
		}
	}
	assert_eq!(world.pending_stage_epoch(), None);

	let cancelled = StageChunkRequest {
		stage_epoch: 3,
		..request
	};
	assert!(
		world
			.process_stage_chunk_cancellable(cancelled, || false)
			.unwrap()
			.pending
	);
	let mut cancellation_observed = false;
	for _ in 0..4 {
		match world.process_stage_chunk_cancellable(cancelled, || true) {
			Err(WorldError::Cancelled) => {
				cancellation_observed = true;
				break;
			}
			Ok(chunk) => assert!(chunk.pending),
			other => panic!("unexpected component-stage cancellation result: {other:?}"),
		}
	}
	assert!(cancellation_observed);
	assert_eq!(world.pending_stage_epoch(), None);
	let final_retry = StageChunkRequest {
		stage_epoch: 4,
		..request
	};
	for _ in 0..8 {
		if !world
			.process_stage_chunk_cancellable(final_retry, || false)
			.unwrap()
			.pending
		{
			break;
		}
	}
	assert_eq!(world.pending_stage_epoch(), None);
}

#[test]
fn chunked_turf_heat_is_confined_to_the_committed_frontier() {
	let active = turf(0, 1);
	let inactive = turf(1, 1);
	let mut world = DogmosWorld::new(1024 * 1024);
	register_turfs(&mut world, &[active, inactive]);
	let state = TurfHeatState {
		temperature: 500.0,
		thermal_conductivity: 1.0,
		heat_capacity: 100.0,
		adjacent_to_space: true,
	};
	world
		.apply_turf_heat(&[
			TurfHeatMutation {
				handle: active,
				state: Some(state),
			},
			TurfHeatMutation {
				handle: inactive,
				state: Some(state),
			},
		])
		.unwrap();
	world.begin_frontier(1, 1).unwrap();
	world.append_frontier(1, 0, &[active]).unwrap();
	world.commit_frontier(1).unwrap();
	let request = StageChunkRequest {
		stage: WorldStage::TurfHeat,
		frontier_epoch: 1,
		stage_epoch: 1,
		work_limit: 1,
		seconds_per_tick: 0.5,
	};
	let mut completed = false;
	for _ in 0..8 {
		let chunk = world
			.process_stage_chunk_cancellable(request, || false)
			.unwrap();
		assert!(chunk.work_items <= 1);
		if !chunk.pending {
			completed = true;
			break;
		}
	}
	assert!(completed);
	assert_ne!(world.turf_heat(active).unwrap(), Some(state));
	assert_eq!(world.turf_heat(inactive).unwrap(), Some(state));
}

#[test]
fn chunked_turf_heat_includes_conductive_neighbors_of_the_committed_frontier() {
	let hot = turf(0, 1);
	let cold = turf(1, 1);
	let mut world = DogmosWorld::new(1024 * 1024);
	register_turfs(&mut world, &[hot, cold]);
	let states = [700.0, 293.15].map(|temperature| TurfHeatState {
		temperature,
		thermal_conductivity: 0.05,
		heat_capacity: 20_000.0,
		adjacent_to_space: false,
	});
	world
		.apply_turf_heat(&[
			TurfHeatMutation {
				handle: hot,
				state: Some(states[0]),
			},
			TurfHeatMutation {
				handle: cold,
				state: Some(states[1]),
			},
		])
		.unwrap();
	world
		.apply_turf_heat_adjacency(&[TurfHeatAdjacencyMutation {
			left: hot,
			right: cold,
			connected: true,
		}])
		.unwrap();
	world.begin_frontier(1, 1).unwrap();
	world.append_frontier(1, 0, &[hot]).unwrap();
	world.commit_frontier(1).unwrap();
	let request = StageChunkRequest {
		stage: WorldStage::TurfHeat,
		frontier_epoch: 1,
		stage_epoch: 1,
		work_limit: 1,
		seconds_per_tick: 0.5,
	};

	for _ in 0..16 {
		if !world
			.process_stage_chunk_cancellable(request, || false)
			.unwrap()
			.pending
		{
			break;
		}
	}

	assert!(world.turf_heat(hot).unwrap().unwrap().temperature < states[0].temperature);
	assert!(world.turf_heat(cold).unwrap().unwrap().temperature > states[1].temperature);
}

#[test]
fn frontier_inspection_cannot_hide_process_turfs_kernel_work() {
	let turf = turf(0, 1);
	let mixture = mixture(0);
	let mut world = DogmosWorld::new(1024 * 1024);
	world
		.apply_lifecycle(&[LifecycleMutation {
			action: LifecycleAction::Register,
			handle: mixture,
		}])
		.unwrap();
	let mut gases = [0.0; MAX_GAS_SLOTS];
	gases[0] = 10.0;
	world
		.apply_mixture_state(&[MixtureStateMutation {
			handle: mixture,
			expected_revision: 0,
			temperature: 293.15,
			volume: 2500.0,
			gases,
		}])
		.unwrap();
	world
		.apply_turf_lifecycle(&[TurfLifecycleMutation::Register {
			handle: turf,
			mixture: Some(mixture),
		}])
		.unwrap();
	world.begin_frontier(1, 1).unwrap();
	world.append_frontier(1, 0, &[turf]).unwrap();
	world.commit_frontier(1).unwrap();
	let request = StageChunkRequest {
		stage: WorldStage::ProcessTurfs,
		frontier_epoch: 1,
		stage_epoch: 1,
		work_limit: 1,
		seconds_per_tick: 0.5,
	};

	let preparation = world
		.process_stage_chunk_cancellable(request, || false)
		.unwrap();
	assert!(preparation.pending);
	assert_eq!(preparation.work_items, 1);
	assert_eq!(world.snapshot(mixture).unwrap().revision, 1);

	let kernel = world
		.process_stage_chunk_cancellable(request, || false)
		.unwrap();
	assert!(!kernel.pending);
	assert_eq!(kernel.work_items, 1);
	assert_eq!(world.snapshot(mixture).unwrap().revision, 2);
}

#[test]
fn process_turfs_computes_at_most_one_frontier_node_per_unit_of_work() {
	let turfs = [turf(0, 1), turf(1, 1)];
	let mixtures = [mixture(0), mixture(1)];
	let mut world = DogmosWorld::new(1024 * 1024);
	world
		.apply_lifecycle(&mixtures.map(|handle| LifecycleMutation {
			action: LifecycleAction::Register,
			handle,
		}))
		.unwrap();
	for (index, handle) in mixtures.into_iter().enumerate() {
		let mut gases = [0.0; MAX_GAS_SLOTS];
		gases[0] = if index == 0 { 16.0 } else { 0.0 };
		world
			.apply_mixture_state(&[MixtureStateMutation {
				handle,
				expected_revision: 0,
				temperature: 293.15,
				volume: 2500.0,
				gases,
			}])
			.unwrap();
	}
	world
		.apply_turf_lifecycle(&[
			TurfLifecycleMutation::Register {
				handle: turfs[0],
				mixture: Some(mixtures[0]),
			},
			TurfLifecycleMutation::Register {
				handle: turfs[1],
				mixture: Some(mixtures[1]),
			},
		])
		.unwrap();
	world
		.apply_turf_adjacency(&[TurfAdjacencyMutation {
			left: turfs[0],
			right: turfs[1],
			connected: true,
		}])
		.unwrap();
	world.begin_frontier(1, 2).unwrap();
	world.append_frontier(1, 0, &turfs).unwrap();
	world.commit_frontier(1).unwrap();
	let request = StageChunkRequest {
		stage: WorldStage::ProcessTurfs,
		frontier_epoch: 1,
		stage_epoch: 1,
		work_limit: 1,
		seconds_per_tick: 0.5,
	};

	for _ in 0..3 {
		let chunk = world
			.process_stage_chunk_cancellable(request, || false)
			.unwrap();
		assert!(chunk.pending);
		assert_eq!(chunk.work_items, 1);
		assert_eq!(world.snapshot(mixtures[0]).unwrap().revision, 1);
		assert_eq!(world.snapshot(mixtures[1]).unwrap().revision, 1);
	}
	let final_chunk = world
		.process_stage_chunk_cancellable(request, || false)
		.unwrap();
	assert!(!final_chunk.pending);
	assert_eq!(final_chunk.work_items, 1);
	assert_eq!(world.snapshot(mixtures[0]).unwrap().revision, 2);
	assert_eq!(world.snapshot(mixtures[1]).unwrap().revision, 2);
	assert_eq!(world.snapshot(mixtures[0]).unwrap().gases[0], 14.0);
	assert_eq!(world.snapshot(mixtures[1]).unwrap().gases[0], 2.0);
}

#[test]
fn process_turfs_diffuses_into_an_inactive_mutable_neighbor() {
	let (mut world, turfs, mixtures) = diffusion_pair(16.0, 500.0, 0.0, 300.0);

	run_diffusion_stage(&mut world, &turfs[..1]);

	let left = world.snapshot(mixtures[0]).unwrap();
	let right = world.snapshot(mixtures[1]).unwrap();
	assert_eq!(left.gases[0], 14.0);
	assert_eq!(right.gases[0], 2.0);
	assert_eq!(left.gases[0] + right.gases[0], 16.0);
}

#[test]
fn process_turfs_drains_into_an_inactive_immutable_boundary() {
	let (mut world, turfs, mixtures) = diffusion_pair(16.0, 500.0, 0.0, 2.7);
	world
		.apply_command(Command::MarkImmutable {
			handle: mixtures[1],
		})
		.unwrap();

	run_diffusion_stage(&mut world, &turfs[..1]);

	let interior = world.snapshot(mixtures[0]).unwrap();
	let boundary = world.snapshot(mixtures[1]).unwrap();
	assert_eq!(interior.gases[0], 14.0);
	assert_eq!(interior.temperature, 500.0);
	assert_eq!(boundary.gases[0], 0.0);
	assert_eq!(boundary.temperature, 2.7);
}

#[test]
fn process_turfs_transports_thermal_energy_with_diffusing_gas() {
	let (mut world, turfs, mixtures) = diffusion_pair(10.0, 500.0, 10.0, 300.0);

	run_diffusion_stage(&mut world, &turfs);

	let left = world.snapshot(mixtures[0]).unwrap();
	let right = world.snapshot(mixtures[1]).unwrap();
	assert_eq!(left.gases[0], 10.0);
	assert_eq!(right.gases[0], 10.0);
	assert_eq!(left.temperature, 475.0);
	assert_eq!(right.temperature, 325.0);
	let total_energy =
		left.heat_capacity * left.temperature + right.heat_capacity * right.temperature;
	assert_eq!(total_energy, 160_000.0);
}

#[test]
fn turf_heat_processes_at_most_one_frontier_node_per_unit_of_work() {
	let turfs = [turf(0, 1), turf(1, 1)];
	let mut world = DogmosWorld::new(1024 * 1024);
	register_turfs(&mut world, &turfs);
	let state = TurfHeatState {
		temperature: 500.0,
		thermal_conductivity: 1.0,
		heat_capacity: 100.0,
		adjacent_to_space: true,
	};
	world
		.apply_turf_heat(&turfs.map(|handle| TurfHeatMutation {
			handle,
			state: Some(state),
		}))
		.unwrap();
	world.begin_frontier(1, 2).unwrap();
	world.append_frontier(1, 0, &turfs).unwrap();
	world.commit_frontier(1).unwrap();
	let request = StageChunkRequest {
		stage: WorldStage::TurfHeat,
		frontier_epoch: 1,
		stage_epoch: 1,
		work_limit: 1,
		seconds_per_tick: 0.5,
	};

	let mut completed = false;
	for _ in 0..16 {
		let chunk = world
			.process_stage_chunk_cancellable(request, || false)
			.unwrap();
		assert!(chunk.work_items <= 1);
		if !chunk.pending {
			completed = true;
			break;
		}
		assert_eq!(world.turf_heat(turfs[0]).unwrap(), Some(state));
		assert_eq!(world.turf_heat(turfs[1]).unwrap(), Some(state));
	}
	assert!(completed);
	assert_ne!(world.turf_heat(turfs[0]).unwrap(), Some(state));
	assert_ne!(world.turf_heat(turfs[1]).unwrap(), Some(state));
}

#[test]
fn reactions_inspect_at_most_one_frontier_target_per_unit_of_work() {
	let turfs = [turf(0, 1), turf(1, 1)];
	let mixtures = [mixture(0), mixture(1)];
	let mut world = DogmosWorld::new(1024 * 1024);
	world.install_gases(Vec::new()).unwrap();
	world.install_reactions(Vec::new()).unwrap();
	world
		.apply_lifecycle(&mixtures.map(|handle| LifecycleMutation {
			action: LifecycleAction::Register,
			handle,
		}))
		.unwrap();
	world
		.apply_turf_lifecycle(&[
			TurfLifecycleMutation::Register {
				handle: turfs[0],
				mixture: Some(mixtures[0]),
			},
			TurfLifecycleMutation::Register {
				handle: turfs[1],
				mixture: Some(mixtures[1]),
			},
		])
		.unwrap();
	world.begin_frontier(1, 2).unwrap();
	world.append_frontier(1, 0, &turfs).unwrap();
	world.commit_frontier(1).unwrap();
	let request = StageChunkRequest {
		stage: WorldStage::React,
		frontier_epoch: 1,
		stage_epoch: 1,
		work_limit: 1,
		seconds_per_tick: 0.5,
	};

	for _ in 0..3 {
		let chunk = world
			.process_stage_chunk_cancellable(request, || false)
			.unwrap();
		assert!(chunk.pending);
		assert_eq!(chunk.work_items, 1);
	}
	let final_chunk = world
		.process_stage_chunk_cancellable(request, || false)
		.unwrap();
	assert!(!final_chunk.pending);
	assert_eq!(final_chunk.work_items, 1);
}

#[test]
fn turf_heat_charges_topology_visits_and_conduction_edges_to_the_work_limit() {
	let turfs = [turf(0, 1), turf(1, 1)];
	let mut world = DogmosWorld::new(1024 * 1024);
	register_turfs(&mut world, &turfs);
	let states = [500.0, 300.0].map(|temperature| TurfHeatState {
		temperature,
		thermal_conductivity: 0.1,
		heat_capacity: 100.0,
		adjacent_to_space: false,
	});
	world
		.apply_turf_heat(&[
			TurfHeatMutation {
				handle: turfs[0],
				state: Some(states[0]),
			},
			TurfHeatMutation {
				handle: turfs[1],
				state: Some(states[1]),
			},
		])
		.unwrap();
	world
		.apply_turf_heat_adjacency(&[TurfHeatAdjacencyMutation {
			left: turfs[0],
			right: turfs[1],
			connected: true,
		}])
		.unwrap();
	world.begin_frontier(1, 2).unwrap();
	world.append_frontier(1, 0, &turfs).unwrap();
	world.commit_frontier(1).unwrap();
	let request = StageChunkRequest {
		stage: WorldStage::TurfHeat,
		frontier_epoch: 1,
		stage_epoch: 1,
		work_limit: 1,
		seconds_per_tick: 0.5,
	};

	for _ in 0..6 {
		let chunk = world
			.process_stage_chunk_cancellable(request, || false)
			.unwrap();
		assert!(chunk.pending);
		assert_eq!(chunk.work_items, 1);
		assert_eq!(world.turf_heat(turfs[0]).unwrap(), Some(states[0]));
		assert_eq!(world.turf_heat(turfs[1]).unwrap(), Some(states[1]));
	}
	let final_chunk = world
		.process_stage_chunk_cancellable(request, || false)
		.unwrap();
	assert!(!final_chunk.pending);
	assert_eq!(final_chunk.work_items, 1);
	assert!(world.turf_heat(turfs[0]).unwrap().unwrap().temperature < 500.0);
	assert!(world.turf_heat(turfs[1]).unwrap().unwrap().temperature > 300.0);
}

#[test]
fn equalize_never_exceeds_the_chunk_work_limit_and_preserves_the_event_transcript() {
	let turfs = [turf(0, 1), turf(1, 1)];
	let mixtures = [mixture(0), mixture(1)];
	let mut world = DogmosWorld::new_with_event_capacity(1024 * 1024, 8);
	world.install_gases(vec![oxygen()]).unwrap();
	world
		.apply_lifecycle(&mixtures.map(|handle| LifecycleMutation {
			action: LifecycleAction::Register,
			handle,
		}))
		.unwrap();
	for (index, handle) in mixtures.into_iter().enumerate() {
		let mut gases = [0.0; MAX_GAS_SLOTS];
		gases[0] = if index == 0 { 100.0 } else { 0.0 };
		world
			.apply_mixture_state(&[MixtureStateMutation {
				handle,
				expected_revision: 0,
				temperature: 293.15,
				volume: 2500.0,
				gases,
			}])
			.unwrap();
	}
	world
		.apply_turf_lifecycle(&[
			TurfLifecycleMutation::Register {
				handle: turfs[0],
				mixture: Some(mixtures[0]),
			},
			TurfLifecycleMutation::Register {
				handle: turfs[1],
				mixture: Some(mixtures[1]),
			},
		])
		.unwrap();
	world
		.apply_turf_adjacency(&[TurfAdjacencyMutation {
			left: turfs[0],
			right: turfs[1],
			connected: true,
		}])
		.unwrap();
	world.begin_frontier(1, 2).unwrap();
	world.append_frontier(1, 0, &turfs).unwrap();
	world.commit_frontier(1).unwrap();
	let request = StageChunkRequest {
		stage: WorldStage::Equalize,
		frontier_epoch: 1,
		stage_epoch: 1,
		work_limit: 1,
		seconds_per_tick: 0.5,
	};

	let mut completed = false;
	for _ in 0..32 {
		let chunk = world
			.process_stage_chunk_cancellable_with_event_limit(request, 1, || false)
			.unwrap();
		assert!(chunk.work_items <= 1);
		if !chunk.pending {
			completed = true;
			break;
		}
	}
	assert!(completed);
	assert_eq!(world.snapshot(mixtures[0]).unwrap().gases[0], 50.0);
	assert_eq!(world.snapshot(mixtures[1]).unwrap().gases[0], 50.0);
	let mut events = Vec::new();
	world.drain_events_into(8, &mut events);
	assert_eq!(
		events,
		vec![dogmos_core::world::WorldEvent::PressureDifference {
			source: turfs[0],
			target: turfs[1],
			moles: 50.0,
		}]
	);
}

#[test]
fn component_stage_commits_disconnected_components_before_resume() {
	let turfs = [turf(0, 1), turf(1, 1), turf(2, 1), turf(3, 1)];
	let mixtures = [mixture(0), mixture(1), mixture(2), mixture(3)];
	let mut world = DogmosWorld::new(1024 * 1024);
	world.install_gases(vec![oxygen()]).unwrap();
	world
		.apply_lifecycle(&mixtures.map(|handle| LifecycleMutation {
			action: LifecycleAction::Register,
			handle,
		}))
		.unwrap();
	for (index, handle) in mixtures.into_iter().enumerate() {
		let mut gases = [0.0; MAX_GAS_SLOTS];
		gases[0] = if index % 2 == 0 { 100.0 } else { 0.0 };
		world
			.apply_mixture_state(&[MixtureStateMutation {
				handle,
				expected_revision: 0,
				temperature: 293.15,
				volume: 2500.0,
				gases,
			}])
			.unwrap();
	}
	world
		.apply_turf_lifecycle(
			&turfs
				.into_iter()
				.zip(mixtures)
				.map(|(handle, mixture)| TurfLifecycleMutation::Register {
					handle,
					mixture: Some(mixture),
				})
				.collect::<Vec<_>>(),
		)
		.unwrap();
	world
		.apply_turf_adjacency(&[
			TurfAdjacencyMutation {
				left: turfs[0],
				right: turfs[1],
				connected: true,
			},
			TurfAdjacencyMutation {
				left: turfs[2],
				right: turfs[3],
				connected: true,
			},
		])
		.unwrap();
	world.begin_frontier(1, 4).unwrap();
	world.append_frontier(1, 0, &turfs).unwrap();
	world.commit_frontier(1).unwrap();
	let request = StageChunkRequest {
		stage: WorldStage::Equalize,
		frontier_epoch: 1,
		stage_epoch: 1,
		work_limit: 1,
		seconds_per_tick: 0.5,
	};

	for _ in 0..9 {
		assert!(
			world
				.process_stage_chunk_cancellable(request, || false)
				.unwrap()
				.pending
		);
	}
	let first_snapshot = world.snapshot(mixtures[0]).unwrap();
	let mut externally_mutated_gases = first_snapshot.gases;
	externally_mutated_gases[0] = 75.0;
	world
		.apply_mixture_state(&[MixtureStateMutation {
			handle: mixtures[0],
			expected_revision: first_snapshot.revision,
			temperature: first_snapshot.temperature,
			volume: first_snapshot.volume,
			gases: externally_mutated_gases,
		}])
		.unwrap();

	let mut completed = false;
	for _ in 0..16 {
		let chunk = world
			.process_stage_chunk_cancellable(request, || false)
			.unwrap();
		if !chunk.pending {
			completed = true;
			break;
		}
	}

	assert!(completed);
	assert_eq!(world.snapshot(mixtures[0]).unwrap().gases[0], 75.0);
	assert_eq!(world.snapshot(mixtures[1]).unwrap().gases[0], 50.0);
	assert_eq!(world.snapshot(mixtures[2]).unwrap().gases[0], 50.0);
	assert_eq!(world.snapshot(mixtures[3]).unwrap().gases[0], 50.0);
}

#[test]
fn equalize_retains_earlier_components_when_a_later_component_overflows_events() {
	let turfs = [turf(0, 1), turf(1, 1), turf(2, 1), turf(3, 1)];
	let mixtures = [mixture(0), mixture(1), mixture(2), mixture(3)];
	let mut world = DogmosWorld::new_with_event_capacity(1024 * 1024, 1);
	world.install_gases(vec![oxygen()]).unwrap();
	world
		.apply_lifecycle(&mixtures.map(|handle| LifecycleMutation {
			action: LifecycleAction::Register,
			handle,
		}))
		.unwrap();
	for (index, handle) in mixtures.into_iter().enumerate() {
		let mut gases = [0.0; MAX_GAS_SLOTS];
		gases[0] = if index % 2 == 0 { 100.0 } else { 0.0 };
		world
			.apply_mixture_state(&[MixtureStateMutation {
				handle,
				expected_revision: 0,
				temperature: 293.15,
				volume: 2500.0,
				gases,
			}])
			.unwrap();
	}
	world
		.apply_turf_lifecycle(
			&turfs
				.into_iter()
				.zip(mixtures)
				.map(|(handle, mixture)| TurfLifecycleMutation::Register {
					handle,
					mixture: Some(mixture),
				})
				.collect::<Vec<_>>(),
		)
		.unwrap();
	world
		.apply_turf_adjacency(&[
			TurfAdjacencyMutation {
				left: turfs[0],
				right: turfs[1],
				connected: true,
			},
			TurfAdjacencyMutation {
				left: turfs[2],
				right: turfs[3],
				connected: true,
			},
		])
		.unwrap();
	world.begin_frontier(1, 4).unwrap();
	world.append_frontier(1, 0, &turfs).unwrap();
	world.commit_frontier(1).unwrap();
	let request = StageChunkRequest {
		stage: WorldStage::Equalize,
		frontier_epoch: 1,
		stage_epoch: 1,
		work_limit: 1,
		seconds_per_tick: 0.5,
	};

	let error = (0..64)
		.find_map(|_| {
			world
				.process_stage_chunk_cancellable(request, || false)
				.err()
		})
		.expect("the combined event transcript must exceed capacity");
	assert_eq!(
		error,
		WorldError::EventCapacityExceeded {
			requested: 2,
			capacity: 1,
		}
	);
	assert_eq!(world.snapshot(mixtures[0]).unwrap().gases[0], 50.0);
	assert_eq!(world.snapshot(mixtures[1]).unwrap().gases[0], 50.0);
	assert_eq!(world.snapshot(mixtures[2]).unwrap().gases[0], 100.0);
	assert_eq!(world.snapshot(mixtures[3]).unwrap().gases[0], 0.0);
}

#[test]
fn equalize_rejects_a_mutable_mixture_shared_by_disconnected_components() {
	let turfs = [turf(0, 1), turf(1, 1), turf(2, 1), turf(3, 1)];
	let mixtures = [mixture(0), mixture(1), mixture(2)];
	let mut world = DogmosWorld::new(1024 * 1024);
	world.install_gases(vec![oxygen()]).unwrap();
	world
		.apply_lifecycle(&mixtures.map(|handle| LifecycleMutation {
			action: LifecycleAction::Register,
			handle,
		}))
		.unwrap();
	for (index, handle) in mixtures.into_iter().enumerate() {
		let mut gases = [0.0; MAX_GAS_SLOTS];
		gases[0] = 100.0 * (index + 1) as f32;
		world
			.apply_mixture_state(&[MixtureStateMutation {
				handle,
				expected_revision: 0,
				temperature: 293.15,
				volume: 2500.0,
				gases,
			}])
			.unwrap();
	}
	world
		.apply_turf_lifecycle(&[
			TurfLifecycleMutation::Register {
				handle: turfs[0],
				mixture: Some(mixtures[0]),
			},
			TurfLifecycleMutation::Register {
				handle: turfs[1],
				mixture: Some(mixtures[1]),
			},
			TurfLifecycleMutation::Register {
				handle: turfs[2],
				mixture: Some(mixtures[1]),
			},
			TurfLifecycleMutation::Register {
				handle: turfs[3],
				mixture: Some(mixtures[2]),
			},
		])
		.unwrap();
	world
		.apply_turf_adjacency(&[
			TurfAdjacencyMutation {
				left: turfs[0],
				right: turfs[1],
				connected: true,
			},
			TurfAdjacencyMutation {
				left: turfs[2],
				right: turfs[3],
				connected: true,
			},
		])
		.unwrap();
	world.begin_frontier(1, 4).unwrap();
	world.append_frontier(1, 0, &turfs).unwrap();
	world.commit_frontier(1).unwrap();
	let request = StageChunkRequest {
		stage: WorldStage::Equalize,
		frontier_epoch: 1,
		stage_epoch: 1,
		work_limit: 1,
		seconds_per_tick: 0.5,
	};

	let error = (0..64)
		.find_map(|_| {
			world
				.process_stage_chunk_cancellable(request, || false)
				.err()
		})
		.expect("the shared mutable mixture must be rejected");

	assert_eq!(error, WorldError::DuplicateMutableTurfMixture(mixtures[1]));
	assert_eq!(world.snapshot(mixtures[0]).unwrap().gases[0], 150.0);
	assert_eq!(world.snapshot(mixtures[1]).unwrap().gases[0], 150.0);
	assert_eq!(world.snapshot(mixtures[2]).unwrap().gases[0], 300.0);
}

#[test]
fn excited_groups_rejects_a_mutable_mixture_shared_by_disconnected_components() {
	let turfs = [turf(0, 1), turf(1, 1), turf(2, 1), turf(3, 1)];
	let mixtures = [mixture(0), mixture(1), mixture(2)];
	let mut world = DogmosWorld::new(1024 * 1024);
	world.install_gases(vec![oxygen()]).unwrap();
	world
		.apply_lifecycle(&mixtures.map(|handle| LifecycleMutation {
			action: LifecycleAction::Register,
			handle,
		}))
		.unwrap();
	for (index, handle) in mixtures.into_iter().enumerate() {
		let mut gases = [0.0; MAX_GAS_SLOTS];
		gases[0] = 100.0 + index as f32 * 0.1;
		world
			.apply_mixture_state(&[MixtureStateMutation {
				handle,
				expected_revision: 0,
				temperature: 293.15,
				volume: 2500.0,
				gases,
			}])
			.unwrap();
	}
	world
		.apply_turf_lifecycle(&[
			TurfLifecycleMutation::Register {
				handle: turfs[0],
				mixture: Some(mixtures[0]),
			},
			TurfLifecycleMutation::Register {
				handle: turfs[1],
				mixture: Some(mixtures[1]),
			},
			TurfLifecycleMutation::Register {
				handle: turfs[2],
				mixture: Some(mixtures[1]),
			},
			TurfLifecycleMutation::Register {
				handle: turfs[3],
				mixture: Some(mixtures[2]),
			},
		])
		.unwrap();
	world
		.apply_turf_adjacency(&[
			TurfAdjacencyMutation {
				left: turfs[0],
				right: turfs[1],
				connected: true,
			},
			TurfAdjacencyMutation {
				left: turfs[2],
				right: turfs[3],
				connected: true,
			},
		])
		.unwrap();
	world.begin_frontier(1, 4).unwrap();
	world.append_frontier(1, 0, &turfs).unwrap();
	world.commit_frontier(1).unwrap();
	let request = StageChunkRequest {
		stage: WorldStage::ExcitedGroups,
		frontier_epoch: 1,
		stage_epoch: 1,
		work_limit: 1,
		seconds_per_tick: 0.5,
	};

	let error = (0..64)
		.find_map(|_| {
			world
				.process_stage_chunk_cancellable(request, || false)
				.err()
		})
		.expect("the shared mutable mixture must be rejected");

	assert_eq!(error, WorldError::DuplicateMutableTurfMixture(mixtures[1]));
	assert_eq!(world.snapshot(mixtures[0]).unwrap().gases[0], 100.05);
	assert_eq!(world.snapshot(mixtures[1]).unwrap().gases[0], 100.05);
	assert_eq!(world.snapshot(mixtures[2]).unwrap().gases[0], 100.2);
}
