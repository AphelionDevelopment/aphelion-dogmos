use crate::metadata::TurfHandle;
use std::collections::BTreeSet;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FrontierError {
	InvalidEpoch(u64),
	EpochConflict {
		committed: Option<u64>,
		uploading: Option<u64>,
		requested: u64,
	},
	CountExceeded {
		actual: u32,
		maximum: u32,
	},
	RangeOutOfBounds {
		offset: u32,
		count: u32,
		expected: u32,
	},
	RangeAlreadyReceived {
		offset: u32,
		count: u32,
	},
	Incomplete {
		epoch: u64,
		expected: u32,
		received: u32,
	},
	DuplicateHandle(TurfHandle),
	AllocationFailed,
}

#[derive(Default)]
pub(crate) struct FrontierState {
	committed_epoch: Option<u64>,
	committed: Vec<TurfHandle>,
	upload_epoch: Option<u64>,
	upload_expected: u32,
	upload_received: u32,
	staging: Vec<TurfHandle>,
	received_bits: Vec<u64>,
}

impl FrontierState {
	pub(crate) fn begin(
		&mut self,
		epoch: u64,
		expected: u32,
		maximum: u32,
	) -> Result<(), FrontierError> {
		if epoch == 0 {
			return Err(FrontierError::InvalidEpoch(epoch));
		}
		if expected > maximum {
			return Err(FrontierError::CountExceeded {
				actual: expected,
				maximum,
			});
		}
		if self
			.committed_epoch
			.is_some_and(|committed| epoch <= committed)
			|| self
				.upload_epoch
				.is_some_and(|uploading| epoch <= uploading)
		{
			return Err(FrontierError::EpochConflict {
				committed: self.committed_epoch,
				uploading: self.upload_epoch,
				requested: epoch,
			});
		}

		let expected_usize = expected as usize;
		self.staging
			.try_reserve(expected_usize.saturating_sub(self.staging.capacity()))
			.map_err(|_| FrontierError::AllocationFailed)?;
		self.staging.resize(
			expected_usize,
			TurfHandle {
				slot: 0,
				generation: 0,
			},
		);
		let word_count = expected_usize.div_ceil(u64::BITS as usize);
		self.received_bits
			.try_reserve(word_count.saturating_sub(self.received_bits.capacity()))
			.map_err(|_| FrontierError::AllocationFailed)?;
		self.received_bits.resize(word_count, 0);
		self.received_bits.fill(0);
		self.upload_epoch = Some(epoch);
		self.upload_expected = expected;
		self.upload_received = 0;
		Ok(())
	}

	pub(crate) fn append(
		&mut self,
		epoch: u64,
		offset: u32,
		handles: &[TurfHandle],
	) -> Result<u32, FrontierError> {
		if self.upload_epoch != Some(epoch) {
			return Err(FrontierError::EpochConflict {
				committed: self.committed_epoch,
				uploading: self.upload_epoch,
				requested: epoch,
			});
		}
		let count = u32::try_from(handles.len()).map_err(|_| FrontierError::RangeOutOfBounds {
			offset,
			count: u32::MAX,
			expected: self.upload_expected,
		})?;
		let Some(end) = offset.checked_add(count) else {
			return Err(FrontierError::RangeOutOfBounds {
				offset,
				count,
				expected: self.upload_expected,
			});
		};
		if end > self.upload_expected {
			return Err(FrontierError::RangeOutOfBounds {
				offset,
				count,
				expected: self.upload_expected,
			});
		}
		if (offset..end).any(|index| self.is_received(index)) {
			return Err(FrontierError::RangeAlreadyReceived { offset, count });
		}
		for (relative_index, handle) in handles.iter().enumerate() {
			let index = offset as usize + relative_index;
			self.staging[index] = *handle;
			self.received_bits[index / u64::BITS as usize] |= 1 << (index % u64::BITS as usize);
		}
		self.upload_received += count;
		Ok(count)
	}

	pub(crate) fn pending(&self, epoch: u64) -> Result<&[TurfHandle], FrontierError> {
		if self.upload_epoch != Some(epoch) {
			return Err(FrontierError::EpochConflict {
				committed: self.committed_epoch,
				uploading: self.upload_epoch,
				requested: epoch,
			});
		}
		if self.upload_received != self.upload_expected {
			return Err(FrontierError::Incomplete {
				epoch,
				expected: self.upload_expected,
				received: self.upload_received,
			});
		}
		let mut unique = BTreeSet::new();
		for handle in &self.staging {
			if !unique.insert(*handle) {
				return Err(FrontierError::DuplicateHandle(*handle));
			}
		}
		Ok(&self.staging)
	}

	pub(crate) fn commit(&mut self, epoch: u64) -> Result<u32, FrontierError> {
		self.pending(epoch)?;
		std::mem::swap(&mut self.committed, &mut self.staging);
		self.staging.clear();
		self.committed_epoch = Some(epoch);
		self.upload_epoch = None;
		self.upload_expected = 0;
		self.upload_received = 0;
		Ok(self.committed.len() as u32)
	}

	pub(crate) fn committed_epoch(&self) -> Option<u64> {
		self.committed_epoch
	}

	pub(crate) fn committed(&self) -> &[TurfHandle] {
		&self.committed
	}

	pub(crate) fn upload_bytes(&self) -> u64 {
		(self.staging.capacity() * std::mem::size_of::<TurfHandle>()
			+ self.received_bits.capacity() * std::mem::size_of::<u64>()) as u64
	}

	fn is_received(&self, index: u32) -> bool {
		let index = index as usize;
		self.received_bits[index / u64::BITS as usize] & (1 << (index % u64::BITS as usize)) != 0
	}
}
