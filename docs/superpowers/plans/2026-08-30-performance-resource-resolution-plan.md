# Dogmos Performance and Resource-Management Resolution Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan sequentially. Do not dispatch subagents or parallel agents without explicit user approval.

**Goal:** Restore atomic callback/resource ownership, make 32-bit memory telemetry truthful, recover the measured component-stage allocation regression, and qualify the result without conflating DreamDaemon and `dogmosd` memory.

**Architecture:** Repair correctness at the service/core boundary before optimizing either process. Keep BYOND conversion and I/O lifecycle in `dogmos-byond`, numerical and transactional invariants in `dogmos-core`, service queue ownership in `dogmos-server`, and retained legacy-DLL ownership in the root crate and `auxcallback`. Do not change the wire protocol, transport, dependency graph, generated bindings, root manifests, toolchain, workflows, or release tooling in this plan.

**Tech Stack:** Rust 1.98.0, Cargo with `--locked`, i686 Windows BYOND shim, x64 Windows service, PowerShell verification, repository allocation probes, paired Meridian-Rift DreamMaker/DreamDaemon gates.

**Spec:** `docs/audits/2026-08-30-performance-resource-reaudit.md`

## Global constraints

- Work test-first: demonstrate each focused failure, make the smallest correction, then rerun focused and wider gates.
- Preserve incremental commit across disconnected components. A rejected component must not publish its own state or events; earlier completed components remain committed by the existing contract.
- Critical gameplay callbacks are all-or-nothing. Do not silently drop, truncate, reorder, or defer a batch that the caller was told completed.
- Preserve public DM proc paths and caller-legible errors. No panic may unwind across the BYOND FFI boundary.
- Do not edit protected files: root/workspace `Cargo.toml`, `Cargo.lock`, `.cargo/`, `rust-toolchain.toml`, `.github/workflows/`, dependency/transport choices, artifact/sync tooling, release tooling, Docker files, or deployment scripts.
- Do not hand-edit generated bindings or manifests.
- Preserve unrelated working-tree changes. Leave all plan work uncommitted unless the user explicitly authorizes commits.
- Use the exact pinned toolchain and `--locked` for every Cargo gate.
- Report DreamDaemon private/committed bytes and address-space pressure separately from `dogmosd` private bytes/RSS.

---

## Task 1: Enforce the live callback budget in chunked stages

**Files:**

- Modify: `crates/dogmos-core/src/world.rs`
- Modify: `crates/dogmos-server/src/state.rs`
- Test: the same files' existing test modules

### Step 1: Add the failing server regression

Add `chunked_stage_rejects_event_batch_that_exceeds_remaining_callback_capacity`. Build an event-producing component, pre-fill the general callback queue so remaining capacity is smaller than that component's batch, capture mixture state and stage progress, then call `process_stage_chunk_cancellable`.

Assert:

```rust
assert!(matches!(result, Err(StateError::CallbackBackpressure)));
assert_eq!(state.pending_callback_count(), callbacks_before);
assert_eq!(state.world.pending_event_count(), 0);
assert_eq!(state.world.mixture_snapshot(mixture)?, snapshot_before);
assert_eq!(state.world.stage_progress(), progress_before);
```

Reuse existing fixture helpers and `#[cfg(test)]` accessors rather than adding public observation APIs.

```powershell
cargo +1.98.0 test --locked --target x86_64-pc-windows-msvc -p dogmos-server chunked_stage_rejects_event_batch -- --nocapture
```

Expected before implementation: the component commits and only the available callback prefix is drained.

### Step 2: Add a capacity-aware core chunk API

Add:

```rust
pub fn process_stage_chunk_cancellable_with_event_limit(
	&mut self,
	request: StageChunkRequest,
	event_limit: u32,
	should_cancel: impl FnMut() -> bool,
) -> Result<StageChunkResult, WorldError>
```

Keep `event_limit` outside `StageChunkRequest`; capacity can change between chunks and must not become part of cursor identity. Make the existing method delegate with `self.max_events` for direct-core callers.

Track a per-call ceiling of `self.events.len() + event_limit` and pass it to every stage path that can publish `WorldEvent` values.

### Step 3: Validate before component publication

Change `validate_indexed_transaction` to accept the active event ceiling. Validation must occur before published-mixture state, transaction/event publication, cursor advance, or component counters change:

