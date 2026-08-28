#[allow(dead_code)]
pub mod constants;
pub mod mixture;
pub mod types;

use byondapi::prelude::*;
use eyre::Result;
pub use mixture::Mixture;
use parking_lot::{const_rwlock, RwLock};
use std::sync::atomic::{AtomicUsize, Ordering};
pub use types::*;

pub type GasIDX = usize;

/// Accessors for the shared gas-mixture arena.
pub struct GasArena {}

// Gas mixtures live in a lock-protected pool so worker threads can process them concurrently.
static GAS_MIXTURES: RwLock<Option<Vec<RwLock<Mixture>>>> = const_rwlock(None);

static NEXT_GAS_IDS: RwLock<Option<Vec<usize>>> = const_rwlock(None);
static ACTIVE_MIXTURE_SLOTS: AtomicUsize = AtomicUsize::new(0);
static MIXTURE_SLOT_HIGH_WATER: AtomicUsize = AtomicUsize::new(0);

#[cfg(test)]
pub(crate) static GAS_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct GasRuntimeMetrics {
	pub arena_len: usize,
	pub arena_capacity: usize,
	pub active_slots: usize,
	pub slot_high_water: usize,
	pub mixture_bytes: usize,
	pub mixture_lock_bytes: usize,
	pub mole_length_zero: usize,
	pub mole_length_one_to_four: usize,
	pub mole_length_five_to_eight: usize,
	pub mole_length_nine: usize,
	pub mole_spills: usize,
}

fn gas_slot_from_number(raw_slot: f32, arena_len: usize) -> Result<usize> {
	if !raw_slot.is_finite() || raw_slot < 0.0 || raw_slot.fract() != 0.0 {
		return Err(eyre::eyre!(
			"Gas mixture has an invalid arena slot: {raw_slot}"
		));
	}
	let slot = raw_slot as usize;
	if slot >= arena_len {
		return Err(eyre::eyre!(
			"Gas mixture arena slot {slot} is outside the arena (length {arena_len})"
		));
	}
	Ok(slot)
}

fn ensure_distinct_mixture_slots(src: usize, arg: usize) -> Result<()> {
	if src == arg {
		return Err(eyre::eyre!(
			"Cannot operate on the same gas mixture as both arguments"
		));
	}
	Ok(())
}

pub(crate) fn gas_slot_for_mix(mix: &ByondValue) -> Result<usize> {
	let raw_slot = mix.read_number_id(byond_string!("_extools_pointer_gasmixture"))?;
	let arena_len = GAS_MIXTURES
		.read()
		.as_ref()
		.ok_or_else(|| eyre::eyre!("Gas arena is not initialized"))?
		.len();
	gas_slot_from_number(raw_slot, arena_len)
}

#[auxmacros::init]
pub fn initialize_gases() {
	*GAS_MIXTURES.write() = Some(Vec::with_capacity(240_000));
	*NEXT_GAS_IDS.write() = Some(Vec::with_capacity(2000));
	ACTIVE_MIXTURE_SLOTS.store(0, Ordering::Relaxed);
	MIXTURE_SLOT_HIGH_WATER.store(0, Ordering::Relaxed);
}

pub fn shut_down_gases() {
	#[cfg(feature = "turf_processing")]
	crate::turfs::wait_for_tasks();
	if let Some(gas_mixtures) = GAS_MIXTURES.write().as_mut() {
		gas_mixtures.clear();
	}
	if let Some(next_gas_ids) = NEXT_GAS_IDS.write().as_mut() {
		next_gas_ids.clear();
	}
}

#[cfg(all(test, feature = "katmos", feature = "superconductivity"))]
pub(crate) fn install_mixtures_for_test(mixtures: Vec<Mixture>) {
	let active = mixtures.len();
	*GAS_MIXTURES.write() = Some(mixtures.into_iter().map(RwLock::new).collect());
	*NEXT_GAS_IDS.write() = Some(Vec::new());
	ACTIVE_MIXTURE_SLOTS.store(active, Ordering::Relaxed);
	MIXTURE_SLOT_HIGH_WATER.store(active, Ordering::Relaxed);
}

impl GasArena {
	/// Locks the gas arena and and runs the given closure with it locked.
	/// # Panics
	/// if `GAS_MIXTURES` hasn't been initialized, somehow.
	pub fn with_all_mixtures<T, F>(f: F) -> T
	where
		F: FnOnce(&[RwLock<Mixture>]) -> T,
	{
		f(GAS_MIXTURES.read().as_ref().unwrap())
	}

