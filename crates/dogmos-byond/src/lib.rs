#![deny(unsafe_op_in_unsafe_fn)]

mod client;
mod ffi;
mod session;

pub use client::{BoundedDogmosClient, ClientError, DogmosClient};
use session::{start_service_session, ServiceSession};

use byondapi::prelude::ByondValue;
use dogmos_process_metrics::{
	sample_current_process, CurrentProcessMetrics, PROCESS_ALL_AVAILABLE, PROCESS_CPU_AVAILABLE,
	PROCESS_PRIVATE_BYTES_AVAILABLE, PROCESS_VIRTUAL_BYTES_AVAILABLE,
	PROCESS_WORKING_SET_AVAILABLE,
};
use dogmos_protocol::{
	decode_adjust_multiple_request, encode_adjust_multiple_request,
	encode_continuation_adjust_multiple_request, encode_gas_metadata_batch, encode_lifecycle_batch,
	encode_mixture_state_batch, encode_reaction_metadata_batch, encode_turf_adjacency_batch,
	encode_turf_heat_adjacency_batch, encode_turf_heat_batch, encode_turf_lifecycle_batch,
	CallbackBatchHeader, CallbackBatchRequest, CallbackEvent, CallbackScope,
	ContinuationCommandRequest, ContinuationResumeRequest, ContinuationToken,
	FrontierAppendRequest, FrontierAppendResponse, FrontierBeginRequest, FrontierBeginResponse,
	FrontierCommitRequest, FrontierCommitResponse, FrontierMutateRequest, FrontierMutateResponse,
	GasMetadataRegistration, LifecycleAction, LifecycleMutation, MixtureAdjustment,
	MixtureCommandRequest, MixtureCommandResponse, MixtureSnapshot, MixtureSnapshotRequest,
	MixtureStateMutation, OperationKind, ReactionMetadataRegistration, ScalarValue,
	ServiceTelemetry, SimulationStage, SimulationStageRequest, SimulationStageResponse,
	TurfAdjacencyMutation, TurfHeatAdjacencyMutation, TurfHeatMutation, TurfHeatSnapshot,
	TurfHeatSnapshotRequest, TurfHeatState, TurfLifecycleMutation, WireFireProducts,
	WireGasFireRole, WireGasProduct, WireGasRequirement, WireHandle, WireReactionExecution,
	CALLBACK_BATCH_HEADER_LEN, CALLBACK_EVENT_LEN, DOGMOS_ABI_VERSION, DOGMOS_PROTOCOL_VERSION,
	GAS_METADATA_RECORD_LEN, MAX_FRONTIER_APPEND_HANDLES, MAX_GAS_SLOTS, MIXTURE_ADJUSTMENT_LEN,
	MIXTURE_ADJUST_MULTIPLE_HEADER_LEN, MIXTURE_COMMAND_REQUEST_LEN, MIXTURE_COMMAND_RESPONSE_LEN,
	MIXTURE_SNAPSHOT_LEN, MIXTURE_STATE_MUTATION_LEN, REACTION_METADATA_RECORD_LEN,
	SERVICE_TELEMETRY_LEN, SIMULATION_STAGE_RESPONSE_LEN, TURF_ADJACENCY_MUTATION_LEN,
	TURF_HEAT_ADJACENCY_MUTATION_LEN, TURF_HEAT_MUTATION_LEN, TURF_HEAT_SNAPSHOT_LEN,
	TURF_LIFECYCLE_MUTATION_LEN,
};
use std::{fs, path::Path, sync::Mutex, time::Duration};

#[cfg(feature = "diagnostic-bindings")]
use dogmos_protocol::{encode_adjacency_batch, AdjacencyMutation, ServiceErrorCode};
#[cfg(feature = "diagnostic-bindings")]
use std::{sync::OnceLock, time::Instant};

#[cfg(feature = "diagnostic-bindings")]
static BENCHMARK_SESSION: Mutex<Option<ServiceSession>> = Mutex::new(None);
static SERVICE_SESSION: Mutex<Option<ServiceSession>> = Mutex::new(None);
#[cfg(feature = "diagnostic-bindings")]
static BENCHMARK_CLOCK: OnceLock<Instant> = OnceLock::new();
#[cfg(feature = "diagnostic-bindings")]
static BENCHMARK_LIFECYCLE_BATCH: OnceLock<Vec<u8>> = OnceLock::new();
#[cfg(feature = "diagnostic-bindings")]
static BENCHMARK_STATE_BATCH: OnceLock<Vec<u8>> = OnceLock::new();
#[cfg(feature = "diagnostic-bindings")]
static BENCHMARK_ADJACENCY_BATCH: OnceLock<Vec<u8>> = OnceLock::new();
const BENCHMARK_CALLBACK_CAPACITY: u32 = 65_536;
const BENCHMARK_CONTROL_PAYLOAD: usize = 64 * 1024;
const BENCHMARK_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_EXACT_BYOND_INTEGER: f32 = 16_777_216.0;
const PRODUCTION_MAX_BATCH_OPERATIONS: usize = 4096;
const PRODUCTION_MAX_MIXTURE_ADJUSTMENTS: usize =
	(BENCHMARK_CONTROL_PAYLOAD - MIXTURE_ADJUST_MULTIPLE_HEADER_LEN) / MIXTURE_ADJUSTMENT_LEN;
const PRODUCTION_MIXTURE_STATE_FIELDS: usize = 6 + MAX_GAS_SLOTS;
const PRODUCTION_MAX_MIXTURE_STATE_MUTATIONS: usize =
	(BENCHMARK_CONTROL_PAYLOAD - 4) / MIXTURE_STATE_MUTATION_LEN;
const PRODUCTION_TURF_LIFECYCLE_FIELDS: usize = 6;
const PRODUCTION_MAX_TURF_LIFECYCLE_MUTATIONS: usize =
	(BENCHMARK_CONTROL_PAYLOAD - 4) / TURF_LIFECYCLE_MUTATION_LEN;
const PRODUCTION_TURF_ADJACENCY_FIELDS: usize = 6;
const PRODUCTION_MAX_TURF_ADJACENCY_MUTATIONS: usize =
	(BENCHMARK_CONTROL_PAYLOAD - 4) / TURF_ADJACENCY_MUTATION_LEN;
const PRODUCTION_TURF_HEAT_FIELDS: usize = 7;
const PRODUCTION_MAX_TURF_HEAT_MUTATIONS: usize =
	(BENCHMARK_CONTROL_PAYLOAD - 4) / TURF_HEAT_MUTATION_LEN;
const PRODUCTION_TURF_HEAT_ADJACENCY_FIELDS: usize = 5;
const PRODUCTION_MAX_TURF_HEAT_ADJACENCY_MUTATIONS: usize =
	(BENCHMARK_CONTROL_PAYLOAD - 4) / TURF_HEAT_ADJACENCY_MUTATION_LEN;
const PRODUCTION_GAS_METADATA_FIELDS: usize = 13;
const PRODUCTION_GAS_PRODUCT_FIELDS: usize = 3;
const PRODUCTION_REACTION_METADATA_FIELDS: usize = 12;
const PRODUCTION_REACTION_REQUIREMENT_FIELDS: usize = 3;
const PRODUCTION_MAX_REACTION_METADATA: usize =
	(BENCHMARK_CONTROL_PAYLOAD - 4) / REACTION_METADATA_RECORD_LEN;
const PRODUCTION_CONTINUATION_TOKEN_FIELDS: usize = 10;
const PRODUCTION_CALLBACK_HEADER_FIELDS: usize = 12;
const PRODUCTION_CALLBACK_EVENT_FIELDS: usize = 36;
const PRODUCTION_MAX_CALLBACK_EVENTS: u32 = 256;
const PROCESS_METRICS_LAYOUT_VERSION: u32 = 1;
const PROCESS_METRICS_FIELDS: usize = 28;
const DREAMDAEMON_PROCESS_FLAGS: u32 = PROCESS_PRIVATE_BYTES_AVAILABLE
	| PROCESS_VIRTUAL_BYTES_AVAILABLE
	| PROCESS_WORKING_SET_AVAILABLE;

#[doc(hidden)]
pub fn generate_bindings_file() {
	byondapi::generate_bindings("dogmos");
	let bindings_path = Path::new("bindings.dm");
	let generated =
		fs::read_to_string(bindings_path).expect("generated bindings should be readable");
	fs::write(bindings_path, normalize_generated_bindings(&generated))
		.expect("normalized bindings should be writable");
}

fn normalize_generated_bindings(bindings: &str) -> String {
	let normalized = bindings.replace("\r\n", "\n").replace('\r', "\n");
	let lines = normalized.lines().map(str::trim_end).collect::<Vec<_>>();
	let define_index = lines
		.iter()
		.position(|line| line.starts_with("#define "))
		.unwrap_or(lines.len());
	let header_end = define_index.saturating_add(1).min(lines.len());
	let generated_state = "/* This comment bypasses grep checks */ /var/__dogmos";
	let mut header = if let Some(state_index) = lines[..header_end]
		.iter()
		.position(|line| *line == generated_state)
	{
		let mut canonical = lines[..state_index].to_vec();
		while canonical.last().is_some_and(|line| line.is_empty()) {
			canonical.pop();
		}
		canonical.push("");
		canonical.push("#define DOGMOS (world.system_type == UNIX ? \"libdogmos\" : \"dogmos\")");
		canonical
	} else {
		lines[..header_end].to_vec()
	};
	while header.last().is_some_and(|line| line.is_empty()) {
		header.pop();
	}

	let mut blocks = lines[header_end..]
		.split(|line| line.is_empty())
		.filter(|block| !block.is_empty())
		.map(normalize_generated_binding_block)
		.collect::<Vec<_>>();
	blocks.sort_by(|left, right| {
		binding_sort_key(left)
			.cmp(binding_sort_key(right))
			.then_with(|| left.cmp(right))
	});

	let mut output = header.join("\n");
	if !blocks.is_empty() {
		output.push_str("\n\n");
		output.push_str(&blocks.join("\n\n"));
	}
	output.push('\n');
	output
}

fn normalize_generated_binding_block(block: &[&str]) -> String {
	const LOAD_PREFIX: &str = "\tvar/static/loaded = load_ext(DOGMOS, ";
	const RETURN_PREFIX: &str = "\treturn call_ext(loaded)";

	let mut output = Vec::with_capacity(block.len());
	let mut index = 0;
	while index < block.len() {
		let line = block[index];
		if let Some(load_argument) = line
			.strip_prefix(LOAD_PREFIX)
			.and_then(|value| value.strip_suffix(')'))
		{
			let invocation = block
				.get(index + 1)
				.and_then(|value| value.strip_prefix(RETURN_PREFIX))
				.expect("generated load_ext must be followed by call_ext");
			output.push(format!(
				"\treturn call_ext(DOGMOS, {load_argument}){invocation}"
			));
			index += 2;
			continue;
		}
		output.push(line.to_owned());
		index += 1;
	}
	output.join("\n")
}

fn binding_sort_key(block: &str) -> &str {
	block
		.lines()
		.find(|line| line.starts_with('/') || line.starts_with("#define "))
		.unwrap_or(block)
}

#[auxmacros::bind("/proc/dogmos_abi_version")]
fn dogmos_abi_version() -> eyre::Result<ByondValue> {
	Ok((DOGMOS_ABI_VERSION as f32).into())
}

#[auxmacros::bind("/proc/dogmos_protocol_version")]
fn dogmos_protocol_version() -> eyre::Result<ByondValue> {
	Ok((DOGMOS_PROTOCOL_VERSION as f32).into())
}

#[auxmacros::bind("/proc/dogmos_source_revision")]
fn dogmos_source_revision() -> eyre::Result<ByondValue> {
	let metadata = dogmos_identity::BuildMetadata::from_compile_environment()?;
	Ok(hex_lower(&metadata.source_revision).try_into()?)
}

#[auxmacros::bind("/proc/dogmos_feature_fingerprint")]
fn dogmos_feature_fingerprint() -> eyre::Result<ByondValue> {
	let metadata = dogmos_identity::BuildMetadata::from_compile_environment()?;
	Ok(hex_lower(&metadata.feature_fingerprint).try_into()?)
}

#[auxmacros::bind("/proc/dogmos_service_start")]
fn dogmos_service_start(service_path: ByondValue) -> eyre::Result<ByondValue> {
	let service_path = service_path.get_string()?;
	let mut session = SERVICE_SESSION
		.lock()
		.map_err(|_| eyre::eyre!("Dogmos production service session lock is poisoned"))?;
	if session.is_some() {
		return Err(eyre::eyre!(
			"Dogmos production service session is already running"
		));
	}
	*session = Some(start_service_session(&service_path)?);
	Ok(true.into())
}

#[auxmacros::bind("/proc/dogmos_service_health")]
fn dogmos_service_health() -> eyre::Result<ByondValue> {
	let mut session = SERVICE_SESSION
		.lock()
		.map_err(|_| eyre::eyre!("Dogmos production service session lock is poisoned"))?;
	let healthy = match session.as_mut() {
		Some(session) => session.is_healthy()?,
		None => false,
	};
	Ok(healthy.into())
}

#[auxmacros::bind("/proc/dogmos_service_pid")]
fn dogmos_service_pid() -> eyre::Result<ByondValue> {
	let session = SERVICE_SESSION
		.lock()
		.map_err(|_| eyre::eyre!("Dogmos production service session lock is poisoned"))?;
	let session = session
		.as_ref()
		.ok_or_else(|| eyre::eyre!("Dogmos production service session is not running"))?;
	Ok((session.client.peer().process_id as f32).into())
}

#[auxmacros::bind("/proc/dogmos_service_world_generation")]
fn dogmos_service_world_generation() -> eyre::Result<ByondValue> {
	let session = SERVICE_SESSION
		.lock()
		.map_err(|_| eyre::eyre!("Dogmos production service session lock is poisoned"))?;
	let session = session
		.as_ref()
		.ok_or_else(|| eyre::eyre!("Dogmos production service session is not running"))?;
	let mut output = ByondValue::new_list()?;
	for word in split_u32_words(session.client.peer().world_generation) {
		output.push_list(f32::from(word).into())?;
	}
	Ok(output)
}

#[auxmacros::bind("/proc/dogmos_service_shutdown")]
fn dogmos_service_shutdown() -> eyre::Result<ByondValue> {
	let mut session = SERVICE_SESSION
		.lock()
		.map_err(|_| eyre::eyre!("Dogmos production service session lock is poisoned"))?;
	let Some(mut active) = session.take() else {
		return Ok(false.into());
	};
	active.shutdown()?;
	Ok(true.into())
}

#[auxmacros::bind("/proc/dogmos_service_telemetry")]
fn dogmos_service_telemetry() -> eyre::Result<ByondValue> {
	let response = production_request(OperationKind::ServiceTelemetry, &[], SERVICE_TELEMETRY_LEN)?;
	let fields = decode_production_service_telemetry(&response)?;
	let mut output = ByondValue::new_list()?;
	for field in fields {
		output.push_list(field.into())?;
	}
	Ok(output)
}

#[auxmacros::bind("/proc/dogmos_process_metrics")]
fn dogmos_process_metrics() -> eyre::Result<ByondValue> {
	let response = production_request(OperationKind::ServiceTelemetry, &[], SERVICE_TELEMETRY_LEN)?;
	let fields = encode_production_process_metrics(sample_current_process(), &response)?;
	let mut output = ByondValue::new_list()?;
	for field in fields {
		output.push_list(field.into())?;
	}
	Ok(output)
}

#[doc(hidden)]
pub fn decode_production_service_telemetry(response: &[u8]) -> eyre::Result<Vec<f32>> {
	let telemetry = ServiceTelemetry::decode(response)?;
	let mut fields = Vec::with_capacity(182);
	for value in [
		telemetry.callback_depth,
		telemetry.callback_capacity,
		telemetry.callback_high_water,
		telemetry.continuation_depth,
		telemetry.continuation_capacity,
		telemetry.continuation_high_water,
	] {
		append_u32_words(&mut fields, value);
	}
	for value in [
		telemetry.oldest_callback_age_ticks,
		telemetry.callback_enqueued,
		telemetry.callback_drained,
		telemetry.callback_rejected,
		telemetry.continuation_timeouts,
		telemetry.request_timeouts,
		telemetry.protocol_errors,
	] {
		append_u64_words(&mut fields, value);
	}
	for counters in [
		telemetry.callback_enqueued_by_kind,
		telemetry.callback_drained_by_kind,
		telemetry.callback_rejected_by_kind,
	] {
		for counter in counters {
			append_u64_words(&mut fields, counter);
		}
	}
	append_u32_words(&mut fields, telemetry.service_process_available_flags);
	append_u64_words(&mut fields, telemetry.service_rss_bytes);
	append_u64_words(&mut fields, telemetry.service_cpu_total_milliseconds);
	for value in [
		telemetry.general_callback_depth,
		telemetry.reaction_callback_depth,
		telemetry.reaction_transaction_depth,
		telemetry.reaction_transaction_high_water,
		telemetry.frontier_count,
		telemetry.stage_kind,
	] {
		append_u32_words(&mut fields, value);
	}
	append_u64_words(&mut fields, telemetry.frontier_upload_bytes);
	append_u64_words(&mut fields, telemetry.stage_epoch);
	append_u32_words(&mut fields, telemetry.stage_cursor);
	append_u32_words(&mut fields, telemetry.stage_remaining);
	append_u64_words(&mut fields, telemetry.topology_revision);
	append_u64_words(&mut fields, telemetry.reusable_workset_bytes);
	append_u64_words(&mut fields, telemetry.packed_topology_bytes);
	Ok(fields)
}

