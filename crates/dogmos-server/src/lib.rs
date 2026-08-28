mod state;

use dogmos_protocol::{
	decode_adjacency_batch, decode_adjust_multiple_request,
	decode_continuation_adjust_multiple_request, decode_gas_metadata_batch, decode_lifecycle_batch,
	decode_mixture_state_batch, decode_reaction_metadata_batch, decode_turf_adjacency_batch,
	decode_turf_heat_adjacency_batch, decode_turf_heat_batch, decode_turf_lifecycle_batch,
	read_frame_into, write_frame, CallbackBatchRequest, ContinuationCommandRequest,
	ContinuationToken, HandshakePayload, MixtureCommandRequest, MixtureSnapshotRequest,
	OperationKind, ProtocolHeader, ServiceErrorCode, SimulationStageRequest,
	SimulationStageResponse, FLAG_ERROR, HANDSHAKE_PAYLOAD_LEN, MAX_CONTROL_PAYLOAD,
};
use interprocess::local_socket::{
	prelude::*, GenericNamespaced, Listener, ListenerNonblockingMode, ListenerOptions, Stream,
};
use state::ServiceState;
use std::{
	error::Error,
	io::{self, Read},
	sync::{
		atomic::{AtomicBool, Ordering},
		Arc,
	},
	thread,
	time::{Duration, Instant},
};

#[derive(Clone, Copy)]
struct RequestDeadline {
	received_at: Instant,
	budget: Option<Duration>,
}

struct AuthenticatedSessionGuard<'a>(&'a AtomicBool);

impl Drop for AuthenticatedSessionGuard<'_> {
	fn drop(&mut self) {
		self.0.store(true, Ordering::Release);
	}
}

impl RequestDeadline {
	fn from_budget_ns(budget_ns: u64) -> Self {
		Self {
			received_at: Instant::now(),
			budget: (budget_ns != 0).then(|| Duration::from_nanos(budget_ns)),
		}
	}

	fn is_expired(self) -> bool {
		self.budget
			.is_some_and(|budget| self.received_at.elapsed() >= budget)
	}
}

struct RequestSequence {
	last_request_id: u64,
}

impl RequestSequence {
	const fn after_handshake(request_id: u64) -> Self {
		Self {
			last_request_id: request_id,
		}
	}

	fn accept(&mut self, request_id: u64) -> bool {
		if request_id <= self.last_request_id {
			return false;
		}
		self.last_request_id = request_id;
		true
	}
}

pub fn run(endpoint: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
	let expected = read_startup_handshake()?;
	verify_executable_identity(&expected)?;
	let listener = create_listener(endpoint)?;
	let active = Arc::new(AtomicBool::new(false));
	let shutdown = Arc::new(AtomicBool::new(false));
	let mut primary = None;

	while !shutdown.load(Ordering::Acquire) {
		match listener.accept() {
			Ok(stream) if active.swap(true, Ordering::AcqRel) => reject_busy(stream, &expected)?,
			Ok(stream) => {
				let active = Arc::clone(&active);
				let shutdown = Arc::clone(&shutdown);
				primary = Some(thread::spawn(move || {
					let result = handle_primary(stream, expected, &shutdown);
					active.store(false, Ordering::Release);
					result
				}));
			}
			Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
				thread::sleep(Duration::from_millis(1));
			}
			Err(error) => return Err(error.into()),
		}
	}

	if let Some(primary) = primary {
		primary
			.join()
			.map_err(|_| "dogmosd client thread panicked")??;
	}
	Ok(())
}

fn verify_executable_identity(
	expected: &HandshakePayload,
) -> Result<(), Box<dyn Error + Send + Sync>> {
	let executable = std::env::current_exe()?;
	let actual = dogmos_identity::sha256_file(&executable)?;
	if actual != expected.identity.executable_digest {
		return Err("dogmosd executable digest does not match the startup identity".into());
	}
	Ok(())
}

fn read_startup_handshake() -> Result<HandshakePayload, Box<dyn Error + Send + Sync>> {
	let mut bytes = [0_u8; HANDSHAKE_PAYLOAD_LEN];
	io::stdin().read_exact(&mut bytes)?;
	Ok(HandshakePayload::decode(&bytes)?)
}