```rust
let requested = self.events.len().saturating_add(staged_event_count);
if requested > event_ceiling {
	return Err(WorldError::EventCapacityExceeded {
		requested: requested.try_into().unwrap_or(u32::MAX),
		capacity: event_ceiling.try_into().unwrap_or(u32::MAX),
	});
}
```

Preserve rollback to the component checkpoint.

### Step 4: Use the live budget in `dogmos-server`

Call the new core method with the server's calculated `event_limit`. Map `EventCapacityExceeded` to `CallbackBackpressure`. After core success, require that the complete pending event count fits; a truncating drain must not be normal control flow.

Add core/server tests for exact-fit, one-too-small, a later rejected component after an earlier component committed, retry after capacity is available, cancellation, and unchanged stage-conflict behavior.

```powershell
cargo +1.98.0 test --locked --target x86_64-pc-windows-msvc -p dogmos-core stage_component
cargo +1.98.0 test --locked --target x86_64-pc-windows-msvc -p dogmos-server chunked_stage
cargo +1.98.0 test --locked --target i686-pc-windows-msvc -p dogmos-server --test frontier_stages
git diff --check
```

Review for prefix drains, new protocol fields, or publication before validation. Do not commit.

---

## Task 2: Make server callback enqueue a prepared atomic commit

**Files:**

- Modify: `crates/dogmos-core/src/world.rs`
- Modify: `crates/dogmos-server/src/state.rs`
- Test: `crates/dogmos-server/src/state.rs`
- Test: `crates/dogmos-server/tests/control_plane.rs`

### Step 1: Add deterministic failure seams and tests

Under `cfg(test)`, inject a one-shot `AllocationFailed` immediately before destination-queue reservation, continuation reservation, and final callback-batch commit. Do not simulate real OOM.

For general and reaction-scoped callbacks, prove after each injected failure:

- no callback queue changed;
- no continuation was inserted;
- sequence/continuation IDs did not advance;
- no world event was lost;
- the reaction transaction is absent or documented retryable, never half-committed;
- one legal retry produces the complete ordered batch once.

The tests should fail against the current drain/insert/reserve order.

### Step 2: Prepare from borrowed pending events

Add to `DogmosWorld`:

```rust
pub fn pending_events(&self, maximum: u32) -> &[WorldEvent] {
	let count = self.events.len().min(maximum as usize);
	&self.events[..count]
}
```

Add `discard_pending_events(count)` for the final commit. It removes exactly the already-prepared prefix and rejects an out-of-range count. Keep `drain_events_into` for existing callers.

Add a private server value:

```rust
struct PreparedCallbackBatch {
	callbacks: Vec<PendingCallbackEvent>,
	continuations: Vec<(u64, PendingContinuation)>,
	first_sequence: u64,
	next_sequence: u64,
}
```

Move existing scratch vectors into/out of this value; do not allocate fresh vectors per batch.

`prepare_callback_batch` must borrow events without draining, validate scope and transaction, check depth/count/ID/deadline arithmetic, reserve the destination `VecDeque`, continuation map, and scratch vectors, then translate and encode-validate every callback without changing visible state.

### Step 3: Commit with no fallible allocation

`commit_prepared_callback_batch` must only:

1. discard the prepared world-event prefix;
2. insert already-reserved continuations;
3. push into the already-reserved callback queue;
4. advance IDs/sequences;
5. update metrics;
6. return scratch ownership.

No `try_reserve`, checked arithmetic, encoding, or transaction lookup belongs in this section.

Before direct reaction or stage calls mutate core state, reserve for the full `event_limit` and check worst-case ID ranges. If conversion of a core-owned event can fail, validate that invariant at core event construction and cover every event variant; do not replace it with an unchecked `unwrap`.

### Step 4: Verify

```powershell
cargo +1.98.0 test --locked --target x86_64-pc-windows-msvc -p dogmos-server callback_enqueue
cargo +1.98.0 test --locked --target x86_64-pc-windows-msvc -p dogmos-server reaction_transaction
cargo +1.98.0 test --locked --target i686-pc-windows-msvc -p dogmos-server --test control_plane
cargo +1.98.0 test --locked --target i686-pc-windows-msvc -p dogmos-server --test frontier_stages
git diff --check
```

Confirm failure paths retain or explicitly cancel ownership and no event exists in two authoritative stores. Do not commit.

---

## Task 3: Clear reaction ownership at legacy world shutdown

**Files:**

- Modify/test: `src/reaction.rs`
- Modify/test: `src/gas/types.rs`

### Step 1: Add the failing lifecycle test

