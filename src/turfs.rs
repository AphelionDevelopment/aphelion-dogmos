pub mod groups;
#[cfg(feature = "katmos")]
pub mod katmos;
pub mod processing;
#[cfg(feature = "superconductivity")]
mod superconduct;

use crate::{
	constants::*,
	gas::{gas_slot_for_mix, Mixture},
	GasArena,
};
use bitflags::bitflags;
use byondapi::prelude::*;
use eyre::{Context, Result};
use indexmap::IndexMap;
use parking_lot::{const_rwlock, RwLock, RwLockUpgradableReadGuard};
use petgraph::{graph::NodeIndex, stable_graph::StableDiGraph, visit::EdgeRef, Direction};
use rayon::prelude::*;
use rustc_hash::FxBuildHasher;
use std::hash::{Hash, Hasher};
use std::time::Duration;
use std::{mem::drop, sync::atomic::AtomicU64};

bitflags! {
	#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
	pub struct Directions: u8 {
		const NORTH = 0b1;
		const SOUTH = 0b10;
		const EAST	= 0b100;
		const WEST	= 0b1000;
		const UP 	= 0b10000;
		const DOWN 	= 0b100000;
		const ALL_CARDINALS = Self::NORTH.bits() | Self::SOUTH.bits() | Self::EAST.bits() | Self::WEST.bits();
		const ALL_CARDINALS_MULTIZ = Self::NORTH.bits() | Self::SOUTH.bits() | Self::EAST.bits() | Self::WEST.bits() | Self::UP.bits() | Self::DOWN.bits();
	}

	#[derive(Default, Debug)]
	pub struct SimulationFlags: u8 {
		const SIMULATION_DIFFUSE = 0b1;
		const SIMULATION_ALL = 0b10;
		const SIMULATION_ANY = Self::SIMULATION_DIFFUSE.bits() | Self::SIMULATION_ALL.bits();
	}

	#[derive(Default, Debug)]
	pub struct AdjacentFlags: u8 {
		const ATMOS_ADJACENT_FIRELOCK = 0b10;
	}

	#[derive(Default, Debug, Clone, Copy)]
	pub struct DirtyFlags: u8 {
		const DIRTY_MIX_REF = 0b1;
		const DIRTY_ADJACENT = 0b10;
		const DIRTY_ADJACENT_TO_SPACE = 0b100;
	}
}

#[allow(unused)]
const fn adj_flag_to_idx(adj_flag: Directions) -> u8 {
	match adj_flag {
		Directions::NORTH => 0,
		Directions::SOUTH => 1,
		Directions::EAST => 2,
		Directions::WEST => 3,
		Directions::UP => 4,
		Directions::DOWN => 5,
		_ => 6,
	}
}

#[allow(unused)]
const fn idx_to_adj_flag(idx: u8) -> Directions {
	match idx {
		0 => Directions::NORTH,
		1 => Directions::SOUTH,
		2 => Directions::EAST,
		3 => Directions::WEST,
		4 => Directions::UP,
		5 => Directions::DOWN,
		_ => Directions::from_bits_truncate(0),
	}
}

type TurfID = u32;

// TurfMixture can be treated as "immutable" for all intents and purposes--put other data somewhere else
#[derive(Default, Debug)]
struct TurfMixture {
	pub mix: usize,
	pub id: TurfID,
	pub generation: u32,
	pub flags: SimulationFlags,
	pub planetary_atmos: Option<u32>,
	pub vis_hash: AtomicU64,
}

#[allow(dead_code)]
impl TurfMixture {
	/// Whether the turf is processed at all or not
	pub fn enabled(&self) -> bool {
		self.flags.intersects(SimulationFlags::SIMULATION_ANY)
	}