#[doc(hidden)]
pub fn encode_production_process_metrics(
	host: CurrentProcessMetrics,
	service_response: &[u8],
) -> eyre::Result<Vec<f32>> {
	validate_current_process_metrics(host)?;
	let service = ServiceTelemetry::decode(service_response)?;
	let mut fields = Vec::with_capacity(PROCESS_METRICS_FIELDS);
	append_u32_words(&mut fields, PROCESS_METRICS_LAYOUT_VERSION);
	append_u32_words(
		&mut fields,
		host.available_flags & DREAMDAEMON_PROCESS_FLAGS,
	);
	append_u32_words(&mut fields, service.service_process_available_flags);
	append_u32_words(&mut fields, 0);
	append_u64_words(&mut fields, host.private_bytes);
	append_u64_words(&mut fields, host.virtual_bytes);
	append_u64_words(&mut fields, host.working_set_bytes);
	append_u64_words(&mut fields, service.service_rss_bytes);
	append_u64_words(&mut fields, service.service_cpu_total_milliseconds);
	debug_assert_eq!(fields.len(), PROCESS_METRICS_FIELDS);
	Ok(fields)
}

fn validate_current_process_metrics(metrics: CurrentProcessMetrics) -> eyre::Result<()> {
	let unknown_flags = metrics.available_flags & !PROCESS_ALL_AVAILABLE;
	if unknown_flags != 0 {
		return Err(eyre::eyre!(
			"DreamDaemon process metrics contained unknown availability flags {unknown_flags:#x}"
		));
	}
	for (flag, value, name) in [
		(
			PROCESS_PRIVATE_BYTES_AVAILABLE,
			metrics.private_bytes,
			"private bytes",
		),
		(
			PROCESS_VIRTUAL_BYTES_AVAILABLE,
			metrics.virtual_bytes,
			"virtual bytes",
		),
		(
			PROCESS_WORKING_SET_AVAILABLE,
			metrics.working_set_bytes,
			"working-set bytes",
		),
		(
			PROCESS_CPU_AVAILABLE,
			metrics.cpu_total_milliseconds,
			"total CPU milliseconds",
		),
	] {
		if metrics.available_flags & flag == 0 && value != 0 {
			return Err(eyre::eyre!(
				"DreamDaemon process metrics reported nonzero {name} without its availability flag"
			));
		}
	}
	Ok(())
}

#[auxmacros::bind("/proc/dogmos_callback_drain")]
fn dogmos_callback_drain(fields: ByondValue) -> eyre::Result<ByondValue> {
	let fields = bounded_number_list(fields, "callback drain", 7)?;
	if fields.len() != 7 {
		return Err(eyre::eyre!(
			"callback drain requires scope, four transaction words, and two maximum-event words"
		));
	}
	let scope = CallbackScope::try_from(exact_u16(fields[0], "callback scope")?)?;
	let transaction_id = join_u64_words([
		exact_u16(fields[1], "callback transaction word 0")?,
		exact_u16(fields[2], "callback transaction word 1")?,
		exact_u16(fields[3], "callback transaction word 2")?,
		exact_u16(fields[4], "callback transaction word 3")?,
	]);
	let max_events = join_u32_words(
		exact_u16(fields[5], "callback maximum word 0")?,
		exact_u16(fields[6], "callback maximum word 1")?,
	);
	if max_events > PRODUCTION_MAX_CALLBACK_EVENTS {
		return Err(eyre::eyre!(
			"callback drain requested {max_events} events, maximum {PRODUCTION_MAX_CALLBACK_EVENTS}"
		));
	}
	let request = CallbackBatchRequest {
		max_events,
		scope,
		transaction_id,
	}
	.encode()?;
	let response_capacity = CALLBACK_BATCH_HEADER_LEN + max_events as usize * CALLBACK_EVENT_LEN;
	let response = production_request(OperationKind::CallbackBatch, &request, response_capacity)?;
	let fields = decode_production_callback_batch(&response, max_events, scope, transaction_id)?;
	let mut output = ByondValue::new_list()?;
	for field in fields {
		output.push_list(field.into())?;
	}
	Ok(output)
}

#[doc(hidden)]
pub fn decode_production_callback_batch(
	response: &[u8],
	requested_max: u32,
	requested_scope: CallbackScope,
	requested_transaction_id: u64,
) -> eyre::Result<Vec<f32>> {
	if response.len() < CALLBACK_BATCH_HEADER_LEN {
		return Err(eyre::eyre!(
			"Dogmos callback response was {} bytes, shorter than its header",
			response.len()
		));
	}
	let header = CallbackBatchHeader::decode(&response[..CALLBACK_BATCH_HEADER_LEN])?;
	if header.returned > requested_max {
		return Err(eyre::eyre!(
			"Dogmos callback response returned {} events after {} were requested",
			header.returned,
			requested_max
		));
	}
	let expected_len = CALLBACK_BATCH_HEADER_LEN + header.returned as usize * CALLBACK_EVENT_LEN;
	if response.len() != expected_len {
		return Err(eyre::eyre!(
			"Dogmos callback response was {} bytes, expected {expected_len}",
			response.len()
		));
	}
	let mut fields = Vec::with_capacity(
		PRODUCTION_CALLBACK_HEADER_FIELDS
			+ header.returned as usize * PRODUCTION_CALLBACK_EVENT_FIELDS,
	);
	for value in [
		header.returned,
		header.remaining,
		header.capacity,
		header.high_water,
	] {
		append_u32_words(&mut fields, value);
	}
	append_u64_words(&mut fields, header.rejected);
	let mut last_sequence: Option<u64> = None;
	for event_bytes in response[CALLBACK_BATCH_HEADER_LEN..]
		.as_chunks::<CALLBACK_EVENT_LEN>()
		.0
	{
		let event = CallbackEvent::decode(event_bytes)?;
		if event.scope != requested_scope || event.transaction_id != requested_transaction_id {
			return Err(eyre::eyre!(
				"Dogmos callback response returned the wrong scope or transaction"
			));
		}
		if let Some(sequence) = last_sequence {
			let expected = sequence.checked_add(1).ok_or_else(|| {
				eyre::eyre!("Dogmos callback sequence overflowed after {sequence}")
			})?;
			if event.scope_sequence != expected {
				return Err(eyre::eyre!(
					"Dogmos callback sequence is not contiguous at {}",
					event.scope_sequence
				));
			}
		}
		last_sequence = Some(event.scope_sequence);
		append_u64_words(&mut fields, event.scope_sequence);
		append_u64_words(&mut fields, event.transaction_id);
		fields.push(event.scope as u16 as f32);
		fields.push(event.kind as u16 as f32);
		fields.push(f32::from(event.flags));
		append_u32_words(&mut fields, event.subject.slot);
		append_u32_words(&mut fields, event.subject.generation);
		append_u32_words(&mut fields, event.target.slot);
		append_u32_words(&mut fields, event.target.generation);
		for (index, value) in event.values.into_iter().enumerate() {
			fields.push(finite_byond_scalar(
				value.0,
				&format!("callback value {index}"),
			)?);
		}
		append_u32_words(&mut fields, event.aux);
		fields.push(f32::from(event.continuation.is_some()));
		append_continuation_token_fields(&mut fields, event.continuation);
	}
	Ok(fields)
}

#[auxmacros::bind("/proc/dogmos_continuation_command")]
fn dogmos_continuation_command(fields: ByondValue) -> eyre::Result<ByondValue> {
	let fields = bounded_number_list(
		fields,
		"continuation command",
		PRODUCTION_CONTINUATION_TOKEN_FIELDS + 11,
	)?;
	let request = encode_production_continuation_command(&fields)?;
	let response = production_request(
		OperationKind::ContinuationCommand,
		&request,
		MIXTURE_COMMAND_RESPONSE_LEN,
	)?;
	mixture_command_response_value(MixtureCommandResponse::decode(&response)?)
}

#[doc(hidden)]
pub fn encode_production_continuation_command(fields: &[f32]) -> eyre::Result<Vec<u8>> {
	if fields.len() != PRODUCTION_CONTINUATION_TOKEN_FIELDS + 11 {
		return Err(eyre::eyre!(
			"continuation command requires a 10-field token and 11 command fields"
		));
	}
	let token =
		decode_production_continuation_token(&fields[..PRODUCTION_CONTINUATION_TOKEN_FIELDS])?;
	let command = MixtureCommandRequest::decode(&encode_production_mixture_command(
		fields[PRODUCTION_CONTINUATION_TOKEN_FIELDS..]
			.try_into()
			.unwrap(),
	)?)?;
	Ok(ContinuationCommandRequest { token, command }
		.encode()?
		.to_vec())
}

#[auxmacros::bind("/proc/dogmos_continuation_adjust_multiple")]
fn dogmos_continuation_adjust_multiple(fields: ByondValue) -> eyre::Result<ByondValue> {
	let fields = bounded_number_list(
		fields,
		"continuation multi-adjust",
		PRODUCTION_CONTINUATION_TOKEN_FIELDS + 2 + PRODUCTION_MAX_MIXTURE_ADJUSTMENTS * 2,
	)?;
	let request = encode_production_continuation_adjust_multiple(&fields)?;
	let response = production_request(
		OperationKind::ContinuationAdjustMultiple,
		&request,
		MIXTURE_COMMAND_RESPONSE_LEN,
	)?;
	mixture_command_response_value(MixtureCommandResponse::decode(&response)?)
}

#[doc(hidden)]
pub fn encode_production_continuation_adjust_multiple(fields: &[f32]) -> eyre::Result<Vec<u8>> {
	if fields.len() < PRODUCTION_CONTINUATION_TOKEN_FIELDS + 2 {
		return Err(eyre::eyre!(
			"continuation multi-adjust requires a 10-field token and mixture adjustments"
		));
	}
	let token =
		decode_production_continuation_token(&fields[..PRODUCTION_CONTINUATION_TOKEN_FIELDS])?;
	let nested =
		encode_production_mixture_adjust_multiple(&fields[PRODUCTION_CONTINUATION_TOKEN_FIELDS..])?;
	let (handle, adjustments) = decode_adjust_multiple_request(&nested)?;
	let mut output = Vec::new();
	encode_continuation_adjust_multiple_request(token, handle, &adjustments, &mut output)?;
	Ok(output)
}

#[auxmacros::bind("/proc/dogmos_continuation_resume")]
fn dogmos_continuation_resume(fields: ByondValue) -> eyre::Result<ByondValue> {
	let fields = bounded_number_list(
		fields,
		"continuation resume",
		PRODUCTION_CONTINUATION_TOKEN_FIELDS + 1,
	)?;
	let request = encode_production_continuation_resume(&fields)?;
	let response = production_request(
		OperationKind::ContinuationResume,
		&request,
		MIXTURE_COMMAND_RESPONSE_LEN,
	)?;
	mixture_command_response_value(MixtureCommandResponse::decode(&response)?)
}

#[doc(hidden)]
pub fn encode_production_continuation_resume(fields: &[f32]) -> eyre::Result<Vec<u8>> {
	if fields.len() != PRODUCTION_CONTINUATION_TOKEN_FIELDS + 1 {
		return Err(eyre::eyre!(
			"continuation resume requires a 10-field token and reaction result"
		));
	}
	let token =
		decode_production_continuation_token(&fields[..PRODUCTION_CONTINUATION_TOKEN_FIELDS])?;
	let reaction_result = exact_u32(
		fields[PRODUCTION_CONTINUATION_TOKEN_FIELDS],
		"continuation reaction result",
	)?;
	Ok(ContinuationResumeRequest {
		token,
		reaction_result,
	}
	.encode()?
	.to_vec())
}

#[auxmacros::bind("/proc/dogmos_continuation_cancel")]
fn dogmos_continuation_cancel(fields: ByondValue) -> eyre::Result<ByondValue> {
	let fields = bounded_number_list(
		fields,
		"continuation cancel token",
		PRODUCTION_CONTINUATION_TOKEN_FIELDS,
	)?;
	let token = decode_production_continuation_token(&fields)?;
	let response = production_request(OperationKind::ContinuationCancel, &token.encode()?, 0)?;
	if !response.is_empty() {
		return Err(eyre::eyre!(
			"Dogmos continuation cancel response was not empty"
		));
	}
	Ok(true.into())
}

#[doc(hidden)]
pub fn decode_production_continuation_token(fields: &[f32]) -> eyre::Result<ContinuationToken> {
	if fields.len() != PRODUCTION_CONTINUATION_TOKEN_FIELDS {
		return Err(eyre::eyre!(
			"continuation token requires exactly {PRODUCTION_CONTINUATION_TOKEN_FIELDS} fields"
		));
	}
	let words = fields
		.iter()
		.enumerate()
		.map(|(index, value)| exact_u16(*value, &format!("continuation token word {index}")))
		.collect::<eyre::Result<Vec<_>>>()?;
	let token = ContinuationToken {
		world_generation: join_u32_words(words[0], words[1]),
		id: join_u64_words(words[2..6].try_into().unwrap()),
		deadline_ticks: join_u64_words(words[6..10].try_into().unwrap()),
	};
	token.encode()?;
	Ok(token)
}

#[auxmacros::bind("/proc/dogmos_mixture_command")]
fn dogmos_mixture_command(fields: ByondValue) -> eyre::Result<ByondValue> {
	let fields = bounded_number_list(fields, "mixture command", 11)?;
	if fields.len() != 11 {
		return Err(eyre::eyre!(
			"mixture command requires exactly 11 numeric fields"
		));
	}
	let request = encode_production_mixture_command(fields.try_into().unwrap())?;
	let response = production_request(
		OperationKind::MixtureCommand,
		&request,
		MIXTURE_COMMAND_RESPONSE_LEN,
	)?;
	if response.len() != MIXTURE_COMMAND_RESPONSE_LEN {
		return Err(eyre::eyre!(
			"Dogmos mixture command response was {} bytes, expected {MIXTURE_COMMAND_RESPONSE_LEN}",
			response.len()
		));
	}
	mixture_command_response_value(MixtureCommandResponse::decode(&response)?)
}

#[doc(hidden)]
pub fn encode_production_mixture_command(
	fields: [f32; 11],
) -> eyre::Result<[u8; MIXTURE_COMMAND_REQUEST_LEN]> {
	encode_dm_mixture_command(DmMixtureCommandFields {
		kind: exact_u16(fields[0], "mixture command kind")?,
		flags: exact_u16(fields[1], "mixture command flags")?,
		primary: WireHandle {
			slot: exact_u32(fields[2], "primary mixture slot")?,
			generation: exact_u32(fields[3], "primary mixture generation")?,
		},
		secondary: WireHandle {
			slot: exact_u32(fields[4], "secondary mixture slot")?,
			generation: exact_u32(fields[5], "secondary mixture generation")?,
		},
		scalars: [fields[6], fields[7], fields[8]],
		gas_id: exact_u16(fields[9], "gas id")?,
		aux: exact_u32(fields[10], "mixture command auxiliary value")?,
	})
}

#[auxmacros::bind("/proc/dogmos_mixture_adjust_multiple")]
fn dogmos_mixture_adjust_multiple(fields: ByondValue) -> eyre::Result<ByondValue> {
	let fields = bounded_number_list(
		fields,
		"mixture multi-adjust command",
		2 + PRODUCTION_MAX_MIXTURE_ADJUSTMENTS * 2,
	)?;
	let request = encode_production_mixture_adjust_multiple(&fields)?;
	let response = production_request(
		OperationKind::MixtureAdjustMultiple,
		&request,
		MIXTURE_COMMAND_RESPONSE_LEN,
	)?;
	if response.len() != MIXTURE_COMMAND_RESPONSE_LEN {
		return Err(eyre::eyre!(
			"Dogmos mixture multi-adjust response was {} bytes, expected {MIXTURE_COMMAND_RESPONSE_LEN}",
			response.len()
		));
	}
	mixture_command_response_value(MixtureCommandResponse::decode(&response)?)
}

