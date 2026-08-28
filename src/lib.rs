#[cfg(any(
	all(feature = "aphelion_reactions", feature = "citadel_reactions"),
	all(feature = "aphelion_reactions", feature = "yogs_reactions"),
	all(feature = "citadel_reactions", feature = "yogs_reactions"),
))]
compile_error!("only one Dogmos reaction backend can be enabled at a time");

mod ffi;
pub mod gas;
mod parser;
mod reaction;
#[cfg(feature = "turf_processing")]
pub mod turfs;

use byondapi::prelude::*;
use eyre::Result;
use gas::constants::{ReactionReturn, GAS_MIN_MOLES, MINIMUM_MOLES_DELTA_TO_MOVE};
use gas::{
	amt_gases, constants, gas_idx_from_string, gas_idx_from_value, gas_idx_to_id, tot_gases, types,
	with_gas_info, with_mix, with_mix_mut, with_mixes, with_mixes_custom, with_mixes_mut, GasArena,
	GasIDX, Mixture,
};
use reaction::{react_by_id, reaction_name_by_id};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

static _SIMD_DETECTED: ::std::sync::OnceLock<bool> = ::std::sync::OnceLock::new();
static DOGMOS_SHUTDOWN: AtomicBool = AtomicBool::new(false);
pub(crate) static DOGMOS_TELEMETRY: dogmos_perf::Telemetry = dogmos_perf::Telemetry::new();

fn refresh_runtime_metrics() {
	use dogmos_perf::RuntimeMetric;

	let callbacks = auxcallback::callback_metrics();
	DOGMOS_TELEMETRY.set_metric(
		RuntimeMetric::CallbackItemsEnqueued,
		callbacks.items_enqueued as u64,
	);
	DOGMOS_TELEMETRY.set_metric(
		RuntimeMetric::CallbackOwnedBytes,
		callbacks.owned_bytes_current as u64,
	);
	DOGMOS_TELEMETRY.set_metric(
		RuntimeMetric::CallbackQueueDepth,
		callbacks.queue_depth as u64,
	);
	DOGMOS_TELEMETRY.set_metric(
		RuntimeMetric::CallbackQueueDepthHighWater,
		callbacks.queue_depth_high_water as u64,
	);
	DOGMOS_TELEMETRY.set_metric(
		RuntimeMetric::CallbackEnqueueFailures,
		callbacks.enqueue_failures as u64,
	);

	let gas = gas::gas_runtime_metrics();
	DOGMOS_TELEMETRY.set_metric(RuntimeMetric::MixtureSlots, gas.active_slots as u64);
	DOGMOS_TELEMETRY.set_metric(
		RuntimeMetric::MixtureSlotHighWater,
		gas.slot_high_water as u64,
	);
	DOGMOS_TELEMETRY.set_metric(
		RuntimeMetric::MixtureMoleLengthZero,
		gas.mole_length_zero as u64,
	);
	DOGMOS_TELEMETRY.set_metric(
		RuntimeMetric::MixtureMoleLengthOneToFour,
		gas.mole_length_one_to_four as u64,
	);
	DOGMOS_TELEMETRY.set_metric(
		RuntimeMetric::MixtureMoleLengthFiveToEight,
		gas.mole_length_five_to_eight as u64,
	);
	DOGMOS_TELEMETRY.set_metric(
		RuntimeMetric::MixtureMoleLengthNine,
		gas.mole_length_nine as u64,
	);
	DOGMOS_TELEMETRY.set_metric(RuntimeMetric::MixtureMoleSpills, gas.mole_spills as u64);

	#[cfg(feature = "turf_processing")]
	{
		let turf = turfs::turf_runtime_metrics();
		DOGMOS_TELEMETRY.set_metric(RuntimeMetric::GasGraphNodes, turf.nodes as u64);
		DOGMOS_TELEMETRY.set_metric(RuntimeMetric::GasGraphEdges, turf.edges as u64);
		DOGMOS_TELEMETRY.set_metric(
			RuntimeMetric::GasGraphNodeCapacity,
			turf.node_capacity as u64,
		);
		DOGMOS_TELEMETRY.set_metric(
			RuntimeMetric::GasGraphEdgeCapacity,
			turf.edge_capacity as u64,
		);
		DOGMOS_TELEMETRY.set_metric(RuntimeMetric::GasGraphMapCapacity, turf.map_capacity as u64);
	}
	#[cfg(feature = "superconductivity")]
	{
		let heat = turfs::heat_runtime_metrics();
		DOGMOS_TELEMETRY.set_metric(RuntimeMetric::HeatGraphNodes, heat.nodes as u64);
		DOGMOS_TELEMETRY.set_metric(RuntimeMetric::HeatGraphEdges, heat.edges as u64);
		DOGMOS_TELEMETRY.set_metric(
			RuntimeMetric::HeatGraphNodeCapacity,
			heat.node_capacity as u64,
		);
		DOGMOS_TELEMETRY.set_metric(
			RuntimeMetric::HeatGraphEdgeCapacity,
			heat.edge_capacity as u64,
		);
		DOGMOS_TELEMETRY.set_metric(
			RuntimeMetric::HeatGraphMapCapacity,
			heat.map_capacity as u64,
		);
	}
}

fn collect_performance_snapshot_json() -> String {
	refresh_runtime_metrics();
	dogmos_perf::snapshot_to_json_with_diagnostics(
		&DOGMOS_TELEMETRY.snapshot(512),
		0,
		0,
		0,
		0,
		current_allocator_diagnostics(),
	)
}

