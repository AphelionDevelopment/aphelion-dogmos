pub use crate::frontier::FrontierError;
#[cfg(debug_assertions)]
use crate::numerics::diffusion::{diffusion_step_into_cancellable, DiffusionError};
pub use crate::stage_cursor::{StageChunkRequest, StageChunkResult};
use crate::{
	frontier::FrontierState,
	metadata::{
		GasMetadata, GasMetadataError, GasMetadataRegistry, ReactionMetadata,
		ReactionMetadataError, ReactionMetadataRegistry, TurfHandle,
	},
	numerics::diffusion::{
		diffusion_self_weight, validate_graph, DiffusionGraph, DirectedEdge, GraphNode, NodeHandle,
		GAS_DIFFUSION_CONSTANT,
	},
	stage_cursor::{StageCursor, MAX_STAGE_WORK_LIMIT},
	topology::{PackedTopology, TopologyError, MAX_TURF_NEIGHBORS},
	transaction::{IndexedTransaction, TransactionError},
	MixtureHandle, MAX_GAS_SLOTS,
};
use std::{
	borrow::Cow,
	collections::{BTreeMap, BTreeSet},
	error::Error,
	fmt,
	time::Instant,
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
	heat_active_index: Option<u32>,
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StageConflictReason {
	ActiveStageMutation {
		operation: &'static str,
	},
	FrontierEpoch {
		requested: u64,
		committed: Option<u64>,
	},
	CursorIdentity {
		requested_stage: WorldStage,
		requested_frontier_epoch: u64,
		requested_stage_epoch: u64,
		requested_seconds_per_tick_bits: u64,
		active_stage: WorldStage,
		active_frontier_epoch: u64,
		active_stage_epoch: u64,
		active_seconds_per_tick_bits: u64,
	},
	TopologyRevision {
		captured: u64,
		current: u64,
	},
	TransactionGeneration {
		requested: MixtureHandle,
		current: MixtureHandle,
	},
	TransactionHandleMissing {
		handle: MixtureHandle,
	},
	TransactionRevision {
		handle: MixtureHandle,
		expected: u32,
		actual: u32,
	},
}

impl fmt::Display for StageConflictReason {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::ActiveStageMutation { operation } => {
				write!(formatter, "{operation} attempted while a stage is active")
			}
			Self::FrontierEpoch {
				requested,
				committed,
			} => write!(
				formatter,
				"requested frontier epoch {requested}, committed frontier epoch {committed:?}"
			),
			Self::CursorIdentity {
				requested_stage,
				requested_frontier_epoch,
				requested_stage_epoch,
				requested_seconds_per_tick_bits,
				active_stage,
				active_frontier_epoch,
				active_stage_epoch,
				active_seconds_per_tick_bits,
			} => write!(
				formatter,
				"requested cursor stage={requested_stage:?} frontier_epoch={requested_frontier_epoch} stage_epoch={requested_stage_epoch} seconds_per_tick_bits={requested_seconds_per_tick_bits}, active cursor stage={active_stage:?} frontier_epoch={active_frontier_epoch} stage_epoch={active_stage_epoch} seconds_per_tick_bits={active_seconds_per_tick_bits}"
			),
			Self::TopologyRevision { captured, current } => write!(
				formatter,
				"captured topology revision {captured}, current topology revision {current}"
			),
			Self::TransactionGeneration { requested, current } => write!(
				formatter,
				"transaction requested mixture {requested:?}, current transaction mixture {current:?}"
			),
			Self::TransactionHandleMissing { handle } => {
				write!(formatter, "transaction mixture {handle:?} is no longer registered")
			}
			Self::TransactionRevision {
				handle,
				expected,
				actual,
			} => write!(
				formatter,
				"transaction mixture {handle:?} expected revision {expected}, current revision {actual}"
			),
		}
	}
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(C)]
pub struct ReactionContinuationToken {
	pub slot: u32,
	pub generation: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReactionProgress {
	pub flags: u32,
	pub work_items: u32,
	pub pending: bool,
}

pub const REACTION_REACTING: u32 = 1 << 0;
pub const REACTION_STOP: u32 = 1 << 1;
pub const REACTION_VOLATILE: u32 = 1 << 2;
const REACTION_FLAGS: u32 = REACTION_REACTING | REACTION_STOP | REACTION_VOLATILE;

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
		turf: Option<TurfHandle>,
		mixture: MixtureHandle,
		target: crate::metadata::GameplayHandle,
		reaction: crate::metadata::ReactionId,
		continuation: ReactionContinuationToken,
	},
	ReactionFinished {
		mixture: MixtureHandle,
		target: crate::metadata::GameplayHandle,
		reaction: crate::metadata::ReactionId,
		kind: crate::metadata::NativeReactionKind,
		values: [f32; 4],
	},
	ReactionProfiled {
		mixture: MixtureHandle,
		target: crate::metadata::GameplayHandle,
		reaction: crate::metadata::ReactionId,
		cost_ms: f32,
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
	pub total_moles: f32,
	pub pressure: f32,
	pub heat_capacity: f32,
	pub gases: [f32; MAX_GAS_SLOTS],
	pub immutable: bool,
}

#[derive(Debug, PartialEq)]
pub enum WorldError {
	Frontier(FrontierError),
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
	PendingEventCountExceeded {
		requested: u32,
		available: u32,
	},
	UnknownReactionContinuation(ReactionContinuationToken),
	StaleReactionContinuation {
		requested: ReactionContinuationToken,
		current: u32,
	},
	ReactionContinuationCapacityExceeded,
	InvalidReactionResult(u32),
	InvalidReactionProfileThreshold,
	InvalidConductivity,
	InvalidEqualizeHardTurfLimit,
	InvalidSecondsPerTick,
	InvalidStageWorkLimit(u32),
	StageConflict(StageConflictReason),
	StageNotImplemented(WorldStage),
	Graph(String),
	State(String),
	StateCapacityExceeded,
	AllocationFailed,
	Cancelled,
}

impl fmt::Display for WorldError {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::StageConflict(reason) => write!(formatter, "stage conflict: {reason}"),
			_ => write!(formatter, "{self:?}"),
		}
	}
}

impl Error for WorldError {}

pub struct DogmosWorld {
	frontier: FrontierState,
	heat_active: Vec<TurfHandle>,
	stage_cursor: Option<StageCursor>,
	stage_diffusion: Option<StageDiffusionState>,
	stage_heat: Option<StageHeatState>,
	stage_reactions: Option<StageReactionState>,
	stage_components: Option<StageComponentState>,
	stage_component_turfs: Option<Vec<TurfHandle>>,
	use_committed_frontier: bool,
	gas_registry: Option<GasMetadataRegistry>,
	reaction_registry: Option<ReactionMetadataRegistry>,
	mixtures: Vec<MixtureSlot>,
	turfs: Vec<TurfSlot>,
	edges: BTreeMap<EdgeKey, f32>,
	topology: PackedTopology,
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
	#[cfg(test)]
	mixture_edge_filter_passes: u64,
}

#[derive(Clone)]
struct ReactionContinuation {
	turf: Option<TurfHandle>,
	mixture: MixtureHandle,
	target: crate::metadata::GameplayHandle,
	next_reaction_index: u32,
	reaction_profile_threshold_ms: Option<f32>,
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
	flags: u32,
	work_items: u32,
	native_updates: u32,
}

struct StageDiffusionState {
	turfs: Vec<TurfHandle>,
	mixtures: Vec<MixtureHandle>,
	index_by_turf: BTreeMap<TurfHandle, usize>,
	seen_mixtures: BTreeSet<MixtureHandle>,
	input: Vec<[f32; MAX_GAS_SLOTS]>,
	output: Vec<[f32; MAX_GAS_SLOTS]>,
	input_temperatures: Vec<f32>,
	minimum_heat_capacities: Vec<f32>,
	input_energy: Vec<f32>,
	output_energy: Vec<f32>,
	specific_heats: [f32; MAX_GAS_SLOTS],
	next_node: usize,
}

type HeatEdge = (u32, u32, f32, f32);

struct StageHeatState {
	nodes: Vec<StageHeatNode>,
	index_by_slot: BTreeMap<u32, u32>,
	temperatures: Vec<f32>,
	conductivities: Vec<f32>,
	heat_capacities: Vec<f32>,
	staged_mixtures: BTreeMap<MixtureHandle, MixtureRecord>,
	linked_mixtures: BTreeSet<MixtureHandle>,
	staged_events: Vec<WorldEvent>,
	next_active_seed: usize,
	next_node: usize,
	next_topology_node: usize,
	next_topology_neighbor: usize,
	/// (first slot, second slot, first's unscaled weight, second's unscaled weight) - the weights
	/// are computed once at discovery (see advance_stage_heat_topology()) since they don't change
	/// across conduction substeps.
	edges: Vec<HeatEdge>,
	row_sums: Vec<f32>,
	conduction_substeps: Option<u32>,
	conduction_substep: u32,
	conduction_edge: usize,
	conduction_scale: f32,
}

#[derive(Clone, Copy)]
struct StageHeatNode {
	handle: TurfHandle,
	heat: TurfHeatState,
	mixture: Option<MixtureHandle>,
	can_continue: bool,
}

struct StageReactionState {
	targets: Vec<(TurfHandle, MixtureHandle)>,
	active_continuations: BTreeSet<MixtureHandle>,
	seen_mixtures: BTreeSet<MixtureHandle>,
	staged: BTreeMap<MixtureHandle, MixtureRecord>,
	staged_events: Vec<WorldEvent>,
	pending: Option<(TurfHandle, MixtureHandle, PendingDmReaction)>,
	next_target: usize,
}

struct StageComponentState {
	targets: Vec<TurfHandle>,
	active_by_slot: BTreeMap<u32, TurfHandle>,
	visited: BTreeSet<u32>,
	next_seed: usize,
	queue: Vec<TurfHandle>,
	queue_index: usize,
	next_neighbor: usize,
	component_ready: bool,
	transaction: IndexedTransaction<MixtureRecord>,
	published_generation_by_slot: Vec<Option<u32>>,
	staged_events: Vec<WorldEvent>,
	callback_events: u32,
	components_processed: u32,
}

impl StageComponentState {
	fn try_new(slot_count: usize, max_entries: usize) -> Result<Self, WorldError> {
		let mut published_generation_by_slot = Vec::new();
		published_generation_by_slot
			.try_reserve_exact(slot_count)
			.map_err(|_| WorldError::AllocationFailed)?;
		published_generation_by_slot.resize(slot_count, None);
		Ok(Self {
			targets: Vec::new(),
			active_by_slot: BTreeMap::new(),
			visited: BTreeSet::new(),
			next_seed: 0,
			queue: Vec::new(),
			queue_index: 0,
			next_neighbor: 0,
			component_ready: false,
			transaction: IndexedTransaction::try_new(slot_count, max_entries)
				.map_err(transaction_world_error)?,
			published_generation_by_slot,
			staged_events: Vec::new(),
			callback_events: 0,
			components_processed: 0,
		})
	}

	fn mixture_was_published(&self, handle: MixtureHandle) -> bool {
		self.published_generation_by_slot
			.get(handle.slot as usize)
			.is_some_and(|generation| *generation == Some(handle.generation))
	}

	fn mark_mixture_published(&mut self, handle: MixtureHandle) {
		self.published_generation_by_slot[handle.slot as usize] = Some(handle.generation);
	}
}

impl StageHeatState {
	fn new() -> Self {
		Self {
			nodes: Vec::new(),
			index_by_slot: BTreeMap::new(),
			temperatures: Vec::new(),
			conductivities: Vec::new(),
			heat_capacities: Vec::new(),
			staged_mixtures: BTreeMap::new(),
			linked_mixtures: BTreeSet::new(),
			staged_events: Vec::new(),
			next_active_seed: 0,
			next_node: 0,
			next_topology_node: 0,
			next_topology_neighbor: 0,
			edges: Vec::new(),
			row_sums: Vec::new(),
			conduction_substeps: None,
			conduction_substep: 0,
			conduction_edge: 0,
			conduction_scale: 0.0,
		}
	}
}

impl StageDiffusionState {
	fn new(specific_heats: [f32; MAX_GAS_SLOTS]) -> Self {
		Self {
			turfs: Vec::new(),
			mixtures: Vec::new(),
			index_by_turf: BTreeMap::new(),
			seen_mixtures: BTreeSet::new(),
			input: Vec::new(),
			output: Vec::new(),
			input_temperatures: Vec::new(),
			minimum_heat_capacities: Vec::new(),
			input_energy: Vec::new(),
			output_energy: Vec::new(),
			specific_heats,
			next_node: 0,
		}
	}
}

fn validate_reaction_profile_threshold(threshold_ms: Option<f32>) -> Result<(), WorldError> {
	if threshold_ms.is_some_and(|threshold| !threshold.is_finite() || threshold < 0.0) {
		return Err(WorldError::InvalidReactionProfileThreshold);
	}
	Ok(())
}

fn map_topology_error(error: TopologyError) -> WorldError {
	match error {
		TopologyError::AllocationFailed => WorldError::AllocationFailed,
		other => WorldError::Graph(format!("{other:?}")),
	}
}

