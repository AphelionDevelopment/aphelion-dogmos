use crate::MixtureHandle;

const UNUSED_INDEX: u32 = u32::MAX;
const BITS_PER_WORD: usize = u64::BITS as usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TransactionError {
	AllocationFailed,
	CapacityExceeded,
	HandleConflict {
		requested: MixtureHandle,
		current: MixtureHandle,
	},
	UnknownHandle(MixtureHandle),
	SameHandle(MixtureHandle),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TransactionEntry<T> {
	pub(crate) handle: MixtureHandle,
	pub(crate) expected_revision: u32,
	pub(crate) candidate: T,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct IndexedTransaction<T> {
	slot_to_index: Vec<u32>,
	touched_bits: Vec<u64>,
	entries: Vec<TransactionEntry<T>>,
	max_entries: usize,
}

impl<T: Clone> IndexedTransaction<T> {
	pub(crate) fn prepare(
		&mut self,
		slot_count: usize,
		max_entries: usize,
	) -> Result<(), TransactionError> {
		self.clear();
		if max_entries >= UNUSED_INDEX as usize {
			return Err(TransactionError::CapacityExceeded);
		}
		let words = slot_count
			.checked_add(BITS_PER_WORD - 1)
			.ok_or(TransactionError::CapacityExceeded)?
			/ BITS_PER_WORD;
		self.slot_to_index
			.try_reserve(slot_count.saturating_sub(self.slot_to_index.len()))
			.map_err(|_| TransactionError::AllocationFailed)?;
		self.slot_to_index.resize(slot_count, UNUSED_INDEX);
		self.touched_bits
			.try_reserve(words.saturating_sub(self.touched_bits.len()))
			.map_err(|_| TransactionError::AllocationFailed)?;
		self.touched_bits.resize(words, 0);
		self.entries
			.try_reserve(max_entries)
			.map_err(|_| TransactionError::AllocationFailed)?;
		self.max_entries = max_entries;
		Ok(())
	}
	pub(crate) fn try_new(slot_count: usize, max_entries: usize) -> Result<Self, TransactionError> {
		if max_entries >= UNUSED_INDEX as usize {
			return Err(TransactionError::CapacityExceeded);
		}
		let bit_words = slot_count
			.checked_add(BITS_PER_WORD - 1)
			.ok_or(TransactionError::CapacityExceeded)?
			/ BITS_PER_WORD;

		let mut slot_to_index = Vec::new();
		slot_to_index
			.try_reserve_exact(slot_count)
			.map_err(|_| TransactionError::AllocationFailed)?;
		slot_to_index.resize(slot_count, UNUSED_INDEX);
		let mut touched_bits = Vec::new();
		touched_bits
			.try_reserve_exact(bit_words)
			.map_err(|_| TransactionError::AllocationFailed)?;
		touched_bits.resize(bit_words, 0);
		let mut entries = Vec::new();
		entries
			.try_reserve_exact(max_entries)
			.map_err(|_| TransactionError::AllocationFailed)?;

		Ok(Self {
			slot_to_index,
			touched_bits,
			entries,
			max_entries,
		})
	}

	#[cfg(test)]
	pub(crate) fn len(&self) -> usize {
		self.entries.len()
	}

	#[cfg(test)]
	pub(crate) fn checkpoint(&self) -> usize {
		self.entries.len()
	}

	pub(crate) fn contains(&self, handle: MixtureHandle) -> bool {
		self.entry_index(handle).is_some()
	}

	pub(crate) fn touch(
		&mut self,
		handle: MixtureHandle,
		expected_revision: u32,
		initial: &T,
	) -> Result<&mut T, TransactionError> {
		let slot = usize::try_from(handle.slot).map_err(|_| TransactionError::CapacityExceeded)?;
		let Some(&dense_index) = self.slot_to_index.get(slot) else {
			return Err(TransactionError::CapacityExceeded);
		};
		if dense_index != UNUSED_INDEX {
			let entry = &mut self.entries[dense_index as usize];
			if entry.handle != handle {
				return Err(TransactionError::HandleConflict {
					requested: handle,
					current: entry.handle,
				});
			}
			return Ok(&mut entry.candidate);
		}
		if self.entries.len() >= self.max_entries {
			return Err(TransactionError::CapacityExceeded);
		}

		let dense_index =
			u32::try_from(self.entries.len()).map_err(|_| TransactionError::CapacityExceeded)?;
		self.entries.push(TransactionEntry {
			handle,
			expected_revision,
			candidate: initial.clone(),
		});
		self.slot_to_index[slot] = dense_index;
		self.touched_bits[slot / BITS_PER_WORD] |= 1 << (slot % BITS_PER_WORD);
		Ok(&mut self.entries[dense_index as usize].candidate)
	}

	pub(crate) fn candidate(&self, handle: MixtureHandle) -> Option<&T> {
		let index = self.entry_index(handle)?;
		Some(&self.entries[index].candidate)
	}

	pub(crate) fn candidate_mut(&mut self, handle: MixtureHandle) -> Option<&mut T> {
		let index = self.entry_index(handle)?;
		Some(&mut self.entries[index].candidate)
	}

	pub(crate) fn candidate_pair_mut(
		&mut self,
		first: MixtureHandle,
		second: MixtureHandle,
	) -> Result<(&mut T, &mut T), TransactionError> {
		if first == second {
			return Err(TransactionError::SameHandle(first));
		}
		let first_index = self
			.entry_index(first)
			.ok_or(TransactionError::UnknownHandle(first))?;
		let second_index = self
			.entry_index(second)
			.ok_or(TransactionError::UnknownHandle(second))?;
		if first_index < second_index {
			let (lower, upper) = self.entries.split_at_mut(second_index);
			Ok((&mut lower[first_index].candidate, &mut upper[0].candidate))
		} else {
			let (lower, upper) = self.entries.split_at_mut(first_index);
			Ok((&mut upper[0].candidate, &mut lower[second_index].candidate))
		}
	}

	#[cfg(test)]
	pub(crate) fn rollback_to(&mut self, checkpoint: usize) {
		for entry in self.entries.drain(checkpoint..) {
			let slot = entry.handle.slot as usize;
			self.slot_to_index[slot] = UNUSED_INDEX;
			self.touched_bits[slot / BITS_PER_WORD] &= !(1 << (slot % BITS_PER_WORD));
		}
	}

	pub(crate) fn retire(&mut self, index: usize) {
		let slot = self.entries[index].handle.slot as usize;
		self.slot_to_index[slot] = UNUSED_INDEX;
		self.touched_bits[slot / BITS_PER_WORD] &= !(1 << (slot % BITS_PER_WORD));
	}

	pub(crate) fn clear_retired(&mut self) {
		self.entries.clear();
	}

	pub(crate) fn clear(&mut self) {
		drop(self.drain_entries());
	}

	#[cfg(debug_assertions)]
	pub(crate) fn sort_by_handle(&mut self) {
		self.entries.sort_unstable_by_key(|entry| entry.handle);
		for (index, entry) in self.entries.iter().enumerate() {
			self.slot_to_index[entry.handle.slot as usize] = index as u32;
		}
	}

	pub(crate) fn entries(&self) -> &[TransactionEntry<T>] {
		&self.entries
	}

	pub(crate) fn drain_entries(&mut self) -> std::vec::Drain<'_, TransactionEntry<T>> {
		for entry in &self.entries {
			let slot = entry.handle.slot as usize;
			self.slot_to_index[slot] = UNUSED_INDEX;
			self.touched_bits[slot / BITS_PER_WORD] &= !(1 << (slot % BITS_PER_WORD));
		}
		self.entries.drain(..)
	}

	pub(crate) fn capacity_bytes_lower_bound(&self) -> usize {
		self.slot_to_index.capacity() * std::mem::size_of::<u32>()
			+ self.touched_bits.capacity() * std::mem::size_of::<u64>()
			+ self.entries.capacity() * std::mem::size_of::<TransactionEntry<T>>()
	}

	fn entry_index(&self, handle: MixtureHandle) -> Option<usize> {
		let slot = handle.slot as usize;
		let word = *self.touched_bits.get(slot / BITS_PER_WORD)?;
		if word & (1 << (slot % BITS_PER_WORD)) == 0 {
			return None;
		}
		let index = *self.slot_to_index.get(slot)?;
		if index == UNUSED_INDEX || self.entries[index as usize].handle != handle {
			return None;
		}
		Some(index as usize)
	}
}

#[cfg(test)]
mod tests {
	use super::{IndexedTransaction, TransactionError};
	use crate::MixtureHandle;

	fn handle(slot: u32, generation: u32) -> MixtureHandle {
		MixtureHandle { slot, generation }
	}

	#[test]
	fn repeated_touch_reuses_the_candidate() {
		let mut transaction = IndexedTransaction::try_new(4, 2).unwrap();
		*transaction.touch(handle(2, 7), 11, &10).unwrap() = 20;

		assert_eq!(transaction.touch(handle(2, 7), 11, &99), Ok(&mut 20));
		assert_eq!(transaction.len(), 1);
	}

	#[test]
	fn conflicting_generation_is_rejected() {
		let mut transaction = IndexedTransaction::try_new(2, 2).unwrap();
		transaction.touch(handle(1, 3), 4, &10).unwrap();

		assert_eq!(
			transaction.touch(handle(1, 4), 5, &20),
			Err(TransactionError::HandleConflict {
				requested: handle(1, 4),
				current: handle(1, 3),
			}),
		);
	}

	#[test]
	fn rollback_clears_dense_and_slot_indexes() {
		let mut transaction = IndexedTransaction::try_new(4, 4).unwrap();
		transaction.touch(handle(0, 1), 2, &10).unwrap();
		let checkpoint = transaction.checkpoint();
		transaction.touch(handle(3, 5), 6, &30).unwrap();

		transaction.rollback_to(checkpoint);

		assert!(!transaction.contains(handle(3, 5)));
		assert_eq!(transaction.candidate(handle(0, 1)), Some(&10));
		assert_eq!(transaction.touch(handle(3, 6), 7, &40), Ok(&mut 40));
	}

	#[test]
	fn clear_reuses_allocated_indexes_and_entries() {
		let mut transaction = IndexedTransaction::try_new(4, 4).unwrap();
		transaction.touch(handle(0, 1), 2, &10).unwrap();
		transaction.touch(handle(3, 5), 6, &30).unwrap();
		let allocated_bytes = transaction.capacity_bytes_lower_bound();

		transaction.clear();

		assert_eq!(transaction.len(), 0);
		assert!(!transaction.contains(handle(0, 1)));
		assert!(!transaction.contains(handle(3, 5)));
		assert_eq!(transaction.touch(handle(0, 2), 7, &40), Ok(&mut 40));
		assert_eq!(transaction.touch(handle(3, 6), 8, &50), Ok(&mut 50));
		assert_eq!(transaction.capacity_bytes_lower_bound(), allocated_bytes);
	}

	#[test]
	fn pair_access_is_disjoint_and_rejects_same_handle() {
		let mut transaction = IndexedTransaction::try_new(4, 4).unwrap();
		transaction.touch(handle(3, 1), 0, &30).unwrap();
		transaction.touch(handle(1, 1), 0, &10).unwrap();

		let (first, second) = transaction
			.candidate_pair_mut(handle(3, 1), handle(1, 1))
			.unwrap();
		*first += 1;
		*second += 2;

		assert_eq!(transaction.candidate(handle(3, 1)), Some(&31));
		assert_eq!(transaction.candidate(handle(1, 1)), Some(&12));
		assert_eq!(
			transaction.candidate_pair_mut(handle(1, 1), handle(1, 1)),
			Err(TransactionError::SameHandle(handle(1, 1))),
		);
	}

	#[test]
	fn entries_sort_by_handle_deterministically() {
		let mut transaction = IndexedTransaction::try_new(4, 4).unwrap();
		transaction.touch(handle(3, 2), 30, &3).unwrap();
		transaction.touch(handle(0, 9), 10, &0).unwrap();
		transaction.touch(handle(2, 1), 20, &2).unwrap();

		transaction.sort_by_handle();

		let entries = transaction.entries();
		assert_eq!(entries[0].handle, handle(0, 9));
		assert_eq!(entries[1].handle, handle(2, 1));
		assert_eq!(entries[2].handle, handle(3, 2));
		assert_eq!(entries[1].expected_revision, 20);
		assert_eq!(entries[2].candidate, 3);
	}

	#[test]
	fn oversized_slot_capacity_is_rejected_without_allocating() {
		assert_eq!(
			IndexedTransaction::<u8>::try_new(usize::MAX, 0),
			Err(TransactionError::CapacityExceeded),
		);
	}
}