fn create_listener(endpoint: &str) -> Result<Listener, Box<dyn Error + Send + Sync>> {
	let name = endpoint.to_ns_name::<GenericNamespaced>()?;
	let options = ListenerOptions::new()
		.name(name)
		.nonblocking(ListenerNonblockingMode::Accept);
	let options = secure_listener_options(options)?;
	Ok(options.create_sync()?)
}

#[cfg(windows)]
fn secure_listener_options(
	options: ListenerOptions<'_>,
) -> Result<ListenerOptions<'_>, Box<dyn Error + Send + Sync>> {
	use interprocess::os::windows::local_socket::ListenerOptionsExt;

	Ok(options.security_descriptor(windows_security::current_user_only()?))
}

#[cfg(unix)]
fn secure_listener_options(
	options: ListenerOptions<'_>,
) -> Result<ListenerOptions<'_>, Box<dyn Error + Send + Sync>> {
	use interprocess::os::unix::local_socket::ListenerOptionsExt;

	Ok(options.mode(0o600))
}

fn reject_busy(
	mut stream: Stream,
	expected: &HandshakePayload,
) -> Result<(), Box<dyn Error + Send + Sync>> {
	let payload = ServiceErrorCode::Busy.encode();
	let mut header = ProtocolHeader::request(
		OperationKind::Handshake,
		0,
		expected.world_generation,
		expected.world_nonce,
		payload.len() as u32,
		0,
	)
	.response();
	header.flags |= FLAG_ERROR;
	write_frame(&mut stream, header, &payload)?;
	Ok(())
}