Under `cfg(test)`, install Rust-side and sentinel BYOND-side entries into `REACTION_VALUES` without live BYOND APIs. Populate `REACTION_INFO`, call the production shutdown helper, and assert both registries are empty/`None`. Rearm, reuse the old identifier, and prove lookup returns only the new entry.

```powershell
cargo +1.98.0 test --locked --target i686-pc-windows-msvc -p dogmos legacy_shutdown_clears_reaction_registries -- --nocapture
```

Expected before implementation: stale entries remain.

### Step 2: Add explicit cleanup

In `src/reaction.rs`:

```rust
pub(crate) fn clear_reaction_values() {
	REACTION_VALUES.with_borrow_mut(|values| values.clear());
}
```

Call it from `destroy_gas_info_structs` after workers stop, on the owning BYOND thread, and set `*REACTION_INFO.write() = None` before rearm.

### Step 3: Verify idempotence

```powershell
cargo +1.98.0 test --locked --target i686-pc-windows-msvc -p dogmos reaction
cargo +1.98.0 test --locked --target i686-pc-windows-msvc -p dogmos shutdown
git diff --check
```

Run the lifecycle twice in one process. Do not fold gas/turf arena release into this task. Do not commit.

---

## Task 4: Make legacy callback ownership telemetry truthful

**Files:**

- Modify/test: `crates/auxcallback/src/lib.rs`
- Modify: `src/gas/types.rs`
- Modify: `src/turfs/katmos.rs`
- Modify: `src/turfs/processing.rs`
- Modify: `src/turfs/superconduct.rs`
- Document: `docs/performance/2026-08-30-callback-queue-pressure-results.md`

### Step 1: Add failing ownership-metric tests

Queue a closure owning a vector with known capacity. Assert current and cumulative bytes increase by at least:

```rust
vector.capacity() * std::mem::size_of::<Element>()
	+ std::mem::size_of::<DeferredFunc>()
```

After drain, current bytes return to the starting value while cumulative bytes remain monotonic. Cover current/high-water/cumulative saturation and cleanup reset. Current code should fail because it counts only the boxed closure pointer.

### Step 2: Queue an ownership envelope

Use:

```rust
struct DeferredCallback {
	callback: DeferredFunc,
	owned_bytes: usize,
}
```

Change repository-owned enqueue calls to:

```rust
pub fn queue_callback(
	callback: DeferredFunc,
	owned_bytes: usize,
) -> Result<(), QueueCallbackError>
```

`owned_bytes` is a lower bound for heap capacity transferred into the closure, excluding allocator metadata and shared/non-owned references. The queue itself adds `size_of::<DeferredFunc>()`.

Remove `byond_callback_sender` only after `rg` confirms no repository caller and compatibility policy permits it. Otherwise replace it with an accounting wrapper and expose `unaccounted_items`; do not leave a raw sender that silently bypasses metrics.

### Step 3: Account every producer

Compute ownership before `move`:

```rust
let owned_bytes = batch.capacity() * std::mem::size_of::<BatchElement>();
auxcallback::queue_callback(Box::new(move || run_batch(batch)), owned_bytes)?;
```

Add nested vector/string capacities with saturating arithmetic. Fixed scalar and BYOND-value captures report zero dynamic bytes. Propagate enqueue errors where the producer already returns `Result`; background workers must publish a fatal/backpressure state for the main thread rather than discard the error.

Expose item and owned-byte current, high-water, cumulative, rejection, and optional unaccounted fields. Names must say `lower_bound`; this is not allocator/process memory.

### Step 4: Verify locally and collect live evidence

```powershell
cargo +1.98.0 test --locked --target i686-pc-windows-msvc -p auxcallback
cargo +1.98.0 test --locked --target i686-pc-windows-msvc -p dogmos callback
cargo +1.98.0 test --locked --target i686-pc-windows-msvc -p dogmos --features turf_processing,katmos,superconductivity
git diff --check
```

Then capture three identical paired Meridian-Rift pressure workloads. Record enqueue/drain counts, item/owned-byte current and high water, DreamDaemon private/committed bytes, callback event counts, and gameplay equivalence in the new results file.

### Step 5: Mandatory policy checkpoint

Stop if live evidence and the paired DM failure behavior are unavailable. Do not invent a capacity and do not change critical-event loss behavior. Present the measured high water and exact DM behavior needed on `QueueCallbackError` for explicit user approval before Task 5.

---

