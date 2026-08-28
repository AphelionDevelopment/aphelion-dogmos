use dogmos_core::{
	metadata::{
		GasFireRole, GasId, GasMetadata, GasRequirement, ReactionExecution, ReactionId,
		ReactionMetadata, ReactionMetadataError, ReactionMetadataRegistry,
	},
	world::{DogmosWorld, LifecycleAction, LifecycleMutation, MixtureStateMutation, WorldError},
	MixtureHandle,
};

fn gas(id: u16, key: &str) -> GasMetadata {
	GasMetadata {
		id: GasId(id),
		key: key.into(),
		name: key.into(),
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

fn reaction(id: u32, key: &str, priority: f32) -> ReactionMetadata {
	ReactionMetadata {
		id: ReactionId(id),
		key: key.into(),
		priority,
		minimum_temperature: None,
		maximum_temperature: None,
		minimum_energy: None,
		minimum_fire_reagents: None,
		gas_requirements: Box::new([]),
		execution: ReactionExecution::Dm,
	}
}

fn gas_registry() -> dogmos_core::metadata::GasMetadataRegistry {
	dogmos_core::metadata::GasMetadataRegistry::try_new(vec![gas(0, "o2"), gas(1, "plasma")])
		.unwrap()
}

#[test]
fn registry_accepts_signed_finite_fusion_power() {
	let mut negative_fusion_gas = gas(0, "bz");
	negative_fusion_gas.fusion_power = -10.0;
	assert!(dogmos_core::metadata::GasMetadataRegistry::try_new(vec![negative_fusion_gas]).is_ok());
}

#[test]
fn registry_freezes_dense_ids_and_descending_priority_order() {
	let registry = ReactionMetadataRegistry::try_new(
		vec![reaction(1, "low", 1.0), reaction(0, "high", 2.0)],
		&gas_registry(),
	)
	.unwrap();

	assert_eq!(registry.len(), 2);
	assert_eq!(registry.by_id(ReactionId(0)).unwrap().key.as_ref(), "high");
	assert_eq!(registry.by_key("low").unwrap().id, ReactionId(1));
	assert_eq!(registry.priority_order(), &[ReactionId(0), ReactionId(1)]);
}

#[test]
fn registry_rejects_ambiguous_reaction_identity_or_priority() {
	let gases = gas_registry();
	assert_eq!(
		ReactionMetadataRegistry::try_new(
			vec![reaction(0, "one", 2.0), reaction(0, "two", 1.0)],
			&gases,
		)
		.unwrap_err(),
		ReactionMetadataError::DuplicateReactionId(ReactionId(0))
	);
	assert_eq!(
		ReactionMetadataRegistry::try_new(vec![reaction(1, "one", 2.0)], &gases).unwrap_err(),
		ReactionMetadataError::NonDenseReactionId {
			expected: ReactionId(0),
			actual: ReactionId(1),
		}
	);
	assert_eq!(
		ReactionMetadataRegistry::try_new(
			vec![reaction(0, "same", 2.0), reaction(1, "same", 1.0)],
			&gases,
		)
		.unwrap_err(),
		ReactionMetadataError::DuplicateReactionKey("same".into())
	);
	assert_eq!(
		ReactionMetadataRegistry::try_new(
			vec![reaction(0, "one", 2.0), reaction(1, "two", 2.0)],
			&gases,
		)
		.unwrap_err(),
		ReactionMetadataError::DuplicateReactionPriority {
			first: ReactionId(0),
			second: ReactionId(1),
		}
	);
	assert_eq!(
		ReactionMetadataRegistry::try_new(vec![reaction(0, "", 2.0)], &gases).unwrap_err(),
		ReactionMetadataError::EmptyReactionKey(ReactionId(0))
	);
}

#[test]
fn registry_rejects_invalid_thresholds() {
	let gases = gas_registry();
	let mut invalid_priority = reaction(0, "one", f32::NAN);
	assert_eq!(
		ReactionMetadataRegistry::try_new(vec![invalid_priority.clone()], &gases).unwrap_err(),
		ReactionMetadataError::InvalidPriority(ReactionId(0))
	);

	let mut invalid_minimum_temperature = reaction(0, "one", 1.0);
	invalid_minimum_temperature.minimum_temperature = Some(-1.0);
	assert_eq!(
		ReactionMetadataRegistry::try_new(vec![invalid_minimum_temperature], &gases).unwrap_err(),
		ReactionMetadataError::InvalidMinimumTemperature(ReactionId(0))
	);

	let mut invalid_maximum_temperature = reaction(0, "one", 1.0);
	invalid_maximum_temperature.maximum_temperature = Some(f32::INFINITY);
	assert_eq!(
		ReactionMetadataRegistry::try_new(vec![invalid_maximum_temperature], &gases).unwrap_err(),
		ReactionMetadataError::InvalidMaximumTemperature(ReactionId(0))
	);

	invalid_priority.priority = 1.0;
	invalid_priority.minimum_temperature = Some(500.0);
	invalid_priority.maximum_temperature = Some(400.0);
	assert_eq!(
		ReactionMetadataRegistry::try_new(vec![invalid_priority], &gases).unwrap_err(),
		ReactionMetadataError::InvalidTemperatureRange(ReactionId(0))
	);

	let mut invalid_energy = reaction(0, "one", 1.0);
	invalid_energy.minimum_energy = Some(-1.0);
	assert_eq!(
		ReactionMetadataRegistry::try_new(vec![invalid_energy], &gases).unwrap_err(),
		ReactionMetadataError::InvalidMinimumEnergy(ReactionId(0))
	);

	let mut invalid_fire = reaction(0, "one", 1.0);
	invalid_fire.minimum_fire_reagents = Some(f32::INFINITY);
	assert_eq!(
		ReactionMetadataRegistry::try_new(vec![invalid_fire], &gases).unwrap_err(),
		ReactionMetadataError::InvalidMinimumFireReagents(ReactionId(0))
	);
}

#[test]
fn registry_validates_gas_requirements_once_at_installation() {
	let gases = gas_registry();
	let mut valid = reaction(0, "plasmafire", 1.0);
	valid.gas_requirements = vec![
		GasRequirement {
			gas: GasId(0),
			minimum_moles: 1.0,
		},
		GasRequirement {
			gas: GasId(1),
			minimum_moles: 0.01,
		},
	]
	.into_boxed_slice();
	assert!(ReactionMetadataRegistry::try_new(vec![valid], &gases).is_ok());

	let mut unknown = reaction(0, "unknown", 1.0);
	unknown.gas_requirements = vec![GasRequirement {
		gas: GasId(2),
		minimum_moles: 1.0,
	}]
	.into_boxed_slice();
	assert_eq!(
		ReactionMetadataRegistry::try_new(vec![unknown], &gases).unwrap_err(),
		ReactionMetadataError::UnknownRequiredGas {
			reaction: ReactionId(0),
			gas: GasId(2),
		}
	);

	let mut duplicate = reaction(0, "duplicate", 1.0);
	duplicate.gas_requirements = vec![
		GasRequirement {
			gas: GasId(0),
			minimum_moles: 1.0,
		},
		GasRequirement {
			gas: GasId(0),
			minimum_moles: 2.0,
		},
	]
	.into_boxed_slice();
	assert_eq!(
		ReactionMetadataRegistry::try_new(vec![duplicate], &gases).unwrap_err(),
		ReactionMetadataError::DuplicateRequiredGas {
			reaction: ReactionId(0),
			gas: GasId(0),
		}
	);

	let mut invalid_amount = reaction(0, "invalid", 1.0);
	invalid_amount.gas_requirements = vec![GasRequirement {
		gas: GasId(0),
		minimum_moles: -0.1,
	}]
	.into_boxed_slice();
	assert_eq!(
		ReactionMetadataRegistry::try_new(vec![invalid_amount], &gases).unwrap_err(),
		ReactionMetadataError::InvalidRequiredMoles {
			reaction: ReactionId(0),
			gas: GasId(0),
		}
	);
}

#[test]
fn world_installs_reactions_once_after_gases_and_before_mixtures() {
	let mut world = DogmosWorld::new(1024 * 1024);
	assert_eq!(
		world.install_reactions(vec![reaction(0, "one", 1.0)]),
		Err(WorldError::GasRegistryMissing)
	);
	world
		.install_gases(vec![gas(0, "o2"), gas(1, "plasma")])
		.unwrap();
	assert_eq!(
		world.install_reactions(vec![reaction(0, "one", f32::NAN)]),
		Err(WorldError::ReactionMetadata(
			ReactionMetadataError::InvalidPriority(ReactionId(0))
		))
	);
	assert_eq!(
		world.install_reactions(vec![reaction(0, "one", 1.0)]),
		Ok(1)
	);
	assert_eq!(
		world.reaction_registry().unwrap().by_key("one").unwrap().id,
		ReactionId(0)
	);
	assert_eq!(
		world.install_reactions(vec![reaction(0, "two", 2.0)]),
		Err(WorldError::ReactionRegistryAlreadyInstalled)
	);

	let mut late_world = DogmosWorld::new(1024 * 1024);
	late_world.install_gases(vec![gas(0, "o2")]).unwrap();
	late_world
		.apply_lifecycle(&[LifecycleMutation {
			action: LifecycleAction::Register,
			handle: MixtureHandle {
				slot: 0,
				generation: 1,
			},
		}])
		.unwrap();
	assert_eq!(
		late_world.install_reactions(vec![reaction(0, "one", 1.0)]),
		Err(WorldError::ReactionRegistryInstallationTooLate)
	);
}

#[test]
fn world_scans_reactions_in_priority_order_without_reallocating_output() {
	let mut oxygen = gas(0, "o2");
	oxygen.fire_role = GasFireRole::Oxidizer {
		minimum_temperature: 300.0,
		power: 2.0,
	};
	let mut plasma = gas(1, "plasma");
	plasma.fire_role = GasFireRole::Fuel {
		minimum_temperature: 300.0,
		burn_rate: 2.0,
	};

	let low = reaction(0, "low", 1.0);
	let mut high = reaction(1, "high", 3.0);
	high.minimum_temperature = Some(400.0);
	high.maximum_temperature = Some(400.0);
	high.minimum_energy = Some(48_000.0);
	high.minimum_fire_reagents = Some(0.5);
	high.gas_requirements = vec![GasRequirement {
		gas: GasId(0),
		minimum_moles: 2.0,
	}]
	.into_boxed_slice();
	let mut blocked = reaction(2, "blocked", 2.0);
	blocked.gas_requirements = vec![GasRequirement {
		gas: GasId(1),
		minimum_moles: 5.0,
	}]
	.into_boxed_slice();

	let handle = MixtureHandle {
		slot: 0,
		generation: 1,
	};
	let mut world = DogmosWorld::new(1024 * 1024);
	world.install_gases(vec![oxygen, plasma]).unwrap();
	world.install_reactions(vec![low, high, blocked]).unwrap();
	world
		.apply_lifecycle(&[LifecycleMutation {
			action: LifecycleAction::Register,
			handle,
		}])
		.unwrap();
	let mut mixture_gases = [0.0; dogmos_core::MAX_GAS_SLOTS];
	mixture_gases[0] = 2.0;
	mixture_gases[1] = 4.0;
	world
		.apply_mixture_state(&[MixtureStateMutation {
			handle,
			expected_revision: 0,
			temperature: 400.0,
			volume: 2_500.0,
			gases: mixture_gases,
		}])
		.unwrap();

	let mut output = Vec::with_capacity(3);
	let allocation = output.as_ptr();
	assert_eq!(world.reactable_reactions_into(handle, &mut output), Ok(2));
	assert_eq!(output, [ReactionId(1), ReactionId(0)]);
	assert_eq!(output.as_ptr(), allocation);
}

#[test]
fn failed_reaction_scan_clears_caller_output() {
	let mut world = DogmosWorld::new(1024 * 1024);
	world.install_gases(vec![gas(0, "o2")]).unwrap();
	let mut output = vec![ReactionId(99)];

	assert_eq!(
		world.reactable_reactions_into(
			MixtureHandle {
				slot: 0,
				generation: 1,
			},
			&mut output,
		),
		Err(WorldError::ReactionRegistryMissing)
	);
	assert!(output.is_empty());
}

#[test]
fn fire_requirements_preserve_strict_ignition_and_minimum_mole_boundaries() {
	let mut oxygen = gas(0, "o2");
	oxygen.fire_role = GasFireRole::Oxidizer {
		minimum_temperature: 300.0,
		power: 1.0,
	};
	let mut plasma = gas(1, "plasma");
	plasma.fire_role = GasFireRole::Fuel {
		minimum_temperature: 300.0,
		burn_rate: 1.0,
	};
	let mut fire = reaction(0, "fire", 1.0);
	fire.minimum_fire_reagents = Some(0.00000001);

	let handle = MixtureHandle {
		slot: 0,
		generation: 1,
	};
	let mut world = DogmosWorld::new(1024 * 1024);
	world.install_gases(vec![oxygen, plasma]).unwrap();
	world.install_reactions(vec![fire]).unwrap();
	world
		.apply_lifecycle(&[LifecycleMutation {
			action: LifecycleAction::Register,
			handle,
		}])
		.unwrap();

	let mut output = Vec::with_capacity(1);
	let mut mixture_gases = [0.0; dogmos_core::MAX_GAS_SLOTS];
	mixture_gases[0] = 1.0;
	mixture_gases[1] = 1.0;
	world
		.apply_mixture_state(&[MixtureStateMutation {
			handle,
			expected_revision: 0,
			temperature: 300.0,
			volume: 2_500.0,
			gases: mixture_gases,
		}])
		.unwrap();
	assert_eq!(world.reactable_reactions_into(handle, &mut output), Ok(0));

	world
		.apply_mixture_state(&[MixtureStateMutation {
			handle,
			expected_revision: 1,
			temperature: f32::from_bits(300.0_f32.to_bits() + 1),
			volume: 2_500.0,
			gases: mixture_gases,
		}])
		.unwrap();
	assert_eq!(world.reactable_reactions_into(handle, &mut output), Ok(1));

	mixture_gases[0] = 0.0001;
	world
		.apply_mixture_state(&[MixtureStateMutation {
			handle,
			expected_revision: 2,
			temperature: 400.0,
			volume: 2_500.0,
			gases: mixture_gases,
		}])
		.unwrap();
	assert_eq!(world.reactable_reactions_into(handle, &mut output), Ok(0));

	mixture_gases[0] = f32::from_bits(0.0001_f32.to_bits() + 1);
	world
		.apply_mixture_state(&[MixtureStateMutation {
			handle,
			expected_revision: 3,
			temperature: 400.0,
			volume: 2_500.0,
			gases: mixture_gases,
		}])
		.unwrap();
	assert_eq!(world.reactable_reactions_into(handle, &mut output), Ok(1));
}