fn handle_primary(
	mut stream: Stream,
	expected: HandshakePayload,
	shutdown: &AtomicBool,
) -> Result<(), Box<dyn Error + Send + Sync>> {
	let mut payload = vec![0_u8; MAX_CONTROL_PAYLOAD as usize];
	let (request, payload_len) = read_frame_into(&mut stream, &mut payload)?;
	if request.operation_kind()? != OperationKind::Handshake {
		return Err("first operation must be a handshake".into());
	}
	let handshake = HandshakePayload::decode(&payload[..payload_len])?;
	if handshake.validate_peer(&expected).is_err() {
		write_error_response(&mut stream, request, ServiceErrorCode::AuthenticationFailed)?;
		return Ok(());
	}
	let response_payload = HandshakePayload {
		process_id: std::process::id(),
		..expected
	}
	.encode();
	let mut response = request.response();
	response.payload_len = HANDSHAKE_PAYLOAD_LEN as u32;
	write_frame(&mut stream, response, &response_payload)?;
	let _authenticated_session = AuthenticatedSessionGuard(shutdown);
	let mut request_sequence = RequestSequence::after_handshake(request.request_id);
	let mut diagnostic_arena: Vec<u8> = Vec::new();
	let mut service_state = ServiceState::new_for_world(
		expected.capacities.max_world_bytes,
		expected.capacities.max_callback_events,
		expected.capacities.max_pending_continuations,
		expected.world_generation,
	);

	loop {
		let (request, payload_len) = read_frame_into(&mut stream, &mut payload)?;
		let deadline = RequestDeadline::from_budget_ns(request.deadline_ns);
		if request.world_generation != expected.world_generation
			|| request.world_nonce != expected.world_nonce
		{
			return Err("request belongs to a different world".into());
		}
		if !request_sequence.accept(request.request_id) {
			service_state.record_protocol_error();
			write_error_response(&mut stream, request, ServiceErrorCode::InvalidRequest)?;
			continue;
		}
		if deadline.is_expired() {
			service_state.record_request_timeout();
			write_error_response(&mut stream, request, ServiceErrorCode::DeadlineExceeded)?;
			continue;
		}
		let operation = match request.operation_kind() {
			Ok(operation) => operation,
			Err(_) => {
				service_state.record_protocol_error();
				write_error_response(&mut stream, request, ServiceErrorCode::InvalidRequest)?;
				continue;
			}
		};
		match operation {
			OperationKind::Echo => write_response(&mut stream, request, &payload[..payload_len])?,
			OperationKind::ScalarGet | OperationKind::Transfer => {
				payload[..8].fill(0);
				write_response(&mut stream, request, &payload[..8])?;
			}
			OperationKind::ScalarSet | OperationKind::AdjacencyUpdate => {
				write_response(&mut stream, request, &[])?;
			}
			OperationKind::GasVector => {
				payload[..260].fill(0);
				write_response(&mut stream, request, &payload[..260])?;
			}
			OperationKind::Batch => {
				let operation_count = read_count(&payload[..payload_len])?;
				let response = operation_count.to_le_bytes();
				write_response(&mut stream, request, &response)?;
			}
			OperationKind::CallbackBatch => {
				let Ok(callback_request) = CallbackBatchRequest::decode(&payload[..payload_len])
				else {
					service_state.record_protocol_error();
					write_error_response(&mut stream, request, ServiceErrorCode::InvalidRequest)?;
					continue;
				};
				if callback_request.max_events > expected.capacities.max_callback_events {
					service_state.record_protocol_error();
					write_error_response(&mut stream, request, ServiceErrorCode::InvalidRequest)?;
					continue;
				}
				let response_len = service_state.drain_callbacks(
					callback_request.max_events,
					&mut payload[..expected.capacities.max_control_payload as usize],
				)?;
				write_response(&mut stream, request, &payload[..response_len])?;
			}
			OperationKind::DiagnosticCallbackEnqueue => {
				let Ok(callback_request) = CallbackBatchRequest::decode(&payload[..payload_len])
				else {
					service_state.record_protocol_error();
					write_error_response(&mut stream, request, ServiceErrorCode::InvalidRequest)?;
					continue;
				};
				match service_state.enqueue_diagnostic_callbacks(callback_request.max_events) {
					Ok(accepted) => write_response(&mut stream, request, &accepted.to_le_bytes())?,
					Err(state::StateError::CallbackBackpressure) => write_error_response(
						&mut stream,
						request,
						ServiceErrorCode::CallbackBackpressure,
					)?,
					Err(error) => return Err(error.into()),
				}
			}
			OperationKind::AllocateDiagnostic => {
				let requested_bytes = read_u64_payload(&payload[..payload_len])?;
				if requested_bytes > expected.capacities.max_world_bytes
					|| requested_bytes > usize::MAX as u64
				{
					service_state.record_protocol_error();
					write_error_response(&mut stream, request, ServiceErrorCode::InvalidRequest)?;
					continue;
				}
				if requested_bytes == 0 {
					diagnostic_arena = Vec::new();
				} else {
					let requested_bytes = requested_bytes as usize;
					diagnostic_arena.clear();
					diagnostic_arena
						.try_reserve_exact(requested_bytes)
						.map_err(|_| "diagnostic allocation failed")?;
					diagnostic_arena.resize(requested_bytes, 0);
					for page in diagnostic_arena.chunks_mut(4096) {
						page[0] = 0xa5;
					}
				}
				let response = (diagnostic_arena.len() as u64).to_le_bytes();
				write_response(&mut stream, request, &response)?;
			}
			OperationKind::MixtureSnapshot => {
				let Ok(snapshot_request) = MixtureSnapshotRequest::decode(&payload[..payload_len])
				else {
					service_state.record_protocol_error();
					write_error_response(&mut stream, request, ServiceErrorCode::InvalidRequest)?;
					continue;
				};
				let snapshot = match service_state.snapshot(snapshot_request.handle) {
					Ok(snapshot) => snapshot,
					Err(error) => {
						write_error_response(&mut stream, request, service_error_code(&error))?;
						continue;
					}
				};
				let response = snapshot.encode()?;
				write_response(&mut stream, request, &response)?;
			}
			OperationKind::MixtureCommand => {
				let Ok(command) = MixtureCommandRequest::decode(&payload[..payload_len]) else {
					service_state.record_protocol_error();
					write_error_response(&mut stream, request, ServiceErrorCode::InvalidRequest)?;
					continue;
				};
				let response = match service_state.apply_mixture_command(command) {
					Ok(response) => response,
					Err(error) => {
						write_error_response(&mut stream, request, service_error_code(&error))?;
						continue;
					}
				};
				write_response(&mut stream, request, &response.encode()?)?;
			}
			OperationKind::MixtureAdjustMultiple => {
				let Ok((handle, adjustments)) =
					decode_adjust_multiple_request(&payload[..payload_len])
				else {
					service_state.record_protocol_error();
					write_error_response(&mut stream, request, ServiceErrorCode::InvalidRequest)?;
					continue;
				};
				let response = match service_state.apply_adjust_multiple(handle, &adjustments) {
					Ok(response) => response,
					Err(error) => {
						write_error_response(&mut stream, request, service_error_code(&error))?;
						continue;
					}
				};
				write_response(&mut stream, request, &response.encode()?)?;
			}
			OperationKind::ContinuationCommand => {
				let Ok(command) = ContinuationCommandRequest::decode(&payload[..payload_len])
				else {
					service_state.record_protocol_error();
					write_error_response(&mut stream, request, ServiceErrorCode::InvalidRequest)?;
					continue;
				};
				let response = match service_state
					.apply_continuation_command(command.token, command.command)
				{
					Ok(response) => response,
					Err(error) => {
						write_error_response(&mut stream, request, service_error_code(&error))?;
						continue;
					}
				};
				write_response(&mut stream, request, &response.encode()?)?;
			}
			OperationKind::ContinuationAdjustMultiple => {
				let Ok((token, handle, adjustments)) =
					decode_continuation_adjust_multiple_request(&payload[..payload_len])
				else {
					service_state.record_protocol_error();
					write_error_response(&mut stream, request, ServiceErrorCode::InvalidRequest)?;
					continue;
				};
				let response = match service_state.apply_continuation_adjust_multiple(
					token,
					handle,
					&adjustments,
				) {
					Ok(response) => response,
					Err(error) => {
						write_error_response(&mut stream, request, service_error_code(&error))?;
						continue;
					}
				};
				write_response(&mut stream, request, &response.encode()?)?;
			}
			OperationKind::ContinuationResume => {
				let Ok(token) = ContinuationToken::decode(&payload[..payload_len]) else {
					service_state.record_protocol_error();
					write_error_response(&mut stream, request, ServiceErrorCode::InvalidRequest)?;
					continue;
				};
				let response = match service_state.resume_continuation(token) {
					Ok(response) => response,
					Err(error) => {
						write_error_response(&mut stream, request, service_error_code(&error))?;
						continue;
					}
				};
				write_response(&mut stream, request, &response.encode()?)?;
			}
			OperationKind::ContinuationCancel => {
				let Ok(token) = ContinuationToken::decode(&payload[..payload_len]) else {
					service_state.record_protocol_error();
					write_error_response(&mut stream, request, ServiceErrorCode::InvalidRequest)?;
					continue;
				};
				if let Err(error) = service_state.cancel_continuation(token) {
					write_error_response(&mut stream, request, service_error_code(&error))?;
					continue;
				}
				write_response(&mut stream, request, &[])?;
			}
			OperationKind::GasMetadataInstall => {
				let Ok(entries) = decode_gas_metadata_batch(&payload[..payload_len]) else {
					service_state.record_protocol_error();
					write_error_response(&mut stream, request, ServiceErrorCode::InvalidRequest)?;
					continue;
				};
				let count = match service_state.install_gases(entries) {
					Ok(count) => count,
					Err(error) => {
						write_error_response(&mut stream, request, service_error_code(&error))?;
						continue;
					}
				};
				write_response(&mut stream, request, &count.to_le_bytes())?;
			}
			OperationKind::ReactionMetadataInstall => {
				let Ok(entries) = decode_reaction_metadata_batch(&payload[..payload_len]) else {
					service_state.record_protocol_error();
					write_error_response(&mut stream, request, ServiceErrorCode::InvalidRequest)?;
					continue;
				};
				let count = match service_state.install_reactions(entries) {
					Ok(count) => count,
					Err(error) => {
						write_error_response(&mut stream, request, service_error_code(&error))?;
						continue;
					}
				};
				write_response(&mut stream, request, &count.to_le_bytes())?;
			}
			OperationKind::MixtureLifecycleBatch => {
				let Ok(mutations) = decode_lifecycle_batch(
					&payload[..payload_len],
					expected.capacities.max_batch_operations,
				) else {
					service_state.record_protocol_error();
					write_error_response(&mut stream, request, ServiceErrorCode::InvalidRequest)?;
					continue;
				};
				let operation_count = match service_state.apply_lifecycle(&mutations) {
					Ok(operation_count) => operation_count,
					Err(error) => {
						write_error_response(&mut stream, request, service_error_code(&error))?;
						continue;
					}
				};
				write_response(&mut stream, request, &operation_count.to_le_bytes())?;
			}
			OperationKind::MixtureStateBatch => {
				let Ok(mutations) = decode_mixture_state_batch(
					&payload[..payload_len],
					expected.capacities.max_batch_operations,
				) else {
					service_state.record_protocol_error();
					write_error_response(&mut stream, request, ServiceErrorCode::InvalidRequest)?;
					continue;
				};
				let operation_count = match service_state.apply_mixture_state(&mutations) {
					Ok(operation_count) => operation_count,
					Err(error) => {
						write_error_response(&mut stream, request, service_error_code(&error))?;
						continue;
					}
				};
				write_response(&mut stream, request, &operation_count.to_le_bytes())?;
			}
			OperationKind::AdjacencyBatch => {
				let Ok(mutations) = decode_adjacency_batch(
					&payload[..payload_len],
					expected.capacities.max_batch_operations,
				) else {
					service_state.record_protocol_error();
					write_error_response(&mut stream, request, ServiceErrorCode::InvalidRequest)?;
					continue;
				};
				let operation_count = match service_state.apply_adjacency(&mutations) {
					Ok(operation_count) => operation_count,
					Err(error) => {
						write_error_response(&mut stream, request, service_error_code(&error))?;
						continue;
					}
				};
				write_response(&mut stream, request, &operation_count.to_le_bytes())?;
			}
			OperationKind::TurfLifecycleBatch => {
				let Ok(mutations) = decode_turf_lifecycle_batch(
					&payload[..payload_len],
					expected.capacities.max_batch_operations,
				) else {
					service_state.record_protocol_error();
					write_error_response(&mut stream, request, ServiceErrorCode::InvalidRequest)?;
					continue;
				};
				let operation_count = match service_state.apply_turf_lifecycle(&mutations) {
					Ok(operation_count) => operation_count,
					Err(error) => {
						write_error_response(&mut stream, request, service_error_code(&error))?;
						continue;
					}
				};
				write_response(&mut stream, request, &operation_count.to_le_bytes())?;
			}
			OperationKind::TurfAdjacencyBatch => {
				let Ok(mutations) = decode_turf_adjacency_batch(
					&payload[..payload_len],
					expected.capacities.max_batch_operations,
				) else {
					service_state.record_protocol_error();
					write_error_response(&mut stream, request, ServiceErrorCode::InvalidRequest)?;
					continue;
				};
				let operation_count = match service_state.apply_turf_adjacency(&mutations) {
					Ok(operation_count) => operation_count,
					Err(error) => {
						write_error_response(&mut stream, request, service_error_code(&error))?;
						continue;
					}
				};
				write_response(&mut stream, request, &operation_count.to_le_bytes())?;
			}
			OperationKind::TurfHeatBatch => {
				let Ok(mutations) = decode_turf_heat_batch(
					&payload[..payload_len],
					expected.capacities.max_batch_operations,
				) else {
					service_state.record_protocol_error();
					write_error_response(&mut stream, request, ServiceErrorCode::InvalidRequest)?;
					continue;
				};
				let operation_count = match service_state.apply_turf_heat(&mutations) {
					Ok(operation_count) => operation_count,
					Err(error) => {
						write_error_response(&mut stream, request, service_error_code(&error))?;
						continue;
					}
				};
				write_response(&mut stream, request, &operation_count.to_le_bytes())?;
			}
			OperationKind::TurfHeatAdjacencyBatch => {
				let Ok(mutations) = decode_turf_heat_adjacency_batch(
					&payload[..payload_len],
					expected.capacities.max_batch_operations,
				) else {
					service_state.record_protocol_error();
					write_error_response(&mut stream, request, ServiceErrorCode::InvalidRequest)?;
					continue;
				};
				let operation_count = match service_state.apply_turf_heat_adjacency(&mutations) {
					Ok(operation_count) => operation_count,
					Err(error) => {
						write_error_response(&mut stream, request, service_error_code(&error))?;
						continue;
					}
				};
				write_response(&mut stream, request, &operation_count.to_le_bytes())?;
			}
			OperationKind::SimulationStage => {
				let Ok(stage_request) = SimulationStageRequest::decode(&payload[..payload_len])
				else {
					service_state.record_protocol_error();
					write_error_response(&mut stream, request, ServiceErrorCode::InvalidRequest)?;
					continue;
				};
				let result = match service_state.process_stage_cancellable(
					stage_request.stage,
					stage_request.seconds_per_tick.0,
					|| deadline.is_expired(),
				) {
					Ok(result) => result,
					Err(error) => {
						write_error_response(&mut stream, request, service_error_code(&error))?;
						continue;
					}
				};
				let response = SimulationStageResponse {
					work_items: result.work_items,
					callback_events: result.callback_events,
				}
				.encode();
				write_response(&mut stream, request, &response)?;
			}
			OperationKind::ServiceTelemetry => {
				if payload_len != 0 {
					service_state.record_protocol_error();
					write_error_response(&mut stream, request, ServiceErrorCode::InvalidRequest)?;
					continue;
				}
				write_response(&mut stream, request, &service_state.telemetry().encode())?;
			}
			OperationKind::Shutdown => {
				write_frame(&mut stream, request.response(), &[])?;
				shutdown.store(true, Ordering::Release);
				return Ok(());
			}
			_ => return Err("operation is not implemented by the echo server".into()),
		}
	}
}

