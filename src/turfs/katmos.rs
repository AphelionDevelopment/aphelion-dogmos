//Monstermos, but zoned, and multithreaded!

use super::*;
use coarsetime::{Duration, Instant};
use indexmap::IndexSet;
use petgraph::{graph::NodeIndex, graphmap::DiGraphMap};
use rustc_hash::FxBuildHasher;
use std::{
	cell::Cell,
	{
		collections::BTreeSet,
		sync::atomic::{AtomicUsize, Ordering},
	},
};

use hashbrown::{HashMap, HashSet};

use parking_lot::{const_mutex, Mutex};

use eyre::{Context, Result};

static EQUALIZE_CHANNEL: Mutex<Option<BTreeSet<TurfID>>> = const_mutex(None);
type PressureDifference = (f32, TurfID, u32, TurfID, u32);

pub fn flush_equalize_channel() {
	*EQUALIZE_CHANNEL.lock() = None;
}

fn with_equalizes<T>(f: impl Fn(Option<BTreeSet<TurfID>>) -> T) -> T {
	f(EQUALIZE_CHANNEL.lock().take())
}

pub fn send_to_equalize(sent: BTreeSet<TurfID>) {
	EQUALIZE_CHANNEL.try_lock().map(|mut opt| opt.replace(sent));
}

#[derive(Copy, Clone, Debug)]
struct MonstermosInfo {
	mole_delta: f32,
	curr_transfer_amount: f32,
	curr_transfer_dir: Option<NodeIndex>,
	fast_done: bool,
}

impl Default for MonstermosInfo {
	fn default() -> MonstermosInfo {
		MonstermosInfo {
			mole_delta: 0_f32,
			curr_transfer_amount: 0_f32,
			curr_transfer_dir: None,
			fast_done: false,
		}
	}
}

#[derive(Copy, Clone, Debug)]
struct ReducedInfo {
	curr_transfer_amount: f32,
	curr_transfer_dir: Option<NodeIndex>,
}

impl Default for ReducedInfo {
	fn default() -> ReducedInfo {
		ReducedInfo {
			curr_transfer_amount: 0_f32,
			curr_transfer_dir: None,
		}
	}
}

fn adjust_eq_movement(
	this_turf: NodeIndex,
	that_turf: NodeIndex,
	amount: f32,
	graph: &DiGraphMap<NodeIndex, Cell<f32>>,
) {
	if let Some(cell) = graph.edge_weight(this_turf, that_turf) {
		cell.set(cell.get() + amount)
	};

	if let Some(cell) = graph.edge_weight(that_turf, this_turf) {
		cell.set(cell.get() - amount)
	};
}

fn finalize_eq(
	index: NodeIndex,
	arena: &TurfGases,
	all_mixtures: &[RwLock<Mixture>],
	eq_movement_graph: &DiGraphMap<NodeIndex, Cell<f32>>,
	pressures: &mut Vec<PressureDifference>,
) {
	// Consume the pending movement for this node.
	let pairs = eq_movement_graph
		.edges(index)
		.map(|edge| (edge.target(), edge.weight().replace(0.0)))
		.collect::<Vec<_>>();
	let turf = arena.get(index).unwrap();
	let cur_turf_id = turf.id;
	let cur_turf_generation = turf.generation;

	pairs
		.iter()
		.filter(|(_, amount)| *amount > 0.0)
		.filter_map(|&(target, amount)| Some((target, amount, arena.get(target)?)))
		.for_each(|(target, amount, adj_mix)| {
			if turf.total_moles_in(all_mixtures) < amount {
				finalize_eq_neighbors(arena, all_mixtures, &pairs, eq_movement_graph, pressures);
			}
			if let Some(weight) = eq_movement_graph.edge_weight(target, index) {
				weight.set(0.0);
			}
			if turf.mix != adj_mix.mix {
				drop(GasArena::with_gas_mixtures_mut_in(
					all_mixtures,
					turf.mix,
					adj_mix.mix,
					|air, other_air| {
						other_air.merge(&air.remove(amount));
						Ok(())
					},
				));
			}
			let adj_turf_id = adj_mix.id;
			pressures.push((
				amount,
				cur_turf_id,
				cur_turf_generation,
				adj_turf_id,
				adj_mix.generation,
			));
		});
}

fn finalize_eq_neighbors(
	arena: &TurfGases,
	all_mixtures: &[RwLock<Mixture>],
	pairs: &[(NodeIndex, f32)],
	eq_movement_graph: &DiGraphMap<NodeIndex, Cell<f32>>,
	pressures: &mut Vec<PressureDifference>,
) {
	pairs
		.iter()
		.filter(|(_, amount)| *amount < 0.0)
		.for_each(|&(adj_index, _)| {
			finalize_eq(adj_index, arena, all_mixtures, eq_movement_graph, pressures)
		})
}

