#![deny(unsafe_op_in_unsafe_fn)]

mod ffi;

use byondapi::prelude::ByondValue;
use dogmos_protocol::{
	encode_adjacency_batch, encode_lifecycle_batch, encode_mixture_state_batch, read_frame_into,
	write_frame, AdjacencyMutation, BuildIdentity, CallbackBatchHeader, CallbackBatchRequest,
	CallbackEvent, CapacityLimits, HandshakePayload, LifecycleAction, LifecycleMutation,
	MixtureSnapshot, MixtureSnapshotRequest, MixtureStateMutation, OperationKind, ProtocolError,
	ProtocolHeader, ScalarValue, ServiceErrorCode, SimulationStage, SimulationStageRequest,
	SimulationStageResponse, TransportError, WireHandle, CALLBACK_BATCH_HEADER_LEN,
	CALLBACK_EVENT_LEN, DOGMOS_ABI_VERSION, DOGMOS_PROTOCOL_VERSION, FLAG_ERROR,
	HANDSHAKE_PAYLOAD_LEN, MAX_CONTROL_PAYLOAD, MAX_GAS_SLOTS, MIXTURE_SNAPSHOT_LEN,
	SIMULATION_STAGE_RESPONSE_LEN,
};
use interprocess::local_socket::{prelude::*, ConnectOptions, GenericNamespaced, Stream};
use std::{
	fmt,
	io::{self, Write},
	path::Path,
	process::{Child, Command, Stdio},
	sync::mpsc::{self, SyncSender},
	sync::{Mutex, OnceLock},
	thread,
	time::{Duration, Instant},
};

static BENCHMARK_SESSION: Mutex<Option<BenchmarkSession>> = Mutex::new(None);
static BENCHMARK_CLOCK: OnceLock<Instant> = OnceLock::new();
static BENCHMARK_LIFECYCLE_BATCH: OnceLock<Vec<u8>> = OnceLock::new();
static BENCHMARK_STATE_BATCH: OnceLock<Vec<u8>> = OnceLock::new();
static BENCHMARK_ADJACENCY_BATCH: OnceLock<Vec<u8>> = OnceLock::new();
const BENCHMARK_CALLBACK_CAPACITY: u32 = 65_536;
const BENCHMARK_CONTROL_PAYLOAD: usize = 64 * 1024;
const BENCHMARK_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

#[doc(hidden)]
pub fn generate_bindings_file() {
	byondapi::generate_bindings(env!("CARGO_CRATE_NAME"));
}

struct BenchmarkSession {
	client: BoundedDogmosClient,
	service: Child,
	#[cfg(windows)]
	service_job: Option<std::os::windows::io::OwnedHandle>,
}

impl BenchmarkSession {
	fn request(
		&mut self,
		operation: OperationKind,
		payload: &[u8],
		response_capacity: usize,
	) -> Result<Vec<u8>, ClientError> {
		let result = self.client.round_trip(
			operation,
			payload,
			response_capacity,
			BENCHMARK_REQUEST_TIMEOUT,
		);
		if matches!(
			result,
			Err(ClientError::RequestTimeout | ClientError::WorkerStopped)
		) {
			self.terminate_service();
		}
		result
	}

	fn terminate_service(&mut self) {
		#[cfg(windows)]
		self.service_job.take();
		let _ = self.service.kill();
	}
}

#[derive(Debug)]
pub enum ClientError {
	Io(io::Error),
	Protocol(ProtocolError),
	Transport(TransportError),
	ServerBusy,
	Server(ServiceErrorCode),
	ConnectTimeout,
	RequestTimeout,
	WorkerStopped,
}

impl fmt::Display for ClientError {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(formatter, "{self:?}")
	}
}

impl std::error::Error for ClientError {}

impl From<io::Error> for ClientError {
	fn from(error: io::Error) -> Self {
		Self::Io(error)
	}
}

impl From<ProtocolError> for ClientError {
	fn from(error: ProtocolError) -> Self {
		Self::Protocol(error)
	}
}

impl From<TransportError> for ClientError {
	fn from(error: TransportError) -> Self {
		Self::Transport(error)
	}
}

pub struct DogmosClient {
	stream: Stream,
	local: HandshakePayload,
	peer: HandshakePayload,
	next_request_id: u64,
}

