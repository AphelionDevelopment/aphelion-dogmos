use crate::metadata::TurfHandle;

pub const MAX_TURF_NEIGHBORS: usize = 6;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TopologyNeighbor {
	pub handle: TurfHandle,
	pub firelock: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TopologyError {
	SelfEdge(TurfHandle),
	DegreeExceeded(TurfHandle),
	MissingGasEdge,
	AllocationFailed,
}

#[derive(Clone, Default)]
struct TopologySlot {
	gas: [Option<TopologyNeighbor>; MAX_TURF_NEIGHBORS],
	heat: [Option<TopologyNeighbor>; MAX_TURF_NEIGHBORS],
}

#[derive(Clone, Default)]
pub struct PackedTopology {
	slots: Vec<TopologySlot>,
	revision: u64,
	gas_edges: usize,
	heat_edges: usize,
}

impl PackedTopology {
	pub fn revision(&self) -> u64 {
		self.revision
	}

	pub fn gas_edge_count(&self) -> usize {
		self.gas_edges
	}

	pub fn heat_edge_count(&self) -> usize {
		self.heat_edges
	}

	pub fn allocated_bytes(&self) -> u64 {
		(self.slots.capacity() * std::mem::size_of::<TopologySlot>()) as u64
	}

	pub fn gas_neighbors(&self, handle: TurfHandle) -> impl Iterator<Item = TopologyNeighbor> + '_ {
		self.slots
			.get(handle.slot as usize)
			.into_iter()
			.flat_map(|slot| slot.gas.iter().flatten().copied())
	}

	pub fn heat_neighbors(
		&self,
		handle: TurfHandle,
	) -> impl Iterator<Item = TopologyNeighbor> + '_ {
		self.slots
			.get(handle.slot as usize)
			.into_iter()
			.flat_map(|slot| slot.heat.iter().flatten().copied())
	}

	pub fn gas_slot_edges(&self) -> impl Iterator<Item = (u32, u32, bool)> + '_ {
		self.slots.iter().enumerate().flat_map(|(slot, entry)| {
			entry.gas.iter().flatten().filter_map(move |neighbor| {
				(slot < neighbor.handle.slot as usize).then_some((
					slot as u32,
					neighbor.handle.slot,
					neighbor.firelock,
				))
			})
		})
	}

	pub fn heat_slot_edges(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
		self.slots.iter().enumerate().flat_map(|(slot, entry)| {
			entry.heat.iter().flatten().filter_map(move |neighbor| {
				(slot < neighbor.handle.slot as usize)
					.then_some((slot as u32, neighbor.handle.slot))
			})
		})
	}

	pub fn connect_gas(
		&mut self,
		left: TurfHandle,
		right: TurfHandle,
	) -> Result<bool, TopologyError> {
		let changed = self.connect(left, right, false)?;
		if changed {
			self.gas_edges += 1;
		}
		Ok(changed)
	}

	pub fn connect_heat(
		&mut self,
		left: TurfHandle,
		right: TurfHandle,
	) -> Result<bool, TopologyError> {
		let changed = self.connect(left, right, true)?;
		if changed {
			self.heat_edges += 1;
		}
		Ok(changed)
	}

	pub fn disconnect_gas(&mut self, left: TurfHandle, right: TurfHandle) -> bool {
		let changed = self.disconnect(left, right, false);
		if changed {
			self.gas_edges -= 1;
		}
		changed
	}

	pub fn disconnect_heat(&mut self, left: TurfHandle, right: TurfHandle) -> bool {
		let changed = self.disconnect(left, right, true);
		if changed {
			self.heat_edges -= 1;
		}
		changed
	}

	pub fn set_firelock(
		&mut self,
		left: TurfHandle,
		right: TurfHandle,
		firelock: bool,
	) -> Result<bool, TopologyError> {
		let Some(left_neighbor) = self.find_neighbor_mut(left, right, false) else {
			return Err(TopologyError::MissingGasEdge);
		};
		if left_neighbor.firelock == firelock {
			return Ok(false);
		}
		left_neighbor.firelock = firelock;
		self.find_neighbor_mut(right, left, false)
			.expect("gas topology is symmetric")
			.firelock = firelock;
		self.revision = self.revision.wrapping_add(1);
		Ok(true)
	}

	pub fn remove_turf(&mut self, handle: TurfHandle) -> bool {
		self.remove_slot(handle.slot)
	}

	pub fn remove_slot(&mut self, slot: u32) -> bool {
		let Some(entry) = self.slots.get_mut(slot as usize) else {
			return false;
		};
		// Copy the fixed-size neighbor arrays out (cheap, stack-only - both are Copy) before
		// clearing the slot, instead of scanning every slot in the world to find who pointed at
		// it. Degree is capped at MAX_TURF_NEIGHBORS and the slot already names its own partners
		// exactly, so only those ≤12 partner slots need their back-reference removed.
		let gas_neighbors = entry.gas;
		let heat_neighbors = entry.heat;
		let removed_gas = gas_neighbors.iter().flatten().count();
		let removed_heat = heat_neighbors.iter().flatten().count();
		if removed_gas == 0 && removed_heat == 0 {
			return false;
		}
		*entry = TopologySlot::default();
		for neighbor in gas_neighbors.into_iter().flatten() {
			if let Some(partner) = self.slots.get_mut(neighbor.handle.slot as usize) {
				remove_neighbor_slot(&mut partner.gas, slot);
			}
		}
		for neighbor in heat_neighbors.into_iter().flatten() {
			if let Some(partner) = self.slots.get_mut(neighbor.handle.slot as usize) {
				remove_neighbor_slot(&mut partner.heat, slot);
			}
		}
		self.gas_edges -= removed_gas;
		self.heat_edges -= removed_heat;
		self.revision = self.revision.wrapping_add(1);
		true
	}

	fn connect(
		&mut self,
		left: TurfHandle,
		right: TurfHandle,
		heat: bool,
	) -> Result<bool, TopologyError> {
		if left.slot == right.slot {
			return Err(TopologyError::SelfEdge(left));
		}
		self.ensure_slot(left.slot.max(right.slot))?;
		if self.find_neighbor(left, right, heat).is_some() {
			return Ok(false);
		}
		if self.neighbors_array(left, heat).iter().all(Option::is_some) {
			return Err(TopologyError::DegreeExceeded(left));
		}
		if self
			.neighbors_array(right, heat)
			.iter()
			.all(Option::is_some)
		{
			return Err(TopologyError::DegreeExceeded(right));
		}
		insert_sorted(
			self.neighbors_array_mut(left, heat),
			TopologyNeighbor {
				handle: right,
				firelock: false,
			},
		);
		insert_sorted(
			self.neighbors_array_mut(right, heat),
			TopologyNeighbor {
				handle: left,
				firelock: false,
			},
		);
		self.revision = self.revision.wrapping_add(1);
		Ok(true)
	}

	fn disconnect(&mut self, left: TurfHandle, right: TurfHandle, heat: bool) -> bool {
		let Some(_) = self.find_neighbor(left, right, heat) else {
			return false;
		};
		remove_neighbor(self.neighbors_array_mut(left, heat), right);
		remove_neighbor(self.neighbors_array_mut(right, heat), left);
		self.revision = self.revision.wrapping_add(1);
		true
	}

	fn ensure_slot(&mut self, slot: u32) -> Result<(), TopologyError> {
		let required = slot as usize + 1;
		if required > self.slots.len() {
			self.slots
				.try_reserve(required - self.slots.len())
				.map_err(|_| TopologyError::AllocationFailed)?;
			self.slots.resize_with(required, TopologySlot::default);
		}
		Ok(())
	}

	fn neighbors_array(
		&self,
		handle: TurfHandle,
		heat: bool,
	) -> &[Option<TopologyNeighbor>; MAX_TURF_NEIGHBORS] {
		if heat {
			&self.slots[handle.slot as usize].heat
		} else {
			&self.slots[handle.slot as usize].gas
		}
	}

	fn neighbors_array_mut(
		&mut self,
		handle: TurfHandle,
		heat: bool,
	) -> &mut [Option<TopologyNeighbor>; MAX_TURF_NEIGHBORS] {
		if heat {
			&mut self.slots[handle.slot as usize].heat
		} else {
			&mut self.slots[handle.slot as usize].gas
		}
	}

	fn find_neighbor(
		&self,
		left: TurfHandle,
		right: TurfHandle,
		heat: bool,
	) -> Option<&TopologyNeighbor> {
		self.slots.get(left.slot as usize).and_then(|slot| {
			let neighbors = if heat { &slot.heat } else { &slot.gas };
			neighbors
				.iter()
				.flatten()
				.find(|entry| entry.handle == right)
		})
	}

	fn find_neighbor_mut(
		&mut self,
		left: TurfHandle,
		right: TurfHandle,
		heat: bool,
	) -> Option<&mut TopologyNeighbor> {
		self.slots.get_mut(left.slot as usize).and_then(|slot| {
			let neighbors = if heat { &mut slot.heat } else { &mut slot.gas };
			neighbors
				.iter_mut()
				.flatten()
				.find(|entry| entry.handle == right)
		})
	}
}

