use super::*;
use crate::{react_hook, GasArena};
use auxcallback::process_callbacks_for_millis;
use byondapi::{byond_string, prelude::*};
use coarsetime::{Duration, Instant};
use dogmos_core::numerics::diffusion::{
	diffusion_self_weight, GAS_DIFFUSION_CONSTANT as CORE_GAS_DIFFUSION_CONSTANT,
};
use parking_lot::RwLock;
use std::collections::{BTreeMap, BTreeSet};
use tinyvec::TinyVec;

const EQUALIZE_PROFILE_FDM_ONLY: i32 = 0;
const EQUALIZE_PROFILE_FAST_ZONE: i32 = 1;
const PRESSURE_CALLBACK_BATCH_SIZE: usize = 256;

/// Returns: If a processing thread is running or not.
#[auxmacros::bind("/datum/controller/subsystem/air/proc/thread_running")]
fn thread_running_hook() -> Result<ByondValue> {
	Ok(TASKS.try_write().is_none().into())
}

fn remaining_duration(value: &ByondValue) -> Result<Duration> {
	let millis = value.get_number()?;
	if !millis.is_finite() {
		return Err(eyre::eyre!("Atmos processing budget must be finite"));
	}
	Ok(Duration::from_millis(millis.max(0.0) as u64))
}

/// Returns: If this cycle is interrupted by overtiming or not. Calls all outstanding callbacks created by other processes, usually ones that can't run on other threads and only the main thread.
#[auxmacros::bind("/datum/controller/subsystem/air/proc/finish_turf_processing_auxtools")]
fn finish_process_turfs(time_remaining: ByondValue) -> Result<ByondValue> {
	Ok(
		process_callbacks_for_millis(remaining_duration(&time_remaining)?.as_millis() as u64)
			.into(),
	)
}
/// Returns: If this cycle is interrupted by overtiming or not. Starts a processing turfs cycle.
#[auxmacros::bind("/datum/controller/subsystem/air/proc/process_turfs_auxtools")]
fn process_turf_hook(src: ByondValue, remaining: ByondValue) -> Result<ByondValue> {
	let remaining_time = remaining_duration(&remaining)?;
	// `share_max_steps` is a relaxation-iteration budget, not elapsed simulation time.
	let fdm_max_steps_value = src
		.read_number_id(byond_string!("share_max_steps"))
		.unwrap_or(1.0);
	let fdm_max_steps = if fdm_max_steps_value.is_finite() {
		fdm_max_steps_value.clamp(0.0, i32::MAX as f32) as i32
	} else {
		1
	};
	let equalize_master_enabled = src.read_number_id(byond_string!("equalize_enabled"))? != 0.0;
	let equalize_profile = src
		.read_number_id(byond_string!("dogmos_equalize_performance_profile"))
		.unwrap_or(EQUALIZE_PROFILE_FAST_ZONE as f32) as i32;
	let equalize_enabled = cfg!(feature = "fastmos")
		&& equalize_enabled_for_profile(equalize_master_enabled, equalize_profile);

	let planet_share_ratio = src
		.read_number_id(byond_string!("planet_share_ratio"))
		.unwrap_or(CORE_GAS_DIFFUSION_CONSTANT);
	let planet_share_ratio = if planet_share_ratio.is_finite() {
		planet_share_ratio.clamp(0.0, 1.0)
	} else {
		CORE_GAS_DIFFUSION_CONSTANT
	};

	process_turf(
		remaining_time,
		fdm_max_steps,
		equalize_enabled,
		planet_share_ratio,
		src,
	)?;
	Ok(ByondValue::null())
}