impl DogmosClient {
	pub fn connect(
		endpoint: &str,
		local: HandshakePayload,
		timeout: Duration,
	) -> Result<Self, ClientError> {
		let name = endpoint.to_ns_name::<GenericNamespaced>()?;
		let deadline = Instant::now() + timeout;
		let mut stream = loop {
			match ConnectOptions::new().name(name.clone()).connect_sync() {
				Ok(stream) => break stream,
				Err(error) if Instant::now() < deadline => {
					if !matches!(
						error.kind(),
						io::ErrorKind::NotFound
							| io::ErrorKind::ConnectionRefused
							| io::ErrorKind::WouldBlock
					) {
						return Err(ClientError::Io(error));
					}
					thread::sleep(Duration::from_millis(5));
				}
				Err(_) => return Err(ClientError::ConnectTimeout),
			}
		};
		let request = ProtocolHeader::request(
			OperationKind::Handshake,
			1,
			local.world_generation,
			local.world_nonce,
			HANDSHAKE_PAYLOAD_LEN as u32,
			0,
		);
		write_frame(&mut stream, request, &local.encode())?;
		let mut handshake_buffer = [0_u8; HANDSHAKE_PAYLOAD_LEN];
		let (response, response_len) = read_frame_into(&mut stream, &mut handshake_buffer)?;
		if response.flags & FLAG_ERROR != 0 {
			let code = ServiceErrorCode::decode(&handshake_buffer[..response_len])?;
			return match code {
				ServiceErrorCode::Busy => Err(ClientError::ServerBusy),
				other => Err(ClientError::Server(other)),
			};
		}
		response.validate_response_to(&request)?;
		let peer = HandshakePayload::decode(&handshake_buffer[..response_len])?;
		peer.validate_peer(&local)?;

		Ok(Self {
			stream,
			local,
			peer,
			next_request_id: 2,
		})
	}

	pub const fn peer(&self) -> &HandshakePayload {
		&self.peer
	}

	pub fn echo(&mut self, payload: &[u8]) -> Result<Vec<u8>, ClientError> {
		let mut response = vec![0_u8; payload.len()];
		let response_len = self.round_trip_into(OperationKind::Echo, payload, &mut response)?;
		response.truncate(response_len);
		Ok(response)
	}

	pub fn round_trip_into(
		&mut self,
		operation: OperationKind,
		payload: &[u8],
		response_buffer: &mut [u8],
	) -> Result<usize, ClientError> {
		self.round_trip_into_with_deadline(operation, payload, response_buffer, 0)
	}

	pub fn round_trip_into_with_deadline(
		&mut self,
		operation: OperationKind,
		payload: &[u8],
		response_buffer: &mut [u8],
		deadline_ns: u64,
	) -> Result<usize, ClientError> {
		let payload_len = u32::try_from(payload.len()).map_err(|_| {
			ClientError::Protocol(ProtocolError::PayloadTooLarge {
				actual: u32::MAX,
				maximum: MAX_CONTROL_PAYLOAD,
			})
		})?;
		let request = ProtocolHeader::request(
			operation,
			self.take_request_id(),
			self.local.world_generation,
			self.local.world_nonce,
			payload_len,
			deadline_ns,
		);
		write_frame(&mut self.stream, request, payload)?;
		let (response, response_len) = read_frame_into(&mut self.stream, response_buffer)?;
		if response.flags & FLAG_ERROR != 0 {
			let code = ServiceErrorCode::decode(&response_buffer[..response_len])?;
			return Err(ClientError::Server(code));
		}
		response.validate_response_to(&request)?;
		Ok(response_len)
	}

	pub fn shutdown(&mut self) -> Result<(), ClientError> {
		let request = ProtocolHeader::request(
			OperationKind::Shutdown,
			self.take_request_id(),
			self.local.world_generation,
			self.local.world_nonce,
			0,
			0,
		);
		write_frame(&mut self.stream, request, &[])?;
		let mut response_buffer = [];
		let (response, response_len) = read_frame_into(&mut self.stream, &mut response_buffer)?;
		response.validate_response_to(&request)?;
		if response_len != 0 {
			return Err(ClientError::Protocol(ProtocolError::TrailingBytes {
				expected_frame_len: dogmos_protocol::PROTOCOL_HEADER_LEN as u32,
				actual_frame_len: dogmos_protocol::PROTOCOL_HEADER_LEN as u32 + response_len as u32,
			}));
		}
		Ok(())
	}