	/// Read locks the given gas mixture and runs the given closure on it.
	/// # Errors
	/// If no such gas mixture exists or the closure itself errors.
	/// # Panics
	/// if `GAS_MIXTURES` hasn't been initialized, somehow.
	pub fn with_gas_mixture<T, F>(id: usize, f: F) -> Result<T>
	where
		F: FnOnce(&Mixture) -> Result<T>,
	{
		let lock = GAS_MIXTURES.read();
		let gas_mixtures = lock.as_ref().unwrap();
		let mix = gas_mixtures
			.get(id)
			.ok_or_else(|| eyre::eyre!("No gas mixture with ID {id} exists!"))?
			.read();
		f(&mix)
	}
	/// Write locks the given gas mixture and runs the given closure on it.
	/// # Errors
	/// If no such gas mixture exists or the closure itself errors.
	/// # Panics
	/// if `GAS_MIXTURES` hasn't been initialized, somehow.
	pub fn with_gas_mixture_mut<T, F>(id: usize, f: F) -> Result<T>
	where
		F: FnOnce(&mut Mixture) -> Result<T>,
	{
		let lock = GAS_MIXTURES.read();
		let gas_mixtures = lock.as_ref().unwrap();
		let mut mix = gas_mixtures
			.get(id)
			.ok_or_else(|| eyre::eyre!("No gas mixture with ID {id} exists!"))?
			.write();
		f(&mut mix)
	}
	/// Read locks the given gas mixtures and runs the given closure on them.
	/// # Errors
	/// If no such gas mixture exists or the closure itself errors.
	/// # Panics
	/// if `GAS_MIXTURES` hasn't been initialized, somehow.
	pub fn with_gas_mixtures<T, F>(src: usize, arg: usize, f: F) -> Result<T>
	where
		F: FnOnce(&Mixture, &Mixture) -> Result<T>,
	{
		let lock = GAS_MIXTURES.read();
		let gas_mixtures = lock.as_ref().unwrap();
		let src_gas = gas_mixtures
			.get(src)
			.ok_or_else(|| eyre::eyre!("No gas mixture with ID {src} exists!"))?
			.read();
		let arg_gas = gas_mixtures
			.get(arg)
			.ok_or_else(|| eyre::eyre!("No gas mixture with ID {arg} exists!"))?
			.read();
		f(&src_gas, &arg_gas)
	}
	/// Locks the given gas mixtures and runs the given closure on them.
	/// # Errors
	/// If no such gas mixture exists or the closure itself errors.
	/// # Panics
	/// if `GAS_MIXTURES` hasn't been initialized, somehow.
	pub fn with_gas_mixtures_mut<T, F>(src: usize, arg: usize, f: F) -> Result<T>
	where
		F: FnOnce(&mut Mixture, &mut Mixture) -> Result<T>,
	{
		let lock = GAS_MIXTURES.read();
		let gas_mixtures = lock.as_ref().unwrap();
		let src_lock = gas_mixtures
			.get(src)
			.ok_or_else(|| eyre::eyre!("No gas mixture with ID {src} exists!"))?;
		let arg_lock = gas_mixtures
			.get(arg)
			.ok_or_else(|| eyre::eyre!("No gas mixture with ID {arg} exists!"))?;
		ensure_distinct_mixture_slots(src, arg)?;
		if src < arg {
			let mut src_mix = src_lock.write();
			let mut arg_mix = arg_lock.write();
			f(&mut src_mix, &mut arg_mix)
		} else {
			let mut arg_mix = arg_lock.write();
			let mut src_mix = src_lock.write();
			f(&mut src_mix, &mut arg_mix)
		}
	}
	/// Runs the given closure on the gas mixture *locks* rather than an already-locked version.
	/// # Errors
	/// If no such gas mixture exists or the closure itself errors.
	/// # Panics
	/// if `GAS_MIXTURES` hasn't been initialized, somehow.
	fn with_gas_mixtures_custom<T, F>(src: usize, arg: usize, f: F) -> Result<T>
	where
		F: FnOnce(&RwLock<Mixture>, &RwLock<Mixture>) -> Result<T>,
	{
		let lock = GAS_MIXTURES.read();
		let gas_mixtures = lock.as_ref().unwrap();
		let src_lock = gas_mixtures
			.get(src)
			.ok_or_else(|| eyre::eyre!("No gas mixture with ID {src} exists!"))?;
		let arg_lock = gas_mixtures
			.get(arg)
			.ok_or_else(|| eyre::eyre!("No gas mixture with ID {arg} exists!"))?;
		ensure_distinct_mixture_slots(src, arg)?;
		f(src_lock, arg_lock)
	}
	/// Fills in the first unused slot in the gas mixtures vector, or adds another one, then sets the argument ByondValue to point to it.
	/// # Errors
	/// If `initial_volume` is incorrect, either gas arena is not initialized, or
	/// `_extools_pointer_gasmixture` doesn't exist.
	pub fn register_mix(mut mix: ByondValue) -> Result<ByondValue> {
		let init_volume = mix.read_number_id(byond_string!("initial_volume"))?;
		if !init_volume.is_finite() || init_volume < 0.0 {
			return Err(eyre::eyre!(
				"Gas mixture volume must be finite and non-negative, got {init_volume}"
			));
		}
		let arena_len = GAS_MIXTURES
			.read()
			.as_ref()
			.ok_or_else(|| eyre::eyre!("Gas arena is not initialized"))?
			.len();
		let reusable_idx = {
			let mut next_gas_ids = NEXT_GAS_IDS.write();
			let next_gas_ids = next_gas_ids
				.as_mut()
				.ok_or_else(|| eyre::eyre!("Gas arena is not initialized"))?;
			let reusable_position = (0..next_gas_ids.len()).rev().find(|position| {
				if next_gas_ids[*position] >= arena_len {
					return false;
				}
				#[cfg(feature = "turf_processing")]
				let referenced = crate::turfs::gas_mix_is_referenced(next_gas_ids[*position]);
				#[cfg(not(feature = "turf_processing"))]
				let referenced = {
					let _ = position;
					false
				};
				!referenced
			});
			reusable_position.map(|position| next_gas_ids.swap_remove(position))
		};

		if let Some(idx) = reusable_idx {
			GAS_MIXTURES
				.read()
				.as_ref()
				.unwrap()
				.get(idx)
				.ok_or_else(|| {
					eyre::eyre!("Reusable gas mixture ID {idx} is outside the gas arena")
				})?
				.write()
				.clear_with_vol(init_volume);
			mix.write_var_id(
				byond_string!("_extools_pointer_gasmixture"),
				&(idx as f32).into(),
			)?;
		} else {
			let mut gas_lock = GAS_MIXTURES.write();
			let gas_mixtures = gas_lock.as_mut().unwrap();
			let next_idx = gas_mixtures.len();
			gas_mixtures.push(RwLock::new(Mixture::from_vol(init_volume)));

			mix.write_var_id(
				byond_string!("_extools_pointer_gasmixture"),
				&(next_idx as f32).into(),
			)?;

			let mut ids_lock = NEXT_GAS_IDS.write();
			let cur_last = gas_mixtures.len();
			let next_gas_ids = ids_lock.as_mut().unwrap();
			let cap = {
				let to_cap = gas_mixtures.capacity().saturating_sub(cur_last);
				if to_cap == 0 {
					next_gas_ids.capacity().saturating_sub(100)
				} else {
					(next_gas_ids.capacity().saturating_sub(100)).min(to_cap)
				}
			};
			next_gas_ids.extend(cur_last..(cur_last + cap));
			gas_mixtures.resize_with(cur_last + cap, Default::default);
		}
		let active_slots = ACTIVE_MIXTURE_SLOTS.fetch_add(1, Ordering::Relaxed) + 1;
		MIXTURE_SLOT_HIGH_WATER.fetch_max(active_slots, Ordering::Relaxed);
		Ok(ByondValue::null())
	}
	/// Marks the ByondValue's gas mixture as unused, allowing it to be reallocated to another.
	///
	/// # Errors
	/// If the mix has no valid arena slot or the arena has not been initialized.
	pub fn unregister_mix(mix: &ByondValue) -> Result<()> {
		let idx = gas_slot_for_mix(mix)?;

		let mut next_gas_ids = NEXT_GAS_IDS.write();
		let next_gas_ids = next_gas_ids
			.as_mut()
			.ok_or_else(|| eyre::eyre!("Gas arena is not initialized"))?;
		if !next_gas_ids.contains(&idx) {
			next_gas_ids.push(idx);
			ACTIVE_MIXTURE_SLOTS.fetch_sub(1, Ordering::Relaxed);
		}
		Ok(())
	}
}

