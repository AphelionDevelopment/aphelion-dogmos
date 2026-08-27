use dogmos_byond::DogmosClient;
use dogmos_protocol::{
	encode_adjacency_batch, encode_lifecycle_batch, AdjacencyMutation, BuildIdentity,
	CapacityLimits, HandshakePayload, LifecycleAction, LifecycleMutation, MixtureSnapshotRequest,
	OperationKind, ScalarValue, SimulationStage, SimulationStageRequest, WireHandle,
	DOGMOS_ABI_VERSION, DOGMOS_PROTOCOL_VERSION, MAX_CONTROL_PAYLOAD, MIXTURE_SNAPSHOT_LEN,
};
use std::{
	error::Error,
	hint::black_box,
	io::Write,
	process::{Child, Command, Stdio},
	time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const DEFAULT_ITERATIONS: usize = 20_000;
const WARMUP_ITERATIONS: usize = 1_000;

struct ChildGuard(Child);

impl Drop for ChildGuard {
	fn drop(&mut self) {
		let _ = self.0.kill();
		let _ = self.0.wait();
	}
}

struct Case {
	name: String,
	operation: OperationKind,
	request: Vec<u8>,
	response_len: usize,
}

fn main() -> Result<(), Box<dyn Error>> {
	let service_path = std::env::var("DOGMOSD_PATH")
		.map_err(|_| "DOGMOSD_PATH must identify the x64 dogmosd executable")?;
	let iterations = std::env::var("DOGMOS_IPC_ITERATIONS")
		.ok()
		.and_then(|value| value.parse().ok())
		.unwrap_or(DEFAULT_ITERATIONS);
	let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
	let endpoint = format!("dogmos-ipc-bench-{pid}-{unique}", pid = std::process::id());
	let service_digest = dogmos_identity::sha256_file(std::path::Path::new(&service_path))?;
	let handshake = benchmark_handshake(service_digest)?;
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

	println!(
		"processes,shim_pid={},service_pid={},iterations={iterations}",
		std::process::id(),
		client.peer().process_id,
	);
	println!("case,request_bytes,response_bytes,iterations,p50_ns,p95_ns,p99_ns,max_ns");
	for case in cases() {
		let case_iterations = if case.operation == OperationKind::SimulationStage {
			iterations.min(500)
		} else {
			iterations
		};
		run_case(&mut client, &case, case_iterations)?;
	}
	client.shutdown()?;
	if !service.0.wait()?.success() {
		return Err("dogmosd did not shut down cleanly".into());
	}
	Ok(())
}

fn run_case(
	client: &mut DogmosClient,
	case: &Case,
	iterations: usize,
) -> Result<(), Box<dyn Error>> {
	let mut response = vec![0_u8; case.response_len];
	let warmup_iterations = if case.operation == OperationKind::SimulationStage {
		50
	} else {
		WARMUP_ITERATIONS
	};
	for _ in 0..warmup_iterations {
		let received = client.round_trip_into(case.operation, &case.request, &mut response)?;
		black_box(&response[..received]);
	}
	let mut samples = Vec::with_capacity(iterations);
	for _ in 0..iterations {
		let started = Instant::now();
		let received = client.round_trip_into(case.operation, &case.request, &mut response)?;
		let elapsed = started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
		black_box(&response[..received]);
		samples.push(elapsed);
	}
	samples.sort_unstable();
	println!(
		"{},{},{},{},{},{},{},{}",
		case.name,
		case.request.len(),
		case.response_len,
		iterations,
		percentile(&samples, 50),
		percentile(&samples, 95),
		percentile(&samples, 99),
		samples.last().copied().unwrap_or(0),
	);
	Ok(())
}

fn percentile(sorted: &[u64], percentile: usize) -> u64 {
	let index = sorted
		.len()
		.saturating_mul(percentile)
		.div_ceil(100)
		.saturating_sub(1)
		.min(sorted.len().saturating_sub(1));
	sorted.get(index).copied().unwrap_or(0)
}

fn cases() -> Vec<Case> {
	let mut cases = vec![
		Case {
			name: "scalar_getter".into(),
			operation: OperationKind::ScalarGet,
			request: vec![0; 8],
			response_len: 8,
		},
		Case {
			name: "scalar_mutator".into(),
			operation: OperationKind::ScalarSet,
			request: vec![0; 24],
			response_len: 0,
		},
		Case {
			name: "two_handle_transfer".into(),
			operation: OperationKind::Transfer,
			request: vec![0; 24],
			response_len: 8,
		},
		Case {
			name: "gas_vector_32".into(),
			operation: OperationKind::GasVector,
			request: vec![0; 8],
			response_len: 260,
		},
		Case {
			name: "adjacency_update".into(),
			operation: OperationKind::AdjacencyUpdate,
			request: vec![0; 24],
			response_len: 0,
		},
	];
	for count in [16_usize, 64, 256, 1024] {
		let lifecycle = (0..count)
			.map(|slot| LifecycleMutation {
				action: LifecycleAction::Register,
				handle: WireHandle {
					slot: slot as u32,
					generation: 1,
				},
			})
			.collect::<Vec<_>>();
		let mut lifecycle_request = Vec::new();
		encode_lifecycle_batch(&lifecycle, &mut lifecycle_request)
			.expect("the benchmark lifecycle batch is within protocol limits");
		cases.push(Case {
			name: format!("mixture_lifecycle_batch_{count}"),
			operation: OperationKind::MixtureLifecycleBatch,
			request: lifecycle_request,
			response_len: 4,
		});

		let adjacency = (0..count)
			.map(|slot| AdjacencyMutation {
				left: WireHandle {
					slot: slot as u32,
					generation: 1,
				},
				right: WireHandle {
					slot: (slot + 1).wrapping_rem(count) as u32,
					generation: 1,
				},
				conductivity: ScalarValue(0.75),
			})
			.collect::<Vec<_>>();
		let mut adjacency_request = Vec::new();
		encode_adjacency_batch(&adjacency, &mut adjacency_request)
			.expect("the benchmark adjacency batch is within protocol limits");
		cases.push(Case {
			name: format!("adjacency_batch_{count}"),
			operation: OperationKind::AdjacencyBatch,
			request: adjacency_request,
			response_len: 4,
		});
	}
	cases.push(Case {
		name: "mixture_snapshot_32_gases".into(),
		operation: OperationKind::MixtureSnapshot,
		request: MixtureSnapshotRequest {
			handle: WireHandle {
				slot: 1,
				generation: 1,
			},
		}
		.encode()
		.to_vec(),
		response_len: MIXTURE_SNAPSHOT_LEN,
	});
	cases.push(Case {
		name: "simulation_stage_1024_mixtures_32_gases".into(),
		operation: OperationKind::SimulationStage,
		request: SimulationStageRequest {
			stage: SimulationStage::ProcessTurfs,
			seconds_per_tick: ScalarValue(0.5),
		}
		.encode()
		.expect("the benchmark stage request is finite")
		.to_vec(),
		response_len: 8,
	});
	for count in [1_usize, 8, 64, 1024] {
		let mut request = Vec::with_capacity(4 + count * 16);
		request.extend_from_slice(&(count as u32).to_le_bytes());
		request.resize(4 + count * 16, 0);
		cases.push(Case {
			name: format!("batch_{count}"),
			operation: OperationKind::Batch,
			request,
			response_len: 4,
		});
	}
	cases.push(Case {
		name: "callback_drain_empty".into(),
		operation: OperationKind::CallbackBatch,
		request: dogmos_protocol::CallbackBatchRequest { max_events: 1024 }
			.encode()
			.to_vec(),
		response_len: dogmos_protocol::CALLBACK_BATCH_HEADER_LEN,
	});
	cases
}

fn benchmark_handshake(service_digest: [u8; 32]) -> Result<HandshakePayload, Box<dyn Error>> {
	let build_metadata = dogmos_identity::BuildMetadata::from_compile_environment()?;
	Ok(HandshakePayload {
		auth_token: [0x7c; 32],
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
		world_nonce: 0x3344_5566_7788_99aa,
	})
}