	pub fn allocate_diagnostic(&mut self, bytes: u64) -> Result<u64, ClientError> {
		let mut response = [0_u8; 8];
		let response_len = self.round_trip_into(
			OperationKind::AllocateDiagnostic,
			&bytes.to_le_bytes(),
			&mut response,
		)?;
		if response_len != response.len() {
			return Err(ClientError::Protocol(ProtocolError::TruncatedPayload {
				expected_frame_len: dogmos_protocol::PROTOCOL_HEADER_LEN as u32 + 8,
				actual_frame_len: dogmos_protocol::PROTOCOL_HEADER_LEN as u32 + response_len as u32,
			}));
		}
		Ok(u64::from_le_bytes(response))
	}

	fn take_request_id(&mut self) -> u64 {
		let request_id = self.next_request_id;
		self.next_request_id = self.next_request_id.wrapping_add(1);
		request_id
	}
}

struct IoWorkerRequest {
	operation: OperationKind,
	payload: Vec<u8>,
	response_capacity: usize,
	deadline_ns: u64,
	reply: SyncSender<Result<Vec<u8>, ClientError>>,
}

pub struct BoundedDogmosClient {
	sender: Option<SyncSender<IoWorkerRequest>>,
	worker: Option<thread::JoinHandle<()>>,
	peer: HandshakePayload,
	canceller: IoCanceller,
}

impl BoundedDogmosClient {
	pub fn new(mut client: DogmosClient) -> Result<Self, ClientError> {
		let peer = *client.peer();
		let io_handle_token = current_io_handle_token(&client)?;
		let (sender, receiver) = mpsc::sync_channel::<IoWorkerRequest>(1);
		let (thread_sender, thread_receiver) = mpsc::sync_channel(1);
		let worker = thread::spawn(move || {
			if thread_sender.send(current_thread_token()).is_err() {
				return;
			}
			while let Ok(request) = receiver.recv() {
				let mut response = vec![0_u8; request.response_capacity];
				let result = client
					.round_trip_into_with_deadline(
						request.operation,
						&request.payload,
						&mut response,
						request.deadline_ns,
					)
					.map(|response_len| {
						response.truncate(response_len);
						response
					});
				if request.reply.send(result).is_err() {
					return;
				}
			}
		});
		let thread_token = thread_receiver
			.recv()
			.map_err(|_| ClientError::WorkerStopped)?;
		let canceller = IoCanceller::new(thread_token, io_handle_token)?;
		Ok(Self {
			sender: Some(sender),
			worker: Some(worker),
			peer,
			canceller,
		})
	}

	pub const fn peer(&self) -> &HandshakePayload {
		&self.peer
	}

	pub fn echo(&mut self, payload: &[u8], timeout: Duration) -> Result<Vec<u8>, ClientError> {
		self.round_trip(OperationKind::Echo, payload, payload.len(), timeout)
	}

	pub fn round_trip(
		&mut self,
		operation: OperationKind,
		payload: &[u8],
		response_capacity: usize,
		timeout: Duration,
	) -> Result<Vec<u8>, ClientError> {
		let Some(sender) = self.sender.as_ref() else {
			return Err(ClientError::WorkerStopped);
		};
		let (reply, response) = mpsc::sync_channel(1);
		let deadline_ns = u64::try_from(timeout.as_nanos()).unwrap_or(u64::MAX);
		sender
			.send(IoWorkerRequest {
				operation,
				payload: payload.to_vec(),
				response_capacity,
				deadline_ns,
				reply,
			})
			.map_err(|_| ClientError::WorkerStopped)?;
		match response.recv_timeout(timeout) {
			Ok(result) => result,
			Err(mpsc::RecvTimeoutError::Timeout) => {
				self.sender.take();
				let _ = self.canceller.cancel();
				Err(ClientError::RequestTimeout)
			}
			Err(mpsc::RecvTimeoutError::Disconnected) => {
				self.sender.take();
				Err(ClientError::WorkerStopped)
			}
		}
	}

	pub fn is_worker_finished(&self) -> bool {
		self.worker
			.as_ref()
			.is_none_or(thread::JoinHandle::is_finished)
	}
}

impl Drop for BoundedDogmosClient {
	fn drop(&mut self) {
		self.sender.take();
		if !self.is_worker_finished() {
			let _ = self.canceller.cancel();
		}
		if self.is_worker_finished() {
			if let Some(worker) = self.worker.take() {
				let _ = worker.join();
			}
		}
	}
}

