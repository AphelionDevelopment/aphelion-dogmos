use crate::{
	metadata::{
		GasMetadata, GasMetadataError, GasMetadataRegistry, ReactionMetadata,
		ReactionMetadataError, ReactionMetadataRegistry,
	},
	numerics::diffusion::{
		diffusion_step_into_cancellable, validate_graph, DiffusionError, DiffusionGraph,
		DirectedEdge, GraphNode, NodeHandle,
	},
	MixtureHandle, MAX_GAS_SLOTS,
};
use std::{collections::BTreeMap, error::Error, fmt};

#[derive(Clone)]
struct MixtureRecord {
	revision: u32,
	temperature: f32,
	volume: f32,
	gases: [f32; MAX_GAS_SLOTS],
}

impl MixtureRecord {
	fn new() -> Self {
		Self {
			revision: 0,
			temperature: MINIMUM_TEMPERATURE_K,
			volume: 0.0,
			gases: [0.0; MAX_GAS_SLOTS],
		}
	}
}

#[derive(Clone, Default)]
struct MixtureSlot {
	generation: Option<u32>,
	mixture: Option<MixtureRecord>,
}

#[derive(Clone, Copy)]
struct ProjectedSlot {
	generation: Option<u32>,
	occupied: bool,
}

const MINIMUM_TEMPERATURE_K: f32 = 2.7;

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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdjacencyMutation {
	pub left: MixtureHandle,
	pub right: MixtureHandle,
	pub conductivity: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MixtureStateMutation {
	pub handle: MixtureHandle,
	pub expected_revision: u32,
	pub temperature: f32,
	pub volume: f32,
	pub gases: [f32; MAX_GAS_SLOTS],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorldStage {
	ProcessTurfs,
	Equalize,
	React,
	TurfHeat,
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
	pub gases: [f32; MAX_GAS_SLOTS],
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
	RevisionMismatch {
		handle: MixtureHandle,
		expected: u32,
		actual: u32,
	},
	RevisionExhausted(MixtureHandle),
	DuplicateMixtureState(u32),
	InvalidMixtureState,
	SelfAdjacency(u32),
	InvalidConductivity,
	InvalidSecondsPerTick,
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
	edges: BTreeMap<EdgeKey, f32>,
	graph: Option<DiffusionGraph>,
	input: Vec<f32>,
	output: Vec<f32>,
	max_world_bytes: u64,
}

impl DogmosWorld {
	pub fn new(max_world_bytes: u64) -> Self {
		Self {
			gas_registry: None,
			reaction_registry: None,
			mixtures: Vec::new(),
			edges: BTreeMap::new(),
			graph: None,
			input: Vec::new(),
			output: Vec::new(),
			max_world_bytes,
		}
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

		for mutation in mutations {
			let slot = mutation.handle.slot as usize;
			match mutation.action {
				LifecycleAction::Register => {
					let replace = self.mixtures[slot].generation
						!= Some(mutation.handle.generation)
						|| self.mixtures[slot].mixture.is_none();
					if replace {
						self.mixtures[slot].generation = Some(mutation.handle.generation);
						self.mixtures[slot].mixture = Some(MixtureRecord::new());
						self.remove_incident_edges(mutation.handle.slot);
						changed = true;
					}
				}
				LifecycleAction::Unregister => {
					self.mixtures[slot].mixture = None;
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
			if mixture.revision == u32::MAX {
				return Err(WorldError::RevisionExhausted(mutation.handle));
			}
			if !valid_mixture_state(mutation) {
				return Err(WorldError::InvalidMixtureState);
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
			mixture.temperature = mutation.temperature;
			mixture.volume = mutation.volume;
			mixture.gases = mutation.gases;
			mixture.revision += 1;
		}
		Ok(mutations.len() as u32)
	}

	pub fn snapshot(&self, handle: MixtureHandle) -> Result<MixtureSnapshot, WorldError> {
		let mixture = self.require_handle(handle)?;
		Ok(MixtureSnapshot {
			revision: mixture.revision,
			temperature: mixture.temperature,
			volume: mixture.volume,
			gases: mixture.gases,
		})
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
		mut should_cancel: impl FnMut() -> bool,
	) -> Result<StageResult, WorldError> {
		if !seconds_per_tick.is_finite() || seconds_per_tick <= 0.0 {
			return Err(WorldError::InvalidSecondsPerTick);
		}
		let work_items = u32::try_from(self.live_count())
			.map_err(|_| WorldError::State("mixture count exceeds u32".into()))?;
		if stage != WorldStage::ProcessTurfs || work_items == 0 {
			return Ok(StageResult { work_items });
		}
		for (slot, mixture_slot) in self.mixtures.iter().enumerate() {
			let Some(mixture) = mixture_slot.mixture.as_ref() else {
				continue;
			};
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
			mixture
				.gases
				.copy_from_slice(&self.output[offset..offset + MAX_GAS_SLOTS]);
			mixture.revision = mixture
				.revision
				.checked_add(1)
				.expect("stage revisions were checked before mutation");
			offset += MAX_GAS_SLOTS;
		}
		Ok(StageResult { work_items })
	}

	pub fn edge_count(&self) -> usize {
		self.edges.len()
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

	fn validate_slot_capacity(&self, slot: u32) -> Result<(), WorldError> {
		let slots = u64::from(slot) + 1;
		let bytes = slots
			.checked_mul(std::mem::size_of::<MixtureSlot>() as u64)
			.ok_or(WorldError::StateCapacityExceeded)?;
		if bytes > self.max_world_bytes {
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

	fn remove_incident_edges(&mut self, slot: u32) {
		self.edges
			.retain(|key, _| key.left != slot && key.right != slot);
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