fn insert_sorted(
	neighbors: &mut [Option<TopologyNeighbor>; MAX_TURF_NEIGHBORS],
	neighbor: TopologyNeighbor,
) {
	let count = neighbors.iter().take_while(|entry| entry.is_some()).count();
	let index = neighbors[..count]
		.partition_point(|entry| entry.expect("occupied prefix").handle < neighbor.handle);
	neighbors.copy_within(index..count, index + 1);
	neighbors[index] = Some(neighbor);
}

fn remove_neighbor(
	neighbors: &mut [Option<TopologyNeighbor>; MAX_TURF_NEIGHBORS],
	handle: TurfHandle,
) {
	let Some(index) = neighbors
		.iter()
		.position(|entry| entry.is_some_and(|neighbor| neighbor.handle == handle))
	else {
		return;
	};
	neighbors.copy_within(index + 1.., index);
	neighbors[MAX_TURF_NEIGHBORS - 1] = None;
}

fn remove_neighbor_slot(neighbors: &mut [Option<TopologyNeighbor>; MAX_TURF_NEIGHBORS], slot: u32) {
	let Some(index) = neighbors
		.iter()
		.position(|entry| entry.is_some_and(|neighbor| neighbor.handle.slot == slot))
	else {
		return;
	};
	neighbors.copy_within(index + 1.., index);
	neighbors[MAX_TURF_NEIGHBORS - 1] = None;
}