#[doc(hidden)]
pub fn encode_production_mixture_adjust_multiple(values: &[f32]) -> eyre::Result<Vec<u8>> {
	if values.len() < 2 || !(values.len() - 2).is_multiple_of(2) {
		return Err(eyre::eyre!(
			"mixture multi-adjust requires slot, generation, and gas/delta pairs"
		));
	}
	let adjustment_count = (values.len() - 2) / 2;
	if adjustment_count > PRODUCTION_MAX_MIXTURE_ADJUSTMENTS {
		return Err(eyre::eyre!(
			"mixture multi-adjust contains {adjustment_count} adjustments, maximum {PRODUCTION_MAX_MIXTURE_ADJUSTMENTS}"
		));
	}
	let handle = WireHandle {
		slot: exact_u32(values[0], "multi-adjust mixture slot")?,
		generation: exact_u32(values[1], "multi-adjust mixture generation")?,
	};
	let adjustments = values[2..]
		.as_chunks::<2>()
		.0
		.iter()
		.enumerate()
		.map(|(index, entry)| {
			// Avoids a &format!(...) allocation per adjustment (this can run once per gas type in
			// a batched multi-adjust call, which is the whole point of batching) - only format
			// the "entry N" label if the value actually fails validation.
			let gas_id = exact_u32(entry[0], "multi-adjust entry gas id")
				.and_then(|value| {
					u16::try_from(value)
						.map_err(|_| eyre::eyre!("value exceeds the u16 wire range"))
				})
				.map_err(|error| eyre::eyre!("multi-adjust entry {index} gas id: {error}"))?;
			Ok(MixtureAdjustment {
				gas_id,
				delta: ScalarValue(f64::from(entry[1])),
			})
		})
		.collect::<eyre::Result<Vec<_>>>()?;
	let mut output = Vec::new();
	encode_adjust_multiple_request(handle, &adjustments, &mut output)?;
	Ok(output)
}

#[auxmacros::bind("/proc/dogmos_mixture_lifecycle_batch")]
fn dogmos_mixture_lifecycle_batch(entries: ByondValue) -> eyre::Result<ByondValue> {
	let values = bounded_number_list(
		entries,
		"mixture lifecycle batch",
		PRODUCTION_MAX_BATCH_OPERATIONS * 3,
	)?;
	let request = encode_production_mixture_lifecycle_batch(&values)?;
	let response = production_request(OperationKind::MixtureLifecycleBatch, &request, 4)?;
	let response: [u8; 4] = response.try_into().map_err(|response: Vec<u8>| {
		eyre::eyre!(
			"Dogmos mixture lifecycle response was {} bytes, expected 4",
			response.len()
		)
	})?;
	Ok((u32::from_le_bytes(response) as f32).into())
}

#[auxmacros::bind("/proc/dogmos_mixture_snapshot")]
fn dogmos_mixture_snapshot(fields: ByondValue) -> eyre::Result<ByondValue> {
	let fields = bounded_number_list(fields, "mixture snapshot", 2)?;
	if fields.len() != 2 {
		return Err(eyre::eyre!(
			"mixture snapshot requires exactly slot and generation"
		));
	}
	let request = MixtureSnapshotRequest {
		handle: WireHandle {
			slot: exact_u32(fields[0], "mixture snapshot slot")?,
			generation: exact_u32(fields[1], "mixture snapshot generation")?,
		},
	}
	.encode();
	let response = production_request(
		OperationKind::MixtureSnapshot,
		&request,
		MIXTURE_SNAPSHOT_LEN,
	)?;
	let fields = decode_production_mixture_snapshot(&response)?;
	let mut output = ByondValue::new_list()?;
	for field in fields {
		output.push_list(field.into())?;
	}
	Ok(output)
}

#[doc(hidden)]
pub fn decode_production_mixture_snapshot(response: &[u8]) -> eyre::Result<Vec<f32>> {
	let snapshot = MixtureSnapshot::decode(response)?;
	let revision_words = split_u32_words(snapshot.revision);
	let mut fields = Vec::with_capacity(10 + MAX_GAS_SLOTS);
	fields.extend([
		f32::from(revision_words[0]),
		f32::from(revision_words[1]),
		snapshot.gas_count as f32,
		finite_byond_scalar(snapshot.temperature.0, "mixture snapshot temperature")?,
		finite_byond_scalar(snapshot.volume.0, "mixture snapshot volume")?,
		finite_byond_scalar(
			snapshot.minimum_heat_capacity.0,
			"mixture snapshot minimum heat capacity",
		)?,
		finite_byond_scalar(snapshot.total_moles.0, "mixture snapshot total moles")?,
		finite_byond_scalar(snapshot.pressure.0, "mixture snapshot pressure")?,
		finite_byond_scalar(snapshot.heat_capacity.0, "mixture snapshot heat capacity")?,
		f32::from(snapshot.immutable),
	]);
	for (index, gas) in snapshot.gases.into_iter().enumerate() {
		fields.push(finite_byond_scalar(
			gas.0,
			&format!("mixture snapshot gas {index}"),
		)?);
	}
	Ok(fields)
}

#[auxmacros::bind("/proc/dogmos_mixture_state_batch")]
fn dogmos_mixture_state_batch(entries: ByondValue) -> eyre::Result<ByondValue> {
	let values = bounded_number_list(
		entries,
		"mixture state batch",
		PRODUCTION_MAX_MIXTURE_STATE_MUTATIONS * PRODUCTION_MIXTURE_STATE_FIELDS,
	)?;
	let request = encode_production_mixture_state_batch(&values)?;
	let response = production_request(OperationKind::MixtureStateBatch, &request, 4)?;
	let response: [u8; 4] = response.try_into().map_err(|response: Vec<u8>| {
		eyre::eyre!(
			"Dogmos mixture state response was {} bytes, expected 4",
			response.len()
		)
	})?;
	Ok((u32::from_le_bytes(response) as f32).into())
}

#[doc(hidden)]
pub fn encode_production_mixture_state_batch(values: &[f32]) -> eyre::Result<Vec<u8>> {
	if !values.len().is_multiple_of(PRODUCTION_MIXTURE_STATE_FIELDS) {
		return Err(eyre::eyre!(
			"mixture state batch requires fixed {PRODUCTION_MIXTURE_STATE_FIELDS}-field records"
		));
	}
	let operation_count = values.len() / PRODUCTION_MIXTURE_STATE_FIELDS;
	if operation_count > PRODUCTION_MAX_MIXTURE_STATE_MUTATIONS {
		return Err(eyre::eyre!(
			"mixture state batch contains {operation_count} operations, maximum {PRODUCTION_MAX_MIXTURE_STATE_MUTATIONS}"
		));
	}
	let mutations = values
		.as_chunks::<PRODUCTION_MIXTURE_STATE_FIELDS>()
		.0
		.iter()
		.enumerate()
		.map(|(index, entry)| {
			let mut gases = [ScalarValue(0.0); MAX_GAS_SLOTS];
			for (gas_index, gas) in gases.iter_mut().enumerate() {
				*gas = ScalarValue(f64::from(entry[6 + gas_index]));
			}
			Ok(MixtureStateMutation {
				handle: WireHandle {
					slot: exact_u32(entry[0], &format!("mixture state entry {index} slot"))?,
					generation: exact_u32(
						entry[1],
						&format!("mixture state entry {index} generation"),
					)?,
				},
				expected_revision: join_u32_words(
					exact_u16(
						entry[2],
						&format!("mixture state entry {index} revision low word"),
					)?,
					exact_u16(
						entry[3],
						&format!("mixture state entry {index} revision high word"),
					)?,
				),
				temperature: ScalarValue(f64::from(entry[4])),
				volume: ScalarValue(f64::from(entry[5])),
				gases,
			})
		})
		.collect::<eyre::Result<Vec<_>>>()?;
	let mut output = Vec::new();
	encode_mixture_state_batch(&mutations, &mut output)?;
	Ok(output)
}

#[auxmacros::bind("/proc/dogmos_gas_metadata_install")]
fn dogmos_gas_metadata_install(
	numeric_records: ByondValue,
	keys: ByondValue,
	names: ByondValue,
	product_records: ByondValue,
) -> eyre::Result<ByondValue> {
	let numeric_records = bounded_number_list(
		numeric_records,
		"gas metadata numeric records",
		MAX_GAS_SLOTS * PRODUCTION_GAS_METADATA_FIELDS,
	)?;
	let record_count = numeric_records.len() / PRODUCTION_GAS_METADATA_FIELDS;
	let keys = bounded_string_list(keys, "gas metadata keys", MAX_GAS_SLOTS)?;
	let names = bounded_string_list(names, "gas metadata names", MAX_GAS_SLOTS)?;
	let product_records = bounded_number_list(
		product_records,
		"gas metadata product records",
		MAX_GAS_SLOTS * MAX_GAS_SLOTS * PRODUCTION_GAS_PRODUCT_FIELDS,
	)?;
	if keys.len() != record_count || names.len() != record_count {
		return Err(eyre::eyre!(
			"gas metadata numeric, key, and name record counts must match"
		));
	}
	let request =
		encode_production_gas_metadata(&numeric_records, &keys, &names, &product_records)?;
	production_counted_request(OperationKind::GasMetadataInstall, &request, "gas metadata")
}

#[doc(hidden)]
pub fn encode_production_gas_metadata(
	numeric_records: &[f32],
	keys: &[String],
	names: &[String],
	product_records: &[f32],
) -> eyre::Result<Vec<u8>> {
	validate_fixed_records(
		numeric_records,
		PRODUCTION_GAS_METADATA_FIELDS,
		MAX_GAS_SLOTS,
		"gas metadata numeric records",
	)?;
	let record_count = numeric_records.len() / PRODUCTION_GAS_METADATA_FIELDS;
	if keys.len() != record_count || names.len() != record_count {
		return Err(eyre::eyre!(
			"gas metadata numeric, key, and name record counts must match"
		));
	}
	validate_fixed_records(
		product_records,
		PRODUCTION_GAS_PRODUCT_FIELDS,
		record_count.saturating_mul(MAX_GAS_SLOTS),
		"gas metadata product records",
	)?;
	let mut products = vec![Vec::new(); record_count];
	for (index, entry) in product_records
		.as_chunks::<PRODUCTION_GAS_PRODUCT_FIELDS>()
		.0
		.iter()
		.enumerate()
	{
		let owner =
			exact_u32(entry[0], &format!("gas product entry {index} owner index"))? as usize;
		let owner_products = products
			.get_mut(owner)
			.ok_or_else(|| eyre::eyre!("gas product entry {index} owner index is out of range"))?;
		if owner_products.len() == MAX_GAS_SLOTS {
			return Err(eyre::eyre!(
				"gas metadata record {owner} exceeds {MAX_GAS_SLOTS} products"
			));
		}
		owner_products.push(WireGasProduct {
			gas_id: exact_u16(entry[1], &format!("gas product entry {index} gas id"))?,
			ratio: ScalarValue(f64::from(entry[2])),
		});
	}
	let entries = numeric_records
		.as_chunks::<PRODUCTION_GAS_METADATA_FIELDS>()
		.0
		.iter()
		.enumerate()
		.map(|(index, fields)| {
			let moles_visible_present = exact_bool(
				fields[5],
				&format!("gas metadata entry {index} moles-visible flag"),
			)?;
			if !moles_visible_present && fields[6] != 0.0 {
				return Err(eyre::eyre!(
					"gas metadata entry {index} has moles-visible data while the flag is false"
				));
			}
			let fire_role =
				match exact_u32(fields[9], &format!("gas metadata entry {index} fire role"))? {
					0 if fields[10] == 0.0 && fields[11] == 0.0 => WireGasFireRole::None,
					0 => {
						return Err(eyre::eyre!(
							"gas metadata entry {index} has fire-role values for role none"
						));
					}
					1 => WireGasFireRole::Oxidizer {
						minimum_temperature: ScalarValue(f64::from(fields[10])),
						power: ScalarValue(f64::from(fields[11])),
					},
					2 => WireGasFireRole::Fuel {
						minimum_temperature: ScalarValue(f64::from(fields[10])),
						burn_rate: ScalarValue(f64::from(fields[11])),
					},
					actual => return Err(eyre::eyre!("unknown gas fire role {actual}")),
				};
			let fire_products = match exact_u32(
				fields[12],
				&format!("gas metadata entry {index} product kind"),
			)? {
				0 if products[index].is_empty() => None,
				1 => Some(WireFireProducts::Generic(products[index].clone())),
				2 if products[index].is_empty() => Some(WireFireProducts::Plasma),
				actual => {
					return Err(eyre::eyre!(
						"gas metadata entry {index} has invalid product kind or unexpected product records: {actual}"
					));
				}
			};
			Ok(GasMetadataRegistration {
				id: exact_u16(fields[0], &format!("gas metadata entry {index} id"))?,
				key: keys[index].clone(),
				name: names[index].clone(),
				flags: join_u32_words(
					exact_u16(
						fields[1],
						&format!("gas metadata entry {index} flags low word"),
					)?,
					exact_u16(
						fields[2],
						&format!("gas metadata entry {index} flags high word"),
					)?,
				),
				specific_heat: ScalarValue(f64::from(fields[3])),
				fusion_power: ScalarValue(f64::from(fields[4])),
				moles_visible: moles_visible_present.then_some(ScalarValue(f64::from(fields[6]))),
				enthalpy: ScalarValue(f64::from(fields[7])),
				fire_radiation_released: ScalarValue(f64::from(fields[8])),
				fire_role,
				fire_products,
			})
		})
		.collect::<eyre::Result<Vec<_>>>()?;
	let mut output = Vec::with_capacity(4 + entries.len() * GAS_METADATA_RECORD_LEN);
	encode_gas_metadata_batch(&entries, &mut output)?;
	Ok(output)
}

#[auxmacros::bind("/proc/dogmos_reaction_metadata_install")]
fn dogmos_reaction_metadata_install(
	numeric_records: ByondValue,
	keys: ByondValue,
	requirement_records: ByondValue,
) -> eyre::Result<ByondValue> {
	let numeric_records = bounded_number_list(
		numeric_records,
		"reaction metadata numeric records",
		PRODUCTION_MAX_REACTION_METADATA * PRODUCTION_REACTION_METADATA_FIELDS,
	)?;
	let record_count = numeric_records.len() / PRODUCTION_REACTION_METADATA_FIELDS;
	let keys = bounded_string_list(
		keys,
		"reaction metadata keys",
		PRODUCTION_MAX_REACTION_METADATA,
	)?;
	let requirement_records = bounded_number_list(
		requirement_records,
		"reaction metadata requirement records",
		PRODUCTION_MAX_REACTION_METADATA * MAX_GAS_SLOTS * PRODUCTION_REACTION_REQUIREMENT_FIELDS,
	)?;
	if keys.len() != record_count {
		return Err(eyre::eyre!(
			"reaction metadata numeric and key record counts must match"
		));
	}
	let request =
		encode_production_reaction_metadata(&numeric_records, &keys, &requirement_records)?;
	production_counted_request(
		OperationKind::ReactionMetadataInstall,
		&request,
		"reaction metadata",
	)
}

