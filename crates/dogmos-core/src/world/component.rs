use super::*;
use std::{
	future::Future,
	pin::Pin,
	task::{Context, Poll},
};

pub(super) type Computation = Pin<
	Box<
		dyn Future<
				Output = (
					ComponentKernel,
					IndexedTransaction<MixtureRecord>,
					Vec<WorldEvent>,
					Result<StageResult, WorldError>,
				),
			> + Send,
	>,
>;

struct Yield(bool);
impl Future for Yield {
	type Output = ();
	fn poll(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
		if self.0 {
			Poll::Ready(())
		} else {
			self.0 = true;
			Poll::Pending
		}
	}
}
async fn cooperate() {
	Yield(false).await;
}

pub(super) fn compute(
	kernel: ComponentKernel,
	mut transaction: IndexedTransaction<MixtureRecord>,
	mut events: Vec<WorldEvent>,
	stage: WorldStage,
) -> Computation {
	Box::pin(async move {
		let result = match stage {
			WorldStage::Equalize => kernel.compute_equalize(&mut transaction, &mut events).await,
			WorldStage::ExcitedGroups => kernel.compute_excited_groups(&mut transaction).await,
			_ => unreachable!("component stage"),
		};
		(kernel, transaction, events, result)
	})
}

struct ComponentTopology(
	SlotIndex<TurfHandle, [Option<crate::topology::TopologyNeighbor>; MAX_TURF_NEIGHBORS]>,
);
impl ComponentTopology {
	fn gas_neighbors(
		&self,
		handle: TurfHandle,
	) -> impl Iterator<Item = crate::topology::TopologyNeighbor> + '_ {
		self.0
			.get(&handle)
			.into_iter()
			.flat_map(|row| row.iter().flatten().copied())
	}
}

pub(super) struct ComponentKernel {
	handles: Vec<TurfHandle>,
	handles_by_slot: SlotIndex<u32, TurfHandle>,
	turfs: SlotIndex<TurfHandle, TurfRecord>,
	mixtures: SlotIndex<MixtureHandle, MixtureRecord>,
	topology: ComponentTopology,
	gas_registry: Option<GasMetadataRegistry>,
	equalize_hard_turf_limit: u32,
}

