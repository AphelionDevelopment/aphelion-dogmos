use dogmos_core::{
	world::{
		AdjacencyMutation as CoreAdjacencyMutation, DogmosWorld,
		LifecycleAction as CoreLifecycleAction, LifecycleMutation as CoreLifecycleMutation,
		MixtureStateMutation as CoreMixtureStateMutation, WorldError, WorldStage,
	},
	MixtureHandle,
};
use dogmos_protocol::{
	AdjacencyMutation, CallbackBatchHeader, CallbackEvent, CallbackEventKind, LifecycleAction,
	LifecycleMutation, MixtureSnapshot, MixtureStateMutation, ScalarValue, SimulationStage,
	WireHandle, CALLBACK_BATCH_HEADER_LEN, CALLBACK_EVENT_LEN, MAX_GAS_SLOTS,
};
use std::{collections::VecDeque, error::Error, fmt};

const _: () = assert!(MAX_GAS_SLOTS == dogmos_core::MAX_GAS_SLOTS);

#[derive(Debug, PartialEq)]
pub enum StateError {
	UnknownHandle(WireHandle),
	StaleHandle {
		requested: WireHandle,
		current: u32,
	},
	RevisionMismatch {
		handle: WireHandle,
		expected: u32,
		actual: u32,
	},
	RevisionExhausted(WireHandle),
	DuplicateMixtureState(u32),
	InvalidMixtureState,
	SelfAdjacency(u32),
	InvalidConductivity,
	InvalidSecondsPerTick,
	Graph(String),
	State(String),
	StateCapacityExceeded,
	AllocationFailed,
	CallbackBackpressure,
	CallbackOutputTooSmall,
	CallbackSequenceExhausted,
	Cancelled,
}

impl fmt::Display for StateError {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(formatter, "{self:?}")
	}
}

impl Error for StateError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StageResult {
	pub work_items: u32,
	pub callback_events: u32,
}

#[derive(Clone, Copy)]
struct PendingCallbackEvent {
	kind: CallbackEventKind,
	flags: u16,
	subject: WireHandle,
	target: WireHandle,
	values: [ScalarValue; 4],
	aux: u32,
}

impl PendingCallbackEvent {
	fn with_sequence(self, sequence: u64) -> CallbackEvent {
		CallbackEvent {
			sequence,
			kind: self.kind,
			flags: self.flags,
			subject: self.subject,
			target: self.target,
			values: self.values,
			aux: self.aux,
		}
	}
}

pub struct ServiceState {
	world: DogmosWorld,
	callback_events: VecDeque<CallbackEvent>,
	max_callback_events: u32,
	callback_high_water: u32,
	callback_rejected: u64,
	next_callback_sequence: u64,
}

impl ServiceState {
	pub fn new(max_world_bytes: u64, max_callback_events: u32) -> Self {
		Self {
			world: DogmosWorld::new(max_world_bytes),
			callback_events: VecDeque::new(),
			max_callback_events,
			callback_high_water: 0,
			callback_rejected: 0,
			next_callback_sequence: 1,
		}
	}

	pub fn enqueue_diagnostic_callbacks(&mut self, count: u32) -> Result<u32, StateError> {
		let callback = PendingCallbackEvent {
			kind: CallbackEventKind::Diagnostic,
			flags: 0,
			subject: WireHandle {
				slot: 0,
				generation: 0,
			},
			target: WireHandle {
				slot: 0,
				generation: 0,
			},
			values: [ScalarValue(0.0); 4],
			aux: 0,
		};
		if count == 1 {
			return self.enqueue_callback_batch(std::slice::from_ref(&callback));
		}
		callback
			.with_sequence(self.next_callback_sequence)
			.encode()
			.map_err(|error| StateError::State(error.to_string()))?;
		let (new_depth, next_callback_sequence) = self.prepare_callback_enqueue(count)?;
		for index in 0..count {
			self.callback_events
				.push_back(callback.with_sequence(self.next_callback_sequence + u64::from(index)));
		}
		self.finish_callback_enqueue(new_depth, next_callback_sequence);
		Ok(count)
	}