#[doc(hidden)]
pub fn encode_production_reaction_metadata(
	numeric_records: &[f32],
	keys: &[String],
	requirement_records: &[f32],
) -> eyre::Result<Vec<u8>> {
	validate_fixed_records(
		numeric_records,
		PRODUCTION_REACTION_METADATA_FIELDS,
		PRODUCTION_MAX_REACTION_METADATA,
		"reaction metadata numeric records",
	)?;
	let record_count = numeric_records.len() / PRODUCTION_REACTION_METADATA_FIELDS;
	if keys.len() != record_count {
		return Err(eyre::eyre!(
			"reaction metadata numeric and key record counts must match"
		));
	}
	validate_fixed_records(
		requirement_records,
		PRODUCTION_REACTION_REQUIREMENT_FIELDS,
		record_count.saturating_mul(MAX_GAS_SLOTS),
		"reaction metadata requirement records",
	)?;
	let mut requirements = vec![Vec::new(); record_count];
	for (index, entry) in requirement_records
		.as_chunks::<PRODUCTION_REACTION_REQUIREMENT_FIELDS>()
		.0
		.iter()
		.enumerate()
	{
		let owner = exact_u32(
			entry[0],
			&format!("reaction requirement entry {index} owner index"),
		)? as usize;
		let owner_requirements = requirements.get_mut(owner).ok_or_else(|| {
			eyre::eyre!("reaction requirement entry {index} owner index is out of range")
		})?;
		if owner_requirements.len() == MAX_GAS_SLOTS {
			return Err(eyre::eyre!(
				"reaction metadata record {owner} exceeds {MAX_GAS_SLOTS} requirements"
			));
		}
		owner_requirements.push(WireGasRequirement {
			gas_id: exact_u16(
				entry[1],
				&format!("reaction requirement entry {index} gas id"),
			)?,
			minimum_moles: ScalarValue(f64::from(entry[2])),
		});
	}
	let entries = numeric_records
		.as_chunks::<PRODUCTION_REACTION_METADATA_FIELDS>()
		.0
		.iter()
		.enumerate()
		.map(|(index, fields)| {
			let execution = match exact_u32(
				fields[2],
				&format!("reaction metadata entry {index} execution"),
			)? {
				0 => WireReactionExecution::Dm,
				1 => WireReactionExecution::NativePlasma,
				2 => WireReactionExecution::NativeHydrogen,
				3 => WireReactionExecution::NativeTritium,
				4 => WireReactionExecution::NativeFreon,
				actual => return Err(eyre::eyre!("unknown reaction execution {actual}")),
			};
			let option = |present: f32, value: f32, label: &str| -> eyre::Result<_> {
				let present = exact_bool(present, label)?;
				if !present && value != 0.0 {
					return Err(eyre::eyre!("{label} has data while the flag is false"));
				}
				Ok(present.then_some(ScalarValue(f64::from(value))))
			};
			Ok(ReactionMetadataRegistration {
				id: join_u32_words(
					exact_u16(
						fields[0],
						&format!("reaction metadata entry {index} id low word"),
					)?,
					exact_u16(
						fields[1],
						&format!("reaction metadata entry {index} id high word"),
					)?,
				),
				key: keys[index].clone(),
				priority: ScalarValue(f64::from(fields[3])),
				minimum_temperature: option(
					fields[4],
					fields[5],
					&format!("reaction metadata entry {index} minimum-temperature"),
				)?,
				maximum_temperature: option(
					fields[6],
					fields[7],
					&format!("reaction metadata entry {index} maximum-temperature"),
				)?,
				minimum_energy: option(
					fields[8],
					fields[9],
					&format!("reaction metadata entry {index} minimum-energy"),
				)?,
				minimum_fire_reagents: option(
					fields[10],
					fields[11],
					&format!("reaction metadata entry {index} minimum-fire-reagents"),
				)?,
				gas_requirements: requirements[index].clone(),
				execution,
			})
		})
		.collect::<eyre::Result<Vec<_>>>()?;
	let mut output = Vec::with_capacity(4 + entries.len() * REACTION_METADATA_RECORD_LEN);
	encode_reaction_metadata_batch(&entries, &mut output)?;
	Ok(output)
}

#[auxmacros::bind("/proc/dogmos_turf_lifecycle_batch")]
fn dogmos_turf_lifecycle_batch(entries: ByondValue) -> eyre::Result<ByondValue> {
	let values = bounded_number_list(
		entries,
		"turf lifecycle batch",
		PRODUCTION_MAX_TURF_LIFECYCLE_MUTATIONS * PRODUCTION_TURF_LIFECYCLE_FIELDS,
	)?;
	let request = encode_production_turf_lifecycle_batch(&values)?;
	production_counted_request(
		OperationKind::TurfLifecycleBatch,
		&request,
		"turf lifecycle",
	)
}

#[doc(hidden)]
pub fn encode_production_turf_lifecycle_batch(values: &[f32]) -> eyre::Result<Vec<u8>> {
	validate_fixed_records(
		values,
		PRODUCTION_TURF_LIFECYCLE_FIELDS,
		PRODUCTION_MAX_TURF_LIFECYCLE_MUTATIONS,
		"turf lifecycle batch",
	)?;
	let mutations = values
		.as_chunks::<PRODUCTION_TURF_LIFECYCLE_FIELDS>()
		.0
		.iter()
		.enumerate()
		.map(|(index, entry)| {
			let action = LifecycleAction::try_from(exact_u32(
				entry[0],
				&format!("turf lifecycle entry {index} action"),
			)?)?;
			let mixture_present = exact_bool(
				entry[3],
				&format!("turf lifecycle entry {index} mixture-present flag"),
			)?;
			let mixture = WireHandle {
				slot: exact_u32(
					entry[4],
					&format!("turf lifecycle entry {index} mixture slot"),
				)?,
				generation: exact_u32(
					entry[5],
					&format!("turf lifecycle entry {index} mixture generation"),
				)?,
			};
			if !mixture_present
				&& mixture
					!= (WireHandle {
						slot: 0,
						generation: 0,
					}) {
				return Err(eyre::eyre!(
					"turf lifecycle entry {index} has a mixture handle while the present flag is false"
				));
			}
			Ok(TurfLifecycleMutation {
				action,
				turf: WireHandle {
					slot: exact_u32(entry[1], &format!("turf lifecycle entry {index} slot"))?,
					generation: exact_u32(
						entry[2],
						&format!("turf lifecycle entry {index} generation"),
					)?,
				},
				mixture: mixture_present.then_some(mixture),
			})
		})
		.collect::<eyre::Result<Vec<_>>>()?;
	let mut output = Vec::new();
	encode_turf_lifecycle_batch(&mutations, &mut output)?;
	Ok(output)
}

#[auxmacros::bind("/proc/dogmos_turf_adjacency_batch")]
fn dogmos_turf_adjacency_batch(entries: ByondValue) -> eyre::Result<ByondValue> {
	let values = bounded_number_list(
		entries,
		"turf adjacency batch",
		PRODUCTION_MAX_TURF_ADJACENCY_MUTATIONS * PRODUCTION_TURF_ADJACENCY_FIELDS,
	)?;
	let request = encode_production_turf_adjacency_batch(&values)?;
	production_counted_request(
		OperationKind::TurfAdjacencyBatch,
		&request,
		"turf adjacency",
	)
}

#[doc(hidden)]
pub fn encode_production_turf_adjacency_batch(values: &[f32]) -> eyre::Result<Vec<u8>> {
	validate_fixed_records(
		values,
		PRODUCTION_TURF_ADJACENCY_FIELDS,
		PRODUCTION_MAX_TURF_ADJACENCY_MUTATIONS,
		"turf adjacency batch",
	)?;
	let mutations = values
		.as_chunks::<PRODUCTION_TURF_ADJACENCY_FIELDS>()
		.0
		.iter()
		.enumerate()
		.map(|(index, entry)| {
			Ok(TurfAdjacencyMutation {
				left: WireHandle {
					slot: exact_u32(entry[0], &format!("turf adjacency entry {index} left slot"))?,
					generation: exact_u32(
						entry[1],
						&format!("turf adjacency entry {index} left generation"),
					)?,
				},
				right: WireHandle {
					slot: exact_u32(
						entry[2],
						&format!("turf adjacency entry {index} right slot"),
					)?,
					generation: exact_u32(
						entry[3],
						&format!("turf adjacency entry {index} right generation"),
					)?,
				},
				connected: exact_bool(
					entry[4],
					&format!("turf adjacency entry {index} connected flag"),
				)?,
				firelock: exact_bool(
					entry[5],
					&format!("turf adjacency entry {index} firelock flag"),
				)?,
			})
		})
		.collect::<eyre::Result<Vec<_>>>()?;
	let mut output = Vec::new();
	encode_turf_adjacency_batch(&mutations, &mut output)?;
	Ok(output)
}

#[auxmacros::bind("/proc/dogmos_turf_heat_batch")]
fn dogmos_turf_heat_batch(entries: ByondValue) -> eyre::Result<ByondValue> {
	let values = bounded_number_list(
		entries,
		"turf heat batch",
		PRODUCTION_MAX_TURF_HEAT_MUTATIONS * PRODUCTION_TURF_HEAT_FIELDS,
	)?;
	let request = encode_production_turf_heat_batch(&values)?;
	production_counted_request(OperationKind::TurfHeatBatch, &request, "turf heat")
}

#[doc(hidden)]
pub fn encode_production_turf_heat_batch(values: &[f32]) -> eyre::Result<Vec<u8>> {
	validate_fixed_records(
		values,
		PRODUCTION_TURF_HEAT_FIELDS,
		PRODUCTION_MAX_TURF_HEAT_MUTATIONS,
		"turf heat batch",
	)?;
	let mutations = values
		.as_chunks::<PRODUCTION_TURF_HEAT_FIELDS>()
		.0
		.iter()
		.enumerate()
		.map(|(index, entry)| {
			let state_present = exact_bool(
				entry[2],
				&format!("turf heat entry {index} state-present flag"),
			)?;
			let adjacent_to_space = exact_bool(
				entry[6],
				&format!("turf heat entry {index} adjacent-to-space flag"),
			)?;
			if !state_present
				&& (entry[3] != 0.0 || entry[4] != 0.0 || entry[5] != 0.0 || adjacent_to_space)
			{
				return Err(eyre::eyre!(
					"turf heat entry {index} has state fields while the present flag is false"
				));
			}
			Ok(TurfHeatMutation {
				turf: WireHandle {
					slot: exact_u32(entry[0], &format!("turf heat entry {index} slot"))?,
					generation: exact_u32(
						entry[1],
						&format!("turf heat entry {index} generation"),
					)?,
				},
				state: state_present.then_some(TurfHeatState {
					temperature: ScalarValue(f64::from(entry[3])),
					thermal_conductivity: ScalarValue(f64::from(entry[4])),
					heat_capacity: ScalarValue(f64::from(entry[5])),
					adjacent_to_space,
				}),
			})
		})
		.collect::<eyre::Result<Vec<_>>>()?;
	let mut output = Vec::new();
	encode_turf_heat_batch(&mutations, &mut output)?;
	Ok(output)
}

#[auxmacros::bind("/proc/dogmos_turf_heat_snapshot")]
fn dogmos_turf_heat_snapshot(fields: ByondValue) -> eyre::Result<ByondValue> {
	let fields = bounded_number_list(fields, "turf heat snapshot", 2)?;
	if fields.len() != 2 {
		return Err(eyre::eyre!(
			"turf heat snapshot requires exactly slot and generation"
		));
	}
	let request = TurfHeatSnapshotRequest {
		turf: WireHandle {
			slot: exact_u32(fields[0], "turf heat snapshot slot")?,
			generation: exact_u32(fields[1], "turf heat snapshot generation")?,
		},
	}
	.encode();
	let response = production_request(
		OperationKind::TurfHeatSnapshot,
		&request,
		TURF_HEAT_SNAPSHOT_LEN,
	)?;
	let fields = decode_production_turf_heat_snapshot(&response)?;
	let mut output = ByondValue::new_list()?;
	for field in fields {
		output.push_list(field.into())?;
	}
	Ok(output)
}

#[doc(hidden)]
pub fn decode_production_turf_heat_snapshot(response: &[u8]) -> eyre::Result<[f32; 5]> {
	let snapshot = TurfHeatSnapshot::decode(response)?;
	let Some(state) = snapshot.state else {
		return Ok([0.0; 5]);
	};
	Ok([
		1.0,
		finite_byond_scalar(state.temperature.0, "turf heat snapshot temperature")?,
		finite_byond_scalar(
			state.thermal_conductivity.0,
			"turf heat snapshot thermal conductivity",
		)?,
		finite_byond_scalar(state.heat_capacity.0, "turf heat snapshot heat capacity")?,
		f32::from(state.adjacent_to_space),
	])
}

#[auxmacros::bind("/proc/dogmos_turf_heat_adjacency_batch")]
fn dogmos_turf_heat_adjacency_batch(entries: ByondValue) -> eyre::Result<ByondValue> {
	let values = bounded_number_list(
		entries,
		"turf heat adjacency batch",
		PRODUCTION_MAX_TURF_HEAT_ADJACENCY_MUTATIONS * PRODUCTION_TURF_HEAT_ADJACENCY_FIELDS,
	)?;
	let request = encode_production_turf_heat_adjacency_batch(&values)?;
	production_counted_request(
		OperationKind::TurfHeatAdjacencyBatch,
		&request,
		"turf heat adjacency",
	)
}

#[doc(hidden)]
pub fn encode_production_turf_heat_adjacency_batch(values: &[f32]) -> eyre::Result<Vec<u8>> {
	validate_fixed_records(
		values,
		PRODUCTION_TURF_HEAT_ADJACENCY_FIELDS,
		PRODUCTION_MAX_TURF_HEAT_ADJACENCY_MUTATIONS,
		"turf heat adjacency batch",
	)?;
	let mutations = values
		.as_chunks::<PRODUCTION_TURF_HEAT_ADJACENCY_FIELDS>()
		.0
		.iter()
		.enumerate()
		.map(|(index, entry)| {
			Ok(TurfHeatAdjacencyMutation {
				left: WireHandle {
					slot: exact_u32(
						entry[0],
						&format!("turf heat adjacency entry {index} left slot"),
					)?,
					generation: exact_u32(
						entry[1],
						&format!("turf heat adjacency entry {index} left generation"),
					)?,
				},
				right: WireHandle {
					slot: exact_u32(
						entry[2],
						&format!("turf heat adjacency entry {index} right slot"),
					)?,
					generation: exact_u32(
						entry[3],
						&format!("turf heat adjacency entry {index} right generation"),
					)?,
				},
				connected: exact_bool(
					entry[4],
					&format!("turf heat adjacency entry {index} connected flag"),
				)?,
			})
		})
		.collect::<eyre::Result<Vec<_>>>()?;
	let mut output = Vec::new();
	encode_turf_heat_adjacency_batch(&mutations, &mut output)?;
	Ok(output)
}

#[auxmacros::bind("/proc/dogmos_frontier_begin")]
fn dogmos_frontier_begin(fields: ByondValue) -> eyre::Result<ByondValue> {
	let fields = bounded_number_list(fields, "frontier begin", 6)?;
	let request = encode_production_frontier_begin(&fields)?;
	let response = production_request(OperationKind::FrontierBegin, &request, 8)?;
	let epoch = FrontierBeginResponse::decode(&response)?.epoch;
	let mut output = ByondValue::new_list()?;
	for word in split_u64_words(epoch) {
		output.push_list(f32::from(word).into())?;
	}
	Ok(output)
}

#[doc(hidden)]
pub fn encode_production_frontier_begin(fields: &[f32]) -> eyre::Result<[u8; 16]> {
	if fields.len() != 6 {
		return Err(eyre::eyre!(
			"frontier begin requires four epoch words and two count words"
		));
	}
	Ok(FrontierBeginRequest {
		epoch: join_u64_words(exact_words4(&fields[..4], "frontier epoch")?),
		expected_count: join_u32_words(
			exact_u16(fields[4], "frontier count word 0")?,
			exact_u16(fields[5], "frontier count word 1")?,
		),
	}
	.encode())
}

#[auxmacros::bind("/proc/dogmos_frontier_append")]
fn dogmos_frontier_append(records: ByondValue) -> eyre::Result<ByondValue> {
	let records = bounded_number_list(
		records,
		"frontier append",
		6 + MAX_FRONTIER_APPEND_HANDLES * 4,
	)?;
	let request = encode_production_frontier_append(&records)?;
	let response = production_request(OperationKind::FrontierAppend, &request, 4)?;
	let accepted = FrontierAppendResponse::decode(&response)?.accepted_count;
	let mut output = ByondValue::new_list()?;
	for word in split_u32_words(accepted) {
		output.push_list(f32::from(word).into())?;
	}
	Ok(output)
}

#[doc(hidden)]
pub fn encode_production_frontier_append(fields: &[f32]) -> eyre::Result<Vec<u8>> {
	if fields.len() < 10 || !(fields.len() - 6).is_multiple_of(4) {
		return Err(eyre::eyre!(
			"frontier append requires epoch, offset, and at least one fixed four-word handle"
		));
	}
	let handle_count = (fields.len() - 6) / 4;
	if handle_count > MAX_FRONTIER_APPEND_HANDLES {
		return Err(eyre::eyre!(
			"frontier append contains {handle_count} handles, maximum {MAX_FRONTIER_APPEND_HANDLES}"
		));
	}
	let handles = fields[6..]
		.as_chunks::<4>()
		.0
		.iter()
		.enumerate()
		.map(|(index, words)| {
			Ok(WireHandle {
				slot: join_u32_words(
					exact_u16(words[0], &format!("frontier handle {index} slot word 0"))?,
					exact_u16(words[1], &format!("frontier handle {index} slot word 1"))?,
				),
				generation: join_u32_words(
					exact_u16(
						words[2],
						&format!("frontier handle {index} generation word 0"),
					)?,
					exact_u16(
						words[3],
						&format!("frontier handle {index} generation word 1"),
					)?,
				),
			})
		})
		.collect::<eyre::Result<Vec<_>>>()?;
	Ok(FrontierAppendRequest {
		epoch: join_u64_words(exact_words4(&fields[..4], "frontier epoch")?),
		offset: join_u32_words(
			exact_u16(fields[4], "frontier offset word 0")?,
			exact_u16(fields[5], "frontier offset word 1")?,
		),
		handles,
	}
	.encode()?)
}

