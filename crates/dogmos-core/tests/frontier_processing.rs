use dogmos_core::{
	metadata::{GasFireRole, GasId, GasMetadata, TurfHandle},
	world::{
		DogmosWorld, FrontierError, LifecycleAction, LifecycleMutation, MixtureStateMutation,
		StageChunkRequest, TurfAdjacencyMutation, TurfHeatAdjacencyMutation, TurfHeatMutation,
		TurfHeatState, TurfLifecycleMutation, WorldError, WorldStage,
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
	world.append_frontier(1, 0, &[first, first]).unwrap();
	assert_eq!(
		world.commit_frontier(1),
		Err(WorldError::Frontier(FrontierError::DuplicateHandle(first)))
	);

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
	let second = world
		.process_stage_chunk_cancellable(request, || false)
		.unwrap();
	assert!(second.pending);
	let final_chunk = world
		.process_stage_chunk_cancellable(request, || false)
		.unwrap();
	assert!(!final_chunk.pending);
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
			.process_stage_chunk_cancellable(request, || false)
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
fn equalize_rolls_back_earlier_components_when_a_later_component_overflows_events() {
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
	for (index, mixture) in mixtures.into_iter().enumerate() {
		assert_eq!(
			world.snapshot(mixture).unwrap().gases[0],
			if index % 2 == 0 { 100.0 } else { 0.0 }
		);
	}
}