fn current_allocator_diagnostics() -> dogmos_perf::AllocatorProcessDiagnostics {
	let gas = gas::gas_runtime_metrics();
	let audited = dogmos_perf::AllocationFloorLayout::audited_i686();
	#[cfg(feature = "turf_processing")]
	let (turf_mixture_bytes, turf_capacity, turf_edge_capacity) = {
		let turf = turfs::turf_runtime_metrics();
		(
			turf.turf_mixture_bytes as u64,
			turf.node_capacity as u64,
			turf.edge_capacity as u64,
		)
	};
	#[cfg(not(feature = "turf_processing"))]
	let (turf_mixture_bytes, turf_capacity, turf_edge_capacity) =
		(audited.turf_mixture_bytes, 0_u64, 0_u64);
	#[cfg(feature = "superconductivity")]
	let (thermal_info_bytes, heat_capacity, heat_edge_capacity) = {
		let heat = turfs::heat_runtime_metrics();
		(
			heat.thermal_info_bytes as u64,
			heat.node_capacity as u64,
			heat.edge_capacity as u64,
		)
	};
	#[cfg(not(feature = "superconductivity"))]
	let (thermal_info_bytes, heat_capacity, heat_edge_capacity) =
		(audited.thermal_info_bytes, 0_u64, 0_u64);
	let layout = dogmos_perf::AllocationFloorLayout {
		mixture_bytes: gas.mixture_bytes as u64,
		mixture_lock_bytes: gas.mixture_lock_bytes as u64,
		turf_mixture_bytes,
		thermal_info_bytes,
		..audited
	};
	let turf_capacity = turf_capacity.max(heat_capacity);
	let directed_edge_capacity = turf_edge_capacity.max(heat_edge_capacity);
	let allocation_floor_bytes = dogmos_perf::allocation_floor_bytes(
		layout,
		gas.arena_capacity as u64,
		turf_capacity,
		directed_edge_capacity,
	)
	.unwrap_or(u64::MAX);
	let process = allocator_process_info();
	dogmos_perf::AllocatorProcessDiagnostics {
		layout,
		allocation_floor_bytes,
		elapsed_milliseconds: process.elapsed_milliseconds as u64,
		user_milliseconds: process.user_milliseconds as u64,
		system_milliseconds: process.system_milliseconds as u64,
		current_rss_bytes: process.current_rss_bytes as u64,
		peak_rss_bytes: process.peak_rss_bytes as u64,
		current_commit_bytes: process.current_commit_bytes as u64,
		peak_commit_bytes: process.peak_commit_bytes as u64,
		page_faults: process.page_faults as u64,
	}
}

#[derive(Default)]
struct AllocatorProcessInfo {
	elapsed_milliseconds: usize,
	user_milliseconds: usize,
	system_milliseconds: usize,
	current_rss_bytes: usize,
	peak_rss_bytes: usize,
	current_commit_bytes: usize,
	peak_commit_bytes: usize,
	page_faults: usize,
}

unsafe extern "C" {
	fn mi_process_info(
		elapsed_milliseconds: *mut usize,
		user_milliseconds: *mut usize,
		system_milliseconds: *mut usize,
		current_rss_bytes: *mut usize,
		peak_rss_bytes: *mut usize,
		current_commit_bytes: *mut usize,
		peak_commit_bytes: *mut usize,
		page_faults: *mut usize,
	);
}

fn allocator_process_info() -> AllocatorProcessInfo {
	let mut info = AllocatorProcessInfo::default();
	// SAFETY: mimalloc is the linked global allocator and each pointer targets a live usize out-param.
	unsafe {
		mi_process_info(
			&mut info.elapsed_milliseconds,
			&mut info.user_milliseconds,
			&mut info.system_milliseconds,
			&mut info.current_rss_bytes,
			&mut info.peak_rss_bytes,
			&mut info.current_commit_bytes,
			&mut info.peak_commit_bytes,
			&mut info.page_faults,
		);
	}
	info
}

fn performance_detailed_from_number(value: f32) -> bool {
	value.is_finite() && value != 0.0
}

/// Enables or disables the bounded operation transcript and latency histograms. Aggregate counters
/// remain enabled because they use fixed preallocated storage.
#[auxmacros::bind("/proc/dogmos_perf_set_detailed")]
fn dogmos_perf_set_detailed(enabled: ByondValue) -> Result<ByondValue> {
	DOGMOS_TELEMETRY.set_detailed(performance_detailed_from_number(enabled.get_number()?));
	Ok(true.into())
}

/// Returns Dogmos' Rust-side operation and arena telemetry as JSON. Process memory is sampled
/// externally so DreamDaemon and any future Dogmos service remain separate measurements.
#[auxmacros::bind("/proc/dogmos_perf_snapshot")]
fn dogmos_perf_snapshot() -> Result<ByondValue> {
	ByondValue::new_str(collect_performance_snapshot_json().into_bytes()).map_err(Into::into)
}

static HYPERNOBLIUM_INDEX: OnceLock<Result<GasIDX, String>> = OnceLock::new();

fn hypernoblium_index() -> Result<GasIDX> {
	HYPERNOBLIUM_INDEX
		.get_or_init(|| {
			gas_idx_from_string(constants::GAS_HYPER_NOBLIUM).map_err(|error| error.to_string())
		})
		.as_ref()
		.copied()
		.map_err(|error| eyre::eyre!(error))
}

fn reactions_are_suppressed(hypernoblium_moles: f32, temperature: f32) -> bool {
	hypernoblium_moles >= constants::REACTION_OPPRESSION_THRESHOLD
		&& temperature > constants::REACTION_OPPRESSION_MIN_TEMP
}

fn try_begin_shutdown(shutdown: &AtomicBool) -> bool {
	shutdown
		.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
		.is_ok()
}

fn reset_shutdown_state(shutdown: &AtomicBool) {
	shutdown.store(false, Ordering::Release);
}

/// Writes panic details from Rust worker threads to `dogmos_panic.log`.
#[auxmacros::init]
pub fn install_diagnostic_panic_hook() {
	std::panic::set_hook(Box::new(|info| {
		let msg = crate::ffi::panic_payload_message(info.payload());
		let location = info
			.location()
			.map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
			.unwrap_or_else(|| "<unknown location>".to_string());
		let thread = std::thread::current();
		let report = format!(
			"[dogmos panic hook] thread {:?} panicked at {}: {}\n",
			thread.name().unwrap_or("<unnamed>"),
			location,
			msg
		);
		use std::io::Write;
		if let Ok(mut f) = std::fs::OpenOptions::new()
			.create(true)
			.append(true)
			.open("dogmos_panic.log")
		{
			let _ = f.write_all(report.as_bytes());
			let _ = f.flush();
		}
	}));
}

#[cfg(feature = "tracy")]
#[auxmacros::init]
pub fn init_eyre() {
	use tracing_subscriber::layer::SubscriberExt;

	tracing::subscriber::set_global_default(
		tracing_subscriber::registry().with(tracing_tracy::TracyLayer::default()),
	)
	.expect("setup tracy layer");
}