	/// Whether the turf's gas is immutable or not, see [`super::gas::Mixture`]
	pub fn is_immutable(&self) -> bool {
		GasArena::with_all_mixtures(|all_mixtures| {
			all_mixtures
				.get(self.mix)
				.unwrap_or_else(|| panic!("Gas mixture not found for turf: {}", self.mix))
				.read()
				.is_immutable()
		})
	}
	/// Returns the pressure of the turf's gas, see [`super::gas::Mixture`]
	pub fn return_pressure(&self) -> f32 {
		GasArena::with_all_mixtures(|all_mixtures| {
			all_mixtures
				.get(self.mix)
				.unwrap_or_else(|| panic!("Gas mixture not found for turf: {}", self.mix))
				.read()
				.return_pressure()
		})
	}
	/// Returns the temperature of the turf's gas, see [`super::gas::Mixture`]
	pub fn return_temperature(&self) -> f32 {
		GasArena::with_all_mixtures(|all_mixtures| {
			all_mixtures
				.get(self.mix)
				.unwrap_or_else(|| panic!("Gas mixture not found for turf: {}", self.mix))
				.read()
				.get_temperature()
		})
	}
	/// Returns the total moles of the turf's gas, see [`super::gas::Mixture`]
	pub fn total_moles(&self) -> f32 {
		GasArena::with_all_mixtures(|all_mixtures| {
			all_mixtures
				.get(self.mix)
				.unwrap_or_else(|| panic!("Gas mixture not found for turf: {}", self.mix))
				.read()
				.total_moles()
		})
	}
	/// Clears the turf's airs, see [`super::gas::Mixture`]
	pub fn clear_air(&self) {
		GasArena::with_all_mixtures(|all_mixtures| {
			all_mixtures
				.get(self.mix)
				.unwrap_or_else(|| panic!("Gas mixture not found for turf: {}", self.mix))
				.write()
				.clear();
		});
	}
	/// Copies from a given gas mixture to the turf's airs, see [`super::gas::Mixture`]
	pub fn copy_from_mutable(&self, sample: &Mixture) {
		GasArena::with_all_mixtures(|all_mixtures| {
			all_mixtures
				.get(self.mix)
				.unwrap_or_else(|| panic!("Gas mixture not found for turf: {}", self.mix))
				.write()
				.copy_from_mutable(sample);
		});
	}
	/// Clears a number of moles from the turf's air
	/// If the number of moles is greater than the turf's total moles, just clears the turf
	pub fn clear_moles(&self, amt: f32) {
		GasArena::with_all_mixtures(|all_mixtures| {
			let moles = all_mixtures
				.get(self.mix)
				.unwrap_or_else(|| panic!("Gas mixture not found for turf: {}", self.mix))
				.read()
				.total_moles();
			if amt >= moles {
				all_mixtures
					.get(self.mix)
					.unwrap_or_else(|| panic!("Gas mixture not found for turf: {}", self.mix))
					.write()
					.clear();
			} else {
				drop(
					all_mixtures
						.get(self.mix)
						.unwrap_or_else(|| panic!("Gas mixture not found for turf: {}", self.mix))
						.write()
						.remove(amt),
				);
			}
		});
	}
	/// Gets a copy of the turf's air, or reports a stale gas-arena slot.
	pub fn get_gas_copy(&self) -> Result<Mixture> {
		let mut ret: Mixture = Mixture::new();
		GasArena::with_all_mixtures(|all_mixtures| -> Result<()> {
			let to_copy = all_mixtures
				.get(self.mix)
				.ok_or_else(|| eyre::eyre!("Gas mixture not found for turf: {}", self.mix))?
				.read();
			ret.copy_from_mutable(&to_copy);
			ret.volume = to_copy.volume;
			Ok(())
		})?;
		Ok(ret)
	}
	/// Invalidates the turf's visibility cache
	/// This turf will most likely be visually updated the next processing cycle
	/// If that is even running
	pub fn invalidate_vis_cache(&self) {
		self.vis_hash.store(0, std::sync::atomic::Ordering::Relaxed);
	}
}

type TurfGraphMap = IndexMap<TurfID, NodeIndex, FxBuildHasher>;