	fn enqueue_callback_batch(
		&mut self,
		callbacks: &[PendingCallbackEvent],
	) -> Result<u32, StateError> {
		let count = u32::try_from(callbacks.len()).map_err(|_| StateError::CallbackBackpressure)?;
		for callback in callbacks {
			callback
				.with_sequence(self.next_callback_sequence)
				.encode()
				.map_err(|error| StateError::State(error.to_string()))?;
		}
		let (new_depth, next_callback_sequence) = self.prepare_callback_enqueue(count)?;
		for (index, callback) in callbacks.iter().copied().enumerate() {
			self.callback_events
				.push_back(callback.with_sequence(self.next_callback_sequence + index as u64));
		}
		self.finish_callback_enqueue(new_depth, next_callback_sequence);
		Ok(count)
	}

	fn prepare_callback_enqueue(&mut self, count: u32) -> Result<(u32, u64), StateError> {
		let depth = u32::try_from(self.callback_events.len())
			.map_err(|_| StateError::CallbackBackpressure)?;
		let Some(new_depth) = depth.checked_add(count) else {
			self.callback_rejected = self.callback_rejected.saturating_add(u64::from(count));
			return Err(StateError::CallbackBackpressure);
		};
		if new_depth > self.max_callback_events {
			self.callback_rejected = self.callback_rejected.saturating_add(u64::from(count));
			return Err(StateError::CallbackBackpressure);
		}
		let next_callback_sequence = self
			.next_callback_sequence
			.checked_add(u64::from(count))
			.ok_or(StateError::CallbackSequenceExhausted)?;
		if count > 0 {
			self.callback_events
				.try_reserve_exact(count as usize)
				.map_err(|_| StateError::AllocationFailed)?;
		}
		Ok((new_depth, next_callback_sequence))
	}

	fn finish_callback_enqueue(&mut self, new_depth: u32, next_callback_sequence: u64) {
		self.next_callback_sequence = next_callback_sequence;
		self.callback_high_water = self.callback_high_water.max(new_depth);
	}

	pub fn drain_callbacks(
		&mut self,
		max_events: u32,
		output: &mut [u8],
	) -> Result<usize, StateError> {
		if output.len() < CALLBACK_BATCH_HEADER_LEN {
			return Err(StateError::CallbackOutputTooSmall);
		}
		let output_event_capacity = (output.len() - CALLBACK_BATCH_HEADER_LEN) / CALLBACK_EVENT_LEN;
		let returned = self
			.callback_events
			.len()
			.min(max_events as usize)
			.min(output_event_capacity);
		for index in 0..returned {
			let event = self
				.callback_events
				.pop_front()
				.expect("returned count was bounded by queue length");
			let start = CALLBACK_BATCH_HEADER_LEN + index * CALLBACK_EVENT_LEN;
			output[start..start + CALLBACK_EVENT_LEN].copy_from_slice(
				&event
					.encode()
					.map_err(|error| StateError::State(error.to_string()))?,
			);
		}
		let header = CallbackBatchHeader {
			returned: returned as u32,
			remaining: self.callback_events.len() as u32,
			capacity: self.max_callback_events,
			high_water: self.callback_high_water,
			rejected: self.callback_rejected,
		};
		output[..CALLBACK_BATCH_HEADER_LEN].copy_from_slice(&header.encode());
		Ok(CALLBACK_BATCH_HEADER_LEN + returned * CALLBACK_EVENT_LEN)
	}

	pub fn apply_lifecycle(&mut self, mutations: &[LifecycleMutation]) -> Result<u32, StateError> {
		let mutations = mutations
			.iter()
			.map(|mutation| CoreLifecycleMutation {
				action: match mutation.action {
					LifecycleAction::Register => CoreLifecycleAction::Register,
					LifecycleAction::Unregister => CoreLifecycleAction::Unregister,
				},
				handle: core_handle(mutation.handle),
			})
			.collect::<Vec<_>>();
		self.world
			.apply_lifecycle(&mutations)
			.map_err(map_world_error)
	}

	pub fn apply_adjacency(&mut self, mutations: &[AdjacencyMutation]) -> Result<u32, StateError> {
		let mutations = mutations
			.iter()
			.map(|mutation| CoreAdjacencyMutation {
				left: core_handle(mutation.left),
				right: core_handle(mutation.right),
				conductivity: mutation.conductivity.0 as f32,
			})
			.collect::<Vec<_>>();
		self.world
			.apply_adjacency(&mutations)
			.map_err(map_world_error)
	}

