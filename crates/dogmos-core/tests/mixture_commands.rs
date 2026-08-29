use dogmos_core::metadata::{GasFireRole, GasId, GasMetadata};
use dogmos_core::world::{
	Command, CommandResult, DogmosWorld, LifecycleAction, LifecycleMutation, WorldError,
};
use dogmos_core::MixtureHandle;

fn handle(slot: u32) -> MixtureHandle {
	MixtureHandle {
		slot,
		generation: 1,
	}
}

fn gas(id: u16, key: &str, specific_heat: f32) -> GasMetadata {
	GasMetadata {
		id: GasId(id),
		key: key.into(),
		name: key.into(),
		flags: 0,
		specific_heat,
		fusion_power: 0.0,
		moles_visible: None,
		enthalpy: 0.0,
		fire_radiation_released: 0.0,
		fire_role: GasFireRole::None,
		fire_products: None,
	}
}

fn world_with_two_mixtures() -> DogmosWorld {
	let mut world = DogmosWorld::new(1024 * 1024);
	world
		.install_gases(vec![gas(0, "o2", 20.0), gas(1, "n2", 10.0)])
		.unwrap();
	world
		.apply_lifecycle(&[
			LifecycleMutation {
				action: LifecycleAction::Register,
				handle: handle(0),
			},
			LifecycleMutation {
				action: LifecycleAction::Register,
				handle: handle(1),
			},
		])
		.unwrap();
	world
}

fn scalar(world: &mut DogmosWorld, command: Command) -> f32 {
	match world.apply_command(command).unwrap() {
		CommandResult::Scalar(value) => value,
		other => panic!("expected scalar command result, got {other:?}"),
	}
}

#[test]
fn scalar_queries_match_legacy_formulas_and_minimum_heat_capacity() {
	let mut world = world_with_two_mixtures();
	world
		.apply_command(Command::SetMoles {
			handle: handle(0),
			gas: GasId(0),
			amount: 2.0,
		})
		.unwrap();
	world
		.apply_command(Command::SetTemperature {
			handle: handle(0),
			temperature: 400.0,
		})
		.unwrap();
	world
		.apply_command(Command::SetVolume {
			handle: handle(0),
			volume: 100.0,
		})
		.unwrap();

	assert_eq!(
		scalar(
			&mut world,
			Command::GetMoles {
				handle: handle(0),
				gas: GasId(0),
			}
		),
		2.0
	);
	assert_eq!(
		scalar(&mut world, Command::TotalMoles { handle: handle(0) }),
		2.0
	);
	assert_eq!(
		scalar(&mut world, Command::HeatCapacity { handle: handle(0) }),
		40.0
	);
	assert_eq!(
		scalar(
			&mut world,
			Command::PartialHeatCapacity {
				handle: handle(0),
				gas: GasId(0),
			}
		),
		40.0
	);
	assert!((scalar(&mut world, Command::Pressure { handle: handle(0) }) - 66.48).abs() < 0.0001);
	assert_eq!(
		scalar(&mut world, Command::ThermalEnergy { handle: handle(0) }),
		16_000.0
	);
	assert_eq!(
		scalar(&mut world, Command::Temperature { handle: handle(0) }),
		400.0
	);
	assert_eq!(
		scalar(&mut world, Command::Volume { handle: handle(0) }),
		100.0
	);

	world
		.apply_command(Command::SetMinimumHeatCapacity {
			handle: handle(1),
			amount: 50.0,
		})
		.unwrap();
	assert_eq!(
		scalar(&mut world, Command::HeatCapacity { handle: handle(1) }),
		50.0
	);
	assert_eq!(
		scalar(&mut world, Command::ThermalEnergy { handle: handle(1) }),
		135.0
	);
	assert_eq!(
		world.snapshot(handle(1)).unwrap().minimum_heat_capacity,
		50.0
	);
}

#[test]
fn snapshots_include_atomic_service_derived_scalars() {
	let mut world = world_with_two_mixtures();
	world
		.apply_command(Command::SetMoles {
			handle: handle(0),
			gas: GasId(0),
			amount: 2.0,
		})
		.unwrap();
	world
		.apply_command(Command::SetMoles {
			handle: handle(0),
			gas: GasId(1),
			amount: 3.0,
		})
		.unwrap();
	world
		.apply_command(Command::SetTemperature {
			handle: handle(0),
			temperature: 400.0,
		})
		.unwrap();
	world
		.apply_command(Command::SetVolume {
			handle: handle(0),
			volume: 100.0,
		})
		.unwrap();

	let snapshot = world.snapshot(handle(0)).unwrap();
	assert_eq!(snapshot.total_moles, 5.0);
	assert_eq!(snapshot.heat_capacity, 70.0);
	assert_eq!(
		snapshot.pressure,
		scalar(&mut world, Command::Pressure { handle: handle(0) })
	);

	let empty = world.snapshot(handle(1)).unwrap();
	assert_eq!(empty.total_moles, 0.0);
	assert_eq!(empty.pressure, 0.0);
	assert_eq!(empty.heat_capacity, empty.minimum_heat_capacity);
}