#[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
fn process_turf(
	remaining: Duration,
	fdm_max_steps: i32,
	equalize_enabled: bool,
	planet_share_ratio: f32,
	mut ssair: ByondValue,
) -> Result<()> {
	//this will block until process_turfs is called
	let (low_pressure_turfs, _high_pressure_turfs) = {
		let start_time = Instant::now();
		let (low_pressure_turfs, high_pressure_turfs) =
			fdm((&start_time, remaining), fdm_max_steps, equalize_enabled);
		let bench = start_time.elapsed().as_millis();
		let (lpt, hpt) = (low_pressure_turfs.len(), high_pressure_turfs.len());
		let prev_cost = ssair.read_number_id(byond_string!("cost_turfs"))?;
		ssair.write_var_id(
			byond_string!("cost_turfs"),
			&(0.8 * prev_cost + 0.2 * (bench as f32)).into(),
		)?;
		ssair.write_var_id(byond_string!("low_pressure_turfs"), &(lpt as f32).into())?;
		ssair.write_var_id(byond_string!("high_pressure_turfs"), &(hpt as f32).into())?;
		(low_pressure_turfs, high_pressure_turfs)
	};
	{
		let start_time = Instant::now();
		post_process();
		let bench = start_time.elapsed().as_millis();
		let prev_cost = ssair.read_number_id(byond_string!("cost_post_process"))?;
		ssair.write_var_id(
			byond_string!("cost_post_process"),
			&(0.8 * prev_cost + 0.2 * (bench as f32)).into(),
		)?;
	}
	{
		planet_process(planet_share_ratio);
	}
	{
		super::groups::send_to_groups(low_pressure_turfs);
	}
	if equalize_enabled {
		#[cfg(feature = "katmos")]
		{
			super::katmos::send_to_equalize(_high_pressure_turfs);
		}
	}
	Ok(())
}

/// Applies the DM master switch and the explicit Katmos/FDM performance profile. Unknown profiles
/// preserve the current Katmos behavior so an older or partially initialized SSair cannot silently
/// change the server's pressure-processing model.
fn equalize_enabled_for_profile(master_enabled: bool, profile: i32) -> bool {
	master_enabled && profile != EQUALIZE_PROFILE_FDM_ONLY
}

fn record_fdm_metrics(
	telemetry: &dogmos_perf::Telemetry,
	nodes_scanned: usize,
	nodes_changed: usize,
) {
	use dogmos_perf::RuntimeMetric;

	telemetry.increment_metric(RuntimeMetric::FdmNodesScanned, nodes_scanned as u64);
	telemetry.increment_metric(RuntimeMetric::FdmNodesChanged, nodes_changed as u64);
}

#[cfg(test)]
mod tests {
	use super::*;
	use dogmos_perf::RuntimeMetric;

	#[test]
	fn equalize_profile_keeps_master_switch_authoritative() {
		assert!(!equalize_enabled_for_profile(
			false,
			EQUALIZE_PROFILE_FAST_ZONE
		));
		assert!(!equalize_enabled_for_profile(
			false,
			EQUALIZE_PROFILE_FDM_ONLY
		));
	}

	#[test]
	fn equalize_profile_selects_fdm_only_or_fast_zone() {
		assert!(!equalize_enabled_for_profile(
			true,
			EQUALIZE_PROFILE_FDM_ONLY
		));
		assert!(equalize_enabled_for_profile(
			true,
			EQUALIZE_PROFILE_FAST_ZONE
		));
		assert!(equalize_enabled_for_profile(true, 99));
	}

	#[test]
	fn fdm_metrics_count_scanned_and_changed_nodes() {
		let telemetry = dogmos_perf::Telemetry::new();
		record_fdm_metrics(&telemetry, 128, 17);
		let snapshot = telemetry.snapshot(0);
		assert_eq!(snapshot.metric(RuntimeMetric::FdmNodesScanned), 128);
		assert_eq!(snapshot.metric(RuntimeMetric::FdmNodesChanged), 17);
	}
}

#[cfg_attr(not(target_feature = "avx2"), auxmacros::generate_simd_functions)]
#[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
fn planet_process(planet_share_ratio: f32) {
	with_turf_gases_read(|arena| {
		GasArena::with_all_mixtures(|all_mixtures| {
			with_planetary_atmos(|map| {
				arena
					.map
					.par_values()
					.filter_map(|&node_idx| {
						let mix = arena.get(node_idx)?;
						Some((mix, mix.planetary_atmos.and_then(|id| map.get(&id))?))
					})
					.for_each(|(turf_mix, planet_atmos)| {
						if let Some(gas_read) = all_mixtures
							.get(turf_mix.mix)
							.and_then(|lock| lock.try_upgradable_read())
						{
							let comparison = gas_read.compare(planet_atmos);
							let has_temp_difference = gas_read.temperature_compare(planet_atmos);
							if let Some(mut gas) = (has_temp_difference
								|| (comparison > GAS_MIN_MOLES))
								.then(|| {
									parking_lot::lock_api::RwLockUpgradableReadGuard::try_upgrade(
										gas_read,
									)
									.ok()
								})
								.flatten()
							{
								if comparison > 0.1 || has_temp_difference {
									gas.share_ratio(planet_atmos, planet_share_ratio);
								} else {
									gas.copy_from_mutable(planet_atmos);
								}
							}
						}
					})
			})
		})
	});
}