fn service_error_code(error: &state::StateError) -> ServiceErrorCode {
	match error {
		state::StateError::UnknownHandle(_) => ServiceErrorCode::UnknownHandle,
		state::StateError::StaleHandle { .. } => ServiceErrorCode::StaleHandle,
		state::StateError::RevisionMismatch { .. } => ServiceErrorCode::RevisionMismatch,
		state::StateError::RevisionExhausted(_) => ServiceErrorCode::RevisionExhausted,
		state::StateError::DuplicateMixtureState(_) => ServiceErrorCode::DuplicateMixtureState,
		state::StateError::InvalidMixtureState => ServiceErrorCode::InvalidMixtureState,
		state::StateError::StateCapacityExceeded => ServiceErrorCode::StateCapacityExceeded,
		state::StateError::AllocationFailed => ServiceErrorCode::AllocationFailed,
		state::StateError::Graph(_) => ServiceErrorCode::InvalidGraph,
		state::StateError::CallbackBackpressure => ServiceErrorCode::CallbackBackpressure,
		state::StateError::ContinuationCapacityExceeded => {
			ServiceErrorCode::ContinuationCapacityExceeded
		}
		state::StateError::UnknownContinuation(_) => ServiceErrorCode::UnknownContinuation,
		state::StateError::ContinuationWorldMismatch { .. } => {
			ServiceErrorCode::ContinuationWorldMismatch
		}
		state::StateError::ContinuationTokenMismatch(_) => {
			ServiceErrorCode::ContinuationTokenMismatch
		}
		state::StateError::ContinuationExpired(_) => ServiceErrorCode::ContinuationExpired,
		state::StateError::Cancelled => ServiceErrorCode::DeadlineExceeded,
		state::StateError::SelfAdjacency(_)
		| state::StateError::DuplicateTurfAdjacency { .. }
		| state::StateError::InvalidMetadata
		| state::StateError::InvalidConductivity
		| state::StateError::InvalidSecondsPerTick
		| state::StateError::StageNotImplemented(_) => ServiceErrorCode::InvalidRequest,
		state::StateError::State(_)
		| state::StateError::CallbackOutputTooSmall
		| state::StateError::CallbackSequenceExhausted
		| state::StateError::ContinuationIdExhausted
		| state::StateError::ContinuationDeadlineExhausted => ServiceErrorCode::Internal,
	}
}