fn monstermos_fast_process(
	cur_index: NodeIndex,
	info: &mut HashMap<NodeIndex, MonstermosInfo, FxBuildHasher>,
	eq_movement_graph: &DiGraphMap<NodeIndex, Cell<f32>>,
) {
	let mut cur_info = {
		let cur_info = info.get_mut(&cur_index).unwrap();
		cur_info.fast_done = true;
		*cur_info
	};
	let mut eligible_adjacents: Vec<NodeIndex> = Default::default();
	if cur_info.mole_delta > 0.0 {
		eligible_adjacents.extend(
			eq_movement_graph
				.neighbors(cur_index)
				.filter_map(|adj_index| Some((adj_index, info.get(&adj_index)?)))
				.filter(|(_, adj_info)| !adj_info.fast_done)
				.map(|(cur_index, _)| cur_index),
		);
		if eligible_adjacents.is_empty() {
			info.entry(cur_index).and_modify(|entry| *entry = cur_info);
			return;
		}
		let moles_to_move = cur_info.mole_delta / eligible_adjacents.len() as f32;
		eligible_adjacents.into_iter().for_each(|adj_index| {
			if let Some(adj_info) = info.get_mut(&adj_index) {
				adjust_eq_movement(cur_index, adj_index, moles_to_move, eq_movement_graph);
				cur_info.mole_delta -= moles_to_move;
				adj_info.mole_delta += moles_to_move;
			}
			info.entry(cur_index).and_modify(|entry| *entry = cur_info);
		});
	}
}

fn give_to_takers(
	giver_turfs: &[NodeIndex],
	info: &mut HashMap<NodeIndex, MonstermosInfo, FxBuildHasher>,
	eq_movement_graph: &DiGraphMap<NodeIndex, Cell<f32>>,
) {
	let mut queue: IndexSet<NodeIndex, FxBuildHasher> = Default::default();
	for &index in giver_turfs {
		let mut giver_info = {
			let giver_info = info.get_mut(&index).unwrap();
			giver_info.curr_transfer_dir = None;
			giver_info.curr_transfer_amount = 0.0;
			*giver_info
		};
		queue.insert(index);
		let mut queue_idx = 0;

		while let Some(&cur_index) = queue.get_index(queue_idx) {
			if giver_info.mole_delta <= 0.0 {
				break;
			}
			for adj_idx in eq_movement_graph.neighbors(cur_index) {
				if giver_info.mole_delta <= 0.0 {
					break;
				}
				if let Some(adj_info) = info.get_mut(&adj_idx) {
					if queue.insert(adj_idx) {
						adj_info.curr_transfer_dir = Some(cur_index);
						adj_info.curr_transfer_amount = 0.0;
						if adj_info.mole_delta < 0.0 {
							// This turf needs gas.
							if -adj_info.mole_delta > giver_info.mole_delta {
								// The source does not have enough gas.
								adj_info.curr_transfer_amount -= giver_info.mole_delta;
								adj_info.mole_delta += giver_info.mole_delta;
								giver_info.mole_delta = 0.0;
							} else {
								// The source has enough gas.
								adj_info.curr_transfer_amount += adj_info.mole_delta;
								giver_info.mole_delta += adj_info.mole_delta;
								adj_info.mole_delta = 0.0;
							}
						}
					}
				}
				info.entry(index).and_modify(|entry| *entry = giver_info);
			}

			queue_idx += 1;
		}

		for cur_index in queue.drain(..).rev() {
			let Some(&(mut turf_info)) = info.get(&cur_index) else {
				continue;
			};
			if turf_info.curr_transfer_amount != 0.0 {
				if let Some(transfer_dir) = turf_info.curr_transfer_dir {
					if let Some(adj_info) = info.get_mut(&transfer_dir) {
						adjust_eq_movement(
							cur_index,
							transfer_dir,
							turf_info.curr_transfer_amount,
							eq_movement_graph,
						);
						adj_info.curr_transfer_amount += turf_info.curr_transfer_amount;
						turf_info.curr_transfer_amount = 0.0;
					}
				}
			}
			info.entry(cur_index)
				.and_modify(|cur_info| *cur_info = turf_info);
		}
	}
}

