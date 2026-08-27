use std::{
	array,
	sync::{
		atomic::{AtomicBool, AtomicU16, AtomicU64, AtomicU8, Ordering},
		OnceLock,
	},
	time::{Duration, Instant},
};

pub const MAX_OPERATION_SLOTS: usize = 256;
pub const LATENCY_BUCKET_COUNT: usize = 16;
pub const TRANSCRIPT_CAPACITY: usize = 4096;
pub const OPERATION_CLASS_COUNT: usize = 7;
pub const BYOND_VALUE_BYTES_I686: u64 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum OperationClass {
	ScalarRead = 0,
	ScalarWrite = 1,
	MixtureTransaction = 2,
	GraphUpdate = 3,
	SimulationStage = 4,
	Callback = 5,
	Other = 6,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum RuntimeMetric {
	FdmNodesScanned,
	FdmNodesChanged,
	HeatNodesScanned,
	HeatNodesChanged,
	HeatEdgesAttempted,
	HeatEdgesApplied,
	CallbackItemsEnqueued,
	CallbackOwnedBytes,
	CallbackQueueDepth,
	CallbackQueueDepthHighWater,
	CallbackOldestAgeNanoseconds,
	CallbackEnqueueFailures,
	GasGraphNodes,
	GasGraphEdges,
	GasGraphNodeCapacity,
	GasGraphEdgeCapacity,
	GasGraphMapCapacity,
	HeatGraphNodes,
	HeatGraphEdges,
	HeatGraphNodeCapacity,
	HeatGraphEdgeCapacity,
	HeatGraphMapCapacity,
	MixtureSlots,
	MixtureSlotHighWater,
	MixtureMoleLengthZero,
	MixtureMoleLengthOneToFour,
	MixtureMoleLengthFiveToEight,
	MixtureMoleLengthNine,
	MixtureMoleSpills,
}

impl RuntimeMetric {
	pub const COUNT: usize = 29;

	#[must_use]
	pub const fn name(self) -> &'static str {
		match self {
			Self::FdmNodesScanned => "fdm_nodes_scanned",
			Self::FdmNodesChanged => "fdm_nodes_changed",
			Self::HeatNodesScanned => "heat_nodes_scanned",
			Self::HeatNodesChanged => "heat_nodes_changed",
			Self::HeatEdgesAttempted => "heat_edges_attempted",
			Self::HeatEdgesApplied => "heat_edges_applied",
			Self::CallbackItemsEnqueued => "callback_items_enqueued",
			Self::CallbackOwnedBytes => "callback_owned_bytes",
			Self::CallbackQueueDepth => "callback_queue_depth",
			Self::CallbackQueueDepthHighWater => "callback_queue_depth_high_water",
			Self::CallbackOldestAgeNanoseconds => "callback_oldest_age_nanoseconds",
			Self::CallbackEnqueueFailures => "callback_enqueue_failures",
			Self::GasGraphNodes => "gas_graph_nodes",
			Self::GasGraphEdges => "gas_graph_edges",
			Self::GasGraphNodeCapacity => "gas_graph_node_capacity",
			Self::GasGraphEdgeCapacity => "gas_graph_edge_capacity",
			Self::GasGraphMapCapacity => "gas_graph_map_capacity",
			Self::HeatGraphNodes => "heat_graph_nodes",
			Self::HeatGraphEdges => "heat_graph_edges",
			Self::HeatGraphNodeCapacity => "heat_graph_node_capacity",
			Self::HeatGraphEdgeCapacity => "heat_graph_edge_capacity",
			Self::HeatGraphMapCapacity => "heat_graph_map_capacity",
			Self::MixtureSlots => "mixture_slots",
			Self::MixtureSlotHighWater => "mixture_slot_high_water",
			Self::MixtureMoleLengthZero => "mixture_mole_length_zero",
			Self::MixtureMoleLengthOneToFour => "mixture_mole_length_one_to_four",
			Self::MixtureMoleLengthFiveToEight => "mixture_mole_length_five_to_eight",
			Self::MixtureMoleLengthNine => "mixture_mole_length_nine",
			Self::MixtureMoleSpills => "mixture_mole_spills",
		}
	}

	const fn all() -> [Self; Self::COUNT] {
		[
			Self::FdmNodesScanned,
			Self::FdmNodesChanged,
			Self::HeatNodesScanned,
			Self::HeatNodesChanged,
			Self::HeatEdgesAttempted,
			Self::HeatEdgesApplied,
			Self::CallbackItemsEnqueued,
			Self::CallbackOwnedBytes,
			Self::CallbackQueueDepth,
			Self::CallbackQueueDepthHighWater,
			Self::CallbackOldestAgeNanoseconds,
			Self::CallbackEnqueueFailures,
			Self::GasGraphNodes,
			Self::GasGraphEdges,
			Self::GasGraphNodeCapacity,
			Self::GasGraphEdgeCapacity,
			Self::GasGraphMapCapacity,
			Self::HeatGraphNodes,
			Self::HeatGraphEdges,
			Self::HeatGraphNodeCapacity,
			Self::HeatGraphEdgeCapacity,
			Self::HeatGraphMapCapacity,
			Self::MixtureSlots,
			Self::MixtureSlotHighWater,
			Self::MixtureMoleLengthZero,
			Self::MixtureMoleLengthOneToFour,
			Self::MixtureMoleLengthFiveToEight,
			Self::MixtureMoleLengthNine,
			Self::MixtureMoleSpills,
		]
	}
}

