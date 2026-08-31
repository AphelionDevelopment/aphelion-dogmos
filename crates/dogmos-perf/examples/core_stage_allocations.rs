use dogmos_core::{
	metadata::{GasFireRole, GasId, GasMetadata, TurfHandle},
	world::{
		AdjacencyMutation, DogmosWorld, LifecycleAction, LifecycleMutation, MixtureStateMutation,
		StageChunkRequest, TurfAdjacencyMutation, TurfHeatAdjacencyMutation, TurfHeatMutation,
		TurfHeatState, TurfLifecycleMutation, WorldEvent, WorldStage,
	},
	MixtureHandle, MAX_GAS_SLOTS,
};
use std::{
	alloc::{GlobalAlloc, Layout, System},
	error::Error,
	fmt::Write as _,
	fs,
	path::PathBuf,
	sync::atomic::{AtomicU64, Ordering},
};

const TURF_COUNTS: [usize; 3] = [1_000, 10_000, 100_000];
const TOPOLOGIES: [Topology; 3] = [Topology::Corridor, Topology::Grid, Topology::Multiz];
const STAGES: [WorldStage; 4] = [
	WorldStage::ProcessTurfs,
	WorldStage::TurfHeat,
	WorldStage::Equalize,
	WorldStage::ExcitedGroups,
];
const WORLD_BYTE_BUDGET: u64 = 8 * 1024 * 1024 * 1024;
const STAGE_WORK_LIMIT: u32 = 4096;

struct CountingAllocator;

static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static DEALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);
static DEALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);

#[global_allocator]
static GLOBAL_ALLOCATOR: CountingAllocator = CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
	unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
		ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
		ALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
		unsafe { System.alloc(layout) }
	}

	unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
		DEALLOCATIONS.fetch_add(1, Ordering::Relaxed);
		DEALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
		unsafe { System.dealloc(pointer, layout) }
	}

	unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
		DEALLOCATIONS.fetch_add(1, Ordering::Relaxed);
		DEALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
		ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
		ALLOCATED_BYTES.fetch_add(new_size as u64, Ordering::Relaxed);
		unsafe { System.realloc(pointer, layout, new_size) }
	}
}

#[derive(Clone, Copy)]
enum Topology {
	Corridor,
	Grid,
	Multiz,
}

impl Topology {
	fn name(self) -> &'static str {
		match self {
			Self::Corridor => "corridor",
			Self::Grid => "grid",
			Self::Multiz => "multiz",
		}
	}
}

#[derive(Clone, Copy)]
struct AllocationSnapshot {
	allocations: u64,
	deallocations: u64,
	allocated_bytes: u64,
	deallocated_bytes: u64,
}

struct AllocationRecord {
	stage: &'static str,
	topology: &'static str,
	turf_count: usize,
	allocation: AllocationSnapshot,
	work_items: u64,
	transcript_hash: u64,
	peak_active_vec_capacity_bytes_lower_bound: u64,
	post_stage_retained_vec_capacity_bytes_lower_bound: u64,
}

fn main() -> Result<(), Box<dyn Error>> {
	let output_path = output_path()?;
	let mut records = Vec::new();
	for turf_count in TURF_COUNTS {
		for topology in TOPOLOGIES {
			for stage in STAGES {
				let mut world = build_world(turf_count, topology)?;
				reset_allocation_counters();
				let (work_items, peak_active_vec_capacity_bytes_lower_bound) =
					run_stage(&mut world, stage)?;
				let allocation = allocation_snapshot();
				let post_stage_retained_vec_capacity_bytes_lower_bound =
					world.reusable_workset_bytes();
				let transcript_hash =
					transcript_hash(&mut world, stage, topology, turf_count, work_items)?;
				records.push(AllocationRecord {
					stage: stage_name(stage),
					topology: topology.name(),
					turf_count,
					allocation,
					work_items,
					transcript_hash,
					peak_active_vec_capacity_bytes_lower_bound,
					post_stage_retained_vec_capacity_bytes_lower_bound,
				});
			}
		}
	}
	write_records(output_path, &records)?;
	Ok(())
}

fn output_path() -> Result<PathBuf, Box<dyn Error>> {
	let mut arguments = std::env::args_os().skip(1);
	if arguments.next().as_deref() != Some(std::ffi::OsStr::new("--output")) {
		return Err("usage: core_stage_allocations --output <path>".into());
	}
	let path = arguments.next().ok_or("--output requires a path")?;
	if arguments.next().is_some() {
		return Err("unexpected trailing arguments".into());
	}
	Ok(path.into())
}

fn reset_allocation_counters() {
	ALLOCATIONS.store(0, Ordering::Relaxed);
	DEALLOCATIONS.store(0, Ordering::Relaxed);
	ALLOCATED_BYTES.store(0, Ordering::Relaxed);
	DEALLOCATED_BYTES.store(0, Ordering::Relaxed);
}

