use dogmos_core::{
	metadata::{
		FireProductRule, GasFireRole, GasId, GasMetadata, GasProduct, GasRequirement,
		NativeReactionKind, ReactionExecution, ReactionId, ReactionMetadata, TurfHandle,
	},
	world::{
		AdjacencyMutation as CoreAdjacencyMutation, Command as CoreCommand,
		CommandResult as CoreCommandResult, DogmosWorld, LifecycleAction as CoreLifecycleAction,
		LifecycleMutation as CoreLifecycleMutation,
		MixtureStateMutation as CoreMixtureStateMutation,
		ReactionContinuationToken as CoreContinuationToken,
		TurfAdjacencyMutation as CoreTurfAdjacencyMutation,
		TurfFirelockMutation as CoreTurfFirelockMutation,
		TurfHeatAdjacencyMutation as CoreTurfHeatAdjacencyMutation,
		TurfHeatMutation as CoreTurfHeatMutation, TurfHeatState as CoreTurfHeatState,
		TurfLifecycleMutation as CoreTurfLifecycleMutation, WorldError, WorldEvent, WorldStage,
	},
	MixtureHandle,
};
use dogmos_protocol::{
	AdjacencyMutation, CallbackBatchHeader, CallbackEvent, CallbackEventKind, ContinuationToken,
	GasMetadataRegistration, LifecycleAction, LifecycleMutation, MixtureAdjustment,
	MixtureCommandRequest, MixtureCommandResponse, MixtureSnapshot, MixtureStateMutation,
	ReactionMetadataRegistration, ScalarValue, ServiceTelemetry, SimulationStage,
	TurfAdjacencyMutation, TurfDestructionReason, TurfHeatAdjacencyMutation, TurfHeatMutation,
	TurfLifecycleMutation, WireFireProducts, WireGasFireRole, WireHandle, WireReactionExecution,
	CALLBACK_BATCH_HEADER_LEN, CALLBACK_EVENT_KIND_COUNT, CALLBACK_EVENT_LEN,
	CONTINUATION_TICK_MILLIS, DEFAULT_CONTINUATION_TIMEOUT_TICKS, MAX_GAS_SLOTS,
};
use std::{
	collections::{BTreeMap, BTreeSet, VecDeque},
	error::Error,
	fmt,
	time::Instant,
};

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
	InvalidMetadata,
	SelfAdjacency(u32),
	DuplicateTurfAdjacency {
		left: u32,
		right: u32,
	},
	InvalidConductivity,
	InvalidSecondsPerTick,
	StageNotImplemented(SimulationStage),
	Graph(String),
	State(String),
	StateCapacityExceeded,
	AllocationFailed,
	CallbackBackpressure,
	CallbackOutputTooSmall,
	CallbackSequenceExhausted,
	ContinuationCapacityExceeded,
	ContinuationIdExhausted,
	ContinuationDeadlineExhausted,
	UnknownContinuation(ContinuationToken),
	ContinuationWorldMismatch {
		expected: u32,
		actual: u32,
	},
	ContinuationTokenMismatch(ContinuationToken),
	ContinuationExpired(ContinuationToken),
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
	continuation: Option<ContinuationToken>,
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
			continuation: self.continuation,
		}
	}
}

#[derive(Clone, Copy)]
struct PendingContinuation {
	core_token: CoreContinuationToken,
	deadline_ticks: u64,
	turf: WireHandle,
	mixture: WireHandle,
}

#[derive(Clone, Copy)]
struct QueuedCallback {
	event: CallbackEvent,
	enqueued_ticks: u64,
}

pub struct ServiceState {
	world: DogmosWorld,
	callback_events: VecDeque<QueuedCallback>,
	max_callback_events: u32,
	callback_high_water: u32,
	callback_rejected: u64,
	callback_enqueued: u64,
	callback_drained: u64,
	callback_enqueued_by_kind: [u64; CALLBACK_EVENT_KIND_COUNT],
	callback_drained_by_kind: [u64; CALLBACK_EVENT_KIND_COUNT],
	callback_rejected_by_kind: [u64; CALLBACK_EVENT_KIND_COUNT],
	next_callback_sequence: u64,
	world_events: Vec<WorldEvent>,
	pending_callback_scratch: Vec<PendingCallbackEvent>,
	pending_continuation_scratch: Vec<(u64, PendingContinuation)>,
	expired_continuation_scratch: Vec<(u64, CoreContinuationToken)>,
	pending_continuations: BTreeMap<u64, PendingContinuation>,
	max_pending_continuations: u32,
	continuation_high_water: u32,
	continuation_timeouts: u64,
	next_continuation_id: u64,
	request_timeouts: u64,
	protocol_errors: u64,
	world_generation: u32,
	session_started_at: Instant,
}

impl ServiceState {
	#[cfg(test)]
	pub fn new(max_world_bytes: u64, max_callback_events: u32) -> Self {
		Self::new_for_world(max_world_bytes, max_callback_events, max_callback_events, 1)
	}

	pub fn new_for_world(
		max_world_bytes: u64,
		max_callback_events: u32,
		max_pending_continuations: u32,
		world_generation: u32,
	) -> Self {
		Self {
			world: DogmosWorld::new_with_capacities(
				max_world_bytes,
				max_callback_events,
				max_pending_continuations,
			),
			callback_events: VecDeque::new(),
			max_callback_events,
			callback_high_water: 0,
			callback_rejected: 0,
			callback_enqueued: 0,
			callback_drained: 0,
			callback_enqueued_by_kind: [0; CALLBACK_EVENT_KIND_COUNT],
			callback_drained_by_kind: [0; CALLBACK_EVENT_KIND_COUNT],
			callback_rejected_by_kind: [0; CALLBACK_EVENT_KIND_COUNT],
			next_callback_sequence: 1,
			world_events: Vec::new(),
			pending_callback_scratch: Vec::new(),
			pending_continuation_scratch: Vec::new(),
			expired_continuation_scratch: Vec::new(),
			pending_continuations: BTreeMap::new(),
			max_pending_continuations,
			continuation_high_water: 0,
			continuation_timeouts: 0,
			next_continuation_id: 1,
			request_timeouts: 0,
			protocol_errors: 0,
			world_generation,
			session_started_at: Instant::now(),
		}
	}

	pub fn telemetry(&self) -> ServiceTelemetry {
		self.telemetry_at(self.current_ticks())
	}

	fn telemetry_at(&self, now_ticks: u64) -> ServiceTelemetry {
		ServiceTelemetry {
			callback_depth: self.callback_events.len().try_into().unwrap_or(u32::MAX),
			callback_capacity: self.max_callback_events,
			callback_high_water: self.callback_high_water,
			continuation_depth: self.pending_continuation_count(),
			continuation_capacity: self.max_pending_continuations,
			continuation_high_water: self.continuation_high_water,
			oldest_callback_age_ticks: self.callback_events.front().map_or(0, |callback| {
				now_ticks.saturating_sub(callback.enqueued_ticks)
			}),
			callback_enqueued: self.callback_enqueued,
			callback_drained: self.callback_drained,
			callback_rejected: self.callback_rejected,
			continuation_timeouts: self.continuation_timeouts,
			request_timeouts: self.request_timeouts,
			protocol_errors: self.protocol_errors,
			callback_enqueued_by_kind: self.callback_enqueued_by_kind,
			callback_drained_by_kind: self.callback_drained_by_kind,
			callback_rejected_by_kind: self.callback_rejected_by_kind,
		}
	}

	pub fn record_protocol_error(&mut self) {
		self.protocol_errors = self.protocol_errors.saturating_add(1);
	}

	pub fn record_request_timeout(&mut self) {
		self.request_timeouts = self.request_timeouts.saturating_add(1);
	}

	pub fn enqueue_diagnostic_callbacks(&mut self, count: u32) -> Result<u32, StateError> {
		let now_ticks = self.current_ticks();
		self.enqueue_diagnostic_callbacks_at(count, now_ticks)
	}