/// Args: (ms). Runs callbacks until time limit is reached. If time limit is omitted, runs all callbacks.
#[auxmacros::bind("/proc/process_atmos_callbacks")]
fn atmos_callback_handle(remaining: ByondValue) -> Result<ByondValue> {
	auxcallback::callback_processing_hook(remaining)
}

/// Returns the number of callbacks rejected because the main-thread callback queue was already
/// closed. A live server should keep this at zero; a non-zero value identifies teardown ordering
/// that attempted to enqueue work after callback processing had stopped.
#[auxmacros::bind("/proc/dogmos_callback_enqueue_failures")]
fn dogmos_callback_enqueue_failures() -> Result<ByondValue> {
	Ok((auxcallback::callback_enqueue_failures() as f32).into())
}

/// Returns the number of panics caught at Dogmos' BYOND FFI and initialization boundaries.
#[auxmacros::bind("/proc/dogmos_ffi_panic_count")]
fn dogmos_ffi_panic_count() -> Result<ByondValue> {
	Ok((crate::ffi::ffi_panic_count() as f32).into())
}

/// Stops Dogmos' asynchronous worker, drains callbacks that can no longer run during world
/// teardown, and releases the Rust-side gas, turf, and heat arenas. This is idempotent because BYOND
/// can reach more than one shutdown path during a hard restart.
#[auxmacros::bind("/proc/dogmos_shutdown")]
fn dogmos_shutdown_hook() -> Result<ByondValue> {
	if !try_begin_shutdown(&DOGMOS_SHUTDOWN) {
		return Ok(ByondValue::null());
	}

	#[cfg(feature = "turf_processing")]
	{
		#[cfg(feature = "superconductivity")]
		crate::turfs::shutdown_turf_heat();
		crate::turfs::shutdown_turfs();
	}
	auxcallback::clean_callbacks();
	crate::gas::shut_down_gases();
	crate::gas::types::destroy_gas_info_structs();
	Ok(ByondValue::null())
}

/// Fills in the first unused slot in the gas mixtures vector, or adds another one, then sets the argument ByondValue to point to it.
#[auxmacros::bind("/datum/gas_mixture/proc/__gasmixture_register")]
fn register_gasmixture_hook(src: ByondValue) -> Result<ByondValue> {
	gas::GasArena::register_mix(src)
}

/// Adds the gas mixture's ID to the queue of mixtures that have been deleted, to be reused later.
/// This version is only if auxcleanup is not being used; it should be called from /datum/gas_mixture/Del.
#[auxmacros::bind("/datum/gas_mixture/proc/__gasmixture_unregister")]
fn unregister_gasmixture_hook(src: ByondValue) -> Result<ByondValue> {
	gas::GasArena::unregister_mix(&src)?;
	Ok(ByondValue::null())
}

/// Returns: Heat capacity, in J/K (probably).
#[auxmacros::bind("/datum/gas_mixture/proc/heat_capacity")]
fn heat_cap_hook(src: ByondValue) -> Result<ByondValue> {
	with_mix(&src, |mix| Ok(mix.heat_capacity().into()))
}

/// Args: (min_heat_cap). Sets the mix's minimum heat capacity.
#[auxmacros::bind("/datum/gas_mixture/proc/set_min_heat_capacity")]
fn min_heat_cap_hook(src: ByondValue, arg_min: ByondValue) -> Result<ByondValue> {
	let min = arg_min.get_number()?;
	with_mix_mut(&src, |mix| {
		mix.set_min_heat_capacity(min);
		Ok(ByondValue::null())
	})
}

/// Returns: Amount of substance, in moles.
#[auxmacros::bind("/datum/gas_mixture/proc/total_moles")]
fn total_moles_hook(src: ByondValue) -> Result<ByondValue> {
	with_mix(&src, |mix| Ok(mix.total_moles().into()))
}

/// Returns: the mix's pressure, in kilopascals.
#[auxmacros::bind("/datum/gas_mixture/proc/return_pressure")]
fn return_pressure_hook(src: ByondValue) -> Result<ByondValue> {
	with_mix(&src, |mix| Ok(mix.return_pressure().into()))
}

/// Returns: the mix's temperature, in kelvins.
#[auxmacros::bind("/datum/gas_mixture/proc/return_temperature")]
fn return_temperature_hook(src: ByondValue) -> Result<ByondValue> {
	with_mix(&src, |mix| Ok(mix.get_temperature().into()))
}

/// Returns: the mix's volume, in liters.
#[auxmacros::bind("/datum/gas_mixture/proc/return_volume")]
fn return_volume_hook(src: ByondValue) -> Result<ByondValue> {
	with_mix(&src, |mix| Ok(mix.get_volume().into()))
}

/// Returns: the mix's thermal energy, the product of the mixture's heat capacity and its temperature.
#[auxmacros::bind("/datum/gas_mixture/proc/thermal_energy")]
fn thermal_energy_hook(src: ByondValue) -> Result<ByondValue> {
	with_mix(&src, |mix| Ok(mix.thermal_energy().into()))
}

/// Args: (mixture). Merges the gas from the giver into src, without modifying the giver mix.
/// Underscored because DM keeps a `merge()` wrapper of its own: it sends COMSIG_GASMIX_MERGED,
/// which gas tanks and the atmos reaction recorder listen for, and returns a success boolean.
#[auxmacros::bind("/datum/gas_mixture/proc/__merge")]
fn merge_hook(src: ByondValue, giver: ByondValue) -> Result<ByondValue> {
	with_mixes_custom(&src, &giver, |src_mix, giver_mix| {
		src_mix.write().merge(&giver_mix.read());
		Ok(ByondValue::null())
	})
}

/// Args: (mixture, ratio). Takes the given ratio of gas from src and puts it into the argument mixture. Ratio is a number between 0 and 1.
#[auxmacros::bind("/datum/gas_mixture/proc/__remove_ratio")]
fn remove_ratio_hook(
	src: ByondValue,
	into: ByondValue,
	ratio_arg: ByondValue,
) -> Result<ByondValue> {
	let ratio = ratio_arg.get_number().unwrap_or_default();
	with_mixes_mut(&src, &into, |src_mix, into_mix| {
		src_mix.remove_ratio_into(ratio, into_mix);
		Ok(ByondValue::null())
	})
}