struct OperationSlot {
	hash: AtomicU64,
	binding: OnceLock<&'static str>,
	class: AtomicU8,
	calls: AtomicU64,
	request_values: AtomicU64,
	request_bytes: AtomicU64,
	response_values: AtomicU64,
	response_bytes: AtomicU64,
	errors: AtomicU64,
	latency_buckets: [AtomicU64; LATENCY_BUCKET_COUNT],
}

impl OperationSlot {
	const fn new() -> Self {
		Self {
			hash: AtomicU64::new(0),
			binding: OnceLock::new(),
			class: AtomicU8::new(OperationClass::Other as u8),
			calls: AtomicU64::new(0),
			request_values: AtomicU64::new(0),
			request_bytes: AtomicU64::new(0),
			response_values: AtomicU64::new(0),
			response_bytes: AtomicU64::new(0),
			errors: AtomicU64::new(0),
			latency_buckets: [const { AtomicU64::new(0) }; LATENCY_BUCKET_COUNT],
		}
	}
}

pub struct Telemetry {
	detailed: AtomicBool,
	slots: [OperationSlot; MAX_OPERATION_SLOTS],
	metrics: [AtomicU64; RuntimeMetric::COUNT],
	sequence_total: AtomicU64,
	sequence: [AtomicU16; TRANSCRIPT_CAPACITY],
	last_class: AtomicU8,
	class_transitions: [[AtomicU64; OPERATION_CLASS_COUNT]; OPERATION_CLASS_COUNT],
}

impl Default for Telemetry {
	fn default() -> Self {
		Self::new()
	}
}

impl Telemetry {
	#[must_use]
	pub const fn new() -> Self {
		Self {
			detailed: AtomicBool::new(false),
			slots: [const { OperationSlot::new() }; MAX_OPERATION_SLOTS],
			metrics: [const { AtomicU64::new(0) }; RuntimeMetric::COUNT],
			sequence_total: AtomicU64::new(0),
			sequence: [const { AtomicU16::new(0) }; TRANSCRIPT_CAPACITY],
			last_class: AtomicU8::new(0),
			class_transitions: [const { [const { AtomicU64::new(0) }; OPERATION_CLASS_COUNT] };
				OPERATION_CLASS_COUNT],
		}
	}

