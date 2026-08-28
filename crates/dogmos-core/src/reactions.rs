use crate::{
	metadata::{GasMetadataRegistry, NativeReactionKind},
	MAX_GAS_SLOTS,
};
use std::{error::Error, fmt};

const MINIMUM_TEMPERATURE_K: f32 = 2.7;
const MINIMUM_HEAT_CAPACITY: f32 = 0.0003;
const MOLAR_ACCURACY: f32 = 0.0001;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeReactionResult {
	pub values: [f32; 4],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeReactionError {
	MissingGas(&'static str),
	NonFiniteResult,
}

impl fmt::Display for NativeReactionError {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(formatter, "{self:?}")
	}
}

impl Error for NativeReactionError {}

pub fn execute_native(
	kind: NativeReactionKind,
	gases: &mut [f32; MAX_GAS_SLOTS],
	temperature: &mut f32,
	volume: f32,
	minimum_heat_capacity: f32,
	registry: &GasMetadataRegistry,
) -> Result<Option<NativeReactionResult>, NativeReactionError> {
	let result = match kind {
		NativeReactionKind::Plasma => plasma(gases, temperature, minimum_heat_capacity, registry)?,
		NativeReactionKind::Hydrogen => {
			hydrogen(gases, temperature, minimum_heat_capacity, registry)?
		}
		NativeReactionKind::Tritium => {
			tritium(gases, temperature, volume, minimum_heat_capacity, registry)?
		}
		NativeReactionKind::Freon => freon(gases, temperature, minimum_heat_capacity, registry)?,
	};
	if !temperature.is_finite()
		|| gases
			.iter()
			.any(|amount| !amount.is_finite() || *amount < 0.0)
		|| result
			.as_ref()
			.is_some_and(|result| result.values.iter().any(|value| !value.is_finite()))
	{
		return Err(NativeReactionError::NonFiniteResult);
	}
	Ok(result)
}

fn plasma(
	gases: &mut [f32; MAX_GAS_SLOTS],
	temperature: &mut f32,
	minimum_heat_capacity: f32,
	registry: &GasMetadataRegistry,
) -> Result<Option<NativeReactionResult>, NativeReactionError> {
	const MINIMUM_BURN_TEMPERATURE: f32 = 373.15;
	const UPPER_TEMPERATURE: f32 = 1643.15;
	const OXYGEN_BURN_RATIO_BASE: f32 = 1.4;
	const OXYGEN_FULL_BURN: f32 = 10.0;
	const SUPER_SATURATION_THRESHOLD: f32 = 96.0;
	const BURN_RATE_DELTA: f32 = 9.0;
	const ENERGY_RELEASED: f32 = 3.0e6;

	let oxygen = gas_index(registry, "o2")?;
	let plasma = gas_index(registry, "plasma")?;
	let carbon_dioxide = gas_index(registry, "co2")?;
	let tritium = gas_index(registry, "tritium")?;
	let water = gas_index(registry, "water_vapor")?;
	let temperature_scale = if *temperature > UPPER_TEMPERATURE {
		1.0
	} else {
		(*temperature - MINIMUM_BURN_TEMPERATURE) / (UPPER_TEMPERATURE - MINIMUM_BURN_TEMPERATURE)
	};
	if temperature_scale <= 0.0 || gases[plasma] <= 0.0 {
		return Ok(None);
	}
	let oxygen_burn_ratio = OXYGEN_BURN_RATIO_BASE - temperature_scale;
	let oxygen_moles = gases[oxygen];
	let plasma_moles = gases[plasma];
	let ratio = oxygen_moles / plasma_moles;
	let (burn_rate, super_saturation) = if ratio >= SUPER_SATURATION_THRESHOLD {
		(plasma_moles / BURN_RATE_DELTA * temperature_scale, true)
	} else if ratio >= OXYGEN_FULL_BURN {
		(plasma_moles / BURN_RATE_DELTA * temperature_scale, false)
	} else {
		(
			(oxygen_moles / OXYGEN_FULL_BURN) / BURN_RATE_DELTA * temperature_scale,
			false,
		)
	};
	if burn_rate < MINIMUM_HEAT_CAPACITY {
		return Ok(None);
	}
	let old_capacity = heat_capacity(gases, minimum_heat_capacity, registry);
	let burn_rate = burn_rate
		.min(plasma_moles)
		.min(oxygen_moles / oxygen_burn_ratio);
	gases[plasma] = quantize(plasma_moles - burn_rate);
	gases[oxygen] = quantize(oxygen_moles - burn_rate * oxygen_burn_ratio);
	if super_saturation {
		gases[tritium] += burn_rate;
	} else {
		gases[carbon_dioxide] += burn_rate * 0.75;
		gases[water] += burn_rate * 0.25;
	}
	let fire_amount = burn_rate * (1.0 + oxygen_burn_ratio);
	let new_capacity = heat_capacity(gases, minimum_heat_capacity, registry);
	if new_capacity > MINIMUM_HEAT_CAPACITY {
		*temperature = (*temperature * old_capacity + ENERGY_RELEASED * burn_rate) / new_capacity;
	}
	Ok(Some(NativeReactionResult {
		values: [fire_amount, *temperature, 0.0, 0.0],
	}))
}

fn hydrogen(
	gases: &mut [f32; MAX_GAS_SLOTS],
	temperature: &mut f32,
	minimum_heat_capacity: f32,
	registry: &GasMetadataRegistry,
) -> Result<Option<NativeReactionResult>, NativeReactionError> {
	combust_fuel(
		"hydrogen",
		NativeReactionKind::Hydrogen,
		gases,
		temperature,
		0.0,
		minimum_heat_capacity,
		registry,
	)
}

fn tritium(
	gases: &mut [f32; MAX_GAS_SLOTS],
	temperature: &mut f32,
	volume: f32,
	minimum_heat_capacity: f32,
	registry: &GasMetadataRegistry,
) -> Result<Option<NativeReactionResult>, NativeReactionError> {
	combust_fuel(
		"tritium",
		NativeReactionKind::Tritium,
		gases,
		temperature,
		volume,
		minimum_heat_capacity,
		registry,
	)
}

fn combust_fuel(
	fuel_key: &'static str,
	kind: NativeReactionKind,
	gases: &mut [f32; MAX_GAS_SLOTS],
	temperature: &mut f32,
	volume: f32,
	minimum_heat_capacity: f32,
	registry: &GasMetadataRegistry,
) -> Result<Option<NativeReactionResult>, NativeReactionError> {
	const BURN_RATE_DELTA: f32 = 2.0;
	const OXYGEN_FULL_BURN: f32 = 10.0;
	const ENERGY_RELEASED: f32 = 2.8e6;
	let oxygen = gas_index(registry, "o2")?;
	let fuel = gas_index(registry, fuel_key)?;
	let water = gas_index(registry, "water_vapor")?;
	let fuel_moles = gases[fuel];
	let oxygen_moles = gases[oxygen];
	let burned_fuel = (fuel_moles / BURN_RATE_DELTA)
		.min(oxygen_moles / (BURN_RATE_DELTA * OXYGEN_FULL_BURN))
		.min(fuel_moles)
		.min(oxygen_moles / 0.5);
	if burned_fuel <= 0.0
		|| fuel_moles - burned_fuel < 0.0
		|| oxygen_moles - burned_fuel * 0.5 < 0.0
	{
		return Ok(None);
	}
	let old_capacity = heat_capacity(gases, minimum_heat_capacity, registry);
	let old_temperature = *temperature;
	gases[fuel] = (gases[fuel] - burned_fuel).max(0.0);
	gases[oxygen] = (gases[oxygen] - burned_fuel * 0.5).max(0.0);
	gases[water] += burned_fuel;
	let energy = ENERGY_RELEASED * burned_fuel;
	let new_capacity = heat_capacity(gases, minimum_heat_capacity, registry);
	if new_capacity > MINIMUM_HEAT_CAPACITY {
		*temperature = (old_temperature * old_capacity + energy) / new_capacity;
	}
	let values = match kind {
		NativeReactionKind::Hydrogen => [burned_fuel, *temperature, 0.0, 0.0],
		NativeReactionKind::Tritium => [burned_fuel, energy, volume, *temperature],
		NativeReactionKind::Plasma | NativeReactionKind::Freon => unreachable!(),
	};
	Ok(Some(NativeReactionResult { values }))
}

fn freon(
	gases: &mut [f32; MAX_GAS_SLOTS],
	temperature: &mut f32,
	minimum_heat_capacity: f32,
	registry: &GasMetadataRegistry,
) -> Result<Option<NativeReactionResult>, NativeReactionError> {
	const MAXIMUM_BURN_TEMPERATURE: f32 = 283.0;
	const LOWER_TEMPERATURE: f32 = 60.0;
	const TERMINAL_TEMPERATURE: f32 = 20.0;
	const OXYGEN_BURN_RATIO_BASE: f32 = 1.4;
	const OXYGEN_FULL_BURN: f32 = 10.0;
	const BURN_RATE_DELTA: f32 = 4.0;
	const ENERGY_CONSUMED: f32 = 3.0e5;
	let oxygen = gas_index(registry, "o2")?;
	let freon = gas_index(registry, "freon")?;
	let carbon_dioxide = gas_index(registry, "co2")?;
	let old_temperature = *temperature;
	let temperature_scale = if *temperature < TERMINAL_TEMPERATURE {
		0.0
	} else if *temperature < LOWER_TEMPERATURE {
		0.5
	} else {
		(MAXIMUM_BURN_TEMPERATURE - *temperature)
			/ (MAXIMUM_BURN_TEMPERATURE - TERMINAL_TEMPERATURE)
	};
	if temperature_scale <= 0.0 || gases[freon] <= 0.0 {
		return Ok(None);
	}
	let oxygen_burn_ratio = OXYGEN_BURN_RATIO_BASE - temperature_scale;
	let oxygen_moles = gases[oxygen];
	let freon_moles = gases[freon];
	let burn_rate = if oxygen_moles < freon_moles * OXYGEN_FULL_BURN {
		(oxygen_moles / OXYGEN_FULL_BURN) / BURN_RATE_DELTA * temperature_scale
	} else {
		freon_moles / BURN_RATE_DELTA * temperature_scale
	};
	if burn_rate < MINIMUM_HEAT_CAPACITY {
		return Ok(None);
	}
	let old_capacity = heat_capacity(gases, minimum_heat_capacity, registry);
	let burn_rate = burn_rate
		.min(freon_moles)
		.min(oxygen_moles / oxygen_burn_ratio);
	gases[freon] = quantize(freon_moles - burn_rate);
	gases[oxygen] = quantize(oxygen_moles - burn_rate * oxygen_burn_ratio);
	gases[carbon_dioxide] += burn_rate;
	let fire_amount = burn_rate * (1.0 + oxygen_burn_ratio);
	let new_capacity = heat_capacity(gases, minimum_heat_capacity, registry);
	if new_capacity > MINIMUM_HEAT_CAPACITY {
		*temperature = ((old_temperature * old_capacity - ENERGY_CONSUMED * burn_rate)
			/ new_capacity)
			.max(MINIMUM_TEMPERATURE_K);
	}
	Ok(Some(NativeReactionResult {
		values: [fire_amount, old_temperature, *temperature, 0.0],
	}))
}

fn gas_index(
	registry: &GasMetadataRegistry,
	key: &'static str,
) -> Result<usize, NativeReactionError> {
	registry
		.by_key(key)
		.map(|gas| usize::from(gas.id.0))
		.ok_or(NativeReactionError::MissingGas(key))
}

fn heat_capacity(
	gases: &[f32; MAX_GAS_SLOTS],
	minimum_heat_capacity: f32,
	registry: &GasMetadataRegistry,
) -> f32 {
	gases
		.iter()
		.zip(registry.specific_heats())
		.fold(0.0, |capacity, (amount, specific_heat)| {
			specific_heat.mul_add(*amount, capacity)
		})
		.max(minimum_heat_capacity)
}

fn quantize(amount: f32) -> f32 {
	(amount / MOLAR_ACCURACY).round() * MOLAR_ACCURACY
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::metadata::{GasFireRole, GasId, GasMetadata};

	fn registry(keys: &[&str]) -> GasMetadataRegistry {
		GasMetadataRegistry::try_new(
			keys.iter()
				.enumerate()
				.map(|(id, key)| GasMetadata {
					id: GasId(id as u16),
					key: (*key).into(),
					name: (*key).into(),
					flags: 0,
					specific_heat: 20.0,
					fusion_power: 0.0,
					moles_visible: None,
					enthalpy: 0.0,
					fire_radiation_released: 0.0,
					fire_role: GasFireRole::None,
					fire_products: None,
				})
				.collect(),
		)
		.unwrap()
	}

	fn close(actual: f32, expected: f32) {
		let tolerance = expected.abs().max(1.0) * 1.0e-6;
		assert!(
			(actual - expected).abs() <= tolerance,
			"{actual} differs from legacy golden {expected} by more than {tolerance}",
		);
	}

	#[test]
	fn hydrogen_matches_legacy_numeric_and_callback_golden() {
		let registry = registry(&["o2", "hydrogen", "water_vapor"]);
		let mut gases = [0.0; MAX_GAS_SLOTS];
		gases[0] = 100.0;
		gases[1] = 10.0;
		let mut temperature = 500.0;

		let result = execute_native(
			NativeReactionKind::Hydrogen,
			&mut gases,
			&mut temperature,
			2500.0,
			0.0,
			&registry,
		)
		.unwrap()
		.unwrap();

		close(gases[0], 97.5);
		close(gases[1], 5.0);
		close(gases[2], 5.0);
		close(temperature, 7_023.255_4);
		close(result.values[0], 5.0);
		close(result.values[1], temperature);
		assert_eq!(result.values[2..], [0.0, 0.0]);
	}

	#[test]
	fn tritium_matches_legacy_numeric_and_callback_golden() {
		let registry = registry(&["o2", "tritium", "water_vapor"]);
		let mut gases = [0.0; MAX_GAS_SLOTS];
		gases[0] = 100.0;
		gases[1] = 10.0;
		let mut temperature = 500.0;

		let result = execute_native(
			NativeReactionKind::Tritium,
			&mut gases,
			&mut temperature,
			2500.0,
			0.0,
			&registry,
		)
		.unwrap()
		.unwrap();

		close(gases[0], 97.5);
		close(gases[1], 5.0);
		close(gases[2], 5.0);
		close(result.values[0], 5.0);
		close(result.values[1], 14_000_000.0);
		close(result.values[2], 2500.0);
		close(result.values[3], temperature);
	}

	#[test]
	fn freon_matches_legacy_numeric_and_callback_golden() {
		let registry = registry(&["o2", "freon", "co2"]);
		let mut gases = [0.0; MAX_GAS_SLOTS];
		gases[0] = 100.0;
		gases[1] = 10.0;
		let mut temperature = 100.0;

		let result = execute_native(
			NativeReactionKind::Freon,
			&mut gases,
			&mut temperature,
			2500.0,
			0.0,
			&registry,
		)
		.unwrap()
		.unwrap();

		close(gases[0], 98.775_1);
		close(gases[1], 8.260_5);
		close(gases[2], 1.739_543_7);
		close(result.values[0], 2.964_5);
		close(result.values[1], 100.0);
		close(result.values[2], 2.7);
		assert_eq!(result.values[3], 0.0);
	}
}