/// Args: (mixture, amount). Takes the given amount of gas from src and puts it into the argument mixture. Amount is amount of substance in moles.
#[auxmacros::bind("/datum/gas_mixture/proc/__remove")]
fn remove_hook(src: ByondValue, into: ByondValue, amount_arg: ByondValue) -> Result<ByondValue> {
	let amount = amount_arg.get_number().unwrap_or_default();
	with_mixes_mut(&src, &into, |src_mix, into_mix| {
		src_mix.remove_into(amount, into_mix);
		Ok(ByondValue::null())
	})
}

/// Arg: (mixture). Makes src into a copy of the argument mixture.
#[auxmacros::bind("/datum/gas_mixture/proc/copy_from")]
fn copy_from_hook(src: ByondValue, giver: ByondValue) -> Result<ByondValue> {
	with_mixes_custom(&src, &giver, |src_mix, giver_mix| {
		src_mix.write().copy_from_mutable(&giver_mix.read());
		Ok(ByondValue::null())
	})
}

/// Args: (src, mixture, conductivity) or (src, conductivity, temperature, heat_capacity). Adjusts temperature of src based on parameters. Returns: temperature of sharer after sharing is complete.
#[auxmacros::bind_raw_args("/datum/gas_mixture/proc/temperature_share")]
fn temperature_share_hook() -> Result<ByondValue> {
	let arg_num = args.len();
	match arg_num {
		3 => with_mixes_mut(&args[0], &args[1], |src_mix, share_mix| {
			Ok(src_mix
				.temperature_share(share_mix, args[2].get_number().unwrap_or_default())
				.into())
		}),
		4 => with_mix_mut(&args[0], |mix| {
			Ok(mix
				.temperature_share_non_gas(
					args[1].get_number().unwrap_or_default(),
					args[2].get_number().unwrap_or_default(),
					args[3].get_number().unwrap_or_default(),
				)
				.into())
		}),
		_ => Err(eyre::eyre!("Invalid args for temperature_share")),
	}
}

/// Returns: a list of the gases in the mixture, associated with their IDs.
/// Raw FFI bind - returns Dogmos' native string ids. Use get_gases() in gas_mixture.dm, which
/// translates back to the typepaths every DM call site expects.
#[auxmacros::bind("/datum/gas_mixture/proc/__get_gases")]
fn get_gases_hook(src: ByondValue) -> Result<ByondValue> {
	with_mix(&src, |mix| {
		let mut gases_list = ByondValue::new_list()?;
		mix.for_each_gas(|idx, gas| {
			if gas > GAS_MIN_MOLES {
				gases_list.push_list(gas_idx_to_id(idx))?;
			}
			Ok(())
		})?;

		Ok(gases_list)
	})
}

/// Args: (temperature). Sets the temperature of the mixture. Will be set to 2.7 if it's too low.
#[auxmacros::bind("/datum/gas_mixture/proc/set_temperature")]
fn set_temperature_hook(src: ByondValue, arg_temp: ByondValue) -> Result<ByondValue> {
	let v = arg_temp.get_number()?;
	if v.is_finite() {
		with_mix_mut(&src, |mix| {
			mix.set_temperature(v.max(2.7));
			Ok(ByondValue::null())
		})
	} else {
		Err(eyre::eyre!(
			"Attempted to set a temperature to a number that is NaN or infinite."
		))
	}
}

/// Args: (gas_id). Returns the heat capacity from the given gas, in J/K (probably).
/// Raw FFI bind - gas_id must already be Dogmos' string form. Use partial_heat_capacity() in gas_mixture.dm.
#[auxmacros::bind("/datum/gas_mixture/proc/__partial_heat_capacity")]
fn partial_heat_capacity(src: ByondValue, gas_id: ByondValue) -> Result<ByondValue> {
	with_mix(&src, |mix| {
		Ok(mix
			.partial_heat_capacity(gas_idx_from_value(&gas_id)?)
			.into())
	})
}

/// Args: (volume). Sets the volume of the gas.
#[auxmacros::bind("/datum/gas_mixture/proc/set_volume")]
fn set_volume_hook(src: ByondValue, vol_arg: ByondValue) -> Result<ByondValue> {
	let volume = vol_arg.get_number()?;
	with_mix_mut(&src, |mix| {
		mix.set_volume(volume)
			.map_err(|error| eyre::eyre!("set_volume rejected {volume}: {error}"))?;
		Ok(ByondValue::null())
	})
}

/// Args: (gas_id). Returns: the amount of substance of the given gas, in moles.
/// Raw FFI bind - gas_id must already be Dogmos' string form. Use get_moles() in gas_mixture.dm.
#[auxmacros::bind("/datum/gas_mixture/proc/__get_moles")]
fn get_moles_hook(src: ByondValue, gas_id: ByondValue) -> Result<ByondValue> {
	with_mix(&src, |mix| {
		Ok(mix.get_moles(gas_idx_from_value(&gas_id)?).into())
	})
}

/// Args: (gas_id, moles). Sets the amount of substance of the given gas, in moles.
/// Raw FFI bind - gas_id must already be Dogmos' string form. Use set_moles() in gas_mixture.dm.
#[auxmacros::bind("/datum/gas_mixture/proc/__set_moles")]
fn set_moles_hook(src: ByondValue, gas_id: ByondValue, amt_val: ByondValue) -> Result<ByondValue> {
	let amount = amt_val.get_number()?;
	let gas_idx = gas_idx_from_value(&gas_id)?;
	with_mix_mut(&src, |mix| {
		mix.set_moles(gas_idx, amount)
			.map_err(|error| eyre::eyre!("__set_moles rejected gas index {gas_idx}: {error}"))?;
		Ok(ByondValue::null())
	})
}
/// Args: (gas_id, moles). Adjusts the given gas's amount by the given amount, e.g. (GAS_O2, -0.1) will remove 0.1 moles of oxygen from the mixture.
/// Raw FFI bind - id_val must already be Dogmos' string form. Use adjust_moles() in gas_mixture.dm.
#[auxmacros::bind("/datum/gas_mixture/proc/__adjust_moles")]
fn adjust_moles_hook(
	src: ByondValue,
	id_val: ByondValue,
	num_val: ByondValue,
) -> Result<ByondValue> {
	let amount = num_val.get_number()?;
	let gas_idx = gas_idx_from_value(&id_val)?;
	with_mix_mut(&src, |mix| {
		mix.adjust_moles(gas_idx, amount)
			.map_err(|error| eyre::eyre!("__adjust_moles rejected gas index {gas_idx}: {error}"))?;
		Ok(ByondValue::null())
	})
}

