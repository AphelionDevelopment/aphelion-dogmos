use super::*;

pub const MAX_METADATA_KEY_BYTES: usize = 64;
pub const MAX_METADATA_NAME_BYTES: usize = 128;
pub const MAX_REACTION_METADATA: u32 = 1024;
pub const GAS_METADATA_RECORD_LEN: usize = 784;
pub const REACTION_METADATA_RECORD_LEN: usize = 632;

#[derive(Clone, Debug, PartialEq)]
pub enum WireGasFireRole {
	None,
	Oxidizer {
		minimum_temperature: ScalarValue,
		power: ScalarValue,
	},
	Fuel {
		minimum_temperature: ScalarValue,
		burn_rate: ScalarValue,
	},
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WireGasProduct {
	pub gas_id: u16,
	pub ratio: ScalarValue,
}

#[derive(Clone, Debug, PartialEq)]
pub enum WireFireProducts {
	Generic(Vec<WireGasProduct>),
	Plasma,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GasMetadataRegistration {
	pub id: u16,
	pub key: String,
	pub name: String,
	pub flags: u32,
	pub specific_heat: ScalarValue,
	pub fusion_power: ScalarValue,
	pub moles_visible: Option<ScalarValue>,
	pub enthalpy: ScalarValue,
	pub fire_radiation_released: ScalarValue,
	pub fire_role: WireGasFireRole,
	pub fire_products: Option<WireFireProducts>,
}

pub fn encode_gas_metadata_batch(
	entries: &[GasMetadataRegistration],
	output: &mut Vec<u8>,
) -> Result<(), ProtocolError> {
	let count =
		u32::try_from(entries.len()).map_err(|_| ProtocolError::OperationCountExceeded {
			actual: u32::MAX,
			maximum: MAX_GAS_SLOTS as u32,
		})?;
	if count > MAX_GAS_SLOTS as u32 {
		return Err(ProtocolError::OperationCountExceeded {
			actual: count,
			maximum: MAX_GAS_SLOTS as u32,
		});
	}
	output.clear();
	output.resize(4 + entries.len() * GAS_METADATA_RECORD_LEN, 0);
	output[0..4].copy_from_slice(&count.to_le_bytes());
	for (index, entry) in entries.iter().enumerate() {
		encode_gas_metadata(
			entry,
			&mut output[4 + index * GAS_METADATA_RECORD_LEN..][..GAS_METADATA_RECORD_LEN],
		)?;
	}
	Ok(())
}

pub fn decode_gas_metadata_batch(
	input: &[u8],
) -> Result<Vec<GasMetadataRegistration>, ProtocolError> {
	let count = validate_counted_payload(input, GAS_METADATA_RECORD_LEN, MAX_GAS_SLOTS as u32)?;
	(0..count as usize)
		.map(|index| {
			let offset = 4 + index * GAS_METADATA_RECORD_LEN;
			decode_gas_metadata(&input[offset..offset + GAS_METADATA_RECORD_LEN])
		})
		.collect()
}

fn encode_gas_metadata(
	entry: &GasMetadataRegistration,
	output: &mut [u8],
) -> Result<(), ProtocolError> {
	let key = bounded_bytes(&entry.key, MAX_METADATA_KEY_BYTES)?;
	let name = bounded_bytes(&entry.name, MAX_METADATA_NAME_BYTES)?;
	let (fire_kind, fire_values) = match entry.fire_role {
		WireGasFireRole::None => (0_u16, [ScalarValue(0.0); 2]),
		WireGasFireRole::Oxidizer {
			minimum_temperature,
			power,
		} => (1, [minimum_temperature, power]),
		WireGasFireRole::Fuel {
			minimum_temperature,
			burn_rate,
		} => (2, [minimum_temperature, burn_rate]),
	};
	let (product_kind, products): (u32, &[WireGasProduct]) = match &entry.fire_products {
		None => (0, &[]),
		Some(WireFireProducts::Generic(products)) => (1, products),
		Some(WireFireProducts::Plasma) => (2, &[]),
	};
	if products.len() > MAX_GAS_SLOTS {
		return Err(ProtocolError::OperationCountExceeded {
			actual: products.len().min(u32::MAX as usize) as u32,
			maximum: MAX_GAS_SLOTS as u32,
		});
	}
	output.fill(0);
	output[0..2].copy_from_slice(&entry.id.to_le_bytes());
	output[2..4].copy_from_slice(&(key.len() as u16).to_le_bytes());
	output[4..6].copy_from_slice(&(name.len() as u16).to_le_bytes());
	output[6..8].copy_from_slice(&fire_kind.to_le_bytes());
	output[8..12].copy_from_slice(&entry.flags.to_le_bytes());
	output[12..16].copy_from_slice(&product_kind.to_le_bytes());
	output[16..20].copy_from_slice(&(products.len() as u32).to_le_bytes());
	output[20..24].copy_from_slice(&u32::from(entry.moles_visible.is_some()).to_le_bytes());
	let scalars = [
		entry.specific_heat,
		entry.fusion_power,
		entry.moles_visible.unwrap_or(ScalarValue(0.0)),
		entry.enthalpy,
		entry.fire_radiation_released,
		fire_values[0],
		fire_values[1],
	];
	for (index, scalar) in scalars.into_iter().enumerate() {
		let offset = 24 + index * 8;
		output[offset..offset + 8].copy_from_slice(&scalar.encode()?);
	}
	output[80..80 + key.len()].copy_from_slice(key);
	output[144..144 + name.len()].copy_from_slice(name);
	for (index, product) in products.iter().enumerate() {
		let offset = 272 + index * 16;
		output[offset..offset + 2].copy_from_slice(&product.gas_id.to_le_bytes());
		output[offset + 4..offset + 12].copy_from_slice(&product.ratio.encode()?);
	}
	Ok(())
}

fn decode_gas_metadata(input: &[u8]) -> Result<GasMetadataRegistration, ProtocolError> {
	let key_len = usize::from(read_u16(input, 2));
	let name_len = usize::from(read_u16(input, 4));
	validate_string_len(key_len, MAX_METADATA_KEY_BYTES)?;
	validate_string_len(name_len, MAX_METADATA_NAME_BYTES)?;
	let product_count = read_u32(input, 16);
	if product_count > MAX_GAS_SLOTS as u32 {
		return Err(ProtocolError::OperationCountExceeded {
			actual: product_count,
			maximum: MAX_GAS_SLOTS as u32,
		});
	}
	let metadata_flags = read_u32(input, 20);
	if metadata_flags & !1 != 0 {
		return Err(ProtocolError::UnknownMetadataFlags(metadata_flags));
	}
	require_zero(&input[80 + key_len..144])?;
	require_zero(&input[144 + name_len..272])?;
	let key = std::str::from_utf8(&input[80..80 + key_len])
		.map_err(|_| ProtocolError::InvalidMetadataUtf8)?
		.to_owned();
	let name = std::str::from_utf8(&input[144..144 + name_len])
		.map_err(|_| ProtocolError::InvalidMetadataUtf8)?
		.to_owned();
	let scalars = decode_scalars::<7>(input, 24)?;
	if metadata_flags & 1 == 0 {
		require_zero(&input[40..48])?;
	}
	let fire_role = match read_u16(input, 6) {
		0 => {
			require_zero(&input[64..80])?;
			WireGasFireRole::None
		}
		1 => WireGasFireRole::Oxidizer {
			minimum_temperature: scalars[5],
			power: scalars[6],
		},
		2 => WireGasFireRole::Fuel {
			minimum_temperature: scalars[5],
			burn_rate: scalars[6],
		},
		actual => return Err(ProtocolError::UnknownGasFireRole(actual)),
	};
	let mut products = Vec::with_capacity(product_count as usize);
	for index in 0..MAX_GAS_SLOTS {
		let offset = 272 + index * 16;
		if index < product_count as usize {
			if read_u16(input, offset + 2) != 0 || read_u32(input, offset + 12) != 0 {
				return Err(ProtocolError::NonZeroMetadataPadding);
			}
			products.push(WireGasProduct {
				gas_id: read_u16(input, offset),
				ratio: ScalarValue::decode(&input[offset + 4..offset + 12])?,
			});
		} else {
			require_zero(&input[offset..offset + 16])?;
		}
	}
	let fire_products = match read_u32(input, 12) {
		0 if products.is_empty() => None,
		1 => Some(WireFireProducts::Generic(products)),
		2 if products.is_empty() => Some(WireFireProducts::Plasma),
		actual => return Err(ProtocolError::UnknownFireProducts(actual)),
	};
	Ok(GasMetadataRegistration {
		id: read_u16(input, 0),
		key,
		name,
		flags: read_u32(input, 8),
		specific_heat: scalars[0],
		fusion_power: scalars[1],
		moles_visible: (metadata_flags & 1 != 0).then_some(scalars[2]),
		enthalpy: scalars[3],
		fire_radiation_released: scalars[4],
		fire_role,
		fire_products,
	})
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WireReactionExecution {
	Dm,
	NativePlasma,
	NativeHydrogen,
	NativeTritium,
	NativeFreon,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WireGasRequirement {
	pub gas_id: u16,
	pub minimum_moles: ScalarValue,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReactionMetadataRegistration {
	pub id: u32,
	pub key: String,
	pub priority: ScalarValue,
	pub minimum_temperature: Option<ScalarValue>,
	pub maximum_temperature: Option<ScalarValue>,
	pub minimum_energy: Option<ScalarValue>,
	pub minimum_fire_reagents: Option<ScalarValue>,
	pub gas_requirements: Vec<WireGasRequirement>,
	pub execution: WireReactionExecution,
}

pub fn encode_reaction_metadata_batch(
	entries: &[ReactionMetadataRegistration],
	output: &mut Vec<u8>,
) -> Result<(), ProtocolError> {
	let count =
		u32::try_from(entries.len()).map_err(|_| ProtocolError::OperationCountExceeded {
			actual: u32::MAX,
			maximum: MAX_REACTION_METADATA,
		})?;
	if count > MAX_REACTION_METADATA {
		return Err(ProtocolError::OperationCountExceeded {
			actual: count,
			maximum: MAX_REACTION_METADATA,
		});
	}
	output.clear();
	output.resize(4 + entries.len() * REACTION_METADATA_RECORD_LEN, 0);
	output[0..4].copy_from_slice(&count.to_le_bytes());
	for (index, entry) in entries.iter().enumerate() {
		encode_reaction_metadata(
			entry,
			&mut output[4 + index * REACTION_METADATA_RECORD_LEN..][..REACTION_METADATA_RECORD_LEN],
		)?;
	}
	Ok(())
}

pub fn decode_reaction_metadata_batch(
	input: &[u8],
) -> Result<Vec<ReactionMetadataRegistration>, ProtocolError> {
	let count =
		validate_counted_payload(input, REACTION_METADATA_RECORD_LEN, MAX_REACTION_METADATA)?;
	(0..count as usize)
		.map(|index| {
			let offset = 4 + index * REACTION_METADATA_RECORD_LEN;
			decode_reaction_metadata(&input[offset..offset + REACTION_METADATA_RECORD_LEN])
		})
		.collect()
}

fn encode_reaction_metadata(
	entry: &ReactionMetadataRegistration,
	output: &mut [u8],
) -> Result<(), ProtocolError> {
	let key = bounded_bytes(&entry.key, MAX_METADATA_KEY_BYTES)?;
	if entry.gas_requirements.len() > MAX_GAS_SLOTS {
		return Err(ProtocolError::OperationCountExceeded {
			actual: entry.gas_requirements.len().min(u32::MAX as usize) as u32,
			maximum: MAX_GAS_SLOTS as u32,
		});
	}
	let execution = match entry.execution {
		WireReactionExecution::Dm => 0_u16,
		WireReactionExecution::NativePlasma => 1,
		WireReactionExecution::NativeHydrogen => 2,
		WireReactionExecution::NativeTritium => 3,
		WireReactionExecution::NativeFreon => 4,
	};
	let options = [
		entry.minimum_temperature,
		entry.maximum_temperature,
		entry.minimum_energy,
		entry.minimum_fire_reagents,
	];
	let flags = options
		.iter()
		.enumerate()
		.fold(0_u32, |flags, (index, value)| {
			flags | (u32::from(value.is_some()) << index)
		});
	output.fill(0);
	output[0..4].copy_from_slice(&entry.id.to_le_bytes());
	output[4..6].copy_from_slice(&(key.len() as u16).to_le_bytes());
	output[6..8].copy_from_slice(&execution.to_le_bytes());
	output[8..12].copy_from_slice(&flags.to_le_bytes());
	output[12..16].copy_from_slice(&(entry.gas_requirements.len() as u32).to_le_bytes());
	output[16..24].copy_from_slice(&entry.priority.encode()?);
	for (index, value) in options.into_iter().enumerate() {
		output[24 + index * 8..32 + index * 8]
			.copy_from_slice(&value.unwrap_or(ScalarValue(0.0)).encode()?);
	}
	output[56..56 + key.len()].copy_from_slice(key);
	for (index, requirement) in entry.gas_requirements.iter().enumerate() {
		let offset = 120 + index * 16;
		output[offset..offset + 2].copy_from_slice(&requirement.gas_id.to_le_bytes());
		output[offset + 4..offset + 12].copy_from_slice(&requirement.minimum_moles.encode()?);
	}
	Ok(())
}

fn decode_reaction_metadata(input: &[u8]) -> Result<ReactionMetadataRegistration, ProtocolError> {
	let key_len = usize::from(read_u16(input, 4));
	validate_string_len(key_len, MAX_METADATA_KEY_BYTES)?;
	require_zero(&input[56 + key_len..120])?;
	let key = std::str::from_utf8(&input[56..56 + key_len])
		.map_err(|_| ProtocolError::InvalidMetadataUtf8)?
		.to_owned();
	let flags = read_u32(input, 8);
	if flags & !0xf != 0 {
		return Err(ProtocolError::UnknownMetadataFlags(flags));
	}
	let requirement_count = read_u32(input, 12);
	if requirement_count > MAX_GAS_SLOTS as u32 {
		return Err(ProtocolError::OperationCountExceeded {
			actual: requirement_count,
			maximum: MAX_GAS_SLOTS as u32,
		});
	}
	let priority = ScalarValue::decode(&input[16..24])?;
	let options = decode_scalars::<4>(input, 24)?;
	for index in 0..options.len() {
		if flags & (1 << index) == 0 {
			require_zero(&input[24 + index * 8..32 + index * 8])?;
		}
	}
	let execution = match read_u16(input, 6) {
		0 => WireReactionExecution::Dm,
		1 => WireReactionExecution::NativePlasma,
		2 => WireReactionExecution::NativeHydrogen,
		3 => WireReactionExecution::NativeTritium,
		4 => WireReactionExecution::NativeFreon,
		actual => return Err(ProtocolError::UnknownReactionExecution(actual)),
	};
	let mut gas_requirements = Vec::with_capacity(requirement_count as usize);
	for index in 0..MAX_GAS_SLOTS {
		let offset = 120 + index * 16;
		if index < requirement_count as usize {
			if read_u16(input, offset + 2) != 0 || read_u32(input, offset + 12) != 0 {
				return Err(ProtocolError::NonZeroMetadataPadding);
			}
			gas_requirements.push(WireGasRequirement {
				gas_id: read_u16(input, offset),
				minimum_moles: ScalarValue::decode(&input[offset + 4..offset + 12])?,
			});
		} else {
			require_zero(&input[offset..offset + 16])?;
		}
	}
	Ok(ReactionMetadataRegistration {
		id: read_u32(input, 0),
		key,
		priority,
		minimum_temperature: option_from_flag(flags, 0, options[0]),
		maximum_temperature: option_from_flag(flags, 1, options[1]),
		minimum_energy: option_from_flag(flags, 2, options[2]),
		minimum_fire_reagents: option_from_flag(flags, 3, options[3]),
		gas_requirements,
		execution,
	})
}

fn bounded_bytes(value: &str, maximum: usize) -> Result<&[u8], ProtocolError> {
	validate_string_len(value.len(), maximum)?;
	Ok(value.as_bytes())
}
fn validate_string_len(actual: usize, maximum: usize) -> Result<(), ProtocolError> {
	if actual > maximum {
		return Err(ProtocolError::MetadataStringTooLong {
			actual: actual.min(u32::MAX as usize) as u32,
			maximum: maximum as u32,
		});
	}
	Ok(())
}
fn require_zero(bytes: &[u8]) -> Result<(), ProtocolError> {
	if bytes.iter().any(|byte| *byte != 0) {
		return Err(ProtocolError::NonZeroMetadataPadding);
	}
	Ok(())
}
fn decode_scalars<const COUNT: usize>(
	input: &[u8],
	start: usize,
) -> Result<[ScalarValue; COUNT], ProtocolError> {
	let mut values = [ScalarValue(0.0); COUNT];
	for (index, value) in values.iter_mut().enumerate() {
		let offset = start + index * 8;
		*value = ScalarValue::decode(&input[offset..offset + 8])?;
	}
	Ok(values)
}
fn option_from_flag(flags: u32, bit: u32, value: ScalarValue) -> Option<ScalarValue> {
	(flags & (1 << bit) != 0).then_some(value)
}