fn take_from_givers(
	taker_turfs: &[NodeIndex],
	info: &mut HashMap<NodeIndex, MonstermosInfo, FxBuildHasher>,
	eq_movement_graph: &DiGraphMap<NodeIndex, Cell<f32>>,
) {
	let mut queue: IndexSet<NodeIndex, FxBuildHasher> = Default::default();
	for &index in taker_turfs {
		let mut taker_info = {
			let taker_info = info.get_mut(&index).unwrap();
			taker_info.curr_transfer_dir = None;
			taker_info.curr_transfer_amount = 0.0;
			*taker_info
		};
		queue.insert(index);
		let mut queue_idx = 0;
		while let Some(&cur_index) = queue.get_index(queue_idx) {
			if taker_info.mole_delta >= 0.0 {
				break;
			}
			for adj_index in eq_movement_graph.neighbors(cur_index) {
				if taker_info.mole_delta >= 0.0 {
					break;
				}
				if let Some(adj_info) = info.get_mut(&adj_index) {
					if queue.insert(adj_index) {
						adj_info.curr_transfer_dir = Some(cur_index);
						adj_info.curr_transfer_amount = 0.0;
						if adj_info.mole_delta > 0.0 {
							// This turf has gas available.
							if adj_info.mole_delta > -taker_info.mole_delta {
								// The source has enough gas.
								adj_info.curr_transfer_amount -= taker_info.mole_delta;
								adj_info.mole_delta += taker_info.mole_delta;
								taker_info.mole_delta = 0.0;
							} else {
								// The source does not have enough gas.
								adj_info.curr_transfer_amount += adj_info.mole_delta;
								taker_info.mole_delta += adj_info.mole_delta;
								adj_info.mole_delta = 0.0;
							}
						}
					}
				}
				info.entry(index).and_modify(|entry| *entry = taker_info);
			}
			queue_idx += 1;
		}
		for cur_index in queue.drain(..).rev() {
			let Some(&(mut turf_info)) = info.get(&cur_index) else {
				continue;
			};
			if turf_info.curr_transfer_amount != 0.0 {
				if let Some(transfer_dir) = turf_info.curr_transfer_dir {
					if let Some(adj_info) = info.get_mut(&transfer_dir) {
						adjust_eq_movement(
							cur_index,
							transfer_dir,
							turf_info.curr_transfer_amount,
							eq_movement_graph,
						);
						adj_info.curr_transfer_amount += turf_info.curr_transfer_amount;
						turf_info.curr_transfer_amount = 0.0;
					}
				}
			}
			info.entry(cur_index)
				.and_modify(|cur_info| *cur_info = turf_info);
		}
	}
}
#[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
fn explosively_depressurize(initial_index: TurfID, equalize_hard_turf_limit: usize) -> Result<()> {
	let Some(initial_index) = with_turf_gases_read(|arena| arena.get_id(initial_index)) else {
		return Ok(());
	};

	//1st floodfill
	let (space_turfs, warned_about_planet_atmos) = {
		let mut cur_queue_idx = 0;
		let mut warned_about_planet_atmos = false;
		let mut space_turfs: IndexSet<NodeIndex, FxBuildHasher> = Default::default();
		let mut turfs: IndexSet<NodeIndex, FxBuildHasher> = Default::default();
		turfs.insert(initial_index);
		while cur_queue_idx < turfs.len() {
			let cur_index = turfs[cur_queue_idx];
			cur_queue_idx += 1;
			let mut firelock_considerations = vec![];
			with_turf_gases_read(|arena| -> Result<()> {
				let Some(cur_mixture) = arena.get(cur_index) else {
					return Ok(());
				};
				if cur_mixture.planetary_atmos.is_some() {
					warned_about_planet_atmos = true;
					return Ok(());
				}
				if cur_mixture.is_immutable() {
					if space_turfs.insert(cur_index) {
						ByondValue::new_ref(ValueType::Turf, cur_mixture.id).write_var_id(
							byond_string!("pressure_specific_target"),
							&ByondValue::new_ref(ValueType::Turf, cur_mixture.id),
						)?;
					}
				} else if cur_mixture.enabled() {
					if cur_queue_idx > equalize_hard_turf_limit {
						return Ok(());
					}
					for (flags, adj_index, adj_mixture) in
						arena.graph.edges(cur_index).filter_map(|edge| {
							Some((edge.weight(), edge.target(), arena.get(edge.target())?))
						}) {
						if turfs.insert(adj_index)
							&& flags.contains(AdjacentFlags::ATMOS_ADJACENT_FIRELOCK)
						{
							firelock_considerations.push((cur_mixture.id, adj_mixture.id));
						}
					}
				}
				Ok(())
			})?;
			for (cur, adj) in firelock_considerations {
				ByondValue::new_ref(ValueType::Turf, cur).call_id(
					byond_string!("consider_firelocks"),
					&[ByondValue::new_ref(ValueType::Turf, adj)],
				)?;
			}

			if warned_about_planet_atmos {
				break;
			}
		}
		(space_turfs, warned_about_planet_atmos)
	};

	if warned_about_planet_atmos || space_turfs.is_empty() {
		return Ok(()); // planet atmos > space
	}

	let floor_rip_turfs =
		with_turf_gases_read(move |arena| -> Result<Vec<(ByondValue, ByondValue)>> {
			let mut info: HashMap<NodeIndex, Cell<ReducedInfo>, FxBuildHasher> = Default::default();
			let mut floor_rip_turfs = vec![];

			let mut progression_order = space_turfs
				.iter()
				.filter_map(|item| arena.get(*item).map_or_else(|| None, |_| Some(*item)))
				.collect::<IndexSet<_, FxBuildHasher>>();

			#[cfg(feature = "katmos_slow_decompression")]
			let mut space_turf_len = 0;
			#[cfg(feature = "katmos_slow_decompression")]
			let mut total_moles = 0.0;
			let mut cur_queue_idx = 0;
			//2nd floodfill
			while cur_queue_idx < progression_order.len() {
				let cur_index = progression_order[cur_queue_idx];
				let cur_mixture = arena.get(cur_index).unwrap();
				cur_queue_idx += 1;

				#[cfg(feature = "katmos_slow_decompression")]
				{
					total_moles += cur_mixture.total_moles();
					cur_mixture.is_immutable().then(|| space_turf_len += 1);
				}

				if cur_queue_idx > equalize_hard_turf_limit {
					continue;
				}

				for adj_index in arena.adjacent_node_ids(cur_index) {
					if let Some(adj_mixture) = arena.get(adj_index) {
						if !adj_mixture.is_immutable() && progression_order.insert(adj_index) {
							let adj_orig = info.entry(adj_index).or_default();
							let mut adj_info = adj_orig.get();

							adj_info.curr_transfer_dir = Some(cur_index);

							let cur_target_turf =
								ByondValue::new_ref(ValueType::Turf, cur_mixture.id)
									.read_var_id(byond_string!("pressure_specific_target"))?;
							ByondValue::new_ref(ValueType::Turf, adj_mixture.id).write_var_id(
								byond_string!("pressure_specific_target"),
								&cur_target_turf,
							)?;
							adj_orig.set(adj_info);
						}
					}
				}
			}

			#[cfg(feature = "katmos_slow_decompression")]
			let non_space_turf_len = progression_order.len().saturating_sub(space_turf_len);
			#[cfg(feature = "katmos_slow_decompression")]
			let average_moles = if non_space_turf_len == 0 {
				0.0
			} else {
				total_moles / non_space_turf_len as f32
			};

			let mut hpd = ByondValue::new_global_ref()
				.read_var_id(byond_string!("SSair"))
				.unwrap()
				.read_var_id(byond_string!("high_pressure_delta"))
				.unwrap();

			/*
				`byond_locatein` is a linear search of the DM list, and the loop below pushes into
				that same list, so checking membership per drained turf made a large breach cost
				O(turfs * list length). Read the existing membership once and track our own pushes
				in Rust instead. If the var somehow isn't an enumerable list we fall back to the
				per-turf search rather than risking duplicate entries.
			*/
			let mut high_pressure_members: HashSet<TurfID, FxBuildHasher> = Default::default();
			let mut membership_is_tracked = hpd.is_list();
			if membership_is_tracked {
				match hpd.iter() {
					Ok(entries) => high_pressure_members
						.extend(entries.filter_map(|(entry, _)| entry.get_ref().ok())),
					Err(_) => membership_is_tracked = false,
				}
			}

			for &cur_index in progression_order.iter().rev() {
				let cur_orig = info.entry(cur_index).or_default();
				let cur_mixture = arena.get(cur_index).unwrap();
				let mut cur_info = cur_orig.get();
				if cur_info.curr_transfer_dir.is_none() {
					continue;
				}
				// Measure loss after clearing because slow decompression may remove less than requested.
				let pre_clear_moles = cur_mixture.total_moles();
				#[cfg(not(feature = "katmos_slow_decompression"))]
				{
					cur_mixture.clear_air();
				}
				#[cfg(feature = "katmos_slow_decompression")]
				{
					cur_mixture.clear_moles(
						decompression_moles_per_turf(average_moles, space_turf_len).abs(),
					);
				}
				let moles_lost = pre_clear_moles - cur_mixture.total_moles();
				let mut byond_turf = ByondValue::new_ref(ValueType::Turf, cur_mixture.id);
				let already_listed = if membership_is_tracked {
					!high_pressure_members.insert(cur_mixture.id)
				} else {
					!byondapi::map::byond_locatein(&byond_turf, &hpd)?.is_null()
				};
				if !already_listed {
					hpd.push_list(byond_turf)?;
				}
				let adj_index = cur_info.curr_transfer_dir.unwrap();

				let adj_mixture = arena.get(adj_index).unwrap();
				let sum = adj_mixture.total_moles();

				cur_info.curr_transfer_amount += sum;
				cur_orig.set(cur_info);

				let adj_orig = info.entry(adj_index).or_default();
				let mut adj_info = adj_orig.get();

				adj_info.curr_transfer_amount += cur_info.curr_transfer_amount;
				adj_orig.set(adj_info);

				let mut byond_turf_adj = ByondValue::new_ref(ValueType::Turf, adj_mixture.id);

				byond_turf.write_var_id(
					byond_string!("pressure_difference"),
					&cur_info.curr_transfer_amount.into(),
				)?;
				byond_turf.write_var_id(
					byond_string!("pressure_direction"),
					&byondapi::global_call::call_global_id(
						byond_string!("get_dir_multiz"),
						&[byond_turf, byond_turf_adj],
					)?,
				)?;

				if adj_info.curr_transfer_dir.is_none() {
					byond_turf_adj.write_var_id(
						byond_string!("pressure_difference"),
						&adj_info.curr_transfer_amount.into(),
					)?;
					byond_turf_adj.write_var_id(
						byond_string!("pressure_direction"),
						&byondapi::global_call::call_global_id(
							byond_string!("get_dir_multiz"),
							&[byond_turf, byond_turf_adj],
						)?,
					)?;
				}

				// Pressure redistribution applies to every drained turf. Floor damage is limited to the
				// first gas layer next to immutable space, or a connected tunnel would lose every floor
				// tile merely because its air was included in the same decompression flood-fill.
				if should_notify_floor_rip(adj_mixture.is_immutable(), moles_lost) {
					floor_rip_turfs.push((byond_turf, moles_lost.into()));
				}
			}
			Ok(floor_rip_turfs)
		})?;
	for (turf, sum) in floor_rip_turfs {
		turf.call_id(byond_string!("handle_decompression_floor_rip"), &[sum])?;
	}

	Ok(())
}