fn write_response(
	stream: &mut Stream,
	request: ProtocolHeader,
	payload: &[u8],
) -> Result<(), Box<dyn Error + Send + Sync>> {
	let mut response = request.response();
	response.payload_len = payload.len() as u32;
	write_frame(stream, response, payload)?;
	Ok(())
}

fn read_count(payload: &[u8]) -> Result<u32, Box<dyn Error + Send + Sync>> {
	let bytes: [u8; 4] = payload
		.get(..4)
		.ok_or("count payload is truncated")?
		.try_into()?;
	Ok(u32::from_le_bytes(bytes))
}

fn read_u64_payload(payload: &[u8]) -> Result<u64, Box<dyn Error + Send + Sync>> {
	let bytes: [u8; 8] = payload
		.try_into()
		.map_err(|_| "u64 payload must contain exactly eight bytes")?;
	Ok(u64::from_le_bytes(bytes))
}

fn write_error_response(
	stream: &mut Stream,
	request: ProtocolHeader,
	code: ServiceErrorCode,
) -> Result<(), Box<dyn Error + Send + Sync>> {
	let payload = code.encode();
	let mut response = request.response();
	response.flags |= FLAG_ERROR;
	response.payload_len = payload.len() as u32;
	write_frame(stream, response, &payload)?;
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::RequestSequence;

	#[test]
	fn request_sequence_rejects_duplicate_and_decreasing_ids() {
		let mut sequence = RequestSequence::after_handshake(7);

		assert!(sequence.accept(8));
		assert!(!sequence.accept(8));
		assert!(!sequence.accept(7));
		assert!(!sequence.accept(1));
		assert!(sequence.accept(9));
	}

	#[test]
	fn request_sequence_accepts_the_full_strictly_increasing_range() {
		let mut sequence = RequestSequence::after_handshake(u64::MAX - 1);

		assert!(sequence.accept(u64::MAX));
		assert!(!sequence.accept(0));
	}
}