//adjacency/turf infos goes here
#[derive(Debug)]
struct TurfGases {
	graph: StableDiGraph<TurfMixture, AdjacentFlags>,
	map: TurfGraphMap,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TurfRuntimeMetrics {
	pub nodes: usize,
	pub edges: usize,
	pub node_capacity: usize,
	pub edge_capacity: usize,
	pub map_capacity: usize,
	pub turf_mixture_bytes: usize,
}

impl TurfGases {
	pub fn insert_turf(&mut self, tmix: TurfMixture) {
		if let Some(&node_id) = self.map.get(&tmix.id) {
			let thin = self.graph.node_weight_mut(node_id).unwrap();
			*thin = tmix
		} else {
			self.map.insert(tmix.id, self.graph.add_node(tmix));
		}
	}
	pub fn remove_turf(&mut self, id: TurfID) {
		if let Some(index) = self.map.shift_remove(&id) {
			self.graph.remove_node(index);
		}
	}
	pub fn update_adjacencies(&mut self, idx: TurfID, adjacent_list: ByondValue) -> Result<()> {
		if let Some(&this_index) = self.map.get(&idx) {
			self.remove_adjacencies(this_index);
			adjacent_list
				.iter()?
				.filter_map(|(k, v)| Some((k.get_ref().ok()?, v.get_number().unwrap_or(0.0) as u8)))
				.filter_map(|(adj_ref, flag)| Some((self.map.get(&adj_ref)?, flag)))
				.for_each(|(adj_index, flag)| {
					let flags = AdjacentFlags::from_bits_truncate(flag);
					self.graph.add_edge(this_index, *adj_index, flags);
				})
		};
		Ok(())
	}

	pub fn remove_adjacencies(&mut self, index: NodeIndex) {
		let edges = self
			.graph
			.edges(index)
			.map(|edgeref| edgeref.id())
			.collect::<Vec<_>>();
		edges.into_iter().for_each(|edgeindex| {
			self.graph.remove_edge(edgeindex);
		});
	}

	pub fn get(&self, idx: NodeIndex) -> Option<&TurfMixture> {
		self.graph.node_weight(idx)
	}

	#[allow(unused)]
	pub fn get_from_id(&self, idx: TurfID) -> Option<&TurfMixture> {
		self.map
			.get(&idx)
			.and_then(|&idx| self.graph.node_weight(idx))
	}

	#[allow(unused)]
	pub fn get_id(&self, idx: TurfID) -> Option<NodeIndex> {
		self.map.get(&idx).copied()
	}

	pub fn adjacent_node_ids(&self, index: NodeIndex) -> impl Iterator<Item = NodeIndex> + '_ {
		self.graph.neighbors(index)
	}

	#[allow(unused)]
	pub fn adjacent_turf_ids(&self, index: NodeIndex) -> impl Iterator<Item = TurfID> + '_ {
		self.graph
			.neighbors(index)
			.filter_map(|index| Some(self.get(index)?.id))
	}

	#[allow(unused)]
	pub fn adjacent_node_ids_enabled(
		&self,
		index: NodeIndex,
	) -> impl Iterator<Item = NodeIndex> + '_ {
		self.graph.neighbors(index).filter(|&adj_index| {
			self.graph
				.node_weight(adj_index)
				.is_some_and(|mix| mix.enabled())
		})
	}

	pub fn adjacent_mixes<'a>(
		&'a self,
		index: NodeIndex,
		all_mixtures: &'a [parking_lot::RwLock<Mixture>],
	) -> impl Iterator<Item = &'a parking_lot::RwLock<Mixture>> {
		self.graph
			.neighbors(index)
			.filter_map(|neighbor| self.graph.node_weight(neighbor))
			.filter_map(move |idx| all_mixtures.get(idx.mix))
	}

	pub fn adjacent_mixes_with_adj_ids<'a>(
		&'a self,
		index: NodeIndex,
		all_mixtures: &'a [parking_lot::RwLock<Mixture>],
		dir: Direction,
	) -> impl Iterator<Item = (&'a TurfID, &'a parking_lot::RwLock<Mixture>)> {
		self.graph
			.neighbors_directed(index, dir)
			.filter_map(|neighbor| self.graph.node_weight(neighbor))
			.filter_map(move |idx| Some((&idx.id, all_mixtures.get(idx.mix)?)))
	}
	pub fn clear(&mut self) {
		self.graph.clear();
		self.map.clear();
	}

	/*
	pub fn adjacent_infos(
		&self,
		index: NodeIndex,
		dir: Direction,
	) -> impl Iterator<Item = &TurfMixture> {
		self.graph
			.neighbors_directed(index, dir)
			.filter_map(|neighbor| self.graph.node_weight(neighbor))
	}

	pub fn adjacent_ids<'a>(&'a self, idx: TurfID) -> impl Iterator<Item = &'a TurfID> {
		self.graph
			.neighbors(*self.map.get(&idx).unwrap())
			.filter_map(|index| self.graph.node_weight(index))
			.map(|tmix| &tmix.id)
	}
	pub fn adjacents_enabled<'a>(&'a self, idx: TurfID) -> impl Iterator<Item = &'a TurfID> {
		self.graph
			.neighbors(*self.map.get(&idx).unwrap())
			.filter_map(|index| self.graph.node_weight(index))
			.filter(|tmix| tmix.enabled())
			.map(|tmix| &tmix.id)
	}
	pub fn get_mixture(&self, idx: TurfID) -> Option<TurfMixture> {
		self.mixtures.read().get(&idx).cloned()
	}
	*/
}

