use crate::{metadata::TurfHandle, MixtureHandle};

pub(crate) trait SlotKey: Copy + Eq {
	fn slot(self) -> usize;
}

impl SlotKey for u32 {
	fn slot(self) -> usize {
		self as usize
	}
}

impl SlotKey for TurfHandle {
	fn slot(self) -> usize {
		self.slot as usize
	}
}

impl SlotKey for MixtureHandle {
	fn slot(self) -> usize {
		self.slot as usize
	}
}

/// Reusable generation-checked lookup storage. An epoch change clears membership without scanning slots.
pub(crate) struct SlotIndex<K, V> {
	entries: Vec<Option<(u64, K, V)>>,
	epoch: u64,
	touched: Vec<usize>,
}

impl<K: SlotKey, V> SlotIndex<K, V> {
	pub(crate) fn new() -> Self {
		Self {
			entries: Vec::new(),
			touched: Vec::new(),
			epoch: 1,
		}
	}
	pub(crate) fn clear(&mut self) {
		self.touched.clear();
		if let Some(epoch) = self.epoch.checked_add(1) {
			self.epoch = epoch;
		} else {
			self.entries.clear();
			self.epoch = 1;
		}
	}
	pub(crate) fn insert(&mut self, key: K, value: V) -> Option<V> {
		let slot = key.slot();
		if slot >= self.entries.len() {
			self.entries.resize_with(slot + 1, || None);
		}
		let previous = self.entries[slot].replace((self.epoch, key, value));
		if previous
			.as_ref()
			.is_none_or(|(epoch, _, _)| *epoch != self.epoch)
		{
			self.touched.push(slot);
		}
		previous
			.and_then(|(epoch, old, value)| (epoch == self.epoch && old == key).then_some(value))
	}
	pub(crate) fn get(&self, key: &K) -> Option<&V> {
		self.entries
			.get(key.slot())?
			.as_ref()
			.and_then(|(epoch, stored, value)| {
				(*epoch == self.epoch && stored == key).then_some(value)
			})
	}
	pub(crate) fn contains_key(&self, key: &K) -> bool {
		self.get(key).is_some()
	}
	pub(crate) fn capacity_bytes(&self) -> usize {
		self.entries.capacity() * std::mem::size_of::<Option<(u64, K, V)>>()
			+ self.touched.capacity() * std::mem::size_of::<usize>()
	}
}

impl<K: SlotKey, V> std::ops::Index<&K> for SlotIndex<K, V> {
	type Output = V;
	fn index(&self, key: &K) -> &V {
		self.get(key).expect("indexed slot must be present")
	}
}

/// Reusable membership storage retaining the full generation of each key.
pub(crate) struct SlotSet<K>(SlotIndex<K, ()>);

impl<K: SlotKey> SlotSet<K> {
	pub(crate) fn new() -> Self {
		Self(SlotIndex::new())
	}
	pub(crate) fn clear(&mut self) {
		self.0.clear();
	}
	pub(crate) fn insert(&mut self, key: K) -> bool {
		self.0.insert(key, ()).is_none()
	}
	pub(crate) fn contains(&self, key: &K) -> bool {
		self.0.contains_key(key)
	}
	pub(crate) fn capacity_bytes(&self) -> usize {
		self.0.capacity_bytes()
	}
}

impl<K: SlotKey> FromIterator<K> for SlotSet<K> {
	fn from_iter<T: IntoIterator<Item = K>>(iter: T) -> Self {
		let mut result = Self::new();
		for key in iter {
			result.insert(key);
		}
		result
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn reuse_rejects_old_generations_and_clears_only_live_entries() {
		let mut index = SlotIndex::new();
		let old = MixtureHandle {
			slot: 100,
			generation: 1,
		};
		let new = MixtureHandle {
			generation: 2,
			..old
		};
		index.insert(old, 4);
		index.insert(new, 7);
		assert_eq!(index.get(&old), None);
		assert_eq!(index.get(&new), Some(&7));
		let capacity = index.capacity_bytes();
		index.clear();
		assert_eq!(index.get(&new), None);
		index.insert(old, 9);
		assert_eq!(index.capacity_bytes(), capacity);
		assert_eq!(index.get(&old), Some(&9));
	}
}
