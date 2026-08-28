use crate::{
	metadata::{
		GasMetadata, GasMetadataError, GasMetadataRegistry, ReactionMetadata,
		ReactionMetadataError, ReactionMetadataRegistry, TurfHandle,
	},
	numerics::diffusion::{
		diffusion_step_into_cancellable, validate_graph, DiffusionError, DiffusionGraph,
		DirectedEdge, GraphNode, NodeHandle,
	},
	MixtureHandle, MAX_GAS_SLOTS,
};
use std::{
	collections::{BTreeMap, BTreeSet},
	error::Error,
	fmt,
};

#[derive(Clone)]
struct MixtureRecord {
	revision: u32,
	temperature: f32,
	volume: f32,
	minimum_heat_capacity: f32,
	gases: [f32; MAX_GAS_SLOTS],
	immutable: bool,
}

impl MixtureRecord {
	fn new() -> Self {
		Self {
			revision: 0,
			temperature: MINIMUM_TEMPERATURE_K,
			volume: DEFAULT_MIXTURE_VOLUME_LITERS,
			minimum_heat_capacity: 0.0,
			gases: [0.0; MAX_GAS_SLOTS],
			immutable: false,
		}
	}
}

#[derive(Clone, Default)]
struct MixtureSlot {
	generation: Option<u32>,
	mixture: Option<MixtureRecord>,
}

#[derive(Clone)]
struct TurfRecord {
	mixture: Option<MixtureHandle>,
	heat: Option<TurfHeatState>,
}

#[derive(Clone, Default)]
struct TurfSlot {
	generation: Option<u32>,
	turf: Option<TurfRecord>,
}

#[derive(Clone, Copy)]
struct ProjectedSlot {
	generation: Option<u32>,
	occupied: bool,
}

const MINIMUM_TEMPERATURE_K: f32 = 2.7;
const DEFAULT_MIXTURE_VOLUME_LITERS: f32 = 2500.0;
const GAS_MIN_MOLES: f32 = 0.0001;
const MOLAR_ACCURACY: f32 = 0.0001;
const MINIMUM_HEAT_CAPACITY: f32 = 0.0003;
const DEFAULT_EVENT_CAPACITY: u32 = 4096;
const MINIMUM_MOLES_DELTA_TO_MOVE: f32 = 0.010_326_37;
const EXCITED_GROUP_PRESSURE_GOAL_KPA: f32 = 0.5;
const DEFAULT_EQUALIZE_HARD_TURF_LIMIT: u32 = 2000;
const IDEAL_GAS_CONSTANT: f32 = 8.31;
const MINIMUM_TEMPERATURE_DELTA_TO_CONSIDER: f32 = 0.5;
const MINIMUM_TEMPERATURE_FOR_SUPERCONDUCTION_K: f32 = 373.15;
const MINIMUM_TEMPERATURE_START_SUPERCONDUCTION_K: f32 = 673.15;
const OPEN_HEAT_TRANSFER_COEFFICIENT: f32 = 0.4;
const STEFAN_BOLTZMANN_CONSTANT: f64 = 5.670_373e-8;
const RADIATION_FROM_SPACE: f64 = STEFAN_BOLTZMANN_CONSTANT
	* MINIMUM_TEMPERATURE_K as f64
	* MINIMUM_TEMPERATURE_K as f64
	* MINIMUM_TEMPERATURE_K as f64
	* MINIMUM_TEMPERATURE_K as f64;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct EdgeKey {
	left: u32,
	right: u32,
}