## Task 5: Bound legacy callback admission without dropping critical events

**Prerequisite:** Task 4 live results and explicit approval of the paired DM backpressure/fatal policy.

**Files:**

- Modify/test: `crates/auxcallback/src/lib.rs`
- Modify: the Task 4 producers and the smallest existing root DM error/metric binding
- Paired external work: Meridian-Rift DM handling, only under separate repository authorization

### Step 1: Configure evidence-derived limits

Add `configure_callback_limits(max_items, max_owned_bytes)` to legacy initialization. Values come from the approved Task 4 evidence, not a guessed constant. Reject zero, overflow, or reconfiguration while the queue is live.

### Step 2: Reserve all-or-nothing

Reserve bytes with atomic compare/exchange before `try_send` to a bounded channel. Release the reservation on send failure and when the envelope is drained/dropped. Return:

```rust
pub enum QueueCallbackError {
	ShuttingDown,
	ItemCapacity { current: usize, capacity: usize },
	OwnedByteCapacity { requested: usize, current: usize, capacity: usize },
}
```

One logical closure batch is admitted completely or rejected before ownership transfer. Never enqueue a prefix.

### Step 3: Stop the producing stage on rejection

Every producer propagates or publishes the typed rejection. Background stage workers stop the current stage and store a fatal/backpressure result for the main thread; they do not continue authoritative atmosphere mutation after a critical callback rejection. The paired DM side must make the failure visible and enter its approved recovery path.

Test exact-limit acceptance, one-item/one-byte overflow, no reservation leak, shutdown races, world-reuse reset, zero prefix delivery, and paired DM recovery. Repeat the identical live series; accept only bounded high water, exact event counts, no silent loss, and no DreamDaemon memory regression outside the declared budget.

Record selected limits and their evidence. Do not commit.

---

## Task 6: Remove remaining successful-path shim label allocations

**Files:**

- Modify/test: `crates/dogmos-byond/src/lib.rs`

### Step 1: Add exact error and allocation regressions

Use a callback value and snapshot gas value that are finite as `f64` but overflow when narrowed to `f32`. Preserve exact index context (`callback value 3`, `mixture snapshot gas 17`). Add an existing-style allocation probe around a full valid callback batch and 32-gas snapshot; current success paths should show indexed label allocations.

### Step 2: Format only on failure

Add a helper equivalent to:

```rust
fn finite_indexed_byond_scalar(
	value: f64,
	prefix: &'static str,
	index: usize,
) -> eyre::Result<f32> {
	let narrowed = value as f32;
	if narrowed.is_finite() {
		return Ok(narrowed);
	}
	Err(eyre::eyre!("{prefix} {index} must be finite"))
}
```

Match current exact wording if it differs. Replace both `&format!` loops without weakening finite validation.

```powershell
cargo +1.98.0 test --locked --target i686-pc-windows-msvc -p dogmos-byond callback_value
cargo +1.98.0 test --locked --target i686-pc-windows-msvc -p dogmos-byond mixture_snapshot
cargo +1.98.0 test --locked --target i686-pc-windows-msvc -p dogmos-byond
git diff --check
```

Do not commit.

---

## Task 7: Measure and recover the component-stage allocation regression

**Files:**

- Modify/test: `crates/dogmos-core/src/world.rs`
- Modify: `crates/dogmos-perf/examples/core_stage_allocations.rs`
- Document: a dated follow-up under `docs/performance/`

### Step 1: Report peak active component storage

Sample `reusable_workset_bytes` after every chunk and record the maximum for each CSV row. Separate post-stage retained lower bound from peak active lower bound. Include `stage_component_turfs` capacity when present and add a unit test proving it is counted once. Keep maps, sets, and allocator metadata explicitly excluded.

Capture three current-head controls before optimizing.

### Step 2: Remove the queue clone

Move the allocation into the temporary field:

```rust
self.stage_component_turfs = Some(std::mem::take(&mut state.queue));
```

Restore it on every success, cancellation, and error path:

```rust
state.queue = self
	.stage_component_turfs
	.take()
	.expect("component stage owns its turf queue");
```

Clear it only after successful publication. Add a cancellation test proving queue/cursor restoration.

### Step 3: Replace the per-entry ordered set

Replace `published_mixtures: BTreeSet<MixtureHandle>` with slot-indexed generation state:

```rust
published_generation_by_slot: Vec<u32>
```