#[test]
fn elementary_mutations_match_legacy_and_fail_before_mutation() {
	let mut world = world_with_two_mixtures();
	for (gas, amount) in [(GasId(0), 1.0), (GasId(1), 2.0)] {
		world
			.apply_command(Command::SetMoles {
				handle: handle(0),
				gas,
				amount,
			})
			.unwrap();
	}
	world
		.apply_command(Command::Add {
			handle: handle(0),
			amount: 1.0,
		})
		.unwrap();
	world
		.apply_command(Command::Multiply {
			handle: handle(0),
			factor: 0.5,
		})
		.unwrap();
	let scaled = world.snapshot(handle(0)).unwrap();
	assert_eq!(scaled.gases[0], 1.0);
	assert_eq!(scaled.gases[1], 1.5);

	let before_invalid = scaled.clone();
	assert_eq!(
		world.apply_command(Command::Multiply {
			handle: handle(0),
			factor: -1.0,
		}),
		Err(WorldError::InvalidMultiplier)
	);
	assert_eq!(world.snapshot(handle(0)).unwrap(), before_invalid);

	world
		.apply_command(Command::Clear { handle: handle(0) })
		.unwrap();
	assert!(world
		.snapshot(handle(0))
		.unwrap()
		.gases
		.iter()
		.all(|amount| *amount == 0.0));
}

#[test]
fn copy_compare_and_adjust_heat_preserve_legacy_state_ownership() {
	let mut world = world_with_two_mixtures();
	world
		.apply_command(Command::SetMoles {
			handle: handle(1),
			gas: GasId(0),
			amount: 2.0,
		})
		.unwrap();
	world
		.apply_command(Command::SetTemperature {
			handle: handle(1),
			temperature: 400.0,
		})
		.unwrap();
	world
		.apply_command(Command::SetVolume {
			handle: handle(0),
			volume: 100.0,
		})
		.unwrap();
	world
		.apply_command(Command::SetMinimumHeatCapacity {
			handle: handle(0),
			amount: 80.0,
		})
		.unwrap();
	world
		.apply_command(Command::CopyFrom {
			receiver: handle(0),
			giver: handle(1),
		})
		.unwrap();
	let copied = world.snapshot(handle(0)).unwrap();
	assert_eq!(copied.gases[0], 2.0);
	assert_eq!(copied.temperature, 400.0);
	assert_eq!(copied.volume, 100.0);
	assert_eq!(copied.minimum_heat_capacity, 80.0);
	assert_eq!(
		world.apply_command(Command::Compare {
			left: handle(0),
			right: handle(1),
		}),
		Ok(CommandResult::Boolean(false))
	);

	world
		.apply_command(Command::AdjustHeat {
			handle: handle(0),
			heat: 800.0,
		})
		.unwrap();
	assert_eq!(world.snapshot(handle(0)).unwrap().temperature, 410.0);
	assert_eq!(
		world.apply_command(Command::Compare {
			left: handle(0),
			right: handle(1),
		}),
		Ok(CommandResult::Boolean(true))
	);
	world
		.apply_command(Command::MarkImmutable { handle: handle(0) })
		.unwrap();
	assert_eq!(
		world.apply_command(Command::IsImmutable { handle: handle(0) }),
		Ok(CommandResult::Boolean(true))
	);
}