/// Gets the mix for the given value, and calls the provided closure with a reference to that mix as an argument.
/// # Errors
/// If a gasmixture ID is not a number or the callback returns an error.
pub fn with_mix<T, F>(mix: &ByondValue, f: F) -> Result<T>
where
	F: FnOnce(&Mixture) -> Result<T>,
{
	GasArena::with_gas_mixture(gas_slot_for_mix(mix)?, f)
}

/// As `with_mix`, but mutable.
/// # Errors
/// If a gasmixture ID is not a number or the callback returns an error.
pub fn with_mix_mut<T, F>(mix: &ByondValue, f: F) -> Result<T>
where
	F: FnOnce(&mut Mixture) -> Result<T>,
{
	GasArena::with_gas_mixture_mut(gas_slot_for_mix(mix)?, f)
}

/// As `with_mix`, but with two mixes.
/// # Errors
/// If a gasmixture ID is not a number or the callback returns an error.
pub fn with_mixes<T, F>(src_mix: &ByondValue, arg_mix: &ByondValue, f: F) -> Result<T>
where
	F: FnOnce(&Mixture, &Mixture) -> Result<T>,
{
	GasArena::with_gas_mixtures(gas_slot_for_mix(src_mix)?, gas_slot_for_mix(arg_mix)?, f)
}