fn allocation_snapshot() -> AllocationSnapshot {
	AllocationSnapshot {
		allocations: ALLOCATIONS.load(Ordering::Relaxed),
		deallocations: DEALLOCATIONS.load(Ordering::Relaxed),
		allocated_bytes: ALLOCATED_BYTES.load(Ordering::Relaxed),
		deallocated_bytes: DEALLOCATED_BYTES.load(Ordering::Relaxed),
	}
}

fn mixture(slot: usize) -> MixtureHandle {
	MixtureHandle {
		slot: slot as u32,
		generation: 1,
	}
}

fn turf(slot: usize) -> TurfHandle {
	TurfHandle {
		slot: slot as u32,
		generation: 1,
	}
}

fn build_world(turf_count: usize, topology: Topology) -> Result<DogmosWorld, Box<dyn Error>> {
	let mut world = DogmosWorld::new_with_event_capacity(WORLD_BYTE_BUDGET, turf_count as u32);
	world.install_gases(vec![GasMetadata {
		id: GasId(0),
		key: "benchmark".into(),
		name: "Benchmark gas".into(),
		flags: 0,
		specific_heat: 20.0,
		fusion_power: 0.0,
		moles_visible: None,
		enthalpy: 0.0,
		fire_radiation_released: 0.0,
		fire_role: GasFireRole::None,
		fire_products: None,
	}])?;
	let mixtures = (0..turf_count)
		.map(|slot| LifecycleMutation {
			action: LifecycleAction::Register,
			handle: mixture(slot),
		})
		.collect::<Vec<_>>();
	world.apply_lifecycle(&mixtures)?;
	let states = (0..turf_count)
		.map(|slot| {
			let mut gases = [0.0; MAX_GAS_SLOTS];
			gases[0] = 5.0 + (slot % 17) as f32;
			MixtureStateMutation {
				handle: mixture(slot),
				expected_revision: 0,
				temperature: 273.15 + (slot % 80) as f32,
				volume: 2500.0,
				gases,
			}
		})
		.collect::<Vec<_>>();
	world.apply_mixture_state(&states)?;
	let turfs = (0..turf_count)
		.map(|slot| TurfLifecycleMutation::Register {
			handle: turf(slot),
			mixture: Some(mixture(slot)),
		})
		.collect::<Vec<_>>();
	world.apply_turf_lifecycle(&turfs)?;
	let heat = (0..turf_count)
		.map(|slot| TurfHeatMutation {
			handle: turf(slot),
			state: Some(TurfHeatState {
				temperature: 273.15 + (slot % 80) as f32,
				thermal_conductivity: 0.05,
				heat_capacity: 20_000.0,
				adjacent_to_space: slot % 97 == 0,
			}),
		})
		.collect::<Vec<_>>();
	world.apply_turf_heat(&heat)?;
	let edges = topology_edges(topology, turf_count);
	let mixture_edges = edges
		.iter()
		.map(|&(left, right)| AdjacencyMutation {
			left: mixture(left),
			right: mixture(right),
			conductivity: 0.75,
		})
		.collect::<Vec<_>>();
	world.apply_adjacency(&mixture_edges)?;
	let turf_edges = edges
		.iter()
		.map(|&(left, right)| TurfAdjacencyMutation {
			left: turf(left),
			right: turf(right),
			connected: true,
		})
		.collect::<Vec<_>>();
	world.apply_turf_adjacency(&turf_edges)?;
	let heat_edges = edges
		.into_iter()
		.map(|(left, right)| TurfHeatAdjacencyMutation {
			left: turf(left),
			right: turf(right),
			connected: true,
		})
		.collect::<Vec<_>>();
	world.apply_turf_heat_adjacency(&heat_edges)?;
	world.begin_frontier(1, turf_count as u32)?;
	let frontier = (0..turf_count).map(turf).collect::<Vec<_>>();
	world.append_frontier(1, 0, &frontier)?;
	world.commit_frontier(1)?;
	Ok(world)
}

fn topology_edges(topology: Topology, turf_count: usize) -> Vec<(usize, usize)> {
	let mut edges = Vec::new();
	match topology {
		Topology::Corridor => {
			for slot in 1..turf_count {
				edges.push((slot - 1, slot));
			}
		}
		Topology::Grid => {
			let width = (turf_count as f64).sqrt().ceil() as usize;
			for slot in 0..turf_count {
				if slot % width + 1 < width && slot + 1 < turf_count {
					edges.push((slot, slot + 1));
				}
				if slot + width < turf_count {
					edges.push((slot, slot + width));
				}
			}
		}
		Topology::Multiz => {
			let layer_size = turf_count.div_ceil(3);
			for slot in 0..turf_count {
				if slot % layer_size + 1 < layer_size && slot + 1 < turf_count {
					edges.push((slot, slot + 1));
				}
				if slot + layer_size < turf_count {
					edges.push((slot, slot + layer_size));
				}
			}
		}
	}
	edges
}