/// Args: (gas_id, moles, temp). Adjusts the given gas's amount by the given amount, with that gas being treated as if it is at the given temperature.
/// Raw FFI bind - gas_id must already be Dogmos' string form. Use adjust_moles_temp() in gas_mixture.dm.
#[auxmacros::bind("/datum/gas_mixture/proc/__adjust_moles_temp")]
fn adjust_moles_temp_hook(
	src: ByondValue,
	id_val: ByondValue,
	num_val: ByondValue,
	temp_val: ByondValue,
) -> Result<ByondValue> {
	let amount = num_val.get_number()?;
	let amount = gas::mixture::validate_mole_amount(amount)
		.map_err(|error| eyre::eyre!("__adjust_moles_temp rejected amount: {error}"))?;
	let temperature = temp_val.get_number()?;
	if !temperature.is_finite() {
		return Err(eyre::eyre!(
			"__adjust_moles_temp rejected non-finite temperature"
		));
	}
	if amount == 0.0 {
		return Ok(ByondValue::null());
	}
	let gas_idx = gas_idx_from_value(&id_val)?;
	let mut new_mix = Mixture::new();
	new_mix.set_moles(gas_idx, amount).map_err(|error| {
		eyre::eyre!("__adjust_moles_temp rejected gas index {gas_idx}: {error}")
	})?;
	new_mix.set_temperature(temperature);
	with_mix_mut(&src, |mix| {
		mix.merge(&new_mix);
		Ok(ByondValue::null())
	})
}

/// Args: (gas_id_1, amount_1, gas_id_2, amount_2, ...). As adjust_moles, but with variadic arguments.
/// Raw FFI bind - gas ids must already be Dogmos' string form. Use adjust_multi() in gas_mixture.dm.
#[auxmacros::bind_raw_args("/datum/gas_mixture/proc/__adjust_multi")]
fn adjust_multi_hook() -> Result<ByondValue> {
	if args.len().is_multiple_of(2) {
		Err(eyre::eyre!(
			"Incorrect arg len for adjust_multi (is even, must be odd to account for src)."
		))
	} else if let Some((src, rest)) = args.split_first() {
		let adjustments = rest
			.chunks(2)
			.enumerate()
			.map(|(pair_index, chunk)| -> Result<_> {
				let gas_idx = gas_idx_from_value(&chunk[0]).map_err(|error| {
					eyre::eyre!("__adjust_multi pair {pair_index} has an invalid gas: {error}")
				})?;
				let amount = chunk[1].get_number().map_err(|error| {
					eyre::eyre!(
						"__adjust_multi pair {pair_index}, gas index {gas_idx}, has a non-number amount: {error}"
					)
				})?;
				Ok((gas_idx, amount))
			})
			.collect::<Result<Vec<_>>>()?;
		with_mix_mut(src, |mix| {
			mix.adjust_multi(&adjustments)
				.map_err(|error| eyre::eyre!("__adjust_multi rejected input: {error}"))?;
			Ok(ByondValue::null())
		})
	} else {
		Err(eyre::eyre!("Invalid number of args for adjust_multi"))
	}
}

fn finite_number_or_default(value: ByondValue, default: f32) -> Result<f32> {
	let number = value.get_number().unwrap_or(default);
	if number.is_finite() {
		Ok(number)
	} else {
		Err(eyre::eyre!("Gas mixture scalar must be finite"))
	}
}

/// Args: (amount). Adds the given amount to each gas.
#[auxmacros::bind("/datum/gas_mixture/proc/add")]
fn add_hook(src: ByondValue, num_val: ByondValue) -> Result<ByondValue> {
	let vf = finite_number_or_default(num_val, 0.0)?;
	with_mix_mut(&src, |mix| {
		mix.add(vf);
		Ok(ByondValue::null())
	})
}

/// Args: (amount). Subtracts the given amount from each gas.
#[auxmacros::bind("/datum/gas_mixture/proc/subtract")]
fn subtract_hook(src: ByondValue, num_val: ByondValue) -> Result<ByondValue> {
	let vf = finite_number_or_default(num_val, 0.0)?;
	with_mix_mut(&src, |mix| {
		mix.add(-vf);
		Ok(ByondValue::null())
	})
}

/// Args: (coefficient). Multiplies all gases by this amount.
#[auxmacros::bind("/datum/gas_mixture/proc/multiply")]
fn multiply_hook(src: ByondValue, num_val: ByondValue) -> Result<ByondValue> {
	let vf = finite_number_or_default(num_val, 1.0)?;
	if vf < 0.0 {
		return Err(eyre::eyre!(
			"multiply rejected numeric class negative finite"
		));
	}
	with_mix_mut(&src, |mix| {
		mix.multiply(vf);
		Ok(ByondValue::null())
	})
}

/// Args: (coefficient). Divides all gases by this amount.
#[auxmacros::bind("/datum/gas_mixture/proc/divide")]
fn divide_hook(src: ByondValue, num_val: ByondValue) -> Result<ByondValue> {
	let divisor = finite_number_or_default(num_val, 1.0)?;
	if divisor <= 0.0 {
		return Err(eyre::eyre!("divide requires a finite positive divisor"));
	}
	let vf = divisor.recip();
	with_mix_mut(&src, |mix| {
		mix.multiply(vf);
		Ok(ByondValue::null())
	})
}