Use zero as empty only after proving generations are non-zero; otherwise use `Vec<Option<u32>>`. Mark a slot only after complete transaction/event validation. The structure is membership only; keep deterministic transaction publication order.

Test disconnected components sharing a mutable handle, slot reuse with a new generation, event-capacity rejection before marking, and cancellation before/after one component commit.

### Step 4: Run identical candidates and IPC controls

```powershell
1..3 | ForEach-Object {
	cargo +1.98.0 run --release --locked -p dogmos-perf --example core_stage_allocations -- --output "tmp/dogmos-perf/component-recovery-candidate-$_.csv"
}
Get-FileHash tmp/dogmos-perf/component-recovery-candidate-*.csv -Algorithm SHA256
```

Acceptance:

- candidate CSVs are byte-identical;
- current-control transcript hashes and work/event counts match exactly;
- the six 100,000-turf equalize/excited rows are no worse in allocated bytes than the qualified `e9f5726` artifact, or the irreducible correctness cost is documented and explicitly approved;
- allocation count, peak active lower bound, and retained lower bound are separate;
- a same-session three-control/three-candidate IPC series stays within the existing 5% p50/p95/p99 budget.

Review generation safety and all cross-component correctness tests. Do not commit.

---

## Task 8: Close worker and arena teardown ownership

**Files:**

- Modify/test: `crates/dogmos-byond/src/client.rs`
- Modify: `crates/dogmos-byond/src/session.rs`
- Test: `crates/dogmos-byond/tests/bounded_io.rs`
- Modify/test: `src/turfs/superconduct.rs`
- Modify/test: `src/gas.rs`
- Modify/test: `src/turfs.rs`

### Step 1: Add lifecycle tests first

Repeat client timeout/terminate cycles and assert the worker is finished after explicit close. On Windows, sample handle count over a bounded series and require cleanup back to the starting range without brittle scheduler timing.

Initialize gas/turf/heat state, create representative capacity, shut down, and assert owners are `None` and worker-running is false before rearm.

### Step 2: Add explicit bounded client close

Add:

```rust
pub fn close(&mut self, timeout: Duration) -> Result<(), ClientError>
```

It closes the request sender, cancels pending I/O, and joins after the service pipe/process is interruptible. `ServiceSession::terminate_service` kills/waits the child, then calls `client.close(REQUEST_WORKER_SHUTDOWN_TIMEOUT)`. Add a distinct timeout error; never block forever. `Drop` remains best-effort and non-panicking.

### Step 3: Own the heat thread

Replace the indefinitely blocked Rayon task with a named `std::thread::JoinHandle` stored by the heat owner. Send an explicit shutdown signal, wake/close the receiver, poll `is_finished` to the existing five-second deadline, then join. Return a shutdown error to the FFI boundary instead of panicking.

Do not join while holding gas, turf, heat, or task locks.

### Step 4: Release arenas instead of clearing them

At shutdown:

```rust
GAS_MIXTURES.write().take();
NEXT_GAS_IDS.write().take();
TURF_GASES.write().take();
PLANETARY_ATMOS.write().take();
```

Reset metrics according to their existing semantics. Explicit preparation recreates owners before registration. During the stopped interval, return caller-legible not-initialized errors instead of unwrap panics.

### Step 5: Verify repeated reuse

```powershell
cargo +1.98.0 test --locked --target i686-pc-windows-msvc -p dogmos shutdown
cargo +1.98.0 test --locked --target i686-pc-windows-msvc -p dogmos world_reuse
cargo +1.98.0 test --locked --target i686-pc-windows-msvc -p dogmos-byond --test bounded_io
git diff --check
```

Run a paired DreamDaemon hard-restart/reuse probe and record DreamDaemon private/committed bytes before shutdown, after shutdown, and after rearm. Do not substitute `dogmosd` RSS. Do not commit.

---

## Task 9: Remove O(free slots x turfs) gas reuse only if measured

**Prerequisite:** A representative trace/profile identifies `register_mix`/`gas_mix_is_referenced` as material. Otherwise document this task as skipped and do not add synchronization state.

**Files:**

- Modify/test: `src/gas.rs`
- Modify/test: `src/turfs.rs`

### Step 1: Prove the current scaling

Under `cfg(test)` or existing diagnostics, count free candidates and turf nodes examined. Construct many referenced free slots followed by one reusable slot and show work grows with candidates times turfs.

### Step 2: Maintain authoritative slot references

