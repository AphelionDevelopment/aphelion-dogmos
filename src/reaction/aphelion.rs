//! Native implementations of the fire reactions selected by the `aphelion_reactions` feature.
//!
//! This module is separate from the Citadel and Yogs reaction sets because their formulas and
//! byproducts differ.
//!
//! DM remains responsible for holder lookup and side effects. Each reaction calls a small DM glue
//! proc after Rust computes its numeric results.

use crate::gas::{constants::*, gas_idx_from_string, with_mix, with_mix_mut};
use byondapi::prelude::*;
use eyre::Result;

const GAS_FREON: &str = "freon";
const GAS_HYDROGEN: &str = "hydrogen";

#[must_use]
pub fn func_from_id(id: &str) -> Option<super::ReactFunc> {
	match id {
		"plasmafire" => Some(plasma_fire),
		"h2fire" => Some(hydrogen_fire),
		"tritfire" => Some(tritium_fire),
		"freonfire" => Some(freon_fire),
		_ => None,
	}
}

/// DM's QUANTIZE(variable) macro: round(variable, MOLAR_ACCURACY).
fn quantize(amount: f32) -> f32 {
	(amount / MOLAR_ACCURACY).round() * MOLAR_ACCURACY
}

/// code/modules/atmospherics/gasmixtures/reactions.dm, /datum/gas_reaction/plasmafire/react().
fn plasma_fire(byond_air: ByondValue, holder: ByondValue) -> Result<ByondValue> {
	const PLASMA_MINIMUM_BURN_TEMPERATURE: f32 = FIRE_MINIMUM_TEMPERATURE_TO_EXIST;
	const PLASMA_UPPER_TEMPERATURE: f32 = PLASMA_MINIMUM_BURN_TEMPERATURE + 1270.0;
	const OXYGEN_BURN_RATIO_BASE: f32 = 1.4;
	const PLASMA_OXYGEN_FULLBURN: f32 = 10.0;
	const SUPER_SATURATION_THRESHOLD: f32 = 96.0;
	const PLASMA_BURN_RATE_DELTA: f32 = 9.0;
	const FIRE_PLASMA_ENERGY_RELEASED: f32 = 3e6;

	let o2 = gas_idx_from_string(GAS_O2)?;
	let plasma = gas_idx_from_string(GAS_PLASMA)?;
	let co2 = gas_idx_from_string(GAS_CO2)?;
	let tritium = gas_idx_from_string(GAS_TRITIUM)?;
	let water_vapor = gas_idx_from_string(GAS_H2O)?;

	struct PreMath {
		plasma_burn_rate: f32,
		oxygen_burn_ratio: f32,
		super_saturation: bool,
		oxygen_moles: f32,
		plasma_moles: f32,
		temperature: f32,
		old_heat_capacity: f32,
	}

	let pre = with_mix(&byond_air, |air| {
		let temperature = air.get_temperature();
		let temperature_scale = if temperature > PLASMA_UPPER_TEMPERATURE {
			1.0
		} else {
			let scale = (temperature - PLASMA_MINIMUM_BURN_TEMPERATURE)
				/ (PLASMA_UPPER_TEMPERATURE - PLASMA_MINIMUM_BURN_TEMPERATURE);
			if scale <= 0.0 {
				return Ok(None);
			}
			scale
		};
		let oxygen_burn_ratio = OXYGEN_BURN_RATIO_BASE - temperature_scale;
		let oxygen_moles = air.get_moles(o2);
		let plasma_moles = air.get_moles(plasma);
		let ratio = oxygen_moles / plasma_moles;
		let (plasma_burn_rate, super_saturation) = if ratio >= SUPER_SATURATION_THRESHOLD {
			(
				plasma_moles / PLASMA_BURN_RATE_DELTA * temperature_scale,
				true,
			)
		} else if ratio >= PLASMA_OXYGEN_FULLBURN {
			(
				plasma_moles / PLASMA_BURN_RATE_DELTA * temperature_scale,
				false,
			)
		} else {
			(
				(oxygen_moles / PLASMA_OXYGEN_FULLBURN) / PLASMA_BURN_RATE_DELTA
					* temperature_scale,
				false,
			)
		};
		if plasma_burn_rate < MINIMUM_HEAT_CAPACITY {
			return Ok(None);
		}
		let old_heat_capacity = air.heat_capacity();
		let plasma_burn_rate = plasma_burn_rate
			.min(plasma_moles)
			.min(oxygen_moles / oxygen_burn_ratio);
		Ok(Some(PreMath {
			plasma_burn_rate,
			oxygen_burn_ratio,
			super_saturation,
			oxygen_moles,
			plasma_moles,
			temperature,
			old_heat_capacity,
		}))
	})?;
	let Some(pre) = pre else {
		return Ok(false.into());
	};

	let (fire_amount, new_temperature) = with_mix_mut(&byond_air, |air| {
		air.set_moles(plasma, quantize(pre.plasma_moles - pre.plasma_burn_rate))?;
		air.set_moles(
			o2,
			quantize(pre.oxygen_moles - pre.plasma_burn_rate * pre.oxygen_burn_ratio),
		)?;
		if pre.super_saturation {
			air.adjust_moles(tritium, pre.plasma_burn_rate)?;
		} else {
			air.adjust_moles(co2, pre.plasma_burn_rate * 0.75)?;
			air.adjust_moles(water_vapor, pre.plasma_burn_rate * 0.25)?;
		}
		let fire_amount = pre.plasma_burn_rate * (1.0 + pre.oxygen_burn_ratio);
		let energy_released = FIRE_PLASMA_ENERGY_RELEASED * pre.plasma_burn_rate;
		let new_heat_capacity = air.heat_capacity();
		let new_temperature = if new_heat_capacity > MINIMUM_HEAT_CAPACITY {
			let t = (pre.temperature * pre.old_heat_capacity + energy_released) / new_heat_capacity;
			air.set_temperature(t);
			t
		} else {
			air.get_temperature()
		};
		air.garbage_collect();
		Ok((fire_amount, new_temperature))
	})?;

	byondapi::global_call::call_global_id(
		byond_string!("dogmos_aphelion_plasmafire_finish"),
		&[
			byond_air,
			holder,
			fire_amount.into(),
			new_temperature.into(),
		],
	)?;
	Ok(true.into())
}

