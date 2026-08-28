use dogmos_core::{
	metadata::{
		FireProductRule, GasFireRole, GasId, GasMetadata, GasMetadataError, GasMetadataRegistry,
		GasProduct, ReactionId, TurfHandle,
	},
	world::{DogmosWorld, LifecycleAction, LifecycleMutation, WorldError},
	MixtureHandle, MAX_GAS_SLOTS,
};

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

#[test]
fn identities_have_fixed_width_and_preserve_generation() {
	assert_eq!(std::mem::size_of::<GasId>(), 2);
	assert_eq!(std::mem::size_of::<ReactionId>(), 4);
	assert_eq!(std::mem::size_of::<TurfHandle>(), 8);
	assert_eq!(
		TurfHandle {
			slot: 41,
			generation: 9,
		},
		TurfHandle {
			slot: 41,
			generation: 9,
		}
	);
}

#[test]
fn registry_freezes_dense_numeric_ids_and_cached_specific_heats() {
	assert!(GasMetadataRegistry::try_new(Vec::new()).unwrap().is_empty());
	let registry =
		GasMetadataRegistry::try_new(vec![gas(1, "n2", 20.0), gas(0, "o2", 20.0)]).unwrap();

	assert_eq!(registry.len(), 2);
	assert_eq!(registry.by_id(GasId(0)).unwrap().key.as_ref(), "o2");
	assert_eq!(registry.by_key("n2").unwrap().id, GasId(1));
	assert_eq!(registry.specific_heats(), &[20.0, 20.0]);
}

#[test]
fn registry_rejects_duplicate_or_non_dense_identity() {
	assert_eq!(
		GasMetadataRegistry::try_new(vec![gas(0, "o2", 20.0), gas(0, "n2", 20.0)]).unwrap_err(),
		GasMetadataError::DuplicateGasId(GasId(0))
	);
	assert_eq!(
		GasMetadataRegistry::try_new(vec![gas(0, "o2", 20.0), gas(1, "o2", 20.0)]).unwrap_err(),
		GasMetadataError::DuplicateGasKey("o2".into())
	);
	assert_eq!(
		GasMetadataRegistry::try_new(vec![gas(1, "n2", 20.0)]).unwrap_err(),
		GasMetadataError::NonDenseGasId {
			expected: GasId(0),
			actual: GasId(1),
		}
	);
}

#[test]
fn registry_rejects_more_gases_than_the_mixture_layout() {
	let gases = (0..=MAX_GAS_SLOTS as u16)
		.map(|id| gas(id, &format!("gas-{id}"), 20.0))
		.collect();

	assert_eq!(
		GasMetadataRegistry::try_new(gases).unwrap_err(),
		GasMetadataError::TooManyGases {
			count: MAX_GAS_SLOTS as u32 + 1,
			maximum: MAX_GAS_SLOTS as u32,
		}
	);
}

#[test]
fn registry_rejects_invalid_physical_metadata() {
	assert_eq!(
		GasMetadataRegistry::try_new(vec![gas(0, "o2", 0.0)]).unwrap_err(),
		GasMetadataError::InvalidSpecificHeat(GasId(0))
	);
	let mut invalid_visibility = gas(0, "o2", 20.0);
	invalid_visibility.moles_visible = Some(f32::NAN);
	assert_eq!(
		GasMetadataRegistry::try_new(vec![invalid_visibility]).unwrap_err(),
		GasMetadataError::InvalidMolesVisible(GasId(0))
	);

	let mut invalid_fusion = gas(0, "o2", 20.0);
	invalid_fusion.fusion_power = f32::INFINITY;
	assert_eq!(
		GasMetadataRegistry::try_new(vec![invalid_fusion]).unwrap_err(),
		GasMetadataError::InvalidFusionPower(GasId(0))
	);
	let mut invalid_enthalpy = gas(0, "o2", 20.0);
	invalid_enthalpy.enthalpy = f32::NAN;
	assert_eq!(
		GasMetadataRegistry::try_new(vec![invalid_enthalpy]).unwrap_err(),
		GasMetadataError::InvalidEnthalpy(GasId(0))
	);

	let mut invalid_radiation = gas(0, "o2", 20.0);
	invalid_radiation.fire_radiation_released = -1.0;
	assert_eq!(
		GasMetadataRegistry::try_new(vec![invalid_radiation]).unwrap_err(),
		GasMetadataError::InvalidFireRadiation(GasId(0))
	);

	let mut invalid_role = gas(0, "o2", 20.0);
	invalid_role.fire_role = GasFireRole::Oxidizer {
		minimum_temperature: 300.0,
		power: f32::NAN,
	};
	assert_eq!(
		GasMetadataRegistry::try_new(vec![invalid_role]).unwrap_err(),
		GasMetadataError::InvalidFireRole(GasId(0))
	);
	let mut zero_burn_rate = gas(0, "plasma", 200.0);
	zero_burn_rate.fire_role = GasFireRole::Fuel {
		minimum_temperature: 300.0,
		burn_rate: 0.0,
	};
	assert_eq!(
		GasMetadataRegistry::try_new(vec![zero_burn_rate]).unwrap_err(),
		GasMetadataError::InvalidFireRole(GasId(0))
	);

	assert_eq!(
		GasMetadataRegistry::try_new(vec![gas(0, "", 20.0)]).unwrap_err(),
		GasMetadataError::EmptyGasKey(GasId(0))
	);
}