Store one reference count per gas slot in the turf owner. Update it in the same authoritative section as new turf insertion, mixture replacement, turf removal, shutdown, and rearm. Use checked increments/decrements and fail closed on underflow/out-of-range slots.

`register_mix` reuses only slots with zero references. Replace the linear `NEXT_GAS_IDS.contains` only if the same profile shows duplicate-unregister checks matter.

Test replacement, last-reference removal, duplicate unregister, invalid slot, shutdown/rearm, and reuse only after the last turf detaches. The scale test must become O(candidates) reference checks with no turf scan.

Run focused i686 tests and the paired registration workload; require exact slot behavior and no DreamDaemon memory regression. Do not commit.

---

## Task 10: Separate frontier upload and committed-storage telemetry

**Files:**

- Modify/test: `crates/dogmos-core/src/frontier.rs`
- Modify: `crates/dogmos-core/src/world.rs`
- Modify/test: `crates/dogmos-server/src/state.rs`
- Protected checkpoint: `crates/dogmos-protocol/src/lib.rs` only with exact approval if no existing diagnostic extension point suffices

### Step 1: Add the failing telemetry test

Commit a non-empty frontier and clear upload scratch. Prove upload bytes can return to zero while committed storage remains non-zero. Current telemetry has no field for the latter.

### Step 2: Add a lower-bound committed metric

```rust
pub(crate) fn committed_storage_bytes_lower_bound(&self) -> u64 {
	(self.committed.capacity() * std::mem::size_of::<TurfHandle>()
		+ self.committed_set.capacity() * std::mem::size_of::<TurfHandle>()) as u64
}
```

Document omitted hash bucket/control bytes and allocator metadata. Expose committed length and capacities where practical. Keep `frontier_upload_bytes` unchanged in meaning.

If no existing diagnostics field carries this without protocol/layout change, keep it in server/core diagnostics and request exact approval before protocol or generated-binding work. Do not silently bump the protocol.

Run core/server telemetry tests and the maintained IPC benchmark. Confirm frontier ordering and canonical hashes are unchanged. Do not commit.

---

## Task 11: Full qualification and evidence handoff

**Files:**

- Update: dated implementation results under `docs/performance/`
- Append: a dated resolution-status section to `docs/audits/2026-08-30-performance-resource-reaudit.md`; do not rewrite the original findings

### Step 1: Run complete local gates

```powershell
rustc +1.98.0 --version
cargo +1.98.0 fmt --all -- --check
cargo +1.98.0 clippy --workspace --locked --target i686-pc-windows-msvc --all-targets -- -D warnings
cargo +1.98.0 test --workspace --locked --target i686-pc-windows-msvc
cargo +1.98.0 test --locked --target x86_64-pc-windows-msvc -p dogmos-core -p dogmos-protocol -p dogmos-server -p dogmos-process-metrics -p dogmos-identity
tools/check_feature_matrix.ps1 -Target i686-pc-windows-msvc
python -m unittest discover -s tools/tests -p 'test_*.py'
tools/benchmark_ipc.ps1 -Iterations 20000 -Repetitions 3
git diff --check
git status --short
```

Run generated-binding drift and paired-artifact checks through the maintained tools in `docs/agent/verification.md`. Do not regenerate protected outputs without exact approval.

### Step 2: Run same-session controls and candidates

Capture three controls and three candidates with identical toolchain, features, source fingerprints, inputs, and machine state. Report p50/p95/p99/max plus transcript, numerical, work, and event equivalence. Do not use only an older run as the control.

### Step 3: Run paired Meridian-Rift gates

Use PowerShell only for DreamMaker/DreamDaemon builds and runtime gates. Record:

- exact revisions, paired artifact hashes, and bitness;
- DreamMaker process, artifact, and log evidence;
- DreamDaemon boot and full-suite outcomes;
- exact map, seed, BYOND version, configuration, workload hash, operation/work/event counts;
- three identical control and three identical candidate live runs;
- DreamDaemon private/committed bytes and address-space high water;
- `dogmosd` private bytes/RSS separately;
- callback item/owned-byte current/high-water and rejection counts;
- stage p50/p95/p99/max and SSair headroom;
- numerical and gameplay-event equivalence.

The production target remains at least 70% lower DreamDaemon Dogmos-attributable peak private bytes. Do not add 64-bit service RSS to DreamDaemon memory.

### Step 4: Final status

Mark every audit finding resolved, skipped by evidence, or externally blocked. List every unrun gate. Leave the worktree uncommitted and present the diff/results for user review.
