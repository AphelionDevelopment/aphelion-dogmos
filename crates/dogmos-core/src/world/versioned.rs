use std::sync::{
	atomic::{AtomicU8, Ordering},
	Arc,
};

const PENDING: u8 = 0;
const PUBLISHED: u8 = 1;
const CONFLICTED: u8 = 2;

/// Publishes a prepared set of records with one visibility change.
pub(super) struct Publication(AtomicU8);

impl Publication {
	pub(super) fn new() -> Arc<Self> {
		Arc::new(Self(AtomicU8::new(PENDING)))
	}
	pub(super) fn publish(&self) -> bool {
		self.0
			.compare_exchange(PENDING, PUBLISHED, Ordering::AcqRel, Ordering::Acquire)
			.is_ok()
	}
	fn visible(&self) -> bool {
		self.0.load(Ordering::Acquire) == PUBLISHED
	}
	fn invalidate(&self) {
		let _ = self
			.0
			.compare_exchange(PENDING, CONFLICTED, Ordering::AcqRel, Ordering::Acquire);
	}
}

/// Keeps tentative writes invisible and invalidates them when the authoritative value changes.
#[derive(Clone)]
pub(super) struct Versioned<T> {
	current: Option<T>,
	pending: Option<(Arc<Publication>, T)>,
}

impl<T> Default for Versioned<T> {
	fn default() -> Self {
		Self {
			current: None,
			pending: None,
		}
	}
}

impl<T> Versioned<T> {
	pub(super) fn new(current: Option<T>) -> Self {
		Self {
			current,
			pending: None,
		}
	}
	pub(super) fn as_ref(&self) -> Option<&T> {
		if let Some((publication, candidate)) = &self.pending {
			if publication.visible() {
				return Some(candidate);
			}
		}
		self.current.as_ref()
	}
	pub(super) fn as_mut(&mut self) -> Option<&mut T> {
		if let Some((publication, candidate)) = self.pending.take() {
			if publication.visible() {
				self.current = Some(candidate);
			} else {
				publication.invalidate();
			}
		}
		self.current.as_mut()
	}
	pub(super) fn is_some(&self) -> bool {
		self.as_ref().is_some()
	}
	pub(super) fn is_none(&self) -> bool {
		self.as_ref().is_none()
	}
	pub(super) fn stage(&mut self, candidate: T, publication: &Arc<Publication>) {
		if self
			.pending
			.as_ref()
			.is_some_and(|(token, _)| Arc::ptr_eq(token, publication))
		{
			self.pending.as_mut().unwrap().1 = candidate;
			return;
		}
		self.as_mut();
		self.pending = Some((Arc::clone(publication), candidate));
	}
}

impl<T: Clone> Versioned<T> {
	pub(super) fn prepare(&mut self, publication: &Arc<Publication>) {
		if self
			.pending
			.as_ref()
			.is_some_and(|(token, _)| Arc::ptr_eq(token, publication))
		{
			return;
		}
		if let Some(candidate) = self.as_ref().cloned() {
			self.stage(candidate, publication);
		}
	}
	pub(super) fn prepared_mut(&mut self, publication: &Arc<Publication>) -> &mut T {
		self.prepare(publication);
		&mut self.pending.as_mut().expect("prepared record exists").1
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn publication_is_atomic_and_conflicts_do_not_hide_new_writes() {
		let mut left = Versioned::new(Some(1));
		let mut right = Versioned::new(Some(2));
		let batch = Publication::new();
		left.stage(3, &batch);
		right.stage(4, &batch);
		assert_eq!(left.as_ref(), Some(&1));
		assert_eq!(right.as_ref(), Some(&2));
		assert!(batch.publish());
		assert_eq!(left.as_ref(), Some(&3));
		assert_eq!(right.as_ref(), Some(&4));
		let conflict = Publication::new();
		left.stage(5, &conflict);
		right.stage(6, &conflict);
		*right.as_mut().unwrap() = 7;
		assert!(!conflict.publish());
		assert_eq!(left.as_ref(), Some(&3));
		assert_eq!(right.as_ref(), Some(&7));
	}
}