	pub fn set_detailed(&self, enabled: bool) {
		self.detailed.store(enabled, Ordering::Release);
		self.last_class.store(0, Ordering::Release);
	}

	#[must_use]
	pub fn begin(
		&self,
		binding: &'static str,
		request_values: u64,
		class: OperationClass,
	) -> CallToken<'_> {
		self.begin_sized(binding, request_values, BYOND_VALUE_BYTES_I686, class)
	}

	#[must_use]
	pub fn begin_sized(
		&self,
		binding: &'static str,
		request_values: u64,
		value_bytes: u64,
		class: OperationClass,
	) -> CallToken<'_> {
		let slot_index = self.operation_slot(binding, class);
		let slot = &self.slots[slot_index];
		slot.calls.fetch_add(1, Ordering::Relaxed);
		slot.request_values
			.fetch_add(request_values, Ordering::Relaxed);
		slot.request_bytes.fetch_add(
			request_values.saturating_mul(value_bytes),
			Ordering::Relaxed,
		);
		let detailed = self.detailed.load(Ordering::Acquire);
		if detailed {
			let sequence_index = self.sequence_total.fetch_add(1, Ordering::Relaxed);
			self.sequence[sequence_index as usize % TRANSCRIPT_CAPACITY]
				.store(slot_index as u16 + 1, Ordering::Release);
			let current_class = class as u8 + 1;
			let previous_class = self.last_class.swap(current_class, Ordering::AcqRel);
			if previous_class != 0 {
				self.class_transitions[previous_class as usize - 1][class as usize]
					.fetch_add(1, Ordering::Relaxed);
			}
		}
		CallToken {
			telemetry: self,
			slot_index,
			value_bytes,
			started: detailed.then(Instant::now),
		}
	}

	pub fn increment_metric(&self, metric: RuntimeMetric, amount: u64) {
		self.metrics[metric as usize].fetch_add(amount, Ordering::Relaxed);
	}

	pub fn set_metric(&self, metric: RuntimeMetric, value: u64) {
		self.metrics[metric as usize].store(value, Ordering::Relaxed);
	}

	pub fn update_high_water(&self, metric: RuntimeMetric, value: u64) {
		self.metrics[metric as usize].fetch_max(value, Ordering::Relaxed);
	}

	#[must_use]
	pub fn snapshot(&self, sequence_limit: usize) -> TelemetrySnapshot {
		let mut operations = self
			.slots
			.iter()
			.enumerate()
			.filter_map(|(slot_index, slot)| {
				let binding = slot.binding.get()?;
				Some(OperationSnapshot {
					slot: slot_index as u16,
					binding: (*binding).to_owned(),
					class: operation_class(slot.class.load(Ordering::Relaxed)),
					calls: slot.calls.load(Ordering::Relaxed),
					request_values: slot.request_values.load(Ordering::Relaxed),
					request_bytes: slot.request_bytes.load(Ordering::Relaxed),
					response_values: slot.response_values.load(Ordering::Relaxed),
					response_bytes: slot.response_bytes.load(Ordering::Relaxed),
					errors: slot.errors.load(Ordering::Relaxed),
					latency_buckets: array::from_fn(|index| {
						slot.latency_buckets[index].load(Ordering::Relaxed)
					}),
				})
			})
			.collect::<Vec<_>>();
		operations.sort_by(|left, right| left.binding.cmp(&right.binding));

		let sequence_total = self.sequence_total.load(Ordering::Acquire);
		let available = usize::try_from(sequence_total)
			.unwrap_or(usize::MAX)
			.min(TRANSCRIPT_CAPACITY);
		let returned = sequence_limit.min(available);
		let start = sequence_total.saturating_sub(returned as u64);
		let sequence = (start..sequence_total)
			.filter_map(|sequence_index| {
				let stored = self.sequence[sequence_index as usize % TRANSCRIPT_CAPACITY]
					.load(Ordering::Acquire);
				(stored != 0).then_some(stored - 1)
			})
			.collect::<Vec<_>>();
		let metrics = RuntimeMetric::all().map(|metric| MetricSnapshot {
			metric,
			value: self.metrics[metric as usize].load(Ordering::Relaxed),
		});
		let class_transitions = array::from_fn(|from| {
			array::from_fn(|to| self.class_transitions[from][to].load(Ordering::Relaxed))
		});
		TelemetrySnapshot {
			operations,
			metrics,
			sequence,
			sequence_dropped: sequence_total.saturating_sub(returned as u64),
			class_transitions,
		}
	}

	fn operation_slot(&self, binding: &'static str, class: OperationClass) -> usize {
		let hash = binding_hash(binding);
		let initial = hash as usize % MAX_OPERATION_SLOTS;
		for offset in 0..MAX_OPERATION_SLOTS {
			let index = (initial + offset) % MAX_OPERATION_SLOTS;
			let slot = &self.slots[index];
			let observed = slot.hash.load(Ordering::Acquire);
			if observed == 0
				&& slot
					.hash
					.compare_exchange(0, hash, Ordering::AcqRel, Ordering::Acquire)
					.is_ok()
			{
				slot.binding
					.set(binding)
					.expect("an unclaimed telemetry slot cannot have a binding");
				slot.class.store(class as u8, Ordering::Release);
				return index;
			}
			if slot.hash.load(Ordering::Acquire) == hash {
				while slot.binding.get().is_none() {
					std::hint::spin_loop();
				}
				if slot.binding.get() == Some(&binding) {
					return index;
				}
			}
		}
		panic!("Dogmos operation telemetry exhausted its fixed slot table")
	}
}