#[test]
fn multi_adjust_temperature_and_metadata_queries_match_legacy() {
	let mut oxidizer = gas(0, "o2", 20.0);
	oxidizer.flags = 1;
	oxidizer.fire_role = GasFireRole::Oxidizer {
		minimum_temperature: 300.0,
		power: 2.0,
	};
	let mut fuel = gas(1, "fuel", 10.0);
	fuel.flags = 2;
	fuel.fire_role = GasFireRole::Fuel {
		minimum_temperature: 350.0,
		burn_rate: 4.0,
	};
	let mut world = DogmosWorld::new(1024 * 1024);
	world.install_gases(vec![oxidizer, fuel]).unwrap();
	world
		.apply_lifecycle(&[
			LifecycleMutation {
				action: LifecycleAction::Register,
				handle: handle(0),
			},
			LifecycleMutation {
				action: LifecycleAction::Register,
				handle: handle(1),
			},
		])
		.unwrap();
	world
		.apply_command(Command::SetMoles {
			handle: handle(0),
			gas: GasId(0),
			amount: 2.0,
		})
		.unwrap();
	world
		.apply_command(Command::SetTemperature {
			handle: handle(0),
			temperature: 300.0,
		})
		.unwrap();
	world
		.apply_command(Command::AdjustMolesTemperature {
			handle: handle(0),
			gas: GasId(0),
			amount: 2.0,
			temperature: 600.0,
		})
		.unwrap();
	let mixed = world.snapshot(handle(0)).unwrap();
	assert_eq!(mixed.gases[0], 4.0);
	assert_eq!(mixed.temperature, 450.0);

	world
		.apply_command(Command::AdjustMultiple {
			handle: handle(0),
			adjustments: vec![(GasId(0), -1.0), (GasId(0), 2.0), (GasId(1), 3.0)]
				.into_boxed_slice(),
		})
		.unwrap();
	let adjusted = world.snapshot(handle(0)).unwrap();
	assert_eq!(adjusted.gases[0], 5.0);
	assert_eq!(adjusted.gases[1], 3.0);
	assert_eq!(
		scalar(
			&mut world,
			Command::GetMolesByFlags {
				handle: handle(0),
				flags: 1,
			}
		),
		5.0
	);
	assert_eq!(
		world.apply_command(Command::Burnability {
			handle: handle(0),
			temperature: Some(400.0),
		}),
		Ok(CommandResult::Scalars([2.5, 0.09375]))
	);

	let before_invalid = world.snapshot(handle(0)).unwrap();
	assert!(matches!(
		world.apply_command(Command::AdjustMultiple {
			handle: handle(0),
			adjustments: vec![(GasId(0), 1.0), (GasId(7), 1.0)].into_boxed_slice(),
		}),
		Err(WorldError::InvalidGasId(GasId(7)))
	));
	assert_eq!(world.snapshot(handle(0)).unwrap(), before_invalid);
}

#[test]
fn flag_transfer_is_atomic_and_uses_total_moles_for_legacy_amount_ratio() {
	let mut flagged = gas(0, "flagged", 20.0);
	flagged.flags = 1;
	let mut world = DogmosWorld::new(1024 * 1024);
	world
		.install_gases(vec![flagged, gas(1, "other", 10.0)])
		.unwrap();
	world
		.apply_lifecycle(&[
			LifecycleMutation {
				action: LifecycleAction::Register,
				handle: handle(0),
			},
			LifecycleMutation {
				action: LifecycleAction::Register,
				handle: handle(1),
			},
		])
		.unwrap();
	for (gas, amount) in [(GasId(0), 6.0), (GasId(1), 4.0)] {
		world
			.apply_command(Command::SetMoles {
				handle: handle(0),
				gas,
				amount,
			})
			.unwrap();
	}
	assert_eq!(
		world.apply_command(Command::TransferByFlags {
			source: handle(0),
			destination: handle(1),
			flags: 1,
			amount: 5.0,
		}),
		Ok(CommandResult::Boolean(true))
	);
	assert_eq!(world.snapshot(handle(0)).unwrap().gases[0], 3.0);
	assert_eq!(world.snapshot(handle(1)).unwrap().gases[0], 3.0);
	assert_eq!(world.snapshot(handle(0)).unwrap().gases[1], 4.0);
	assert_eq!(
		world.apply_command(Command::TransferByFlags {
			source: handle(0),
			destination: handle(1),
			flags: 8,
			amount: 1.0,
		}),
		Ok(CommandResult::Boolean(false))
	);
}

#[test]
fn transfer_equalize_and_temperature_share_match_legacy_thermodynamics() {
	let mut world = world_with_two_mixtures();
	for (mixture, moles, temperature, volume) in [
		(handle(0), 10.0, 500.0, 100.0),
		(handle(1), 10.0, 300.0, 200.0),
	] {
		world
			.apply_command(Command::SetMoles {
				handle: mixture,
				gas: GasId(0),
				amount: moles,
			})
			.unwrap();
		world
			.apply_command(Command::SetTemperature {
				handle: mixture,
				temperature,
			})
			.unwrap();
		world
			.apply_command(Command::SetVolume {
				handle: mixture,
				volume,
			})
			.unwrap();
	}

	assert_eq!(
		world.apply_command(Command::TransferAmount {
			source: handle(0),
			destination: handle(1),
			amount: 5.0,
		}),
		Ok(CommandResult::Applied { updated: 2 })
	);
	let source = world.snapshot(handle(0)).unwrap();
	let destination = world.snapshot(handle(1)).unwrap();
	assert_eq!(source.gases[0], 5.0);
	assert_eq!(destination.gases[0], 15.0);
	assert!((destination.temperature - 366.666_66).abs() < 0.0001);

	world
		.apply_command(Command::EqualizeWith {
			receiver: handle(0),
			total: handle(1),
		})
		.unwrap();
	let equalized = world.snapshot(handle(0)).unwrap();
	assert_eq!(equalized.volume, 100.0);
	assert_eq!(equalized.gases[0], 7.5);
	assert!((equalized.temperature - destination.temperature).abs() < 0.0001);

	world
		.apply_command(Command::SetMoles {
			handle: handle(0),
			gas: GasId(0),
			amount: 10.0,
		})
		.unwrap();
	world
		.apply_command(Command::SetMoles {
			handle: handle(1),
			gas: GasId(0),
			amount: 10.0,
		})
		.unwrap();
	world
		.apply_command(Command::SetTemperature {
			handle: handle(0),
			temperature: 500.0,
		})
		.unwrap();
	world
		.apply_command(Command::SetTemperature {
			handle: handle(1),
			temperature: 300.0,
		})
		.unwrap();
	assert_eq!(
		world.apply_command(Command::TemperatureShare {
			first: handle(0),
			second: handle(1),
			conduction_coefficient: 0.4,
		}),
		Ok(CommandResult::Scalar(340.0))
	);
	assert_eq!(world.snapshot(handle(0)).unwrap().temperature, 460.0);
	assert_eq!(world.snapshot(handle(1)).unwrap().temperature, 340.0);
}