	pub fn apply_mixture_state(
		&mut self,
		mutations: &[MixtureStateMutation],
	) -> Result<u32, StateError> {
		let mutations = mutations
			.iter()
			.map(|mutation| {
				let mut gases = [0.0; MAX_GAS_SLOTS];
				for (output, value) in gases.iter_mut().zip(mutation.gases) {
					*output = value.0 as f32;
				}
				CoreMixtureStateMutation {
					handle: core_handle(mutation.handle),
					expected_revision: mutation.expected_revision,
					temperature: mutation.temperature.0 as f32,
					volume: mutation.volume.0 as f32,
					gases,
				}
			})
			.collect::<Vec<_>>();
		self.world
			.apply_mixture_state(&mutations)
			.map_err(map_world_error)
	}

	pub fn snapshot(&self, handle: WireHandle) -> Result<MixtureSnapshot, StateError> {
		let mixture = self
			.world
			.snapshot(core_handle(handle))
			.map_err(map_world_error)?;
		let mut gases = [ScalarValue(0.0); MAX_GAS_SLOTS];
		for (output, value) in gases.iter_mut().zip(mixture.gases) {
			*output = ScalarValue(f64::from(value));
		}
		Ok(MixtureSnapshot {
			revision: mixture.revision,
			gas_count: MAX_GAS_SLOTS as u32,
			temperature: ScalarValue(f64::from(mixture.temperature)),
			volume: ScalarValue(f64::from(mixture.volume)),
			gases,
		})
	}

	pub fn process_stage_cancellable(
		&mut self,
		stage: SimulationStage,
		seconds_per_tick: f64,
		should_cancel: impl FnMut() -> bool,
	) -> Result<StageResult, StateError> {
		let stage = match stage {
			SimulationStage::ProcessTurfs => WorldStage::ProcessTurfs,
			SimulationStage::ProcessTurfEqualize => WorldStage::Equalize,
			SimulationStage::ProcessExcitedGroups => WorldStage::React,
			SimulationStage::ProcessTurfHeat => WorldStage::TurfHeat,
		};
		let result = self
			.world
			.process_stage_cancellable(stage, seconds_per_tick, should_cancel)
			.map_err(map_world_error)?;
		Ok(StageResult {
			work_items: result.work_items,
			callback_events: 0,
		})
	}

	#[cfg(test)]
	fn edge_count(&self) -> usize {
		self.world.edge_count()
	}

	#[cfg(test)]
	fn slot_count(&self) -> usize {
		self.world.slot_count()
	}
}

fn core_handle(handle: WireHandle) -> MixtureHandle {
	MixtureHandle {
		slot: handle.slot,
		generation: handle.generation,
	}
}

fn wire_handle(handle: MixtureHandle) -> WireHandle {
	WireHandle {
		slot: handle.slot,
		generation: handle.generation,
	}
}