/// Args: (mixture, flag, amount). Takes `amount` from src that have the given `flag` and puts them into the given `mixture`. Returns: 0 if gas didn't have any with that flag, 1 if it did.
#[auxmacros::bind("/datum/gas_mixture/proc/__remove_by_flag")]
fn remove_by_flag_hook(
	src: ByondValue,
	into: ByondValue,
	flag_val: ByondValue,
	amount_val: ByondValue,
) -> Result<ByondValue> {
	let flag = flag_val.get_number().map_or(0, |n: f32| n as u32);
	let amount = amount_val.get_number().unwrap_or(0.0);
	if !amount.is_finite() || amount <= 0.0 {
		return Ok(false.into());
	}
	let pertinent_gases = with_gas_info(|gas_info| {
		gas_info
			.iter()
			.filter(|g| g.flags & flag != 0)
			.map(|g| g.idx)
			.collect::<Vec<_>>()
	});
	if pertinent_gases.is_empty() {
		return Ok(false.into());
	}
	with_mixes_mut(&src, &into, |src_gas, dest_gas| {
		let tot = src_gas.total_moles();
		if !tot.is_finite() || tot <= 0.0 {
			return Ok(false.into());
		}
		src_gas
			.transfer_gases_to(amount / tot, &pertinent_gases, dest_gas)
			.map_err(|error| eyre::eyre!("__remove_by_flag rejected transfer: {error}"))?;
		Ok(true.into())
	})
}
/// Args: (flag). As get_gases(), but only returns gases with the given flag.
#[auxmacros::bind("/datum/gas_mixture/proc/get_by_flag")]
fn get_by_flag_hook(src: ByondValue, flag_val: ByondValue) -> Result<ByondValue> {
	let flag = flag_val.get_number().map_or(0, |n: f32| n as u32);
	let pertinent_gases = with_gas_info(|gas_info| {
		gas_info
			.iter()
			.filter(|g| g.flags & flag != 0)
			.map(|g| g.idx)
			.collect::<Vec<_>>()
	});
	if pertinent_gases.is_empty() {
		return Ok(0.0.into());
	}
	with_mix(&src, |mix| {
		Ok(pertinent_gases
			.iter()
			.fold(0.0, |acc, idx| acc + mix.get_moles(*idx))
			.into())
	})
}

/// Args: (mixture, ratio, gas_list). Takes gases given by `gas_list` and moves `ratio` amount of those gases from `src` into `mixture`.
#[auxmacros::bind("/datum/gas_mixture/proc/scrub_into")]
fn scrub_into_hook(
	src: ByondValue,
	into: ByondValue,
	ratio_v: ByondValue,
	gas_list: ByondValue,
) -> Result<ByondValue> {
	let ratio = ratio_v.get_number()?;
	if !ratio.is_finite() {
		return Err(eyre::eyre!("Scrub ratio must be finite"));
	}
	if !gas_list.is_list() {
		return Err(eyre::eyre!("Non-list gas_list passed to scrub_into!"));
	}
	if gas_list.builtin_length()?.get_number()? as u32 == 0 {
		return Ok(false.into());
	}
	let gas_scrub_vec = gas_list
		.iter()?
		.filter_map(|(k, _)| gas_idx_from_value(&k).ok())
		.collect::<Vec<_>>();
	with_mixes_mut(&src, &into, |src_gas, dest_gas| {
		src_gas
			.transfer_gases_to(ratio, &gas_scrub_vec, dest_gas)
			.map_err(|error| eyre::eyre!("scrub_into rejected transfer: {error}"))?;
		Ok(true.into())
	})
}

/// Marks the mix as immutable, meaning it will never change. This cannot be undone.
#[auxmacros::bind("/datum/gas_mixture/proc/mark_immutable")]
fn mark_immutable_hook(src: ByondValue) -> Result<ByondValue> {
	with_mix_mut(&src, |mix| {
		mix.mark_immutable();
		Ok(ByondValue::null())
	})
}

/// Returns whether the mix has been marked immutable.
#[auxmacros::bind("/datum/gas_mixture/proc/is_immutable")]
fn is_immutable_hook(src: ByondValue) -> Result<ByondValue> {
	with_mix(&src, |mix| Ok(mix.is_immutable().into()))
}

/// Clears the gas mixture my removing all of its gases.
#[auxmacros::bind("/datum/gas_mixture/proc/clear")]
fn clear_hook(src: ByondValue) -> Result<ByondValue> {
	with_mix_mut(&src, |mix| {
		mix.clear();
		Ok(ByondValue::null())
	})
}

/// Returns: true if the two mixtures are different enough for processing, false otherwise.
#[auxmacros::bind("/datum/gas_mixture/proc/compare")]
fn compare_hook(src: ByondValue, other: ByondValue) -> Result<ByondValue> {
	with_mixes(&src, &other, |gas_one, gas_two| {
		Ok((gas_one.temperature_compare(gas_two)
			|| gas_one.compare_with(gas_two, MINIMUM_MOLES_DELTA_TO_MOVE))
		.into())
	})
}

/// Args: (holder). Runs all reactions on this gas mixture. Holder is used by the reactions, and can be any arbitrary datum or null.
/// Underscored because DM keeps a `react()` wrapper of its own, carrying behaviour Dogmos has no
/// equivalent for: the hypernoblium oppression gate that stops all reactions before any are
/// considered, the reaction_results bookkeeping, and COMSIG_GASMIX_REACTED.
///
/// Optional profiling reads its toggle on each call and records slow reactions directly on the DM
/// thread, avoiding callback overhead when profiling is disabled.
#[auxmacros::bind("/datum/gas_mixture/proc/__react")]
fn react_hook(src: ByondValue, holder: ByondValue) -> Result<ByondValue> {
	let mut ret = ReactionReturn::NO_REACTION;
	let hypernoblium_idx = hypernoblium_index()?;
	let reactions = with_mix(&src, |mix| {
		if reactions_are_suppressed(mix.get_moles(hypernoblium_idx), mix.get_temperature()) {
			return Ok(None);
		}
		Ok(Some(mix.all_reactable()))
	})?;
	let Some(reactions) = reactions else {
		return Ok((ReactionReturn::STOP_REACTIONS.bits() as f32).into());
	};

	let ssair = ByondValue::new_global_ref().read_var_id(byond_string!("SSair"))?;
	let profile_reactions = ssair
		.read_number_id(byond_string!("kennel_profile_reactions"))
		.is_ok_and(|v| v != 0.0);
	let cost_threshold_ms = if profile_reactions {
		{
			ssair
				.read_number_id(byond_string!("kennel_high_cost_ms_threshold"))
				.unwrap_or(4.0)
		}
	} else {
		0.0
	};

	for reaction in reactions {
		let started = profile_reactions.then(std::time::Instant::now);
		let result = react_by_id(reaction, src, holder)?;
		if let Some(started) = started {
			let elapsed_ms = started.elapsed().as_secs_f32() * 1000.0;
			if elapsed_ms >= cost_threshold_ms {
				let name = reaction_name_by_id(reaction).unwrap_or_else(|| "unknown".to_string());
				if let Ok(name_val) = ByondValue::try_from(name) {
					let _ = ssair.call_id(
						byond_string!("kennel_record_reaction_cost"),
						&[name_val, holder, elapsed_ms.into()],
					);
				}
			}
		}
		ret |= ReactionReturn::from_bits_truncate(result.get_number().unwrap_or_default() as u32);
		if ret.contains(ReactionReturn::STOP_REACTIONS) {
			return Ok((ret.bits() as f32).into());
		}
	}
	Ok((ret.bits() as f32).into())
}

