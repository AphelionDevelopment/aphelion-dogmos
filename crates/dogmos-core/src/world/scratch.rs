use super::*;

impl StageDiffusionState {
	pub(super) fn clear(&mut self) {
		self.publication = Publication::new();
		self.publication_index = 0;
		self.turfs.clear();
		self.mixtures.clear();
		self.index_by_turf.clear();
		self.seen_mixtures.clear();
		self.input.clear();
		self.output.clear();
		self.input_temperatures.clear();
		self.minimum_heat_capacities.clear();
		self.input_energy.clear();
		self.output_energy.clear();
		self.next_node = 0;
	}
}

impl StageHeatState {
	pub(super) fn clear(&mut self) {
		self.staged_heat_active.clear();
		self.maximum_row_sum = 0.0;
		self.publication = Publication::new();
		self.publication_index = 0;
		self.nodes.clear();
		self.index_by_slot.clear();
		self.temperatures.clear();
		self.conductivities.clear();
		self.heat_capacities.clear();
		self.staged_mixtures.clear();
		self.linked_mixtures.clear();
		self.staged_events.clear();
		self.next_active_seed = 0;
		self.next_node = 0;
		self.next_topology_node = 0;
		self.next_topology_neighbor = 0;
		self.edges.clear();
		self.row_sums.clear();
		self.conduction_substeps = None;
		self.conduction_substep = 0;
		self.conduction_edge = 0;
		self.conduction_scale = 0.0;
	}
}

impl StageReactionState {
	pub(super) fn new() -> Self {
		Self {
			publication: Publication::new(),
			publication_index: 0,
			targets: Vec::new(),
			active_continuations: SlotSet::new(),
			seen_mixtures: SlotSet::new(),
			staged: BTreeMap::new(),
			staged_events: Vec::new(),
			pending: None,
			next_target: 0,
		}
	}
	pub(super) fn clear(&mut self) {
		self.publication = Publication::new();
		self.publication_index = 0;
		self.targets.clear();
		self.active_continuations.clear();
		self.seen_mixtures.clear();
		self.staged.clear();
		self.staged_events.clear();
		self.pending = None;
		self.next_target = 0;
	}
}

impl StageComponentState {
	pub(super) fn clear(&mut self) {
		self.publication = Publication::new();
		self.publication_index = 0;
		self.targets.clear();
		self.active_by_slot.clear();
		self.visited.clear();
		self.next_seed = 0;
		self.queue.clear();
		self.queue_index = 0;
		self.next_neighbor = 0;
		self.component_ready = false;
		self.computed = false;
		self.computation = None;
		if let Some(kernel) = &mut self.component_kernel {
			kernel.clear();
		}
		self.prepared_turfs = 0;
		self.transaction.clear();
		self.published_mixtures.clear();
		self.staged_events.clear();
		self.callback_events = 0;
		self.components_processed = 0;
	}
	pub(super) fn prepare(&mut self, slots: usize, entries: usize) -> Result<(), WorldError> {
		self.clear();
		self.transaction
			.prepare(slots, entries)
			.map_err(transaction_world_error)?;
		Ok(())
	}
}