fn run_stage(world: &mut DogmosWorld, stage: WorldStage) -> Result<(u64, u64), Box<dyn Error>> {
	let request = StageChunkRequest {
		stage,
		frontier_epoch: 1,
		stage_epoch: 1,
		work_limit: STAGE_WORK_LIMIT,
		seconds_per_tick: 0.5,
	};
	let mut work_items = 0_u64;
	let mut peak_active_vec_capacity_bytes_lower_bound = world.reusable_workset_bytes();
	loop {
		let result = world.process_stage_chunk_cancellable(request, || false)?;
		work_items += u64::from(result.work_items);
		peak_active_vec_capacity_bytes_lower_bound =
			peak_active_vec_capacity_bytes_lower_bound.max(world.reusable_workset_bytes());
		if !result.pending {
			return Ok((work_items, peak_active_vec_capacity_bytes_lower_bound));
		}
	}
}

fn transcript_hash(
	world: &mut DogmosWorld,
	stage: WorldStage,
	topology: Topology,
	turf_count: usize,
	work_items: u64,
) -> Result<u64, Box<dyn Error>> {
	let mut hash = 0xcbf2_9ce4_8422_2325_u64;
	hash_bytes(&mut hash, stage_name(stage).as_bytes());
	hash_bytes(&mut hash, topology.name().as_bytes());
	hash_bytes(&mut hash, &turf_count.to_le_bytes());
	hash_bytes(&mut hash, &work_items.to_le_bytes());
	for slot in 0..turf_count {
		let snapshot = world.snapshot(mixture(slot))?;
		hash_bytes(&mut hash, &snapshot.revision.to_le_bytes());
		hash_bytes(&mut hash, &snapshot.temperature.to_bits().to_le_bytes());
		hash_bytes(&mut hash, &snapshot.volume.to_bits().to_le_bytes());
		for gas in snapshot.gases {
			hash_bytes(&mut hash, &gas.to_bits().to_le_bytes());
		}
		if let Some(state) = world.turf_heat(turf(slot))? {
			hash_bytes(&mut hash, &state.temperature.to_bits().to_le_bytes());
			hash_bytes(
				&mut hash,
				&state.thermal_conductivity.to_bits().to_le_bytes(),
			);
			hash_bytes(&mut hash, &state.heat_capacity.to_bits().to_le_bytes());
			hash_bytes(&mut hash, &[u8::from(state.adjacent_to_space)]);
		}
	}
	let mut events = Vec::new();
	world.drain_events_into(u32::MAX, &mut events);
	for event in events {
		hash_event(&mut hash, event)?;
	}
	Ok(hash)
}

fn hash_event(hash: &mut u64, event: WorldEvent) -> Result<(), std::fmt::Error> {
	let mut encoded = String::new();
	write!(&mut encoded, "{event:?}")?;
	hash_bytes(hash, encoded.as_bytes());
	Ok(())
}

fn hash_bytes(hash: &mut u64, bytes: &[u8]) {
	for byte in bytes {
		*hash ^= u64::from(*byte);
		*hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
	}
}

fn stage_name(stage: WorldStage) -> &'static str {
	match stage {
		WorldStage::ProcessTurfs => "process_turfs",
		WorldStage::Equalize => "equalize",
		WorldStage::ExcitedGroups => "excited_groups",
		WorldStage::TurfHeat => "turf_heat",
		WorldStage::React => "react",
	}
}

fn write_records(path: PathBuf, records: &[AllocationRecord]) -> Result<(), Box<dyn Error>> {
	if let Some(parent) = path.parent() {
		fs::create_dir_all(parent)?;
	}
	let mut output = String::from(
		"stage,topology,turf_count,allocations,deallocations,allocated_bytes,deallocated_bytes,work_items,transcript_hash,peak_active_vec_capacity_bytes_lower_bound,post_stage_retained_vec_capacity_bytes_lower_bound\n",
	);
	for record in records {
		writeln!(
			output,
			"{},{},{},{},{},{},{},{},{:016x},{},{}",
			record.stage,
			record.topology,
			record.turf_count,
			record.allocation.allocations,
			record.allocation.deallocations,
			record.allocation.allocated_bytes,
			record.allocation.deallocated_bytes,
			record.work_items,
			record.transcript_hash,
			record.peak_active_vec_capacity_bytes_lower_bound,
			record.post_stage_retained_vec_capacity_bytes_lower_bound,
		)?;
	}
	fs::write(path, output)?;
	Ok(())
}