pub struct CallToken<'a> {
	telemetry: &'a Telemetry,
	slot_index: usize,
	value_bytes: u64,
	started: Option<Instant>,
}

impl CallToken<'_> {
	pub fn finish(self, response_values: u64) {
		let duration = self.started.map(|started| started.elapsed());
		self.finish_inner(response_values, false, duration);
	}

	pub fn finish_error(self) {
		let duration = self.started.map(|started| started.elapsed());
		self.finish_inner(0, true, duration);
	}

	pub fn finish_with_duration(self, response_values: u64, duration: Duration) {
		self.finish_inner(response_values, false, Some(duration));
	}

	fn finish_inner(self, response_values: u64, error: bool, duration: Option<Duration>) {
		let slot = &self.telemetry.slots[self.slot_index];
		slot.response_values
			.fetch_add(response_values, Ordering::Relaxed);
		slot.response_bytes.fetch_add(
			response_values.saturating_mul(self.value_bytes),
			Ordering::Relaxed,
		);
		if error {
			slot.errors.fetch_add(1, Ordering::Relaxed);
		}
		if let Some(duration) = duration {
			let nanoseconds = duration.as_nanos().min(u128::from(u64::MAX)) as u64;
			let bucket = if nanoseconds == 0 {
				0
			} else {
				(63 - nanoseconds.leading_zeros() as usize).min(LATENCY_BUCKET_COUNT - 1)
			};
			slot.latency_buckets[bucket].fetch_add(1, Ordering::Relaxed);
		}
	}
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationSnapshot {
	pub slot: u16,
	pub binding: String,
	pub class: OperationClass,
	pub calls: u64,
	pub request_values: u64,
	pub request_bytes: u64,
	pub response_values: u64,
	pub response_bytes: u64,
	pub errors: u64,
	pub latency_buckets: [u64; LATENCY_BUCKET_COUNT],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MetricSnapshot {
	pub metric: RuntimeMetric,
	pub value: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TelemetrySnapshot {
	pub operations: Vec<OperationSnapshot>,
	pub metrics: [MetricSnapshot; RuntimeMetric::COUNT],
	pub sequence: Vec<u16>,
	pub sequence_dropped: u64,
	pub class_transitions: [[u64; OPERATION_CLASS_COUNT]; OPERATION_CLASS_COUNT],
}

impl TelemetrySnapshot {
	#[must_use]
	pub fn metric(&self, metric: RuntimeMetric) -> u64 {
		self.metrics[metric as usize].value
	}
}

#[must_use]
pub fn snapshot_to_json(
	snapshot: &TelemetrySnapshot,
	dreamdaemon_private_bytes: u64,
	dreamdaemon_virtual_bytes: u64,
	server_private_bytes: u64,
	server_virtual_bytes: u64,
) -> String {
	let mut json = String::from("{\"schema_version\":1");
	json.push_str(&format!(
		",\"dreamdaemon_private_bytes\":{dreamdaemon_private_bytes},\"dreamdaemon_virtual_bytes\":{dreamdaemon_virtual_bytes}"
	));
	json.push_str(&format!(
		",\"server_private_bytes\":{server_private_bytes},\"server_virtual_bytes\":{server_virtual_bytes}"
	));
	json.push_str(",\"server_memory_is_separate\":true,\"operations\":[");
	for (index, operation) in snapshot.operations.iter().enumerate() {
		if index != 0 {
			json.push(',');
		}
		json.push_str("{\"slot\":");
		json.push_str(&operation.slot.to_string());
		json.push_str(",\"binding\":\"");
		push_json_string(&mut json, &operation.binding);
		json.push_str("\",\"class\":\"");
		json.push_str(operation_class_name(operation.class));
		json.push_str("\",\"calls\":");
		json.push_str(&operation.calls.to_string());
		json.push_str(",\"request_values\":");
		json.push_str(&operation.request_values.to_string());
		json.push_str(",\"request_bytes\":");
		json.push_str(&operation.request_bytes.to_string());
		json.push_str(",\"response_values\":");
		json.push_str(&operation.response_values.to_string());
		json.push_str(",\"response_bytes\":");
		json.push_str(&operation.response_bytes.to_string());
		json.push_str(",\"errors\":");
		json.push_str(&operation.errors.to_string());
		json.push_str(",\"latency_buckets\":[");
		for (bucket_index, count) in operation.latency_buckets.iter().enumerate() {
			if bucket_index != 0 {
				json.push(',');
			}
			json.push_str(&count.to_string());
		}
		json.push_str("]}");
	}
	json.push_str("],\"metrics\":{");
	for (index, metric) in snapshot.metrics.iter().enumerate() {
		if index != 0 {
			json.push(',');
		}
		json.push('"');
		json.push_str(metric.metric.name());
		json.push_str("\":");
		json.push_str(&metric.value.to_string());
	}
	json.push_str("},\"sequence\":[");
	for (index, slot) in snapshot.sequence.iter().enumerate() {
		if index != 0 {
			json.push(',');
		}
		json.push_str(&slot.to_string());
	}
	json.push_str("],\"sequence_dropped\":");
	json.push_str(&snapshot.sequence_dropped.to_string());
	json.push_str(",\"class_transitions\":[");
	for (row_index, row) in snapshot.class_transitions.iter().enumerate() {
		if row_index != 0 {
			json.push(',');
		}
		json.push('[');
		for (column_index, count) in row.iter().enumerate() {
			if column_index != 0 {
				json.push(',');
			}
			json.push_str(&count.to_string());
		}
		json.push(']');
	}
	json.push_str("]}");
	json
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AllocatorProcessDiagnostics {
	pub layout: AllocationFloorLayout,
	pub allocation_floor_bytes: u64,
	pub elapsed_milliseconds: u64,
	pub user_milliseconds: u64,
	pub system_milliseconds: u64,
	pub current_rss_bytes: u64,
	pub peak_rss_bytes: u64,
	pub current_commit_bytes: u64,
	pub peak_commit_bytes: u64,
	pub page_faults: u64,
}

#[must_use]
pub fn snapshot_to_json_with_diagnostics(
	snapshot: &TelemetrySnapshot,
	dreamdaemon_private_bytes: u64,
	dreamdaemon_virtual_bytes: u64,
	server_private_bytes: u64,
	server_virtual_bytes: u64,
	diagnostics: AllocatorProcessDiagnostics,
) -> String {
	let mut json = snapshot_to_json(
		snapshot,
		dreamdaemon_private_bytes,
		dreamdaemon_virtual_bytes,
		server_private_bytes,
		server_virtual_bytes,
	);
	let closing_brace = json.pop();
	debug_assert_eq!(closing_brace, Some('}'));
	let layout = diagnostics.layout;
	json.push_str(&format!(
		",\"allocation_layout\":{{\"mixture_bytes\":{},\"mixture_lock_bytes\":{},\"turf_mixture_bytes\":{},\"thermal_info_bytes\":{},\"gas_graph_node_bytes\":{},\"heat_graph_node_bytes\":{},\"graph_edge_bytes\":{},\"map_bucket_bytes\":{}}}",
		layout.mixture_bytes,
		layout.mixture_lock_bytes,
		layout.turf_mixture_bytes,
		layout.thermal_info_bytes,
		layout.gas_graph_node_bytes,
		layout.heat_graph_node_bytes,
		layout.graph_edge_bytes,
		layout.map_bucket_bytes,
	));
	json.push_str(&format!(
		",\"allocation_floor_bytes\":{},\"allocator_process_scope\":\"current_process_not_server\",\"allocator_process_elapsed_milliseconds\":{},\"allocator_process_user_milliseconds\":{},\"allocator_process_system_milliseconds\":{},\"allocator_process_current_rss_bytes\":{},\"allocator_process_peak_rss_bytes\":{},\"allocator_process_current_commit_bytes\":{},\"allocator_process_peak_commit_bytes\":{},\"allocator_process_page_faults\":{}",
		diagnostics.allocation_floor_bytes,
		diagnostics.elapsed_milliseconds,
		diagnostics.user_milliseconds,
		diagnostics.system_milliseconds,
		diagnostics.current_rss_bytes,
		diagnostics.peak_rss_bytes,
		diagnostics.current_commit_bytes,
		diagnostics.peak_commit_bytes,
		diagnostics.page_faults,
	));
	json.push('}');
	json
}

fn push_json_string(output: &mut String, value: &str) {
	for character in value.chars() {
		match character {
			'"' => output.push_str("\\\""),
			'\\' => output.push_str("\\\\"),
			'\n' => output.push_str("\\n"),
			'\r' => output.push_str("\\r"),
			'\t' => output.push_str("\\t"),
			character if character.is_control() => {
				output.push_str(&format!("\\u{:04x}", character as u32));
			}
			character => output.push(character),
		}
	}
}

const fn operation_class_name(class: OperationClass) -> &'static str {
	match class {
		OperationClass::ScalarRead => "scalar_read",
		OperationClass::ScalarWrite => "scalar_write",
		OperationClass::MixtureTransaction => "mixture_transaction",
		OperationClass::GraphUpdate => "graph_update",
		OperationClass::SimulationStage => "simulation_stage",
		OperationClass::Callback => "callback",
		OperationClass::Other => "other",
	}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AllocationFloorLayout {
	pub mixture_bytes: u64,
	pub mixture_lock_bytes: u64,
	pub turf_mixture_bytes: u64,
	pub thermal_info_bytes: u64,
	pub gas_graph_node_bytes: u64,
	pub heat_graph_node_bytes: u64,
	pub graph_edge_bytes: u64,
	pub map_bucket_bytes: u64,
}

impl AllocationFloorLayout {
	#[must_use]
	pub const fn audited_i686() -> Self {
		Self {
			mixture_bytes: 60,
			mixture_lock_bytes: 64,
			turf_mixture_bytes: 32,
			thermal_info_bytes: 28,
			gas_graph_node_bytes: 40,
			heat_graph_node_bytes: 36,
			graph_edge_bytes: 20,
			map_bucket_bytes: 12,
		}
	}
}

#[must_use]
pub fn allocation_floor_bytes(
	layout: AllocationFloorLayout,
	mixture_capacity: u64,
	turf_capacity: u64,
	directed_edge_capacity: u64,
) -> Option<u64> {
	let mixture_locks = mixture_capacity.checked_mul(layout.mixture_lock_bytes)?;
	let gas_nodes = turf_capacity.checked_mul(layout.gas_graph_node_bytes)?;
	let heat_nodes = turf_capacity.checked_mul(layout.heat_graph_node_bytes)?;
	let graph_edges = directed_edge_capacity
		.checked_mul(layout.graph_edge_bytes)?
		.checked_mul(2)?;
	let maps = turf_capacity
		.checked_mul(layout.map_bucket_bytes)?
		.checked_mul(2)?;
	mixture_locks
		.checked_add(gas_nodes)?
		.checked_add(heat_nodes)?
		.checked_add(graph_edges)?
		.checked_add(maps)
}

#[must_use]
pub const fn classify_binding(binding: &str) -> OperationClass {
	if contains(binding, "process_") || contains(binding, "equalize_all") {
		return OperationClass::SimulationStage;
	}
	if contains(binding, "callback") {
		return OperationClass::Callback;
	}
	if contains(binding, "adjacency")
		|| contains(binding, "register")
		|| contains(binding, "unregister")
	{
		return OperationClass::GraphUpdate;
	}
	if contains(binding, "merge")
		|| contains(binding, "transfer")
		|| contains(binding, "share")
		|| contains(binding, "equalize_with")
	{
		return OperationClass::MixtureTransaction;
	}
	if contains(binding, "set_")
		|| contains(binding, "adjust")
		|| contains(binding, "clear")
		|| contains(binding, "add")
		|| contains(binding, "subtract")
		|| contains(binding, "multiply")
		|| contains(binding, "divide")
		|| contains(binding, "scrub")
		|| contains(binding, "remove")
		|| contains(binding, "mark_immutable")
	{
		return OperationClass::ScalarWrite;
	}
	if contains(binding, "get_")
		|| contains(binding, "return_")
		|| contains(binding, "total_")
		|| contains(binding, "heat_capacity")
		|| contains(binding, "thermal_energy")
		|| contains(binding, "compare")
		|| contains(binding, "is_immutable")
	{
		return OperationClass::ScalarRead;
	}
	OperationClass::Other
}

const fn contains(haystack: &str, needle: &str) -> bool {
	let haystack = haystack.as_bytes();
	let needle = needle.as_bytes();
	if needle.is_empty() {
		return true;
	}
	if needle.len() > haystack.len() {
		return false;
	}
	let mut start = 0;
	while start + needle.len() <= haystack.len() {
		let mut offset = 0;
		while offset < needle.len() && haystack[start + offset] == needle[offset] {
			offset += 1;
		}
		if offset == needle.len() {
			return true;
		}
		start += 1;
	}
	false
}

const fn binding_hash(binding: &str) -> u64 {
	let mut hash = 0xcbf2_9ce4_8422_2325_u64;
	let bytes = binding.as_bytes();
	let mut index = 0;
	while index < bytes.len() {
		hash ^= bytes[index] as u64;
		hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
		index += 1;
	}
	if hash == 0 {
		1
	} else {
		hash
	}
}

const fn operation_class(value: u8) -> OperationClass {
	match value {
		0 => OperationClass::ScalarRead,
		1 => OperationClass::ScalarWrite,
		2 => OperationClass::MixtureTransaction,
		3 => OperationClass::GraphUpdate,
		4 => OperationClass::SimulationStage,
		5 => OperationClass::Callback,
		_ => OperationClass::Other,
	}
}