fn transaction_world_error(error: TransactionError) -> WorldError {
	match error {
		TransactionError::AllocationFailed => WorldError::AllocationFailed,
		TransactionError::CapacityExceeded => WorldError::StateCapacityExceeded,
		TransactionError::HandleConflict { requested, current } => {
			WorldError::StageConflict(StageConflictReason::TransactionGeneration {
				requested,
				current,
			})
		}
		TransactionError::UnknownHandle(handle) | TransactionError::SameHandle(handle) => {
			WorldError::DuplicateMutableTurfMixture(handle)
		}
	}
}

fn require_turf_handle_in(
	turfs: &[TurfSlot],
	handle: TurfHandle,
) -> Result<&TurfRecord, WorldError> {
	let Some(slot) = turfs.get(handle.slot as usize) else {
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
			frontier: FrontierState::default(),
			heat_active: Vec::new(),
			stage_cursor: None,
			stage_diffusion: None,
			stage_heat: None,
			stage_reactions: None,
			stage_components: None,
			stage_component_turfs: None,
			use_committed_frontier: false,
			gas_registry: None,
			reaction_registry: None,
			mixtures: Vec::new(),
			turfs: Vec::new(),
			edges: BTreeMap::new(),
			topology: PackedTopology::default(),
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
			#[cfg(test)]
			mixture_edge_filter_passes: 0,
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

	pub fn begin_frontier(&mut self, epoch: u64, expected: u32) -> Result<(), WorldError> {
		if self.stage_cursor.is_some() {
			return Err(WorldError::StageConflict(
				StageConflictReason::ActiveStageMutation {
					operation: "begin frontier",
				},
			));
		}
		let maximum = u32::try_from(self.turfs.len()).unwrap_or(u32::MAX);
		self.frontier
			.begin(epoch, expected, maximum)
			.map_err(WorldError::Frontier)
	}

	pub fn append_frontier(
		&mut self,
		epoch: u64,
		offset: u32,
		handles: &[TurfHandle],
	) -> Result<u32, WorldError> {
		self.frontier
			.append(epoch, offset, handles)
			.map_err(WorldError::Frontier)
	}

	pub fn commit_frontier(&mut self, epoch: u64) -> Result<u32, WorldError> {
		if self.stage_cursor.is_some() {
			return Err(WorldError::StageConflict(
				StageConflictReason::ActiveStageMutation {
					operation: "commit frontier",
				},
			));
		}
		for handle in self.frontier.pending(epoch).map_err(WorldError::Frontier)? {
			require_turf_handle_in(&self.turfs, *handle)?;
		}
		self.frontier
			.commit_validated(epoch)
			.map_err(WorldError::Frontier)
	}

	/// Adds handles directly to the committed frontier - the incremental-sync counterpart to
	/// begin/append/commit. See `FrontierState::add` for the rationale.
	pub fn add_frontier(&mut self, epoch: u64, handles: &[TurfHandle]) -> Result<u32, WorldError> {
		if self.stage_cursor.is_some() {
			return Err(WorldError::StageConflict(
				StageConflictReason::ActiveStageMutation {
					operation: "add frontier handles",
				},
			));
		}
		for handle in handles {
			require_turf_handle_in(&self.turfs, *handle)?;
		}
		let maximum = u32::try_from(self.turfs.len()).unwrap_or(u32::MAX);
		self.frontier
			.add(epoch, handles, maximum)
			.map_err(WorldError::Frontier)
	}

	/// Removes handles directly from the committed frontier - the incremental-sync counterpart
	/// to begin/append/commit. See `FrontierState::remove` for the rationale.
	pub fn remove_frontier(
		&mut self,
		epoch: u64,
		handles: &[TurfHandle],
	) -> Result<u32, WorldError> {
		if self.stage_cursor.is_some() {
			return Err(WorldError::StageConflict(
				StageConflictReason::ActiveStageMutation {
					operation: "remove frontier handles",
				},
			));
		}
		self.frontier
			.remove(epoch, handles)
			.map_err(WorldError::Frontier)
	}

	pub fn committed_frontier_epoch(&self) -> Option<u64> {
		self.frontier.committed_epoch()
	}

	pub fn committed_frontier(&self) -> &[TurfHandle] {
		self.frontier.committed()
	}

	pub fn frontier_upload_bytes(&self) -> u64 {
		self.frontier.upload_bytes()
	}

	/// Returns the committed frontier's element-storage lower bound.
	///
	/// Hash-table control bytes, bucket padding, and allocator metadata are excluded.
	pub fn frontier_committed_storage_bytes_lower_bound(&self) -> u64 {
		self.frontier.committed_storage_bytes_lower_bound()
	}

	pub fn frontier_committed_capacities(&self) -> (usize, usize) {
		self.frontier.committed_capacities()
	}

	pub fn topology_revision(&self) -> u64 {
		self.topology.revision()
	}

	pub fn packed_topology_bytes(&self) -> u64 {
		self.topology.allocated_bytes()
	}

	pub fn stage_telemetry(&self) -> Option<(WorldStage, u64, u32, u32)> {
		self.stage_cursor.as_ref().map(|cursor| {
			let frontier_count = u32::try_from(self.frontier.committed().len()).unwrap_or(u32::MAX);
			let (active_heat_cursor, active_heat_count) = if cursor.stage == WorldStage::TurfHeat {
				(
					self.stage_heat
						.as_ref()
						.map(|state| u32::try_from(state.next_active_seed).unwrap_or(u32::MAX))
						.unwrap_or_default(),
					u32::try_from(self.heat_active.len()).unwrap_or(u32::MAX),
				)
			} else {
				(0, 0)
			};
			(
				cursor.stage,
				cursor.stage_epoch,
				cursor
					.next_frontier_index
					.saturating_add(active_heat_cursor),
				frontier_count
					.saturating_sub(cursor.next_frontier_index)
					.saturating_add(active_heat_count.saturating_sub(active_heat_cursor)),
			)
		})
	}

	/// Returns a lower bound for active reusable vector capacity in bytes.
	///
	/// The value excludes maps, sets, and allocator metadata. Per-stage state contributes only
	/// while that stage is active because committed stages currently drop their state.
	pub fn reusable_workset_bytes(&self) -> u64 {
		let mut active_vec_capacity_bytes_lower_bound = self.input.capacity()
			* std::mem::size_of::<f32>()
			+ self.output.capacity() * std::mem::size_of::<f32>()
			+ self.events.capacity() * std::mem::size_of::<WorldEvent>();
		active_vec_capacity_bytes_lower_bound +=
			self.heat_active.capacity() * std::mem::size_of::<TurfHandle>();
		if let Some(state) = &self.stage_diffusion {
			active_vec_capacity_bytes_lower_bound +=
				state.turfs.capacity() * std::mem::size_of::<TurfHandle>();
			active_vec_capacity_bytes_lower_bound +=
				state.mixtures.capacity() * std::mem::size_of::<MixtureHandle>();
			active_vec_capacity_bytes_lower_bound +=
				state.input.capacity() * std::mem::size_of::<[f32; MAX_GAS_SLOTS]>();
			active_vec_capacity_bytes_lower_bound +=
				state.output.capacity() * std::mem::size_of::<[f32; MAX_GAS_SLOTS]>();
		}
		if let Some(state) = &self.stage_heat {
			active_vec_capacity_bytes_lower_bound +=
				state.nodes.capacity() * std::mem::size_of::<StageHeatNode>();
			active_vec_capacity_bytes_lower_bound +=
				state.temperatures.capacity() * std::mem::size_of::<f32>();
			active_vec_capacity_bytes_lower_bound +=
				state.conductivities.capacity() * std::mem::size_of::<f32>();
			active_vec_capacity_bytes_lower_bound +=
				state.heat_capacities.capacity() * std::mem::size_of::<f32>();
			active_vec_capacity_bytes_lower_bound +=
				state.edges.capacity() * std::mem::size_of::<HeatEdge>();
			active_vec_capacity_bytes_lower_bound +=
				state.row_sums.capacity() * std::mem::size_of::<f32>();
		}
		if let Some(state) = &self.stage_reactions {
			active_vec_capacity_bytes_lower_bound +=
				state.targets.capacity() * std::mem::size_of::<(TurfHandle, MixtureHandle)>();
			active_vec_capacity_bytes_lower_bound +=
				state.staged_events.capacity() * std::mem::size_of::<WorldEvent>();
		}
		if let Some(state) = &self.stage_components {
			active_vec_capacity_bytes_lower_bound +=
				state.targets.capacity() * std::mem::size_of::<TurfHandle>();
			active_vec_capacity_bytes_lower_bound +=
				state.queue.capacity() * std::mem::size_of::<TurfHandle>();
			active_vec_capacity_bytes_lower_bound +=
				state.staged_events.capacity() * std::mem::size_of::<WorldEvent>();
			active_vec_capacity_bytes_lower_bound += state.transaction.capacity_bytes_lower_bound();
			active_vec_capacity_bytes_lower_bound +=
				state.published_generation_by_slot.capacity() * std::mem::size_of::<Option<u32>>();
		}
		if let Some(component_turfs) = &self.stage_component_turfs {
			active_vec_capacity_bytes_lower_bound +=
				component_turfs.capacity() * std::mem::size_of::<TurfHandle>();
		}
		active_vec_capacity_bytes_lower_bound as u64
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

		// Unregistering N mixtures used to scan the entire turf slot table N times (once per
		// mutation) to find turfs pointing at each departing mixture - O(unregisters × total
		// turfs), worst case exactly when mass mixture teardown makes the batch large. Collect
		// every handle being unregistered in this batch first, then do one pass over self.turfs
		// for the whole batch below instead.
		let unregistering: std::collections::BTreeSet<MixtureHandle> = mutations
			.iter()
			.filter(|mutation| matches!(mutation.action, LifecycleAction::Unregister))
			.map(|mutation| mutation.handle)
			.collect();
		let mut invalidated_slots = BTreeSet::new();

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
						invalidated_slots.insert(mutation.handle.slot);
						changed = true;
					}
				}
				LifecycleAction::Unregister => {
					self.invalidate_continuations_for_mixture_slot(mutation.handle.slot);
					self.mixtures[slot].mixture = None;
					invalidated_slots.insert(mutation.handle.slot);
					changed = true;
				}
			}
		}
		if !invalidated_slots.is_empty() {
			#[cfg(test)]
			{
				self.mixture_edge_filter_passes += 1;
			}
			self.edges.retain(|key, _| {
				!invalidated_slots.contains(&key.left) && !invalidated_slots.contains(&key.right)
			});
			self.graph = None;
		}
		if !unregistering.is_empty() {
			let mut detached_turfs = Vec::new();
			for (turf_slot, turf) in self
				.turfs
				.iter_mut()
				.enumerate()
				.filter_map(|(slot, turf_slot)| turf_slot.turf.as_mut().map(|turf| (slot, turf)))
			{
				if turf
					.mixture
					.is_some_and(|handle| unregistering.contains(&handle))
				{
					turf.mixture = None;
					detached_turfs.push(turf_slot as u32);
				}
			}
			for turf_slot in detached_turfs {
				self.remove_incident_turf_edges(turf_slot);
			}
			self.turf_graph = None;
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
		if self.stage_cursor.is_some() {
			return Err(WorldError::StageConflict(
				StageConflictReason::ActiveStageMutation {
					operation: "apply turf lifecycle",
				},
			));
		}
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
					let replaces_generation =
						self.turfs[handle.slot as usize].generation != Some(handle.generation);
					let invalidates_continuation = replaces_generation
						|| self.turfs[handle.slot as usize]
							.turf
							.as_ref()
							.is_none_or(|turf| turf.mixture != *mixture);
					if invalidates_continuation {
						self.invalidate_continuations_for_turf_slot(handle.slot);
					}
					if replaces_generation {
						self.deactivate_turf_heat_slot(handle.slot);
						self.remove_incident_turf_edges(handle.slot);
					}
					let slot = &mut self.turfs[handle.slot as usize];
					let remove_edges = mixture.is_none();
					let heat = (slot.generation == Some(handle.generation))
						.then(|| slot.turf.as_ref().and_then(|turf| turf.heat))
						.flatten();
					let heat_active_index = (slot.generation == Some(handle.generation))
						.then(|| slot.turf.as_ref().and_then(|turf| turf.heat_active_index))
						.flatten();
					slot.generation = Some(handle.generation);
					slot.turf = Some(TurfRecord {
						mixture: *mixture,
						heat,
						heat_active_index,
					});
					if remove_edges {
						self.remove_incident_gas_edges(handle.slot);
					}
					if heat.is_none() {
						self.remove_incident_heat_edges(handle.slot);
					}
				}
				TurfLifecycleMutation::Unregister { handle } => {
					self.invalidate_continuations_for_turf_slot(handle.slot);
					self.deactivate_turf_heat_slot(handle.slot);
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
		if self.stage_cursor.is_some() {
			return Err(WorldError::StageConflict(
				StageConflictReason::ActiveStageMutation {
					operation: "apply turf heat",
				},
			));
		}
		for mutation in mutations {
			self.require_turf_handle(mutation.handle)?;
			if let Some(state) = mutation.state {
				if !valid_turf_heat_state(state) {
					return Err(WorldError::InvalidTurfHeatState(mutation.handle));
				}
			}
		}
		self.heat_active
			.try_reserve(mutations.len())
			.map_err(|_| WorldError::AllocationFailed)?;
		for mutation in mutations {
			let was_active = self.turfs[mutation.handle.slot as usize]
				.turf
				.as_ref()
				.expect("turf heat batch was validated")
				.heat_active_index
				.is_some();
			self.turfs[mutation.handle.slot as usize]
				.turf
				.as_mut()
				.expect("turf heat batch was validated")
				.heat = mutation.state;
			let activation_threshold = if was_active {
				MINIMUM_TEMPERATURE_FOR_SUPERCONDUCTION_K
			} else {
				MINIMUM_TEMPERATURE_START_SUPERCONDUCTION_K
			};
			let activation_temperature = mutation
				.state
				.map(|state| {
					self.turf_superconduction_temperature(mutation.handle, state.temperature)
				})
				.transpose()?;
			if activation_temperature.is_some_and(|temperature| temperature >= activation_threshold)
			{
				self.activate_turf_heat(mutation.handle)?;
			} else {
				self.deactivate_turf_heat_slot(mutation.handle.slot);
			}
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
		if self.stage_cursor.is_some() {
			return Err(WorldError::StageConflict(
				StageConflictReason::ActiveStageMutation {
					operation: "apply turf heat adjacency",
				},
			));
		}
		// See apply_turf_adjacency() above for why this mutates self.topology directly instead of
		// cloning it into a candidate first.
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
			if mutation.connected {
				self.topology
					.connect_heat(mutation.left, mutation.right)
					.map_err(map_topology_error)?;
			} else {
				self.topology.disconnect_heat(mutation.left, mutation.right);
			}
		}
		Ok(mutations.len() as u32)
	}

	pub fn apply_turf_adjacency(
		&mut self,
		mutations: &[TurfAdjacencyMutation],
	) -> Result<u32, WorldError> {
		if self.stage_cursor.is_some() {
			return Err(WorldError::StageConflict(
				StageConflictReason::ActiveStageMutation {
					operation: "apply turf adjacency",
				},
			));
		}
		// Mutates self.topology directly rather than cloning it into a candidate first: the clone
		// was a full copy of every turf slot in the world, paid on every call regardless of batch
		// size (down to a single edge), to get all-or-nothing rollback on a mid-batch failure. That
		// rollback isn't load-bearing - every caller (the DM dispatcher and the service's own
		// wire handler) already treats a rejected batch as fatal and halts, so a partially-applied
		// topology on failure is no worse than what already happens. The clone was unnoticeable on
		// a handful of test turfs and minutes of wall time on a real map's turf count.
		let revision_before = self.topology.revision();
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
			if mutation.connected {
				self.topology
					.connect_gas(mutation.left, mutation.right)
					.map_err(map_topology_error)?;
			} else {
				self.topology.disconnect_gas(mutation.left, mutation.right);
			}
		}
		if self.topology.revision() == revision_before {
			return Ok(mutations.len() as u32);
		}
		// Invalidate rather than eagerly rebuild: process_turf_diffusion already rebuilds
		// self.turf_graph lazily from self.topology the first time it is actually needed (the
		// only other reader of this field).
		self.turf_graph = None;
		Ok(mutations.len() as u32)
	}

	pub fn apply_turf_firelocks(
		&mut self,
		mutations: &[TurfFirelockMutation],
	) -> Result<u32, WorldError> {
		if self.stage_cursor.is_some() {
			return Err(WorldError::StageConflict(
				StageConflictReason::ActiveStageMutation {
					operation: "apply turf firelocks",
				},
			));
		}
		// See apply_turf_adjacency() above for why this mutates self.topology directly instead of
		// cloning it into a candidate first.
		for mutation in mutations {
			self.require_turf_handle(mutation.left)?;
			self.require_turf_handle(mutation.right)?;
			if mutation.left.slot == mutation.right.slot {
				return Err(WorldError::SelfTurfAdjacency(mutation.left));
			}
			self.topology
				.set_firelock(mutation.left, mutation.right, mutation.firelock)
				.map_err(map_topology_error)?;
		}
		Ok(mutations.len() as u32)
	}

	pub fn apply_adjacency(&mut self, mutations: &[AdjacencyMutation]) -> Result<u32, WorldError> {
		// Mirrors apply_turf_adjacency(): mutate self.edges directly and invalidate self.graph
		// lazily instead of cloning the entire edge map every call regardless of batch size. The
		// rollback guarantee a clone-and-compare gives isn't load-bearing here for the same
		// reason apply_turf_adjacency()'s doc comment gives - this just wasn't updated with it.
		let mut changed = false;
		for mutation in mutations {
			self.require_handle(mutation.left)?;
			self.require_handle(mutation.right)?;
			if !mutation.conductivity.is_finite() || mutation.conductivity < 0.0 {
				return Err(WorldError::InvalidConductivity);
			}
			let key = EdgeKey::new(mutation.left.slot, mutation.right.slot)?;
			if mutation.conductivity == 0.0 {
				if self.edges.remove(&key).is_some() {
					changed = true;
				}
			} else if self.edges.insert(key, mutation.conductivity) != Some(mutation.conductivity) {
				changed = true;
			}
		}
		if !changed {
			return Ok(mutations.len() as u32);
		}
		let graph = self.build_graph(&self.edges)?;
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
		let heat_capacity = self
			.gas_registry
			.as_ref()
			.map_or(mixture.minimum_heat_capacity, |registry| {
				record_heat_capacity(mixture, registry.specific_heats())
			});
		Ok(MixtureSnapshot {
			revision: mixture.revision,
			temperature: mixture.temperature,
			volume: mixture.volume,
			minimum_heat_capacity: mixture.minimum_heat_capacity,
			total_moles: total_moles(mixture),
			pressure: mixture_pressure(mixture),
			heat_capacity,
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

	pub fn pending_events(&self, maximum: u32) -> &[WorldEvent] {
		let count = self.events.len().min(maximum as usize);
		&self.events[..count]
	}

	pub fn discard_pending_events(&mut self, count: u32) -> Result<(), WorldError> {
		let available = self.events.len();
		if count as usize > available {
			return Err(WorldError::PendingEventCountExceeded {
				requested: count,
				available: u32::try_from(available).unwrap_or(u32::MAX),
			});
		}
		self.events.drain(..count as usize);
		Ok(())
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

	pub fn pending_stage_epoch(&self) -> Option<u64> {
		self.stage_cursor.as_ref().map(|cursor| cursor.stage_epoch)
	}

	pub fn process_stage_chunk_cancellable(
		&mut self,
		request: StageChunkRequest,
		should_cancel: impl FnMut() -> bool,
	) -> Result<StageChunkResult, WorldError> {
		self.process_stage_chunk_cancellable_with_event_limit(
			request,
			self.max_events,
			should_cancel,
		)
	}

	pub fn process_stage_chunk_cancellable_with_event_limit(
		&mut self,
		request: StageChunkRequest,
		event_limit: u32,
		mut should_cancel: impl FnMut() -> bool,
	) -> Result<StageChunkResult, WorldError> {
		let event_capacity = self.max_events.min(event_limit) as usize;
		let result =
			self.process_stage_chunk_cancellable_inner(request, event_capacity, &mut should_cancel);
		if result.is_err() {
			self.abort_stage();
		}
		result
	}

	fn abort_stage(&mut self) {
		self.stage_cursor = None;
		self.stage_diffusion = None;
		self.stage_heat = None;
		self.stage_reactions = None;
		self.stage_components = None;
		self.stage_component_turfs = None;
		self.use_committed_frontier = false;
	}

	fn process_stage_chunk_cancellable_inner(
		&mut self,
		request: StageChunkRequest,
		event_capacity: usize,
		mut should_cancel: impl FnMut() -> bool,
	) -> Result<StageChunkResult, WorldError> {
		if request.work_limit == 0 || request.work_limit > MAX_STAGE_WORK_LIMIT {
			return Err(WorldError::InvalidStageWorkLimit(request.work_limit));
		}
		if !request.seconds_per_tick.is_finite() || request.seconds_per_tick <= 0.0 {
			return Err(WorldError::InvalidSecondsPerTick);
		}
		if self.frontier.committed_epoch() != Some(request.frontier_epoch) {
			return Err(WorldError::StageConflict(
				StageConflictReason::FrontierEpoch {
					requested: request.frontier_epoch,
					committed: self.frontier.committed_epoch(),
				},
			));
		}
		match &self.stage_cursor {
			Some(cursor) if !cursor.matches(request) => {
				return Err(WorldError::StageConflict(
					StageConflictReason::CursorIdentity {
						requested_stage: request.stage,
						requested_frontier_epoch: request.frontier_epoch,
						requested_stage_epoch: request.stage_epoch,
						requested_seconds_per_tick_bits: request.seconds_per_tick.to_bits(),
						active_stage: cursor.stage,
						active_frontier_epoch: cursor.frontier_epoch,
						active_stage_epoch: cursor.stage_epoch,
						active_seconds_per_tick_bits: cursor.seconds_per_tick_bits,
					},
				));
			}
			None => {
				if request.stage == WorldStage::React {
					self.gas_registry
						.as_ref()
						.ok_or(WorldError::GasRegistryMissing)?;
					self.reaction_registry
						.as_ref()
						.ok_or(WorldError::ReactionRegistryMissing)?;
				}
				let stage_components = if matches!(
					request.stage,
					WorldStage::Equalize | WorldStage::ExcitedGroups
				) {
					Some(StageComponentState::try_new(
						self.mixtures.len(),
						self.frontier.committed().len().min(self.mixtures.len()),
					)?)
				} else {
					None
				};
				let diffusion_specific_heats = self
					.gas_registry
					.as_ref()
					.map(|registry| {
						let mut values = [0.0; MAX_GAS_SLOTS];
						values[..registry.specific_heats().len()]
							.copy_from_slice(registry.specific_heats());
						values
					})
					.unwrap_or([0.0; MAX_GAS_SLOTS]);
				self.stage_cursor = Some(StageCursor::new(request, self.topology.revision()));
				self.stage_diffusion = (request.stage == WorldStage::ProcessTurfs)
					.then(|| StageDiffusionState::new(diffusion_specific_heats));
				self.stage_heat = (request.stage == WorldStage::TurfHeat).then(StageHeatState::new);
				self.stage_reactions = (request.stage == WorldStage::React).then(|| {
					let active_continuations = self
						.continuations
						.iter()
						.filter_map(|slot| {
							slot.continuation
								.as_ref()
								.map(|continuation| continuation.mixture)
						})
						.collect();
					StageReactionState {
						targets: Vec::new(),
						active_continuations,
						seen_mixtures: BTreeSet::new(),
						staged: BTreeMap::new(),
						staged_events: Vec::new(),
						pending: None,
						next_target: 0,
					}
				});
				self.stage_components = stage_components;
			}
			Some(_) => {}
		}

		let preparation_len = u32::try_from(self.frontier.committed().len()).unwrap_or(u32::MAX);
		if let Some(cursor) = self
			.stage_cursor
			.as_ref()
			.filter(|cursor| cursor.topology_revision != self.topology.revision())
		{
			return Err(WorldError::StageConflict(
				StageConflictReason::TopologyRevision {
					captured: cursor.topology_revision,
					current: self.topology.revision(),
				},
			));
		}
		let mut work_items = 0;
		while self
			.stage_cursor
			.as_ref()
			.is_some_and(|cursor| cursor.next_frontier_index < preparation_len)
			&& work_items < request.work_limit
		{
			if should_cancel() {
				return Err(WorldError::Cancelled);
			}
			let index = self
				.stage_cursor
				.as_ref()
				.expect("stage cursor was created")
				.next_frontier_index;
			if request.stage == WorldStage::TurfHeat {
				let handle = self.frontier.committed()[index as usize];
				self.prepare_stage_heat_turf(handle, false)?;
			} else if request.stage == WorldStage::ProcessTurfs {
				let handle = self.frontier.committed()[index as usize];
				self.prepare_stage_diffusion_turf(handle)?;
			} else if request.stage == WorldStage::React {
				let handle = self.frontier.committed()[index as usize];
				self.prepare_stage_reaction_turf(handle);
			} else if matches!(
				request.stage,
				WorldStage::Equalize | WorldStage::ExcitedGroups
			) {
				let handle = self.frontier.committed()[index as usize];
				self.prepare_stage_component_turf(handle);
			}
			self.stage_cursor
				.as_mut()
				.expect("stage cursor was created")
				.next_frontier_index += 1;
			work_items += 1;
		}
		let remaining_estimate = preparation_len.saturating_sub(
			self.stage_cursor
				.as_ref()
				.expect("stage cursor was created")
				.next_frontier_index,
		);
		if remaining_estimate != 0 {
			return Ok(StageChunkResult {
				work_items,
				pending: true,
				remaining_estimate,
				..StageChunkResult::default()
			});
		}
		if request.stage == WorldStage::TurfHeat {
			while self
				.stage_heat
				.as_ref()
				.is_some_and(|state| state.next_active_seed < self.heat_active.len())
				&& work_items < request.work_limit
			{
				if should_cancel() {
					return Err(WorldError::Cancelled);
				}
				let next_active_seed = self
					.stage_heat
					.as_ref()
					.expect("turf-heat stage owns heat state")
					.next_active_seed;
				let handle = self.heat_active[next_active_seed];
				self.prepare_stage_heat_turf(handle, true)?;
				self.stage_heat
					.as_mut()
					.expect("turf-heat stage owns heat state")
					.next_active_seed += 1;
				work_items += 1;
			}
			let remaining_active = self
				.stage_heat
				.as_ref()
				.map(|state| {
					self.heat_active
						.len()
						.saturating_sub(state.next_active_seed)
				})
				.unwrap_or_default();
			if remaining_active != 0 {
				return Ok(StageChunkResult {
					work_items,
					pending: true,
					remaining_estimate: u32::try_from(remaining_active).unwrap_or(u32::MAX),
					..StageChunkResult::default()
				});
			}
		}
		if request.stage == WorldStage::ProcessTurfs {
			while work_items < request.work_limit {
				let Some(next_node) = self
					.stage_diffusion
					.as_ref()
					.filter(|state| state.next_node < state.turfs.len())
					.map(|state| state.next_node)
				else {
					break;
				};
				if should_cancel() {
					return Err(WorldError::Cancelled);
				}
				self.compute_stage_diffusion_node(next_node)?;
				self.stage_diffusion
					.as_mut()
					.expect("process-turfs stage owns diffusion state")
					.next_node += 1;
				work_items += 1;
			}
			let remaining_nodes = self
				.stage_diffusion
				.as_ref()
				.map(|state| state.turfs.len().saturating_sub(state.next_node))
				.unwrap_or_default();
			if remaining_nodes != 0 {
				return Ok(StageChunkResult {
					work_items,
					pending: true,
					remaining_estimate: u32::try_from(remaining_nodes).unwrap_or(u32::MAX),
					..StageChunkResult::default()
				});
			}
			// commit_stage_diffusion()'s return is a count of committed FDM turf mixtures, not an
			// equalize count - it used to be misreported here as produced_equalize_seeds, which
			// would have collided with the real count now returned by the Equalize stage below.
			self.commit_stage_diffusion()?;
			self.stage_cursor = None;
			return Ok(StageChunkResult {
				work_items,
				pending: false,
				remaining_estimate: 0,
				..StageChunkResult::default()
			});
		}
		if request.stage == WorldStage::TurfHeat {
			while work_items < request.work_limit {
				let Some(next_node) = self
					.stage_heat
					.as_ref()
					.filter(|state| state.next_node < state.nodes.len())
					.map(|state| state.next_node)
				else {
					break;
				};
				if should_cancel() {
					return Err(WorldError::Cancelled);
				}
				self.compute_stage_heat_node(next_node, request.seconds_per_tick as f32)?;
				self.stage_heat
					.as_mut()
					.expect("turf-heat stage owns heat state")
					.next_node += 1;
				work_items += 1;
			}
			let remaining_nodes = self
				.stage_heat
				.as_ref()
				.map(|state| state.nodes.len().saturating_sub(state.next_node))
				.unwrap_or_default();
			if remaining_nodes != 0 {
				return Ok(StageChunkResult {
					work_items,
					pending: true,
					remaining_estimate: u32::try_from(remaining_nodes).unwrap_or(u32::MAX),
					..StageChunkResult::default()
				});
			}
			while work_items < request.work_limit && self.advance_stage_heat_topology()? {
				if should_cancel() {
					return Err(WorldError::Cancelled);
				}
				work_items += 1;
			}
			if !self.stage_heat_topology_complete() {
				return Ok(StageChunkResult {
					work_items,
					pending: true,
					remaining_estimate: 1,
					..StageChunkResult::default()
				});
			}
			self.prepare_stage_heat_conduction(request.seconds_per_tick as f32)?;
			while work_items < request.work_limit && self.advance_stage_heat_conduction()? {
				if should_cancel() {
					return Err(WorldError::Cancelled);
				}
				work_items += 1;
			}
			if !self.stage_heat_conduction_complete() {
				return Ok(StageChunkResult {
					work_items,
					pending: true,
					remaining_estimate: 1,
					..StageChunkResult::default()
				});
			}
			let (completed, callback_events) = self.commit_stage_heat(event_capacity)?;
			self.stage_cursor = None;
			return Ok(StageChunkResult {
				work_items,
				callback_events,
				pending: false,
				remaining_estimate: 0,
				produced_heat_seeds: completed,
				..StageChunkResult::default()
			});
		}
		if request.stage == WorldStage::React {
			while work_items < request.work_limit {
				let Some(next_target) = self
					.stage_reactions
					.as_ref()
					.filter(|state| {
						state.pending.is_none() && state.next_target < state.targets.len()
					})
					.map(|state| state.next_target)
				else {
					break;
				};
				if should_cancel() {
					return Err(WorldError::Cancelled);
				}
				self.compute_stage_reaction_target(next_target)?;
				self.stage_reactions
					.as_mut()
					.expect("reaction stage owns reaction state")
					.next_target += 1;
				work_items += 1;
			}
			let remaining_targets = self
				.stage_reactions
				.as_ref()
				.map(|state| {
					if state.pending.is_some() {
						0
					} else {
						state.targets.len().saturating_sub(state.next_target)
					}
				})
				.unwrap_or_default();
			if remaining_targets != 0 {
				return Ok(StageChunkResult {
					work_items,
					pending: true,
					remaining_estimate: u32::try_from(remaining_targets).unwrap_or(u32::MAX),
					..StageChunkResult::default()
				});
			}
			let (_, callback_events) = self.commit_stage_reactions(event_capacity)?;
			self.stage_cursor = None;
			return Ok(StageChunkResult {
				work_items,
				callback_events,
				pending: false,
				remaining_estimate: 0,
				..StageChunkResult::default()
			});
		}
		if matches!(
			request.stage,
			WorldStage::Equalize | WorldStage::ExcitedGroups
		) {
			while work_items < request.work_limit {
				if self
					.stage_components
					.as_ref()
					.is_some_and(|state| state.component_ready)
				{
					if should_cancel() {
						return Err(WorldError::Cancelled);
					}
					self.process_ready_stage_component(
						request.stage,
						event_capacity,
						&mut should_cancel,
					)?;
					work_items += 1;
					continue;
				}
				if !self.advance_stage_component_discovery() {
					break;
				}
				work_items += 1;
			}
			if !self.stage_component_discovery_complete() {
				return Ok(StageChunkResult {
					work_items,
					callback_events: 0,
					pending: true,
					remaining_estimate: 1,
					..StageChunkResult::default()
				});
			}
			let (callback_events, components_processed) = self.finish_stage_components();
			self.stage_cursor = None;
			return Ok(StageChunkResult {
				work_items,
				callback_events,
				pending: false,
				remaining_estimate: 0,
				produced_equalize_seeds: if request.stage == WorldStage::Equalize {
					components_processed
				} else {
					0
				},
				produced_group_seeds: if request.stage == WorldStage::ExcitedGroups {
					components_processed
				} else {
					0
				},
				..StageChunkResult::default()
			});
		}
		unreachable!("every simulation stage has a dedicated resumable cursor")
	}

	fn prepare_stage_diffusion_turf(&mut self, turf_handle: TurfHandle) -> Result<(), WorldError> {
		let neighbors = self
			.topology
			.gas_neighbors(turf_handle)
			.map(|neighbor| neighbor.handle)
			.collect::<Vec<_>>();
		self.append_stage_diffusion_turf(turf_handle)?;
		for neighbor in neighbors {
			self.append_stage_diffusion_turf(neighbor)?;
		}
		Ok(())
	}

	fn append_stage_diffusion_turf(&mut self, turf_handle: TurfHandle) -> Result<(), WorldError> {
		if self
			.stage_diffusion
			.as_ref()
			.expect("process-turfs stage owns diffusion state")
			.index_by_turf
			.contains_key(&turf_handle)
		{
			return Ok(());
		}
		let Ok(turf) = self.require_turf_handle(turf_handle) else {
			return Ok(());
		};
		let Some(mixture_handle) = turf.mixture else {
			return Ok(());
		};
		let mixture = self.require_handle(mixture_handle)?;
		let immutable = mixture.immutable;
		let revision = mixture.revision;
		let gases = mixture.gases;
		let temperature = mixture.temperature;
		let minimum_heat_capacity = mixture.minimum_heat_capacity;
		let state = self
			.stage_diffusion
			.as_mut()
			.expect("process-turfs stage owns diffusion state");
		if !immutable && !state.seen_mixtures.insert(mixture_handle) {
			return Err(WorldError::DuplicateMutableTurfMixture(mixture_handle));
		}
		if !immutable && revision == u32::MAX {
			return Err(WorldError::RevisionExhausted(mixture_handle));
		}
		let index = state.turfs.len();
		state.turfs.push(turf_handle);
		state.mixtures.push(mixture_handle);
		state.index_by_turf.insert(turf_handle, index);
		state.input.push(gases);
		state.output.push([0.0; MAX_GAS_SLOTS]);
		state.input_temperatures.push(temperature);
		state.minimum_heat_capacities.push(minimum_heat_capacity);
		let heat_capacity = gases
			.iter()
			.zip(state.specific_heats)
			.fold(0.0, |capacity, (amount, specific_heat)| {
				specific_heat.mul_add(*amount, capacity)
			})
			.max(minimum_heat_capacity);
		state.input_energy.push(heat_capacity * temperature);
		state.output_energy.push(0.0);
		Ok(())
	}

	fn compute_stage_diffusion_node(&mut self, index: usize) -> Result<(), WorldError> {
		let state = self
			.stage_diffusion
			.as_ref()
			.expect("process-turfs stage owns diffusion state");
		let turf = state.turfs[index];
		let mut neighbors = [0_usize; MAX_TURF_NEIGHBORS];
		let mut neighbor_count = 0;
		for neighbor in self.topology.gas_neighbors(turf) {
			if let Some(index) = state.index_by_turf.get(&neighbor.handle).copied() {
				neighbors[neighbor_count] = index;
				neighbor_count += 1;
			}
		}
		let self_weight = diffusion_self_weight(neighbor_count as u32)
			.map_err(|error| WorldError::State(error.to_string()))?;
		let mut output = [0.0; MAX_GAS_SLOTS];
		for (gas_index, output_value) in output.iter_mut().enumerate() {
			let mut next_value = state.input[index][gas_index] * self_weight;
			for neighbor in &neighbors[..neighbor_count] {
				next_value += state.input[*neighbor][gas_index] * GAS_DIFFUSION_CONSTANT;
			}
			*output_value = next_value;
		}
		let mut output_energy = state.input_energy[index] * self_weight;
		for neighbor in &neighbors[..neighbor_count] {
			output_energy += state.input_energy[*neighbor] * GAS_DIFFUSION_CONSTANT;
		}
		let state = self
			.stage_diffusion
			.as_mut()
			.expect("process-turfs stage owns diffusion state");
		state.output[index] = output;
		state.output_energy[index] = output_energy;
		Ok(())
	}

	fn commit_stage_diffusion(&mut self) -> Result<u32, WorldError> {
		let state = self
			.stage_diffusion
			.take()
			.expect("process-turfs stage owns diffusion state");
		let work_items = u32::try_from(state.mixtures.len())
			.map_err(|_| WorldError::State("turf count exceeds u32".into()))?;
		for index in 0..state.mixtures.len() {
			let handle = state.mixtures[index];
			let mixture = self.require_handle_mut(handle)?;
			if mixture.immutable {
				continue;
			}
			let gases = state.output[index];
			let heat_capacity = gases
				.iter()
				.zip(state.specific_heats)
				.fold(0.0, |capacity, (amount, specific_heat)| {
					specific_heat.mul_add(*amount, capacity)
				})
				.max(state.minimum_heat_capacities[index]);
			mixture.gases = gases;
			mixture.temperature = if heat_capacity > MINIMUM_HEAT_CAPACITY {
				(state.output_energy[index] / heat_capacity).max(MINIMUM_TEMPERATURE_K)
			} else {
				state.input_temperatures[index]
			};
			mixture.revision += 1;
		}
		Ok(work_items)
	}

	fn prepare_stage_heat_turf(
		&mut self,
		turf_handle: TurfHandle,
		can_continue: bool,
	) -> Result<(), WorldError> {
		let neighbors = self
			.topology
			.heat_neighbors(turf_handle)
			.map(|neighbor| neighbor.handle)
			.collect::<Vec<_>>();
		self.append_stage_heat_turf(turf_handle, can_continue)?;
		for neighbor in neighbors {
			self.append_stage_heat_turf(neighbor, true)?;
		}
		Ok(())
	}

	fn append_stage_heat_turf(
		&mut self,
		turf_handle: TurfHandle,
		can_continue: bool,
	) -> Result<(), WorldError> {
		let Ok(turf) = self.require_turf_handle(turf_handle) else {
			return Ok(());
		};
		let Some(heat) = turf.heat else {
			return Ok(());
		};
		let mixture = turf.mixture;
		let state = self
			.stage_heat
			.as_mut()
			.expect("turf-heat stage owns heat state");
		if let Some(&index) = state.index_by_slot.get(&turf_handle.slot) {
			state.nodes[index as usize].can_continue |= can_continue;
			return Ok(());
		}
		let index = u32::try_from(state.nodes.len())
			.map_err(|_| WorldError::State("turf heat count exceeds u32".into()))?;
		state.nodes.push(StageHeatNode {
			handle: turf_handle,
			heat,
			mixture,
			can_continue,
		});
		state.index_by_slot.insert(turf_handle.slot, index);
		state.temperatures.push(heat.temperature);
		state.conductivities.push(heat.thermal_conductivity);
		state.heat_capacities.push(heat.heat_capacity);
		state.row_sums.push(0.0);
		Ok(())
	}

	fn compute_stage_heat_node(
		&mut self,
		index: usize,
		seconds_per_tick: f32,
	) -> Result<(), WorldError> {
		let (turf, heat_state, mixture_handle, mut temperature) = {
			let state = self
				.stage_heat
				.as_ref()
				.expect("turf-heat stage owns heat state");
			let node = state.nodes[index];
			(
				node.handle,
				node.heat,
				node.mixture,
				state.temperatures[index],
			)
		};
		let elapsed_heat_scale =
			seconds_per_tick / crate::numerics::conduction::BASE_HEAT_STEP_SECONDS;
		if heat_state.adjacent_to_space && temperature > 273.15 {
			if self.realistic_space_radiation {
				let emitted = STEFAN_BOLTZMANN_CONSTANT
					* f64::from(seconds_per_tick)
					* f64::from(temperature).powi(4);
				let received = RADIATION_FROM_SPACE * f64::from(seconds_per_tick);
				temperature = (f64::from(temperature)
					- (emitted - received) / f64::from(heat_state.heat_capacity))
				.max(f64::from(MINIMUM_TEMPERATURE_K)) as f32;
			} else if temperature > 293.15 {
				let heat = heat_exchange_energy(
					heat_state.thermal_conductivity
						* elapsed_heat_scale
						* (temperature - MINIMUM_TEMPERATURE_K),
					7000.0,
					heat_state.heat_capacity,
				);
				temperature =
					(temperature - heat / heat_state.heat_capacity).max(MINIMUM_TEMPERATURE_K);
			}
		}

		if let Some(mixture_handle) = mixture_handle {
			if !self
				.stage_heat
				.as_mut()
				.expect("turf-heat stage owns heat state")
				.linked_mixtures
				.insert(mixture_handle)
			{
				return Err(WorldError::DuplicateMutableTurfMixture(mixture_handle));
			}
			let specific_heats = self
				.gas_registry
				.as_ref()
				.ok_or(WorldError::GasRegistryMissing)?
				.specific_heats();
			let mut mixture = self.require_handle(mixture_handle)?.clone();
			if !mixture.immutable {
				let gas_capacity = record_heat_capacity(&mixture, specific_heats);
				let temperature_delta = mixture.temperature - temperature;
				if (temperature > MINIMUM_TEMPERATURE_FOR_SUPERCONDUCTION_K
					|| mixture.temperature >= MINIMUM_TEMPERATURE_FOR_SUPERCONDUCTION_K)
					&& temperature_delta.abs() > MINIMUM_TEMPERATURE_DELTA_TO_CONSIDER
					&& gas_capacity > MINIMUM_HEAT_CAPACITY
				{
					if mixture.revision == u32::MAX {
						return Err(WorldError::RevisionExhausted(mixture_handle));
					}
					let heat = heat_state.thermal_conductivity
						* OPEN_HEAT_TRANSFER_COEFFICIENT
						* elapsed_heat_scale
						* temperature_delta
						* harmonic_heat_capacity(gas_capacity, heat_state.heat_capacity);
					mixture.temperature =
						(mixture.temperature - heat / gas_capacity).max(MINIMUM_TEMPERATURE_K);
					temperature =
						(temperature + heat / heat_state.heat_capacity).max(MINIMUM_TEMPERATURE_K);
					self.stage_heat
						.as_mut()
						.expect("turf-heat stage owns heat state")
						.staged_mixtures
						.insert(mixture_handle, mixture);
				}
			}
		}

		let state = self
			.stage_heat
			.as_mut()
			.expect("turf-heat stage owns heat state");
		state.temperatures[index] = temperature;
		if temperature > MINIMUM_TEMPERATURE_START_SUPERCONDUCTION_K
			&& temperature > heat_state.heat_capacity
		{
			state
				.staged_events
				.push(WorldEvent::TurfDestructionRequest { turf });
		}
		Ok(())
	}

	fn advance_stage_heat_topology(&mut self) -> Result<bool, WorldError> {
		loop {
			let Some((turf, neighbor_index)) = self.stage_heat.as_ref().and_then(|state| {
				state
					.nodes
					.get(state.next_topology_node)
					.map(|node| (node.handle, state.next_topology_neighbor))
			}) else {
				return Ok(false);
			};
			// nth() walks the same fixed-size (≤6-entry) neighbor iterator .collect() would have,
			// but without allocating a Vec every single call - this runs once per neighbor step,
			// so a degree-4 turf was allocating and filling the same 4-element vector 5 times.
			let Some(neighbor) = self.topology.heat_neighbors(turf).nth(neighbor_index) else {
				let state = self
					.stage_heat
					.as_mut()
					.expect("turf-heat stage owns heat state");
				state.next_topology_node += 1;
				state.next_topology_neighbor = 0;
				continue;
			};
			let neighbor = neighbor.handle;
			let state = self
				.stage_heat
				.as_mut()
				.expect("turf-heat stage owns heat state");
			state.next_topology_neighbor += 1;
			let Some(&second) = state.index_by_slot.get(&neighbor.slot) else {
				return Ok(true);
			};
			let first = state.index_by_slot[&turf.slot];
			if first >= second {
				return Ok(true);
			}
			let first_index = first as usize;
			let second_index = second as usize;
			// Conductivities and heat capacities don't change across substeps, so these two
			// weights are loop-invariant with respect to advance_stage_heat_conduction()'s
			// per-substep loop - compute them once here (where row_sums already needs them) and
			// carry them on the edge instead of recomputing from scratch every substep.
			let first_weight = crate::numerics::conduction::heat_row_weight(
				state.conductivities[first_index],
				state.conductivities[second_index],
				state.heat_capacities[first_index],
				state.heat_capacities[second_index],
			)
			.map_err(|error| WorldError::State(error.to_string()))?;
			let second_weight = crate::numerics::conduction::heat_row_weight(
				state.conductivities[second_index],
				state.conductivities[first_index],
				state.heat_capacities[second_index],
				state.heat_capacities[first_index],
			)
			.map_err(|error| WorldError::State(error.to_string()))?;
			state.row_sums[first_index] += first_weight;
			state.row_sums[second_index] += second_weight;
			state
				.edges
				.push((first, second, first_weight, second_weight));
			return Ok(true);
		}
	}

	fn stage_heat_topology_complete(&self) -> bool {
		self.stage_heat
			.as_ref()
			.is_some_and(|state| state.next_topology_node >= state.nodes.len())
	}

	fn prepare_stage_heat_conduction(&mut self, seconds_per_tick: f32) -> Result<(), WorldError> {
		let state = self
			.stage_heat
			.as_mut()
			.expect("turf-heat stage owns heat state");
		if state.conduction_substeps.is_some() {
			return Ok(());
		}
		let elapsed_scale = seconds_per_tick / crate::numerics::conduction::BASE_HEAT_STEP_SECONDS;
		let maximum_scaled_sum =
			state.row_sums.iter().copied().fold(0.0_f32, f32::max) * elapsed_scale;
		if !maximum_scaled_sum.is_finite()
			|| maximum_scaled_sum > crate::numerics::conduction::MAX_CONDUCTION_SUBSTEPS as f32
		{
			return Err(WorldError::State(
				crate::numerics::conduction::ConductionError::TooManySubsteps.to_string(),
			));
		}
		let substeps = (maximum_scaled_sum.ceil() as u32).max(1);
		state.conduction_substeps = Some(substeps);
		state.conduction_scale = elapsed_scale / substeps as f32;
		Ok(())
	}

	fn advance_stage_heat_conduction(&mut self) -> Result<bool, WorldError> {
		if self.stage_heat_conduction_complete() {
			return Ok(false);
		}
		let state = self
			.stage_heat
			.as_mut()
			.expect("turf-heat stage owns heat state");
		if state.edges.is_empty() {
			state.conduction_substep = state.conduction_substeps.unwrap_or(1);
			return Ok(false);
		}
		let (first, second, first_weight, second_weight) = state.edges[state.conduction_edge];
		let first_index = first as usize;
		let second_index = second as usize;
		let difference = state.temperatures[second_index] - state.temperatures[first_index];
		let first_weight = first_weight * state.conduction_scale;
		let second_weight = second_weight * state.conduction_scale;
		state.temperatures[first_index] += difference * first_weight;
		state.temperatures[second_index] -= difference * second_weight;
		state.conduction_edge += 1;
		if state.conduction_edge == state.edges.len() {
			state.conduction_edge = 0;
			state.conduction_substep += 1;
		}
		Ok(true)
	}

	fn stage_heat_conduction_complete(&self) -> bool {
		self.stage_heat.as_ref().is_some_and(|state| {
			state.conduction_substeps.is_some_and(|substeps| {
				state.edges.is_empty() || state.conduction_substep >= substeps
			})
		})
	}

	fn commit_stage_heat(&mut self, event_capacity: usize) -> Result<(u32, u32), WorldError> {
		let state = self
			.stage_heat
			.take()
			.expect("turf-heat stage owns heat state");
		let requested_events = self.events.len().saturating_add(state.staged_events.len());
		if requested_events > event_capacity {
			self.stage_heat = Some(state);
			return Err(WorldError::EventCapacityExceeded {
				requested: u32::try_from(requested_events).unwrap_or(u32::MAX),
				capacity: u32::try_from(event_capacity).unwrap_or(u32::MAX),
			});
		}
		if self.heat_active.try_reserve(state.nodes.len()).is_err() {
			self.stage_heat = Some(state);
			return Err(WorldError::AllocationFailed);
		}
		let completed = u32::try_from(state.nodes.len())
			.map_err(|_| WorldError::State("turf heat count exceeds u32".into()))?;
		let callback_events = u32::try_from(state.staged_events.len()).unwrap_or(u32::MAX);
		for (handle, mut mixture) in state.staged_mixtures {
			let current = self.require_handle_mut(handle)?;
			mixture.revision = current.revision + 1;
			*current = mixture;
		}
		for (node, temperature) in state.nodes.into_iter().zip(state.temperatures) {
			self.require_turf_handle_mut(node.handle)?
				.heat
				.as_mut()
				.expect("turf heat state was validated")
				.temperature = temperature;
			let was_active = self
				.require_turf_handle(node.handle)?
				.heat_active_index
				.is_some();
			let activation_threshold = if was_active || node.can_continue {
				MINIMUM_TEMPERATURE_FOR_SUPERCONDUCTION_K
			} else {
				MINIMUM_TEMPERATURE_START_SUPERCONDUCTION_K
			};
			let activation_temperature =
				self.turf_superconduction_temperature(node.handle, temperature)?;
			if activation_temperature >= activation_threshold {
				self.activate_turf_heat(node.handle)?;
			} else {
				self.deactivate_turf_heat_slot(node.handle.slot);
			}
		}
		self.events.extend(state.staged_events);
		Ok((completed, callback_events))
	}

	fn prepare_stage_reaction_turf(&mut self, turf_handle: TurfHandle) {
		let Some(mixture) = self
			.require_turf_handle(turf_handle)
			.ok()
			.and_then(|turf| turf.mixture)
		else {
			return;
		};
		self.stage_reactions
			.as_mut()
			.expect("reaction stage owns reaction state")
			.targets
			.push((turf_handle, mixture));
	}

	fn compute_stage_reaction_target(&mut self, index: usize) -> Result<(), WorldError> {
		let (turf, mixture, should_process) = {
			let state = self
				.stage_reactions
				.as_mut()
				.expect("reaction stage owns reaction state");
			let (turf, mixture) = state.targets[index];
			let should_process = state.seen_mixtures.insert(mixture)
				&& !state.active_continuations.contains(&mixture);
			(turf, mixture, should_process)
		};
		if !should_process {
			return Ok(());
		}
		let sequence = self.evaluate_reaction_sequence(turf.into(), mixture, 0, 0, None)?;
		let state = self
			.stage_reactions
			.as_mut()
			.expect("reaction stage owns reaction state");
		state.staged_events.extend(sequence.events);
		if sequence.native_updates > 0 {
			state.staged.insert(mixture, sequence.mixture);
		}
		if let Some(dm_reaction) = sequence.pending {
			state.pending = Some((turf, mixture, dm_reaction));
		}
		Ok(())
	}

	fn commit_stage_reactions(&mut self, event_capacity: usize) -> Result<(u32, u32), WorldError> {
		let state = self
			.stage_reactions
			.take()
			.expect("reaction stage owns reaction state");
		let requested_events = self
			.events
			.len()
			.saturating_add(state.staged_events.len())
			.saturating_add(usize::from(state.pending.is_some()));
		if requested_events > event_capacity {
			self.stage_reactions = Some(state);
			return Err(WorldError::EventCapacityExceeded {
				requested: u32::try_from(requested_events).unwrap_or(u32::MAX),
				capacity: u32::try_from(event_capacity).unwrap_or(u32::MAX),
			});
		}
		let continuation_event = if let Some((turf, mixture, pending)) = state.pending {
			let token = self.allocate_continuation(ReactionContinuation {
				turf: Some(turf),
				mixture,
				target: turf.into(),
				next_reaction_index: pending.next_reaction_index,
				reaction_profile_threshold_ms: None,
			})?;
			Some(WorldEvent::RunDmReaction {
				turf: Some(turf),
				mixture,
				target: turf.into(),
				reaction: pending.reaction,
				continuation: token,
			})
		} else {
			None
		};
		for (handle, mut record) in state.staged {
			let current = self.require_handle_mut(handle)?;
			record.revision = current.revision + 1;
			*current = record;
		}
		let callback_events = state
			.staged_events
			.len()
			.saturating_add(usize::from(continuation_event.is_some()));
		self.events.extend(state.staged_events);
		if let Some(event) = continuation_event {
			self.events.push(event);
		}
		Ok((
			u32::try_from(state.targets.len()).unwrap_or(u32::MAX),
			u32::try_from(callback_events).unwrap_or(u32::MAX),
		))
	}

	fn prepare_stage_component_turf(&mut self, turf_handle: TurfHandle) {
		if self
			.require_turf_handle(turf_handle)
			.ok()
			.is_none_or(|turf| turf.mixture.is_none())
		{
			return;
		}
		let state = self
			.stage_components
			.as_mut()
			.expect("component stage owns traversal state");
		state.targets.push(turf_handle);
		state.active_by_slot.insert(turf_handle.slot, turf_handle);
	}

	fn advance_stage_component_discovery(&mut self) -> bool {
		loop {
			let state = self
				.stage_components
				.as_ref()
				.expect("component stage owns traversal state");
			if let Some(current) = state.queue.get(state.queue_index).copied() {
				// See advance_stage_heat_topology()'s identical fix: nth() walks the same fixed
				// (≤6-entry) neighbor iterator .collect() would have, without allocating a Vec
				// every single call.
				let Some(neighbor) = self
					.topology
					.gas_neighbors(current)
					.nth(state.next_neighbor)
				else {
					let state = self
						.stage_components
						.as_mut()
						.expect("component stage owns traversal state");
					state.queue_index += 1;
					state.next_neighbor = 0;
					continue;
				};
				let neighbor = neighbor.handle;
				let state = self
					.stage_components
					.as_mut()
					.expect("component stage owns traversal state");
				state.next_neighbor += 1;
				if state.active_by_slot.get(&neighbor.slot) == Some(&neighbor)
					&& state.visited.insert(neighbor.slot)
				{
					state.queue.push(neighbor);
				}
				return true;
			}
			if !state.queue.is_empty() {
				self.stage_components
					.as_mut()
					.expect("component stage owns traversal state")
					.component_ready = true;
				return false;
			}
			if state.next_seed >= state.targets.len() {
				return false;
			}
			let seed = state.targets[state.next_seed];
			let state = self
				.stage_components
				.as_mut()
				.expect("component stage owns traversal state");
			state.next_seed += 1;
			if state.visited.insert(seed.slot) {
				state.queue.push(seed);
			}
			return true;
		}
	}

	fn process_ready_stage_component(
		&mut self,
		stage: WorldStage,
		event_capacity: usize,
		should_cancel: &mut impl FnMut() -> bool,
	) -> Result<(), WorldError> {
		let mut state = self
			.stage_components
			.take()
			.expect("component stage owns traversal state");
		let checkpoint = state.transaction.checkpoint();
		let event_checkpoint = state.staged_events.len();
		self.stage_component_turfs = Some(std::mem::take(&mut state.queue));
		self.use_committed_frontier = true;
		let result = match stage {
			WorldStage::Equalize => self.compute_equalize(
				&mut state.transaction,
				&mut state.staged_events,
				should_cancel,
			),
			WorldStage::ExcitedGroups => {
				self.compute_excited_groups(&mut state.transaction, should_cancel)
			}
			_ => unreachable!("only component stages use component traversal"),
		};
		self.use_committed_frontier = false;
		state.queue = self
			.stage_component_turfs
			.take()
			.expect("component stage owns its turf queue");
		if let Err(error) = result {
			state.transaction.rollback_to(checkpoint);
			state.staged_events.truncate(event_checkpoint);
			self.stage_components = Some(state);
			return Err(error);
		}
		if let Some(handle) = state
			.transaction
			.entries()
			.iter()
			.map(|entry| entry.handle)
			.find(|handle| state.mixture_was_published(*handle))
		{
			state.transaction.clear();
			state.staged_events.clear();
			self.stage_components = Some(state);
			return Err(WorldError::DuplicateMutableTurfMixture(handle));
		}
		if let Err(error) = self.validate_indexed_transaction(
			&state.transaction,
			state.staged_events.len(),
			event_capacity,
		) {
			state.transaction.clear();
			state.staged_events.clear();
			self.stage_components = Some(state);
			return Err(error);
		}
		state.callback_events = state
			.callback_events
			.saturating_add(u32::try_from(state.staged_events.len()).unwrap_or(u32::MAX));
		for index in 0..state.transaction.entries().len() {
			state.mark_mixture_published(state.transaction.entries()[index].handle);
		}
		self.publish_indexed_transaction_reusing(&mut state.transaction, &mut state.staged_events);
		state.queue.clear();
		state.queue_index = 0;
		state.next_neighbor = 0;
		state.component_ready = false;
		state.components_processed += 1;
		self.stage_components = Some(state);
		Ok(())
	}

	/// Returns (callback_events, components_processed) - the latter is the number of components
	/// process_ready_stage_component() actually ran this stage, which is what
	/// produced_equalize_seeds/produced_group_seeds report back over the wire. Before this,
	/// the Equalize/ExcitedGroups branch of process_stage_chunk_cancellable() always returned
	/// both of those fields as 0 regardless of how much real work ran - DM-side telemetry (and
	/// the dogmos_excited_groups unit test) had nothing but a permanent zero to read.
	fn finish_stage_components(&mut self) -> (u32, u32) {
		let state = self
			.stage_components
			.take()
			.expect("component stage owns traversal state");
		(state.callback_events, state.components_processed)
	}

	fn validate_indexed_transaction(
		&self,
		transaction: &IndexedTransaction<MixtureRecord>,
		staged_event_count: usize,
		event_capacity: usize,
	) -> Result<(), WorldError> {
		let requested_events = self.events.len().saturating_add(staged_event_count);
		if requested_events > event_capacity {
			return Err(WorldError::EventCapacityExceeded {
				requested: u32::try_from(requested_events).unwrap_or(u32::MAX),
				capacity: u32::try_from(event_capacity).unwrap_or(u32::MAX),
			});
		}
		for entry in transaction.entries() {
			let current = self.require_handle(entry.handle).map_err(|_| {
				WorldError::StageConflict(StageConflictReason::TransactionHandleMissing {
					handle: entry.handle,
				})
			})?;
			if current.revision != entry.expected_revision {
				return Err(WorldError::StageConflict(
					StageConflictReason::TransactionRevision {
						handle: entry.handle,
						expected: entry.expected_revision,
						actual: current.revision,
					},
				));
			}
		}
		Ok(())
	}

	fn publish_indexed_transaction_reusing(
		&mut self,
		transaction: &mut IndexedTransaction<MixtureRecord>,
		staged_events: &mut Vec<WorldEvent>,
	) {
		transaction.sort_by_handle();
		for entry in transaction.drain_entries() {
			let current = self
				.require_handle_mut(entry.handle)
				.expect("transaction handles were validated before commit");
			if current.gases == entry.candidate.gases
				&& current.temperature == entry.candidate.temperature
			{
				continue;
			}
			let mut candidate = entry.candidate;
			candidate.revision = current.revision + 1;
			*current = candidate;
		}
		self.events.append(staged_events);
	}

	fn stage_component_discovery_complete(&self) -> bool {
		self.stage_components.as_ref().is_some_and(|state| {
			state.next_seed >= state.targets.len()
				&& state.queue.is_empty()
				&& !state.component_ready
		})
	}

	#[cfg(debug_assertions)]
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

	#[cfg(debug_assertions)]
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

	#[cfg(debug_assertions)]
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
		let active_turfs = self.stage_turf_handles();
		let has_turf_state = !active_turfs.is_empty();
		let turf_handles = active_turfs
			.iter()
			.filter_map(|handle| self.require_turf_handle(*handle).ok()?.mixture)
			.collect::<Vec<_>>();
		if has_turf_state {
			let frontier_graph = self
				.use_committed_frontier
				.then(|| self.build_turf_graph_for_handles(&active_turfs))
				.transpose()?;
			return self.process_turf_diffusion(turf_handles, frontier_graph, &mut should_cancel);
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

	#[cfg(debug_assertions)]
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
			.stage_turf_handles()
			.iter()
			.copied()
			.filter_map(|handle| Some((handle, self.require_turf_handle(handle).ok()?.mixture?)))
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
			let sequence = self.evaluate_reaction_sequence(turf.into(), mixture, 0, 0, None)?;
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
				turf: Some(turf),
				mixture,
				target: turf.into(),
				next_reaction_index: pending.next_reaction_index,
				reaction_profile_threshold_ms: None,
			})?;
			Some(WorldEvent::RunDmReaction {
				turf: Some(turf),
				mixture,
				target: turf.into(),
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
		target: crate::metadata::GameplayHandle,
		mixture_handle: MixtureHandle,
		start_index: u32,
		initial_flags: u32,
		reaction_profile_threshold_ms: Option<f32>,
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
		let mut flags = initial_flags;
		if mixture.immutable {
			return Ok(ReactionSequence {
				mixture,
				events,
				pending: None,
				flags,
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
					let started = reaction_profile_threshold_ms.map(|_| Instant::now());
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
					flags |= REACTION_REACTING | REACTION_VOLATILE;
					events.push(WorldEvent::ReactionFinished {
						mixture: mixture_handle,
						target,
						reaction: *reaction_id,
						kind,
						values: result.values,
					});
					if let (Some(started), Some(threshold_ms)) =
						(started, reaction_profile_threshold_ms)
					{
						let cost_ms = started.elapsed().as_secs_f32() * 1000.0;
						if cost_ms >= threshold_ms {
							events.push(WorldEvent::ReactionProfiled {
								mixture: mixture_handle,
								target,
								reaction: *reaction_id,
								cost_ms,
							});
						}
					}
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
						flags,
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
			flags,
			work_items,
			native_updates,
		})
	}

	pub fn react_mixture_with_event_limit(
		&mut self,
		mixture: MixtureHandle,
		target: crate::metadata::GameplayHandle,
		reaction_profile_threshold_ms: Option<f32>,
		event_limit: u32,
	) -> Result<ReactionProgress, WorldError> {
		validate_reaction_profile_threshold(reaction_profile_threshold_ms)?;
		let sequence =
			self.evaluate_reaction_sequence(target, mixture, 0, 0, reaction_profile_threshold_ms)?;
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
			let token = self.allocate_continuation(ReactionContinuation {
				turf: None,
				mixture,
				target,
				next_reaction_index: pending.next_reaction_index,
				reaction_profile_threshold_ms,
			})?;
			Some(WorldEvent::RunDmReaction {
				turf: None,
				mixture,
				target,
				reaction: pending.reaction,
				continuation: token,
			})
		} else {
			None
		};
		if sequence.native_updates > 0 {
			let current = self.require_handle_mut(mixture)?;
			let mut updated = sequence.mixture;
			updated.revision = current.revision + 1;
			*current = updated;
		}
		let progress = ReactionProgress {
			flags: sequence.flags,
			work_items: sequence.work_items,
			pending: continuation_event.is_some(),
		};
		self.events.extend(sequence.events);
		if let Some(event) = continuation_event {
			self.events.push(event);
		}
		Ok(progress)
	}

	pub fn resume_reaction_with_result_and_event_limit(
		&mut self,
		token: ReactionContinuationToken,
		reaction_result: u32,
		event_limit: u32,
	) -> Result<ReactionProgress, WorldError> {
		self.resume_reaction_inner(token, reaction_result, event_limit)
			.map(|(progress, _)| progress)
	}

	pub fn resume_reaction_with_event_limit(
		&mut self,
		token: ReactionContinuationToken,
		event_limit: u32,
	) -> Result<u32, WorldError> {
		self.resume_reaction_inner(token, 0, event_limit)
			.map(|(_, native_updates)| native_updates)
	}

	fn resume_reaction_inner(
		&mut self,
		token: ReactionContinuationToken,
		reaction_result: u32,
		event_limit: u32,
	) -> Result<(ReactionProgress, u32), WorldError> {
		if reaction_result & !REACTION_FLAGS != 0 {
			return Err(WorldError::InvalidReactionResult(reaction_result));
		}
		let continuation = self.require_continuation(token)?.clone();
		self.require_handle(continuation.mixture)?;
		if let Some(turf_handle) = continuation.turf {
			let turf = self.require_turf_handle(turf_handle)?;
			if turf.mixture != Some(continuation.mixture) {
				return Err(WorldError::TurfMissingMixture(turf_handle));
			}
		}
		if reaction_result & REACTION_STOP != 0 {
			self.complete_continuation(token)?;
			return Ok((
				ReactionProgress {
					flags: reaction_result,
					work_items: 0,
					pending: false,
				},
				0,
			));
		}
		let sequence = self.evaluate_reaction_sequence(
			continuation.target,
			continuation.mixture,
			continuation.next_reaction_index,
			reaction_result,
			continuation.reaction_profile_threshold_ms,
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
					target: continuation.target,
					next_reaction_index: pending.next_reaction_index,
					reaction_profile_threshold_ms: continuation.reaction_profile_threshold_ms,
				},
			)?;
			Some(WorldEvent::RunDmReaction {
				turf: continuation.turf,
				mixture: continuation.mixture,
				target: continuation.target,
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
		Ok((
			ReactionProgress {
				flags: sequence.flags,
				work_items: sequence.work_items,
				pending: continuation_event.is_some(),
			},
			sequence.native_updates,
		))
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
			if entry.continuation.as_ref().is_some_and(|continuation| {
				continuation.turf.is_some_and(|turf| turf.slot == turf_slot)
			}) {
				entry.continuation = None;
				self.free_continuations.push(slot as u32);
			}
		}
	}

	#[cfg(debug_assertions)]
	fn process_excited_groups(
		&mut self,
		should_cancel: &mut impl FnMut() -> bool,
	) -> Result<StageResult, WorldError> {
		let max_entries = self.stage_turf_handles().len().min(self.mixtures.len());
		let mut transaction = IndexedTransaction::try_new(self.mixtures.len(), max_entries)
			.map_err(transaction_world_error)?;
		let mut staged_events = Vec::new();
		let result = self.compute_excited_groups(&mut transaction, should_cancel)?;
		self.validate_indexed_transaction(&transaction, 0, self.max_events as usize)?;
		self.publish_indexed_transaction_reusing(&mut transaction, &mut staged_events);
		Ok(result)
	}

	fn compute_excited_groups(
		&self,
		transaction: &mut IndexedTransaction<MixtureRecord>,
		should_cancel: &mut impl FnMut() -> bool,
	) -> Result<StageResult, WorldError> {
		if should_cancel() {
			return Err(WorldError::Cancelled);
		}
		let nodes = self
			.stage_turf_handles()
			.iter()
			.copied()
			.filter_map(|handle| {
				let turf = self.require_turf_handle(handle).ok()?;
				let mixture = turf.mixture?;
				Some((handle.slot, (handle, mixture)))
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
		let mut found = BTreeSet::new();
		let mut work_items = 0_u32;
		for initial_slot in nodes.keys().copied() {
			if found.contains(&initial_slot)
				|| !self
					.topology
					.gas_neighbors(nodes[&initial_slot].0)
					.any(|neighbor| {
						nodes
							.get(&neighbor.handle.slot)
							.is_some_and(|(handle, _)| *handle == neighbor.handle)
					}) {
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
				for neighbor in self.topology.gas_neighbors(nodes[&slot].0) {
					if nodes
						.get(&neighbor.handle.slot)
						.is_some_and(|(handle, _)| *handle == neighbor.handle)
						&& found.insert(neighbor.handle.slot)
					{
						queue.push(neighbor.handle.slot);
					}
				}
			}
			if accepted.is_empty() {
				continue;
			}
			let mut mixed_gases = [0.0; MAX_GAS_SLOTS];
			let mut total_capacity = 0.0;
			let mut total_energy = 0.0;
			let mut mutable_mixtures = BTreeSet::new();
			for slot in &accepted {
				let handle = nodes[slot].1;
				let mixture = self.require_handle(handle)?;
				if transaction.contains(handle) || !mutable_mixtures.insert(handle) {
					return Err(WorldError::DuplicateMutableTurfMixture(handle));
				}
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
				let mixture = self.require_handle(handle)?;
				let candidate = transaction
					.touch(handle, mixture.revision, mixture)
					.map_err(transaction_world_error)?;
				candidate.gases = mixed_gases;
				candidate.temperature = mixed_temperature;
				work_items = work_items
					.checked_add(1)
					.ok_or_else(|| WorldError::State("excited turf count exceeds u32".into()))?;
			}
		}
		if should_cancel() {
			return Err(WorldError::Cancelled);
		}
		Ok(StageResult { work_items })
	}

	#[cfg(debug_assertions)]
	fn process_equalize(
		&mut self,
		should_cancel: &mut impl FnMut() -> bool,
	) -> Result<StageResult, WorldError> {
		let max_entries = self.stage_turf_handles().len().min(self.mixtures.len());
		let mut transaction = IndexedTransaction::try_new(self.mixtures.len(), max_entries)
			.map_err(transaction_world_error)?;
		let mut staged_events = Vec::new();
		let result = self.compute_equalize(&mut transaction, &mut staged_events, should_cancel)?;
		self.validate_indexed_transaction(
			&transaction,
			staged_events.len(),
			self.max_events as usize,
		)?;
		self.publish_indexed_transaction_reusing(&mut transaction, &mut staged_events);
		Ok(result)
	}

	fn compute_equalize(
		&self,
		transaction: &mut IndexedTransaction<MixtureRecord>,
		staged_events: &mut Vec<WorldEvent>,
		should_cancel: &mut impl FnMut() -> bool,
	) -> Result<StageResult, WorldError> {
		if should_cancel() {
			return Err(WorldError::Cancelled);
		}
		let turf_handles = self
			.stage_turf_handles()
			.iter()
			.copied()
			.filter(|handle| {
				self.require_turf_handle(*handle)
					.is_ok_and(|turf| turf.mixture.is_some())
			})
			.collect::<Vec<_>>();
		if turf_handles.is_empty() {
			return Ok(StageResult { work_items: 0 });
		}
		let active_by_slot = turf_handles
			.iter()
			.map(|handle| (handle.slot, *handle))
			.collect::<BTreeMap<_, _>>();
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
				for neighbor in self.topology.gas_neighbors(active_by_slot[&current]) {
					if active_by_slot.get(&neighbor.handle.slot) != Some(&neighbor.handle)
						|| component.len() >= self.equalize_hard_turf_limit as usize
					{
						continue;
					}
					if visited.insert(neighbor.handle.slot) {
						parents.insert(neighbor.handle.slot, current);
						component.push(neighbor.handle.slot);
					}
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
				if transaction.contains(mixture_handle) {
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
				transaction
					.touch(mixture_handle, mixture.revision, mixture)
					.map_err(transaction_world_error)?;
			}
			if !immutable_turfs.is_empty() {
				if maximum_moles >= 10.0 && !mixtures_by_turf.is_empty() {
					self.stage_decompression_component(
						&component,
						&immutable_turfs,
						&mixtures_by_turf,
						component_moles,
						transaction,
						staged_events,
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
					(
						*slot,
						total_moles(
							transaction
								.candidate(handle)
								.expect("component mixtures were touched before balancing"),
						) - average_moles,
					)
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
					transaction,
					staged_events,
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
					transaction,
					staged_events,
				)?;
			}
			work_items = work_items
				.checked_add(component.len() as u32)
				.ok_or_else(|| WorldError::State("equalized turf count exceeds u32".into()))?;
		}
		if should_cancel() {
			return Err(WorldError::Cancelled);
		}
		Ok(StageResult { work_items })
	}

	#[allow(clippy::too_many_arguments)]
	fn stage_decompression_component(
		&self,
		component: &[u32],
		immutable_turfs: &BTreeSet<u32>,
		mixtures_by_turf: &BTreeMap<u32, MixtureHandle>,
		component_moles: f32,
		transaction: &mut IndexedTransaction<MixtureRecord>,
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
			let current_handle = self.current_turf_handle(current)?;
			for neighbor in self.topology.gas_neighbors(current_handle) {
				if component_slots.contains(&neighbor.handle.slot)
					&& reached.insert(neighbor.handle.slot)
				{
					parents.insert(neighbor.handle.slot, current);
					queue.push(neighbor.handle.slot);
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
			let mixture = transaction
				.candidate_mut(mixture_handle)
				.expect("component mixtures were touched before decompression");
			let before = total_moles(mixture);
			let ratio = if before > 0.0 {
				(removal_per_turf / before).clamp(0.0, 1.0)
			} else {
				0.0
			};
			for amount in &mut mixture.gases {
				*amount -= quantized_removal(*amount, ratio);
			}
			let lost = before - total_moles(mixture);
			local_losses.insert(turf_slot, lost);
		}
		for left in component_slots.iter().copied() {
			let left_handle = self.current_turf_handle(left)?;
			for neighbor in self.topology.gas_neighbors(left_handle).filter(|neighbor| {
				neighbor.firelock
					&& left < neighbor.handle.slot
					&& component_slots.contains(&neighbor.handle.slot)
			}) {
				let right = neighbor.handle.slot;
				let (source_slot, target_slot) =
					if immutable_turfs.contains(&left) && !immutable_turfs.contains(&right) {
						(right, left)
					} else {
						(left, right)
					};
				events.push(WorldEvent::FirelockConsideration {
					source: self.current_turf_handle(source_slot)?,
					target: self.current_turf_handle(target_slot)?,
				});
			}
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
		transaction: &mut IndexedTransaction<MixtureRecord>,
		events: &mut Vec<WorldEvent>,
	) -> Result<(), WorldError> {
		let source_handle = mixtures_by_turf[&source_slot];
		let target_handle = mixtures_by_turf[&target_slot];
		if source_handle == target_handle {
			return Err(WorldError::DuplicateMutableTurfMixture(source_handle));
		}
		let (source, target) = transaction
			.candidate_pair_mut(source_handle, target_handle)
			.map_err(transaction_world_error)?;
		let moved = transfer_moles(source, target, amount, specific_heats)?;
		if moved > 0.0 {
			events.push(WorldEvent::PressureDifference {
				source: self.current_turf_handle(source_slot)?,
				target: self.current_turf_handle(target_slot)?,
				moles: moved,
			});
		}
		Ok(())
	}

	#[cfg(debug_assertions)]
	fn process_turf_heat(
		&mut self,
		should_cancel: &mut impl FnMut() -> bool,
		seconds_per_tick: f32,
	) -> Result<StageResult, WorldError> {
		if should_cancel() {
			return Err(WorldError::Cancelled);
		}
		let mut candidates = BTreeMap::<u32, (TurfHandle, bool)>::new();
		for handle in self.frontier.committed().iter().copied() {
			candidates.insert(handle.slot, (handle, false));
		}
		for handle in self.heat_active.iter().copied() {
			candidates
				.entry(handle.slot)
				.and_modify(|candidate| candidate.1 = true)
				.or_insert((handle, true));
		}
		let seeds = candidates.values().copied().collect::<Vec<_>>();
		for (handle, _) in seeds {
			for neighbor in self.topology.heat_neighbors(handle) {
				candidates
					.entry(neighbor.handle.slot)
					.and_modify(|candidate| candidate.1 = true)
					.or_insert((neighbor.handle, true));
			}
		}
		let nodes = candidates
			.into_values()
			.filter_map(|(handle, can_continue)| {
				let turf = self.require_turf_handle(handle).ok()?;
				Some(StageHeatNode {
					handle,
					heat: turf.heat?,
					mixture: turf.mixture,
					can_continue,
				})
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
			.map(|(index, node)| (node.handle.slot, index as u32))
			.collect::<BTreeMap<_, _>>();
		let edges = self
			.topology
			.heat_slot_edges()
			.filter_map(|(left, right)| {
				let first = dense_by_slot.get(&left).copied()?;
				let second = dense_by_slot.get(&right).copied()?;
				Some((first, second))
			})
			.map(|(first, second)| Ok((first, second)))
			.collect::<Result<Vec<_>, WorldError>>()?;
		let mut temperatures = nodes
			.iter()
			.map(|node| node.heat.temperature)
			.collect::<Vec<_>>();
		let conductivities = nodes
			.iter()
			.map(|node| node.heat.thermal_conductivity)
			.collect::<Vec<_>>();
		let heat_capacities = nodes
			.iter()
			.map(|node| node.heat.heat_capacity)
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
		for (index, node) in nodes.iter().copied().enumerate() {
			if should_cancel() {
				return Err(WorldError::Cancelled);
			}
			let turf = node.handle;
			let state = node.heat;
			let mixture_handle = node.mixture;
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
		self.heat_active
			.try_reserve(nodes.len())
			.map_err(|_| WorldError::AllocationFailed)?;
		for (handle, mut mixture) in staged_mixtures {
			let current = self.require_handle_mut(handle)?;
			mixture.revision = current.revision + 1;
			*current = mixture;
		}
		for (node, temperature) in nodes.into_iter().zip(temperatures) {
			self.require_turf_handle_mut(node.handle)?
				.heat
				.as_mut()
				.expect("turf heat state was validated")
				.temperature = temperature;
			let was_active = self
				.require_turf_handle(node.handle)?
				.heat_active_index
				.is_some();
			let activation_threshold = if was_active || node.can_continue {
				MINIMUM_TEMPERATURE_FOR_SUPERCONDUCTION_K
			} else {
				MINIMUM_TEMPERATURE_START_SUPERCONDUCTION_K
			};
			let activation_temperature =
				self.turf_superconduction_temperature(node.handle, temperature)?;
			if activation_temperature >= activation_threshold {
				self.activate_turf_heat(node.handle)?;
			} else {
				self.deactivate_turf_heat_slot(node.handle.slot);
			}
		}
		self.events.extend(staged_events);
		Ok(StageResult { work_items })
	}

	#[cfg(debug_assertions)]
	fn process_turf_diffusion(
		&mut self,
		handles: Vec<MixtureHandle>,
		frontier_graph: Option<DiffusionGraph>,
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
		if frontier_graph.is_none() && self.turf_graph.is_none() {
			self.turf_graph = Some(self.build_turf_graph(&self.topology)?);
		}
		self.input.clear();
		for handle in handles.iter().copied() {
			let gases = self.require_handle(handle)?.gases;
			self.input.extend_from_slice(&gases);
		}
		self.output.resize(self.input.len(), 0.0);
		let graph = frontier_graph
			.as_ref()
			.or(self.turf_graph.as_ref())
			.expect("turf graph was built above");
		diffusion_step_into_cancellable(
			graph,
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
		self.topology.gas_edge_count()
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
		let removed = source_before
			.gases
			.map(|amount| quantized_removal(amount, ratio));
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
		let removed = source_before
			.gases
			.map(|amount| quantized_removal(amount, ratio));
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
		require_turf_handle_in(&self.turfs, handle)
	}

	fn stage_turf_handles(&self) -> Cow<'_, [TurfHandle]> {
		if let Some(component) = &self.stage_component_turfs {
			return Cow::Borrowed(component);
		}
		if self.use_committed_frontier {
			return Cow::Owned(
				self.frontier
					.committed()
					.iter()
					.copied()
					.filter(|handle| self.require_turf_handle(*handle).is_ok())
					.collect(),
			);
		}
		Cow::Owned(
			self.turfs
				.iter()
				.enumerate()
				.filter_map(|(slot, turf_slot)| {
					turf_slot.turf.as_ref()?;
					Some(TurfHandle {
						slot: slot as u32,
						generation: turf_slot.generation?,
					})
				})
				.collect(),
		)
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

	fn turf_superconduction_temperature(
		&self,
		handle: TurfHandle,
		closed_turf_temperature: f32,
	) -> Result<f32, WorldError> {
		let turf = self.require_turf_handle(handle)?;
		let Some(mixture) = turf.mixture else {
			return Ok(closed_turf_temperature);
		};
		Ok(closed_turf_temperature.max(self.require_handle(mixture)?.temperature))
	}

	fn activate_turf_heat(&mut self, handle: TurfHandle) -> Result<(), WorldError> {
		if self
			.require_turf_handle(handle)?
			.heat_active_index
			.is_some()
		{
			return Ok(());
		}
		self.heat_active
			.try_reserve(1)
			.map_err(|_| WorldError::AllocationFailed)?;
		let index = u32::try_from(self.heat_active.len())
			.map_err(|_| WorldError::State("active turf heat count exceeds u32".into()))?;
		self.heat_active.push(handle);
		self.require_turf_handle_mut(handle)?.heat_active_index = Some(index);
		Ok(())
	}

	fn deactivate_turf_heat_slot(&mut self, slot: u32) {
		let Some(index) = self
			.turfs
			.get(slot as usize)
			.and_then(|slot| slot.turf.as_ref())
			.and_then(|turf| turf.heat_active_index)
		else {
			return;
		};
		let index = index as usize;
		self.heat_active.swap_remove(index);
		if let Some(moved) = self.heat_active.get(index).copied() {
			self.require_turf_handle_mut(moved)
				.expect("active turf heat handle remains current")
				.heat_active_index = Some(index as u32);
		}
		self.turfs[slot as usize]
			.turf
			.as_mut()
			.expect("active turf heat record still exists")
			.heat_active_index = None;
	}

	fn remove_incident_turf_edges(&mut self, slot: u32) {
		self.topology.remove_slot(slot);
		self.turf_graph = None;
	}

	/// Removes this slot's gas edges while preserving its heat edges (topology.remove_slot()
	/// clears both, so heat edges are captured first and reconnected after).
	///
	/// Looks up this slot's own neighbors via topology.heat_neighbors() (an O(its own degree,
	/// <=6) direct slot lookup) rather than topology.heat_slot_edges() (an O(total heat edges in
	/// the world) scan filtered down to this slot). apply_turf_lifecycle calls this for every
	/// turf whose registration declares no mixture (heat-only turfs), and does so on every
	/// re-registration, not just the first - unnoticeable at unit-test scale, this scan cost
	/// multiplying against a real map's edge count and re-registration frequency was minutes.
	fn remove_incident_gas_edges(&mut self, slot: u32) {
		let heat_partners = self
			.current_turf_handle(slot)
			.map(|handle| {
				self.topology
					.heat_neighbors(handle)
					.map(|neighbor| neighbor.handle)
					.collect::<Vec<_>>()
			})
			.unwrap_or_default();
		self.topology.remove_slot(slot);
		if let Ok(this) = self.current_turf_handle(slot) {
			for other in heat_partners {
				let _ = self.topology.connect_heat(this, other);
			}
		}
		self.turf_graph = None;
	}

	/// Removes this slot's heat edges while preserving its gas edges and their firelock flags.
	/// See remove_incident_gas_edges() above for why this looks up neighbors via
	/// topology.gas_neighbors() (this slot's own degree) instead of topology.gas_slot_edges()
	/// (the whole world's edge count).
	fn remove_incident_heat_edges(&mut self, slot: u32) {
		let gas_partners = self
			.current_turf_handle(slot)
			.map(|handle| self.topology.gas_neighbors(handle).collect::<Vec<_>>())
			.unwrap_or_default();
		self.topology.remove_slot(slot);
		if let Ok(this) = self.current_turf_handle(slot) {
			for other in gas_partners {
				let _ = self.topology.connect_gas(this, other.handle);
				let _ = self
					.topology
					.set_firelock(this, other.handle, other.firelock);
			}
		}
	}

	#[cfg(debug_assertions)]
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

	// Only used by the debug-only process_turf_diffusion fallback below. The production
	// process_stage_chunk_cancellable diffusion path never read self.turf_graph even before
	// apply_turf_adjacency stopped eagerly rebuilding it.
	#[cfg(debug_assertions)]
	fn build_turf_graph(&self, topology: &PackedTopology) -> Result<DiffusionGraph, WorldError> {
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
		let directed = topology
			.gas_slot_edges()
			.flat_map(|(left, right, _)| {
				[
					DirectedEdge {
						from: NodeHandle(left),
						to: NodeHandle(right),
					},
					DirectedEdge {
						from: NodeHandle(right),
						to: NodeHandle(left),
					},
				]
			})
			.collect::<Vec<_>>();
		validate_graph(&nodes, &directed).map_err(|error| WorldError::Graph(error.to_string()))
	}

	#[cfg(debug_assertions)]
	fn build_turf_graph_for_handles(
		&self,
		handles: &[TurfHandle],
	) -> Result<DiffusionGraph, WorldError> {
		let allowed = handles
			.iter()
			.map(|handle| handle.slot)
			.collect::<BTreeSet<_>>();
		let nodes = handles
			.iter()
			.filter_map(|handle| {
				let turf = self.require_turf_handle(*handle).ok()?;
				let mixture = turf.mixture?;
				Some(GraphNode {
					handle: NodeHandle(handle.slot),
					generation: handle.generation,
					mixture: Some(mixture),
				})
			})
			.collect::<Vec<_>>();
		let directed = self
			.topology
			.gas_slot_edges()
			.filter(|(left, right, _)| allowed.contains(left) && allowed.contains(right))
			.flat_map(|(left, right, _)| {
				[
					DirectedEdge {
						from: NodeHandle(left),
						to: NodeHandle(right),
					},
					DirectedEdge {
						from: NodeHandle(right),
						to: NodeHandle(left),
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

fn quantized_removal(amount: f32, ratio: f32) -> f32 {
	quantize(amount * ratio).min(amount)
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
		*removed_amount = quantized_removal(*source_amount, ratio);
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
	let removed = source.gases.map(|moles| quantized_removal(moles, ratio));
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
	fn record_ratio_removal_does_not_quantize_past_the_source_amount() {
		let mut source = MixtureRecord::new();
		source.gases[0] = 0.00006;

		let removed = remove_ratio_record(&mut source, 1.0);

		assert_eq!(source.gases[0], 0.0);
		assert_eq!(removed.gases[0], 0.00006);
		assert_eq!(source.gases[0] + removed.gases[0], 0.00006);
	}

	#[test]
	fn mole_transfer_does_not_quantize_past_the_source_amount() {
		let mut source = MixtureRecord::new();
		source.gases[0] = 0.00006;
		let mut target = MixtureRecord::new();
		let specific_heats = [20.0; MAX_GAS_SLOTS];

		let moved = transfer_moles(&mut source, &mut target, 0.00006, &specific_heats).unwrap();

		assert_eq!(source.gases[0], 0.0);
		assert_eq!(target.gases[0], 0.00006);
		assert_eq!(moved, 0.00006);
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

	#[test]
	fn reusable_workset_counts_the_complete_heat_edge_tuple_capacity() {
		let mut world = DogmosWorld::new(1024 * 1024);
		world.stage_heat = Some(StageHeatState::new());
		let before = world.reusable_workset_bytes();
		let state = world.stage_heat.as_mut().unwrap();
		state.edges.push((0, 1, 0.25, 0.5));
		let expected_edge_bytes = state.edges.capacity() * std::mem::size_of::<HeatEdge>();

		assert_eq!(
			world.reusable_workset_bytes() - before,
			expected_edge_bytes as u64
		);
	}

	#[test]
	fn reusable_workset_counts_component_transaction_and_generation_marker_capacity() {
		let mut world = DogmosWorld::new(1024 * 1024);
		let before = world.reusable_workset_bytes();
		let state = StageComponentState::try_new(4, 3).unwrap();
		let expected_component_bytes = state.transaction.capacity_bytes_lower_bound()
			+ state.published_generation_by_slot.capacity() * std::mem::size_of::<Option<u32>>();
		world.stage_components = Some(state);

		assert_eq!(
			world.reusable_workset_bytes() - before,
			expected_component_bytes as u64
		);
	}

	#[test]
	fn reusable_workset_counts_temporary_component_turfs_once() {
		let mut world = DogmosWorld::new(1024 * 1024);
		let before = world.reusable_workset_bytes();
		let mut component_turfs = Vec::with_capacity(4);
		component_turfs.push(TurfHandle {
			slot: 0,
			generation: 1,
		});
		let expected_bytes = component_turfs.capacity() * std::mem::size_of::<TurfHandle>();
		world.stage_component_turfs = Some(component_turfs);

		assert_eq!(
			world.reusable_workset_bytes() - before,
			expected_bytes as u64
		);
	}

	#[test]
	fn component_transaction_revalidates_the_initial_mixture_revision() {
		let mut world = DogmosWorld::new(1024 * 1024);
		world
			.apply_lifecycle(&[LifecycleMutation {
				action: LifecycleAction::Register,
				handle: handle(0),
			}])
			.unwrap();
		let original = world.require_handle(handle(0)).unwrap().clone();
		let mut state = StageComponentState::try_new(1, 1).unwrap();
		state
			.transaction
			.touch(handle(0), original.revision, &original)
			.unwrap()
			.temperature = 500.0;
		world.mixtures[0].mixture.as_mut().unwrap().revision += 1;
		assert_eq!(
			world.validate_indexed_transaction(
				&state.transaction,
				state.staged_events.len(),
				world.max_events as usize,
			),
			Err(WorldError::StageConflict(
				StageConflictReason::TransactionRevision {
					handle: handle(0),
					expected: original.revision,
					actual: original.revision + 1,
				},
			))
		);
		assert_eq!(world.require_handle(handle(0)).unwrap().temperature, 2.7);
	}

	#[test]
	fn component_publication_identity_includes_the_mixture_generation() {
		let mut state = StageComponentState::try_new(1, 1).unwrap();
		let original = MixtureHandle {
			slot: 0,
			generation: 1,
		};
		let reused = MixtureHandle {
			slot: 0,
			generation: 2,
		};
		state.mark_mixture_published(original);

		assert!(state.mixture_was_published(original));
		assert!(!state.mixture_was_published(reused));
	}

	#[test]
	fn abort_stage_clears_every_resumable_stage_field() {
		let mut world = DogmosWorld::new(1024 * 1024);
		let request = StageChunkRequest {
			stage: WorldStage::Equalize,
			frontier_epoch: 1,
			stage_epoch: 2,
			work_limit: 1,
			seconds_per_tick: 0.5,
		};
		world.stage_cursor = Some(StageCursor::new(request, 0));
		world.stage_diffusion = Some(StageDiffusionState::new([0.0; MAX_GAS_SLOTS]));
		world.stage_heat = Some(StageHeatState::new());
		world.stage_components = Some(StageComponentState::try_new(1, 1).unwrap());
		world.stage_component_turfs = Some(Vec::new());
		world.use_committed_frontier = true;

		world.abort_stage();

		assert!(world.stage_cursor.is_none());
		assert!(world.stage_diffusion.is_none());
		assert!(world.stage_heat.is_none());
		assert!(world.stage_reactions.is_none());
		assert!(world.stage_components.is_none());
		assert!(world.stage_component_turfs.is_none());
		assert!(!world.use_committed_frontier);
	}

	#[test]
	fn lifecycle_batch_filters_mixture_edges_once_for_all_invalidated_slots() {
		let mut world = DogmosWorld::new(1024 * 1024);
		let handles = [handle(0), handle(1), handle(2), handle(3), handle(4)];
		world
			.apply_lifecycle(&handles.map(|handle| LifecycleMutation {
				action: LifecycleAction::Register,
				handle,
			}))
			.unwrap();
		world
			.apply_adjacency(&[
				AdjacencyMutation {
					left: handles[0],
					right: handles[1],
					conductivity: 0.5,
				},
				AdjacencyMutation {
					left: handles[1],
					right: handles[2],
					conductivity: 0.5,
				},
				AdjacencyMutation {
					left: handles[3],
					right: handles[4],
					conductivity: 0.5,
				},
			])
			.unwrap();
		world.mixture_edge_filter_passes = 0;

		world
			.apply_lifecycle(&[
				LifecycleMutation {
					action: LifecycleAction::Unregister,
					handle: handles[0],
				},
				LifecycleMutation {
					action: LifecycleAction::Unregister,
					handle: handles[2],
				},
			])
			.unwrap();

		assert_eq!(world.mixture_edge_filter_passes, 1);
		assert_eq!(world.edges.len(), 1);
		assert!(world.edges.contains_key(&EdgeKey::new(3, 4).unwrap()));
	}
}
