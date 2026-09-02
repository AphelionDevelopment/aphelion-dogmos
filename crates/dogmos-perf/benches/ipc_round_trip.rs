use dogmos_byond::{BoundedDogmosClient, DogmosClient};
use dogmos_protocol::{
	encode_gas_metadata_batch, encode_lifecycle_batch, encode_mixture_snapshot_batch_request,
	encode_mixture_state_batch, encode_turf_adjacency_batch, encode_turf_lifecycle_batch,
	BuildIdentity, CallbackBatchRequest, CallbackScope, CapacityLimits, FrontierAppendRequest,
	FrontierBeginRequest, FrontierCommitRequest, GasMetadataRegistration, HandshakePayload,
	LifecycleAction, LifecycleMutation, MixtureSnapshotRequest, MixtureStateMutation,
	OperationKind, ScalarValue, SimulationStage, SimulationStageRequest, SimulationStageResponse,
	TurfAdjacencyMutation, TurfLifecycleMutation, WireGasFireRole, WireHandle, DOGMOS_ABI_VERSION,
	DOGMOS_PROTOCOL_VERSION, MAX_CONTROL_PAYLOAD, MAX_FRONTIER_APPEND_HANDLES, MAX_GAS_SLOTS,
	MIXTURE_SNAPSHOT_LEN, MIXTURE_SNAPSHOT_RECORD_LEN, SIMULATION_STAGE_RESPONSE_LEN,
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
const SERVICE_MIXTURE_COUNT: usize = 1024;
const SERVICE_GAS_COUNT: usize = 32;
const SERVICE_STAGE_ITERATION_LIMIT: usize = 500;
const SERVICE_STAGE_WORK_LIMIT: u32 = 1024;
const SNAPSHOT_BATCH_HANDLES: usize = 256;
const SNAPSHOT_BATCH_ITERATIONS: usize = 2_000;

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

struct ServiceBenchmarkState {
	frontier_epoch: u64,
	next_stage_epoch: u64,
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
	println!(
		"case,request_bytes,response_bytes,iterations,p50_ns,p95_ns,p99_ns,max_ns,work_items_per_iteration"
	);
	for case in cases() {
		run_case(&mut client, &case, iterations)?;
	}
	let mut service_state = prepare_service_world(&mut client)?;
	run_case(
		&mut client,
		&Case {
			name: "service_mixture_snapshot_32_gases".into(),
			operation: OperationKind::MixtureSnapshot,
			request: MixtureSnapshotRequest { handle: handle(1) }
				.encode()
				.to_vec(),
			response_len: MIXTURE_SNAPSHOT_LEN,
		},
		iterations,
	)?;
	run_service_stage_case(
		&mut client,
		&mut service_state,
		iterations.min(SERVICE_STAGE_ITERATION_LIMIT),
	)?;

	// Re-run the same cases through the bounded wrapper, on the same connection and the same
	// service process, so the two sets of rows are directly comparable.
	let mut bounded = BoundedDogmosClient::new(client)?;
	for case in cases() {
		run_bounded_case(&mut bounded, &case, iterations)?;
	}
	run_bounded_case(
		&mut bounded,
		&Case {
			name: "service_mixture_snapshot_32_gases".into(),
			operation: OperationKind::MixtureSnapshot,
			request: MixtureSnapshotRequest { handle: handle(1) }
				.encode()
				.to_vec(),
			response_len: MIXTURE_SNAPSHOT_LEN,
		},
		iterations,
	)?;
	// The point of the batch: one round trip against SNAPSHOT_BATCH_HANDLES singular ones. Divide
	// this row's p50 by the handle count to compare it against the singular row above.
	{
		let handles = (1..=SNAPSHOT_BATCH_HANDLES).map(handle).collect::<Vec<_>>();
		let mut request = Vec::new();
		encode_mixture_snapshot_batch_request(&handles, &mut request)?;
		run_bounded_case(
			&mut bounded,
			&Case {
				name: format!("service_mixture_snapshot_batch_{SNAPSHOT_BATCH_HANDLES}"),
				operation: OperationKind::MixtureSnapshotBatch,
				request,
				response_len: 4 + SNAPSHOT_BATCH_HANDLES * MIXTURE_SNAPSHOT_RECORD_LEN,
			},
			iterations.min(SNAPSHOT_BATCH_ITERATIONS),
		)?;
	}
	bounded.round_trip(OperationKind::Shutdown, &[], 0, Duration::from_secs(5))?;
	bounded.close(Duration::from_secs(5))?;
	if !service.0.wait()?.success() {
		return Err("dogmosd did not shut down cleanly".into());
	}
	Ok(())
}

/// As `run_case`, but through `BoundedDogmosClient` - the wrapper the shim actually calls.
///
/// The cases above measure the bare pipe round trip, which is not what a DM proc pays: every
/// bound call goes through the bounded client's cancellation path. Measuring both makes the cost
/// of that wrapper visible instead of leaving it attributed to "FFI overhead".
fn run_bounded_case(
	client: &mut BoundedDogmosClient,
	case: &Case,
	iterations: usize,
) -> Result<(), Box<dyn Error>> {
	let timeout = Duration::from_secs(5);
	for _ in 0..WARMUP_ITERATIONS {
		let received =
			client.round_trip(case.operation, &case.request, case.response_len, timeout)?;
		black_box(received);
	}
	let mut samples = Vec::with_capacity(iterations);
	for _ in 0..iterations {
		let started = Instant::now();
		let received =
			client.round_trip(case.operation, &case.request, case.response_len, timeout)?;
		let elapsed = started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
		black_box(received);
		samples.push(elapsed);
	}
	samples.sort_unstable();
	println!(
		"bounded_{},{},{},{},{},{},{},{},0",
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

fn run_case(
	client: &mut DogmosClient,
	case: &Case,
	iterations: usize,
) -> Result<(), Box<dyn Error>> {
	let mut response = vec![0_u8; case.response_len];
	for _ in 0..WARMUP_ITERATIONS {
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
		"{},{},{},{},{},{},{},{},0",
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

fn handle(slot: usize) -> WireHandle {
	WireHandle {
		slot: slot as u32,
		generation: 1,
	}
}

fn validate_count(
	response: [u8; 4],
	expected: usize,
	operation: &str,
) -> Result<(), Box<dyn Error>> {
	let actual = u32::from_le_bytes(response) as usize;
	if actual != expected {
		return Err(format!("{operation} processed {actual} records; expected {expected}").into());
	}
	Ok(())
}

fn prepare_service_world(
	client: &mut DogmosClient,
) -> Result<ServiceBenchmarkState, Box<dyn Error>> {
	let gases = (0..SERVICE_GAS_COUNT)
		.map(|id| GasMetadataRegistration {
			id: id as u16,
			key: format!("gas_{id}"),
			name: format!("Benchmark gas {id}"),
			flags: 0,
			specific_heat: ScalarValue(20.0 + id as f64),
			fusion_power: ScalarValue(0.0),
			moles_visible: None,
			enthalpy: ScalarValue(0.0),
			fire_radiation_released: ScalarValue(0.0),
			fire_role: WireGasFireRole::None,
			fire_products: None,
		})
		.collect::<Vec<_>>();
	let mut request = Vec::new();
	let mut count_response = [0_u8; 4];
	encode_gas_metadata_batch(&gases, &mut request)?;
	client.round_trip_into(
		OperationKind::GasMetadataInstall,
		&request,
		&mut count_response,
	)?;
	validate_count(count_response, SERVICE_GAS_COUNT, "gas metadata install")?;

	let mixtures = (0..SERVICE_MIXTURE_COUNT)
		.map(|slot| LifecycleMutation {
			action: LifecycleAction::Register,
			handle: handle(slot),
		})
		.collect::<Vec<_>>();
	encode_lifecycle_batch(&mixtures, &mut request)?;
	client.round_trip_into(
		OperationKind::MixtureLifecycleBatch,
		&request,
		&mut count_response,
	)?;
	validate_count(count_response, SERVICE_MIXTURE_COUNT, "mixture lifecycle")?;

	for start in (0..SERVICE_MIXTURE_COUNT).step_by(128) {
		let end = (start + 128).min(SERVICE_MIXTURE_COUNT);
		let states = (start..end)
			.map(|slot| {
				let mut gas_values = [ScalarValue(0.0); MAX_GAS_SLOTS];
				gas_values[slot % SERVICE_GAS_COUNT] = ScalarValue(10.0 + slot as f64 / 100.0);
				MixtureStateMutation {
					handle: handle(slot),
					expected_revision: 0,
					temperature: ScalarValue(273.15 + (slot % 80) as f64),
					volume: ScalarValue(2500.0),
					gases: gas_values,
				}
			})
			.collect::<Vec<_>>();
		encode_mixture_state_batch(&states, &mut request)?;
		client.round_trip_into(
			OperationKind::MixtureStateBatch,
			&request,
			&mut count_response,
		)?;
		validate_count(count_response, states.len(), "mixture state seed")?;
	}

	let turfs = (0..SERVICE_MIXTURE_COUNT)
		.map(|slot| TurfLifecycleMutation {
			action: LifecycleAction::Register,
			turf: handle(slot),
			mixture: Some(handle(slot)),
		})
		.collect::<Vec<_>>();
	encode_turf_lifecycle_batch(&turfs, &mut request)?;
	client.round_trip_into(
		OperationKind::TurfLifecycleBatch,
		&request,
		&mut count_response,
	)?;
	validate_count(count_response, SERVICE_MIXTURE_COUNT, "turf lifecycle")?;

	let topology = (0..SERVICE_MIXTURE_COUNT)
		.map(|slot| TurfAdjacencyMutation {
			left: handle(slot),
			right: handle((slot + 1) % SERVICE_MIXTURE_COUNT),
			connected: true,
			firelock: false,
		})
		.collect::<Vec<_>>();
	encode_turf_adjacency_batch(&topology, &mut request)?;
	client.round_trip_into(
		OperationKind::TurfAdjacencyBatch,
		&request,
		&mut count_response,
	)?;
	validate_count(count_response, SERVICE_MIXTURE_COUNT, "turf topology")?;

	let frontier_epoch = 1;
	let mut begin_response = [0_u8; 8];
	client.round_trip_into(
		OperationKind::FrontierBegin,
		&FrontierBeginRequest {
			epoch: frontier_epoch,
			expected_count: SERVICE_MIXTURE_COUNT as u32,
		}
		.encode(),
		&mut begin_response,
	)?;
	if u64::from_le_bytes(begin_response) != frontier_epoch {
		return Err("frontier begin returned the wrong epoch".into());
	}
	let frontier_handles = (0..SERVICE_MIXTURE_COUNT).map(handle).collect::<Vec<_>>();
	for (chunk_index, handles) in frontier_handles
		.chunks(MAX_FRONTIER_APPEND_HANDLES)
		.enumerate()
	{
		client.round_trip_into(
			OperationKind::FrontierAppend,
			&FrontierAppendRequest {
				epoch: frontier_epoch,
				offset: (chunk_index * MAX_FRONTIER_APPEND_HANDLES) as u32,
				handles: handles.to_vec(),
			}
			.encode()?,
			&mut count_response,
		)?;
		validate_count(count_response, handles.len(), "frontier append")?;
	}
	let mut commit_response = [0_u8; 16];
	client.round_trip_into(
		OperationKind::FrontierCommit,
		&FrontierCommitRequest {
			epoch: frontier_epoch,
		}
		.encode(),
		&mut commit_response,
	)?;
	if u64::from_le_bytes(commit_response[0..8].try_into()?) != frontier_epoch
		|| u32::from_le_bytes(commit_response[8..12].try_into()?) as usize != SERVICE_MIXTURE_COUNT
	{
		return Err("frontier commit returned unexpected identity or count".into());
	}

	Ok(ServiceBenchmarkState {
		frontier_epoch,
		next_stage_epoch: 1,
	})
}

fn run_service_stage_iteration(
	client: &mut DogmosClient,
	state: &mut ServiceBenchmarkState,
) -> Result<u32, Box<dyn Error>> {
	let request = SimulationStageRequest {
		stage: SimulationStage::ProcessTurfs,
		frontier_epoch: state.frontier_epoch,
		stage_epoch: state.next_stage_epoch,
		work_limit: SERVICE_STAGE_WORK_LIMIT,
		seconds_per_tick: ScalarValue(0.5),
	}
	.encode()?;
	let mut response = [0_u8; SIMULATION_STAGE_RESPONSE_LEN];
	let mut total_work = 0_u32;
	loop {
		client.round_trip_into(OperationKind::SimulationStage, &request, &mut response)?;
		let result = SimulationStageResponse::decode(&response)?;
		total_work = total_work
			.checked_add(result.work_items)
			.ok_or("stage work count exhausted")?;
		black_box(result);
		if !result.pending {
			break;
		}
	}
	state.next_stage_epoch = state
		.next_stage_epoch
		.checked_add(1)
		.ok_or("stage epoch exhausted")?;
	Ok(total_work)
}

fn run_service_stage_case(
	client: &mut DogmosClient,
	state: &mut ServiceBenchmarkState,
	iterations: usize,
) -> Result<(), Box<dyn Error>> {
	for _ in 0..50 {
		run_service_stage_iteration(client, state)?;
	}
	let mut samples = Vec::with_capacity(iterations);
	let mut expected_work = None;
	for _ in 0..iterations {
		let started = Instant::now();
		let work_items = run_service_stage_iteration(client, state)?;
		let elapsed = started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
		if let Some(expected) = expected_work {
			if work_items != expected {
				return Err(
					format!("service stage work changed from {expected} to {work_items}").into(),
				);
			}
		} else {
			expected_work = Some(work_items);
		}
		samples.push(elapsed);
	}
	samples.sort_unstable();
	println!(
		"service_simulation_stage_1024_mixtures_32_gases,{},{},{},{},{},{},{},{}",
		dogmos_protocol::SIMULATION_STAGE_REQUEST_LEN,
		SIMULATION_STAGE_RESPONSE_LEN,
		iterations,
		percentile(&samples, 50),
		percentile(&samples, 95),
		percentile(&samples, 99),
		samples.last().copied().unwrap_or(0),
		expected_work.unwrap_or(0),
	);
	Ok(())
}

fn cases() -> Vec<Case> {
	let mut cases = vec![
		Case {
			name: "transport_scalar_getter".into(),
			operation: OperationKind::ScalarGet,
			request: vec![0; 8],
			response_len: 8,
		},
		Case {
			name: "transport_scalar_mutator".into(),
			operation: OperationKind::ScalarSet,
			request: vec![0; 24],
			response_len: 0,
		},
		Case {
			name: "transport_two_handle_transfer".into(),
			operation: OperationKind::Transfer,
			request: vec![0; 24],
			response_len: 8,
		},
		Case {
			name: "transport_gas_vector_32".into(),
			operation: OperationKind::GasVector,
			request: vec![0; 8],
			response_len: 260,
		},
		Case {
			name: "transport_adjacency_update".into(),
			operation: OperationKind::AdjacencyUpdate,
			request: vec![0; 24],
			response_len: 0,
		},
	];
	for count in [1_usize, 8, 64, 1024] {
		let mut request = Vec::with_capacity(4 + count * 16);
		request.extend_from_slice(&(count as u32).to_le_bytes());
		request.resize(4 + count * 16, 0);
		cases.push(Case {
			name: format!("transport_batch_{count}"),
			operation: OperationKind::Batch,
			request,
			response_len: 4,
		});
	}
	cases.push(Case {
		name: "transport_callback_drain_empty".into(),
		operation: OperationKind::CallbackBatch,
		request: CallbackBatchRequest {
			max_events: 1024,
			scope: CallbackScope::General,
			transaction_id: 0,
		}
		.encode()
		.expect("the benchmark callback request uses the general scope")
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
			max_pending_continuations: 1024,
			max_frontier_handles: 1_048_576,
			max_stage_work_items: 4096,
			max_reaction_transactions: 1024,
			reserved: 0,
			max_world_bytes: 8 * 1024 * 1024 * 1024,
		},
		process_id: std::process::id(),
		world_generation: 1,
		world_nonce: 0x3344_5566_7788_99aa,
	})
}