// Compares with neighbors, returning early if any of them are valid.
fn should_process(
	index: NodeIndex,
	mixture: &TurfMixture,
	all_mixtures: &[RwLock<Mixture>],
	arena: &TurfGases,
) -> bool {
	mixture.enabled()
		&& arena.adjacent_node_ids(index).next().is_some()
		&& all_mixtures
			.get(mixture.mix)
			.and_then(RwLock::try_read)
			.is_some_and(|gas| {
				for entry in arena.adjacent_mixes(index, all_mixtures) {
					if let Some(mix) = entry.try_read() {
						if gas.temperature_compare(&mix)
							|| gas.compare_with(&mix, MINIMUM_MOLES_DELTA_TO_MOVE)
						{
							return true;
						}
					} else {
						return false;
					}
				}
				false
			})
}

// Creates the combined gas mixture of all this mix's neighbors, as well as gathering some other pertinent info for future processing.
// Clippy go away, this type is only used once
#[allow(clippy::type_complexity)]
fn process_cell(
	index: NodeIndex,
	all_mixtures: &[RwLock<Mixture>],
	arena: &TurfGases,
) -> Option<(NodeIndex, Mixture, TinyVec<[(TurfID, u32, f32); 6]>, i32)> {
	let mut adj_amount = 0;
	/*
		Getting write locks is potential danger zone,
		so we make sure we don't do that unless we
		absolutely need to. Saving is fast enough.
	*/
	let mut end_gas = Mixture::from_vol(crate::constants::CELL_VOLUME);
	let mut pressure_diffs: TinyVec<[(TurfID, u32, f32); 6]> = Default::default();
	/*
		The pressure here is negative
		because we're going to be adding it
		to the base turf's pressure later on.
		It's multiplied by the diffusion constant
		because it's not representing the total
		gas pressure difference but the force exerted
		due to the pressure gradient.
		The exact physical coefficient is intentionally simplified for this model.
	*/
	for (&loc, entry) in
		arena.adjacent_mixes_with_adj_ids(index, all_mixtures, petgraph::Direction::Incoming)
	{
		{
			let mix = entry.try_read()?;
			end_gas.merge(&mix);
			adj_amount += 1;
			pressure_diffs.push((
				loc,
				arena.get_from_id(loc)?.generation,
				-mix.return_pressure() * CORE_GAS_DIFFUSION_CONSTANT,
			));
		}
	}
	/*
		This method of simulating diffusion
		diverges at coefficients that are
		larger than the inverse of the number
		of adjacent finite elements.
		As such, we must multiply it
		by a coefficient that is at most
		as big as this coefficient. The
		GAS_DIFFUSION_CONSTANT chosen here
		is 1/8, chosen both because it is
		smaller than 1/7 and because, in
		floats, 1/8 is exact and so are
		all multiples of it up to 1.
		(Technically up to 2,097,152,
		but I digress.)
	*/
	end_gas.multiply(CORE_GAS_DIFFUSION_CONSTANT);
	Some((index, end_gas, pressure_diffs, adj_amount))
}

