use dogmos_byond::DogmosClient;
use dogmos_protocol::{
	BuildIdentity, CapacityLimits, HandshakePayload, DOGMOS_ABI_VERSION, DOGMOS_PROTOCOL_VERSION,
	MAX_CONTROL_PAYLOAD,
};
use std::{
	io::Write,
	process::{Child, Command, Stdio},
	time::{Duration, SystemTime, UNIX_EPOCH},
};

pub struct TestService {
	pub client: DogmosClient,
	child: Child,
}

impl Drop for TestService {
	fn drop(&mut self) {
		let _ = self.child.kill();
		let _ = self.child.wait();
	}
}

pub fn start(
	callback_capacity: u32,
	continuation_capacity: u32,
	reaction_transaction_capacity: u32,
) -> TestService {
	let unique = SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.unwrap()
		.as_nanos();
	let endpoint = format!("dogmos-server-test-{}-{unique}", std::process::id());
	let service_path = std::path::Path::new(env!("CARGO_BIN_EXE_dogmosd"));
	let handshake = HandshakePayload {
		auth_token: [0x6d; 32],
		identity: BuildIdentity {
			abi_version: DOGMOS_ABI_VERSION,
			protocol_version: DOGMOS_PROTOCOL_VERSION,
			source_revision: [0x11; 20],
			feature_fingerprint: [0x22; 32],
			executable_digest: dogmos_identity::sha256_file(service_path).unwrap(),
		},
		capacities: CapacityLimits {
			max_control_payload: MAX_CONTROL_PAYLOAD,
			max_batch_operations: 4096,
			max_callback_events: callback_capacity,
			max_pending_continuations: continuation_capacity,
			max_frontier_handles: 4096,
			max_stage_work_items: 4096,
			max_reaction_transactions: reaction_transaction_capacity,
			reserved: 0,
			max_world_bytes: 1024 * 1024,
		},
		process_id: std::process::id(),
		world_generation: 7,
		world_nonce: 0x1234_5678_90ab_cdef,
	};
	let mut child = Command::new(service_path)
		.arg("--echo-server")
		.arg(&endpoint)
		.stdin(Stdio::piped())
		.stdout(Stdio::null())
		.stderr(Stdio::inherit())
		.spawn()
		.unwrap();
	child
		.stdin
		.take()
		.unwrap()
		.write_all(&handshake.encode())
		.unwrap();
	let client = DogmosClient::connect(&endpoint, handshake, Duration::from_secs(5)).unwrap();
	TestService { client, child }
}