static TURF_GASES: RwLock<Option<TurfGases>> = const_rwlock(None);

// We store planetary atmos by hash of the initial atmos string here for speed.
static PLANETARY_ATMOS: RwLock<Option<IndexMap<u32, Mixture, FxBuildHasher>>> = const_rwlock(None);

//whether there is any tasks running
static TASKS: RwLock<()> = const_rwlock(());

pub fn wait_for_tasks() {
	match TASKS.try_write_for(Duration::from_secs(5)) {
		Some(_) => (),
		None => panic!(
			"Threads failed to release resources within 5 seconds, this may indicate a deadlock!"
		),
	}
}
#[auxmacros::init]
pub fn initialize_turfs() {
	// 10x 255x255 zlevels
	// Reserve room for the graph's expected node and edge counts.
	*TURF_GASES.write() = Some(TurfGases {
		graph: StableDiGraph::with_capacity(650_250, 1_300_500),
		map: IndexMap::with_capacity_and_hasher(650_250, FxBuildHasher),
	});
	*PLANETARY_ATMOS.write() = Some(Default::default());
}

pub fn shutdown_turfs() {
	wait_for_tasks();
	if let Some(turf_gases) = TURF_GASES.write().as_mut() {
		turf_gases.clear();
	}
	if let Some(planetary_atmos) = PLANETARY_ATMOS.write().as_mut() {
		planetary_atmos.clear();
	}
}

pub(crate) fn turf_runtime_metrics() -> TurfRuntimeMetrics {
	let arena = TURF_GASES.read();
	let Some(arena) = arena.as_ref() else {
		return TurfRuntimeMetrics {
			turf_mixture_bytes: std::mem::size_of::<TurfMixture>(),
			..Default::default()
		};
	};
	let (node_capacity, edge_capacity) = arena.graph.capacity();
	TurfRuntimeMetrics {
		nodes: arena.graph.node_count(),
		edges: arena.graph.edge_count(),
		node_capacity,
		edge_capacity,
		map_capacity: arena.map.capacity(),
		turf_mixture_bytes: std::mem::size_of::<TurfMixture>(),
	}
}

#[cfg(feature = "superconductivity")]
pub(crate) fn heat_runtime_metrics() -> superconduct::HeatRuntimeMetrics {
	superconduct::heat_runtime_metrics()
}

#[cfg(feature = "superconductivity")]
pub fn shutdown_turf_heat() {
	superconduct::shutdown_turf_heat();
}

#[cfg(feature = "superconductivity")]
pub fn prepare_turf_heat_for_world() {
	superconduct::prepare_turf_heat_for_world();
}

fn with_turf_gases_read<T, F>(f: F) -> T
where
	F: FnOnce(&TurfGases) -> T,
{
	f(TURF_GASES.read().as_ref().unwrap())
}

/// Returns whether a gas-arena slot is still named by a turf graph node.
pub(crate) fn gas_mix_is_referenced(mix: usize) -> bool {
	TURF_GASES
		.read()
		.as_ref()
		.is_some_and(|arena| arena.graph.node_weights().any(|turf| turf.mix == mix))
}

/// Returns the number of on-demand space-boundary nodes in the gas graph.
#[auxmacros::bind("/proc/dogmos_space_boundary_count")]
fn dogmos_space_boundary_count() -> Result<ByondValue> {
	Ok((with_turf_gases_read(|arena| {
		arena
			.graph
			.node_weights()
			.filter(|mix| !mix.enabled())
			.count()
	}) as f32)
		.into())
}