#[auxmacros::bind("/proc/dogmos_frontier_add")]
fn dogmos_frontier_add(records: ByondValue) -> eyre::Result<ByondValue> {
	let records =
		bounded_number_list(records, "frontier add", 4 + MAX_FRONTIER_APPEND_HANDLES * 4)?;
	let request = encode_production_frontier_mutate(&records, "frontier add")?;
	let response = production_request(OperationKind::FrontierAdd, &request, 4)?;
	let count = FrontierMutateResponse::decode(&response)?.count;
	let mut output = ByondValue::new_list()?;
	for word in split_u32_words(count) {
		output.push_list(f32::from(word).into())?;
	}
	Ok(output)
}

#[auxmacros::bind("/proc/dogmos_frontier_remove")]
fn dogmos_frontier_remove(records: ByondValue) -> eyre::Result<ByondValue> {
	let records = bounded_number_list(
		records,
		"frontier remove",
		4 + MAX_FRONTIER_APPEND_HANDLES * 4,
	)?;
	let request = encode_production_frontier_mutate(&records, "frontier remove")?;
	let response = production_request(OperationKind::FrontierRemove, &request, 4)?;
	let count = FrontierMutateResponse::decode(&response)?.count;
	let mut output = ByondValue::new_list()?;
	for word in split_u32_words(count) {
		output.push_list(f32::from(word).into())?;
	}
	Ok(output)
}

#[doc(hidden)]
pub fn encode_production_frontier_mutate(fields: &[f32], label: &str) -> eyre::Result<Vec<u8>> {
	if fields.len() < 8 || !(fields.len() - 4).is_multiple_of(4) {
		return Err(eyre::eyre!(
			"{label} requires epoch and at least one fixed four-word handle"
		));
	}
	let handle_count = (fields.len() - 4) / 4;
	if handle_count > MAX_FRONTIER_APPEND_HANDLES {
		return Err(eyre::eyre!(
			"{label} contains {handle_count} handles, maximum {MAX_FRONTIER_APPEND_HANDLES}"
		));
	}
	let handles = fields[4..]
		.as_chunks::<4>()
		.0
		.iter()
		.enumerate()
		.map(|(index, words)| {
			Ok(WireHandle {
				slot: join_u32_words(
					exact_u16(words[0], &format!("{label} handle {index} slot word 0"))?,
					exact_u16(words[1], &format!("{label} handle {index} slot word 1"))?,
				),
				generation: join_u32_words(
					exact_u16(
						words[2],
						&format!("{label} handle {index} generation word 0"),
					)?,
					exact_u16(
						words[3],
						&format!("{label} handle {index} generation word 1"),
					)?,
				),
			})
		})
		.collect::<eyre::Result<Vec<_>>>()?;
	Ok(FrontierMutateRequest {
		epoch: join_u64_words(exact_words4(&fields[..4], "frontier epoch")?),
		handles,
	}
	.encode()?)
}

#[auxmacros::bind("/proc/dogmos_frontier_commit")]
fn dogmos_frontier_commit(fields: ByondValue) -> eyre::Result<ByondValue> {
	let fields = bounded_number_list(fields, "frontier commit", 4)?;
	if fields.len() != 4 {
		return Err(eyre::eyre!("frontier commit requires four epoch words"));
	}
	let request = FrontierCommitRequest {
		epoch: join_u64_words(exact_words4(&fields, "frontier epoch")?),
	}
	.encode();
	let response = production_request(OperationKind::FrontierCommit, &request, 16)?;
	let response = FrontierCommitResponse::decode(&response)?;
	let mut output_fields = Vec::with_capacity(6);
	append_u64_words(&mut output_fields, response.epoch);
	append_u32_words(&mut output_fields, response.count);
	let mut output = ByondValue::new_list()?;
	for field in output_fields {
		output.push_list(field.into())?;
	}
	Ok(output)
}

#[auxmacros::bind("/proc/dogmos_simulation_stage")]
fn dogmos_simulation_stage(fields: ByondValue) -> eyre::Result<ByondValue> {
	let fields = bounded_number_list(fields, "simulation stage", 12)?;
	if fields.len() != 12 {
		return Err(eyre::eyre!(
			"simulation stage requires stage, frontier epoch, stage epoch, work limit, and seconds-per-tick"
		));
	}
	let request = encode_production_simulation_stage(fields.try_into().unwrap())?;
	let response = production_request(
		OperationKind::SimulationStage,
		&request,
		SIMULATION_STAGE_RESPONSE_LEN,
	)?;
	let fields = decode_production_simulation_stage(&response)?;
	let mut output = ByondValue::new_list()?;
	for field in fields {
		output.push_list(field.into())?;
	}
	Ok(output)
}

#[doc(hidden)]
pub fn encode_production_simulation_stage(
	fields: [f32; 12],
) -> eyre::Result<[u8; dogmos_protocol::SIMULATION_STAGE_REQUEST_LEN]> {
	Ok(SimulationStageRequest {
		stage: SimulationStage::try_from(exact_u32(fields[0], "simulation stage")?)?,
		frontier_epoch: join_u64_words(exact_words4(&fields[1..5], "frontier epoch")?),
		stage_epoch: join_u64_words(exact_words4(&fields[5..9], "stage epoch")?),
		work_limit: join_u32_words(
			exact_u16(fields[9], "stage work-limit word 0")?,
			exact_u16(fields[10], "stage work-limit word 1")?,
		),
		seconds_per_tick: ScalarValue(f64::from(fields[11])),
	}
	.encode()?)
}

#[doc(hidden)]
pub fn decode_production_simulation_stage(response: &[u8]) -> eyre::Result<[f32; 13]> {
	let response = SimulationStageResponse::decode(response)?;
	let mut fields = Vec::with_capacity(13);
	append_u32_words(&mut fields, response.work_items);
	append_u32_words(&mut fields, response.callback_events);
	fields.push(f32::from(response.pending));
	append_u32_words(&mut fields, response.remaining_estimate);
	append_u32_words(&mut fields, response.produced_equalize_seeds);
	append_u32_words(&mut fields, response.produced_group_seeds);
	append_u32_words(&mut fields, response.produced_heat_seeds);
	Ok(fields
		.try_into()
		.expect("stage response has thirteen fields"))
}

fn production_counted_request(
	operation: OperationKind,
	payload: &[u8],
	label: &str,
) -> eyre::Result<ByondValue> {
	let response = production_request(operation, payload, 4)?;
	let response: [u8; 4] = response.try_into().map_err(|response: Vec<u8>| {
		eyre::eyre!(
			"Dogmos {label} response was {} bytes, expected 4",
			response.len()
		)
	})?;
	Ok((u32::from_le_bytes(response) as f32).into())
}

fn production_request(
	operation: OperationKind,
	payload: &[u8],
	response_capacity: usize,
) -> eyre::Result<Vec<u8>> {
	let mut session = SERVICE_SESSION
		.lock()
		.map_err(|_| eyre::eyre!("Dogmos production service session lock is poisoned"))?;
	let session = session
		.as_mut()
		.ok_or_else(|| eyre::eyre!("Dogmos production service session is not running"))?;
	Ok(session.request(operation, payload, response_capacity)?)
}

#[cfg(feature = "diagnostic-bindings")]
#[auxmacros::bind("/proc/dogmos_ipc_benchmark_start")]
fn dogmos_ipc_benchmark_start(service_path: ByondValue) -> eyre::Result<ByondValue> {
	let service_path = service_path.get_string()?;
	let mut session = BENCHMARK_SESSION
		.lock()
		.map_err(|_| eyre::eyre!("Dogmos IPC benchmark session lock is poisoned"))?;
	if session.is_some() {
		return Err(eyre::eyre!(
			"Dogmos IPC benchmark session is already running"
		));
	}
	*session = Some(start_service_session(&service_path)?);
	Ok(true.into())
}

#[cfg(feature = "diagnostic-bindings")]
#[auxmacros::bind("/proc/dogmos_ipc_benchmark_scalar_get")]
fn dogmos_ipc_benchmark_scalar_get() -> eyre::Result<ByondValue> {
	let mut session = BENCHMARK_SESSION
		.lock()
		.map_err(|_| eyre::eyre!("Dogmos IPC benchmark session lock is poisoned"))?;
	let session = session
		.as_mut()
		.ok_or_else(|| eyre::eyre!("Dogmos IPC benchmark session is not running"))?;
	let response = session.request(OperationKind::ScalarGet, &[0; 8], 8)?;
	Ok(scalar_response_value(&response, response.len())?.into())
}

#[cfg(feature = "diagnostic-bindings")]
#[auxmacros::bind("/proc/dogmos_ipc_benchmark_snapshot")]
fn dogmos_ipc_benchmark_snapshot() -> eyre::Result<ByondValue> {
	let mut session = BENCHMARK_SESSION
		.lock()
		.map_err(|_| eyre::eyre!("Dogmos IPC benchmark session lock is poisoned"))?;
	let session = session
		.as_mut()
		.ok_or_else(|| eyre::eyre!("Dogmos IPC benchmark session is not running"))?;
	let request = MixtureSnapshotRequest {
		handle: WireHandle {
			slot: 1,
			generation: 1,
		},
	}
	.encode();
	let response = session.request(
		OperationKind::MixtureSnapshot,
		&request,
		MIXTURE_SNAPSHOT_LEN,
	)?;
	if response.len() != MIXTURE_SNAPSHOT_LEN {
		return Err(eyre::eyre!(
			"Dogmos snapshot response was {} bytes, expected {MIXTURE_SNAPSHOT_LEN}",
			response.len(),
		));
	}
	Ok((MixtureSnapshot::decode(&response)?.gas_count as f32).into())
}

#[cfg(feature = "diagnostic-bindings")]
#[auxmacros::bind("/proc/dogmos_ipc_benchmark_lifecycle_batch")]
fn dogmos_ipc_benchmark_lifecycle_batch() -> eyre::Result<ByondValue> {
	let request = BENCHMARK_LIFECYCLE_BATCH.get_or_init(make_benchmark_lifecycle_batch);
	benchmark_counted_command(OperationKind::MixtureLifecycleBatch, request)
}

#[cfg(feature = "diagnostic-bindings")]
#[auxmacros::bind("/proc/dogmos_ipc_benchmark_state_batch")]
fn dogmos_ipc_benchmark_state_batch() -> eyre::Result<ByondValue> {
	let request = BENCHMARK_STATE_BATCH.get_or_init(make_benchmark_state_batch);
	benchmark_counted_command(OperationKind::MixtureStateBatch, request)
}

#[cfg(feature = "diagnostic-bindings")]
#[auxmacros::bind("/proc/dogmos_ipc_benchmark_adjacency_batch")]
fn dogmos_ipc_benchmark_adjacency_batch() -> eyre::Result<ByondValue> {
	let request = BENCHMARK_ADJACENCY_BATCH.get_or_init(make_benchmark_adjacency_batch);
	benchmark_counted_command(OperationKind::AdjacencyBatch, request)
}

#[cfg(feature = "diagnostic-bindings")]
#[auxmacros::bind("/proc/dogmos_ipc_benchmark_simulation_stage")]
fn dogmos_ipc_benchmark_simulation_stage() -> eyre::Result<ByondValue> {
	let request = SimulationStageRequest {
		stage: SimulationStage::ProcessTurfs,
		frontier_epoch: 1,
		stage_epoch: 1,
		work_limit: 1,
		seconds_per_tick: ScalarValue(0.5),
	}
	.encode()?;
	let mut session = BENCHMARK_SESSION
		.lock()
		.map_err(|_| eyre::eyre!("Dogmos IPC benchmark session lock is poisoned"))?;
	let session = session
		.as_mut()
		.ok_or_else(|| eyre::eyre!("Dogmos IPC benchmark session is not running"))?;
	let response = session.request(
		OperationKind::SimulationStage,
		&request,
		SIMULATION_STAGE_RESPONSE_LEN,
	)?;
	if response.len() != SIMULATION_STAGE_RESPONSE_LEN {
		return Err(eyre::eyre!(
			"Dogmos stage response was {} bytes, expected {SIMULATION_STAGE_RESPONSE_LEN}",
			response.len(),
		));
	}
	Ok((SimulationStageResponse::decode(&response)?.work_items as f32).into())
}

#[cfg(feature = "diagnostic-bindings")]
#[auxmacros::bind("/proc/dogmos_ipc_benchmark_callback_enqueue")]
fn dogmos_ipc_benchmark_callback_enqueue(count: ByondValue) -> eyre::Result<ByondValue> {
	let request = CallbackBatchRequest {
		max_events: callback_count_from_number(count.get_number()?)?,
		scope: CallbackScope::General,
		transaction_id: 0,
	}
	.encode()?;
	let mut session = BENCHMARK_SESSION
		.lock()
		.map_err(|_| eyre::eyre!("Dogmos IPC benchmark session lock is poisoned"))?;
	let session = session
		.as_mut()
		.ok_or_else(|| eyre::eyre!("Dogmos IPC benchmark session is not running"))?;
	match session.request(OperationKind::DiagnosticCallbackEnqueue, &request, 4) {
		Ok(response) if response.len() == 4 => {
			Ok((u32::from_le_bytes(response.try_into().unwrap()) as f32).into())
		}
		Ok(response) => Err(eyre::eyre!(
			"Dogmos callback enqueue response was {} bytes, expected 4",
			response.len(),
		)),
		Err(ClientError::Server(ServiceErrorCode::CallbackBackpressure)) => Ok((-1.0_f32).into()),
		Err(error) => Err(error.into()),
	}
}

#[cfg(feature = "diagnostic-bindings")]
#[auxmacros::bind("/proc/dogmos_ipc_benchmark_callback_drain")]
fn dogmos_ipc_benchmark_callback_drain(max_events: ByondValue) -> eyre::Result<ByondValue> {
	let request = CallbackBatchRequest {
		max_events: callback_count_from_number(max_events.get_number()?)?,
		scope: CallbackScope::General,
		transaction_id: 0,
	}
	.encode()?;
	let mut session = BENCHMARK_SESSION
		.lock()
		.map_err(|_| eyre::eyre!("Dogmos IPC benchmark session lock is poisoned"))?;
	let session = session
		.as_mut()
		.ok_or_else(|| eyre::eyre!("Dogmos IPC benchmark session is not running"))?;
	let response = session.request(
		OperationKind::CallbackBatch,
		&request,
		BENCHMARK_CONTROL_PAYLOAD,
	)?;
	let response_len = response.len();
	if response_len < CALLBACK_BATCH_HEADER_LEN {
		return Err(eyre::eyre!(
			"Dogmos callback drain response was {response_len} bytes, shorter than its header"
		));
	}
	let header = CallbackBatchHeader::decode(&response[..CALLBACK_BATCH_HEADER_LEN])?;
	let expected_len = CALLBACK_BATCH_HEADER_LEN
		+ usize::try_from(header.returned)?
			.checked_mul(CALLBACK_EVENT_LEN)
			.ok_or_else(|| eyre::eyre!("Dogmos callback response length overflow"))?;
	if response_len != expected_len {
		return Err(eyre::eyre!(
			"Dogmos callback drain response was {response_len} bytes, expected {expected_len}"
		));
	}
	let mut first_sequence = 0;
	let mut last_sequence: u64 = 0;
	for (index, event_bytes) in response[CALLBACK_BATCH_HEADER_LEN..response_len]
		.as_chunks::<CALLBACK_EVENT_LEN>()
		.0
		.iter()
		.enumerate()
	{
		let event = CallbackEvent::decode(event_bytes)?;
		if index == 0 {
			first_sequence = event.sequence;
		} else {
			let expected = last_sequence.checked_add(1).ok_or_else(|| {
				eyre::eyre!("Dogmos callback sequence overflowed after {last_sequence}")
			})?;
			if event.sequence != expected {
				return Err(eyre::eyre!(
					"Dogmos callback sequence skipped from {last_sequence} to {}",
					event.sequence
				));
			}
		}
		last_sequence = event.sequence;
	}
	let summary: ByondValue = format!(
		"{},{},{},{},{},{},{}",
		header.returned,
		header.remaining,
		header.capacity,
		header.high_water,
		header.rejected,
		first_sequence,
		last_sequence
	)
	.try_into()?;
	Ok(summary)
}

#[cfg(feature = "diagnostic-bindings")]
fn benchmark_counted_command(operation: OperationKind, request: &[u8]) -> eyre::Result<ByondValue> {
	let mut session = BENCHMARK_SESSION
		.lock()
		.map_err(|_| eyre::eyre!("Dogmos IPC benchmark session lock is poisoned"))?;
	let session = session
		.as_mut()
		.ok_or_else(|| eyre::eyre!("Dogmos IPC benchmark session is not running"))?;
	let response = session.request(operation, request, 4)?;
	if response.len() != 4 {
		return Err(eyre::eyre!(
			"Dogmos counted response was {} bytes, expected 4",
			response.len(),
		));
	}
	Ok((u32::from_le_bytes(response.try_into().unwrap()) as f32).into())
}