// Solving the heat equation using a Finite Difference Method, an iterative stencil loop.
#[cfg_attr(not(target_feature = "avx2"), auxmacros::generate_simd_functions)]
#[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
fn fdm(
	(start_time, remaining_time): (&Instant, Duration),
	fdm_max_steps: i32,
	equalize_enabled: bool,
) -> (BTreeSet<TurfID>, BTreeSet<TurfID>) {
	/*
		This is the replacement system for LINDA. LINDA requires a lot of bookkeeping,
		which, when coefficient-wise operations are this fast, is all just unnecessary overhead.
		This is a much simpler FDM system, basically like LINDA but without its most important feature,
		sleeping turfs, which is why I've renamed it to fdm.
	*/
	let mut low_pressure_turfs: BTreeSet<TurfID> = Default::default();
	let mut high_pressure_turfs: BTreeSet<TurfID> = Default::default();
	let mut cur_count = 1;
	with_turf_gases_read(|arena| {
		loop {
			if cur_count > fdm_max_steps || start_time.elapsed() >= remaining_time {
				break;
			}
			GasArena::with_all_mixtures(|all_mixtures| {
				let nodes_scanned = arena.map.len();
				let turfs_to_save = arena
					.map
					/*
						This directly yanks the internal node vec
						of the graph as a slice to parallelize the process.
						The speedup gained from this is actually linear
						with the amount of cores the CPU has, which, to be frank,
						is way better than I was expecting, even though this operation
						is technically embarassingly parallel. It'll probably reach
						some maximum due to the global turf mixture lock access,
						but it's already blazingly fast on my i7, so it should be fine.
					*/
					.par_values()
					.map(|&idx| (idx, arena.get(idx).unwrap()))
					.filter(|(index, mixture)| should_process(*index, mixture, all_mixtures, arena))
					.filter_map(|(index, _)| process_cell(index, all_mixtures, arena))
					.collect::<Vec<_>>();
				record_fdm_metrics(&crate::DOGMOS_TELEMETRY, nodes_scanned, turfs_to_save.len());
				/*
					For the optimization-heads reading this: this is not an unnecessary collect().
					Saving all this to the turfs_to_save vector is, in fact, the reason
					that gases don't need an archive anymore--this *is* the archival step,
					simultaneously saving how the gases will change after the fact.
					In short: the above actually needs to finish before the below starts
					for consistency, so collect() is desired. This has been tested, by the way.
				*/
				let (low_pressure, high_pressure): (Vec<_>, Vec<_>) = turfs_to_save
					.into_par_iter()
					.filter_map(|(i, end_gas, mut pressure_diffs, adj_amount)| {
						let m = arena.get(i).unwrap();
						let self_weight = diffusion_self_weight(adj_amount as u32).ok()?;
						all_mixtures.get(m.mix).map(|entry| {
							let mut max_diff = 0.0_f32;
							let moved_pressure = {
								let gas = entry.read();
								gas.return_pressure() * CORE_GAS_DIFFUSION_CONSTANT
							};
							for pressure_diff in &mut pressure_diffs {
								// pressure_diff.2 here was set to a negative above, so we just add.
								pressure_diff.2 += moved_pressure;
								max_diff = max_diff.max(pressure_diff.2.abs());
							}
							/*
								1.0 - GAS_DIFFUSION_CONSTANT * adj_amount is going to be
								precisely equal to the amount the surrounding tiles'
								end_gas have "taken" from this tile--
								they didn't actually take anything, just calculated
								how much would be. This is the "taking" step.
								Just to illustrate: say you have a turf with 3 neighbors.
								Each of those neighbors will have their end_gas added to by
								GAS_DIFFUSION_CONSTANT (at this writing, 0.125) times
								this gas. So, 1.0 - (0.125 * adj_amount) = 0.625--
								exactly the amount those gases "took" from this.
							*/
							{
								let gas: &mut Mixture = &mut entry.write();
								gas.multiply(self_weight);
								gas.merge(&end_gas);
							}
							/*
								If there is neither a major pressure difference
								nor are there any visible gases nor does it need
								to react, we're done outright. We don't need
								to do any more and we don't need to send the
								value to byond, so we don't. However, if we do...
							*/
							(m.id, m.generation, pressure_diffs, max_diff, i)
						})
					})
					.partition(|&(_, _, _, max_diff, _)| max_diff <= 5.0);

				high_pressure_turfs.par_extend(high_pressure.par_iter().map(|(i, _, _, _, _)| i));
				low_pressure_turfs.par_extend(low_pressure.par_iter().map(|(i, _, _, _, _)| i));
				//tossing things around is already handled by katmos, so we don't need to do it here.
				if !equalize_enabled {
					let mut pressure_callbacks = high_pressure
						.into_par_iter()
						.filter_map(|(_, generation, pressures, _, node_id)| {
							Some((arena.get(node_id)?.id, generation, pressures))
						})
						.collect::<Vec<_>>();
					while !pressure_callbacks.is_empty() {
						let batch_len = PRESSURE_CALLBACK_BATCH_SIZE.min(pressure_callbacks.len());
						let batch = pressure_callbacks.drain(..batch_len).collect::<Vec<_>>();
						auxcallback::queue_callback(Box::new(move || {
							let mut first_error = None;
							for (id, generation, diffs) in batch {
								let turf = ByondValue::new_ref(ValueType::Turf, id);
								if !crate::turfs::turf_callback_is_current(id, generation) {
									continue;
								}
								for (id, generation, diff) in diffs {
									if id == 0
										|| !crate::turfs::turf_callback_is_current(id, generation)
									{
										continue;
									}
									let enemy_tile = ByondValue::new_ref(ValueType::Turf, id);
									if diff > 5.0 {
										let result = turf
											.call_id(
												byond_string!("consider_pressure_difference"),
												&[enemy_tile, diff.into()],
											)
											.wrap_err("Processing consider pressure differences");
										if let Err(error) = result {
											first_error.get_or_insert(error);
										}
									} else if diff < -5.0 {
										let result = enemy_tile
											.call_id(
												byond_string!("consider_pressure_difference"),
												&[turf, (-diff).into()],
											)
											.wrap_err("Processing consider pressure differences");
										if let Err(error) = result {
											first_error.get_or_insert(error);
										}
									}
								}
							}
							first_error.map_or(Ok(()), Err)
						}));
					}
				}
			});

			cur_count += 1;
		}
	});
	(low_pressure_turfs, high_pressure_turfs)
}