/// code/modules/atmospherics/gasmixtures/reactions.dm, /datum/gas_reaction/h2fire/react().
fn hydrogen_fire(byond_air: ByondValue, holder: ByondValue) -> Result<ByondValue> {
	const FIRE_HYDROGEN_BURN_RATE_DELTA: f32 = 2.0;
	const HYDROGEN_OXYGEN_FULLBURN: f32 = 10.0;
	const FIRE_HYDROGEN_ENERGY_RELEASED: f32 = 2.8e6;

	let o2 = gas_idx_from_string(GAS_O2)?;
	let hydrogen = gas_idx_from_string(GAS_HYDROGEN)?;
	let water_vapor = gas_idx_from_string(GAS_H2O)?;

	let (burned_fuel, temperature) = with_mix_mut(&byond_air, |air| {
		let hydrogen_moles = air.get_moles(hydrogen);
		let oxygen_moles = air.get_moles(o2);
		let old_heat_capacity = air.heat_capacity();
		let temperature = air.get_temperature();

		let burned_fuel = (hydrogen_moles / FIRE_HYDROGEN_BURN_RATE_DELTA)
			.min(oxygen_moles / (FIRE_HYDROGEN_BURN_RATE_DELTA * HYDROGEN_OXYGEN_FULLBURN))
			.min(hydrogen_moles)
			.min(oxygen_moles / 0.5);
		if burned_fuel <= 0.0
			|| hydrogen_moles - burned_fuel < 0.0
			|| oxygen_moles - burned_fuel * 0.5 < 0.0
		{
			return Ok((0.0, temperature));
		}

		air.adjust_moles(hydrogen, -burned_fuel)?;
		air.adjust_moles(o2, -(burned_fuel * 0.5))?;
		air.adjust_moles(water_vapor, burned_fuel)?;

		let energy_released = FIRE_HYDROGEN_ENERGY_RELEASED * burned_fuel;
		let mut new_temperature = temperature;
		if energy_released > 0.0 {
			let new_heat_capacity = air.heat_capacity();
			if new_heat_capacity > MINIMUM_HEAT_CAPACITY {
				new_temperature =
					(temperature * old_heat_capacity + energy_released) / new_heat_capacity;
				air.set_temperature(new_temperature);
			}
		}
		air.garbage_collect();
		Ok((burned_fuel, new_temperature))
	})?;

	if burned_fuel <= 0.0 {
		return Ok(false.into());
	}

	byondapi::global_call::call_global_id(
		byond_string!("dogmos_aphelion_h2fire_finish"),
		&[byond_air, holder, burned_fuel.into(), temperature.into()],
	)?;
	Ok(true.into())
}