	fn enqueue_diagnostic_callbacks_at(
		&mut self,
		count: u32,
		now_ticks: u64,
	) -> Result<u32, StateError> {
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
			continuation: None,
		};
		if count == 1 {
			return self.enqueue_callback_batch_at(std::slice::from_ref(&callback), now_ticks);
		}
		callback
			.with_sequence(self.next_callback_sequence)
			.encode()
			.map_err(|error| StateError::State(error.to_string()))?;
		let (new_depth, next_callback_sequence) = match self.prepare_callback_enqueue(count) {
			Ok(prepared) => prepared,
			Err(StateError::CallbackBackpressure) => {
				self.record_callback_rejected(CallbackEventKind::Diagnostic, count);
				return Err(StateError::CallbackBackpressure);
			}
			Err(error) => return Err(error),
		};
		for index in 0..count {
			self.callback_events.push_back(QueuedCallback {
				event: callback.with_sequence(self.next_callback_sequence + u64::from(index)),
				enqueued_ticks: now_ticks,
			});
		}
		self.record_callback_enqueued(CallbackEventKind::Diagnostic, count);
		self.finish_callback_enqueue(new_depth, next_callback_sequence);
		Ok(count)
	}

	#[cfg(test)]
	fn enqueue_callback_batch(
		&mut self,
		callbacks: &[PendingCallbackEvent],
	) -> Result<u32, StateError> {
		let now_ticks = self.current_ticks();
		self.enqueue_callback_batch_at(callbacks, now_ticks)
	}

	fn enqueue_callback_batch_at(
		&mut self,
		callbacks: &[PendingCallbackEvent],
		now_ticks: u64,
	) -> Result<u32, StateError> {
		let count = u32::try_from(callbacks.len()).map_err(|_| StateError::CallbackBackpressure)?;
		for callback in callbacks {
			callback
				.with_sequence(self.next_callback_sequence)
				.encode()
				.map_err(|error| StateError::State(error.to_string()))?;
		}
		let (new_depth, next_callback_sequence) = match self.prepare_callback_enqueue(count) {
			Ok(prepared) => prepared,
			Err(StateError::CallbackBackpressure) => {
				for callback in callbacks {
					self.record_callback_rejected(callback.kind, 1);
				}
				return Err(StateError::CallbackBackpressure);
			}
			Err(error) => return Err(error),
		};
		for (index, callback) in callbacks.iter().copied().enumerate() {
			self.callback_events.push_back(QueuedCallback {
				event: callback.with_sequence(self.next_callback_sequence + index as u64),
				enqueued_ticks: now_ticks,
			});
			self.record_callback_enqueued(callback.kind, 1);
		}
		self.finish_callback_enqueue(new_depth, next_callback_sequence);
		Ok(count)
	}

	fn prepare_callback_enqueue(&mut self, count: u32) -> Result<(u32, u64), StateError> {
		let depth = u32::try_from(self.callback_events.len())
			.map_err(|_| StateError::CallbackBackpressure)?;
		let Some(new_depth) = depth.checked_add(count) else {
			return Err(StateError::CallbackBackpressure);
		};
		if new_depth > self.max_callback_events {
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

	fn record_callback_enqueued(&mut self, kind: CallbackEventKind, count: u32) {
		self.callback_enqueued = self.callback_enqueued.saturating_add(u64::from(count));
		let counter = &mut self.callback_enqueued_by_kind[callback_kind_index(kind)];
		*counter = counter.saturating_add(u64::from(count));
	}

	fn record_callback_drained(&mut self, kind: CallbackEventKind) {
		self.callback_drained = self.callback_drained.saturating_add(1);
		let counter = &mut self.callback_drained_by_kind[callback_kind_index(kind)];
		*counter = counter.saturating_add(1);
	}

	fn record_callback_rejected(&mut self, kind: CallbackEventKind, count: u32) {
		self.callback_rejected = self.callback_rejected.saturating_add(u64::from(count));
		let counter = &mut self.callback_rejected_by_kind[callback_kind_index(kind)];
		*counter = counter.saturating_add(u64::from(count));
	}

	pub fn drain_callbacks(
		&mut self,
		max_events: u32,
		output: &mut [u8],
	) -> Result<usize, StateError> {
		let now_ticks = self.current_ticks();
		self.drain_callbacks_at(max_events, output, now_ticks)
	}

	fn drain_callbacks_at(
		&mut self,
		max_events: u32,
		output: &mut [u8],
		now_ticks: u64,
	) -> Result<usize, StateError> {
		self.expire_continuations_at(now_ticks)?;
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
			let callback = self
				.callback_events
				.pop_front()
				.expect("returned count was bounded by queue length");
			let start = CALLBACK_BATCH_HEADER_LEN + index * CALLBACK_EVENT_LEN;
			output[start..start + CALLBACK_EVENT_LEN].copy_from_slice(
				&callback
					.event
					.encode()
					.map_err(|error| StateError::State(error.to_string()))?,
			);
			self.record_callback_drained(callback.event.kind);
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

	fn expire_continuations_at(&mut self, now_ticks: u64) -> Result<(), StateError> {
		self.expired_continuation_scratch.clear();
		self.expired_continuation_scratch
			.try_reserve(self.pending_continuations.len())
			.map_err(|_| StateError::AllocationFailed)?;
		for (id, continuation) in &self.pending_continuations {
			if now_ticks >= continuation.deadline_ticks {
				self.expired_continuation_scratch
					.push((*id, continuation.core_token));
			}
		}
		for (id, core_token) in self.expired_continuation_scratch.drain(..) {
			self.world
				.cancel_reaction(core_token)
				.map_err(map_world_error)?;
			self.pending_continuations.remove(&id);
			self.continuation_timeouts = self.continuation_timeouts.saturating_add(1);
		}
		self.callback_events.retain(|callback| {
			callback
				.event
				.continuation
				.is_none_or(|token| self.pending_continuations.contains_key(&token.id))
		});
		Ok(())
	}

	pub fn install_gases(
		&mut self,
		entries: Vec<GasMetadataRegistration>,
	) -> Result<u32, StateError> {
		let gases = entries
			.into_iter()
			.map(|entry| GasMetadata {
				id: GasId(entry.id),
				key: entry.key.into_boxed_str(),
				name: entry.name.into_boxed_str(),
				flags: entry.flags,
				specific_heat: entry.specific_heat.0 as f32,
				fusion_power: entry.fusion_power.0 as f32,
				moles_visible: entry.moles_visible.map(|value| value.0 as f32),
				enthalpy: entry.enthalpy.0 as f32,
				fire_radiation_released: entry.fire_radiation_released.0 as f32,
				fire_role: match entry.fire_role {
					WireGasFireRole::None => GasFireRole::None,
					WireGasFireRole::Oxidizer {
						minimum_temperature,
						power,
					} => GasFireRole::Oxidizer {
						minimum_temperature: minimum_temperature.0 as f32,
						power: power.0 as f32,
					},
					WireGasFireRole::Fuel {
						minimum_temperature,
						burn_rate,
					} => GasFireRole::Fuel {
						minimum_temperature: minimum_temperature.0 as f32,
						burn_rate: burn_rate.0 as f32,
					},
				},
				fire_products: entry.fire_products.map(|products| match products {
					WireFireProducts::Generic(products) => FireProductRule::Generic(
						products
							.into_iter()
							.map(|product| GasProduct {
								gas: GasId(product.gas_id),
								ratio: product.ratio.0 as f32,
							})
							.collect::<Vec<_>>()
							.into_boxed_slice(),
					),
					WireFireProducts::Plasma => FireProductRule::Plasma,
				}),
			})
			.collect();
		self.world.install_gases(gases).map_err(map_world_error)
	}

	pub fn install_reactions(
		&mut self,
		entries: Vec<ReactionMetadataRegistration>,
	) -> Result<u32, StateError> {
		let reactions = entries
			.into_iter()
			.map(|entry| ReactionMetadata {
				id: ReactionId(entry.id),
				key: entry.key.into_boxed_str(),
				priority: entry.priority.0 as f32,
				minimum_temperature: entry.minimum_temperature.map(|value| value.0 as f32),
				maximum_temperature: entry.maximum_temperature.map(|value| value.0 as f32),
				minimum_energy: entry.minimum_energy.map(|value| value.0 as f32),
				minimum_fire_reagents: entry.minimum_fire_reagents.map(|value| value.0 as f32),
				gas_requirements: entry
					.gas_requirements
					.into_iter()
					.map(|requirement| GasRequirement {
						gas: GasId(requirement.gas_id),
						minimum_moles: requirement.minimum_moles.0 as f32,
					})
					.collect::<Vec<_>>()
					.into_boxed_slice(),
				execution: match entry.execution {
					WireReactionExecution::Dm => ReactionExecution::Dm,
					WireReactionExecution::NativePlasma => {
						ReactionExecution::Native(NativeReactionKind::Plasma)
					}
					WireReactionExecution::NativeHydrogen => {
						ReactionExecution::Native(NativeReactionKind::Hydrogen)
					}
					WireReactionExecution::NativeTritium => {
						ReactionExecution::Native(NativeReactionKind::Tritium)
					}
					WireReactionExecution::NativeFreon => {
						ReactionExecution::Native(NativeReactionKind::Freon)
					}
				},
			})
			.collect();
		self.world
			.install_reactions(reactions)
			.map_err(map_world_error)
	}

	pub fn apply_lifecycle(&mut self, mutations: &[LifecycleMutation]) -> Result<u32, StateError> {
		let core_mutations = mutations
			.iter()
			.map(|mutation| CoreLifecycleMutation {
				action: match mutation.action {
					LifecycleAction::Register => CoreLifecycleAction::Register,
					LifecycleAction::Unregister => CoreLifecycleAction::Unregister,
				},
				handle: core_handle(mutation.handle),
			})
			.collect::<Vec<_>>();
		let applied = self
			.world
			.apply_lifecycle(&core_mutations)
			.map_err(map_world_error)?;
		self.pending_continuations.retain(|_, continuation| {
			!mutations.iter().any(|mutation| {
				mutation.action == LifecycleAction::Unregister
					&& mutation.handle.slot == continuation.mixture.slot
			})
		});
		self.remove_orphaned_continuation_callbacks();
		Ok(applied)
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

	pub fn apply_turf_lifecycle(
		&mut self,
		mutations: &[TurfLifecycleMutation],
	) -> Result<u32, StateError> {
		let core_mutations = mutations
			.iter()
			.map(|mutation| match mutation.action {
				LifecycleAction::Register => CoreTurfLifecycleMutation::Register {
					handle: core_turf_handle(mutation.turf),
					mixture: mutation.mixture.map(core_handle),
				},
				LifecycleAction::Unregister => CoreTurfLifecycleMutation::Unregister {
					handle: core_turf_handle(mutation.turf),
				},
			})
			.collect::<Vec<_>>();
		let applied = self
			.world
			.apply_turf_lifecycle(&core_mutations)
			.map_err(map_world_error)?;
		self.pending_continuations.retain(|_, continuation| {
			!mutations.iter().any(|mutation| {
				mutation.action == LifecycleAction::Unregister
					&& mutation.turf.slot == continuation.turf.slot
			})
		});
		self.remove_orphaned_continuation_callbacks();
		Ok(applied)
	}

	pub fn apply_turf_adjacency(
		&mut self,
		mutations: &[TurfAdjacencyMutation],
	) -> Result<u32, StateError> {
		let mut edges = BTreeSet::new();
		for mutation in mutations {
			let edge = (
				mutation.left.slot.min(mutation.right.slot),
				mutation.left.slot.max(mutation.right.slot),
			);
			if !edges.insert(edge) {
				return Err(StateError::DuplicateTurfAdjacency {
					left: edge.0,
					right: edge.1,
				});
			}
		}
		let adjacency = mutations
			.iter()
			.map(|mutation| CoreTurfAdjacencyMutation {
				left: core_turf_handle(mutation.left),
				right: core_turf_handle(mutation.right),
				connected: mutation.connected,
			})
			.collect::<Vec<_>>();
		let firelocks = mutations
			.iter()
			.map(|mutation| CoreTurfFirelockMutation {
				left: core_turf_handle(mutation.left),
				right: core_turf_handle(mutation.right),
				firelock: mutation.firelock,
			})
			.collect::<Vec<_>>();
		self.world
			.apply_turf_adjacency(&adjacency)
			.map_err(map_world_error)?;
		self.world
			.apply_turf_firelocks(&firelocks)
			.map_err(map_world_error)?;
		Ok(mutations.len() as u32)
	}

	pub fn apply_turf_heat(&mut self, mutations: &[TurfHeatMutation]) -> Result<u32, StateError> {
		let mutations = mutations
			.iter()
			.map(|mutation| CoreTurfHeatMutation {
				handle: core_turf_handle(mutation.turf),
				state: mutation.state.map(|state| CoreTurfHeatState {
					temperature: state.temperature.0 as f32,
					thermal_conductivity: state.thermal_conductivity.0 as f32,
					heat_capacity: state.heat_capacity.0 as f32,
					adjacent_to_space: state.adjacent_to_space,
				}),
			})
			.collect::<Vec<_>>();
		self.world
			.apply_turf_heat(&mutations)
			.map_err(map_world_error)
	}

	pub fn apply_turf_heat_adjacency(
		&mut self,
		mutations: &[TurfHeatAdjacencyMutation],
	) -> Result<u32, StateError> {
		let mutations = mutations
			.iter()
			.map(|mutation| CoreTurfHeatAdjacencyMutation {
				left: core_turf_handle(mutation.left),
				right: core_turf_handle(mutation.right),
				connected: mutation.connected,
			})
			.collect::<Vec<_>>();
		self.world
			.apply_turf_heat_adjacency(&mutations)
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
			minimum_heat_capacity: ScalarValue(f64::from(mixture.minimum_heat_capacity)),
			immutable: mixture.immutable,
			gases,
		})
	}

	pub fn apply_adjust_multiple(
		&mut self,
		handle: WireHandle,
		adjustments: &[MixtureAdjustment],
	) -> Result<MixtureCommandResponse, StateError> {
		let adjustments = adjustments
			.iter()
			.map(|adjustment| (GasId(adjustment.gas_id), adjustment.delta.0 as f32))
			.collect::<Vec<_>>()
			.into_boxed_slice();
		match self
			.world
			.apply_command(CoreCommand::AdjustMultiple {
				handle: core_handle(handle),
				adjustments,
			})
			.map_err(map_world_error)?
		{
			CoreCommandResult::Applied { updated } => {
				Ok(MixtureCommandResponse::Applied { updated })
			}
			other => Err(StateError::State(format!(
				"adjust-multiple command returned an unexpected result: {other:?}"
			))),
		}
	}

	pub fn apply_mixture_command(
		&mut self,
		request: MixtureCommandRequest,
	) -> Result<MixtureCommandResponse, StateError> {
		let command = match request {
			MixtureCommandRequest::SetMoles {
				handle,
				gas_id,
				amount,
			} => CoreCommand::SetMoles {
				handle: core_handle(handle),
				gas: GasId(gas_id),
				amount: amount.0 as f32,
			},
			MixtureCommandRequest::AdjustMoles {
				handle,
				gas_id,
				delta,
			} => CoreCommand::AdjustMoles {
				handle: core_handle(handle),
				gas: GasId(gas_id),
				delta: delta.0 as f32,
			},
			MixtureCommandRequest::AdjustMolesTemperature {
				handle,
				gas_id,
				amount,
				temperature,
			} => CoreCommand::AdjustMolesTemperature {
				handle: core_handle(handle),
				gas: GasId(gas_id),
				amount: amount.0 as f32,
				temperature: temperature.0 as f32,
			},
			MixtureCommandRequest::GetMoles { handle, gas_id } => CoreCommand::GetMoles {
				handle: core_handle(handle),
				gas: GasId(gas_id),
			},
			MixtureCommandRequest::Temperature { handle } => CoreCommand::Temperature {
				handle: core_handle(handle),
			},
			MixtureCommandRequest::Volume { handle } => CoreCommand::Volume {
				handle: core_handle(handle),
			},
			MixtureCommandRequest::HeatCapacity { handle } => CoreCommand::HeatCapacity {
				handle: core_handle(handle),
			},
			MixtureCommandRequest::PartialHeatCapacity { handle, gas_id } => {
				CoreCommand::PartialHeatCapacity {
					handle: core_handle(handle),
					gas: GasId(gas_id),
				}
			}
			MixtureCommandRequest::TotalMoles { handle } => CoreCommand::TotalMoles {
				handle: core_handle(handle),
			},
			MixtureCommandRequest::Pressure { handle } => CoreCommand::Pressure {
				handle: core_handle(handle),
			},
			MixtureCommandRequest::ThermalEnergy { handle } => CoreCommand::ThermalEnergy {
				handle: core_handle(handle),
			},
			MixtureCommandRequest::GetMolesByFlags { handle, flags } => {
				CoreCommand::GetMolesByFlags {
					handle: core_handle(handle),
					flags,
				}
			}
			MixtureCommandRequest::Burnability {
				handle,
				temperature,
			} => CoreCommand::Burnability {
				handle: core_handle(handle),
				temperature: temperature.map(|value| value.0 as f32),
			},
			MixtureCommandRequest::SetTemperature {
				handle,
				temperature,
			} => CoreCommand::SetTemperature {
				handle: core_handle(handle),
				temperature: temperature.0 as f32,
			},
			MixtureCommandRequest::SetVolume { handle, volume } => CoreCommand::SetVolume {
				handle: core_handle(handle),
				volume: volume.0 as f32,
			},
			MixtureCommandRequest::SetMinimumHeatCapacity { handle, amount } => {
				CoreCommand::SetMinimumHeatCapacity {
					handle: core_handle(handle),
					amount: amount.0 as f32,
				}
			}
			MixtureCommandRequest::Clear { handle } => CoreCommand::Clear {
				handle: core_handle(handle),
			},
			MixtureCommandRequest::Add { handle, amount } => CoreCommand::Add {
				handle: core_handle(handle),
				amount: amount.0 as f32,
			},
			MixtureCommandRequest::Multiply { handle, factor } => CoreCommand::Multiply {
				handle: core_handle(handle),
				factor: factor.0 as f32,
			},
			MixtureCommandRequest::CopyFrom { receiver, giver } => CoreCommand::CopyFrom {
				receiver: core_handle(receiver),
				giver: core_handle(giver),
			},
			MixtureCommandRequest::AdjustHeat { handle, heat } => CoreCommand::AdjustHeat {
				handle: core_handle(handle),
				heat: heat.0 as f32,
			},
			MixtureCommandRequest::Compare { left, right } => CoreCommand::Compare {
				left: core_handle(left),
				right: core_handle(right),
			},
			MixtureCommandRequest::EqualizeWith { receiver, total } => CoreCommand::EqualizeWith {
				receiver: core_handle(receiver),
				total: core_handle(total),
			},
			MixtureCommandRequest::TemperatureShare {
				first,
				second,
				conduction_coefficient,
			} => CoreCommand::TemperatureShare {
				first: core_handle(first),
				second: core_handle(second),
				conduction_coefficient: conduction_coefficient.0 as f32,
			},
			MixtureCommandRequest::TemperatureShareNonGas {
				handle,
				conduction_coefficient,
				sharer_temperature,
				sharer_heat_capacity,
			} => CoreCommand::TemperatureShareNonGas {
				handle: core_handle(handle),
				conduction_coefficient: conduction_coefficient.0 as f32,
				sharer_temperature: sharer_temperature.0 as f32,
				sharer_heat_capacity: sharer_heat_capacity.0 as f32,
			},
			MixtureCommandRequest::MarkImmutable { handle } => CoreCommand::MarkImmutable {
				handle: core_handle(handle),
			},
			MixtureCommandRequest::IsImmutable { handle } => CoreCommand::IsImmutable {
				handle: core_handle(handle),
			},
			MixtureCommandRequest::Merge { receiver, giver } => CoreCommand::Merge {
				receiver: core_handle(receiver),
				giver: core_handle(giver),
			},
			MixtureCommandRequest::RemoveRatioInto {
				source,
				destination,
				ratio,
			} => CoreCommand::RemoveRatioInto {
				source: core_handle(source),
				destination: core_handle(destination),
				ratio: ratio.0 as f32,
			},
			MixtureCommandRequest::RemoveAmountInto {
				source,
				destination,
				amount,
			} => CoreCommand::RemoveAmountInto {
				source: core_handle(source),
				destination: core_handle(destination),
				amount: amount.0 as f32,
			},
			MixtureCommandRequest::TransferGases {
				source,
				destination,
				ratio,
				gas_mask,
			} => {
				let gases = (0..MAX_GAS_SLOTS)
					.filter(|index| gas_mask & (1_u32 << index) != 0)
					.map(|index| GasId(index as u16))
					.collect::<Vec<_>>()
					.into_boxed_slice();
				CoreCommand::TransferGases {
					source: core_handle(source),
					destination: core_handle(destination),
					ratio: ratio.0 as f32,
					gases,
				}
			}
			MixtureCommandRequest::TransferAmount {
				source,
				destination,
				amount,
			} => CoreCommand::TransferAmount {
				source: core_handle(source),
				destination: core_handle(destination),
				amount: amount.0 as f32,
			},
			MixtureCommandRequest::TransferRatio {
				source,
				destination,
				ratio,
			} => CoreCommand::TransferRatio {
				source: core_handle(source),
				destination: core_handle(destination),
				ratio: ratio.0 as f32,
			},
			MixtureCommandRequest::TransferByFlags {
				source,
				destination,
				flags,
				amount,
			} => CoreCommand::TransferByFlags {
				source: core_handle(source),
				destination: core_handle(destination),
				flags,
				amount: amount.0 as f32,
			},
			MixtureCommandRequest::ShareRatio {
				first,
				second,
				ratio,
				one_way,
			} => CoreCommand::ShareRatio {
				first: core_handle(first),
				second: core_handle(second),
				ratio: ratio.0 as f32,
				one_way,
			},
		};
		match self.world.apply_command(command).map_err(map_world_error)? {
			CoreCommandResult::Applied { updated } => {
				Ok(MixtureCommandResponse::Applied { updated })
			}
			CoreCommandResult::Scalar(value) => Ok(MixtureCommandResponse::Scalar(ScalarValue(
				f64::from(value),
			))),
			CoreCommandResult::Scalars(values) => Ok(MixtureCommandResponse::Scalars(
				values.map(|value| ScalarValue(f64::from(value))),
			)),
			CoreCommandResult::Boolean(value) => Ok(MixtureCommandResponse::Boolean(value)),
			CoreCommandResult::Snapshot(_) => Err(StateError::State(
				"snapshot command must use the snapshot operation".into(),
			)),
		}
	}

	pub fn process_stage_cancellable(
		&mut self,
		stage: SimulationStage,
		seconds_per_tick: f64,
		should_cancel: impl FnMut() -> bool,
	) -> Result<StageResult, StateError> {
		let now_ticks = self.current_ticks();
		self.process_stage_cancellable_at(stage, seconds_per_tick, now_ticks, should_cancel)
	}

	fn process_stage_cancellable_at(
		&mut self,
		stage: SimulationStage,
		seconds_per_tick: f64,
		now_ticks: u64,
		should_cancel: impl FnMut() -> bool,
	) -> Result<StageResult, StateError> {
		let stage = match stage {
			SimulationStage::ProcessTurfs => WorldStage::ProcessTurfs,
			SimulationStage::ProcessTurfEqualize => WorldStage::Equalize,
			SimulationStage::ProcessExcitedGroups => WorldStage::ExcitedGroups,
			SimulationStage::ProcessTurfHeat => WorldStage::TurfHeat,
			SimulationStage::ProcessReactions => WorldStage::React,
		};
		let queue_depth = u32::try_from(self.callback_events.len())
			.map_err(|_| StateError::CallbackBackpressure)?;
		let remaining_capacity = self
			.max_callback_events
			.checked_sub(queue_depth)
			.ok_or(StateError::CallbackBackpressure)?;
		let event_limit = remaining_capacity;
		self.next_callback_sequence
			.checked_add(u64::from(event_limit))
			.ok_or(StateError::CallbackSequenceExhausted)?;
		self.callback_events
			.try_reserve_exact(event_limit as usize)
			.map_err(|_| StateError::AllocationFailed)?;
		self.world_events
			.try_reserve_exact(event_limit as usize)
			.map_err(|_| StateError::AllocationFailed)?;
		self.pending_callback_scratch
			.try_reserve_exact(event_limit as usize)
			.map_err(|_| StateError::AllocationFailed)?;
		self.pending_continuation_scratch
			.try_reserve_exact(event_limit as usize)
			.map_err(|_| StateError::AllocationFailed)?;
		let result = self
			.world
			.process_stage_cancellable_with_event_limit(
				stage,
				seconds_per_tick,
				event_limit,
				should_cancel,
			)
			.map_err(map_world_error)?;
		let event_count = self.enqueue_world_events_at(event_limit, now_ticks)?;
		Ok(StageResult {
			work_items: result.work_items,
			callback_events: event_count,
		})
	}

	pub fn pending_continuation_count(&self) -> u32 {
		self.pending_continuations
			.len()
			.try_into()
			.unwrap_or(u32::MAX)
	}

	pub fn apply_continuation_command_at(
		&mut self,
		token: ContinuationToken,
		command: MixtureCommandRequest,
		now_ticks: u64,
	) -> Result<MixtureCommandResponse, StateError> {
		self.require_continuation_at(token, now_ticks)?;
		self.apply_mixture_command(command)
	}

	pub fn apply_continuation_command(
		&mut self,
		token: ContinuationToken,
		command: MixtureCommandRequest,
	) -> Result<MixtureCommandResponse, StateError> {
		let now_ticks = self.current_ticks();
		self.apply_continuation_command_at(token, command, now_ticks)
	}

	pub fn apply_continuation_adjust_multiple_at(
		&mut self,
		token: ContinuationToken,
		handle: WireHandle,
		adjustments: &[MixtureAdjustment],
		now_ticks: u64,
	) -> Result<MixtureCommandResponse, StateError> {
		self.require_continuation_at(token, now_ticks)?;
		self.apply_adjust_multiple(handle, adjustments)
	}

	pub fn apply_continuation_adjust_multiple(
		&mut self,
		token: ContinuationToken,
		handle: WireHandle,
		adjustments: &[MixtureAdjustment],
	) -> Result<MixtureCommandResponse, StateError> {
		let now_ticks = self.current_ticks();
		self.apply_continuation_adjust_multiple_at(token, handle, adjustments, now_ticks)
	}

	pub fn resume_continuation_at(
		&mut self,
		token: ContinuationToken,
		now_ticks: u64,
	) -> Result<MixtureCommandResponse, StateError> {
		let continuation = self.require_continuation_at(token, now_ticks)?;
		let queue_depth = u32::try_from(self.callback_events.len())
			.map_err(|_| StateError::CallbackBackpressure)?;
		let remaining_callbacks = self
			.max_callback_events
			.checked_sub(queue_depth)
			.ok_or(StateError::CallbackBackpressure)?;
		let event_limit = remaining_callbacks;
		let updated = self
			.world
			.resume_reaction_with_event_limit(continuation.core_token, event_limit)
			.map_err(map_world_error)?;
		self.pending_continuations.remove(&token.id);
		self.remove_queued_continuation(token.id);
		self.enqueue_world_events_at(event_limit, now_ticks)?;
		Ok(MixtureCommandResponse::Applied { updated })
	}

	pub fn resume_continuation(
		&mut self,
		token: ContinuationToken,
	) -> Result<MixtureCommandResponse, StateError> {
		let now_ticks = self.current_ticks();
		self.resume_continuation_at(token, now_ticks)
	}

	pub fn cancel_continuation_at(
		&mut self,
		token: ContinuationToken,
		now_ticks: u64,
	) -> Result<(), StateError> {
		let continuation = self.require_continuation_at(token, now_ticks)?;
		self.world
			.cancel_reaction(continuation.core_token)
			.map_err(map_world_error)?;
		self.pending_continuations.remove(&token.id);
		self.remove_queued_continuation(token.id);
		Ok(())
	}

	pub fn cancel_continuation(&mut self, token: ContinuationToken) -> Result<(), StateError> {
		let now_ticks = self.current_ticks();
		self.cancel_continuation_at(token, now_ticks)
	}

	fn require_continuation_at(
		&mut self,
		token: ContinuationToken,
		now_ticks: u64,
	) -> Result<PendingContinuation, StateError> {
		if token.world_generation != self.world_generation {
			return Err(StateError::ContinuationWorldMismatch {
				expected: self.world_generation,
				actual: token.world_generation,
			});
		}
		let Some(continuation) = self.pending_continuations.get(&token.id).copied() else {
			return Err(StateError::UnknownContinuation(token));
		};
		if continuation.deadline_ticks != token.deadline_ticks {
			return Err(StateError::ContinuationTokenMismatch(token));
		}
		if now_ticks >= continuation.deadline_ticks {
			self.pending_continuations.remove(&token.id);
			self.remove_queued_continuation(token.id);
			self.world
				.cancel_reaction(continuation.core_token)
				.map_err(map_world_error)?;
			self.continuation_timeouts = self.continuation_timeouts.saturating_add(1);
			return Err(StateError::ContinuationExpired(token));
		}
		Ok(continuation)
	}

	fn enqueue_world_events_at(&mut self, maximum: u32, now_ticks: u64) -> Result<u32, StateError> {
		let event_count = self
			.world
			.drain_events_into(maximum, &mut self.world_events);
		let (new_depth, next_callback_sequence) = match self.prepare_callback_enqueue(event_count) {
			Ok(prepared) => prepared,
			Err(StateError::CallbackBackpressure) => {
				for index in 0..self.world_events.len() {
					let kind = world_event_kind(self.world_events[index]);
					self.record_callback_rejected(kind, 1);
				}
				return Err(StateError::CallbackBackpressure);
			}
			Err(error) => return Err(error),
		};
		self.pending_callback_scratch.clear();
		self.pending_continuation_scratch.clear();
		let continuation_count = self
			.world_events
			.iter()
			.filter(|event| matches!(event, WorldEvent::RunDmReaction { .. }))
			.count();
		let continuation_count = u32::try_from(continuation_count)
			.map_err(|_| StateError::ContinuationCapacityExceeded)?;
		let deadline_ticks = if continuation_count == 0 {
			None
		} else {
			Some(
				now_ticks
					.checked_add(DEFAULT_CONTINUATION_TIMEOUT_TICKS)
					.ok_or(StateError::ContinuationDeadlineExhausted)?,
			)
		};
		let new_continuation_count = self
			.pending_continuation_count()
			.checked_add(continuation_count)
			.ok_or(StateError::ContinuationCapacityExceeded)?;
		if new_continuation_count > self.max_pending_continuations {
			return Err(StateError::ContinuationCapacityExceeded);
		}
		self.next_continuation_id
			.checked_add(u64::from(continuation_count))
			.ok_or(StateError::ContinuationIdExhausted)?;
		for (index, event) in self.world_events.iter().copied().enumerate() {
			let continuation = match event {
				WorldEvent::RunDmReaction {
					turf,
					mixture,
					continuation: core_token,
					..
				} => {
					let deadline_ticks =
						deadline_ticks.expect("reaction continuation count requires a deadline");
					let id =
						self.next_continuation_id + self.pending_continuation_scratch.len() as u64;
					let token = ContinuationToken {
						world_generation: self.world_generation,
						id,
						deadline_ticks,
					};
					self.pending_continuation_scratch.push((
						id,
						PendingContinuation {
							core_token,
							deadline_ticks,
							turf: wire_handle_from_turf(turf),
							mixture: wire_handle(mixture),
						},
					));
					Some(token)
				}
				_ => None,
			};
			let callback = pending_callback_from_world_event(event, continuation);
			callback
				.with_sequence(self.next_callback_sequence + index as u64)
				.encode()
				.map_err(|error| StateError::State(error.to_string()))?;
			self.pending_callback_scratch.push(callback);
		}
		self.next_continuation_id += u64::from(continuation_count);
		for (id, continuation) in self.pending_continuation_scratch.drain(..) {
			self.pending_continuations.insert(id, continuation);
		}
		self.continuation_high_water = self.continuation_high_water.max(new_continuation_count);
		for index in 0..self.pending_callback_scratch.len() {
			let callback = self.pending_callback_scratch[index];
			self.callback_events.push_back(QueuedCallback {
				event: callback.with_sequence(self.next_callback_sequence + index as u64),
				enqueued_ticks: now_ticks,
			});
			self.record_callback_enqueued(callback.kind, 1);
		}
		self.pending_callback_scratch.clear();
		self.finish_callback_enqueue(new_depth, next_callback_sequence);
		Ok(event_count)
	}

	fn remove_queued_continuation(&mut self, continuation_id: u64) {
		self.callback_events.retain(|callback| {
			callback
				.event
				.continuation
				.is_none_or(|token| token.id != continuation_id)
		});
	}

	fn remove_orphaned_continuation_callbacks(&mut self) {
		self.callback_events.retain(|callback| {
			callback
				.event
				.continuation
				.is_none_or(|token| self.pending_continuations.contains_key(&token.id))
		});
	}

	fn current_ticks(&self) -> u64 {
		let elapsed_millis = self.session_started_at.elapsed().as_millis();
		let ticks = elapsed_millis / u128::from(CONTINUATION_TICK_MILLIS);
		u64::try_from(ticks).unwrap_or(u64::MAX)
	}

	#[cfg(test)]
	fn edge_count(&self) -> usize {
		self.world.edge_count()
	}

	#[cfg(test)]
	fn turf_edge_count(&self) -> usize {
		self.world.turf_edge_count()
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

fn core_turf_handle(handle: WireHandle) -> TurfHandle {
	TurfHandle {
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

fn wire_handle_from_turf(handle: TurfHandle) -> WireHandle {
	WireHandle {
		slot: handle.slot,
		generation: handle.generation,
	}
}

fn callback_kind_index(kind: CallbackEventKind) -> usize {
	kind as usize - 1
}

fn world_event_kind(event: WorldEvent) -> CallbackEventKind {
	match event {
		WorldEvent::PressureDifference { .. } => CallbackEventKind::PressureDifference,
		WorldEvent::DecompressionFloorRip { .. } => CallbackEventKind::DecompressionFloorRip,
		WorldEvent::FirelockConsideration { .. } => CallbackEventKind::FirelockConsideration,
		WorldEvent::RunDmReaction { .. } => CallbackEventKind::RunDmReaction,
		WorldEvent::ReactionFinished { .. } => CallbackEventKind::ReactionFinished,
		WorldEvent::TurfDestructionRequest { .. } => CallbackEventKind::TurfDestructionRequest,
	}
}

fn pending_callback_from_world_event(
	event: WorldEvent,
	continuation: Option<ContinuationToken>,
) -> PendingCallbackEvent {
	let zero = WireHandle {
		slot: 0,
		generation: 0,
	};
	let mut callback = PendingCallbackEvent {
		kind: CallbackEventKind::Diagnostic,
		flags: 0,
		subject: zero,
		target: zero,
		values: [ScalarValue(0.0); 4],
		aux: 0,
		continuation: None,
	};
	match event {
		WorldEvent::PressureDifference {
			source,
			target,
			moles,
		} => {
			callback.kind = CallbackEventKind::PressureDifference;
			callback.subject = wire_handle_from_turf(source);
			callback.target = wire_handle_from_turf(target);
			callback.values[0] = ScalarValue(f64::from(moles));
		}
		WorldEvent::DecompressionFloorRip { turf, moles_lost } => {
			callback.kind = CallbackEventKind::DecompressionFloorRip;
			callback.subject = wire_handle_from_turf(turf);
			callback.values[0] = ScalarValue(f64::from(moles_lost));
		}
		WorldEvent::FirelockConsideration { source, target } => {
			callback.kind = CallbackEventKind::FirelockConsideration;
			callback.subject = wire_handle_from_turf(source);
			callback.target = wire_handle_from_turf(target);
		}
		WorldEvent::RunDmReaction {
			turf,
			mixture,
			reaction,
			..
		} => {
			callback.kind = CallbackEventKind::RunDmReaction;
			callback.subject = wire_handle_from_turf(turf);
			callback.target = wire_handle(mixture);
			callback.aux = reaction.0;
			callback.continuation = continuation;
		}
		WorldEvent::ReactionFinished {
			turf,
			mixture,
			kind,
			values,
			..
		} => {
			callback.kind = CallbackEventKind::ReactionFinished;
			callback.subject = wire_handle_from_turf(turf);
			callback.target = wire_handle(mixture);
			for (output, value) in callback.values.iter_mut().zip(values) {
				*output = ScalarValue(f64::from(value));
			}
			callback.aux = match kind {
				NativeReactionKind::Plasma => dogmos_protocol::ReactionKind::Plasma as u32,
				NativeReactionKind::Hydrogen => dogmos_protocol::ReactionKind::Hydrogen as u32,
				NativeReactionKind::Tritium => dogmos_protocol::ReactionKind::Tritium as u32,
				NativeReactionKind::Freon => dogmos_protocol::ReactionKind::Freon as u32,
			};
		}
		WorldEvent::TurfDestructionRequest { turf } => {
			callback.kind = CallbackEventKind::TurfDestructionRequest;
			callback.subject = wire_handle_from_turf(turf);
			callback.aux = TurfDestructionReason::SuperconductiveHeat as u32;
		}
	}
	callback
}

fn map_world_error(error: WorldError) -> StateError {
	match error {
		WorldError::GasMetadata(_)
		| WorldError::GasRegistryAlreadyInstalled
		| WorldError::GasRegistryInstallationTooLate
		| WorldError::GasRegistryMissing
		| WorldError::ReactionMetadata(_)
		| WorldError::ReactionRegistryAlreadyInstalled
		| WorldError::ReactionRegistryInstallationTooLate
		| WorldError::ReactionRegistryMissing => StateError::InvalidMetadata,
		WorldError::UnknownHandle(handle) => StateError::UnknownHandle(wire_handle(handle)),
		WorldError::StaleHandle { requested, current } => StateError::StaleHandle {
			requested: wire_handle(requested),
			current,
		},
		WorldError::UnknownTurfHandle(handle) => {
			StateError::State(WorldError::UnknownTurfHandle(handle).to_string())
		}
		WorldError::StaleTurfHandle { requested, current } => {
			StateError::State(WorldError::StaleTurfHandle { requested, current }.to_string())
		}
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
		error @ (WorldError::InvalidGasId(_)
		| WorldError::InvalidMoleAmount
		| WorldError::InvalidMoleDelta
		| WorldError::MoleOverflow(_)
		| WorldError::InvalidTemperature
		| WorldError::InvalidVolume
		| WorldError::InvalidMinimumHeatCapacity
		| WorldError::InvalidAddend
		| WorldError::InvalidMultiplier
		| WorldError::InvalidHeat
		| WorldError::InvalidHeatCapacity
		| WorldError::InvalidRatio
		| WorldError::SameMixtureHandles(_)) => StateError::State(error.to_string()),
		WorldError::SelfAdjacency(slot) => StateError::SelfAdjacency(slot),
		error @ (WorldError::SelfTurfAdjacency(_)
		| WorldError::TurfMissingMixture(_)
		| WorldError::DuplicateMutableTurfMixture(_)
		| WorldError::InvalidTurfHeatState(_)
		| WorldError::TurfHeatMissing(_)
		| WorldError::SelfTurfHeatAdjacency(_)
		| WorldError::ImmutableEqualizationBoundary(_)
		| WorldError::UnknownReactionContinuation(_)
		| WorldError::StaleReactionContinuation { .. }) => StateError::State(error.to_string()),
		WorldError::ReactionContinuationCapacityExceeded => {
			StateError::ContinuationCapacityExceeded
		}
		WorldError::EventCapacityExceeded { .. } => StateError::CallbackBackpressure,
		WorldError::InvalidConductivity => StateError::InvalidConductivity,
		WorldError::InvalidEqualizeHardTurfLimit => {
			StateError::State(WorldError::InvalidEqualizeHardTurfLimit.to_string())
		}
		WorldError::InvalidSecondsPerTick => StateError::InvalidSecondsPerTick,
		WorldError::StageNotImplemented(stage) => StateError::StageNotImplemented(match stage {
			WorldStage::ProcessTurfs => SimulationStage::ProcessTurfs,
			WorldStage::Equalize => SimulationStage::ProcessTurfEqualize,
			WorldStage::ExcitedGroups => SimulationStage::ProcessExcitedGroups,
			WorldStage::React => SimulationStage::ProcessReactions,
			WorldStage::TurfHeat => SimulationStage::ProcessTurfHeat,
		}),
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
	use dogmos_core::world::{Command, TurfAdjacencyMutation, TurfLifecycleMutation, WorldEvent};
	use dogmos_protocol::{
		AdjacencyMutation, CallbackBatchHeader, CallbackEvent, LifecycleAction, LifecycleMutation,
		MixtureStateMutation, ScalarValue, SimulationStage,
		TurfAdjacencyMutation as WireTurfAdjacencyMutation,
		TurfHeatAdjacencyMutation as WireTurfHeatAdjacencyMutation,
		TurfHeatMutation as WireTurfHeatMutation, TurfHeatState as WireTurfHeatState,
		TurfLifecycleMutation as WireTurfLifecycleMutation, WireGasRequirement, WireHandle,
		CALLBACK_BATCH_HEADER_LEN, CALLBACK_EVENT_LEN, MAX_GAS_SLOTS,
	};

	fn handle(slot: u32, generation: u32) -> WireHandle {
		WireHandle { slot, generation }
	}

	#[test]
	fn metadata_errors_map_to_stable_invalid_metadata() {
		assert_eq!(
			map_world_error(WorldError::GasMetadata(
				GasMetadataError::InvalidSpecificHeat(GasId(0))
			)),
			StateError::InvalidMetadata
		);
		assert_eq!(
			map_world_error(WorldError::GasRegistryAlreadyInstalled),
			StateError::InvalidMetadata
		);
		assert_eq!(
			map_world_error(WorldError::GasRegistryInstallationTooLate),
			StateError::InvalidMetadata
		);
		assert_eq!(
			map_world_error(WorldError::GasRegistryMissing),
			StateError::InvalidMetadata
		);
		assert_eq!(
			map_world_error(WorldError::ReactionMetadata(
				ReactionMetadataError::InvalidPriority(ReactionId(0))
			)),
			StateError::InvalidMetadata
		);
		assert_eq!(
			map_world_error(WorldError::ReactionRegistryAlreadyInstalled),
			StateError::InvalidMetadata
		);
		assert_eq!(
			map_world_error(WorldError::ReactionRegistryInstallationTooLate),
			StateError::InvalidMetadata
		);
		assert_eq!(
			map_world_error(WorldError::ReactionRegistryMissing),
			StateError::InvalidMetadata
		);
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
				continuation: None,
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
				continuation: None,
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

	#[test]
	fn callback_telemetry_is_fixed_cost_and_tracks_age_and_kind() {
		let mut state = ServiceState::new_for_world(1024 * 1024, 2, 1, 1);
		state.enqueue_diagnostic_callbacks_at(2, 10).unwrap();

		let telemetry = state.telemetry_at(15);
		assert_eq!(telemetry.callback_depth, 2);
		assert_eq!(telemetry.callback_capacity, 2);
		assert_eq!(telemetry.callback_high_water, 2);
		assert_eq!(telemetry.oldest_callback_age_ticks, 5);
		assert_eq!(telemetry.callback_enqueued, 2);
		assert_eq!(telemetry.callback_enqueued_by_kind[0], 2);
		assert_eq!(telemetry.continuation_capacity, 1);

		assert_eq!(
			state.enqueue_diagnostic_callbacks_at(1, 16),
			Err(StateError::CallbackBackpressure)
		);
		state.record_protocol_error();
		state.record_protocol_error();
		state.record_request_timeout();
		let telemetry = state.telemetry_at(16);
		assert_eq!(telemetry.callback_rejected, 1);
		assert_eq!(telemetry.callback_rejected_by_kind[0], 1);
		assert_eq!(telemetry.protocol_errors, 2);
		assert_eq!(telemetry.request_timeouts, 1);

		let mut output = [0_u8; CALLBACK_BATCH_HEADER_LEN + CALLBACK_EVENT_LEN];
		state.drain_callbacks_at(1, &mut output, 18).unwrap();
		let telemetry = state.telemetry_at(18);
		assert_eq!(telemetry.callback_depth, 1);
		assert_eq!(telemetry.callback_drained, 1);
		assert_eq!(telemetry.callback_drained_by_kind[0], 1);
		assert_eq!(telemetry.oldest_callback_age_ticks, 8);
	}

	#[test]
	fn simulation_stage_enqueues_real_world_events_in_order() {
		let room_mixture = MixtureHandle {
			slot: 0,
			generation: 1,
		};
		let space_mixture = MixtureHandle {
			slot: 1,
			generation: 1,
		};
		let room_turf = dogmos_core::metadata::TurfHandle {
			slot: 0,
			generation: 1,
		};
		let space_turf = dogmos_core::metadata::TurfHandle {
			slot: 1,
			generation: 1,
		};
		let mut state = ServiceState::new(1024 * 1024, 8);
		state
			.world
			.apply_lifecycle(&[
				CoreLifecycleMutation {
					action: CoreLifecycleAction::Register,
					handle: room_mixture,
				},
				CoreLifecycleMutation {
					action: CoreLifecycleAction::Register,
					handle: space_mixture,
				},
			])
			.unwrap();
		let mut room_gases = [0.0; MAX_GAS_SLOTS];
		room_gases[0] = 100.0;
		state
			.world
			.apply_mixture_state(&[
				CoreMixtureStateMutation {
					handle: room_mixture,
					expected_revision: 0,
					temperature: 293.15,
					volume: 2500.0,
					gases: room_gases,
				},
				CoreMixtureStateMutation {
					handle: space_mixture,
					expected_revision: 0,
					temperature: 293.15,
					volume: 2500.0,
					gases: [0.0; MAX_GAS_SLOTS],
				},
			])
			.unwrap();
		state
			.world
			.apply_command(Command::MarkImmutable {
				handle: space_mixture,
			})
			.unwrap();
		state
			.world
			.apply_turf_lifecycle(&[
				TurfLifecycleMutation::Register {
					handle: room_turf,
					mixture: Some(room_mixture),
				},
				TurfLifecycleMutation::Register {
					handle: space_turf,
					mixture: Some(space_mixture),
				},
			])
			.unwrap();
		state
			.world
			.apply_turf_adjacency(&[TurfAdjacencyMutation {
				left: room_turf,
				right: space_turf,
				connected: true,
			}])
			.unwrap();

		let result = state
			.process_stage_cancellable_at(
				SimulationStage::ProcessTurfEqualize,
				0.5,
				u64::MAX,
				|| false,
			)
			.unwrap();
		assert_eq!(result.callback_events, 2);
		let mut output = [0_u8; CALLBACK_BATCH_HEADER_LEN + 2 * CALLBACK_EVENT_LEN];
		state.drain_callbacks(2, &mut output).unwrap();
		let first = CallbackEvent::decode(
			&output[CALLBACK_BATCH_HEADER_LEN..CALLBACK_BATCH_HEADER_LEN + CALLBACK_EVENT_LEN],
		)
		.unwrap();
		let second =
			CallbackEvent::decode(&output[CALLBACK_BATCH_HEADER_LEN + CALLBACK_EVENT_LEN..])
				.unwrap();
		assert_eq!(first.kind, CallbackEventKind::PressureDifference);
		assert_eq!(first.subject, wire_handle_from_turf(room_turf));
		assert_eq!(first.target, wire_handle_from_turf(space_turf));
		assert_eq!(first.values[0], ScalarValue(25.0));
		assert_eq!(second.kind, CallbackEventKind::DecompressionFloorRip);
		assert_eq!(second.subject, wire_handle_from_turf(room_turf));
		assert_eq!(second.values[0], ScalarValue(25.0));
		assert!(matches!(
			state.world.snapshot(room_mixture),
			Ok(snapshot) if snapshot.gases[0] == 75.0
		));
		assert!(
			state
				.world
				.drain_events_into(8, &mut Vec::<WorldEvent>::new())
				== 0
		);

		state
			.world
			.apply_mixture_state(&[CoreMixtureStateMutation {
				handle: room_mixture,
				expected_revision: 2,
				temperature: 293.15,
				volume: 2500.0,
				gases: room_gases,
			}])
			.unwrap();
		let before_backpressure = state.world.snapshot(room_mixture).unwrap();
		state.enqueue_diagnostic_callbacks(8).unwrap();
		assert_eq!(
			state.process_stage_cancellable(SimulationStage::ProcessTurfEqualize, 0.5, || false,),
			Err(StateError::CallbackBackpressure)
		);
		assert_eq!(
			state.world.snapshot(room_mixture).unwrap(),
			before_backpressure
		);
		assert!(
			state
				.world
				.drain_events_into(8, &mut Vec::<WorldEvent>::new())
				== 0
		);
	}

	#[test]
	fn wire_turf_batches_construct_authoritative_core_topology() {
		let mixture = handle(0, 1);
		let other_mixture = handle(1, 1);
		let first = handle(10, 2);
		let second = handle(11, 3);
		let mut state = ServiceState::new(1024 * 1024, 8);
		state
			.apply_lifecycle(&[
				LifecycleMutation {
					action: LifecycleAction::Register,
					handle: mixture,
				},
				LifecycleMutation {
					action: LifecycleAction::Register,
					handle: other_mixture,
				},
			])
			.unwrap();
		assert_eq!(
			state
				.apply_turf_lifecycle(&[
					WireTurfLifecycleMutation {
						action: LifecycleAction::Register,
						turf: first,
						mixture: Some(mixture),
					},
					WireTurfLifecycleMutation {
						action: LifecycleAction::Register,
						turf: second,
						mixture: None,
					},
				])
				.unwrap(),
			2
		);
		state
			.apply_turf_adjacency(&[WireTurfAdjacencyMutation {
				left: first,
				right: second,
				connected: true,
				firelock: true,
			}])
			.unwrap_err();
		state
			.apply_turf_lifecycle(&[WireTurfLifecycleMutation {
				action: LifecycleAction::Register,
				turf: second,
				mixture: Some(other_mixture),
			}])
			.unwrap();
		assert_eq!(
			state.apply_turf_adjacency(&[
				WireTurfAdjacencyMutation {
					left: first,
					right: second,
					connected: true,
					firelock: true,
				},
				WireTurfAdjacencyMutation {
					left: second,
					right: first,
					connected: false,
					firelock: false,
				},
			]),
			Err(StateError::DuplicateTurfAdjacency {
				left: 10,
				right: 11
			})
		);
		assert_eq!(state.turf_edge_count(), 0);
		assert_eq!(
			state
				.apply_turf_adjacency(&[WireTurfAdjacencyMutation {
					left: first,
					right: second,
					connected: true,
					firelock: true,
				}])
				.unwrap(),
			1
		);
		state
			.apply_turf_heat(&[
				WireTurfHeatMutation {
					turf: first,
					state: Some(WireTurfHeatState {
						temperature: ScalarValue(700.0),
						thermal_conductivity: ScalarValue(0.4),
						heat_capacity: ScalarValue(100.0),
						adjacent_to_space: false,
					}),
				},
				WireTurfHeatMutation {
					turf: second,
					state: Some(WireTurfHeatState {
						temperature: ScalarValue(300.0),
						thermal_conductivity: ScalarValue(0.4),
						heat_capacity: ScalarValue(100.0),
						adjacent_to_space: false,
					}),
				},
			])
			.unwrap();
		state
			.apply_turf_heat_adjacency(&[WireTurfHeatAdjacencyMutation {
				left: first,
				right: second,
				connected: true,
			}])
			.unwrap();
		assert_eq!(
			state
				.world
				.turf_heat(core_turf_handle(first))
				.unwrap()
				.unwrap()
				.temperature,
			700.0
		);
	}

	#[test]
	fn fixed_mixture_commands_route_through_authoritative_world_state() {
		let mixture = handle(0, 1);
		let mut state = ServiceState::new(1024 * 1024, 8);
		state
			.apply_lifecycle(&[LifecycleMutation {
				action: LifecycleAction::Register,
				handle: mixture,
			}])
			.unwrap();
		assert_eq!(
			state
				.apply_mixture_command(MixtureCommandRequest::SetTemperature {
					handle: mixture,
					temperature: ScalarValue(400.0),
				})
				.unwrap(),
			MixtureCommandResponse::Applied { updated: 1 }
		);
		assert_eq!(
			state
				.apply_mixture_command(MixtureCommandRequest::Temperature { handle: mixture })
				.unwrap(),
			MixtureCommandResponse::Scalar(ScalarValue(400.0))
		);
		state
			.apply_mixture_command(MixtureCommandRequest::MarkImmutable { handle: mixture })
			.unwrap();
		assert_eq!(
			state
				.apply_mixture_command(MixtureCommandRequest::IsImmutable { handle: mixture })
				.unwrap(),
			MixtureCommandResponse::Boolean(true)
		);
	}

	#[test]
	fn wire_metadata_installs_before_world_state_and_enables_gas_commands() {
		let mut state = ServiceState::new(1024 * 1024, 8);
		assert_eq!(
			state
				.install_gases(vec![GasMetadataRegistration {
					id: 0,
					key: "o2".into(),
					name: "Oxygen".into(),
					flags: 1,
					specific_heat: ScalarValue(20.0),
					fusion_power: ScalarValue(0.0),
					moles_visible: None,
					enthalpy: ScalarValue(0.0),
					fire_radiation_released: ScalarValue(0.0),
					fire_role: WireGasFireRole::None,
					fire_products: None,
				}])
				.unwrap(),
			1
		);
		assert_eq!(
			state
				.install_reactions(vec![ReactionMetadataRegistration {
					id: 0,
					key: "dm_reaction".into(),
					priority: ScalarValue(1.0),
					minimum_temperature: None,
					maximum_temperature: None,
					minimum_energy: None,
					minimum_fire_reagents: None,
					gas_requirements: vec![WireGasRequirement {
						gas_id: 0,
						minimum_moles: ScalarValue(0.1),
					}],
					execution: WireReactionExecution::Dm,
				}])
				.unwrap(),
			1
		);
		let mixture = handle(0, 1);
		state
			.apply_lifecycle(&[LifecycleMutation {
				action: LifecycleAction::Register,
				handle: mixture,
			}])
			.unwrap();
		assert_eq!(
			state
				.apply_mixture_command(MixtureCommandRequest::SetMoles {
					handle: mixture,
					gas_id: 0,
					amount: ScalarValue(5.0),
				})
				.unwrap(),
			MixtureCommandResponse::Applied { updated: 1 }
		);
		assert_eq!(
			state
				.apply_adjust_multiple(
					mixture,
					&[
						MixtureAdjustment {
							gas_id: 0,
							delta: ScalarValue(-1.5),
						},
						MixtureAdjustment {
							gas_id: 0,
							delta: ScalarValue(0.5),
						},
					],
				)
				.unwrap(),
			MixtureCommandResponse::Applied { updated: 1 }
		);
		assert_eq!(
			state
				.apply_mixture_command(MixtureCommandRequest::GetMoles {
					handle: mixture,
					gas_id: 0,
				})
				.unwrap(),
			MixtureCommandResponse::Scalar(ScalarValue(4.0))
		);
		assert_eq!(
			state.install_gases(Vec::new()),
			Err(StateError::InvalidMetadata)
		);
	}

	#[test]
	fn continuation_tokens_are_deadline_bound_and_single_use() {
		let mut state = ServiceState::new_for_world(1024 * 1024, 8, 1, 7);
		state
			.install_gases(vec![GasMetadataRegistration {
				id: 0,
				key: "o2".into(),
				name: "Oxygen".into(),
				flags: 0,
				specific_heat: ScalarValue(20.0),
				fusion_power: ScalarValue(0.0),
				moles_visible: None,
				enthalpy: ScalarValue(0.0),
				fire_radiation_released: ScalarValue(0.0),
				fire_role: WireGasFireRole::None,
				fire_products: None,
			}])
			.unwrap();
		state
			.install_reactions(vec![ReactionMetadataRegistration {
				id: 0,
				key: "dm".into(),
				priority: ScalarValue(1.0),
				minimum_temperature: None,
				maximum_temperature: None,
				minimum_energy: None,
				minimum_fire_reagents: None,
				gas_requirements: vec![WireGasRequirement {
					gas_id: 0,
					minimum_moles: ScalarValue(1.0),
				}],
				execution: WireReactionExecution::Dm,
			}])
			.unwrap();
		let mixture = handle(0, 1);
		let second_mixture = handle(1, 1);
		let turf = handle(0, 1);
		let second_turf = handle(1, 1);
		state
			.apply_lifecycle(&[
				LifecycleMutation {
					action: LifecycleAction::Register,
					handle: mixture,
				},
				LifecycleMutation {
					action: LifecycleAction::Register,
					handle: second_mixture,
				},
			])
			.unwrap();
		state
			.apply_turf_lifecycle(&[
				WireTurfLifecycleMutation {
					action: LifecycleAction::Register,
					turf,
					mixture: Some(mixture),
				},
				WireTurfLifecycleMutation {
					action: LifecycleAction::Register,
					turf: second_turf,
					mixture: Some(second_mixture),
				},
			])
			.unwrap();
		state
			.apply_mixture_command(MixtureCommandRequest::SetMoles {
				handle: mixture,
				gas_id: 0,
				amount: ScalarValue(2.0),
			})
			.unwrap();

		state
			.process_stage_cancellable_at(SimulationStage::ProcessReactions, 0.5, 10, || false)
			.unwrap();
		assert_eq!(state.pending_continuation_count(), 1);
		assert_eq!(state.telemetry_at(10).continuation_high_water, 1);
		state
			.apply_mixture_command(MixtureCommandRequest::SetMoles {
				handle: second_mixture,
				gas_id: 0,
				amount: ScalarValue(2.0),
			})
			.unwrap();
		assert_eq!(
			state
				.process_stage_cancellable_at(SimulationStage::ProcessReactions, 0.5, 11, || false,),
			Err(StateError::ContinuationCapacityExceeded)
		);
		assert_eq!(state.pending_continuation_count(), 1);
		assert_eq!(state.world.pending_reaction_continuations(), 1);
		let mut output = vec![0_u8; CALLBACK_BATCH_HEADER_LEN + CALLBACK_EVENT_LEN];
		state.drain_callbacks(1, &mut output).unwrap();
		let event = CallbackEvent::decode(&output[CALLBACK_BATCH_HEADER_LEN..]).unwrap();
		let token = event.continuation.unwrap();
		assert_eq!(token.world_generation, 7);
		assert_eq!(token.deadline_ticks, 60);
		assert_eq!(
			state
				.apply_continuation_command_at(
					token,
					MixtureCommandRequest::SetMoles {
						handle: mixture,
						gas_id: 0,
						amount: ScalarValue(4.0),
					},
					59,
				)
				.unwrap(),
			MixtureCommandResponse::Applied { updated: 1 }
		);
		assert_eq!(
			state.resume_continuation_at(token, 60),
			Err(StateError::ContinuationExpired(token))
		);
		assert_eq!(state.pending_continuation_count(), 0);
		assert_eq!(state.telemetry_at(60).continuation_timeouts, 1);
		assert_eq!(
			state.resume_continuation_at(token, 60),
			Err(StateError::UnknownContinuation(token))
		);

		state
			.process_stage_cancellable_at(SimulationStage::ProcessReactions, 0.5, 61, || false)
			.unwrap();
		state.drain_callbacks(1, &mut output).unwrap();
		let cancel_token = CallbackEvent::decode(&output[CALLBACK_BATCH_HEADER_LEN..])
			.unwrap()
			.continuation
			.unwrap();
		assert_eq!(state.cancel_continuation_at(cancel_token, 62), Ok(()));
		assert_eq!(
			state.cancel_continuation_at(cancel_token, 62),
			Err(StateError::UnknownContinuation(cancel_token))
		);

		state
			.process_stage_cancellable_at(SimulationStage::ProcessReactions, 0.5, 63, || false)
			.unwrap();
		state.drain_callbacks(1, &mut output).unwrap();
		let resume_token = CallbackEvent::decode(&output[CALLBACK_BATCH_HEADER_LEN..])
			.unwrap()
			.continuation
			.unwrap();
		assert_eq!(
			state.resume_continuation_at(resume_token, 64),
			Ok(MixtureCommandResponse::Applied { updated: 0 })
		);
		assert_eq!(
			state.resume_continuation_at(resume_token, 64),
			Err(StateError::UnknownContinuation(resume_token))
		);

		state
			.process_stage_cancellable_at(SimulationStage::ProcessReactions, 0.5, 65, || false)
			.unwrap();
		assert_eq!(state.pending_continuation_count(), 1);
		let output_len = state.drain_callbacks_at(1, &mut output, 115).unwrap();
		assert_eq!(output_len, CALLBACK_BATCH_HEADER_LEN);
		assert_eq!(
			CallbackBatchHeader::decode(&output[..CALLBACK_BATCH_HEADER_LEN])
				.unwrap()
				.returned,
			0
		);
		assert_eq!(state.pending_continuation_count(), 0);
		assert_eq!(state.telemetry_at(115).continuation_timeouts, 2);

		state
			.process_stage_cancellable_at(SimulationStage::ProcessReactions, 0.5, 116, || false)
			.unwrap();
		let lifecycle_event = state.callback_events.front().unwrap().event;
		let lifecycle_token = lifecycle_event.continuation.unwrap();
		state
			.apply_turf_lifecycle(&[WireTurfLifecycleMutation {
				action: LifecycleAction::Unregister,
				turf: lifecycle_event.subject,
				mixture: None,
			}])
			.unwrap();
		assert_eq!(state.pending_continuation_count(), 0);
		assert!(state.callback_events.is_empty());
		assert_eq!(
			state.resume_continuation_at(lifecycle_token, 117),
			Err(StateError::UnknownContinuation(lifecycle_token))
		);
	}
}