fn with_turf_gases_write<T, F>(f: F) -> T
where
	F: FnOnce(&mut TurfGases) -> T,
{
	f(TURF_GASES.write().as_mut().unwrap())
}

/// Returns whether a queued callback still targets the current occupant of a stable turf ref.
pub(crate) fn turf_callback_is_current(id: TurfID, generation: u32) -> bool {
	ByondValue::new_ref(ValueType::Turf, id)
		.read_number_id(byond_string!("dogmos_registration_generation"))
		.is_ok_and(|current| current >= 0.0 && current as u32 == generation)
}

fn with_planetary_atmos<T, F>(f: F) -> T
where
	F: FnOnce(&IndexMap<u32, Mixture, FxBuildHasher>) -> T,
{
	f(PLANETARY_ATMOS.read().as_ref().unwrap())
}

fn with_planetary_atmos_upgradeable_read<T, F>(f: F) -> Result<T>
where
	F: FnOnce(
		RwLockUpgradableReadGuard<'_, Option<IndexMap<u32, Mixture, FxBuildHasher>>>,
	) -> Result<T>,
{
	f(PLANETARY_ATMOS.upgradable_read())
}

/// Sentinel used to register a discovered space turf as a gas-graph boundary without processing it.
/// All space turfs share one immutable DM mixture, so boundary nodes must never receive diffusion
/// writes.
const SPACE_BOUNDARY_FLAG: i32 = -2;
/// Mirrors DOGMOS_SIMULATION_REMOVE in code/__DEFINES/dogmos_defines.dm.
const REMOVE_TURF_FLAG: i32 = -3;

/// Returns: null. Updates turf air infos, whether the turf is closed, is space or a regular turf, or even a planet turf is decided here.
#[auxmacros::bind("/turf/proc/update_air_ref")]
fn hook_register_turf(src: ByondValue, flag: ByondValue) -> Result<ByondValue> {
	let id = src.get_ref()?;
	let raw_flag = flag.get_number()? as i32;
	if raw_flag == REMOVE_TURF_FLAG {
		with_turf_gases_write(|arena| arena.remove_turf(id));
		#[cfg(feature = "superconductivity")]
		superconduct::remove_turf(id);
		return Ok(ByondValue::null());
	}
	let is_space_boundary = raw_flag == SPACE_BOUNDARY_FLAG;
	let flag = if is_space_boundary { 0 } else { raw_flag };
	if let Ok(blocks) = src.read_number_id(byond_string!("blocks_air")) {
		if blocks > 0.0 {
			with_turf_gases_write(|arena| arena.remove_turf(id));
			#[cfg(feature = "superconductivity")]
			superconduct::supercond_update_ref(src)?;
			return Ok(ByondValue::null());
		}
	}
	if flag >= 0 {
		let mut to_insert: TurfMixture = TurfMixture::default();
		let air = src.read_var_id(byond_string!("air"))?;
		to_insert.mix = gas_slot_for_mix(&air)?;
		to_insert.flags = SimulationFlags::from_bits_truncate(flag as u8);
		to_insert.id = id;
		to_insert.generation = src
			.read_number_id(byond_string!("dogmos_registration_generation"))
			.unwrap_or(0.0) as u32;

		if !is_space_boundary {
			if let Ok(is_planet) = src.read_number_id(byond_string!("planetary_atmos")) {
				if is_planet != 0.0 {
					if let Ok(at_str) = src.read_string_id(byond_string!("initial_gas_mix")) {
						with_planetary_atmos_upgradeable_read(|lock| {
							to_insert.planetary_atmos = Some({
								let mut state = rustc_hash::FxHasher::default();
								at_str.hash(&mut state);
								state.finish() as u32
							});
							if lock
								.as_ref()
								.unwrap()
								.contains_key(&to_insert.planetary_atmos.unwrap())
							{
								return Ok(());
							}

							let mut write =
								parking_lot::lock_api::RwLockUpgradableReadGuard::upgrade(lock);

							write
								.as_mut()
								.unwrap()
								.insert(to_insert.planetary_atmos.unwrap(), {
									let mut gas = to_insert.get_gas_copy()?;
									gas.mark_immutable();
									gas
								});
							Ok(())
						})?;
					}
				}
			}
		}

		let mix_index = to_insert.mix;
		with_turf_gases_write(|arena| arena.insert_turf(to_insert));

		if is_space_boundary {
			GasArena::with_all_mixtures(|all_mixtures| {
				if let Some(entry) = all_mixtures.get(mix_index) {
					entry.write().mark_immutable();
				}
			});
			// Space never participates in Dogmos' heat graph - an ordinary open border to space
			// already never gets a heat edge under the existing gas-adjacency-vs-conductivity-
			// blocked-directions rule (superconduction only runs where gas can't flow), and this
			// registration path is scoped to fixing gas diffusion/breach detection specifically,
			// not to reopening the already-tested SSAIR_SUPERCONDUCTIVITY blackbody model to a new
			// space-as-a-live-heat-node interaction this session hasn't verified.
			return Ok(ByondValue::null());
		}
	} else {
		with_turf_gases_write(|arena| arena.remove_turf(id));
	}

	#[cfg(feature = "superconductivity")]
	superconduct::supercond_update_ref(src)?;
	Ok(ByondValue::null())
}