#[cfg(windows)]
struct IoCanceller {
	thread: std::os::windows::io::OwnedHandle,
	pipe: std::os::windows::io::OwnedHandle,
}

#[cfg(windows)]
impl IoCanceller {
	fn new(thread_id: u32, pipe: std::os::windows::io::OwnedHandle) -> Result<Self, ClientError> {
		use std::os::windows::io::FromRawHandle;
		use windows_sys::Win32::System::Threading::{OpenThread, THREAD_TERMINATE};

		// SAFETY: the thread ID came from the live worker; a successful call returns an owned handle.
		let handle = unsafe { OpenThread(THREAD_TERMINATE, 0, thread_id) };
		if handle.is_null() {
			return Err(ClientError::Io(io::Error::last_os_error()));
		}
		// SAFETY: OpenThread returned a new handle owned by this caller.
		Ok(Self {
			// SAFETY: OpenThread returned a new handle owned by this caller.
			thread: unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(handle) },
			pipe,
		})
	}

	fn cancel(&self) -> io::Result<()> {
		use std::os::windows::io::AsRawHandle;
		use windows_sys::Win32::System::IO::{CancelIoEx, CancelSynchronousIo};

		// SAFETY: the pipe is still owned by the worker while its request is outstanding.
		if unsafe { CancelIoEx(self.pipe.as_raw_handle(), std::ptr::null()) } == 0 {
			let error = io::Error::last_os_error();
			if error.raw_os_error() != Some(windows_sys::Win32::Foundation::ERROR_NOT_FOUND as i32)
			{
				return Err(error);
			}
		}

		// SAFETY: the handle remains live and identifies only the dedicated I/O worker thread.
		if unsafe { CancelSynchronousIo(self.thread.as_raw_handle()) } == 0 {
			let error = io::Error::last_os_error();
			if error.raw_os_error() != Some(windows_sys::Win32::Foundation::ERROR_NOT_FOUND as i32)
			{
				return Err(error);
			}
		}
		Ok(())
	}
}

#[cfg(windows)]
fn current_thread_token() -> u32 {
	// SAFETY: GetCurrentThreadId has no preconditions.
	unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() }
}

#[cfg(windows)]
fn current_io_handle_token(
	client: &DogmosClient,
) -> Result<std::os::windows::io::OwnedHandle, ClientError> {
	use interprocess::TryClone;

	match client.stream.try_clone()? {
		Stream::NamedPipe(stream) => Ok(stream.into()),
	}
}

#[cfg(not(windows))]
struct IoCanceller;

#[cfg(not(windows))]
impl IoCanceller {
	fn new(_thread_id: u32, _pipe: ()) -> Result<Self, ClientError> {
		Ok(Self)
	}

	fn cancel(&self) -> io::Result<()> {
		Ok(())
	}
}

#[cfg(not(windows))]
fn current_thread_token() -> u32 {
	0
}

#[cfg(not(windows))]
fn current_io_handle_token(_client: &DogmosClient) -> Result<(), ClientError> {
	Ok(())
}

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
	*session = Some(start_benchmark_session(&service_path)?);
	Ok(true.into())
}

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

#[auxmacros::bind("/proc/dogmos_ipc_benchmark_lifecycle_batch")]
fn dogmos_ipc_benchmark_lifecycle_batch() -> eyre::Result<ByondValue> {
	let request = BENCHMARK_LIFECYCLE_BATCH.get_or_init(make_benchmark_lifecycle_batch);
	benchmark_counted_command(OperationKind::MixtureLifecycleBatch, request)
}

#[auxmacros::bind("/proc/dogmos_ipc_benchmark_state_batch")]
fn dogmos_ipc_benchmark_state_batch() -> eyre::Result<ByondValue> {
	let request = BENCHMARK_STATE_BATCH.get_or_init(make_benchmark_state_batch);
	benchmark_counted_command(OperationKind::MixtureStateBatch, request)
}

#[auxmacros::bind("/proc/dogmos_ipc_benchmark_adjacency_batch")]
fn dogmos_ipc_benchmark_adjacency_batch() -> eyre::Result<ByondValue> {
	let request = BENCHMARK_ADJACENCY_BATCH.get_or_init(make_benchmark_adjacency_batch);
	benchmark_counted_command(OperationKind::AdjacencyBatch, request)
}