/// Args: (heat). Adds a given amount of heat to the mixture, i.e. in joules taking into account capacity.
#[auxmacros::bind("/datum/gas_mixture/proc/adjust_heat")]
fn adjust_heat_hook(src: ByondValue, temp: ByondValue) -> Result<ByondValue> {
	with_mix_mut(&src, |mix| {
		mix.adjust_heat(temp.get_number()?);
		Ok(ByondValue::null())
	})
}

/// Args: (mixture, amount). Takes the `amount` given and transfers it from `src` to `mixture`.
#[auxmacros::bind("/datum/gas_mixture/proc/transfer_to")]
fn transfer_hook(src: ByondValue, other: ByondValue, moles: ByondValue) -> Result<ByondValue> {
	with_mixes_mut(&src, &other, |our_mix, other_mix| {
		other_mix.merge(&our_mix.remove(moles.get_number()?));
		Ok(ByondValue::null())
	})
}

/// Args: (mixture, ratio). Transfers `ratio` of `src` to `mixture`.
#[auxmacros::bind("/datum/gas_mixture/proc/transfer_ratio_to")]
fn transfer_ratio_hook(
	src: ByondValue,
	other: ByondValue,
	ratio: ByondValue,
) -> Result<ByondValue> {
	with_mixes_mut(&src, &other, |our_mix, other_mix| {
		other_mix.merge(&our_mix.remove_ratio(ratio.get_number()?));
		Ok(ByondValue::null())
	})
}

/// Args: (mixture). Makes `src` a copy of `mixture`, with volumes taken into account.
#[auxmacros::bind("/datum/gas_mixture/proc/equalize_with")]
fn equalize_with_hook(src: ByondValue, total: ByondValue) -> Result<ByondValue> {
	with_mixes_custom(&src, &total, |src_lock, total_lock| {
		let src_gas = &mut src_lock.write();
		let vol = src_gas.volume;
		let total_gas = total_lock.read();
		if !total_gas.volume.is_finite() || total_gas.volume <= 0.0 {
			return Ok(ByondValue::null());
		}
		src_gas.copy_from_mutable(&total_gas);
		src_gas.multiply(vol / total_gas.volume);
		Ok(ByondValue::null())
	})
}

/// Args: (temperature). Returns: how much fuel for fire is in the mixture at the given temperature. If temperature is omitted, just uses current temperature instead.
#[auxmacros::bind("/datum/gas_mixture/proc/get_fuel_amount")]
fn fuel_amount_hook(src: ByondValue, temp: ByondValue) -> Result<ByondValue> {
	with_mix(&src, |air| {
		Ok(temp
			.get_number()
			.ok()
			.map_or_else(
				|| air.get_fuel_amount(),
				|new_temp| {
					let mut test_air = air.copy_to_mutable();
					test_air.set_temperature(new_temp);
					test_air.get_fuel_amount()
				},
			)
			.into())
	})
}

/// Args: (temperature). Returns: how much oxidizer for fire is in the mixture at the given temperature. If temperature is omitted, just uses current temperature instead.
#[auxmacros::bind("/datum/gas_mixture/proc/get_oxidation_power")]
fn oxidation_power_hook(src: ByondValue, temp: ByondValue) -> Result<ByondValue> {
	with_mix(&src, |air| {
		Ok(temp
			.get_number()
			.ok()
			.map_or_else(
				|| air.get_oxidation_power(),
				|new_temp| {
					let mut test_air = air.clone();
					test_air.set_temperature(new_temp);
					test_air.get_oxidation_power()
				},
			)
			.into())
	})
}

/// Args: (mixture, ratio, one_way). Shares the given `ratio` of `src` with `mixture`, and, unless `one_way` is truthy, vice versa.
#[cfg(feature = "zas_hooks")]
#[auxmacros::bind("/datum/gas_mixture/proc/share_ratio")]
fn share_ratio_hook(
	src: ByondValue,
	other_gas: ByondValue,
	ratio_val: ByondValue,
	one_way_val: ByondValue,
) -> Result<ByondValue> {
	let one_way = one_way_val.get_bool().unwrap_or(false);
	let ratio = ratio_val.get_number().unwrap_or(0.6);
	let mut inbetween = Mixture::new();
	if one_way {
		with_mixes_custom(&src, &other_gas, |src_lock, other_lock| {
			let mut src_mix = src_lock.write();
			let other_mix = other_lock.read();
			inbetween.copy_from_mutable(&other_mix);
			inbetween.multiply(ratio);
			inbetween.merge(&src_mix.remove_ratio(ratio));
			inbetween.multiply(0.5);
			src_mix.merge(&inbetween);
			Ok(ByondValue::from(
				src_mix.temperature_compare(&other_mix)
					|| src_mix.compare_with(&other_mix, MINIMUM_MOLES_DELTA_TO_MOVE),
			))
		})
	} else {
		with_mixes_mut(&src, &other_gas, |src_mix, other_mix| {
			src_mix.remove_ratio_into(ratio, &mut inbetween);
			inbetween.merge(&other_mix.remove_ratio(ratio));
			inbetween.multiply(0.5);
			src_mix.merge(&inbetween);
			other_mix.merge(&inbetween);
			Ok(ByondValue::from(
				src_mix.temperature_compare(other_mix)
					|| src_mix.compare_with(other_mix, MINIMUM_MOLES_DELTA_TO_MOVE),
			))
		})
	}
}