// Checks if the gas can react or can update visuals, returns None if not.
fn post_process_cell<'a>(
	mixture: &'a TurfMixture,
	vis: &[Option<f32>],
	all_mixtures: &[RwLock<Mixture>],
	reactions: &BTreeMap<crate::reaction::ReactionPriority, crate::reaction::Reaction>,
) -> Option<(&'a TurfMixture, bool, bool)> {
	all_mixtures
		.get(mixture.mix)
		.and_then(RwLock::try_read)
		.and_then(|gas| {
			let should_update_visuals = gas.vis_hash_changed(vis, &mixture.vis_hash);
			let reactable = gas.can_react_with_reactions(reactions);
			(should_update_visuals || reactable).then_some((
				mixture,
				should_update_visuals,
				reactable,
			))
		})
}

// Goes through every turf, checks if it should reset to planet atmos, if it should
// update visuals, if it should react, sends a callback if it should.
#[cfg_attr(not(target_feature = "avx2"), auxmacros::generate_simd_functions)]
#[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
fn post_process() {
	let vis = crate::gas::visibility_copies();
	with_turf_gases_read(|arena| {
		let processables = crate::gas::types::with_reactions(|reactions| {
			GasArena::with_all_mixtures(|all_mixtures| {
				arena
					.map
					.par_values()
					.filter_map(|&node_index| {
						let mix = arena.get(node_index).unwrap();
						mix.enabled().then_some(mix)
					})
					.filter_map(|mixture| post_process_cell(mixture, &vis, all_mixtures, reactions))
					.collect::<Vec<_>>()
			})
		});
		processables
			.into_par_iter()
			.for_each(|(tmix, should_update_vis, should_react)| {
				let id = tmix.id;
				let generation = tmix.generation;

				if should_react {
					auxcallback::queue_callback(Box::new(move || {
						if !crate::turfs::turf_callback_is_current(id, generation) {
							return Ok(());
						}
						let turf = ByondValue::new_ref(ValueType::Turf, id);
						match turf.read_var_id(byond_string!("air")) {
							Ok(air) if !air.is_null() => {
								react_hook(air, turf).wrap_err("Reacting")?;
								Ok(())
							}
							//turf is no longer valid for reactions
							_ => Ok(()),
						}
					}));
				}

				if should_update_vis {
					auxcallback::queue_callback(Box::new(move || {
						if !crate::turfs::turf_callback_is_current(id, generation) {
							return Ok(());
						}
						let turf = ByondValue::new_ref(ValueType::Turf, id);

						//turf is checked for validity in update_visuals
						update_visuals(turf).wrap_err("Updating Visuals")?;
						Ok(())
					}));
				}
			});
	});
}
