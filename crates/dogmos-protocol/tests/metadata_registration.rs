use dogmos_protocol::{
	decode_gas_metadata_batch, decode_reaction_metadata_batch, encode_gas_metadata_batch,
	encode_reaction_metadata_batch, GasMetadataRegistration, OperationKind, ProtocolError,
	ReactionMetadataRegistration, ScalarValue, WireFireProducts, WireGasFireRole, WireGasProduct,
	WireGasRequirement, WireReactionExecution, GAS_METADATA_RECORD_LEN, MAX_GAS_SLOTS,
	REACTION_METADATA_RECORD_LEN,
};

#[test]
fn metadata_operation_ids_and_fixed_records_are_stable() {
	assert_eq!(OperationKind::GasMetadataInstall as u16, 29);
	assert_eq!(OperationKind::ReactionMetadataInstall as u16, 30);
	assert_eq!(GAS_METADATA_RECORD_LEN, 784);
	assert_eq!(REACTION_METADATA_RECORD_LEN, 632);
}

#[test]
fn gas_metadata_round_trips_fixed_bounded_strings_and_products() {
	let gas = GasMetadataRegistration {
		id: 0,
		key: "o2".into(),
		name: "Oxygen".into(),
		flags: 3,
		specific_heat: ScalarValue(20.0),
		fusion_power: ScalarValue(0.0),
		moles_visible: Some(ScalarValue(0.25)),
		enthalpy: ScalarValue(1.0),
		fire_radiation_released: ScalarValue(2.0),
		fire_role: WireGasFireRole::Oxidizer {
			minimum_temperature: ScalarValue(300.0),
			power: ScalarValue(1.5),
		},
		fire_products: Some(WireFireProducts::Generic(vec![WireGasProduct {
			gas_id: 1,
			ratio: ScalarValue(0.5),
		}])),
	};
	let mut bytes = Vec::new();
	encode_gas_metadata_batch(std::slice::from_ref(&gas), &mut bytes).unwrap();
	assert_eq!(bytes.len(), 4 + GAS_METADATA_RECORD_LEN);
	assert_eq!(decode_gas_metadata_batch(&bytes).unwrap(), vec![gas]);

	bytes[4 + 80 + 2] = 1;
	assert_eq!(
		decode_gas_metadata_batch(&bytes),
		Err(ProtocolError::NonZeroMetadataPadding)
	);
}

#[test]
fn reaction_metadata_round_trips_optional_thresholds_and_requirements() {
	let reaction = ReactionMetadataRegistration {
		id: 0,
		key: "plasmafire".into(),
		priority: ScalarValue(10.0),
		minimum_temperature: Some(ScalarValue(373.15)),
		maximum_temperature: None,
		minimum_energy: Some(ScalarValue(1.0)),
		minimum_fire_reagents: None,
		gas_requirements: vec![WireGasRequirement {
			gas_id: 0,
			minimum_moles: ScalarValue(0.1),
		}],
		execution: WireReactionExecution::NativePlasma,
	};
	let mut bytes = Vec::new();
	encode_reaction_metadata_batch(std::slice::from_ref(&reaction), &mut bytes).unwrap();
	assert_eq!(bytes.len(), 4 + REACTION_METADATA_RECORD_LEN);
	assert_eq!(
		decode_reaction_metadata_batch(&bytes).unwrap(),
		vec![reaction]
	);
}

#[test]
fn metadata_codec_rejects_oversized_counts_and_non_finite_scalars() {
	let mut bytes = 1_u32.to_le_bytes().to_vec();
	bytes.resize(4 + GAS_METADATA_RECORD_LEN, 0);
	bytes[4 + 24..4 + 32].copy_from_slice(&f64::NAN.to_le_bytes());
	assert_eq!(
		decode_gas_metadata_batch(&bytes),
		Err(ProtocolError::NonFiniteScalar)
	);
	bytes[0..4].copy_from_slice(&((MAX_GAS_SLOTS as u32) + 1).to_le_bytes());
	assert!(matches!(
		decode_gas_metadata_batch(&bytes),
		Err(ProtocolError::OperationCountExceeded { .. })
	));
}

#[test]
fn metadata_codec_rejects_noncanonical_absent_values() {
	let gas = GasMetadataRegistration {
		id: 0,
		key: "o2".into(),
		name: "Oxygen".into(),
		flags: 0,
		specific_heat: ScalarValue(20.0),
		fusion_power: ScalarValue(0.0),
		moles_visible: None,
		enthalpy: ScalarValue(0.0),
		fire_radiation_released: ScalarValue(0.0),
		fire_role: WireGasFireRole::None,
		fire_products: None,
	};
	let mut gas_bytes = Vec::new();
	encode_gas_metadata_batch(&[gas], &mut gas_bytes).unwrap();
	gas_bytes[4 + 40..4 + 48].copy_from_slice(&(-0.0_f64).to_le_bytes());
	assert_eq!(
		decode_gas_metadata_batch(&gas_bytes),
		Err(ProtocolError::NonZeroMetadataPadding)
	);

	let reaction = ReactionMetadataRegistration {
		id: 0,
		key: "dm".into(),
		priority: ScalarValue(1.0),
		minimum_temperature: None,
		maximum_temperature: None,
		minimum_energy: None,
		minimum_fire_reagents: None,
		gas_requirements: Vec::new(),
		execution: WireReactionExecution::Dm,
	};
	let mut reaction_bytes = Vec::new();
	encode_reaction_metadata_batch(&[reaction], &mut reaction_bytes).unwrap();
	reaction_bytes[4 + 24..4 + 32].copy_from_slice(&(-0.0_f64).to_le_bytes());
	assert_eq!(
		decode_reaction_metadata_batch(&reaction_bytes),
		Err(ProtocolError::NonZeroMetadataPadding)
	);
}

#[test]
fn gas_metadata_without_a_fire_role_rejects_fire_role_values() {
	let gas = GasMetadataRegistration {
		id: 0,
		key: "o2".into(),
		name: "Oxygen".into(),
		flags: 0,
		specific_heat: ScalarValue(20.0),
		fusion_power: ScalarValue(0.0),
		moles_visible: None,
		enthalpy: ScalarValue(0.0),
		fire_radiation_released: ScalarValue(0.0),
		fire_role: WireGasFireRole::None,
		fire_products: None,
	};
	let mut bytes = Vec::new();
	encode_gas_metadata_batch(&[gas], &mut bytes).unwrap();
	bytes[4 + 64..4 + 72].copy_from_slice(&300.0_f64.to_le_bytes());
	assert_eq!(
		decode_gas_metadata_batch(&bytes),
		Err(ProtocolError::NonZeroMetadataPadding)
	);
}