/// As `with_mix_mut`, but with two mixes.
/// # Errors
/// If a gasmixture ID is not a number or the callback returns an error.
pub fn with_mixes_mut<T, F>(src_mix: &ByondValue, arg_mix: &ByondValue, f: F) -> Result<T>
where
	F: FnOnce(&mut Mixture, &mut Mixture) -> Result<T>,
{
	GasArena::with_gas_mixtures_mut(gas_slot_for_mix(src_mix)?, gas_slot_for_mix(arg_mix)?, f)
}

/// Allows different lock levels for each gas. Instead of relevant refs to the gases, returns the `RWLock` object.
/// # Errors
/// If a gasmixture ID is not a number or the callback returns an error.
pub fn with_mixes_custom<T, F>(src_mix: &ByondValue, arg_mix: &ByondValue, f: F) -> Result<T>
where
	F: FnMut(&RwLock<Mixture>, &RwLock<Mixture>) -> Result<T>,
{
	GasArena::with_gas_mixtures_custom(gas_slot_for_mix(src_mix)?, gas_slot_for_mix(arg_mix)?, f)
}

/// Gets the amount of gases that are active in byond.
/// # Panics
/// if `GAS_MIXTURES` hasn't been initialized, somehow.
pub fn amt_gases() -> usize {
	GAS_MIXTURES.read().as_ref().unwrap().len() - NEXT_GAS_IDS.read().as_ref().unwrap().len()
}

/// Gets the amount of gases that are allocated, but not necessarily active in byond.
/// # Panics
/// if `GAS_MIXTURES` hasn't been initialized, somehow.
pub fn tot_gases() -> usize {
	GAS_MIXTURES.read().as_ref().unwrap().len()
}

pub(crate) fn gas_runtime_metrics() -> GasRuntimeMetrics {
	let gas_mixtures = GAS_MIXTURES.read();
	let Some(gas_mixtures) = gas_mixtures.as_ref() else {
		return GasRuntimeMetrics {
			mixture_bytes: std::mem::size_of::<Mixture>(),
			mixture_lock_bytes: std::mem::size_of::<RwLock<Mixture>>(),
			..Default::default()
		};
	};
	let mut metrics = GasRuntimeMetrics {
		arena_len: gas_mixtures.len(),
		arena_capacity: gas_mixtures.capacity(),
		active_slots: ACTIVE_MIXTURE_SLOTS.load(Ordering::Relaxed),
		slot_high_water: MIXTURE_SLOT_HIGH_WATER.load(Ordering::Relaxed),
		mixture_bytes: std::mem::size_of::<Mixture>(),
		mixture_lock_bytes: std::mem::size_of::<RwLock<Mixture>>(),
		..Default::default()
	};
	for mixture in gas_mixtures {
		let mixture = mixture.read();
		match mixture.mole_len() {
			0 => metrics.mole_length_zero += 1,
			1..=4 => metrics.mole_length_one_to_four += 1,
			5..=8 => metrics.mole_length_five_to_eight += 1,
			9 => metrics.mole_length_nine += 1,
			_ => (),
		}
		metrics.mole_spills += usize::from(mixture.moles_spilled());
	}
	metrics
}

#[cfg(test)]
mod tests {
	use super::{
		ensure_distinct_mixture_slots, gas_runtime_metrics, gas_slot_from_number, initialize_gases,
		GAS_TEST_LOCK,
	};

	#[test]
	fn rejects_invalid_or_stale_gas_arena_slots() {
		assert!(gas_slot_from_number(f32::NAN, 4).is_err());
		assert!(gas_slot_from_number(-1.0, 4).is_err());
		assert!(gas_slot_from_number(1.5, 4).is_err());
		assert!(gas_slot_from_number(4.0, 4).is_err());
		assert_eq!(gas_slot_from_number(3.0, 4).unwrap(), 3);
	}

	#[test]
	fn rejects_aliased_mutation_slots() {
		assert!(ensure_distinct_mixture_slots(4, 4).is_err());
		assert!(ensure_distinct_mixture_slots(4, 5).is_ok());
	}

	#[test]
	fn gas_runtime_metrics_report_source_layout_and_reserved_capacity() {
		let _guard = GAS_TEST_LOCK.lock().unwrap();
		initialize_gases();
		let metrics = gas_runtime_metrics();
		assert_eq!(metrics.mixture_bytes, 60);
		assert_eq!(metrics.mixture_lock_bytes, 64);
		assert_eq!(metrics.arena_capacity, 240_000);
		assert_eq!(metrics.active_slots, 0);
	}
}