impl EdgeKey {
	fn new(left: u32, right: u32) -> Result<Self, WorldError> {
		if left == right {
			return Err(WorldError::SelfAdjacency(left));
		}
		Ok(Self {
			left: left.min(right),
			right: left.max(right),
		})
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecycleAction {
	Register,
	Unregister,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LifecycleMutation {
	pub action: LifecycleAction,
	pub handle: MixtureHandle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TurfLifecycleMutation {
	Register {
		handle: TurfHandle,
		mixture: Option<MixtureHandle>,
	},
	Unregister {
		handle: TurfHandle,
	},
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdjacencyMutation {
	pub left: MixtureHandle,
	pub right: MixtureHandle,
	pub conductivity: f32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TurfAdjacencyMutation {
	pub left: TurfHandle,
	pub right: TurfHandle,
	pub connected: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TurfFirelockMutation {
	pub left: TurfHandle,
	pub right: TurfHandle,
	pub firelock: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TurfHeatState {
	pub temperature: f32,
	pub thermal_conductivity: f32,
	pub heat_capacity: f32,
	pub adjacent_to_space: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TurfHeatMutation {
	pub handle: TurfHandle,
	pub state: Option<TurfHeatState>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TurfHeatAdjacencyMutation {
	pub left: TurfHandle,
	pub right: TurfHandle,
	pub connected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MixtureStateMutation {
	pub handle: MixtureHandle,
	pub expected_revision: u32,
	pub temperature: f32,
	pub volume: f32,
	pub gases: [f32; MAX_GAS_SLOTS],
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
	Snapshot {
		handle: MixtureHandle,
	},
	SetMoles {
		handle: MixtureHandle,
		gas: crate::metadata::GasId,
		amount: f32,
	},
	AdjustMoles {
		handle: MixtureHandle,
		gas: crate::metadata::GasId,
		delta: f32,
	},
	AdjustMolesTemperature {
		handle: MixtureHandle,
		gas: crate::metadata::GasId,
		amount: f32,
		temperature: f32,
	},
	AdjustMultiple {
		handle: MixtureHandle,
		adjustments: Box<[(crate::metadata::GasId, f32)]>,
	},
	GetMoles {
		handle: MixtureHandle,
		gas: crate::metadata::GasId,
	},
	Temperature {
		handle: MixtureHandle,
	},
	Volume {
		handle: MixtureHandle,
	},
	HeatCapacity {
		handle: MixtureHandle,
	},
	PartialHeatCapacity {
		handle: MixtureHandle,
		gas: crate::metadata::GasId,
	},
	TotalMoles {
		handle: MixtureHandle,
	},
	Pressure {
		handle: MixtureHandle,
	},
	ThermalEnergy {
		handle: MixtureHandle,
	},
	GetMolesByFlags {
		handle: MixtureHandle,
		flags: u32,
	},
	Burnability {
		handle: MixtureHandle,
		temperature: Option<f32>,
	},
	SetTemperature {
		handle: MixtureHandle,
		temperature: f32,
	},
	SetVolume {
		handle: MixtureHandle,
		volume: f32,
	},
	SetMinimumHeatCapacity {
		handle: MixtureHandle,
		amount: f32,
	},
	Clear {
		handle: MixtureHandle,
	},
	Add {
		handle: MixtureHandle,
		amount: f32,
	},
	Multiply {
		handle: MixtureHandle,
		factor: f32,
	},
	CopyFrom {
		receiver: MixtureHandle,
		giver: MixtureHandle,
	},
	AdjustHeat {
		handle: MixtureHandle,
		heat: f32,
	},
	Compare {
		left: MixtureHandle,
		right: MixtureHandle,
	},
	EqualizeWith {
		receiver: MixtureHandle,
		total: MixtureHandle,
	},
	TemperatureShare {
		first: MixtureHandle,
		second: MixtureHandle,
		conduction_coefficient: f32,
	},
	TemperatureShareNonGas {
		handle: MixtureHandle,
		conduction_coefficient: f32,
		sharer_temperature: f32,
		sharer_heat_capacity: f32,
	},
	MarkImmutable {
		handle: MixtureHandle,
	},
	IsImmutable {
		handle: MixtureHandle,
	},
	Merge {
		receiver: MixtureHandle,
		giver: MixtureHandle,
	},
	RemoveRatioInto {
		source: MixtureHandle,
		destination: MixtureHandle,
		ratio: f32,
	},
	RemoveAmountInto {
		source: MixtureHandle,
		destination: MixtureHandle,
		amount: f32,
	},
	TransferGases {
		source: MixtureHandle,
		destination: MixtureHandle,
		ratio: f32,
		gases: Box<[crate::metadata::GasId]>,
	},
	TransferAmount {
		source: MixtureHandle,
		destination: MixtureHandle,
		amount: f32,
	},
	TransferRatio {
		source: MixtureHandle,
		destination: MixtureHandle,
		ratio: f32,
	},
	TransferByFlags {
		source: MixtureHandle,
		destination: MixtureHandle,
		flags: u32,
		amount: f32,
	},
	ShareRatio {
		first: MixtureHandle,
		second: MixtureHandle,
		ratio: f32,
		one_way: bool,
	},
	ResumeReaction {
		continuation: ReactionContinuationToken,
	},
}

#[derive(Clone, Debug, PartialEq)]
pub enum CommandResult {
	Applied { updated: u32 },
	Snapshot(MixtureSnapshot),
	Scalar(f32),
	Scalars([f32; 2]),
	Boolean(bool),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorldStage {
	ProcessTurfs,
	Equalize,
	ExcitedGroups,
	React,
	TurfHeat,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(C)]
pub struct ReactionContinuationToken {
	pub slot: u32,
	pub generation: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WorldEvent {
	PressureDifference {
		source: TurfHandle,
		target: TurfHandle,
		moles: f32,
	},
	DecompressionFloorRip {
		turf: TurfHandle,
		moles_lost: f32,
	},
	FirelockConsideration {
		source: TurfHandle,
		target: TurfHandle,
	},
	RunDmReaction {
		turf: TurfHandle,
		mixture: MixtureHandle,
		reaction: crate::metadata::ReactionId,
		continuation: ReactionContinuationToken,
	},
	ReactionFinished {
		turf: TurfHandle,
		mixture: MixtureHandle,
		reaction: crate::metadata::ReactionId,
		kind: crate::metadata::NativeReactionKind,
		values: [f32; 4],
	},
	TurfDestructionRequest {
		turf: TurfHandle,
	},
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StageResult {
	pub work_items: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MixtureSnapshot {
	pub revision: u32,
	pub temperature: f32,
	pub volume: f32,
	pub minimum_heat_capacity: f32,
	pub gases: [f32; MAX_GAS_SLOTS],
	pub immutable: bool,
}

#[derive(Debug, PartialEq)]
pub enum WorldError {
	GasMetadata(GasMetadataError),
	GasRegistryAlreadyInstalled,
	GasRegistryInstallationTooLate,
	GasRegistryMissing,
	ReactionMetadata(ReactionMetadataError),
	ReactionRegistryAlreadyInstalled,
	ReactionRegistryInstallationTooLate,
	ReactionRegistryMissing,
	UnknownHandle(MixtureHandle),
	StaleHandle {
		requested: MixtureHandle,
		current: u32,
	},
	UnknownTurfHandle(TurfHandle),
	StaleTurfHandle {
		requested: TurfHandle,
		current: u32,
	},
	RevisionMismatch {
		handle: MixtureHandle,
		expected: u32,
		actual: u32,
	},
	RevisionExhausted(MixtureHandle),
	DuplicateMixtureState(u32),
	InvalidMixtureState,
	InvalidGasId(crate::metadata::GasId),
	InvalidMoleAmount,
	InvalidMoleDelta,
	MoleOverflow(crate::metadata::GasId),
	InvalidTemperature,
	InvalidVolume,
	InvalidMinimumHeatCapacity,
	InvalidAddend,
	InvalidMultiplier,
	InvalidHeat,
	InvalidHeatCapacity,
	InvalidRatio,
	SameMixtureHandles(MixtureHandle),
	SelfAdjacency(u32),
	SelfTurfAdjacency(TurfHandle),
	TurfMissingMixture(TurfHandle),
	DuplicateMutableTurfMixture(MixtureHandle),
	InvalidTurfHeatState(TurfHandle),
	TurfHeatMissing(TurfHandle),
	SelfTurfHeatAdjacency(TurfHandle),
	ImmutableEqualizationBoundary(TurfHandle),
	EventCapacityExceeded {
		requested: u32,
		capacity: u32,
	},
	UnknownReactionContinuation(ReactionContinuationToken),
	StaleReactionContinuation {
		requested: ReactionContinuationToken,
		current: u32,
	},
	ReactionContinuationCapacityExceeded,
	InvalidConductivity,
	InvalidEqualizeHardTurfLimit,
	InvalidSecondsPerTick,
	StageNotImplemented(WorldStage),
	Graph(String),
	State(String),
	StateCapacityExceeded,
	AllocationFailed,
	Cancelled,
}

impl fmt::Display for WorldError {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(formatter, "{self:?}")
	}
}

impl Error for WorldError {}

pub struct DogmosWorld {
	gas_registry: Option<GasMetadataRegistry>,
	reaction_registry: Option<ReactionMetadataRegistry>,
	mixtures: Vec<MixtureSlot>,
	turfs: Vec<TurfSlot>,
	edges: BTreeMap<EdgeKey, f32>,
	turf_edges: BTreeSet<EdgeKey>,
	turf_firelock_edges: BTreeSet<EdgeKey>,
	heat_edges: BTreeSet<EdgeKey>,
	graph: Option<DiffusionGraph>,
	turf_graph: Option<DiffusionGraph>,
	input: Vec<f32>,
	output: Vec<f32>,
	events: Vec<WorldEvent>,
	max_events: u32,
	max_continuations: u32,
	continuations: Vec<ContinuationSlot>,
	free_continuations: Vec<u32>,
	realistic_space_radiation: bool,
	equalize_hard_turf_limit: u32,
	max_world_bytes: u64,
}

#[derive(Clone)]
struct ReactionContinuation {
	turf: TurfHandle,
	mixture: MixtureHandle,
	next_reaction_index: u32,
}

#[derive(Clone, Default)]
struct ContinuationSlot {
	generation: u32,
	continuation: Option<ReactionContinuation>,
}

struct PendingDmReaction {
	reaction: crate::metadata::ReactionId,
	next_reaction_index: u32,
}

struct ReactionSequence {
	mixture: MixtureRecord,
	events: Vec<WorldEvent>,
	pending: Option<PendingDmReaction>,
	work_items: u32,
	native_updates: u32,
}

impl DogmosWorld {
	pub fn new(max_world_bytes: u64) -> Self {
		Self::new_with_event_capacity(max_world_bytes, DEFAULT_EVENT_CAPACITY)
	}

	pub fn new_with_event_capacity(max_world_bytes: u64, max_events: u32) -> Self {
		Self::new_with_capacities(max_world_bytes, max_events, max_events)
	}

	pub fn new_with_capacities(
		max_world_bytes: u64,
		max_events: u32,
		max_continuations: u32,
	) -> Self {
		Self {
			gas_registry: None,
			reaction_registry: None,
			mixtures: Vec::new(),
			turfs: Vec::new(),
			edges: BTreeMap::new(),
			turf_edges: BTreeSet::new(),
			turf_firelock_edges: BTreeSet::new(),
			heat_edges: BTreeSet::new(),
			graph: None,
			turf_graph: None,
			input: Vec::new(),
			output: Vec::new(),
			events: Vec::new(),
			max_events,
			max_continuations,
			continuations: Vec::new(),
			free_continuations: Vec::new(),
			realistic_space_radiation: true,
			equalize_hard_turf_limit: DEFAULT_EQUALIZE_HARD_TURF_LIMIT,
			max_world_bytes,
		}
	}

	pub fn set_realistic_space_radiation(&mut self, enabled: bool) {
		self.realistic_space_radiation = enabled;
	}

	pub fn set_equalize_hard_turf_limit(&mut self, limit: u32) -> Result<(), WorldError> {
		if limit == 0 {
			return Err(WorldError::InvalidEqualizeHardTurfLimit);
		}
		self.equalize_hard_turf_limit = limit;
		Ok(())
	}

	pub fn install_gases(&mut self, gases: Vec<GasMetadata>) -> Result<u32, WorldError> {
		if self.gas_registry.is_some() {
			return Err(WorldError::GasRegistryAlreadyInstalled);
		}
		if !self.mixtures.is_empty() {
			return Err(WorldError::GasRegistryInstallationTooLate);
		}
		let registry = GasMetadataRegistry::try_new(gases).map_err(WorldError::GasMetadata)?;
		let count = registry.len();
		self.gas_registry = Some(registry);
		Ok(count)
	}

	pub fn gas_registry(&self) -> Option<&GasMetadataRegistry> {
		self.gas_registry.as_ref()
	}

	pub fn install_reactions(
		&mut self,
		reactions: Vec<ReactionMetadata>,
	) -> Result<u32, WorldError> {
		if self.reaction_registry.is_some() {
			return Err(WorldError::ReactionRegistryAlreadyInstalled);
		}
		if !self.mixtures.is_empty() {
			return Err(WorldError::ReactionRegistryInstallationTooLate);
		}
		let gases = self
			.gas_registry
			.as_ref()
			.ok_or(WorldError::GasRegistryMissing)?;
		let registry = ReactionMetadataRegistry::try_new(reactions, gases)
			.map_err(WorldError::ReactionMetadata)?;
		let count = registry.len();
		self.reaction_registry = Some(registry);
		Ok(count)
	}

	pub fn reaction_registry(&self) -> Option<&ReactionMetadataRegistry> {
		self.reaction_registry.as_ref()
	}

	pub fn apply_lifecycle(&mut self, mutations: &[LifecycleMutation]) -> Result<u32, WorldError> {
		let mut projected = BTreeMap::<u32, ProjectedSlot>::new();
		let mut changed = false;
		let mut required_slots = self.mixtures.len();
		for mutation in mutations {
			self.validate_slot_capacity(mutation.handle.slot)?;
			required_slots = required_slots.max(
				usize::try_from(u64::from(mutation.handle.slot) + 1)
					.map_err(|_| WorldError::StateCapacityExceeded)?,
			);
			let current = projected
				.get(&mutation.handle.slot)
				.copied()
				.unwrap_or_else(|| self.projected_slot(mutation.handle.slot));
			match mutation.action {
				LifecycleAction::Register => {
					if let Some(generation) = current.generation {
						if mutation.handle.generation < generation
							|| (!current.occupied && mutation.handle.generation == generation)
						{
							return Err(WorldError::StaleHandle {
								requested: mutation.handle,
								current: generation,
							});
						}
					}
					projected.insert(
						mutation.handle.slot,
						ProjectedSlot {
							generation: Some(mutation.handle.generation),
							occupied: true,
						},
					);
				}
				LifecycleAction::Unregister => {
					let Some(generation) = current.generation else {
						return Err(WorldError::UnknownHandle(mutation.handle));
					};
					if mutation.handle.generation != generation {
						return Err(WorldError::StaleHandle {
							requested: mutation.handle,
							current: generation,
						});
					}
					if !current.occupied {
						return Err(WorldError::UnknownHandle(mutation.handle));
					}
					projected.insert(
						mutation.handle.slot,
						ProjectedSlot {
							generation: Some(generation),
							occupied: false,
						},
					);
				}
			}
		}

		if required_slots > self.mixtures.len() {
			self.mixtures
				.try_reserve_exact(required_slots - self.mixtures.len())
				.map_err(|_| WorldError::AllocationFailed)?;
			self.mixtures
				.resize_with(required_slots, MixtureSlot::default);
		}
		if !mutations.is_empty() {
			self.free_continuations
				.try_reserve(self.continuations.len())
				.map_err(|_| WorldError::AllocationFailed)?;
		}

		for mutation in mutations {
			let slot = mutation.handle.slot as usize;
			match mutation.action {
				LifecycleAction::Register => {
					let replace = self.mixtures[slot].generation
						!= Some(mutation.handle.generation)
						|| self.mixtures[slot].mixture.is_none();
					if replace {
						self.invalidate_continuations_for_mixture_slot(mutation.handle.slot);
						self.mixtures[slot].generation = Some(mutation.handle.generation);
						self.mixtures[slot].mixture = Some(MixtureRecord::new());
						self.remove_incident_edges(mutation.handle.slot);
						changed = true;
					}
				}
				LifecycleAction::Unregister => {
					self.invalidate_continuations_for_mixture_slot(mutation.handle.slot);
					self.mixtures[slot].mixture = None;
					let mut detached_turfs = Vec::new();
					for (turf_slot, turf) in
						self.turfs
							.iter_mut()
							.enumerate()
							.filter_map(|(slot, turf_slot)| {
								turf_slot.turf.as_mut().map(|turf| (slot, turf))
							}) {
						if turf.mixture == Some(mutation.handle) {
							turf.mixture = None;
							detached_turfs.push(turf_slot as u32);
						}
					}
					for turf_slot in detached_turfs {
						self.remove_incident_turf_edges(turf_slot);
					}
					self.turf_graph = None;
					self.remove_incident_edges(mutation.handle.slot);
					changed = true;
				}
			}
		}
		if changed {
			self.graph = None;
		}
		Ok(mutations.len() as u32)
	}

	pub fn apply_turf_lifecycle(
		&mut self,
		mutations: &[TurfLifecycleMutation],
	) -> Result<u32, WorldError> {
		let mut projected = BTreeMap::<u32, ProjectedSlot>::new();
		let mut required_slots = self.turfs.len();
		for mutation in mutations {
			let (handle, mixture) = match mutation {
				TurfLifecycleMutation::Register { handle, mixture } => (*handle, *mixture),
				TurfLifecycleMutation::Unregister { handle } => (*handle, None),
			};
			if let Some(mixture) = mixture {
				self.require_handle(mixture)?;
			}
			self.validate_turf_slot_capacity(handle.slot)?;
			required_slots = required_slots.max(
				usize::try_from(u64::from(handle.slot) + 1)
					.map_err(|_| WorldError::StateCapacityExceeded)?,
			);
			let current = projected
				.get(&handle.slot)
				.copied()
				.unwrap_or_else(|| self.projected_turf_slot(handle.slot));
			match mutation {
				TurfLifecycleMutation::Register { .. } => {
					if let Some(generation) = current.generation {
						if handle.generation < generation
							|| (!current.occupied && handle.generation == generation)
						{
							return Err(WorldError::StaleTurfHandle {
								requested: handle,
								current: generation,
							});
						}
					}
					projected.insert(
						handle.slot,
						ProjectedSlot {
							generation: Some(handle.generation),
							occupied: true,
						},
					);
				}
				TurfLifecycleMutation::Unregister { .. } => {
					let Some(generation) = current.generation else {
						return Err(WorldError::UnknownTurfHandle(handle));
					};
					if handle.generation != generation {
						return Err(WorldError::StaleTurfHandle {
							requested: handle,
							current: generation,
						});
					}
					if !current.occupied {
						return Err(WorldError::UnknownTurfHandle(handle));
					}
					projected.insert(
						handle.slot,
						ProjectedSlot {
							generation: Some(generation),
							occupied: false,
						},
					);
				}
			}
		}

		if required_slots > self.turfs.len() {
			self.turfs
				.try_reserve_exact(required_slots - self.turfs.len())
				.map_err(|_| WorldError::AllocationFailed)?;
			self.turfs.resize_with(required_slots, TurfSlot::default);
		}
		if !mutations.is_empty() {
			self.free_continuations
				.try_reserve(self.continuations.len())
				.map_err(|_| WorldError::AllocationFailed)?;
		}

		for mutation in mutations {
			match mutation {
				TurfLifecycleMutation::Register { handle, mixture } => {
					let invalidates_continuation = self.turfs[handle.slot as usize].generation
						!= Some(handle.generation)
						|| self.turfs[handle.slot as usize]
							.turf
							.as_ref()
							.is_none_or(|turf| turf.mixture != *mixture);
					if invalidates_continuation {
						self.invalidate_continuations_for_turf_slot(handle.slot);
					}
					let slot = &mut self.turfs[handle.slot as usize];
					let remove_edges = mixture.is_none();
					let heat = (slot.generation == Some(handle.generation))
						.then(|| slot.turf.as_ref().and_then(|turf| turf.heat))
						.flatten();
					slot.generation = Some(handle.generation);
					slot.turf = Some(TurfRecord {
						mixture: *mixture,
						heat,
					});
					if remove_edges {
						self.remove_incident_turf_edges(handle.slot);
					}
					if heat.is_none() {
						self.remove_incident_heat_edges(handle.slot);
					}
				}
				TurfLifecycleMutation::Unregister { handle } => {
					self.invalidate_continuations_for_turf_slot(handle.slot);
					self.turfs[handle.slot as usize].turf = None;
					self.remove_incident_turf_edges(handle.slot);
					self.remove_incident_heat_edges(handle.slot);
				}
			}
		}
		if !mutations.is_empty() {
			self.turf_graph = None;
		}
		Ok(mutations.len() as u32)
	}

	pub fn apply_turf_heat(&mut self, mutations: &[TurfHeatMutation]) -> Result<u32, WorldError> {
		for mutation in mutations {
			self.require_turf_handle(mutation.handle)?;
			if let Some(state) = mutation.state {
				if !valid_turf_heat_state(state) {
					return Err(WorldError::InvalidTurfHeatState(mutation.handle));
				}
			}
		}
		for mutation in mutations {
			self.turfs[mutation.handle.slot as usize]
				.turf
				.as_mut()
				.expect("turf heat batch was validated")
				.heat = mutation.state;
			if mutation.state.is_none() {
				self.remove_incident_heat_edges(mutation.handle.slot);
			}
		}
		Ok(mutations.len() as u32)
	}

	pub fn apply_turf_heat_adjacency(
		&mut self,
		mutations: &[TurfHeatAdjacencyMutation],
	) -> Result<u32, WorldError> {
		let mut candidate = self.heat_edges.clone();
		for mutation in mutations {
			let left = self.require_turf_handle(mutation.left)?;
			let right = self.require_turf_handle(mutation.right)?;
			if left.heat.is_none() {
				return Err(WorldError::TurfHeatMissing(mutation.left));
			}
			if right.heat.is_none() {
				return Err(WorldError::TurfHeatMissing(mutation.right));
			}
			if mutation.left.slot == mutation.right.slot {
				return Err(WorldError::SelfTurfHeatAdjacency(mutation.left));
			}
			let key = EdgeKey {
				left: mutation.left.slot.min(mutation.right.slot),
				right: mutation.left.slot.max(mutation.right.slot),
			};
			if mutation.connected {
				candidate.insert(key);
			} else {
				candidate.remove(&key);
			}
		}
		self.heat_edges = candidate;
		Ok(mutations.len() as u32)
	}

	pub fn apply_turf_adjacency(
		&mut self,
		mutations: &[TurfAdjacencyMutation],
	) -> Result<u32, WorldError> {
		let mut candidate = self.turf_edges.clone();
		let mut candidate_firelocks = self.turf_firelock_edges.clone();
		for mutation in mutations {
			let left = self.require_turf_handle(mutation.left)?;
			let right = self.require_turf_handle(mutation.right)?;
			if left.mixture.is_none() {
				return Err(WorldError::TurfMissingMixture(mutation.left));
			}
			if right.mixture.is_none() {
				return Err(WorldError::TurfMissingMixture(mutation.right));
			}
			if mutation.left.slot == mutation.right.slot {
				return Err(WorldError::SelfTurfAdjacency(mutation.left));
			}
			let key = EdgeKey {
				left: mutation.left.slot.min(mutation.right.slot),
				right: mutation.left.slot.max(mutation.right.slot),
			};
			if mutation.connected {
				candidate.insert(key);
			} else {
				candidate.remove(&key);
				candidate_firelocks.remove(&key);
			}
		}
		if candidate == self.turf_edges {
			return Ok(mutations.len() as u32);
		}
		let graph = self.build_turf_graph(&candidate)?;
		self.turf_edges = candidate;
		self.turf_firelock_edges = candidate_firelocks;
		self.turf_graph = Some(graph);
		Ok(mutations.len() as u32)
	}

	pub fn apply_turf_firelocks(
		&mut self,
		mutations: &[TurfFirelockMutation],
	) -> Result<u32, WorldError> {
		let mut candidate = self.turf_firelock_edges.clone();
		for mutation in mutations {
			self.require_turf_handle(mutation.left)?;
			self.require_turf_handle(mutation.right)?;
			if mutation.left.slot == mutation.right.slot {
				return Err(WorldError::SelfTurfAdjacency(mutation.left));
			}
			let key = EdgeKey {
				left: mutation.left.slot.min(mutation.right.slot),
				right: mutation.left.slot.max(mutation.right.slot),
			};
			if mutation.firelock {
				if !self.turf_edges.contains(&key) {
					return Err(WorldError::Graph(
						"firelock metadata references a disconnected turf edge".into(),
					));
				}
				candidate.insert(key);
			} else {
				candidate.remove(&key);
			}
		}
		self.turf_firelock_edges = candidate;
		Ok(mutations.len() as u32)
	}

	pub fn apply_adjacency(&mut self, mutations: &[AdjacencyMutation]) -> Result<u32, WorldError> {
		let mut candidate = self.edges.clone();
		for mutation in mutations {
			self.require_handle(mutation.left)?;
			self.require_handle(mutation.right)?;
			if !mutation.conductivity.is_finite() || mutation.conductivity < 0.0 {
				return Err(WorldError::InvalidConductivity);
			}
			let key = EdgeKey::new(mutation.left.slot, mutation.right.slot)?;
			if mutation.conductivity == 0.0 {
				candidate.remove(&key);
			} else {
				candidate.insert(key, mutation.conductivity);
			}
		}
		if candidate == self.edges {
			return Ok(mutations.len() as u32);
		}
		let graph = self.build_graph(&candidate)?;
		self.edges = candidate;
		self.graph = Some(graph);
		Ok(mutations.len() as u32)
	}

	pub fn apply_mixture_state(
		&mut self,
		mutations: &[MixtureStateMutation],
	) -> Result<u32, WorldError> {
		let mut slots = Vec::new();
		slots
			.try_reserve_exact(mutations.len())
			.map_err(|_| WorldError::AllocationFailed)?;
		for mutation in mutations {
			let mixture = self.require_handle(mutation.handle)?;
			if mixture.revision != mutation.expected_revision {
				return Err(WorldError::RevisionMismatch {
					handle: mutation.handle,
					expected: mutation.expected_revision,
					actual: mixture.revision,
				});
			}
			if !valid_mixture_state(mutation) {
				return Err(WorldError::InvalidMixtureState);
			}
			if !mixture.immutable && mixture.revision == u32::MAX {
				return Err(WorldError::RevisionExhausted(mutation.handle));
			}
			slots.push(mutation.handle.slot);
		}
		slots.sort_unstable();
		if let Some(duplicate) = slots
			.windows(2)
			.find_map(|pair| (pair[0] == pair[1]).then_some(pair[0]))
		{
			return Err(WorldError::DuplicateMixtureState(duplicate));
		}

		for mutation in mutations {
			let mixture = self
				.mixtures
				.get_mut(mutation.handle.slot as usize)
				.and_then(|slot| slot.mixture.as_mut())
				.expect("mixture state batch was validated before mutation");
			if mixture.immutable {
				continue;
			}
			mixture.temperature = mutation.temperature;
			mixture.volume = mutation.volume;
			mixture.gases = mutation.gases;
			mixture.revision += 1;
		}
		Ok(mutations.len() as u32)
	}

	pub fn apply_command(&mut self, command: Command) -> Result<CommandResult, WorldError> {
		match command {
			Command::Snapshot { handle } => Ok(CommandResult::Snapshot(self.snapshot(handle)?)),
			Command::SetMoles {
				handle,
				gas,
				amount,
			} => {
				let gas_index = self.gas_index(gas)?;
				if !amount.is_finite() || amount < 0.0 {
					return Err(WorldError::InvalidMoleAmount);
				}
				let amount = if amount <= GAS_MIN_MOLES { 0.0 } else { amount };
				let updated = self.mutate_mixture(handle, |mixture| {
					if mixture.immutable || mixture.gases[gas_index] == amount {
						return false;
					}
					mixture.gases[gas_index] = amount;
					true
				})?;
				Ok(CommandResult::Applied {
					updated: u32::from(updated),
				})
			}
			Command::AdjustMoles { handle, gas, delta } => {
				let gas_index = self.gas_index(gas)?;
				if !delta.is_finite() {
					return Err(WorldError::InvalidMoleDelta);
				}
				let updated = self.mutate_mixture(handle, |mixture| {
					if mixture.immutable || delta == 0.0 || !delta.is_normal() {
						return false;
					}
					let adjusted =
						(f64::from(mixture.gases[gas_index]) + f64::from(delta)).max(0.0);
					if adjusted > f64::from(f32::MAX) {
						return false;
					}
					let adjusted = adjusted as f32;
					if mixture.gases[gas_index] == adjusted {
						return false;
					}
					mixture.gases[gas_index] = adjusted;
					true
				})?;
				if !updated {
					let mixture = self.require_handle(handle)?;
					if !mixture.immutable
						&& (f64::from(mixture.gases[gas_index]) + f64::from(delta))
							> f64::from(f32::MAX)
					{
						return Err(WorldError::MoleOverflow(gas));
					}
				}
				Ok(CommandResult::Applied {
					updated: u32::from(updated),
				})
			}
			Command::AdjustMolesTemperature {
				handle,
				gas,
				amount,
				temperature,
			} => {
				if !amount.is_finite() || amount < 0.0 {
					return Err(WorldError::InvalidMoleAmount);
				}
				if !temperature.is_finite() {
					return Err(WorldError::InvalidTemperature);
				}
				if amount <= GAS_MIN_MOLES {
					return Ok(CommandResult::Applied { updated: 0 });
				}
				let gas_index = self.gas_index(gas)?;
				let specific_heat = self
					.gas_registry
					.as_ref()
					.expect("gas index validation requires the registry")
					.specific_heats()[gas_index];
				let before = self.require_handle(handle)?.clone();
				let previous_capacity = self.heat_capacity(&before)?;
				let added_capacity = amount * specific_heat;
				let combined_capacity = previous_capacity + added_capacity;
				let adjusted = (f64::from(before.gases[gas_index]) + f64::from(amount))
					.min(f64::from(f32::MAX)) as f32;
				let mixed_temperature = if combined_capacity > MINIMUM_HEAT_CAPACITY {
					(previous_capacity * before.temperature
						+ added_capacity * temperature.max(MINIMUM_TEMPERATURE_K))
						/ combined_capacity
				} else {
					before.temperature
				};
				let updated = self.mutate_mixture(handle, |mixture| {
					if mixture.immutable {
						return false;
					}
					mixture.gases[gas_index] = adjusted;
					mixture.temperature = mixed_temperature.max(MINIMUM_TEMPERATURE_K);
					true
				})?;
				Ok(CommandResult::Applied {
					updated: u32::from(updated),
				})
			}
			Command::AdjustMultiple {
				handle,
				adjustments,
			} => {
				let before = self.require_handle(handle)?.clone();
				let mut adjusted = BTreeMap::<usize, f64>::new();
				for (gas, delta) in adjustments.iter().copied() {
					let gas_index = self.gas_index(gas)?;
					if !delta.is_finite() {
						return Err(WorldError::InvalidMoleDelta);
					}
					let current = adjusted
						.get(&gas_index)
						.copied()
						.unwrap_or_else(|| f64::from(before.gases[gas_index]));
					let amount = (current + f64::from(delta)).max(0.0);
					if amount > f64::from(f32::MAX) {
						return Err(WorldError::MoleOverflow(gas));
					}
					adjusted.insert(gas_index, amount);
				}
				let updated = self.mutate_mixture(handle, |mixture| {
					if mixture.immutable {
						return false;
					}
					let mut changed = false;
					for (gas_index, amount) in adjusted {
						let amount = amount as f32;
						let amount = if amount <= GAS_MIN_MOLES { 0.0 } else { amount };
						changed |= mixture.gases[gas_index] != amount;
						mixture.gases[gas_index] = amount;
					}
					changed
				})?;
				Ok(CommandResult::Applied {
					updated: u32::from(updated),
				})
			}
			Command::GetMoles { handle, gas } => {
				let gas_index = self.gas_index(gas)?;
				Ok(CommandResult::Scalar(
					self.require_handle(handle)?.gases[gas_index],
				))
			}
			Command::Temperature { handle } => Ok(CommandResult::Scalar(
				self.require_handle(handle)?.temperature,
			)),
			Command::Volume { handle } => {
				Ok(CommandResult::Scalar(self.require_handle(handle)?.volume))
			}
			Command::HeatCapacity { handle } => Ok(CommandResult::Scalar(
				self.heat_capacity(self.require_handle(handle)?)?,
			)),
			Command::PartialHeatCapacity { handle, gas } => {
				let gas_index = self.gas_index(gas)?;
				let specific_heat = self
					.gas_registry
					.as_ref()
					.expect("gas index validation requires the registry")
					.specific_heats()[gas_index];
				let amount = self.require_handle(handle)?.gases[gas_index];
				Ok(CommandResult::Scalar(if amount.is_normal() {
					amount * specific_heat
				} else {
					0.0
				}))
			}
			Command::TotalMoles { handle } => Ok(CommandResult::Scalar(total_moles(
				self.require_handle(handle)?,
			))),
			Command::Pressure { handle } => Ok(CommandResult::Scalar(mixture_pressure(
				self.require_handle(handle)?,
			))),
			Command::ThermalEnergy { handle } => {
				let mixture = self.require_handle(handle)?;
				Ok(CommandResult::Scalar(
					self.heat_capacity(mixture)? * mixture.temperature,
				))
			}
			Command::GetMolesByFlags { handle, flags } => {
				let mixture = self.require_handle(handle)?;
				let gases = self
					.gas_registry
					.as_ref()
					.ok_or(WorldError::GasRegistryMissing)?;
				let amount = gases
					.iter()
					.filter(|gas| gas.flags & flags != 0)
					.fold(0.0, |total, gas| {
						total + mixture.gases[usize::from(gas.id.0)]
					});
				Ok(CommandResult::Scalar(amount))
			}
			Command::Burnability {
				handle,
				temperature,
			} => {
				let mixture = self.require_handle(handle)?;
				let temperature = temperature.unwrap_or(mixture.temperature);
				if !temperature.is_finite() {
					return Err(WorldError::InvalidTemperature);
				}
				let temperature = temperature.max(MINIMUM_TEMPERATURE_K);
				let gases = self
					.gas_registry
					.as_ref()
					.ok_or(WorldError::GasRegistryMissing)?;
				let mut oxidation_power = 0.0;
				let mut fuel_amount = 0.0;
				for gas in gases.iter() {
					let amount = mixture.gases[usize::from(gas.id.0)];
					if amount <= GAS_MIN_MOLES {
						continue;
					}
					match gas.fire_role {
						crate::metadata::GasFireRole::Oxidizer {
							minimum_temperature,
							power,
						} if temperature > minimum_temperature => {
							let available =
								amount * (1.0 - minimum_temperature / temperature).max(0.0);
							oxidation_power += available * power;
						}
						crate::metadata::GasFireRole::Fuel {
							minimum_temperature,
							burn_rate,
						} if temperature > minimum_temperature => {
							let available =
								amount * (1.0 - minimum_temperature / temperature).max(0.0);
							fuel_amount += available / burn_rate;
						}
						_ => {}
					}
				}
				Ok(CommandResult::Scalars([oxidation_power, fuel_amount]))
			}
			Command::SetTemperature {
				handle,
				temperature,
			} => {
				if !temperature.is_finite() {
					return Err(WorldError::InvalidTemperature);
				}
				let temperature = temperature.max(MINIMUM_TEMPERATURE_K);
				let updated = self.mutate_mixture(handle, |mixture| {
					if mixture.immutable || mixture.temperature == temperature {
						return false;
					}
					mixture.temperature = temperature;
					true
				})?;
				Ok(CommandResult::Applied {
					updated: u32::from(updated),
				})
			}
			Command::SetVolume { handle, volume } => {
				if !volume.is_finite() || volume < 0.0 {
					return Err(WorldError::InvalidVolume);
				}
				let updated = self.mutate_mixture(handle, |mixture| {
					if mixture.immutable || mixture.volume == volume {
						return false;
					}
					mixture.volume = volume;
					true
				})?;
				Ok(CommandResult::Applied {
					updated: u32::from(updated),
				})
			}
			Command::SetMinimumHeatCapacity { handle, amount } => {
				if !amount.is_finite() || amount < 0.0 {
					return Err(WorldError::InvalidMinimumHeatCapacity);
				}
				let updated = self.mutate_mixture(handle, |mixture| {
					if mixture.immutable || mixture.minimum_heat_capacity == amount {
						return false;
					}
					mixture.minimum_heat_capacity = amount;
					true
				})?;
				Ok(CommandResult::Applied {
					updated: u32::from(updated),
				})
			}
			Command::Clear { handle } => {
				let updated = self.mutate_mixture(handle, |mixture| {
					if mixture.immutable || mixture.gases.iter().all(|amount| *amount == 0.0) {
						return false;
					}
					mixture.gases.fill(0.0);
					true
				})?;
				Ok(CommandResult::Applied {
					updated: u32::from(updated),
				})
			}
			Command::Add { handle, amount } => {
				if !amount.is_finite() {
					return Err(WorldError::InvalidAddend);
				}
				let updated = self.mutate_mixture(handle, |mixture| {
					if mixture.immutable || amount == 0.0 {
						return false;
					}
					let active_len = mixture
						.gases
						.iter()
						.rposition(|moles| *moles != 0.0)
						.map_or(0, |index| index + 1);
					let mut changed = false;
					for moles in &mut mixture.gases[..active_len] {
						let adjusted = (f64::from(*moles) + f64::from(amount))
							.clamp(0.0, f64::from(f32::MAX)) as f32;
						changed |= *moles != adjusted;
						*moles = if adjusted <= GAS_MIN_MOLES {
							0.0
						} else {
							adjusted
						};
					}
					changed
				})?;
				Ok(CommandResult::Applied {
					updated: u32::from(updated),
				})
			}
			Command::Multiply { handle, factor } => {
				if !factor.is_finite() || factor < 0.0 {
					return Err(WorldError::InvalidMultiplier);
				}
				let updated = self.mutate_mixture(handle, |mixture| {
					if mixture.immutable || factor == 1.0 {
						return false;
					}
					let mut changed = false;
					for moles in &mut mixture.gases {
						let adjusted =
							(f64::from(*moles) * f64::from(factor)).min(f64::from(f32::MAX)) as f32;
						let adjusted = if adjusted <= GAS_MIN_MOLES {
							0.0
						} else {
							adjusted
						};
						changed |= *moles != adjusted;
						*moles = adjusted;
					}
					changed
				})?;
				Ok(CommandResult::Applied {
					updated: u32::from(updated),
				})
			}
			Command::CopyFrom { receiver, giver } => {
				if receiver == giver {
					return Err(WorldError::SameMixtureHandles(receiver));
				}
				let giver = self.require_handle(giver)?.clone();
				let updated = self.mutate_mixture(receiver, |mixture| {
					if mixture.immutable
						|| (mixture.gases == giver.gases
							&& mixture.temperature == giver.temperature)
					{
						return false;
					}
					mixture.gases = giver.gases;
					mixture.temperature = giver.temperature;
					true
				})?;
				Ok(CommandResult::Applied {
					updated: u32::from(updated),
				})
			}
			Command::AdjustHeat { handle, heat } => {
				if !heat.is_finite() {
					return Err(WorldError::InvalidHeat);
				}
				let before = self.require_handle(handle)?.clone();
				let capacity = self.heat_capacity(&before)?;
				let temperature =
					((capacity * before.temperature + heat) / capacity).max(MINIMUM_TEMPERATURE_K);
				let updated = self.mutate_mixture(handle, |mixture| {
					if mixture.immutable
						|| !temperature.is_finite()
						|| mixture.temperature == temperature
					{
						return false;
					}
					mixture.temperature = temperature;
					true
				})?;
				Ok(CommandResult::Applied {
					updated: u32::from(updated),
				})
			}
			Command::Compare { left, right } => {
				let left = self.require_handle(left)?;
				let right = self.require_handle(right)?;
				let temperature_differs = (left.temperature - right.temperature).abs() > 4.0
					&& total_moles(left) > MINIMUM_MOLES_DELTA_TO_MOVE;
				let gases_differ = left
					.gases
					.iter()
					.zip(right.gases.iter())
					.any(|(left, right)| (left - right).abs() >= MINIMUM_MOLES_DELTA_TO_MOVE);
				Ok(CommandResult::Boolean(temperature_differs || gases_differ))
			}
			Command::EqualizeWith { receiver, total } => {
				if receiver == total {
					return Err(WorldError::SameMixtureHandles(receiver));
				}
				let total = self.require_handle(total)?.clone();
				if !total.volume.is_finite() || total.volume <= 0.0 {
					return Ok(CommandResult::Applied { updated: 0 });
				}
				let updated = self.mutate_mixture(receiver, |mixture| {
					if mixture.immutable {
						return false;
					}
					let ratio = mixture.volume / total.volume;
					let gases = total.gases.map(|amount| {
						let scaled =
							(f64::from(amount) * f64::from(ratio)).min(f64::from(f32::MAX)) as f32;
						if scaled <= GAS_MIN_MOLES {
							0.0
						} else {
							scaled
						}
					});
					if mixture.gases == gases && mixture.temperature == total.temperature {
						return false;
					}
					mixture.gases = gases;
					mixture.temperature = total.temperature;
					true
				})?;
				Ok(CommandResult::Applied {
					updated: u32::from(updated),
				})
			}
			Command::TemperatureShare {
				first,
				second,
				conduction_coefficient,
			} => {
				if first == second {
					return Err(WorldError::SameMixtureHandles(first));
				}
				if !conduction_coefficient.is_finite() {
					return Err(WorldError::InvalidConductivity);
				}
				let first_before = self.require_handle(first)?.clone();
				let second_before = self.require_handle(second)?.clone();
				let mut first_temperature = first_before.temperature;
				let mut second_temperature = second_before.temperature;
				let delta = first_temperature - second_temperature;
				if delta.abs() > MINIMUM_TEMPERATURE_DELTA_TO_CONSIDER {
					let first_capacity = self.heat_capacity(&first_before)?;
					let second_capacity = self.heat_capacity(&second_before)?;
					if first_capacity > MINIMUM_HEAT_CAPACITY
						&& second_capacity > MINIMUM_HEAT_CAPACITY
					{
						let heat =
							conduction_coefficient
								* delta * harmonic_heat_capacity(first_capacity, second_capacity);
						if !first_before.immutable {
							first_temperature = (first_temperature - heat / first_capacity)
								.max(MINIMUM_TEMPERATURE_K);
						}
						if !second_before.immutable {
							second_temperature = (second_temperature + heat / second_capacity)
								.max(MINIMUM_TEMPERATURE_K);
						}
					}
				}
				let first_changed = first_temperature != first_before.temperature;
				let second_changed = second_temperature != second_before.temperature;
				self.commit_pair_temperatures(
					first,
					second,
					first_temperature,
					second_temperature,
					first_changed,
					second_changed,
				)?;
				Ok(CommandResult::Scalar(second_temperature))
			}
			Command::TemperatureShareNonGas {
				handle,
				conduction_coefficient,
				sharer_temperature,
				sharer_heat_capacity,
			} => {
				if !conduction_coefficient.is_finite() {
					return Err(WorldError::InvalidConductivity);
				}
				if !sharer_temperature.is_finite() {
					return Err(WorldError::InvalidTemperature);
				}
				if !sharer_heat_capacity.is_finite() {
					return Err(WorldError::InvalidHeatCapacity);
				}
				let before = self.require_handle(handle)?.clone();
				let mut mixture_temperature = before.temperature;
				let mut returned_temperature = sharer_temperature;
				let delta = mixture_temperature - sharer_temperature;
				if delta.abs() > MINIMUM_TEMPERATURE_DELTA_TO_CONSIDER {
					let mixture_capacity = self.heat_capacity(&before)?;
					if mixture_capacity > MINIMUM_HEAT_CAPACITY
						&& sharer_heat_capacity > MINIMUM_HEAT_CAPACITY
					{
						let heat = conduction_coefficient
							* delta * harmonic_heat_capacity(
							mixture_capacity,
							sharer_heat_capacity,
						);
						if !before.immutable {
							mixture_temperature = (mixture_temperature - heat / mixture_capacity)
								.max(MINIMUM_TEMPERATURE_K);
						}
						returned_temperature = (sharer_temperature + heat / sharer_heat_capacity)
							.max(MINIMUM_TEMPERATURE_K);
					}
				}
				self.mutate_mixture(handle, |mixture| {
					if mixture.temperature == mixture_temperature {
						return false;
					}
					mixture.temperature = mixture_temperature;
					true
				})?;
				Ok(CommandResult::Scalar(returned_temperature))
			}
			Command::MarkImmutable { handle } => {
				let updated = self.mutate_mixture(handle, |mixture| {
					if mixture.immutable {
						return false;
					}
					mixture.immutable = true;
					true
				})?;
				Ok(CommandResult::Applied {
					updated: u32::from(updated),
				})
			}
			Command::IsImmutable { handle } => Ok(CommandResult::Boolean(
				self.require_handle(handle)?.immutable,
			)),
			Command::Merge { receiver, giver } => {
				self.merge_mixtures(receiver, giver)?;
				Ok(CommandResult::Applied {
					updated: u32::from(!self.require_handle(receiver)?.immutable),
				})
			}
			Command::RemoveRatioInto {
				source,
				destination,
				ratio,
			} => {
				let updated = self.remove_ratio_into(source, destination, ratio)?;
				Ok(CommandResult::Applied { updated })
			}
			Command::RemoveAmountInto {
				source,
				destination,
				amount,
			} => {
				if !amount.is_finite() {
					return Err(WorldError::InvalidMoleAmount);
				}
				let total = total_moles(self.require_handle(source)?);
				let ratio = if total > 0.0 { amount / total } else { 0.0 };
				let updated = self.remove_ratio_into(source, destination, ratio)?;
				Ok(CommandResult::Applied { updated })
			}
			Command::TransferGases {
				source,
				destination,
				ratio,
				gases,
			} => {
				let updated = self.transfer_gases(source, destination, ratio, &gases)?;
				Ok(CommandResult::Applied { updated })
			}
			Command::TransferAmount {
				source,
				destination,
				amount,
			} => {
				if !amount.is_finite() {
					return Err(WorldError::InvalidMoleAmount);
				}
				let total = total_moles(self.require_handle(source)?);
				let ratio = if total > 0.0 { amount / total } else { 0.0 };
				let updated = self.transfer_ratio_to(source, destination, ratio)?;
				Ok(CommandResult::Applied { updated })
			}
			Command::TransferRatio {
				source,
				destination,
				ratio,
			} => {
				let updated = self.transfer_ratio_to(source, destination, ratio)?;
				Ok(CommandResult::Applied { updated })
			}
			Command::TransferByFlags {
				source,
				destination,
				flags,
				amount,
			} => {
				if !amount.is_finite() || amount <= 0.0 {
					return Ok(CommandResult::Boolean(false));
				}
				let gases = self
					.gas_registry
					.as_ref()
					.ok_or(WorldError::GasRegistryMissing)?
					.iter()
					.filter(|gas| gas.flags & flags != 0)
					.map(|gas| gas.id)
					.collect::<Vec<_>>();
				if gases.is_empty() {
					return Ok(CommandResult::Boolean(false));
				}
				let total = total_moles(self.require_handle(source)?);
				if !total.is_finite() || total <= 0.0 {
					return Ok(CommandResult::Boolean(false));
				}
				self.transfer_gases(source, destination, amount / total, &gases)?;
				Ok(CommandResult::Boolean(true))
			}
			Command::ShareRatio {
				first,
				second,
				ratio,
				one_way,
			} => {
				let different = self.share_ratio(first, second, ratio, one_way)?;
				Ok(CommandResult::Boolean(different))
			}
			Command::ResumeReaction { continuation } => {
				let updated =
					self.resume_reaction_with_event_limit(continuation, self.max_events)?;
				Ok(CommandResult::Applied { updated })
			}
		}
	}

	pub fn snapshot(&self, handle: MixtureHandle) -> Result<MixtureSnapshot, WorldError> {
		let mixture = self.require_handle(handle)?;
		Ok(MixtureSnapshot {
			revision: mixture.revision,
			temperature: mixture.temperature,
			volume: mixture.volume,
			minimum_heat_capacity: mixture.minimum_heat_capacity,
			gases: mixture.gases,
			immutable: mixture.immutable,
		})
	}

	pub fn turf_mixture(&self, handle: TurfHandle) -> Result<Option<MixtureHandle>, WorldError> {
		Ok(self.require_turf_handle(handle)?.mixture)
	}

	pub fn turf_heat(&self, handle: TurfHandle) -> Result<Option<TurfHeatState>, WorldError> {
		Ok(self.require_turf_handle(handle)?.heat)
	}

	pub fn drain_events_into(&mut self, maximum: u32, output: &mut Vec<WorldEvent>) -> u32 {
		output.clear();
		let count = self.events.len().min(maximum as usize);
		output.extend(self.events.drain(..count));
		count as u32
	}

	pub fn pending_reaction_continuations(&self) -> u32 {
		self.continuations
			.iter()
			.filter(|slot| slot.continuation.is_some())
			.count()
			.try_into()
			.unwrap_or(u32::MAX)
	}

	pub fn cancel_reaction(&mut self, token: ReactionContinuationToken) -> Result<(), WorldError> {
		self.complete_continuation(token)
	}

	pub fn reactable_reactions_into(
		&self,
		handle: MixtureHandle,
		output: &mut Vec<crate::metadata::ReactionId>,
	) -> Result<u32, WorldError> {
		output.clear();
		let gases = self
			.gas_registry
			.as_ref()
			.ok_or(WorldError::GasRegistryMissing)?;
		let reactions = self
			.reaction_registry
			.as_ref()
			.ok_or(WorldError::ReactionRegistryMissing)?;
		let mixture = self.require_handle(handle)?;
		reactions.reactable_ids_into(mixture.temperature, &mixture.gases, gases, output);
		Ok(u32::try_from(output.len()).unwrap_or(u32::MAX))
	}

	pub fn process_stage_cancellable(
		&mut self,
		stage: WorldStage,
		seconds_per_tick: f64,
		should_cancel: impl FnMut() -> bool,
	) -> Result<StageResult, WorldError> {
		self.process_stage_cancellable_with_event_limit(
			stage,
			seconds_per_tick,
			self.max_events,
			should_cancel,
		)
	}

	pub fn process_stage_cancellable_with_event_limit(
		&mut self,
		stage: WorldStage,
		seconds_per_tick: f64,
		event_limit: u32,
		should_cancel: impl FnMut() -> bool,
	) -> Result<StageResult, WorldError> {
		let previous_limit = self.max_events;
		self.max_events = self.max_events.min(event_limit);
		let result = self.process_stage_cancellable_inner(stage, seconds_per_tick, should_cancel);
		self.max_events = previous_limit;
		result
	}

	fn process_stage_cancellable_inner(
		&mut self,
		stage: WorldStage,
		seconds_per_tick: f64,
		mut should_cancel: impl FnMut() -> bool,
	) -> Result<StageResult, WorldError> {
		if !seconds_per_tick.is_finite() || seconds_per_tick <= 0.0 {
			return Err(WorldError::InvalidSecondsPerTick);
		}
		match stage {
			WorldStage::Equalize => return self.process_equalize(&mut should_cancel),
			WorldStage::ExcitedGroups => return self.process_excited_groups(&mut should_cancel),
			WorldStage::TurfHeat => {
				return self.process_turf_heat(&mut should_cancel, seconds_per_tick as f32)
			}
			WorldStage::ProcessTurfs => {}
			WorldStage::React => return self.process_reactions(&mut should_cancel),
		}
		let has_turf_state = self.turfs.iter().any(|slot| slot.turf.is_some());
		let turf_handles = self
			.turfs
			.iter()
			.filter_map(|slot| slot.turf.as_ref()?.mixture)
			.collect::<Vec<_>>();
		if has_turf_state {
			return self.process_turf_diffusion(turf_handles, &mut should_cancel);
		}
		let work_items = u32::try_from(self.live_count())
			.map_err(|_| WorldError::State("mixture count exceeds u32".into()))?;
		if work_items == 0 {
			return Ok(StageResult { work_items });
		}
		for (slot, mixture_slot) in self.mixtures.iter().enumerate() {
			let Some(mixture) = mixture_slot.mixture.as_ref() else {
				continue;
			};
			if mixture.immutable {
				continue;
			}
			if mixture.revision == u32::MAX {
				return Err(WorldError::RevisionExhausted(MixtureHandle {
					slot: slot as u32,
					generation: mixture_slot
						.generation
						.expect("occupied mixture slot has a generation"),
				}));
			}
		}
		if self.graph.is_none() {
			self.graph = Some(self.build_graph(&self.edges)?);
		}
		self.input.clear();
		for mixture in self
			.mixtures
			.iter()
			.filter_map(|slot| slot.mixture.as_ref())
		{
			self.input.extend_from_slice(&mixture.gases);
		}
		self.output.resize(self.input.len(), 0.0);
		diffusion_step_into_cancellable(
			self.graph.as_ref().expect("graph was built above"),
			MAX_GAS_SLOTS as u32,
			&self.input,
			&mut self.output,
			&mut should_cancel,
		)
		.map_err(|error| match error {
			DiffusionError::Cancelled => WorldError::Cancelled,
			other => WorldError::State(other.to_string()),
		})?;
		if should_cancel() {
			return Err(WorldError::Cancelled);
		}
		let mut offset = 0;
		for mixture in self
			.mixtures
			.iter_mut()
			.filter_map(|slot| slot.mixture.as_mut())
		{
			if !mixture.immutable {
				mixture
					.gases
					.copy_from_slice(&self.output[offset..offset + MAX_GAS_SLOTS]);
				mixture.revision = mixture
					.revision
					.checked_add(1)
					.expect("stage revisions were checked before mutation");
			}
			offset += MAX_GAS_SLOTS;
		}
		Ok(StageResult { work_items })
	}

	fn process_reactions(
		&mut self,
		should_cancel: &mut impl FnMut() -> bool,
	) -> Result<StageResult, WorldError> {
		if should_cancel() {
			return Err(WorldError::Cancelled);
		}
		self.gas_registry
			.as_ref()
			.ok_or(WorldError::GasRegistryMissing)?;
		self.reaction_registry
			.as_ref()
			.ok_or(WorldError::ReactionRegistryMissing)?;
		let active_continuations = self
			.continuations
			.iter()
			.filter_map(|slot| {
				slot.continuation
					.as_ref()
					.map(|continuation| continuation.mixture)
			})
			.collect::<BTreeSet<_>>();
		let targets = self
			.turfs
			.iter()
			.enumerate()
			.filter_map(|(slot, turf_slot)| {
				let turf = turf_slot.turf.as_ref()?;
				Some((
					TurfHandle {
						slot: slot as u32,
						generation: turf_slot.generation?,
					},
					turf.mixture?,
				))
			})
			.collect::<Vec<_>>();
		let mut seen_mixtures = BTreeSet::new();
		let mut staged = BTreeMap::<MixtureHandle, MixtureRecord>::new();
		let mut staged_events = Vec::new();
		let mut pending = None;
		let mut work_items = 0_u32;
		for (turf, mixture) in targets {
			if !seen_mixtures.insert(mixture) || active_continuations.contains(&mixture) {
				continue;
			}
			if should_cancel() {
				return Err(WorldError::Cancelled);
			}
			let sequence = self.evaluate_reaction_sequence(turf, mixture, 0)?;
			work_items = work_items
				.checked_add(sequence.work_items)
				.ok_or_else(|| WorldError::State("reaction work count exceeds u32".into()))?;
			staged_events.extend(sequence.events);
			if sequence.native_updates > 0 {
				staged.insert(mixture, sequence.mixture);
			}
			if let Some(dm_reaction) = sequence.pending {
				pending = Some((turf, mixture, dm_reaction));
				break;
			}
		}
		if should_cancel() {
			return Err(WorldError::Cancelled);
		}
		let requested_events = self
			.events
			.len()
			.saturating_add(staged_events.len())
			.saturating_add(usize::from(pending.is_some()));
		if requested_events > self.max_events as usize {
			return Err(WorldError::EventCapacityExceeded {
				requested: u32::try_from(requested_events).unwrap_or(u32::MAX),
				capacity: self.max_events,
			});
		}
		let continuation_event = if let Some((turf, mixture, pending)) = pending {
			let token = self.allocate_continuation(ReactionContinuation {
				turf,
				mixture,
				next_reaction_index: pending.next_reaction_index,
			})?;
			Some(WorldEvent::RunDmReaction {
				turf,
				mixture,
				reaction: pending.reaction,
				continuation: token,
			})
		} else {
			None
		};
		for (handle, mut record) in staged {
			let current = self.require_handle_mut(handle)?;
			record.revision = current.revision + 1;
			*current = record;
		}
		self.events.extend(staged_events);
		if let Some(event) = continuation_event {
			self.events.push(event);
		}
		Ok(StageResult { work_items })
	}

	fn evaluate_reaction_sequence(
		&self,
		turf: TurfHandle,
		mixture_handle: MixtureHandle,
		start_index: u32,
	) -> Result<ReactionSequence, WorldError> {
		let gases = self
			.gas_registry
			.as_ref()
			.ok_or(WorldError::GasRegistryMissing)?;
		let reactions = self
			.reaction_registry
			.as_ref()
			.ok_or(WorldError::ReactionRegistryMissing)?;
		let mut mixture = self.require_handle(mixture_handle)?.clone();
		let mut events = Vec::new();
		let mut work_items = 0_u32;
		let mut native_updates = 0_u32;
		if mixture.immutable {
			return Ok(ReactionSequence {
				mixture,
				events,
				pending: None,
				work_items,
				native_updates,
			});
		}
		for (index, reaction_id) in reactions
			.priority_order()
			.iter()
			.enumerate()
			.skip(start_index as usize)
		{
			if !reactions.is_reactable(*reaction_id, mixture.temperature, &mixture.gases, gases) {
				continue;
			}
			let reaction = reactions
				.by_id(*reaction_id)
				.expect("reaction priority order contains registered ids");
			match reaction.execution {
				crate::metadata::ReactionExecution::Native(kind) => {
					let Some(result) = crate::reactions::execute_native(
						kind,
						&mut mixture.gases,
						&mut mixture.temperature,
						mixture.volume,
						mixture.minimum_heat_capacity,
						gases,
					)
					.map_err(|error| WorldError::State(error.to_string()))?
					else {
						continue;
					};
					work_items = work_items.checked_add(1).ok_or_else(|| {
						WorldError::State("reaction work count exceeds u32".into())
					})?;
					native_updates = native_updates.checked_add(1).ok_or_else(|| {
						WorldError::State("native reaction count exceeds u32".into())
					})?;
					events.push(WorldEvent::ReactionFinished {
						turf,
						mixture: mixture_handle,
						reaction: *reaction_id,
						kind,
						values: result.values,
					});
				}
				crate::metadata::ReactionExecution::Dm => {
					work_items = work_items.checked_add(1).ok_or_else(|| {
						WorldError::State("reaction work count exceeds u32".into())
					})?;
					if native_updates > 0 && mixture.revision == u32::MAX {
						return Err(WorldError::RevisionExhausted(mixture_handle));
					}
					return Ok(ReactionSequence {
						mixture,
						events,
						pending: Some(PendingDmReaction {
							reaction: *reaction_id,
							next_reaction_index: u32::try_from(index + 1).map_err(|_| {
								WorldError::State("reaction index exceeds u32".into())
							})?,
						}),
						work_items,
						native_updates,
					});
				}
			}
		}
		if native_updates > 0 && mixture.revision == u32::MAX {
			return Err(WorldError::RevisionExhausted(mixture_handle));
		}
		Ok(ReactionSequence {
			mixture,
			events,
			pending: None,
			work_items,
			native_updates,
		})
	}

	pub fn resume_reaction_with_event_limit(
		&mut self,
		token: ReactionContinuationToken,
		event_limit: u32,
	) -> Result<u32, WorldError> {
		let continuation = self.require_continuation(token)?.clone();
		let turf = self.require_turf_handle(continuation.turf)?;
		if turf.mixture != Some(continuation.mixture) {
			return Err(WorldError::TurfMissingMixture(continuation.turf));
		}
		let sequence = self.evaluate_reaction_sequence(
			continuation.turf,
			continuation.mixture,
			continuation.next_reaction_index,
		)?;
		let requested_events = self
			.events
			.len()
			.saturating_add(sequence.events.len())
			.saturating_add(usize::from(sequence.pending.is_some()));
		let event_capacity = self.max_events.min(event_limit);
		if requested_events > event_capacity as usize {
			return Err(WorldError::EventCapacityExceeded {
				requested: u32::try_from(requested_events).unwrap_or(u32::MAX),
				capacity: event_capacity,
			});
		}
		let continuation_event = if let Some(pending) = &sequence.pending {
			let next_token = self.rotate_continuation(
				token,
				ReactionContinuation {
					turf: continuation.turf,
					mixture: continuation.mixture,
					next_reaction_index: pending.next_reaction_index,
				},
			)?;
			Some(WorldEvent::RunDmReaction {
				turf: continuation.turf,
				mixture: continuation.mixture,
				reaction: pending.reaction,
				continuation: next_token,
			})
		} else {
			self.complete_continuation(token)?;
			None
		};
		if sequence.native_updates > 0 {
			let current = self.require_handle_mut(continuation.mixture)?;
			let mut mixture = sequence.mixture;
			mixture.revision = current.revision + 1;
			*current = mixture;
		}
		self.events.extend(sequence.events);
		if let Some(event) = continuation_event {
			self.events.push(event);
		}
		Ok(sequence.native_updates)
	}

	fn allocate_continuation(
		&mut self,
		continuation: ReactionContinuation,
	) -> Result<ReactionContinuationToken, WorldError> {
		if let Some(slot) = self.free_continuations.pop() {
			let entry = &mut self.continuations[slot as usize];
			entry.generation = entry
				.generation
				.checked_add(1)
				.ok_or(WorldError::ReactionContinuationCapacityExceeded)?;
			entry.continuation = Some(continuation);
			return Ok(ReactionContinuationToken {
				slot,
				generation: entry.generation,
			});
		}
		if self.pending_reaction_continuations() >= self.max_continuations {
			return Err(WorldError::ReactionContinuationCapacityExceeded);
		}
		let slot = u32::try_from(self.continuations.len())
			.map_err(|_| WorldError::ReactionContinuationCapacityExceeded)?;
		self.continuations.push(ContinuationSlot {
			generation: 1,
			continuation: Some(continuation),
		});
		Ok(ReactionContinuationToken {
			slot,
			generation: 1,
		})
	}

	fn require_continuation(
		&self,
		token: ReactionContinuationToken,
	) -> Result<&ReactionContinuation, WorldError> {
		let Some(slot) = self.continuations.get(token.slot as usize) else {
			return Err(WorldError::UnknownReactionContinuation(token));
		};
		if slot.generation != token.generation {
			return Err(WorldError::StaleReactionContinuation {
				requested: token,
				current: slot.generation,
			});
		}
		slot.continuation
			.as_ref()
			.ok_or(WorldError::UnknownReactionContinuation(token))
	}

	fn rotate_continuation(
		&mut self,
		token: ReactionContinuationToken,
		continuation: ReactionContinuation,
	) -> Result<ReactionContinuationToken, WorldError> {
		self.require_continuation(token)?;
		let slot = &mut self.continuations[token.slot as usize];
		slot.generation = slot
			.generation
			.checked_add(1)
			.ok_or(WorldError::ReactionContinuationCapacityExceeded)?;
		slot.continuation = Some(continuation);
		Ok(ReactionContinuationToken {
			slot: token.slot,
			generation: slot.generation,
		})
	}

	fn complete_continuation(
		&mut self,
		token: ReactionContinuationToken,
	) -> Result<(), WorldError> {
		self.require_continuation(token)?;
		self.continuations[token.slot as usize].continuation = None;
		self.free_continuations.push(token.slot);
		Ok(())
	}

	fn invalidate_continuations_for_mixture_slot(&mut self, mixture_slot: u32) {
		for (slot, entry) in self.continuations.iter_mut().enumerate() {
			if entry
				.continuation
				.as_ref()
				.is_some_and(|continuation| continuation.mixture.slot == mixture_slot)
			{
				entry.continuation = None;
				self.free_continuations.push(slot as u32);
			}
		}
	}

	fn invalidate_continuations_for_turf_slot(&mut self, turf_slot: u32) {
		for (slot, entry) in self.continuations.iter_mut().enumerate() {
			if entry
				.continuation
				.as_ref()
				.is_some_and(|continuation| continuation.turf.slot == turf_slot)
			{
				entry.continuation = None;
				self.free_continuations.push(slot as u32);
			}
		}
	}

	fn process_excited_groups(
		&mut self,
		should_cancel: &mut impl FnMut() -> bool,
	) -> Result<StageResult, WorldError> {
		if should_cancel() {
			return Err(WorldError::Cancelled);
		}
		let nodes = self
			.turfs
			.iter()
			.enumerate()
			.filter_map(|(slot, turf_slot)| {
				let turf = turf_slot.turf.as_ref()?;
				let mixture = turf.mixture?;
				Some((
					slot as u32,
					(
						TurfHandle {
							slot: slot as u32,
							generation: turf_slot.generation?,
						},
						mixture,
					),
				))
			})
			.collect::<BTreeMap<_, _>>();
		if nodes.is_empty() {
			return Ok(StageResult { work_items: 0 });
		}
		let specific_heats = self
			.gas_registry
			.as_ref()
			.ok_or(WorldError::GasRegistryMissing)?
			.specific_heats();
		let mut heat_values = [0.0; MAX_GAS_SLOTS];
		heat_values[..specific_heats.len()].copy_from_slice(specific_heats);
		let mut adjacency = BTreeMap::<u32, Vec<u32>>::new();
		for slot in nodes.keys().copied() {
			adjacency.entry(slot).or_default();
		}
		for edge in &self.turf_edges {
			if nodes.contains_key(&edge.left) && nodes.contains_key(&edge.right) {
				adjacency.entry(edge.left).or_default().push(edge.right);
				adjacency.entry(edge.right).or_default().push(edge.left);
			}
		}
		for neighbors in adjacency.values_mut() {
			neighbors.sort_unstable();
		}
		let mut found = BTreeSet::new();
		let mut staged = BTreeMap::<MixtureHandle, MixtureRecord>::new();
		let mut work_items = 0_u32;
		for initial_slot in nodes.keys().copied() {
			if found.contains(&initial_slot) || adjacency[&initial_slot].is_empty() {
				continue;
			}
			if should_cancel() {
				return Err(WorldError::Cancelled);
			}
			let initial_mixture = self.require_handle(nodes[&initial_slot].1)?;
			if initial_mixture.immutable {
				continue;
			}
			let initial_pressure = mixture_pressure(initial_mixture);
			let mut minimum_pressure = initial_pressure;
			let mut maximum_pressure = initial_pressure;
			let mut queue = vec![initial_slot];
			let mut queue_index = 0;
			let mut accepted = Vec::new();
			found.insert(initial_slot);
			while queue_index < queue.len() && accepted.len() < 2500 {
				if should_cancel() {
					return Err(WorldError::Cancelled);
				}
				let slot = queue[queue_index];
				queue_index += 1;
				let mixture = self.require_handle(nodes[&slot].1)?;
				if mixture.immutable {
					continue;
				}
				let pressure = mixture_pressure(mixture);
				let next_minimum = minimum_pressure.min(pressure);
				let next_maximum = maximum_pressure.max(pressure);
				if (next_maximum - next_minimum).abs() >= EXCITED_GROUP_PRESSURE_GOAL_KPA {
					continue;
				}
				minimum_pressure = next_minimum;
				maximum_pressure = next_maximum;
				accepted.push(slot);
				for neighbor in adjacency[&slot].iter().copied() {
					if found.insert(neighbor) {
						queue.push(neighbor);
					}
				}
			}
			if accepted.is_empty() {
				continue;
			}
			let mut mixed_gases = [0.0; MAX_GAS_SLOTS];
			let mut total_capacity = 0.0;
			let mut total_energy = 0.0;
			for slot in &accepted {
				let handle = nodes[slot].1;
				let mixture = self.require_handle(handle)?;
				if mixture.revision == u32::MAX {
					return Err(WorldError::RevisionExhausted(handle));
				}
				for (total, amount) in mixed_gases.iter_mut().zip(mixture.gases) {
					*total += amount;
				}
				let capacity = record_heat_capacity(mixture, &heat_values);
				total_capacity += capacity;
				total_energy += capacity * mixture.temperature;
			}
			let divisor = accepted.len() as f32;
			for amount in &mut mixed_gases {
				*amount /= divisor;
			}
			let mixed_temperature = if total_capacity > MINIMUM_HEAT_CAPACITY {
				total_energy / total_capacity
			} else {
				MINIMUM_TEMPERATURE_K
			};
			for slot in accepted {
				let handle = nodes[&slot].1;
				let mut mixture = self.require_handle(handle)?.clone();
				mixture.gases = mixed_gases;
				mixture.temperature = mixed_temperature;
				staged.insert(handle, mixture);
				work_items = work_items
					.checked_add(1)
					.ok_or_else(|| WorldError::State("excited turf count exceeds u32".into()))?;
			}
		}
		if should_cancel() {
			return Err(WorldError::Cancelled);
		}
		for (handle, mut record) in staged {
			let current = self.require_handle_mut(handle)?;
			if current.gases == record.gases && current.temperature == record.temperature {
				continue;
			}
			record.revision = current.revision + 1;
			*current = record;
		}
		Ok(StageResult { work_items })
	}

	fn process_equalize(
		&mut self,
		should_cancel: &mut impl FnMut() -> bool,
	) -> Result<StageResult, WorldError> {
		if should_cancel() {
			return Err(WorldError::Cancelled);
		}
		let turf_handles = self
			.turfs
			.iter()
			.enumerate()
			.filter_map(|(slot, turf_slot)| {
				let turf = turf_slot.turf.as_ref()?;
				turf.mixture?;
				Some(TurfHandle {
					slot: slot as u32,
					generation: turf_slot.generation?,
				})
			})
			.collect::<Vec<_>>();
		if turf_handles.is_empty() {
			return Ok(StageResult { work_items: 0 });
		}
		let mut adjacency = BTreeMap::<u32, Vec<u32>>::new();
		for handle in &turf_handles {
			adjacency.entry(handle.slot).or_default();
		}
		for edge in &self.turf_edges {
			adjacency.entry(edge.left).or_default().push(edge.right);
			adjacency.entry(edge.right).or_default().push(edge.left);
		}
		for neighbors in adjacency.values_mut() {
			neighbors.sort_unstable();
		}
		let specific_heats = self
			.gas_registry
			.as_ref()
			.map(|registry| {
				let mut values = [0.0; MAX_GAS_SLOTS];
				values[..registry.specific_heats().len()]
					.copy_from_slice(registry.specific_heats());
				values
			})
			.unwrap_or([0.0; MAX_GAS_SLOTS]);
		let mut visited = BTreeSet::new();
		let mut staged_records = BTreeMap::<MixtureHandle, MixtureRecord>::new();
		let mut staged_events = Vec::new();
		let mut work_items = 0_u32;
		for start in turf_handles {
			if !visited.insert(start.slot) {
				continue;
			}
			if should_cancel() {
				return Err(WorldError::Cancelled);
			}
			let mut component = vec![start.slot];
			let mut parents = BTreeMap::<u32, u32>::new();
			let mut queue_index = 0;
			while queue_index < component.len() {
				if should_cancel() {
					return Err(WorldError::Cancelled);
				}
				let current = component[queue_index];
				queue_index += 1;
				for neighbor in adjacency.get(&current).into_iter().flatten().copied() {
					if visited.contains(&neighbor)
						|| component.len() >= self.equalize_hard_turf_limit as usize
					{
						continue;
					}
					visited.insert(neighbor);
					parents.insert(neighbor, current);
					component.push(neighbor);
				}
			}
			if component.len() < 2 {
				continue;
			}
			let mut component_moles = 0.0;
			let mut minimum_moles = f32::INFINITY;
			let mut maximum_moles = 0.0_f32;
			let mut mixtures_by_turf = BTreeMap::new();
			let mut immutable_turfs = BTreeSet::new();
			let mut mutable_mixtures = BTreeSet::new();
			for turf_slot in &component {
				if should_cancel() {
					return Err(WorldError::Cancelled);
				}
				let turf_handle = self.current_turf_handle(*turf_slot)?;
				let mixture_handle = self
					.require_turf_handle(turf_handle)?
					.mixture
					.ok_or(WorldError::TurfMissingMixture(turf_handle))?;
				let mixture = self.require_handle(mixture_handle)?;
				if mixture.immutable {
					immutable_turfs.insert(*turf_slot);
					continue;
				}
				if !mutable_mixtures.insert(mixture_handle) {
					return Err(WorldError::DuplicateMutableTurfMixture(mixture_handle));
				}
				if mixture.revision == u32::MAX {
					return Err(WorldError::RevisionExhausted(mixture_handle));
				}
				let moles = total_moles(mixture);
				component_moles += moles;
				minimum_moles = minimum_moles.min(moles);
				maximum_moles = maximum_moles.max(moles);
				mixtures_by_turf.insert(*turf_slot, mixture_handle);
				staged_records
					.entry(mixture_handle)
					.or_insert_with(|| mixture.clone());
			}
			if !immutable_turfs.is_empty() {
				if maximum_moles >= 10.0 && !mixtures_by_turf.is_empty() {
					self.stage_decompression_component(
						&component,
						&adjacency,
						&immutable_turfs,
						&mixtures_by_turf,
						component_moles,
						&mut staged_records,
						&mut staged_events,
					)?;
				}
				work_items = work_items
					.checked_add(u32::try_from(component.len()).map_err(|_| {
						WorldError::State("equalized turf count exceeds u32".into())
					})?)
					.ok_or_else(|| WorldError::State("equalized turf count exceeds u32".into()))?;
				continue;
			}
			if maximum_moles < 10.0 || maximum_moles - minimum_moles < MINIMUM_MOLES_DELTA_TO_MOVE {
				continue;
			}
			let average_moles = component_moles / component.len() as f32;
			let mut subtree_balance = component
				.iter()
				.map(|slot| {
					let handle = mixtures_by_turf[slot];
					(*slot, total_moles(&staged_records[&handle]) - average_moles)
				})
				.collect::<BTreeMap<_, _>>();
			let mut flows = Vec::<(u32, u32, f32)>::new();
			for child in component.iter().copied().skip(1).rev() {
				let parent = parents[&child];
				let balance = subtree_balance[&child];
				flows.push((child, parent, balance));
				*subtree_balance
					.get_mut(&parent)
					.expect("component parent has a balance") += balance;
			}
			for &(child, parent, balance) in flows.iter().filter(|(_, _, balance)| *balance > 0.0) {
				self.stage_equalization_transfer(
					child,
					parent,
					balance,
					&mixtures_by_turf,
					&specific_heats,
					&mut staged_records,
					&mut staged_events,
				)?;
			}
			for &(child, parent, balance) in
				flows.iter().rev().filter(|(_, _, balance)| *balance < 0.0)
			{
				self.stage_equalization_transfer(
					parent,
					child,
					-balance,
					&mixtures_by_turf,
					&specific_heats,
					&mut staged_records,
					&mut staged_events,
				)?;
			}
			work_items = work_items
				.checked_add(component.len() as u32)
				.ok_or_else(|| WorldError::State("equalized turf count exceeds u32".into()))?;
		}
		if should_cancel() {
			return Err(WorldError::Cancelled);
		}
		let requested_events = self.events.len().saturating_add(staged_events.len());
		if requested_events > self.max_events as usize {
			return Err(WorldError::EventCapacityExceeded {
				requested: u32::try_from(requested_events).unwrap_or(u32::MAX),
				capacity: self.max_events,
			});
		}
		for (handle, mut record) in staged_records {
			let current = self.require_handle_mut(handle)?;
			if current.gases == record.gases && current.temperature == record.temperature {
				continue;
			}
			record.revision = current.revision + 1;
			*current = record;
		}
		self.events.extend(staged_events);
		Ok(StageResult { work_items })
	}

	#[allow(clippy::too_many_arguments)]
	fn stage_decompression_component(
		&self,
		component: &[u32],
		adjacency: &BTreeMap<u32, Vec<u32>>,
		immutable_turfs: &BTreeSet<u32>,
		mixtures_by_turf: &BTreeMap<u32, MixtureHandle>,
		component_moles: f32,
		records: &mut BTreeMap<MixtureHandle, MixtureRecord>,
		events: &mut Vec<WorldEvent>,
	) -> Result<(), WorldError> {
		let component_slots = component.iter().copied().collect::<BTreeSet<_>>();
		let mut queue = immutable_turfs.iter().copied().collect::<Vec<_>>();
		let mut reached = immutable_turfs.clone();
		let mut parents = BTreeMap::<u32, u32>::new();
		let mut queue_index = 0;
		while queue_index < queue.len() {
			let current = queue[queue_index];
			queue_index += 1;
			for neighbor in adjacency.get(&current).into_iter().flatten().copied() {
				if component_slots.contains(&neighbor) && reached.insert(neighbor) {
					parents.insert(neighbor, current);
					queue.push(neighbor);
				}
			}
		}

		let mutable_count = mixtures_by_turf.len();
		if mutable_count == 0 {
			return Ok(());
		}
		let frontage = immutable_turfs.len().clamp(1, 4) as f32;
		let removal_per_turf = component_moles / mutable_count as f32 * frontage / 4.0;
		let mut local_losses = BTreeMap::<u32, f32>::new();
		for (&turf_slot, &mixture_handle) in mixtures_by_turf {
			let mut mixture = records[&mixture_handle].clone();
			let before = total_moles(&mixture);
			let ratio = if before > 0.0 {
				(removal_per_turf / before).clamp(0.0, 1.0)
			} else {
				0.0
			};
			for amount in &mut mixture.gases {
				*amount -= quantize(*amount * ratio);
			}
			let lost = before - total_moles(&mixture);
			records.insert(mixture_handle, mixture);
			local_losses.insert(turf_slot, lost);
		}
		for edge in self.turf_firelock_edges.iter().filter(|edge| {
			component_slots.contains(&edge.left) && component_slots.contains(&edge.right)
		}) {
			let (source_slot, target_slot) =
				if immutable_turfs.contains(&edge.left) && !immutable_turfs.contains(&edge.right) {
					(edge.right, edge.left)
				} else {
					(edge.left, edge.right)
				};
			events.push(WorldEvent::FirelockConsideration {
				source: self.current_turf_handle(source_slot)?,
				target: self.current_turf_handle(target_slot)?,
			});
		}

		let mut accumulated_losses = local_losses.clone();
		for &source_slot in queue.iter().rev() {
			let Some(&source_handle) = mixtures_by_turf.get(&source_slot) else {
				continue;
			};
			let Some(&target_slot) = parents.get(&source_slot) else {
				continue;
			};
			let pressure_moles = accumulated_losses[&source_slot];
			if pressure_moles > 0.0 {
				events.push(WorldEvent::PressureDifference {
					source: self.current_turf_handle(source_slot)?,
					target: self.current_turf_handle(target_slot)?,
					moles: pressure_moles,
				});
			}
			if immutable_turfs.contains(&target_slot) {
				let moles_lost = local_losses[&source_slot];
				if moles_lost > 0.0 {
					events.push(WorldEvent::DecompressionFloorRip {
						turf: self.current_turf_handle(source_slot)?,
						moles_lost,
					});
				}
			} else if mixtures_by_turf.contains_key(&target_slot) {
				*accumulated_losses.entry(target_slot).or_default() += pressure_moles;
			}
			let _ = source_handle;
		}
		Ok(())
	}

	#[allow(clippy::too_many_arguments)]
	fn stage_equalization_transfer(
		&self,
		source_slot: u32,
		target_slot: u32,
		amount: f32,
		mixtures_by_turf: &BTreeMap<u32, MixtureHandle>,
		specific_heats: &[f32; MAX_GAS_SLOTS],
		records: &mut BTreeMap<MixtureHandle, MixtureRecord>,
		events: &mut Vec<WorldEvent>,
	) -> Result<(), WorldError> {
		let source_handle = mixtures_by_turf[&source_slot];
		let target_handle = mixtures_by_turf[&target_slot];
		if source_handle == target_handle {
			return Err(WorldError::DuplicateMutableTurfMixture(source_handle));
		}
		let mut source = records[&source_handle].clone();
		let mut target = records[&target_handle].clone();
		let moved = transfer_moles(&mut source, &mut target, amount, specific_heats)?;
		records.insert(source_handle, source);
		records.insert(target_handle, target);
		if moved > 0.0 {
			events.push(WorldEvent::PressureDifference {
				source: self.current_turf_handle(source_slot)?,
				target: self.current_turf_handle(target_slot)?,
				moles: moved,
			});
		}
		Ok(())
	}

	fn process_turf_heat(
		&mut self,
		should_cancel: &mut impl FnMut() -> bool,
		seconds_per_tick: f32,
	) -> Result<StageResult, WorldError> {
		if should_cancel() {
			return Err(WorldError::Cancelled);
		}
		let nodes = self
			.turfs
			.iter()
			.enumerate()
			.filter_map(|(slot, turf_slot)| {
				let turf = turf_slot.turf.as_ref()?;
				let handle = TurfHandle {
					slot: slot as u32,
					generation: turf_slot.generation?,
				};
				Some((handle, turf.heat?, turf.mixture))
			})
			.collect::<Vec<_>>();
		let work_items = u32::try_from(nodes.len())
			.map_err(|_| WorldError::State("turf heat count exceeds u32".into()))?;
		if nodes.is_empty() {
			return Ok(StageResult { work_items });
		}
		let dense_by_slot = nodes
			.iter()
			.enumerate()
			.map(|(index, (handle, _, _))| (handle.slot, index as u32))
			.collect::<BTreeMap<_, _>>();
		let edges = self
			.heat_edges
			.iter()
			.map(|edge| {
				let first = dense_by_slot
					.get(&edge.left)
					.copied()
					.ok_or(WorldError::State("heat edge has no first node".into()))?;
				let second = dense_by_slot
					.get(&edge.right)
					.copied()
					.ok_or(WorldError::State("heat edge has no second node".into()))?;
				Ok((first, second))
			})
			.collect::<Result<Vec<_>, WorldError>>()?;
		let mut temperatures = nodes
			.iter()
			.map(|(_, state, _)| state.temperature)
			.collect::<Vec<_>>();
		let conductivities = nodes
			.iter()
			.map(|(_, state, _)| state.thermal_conductivity)
			.collect::<Vec<_>>();
		let heat_capacities = nodes
			.iter()
			.map(|(_, state, _)| state.heat_capacity)
			.collect::<Vec<_>>();
		let specific_heats = self
			.gas_registry
			.as_ref()
			.map(GasMetadataRegistry::specific_heats);
		let elapsed_heat_scale =
			seconds_per_tick / crate::numerics::conduction::BASE_HEAT_STEP_SECONDS;
		let mut staged_mixtures = BTreeMap::<MixtureHandle, MixtureRecord>::new();
		let mut linked_mixtures = BTreeSet::new();
		let mut staged_events = Vec::new();
		for (index, (turf, state, mixture_handle)) in nodes.iter().copied().enumerate() {
			if should_cancel() {
				return Err(WorldError::Cancelled);
			}
			let temperature = &mut temperatures[index];
			if state.adjacent_to_space && *temperature > 273.15 {
				if self.realistic_space_radiation {
					let emitted = STEFAN_BOLTZMANN_CONSTANT
						* f64::from(seconds_per_tick)
						* f64::from(*temperature).powi(4);
					let received = RADIATION_FROM_SPACE * f64::from(seconds_per_tick);
					*temperature = (f64::from(*temperature)
						- (emitted - received) / f64::from(state.heat_capacity))
					.max(f64::from(MINIMUM_TEMPERATURE_K)) as f32;
				} else if *temperature > 293.15 {
					let heat = heat_exchange_energy(
						state.thermal_conductivity
							* elapsed_heat_scale * (*temperature - MINIMUM_TEMPERATURE_K),
						7000.0,
						state.heat_capacity,
					);
					*temperature =
						(*temperature - heat / state.heat_capacity).max(MINIMUM_TEMPERATURE_K);
				}
			}

			if let Some(mixture_handle) = mixture_handle {
				if !linked_mixtures.insert(mixture_handle) {
					return Err(WorldError::DuplicateMutableTurfMixture(mixture_handle));
				}
				let specific_heats = specific_heats.ok_or(WorldError::GasRegistryMissing)?;
				let mut mixture = self.require_handle(mixture_handle)?.clone();
				if !mixture.immutable {
					let gas_capacity = record_heat_capacity(&mixture, specific_heats);
					let temperature_delta = mixture.temperature - *temperature;
					if (*temperature > MINIMUM_TEMPERATURE_FOR_SUPERCONDUCTION_K
						|| mixture.temperature >= MINIMUM_TEMPERATURE_FOR_SUPERCONDUCTION_K)
						&& temperature_delta.abs() > MINIMUM_TEMPERATURE_DELTA_TO_CONSIDER
						&& gas_capacity > MINIMUM_HEAT_CAPACITY
					{
						if mixture.revision == u32::MAX {
							return Err(WorldError::RevisionExhausted(mixture_handle));
						}
						let heat = state.thermal_conductivity
							* OPEN_HEAT_TRANSFER_COEFFICIENT
							* elapsed_heat_scale * temperature_delta
							* harmonic_heat_capacity(gas_capacity, state.heat_capacity);
						mixture.temperature =
							(mixture.temperature - heat / gas_capacity).max(MINIMUM_TEMPERATURE_K);
						*temperature =
							(*temperature + heat / state.heat_capacity).max(MINIMUM_TEMPERATURE_K);
						staged_mixtures.insert(mixture_handle, mixture);
					}
				}
			}

			if *temperature > MINIMUM_TEMPERATURE_START_SUPERCONDUCTION_K
				&& *temperature > state.heat_capacity
			{
				staged_events.push(WorldEvent::TurfDestructionRequest { turf });
			}
		}
		crate::numerics::conduction::conduction_step_cancellable(
			&mut temperatures,
			&conductivities,
			&heat_capacities,
			&edges,
			seconds_per_tick,
			&mut *should_cancel,
		)
		.map_err(|error| match error {
			crate::numerics::conduction::ConductionError::Cancelled => WorldError::Cancelled,
			other => WorldError::State(other.to_string()),
		})?;
		if should_cancel() {
			return Err(WorldError::Cancelled);
		}
		let requested_events = self.events.len().saturating_add(staged_events.len());
		if requested_events > self.max_events as usize {
			return Err(WorldError::EventCapacityExceeded {
				requested: u32::try_from(requested_events).unwrap_or(u32::MAX),
				capacity: self.max_events,
			});
		}
		for (handle, mut mixture) in staged_mixtures {
			let current = self.require_handle_mut(handle)?;
			mixture.revision = current.revision + 1;
			*current = mixture;
		}
		for ((handle, _, _), temperature) in nodes.into_iter().zip(temperatures) {
			self.require_turf_handle_mut(handle)?
				.heat
				.as_mut()
				.expect("turf heat state was validated")
				.temperature = temperature;
		}
		self.events.extend(staged_events);
		Ok(StageResult { work_items })
	}

	fn process_turf_diffusion(
		&mut self,
		handles: Vec<MixtureHandle>,
		should_cancel: &mut impl FnMut() -> bool,
	) -> Result<StageResult, WorldError> {
		let work_items = u32::try_from(handles.len())
			.map_err(|_| WorldError::State("turf count exceeds u32".into()))?;
		let mut mutable_handles = BTreeSet::new();
		for handle in handles.iter().copied() {
			let mixture = self.require_handle(handle)?;
			if mixture.immutable {
				continue;
			}
			if !mutable_handles.insert(handle) {
				return Err(WorldError::DuplicateMutableTurfMixture(handle));
			}
			if mixture.revision == u32::MAX {
				return Err(WorldError::RevisionExhausted(handle));
			}
		}
		if self.turf_graph.is_none() {
			self.turf_graph = Some(self.build_turf_graph(&self.turf_edges)?);
		}
		self.input.clear();
		for handle in handles.iter().copied() {
			let gases = self.require_handle(handle)?.gases;
			self.input.extend_from_slice(&gases);
		}
		self.output.resize(self.input.len(), 0.0);
		diffusion_step_into_cancellable(
			self.turf_graph
				.as_ref()
				.expect("turf graph was built above"),
			MAX_GAS_SLOTS as u32,
			&self.input,
			&mut self.output,
			&mut *should_cancel,
		)
		.map_err(|error| match error {
			DiffusionError::Cancelled => WorldError::Cancelled,
			other => WorldError::State(other.to_string()),
		})?;
		if should_cancel() {
			return Err(WorldError::Cancelled);
		}
		for (index, handle) in handles.into_iter().enumerate() {
			let offset = index * MAX_GAS_SLOTS;
			let gases: [f32; MAX_GAS_SLOTS] = self.output[offset..offset + MAX_GAS_SLOTS]
				.try_into()
				.expect("diffusion output uses the fixed gas layout");
			let mixture = self.require_handle_mut(handle)?;
			if mixture.immutable {
				continue;
			}
			mixture.gases = gases;
			mixture.revision += 1;
		}
		Ok(StageResult { work_items })
	}

	pub fn edge_count(&self) -> usize {
		self.edges.len()
	}

	pub fn turf_edge_count(&self) -> usize {
		self.turf_edges.len()
	}

	pub fn slot_count(&self) -> usize {
		self.mixtures.len()
	}

	fn projected_slot(&self, slot: u32) -> ProjectedSlot {
		self.mixtures.get(slot as usize).map_or(
			ProjectedSlot {
				generation: None,
				occupied: false,
			},
			|slot| ProjectedSlot {
				generation: slot.generation,
				occupied: slot.mixture.is_some(),
			},
		)
	}

	fn projected_turf_slot(&self, slot: u32) -> ProjectedSlot {
		self.turfs.get(slot as usize).map_or(
			ProjectedSlot {
				generation: None,
				occupied: false,
			},
			|slot| ProjectedSlot {
				generation: slot.generation,
				occupied: slot.turf.is_some(),
			},
		)
	}

	fn current_turf_handle(&self, slot: u32) -> Result<TurfHandle, WorldError> {
		let Some(turf_slot) = self.turfs.get(slot as usize) else {
			return Err(WorldError::UnknownTurfHandle(TurfHandle {
				slot,
				generation: 0,
			}));
		};
		let Some(generation) = turf_slot.generation else {
			return Err(WorldError::UnknownTurfHandle(TurfHandle {
				slot,
				generation: 0,
			}));
		};
		let handle = TurfHandle { slot, generation };
		self.require_turf_handle(handle)?;
		Ok(handle)
	}

	fn validate_slot_capacity(&self, slot: u32) -> Result<(), WorldError> {
		let mixture_slots = u64::from(slot) + 1;
		self.validate_world_capacity(mixture_slots, self.turfs.len() as u64)
	}

	fn validate_turf_slot_capacity(&self, slot: u32) -> Result<(), WorldError> {
		let turf_slots = u64::from(slot) + 1;
		self.validate_world_capacity(self.mixtures.len() as u64, turf_slots)
	}

	fn validate_world_capacity(
		&self,
		mixture_slots: u64,
		turf_slots: u64,
	) -> Result<(), WorldError> {
		let mixture_bytes = mixture_slots
			.checked_mul(std::mem::size_of::<MixtureSlot>() as u64)
			.ok_or(WorldError::StateCapacityExceeded)?;
		let turf_bytes = turf_slots
			.checked_mul(std::mem::size_of::<TurfSlot>() as u64)
			.ok_or(WorldError::StateCapacityExceeded)?;
		let world_bytes = mixture_bytes
			.checked_add(turf_bytes)
			.ok_or(WorldError::StateCapacityExceeded)?;
		if world_bytes > self.max_world_bytes {
			return Err(WorldError::StateCapacityExceeded);
		}
		Ok(())
	}

	fn require_handle(&self, handle: MixtureHandle) -> Result<&MixtureRecord, WorldError> {
		let Some(slot) = self.mixtures.get(handle.slot as usize) else {
			return Err(WorldError::UnknownHandle(handle));
		};
		let Some(generation) = slot.generation else {
			return Err(WorldError::UnknownHandle(handle));
		};
		if generation != handle.generation {
			return Err(WorldError::StaleHandle {
				requested: handle,
				current: generation,
			});
		}
		slot.mixture
			.as_ref()
			.ok_or(WorldError::UnknownHandle(handle))
	}

	fn require_handle_mut(
		&mut self,
		handle: MixtureHandle,
	) -> Result<&mut MixtureRecord, WorldError> {
		let Some(slot) = self.mixtures.get_mut(handle.slot as usize) else {
			return Err(WorldError::UnknownHandle(handle));
		};
		let Some(generation) = slot.generation else {
			return Err(WorldError::UnknownHandle(handle));
		};
		if generation != handle.generation {
			return Err(WorldError::StaleHandle {
				requested: handle,
				current: generation,
			});
		}
		slot.mixture
			.as_mut()
			.ok_or(WorldError::UnknownHandle(handle))
	}

	fn mutate_mixture(
		&mut self,
		handle: MixtureHandle,
		mutation: impl FnOnce(&mut MixtureRecord) -> bool,
	) -> Result<bool, WorldError> {
		let mixture = self.require_handle_mut(handle)?;
		if mixture.revision == u32::MAX {
			return Err(WorldError::RevisionExhausted(handle));
		}
		let changed = mutation(mixture);
		if changed {
			mixture.revision += 1;
		}
		Ok(changed)
	}

	fn gas_index(&self, gas: crate::metadata::GasId) -> Result<usize, WorldError> {
		let registry = self
			.gas_registry
			.as_ref()
			.ok_or(WorldError::GasRegistryMissing)?;
		if registry.by_id(gas).is_none() {
			return Err(WorldError::InvalidGasId(gas));
		}
		Ok(usize::from(gas.0))
	}

	fn heat_capacity(&self, mixture: &MixtureRecord) -> Result<f32, WorldError> {
		let registry = self
			.gas_registry
			.as_ref()
			.ok_or(WorldError::GasRegistryMissing)?;
		Ok(mixture
			.gases
			.iter()
			.zip(registry.specific_heats())
			.fold(0.0, |capacity, (amount, specific_heat)| {
				specific_heat.mul_add(*amount, capacity)
			})
			.max(mixture.minimum_heat_capacity))
	}

	fn merge_mixtures(
		&mut self,
		receiver: MixtureHandle,
		giver: MixtureHandle,
	) -> Result<(), WorldError> {
		if receiver == giver {
			return Err(WorldError::SameMixtureHandles(receiver));
		}
		let receiver_before = self.require_handle(receiver)?.clone();
		let giver = self.require_handle(giver)?.clone();
		if receiver_before.immutable {
			return Ok(());
		}
		let receiver_capacity = self.heat_capacity(&receiver_before)?;
		let giver_capacity = self.heat_capacity(&giver)?;
		self.mutate_mixture(receiver, |mixture| {
			for (amount, added) in mixture.gases.iter_mut().zip(giver.gases) {
				*amount = (f64::from(*amount) + f64::from(added)).min(f64::from(f32::MAX)) as f32;
			}
			let combined_capacity = receiver_capacity + giver_capacity;
			if combined_capacity > MINIMUM_HEAT_CAPACITY {
				mixture.temperature = (receiver_capacity * receiver_before.temperature
					+ giver_capacity * giver.temperature)
					/ combined_capacity;
			}
			true
		})?;
		Ok(())
	}

	fn remove_ratio_into(
		&mut self,
		source: MixtureHandle,
		destination: MixtureHandle,
		ratio: f32,
	) -> Result<u32, WorldError> {
		if source == destination {
			return Err(WorldError::SameMixtureHandles(source));
		}
		if !ratio.is_finite() {
			return Err(WorldError::InvalidRatio);
		}
		let ratio = ratio.clamp(0.0, 1.0);
		let source_before = self.require_handle(source)?.clone();
		let destination_before = self.require_handle(destination)?.clone();
		if ratio == 0.0 || destination_before.immutable {
			return Ok(0);
		}
		if source_before.revision == u32::MAX {
			return Err(WorldError::RevisionExhausted(source));
		}
		if destination_before.revision == u32::MAX {
			return Err(WorldError::RevisionExhausted(destination));
		}
		let removed = source_before.gases.map(|amount| quantize(amount * ratio));
		let (source_record, destination_record) =
			self.require_two_handles_mut(source, destination)?;
		destination_record.gases = removed;
		destination_record.temperature = source_before.temperature;
		destination_record.revision += 1;
		if !source_before.immutable {
			for (amount, removed) in source_record.gases.iter_mut().zip(removed) {
				*amount -= removed;
			}
			source_record.revision += 1;
			Ok(2)
		} else {
			Ok(1)
		}
	}

	fn transfer_gases(
		&mut self,
		source: MixtureHandle,
		destination: MixtureHandle,
		ratio: f32,
		gases: &[crate::metadata::GasId],
	) -> Result<u32, WorldError> {
		if source == destination {
			return Err(WorldError::SameMixtureHandles(source));
		}
		if !ratio.is_finite() {
			return Err(WorldError::InvalidRatio);
		}
		let gas_indices = gases
			.iter()
			.map(|gas| self.gas_index(*gas))
			.collect::<Result<Vec<_>, _>>()?;
		let ratio = ratio.clamp(0.0, 1.0);
		let source_before = self.require_handle(source)?.clone();
		let destination_before = self.require_handle(destination)?.clone();
		if ratio == 0.0 || source_before.immutable || destination_before.immutable {
			return Ok(0);
		}
		if source_before.revision == u32::MAX {
			return Err(WorldError::RevisionExhausted(source));
		}
		if destination_before.revision == u32::MAX {
			return Err(WorldError::RevisionExhausted(destination));
		}
		let registered_specific_heats = self
			.gas_registry
			.as_ref()
			.ok_or(WorldError::GasRegistryMissing)?
			.specific_heats();
		let mut specific_heats = [0.0; MAX_GAS_SLOTS];
		specific_heats[..registered_specific_heats.len()]
			.copy_from_slice(registered_specific_heats);
		let initial_energy =
			self.heat_capacity(&destination_before)? * destination_before.temperature;
		let mut transfers = Vec::with_capacity(gas_indices.len());
		let mut heat_transfer = 0.0;
		for gas_index in gas_indices {
			let amount = source_before.gases[gas_index] * ratio;
			let adjusted = f64::from(destination_before.gases[gas_index]) + f64::from(amount);
			if adjusted > f64::from(f32::MAX) {
				return Err(WorldError::MoleOverflow(crate::metadata::GasId(
					gas_index as u16,
				)));
			}
			heat_transfer += amount * source_before.temperature * specific_heats[gas_index];
			transfers.push((gas_index, amount));
		}
		let (source_record, destination_record) =
			self.require_two_handles_mut(source, destination)?;
		for (gas_index, amount) in transfers {
			source_record.gases[gas_index] -= amount;
			destination_record.gases[gas_index] += amount;
		}
		let destination_capacity = destination_record
			.gases
			.iter()
			.zip(specific_heats)
			.fold(0.0, |capacity, (amount, specific_heat)| {
				specific_heat.mul_add(*amount, capacity)
			});
		if destination_capacity > MINIMUM_HEAT_CAPACITY {
			destination_record.temperature =
				(initial_energy + heat_transfer) / destination_capacity;
		}
		source_record.revision += 1;
		destination_record.revision += 1;
		Ok(2)
	}

	fn transfer_ratio_to(
		&mut self,
		source: MixtureHandle,
		destination: MixtureHandle,
		ratio: f32,
	) -> Result<u32, WorldError> {
		if source == destination {
			return Err(WorldError::SameMixtureHandles(source));
		}
		if !ratio.is_finite() {
			return Err(WorldError::InvalidRatio);
		}
		let ratio = ratio.clamp(0.0, 1.0);
		let source_before = self.require_handle(source)?.clone();
		let destination_before = self.require_handle(destination)?.clone();
		if ratio == 0.0 {
			return Ok(0);
		}
		let removed = source_before.gases.map(|amount| quantize(amount * ratio));
		let mut source_after = source_before.clone();
		if !source_after.immutable {
			for (amount, removed) in source_after.gases.iter_mut().zip(removed) {
				*amount -= removed;
			}
		}
		let mut destination_after = destination_before.clone();
		if !destination_after.immutable {
			let registry = self
				.gas_registry
				.as_ref()
				.ok_or(WorldError::GasRegistryMissing)?;
			let destination_capacity = self.heat_capacity(&destination_before)?;
			let removed_capacity = removed
				.iter()
				.zip(registry.specific_heats())
				.fold(0.0, |capacity, (amount, specific_heat)| {
					specific_heat.mul_add(*amount, capacity)
				});
			for (amount, added) in destination_after.gases.iter_mut().zip(removed) {
				*amount = (f64::from(*amount) + f64::from(added)).min(f64::from(f32::MAX)) as f32;
			}
			let combined_capacity = destination_capacity + removed_capacity;
			if combined_capacity > MINIMUM_HEAT_CAPACITY {
				destination_after.temperature = (destination_capacity
					* destination_before.temperature
					+ removed_capacity * source_before.temperature)
					/ combined_capacity;
			}
		}
		let source_changed = source_after.gases != source_before.gases;
		let destination_changed = destination_after.gases != destination_before.gases
			|| destination_after.temperature != destination_before.temperature;
		if source_changed && source_before.revision == u32::MAX {
			return Err(WorldError::RevisionExhausted(source));
		}
		if destination_changed && destination_before.revision == u32::MAX {
			return Err(WorldError::RevisionExhausted(destination));
		}
		if !source_changed && !destination_changed {
			return Ok(0);
		}
		let (source_record, destination_record) =
			self.require_two_handles_mut(source, destination)?;
		if source_changed {
			source_after.revision += 1;
			*source_record = source_after;
		}
		if destination_changed {
			destination_after.revision += 1;
			*destination_record = destination_after;
		}
		Ok(u32::from(source_changed) + u32::from(destination_changed))
	}

	fn share_ratio(
		&mut self,
		first: MixtureHandle,
		second: MixtureHandle,
		ratio: f32,
		one_way: bool,
	) -> Result<bool, WorldError> {
		if first == second {
			return Err(WorldError::SameMixtureHandles(first));
		}
		if !ratio.is_finite() {
			return Err(WorldError::InvalidRatio);
		}
		let ratio = ratio.clamp(0.0, 1.0);
		let specific_heats = self
			.gas_registry
			.as_ref()
			.ok_or(WorldError::GasRegistryMissing)?
			.specific_heats();
		let first_before = self.require_handle(first)?.clone();
		let second_before = self.require_handle(second)?.clone();
		let mut first_after = first_before.clone();
		let mut second_after = second_before.clone();
		let mut inbetween = MixtureRecord::new();
		if one_way {
			copy_record(&mut inbetween, &second_after);
			scale_record(&mut inbetween, ratio);
			let removed = remove_ratio_record(&mut first_after, ratio);
			merge_record(&mut inbetween, &removed, specific_heats);
			scale_record(&mut inbetween, 0.5);
			merge_record(&mut first_after, &inbetween, specific_heats);
		} else {
			remove_ratio_into_record(&mut first_after, &mut inbetween, ratio);
			let removed = remove_ratio_record(&mut second_after, ratio);
			merge_record(&mut inbetween, &removed, specific_heats);
			scale_record(&mut inbetween, 0.5);
			merge_record(&mut first_after, &inbetween, specific_heats);
			merge_record(&mut second_after, &inbetween, specific_heats);
		}
		let first_changed = mixture_thermodynamics_differ(&first_before, &first_after);
		let second_changed = mixture_thermodynamics_differ(&second_before, &second_after);
		if first_changed && first_before.revision == u32::MAX {
			return Err(WorldError::RevisionExhausted(first));
		}
		if second_changed && second_before.revision == u32::MAX {
			return Err(WorldError::RevisionExhausted(second));
		}
		if first_changed || second_changed {
			let (first_record, second_record) = self.require_two_handles_mut(first, second)?;
			if first_changed {
				first_after.revision += 1;
				*first_record = first_after.clone();
			}
			if second_changed {
				second_after.revision += 1;
				*second_record = second_after.clone();
			}
		}
		Ok(mixtures_require_processing(&first_after, &second_after))
	}

	fn commit_pair_temperatures(
		&mut self,
		first: MixtureHandle,
		second: MixtureHandle,
		first_temperature: f32,
		second_temperature: f32,
		first_changed: bool,
		second_changed: bool,
	) -> Result<u32, WorldError> {
		if first_changed && self.require_handle(first)?.revision == u32::MAX {
			return Err(WorldError::RevisionExhausted(first));
		}
		if second_changed && self.require_handle(second)?.revision == u32::MAX {
			return Err(WorldError::RevisionExhausted(second));
		}
		if !first_changed && !second_changed {
			return Ok(0);
		}
		let (first_record, second_record) = self.require_two_handles_mut(first, second)?;
		if first_changed {
			first_record.temperature = first_temperature;
			first_record.revision += 1;
		}
		if second_changed {
			second_record.temperature = second_temperature;
			second_record.revision += 1;
		}
		Ok(u32::from(first_changed) + u32::from(second_changed))
	}

	fn require_two_handles_mut(
		&mut self,
		first: MixtureHandle,
		second: MixtureHandle,
	) -> Result<(&mut MixtureRecord, &mut MixtureRecord), WorldError> {
		self.require_handle(first)?;
		self.require_handle(second)?;
		if first.slot == second.slot {
			return Err(WorldError::SameMixtureHandles(first));
		}
		let first_index = first.slot as usize;
		let second_index = second.slot as usize;
		if first_index < second_index {
			let (left, right) = self.mixtures.split_at_mut(second_index);
			Ok((
				left[first_index]
					.mixture
					.as_mut()
					.expect("mixture handles were validated"),
				right[0]
					.mixture
					.as_mut()
					.expect("mixture handles were validated"),
			))
		} else {
			let (left, right) = self.mixtures.split_at_mut(first_index);
			Ok((
				right[0]
					.mixture
					.as_mut()
					.expect("mixture handles were validated"),
				left[second_index]
					.mixture
					.as_mut()
					.expect("mixture handles were validated"),
			))
		}
	}

	fn require_turf_handle(&self, handle: TurfHandle) -> Result<&TurfRecord, WorldError> {
		let Some(slot) = self.turfs.get(handle.slot as usize) else {
			return Err(WorldError::UnknownTurfHandle(handle));
		};
		let Some(generation) = slot.generation else {
			return Err(WorldError::UnknownTurfHandle(handle));
		};
		if generation != handle.generation {
			return Err(WorldError::StaleTurfHandle {
				requested: handle,
				current: generation,
			});
		}
		slot.turf
			.as_ref()
			.ok_or(WorldError::UnknownTurfHandle(handle))
	}

	fn require_turf_handle_mut(
		&mut self,
		handle: TurfHandle,
	) -> Result<&mut TurfRecord, WorldError> {
		let Some(slot) = self.turfs.get_mut(handle.slot as usize) else {
			return Err(WorldError::UnknownTurfHandle(handle));
		};
		let Some(generation) = slot.generation else {
			return Err(WorldError::UnknownTurfHandle(handle));
		};
		if generation != handle.generation {
			return Err(WorldError::StaleTurfHandle {
				requested: handle,
				current: generation,
			});
		}
		slot.turf
			.as_mut()
			.ok_or(WorldError::UnknownTurfHandle(handle))
	}

	fn remove_incident_edges(&mut self, slot: u32) {
		self.edges
			.retain(|key, _| key.left != slot && key.right != slot);
	}

	fn remove_incident_turf_edges(&mut self, slot: u32) {
		let previous_len = self.turf_edges.len();
		self.turf_edges
			.retain(|key| key.left != slot && key.right != slot);
		self.turf_firelock_edges
			.retain(|key| key.left != slot && key.right != slot);
		if self.turf_edges.len() != previous_len {
			self.turf_graph = None;
		}
	}

	fn remove_incident_heat_edges(&mut self, slot: u32) {
		self.heat_edges
			.retain(|key| key.left != slot && key.right != slot);
	}

	fn live_count(&self) -> usize {
		self.mixtures
			.iter()
			.filter(|slot| slot.mixture.is_some())
			.count()
	}

	fn build_graph(&self, edges: &BTreeMap<EdgeKey, f32>) -> Result<DiffusionGraph, WorldError> {
		let nodes = self
			.mixtures
			.iter()
			.enumerate()
			.filter_map(|(slot, mixture_slot)| {
				mixture_slot.mixture.as_ref().map(|_| GraphNode {
					handle: NodeHandle(slot as u32),
					generation: mixture_slot
						.generation
						.expect("occupied mixture slot has a generation"),
					mixture: Some(MixtureHandle {
						slot: slot as u32,
						generation: mixture_slot
							.generation
							.expect("occupied mixture slot has a generation"),
					}),
				})
			})
			.collect::<Vec<_>>();
		let directed = edges
			.keys()
			.flat_map(|edge| {
				[
					DirectedEdge {
						from: NodeHandle(edge.left),
						to: NodeHandle(edge.right),
					},
					DirectedEdge {
						from: NodeHandle(edge.right),
						to: NodeHandle(edge.left),
					},
				]
			})
			.collect::<Vec<_>>();
		validate_graph(&nodes, &directed).map_err(|error| WorldError::Graph(error.to_string()))
	}

	fn build_turf_graph(&self, edges: &BTreeSet<EdgeKey>) -> Result<DiffusionGraph, WorldError> {
		let nodes = self
			.turfs
			.iter()
			.enumerate()
			.filter_map(|(slot, turf_slot)| {
				let turf = turf_slot.turf.as_ref()?;
				let mixture = turf.mixture?;
				Some(GraphNode {
					handle: NodeHandle(slot as u32),
					generation: turf_slot
						.generation
						.expect("occupied turf slot has a generation"),
					mixture: Some(mixture),
				})
			})
			.collect::<Vec<_>>();
		let directed = edges
			.iter()
			.flat_map(|edge| {
				[
					DirectedEdge {
						from: NodeHandle(edge.left),
						to: NodeHandle(edge.right),
					},
					DirectedEdge {
						from: NodeHandle(edge.right),
						to: NodeHandle(edge.left),
					},
				]
			})
			.collect::<Vec<_>>();
		validate_graph(&nodes, &directed).map_err(|error| WorldError::Graph(error.to_string()))
	}
}

fn valid_mixture_state(mutation: &MixtureStateMutation) -> bool {
	mutation.temperature.is_finite()
		&& mutation.temperature >= MINIMUM_TEMPERATURE_K
		&& mutation.volume.is_finite()
		&& mutation.volume >= 0.0
		&& mutation
			.gases
			.iter()
			.all(|value| value.is_finite() && *value >= 0.0)
}

fn valid_turf_heat_state(state: TurfHeatState) -> bool {
	state.temperature.is_finite()
		&& state.thermal_conductivity.is_finite()
		&& state.thermal_conductivity > 0.0
		&& state.heat_capacity.is_finite()
		&& state.heat_capacity > 0.0
}

fn quantize(amount: f32) -> f32 {
	(amount / MOLAR_ACCURACY).round() * MOLAR_ACCURACY
}

fn copy_record(receiver: &mut MixtureRecord, giver: &MixtureRecord) {
	if receiver.immutable {
		return;
	}
	receiver.gases = giver.gases;
	receiver.temperature = giver.temperature;
}

fn scale_record(mixture: &mut MixtureRecord, factor: f32) {
	if mixture.immutable || !factor.is_finite() || factor < 0.0 {
		return;
	}
	for amount in &mut mixture.gases {
		*amount = (f64::from(*amount) * f64::from(factor)).min(f64::from(f32::MAX)) as f32;
		if *amount <= GAS_MIN_MOLES {
			*amount = 0.0;
		}
	}
}

fn merge_record(receiver: &mut MixtureRecord, giver: &MixtureRecord, specific_heats: &[f32]) {
	if receiver.immutable {
		return;
	}
	let receiver_capacity = record_heat_capacity(receiver, specific_heats);
	let giver_capacity = record_heat_capacity(giver, specific_heats);
	for (amount, added) in receiver.gases.iter_mut().zip(giver.gases) {
		*amount = (f64::from(*amount) + f64::from(added)).min(f64::from(f32::MAX)) as f32;
	}
	let combined_capacity = receiver_capacity + giver_capacity;
	if combined_capacity > MINIMUM_HEAT_CAPACITY {
		receiver.temperature = (receiver_capacity * receiver.temperature
			+ giver_capacity * giver.temperature)
			/ combined_capacity;
	}
}

fn remove_ratio_record(source: &mut MixtureRecord, ratio: f32) -> MixtureRecord {
	let mut removed = MixtureRecord::new();
	remove_ratio_into_record(source, &mut removed, ratio);
	removed
}

fn remove_ratio_into_record(
	source: &mut MixtureRecord,
	destination: &mut MixtureRecord,
	ratio: f32,
) {
	if !ratio.is_finite() || ratio <= 0.0 || destination.immutable {
		return;
	}
	let ratio = ratio.min(1.0);
	copy_record(destination, source);
	if source.immutable {
		for amount in &mut destination.gases {
			*amount = quantize(*amount * ratio);
		}
		return;
	}
	for (source_amount, removed_amount) in source.gases.iter_mut().zip(destination.gases.iter_mut())
	{
		*removed_amount = quantize(*source_amount * ratio);
		*source_amount -= *removed_amount;
	}
}

fn mixture_thermodynamics_differ(left: &MixtureRecord, right: &MixtureRecord) -> bool {
	left.temperature != right.temperature || left.gases != right.gases
}

fn mixtures_require_processing(left: &MixtureRecord, right: &MixtureRecord) -> bool {
	((left.temperature - right.temperature).abs() > 4.0
		&& total_moles(left) > MINIMUM_MOLES_DELTA_TO_MOVE)
		|| left
			.gases
			.iter()
			.zip(right.gases.iter())
			.any(|(left, right)| (left - right).abs() >= MINIMUM_MOLES_DELTA_TO_MOVE)
}

fn total_moles(mixture: &MixtureRecord) -> f32 {
	mixture.gases.iter().sum()
}

fn mixture_pressure(mixture: &MixtureRecord) -> f32 {
	if mixture.volume <= 0.0 {
		return 0.0;
	}
	total_moles(mixture) * IDEAL_GAS_CONSTANT * mixture.temperature / mixture.volume
}

fn record_heat_capacity(mixture: &MixtureRecord, specific_heats: &[f32]) -> f32 {
	mixture
		.gases
		.iter()
		.zip(specific_heats)
		.fold(0.0, |capacity, (amount, specific_heat)| {
			specific_heat.mul_add(*amount, capacity)
		})
		.max(mixture.minimum_heat_capacity)
}

fn harmonic_heat_capacity(first: f32, second: f32) -> f32 {
	let (smaller, larger) = if first < second {
		(first, second)
	} else {
		(second, first)
	};
	smaller / (1.0 + smaller / larger)
}

fn heat_exchange_energy(delta: f32, first_capacity: f32, second_capacity: f32) -> f32 {
	let first_infinite = first_capacity >= crate::numerics::conduction::BYOND_INFINITY_THRESHOLD;
	let second_infinite = second_capacity >= crate::numerics::conduction::BYOND_INFINITY_THRESHOLD;
	if first_infinite && second_infinite {
		return 0.0;
	}
	if first_infinite {
		return delta * second_capacity;
	}
	if second_infinite {
		return delta * first_capacity;
	}
	delta * harmonic_heat_capacity(first_capacity, second_capacity)
}

fn transfer_moles(
	source: &mut MixtureRecord,
	target: &mut MixtureRecord,
	amount: f32,
	specific_heats: &[f32; MAX_GAS_SLOTS],
) -> Result<f32, WorldError> {
	let source_total = total_moles(source);
	if !amount.is_finite() || amount <= 0.0 || source_total <= 0.0 {
		return Ok(0.0);
	}
	let ratio = (amount / source_total).min(1.0);
	let removed = source.gases.map(|moles| quantize(moles * ratio));
	for (gas_index, (target_amount, added)) in target.gases.iter().zip(removed).enumerate() {
		if f64::from(*target_amount) + f64::from(added) > f64::from(f32::MAX) {
			return Err(WorldError::MoleOverflow(crate::metadata::GasId(
				gas_index as u16,
			)));
		}
	}
	let target_energy = record_heat_capacity(target, specific_heats) * target.temperature;
	let removed_capacity = removed
		.iter()
		.zip(specific_heats)
		.fold(0.0, |capacity, (moles, specific_heat)| {
			specific_heat.mul_add(*moles, capacity)
		});
	for ((source_amount, target_amount), added) in source
		.gases
		.iter_mut()
		.zip(target.gases.iter_mut())
		.zip(removed)
	{
		*source_amount -= added;
		*target_amount += added;
	}
	let target_capacity = record_heat_capacity(target, specific_heats);
	if target_capacity > MINIMUM_HEAT_CAPACITY {
		target.temperature =
			(target_energy + removed_capacity * source.temperature) / target_capacity;
	}
	Ok(total_moles_from_gases(&removed))
}

fn total_moles_from_gases(gases: &[f32; MAX_GAS_SLOTS]) -> f32 {
	gases.iter().sum()
}

#[cfg(test)]
mod tests {
	use super::*;

	fn handle(slot: u32) -> MixtureHandle {
		MixtureHandle {
			slot,
			generation: 1,
		}
	}

	#[test]
	fn stage_rejects_revision_exhaustion_before_mutating_any_mixture() {
		let mut world = DogmosWorld::new(1024 * 1024);
		world
			.apply_lifecycle(&[
				LifecycleMutation {
					action: LifecycleAction::Register,
					handle: handle(0),
				},
				LifecycleMutation {
					action: LifecycleAction::Register,
					handle: handle(1),
				},
			])
			.unwrap();
		world.mixtures[0].mixture.as_mut().unwrap().revision = u32::MAX;
		world.mixtures[1].mixture.as_mut().unwrap().revision = 7;

		assert_eq!(
			world.process_stage_cancellable(WorldStage::ProcessTurfs, 0.5, || false),
			Err(WorldError::RevisionExhausted(handle(0)))
		);
		assert_eq!(world.snapshot(handle(0)).unwrap().revision, u32::MAX);
		assert_eq!(world.snapshot(handle(1)).unwrap().revision, 7);
	}
}
