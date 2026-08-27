use dogmos_byond::{ClientError, DogmosClient};
use dogmos_protocol::{
	encode_lifecycle_batch, encode_mixture_state_batch, BuildIdentity, CapacityLimits,
	HandshakePayload, LifecycleAction, LifecycleMutation, MixtureSnapshot, MixtureSnapshotRequest,
	MixtureStateMutation, OperationKind, ScalarValue, WireHandle, DOGMOS_ABI_VERSION,
	DOGMOS_PROTOCOL_VERSION, MAX_CONTROL_PAYLOAD, MAX_GAS_SLOTS, MIXTURE_SNAPSHOT_LEN,
};
use std::{
	error::Error,
	io::Write,
	process::{Child, Command, Stdio},
	time::{Duration, SystemTime, UNIX_EPOCH},
};

struct ChildGuard(Child);

impl Drop for ChildGuard {
	fn drop(&mut self) {
		let _ = self.0.kill();
		let _ = self.0.wait();
	}
}

fn main() -> Result<(), Box<dyn Error>> {
	let mut arguments = std::env::args().skip(1);
	let service_path = arguments
		.next()
		.ok_or("usage: cross_bitness_probe <dogmosd-path>")?;
	let diagnostic_bytes = arguments
		.next()
		.map(|value| value.parse::<u64>())
		.transpose()?
		.unwrap_or(0);
	let hold_milliseconds = arguments
		.next()
		.map(|value| value.parse::<u64>())
		.transpose()?
		.unwrap_or(0);
	let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
	let endpoint = format!(
		"dogmos-cross-bitness-{pid}-{unique}",
		pid = std::process::id()
	);
	let service_digest = dogmos_identity::sha256_file(std::path::Path::new(&service_path))?;
	let handshake = test_handshake(service_digest)?;
	let mut service = ChildGuard(
		Command::new(service_path)
			.arg("--echo-server")
			.arg(&endpoint)
			.stdin(Stdio::piped())
			.stdout(Stdio::null())
			.stderr(Stdio::inherit())
			.spawn()?,
	);
	service
		.0
		.stdin
		.take()
		.ok_or("dogmosd stdin was not piped")?
		.write_all(&handshake.encode())?;

	let mut client = DogmosClient::connect(&endpoint, handshake, Duration::from_secs(5))?;
	let service_pid = client.peer().process_id;
	if diagnostic_bytes != 0 {
		println!(
			"isolation_baseline,shim_pid={},service_pid={service_pid}",
			std::process::id()
		);
		std::io::stdout().flush()?;
		std::thread::sleep(Duration::from_millis(hold_milliseconds));
		let allocated = client.allocate_diagnostic(diagnostic_bytes)?;
		println!(
			"isolation_allocated,shim_pid={},service_pid={service_pid},bytes={allocated}",
			std::process::id()
		);
		std::io::stdout().flush()?;
		std::thread::sleep(Duration::from_millis(hold_milliseconds));
		client.allocate_diagnostic(0)?;
	}
	if client.echo(b"i686-to-x64")? != b"i686-to-x64" {
		return Err("cross-bitness echo payload changed".into());
	}
	let handle = WireHandle {
		slot: 0,
		generation: 1,
	};
	let mut lifecycle_request = Vec::new();
	encode_lifecycle_batch(
		&[LifecycleMutation {
			action: LifecycleAction::Register,
			handle,
		}],
		&mut lifecycle_request,
	)?;
	let mut processed = [0_u8; 4];
	client.round_trip_into(
		OperationKind::MixtureLifecycleBatch,
		&lifecycle_request,
		&mut processed,
	)?;
	let mut gases = [ScalarValue(0.0); MAX_GAS_SLOTS];
	gases[0] = ScalarValue(21.0);
	let mut state_request = Vec::new();
	encode_mixture_state_batch(
		&[MixtureStateMutation {
			handle,
			expected_revision: 0,
			temperature: ScalarValue(293.15),
			volume: ScalarValue(2500.0),
			gases,
		}],
		&mut state_request,
	)?;
	client.round_trip_into(
		OperationKind::MixtureStateBatch,
		&state_request,
		&mut processed,
	)?;
	let mut snapshot = [0_u8; MIXTURE_SNAPSHOT_LEN];
	client.round_trip_into(
		OperationKind::MixtureSnapshot,
		&MixtureSnapshotRequest { handle }.encode(),
		&mut snapshot,
	)?;
	let snapshot = MixtureSnapshot::decode(&snapshot)?;
	if snapshot.revision != 1 || snapshot.gases[0] != ScalarValue(21.0) {
		return Err("cross-bitness mixture state changed".into());
	}
	if !matches!(
		DogmosClient::connect(&endpoint, handshake, Duration::from_secs(1)),
		Err(ClientError::ServerBusy)
	) {
		return Err("dogmosd accepted a second concurrent client".into());
	}
	client.shutdown()?;
	if !service.0.wait()?.success() {
		return Err("dogmosd did not shut down cleanly".into());
	}
	println!(
		"cross-bitness IPC passed: shim_pid={} service_pid={service_pid}",
		std::process::id()
	);
	Ok(())
}

fn test_handshake(service_digest: [u8; 32]) -> Result<HandshakePayload, Box<dyn Error>> {
	let build_metadata = dogmos_identity::BuildMetadata::from_compile_environment()?;
	Ok(HandshakePayload {
		auth_token: [0x6b; 32],
		identity: BuildIdentity {
			abi_version: DOGMOS_ABI_VERSION,
			protocol_version: DOGMOS_PROTOCOL_VERSION,
			source_revision: build_metadata.source_revision,
			feature_fingerprint: build_metadata.feature_fingerprint,
			executable_digest: service_digest,
		},
		capacities: CapacityLimits {
			max_control_payload: MAX_CONTROL_PAYLOAD,
			max_batch_operations: 4096,
			max_callback_events: 1024,
			reserved: 0,
			max_world_bytes: 8 * 1024 * 1024 * 1024,
		},
		process_id: std::process::id(),
		world_generation: 1,
		world_nonce: 0x2233_4455_6677_8899,
	})
}