#[cfg(feature = "katmos_slow_decompression")]
const DECOMP_BASE_REMOVE_RATIO: f32 = 4.0;
#[cfg(feature = "katmos_slow_decompression")]
const DECOMP_MAX_FRONTAGE_TURFS: usize = 4;

#[cfg(feature = "katmos_slow_decompression")]
fn decompression_moles_per_turf(average_moles: f32, space_turf_len: usize) -> f32 {
	// A wider opening exposes more room air to space during the same equalizer pass. Cap the frontage
	// multiplier so a map-scale boundary remains a slow drain rather than becoming an instant clear.
	let frontage_turfs = space_turf_len.clamp(1, DECOMP_MAX_FRONTAGE_TURFS) as f32;
	average_moles * frontage_turfs / DECOMP_BASE_REMOVE_RATIO
}

enum FloodFillResult {
	ZoneIgnored,
	Overtime,
	Complete(DiGraphMap<NodeIndex, Cell<f32>>, f32),
}

#[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
fn flood_fill_zones(
	(index_node, index_turf): (NodeIndex, TurfID),
	equalize_hard_turf_limit: usize,
	found_turfs: &mut HashSet<TurfID, FxBuildHasher>,
	arena: &TurfGases,
	all_mixtures: &[RwLock<Mixture>],
	(start_time, remaining_time): (&Instant, Duration),
) -> FloodFillResult {
	let mut turf_graph: DiGraphMap<NodeIndex, Cell<f32>> = Default::default();
	let mut border_turfs: std::collections::VecDeque<NodeIndex> = Default::default();
	let mut total_moles = 0.0_f32;
	let mut is_planet = false;
	turf_graph.add_node(index_node);
	border_turfs.push_back(index_node);
	found_turfs.insert(index_turf);
	let mut ignore_zone = false;
	while let Some(cur_index) = border_turfs.pop_front() {
		let cur_turf = arena.get(cur_index).unwrap();
		let cur_turf_id = cur_turf.id;
		let cur_turf_generation = cur_turf.generation;
		//hard cap for planet atmos because very large open space
		if cur_turf.planetary_atmos.is_some() {
			is_planet = true;
		}
		if is_planet && turf_graph.node_count() > equalize_hard_turf_limit {
			break;
		}
		total_moles += cur_turf.total_moles_in(all_mixtures);

		//we are already overtime, bail NOW
		if start_time.elapsed() >= remaining_time {
			return FloodFillResult::Overtime;
		}

		for (weight, adj_index, adj_mixture) in arena
			.graph
			.edges(cur_index)
			.filter_map(|edge| Some((edge.weight(), edge.target(), arena.get(edge.target())?)))
		{
			if adj_mixture.enabled() {
				turf_graph.add_edge(cur_index, adj_index, Cell::new(0.0));
			}
			if found_turfs.insert(adj_mixture.id) {
				if adj_mixture.enabled() {
					border_turfs.push_back(adj_index);
				}

				if ignore_zone {
					continue;
				}

				if adj_mixture.is_immutable_in(all_mixtures) {
					// An opening to immutable space triggers decompression.
					let _ = auxcallback::queue_callback(
						Box::new(move || {
							if !crate::turfs::turf_callback_is_current(
								cur_turf_id,
								cur_turf_generation,
							) {
								return Ok(());
							}
							explosively_depressurize(cur_turf_id, equalize_hard_turf_limit)
								.wrap_err("Decompressing")
						}),
						0,
					);
					ignore_zone = true;
				}

				if adj_mixture.planetary_atmos.is_some()
					&& weight.contains(AdjacentFlags::ATMOS_ADJACENT_FIRELOCK)
				{
					let _ = auxcallback::queue_callback(
						Box::new(move || {
							if !crate::turfs::turf_callback_is_current(
								cur_turf_id,
								cur_turf_generation,
							) {
								return Ok(());
							}
							planet_equalize(cur_turf_id, equalize_hard_turf_limit)
								.wrap_err("Equalising planet air")
						}),
						0,
					);
				}
			}
		}
	}
	if !ignore_zone {
		FloodFillResult::Complete(turf_graph, total_moles)
	} else {
		FloodFillResult::ZoneIgnored
	}
}