/// Updates adjacency infos for turfs, only use this in immediateupdateturfs.
///
/// The map bounds are passed from DM because World intrinsic properties are not regular FFI vars.
#[auxmacros::bind("/turf/proc/__update_auxtools_turf_adjacency_info")]
fn hook_infos(src: ByondValue, _max_x: ByondValue, _max_y: ByondValue) -> Result<ByondValue> {
	let id = src.get_ref()?;
	let adjacent_list = src
		.read_var_id(byond_string!("atmos_adjacent_turfs"))
		.ok()
		.and_then(|adjs| adjs.is_list().then_some(adjs));
	with_turf_gases_write(|arena| -> Result<()> {
		if let Some(adjacent_list) = adjacent_list {
			arena.update_adjacencies(id, adjacent_list)?;
		} else if let Some(&idx) = arena.map.get(&id) {
			arena.remove_adjacencies(idx);
		}
		Ok(())
	})?;

	#[cfg(feature = "superconductivity")]
	superconduct::supercond_update_adjacencies(
		id,
		_max_x.get_number()? as i32,
		_max_y.get_number()? as i32,
	)?;
	Ok(ByondValue::null())
}

/// Updates the visual overlays for the given turf.
/// Will use a cached overlay list if one exists.
///
/// Gas overlays are indexed by gas id, plane offset, and visibility factor because each z-level uses
/// a distinct render plane. The overlay objects are shared with the DM gas metadata.
/// # Errors
/// If auxgm wasn't implemented properly or there's an invalid gas mixture.
fn update_visuals(src: ByondValue) -> Result<ByondValue> {
	use super::gas;
	match src.read_var_id(byond_string!("air")) {
		Ok(air) if !air.is_null() => {
			// gas_overlays: list( GAS_ID = list( PLANE_OFFSET+1 = list( VIS_FACTORS = OVERLAYS ))) got it? I don't
			let gas_overlays = ByondValue::new_global_ref()
				.read_var_id(byond_string!("GLOB"))
				.wrap_err("Unable to get GLOB from BYOND globals")?
				.read_var_id(byond_string!("gas_data"))
				.wrap_err("gas_data is undefined on GLOB")?
				.read_var_id(byond_string!("overlays"))
				.wrap_err("overlays is undefined in GLOB.gas_data")?;

			// Mirrors GET_TURF_PLANE_OFFSET(src) - only look at z_level_to_plane_offset at all if
			// multiz plane offsetting is actually in use, matching that macro's own fast path for the
			// common single-plane case.
			let ssmapping = ByondValue::new_global_ref()
				.read_var_id(byond_string!("SSmapping"))
				.wrap_err("Unable to get SSmapping from BYOND globals")?;
			let max_plane_offset = ssmapping
				.read_number_id(byond_string!("max_plane_offset"))
				.unwrap_or(0.0);
			let plane_offset_index = if max_plane_offset > 0.0 {
				let z = src.read_number_id(byond_string!("z")).unwrap_or(0.0);
				let offset = ssmapping
					.read_var_id(byond_string!("z_level_to_plane_offset"))
					.ok()
					.and_then(|list| list.read_list_index(z).ok())
					.and_then(|v| v.get_number().ok())
					.unwrap_or(0.0);
				offset + 1.0
			} else {
				1.0
			};

			let ptr = gas_slot_for_mix(&air).wrap_err("air has an invalid gas mixture slot")?;
			let overlay_types = GasArena::with_gas_mixture(ptr, |mix| {
				Ok(mix
					.enumerate()
					.filter_map(|(idx, moles)| Some((idx, moles, gas::types::gas_visibility(idx)?)))
					.filter(|(_, moles, amt)| moles > amt)
					// getting the list(PLANE_OFFSET+1 = list(VIS_FACTORS = OVERLAYS)) with GAS_ID
					.filter_map(|(idx, moles, _)| {
						Some((
							gas_overlays.read_list_index(gas::gas_idx_to_id(idx)).ok()?,
							moles,
						))
					})
					// getting the list(VIS_FACTORS = OVERLAYS) with PLANE_OFFSET+1
					.filter_map(|(per_offset_list, moles)| {
						Some((
							per_offset_list.read_list_index(plane_offset_index).ok()?,
							moles,
						))
					})
					// getting the OVERLAYS with VIS_FACTOR
					.filter_map(|(this_overlay_list, moles)| {
						this_overlay_list
							.read_list_index(gas::mixture::visibility_step(moles) as f32)
							.ok()
					})
					.collect::<Vec<_>>())
			})?;

			Ok(src
				.call_id(
					byond_string!("set_visuals"),
					&[overlay_types.as_slice().try_into()?],
				)
				.wrap_err("Calling set_visuals")?)
		}
		// If air is null, clear the visuals
		Ok(_) => Ok(src
			.call_id(byond_string!("set_visuals"), &[])
			.wrap_err("Calling set_visuals with no args")?),
		// If air is not defined, it must be a closed turf. Do .othing
		Err(_) => Ok(ByondValue::null()),
	}
}