#[cfg(feature = "diagnostic-bindings")]
fn make_benchmark_lifecycle_batch() -> Vec<u8> {
	let entries = (0..64)
		.map(|slot| LifecycleMutation {
			action: LifecycleAction::Register,
			handle: WireHandle {
				slot,
				generation: 1,
			},
		})
		.collect::<Vec<_>>();
	let mut output = Vec::new();
	encode_lifecycle_batch(&entries, &mut output)
		.expect("the fixed benchmark lifecycle batch is valid");
	output
}

#[cfg(feature = "diagnostic-bindings")]
fn make_benchmark_state_batch() -> Vec<u8> {
	let entries = (0..64)
		.map(|slot| {
			let mut gases = [ScalarValue(0.0); MAX_GAS_SLOTS];
			gases[0] = ScalarValue(if slot % 2 == 0 { 20.0 } else { 5.0 });
			MixtureStateMutation {
				handle: WireHandle {
					slot,
					generation: 1,
				},
				expected_revision: 0,
				temperature: ScalarValue(293.15),
				volume: ScalarValue(2500.0),
				gases,
			}
		})
		.collect::<Vec<_>>();
	let mut output = Vec::new();
	encode_mixture_state_batch(&entries, &mut output)
		.expect("the fixed benchmark mixture-state batch is valid");
	output
}

#[cfg(feature = "diagnostic-bindings")]
fn make_benchmark_adjacency_batch() -> Vec<u8> {
	let entries = (0..64)
		.map(|slot| AdjacencyMutation {
			left: WireHandle {
				slot,
				generation: 1,
			},
			right: WireHandle {
				slot: (slot + 1) % 64,
				generation: 1,
			},
			conductivity: ScalarValue(0.75),
		})
		.collect::<Vec<_>>();
	let mut output = Vec::new();
	encode_adjacency_batch(&entries, &mut output)
		.expect("the fixed benchmark adjacency batch is valid");
	output
}

#[cfg(feature = "diagnostic-bindings")]
#[auxmacros::bind("/proc/dogmos_ipc_benchmark_service_pid")]
fn dogmos_ipc_benchmark_service_pid() -> eyre::Result<ByondValue> {
	let session = BENCHMARK_SESSION
		.lock()
		.map_err(|_| eyre::eyre!("Dogmos IPC benchmark session lock is poisoned"))?;
	let session = session
		.as_ref()
		.ok_or_else(|| eyre::eyre!("Dogmos IPC benchmark session is not running"))?;
	Ok((session.client.peer().process_id as f32).into())
}

#[cfg(feature = "diagnostic-bindings")]
#[auxmacros::bind("/proc/dogmos_ipc_benchmark_clock_microseconds")]
fn dogmos_ipc_benchmark_clock_microseconds() -> eyre::Result<ByondValue> {
	let origin = BENCHMARK_CLOCK.get_or_init(Instant::now);
	Ok((origin.elapsed().as_secs_f32() * 1_000_000.0).into())
}

#[cfg(feature = "diagnostic-bindings")]
#[auxmacros::bind("/proc/dogmos_ipc_benchmark_allocate")]
fn dogmos_ipc_benchmark_allocate(bytes: ByondValue) -> eyre::Result<ByondValue> {
	let bytes = diagnostic_bytes_from_number(bytes.get_number()?)?;
	let mut session = BENCHMARK_SESSION
		.lock()
		.map_err(|_| eyre::eyre!("Dogmos IPC benchmark session lock is poisoned"))?;
	let session = session
		.as_mut()
		.ok_or_else(|| eyre::eyre!("Dogmos IPC benchmark session is not running"))?;
	let response = session.request(OperationKind::AllocateDiagnostic, &bytes.to_le_bytes(), 8)?;
	let response: [u8; 8] = response.try_into().map_err(|response: Vec<u8>| {
		eyre::eyre!(
			"Dogmos allocation response was {} bytes, expected 8",
			response.len(),
		)
	})?;
	Ok((u64::from_le_bytes(response) as f32).into())
}

#[cfg(feature = "diagnostic-bindings")]
#[auxmacros::bind("/proc/dogmos_ipc_benchmark_stop")]
fn dogmos_ipc_benchmark_stop() -> eyre::Result<ByondValue> {
	let mut session = BENCHMARK_SESSION
		.lock()
		.map_err(|_| eyre::eyre!("Dogmos IPC benchmark session lock is poisoned"))?;
	let mut active = session
		.take()
		.ok_or_else(|| eyre::eyre!("Dogmos IPC benchmark session is not running"))?;
	active.request(OperationKind::AllocateDiagnostic, &0_u64.to_le_bytes(), 8)?;
	active.shutdown()?;
	Ok(true.into())
}

#[cfg(any(feature = "diagnostic-bindings", test))]
fn diagnostic_bytes_from_number(bytes: f32) -> eyre::Result<u64> {
	if !bytes.is_finite() || bytes < 0.0 || bytes > 8.0 * 1024.0 * 1024.0 * 1024.0 {
		return Err(eyre::eyre!(
			"diagnostic bytes are outside the supported range"
		));
	}
	Ok(bytes as u64)
}

#[cfg(any(feature = "diagnostic-bindings", test))]
fn callback_count_from_number(count: f32) -> eyre::Result<u32> {
	if !count.is_finite()
		|| count < 0.0
		|| count > BENCHMARK_CALLBACK_CAPACITY as f32
		|| count.fract() != 0.0
	{
		return Err(eyre::eyre!(
			"callback count is outside the supported integer range"
		));
	}
	Ok(count as u32)
}

#[derive(Clone, Copy, Debug)]
struct DmMixtureCommandFields {
	kind: u16,
	flags: u16,
	primary: WireHandle,
	secondary: WireHandle,
	scalars: [f32; 3],
	gas_id: u16,
	aux: u32,
}

fn exact_u32(number: f32, field: &str) -> eyre::Result<u32> {
	if !number.is_finite()
		|| number < 0.0
		|| number > MAX_EXACT_BYOND_INTEGER
		|| number.fract() != 0.0
	{
		return Err(eyre::eyre!(
			"{field} must be an exact non-negative BYOND integer"
		));
	}
	Ok(number as u32)
}

fn exact_u16(number: f32, field: &str) -> eyre::Result<u16> {
	let number = exact_u32(number, field)?;
	u16::try_from(number).map_err(|_| eyre::eyre!("{field} exceeds the u16 wire range"))
}

fn exact_bool(number: f32, field: &str) -> eyre::Result<bool> {
	match exact_u32(number, field)? {
		0 => Ok(false),
		1 => Ok(true),
		actual => Err(eyre::eyre!("{field} must be 0 or 1, got {actual}")),
	}
}

fn validate_fixed_records(
	values: &[f32],
	record_fields: usize,
	maximum_records: usize,
	label: &str,
) -> eyre::Result<()> {
	if !values.len().is_multiple_of(record_fields) {
		return Err(eyre::eyre!(
			"{label} requires fixed {record_fields}-field records"
		));
	}
	let record_count = values.len() / record_fields;
	if record_count > maximum_records {
		return Err(eyre::eyre!(
			"{label} contains {record_count} operations, maximum {maximum_records}"
		));
	}
	Ok(())
}

fn split_u32_words(value: u32) -> [u16; 2] {
	[value as u16, (value >> 16) as u16]
}

fn join_u32_words(low: u16, high: u16) -> u32 {
	u32::from(low) | (u32::from(high) << 16)
}

fn append_u32_words(output: &mut Vec<f32>, value: u32) {
	output.extend(split_u32_words(value).map(f32::from));
}

fn append_u64_words(output: &mut Vec<f32>, value: u64) {
	output.extend(split_u64_words(value).map(f32::from));
}

fn split_u64_words(value: u64) -> [u16; 4] {
	[
		value as u16,
		(value >> 16) as u16,
		(value >> 32) as u16,
		(value >> 48) as u16,
	]
}

fn join_u64_words(words: [u16; 4]) -> u64 {
	u64::from(words[0])
		| (u64::from(words[1]) << 16)
		| (u64::from(words[2]) << 32)
		| (u64::from(words[3]) << 48)
}

fn exact_words4(words: &[f32], field: &str) -> eyre::Result<[u16; 4]> {
	if words.len() != 4 {
		return Err(eyre::eyre!("{field} requires four 16-bit words"));
	}
	Ok([
		exact_u16(words[0], &format!("{field} word 0"))?,
		exact_u16(words[1], &format!("{field} word 1"))?,
		exact_u16(words[2], &format!("{field} word 2"))?,
		exact_u16(words[3], &format!("{field} word 3"))?,
	])
}

fn append_continuation_token_fields(output: &mut Vec<f32>, token: Option<ContinuationToken>) {
	let token = token.unwrap_or(ContinuationToken {
		world_generation: 0,
		id: 0,
		deadline_ticks: 0,
	});
	append_u32_words(output, token.world_generation);
	append_u64_words(output, token.id);
	append_u64_words(output, token.deadline_ticks);
}

fn finite_byond_scalar(value: f64, field: &str) -> eyre::Result<f32> {
	let value = value as f32;
	if !value.is_finite() {
		return Err(eyre::eyre!(
			"{field} is outside the finite BYOND number range"
		));
	}
	Ok(value)
}

/// Inlines exact_u32()'s check instead of calling it with a &format!(...) label: that label was
/// built unconditionally on every call to bounded_number_list()/bounded_string_list() - both on
/// the hot decode path for every DM proc call - even though it's only read in the error branch.
fn checked_declared_length(number: f32, field: &str) -> eyre::Result<usize> {
	if !number.is_finite()
		|| number < 0.0
		|| number > MAX_EXACT_BYOND_INTEGER
		|| number.fract() != 0.0
	{
		return Err(eyre::eyre!(
			"{field} length must be an exact non-negative BYOND integer"
		));
	}
	Ok(number as u32 as usize)
}

fn bounded_number_list(
	value: ByondValue,
	field: &str,
	maximum_values: usize,
) -> eyre::Result<Vec<f32>> {
	if !value.is_list() {
		return Err(eyre::eyre!("{field} must be a BYOND list"));
	}
	let declared_length = checked_declared_length(value.builtin_length()?.get_number()?, field)?;
	if declared_length > maximum_values {
		return Err(eyre::eyre!(
			"{field} contains {declared_length} values, maximum {maximum_values}"
		));
	}
	let values = value
		.values()?
		.map(|entry| entry.get_number().map_err(Into::into))
		.collect::<eyre::Result<Vec<_>>>()?;
	if values.len() != declared_length {
		return Err(eyre::eyre!("{field} changed length while being decoded"));
	}
	Ok(values)
}

fn bounded_string_list(
	value: ByondValue,
	field: &str,
	maximum_values: usize,
) -> eyre::Result<Vec<String>> {
	if !value.is_list() {
		return Err(eyre::eyre!("{field} must be a BYOND list"));
	}
	let declared_length = checked_declared_length(value.builtin_length()?.get_number()?, field)?;
	if declared_length > maximum_values {
		return Err(eyre::eyre!(
			"{field} contains {declared_length} values, maximum {maximum_values}"
		));
	}
	let values = value
		.values()?
		.map(|entry| entry.get_string().map_err(Into::into))
		.collect::<eyre::Result<Vec<_>>>()?;
	if values.len() != declared_length {
		return Err(eyre::eyre!("{field} changed length while being decoded"));
	}
	Ok(values)
}

#[doc(hidden)]
pub fn encode_production_mixture_lifecycle_batch(values: &[f32]) -> eyre::Result<Vec<u8>> {
	if !values.len().is_multiple_of(3) {
		return Err(eyre::eyre!(
			"mixture lifecycle batch requires action, slot, generation triples"
		));
	}
	let operation_count = values.len() / 3;
	if operation_count > PRODUCTION_MAX_BATCH_OPERATIONS {
		return Err(eyre::eyre!(
			"mixture lifecycle batch contains {operation_count} operations, maximum {PRODUCTION_MAX_BATCH_OPERATIONS}"
		));
	}
	let mutations = values
		.as_chunks::<3>()
		.0
		.iter()
		.enumerate()
		.map(|(index, entry)| {
			let action = LifecycleAction::try_from(exact_u32(
				entry[0],
				&format!("mixture lifecycle entry {index} action"),
			)?)?;
			Ok(LifecycleMutation {
				action,
				handle: WireHandle {
					slot: exact_u32(entry[1], &format!("mixture lifecycle entry {index} slot"))?,
					generation: exact_u32(
						entry[2],
						&format!("mixture lifecycle entry {index} generation"),
					)?,
				},
			})
		})
		.collect::<eyre::Result<Vec<_>>>()?;
	let mut output = Vec::new();
	encode_lifecycle_batch(&mutations, &mut output)?;
	Ok(output)
}

fn encode_dm_mixture_command(
	fields: DmMixtureCommandFields,
) -> eyre::Result<[u8; MIXTURE_COMMAND_REQUEST_LEN]> {
	let mut bytes = [0_u8; MIXTURE_COMMAND_REQUEST_LEN];
	bytes[0..2].copy_from_slice(&fields.kind.to_le_bytes());
	bytes[2..4].copy_from_slice(&fields.flags.to_le_bytes());
	bytes[4..12].copy_from_slice(&fields.primary.encode());
	bytes[12..20].copy_from_slice(&fields.secondary.encode());
	for (index, scalar) in fields.scalars.into_iter().enumerate() {
		let offset = 20 + index * 8;
		bytes[offset..offset + 8].copy_from_slice(&ScalarValue(f64::from(scalar)).encode()?);
	}
	bytes[44..46].copy_from_slice(&fields.gas_id.to_le_bytes());
	bytes[48..52].copy_from_slice(&fields.aux.to_le_bytes());
	Ok(MixtureCommandRequest::decode(&bytes)?.encode()?)
}

fn mixture_command_response_value(response: MixtureCommandResponse) -> eyre::Result<ByondValue> {
	let (kind, first, second, third, transaction_id) = match response {
		MixtureCommandResponse::Applied { updated } => (1.0, updated as f32, 0.0, 0.0, None),
		MixtureCommandResponse::Scalar(value) => (2.0, value.0 as f32, 0.0, 0.0, None),
		MixtureCommandResponse::Scalars(values) => {
			(3.0, values[0].0 as f32, values[1].0 as f32, 0.0, None)
		}
		MixtureCommandResponse::Boolean(value) => (4.0, f32::from(value), 0.0, 0.0, None),
		MixtureCommandResponse::ReactionProgress {
			flags,
			work_items,
			pending,
			transaction_id,
		} => (
			5.0,
			flags as f32,
			work_items as f32,
			f32::from(pending),
			Some(transaction_id),
		),
	};
	let mut output = ByondValue::new_list()?;
	output.push_list(kind.into())?;
	output.push_list(first.into())?;
	output.push_list(second.into())?;
	output.push_list(third.into())?;
	if let Some(transaction_id) = transaction_id {
		for word in split_u64_words(transaction_id) {
			output.push_list(f32::from(word).into())?;
		}
	}
	Ok(output)
}

#[cfg(any(feature = "diagnostic-bindings", test))]
fn scalar_response_value(response: &[u8], response_len: usize) -> eyre::Result<f32> {
	if response_len != response.len() {
		return Err(eyre::eyre!(
			"Dogmos scalar response was {response_len} bytes, expected {}",
			response.len()
		));
	}
	Ok(f64::from_le_bytes(response.try_into().unwrap()) as f32)
}

fn hex_lower(bytes: &[u8]) -> String {
	const HEX: &[u8; 16] = b"0123456789abcdef";
	let mut output = String::with_capacity(bytes.len() * 2);
	for byte in bytes {
		output.push(HEX[usize::from(byte >> 4)] as char);
		output.push(HEX[usize::from(byte & 0x0f)] as char);
	}
	output
}