/// code/modules/atmospherics/gasmixtures/reactions.dm, /datum/gas_reaction/tritfire/react().
fn tritium_fire(byond_air: ByondValue, holder: ByondValue) -> Result<ByondValue> {
	const FIRE_TRITIUM_BURN_RATE_DELTA: f32 = 2.0;
	const TRITIUM_OXYGEN_FULLBURN: f32 = 10.0;
	const FIRE_TRITIUM_ENERGY_RELEASED: f32 = 2.8e6;

	let o2 = gas_idx_from_string(GAS_O2)?;
	let tritium = gas_idx_from_string(GAS_TRITIUM)?;
	let water_vapor = gas_idx_from_string(GAS_H2O)?;

	let (burned_fuel, energy_released, volume, temperature) = with_mix_mut(&byond_air, |air| {
		let tritium_moles = air.get_moles(tritium);
		let oxygen_moles = air.get_moles(o2);
		let old_heat_capacity = air.heat_capacity();
		let temperature = air.get_temperature();
		let volume = air.volume;

		let burned_fuel = (tritium_moles / FIRE_TRITIUM_BURN_RATE_DELTA)
			.min(oxygen_moles / (FIRE_TRITIUM_BURN_RATE_DELTA * TRITIUM_OXYGEN_FULLBURN))
			.min(tritium_moles)
			.min(oxygen_moles / 0.5);
		if burned_fuel <= 0.0
			|| tritium_moles - burned_fuel < 0.0
			|| oxygen_moles - burned_fuel * 0.5 < 0.0
		{
			return Ok((0.0, 0.0, volume, temperature));
		}

		air.adjust_moles(tritium, -burned_fuel)?;
		air.adjust_moles(o2, -(burned_fuel * 0.5))?;
		air.adjust_moles(water_vapor, burned_fuel)?;

		let energy_released = FIRE_TRITIUM_ENERGY_RELEASED * burned_fuel;
		let mut new_temperature = temperature;
		if energy_released > 0.0 {
			let new_heat_capacity = air.heat_capacity();
			if new_heat_capacity > MINIMUM_HEAT_CAPACITY {
				new_temperature =
					(temperature * old_heat_capacity + energy_released) / new_heat_capacity;
				air.set_temperature(new_temperature);
			}
		}
		air.garbage_collect();
		Ok((burned_fuel, energy_released, volume, new_temperature))
	})?;

	if burned_fuel <= 0.0 {
		return Ok(false.into());
	}

	byondapi::global_call::call_global_id(
		byond_string!("dogmos_aphelion_tritfire_finish"),
		&[
			byond_air,
			holder,
			burned_fuel.into(),
			energy_released.into(),
			volume.into(),
			temperature.into(),
		],
	)?;
	Ok(true.into())
}