#[test]
fn non_gas_temperature_share_returns_external_temperature_and_honors_immutability() {
	let mut world = world_with_two_mixtures();
	world
		.apply_command(Command::SetMoles {
			handle: handle(0),
			gas: GasId(0),
			amount: 10.0,
		})
		.unwrap();
	world
		.apply_command(Command::SetTemperature {
			handle: handle(0),
			temperature: 500.0,
		})
		.unwrap();
	let returned = scalar(
		&mut world,
		Command::TemperatureShareNonGas {
			handle: handle(0),
			conduction_coefficient: 0.4,
			sharer_temperature: 300.0,
			sharer_heat_capacity: 100.0,
		},
	);
	assert!((returned - 353.333_34).abs() < 0.0001);
	assert!((world.snapshot(handle(0)).unwrap().temperature - 473.333_34).abs() < 0.0001);

	world
		.apply_command(Command::MarkImmutable { handle: handle(0) })
		.unwrap();
	let immutable = world.snapshot(handle(0)).unwrap();
	let returned = scalar(
		&mut world,
		Command::TemperatureShareNonGas {
			handle: handle(0),
			conduction_coefficient: 0.4,
			sharer_temperature: 300.0,
			sharer_heat_capacity: 100.0,
		},
	);
	assert!(returned > 300.0);
	assert_eq!(world.snapshot(handle(0)).unwrap(), immutable);
}

#[test]
fn amount_removal_ratio_transfer_and_share_ratio_preserve_legacy_ordering() {
	let mut world = world_with_two_mixtures();
	for (mixture, moles, temperature) in [(handle(0), 10.0, 500.0), (handle(1), 20.0, 300.0)] {
		world
			.apply_command(Command::SetMoles {
				handle: mixture,
				gas: GasId(0),
				amount: moles,
			})
			.unwrap();
		world
			.apply_command(Command::SetTemperature {
				handle: mixture,
				temperature,
			})
			.unwrap();
	}
	assert_eq!(
		world.apply_command(Command::RemoveAmountInto {
			source: handle(0),
			destination: handle(1),
			amount: 2.5,
		}),
		Ok(CommandResult::Applied { updated: 2 })
	);
	assert_eq!(world.snapshot(handle(0)).unwrap().gases[0], 7.5);
	assert_eq!(world.snapshot(handle(1)).unwrap().gases[0], 2.5);
	assert_eq!(world.snapshot(handle(1)).unwrap().temperature, 500.0);

	assert_eq!(
		world.apply_command(Command::TransferRatio {
			source: handle(0),
			destination: handle(1),
			ratio: 0.5,
		}),
		Ok(CommandResult::Applied { updated: 2 })
	);
	assert_eq!(world.snapshot(handle(0)).unwrap().gases[0], 3.75);
	assert_eq!(world.snapshot(handle(1)).unwrap().gases[0], 6.25);

	assert_eq!(
		world.apply_command(Command::ShareRatio {
			first: handle(0),
			second: handle(1),
			ratio: 0.4,
			one_way: false,
		}),
		Ok(CommandResult::Boolean(true))
	);
	let first = world.snapshot(handle(0)).unwrap();
	let second = world.snapshot(handle(1)).unwrap();
	assert!((first.gases[0] - 4.25).abs() < 0.0001);
	assert!((second.gases[0] - 5.75).abs() < 0.0001);
	assert!((first.gases[0] + second.gases[0] - 10.0).abs() < 0.0001);
}