#[cfg(test)]
mod tests {
	use super::{
		callback_count_from_number, decode_production_callback_batch,
		decode_production_continuation_token, decode_production_mixture_snapshot,
		decode_production_service_telemetry, decode_production_simulation_stage,
		decode_production_turf_heat_snapshot, diagnostic_bytes_from_number,
		encode_dm_mixture_command, encode_production_continuation_adjust_multiple,
		encode_production_continuation_command, encode_production_continuation_resume,
		encode_production_frontier_append, encode_production_frontier_begin,
		encode_production_gas_metadata, encode_production_mixture_adjust_multiple,
		encode_production_mixture_lifecycle_batch, encode_production_mixture_state_batch,
		encode_production_process_metrics, encode_production_reaction_metadata,
		encode_production_simulation_stage, encode_production_turf_adjacency_batch,
		encode_production_turf_heat_adjacency_batch, encode_production_turf_heat_batch,
		encode_production_turf_lifecycle_batch, exact_u16, exact_u32, hex_lower,
		normalize_generated_bindings, scalar_response_value, DmMixtureCommandFields,
	};
	use dogmos_process_metrics::{
		CurrentProcessMetrics, PROCESS_ALL_AVAILABLE, PROCESS_PRIVATE_BYTES_AVAILABLE,
	};
	use dogmos_protocol::{
		decode_adjust_multiple_request, decode_continuation_adjust_multiple_request,
		decode_gas_metadata_batch, decode_lifecycle_batch, decode_mixture_state_batch,
		decode_reaction_metadata_batch, decode_turf_adjacency_batch,
		decode_turf_heat_adjacency_batch, decode_turf_heat_batch, decode_turf_lifecycle_batch,
		CallbackBatchHeader, CallbackEvent, CallbackEventKind, CallbackScope,
		ContinuationCommandRequest, ContinuationResumeRequest, ContinuationToken,
		FrontierBeginRequest, GasMetadataRegistration, LifecycleAction, LifecycleMutation,
		MixtureAdjustment, MixtureCommandRequest, MixtureSnapshot, ReactionMetadataRegistration,
		ScalarValue, ServiceTelemetry, SimulationStage, SimulationStageRequest,
		SimulationStageResponse, TurfAdjacencyMutation, TurfHeatAdjacencyMutation,
		TurfHeatMutation, TurfHeatSnapshot, TurfHeatState, TurfLifecycleMutation, WireFireProducts,
		WireGasFireRole, WireGasProduct, WireGasRequirement, WireHandle, WireReactionExecution,
		MAX_GAS_SLOTS, SERVICE_PROCESS_ALL_AVAILABLE,
	};

	fn handle(slot: u32, generation: u32) -> WireHandle {
		WireHandle { slot, generation }
	}

	#[test]
	fn production_mixture_fields_encode_only_canonical_commands() {
		let request = encode_dm_mixture_command(DmMixtureCommandFields {
			kind: 1,
			flags: 0,
			primary: handle(7, 2),
			secondary: handle(0, 0),
			scalars: [12.5, 0.0, 0.0],
			gas_id: 3,
			aux: 0,
		})
		.unwrap();
		assert_eq!(
			MixtureCommandRequest::decode(&request),
			Ok(MixtureCommandRequest::SetMoles {
				handle: handle(7, 2),
				gas_id: 3,
				amount: ScalarValue(12.5),
			})
		);

		let error = encode_dm_mixture_command(DmMixtureCommandFields {
			kind: 5,
			flags: 0,
			primary: handle(7, 2),
			secondary: handle(0, 0),
			scalars: [1.0, 0.0, 0.0],
			gas_id: 0,
			aux: 0,
		});
		assert!(error.is_err(), "unused scalar must fail closed");

		let direct_reaction = encode_dm_mixture_command(DmMixtureCommandFields {
			kind: 36,
			flags: 0,
			primary: handle(7, 2),
			secondary: handle(41, 9),
			scalars: [0.0; 3],
			gas_id: 0,
			aux: 0,
		})
		.unwrap();
		assert_eq!(
			MixtureCommandRequest::decode(&direct_reaction),
			Ok(MixtureCommandRequest::React {
				handle: handle(7, 2),
				target: handle(41, 9),
				reaction_profile_threshold_ms: None,
			})
		);

		let profiled_reaction = encode_dm_mixture_command(DmMixtureCommandFields {
			kind: 36,
			flags: 1,
			primary: handle(7, 2),
			secondary: handle(41, 9),
			scalars: [0.5, 0.0, 0.0],
			gas_id: 0,
			aux: 0,
		})
		.unwrap();
		assert_eq!(
			MixtureCommandRequest::decode(&profiled_reaction),
			Ok(MixtureCommandRequest::React {
				handle: handle(7, 2),
				target: handle(41, 9),
				reaction_profile_threshold_ms: Some(ScalarValue(0.5)),
			})
		);
	}

	#[test]
	fn production_mixture_integer_fields_reject_hostile_numbers() {
		for number in [f32::NAN, f32::INFINITY, -1.0, 1.5, 16_777_218.0] {
			assert!(exact_u32(number, "test field").is_err());
		}
		assert_eq!(exact_u32(16_777_216.0, "test field").unwrap(), 16_777_216);
		assert!(exact_u16(65_536.0, "test field").is_err());
	}

	#[test]
	fn production_lifecycle_batches_are_bounded_exact_triples() {
		let request =
			encode_production_mixture_lifecycle_batch(&[1.0, 7.0, 2.0, 2.0, 7.0, 2.0]).unwrap();
		assert_eq!(
			decode_lifecycle_batch(&request, 2),
			Ok(vec![
				LifecycleMutation {
					action: LifecycleAction::Register,
					handle: handle(7, 2),
				},
				LifecycleMutation {
					action: LifecycleAction::Unregister,
					handle: handle(7, 2),
				},
			])
		);
		assert!(encode_production_mixture_lifecycle_batch(&[1.0, 7.0]).is_err());
		assert!(encode_production_mixture_lifecycle_batch(&[3.0, 7.0, 2.0]).is_err());
		assert!(encode_production_mixture_lifecycle_batch(&[1.0, 7.5, 2.0]).is_err());
	}

	#[test]
	fn production_multi_adjust_is_bounded_and_validated() {
		let request =
			encode_production_mixture_adjust_multiple(&[7.0, 2.0, 1.0, -0.5, 3.0, 2.0]).unwrap();
		assert_eq!(
			decode_adjust_multiple_request(&request),
			Ok((
				handle(7, 2),
				vec![
					MixtureAdjustment {
						gas_id: 1,
						delta: ScalarValue(-0.5),
					},
					MixtureAdjustment {
						gas_id: 3,
						delta: ScalarValue(2.0),
					},
				],
			))
		);
		assert!(encode_production_mixture_adjust_multiple(&[7.0]).is_err());
		assert!(encode_production_mixture_adjust_multiple(&[7.0, 2.0, 1.0]).is_err());
		assert!(encode_production_mixture_adjust_multiple(&[7.0, 2.0, 1.5, 1.0]).is_err());
		assert!(encode_production_mixture_adjust_multiple(&[7.0, 2.0, 1.0, f32::NAN]).is_err());
	}

	#[test]
	fn production_snapshot_preserves_revision_and_fixed_gas_layout() {
		let mut gases = [ScalarValue(0.0); MAX_GAS_SLOTS];
		gases[0] = ScalarValue(1.25);
		gases[31] = ScalarValue(9.5);
		let response = MixtureSnapshot {
			revision: 0xfedc_ba98,
			gas_count: 2,
			temperature: ScalarValue(293.15),
			volume: ScalarValue(2_500.0),
			minimum_heat_capacity: ScalarValue(0.5),
			total_moles: ScalarValue(10.75),
			pressure: ScalarValue(10.5),
			heat_capacity: ScalarValue(215.0),
			immutable: true,
			gases,
		}
		.encode()
		.unwrap();
		let fields = decode_production_mixture_snapshot(&response).unwrap();
		assert_eq!(fields.len(), 10 + MAX_GAS_SLOTS);
		assert_eq!(&fields[..3], &[0xba98 as f32, 0xfedc as f32, 2.0]);
		assert_eq!(
			&fields[3..10],
			&[293.15, 2_500.0, 0.5, 10.75, 10.5, 215.0, 1.0]
		);
		assert_eq!(fields[10], 1.25);
		assert_eq!(fields[10 + 31], 9.5);
	}

	#[test]
	fn production_turf_heat_snapshot_preserves_presence_and_values() {
		let response = TurfHeatSnapshot {
			state: Some(TurfHeatState {
				temperature: ScalarValue(700.0),
				thermal_conductivity: ScalarValue(0.4),
				heat_capacity: ScalarValue(2500.0),
				adjacent_to_space: true,
			}),
		}
		.encode()
		.unwrap();
		assert_eq!(
			decode_production_turf_heat_snapshot(&response).unwrap(),
			[1.0, 700.0, 0.4, 2500.0, 1.0]
		);
		let absent = TurfHeatSnapshot { state: None }.encode().unwrap();
		assert_eq!(
			decode_production_turf_heat_snapshot(&absent).unwrap(),
			[0.0; 5]
		);
	}

	#[test]
	fn production_state_batch_uses_lossless_revision_words() {
		let mut fields = vec![7.0, 2.0, 0xba98 as f32, 0xfedc as f32, 293.15, 2_500.0];
		fields.extend((0..MAX_GAS_SLOTS).map(|index| index as f32 * 0.25));
		let request = encode_production_mixture_state_batch(&fields).unwrap();
		let mutations = decode_mixture_state_batch(&request, 1).unwrap();
		assert_eq!(mutations.len(), 1);
		assert_eq!(mutations[0].handle, handle(7, 2));
		assert_eq!(mutations[0].expected_revision, 0xfedc_ba98);
		assert_eq!(mutations[0].temperature, ScalarValue(293.15_f32.into()));
		assert_eq!(mutations[0].volume, ScalarValue(2_500.0));
		assert_eq!(mutations[0].gases[31], ScalarValue(7.75));

		assert!(encode_production_mixture_state_batch(&fields[..fields.len() - 1]).is_err());
		fields[2] = 65_536.0;
		assert!(encode_production_mixture_state_batch(&fields).is_err());
		fields[2] = 0.0;
		fields[6] = f32::NAN;
		assert!(encode_production_mixture_state_batch(&fields).is_err());
	}

	#[test]
	fn production_turf_lifecycle_and_topology_are_fixed_records() {
		let lifecycle = encode_production_turf_lifecycle_batch(&[
			1.0, 10.0, 1.0, 1.0, 0.0, 1.0, 2.0, 11.0, 1.0, 0.0, 0.0, 0.0,
		])
		.unwrap();
		assert_eq!(
			decode_turf_lifecycle_batch(&lifecycle, 2).unwrap(),
			vec![
				TurfLifecycleMutation {
					action: LifecycleAction::Register,
					turf: handle(10, 1),
					mixture: Some(handle(0, 1)),
				},
				TurfLifecycleMutation {
					action: LifecycleAction::Unregister,
					turf: handle(11, 1),
					mixture: None,
				},
			]
		);
		assert!(encode_production_turf_lifecycle_batch(&[2.0, 11.0, 1.0, 0.0, 4.0, 1.0]).is_err());

		let adjacency =
			encode_production_turf_adjacency_batch(&[10.0, 1.0, 11.0, 1.0, 1.0, 1.0]).unwrap();
		assert_eq!(
			decode_turf_adjacency_batch(&adjacency, 1).unwrap(),
			vec![TurfAdjacencyMutation {
				left: handle(10, 1),
				right: handle(11, 1),
				connected: true,
				firelock: true,
			}]
		);
		assert!(encode_production_turf_adjacency_batch(&[10.0, 1.0, 11.0, 1.0, 0.0, 1.0]).is_err());
	}

	#[test]
	fn production_turf_heat_state_and_topology_are_fixed_records() {
		let heat = encode_production_turf_heat_batch(&[
			10.0, 1.0, 1.0, 700.0, 0.4, 2_500.0, 1.0, 11.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0,
		])
		.unwrap();
		assert_eq!(
			decode_turf_heat_batch(&heat, 2).unwrap(),
			vec![
				TurfHeatMutation {
					turf: handle(10, 1),
					state: Some(TurfHeatState {
						temperature: ScalarValue(700.0),
						thermal_conductivity: ScalarValue(0.4_f32.into()),
						heat_capacity: ScalarValue(2_500.0),
						adjacent_to_space: true,
					}),
				},
				TurfHeatMutation {
					turf: handle(11, 1),
					state: None,
				},
			]
		);
		assert!(encode_production_turf_heat_batch(&[11.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0]).is_err());

		let adjacency =
			encode_production_turf_heat_adjacency_batch(&[10.0, 1.0, 11.0, 1.0, 1.0]).unwrap();
		assert_eq!(
			decode_turf_heat_adjacency_batch(&adjacency, 1).unwrap(),
			vec![TurfHeatAdjacencyMutation {
				left: handle(10, 1),
				right: handle(11, 1),
				connected: true,
			}]
		);
		assert!(encode_production_turf_heat_adjacency_batch(&[10.0, 1.0, 11.0, 1.0, 2.0]).is_err());
	}

	#[test]
	fn production_stage_request_and_response_are_typed_and_lossless() {
		let request = encode_production_simulation_stage([
			4.0,
			0x4444 as f32,
			0x3333 as f32,
			0x2222 as f32,
			0x1111 as f32,
			0x8888 as f32,
			0x7777 as f32,
			0x6666 as f32,
			0x5555 as f32,
			0x0100 as f32,
			0.0,
			0.5,
		])
		.unwrap();
		assert_eq!(
			SimulationStageRequest::decode(&request).unwrap(),
			SimulationStageRequest {
				stage: SimulationStage::ProcessTurfs,
				frontier_epoch: 0x1111_2222_3333_4444,
				stage_epoch: 0x5555_6666_7777_8888,
				work_limit: 256,
				seconds_per_tick: ScalarValue(0.5),
			}
		);
		let response = SimulationStageResponse {
			work_items: 0xfedc_ba98,
			callback_events: 0x7654_3210,
			pending: true,
			remaining_estimate: 0x1111_2222,
			produced_equalize_seeds: 0x3333_4444,
			produced_group_seeds: 0x5555_6666,
			produced_heat_seeds: 0x7777_8888,
		}
		.encode();
		assert_eq!(
			decode_production_simulation_stage(&response).unwrap(),
			[
				0xba98 as f32,
				0xfedc as f32,
				0x3210 as f32,
				0x7654 as f32,
				1.0,
				0x2222 as f32,
				0x1111 as f32,
				0x4444 as f32,
				0x3333 as f32,
				0x6666 as f32,
				0x5555 as f32,
				0x8888 as f32,
				0x7777 as f32,
			]
		);
		let mut invalid = [0.0; 12];
		invalid[0] = 6.0;
		invalid[9] = 1.0;
		invalid[11] = 0.5;
		assert!(encode_production_simulation_stage(invalid).is_err());
		invalid[0] = 4.0;
		invalid[11] = f32::NAN;
		assert!(encode_production_simulation_stage(invalid).is_err());
	}

	#[test]
	fn production_frontier_requests_preserve_all_integer_bits_and_bounds() {
		let begin = encode_production_frontier_begin(&[
			0x4444 as f32,
			0x3333 as f32,
			0x2222 as f32,
			0x1111 as f32,
			0xba98 as f32,
			0xfedc as f32,
		])
		.unwrap();
		assert_eq!(
			FrontierBeginRequest::decode(&begin).unwrap(),
			FrontierBeginRequest {
				epoch: 0x1111_2222_3333_4444,
				expected_count: 0xfedc_ba98,
			}
		);

		let append = encode_production_frontier_append(&[
			0x4444 as f32,
			0x3333 as f32,
			0x2222 as f32,
			0x1111 as f32,
			0x3210 as f32,
			0x7654 as f32,
			0xcdef as f32,
			0x89ab as f32,
			0x4567 as f32,
			0x0123 as f32,
		])
		.unwrap();
		let mut handles = Vec::new();
		let header = dogmos_protocol::decode_frontier_append_into(&append, &mut handles).unwrap();
		assert_eq!(header.epoch, 0x1111_2222_3333_4444);
		assert_eq!(header.offset, 0x7654_3210);
		assert_eq!(
			handles,
			vec![WireHandle {
				slot: 0x89ab_cdef,
				generation: 0x0123_4567,
			}]
		);
		assert!(encode_production_frontier_append(&[0.0; 6]).is_err());
		assert!(encode_production_frontier_append(&vec![0.0; 6 + 513 * 4]).is_err());
	}