fn map_world_error(error: WorldError) -> StateError {
	match error {
		WorldError::GasMetadata(error) => StateError::State(error.to_string()),
		WorldError::GasRegistryAlreadyInstalled => {
			StateError::State(WorldError::GasRegistryAlreadyInstalled.to_string())
		}
		WorldError::GasRegistryInstallationTooLate => {
			StateError::State(WorldError::GasRegistryInstallationTooLate.to_string())
		}
		WorldError::GasRegistryMissing => {
			StateError::State(WorldError::GasRegistryMissing.to_string())
		}
		WorldError::ReactionMetadata(error) => StateError::State(error.to_string()),
		WorldError::ReactionRegistryAlreadyInstalled => {
			StateError::State(WorldError::ReactionRegistryAlreadyInstalled.to_string())
		}
		WorldError::ReactionRegistryInstallationTooLate => {
			StateError::State(WorldError::ReactionRegistryInstallationTooLate.to_string())
		}
		WorldError::ReactionRegistryMissing => {
			StateError::State(WorldError::ReactionRegistryMissing.to_string())
		}
		WorldError::UnknownHandle(handle) => StateError::UnknownHandle(wire_handle(handle)),
		WorldError::StaleHandle { requested, current } => StateError::StaleHandle {
			requested: wire_handle(requested),
			current,
		},
		WorldError::RevisionMismatch {
			handle,
			expected,
			actual,
		} => StateError::RevisionMismatch {
			handle: wire_handle(handle),
			expected,
			actual,
		},
		WorldError::RevisionExhausted(handle) => StateError::RevisionExhausted(wire_handle(handle)),
		WorldError::DuplicateMixtureState(slot) => StateError::DuplicateMixtureState(slot),
		WorldError::InvalidMixtureState => StateError::InvalidMixtureState,
		WorldError::SelfAdjacency(slot) => StateError::SelfAdjacency(slot),
		WorldError::InvalidConductivity => StateError::InvalidConductivity,
		WorldError::InvalidSecondsPerTick => StateError::InvalidSecondsPerTick,
		WorldError::Graph(message) => StateError::Graph(message),
		WorldError::State(message) => StateError::State(message),
		WorldError::StateCapacityExceeded => StateError::StateCapacityExceeded,
		WorldError::AllocationFailed => StateError::AllocationFailed,
		WorldError::Cancelled => StateError::Cancelled,
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use dogmos_core::metadata::{GasId, GasMetadataError, ReactionId, ReactionMetadataError};
	use dogmos_protocol::{
		AdjacencyMutation, CallbackBatchHeader, CallbackEvent, LifecycleAction, LifecycleMutation,
		MixtureStateMutation, ScalarValue, SimulationStage, WireHandle, CALLBACK_BATCH_HEADER_LEN,
		CALLBACK_EVENT_LEN, MAX_GAS_SLOTS,
	};

	fn handle(slot: u32, generation: u32) -> WireHandle {
		WireHandle { slot, generation }
	}

	#[test]
	fn unexposed_metadata_errors_remain_diagnostic_internal_state_errors() {
		assert!(matches!(
			map_world_error(WorldError::GasMetadata(
				GasMetadataError::InvalidSpecificHeat(GasId(0))
			)),
			StateError::State(message) if message.contains("InvalidSpecificHeat")
		));
		assert!(matches!(
			map_world_error(WorldError::GasRegistryAlreadyInstalled),
			StateError::State(message) if message.contains("GasRegistryAlreadyInstalled")
		));
		assert!(matches!(
			map_world_error(WorldError::GasRegistryInstallationTooLate),
			StateError::State(message) if message.contains("GasRegistryInstallationTooLate")
		));
		assert!(matches!(
			map_world_error(WorldError::GasRegistryMissing),
			StateError::State(message) if message.contains("GasRegistryMissing")
		));
		assert!(matches!(
			map_world_error(WorldError::ReactionMetadata(
				ReactionMetadataError::InvalidPriority(ReactionId(0))
			)),
			StateError::State(message) if message.contains("InvalidPriority")
		));
		assert!(matches!(
			map_world_error(WorldError::ReactionRegistryAlreadyInstalled),
			StateError::State(message) if message.contains("ReactionRegistryAlreadyInstalled")
		));
		assert!(matches!(
			map_world_error(WorldError::ReactionRegistryInstallationTooLate),
			StateError::State(message) if message.contains("ReactionRegistryInstallationTooLate")
		));
		assert!(matches!(
			map_world_error(WorldError::ReactionRegistryMissing),
			StateError::State(message) if message.contains("ReactionRegistryMissing")
		));
	}

	#[test]
	fn lifecycle_adjacency_snapshot_and_stage_use_service_owned_state() {
		let mut state = ServiceState::new(1024 * 1024, 1024);
		assert_eq!(
			state
				.apply_lifecycle(&[
					LifecycleMutation {
						action: LifecycleAction::Register,
						handle: handle(0, 1),
					},
					LifecycleMutation {
						action: LifecycleAction::Register,
						handle: handle(1, 1),
					},
				])
				.unwrap(),
			2
		);
		assert_eq!(state.snapshot(handle(0, 1)).unwrap().gas_count, 32);
		assert_eq!(
			state
				.apply_adjacency(&[AdjacencyMutation {
					left: handle(0, 1),
					right: handle(1, 1),
					conductivity: ScalarValue(0.75),
				}])
				.unwrap(),
			1
		);
		let result = state
			.process_stage_cancellable(SimulationStage::ProcessTurfs, 0.5, || false)
			.unwrap();
		assert_eq!(result.work_items, 2);
		assert_eq!(result.callback_events, 0);
	}

	#[test]
	fn wire_state_batches_seed_authoritative_service_mixtures() {
		let mut state = ServiceState::new(1024 * 1024, 1024);
		state
			.apply_lifecycle(&[LifecycleMutation {
				action: LifecycleAction::Register,
				handle: handle(0, 1),
			}])
			.unwrap();
		let mut gases = [ScalarValue(0.0); MAX_GAS_SLOTS];
		gases[0] = ScalarValue(18.5);
		assert_eq!(
			state
				.apply_mixture_state(&[MixtureStateMutation {
					handle: handle(0, 1),
					expected_revision: 0,
					temperature: ScalarValue(293.15),
					volume: ScalarValue(2500.0),
					gases,
				}])
				.unwrap(),
			1
		);
		let snapshot = state.snapshot(handle(0, 1)).unwrap();
		assert_eq!(snapshot.revision, 1);
		assert!((snapshot.temperature.0 - 293.15).abs() < 0.0001);
		assert_eq!(snapshot.volume, ScalarValue(2500.0));
		assert_eq!(snapshot.gases[0], ScalarValue(18.5));
	}

	#[test]
	fn stale_handles_and_unknown_adjacency_are_rejected() {
		let mut state = ServiceState::new(1024 * 1024, 1024);
		state
			.apply_lifecycle(&[LifecycleMutation {
				action: LifecycleAction::Register,
				handle: handle(0, 2),
			}])
			.unwrap();
		assert!(matches!(
			state.snapshot(handle(0, 1)),
			Err(StateError::StaleHandle { .. })
		));
		assert!(matches!(
			state.apply_adjacency(&[AdjacencyMutation {
				left: handle(0, 2),
				right: handle(1, 1),
				conductivity: ScalarValue(1.0),
			}]),
			Err(StateError::UnknownHandle(_))
		));
	}

	#[test]
	fn cancelled_stage_does_not_commit_partial_mixture_revisions() {
		let mut state = ServiceState::new(1024 * 1024, 1024);
		state
			.apply_lifecycle(&[
				LifecycleMutation {
					action: LifecycleAction::Register,
					handle: handle(0, 1),
				},
				LifecycleMutation {
					action: LifecycleAction::Register,
					handle: handle(1, 1),
				},
			])
			.unwrap();
		state
			.apply_adjacency(&[AdjacencyMutation {
				left: handle(0, 1),
				right: handle(1, 1),
				conductivity: ScalarValue(0.5),
			}])
			.unwrap();
		assert_eq!(
			state.process_stage_cancellable(SimulationStage::ProcessTurfs, 0.5, || true),
			Err(StateError::Cancelled)
		);
		assert_eq!(state.snapshot(handle(0, 1)).unwrap().revision, 0);
		assert_eq!(state.snapshot(handle(1, 1)).unwrap().revision, 0);
	}

	#[test]
	fn unregister_releases_state_and_incident_edges() {
		let mut state = ServiceState::new(1024 * 1024, 1024);
		state
			.apply_lifecycle(&[
				LifecycleMutation {
					action: LifecycleAction::Register,
					handle: handle(0, 1),
				},
				LifecycleMutation {
					action: LifecycleAction::Register,
					handle: handle(1, 1),
				},
			])
			.unwrap();
		state
			.apply_adjacency(&[AdjacencyMutation {
				left: handle(0, 1),
				right: handle(1, 1),
				conductivity: ScalarValue(0.5),
			}])
			.unwrap();
		state
			.apply_lifecycle(&[LifecycleMutation {
				action: LifecycleAction::Unregister,
				handle: handle(0, 1),
			}])
			.unwrap();
		assert!(matches!(
			state.snapshot(handle(0, 1)),
			Err(StateError::UnknownHandle(_))
		));
		assert_eq!(state.edge_count(), 0);
	}

	#[test]
	fn sparse_slots_cannot_exceed_the_negotiated_service_budget() {
		let mut state = ServiceState::new(1024, 3);
		assert_eq!(
			state.apply_lifecycle(&[LifecycleMutation {
				action: LifecycleAction::Register,
				handle: handle(1_000_000, 1),
			}]),
			Err(StateError::StateCapacityExceeded)
		);
		assert_eq!(state.slot_count(), 0);
	}

	#[test]
	fn callback_queue_is_bounded_atomic_and_deterministic() {
		let mut state = ServiceState::new(1024 * 1024, 3);
		assert_eq!(state.enqueue_diagnostic_callbacks(3).unwrap(), 3);
		assert_eq!(
			state.enqueue_diagnostic_callbacks(1),
			Err(StateError::CallbackBackpressure)
		);

		let mut first = [0_u8; CALLBACK_BATCH_HEADER_LEN + 2 * CALLBACK_EVENT_LEN];
		let first_len = state.drain_callbacks(2, &mut first).unwrap();
		assert_eq!(first_len, first.len());
		let first_header =
			CallbackBatchHeader::decode(&first[..CALLBACK_BATCH_HEADER_LEN]).unwrap();
		assert_eq!(first_header.returned, 2);
		assert_eq!(first_header.remaining, 1);
		assert_eq!(first_header.capacity, 3);
		assert_eq!(first_header.high_water, 3);
		assert_eq!(first_header.rejected, 1);
		let first_event = CallbackEvent::decode(
			&first[CALLBACK_BATCH_HEADER_LEN..CALLBACK_BATCH_HEADER_LEN + CALLBACK_EVENT_LEN],
		)
		.unwrap();
		let second_event =
			CallbackEvent::decode(&first[CALLBACK_BATCH_HEADER_LEN + CALLBACK_EVENT_LEN..])
				.unwrap();
		assert_eq!(first_event.sequence, 1);
		assert_eq!(second_event.sequence, 2);

		let mut second = [0_u8; CALLBACK_BATCH_HEADER_LEN + CALLBACK_EVENT_LEN];
		let second_len = state.drain_callbacks(3, &mut second).unwrap();
		assert_eq!(second_len, second.len());
		let second_header =
			CallbackBatchHeader::decode(&second[..CALLBACK_BATCH_HEADER_LEN]).unwrap();
		assert_eq!(second_header.returned, 1);
		assert_eq!(second_header.remaining, 0);
		assert_eq!(
			CallbackEvent::decode(&second[CALLBACK_BATCH_HEADER_LEN..])
				.unwrap()
				.sequence,
			3
		);

		state.next_callback_sequence = u64::MAX;
		assert_eq!(
			state.enqueue_diagnostic_callbacks(1),
			Err(StateError::CallbackSequenceExhausted)
		);
		assert!(state.callback_events.is_empty());
	}

	#[test]
	fn gameplay_callback_batch_is_enqueued_all_or_nothing() {
		let mut state = ServiceState::new(1024 * 1024, 2);
		let callbacks = [
			PendingCallbackEvent {
				kind: CallbackEventKind::PressureDifference,
				flags: 0,
				subject: handle(10, 2),
				target: handle(11, 4),
				values: [
					ScalarValue(125.0),
					ScalarValue(0.0),
					ScalarValue(0.0),
					ScalarValue(0.0),
				],
				aux: 0,
			},
			PendingCallbackEvent {
				kind: CallbackEventKind::DecompressionFloorRip,
				flags: 0,
				subject: handle(12, 6),
				target: handle(0, 0),
				values: [
					ScalarValue(45.0),
					ScalarValue(0.0),
					ScalarValue(0.0),
					ScalarValue(0.0),
				],
				aux: 0,
			},
		];

		assert_eq!(state.enqueue_callback_batch(&callbacks).unwrap(), 2);
		assert_eq!(
			state.enqueue_callback_batch(&callbacks[..1]),
			Err(StateError::CallbackBackpressure)
		);

		let mut output = [0_u8; CALLBACK_BATCH_HEADER_LEN + 2 * CALLBACK_EVENT_LEN];
		state.drain_callbacks(2, &mut output).unwrap();
		let first = CallbackEvent::decode(
			&output[CALLBACK_BATCH_HEADER_LEN..CALLBACK_BATCH_HEADER_LEN + CALLBACK_EVENT_LEN],
		)
		.unwrap();
		let second =
			CallbackEvent::decode(&output[CALLBACK_BATCH_HEADER_LEN + CALLBACK_EVENT_LEN..])
				.unwrap();
		assert_eq!(first.sequence, 1);
		assert_eq!(first.kind, CallbackEventKind::PressureDifference);
		assert_eq!(second.sequence, 2);
		assert_eq!(second.kind, CallbackEventKind::DecompressionFloorRip);
	}
}