#[test]
fn registry_resolves_and_validates_generic_fire_products() {
	let mut plasma = gas(1, "plasma", 200.0);
	plasma.fire_role = GasFireRole::Fuel {
		minimum_temperature: 373.15,
		burn_rate: 0.4,
	};
	plasma.fire_products = Some(FireProductRule::Generic(
		vec![GasProduct {
			gas: GasId(0),
			ratio: 0.75,
		}]
		.into_boxed_slice(),
	));
	let registry = GasMetadataRegistry::try_new(vec![gas(0, "co2", 30.0), plasma]).unwrap();

	assert_eq!(
		registry.by_id(GasId(1)).unwrap().fire_products,
		Some(FireProductRule::Generic(
			vec![GasProduct {
				gas: GasId(0),
				ratio: 0.75,
			}]
			.into_boxed_slice(),
		))
	);

	let mut invalid = gas(0, "plasma", 200.0);
	invalid.fire_products = Some(FireProductRule::Generic(
		vec![GasProduct {
			gas: GasId(1),
			ratio: 1.0,
		}]
		.into_boxed_slice(),
	));
	assert_eq!(
		GasMetadataRegistry::try_new(vec![invalid]).unwrap_err(),
		GasMetadataError::UnknownFireProduct {
			gas: GasId(0),
			product: GasId(1),
		}
	);

	let mut invalid_ratio = gas(0, "co2", 30.0);
	invalid_ratio.fire_products = Some(FireProductRule::Generic(
		vec![GasProduct {
			gas: GasId(0),
			ratio: -0.1,
		}]
		.into_boxed_slice(),
	));
	assert_eq!(
		GasMetadataRegistry::try_new(vec![invalid_ratio]).unwrap_err(),
		GasMetadataError::InvalidFireProductRatio {
			gas: GasId(0),
			product: GasId(0),
		}
	);

	let mut duplicate_product = gas(0, "co2", 30.0);
	duplicate_product.fire_products = Some(FireProductRule::Generic(
		vec![
			GasProduct {
				gas: GasId(0),
				ratio: 0.5,
			},
			GasProduct {
				gas: GasId(0),
				ratio: 0.25,
			},
		]
		.into_boxed_slice(),
	));
	assert_eq!(
		GasMetadataRegistry::try_new(vec![duplicate_product]).unwrap_err(),
		GasMetadataError::DuplicateFireProduct {
			gas: GasId(0),
			product: GasId(0),
		}
	);
}

#[test]
fn world_owns_one_immutable_gas_registry() {
	let mut world = DogmosWorld::new(1024 * 1024);
	assert_eq!(world.install_gases(vec![gas(0, "o2", 20.0)]), Ok(1));
	assert_eq!(
		world.gas_registry().unwrap().by_key("o2").unwrap().id,
		GasId(0)
	);
	assert_eq!(
		world.install_gases(vec![gas(0, "n2", 20.0)]),
		Err(WorldError::GasRegistryAlreadyInstalled)
	);
	assert_eq!(world.gas_registry().unwrap().by_key("n2"), None);
}

#[test]
fn world_rejects_late_registration_but_allows_retry_after_invalid_metadata() {
	let mut retry_world = DogmosWorld::new(1024 * 1024);
	assert_eq!(
		retry_world.install_gases(vec![gas(0, "o2", 0.0)]),
		Err(WorldError::GasMetadata(
			GasMetadataError::InvalidSpecificHeat(GasId(0))
		))
	);
	assert_eq!(retry_world.install_gases(vec![gas(0, "o2", 20.0)]), Ok(1));

	let mut late_world = DogmosWorld::new(1024 * 1024);
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
		late_world.install_gases(vec![gas(0, "o2", 20.0)]),
		Err(WorldError::GasRegistryInstallationTooLate)
	);
}