const fn adjacent_tile_id(id: u8, i: TurfID, max_x: i32, max_y: i32) -> TurfID {
	let z_size = max_x * max_y;
	let i = i as i32;
	match id {
		0 => (i + max_x) as TurfID,
		1 => (i - max_x) as TurfID,
		2 => (i + 1) as TurfID,
		3 => (i - 1) as TurfID,
		4 => (i + z_size) as TurfID,
		5 => (i - z_size) as TurfID,
		_ => panic!("Invalid id passed to adjacent_tile_id!"),
	}
}

#[derive(Clone, Copy)]
struct AdjacentTileIDs {
	adj: Directions,
	i: TurfID,
	max_x: i32,
	max_y: i32,
	count: u8,
}

impl Iterator for AdjacentTileIDs {
	type Item = (Directions, TurfID);

	fn next(&mut self) -> Option<Self::Item> {
		loop {
			if self.count == 6 {
				return None;
			}
			//SAFETY: count can never be invalid
			let dir = Directions::from_bits_retain(1 << self.count);
			self.count += 1;
			if self.adj.contains(dir) {
				return Some((
					dir,
					adjacent_tile_id(self.count - 1, self.i, self.max_x, self.max_y),
				));
			}
		}
	}

	fn size_hint(&self) -> (usize, Option<usize>) {
		(0, Some(self.adj.bits().count_ones() as usize))
	}
}

use std::iter::FusedIterator;

impl FusedIterator for AdjacentTileIDs {}

#[allow(unused)]
fn adjacent_tile_ids(adj: Directions, i: TurfID, max_x: i32, max_y: i32) -> AdjacentTileIDs {
	AdjacentTileIDs {
		adj,
		i,
		max_x,
		max_y,
		count: 0,
	}
}

#[cfg(test)]
mod tests {
	use super::{initialize_turfs, turf_runtime_metrics};

	#[test]
	fn turf_runtime_metrics_report_source_layout_and_reserved_capacity() {
		initialize_turfs();
		let metrics = turf_runtime_metrics();
		assert_eq!(metrics.turf_mixture_bytes, 32);
		assert_eq!(metrics.node_capacity, 650_250);
		assert_eq!(metrics.edge_capacity, 1_300_500);
		assert_eq!(metrics.map_capacity, 650_250);
	}
}
