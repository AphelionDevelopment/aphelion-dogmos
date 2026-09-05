use super::*;

impl DogmosWorld {
	pub(super) fn unlink_turf_mixture(&mut self, turf_slot: u32) {
		let mixture = self
			.turfs
			.get(turf_slot as usize)
			.and_then(|slot| slot.turf.as_ref())
			.and_then(|turf| turf.mixture);
		if let Some(mixture) = mixture {
			if let Some(turfs) = self.mixture_turfs.get_mut(&mixture) {
				turfs.remove(&turf_slot);
				if turfs.is_empty() {
					self.mixture_turfs.remove(&mixture);
				}
			}
		}
	}

	pub(super) fn unlink_mixture_edge(&mut self, key: EdgeKey) {
		for slot in [key.left, key.right] {
			if let Some(edges) = self.mixture_edges.get_mut(&slot) {
				edges.remove(&key);
				if edges.is_empty() {
					self.mixture_edges.remove(&slot);
				}
			}
		}
	}

	pub(super) fn remove_incident_mixture_edges(&mut self, slot: u32) {
		if let Some(edges) = self.mixture_edges.remove(&slot) {
			for key in edges {
				self.edges.remove(&key);
				self.unlink_mixture_edge(key);
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	fn assert_indexes(world: &DogmosWorld) {
		let mut edges = BTreeMap::<u32, BTreeSet<EdgeKey>>::new();
		for &key in world.edges.keys() {
			for slot in [key.left, key.right] {
				edges.entry(slot).or_default().insert(key);
			}
		}
		assert_eq!(edges, world.mixture_edges);
		let mut turfs = BTreeMap::<MixtureHandle, BTreeSet<u32>>::new();
		for (slot, record) in world.turfs.iter().enumerate() {
			if let Some(mixture) = record.turf.as_ref().and_then(|turf| turf.mixture) {
				turfs.entry(mixture).or_default().insert(slot as u32);
			}
		}
		assert_eq!(turfs, world.mixture_turfs);
	}
	#[test]
	fn reverse_ownership_matches_full_scan_after_reassignment_and_generation_reuse() {
		let mut world = DogmosWorld::new(1024 * 1024);
		let mixtures = [0, 1, 2].map(|slot| MixtureHandle {
			slot,
			generation: 1,
		});
		world
			.apply_lifecycle(&mixtures.map(|handle| LifecycleMutation {
				action: LifecycleAction::Register,
				handle,
			}))
			.unwrap();
		let turfs = [0, 1, 2].map(|slot| TurfHandle {
			slot,
			generation: 1,
		});
		world
			.apply_turf_lifecycle(&turfs.map(|handle| TurfLifecycleMutation::Register {
				handle,
				mixture: Some(mixtures[0]),
			}))
			.unwrap();
		world
			.apply_adjacency(&[
				AdjacencyMutation {
					left: mixtures[0],
					right: mixtures[1],
					conductivity: 0.1,
				},
				AdjacencyMutation {
					left: mixtures[1],
					right: mixtures[2],
					conductivity: 0.1,
				},
			])
			.unwrap();
		assert_indexes(&world);
		world
			.apply_turf_lifecycle(&[TurfLifecycleMutation::Register {
				handle: turfs[0],
				mixture: Some(mixtures[2]),
			}])
			.unwrap();
		assert_indexes(&world);
		world
			.apply_lifecycle(&[LifecycleMutation {
				action: LifecycleAction::Unregister,
				handle: mixtures[0],
			}])
			.unwrap();
		assert_indexes(&world);
		assert_eq!(
			world.require_turf_handle(turfs[0]).unwrap().mixture,
			Some(mixtures[2])
		);
		assert!(world
			.require_turf_handle(turfs[1])
			.unwrap()
			.mixture
			.is_none());
		let replacement = MixtureHandle {
			slot: 0,
			generation: 2,
		};
		world
			.apply_lifecycle(&[LifecycleMutation {
				action: LifecycleAction::Register,
				handle: replacement,
			}])
			.unwrap();
		world
			.apply_adjacency(&[AdjacencyMutation {
				left: replacement,
				right: mixtures[2],
				conductivity: 0.1,
			}])
			.unwrap();
		world
			.apply_turf_lifecycle(&[TurfLifecycleMutation::Register {
				handle: turfs[1],
				mixture: Some(replacement),
			}])
			.unwrap();
		assert_indexes(&world);
	}
}