#[cfg(windows)]
mod windows_security {
	use interprocess::os::windows::security_descriptor::{
		AsSecurityDescriptorMutExt, SecurityDescriptor,
	};
	use std::{ffi::c_void, io, mem, ptr};
	use windows_sys::Win32::{
		Foundation::{CloseHandle, LocalFree, GENERIC_ALL, HANDLE},
		Security::{
			AddAccessAllowedAce, GetLengthSid, GetTokenInformation, InitializeAcl, TokenUser,
			ACCESS_ALLOWED_ACE, ACL, ACL_REVISION, TOKEN_QUERY, TOKEN_USER,
		},
		System::{
			Memory::{LocalAlloc, LMEM_FIXED},
			Threading::{GetCurrentProcess, OpenProcessToken},
		},
	};

	pub fn current_user_only() -> io::Result<SecurityDescriptor> {
		let token = open_process_token()?;
		let token_user = token_user(token);
		unsafe {
			CloseHandle(token);
		}
		let token_user = token_user?;
		let sid = unsafe { (*(token_user.as_ptr().cast::<TOKEN_USER>())).User.Sid };
		let sid_len = unsafe { GetLengthSid(sid) } as usize;
		let acl_len = mem::size_of::<ACL>() + mem::size_of::<ACCESS_ALLOWED_ACE>()
			- mem::size_of::<u32>()
			+ sid_len;
		let acl = unsafe { LocalAlloc(LMEM_FIXED, acl_len) }.cast::<ACL>();
		if acl.is_null() {
			return Err(io::Error::last_os_error());
		}
		if unsafe { InitializeAcl(acl, acl_len as u32, ACL_REVISION) } == 0
			|| unsafe { AddAccessAllowedAce(acl, ACL_REVISION, GENERIC_ALL, sid) } == 0
		{
			unsafe {
				LocalFree(acl.cast::<c_void>());
			}
			return Err(io::Error::last_os_error());
		}
		let mut descriptor = SecurityDescriptor::new()?;
		unsafe {
			descriptor.set_dacl(acl, false)?;
		}
		Ok(descriptor)
	}

	fn open_process_token() -> io::Result<HANDLE> {
		let mut token = ptr::null_mut();
		if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
			return Err(io::Error::last_os_error());
		}
		Ok(token)
	}

	fn token_user(token: HANDLE) -> io::Result<Vec<u8>> {
		let mut required = 0_u32;
		unsafe {
			GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut required);
		}
		if required == 0 {
			return Err(io::Error::last_os_error());
		}
		let mut buffer = vec![0_u8; required as usize];
		if unsafe {
			GetTokenInformation(
				token,
				TokenUser,
				buffer.as_mut_ptr().cast(),
				required,
				&mut required,
			)
		} == 0
		{
			return Err(io::Error::last_os_error());
		}
		Ok(buffer)
	}
}