	#[test]
	fn production_gas_metadata_uses_parallel_bounded_records() {
		let numeric = [
			7.0,
			0x4321 as f32,
			0x8765 as f32,
			20.0,
			0.0,
			1.0,
			0.25,
			0.0,
			1.5,
			2.0,
			373.15,
			0.4,
			1.0,
		];
		let request = encode_production_gas_metadata(
			&numeric,
			&["plasma".to_owned()],
			&["Plasma".to_owned()],
			&[0.0, 3.0, 0.75],
		)
		.unwrap();
		assert_eq!(
			decode_gas_metadata_batch(&request).unwrap(),
			vec![GasMetadataRegistration {
				id: 7,
				key: "plasma".to_owned(),
				name: "Plasma".to_owned(),
				flags: 0x8765_4321,
				specific_heat: ScalarValue(20.0),
				fusion_power: ScalarValue(0.0),
				moles_visible: Some(ScalarValue(0.25)),
				enthalpy: ScalarValue(0.0),
				fire_radiation_released: ScalarValue(1.5),
				fire_role: WireGasFireRole::Fuel {
					minimum_temperature: ScalarValue(373.15_f32.into()),
					burn_rate: ScalarValue(0.4_f32.into()),
				},
				fire_products: Some(WireFireProducts::Generic(vec![WireGasProduct {
					gas_id: 3,
					ratio: ScalarValue(0.75),
				}])),
			}]
		);
		let mut noncanonical = numeric;
		noncanonical[5] = 0.0;
		assert!(encode_production_gas_metadata(
			&noncanonical,
			&["plasma".to_owned()],
			&["Plasma".to_owned()],
			&[0.0, 3.0, 0.75],
		)
		.is_err());
		assert!(
			encode_production_gas_metadata(&numeric, &[], &["Plasma".to_owned()], &[],).is_err()
		);
	}

	#[test]
	fn production_reaction_metadata_uses_lossless_ids_and_option_flags() {
		let numeric = [
			0xba98 as f32,
			0xfedc as f32,
			0.0,
			10.0,
			1.0,
			373.15,
			0.0,
			0.0,
			1.0,
			5.0,
			0.0,
			0.0,
		];
		let request = encode_production_reaction_metadata(
			&numeric,
			&["combustion".to_owned()],
			&[0.0, 7.0, 0.25],
		)
		.unwrap();
		assert_eq!(
			decode_reaction_metadata_batch(&request).unwrap(),
			vec![ReactionMetadataRegistration {
				id: 0xfedc_ba98,
				key: "combustion".to_owned(),
				priority: ScalarValue(10.0),
				minimum_temperature: Some(ScalarValue(373.15_f32.into())),
				maximum_temperature: None,
				minimum_energy: Some(ScalarValue(5.0)),
				minimum_fire_reagents: None,
				gas_requirements: vec![WireGasRequirement {
					gas_id: 7,
					minimum_moles: ScalarValue(0.25),
				}],
				execution: WireReactionExecution::Dm,
			}]
		);
		let mut noncanonical = numeric;
		noncanonical[6] = 0.0;
		noncanonical[7] = 1.0;
		assert!(encode_production_reaction_metadata(
			&noncanonical,
			&["combustion".to_owned()],
			&[],
		)
		.is_err());
		assert!(encode_production_reaction_metadata(
			&numeric,
			&["combustion".to_owned()],
			&[1.0, 7.0, 0.25],
		)
		.is_err());
	}

	#[test]
	fn production_telemetry_preserves_all_counter_bits() {
		let telemetry = ServiceTelemetry {
			callback_depth: 0xfedc_ba98,
			callback_capacity: 2,
			callback_high_water: 3,
			continuation_depth: 4,
			continuation_capacity: 5,
			continuation_high_water: 6,
			oldest_callback_age_ticks: 0x0123_4567_89ab_cdef,
			callback_enqueued: 8,
			callback_drained: 9,
			callback_rejected: 10,
			continuation_timeouts: 11,
			request_timeouts: 12,
			protocol_errors: 13,
			callback_enqueued_by_kind: [14, 15, 16, 17, 18, 19, 20, 21],
			callback_drained_by_kind: [22, 23, 24, 25, 26, 27, 28, 29],
			callback_rejected_by_kind: [30, 31, 32, 33, 34, 35, 36, u64::MAX],
			service_process_available_flags: SERVICE_PROCESS_ALL_AVAILABLE,
			service_rss_bytes: 0x1111_2222_3333_4444,
			service_cpu_total_milliseconds: 0x5555_6666_7777_8888,
			general_callback_depth: 38,
			reaction_callback_depth: 39,
			reaction_transaction_depth: 40,
			reaction_transaction_high_water: 41,
			frontier_count: 42,
			stage_kind: 5,
			frontier_upload_bytes: 43,
			stage_epoch: 44,
			stage_cursor: 45,
			stage_remaining: 46,
			topology_revision: 47,
			reusable_workset_bytes: 48,
			packed_topology_bytes: 49,
		};
		let fields = decode_production_service_telemetry(&telemetry.encode()).unwrap();
		assert_eq!(fields.len(), 182);
		assert_eq!(&fields[..2], &[0xba98 as f32, 0xfedc as f32]);
		assert_eq!(
			&fields[12..16],
			&[0xcdef as f32, 0x89ab as f32, 0x4567 as f32, 0x0123 as f32]
		);
		assert_eq!(&fields[132..136], &[65_535.0; 4]);
		assert_eq!(&fields[136..138], &[3.0, 0.0]);
		assert_eq!(&fields[138..142], &[17_476.0, 13_107.0, 8_738.0, 4_369.0]);
		assert_eq!(&fields[142..146], &[34_952.0, 30_583.0, 26_214.0, 21_845.0]);
		assert_eq!(&fields[146..148], &[38.0, 0.0]);
		assert_eq!(&fields[178..182], &[49.0, 0.0, 0.0, 0.0]);
	}

	#[test]
	fn production_process_metrics_preserve_roles_width_and_word_order() {
		let host = CurrentProcessMetrics {
			available_flags: PROCESS_ALL_AVAILABLE,
			private_bytes: 0x1111_2222_3333_4444,
			virtual_bytes: 0x5555_6666_7777_8888,
			working_set_bytes: 0x9999_aaaa_bbbb_cccc,
			cpu_total_milliseconds: 99,
		};
		let service = ServiceTelemetry {
			callback_depth: 0,
			callback_capacity: 0,
			callback_high_water: 0,
			continuation_depth: 0,
			continuation_capacity: 0,
			continuation_high_water: 0,
			oldest_callback_age_ticks: 0,
			callback_enqueued: 0,
			callback_drained: 0,
			callback_rejected: 0,
			continuation_timeouts: 0,
			request_timeouts: 0,
			protocol_errors: 0,
			callback_enqueued_by_kind: [0; 8],
			callback_drained_by_kind: [0; 8],
			callback_rejected_by_kind: [0; 8],
			service_process_available_flags: SERVICE_PROCESS_ALL_AVAILABLE,
			service_rss_bytes: 0xdddd_eeee_ffff_0001,
			service_cpu_total_milliseconds: u64::MAX,
			..Default::default()
		};

		let fields = encode_production_process_metrics(host, &service.encode()).unwrap();

		assert_eq!(fields.len(), 28);
		assert_eq!(
			fields,
			vec![
				1.0, 0.0, 7.0, 0.0, 3.0, 0.0, 0.0, 0.0, 17_476.0, 13_107.0, 8_738.0, 4_369.0,
				34_952.0, 30_583.0, 26_214.0, 21_845.0, 52_428.0, 48_059.0, 43_690.0, 39_321.0,
				1.0, 65_535.0, 61_166.0, 56_797.0, 65_535.0, 65_535.0, 65_535.0, 65_535.0,
			]
		);
	}

	#[test]
	fn production_process_metrics_reject_noncanonical_host_samples() {
		let empty_service = ServiceTelemetry {
			callback_depth: 0,
			callback_capacity: 0,
			callback_high_water: 0,
			continuation_depth: 0,
			continuation_capacity: 0,
			continuation_high_water: 0,
			oldest_callback_age_ticks: 0,
			callback_enqueued: 0,
			callback_drained: 0,
			callback_rejected: 0,
			continuation_timeouts: 0,
			request_timeouts: 0,
			protocol_errors: 0,
			callback_enqueued_by_kind: [0; 8],
			callback_drained_by_kind: [0; 8],
			callback_rejected_by_kind: [0; 8],
			service_process_available_flags: 0,
			service_rss_bytes: 0,
			service_cpu_total_milliseconds: 0,
			..Default::default()
		}
		.encode();
		let unknown_flags = CurrentProcessMetrics {
			available_flags: 16,
			..Default::default()
		};
		let nonzero_unavailable = CurrentProcessMetrics {
			private_bytes: 1,
			..Default::default()
		};

		assert!(encode_production_process_metrics(unknown_flags, &empty_service).is_err());
		assert!(encode_production_process_metrics(nonzero_unavailable, &empty_service).is_err());
		let partial = CurrentProcessMetrics {
			available_flags: PROCESS_PRIVATE_BYTES_AVAILABLE,
			private_bytes: 1,
			..Default::default()
		};
		assert!(encode_production_process_metrics(partial, &empty_service).is_ok());
	}

	#[test]
	fn production_callbacks_preserve_events_and_continuation_tokens() {
		let token = ContinuationToken {
			world_generation: 0x8765_4321,
			id: 0x0123_4567_89ab_cdef,
			deadline_ticks: 0xfedc_ba98_7654_3210,
		};
		let event = CallbackEvent {
			scope_sequence: 0x1111_2222_3333_4444,
			transaction_id: 0x9999_aaaa_bbbb_cccc,
			scope: CallbackScope::Reaction,
			kind: CallbackEventKind::RunDmReaction,
			flags: 0,
			subject: handle(0x1234, 2),
			target: handle(0x5678, 3),
			values: [
				ScalarValue(1.0),
				ScalarValue(2.0),
				ScalarValue(3.0),
				ScalarValue(4.0),
			],
			aux: 0xaaaa_bbbb,
			continuation: Some(token),
		};
		let mut response = CallbackBatchHeader {
			returned: 1,
			remaining: 2,
			capacity: 256,
			high_water: 10,
			rejected: 0x0123_4567_89ab_cdef,
		}
		.encode()
		.to_vec();
		response.extend(event.encode().unwrap());
		let fields = decode_production_callback_batch(
			&response,
			1,
			CallbackScope::Reaction,
			0x9999_aaaa_bbbb_cccc,
		)
		.unwrap();
		assert_eq!(fields.len(), 12 + 36);
		assert_eq!(&fields[..2], &[1.0, 0.0]);
		assert_eq!(
			&fields[12..16],
			&[0x4444 as f32, 0x3333 as f32, 0x2222 as f32, 0x1111 as f32]
		);
		assert_eq!(
			&fields[16..20],
			&[0xcccc as f32, 0xbbbb as f32, 0xaaaa as f32, 0x9999 as f32]
		);
		assert_eq!(fields[20], CallbackScope::Reaction as u16 as f32);
		assert_eq!(fields[21], CallbackEventKind::RunDmReaction as u16 as f32);
		assert_eq!(fields[37], 1.0);
		assert_eq!(
			decode_production_continuation_token(&fields[38..48]).unwrap(),
			token
		);
		assert!(decode_production_callback_batch(
			&response,
			0,
			CallbackScope::Reaction,
			0x9999_aaaa_bbbb_cccc
		)
		.is_err());
		response.pop();
		assert!(decode_production_callback_batch(
			&response,
			1,
			CallbackScope::Reaction,
			0x9999_aaaa_bbbb_cccc
		)
		.is_err());

		let mut wrapped = CallbackBatchHeader {
			returned: 2,
			remaining: 0,
			capacity: 256,
			high_water: 2,
			rejected: 0,
		}
		.encode()
		.to_vec();
		wrapped.extend(
			CallbackEvent {
				scope_sequence: u64::MAX,
				..event
			}
			.encode()
			.unwrap(),
		);
		wrapped.extend(
			CallbackEvent {
				scope_sequence: 0,
				..event
			}
			.encode()
			.unwrap(),
		);
		assert!(decode_production_callback_batch(
			&wrapped,
			2,
			CallbackScope::Reaction,
			0x9999_aaaa_bbbb_cccc
		)
		.is_err());
	}

	#[test]
	fn production_continuation_commands_use_exact_tokens() {
		let token_fields = [
			0x4321 as f32,
			0x8765 as f32,
			0xcdef as f32,
			0x89ab as f32,
			0x4567 as f32,
			0x0123 as f32,
			0x3210 as f32,
			0x7654 as f32,
			0xba98 as f32,
			0xfedc as f32,
		];
		let token = decode_production_continuation_token(&token_fields).unwrap();
		let mut command_fields = token_fields.to_vec();
		command_fields.extend([1.0, 0.0, 7.0, 2.0, 0.0, 0.0, 5.0, 0.0, 0.0, 3.0, 0.0]);
		let command = encode_production_continuation_command(&command_fields).unwrap();
		assert_eq!(
			ContinuationCommandRequest::decode(&command).unwrap(),
			ContinuationCommandRequest {
				token,
				command: MixtureCommandRequest::SetMoles {
					handle: handle(7, 2),
					gas_id: 3,
					amount: ScalarValue(5.0),
				},
			}
		);
		let mut adjust_fields = token_fields.to_vec();
		adjust_fields.extend([7.0, 2.0, 3.0, -0.5]);
		let (actual_token, actual_handle, adjustments) =
			decode_continuation_adjust_multiple_request(
				&encode_production_continuation_adjust_multiple(&adjust_fields).unwrap(),
			)
			.unwrap();
		assert_eq!(actual_token, token);
		assert_eq!(actual_handle, handle(7, 2));
		assert_eq!(adjustments[0].delta, ScalarValue(-0.5));
		let mut resume_fields = token_fields.to_vec();
		resume_fields.push(5.0);
		assert_eq!(
			ContinuationResumeRequest::decode(
				&encode_production_continuation_resume(&resume_fields).unwrap()
			),
			Ok(ContinuationResumeRequest {
				token,
				reaction_result: 5,
			})
		);
		let mut invalid = token_fields;
		invalid[2] = 65_536.0;
		assert!(decode_production_continuation_token(&invalid).is_err());
	}

	#[test]
	fn binary_identity_fields_use_exact_lowercase_hex() {
		assert_eq!(hex_lower(&[0x00, 0x09, 0x10, 0xab, 0xff]), "000910abff");
	}

	#[test]
	fn generated_bindings_are_sorted_with_canonical_whitespace() {
		let generated = "header  \r\n#define DOGMOS value\r\n\r\n/proc/zeta()\r\n\treturn 2 \r\n\r\n/// Alpha\r\n/proc/alpha()\r\n\treturn 1\r\n\r\n";
		let expected = "header\n#define DOGMOS value\n\n/// Alpha\n/proc/alpha()\n\treturn 1\n\n/proc/zeta()\n\treturn 2\n";

		assert_eq!(normalize_generated_bindings(generated), expected);
	}

	#[test]
	fn generated_bindings_use_the_deployed_library_and_opendream_compatible_calls() {
		let generated = "generated header\n\n/* This comment bypasses grep checks */ /var/__dogmos\n\n/proc/__detect_dogmos()\n\tif (world.system_type == UNIX)\n\t\treturn __dogmos = \"libdogmos\"\n\telse\n\t\treturn __dogmos = \"dogmos\"\n\n#define DOGMOS (__dogmos || __detect_dogmos())\n\n/proc/dogmos_example(value)\n\tvar/static/loaded = load_ext(DOGMOS, \"byond:dogmos_example_ffi\")\n\treturn call_ext(loaded)(value)\n";
		let expected = "generated header\n\n#define DOGMOS (world.system_type == UNIX ? \"libdogmos\" : \"dogmos\")\n\n/proc/dogmos_example(value)\n\treturn call_ext(DOGMOS, \"byond:dogmos_example_ffi\")(value)\n";

		assert_eq!(normalize_generated_bindings(generated), expected);
	}

	#[test]
	fn diagnostic_allocation_rejects_non_finite_negative_and_oversized_values() {
		assert!(diagnostic_bytes_from_number(f32::NAN).is_err());
		assert!(diagnostic_bytes_from_number(-1.0).is_err());
		assert!(diagnostic_bytes_from_number(9.0 * 1024.0 * 1024.0 * 1024.0).is_err());
		assert_eq!(
			diagnostic_bytes_from_number(512.0 * 1024.0 * 1024.0).unwrap(),
			536_870_912
		);
	}

	#[test]
	fn scalar_response_requires_the_exact_wire_width() {
		let response = 42.5_f64.to_le_bytes();
		assert_eq!(scalar_response_value(&response, 8).unwrap(), 42.5);
		assert!(scalar_response_value(&response, 0).is_err());
		assert!(scalar_response_value(&response, 7).is_err());
	}

	#[test]
	fn callback_count_requires_a_bounded_integer() {
		assert_eq!(callback_count_from_number(65_536.0).unwrap(), 65_536);
		assert!(callback_count_from_number(65_537.0).is_err());
		assert!(callback_count_from_number(1.5).is_err());
		assert!(callback_count_from_number(-1.0).is_err());
		assert!(callback_count_from_number(f32::NAN).is_err());
	}

	#[cfg(not(windows))]
	#[test]
	fn system_auth_tokens_are_nonempty_and_fresh() {
		let first = super::session::system_auth_token().unwrap();
		let second = super::session::system_auth_token().unwrap();
		assert_ne!(first, [0; 32]);
		assert_ne!(second, [0; 32]);
		assert_ne!(first, second);
	}
}