/// code/modules/atmospherics/gasmixtures/reactions.dm, /datum/gas_reaction/freonfire/react().
fn freon_fire(byond_air: ByondValue, holder: ByondValue) -> Result<ByondValue> {
	const FREON_MAXIMUM_BURN_TEMPERATURE: f32 = 283.0;
	const FREON_LOWER_TEMPERATURE: f32 = 60.0;
	const FREON_TERMINAL_TEMPERATURE: f32 = 20.0;
	const OXYGEN_BURN_RATIO_BASE: f32 = 1.4;
	const FREON_OXYGEN_FULLBURN: f32 = 10.0;
	const FREON_BURN_RATE_DELTA: f32 = 4.0;
	const FIRE_FREON_ENERGY_CONSUMED: f32 = 3e5;

	let o2 = gas_idx_from_string(GAS_O2)?;
	let freon = gas_idx_from_string(GAS_FREON)?;
	let co2 = gas_idx_from_string(GAS_CO2)?;

	struct PreMath {
		freon_burn_rate: f32,
		oxygen_burn_ratio: f32,
		oxygen_moles: f32,
		freon_moles: f32,
		temperature: f32,
		old_heat_capacity: f32,
	}

	let pre = with_mix(&byond_air, |air| {
		let temperature = air.get_temperature();
		let temperature_scale = if temperature < FREON_TERMINAL_TEMPERATURE {
			0.0
		} else if temperature < FREON_LOWER_TEMPERATURE {
			0.5
		} else {
			(FREON_MAXIMUM_BURN_TEMPERATURE - temperature)
				/ (FREON_MAXIMUM_BURN_TEMPERATURE - FREON_TERMINAL_TEMPERATURE)
		};
		if temperature_scale <= 0.0 {
			return Ok(None);
		}

		let oxygen_burn_ratio = OXYGEN_BURN_RATIO_BASE - temperature_scale;
		let freon_moles = air.get_moles(freon);
		let oxygen_moles = air.get_moles(o2);
		let freon_burn_rate = if oxygen_moles < freon_moles * FREON_OXYGEN_FULLBURN {
			(oxygen_moles / FREON_OXYGEN_FULLBURN) / FREON_BURN_RATE_DELTA * temperature_scale
		} else {
			freon_moles / FREON_BURN_RATE_DELTA * temperature_scale
		};
		if freon_burn_rate < MINIMUM_HEAT_CAPACITY {
			return Ok(None);
		}

		let old_heat_capacity = air.heat_capacity();
		let freon_burn_rate = freon_burn_rate
			.min(freon_moles)
			.min(oxygen_moles / oxygen_burn_ratio);
		Ok(Some(PreMath {
			freon_burn_rate,
			oxygen_burn_ratio,
			oxygen_moles,
			freon_moles,
			temperature,
			old_heat_capacity,
		}))
	})?;
	let Some(pre) = pre else {
		return Ok(false.into());
	};

	let (fire_amount, new_temperature) = with_mix_mut(&byond_air, |air| {
		air.set_moles(freon, quantize(pre.freon_moles - pre.freon_burn_rate))?;
		air.set_moles(
			o2,
			quantize(pre.oxygen_moles - pre.freon_burn_rate * pre.oxygen_burn_ratio),
		)?;
		air.adjust_moles(co2, pre.freon_burn_rate)?;

		let fire_amount = pre.freon_burn_rate * (1.0 + pre.oxygen_burn_ratio);
		let energy_consumed = FIRE_FREON_ENERGY_CONSUMED * pre.freon_burn_rate;
		let new_heat_capacity = air.heat_capacity();
		let new_temperature = if new_heat_capacity > MINIMUM_HEAT_CAPACITY {
			let t = ((pre.temperature * pre.old_heat_capacity - energy_consumed)
				/ new_heat_capacity)
				.max(TCMB);
			air.set_temperature(t);
			t
		} else {
			air.get_temperature()
		};
		air.garbage_collect();
		Ok((fire_amount, new_temperature))
	})?;

	byondapi::global_call::call_global_id(
		byond_string!("dogmos_aphelion_freonfire_finish"),
		&[
			byond_air,
			holder,
			fire_amount.into(),
			pre.temperature.into(),
			new_temperature.into(),
		],
	)?;
	Ok(true.into())
}