#[auxmacros::bind("/proc/dogmos_ipc_benchmark_simulation_stage")]
fn dogmos_ipc_benchmark_simulation_stage() -> eyre::Result<ByondValue> {
	let request = SimulationStageRequest {
		stage: SimulationStage::ProcessTurfs,
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

#[auxmacros::bind("/proc/dogmos_ipc_benchmark_callback_enqueue")]
fn dogmos_ipc_benchmark_callback_enqueue(count: ByondValue) -> eyre::Result<ByondValue> {
	let request = CallbackBatchRequest {
		max_events: callback_count_from_number(count.get_number()?)?,
	}
	.encode();
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

#[auxmacros::bind("/proc/dogmos_ipc_benchmark_callback_drain")]
fn dogmos_ipc_benchmark_callback_drain(max_events: ByondValue) -> eyre::Result<ByondValue> {
	let request = CallbackBatchRequest {
		max_events: callback_count_from_number(max_events.get_number()?)?,
	}
	.encode();
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
	let mut last_sequence = 0;
	for (index, event_bytes) in response[CALLBACK_BATCH_HEADER_LEN..response_len]
		.as_chunks::<CALLBACK_EVENT_LEN>()
		.0
		.iter()
		.enumerate()
	{
		let event = CallbackEvent::decode(event_bytes)?;
		if index == 0 {
			first_sequence = event.sequence;
		} else if event.sequence != last_sequence + 1 {
			return Err(eyre::eyre!(
				"Dogmos callback sequence skipped from {last_sequence} to {}",
				event.sequence
			));
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

#[auxmacros::bind("/proc/dogmos_ipc_benchmark_clock_microseconds")]
fn dogmos_ipc_benchmark_clock_microseconds() -> eyre::Result<ByondValue> {
	let origin = BENCHMARK_CLOCK.get_or_init(Instant::now);
	Ok((origin.elapsed().as_secs_f32() * 1_000_000.0).into())
}

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

#[auxmacros::bind("/proc/dogmos_ipc_benchmark_stop")]
fn dogmos_ipc_benchmark_stop() -> eyre::Result<ByondValue> {
	let mut session = BENCHMARK_SESSION
		.lock()
		.map_err(|_| eyre::eyre!("Dogmos IPC benchmark session lock is poisoned"))?;
	let mut active = session
		.take()
		.ok_or_else(|| eyre::eyre!("Dogmos IPC benchmark session is not running"))?;
	active.request(OperationKind::AllocateDiagnostic, &0_u64.to_le_bytes(), 8)?;
	active.request(OperationKind::Shutdown, &[], 0)?;
	if !active.service.wait()?.success() {
		return Err(eyre::eyre!("dogmosd did not shut down cleanly"));
	}
	Ok(true.into())
}

fn start_benchmark_session(service_path: &str) -> eyre::Result<BenchmarkSession> {
	let auth_token = system_auth_token()?;
	let service_digest = dogmos_identity::sha256_file(Path::new(service_path))?;
	let build_metadata = dogmos_identity::BuildMetadata::from_compile_environment()?;
	let endpoint = format!(
		"dogmos-byond-bench-{}-{}",
		std::process::id(),
		u64::from_le_bytes(auth_token[..8].try_into().unwrap()),
	);
	let handshake = HandshakePayload {
		auth_token,
		identity: BuildIdentity {
			abi_version: DOGMOS_ABI_VERSION,
			protocol_version: DOGMOS_PROTOCOL_VERSION,
			source_revision: build_metadata.source_revision,
			feature_fingerprint: build_metadata.feature_fingerprint,
			executable_digest: service_digest,
		},
		capacities: CapacityLimits {
			max_control_payload: BENCHMARK_CONTROL_PAYLOAD as u32,
			max_batch_operations: 4096,
			max_callback_events: BENCHMARK_CALLBACK_CAPACITY,
			reserved: 0,
			max_world_bytes: 8 * 1024 * 1024 * 1024,
		},
		process_id: std::process::id(),
		world_generation: 1,
		world_nonce: u64::from_le_bytes(auth_token[8..16].try_into().unwrap()),
	};
	let mut command = Command::new(service_path);
	command
		.arg("--echo-server")
		.arg(&endpoint)
		.stdin(Stdio::piped())
		.stdout(Stdio::null())
		.stderr(Stdio::null());
	configure_service_command(&mut command);
	let mut service = command.spawn()?;
	#[cfg(windows)]
	let service_job = match attach_kill_on_close_job(&service) {
		Ok(job) => job,
		Err(error) => {
			let _ = service.kill();
			let _ = service.wait();
			return Err(error.into());
		}
	};
	if let Err(error) = service
		.stdin
		.take()
		.ok_or_else(|| eyre::eyre!("dogmosd stdin was not piped"))?
		.write_all(&handshake.encode())
	{
		let _ = service.kill();
		let _ = service.wait();
		return Err(error.into());
	}
	let client = match DogmosClient::connect(&endpoint, handshake, Duration::from_secs(5)) {
		Ok(client) => client,
		Err(error) => {
			let _ = service.kill();
			let _ = service.wait();
			return Err(error.into());
		}
	};
	let client = BoundedDogmosClient::new(client)?;
	Ok(BenchmarkSession {
		client,
		service,
		#[cfg(windows)]
		service_job: Some(service_job),
	})
}

#[cfg(windows)]
fn configure_service_command(command: &mut Command) {
	use std::os::windows::process::CommandExt;
	use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

	command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn configure_service_command(_command: &mut Command) {}

#[cfg(windows)]
fn attach_kill_on_close_job(service: &Child) -> io::Result<std::os::windows::io::OwnedHandle> {
	use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
	use windows_sys::Win32::System::JobObjects::{
		AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
		SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
		JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
	};

	// SAFETY: null security attributes and name request an unnamed job with the caller's default
	// security descriptor. The returned handle is checked before ownership is transferred.
	let raw_job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
	if raw_job.is_null() {
		return Err(io::Error::last_os_error());
	}
	// SAFETY: `raw_job` is a newly-created, non-null owned handle and is transferred exactly once.
	let job = unsafe { OwnedHandle::from_raw_handle(raw_job) };
	// SAFETY: this Windows structure is plain integer/pointer data whose documented default is all
	// zero before selecting the one limit flag used below.
	let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
	limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
	// SAFETY: `job` remains live for the call and `limits` points to a correctly-sized initialized
	// JOBOBJECT_EXTENDED_LIMIT_INFORMATION value.
	let configured = unsafe {
		SetInformationJobObject(
			job.as_raw_handle(),
			JobObjectExtendedLimitInformation,
			std::ptr::from_ref(&limits).cast(),
			std::mem::size_of_val(&limits) as u32,
		)
	};
	if configured == 0 {
		return Err(io::Error::last_os_error());
	}
	// SAFETY: both handles are live for the call; the child handle belongs to `service`, while the
	// job handle remains owned by the returned `OwnedHandle`.
	let assigned =
		unsafe { AssignProcessToJobObject(job.as_raw_handle(), service.as_raw_handle()) };
	if assigned == 0 {
		return Err(io::Error::last_os_error());
	}
	Ok(job)
}

fn diagnostic_bytes_from_number(bytes: f32) -> eyre::Result<u64> {
	if !bytes.is_finite() || bytes < 0.0 || bytes > 8.0 * 1024.0 * 1024.0 * 1024.0 {
		return Err(eyre::eyre!(
			"diagnostic bytes are outside the supported range"
		));
	}
	Ok(bytes as u64)
}

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

fn scalar_response_value(response: &[u8], response_len: usize) -> eyre::Result<f32> {
	if response_len != response.len() {
		return Err(eyre::eyre!(
			"Dogmos scalar response was {response_len} bytes, expected {}",
			response.len()
		));
	}
	Ok(f64::from_le_bytes(response.try_into().unwrap()) as f32)
}

#[cfg(windows)]
fn system_auth_token() -> eyre::Result<[u8; 32]> {
	use windows_sys::Win32::Security::Cryptography::{
		BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
	};

	let mut token = [0_u8; 32];
	// SAFETY: the null algorithm handle selects the system RNG, and `token` is a live writable
	// 32-byte buffer for the exact length passed to BCryptGenRandom.
	let status = unsafe {
		BCryptGenRandom(
			std::ptr::null_mut(),
			token.as_mut_ptr(),
			token.len() as u32,
			BCRYPT_USE_SYSTEM_PREFERRED_RNG,
		)
	};
	if status < 0 {
		return Err(eyre::eyre!(
			"BCryptGenRandom failed with NTSTATUS {status:#x}"
		));
	}
	Ok(token)
}

#[cfg(test)]
mod tests {
	use super::{callback_count_from_number, diagnostic_bytes_from_number, scalar_response_value};

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
}
