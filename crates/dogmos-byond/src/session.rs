use crate::{
	BoundedDogmosClient, ClientError, DogmosClient, BENCHMARK_CALLBACK_CAPACITY,
	BENCHMARK_CONTROL_PAYLOAD, BENCHMARK_REQUEST_TIMEOUT,
};
use dogmos_protocol::{
	BuildIdentity, CapacityLimits, HandshakePayload, OperationKind, DOGMOS_ABI_VERSION,
	DOGMOS_PROTOCOL_VERSION,
};
use std::{
	io::{self, Write},
	path::Path,
	process::{Child, Command, Stdio},
	time::Duration,
};

const REQUEST_WORKER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);

pub(crate) struct ServiceSession {
	pub(crate) client: BoundedDogmosClient,
	service: Child,
	reaped: bool,
	#[cfg(windows)]
	service_job: Option<std::os::windows::io::OwnedHandle>,
}

impl ServiceSession {
	pub(crate) fn request_with_response<T, E>(
		&mut self,
		operation: OperationKind,
		payload: &[u8],
		response_capacity: usize,
		decode: impl FnOnce(&[u8]) -> Result<T, E>,
	) -> Result<T, E>
	where
		E: From<ClientError>,
	{
		match self.client.round_trip(
			operation,
			payload,
			response_capacity,
			BENCHMARK_REQUEST_TIMEOUT,
		) {
			Ok(response) => decode(response),
			Err(error @ (ClientError::RequestTimeout | ClientError::WorkerStopped)) => {
				let process_id = self.service.id();
				let process_state = self.terminate_service();
				Err(ClientError::ServiceProcess {
					source: Box::new(error),
					process_id,
					process_state,
				}
				.into())
			}
			Err(error @ ClientError::Server(dogmos_protocol::ServiceErrorCode::Internal)) => {
				Err(self.with_process_context(error).into())
			}
			Err(error) => Err(error.into()),
		}
	}

	pub(crate) fn request_without_response(
		&mut self,
		operation: OperationKind,
		payload: &[u8],
	) -> Result<(), ClientError> {
		self.request_with_response(operation, payload, 0, |_| Ok(()))
	}

	fn terminate_service(&mut self) -> String {
		#[cfg(windows)]
		self.service_job.take();
		let kill_result = self.service.kill();
		let wait_result = self.service.wait();
		self.reaped = true;
		let process_state = match (kill_result, wait_result) {
			(_, Ok(status)) => format!("terminated ({status})"),
			(Err(error), Err(wait_error)) => {
				format!("termination failed ({error}); wait failed ({wait_error})")
			}
			(Ok(()), Err(error)) => format!("terminated; wait failed ({error})"),
		};
		match self.client.close(REQUEST_WORKER_SHUTDOWN_TIMEOUT) {
			Ok(()) => process_state,
			Err(error) => format!("{process_state}; request worker close failed ({error})"),
		}
	}

	fn with_process_context(&mut self, error: ClientError) -> ClientError {
		let process_id = self.service.id();
		let process_state = match self.service.try_wait() {
			Ok(Some(status)) => {
				self.reaped = true;
				format!("exited ({status})")
			}
			Ok(None) => "running".into(),
			Err(status_error) => format!("unavailable ({status_error})"),
		};
		ClientError::ServiceProcess {
			source: Box::new(error),
			process_id,
			process_state,
		}
	}

	pub(crate) fn is_healthy(&mut self) -> io::Result<bool> {
		if self.client.is_worker_finished() {
			return Ok(false);
		}
		match self.service.try_wait()? {
			Some(_) => {
				self.reaped = true;
				Ok(false)
			}
			None => Ok(true),
		}
	}

	pub(crate) fn shutdown(&mut self) -> eyre::Result<()> {
		self.request_without_response(OperationKind::Shutdown, &[])?;
		let status = self.service.wait()?;
		self.reaped = true;
		self.client.close(REQUEST_WORKER_SHUTDOWN_TIMEOUT)?;
		if !status.success() {
			return Err(eyre::eyre!("dogmosd did not shut down cleanly"));
		}
		Ok(())
	}
}

impl Drop for ServiceSession {
	fn drop(&mut self) {
		if !self.reaped {
			let _ = self.terminate_service();
		}
	}
}

pub(crate) fn start_service_session(service_path: &str) -> eyre::Result<ServiceSession> {
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
			max_pending_continuations: BENCHMARK_CALLBACK_CAPACITY,
			max_frontier_handles: 1_048_576,
			max_stage_work_items: 4096,
			max_reaction_transactions: BENCHMARK_CALLBACK_CAPACITY,
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
		.stderr(Stdio::inherit());
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
	Ok(ServiceSession {
		client,
		service,
		reaped: false,
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

#[cfg(windows)]
pub(crate) fn system_auth_token() -> eyre::Result<[u8; 32]> {
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

#[cfg(not(windows))]
pub(crate) fn system_auth_token() -> eyre::Result<[u8; 32]> {
	use std::{fs, io::Read};

	let mut token = [0_u8; 32];
	fs::File::open("/dev/urandom")?.read_exact(&mut token)?;
	Ok(token)
}