/// Args: (list). Takes every gas in the list and makes them all identical, scaled to their respective volumes. The total heat and amount of substance in all of the combined gases is conserved.
#[auxmacros::bind("/proc/equalize_all_gases_in_list")]
fn equalize_all_hook(gas_list: ByondValue) -> Result<ByondValue> {
	use std::collections::BTreeSet;
	let gas_list = gas_list
		.iter()?
		.map(|(value, _)| gas::gas_slot_for_mix(&value))
		.collect::<Result<BTreeSet<_>>>()?;
	GasArena::with_all_mixtures(move |all_mixtures| {
		let mut tot = gas::Mixture::new();
		let mut tot_vol: f64 = 0.0;
		gas_list
			.iter()
			.filter_map(|&id| all_mixtures.get(id))
			.for_each(|src_gas_lock| {
				let src_gas = src_gas_lock.read();
				tot.merge(&src_gas);
				tot_vol += f64::from(src_gas.volume);
			});
		if tot_vol > 0.0 {
			gas_list
				.iter()
				.filter_map(|&id| all_mixtures.get(id))
				.for_each(|dest_gas_lock| {
					let dest_gas = &mut dest_gas_lock.write();
					let vol = dest_gas.volume; // don't wanna borrow it in the below
					dest_gas.copy_from_mutable(&tot);
					dest_gas.multiply((f64::from(vol) / tot_vol) as f32);
				});
		}
	});
	Ok(ByondValue::null())
}

/// Returns: the amount of gas mixtures that are attached to a byond gas mixture.
#[auxmacros::bind("/datum/controller/subsystem/air/proc/get_amt_gas_mixes")]
fn hook_amt_gas_mixes() -> Result<ByondValue> {
	Ok((amt_gases() as f32).into())
}

/// Returns: the total amount of gas mixtures in the arena, including "free" ones.
#[auxmacros::bind("/datum/controller/subsystem/air/proc/get_max_gas_mixes")]
fn hook_max_gas_mixes() -> Result<ByondValue> {
	Ok((tot_gases() as f32).into())
}
/// Returns: true. Parses gas strings like "o2=2500;plasma=5000;TEMP=370" and turns src mixes into the parsed gas mixture, invalid patterns will be ignored
#[auxmacros::bind("/datum/gas_mixture/proc/__auxtools_parse_gas_string")]
fn parse_gas_string(src: ByondValue, string: ByondValue) -> Result<ByondValue> {
	let actual_string = string.get_string()?;

	let (_, vec) = parser::parse_gas_string(&actual_string)
		.map_err(|_| eyre::eyre!(format!("Failed to parse gas string: {actual_string}")))?;

	with_mix_mut(&src, move |air| {
		air.clear();
		for (gas, moles) in vec.iter() {
			if let Ok(idx) = gas_idx_from_string(gas) {
				if (*moles).is_normal() && *moles > 0.0 {
					air.set_moles(idx, *moles).map_err(|error| {
						eyre::eyre!("__auxtools_parse_gas_string rejected gas index {idx}: {error}")
					})?;
				}
			} else if gas.contains("TEMP") {
				let mut checked_temp = *moles;
				if !checked_temp.is_normal() || checked_temp < constants::TCMB {
					checked_temp = constants::TCMB
				}
				air.set_temperature(checked_temp)
			} else {
				return Err(eyre::eyre!(format!("Unknown gas id: {gas}")));
			}
		}
		Ok(())
	})?;
	Ok(true.into())
}

#[cfg(test)]
mod lifecycle_tests {
	use super::{
		collect_performance_snapshot_json, performance_detailed_from_number, reset_shutdown_state,
		try_begin_shutdown,
	};
	use std::sync::atomic::AtomicBool;

	#[test]
	fn shutdown_transition_is_idempotent() {
		let shutdown = AtomicBool::new(false);
		assert!(try_begin_shutdown(&shutdown));
		assert!(!try_begin_shutdown(&shutdown));
	}

	#[test]
	fn shutdown_transition_can_be_rearmed_for_a_new_world() {
		let shutdown = AtomicBool::new(true);
		reset_shutdown_state(&shutdown);
		assert!(try_begin_shutdown(&shutdown));
	}

	#[test]
	fn performance_snapshot_keeps_process_memory_roles_separate() {
		let snapshot = collect_performance_snapshot_json();
		assert!(snapshot.contains("\"server_memory_is_separate\":true"));
		assert!(snapshot.contains("\"allocator_process_scope\":\"current_process_not_server\""));
		assert!(snapshot.contains("\"mixture_bytes\":60"));
		assert!(!snapshot.contains("combined_memory"));
	}

	#[test]
	fn performance_detail_toggle_uses_byond_numeric_truthiness() {
		assert!(!performance_detailed_from_number(0.0));
		assert!(performance_detailed_from_number(1.0));
		assert!(performance_detailed_from_number(-1.0));
		assert!(!performance_detailed_from_number(f32::NAN));
	}
}

#[cfg(test)]
mod reaction_tests {
	use super::reactions_are_suppressed;
	use crate::gas::constants::{REACTION_OPPRESSION_MIN_TEMP, REACTION_OPPRESSION_THRESHOLD};

	#[test]
	fn hypernoblium_oppression_matches_dm_boundaries() {
		assert!(reactions_are_suppressed(
			REACTION_OPPRESSION_THRESHOLD,
			REACTION_OPPRESSION_MIN_TEMP + 1.0,
		));

		assert!(!reactions_are_suppressed(
			REACTION_OPPRESSION_THRESHOLD - 0.01,
			REACTION_OPPRESSION_MIN_TEMP + 1.0,
		));

		assert!(!reactions_are_suppressed(
			REACTION_OPPRESSION_THRESHOLD,
			REACTION_OPPRESSION_MIN_TEMP,
		));
	}
}

#[cfg(all(
	test,
	feature = "turf_processing",
	feature = "katmos",
	feature = "superconductivity"
))]
mod legacy_transcript_tests;

#[cfg(test)]
fn normalize_generated_bindings(contents: &str) -> String {
	let mut normalized = contents.trim_end_matches(&['\r', '\n'][..]).to_owned();
	normalized.push('\n');
	normalized
}

#[test]
fn generated_bindings_have_one_trailing_newline() {
	assert_eq!(normalize_generated_bindings("binding\n\n"), "binding\n");
	assert_eq!(normalize_generated_bindings("binding\r\n\r\n"), "binding\n");
}

#[test]
fn generate_binds() {
	byondapi::generate_bindings(env!("CARGO_CRATE_NAME"));
	let bindings_path = "bindings.dm";
	let bindings = std::fs::read_to_string(bindings_path)
		.expect("generated DreamMaker bindings must be readable");
	std::fs::write(bindings_path, normalize_generated_bindings(&bindings))
		.expect("generated DreamMaker bindings must be normalized");
}