#[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
fn planet_equalize(initial_index: TurfID, equalize_hard_turf_limit: usize) -> Result<()> {
	let Some(initial_index) = with_turf_gases_read(|arena| arena.get_id(initial_index)) else {
		return Ok(());
	};

	let mut cur_queue_idx = 0;
	let mut warned_about_space = false;
	let mut planet_turfs: IndexSet<NodeIndex, FxBuildHasher> = Default::default();
	let mut turfs: IndexSet<NodeIndex, FxBuildHasher> = Default::default();
	turfs.insert(initial_index);
	while cur_queue_idx < turfs.len() {
		let cur_index = turfs[cur_queue_idx];
		cur_queue_idx += 1;
		let mut firelock_considerations = vec![];
		with_turf_gases_read(|arena| -> Result<()> {
			let Some(cur_mixture) = arena.get(cur_index) else {
				return Ok(());
			};
			if cur_mixture.planetary_atmos.is_some() {
				planet_turfs.insert(cur_index);
			}
			if cur_mixture.is_immutable() {
				warned_about_space = true;
				return Ok(());
			} else if cur_mixture.enabled() {
				if cur_queue_idx > equalize_hard_turf_limit {
					return Ok(());
				}
				for (_, _, adj_mixture) in arena
					.graph
					.edges(cur_index)
					.filter_map(|edge| {
						Some((edge.weight(), edge.target(), arena.get(edge.target())?))
					})
					.filter(|(flags, adj_index, _)| {
						turfs.insert(*adj_index)
							&& flags.contains(AdjacentFlags::ATMOS_ADJACENT_FIRELOCK)
					}) {
					firelock_considerations.push((cur_mixture.id, adj_mixture.id));
				}
			}
			Ok(())
		})?;
		for (cur, adj) in firelock_considerations {
			ByondValue::new_ref(ValueType::Turf, cur).call_id(
				byond_string!("consider_firelocks"),
				&[ByondValue::new_ref(ValueType::Turf, adj)],
			)?;
		}
		if warned_about_space || planet_turfs.is_empty() {
			break;
		}
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn only_hull_boundary_turfs_are_floor_rip_candidates() {
		assert!(should_notify_floor_rip(true, 1.0));
		assert!(!should_notify_floor_rip(false, 1.0));
		assert!(!should_notify_floor_rip(true, 0.0));
	}

	#[cfg(feature = "katmos_slow_decompression")]
	#[test]
	fn slow_decompression_scales_with_breach_frontage() {
		assert_eq!(decompression_moles_per_turf(100.0, 1), 25.0);
		assert_eq!(decompression_moles_per_turf(100.0, 2), 50.0);
		assert_eq!(decompression_moles_per_turf(100.0, 4), 100.0);
		assert_eq!(
			decompression_moles_per_turf(100.0, DECOMP_MAX_FRONTAGE_TURFS + 1),
			100.0,
		);
	}
}

#[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
fn process_zone(
	graph: DiGraphMap<NodeIndex, Cell<f32>>,
	average_moles: f32,
	arena: &TurfGases,
	all_mixtures: &[RwLock<Mixture>],
	turfs_processed: Option<&AtomicUsize>,
) -> DiGraphMap<NodeIndex, Cell<f32>> {
	let mut info = graph
		.nodes()
		.map(|index| {
			let mixture = arena.get(index).unwrap();
			let cur_info = MonstermosInfo {
				mole_delta: mixture.total_moles_in(all_mixtures) - average_moles,
				..Default::default()
			};
			(index, cur_info)
		})
		.collect::<HashMap<_, _, FxBuildHasher>>();

	let (mut giver_turfs, mut taker_turfs): (Vec<_>, Vec<_>) = graph
		.nodes()
		.partition(|i| info.get(i).unwrap().mole_delta > 0.0);

	let log_n = ((graph.node_count() as f32).log2().floor()) as usize;
	if giver_turfs.len() > log_n && taker_turfs.len() > log_n {
		graph.nodes().for_each(|cur_index| {
			monstermos_fast_process(cur_index, &mut info, &graph);
		});

		giver_turfs.clear();
		taker_turfs.clear();

		giver_turfs.extend(
			graph
				.nodes()
				.filter(|cur_index| info.get(cur_index).unwrap().mole_delta > 0.0),
		);

		taker_turfs.extend(
			graph
				.nodes()
				.filter(|cur_index| info.get(cur_index).unwrap().mole_delta <= 0.0),
		);
	}

	// alright this is the part that can become O(n^2).
	if giver_turfs.len() < taker_turfs.len() {
		// as an optimization, we choose one of two methods based on which list is smaller.
		give_to_takers(&giver_turfs, &mut info, &graph);
	} else {
		take_from_givers(&taker_turfs, &mut info, &graph);
	}

	if let Some(ctr) = turfs_processed {
		ctr.fetch_add(graph.node_count(), Ordering::Relaxed);
	}

	graph
}

#[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
fn finalize_eq_zone(
	arena: &TurfGases,
	all_mixtures: &[RwLock<Mixture>],
	graph: DiGraphMap<NodeIndex, Cell<f32>>,
) -> Option<Vec<PressureDifference>> {
	let mut pressures: Vec<PressureDifference> = Vec::new();
	graph.nodes().for_each(|cur_index| {
		finalize_eq(cur_index, arena, all_mixtures, &graph, &mut pressures);
	});
	(!pressures.is_empty()).then_some(pressures)
}

#[cfg(all(test, feature = "superconductivity"))]
#[derive(Debug, PartialEq)]
pub(crate) struct LegacyStageTrace {
	pub work_items: u32,
	pub left_value: f32,
	pub right_value: f32,
	pub pressure_events: Vec<(f32, TurfID, u32, TurfID, u32)>,
}

#[cfg(all(test, feature = "superconductivity"))]
pub(crate) fn capture_two_turf_equalize_trace() -> LegacyStageTrace {
	use crate::gas::{
		install_mixtures_for_test, shut_down_gases,
		types::{destroy_gas_statics, register_gas_manually, set_gas_statics_manually},
		GAS_TEST_LOCK,
	};

	struct LegacyGasState;

	impl Drop for LegacyGasState {
		fn drop(&mut self) {
			shut_down_gases();
			destroy_gas_statics();
		}
	}

	let _lock = GAS_TEST_LOCK.lock().unwrap();
	set_gas_statics_manually();
	register_gas_manually("o2", 20.0);
	let _gas_state = LegacyGasState;

	let mut left_gas = Mixture::from_vol(crate::constants::CELL_VOLUME);
	left_gas.set_moles(0, 100.0).unwrap();
	let right_gas = Mixture::from_vol(crate::constants::CELL_VOLUME);
	install_mixtures_for_test(vec![left_gas, right_gas]);

	let mut arena = TurfGases::with_capacity(0, 0);
	arena.insert_turf(TurfMixture {
		mix: 0,
		id: 10,
		generation: 1,
		flags: SimulationFlags::SIMULATION_ALL,
		planetary_atmos: None,
		vis_hash: AtomicU64::new(0),
	});
	arena.insert_turf(TurfMixture {
		mix: 1,
		id: 11,
		generation: 1,
		flags: SimulationFlags::SIMULATION_ALL,
		planetary_atmos: None,
		vis_hash: AtomicU64::new(0),
	});
	let left = arena.get_id(10).unwrap();
	let right = arena.get_id(11).unwrap();
	arena.graph.add_edge(left, right, AdjacentFlags::empty());
	arena.graph.add_edge(right, left, AdjacentFlags::empty());

	let mut zone = DiGraphMap::new();
	zone.add_edge(left, right, Cell::new(0.0));
	zone.add_edge(right, left, Cell::new(0.0));
	let work_items = zone.node_count() as u32;
	let (pressure_events, left_value, right_value) = GasArena::with_all_mixtures(|all_mixtures| {
		let zone = process_zone(zone, 50.0, &arena, all_mixtures, None);
		let pressure_events = finalize_eq_zone(&arena, all_mixtures, zone).unwrap_or_default();
		(
			pressure_events,
			all_mixtures[0].read().total_moles(),
			all_mixtures[1].read().total_moles(),
		)
	});

	LegacyStageTrace {
		work_items,
		left_value,
		right_value,
		pressure_events,
	}
}

fn send_pressure_differences(pressures: Vec<PressureDifference>) {
	const PRESSURE_CALLBACK_BATCH_SIZE: usize = 256;
	let mut pressures = pressures;
	while !pressures.is_empty() {
		let batch_len = PRESSURE_CALLBACK_BATCH_SIZE.min(pressures.len());
		let batch = pressures.drain(..batch_len).collect::<Vec<_>>();
		let owned_bytes = batch
			.capacity()
			.saturating_mul(std::mem::size_of::<PressureDifference>());
		let _ = auxcallback::queue_callback(
			Box::new(move || {
				let mut first_error = None;
				for (
					amount,
					current_turf,
					current_generation,
					adjacent_turf,
					adjacent_generation,
				) in batch
				{
					if !crate::turfs::turf_callback_is_current(current_turf, current_generation)
						|| !crate::turfs::turf_callback_is_current(
							adjacent_turf,
							adjacent_generation,
						) {
						continue;
					}
					let turf = ByondValue::new_ref(ValueType::Turf, current_turf);
					let other_turf = ByondValue::new_ref(ValueType::Turf, adjacent_turf);
					let result = turf
						.call_id(
							byond_string!("consider_pressure_difference"),
							&[other_turf, amount.into()],
						)
						.map(|_| ())
						.wrap_err("Katmos considering pressure differences");
					if let Err(error) = result {
						first_error.get_or_insert(error);
					}
				}
				first_error.map_or(Ok(()), Err)
			}),
			owned_bytes,
		);
	}
}

fn should_notify_floor_rip(parent_is_space: bool, moles_lost: f32) -> bool {
	parent_is_space && moles_lost > 0.0
}

/// Returns: If this cycle is interrupted by overtiming or not. Starts a katmos equalize cycle, does nothing if process_turfs isn't ran.
#[auxmacros::bind("/datum/controller/subsystem/air/proc/process_turf_equalize_auxtools")]
fn equalize_hook(mut src: ByondValue, remaining: ByondValue) -> Result<ByondValue> {
	let equalize_hard_turf_limit = src
		.read_number_id(byond_string!("equalize_hard_turf_limit"))
		.ok()
		.filter(|limit| limit.is_finite() && *limit >= 0.0)
		.map_or(2000, |limit| limit as usize);
	let remaining_millis = remaining
		.get_number()
		.ok()
		.filter(|budget| budget.is_finite())
		.map_or(50.0, |budget| budget.max(0.0));
	let remaining_time = Duration::from_millis(remaining_millis as u64);
	let start_time = Instant::now();
	let (num_eq, is_cancelled) = with_equalizes(|thing| {
		if let Some(high_pressure_turfs) = thing {
			equalize(
				equalize_hard_turf_limit,
				&high_pressure_turfs,
				(&start_time, remaining_time),
			)
		} else {
			(0, false)
		}
	});

	let bench = start_time.elapsed().as_millis();
	let prev_cost = src.read_number_id(byond_string!("cost_equalize"))?;
	src.write_var_id(
		byond_string!("cost_equalize"),
		&(0.8 * prev_cost + 0.2 * (bench as f32)).into(),
	)?;
	src.write_var_id(
		byond_string!("num_equalize_processed"),
		&(num_eq as f32).into(),
	)?;
	Ok(is_cancelled.into())
}

#[cfg_attr(not(target_feature = "avx2"), auxmacros::generate_simd_functions)]
#[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
fn equalize(
	equalize_hard_turf_limit: usize,
	high_pressure_turfs: &BTreeSet<TurfID>,
	(start_time, remaining_time): (&Instant, Duration),
) -> (usize, bool) {
	let turfs_processed: AtomicUsize = AtomicUsize::new(0);
	let is_cancelled = with_turf_gases_read(|arena| {
		/*
			The arena slice is taken once for the whole equalize pass and threaded into the flood
			fill, zone solve, and finalize stages. Those stages previously re-entered
			`with_all_mixtures` per visited turf - and `process_zone`/`finalize_eq_zone` do so
			from rayon workers, so every zone node was contending for the same global read lock.

			Nothing below calls a DM proc or needs the arena's write lock: the decompression and
			planet-equalize handlers are queued as callbacks and run later on the main thread,
			after this lock has been released.
		*/
		GasArena::with_all_mixtures(|all_mixtures| {
			let mut found_turfs: HashSet<TurfID, FxBuildHasher> = Default::default();
			let mut zoned_turfs = vec![];
			for &cur_index_turf in high_pressure_turfs {
				//is this turf already visited?
				if found_turfs.contains(&cur_index_turf) {
					continue;
				};

				//does this turf exists/enabled/have adjacencies?
				let Some(cur_mixture) = arena.get_from_id(cur_index_turf) else {
					continue;
				};
				let Some(cur_index_node) = arena.get_id(cur_index_turf) else {
					continue;
				};
				if !cur_mixture.enabled()
					|| arena.adjacent_node_ids(cur_index_node).next().is_none()
				{
					continue;
				}

				let is_unshareable = {
					let Some(our_mix) = all_mixtures.get(cur_mixture.mix) else {
						continue;
					};
					let our_moles = our_mix.read().total_moles();
					our_moles < 10.0
						|| arena
							.adjacent_mixes(cur_index_node, all_mixtures)
							.all(|lock| {
								(lock.read().total_moles() - our_moles).abs()
									< MINIMUM_MOLES_DELTA_TO_MOVE
							})
				};

				//does this turf or its adjacencies have enough moles to share?
				if is_unshareable {
					continue;
				}

				match flood_fill_zones(
					(cur_index_node, cur_index_turf),
					equalize_hard_turf_limit,
					&mut found_turfs,
					arena,
					all_mixtures,
					(start_time, remaining_time),
				) {
					FloodFillResult::Complete(zone, num) => {
						zoned_turfs.push((zone, num));
					}
					FloodFillResult::Overtime => return true,
					FloodFillResult::ZoneIgnored => (),
				}
			}

			if start_time.elapsed() >= remaining_time {
				return true;
			}

			let turfs = zoned_turfs
				.into_par_iter()
				.map(|(graph, total_moles)| {
					let len = graph.node_count();
					process_zone(
						graph,
						total_moles / len as f32,
						arena,
						all_mixtures,
						Some(&turfs_processed),
					)
				})
				.collect::<Vec<_>>();

			if start_time.elapsed() >= remaining_time {
				return true;
			}

			let final_pressures = turfs
				.into_par_iter()
				.filter_map(|graph| finalize_eq_zone(arena, all_mixtures, graph))
				.collect::<Vec<_>>();

			final_pressures
				.into_iter()
				.for_each(send_pressure_differences);
			false
		})
	});
	(turfs_processed.load(Ordering::Relaxed), is_cancelled)
}
