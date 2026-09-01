use dogmos_core::metadata::{GasFireRole, GasId, GasMetadata, TurfHandle};
use dogmos_core::world::{
	AdjacencyMutation, Command, CommandResult, DogmosWorld, LifecycleAction, LifecycleMutation,
	MixtureStateMutation, TurfAdjacencyMutation, TurfFirelockMutation, TurfHeatAdjacencyMutation,
	TurfHeatMutation, TurfHeatState, TurfLifecycleMutation, WorldError, WorldEvent, WorldStage,
};
use dogmos_core::{MixtureHandle, MAX_GAS_SLOTS};

fn handle(slot: u32, generation: u32) -> MixtureHandle {
	MixtureHandle { slot, generation }
}

fn turf_handle(slot: u32, generation: u32) -> TurfHandle {
	TurfHandle { slot, generation }
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
fn turf_heat_snapshot_is_generation_checked_and_reports_absence() {
	let mut world = DogmosWorld::new(1024 * 1024);
	let turf = turf_handle(7, 3);
	world
		.apply_turf_lifecycle(&[TurfLifecycleMutation::Register {
			handle: turf,
			mixture: None,
		}])
		.unwrap();
	assert_eq!(world.turf_heat(turf).unwrap(), None);

	let state = TurfHeatState {
		temperature: 700.0,
		thermal_conductivity: 0.4,
		heat_capacity: 2500.0,
		adjacent_to_space: true,
	};
	world
		.apply_turf_heat(&[TurfHeatMutation {
			handle: turf,
			state: Some(state),
		}])
		.unwrap();
	assert_eq!(world.turf_heat(turf).unwrap(), Some(state));
	assert!(matches!(
		world.turf_heat(turf_handle(7, 2)),
		Err(WorldError::StaleTurfHandle { .. })
	));
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
	assert_eq!(empty.volume, 2500.0);
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
fn unimplemented_stages_do_not_report_synthetic_work() {
	let mut world = DogmosWorld::new(1024 * 1024);
	world
		.apply_lifecycle(&[LifecycleMutation {
			action: LifecycleAction::Register,
			handle: handle(0, 1),
		}])
		.unwrap();

	assert_eq!(
		world.process_stage_cancellable(WorldStage::React, 0.5, || false),
		Err(WorldError::GasRegistryMissing)
	);
	assert_eq!(world.snapshot(handle(0, 1)).unwrap().revision, 0);
	assert_eq!(
		world.process_stage_cancellable(WorldStage::ExcitedGroups, 0.5, || false),
		Ok(dogmos_core::world::StageResult { work_items: 0 })
	);
	assert_eq!(
		world.process_stage_cancellable(WorldStage::Equalize, 0.5, || false),
		Ok(dogmos_core::world::StageResult { work_items: 0 })
	);
	assert_eq!(
		world.process_stage_cancellable(WorldStage::TurfHeat, 0.5, || false),
		Ok(dogmos_core::world::StageResult { work_items: 0 })
	);
}

#[test]
fn turf_lifecycle_owns_generation_and_detaches_removed_mixtures() {
	let mixture = handle(0, 1);
	let turf = turf_handle(7, 2);
	let mut world = DogmosWorld::new(1024 * 1024);
	world
		.apply_lifecycle(&[LifecycleMutation {
			action: LifecycleAction::Register,
			handle: mixture,
		}])
		.unwrap();
	assert_eq!(
		world
			.apply_turf_lifecycle(&[TurfLifecycleMutation::Register {
				handle: turf,
				mixture: Some(mixture),
			}])
			.unwrap(),
		1
	);
	assert_eq!(world.turf_mixture(turf), Ok(Some(mixture)));

	world
		.apply_lifecycle(&[LifecycleMutation {
			action: LifecycleAction::Unregister,
			handle: mixture,
		}])
		.unwrap();
	assert_eq!(world.turf_mixture(turf), Ok(None));

	world
		.apply_turf_lifecycle(&[TurfLifecycleMutation::Unregister { handle: turf }])
		.unwrap();
	assert!(matches!(
		world.turf_mixture(turf),
		Err(WorldError::UnknownTurfHandle(_))
	));
	assert!(matches!(
		world.apply_turf_lifecycle(&[TurfLifecycleMutation::Register {
			handle: turf_handle(7, 1),
			mixture: None,
		}]),
		Err(WorldError::StaleTurfHandle { current: 2, .. })
	));
	world
		.apply_turf_lifecycle(&[TurfLifecycleMutation::Register {
			handle: turf_handle(7, 3),
			mixture: None,
		}])
		.unwrap();
	assert_eq!(world.turf_mixture(turf_handle(7, 3)), Ok(None));
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

#[test]
fn commands_match_legacy_scalar_rules_and_immutable_mixtures_do_not_change() {
	let mixture = handle(0, 1);
	let mut world = DogmosWorld::new(1024 * 1024);
	world.install_gases(vec![oxygen()]).unwrap();
	world
		.apply_lifecycle(&[LifecycleMutation {
			action: LifecycleAction::Register,
			handle: mixture,
		}])
		.unwrap();

	assert_eq!(
		world.apply_command(Command::SetMoles {
			handle: mixture,
			gas: GasId(0),
			amount: 12.0,
		}),
		Ok(CommandResult::Applied { updated: 1 })
	);
	world
		.apply_command(Command::AdjustMoles {
			handle: mixture,
			gas: GasId(0),
			delta: -20.0,
		})
		.unwrap();
	world
		.apply_command(Command::SetTemperature {
			handle: mixture,
			temperature: 1.0,
		})
		.unwrap();
	world
		.apply_command(Command::SetVolume {
			handle: mixture,
			volume: 1000.0,
		})
		.unwrap();
	let before_immutable = world.snapshot(mixture).unwrap();
	assert_eq!(before_immutable.gases[0], 0.0);
	assert_eq!(before_immutable.temperature, 2.7);
	assert_eq!(before_immutable.volume, 1000.0);

	world
		.apply_command(Command::MarkImmutable { handle: mixture })
		.unwrap();
	let immutable = world.snapshot(mixture).unwrap();
	assert!(immutable.immutable);
	world
		.apply_command(Command::SetMoles {
			handle: mixture,
			gas: GasId(0),
			amount: 40.0,
		})
		.unwrap();
	assert_eq!(world.snapshot(mixture).unwrap(), immutable);
	let mut replacement = state(mixture, immutable.revision, 99.0);
	replacement.temperature = 900.0;
	assert_eq!(world.apply_mixture_state(&[replacement]), Ok(1));
	assert_eq!(world.snapshot(mixture).unwrap(), immutable);
	assert_eq!(
		world.process_stage_cancellable(WorldStage::ProcessTurfs, 0.5, || false),
		Ok(dogmos_core::world::StageResult { work_items: 1 })
	);
	assert_eq!(world.snapshot(mixture).unwrap(), immutable);

	assert_eq!(
		world.apply_command(Command::SetMoles {
			handle: mixture,
			gas: GasId(0),
			amount: f32::NAN,
		}),
		Err(WorldError::InvalidMoleAmount)
	);
	assert_eq!(
		world.apply_command(Command::SetVolume {
			handle: mixture,
			volume: -1.0,
		}),
		Err(WorldError::InvalidVolume)
	);
}

#[test]
fn merge_remove_and_transfer_conserve_moles_and_thermal_energy() {
	let source = handle(0, 1);
	let destination = handle(1, 1);
	let giver = handle(2, 1);
	let mut world = DogmosWorld::new(1024 * 1024);
	world.install_gases(vec![oxygen()]).unwrap();
	world
		.apply_lifecycle(&[
			LifecycleMutation {
				action: LifecycleAction::Register,
				handle: source,
			},
			LifecycleMutation {
				action: LifecycleAction::Register,
				handle: destination,
			},
			LifecycleMutation {
				action: LifecycleAction::Register,
				handle: giver,
			},
		])
		.unwrap();
	world
		.apply_mixture_state(&[
			state(source, 0, 100.0),
			state(destination, 0, 0.0),
			state(giver, 0, 20.0),
		])
		.unwrap();
	world
		.apply_command(Command::SetTemperature {
			handle: source,
			temperature: 400.0,
		})
		.unwrap();
	world
		.apply_command(Command::SetTemperature {
			handle: giver,
			temperature: 600.0,
		})
		.unwrap();

	world
		.apply_command(Command::RemoveRatioInto {
			source,
			destination,
			ratio: 0.25,
		})
		.unwrap();
	let source_after_remove = world.snapshot(source).unwrap();
	let destination_after_remove = world.snapshot(destination).unwrap();
	assert_eq!(source_after_remove.gases[0], 75.0);
	assert_eq!(destination_after_remove.gases[0], 25.0);
	assert_eq!(destination_after_remove.temperature, 400.0);
	assert_eq!(
		source_after_remove.gases[0] + destination_after_remove.gases[0],
		100.0
	);

	world
		.apply_command(Command::TransferGases {
			source,
			destination,
			ratio: 0.2,
			gases: vec![GasId(0)].into_boxed_slice(),
		})
		.unwrap();
	let source_after_transfer = world.snapshot(source).unwrap();
	let destination_after_transfer = world.snapshot(destination).unwrap();
	assert_eq!(source_after_transfer.gases[0], 60.0);
	assert_eq!(destination_after_transfer.gases[0], 40.0);
	assert_eq!(
		source_after_transfer.gases[0] + destination_after_transfer.gases[0],
		100.0
	);

	let energy_before_merge = destination_after_transfer.gases[0]
		* 20.0 * destination_after_transfer.temperature
		+ world.snapshot(giver).unwrap().gases[0] * 20.0 * 600.0;
	world
		.apply_command(Command::Merge {
			receiver: destination,
			giver,
		})
		.unwrap();
	let merged = world.snapshot(destination).unwrap();
	assert_eq!(merged.gases[0], 60.0);
	let energy_after_merge = merged.gases[0] * 20.0 * merged.temperature;
	assert!((energy_before_merge - energy_after_merge).abs() <= energy_before_merge * 1.0e-6);
}

#[test]
fn remove_ratio_does_not_quantize_past_the_source_amount() {
	let source = handle(0, 1);
	let destination = handle(1, 1);
	let mut world = DogmosWorld::new(1024 * 1024);
	world.install_gases(vec![oxygen()]).unwrap();
	world
		.apply_lifecycle(&[
			LifecycleMutation {
				action: LifecycleAction::Register,
				handle: source,
			},
			LifecycleMutation {
				action: LifecycleAction::Register,
				handle: destination,
			},
		])
		.unwrap();
	world
		.apply_mixture_state(&[state(source, 0, 0.00006), state(destination, 0, 0.0)])
		.unwrap();

	world
		.apply_command(Command::RemoveRatioInto {
			source,
			destination,
			ratio: 1.0,
		})
		.unwrap();

	let source_after = world.snapshot(source).unwrap();
	let destination_after = world.snapshot(destination).unwrap();
	assert_eq!(source_after.gases[0], 0.0);
	assert_eq!(destination_after.gases[0], 0.00006);
	assert_eq!(source_after.gases[0] + destination_after.gases[0], 0.00006);
}

#[test]
fn transfer_ratio_does_not_quantize_past_the_source_amount() {
	let source = handle(0, 1);
	let destination = handle(1, 1);
	let mut world = DogmosWorld::new(1024 * 1024);
	world.install_gases(vec![oxygen()]).unwrap();
	world
		.apply_lifecycle(&[
			LifecycleMutation {
				action: LifecycleAction::Register,
				handle: source,
			},
			LifecycleMutation {
				action: LifecycleAction::Register,
				handle: destination,
			},
		])
		.unwrap();
	world
		.apply_mixture_state(&[state(source, 0, 0.00006), state(destination, 0, 0.0)])
		.unwrap();

	world
		.apply_command(Command::TransferRatio {
			source,
			destination,
			ratio: 1.0,
		})
		.unwrap();

	let source_after = world.snapshot(source).unwrap();
	let destination_after = world.snapshot(destination).unwrap();
	assert_eq!(source_after.gases[0], 0.0);
	assert_eq!(destination_after.gases[0], 0.00006);
	assert_eq!(source_after.gases[0] + destination_after.gases[0], 0.00006);
}

#[test]
fn turf_topology_is_generation_checked_reciprocal_bounded_and_atomic() {
	let mut world = DogmosWorld::new(1024 * 1024);
	for slot in 0..8 {
		let mixture = handle(slot, 1);
		let turf = turf_handle(slot, 1);
		world
			.apply_lifecycle(&[LifecycleMutation {
				action: LifecycleAction::Register,
				handle: mixture,
			}])
			.unwrap();
		world
			.apply_turf_lifecycle(&[TurfLifecycleMutation::Register {
				handle: turf,
				mixture: Some(mixture),
			}])
			.unwrap();
	}

	let six_neighbors = (1..=6)
		.map(|slot| TurfAdjacencyMutation {
			left: turf_handle(0, 1),
			right: turf_handle(slot, 1),
			connected: true,
		})
		.collect::<Vec<_>>();
	assert_eq!(world.apply_turf_adjacency(&six_neighbors), Ok(6));
	assert_eq!(world.turf_edge_count(), 6);
	assert!(matches!(
		world.apply_turf_adjacency(&[TurfAdjacencyMutation {
			left: turf_handle(0, 1),
			right: turf_handle(7, 1),
			connected: true,
		}]),
		Err(WorldError::Graph(_))
	));
	assert_eq!(world.turf_edge_count(), 6);
	assert!(matches!(
		world.apply_turf_adjacency(&[TurfAdjacencyMutation {
			left: turf_handle(0, 0),
			right: turf_handle(1, 1),
			connected: true,
		}]),
		Err(WorldError::StaleTurfHandle { .. })
	));
	assert_eq!(
		world.apply_turf_adjacency(&[TurfAdjacencyMutation {
			left: turf_handle(0, 1),
			right: turf_handle(0, 1),
			connected: true,
		}]),
		Err(WorldError::SelfTurfAdjacency(turf_handle(0, 1)))
	);

	world
		.apply_turf_lifecycle(&[TurfLifecycleMutation::Unregister {
			handle: turf_handle(0, 1),
		}])
		.unwrap();
	assert_eq!(world.turf_edge_count(), 0);
}

#[test]
fn heat_only_turf_reregistration_preserves_conduction_edges() {
	let hot = turf_handle(0, 1);
	let cold = turf_handle(1, 1);
	let mut world = DogmosWorld::new(1024 * 1024);
	world
		.apply_turf_lifecycle(&[
			TurfLifecycleMutation::Register {
				handle: hot,
				mixture: None,
			},
			TurfLifecycleMutation::Register {
				handle: cold,
				mixture: None,
			},
		])
		.unwrap();
	world
		.apply_turf_heat(&[
			TurfHeatMutation {
				handle: hot,
				state: Some(TurfHeatState {
					temperature: 700.0,
					thermal_conductivity: 0.05,
					heat_capacity: 20_000.0,
					adjacent_to_space: false,
				}),
			},
			TurfHeatMutation {
				handle: cold,
				state: Some(TurfHeatState {
					temperature: 293.15,
					thermal_conductivity: 0.05,
					heat_capacity: 20_000.0,
					adjacent_to_space: false,
				}),
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
	world
		.apply_turf_lifecycle(&[TurfLifecycleMutation::Register {
			handle: hot,
			mixture: None,
		}])
		.unwrap();

	world
		.process_stage_cancellable(WorldStage::TurfHeat, 0.5, || false)
		.unwrap();

	assert!(world.turf_heat(hot).unwrap().unwrap().temperature < 700.0);
	assert!(world.turf_heat(cold).unwrap().unwrap().temperature > 293.15);
}

#[test]
fn process_turfs_uses_turf_topology_and_linked_mixtures() {
	let left_mixture = handle(0, 1);
	let right_mixture = handle(1, 1);
	let left_turf = turf_handle(10, 1);
	let right_turf = turf_handle(11, 1);
	let mut world = DogmosWorld::new(1024 * 1024);
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
	world
		.apply_mixture_state(&[state(left_mixture, 0, 100.0), state(right_mixture, 0, 0.0)])
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
	assert_eq!(
		world
			.process_stage_cancellable(WorldStage::ProcessTurfs, 0.5, || false)
			.unwrap(),
		dogmos_core::world::StageResult { work_items: 2 }
	);
	assert_eq!(world.snapshot(left_mixture).unwrap().gases[0], 87.5);
	assert_eq!(world.snapshot(right_mixture).unwrap().gases[0], 12.5);
}

#[test]
fn turf_heat_uses_separate_topology_and_conserves_finite_energy() {
	let hot = turf_handle(0, 1);
	let cold = turf_handle(1, 1);
	let mut world = DogmosWorld::new(1024 * 1024);
	world
		.apply_turf_lifecycle(&[
			TurfLifecycleMutation::Register {
				handle: hot,
				mixture: None,
			},
			TurfLifecycleMutation::Register {
				handle: cold,
				mixture: None,
			},
		])
		.unwrap();
	world
		.apply_turf_heat(&[
			TurfHeatMutation {
				handle: hot,
				state: Some(TurfHeatState {
					temperature: 1000.0,
					thermal_conductivity: 0.05,
					heat_capacity: 100.0,
					adjacent_to_space: false,
				}),
			},
			TurfHeatMutation {
				handle: cold,
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
			left: hot,
			right: cold,
			connected: true,
		}])
		.unwrap();

	let energy_before: f32 = 1000.0 * 100.0 + 300.0 * 200.0;
	assert_eq!(
		world
			.process_stage_cancellable(WorldStage::TurfHeat, 0.5, || false)
			.unwrap(),
		dogmos_core::world::StageResult { work_items: 2 }
	);
	let hot_after = world.turf_heat(hot).unwrap().unwrap().temperature;
	let cold_after = world.turf_heat(cold).unwrap().unwrap().temperature;
	assert!(hot_after < 1000.0);
	assert!(cold_after > 300.0);
	assert!(hot_after >= cold_after);
	let energy_after = hot_after * 100.0 + cold_after * 200.0;
	assert!((energy_before - energy_after).abs() <= energy_before * 1.0e-6);

	let before_cancel = (hot_after, cold_after);
	assert_eq!(
		world.process_stage_cancellable(WorldStage::TurfHeat, 0.5, || true),
		Err(WorldError::Cancelled)
	);
	assert_eq!(
		(
			world.turf_heat(hot).unwrap().unwrap().temperature,
			world.turf_heat(cold).unwrap().unwrap().temperature,
		),
		before_cancel
	);
}

#[test]
fn turf_heat_exchanges_energy_with_linked_gas_and_emits_destruction_requests() {
	let mixture = handle(0, 1);
	let turf = turf_handle(0, 1);
	let mut world = DogmosWorld::new_with_event_capacity(1024 * 1024, 1);
	world.install_gases(vec![oxygen()]).unwrap();
	world
		.apply_lifecycle(&[LifecycleMutation {
			action: LifecycleAction::Register,
			handle: mixture,
		}])
		.unwrap();
	let mut mixture_state = state(mixture, 0, 10.0);
	mixture_state.temperature = 300.0;
	world.apply_mixture_state(&[mixture_state]).unwrap();
	world
		.apply_turf_lifecycle(&[TurfLifecycleMutation::Register {
			handle: turf,
			mixture: Some(mixture),
		}])
		.unwrap();
	world
		.apply_turf_heat(&[TurfHeatMutation {
			handle: turf,
			state: Some(TurfHeatState {
				temperature: 1000.0,
				thermal_conductivity: 0.4,
				heat_capacity: 100.0,
				adjacent_to_space: false,
			}),
		}])
		.unwrap();

	let energy_before = 1000.0 * 100.0 + 300.0 * 200.0;
	assert_eq!(
		world
			.process_stage_cancellable(WorldStage::TurfHeat, 0.5, || false)
			.unwrap(),
		dogmos_core::world::StageResult { work_items: 1 }
	);
	let turf_after = world.turf_heat(turf).unwrap().unwrap().temperature;
	let gas_after = world.snapshot(mixture).unwrap();
	assert!(turf_after < 1000.0);
	assert!(gas_after.temperature > 300.0);
	assert_eq!(gas_after.revision, 2);
	let energy_after = turf_after * 100.0 + gas_after.temperature * 200.0;
	assert!((energy_before - energy_after).abs() <= energy_before * 1.0e-6);
	let mut events = Vec::new();
	assert_eq!(world.drain_events_into(1, &mut events), 1);
	assert_eq!(events, vec![WorldEvent::TurfDestructionRequest { turf }]);
}

#[test]
fn turf_heat_space_radiation_is_finite_and_event_overflow_is_atomic() {
	let turf = turf_handle(0, 1);
	let mut world = DogmosWorld::new_with_event_capacity(1024 * 1024, 0);
	world
		.apply_turf_lifecycle(&[TurfLifecycleMutation::Register {
			handle: turf,
			mixture: None,
		}])
		.unwrap();
	world
		.apply_turf_heat(&[TurfHeatMutation {
			handle: turf,
			state: Some(TurfHeatState {
				temperature: 1000.0,
				thermal_conductivity: 0.4,
				heat_capacity: 100.0,
				adjacent_to_space: true,
			}),
		}])
		.unwrap();

	assert_eq!(
		world.process_stage_cancellable(WorldStage::TurfHeat, 0.5, || false),
		Err(WorldError::EventCapacityExceeded {
			requested: 1,
			capacity: 0,
		})
	);
	assert_eq!(world.turf_heat(turf).unwrap().unwrap().temperature, 1000.0);
}

#[test]
fn equalize_moves_gas_conservatively_and_emits_ordered_pressure_events() {
	let high_mixture = handle(0, 1);
	let low_mixture = handle(1, 1);
	let high_turf = turf_handle(0, 1);
	let low_turf = turf_handle(1, 1);
	let mut world = DogmosWorld::new_with_event_capacity(1024 * 1024, 8);
	world.install_gases(vec![oxygen()]).unwrap();
	world
		.apply_lifecycle(&[
			LifecycleMutation {
				action: LifecycleAction::Register,
				handle: high_mixture,
			},
			LifecycleMutation {
				action: LifecycleAction::Register,
				handle: low_mixture,
			},
		])
		.unwrap();
	world
		.apply_mixture_state(&[state(high_mixture, 0, 100.0), state(low_mixture, 0, 0.0)])
		.unwrap();
	world
		.apply_turf_lifecycle(&[
			TurfLifecycleMutation::Register {
				handle: high_turf,
				mixture: Some(high_mixture),
			},
			TurfLifecycleMutation::Register {
				handle: low_turf,
				mixture: Some(low_mixture),
			},
		])
		.unwrap();
	world
		.apply_turf_adjacency(&[TurfAdjacencyMutation {
			left: high_turf,
			right: low_turf,
			connected: true,
		}])
		.unwrap();

	let before_cancel = (
		world.snapshot(high_mixture).unwrap(),
		world.snapshot(low_mixture).unwrap(),
	);
	assert_eq!(
		world.process_stage_cancellable(WorldStage::Equalize, 0.5, || true),
		Err(WorldError::Cancelled)
	);
	assert_eq!(world.snapshot(high_mixture).unwrap(), before_cancel.0);
	assert_eq!(world.snapshot(low_mixture).unwrap(), before_cancel.1);

	assert_eq!(
		world
			.process_stage_cancellable(WorldStage::Equalize, 0.5, || false)
			.unwrap(),
		dogmos_core::world::StageResult { work_items: 2 }
	);
	let high_after = world.snapshot(high_mixture).unwrap();
	let low_after = world.snapshot(low_mixture).unwrap();
	assert_eq!(high_after.gases[0], 50.0);
	assert_eq!(low_after.gases[0], 50.0);
	assert_eq!(high_after.gases[0] + low_after.gases[0], 100.0);
	let mut events = Vec::new();
	assert_eq!(world.drain_events_into(8, &mut events), 1);
	assert_eq!(
		events,
		vec![WorldEvent::PressureDifference {
			source: high_turf,
			target: low_turf,
			moles: 50.0,
		}]
	);
}

#[test]
fn equalize_slowly_decompresses_to_immutable_space_and_reports_boundary_loss() {
	let room_mixture = handle(0, 1);
	let space_mixture = handle(1, 1);
	let room_turf = turf_handle(0, 1);
	let space_turf = turf_handle(1, 1);
	let mut world = DogmosWorld::new_with_event_capacity(1024 * 1024, 8);
	world.install_gases(vec![oxygen()]).unwrap();
	world
		.apply_lifecycle(&[
			LifecycleMutation {
				action: LifecycleAction::Register,
				handle: room_mixture,
			},
			LifecycleMutation {
				action: LifecycleAction::Register,
				handle: space_mixture,
			},
		])
		.unwrap();
	world
		.apply_mixture_state(&[state(room_mixture, 0, 100.0), state(space_mixture, 0, 0.0)])
		.unwrap();
	world
		.apply_command(Command::MarkImmutable {
			handle: space_mixture,
		})
		.unwrap();
	world
		.apply_turf_lifecycle(&[
			TurfLifecycleMutation::Register {
				handle: room_turf,
				mixture: Some(room_mixture),
			},
			TurfLifecycleMutation::Register {
				handle: space_turf,
				mixture: Some(space_mixture),
			},
		])
		.unwrap();
	world
		.apply_turf_adjacency(&[TurfAdjacencyMutation {
			left: room_turf,
			right: space_turf,
			connected: true,
		}])
		.unwrap();
	world
		.apply_turf_firelocks(&[TurfFirelockMutation {
			left: room_turf,
			right: space_turf,
			firelock: true,
		}])
		.unwrap();

	assert_eq!(
		world
			.process_stage_cancellable(WorldStage::Equalize, 0.5, || false)
			.unwrap(),
		dogmos_core::world::StageResult { work_items: 2 }
	);
	assert_eq!(world.snapshot(room_mixture).unwrap().gases[0], 75.0);
	assert_eq!(world.snapshot(space_mixture).unwrap().gases[0], 0.0);
	let mut events = Vec::new();
	assert_eq!(world.drain_events_into(8, &mut events), 3);
	assert_eq!(
		events,
		vec![
			WorldEvent::FirelockConsideration {
				source: room_turf,
				target: space_turf,
			},
			WorldEvent::PressureDifference {
				source: room_turf,
				target: space_turf,
				moles: 25.0,
			},
			WorldEvent::DecompressionFloorRip {
				turf: room_turf,
				moles_lost: 25.0,
			},
		]
	);
}

#[test]
fn equalize_checks_cancellation_inside_a_component_and_rolls_back() {
	let mixtures = [handle(0, 1), handle(1, 1), handle(2, 1)];
	let turfs = [turf_handle(0, 1), turf_handle(1, 1), turf_handle(2, 1)];
	let mut world = DogmosWorld::new_with_event_capacity(1024 * 1024, 8);
	world.install_gases(vec![oxygen()]).unwrap();
	world
		.apply_lifecycle(&mixtures.map(|handle| LifecycleMutation {
			action: LifecycleAction::Register,
			handle,
		}))
		.unwrap();
	world
		.apply_mixture_state(&[
			state(mixtures[0], 0, 100.0),
			state(mixtures[1], 0, 0.0),
			state(mixtures[2], 0, 0.0),
		])
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
			TurfLifecycleMutation::Register {
				handle: turfs[2],
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
				left: turfs[1],
				right: turfs[2],
				connected: true,
			},
		])
		.unwrap();
	let before = mixtures.map(|mixture| world.snapshot(mixture).unwrap());
	let mut checks = 0;

	assert_eq!(
		world.process_stage_cancellable(WorldStage::Equalize, 0.5, || {
			checks += 1;
			checks >= 4
		}),
		Err(WorldError::Cancelled)
	);
	assert!(checks >= 4);
	assert_eq!(
		mixtures.map(|mixture| world.snapshot(mixture).unwrap()),
		before
	);
	let mut events = Vec::new();
	assert_eq!(world.drain_events_into(8, &mut events), 0);
	world.set_equalize_hard_turf_limit(2).unwrap();
	assert_eq!(
		world
			.process_stage_cancellable(WorldStage::Equalize, 0.5, || false)
			.unwrap(),
		dogmos_core::world::StageResult { work_items: 2 }
	);
	assert_eq!(world.snapshot(mixtures[0]).unwrap().gases[0], 50.0);
	assert_eq!(world.snapshot(mixtures[1]).unwrap().gases[0], 50.0);
	assert_eq!(world.snapshot(mixtures[2]).unwrap().gases[0], 0.0);
}

#[test]
fn excited_groups_fully_mix_only_low_pressure_components() {
	let first_mixture = handle(0, 1);
	let second_mixture = handle(1, 1);
	let first_turf = turf_handle(0, 1);
	let second_turf = turf_handle(1, 1);
	let mut world = DogmosWorld::new(1024 * 1024);
	world.install_gases(vec![oxygen()]).unwrap();
	world
		.apply_lifecycle(&[
			LifecycleMutation {
				action: LifecycleAction::Register,
				handle: first_mixture,
			},
			LifecycleMutation {
				action: LifecycleAction::Register,
				handle: second_mixture,
			},
		])
		.unwrap();
	world
		.apply_mixture_state(&[state(first_mixture, 0, 10.0), state(second_mixture, 0, 9.9)])
		.unwrap();
	world
		.apply_turf_lifecycle(&[
			TurfLifecycleMutation::Register {
				handle: first_turf,
				mixture: Some(first_mixture),
			},
			TurfLifecycleMutation::Register {
				handle: second_turf,
				mixture: Some(second_mixture),
			},
		])
		.unwrap();
	world
		.apply_turf_adjacency(&[TurfAdjacencyMutation {
			left: first_turf,
			right: second_turf,
			connected: true,
		}])
		.unwrap();

	assert_eq!(
		world
			.process_stage_cancellable(WorldStage::ExcitedGroups, 0.5, || false)
			.unwrap(),
		dogmos_core::world::StageResult { work_items: 2 }
	);
	assert!((world.snapshot(first_mixture).unwrap().gases[0] - 9.95).abs() < 1.0e-5);
	assert!((world.snapshot(second_mixture).unwrap().gases[0] - 9.95).abs() < 1.0e-5);
	world
		.apply_command(Command::SetMoles {
			handle: first_mixture,
			gas: GasId(0),
			amount: 10.0,
		})
		.unwrap();
	world
		.apply_command(Command::SetMoles {
			handle: second_mixture,
			gas: GasId(0),
			amount: 9.9,
		})
		.unwrap();
	let before_cancel = (
		world.snapshot(first_mixture).unwrap(),
		world.snapshot(second_mixture).unwrap(),
	);
	let mut checks = 0;
	assert_eq!(
		world.process_stage_cancellable(WorldStage::ExcitedGroups, 0.5, || {
			checks += 1;
			checks >= 4
		}),
		Err(WorldError::Cancelled)
	);
	assert!(checks >= 4);
	assert_eq!(
		(
			world.snapshot(first_mixture).unwrap(),
			world.snapshot(second_mixture).unwrap(),
		),
		before_cancel
	);
}