impl ComponentKernel {
	pub(super) fn capacity_bytes(&self) -> usize {
		self.handles.capacity() * std::mem::size_of::<TurfHandle>()
			+ self.handles_by_slot.capacity_bytes()
			+ self.turfs.capacity_bytes()
			+ self.mixtures.capacity_bytes()
			+ self.topology.0.capacity_bytes()
	}
	pub(super) fn new(world: &DogmosWorld) -> Self {
		Self {
			handles: Vec::new(),
			handles_by_slot: SlotIndex::new(),
			turfs: SlotIndex::new(),
			mixtures: SlotIndex::new(),
			topology: ComponentTopology(SlotIndex::new()),
			gas_registry: world.gas_registry.clone(),
			equalize_hard_turf_limit: world.equalize_hard_turf_limit,
		}
	}
	pub(super) fn capture(
		&mut self,
		world: &DogmosWorld,
		handle: TurfHandle,
	) -> Result<(), WorldError> {
		let turf = world.require_turf_handle(handle)?.clone();
		if let Some(mixture) = turf.mixture {
			self.mixtures
				.insert(mixture, world.require_handle(mixture)?.clone());
		}
		self.turfs.insert(handle, turf);
		self.handles.push(handle);
		self.handles_by_slot.insert(handle.slot, handle);
		let mut row = [None; MAX_TURF_NEIGHBORS];
		for (entry, neighbor) in row.iter_mut().zip(world.topology.gas_neighbors(handle)) {
			*entry = Some(neighbor);
		}
		self.topology.0.insert(handle, row);
		Ok(())
	}
	pub(super) fn clear(&mut self) {
		self.handles.clear();
		self.handles_by_slot.clear();
		self.turfs.clear();
		self.mixtures.clear();
		self.topology.0.clear();
	}
	fn stage_turf_handles(&self) -> Cow<'_, [TurfHandle]> {
		Cow::Borrowed(&self.handles)
	}
	fn require_turf_handle(&self, handle: TurfHandle) -> Result<&TurfRecord, WorldError> {
		self.turfs
			.get(&handle)
			.ok_or(WorldError::UnknownTurfHandle(handle))
	}
	fn require_handle(&self, handle: MixtureHandle) -> Result<&MixtureRecord, WorldError> {
		self.mixtures
			.get(&handle)
			.ok_or(WorldError::UnknownHandle(handle))
	}
	fn current_turf_handle(&self, slot: u32) -> Result<TurfHandle, WorldError> {
		self.handles_by_slot
			.get(&slot)
			.copied()
			.ok_or(WorldError::UnknownTurfHandle(TurfHandle {
				slot,
				generation: 0,
			}))
	}
	pub(super) async fn compute_excited_groups(
		&self,
		transaction: &mut IndexedTransaction<MixtureRecord>,
	) -> Result<StageResult, WorldError> {
		cooperate().await;
		let mut ordered = BTreeMap::new();
		for &handle in self.stage_turf_handles().iter() {
			cooperate().await;
			if let Some(mixture) = self
				.require_turf_handle(handle)
				.ok()
				.and_then(|turf| turf.mixture)
			{
				ordered.insert(handle.slot, (handle, mixture));
			}
		}
		let mut nodes = Vec::with_capacity(ordered.len());
		for (slot, (handle, mixture)) in ordered {
			cooperate().await;
			nodes.push((slot, handle, mixture));
		}
		if nodes.is_empty() {
			return Ok(StageResult { work_items: 0 });
		}
		let position_of = |slot: u32| -> Option<usize> {
			nodes
				.binary_search_by_key(&slot, |&(candidate, _, _)| candidate)
				.ok()
		};
		let specific_heats = self
			.gas_registry
			.as_ref()
			.ok_or(WorldError::GasRegistryMissing)?
			.specific_heats();
		let mut heat_values = [0.0; MAX_GAS_SLOTS];
		heat_values[..specific_heats.len()].copy_from_slice(specific_heats);
		// Positions in `nodes` are dense, so visited marking is a direct index rather than a
		// tree insert. `queue` and `accepted` carry positions for the same reason.
		let mut found = vec![false; nodes.len()];
		let mut queue: Vec<usize> = Vec::new();
		let mut accepted: Vec<usize> = Vec::new();
		let mut mutable_mixtures: BTreeSet<MixtureHandle> = BTreeSet::new();
		let mut work_items = 0_u32;
		for initial_position in 0..nodes.len() {
			cooperate().await;
			if found[initial_position]
				|| !self
					.topology
					.gas_neighbors(nodes[initial_position].1)
					.any(|neighbor| {
						position_of(neighbor.handle.slot)
							.is_some_and(|position| nodes[position].1 == neighbor.handle)
					}) {
				continue;
			}
			cooperate().await;
			let initial_mixture = self.require_handle(nodes[initial_position].2)?;
			if initial_mixture.immutable {
				continue;
			}
			let initial_pressure = mixture_pressure(initial_mixture);
			let mut minimum_pressure = initial_pressure;
			let mut maximum_pressure = initial_pressure;
			queue.clear();
			queue.push(initial_position);
			let mut queue_index = 0;
			accepted.clear();
			found[initial_position] = true;
			while queue_index < queue.len() && accepted.len() < 2500 {
				cooperate().await;
				let position = queue[queue_index];
				queue_index += 1;
				let mixture = self.require_handle(nodes[position].2)?;
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
				accepted.push(position);
				for neighbor in self.topology.gas_neighbors(nodes[position].1) {
					let Some(neighbor_position) = position_of(neighbor.handle.slot) else {
						continue;
					};
					if nodes[neighbor_position].1 == neighbor.handle && !found[neighbor_position] {
						found[neighbor_position] = true;
						queue.push(neighbor_position);
					}
				}
			}
			if accepted.is_empty() {
				continue;
			}
			let mut mixed_gases = [0.0; MAX_GAS_SLOTS];
			let mut total_capacity = 0.0;
			let mut total_energy = 0.0;
			mutable_mixtures.clear();
			for &position in &accepted {
				cooperate().await;
				let handle = nodes[position].2;
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
			for &position in &accepted {
				cooperate().await;
				let handle = nodes[position].2;
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
		cooperate().await;
		Ok(StageResult { work_items })
	}
	pub(super) async fn compute_equalize(
		&self,
		transaction: &mut IndexedTransaction<MixtureRecord>,
		staged_events: &mut Vec<WorldEvent>,
	) -> Result<StageResult, WorldError> {
		cooperate().await;
		let turf_handles = self.stage_turf_handles();
		if turf_handles.is_empty() {
			return Ok(StageResult { work_items: 0 });
		}
		let active_by_slot = &self.handles_by_slot;
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
		for &start in turf_handles.iter() {
			cooperate().await;
			if self.require_turf_handle(start)?.mixture.is_none() || !visited.insert(start.slot) {
				continue;
			}
			cooperate().await;
			let mut component = vec![start.slot];
			let mut parents = BTreeMap::<u32, u32>::new();
			let mut queue_index = 0;
			while queue_index < component.len() {
				cooperate().await;
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
				cooperate().await;
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
					)
					.await?;
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
			let mut subtree_balance = BTreeMap::new();
			for slot in &component {
				cooperate().await;
				let handle = mixtures_by_turf[slot];
				subtree_balance.insert(
					*slot,
					total_moles(transaction.candidate(handle).expect("component mixture"))
						- average_moles,
				);
			}
			let mut flows = Vec::<(u32, u32, f32)>::new();
			for child in component.iter().copied().skip(1).rev() {
				cooperate().await;
				let parent = parents[&child];
				let balance = subtree_balance[&child];
				flows.push((child, parent, balance));
				*subtree_balance
					.get_mut(&parent)
					.expect("component parent has a balance") += balance;
			}
			for &(child, parent, balance) in flows.iter().filter(|(_, _, balance)| *balance > 0.0) {
				cooperate().await;
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
				cooperate().await;
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
		cooperate().await;
		Ok(StageResult { work_items })
	}
	async fn stage_decompression_component(
		&self,
		component: &[u32],
		immutable_turfs: &BTreeSet<u32>,
		mixtures_by_turf: &BTreeMap<u32, MixtureHandle>,
		component_moles: f32,
		transaction: &mut IndexedTransaction<MixtureRecord>,
		events: &mut Vec<WorldEvent>,
	) -> Result<(), WorldError> {
		let mut component_slots = BTreeSet::new();
		for &slot in component {
			cooperate().await;
			component_slots.insert(slot);
		}
		let mut queue = Vec::new();
		let mut reached = BTreeSet::new();
		for &slot in immutable_turfs {
			cooperate().await;
			queue.push(slot);
			reached.insert(slot);
		}
		let mut parents = BTreeMap::<u32, u32>::new();
		let mut queue_index = 0;
		while queue_index < queue.len() {
			cooperate().await;
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
			cooperate().await;
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
			cooperate().await;
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
			cooperate().await;
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
}
