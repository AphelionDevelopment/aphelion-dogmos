# Dogmos Performance and Resource Management Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restore trustworthy performance evidence, reduce measured DreamDaemon shim allocation pressure, and remove measured core/service hot-path churn without changing numerical results, event order, rollback, or failure behavior.

**Architecture:** Repair the protocol/release and benchmark baselines first. Add allocation/work evidence at the owning layer, then make small shim and core changes in isolation. Keep DreamDaemon memory acceptance separate from 64-bit `dogmosd` profiling, and defer the transactional component rewrite until lower-risk changes have been re-measured.

**Tech Stack:** Rust 1.98.0, i686/x86_64 MSVC targets, BYOND-facing `dogmos-byond`, `dogmos-protocol`, `dogmos-core`, `dogmos-server`, PowerShell performance tooling, Python contract tests.

**Spec:** `docs/audits/2026-08-30-performance-resource-audit.md`

## Global Constraints

- The user granted standing authorization on 2026-08-30 to execute this plan autonomously to completion, commit each review boundary, and edit the protected files named by this plan for their stated effects. No additional permission prompt is required for those in-scope edits.
- Execute in the existing `aphelion-dogmos` checkout on `master`; the user explicitly authorized commits in this checkout.
- Work only in `aphelion-dogmos`; paired game-repository qualification is an external acceptance gate, not part of these source edits.
- Use Rust 1.98.0 and `--locked` for every Cargo gate.
- Preserve protocol layouts, public DM proc paths, deterministic event order, numerical/event transcripts, atomic stage commit, cancellation rollback, and caller-legible errors.
- No panic may cross the BYOND FFI boundary.
- Generated bindings are regenerated, never hand-edited.
- DreamDaemon private/committed bytes and address-space pressure are the shim memory target; report `dogmosd` memory separately.
- Accept performance changes only from at least three identical controls and three identical candidates with equivalence evidence.
- Create the scoped commits below after their verification gates pass. Do not push unless the user separately requests it.

## File map

- `crates/dogmos-byond/src/session.rs`: borrowed fixed-buffer response lifecycle and timeout poisoning.
- `crates/dogmos-byond/src/lib.rs`: DM conversion, lazy validation context, exact-capacity wire payload construction.
- `crates/dogmos-byond/bindings.dm`: deterministic generated DM contract.
- `crates/dogmos-byond/tests/`: response-allocation and exact-error regression coverage.
- `crates/dogmos-core/src/world.rs`: stage worksets, topology mutation, diffusion, component transactions, telemetry.
- `crates/dogmos-core/src/frontier.rs`: bootstrap duplicate tracking and order-preserving committed frontier.
- `crates/dogmos-core/tests/`: numerical, event-order, rollback, frontier, topology, and work-count gates.
- `crates/dogmos-server/src/state.rs`: wire-to-core translation and service telemetry.
- `crates/dogmos-server/tests/control_plane.rs`: production-like cross-process setup.
- `crates/dogmos-perf/benches/ipc_round_trip.rs`: legal, reproducible transport/service cases.
- `crates/dogmos-perf/examples/core_stage_allocations.rs`: allocation-count and stage-work probe without a new dependency.
- `tools/benchmark_ipc.ps1`: repeated benchmark orchestration and artifact identity.
- `tools/tests/test_dogmos_contract.py`: protocol/canonical contract expectations.
- `docs/performance/`: reviewed control/candidate summaries only; raw output remains ignored under `tmp/`.

---

### Task 1: Restore protocol, generated-binding, and i686 baseline gates

**Status:** Complete in `7e7b513`.

**Files:**
- Modify: `tools/tests/test_dogmos_contract.py`
- Modify by generator: `crates/dogmos-byond/bindings.dm`
- Modify: `crates/dogmos-protocol/tests/compound_commands.rs`
- Modify: `crates/dogmos-server/tests/control_plane.rs`
- Verify only: `dogmos-build-manifest.toml`

**Interfaces:**
- Consumes: `DOGMOS_PROTOCOL_VERSION == 10`, current frontier add/remove exports, metadata registration protocol.
- Produces: deterministic protocol-10 contract tests and a cross-process fixture whose asserted gas count matches installed metadata.

- [ ] **Step 1: Preserve the current red evidence**

Run:

```powershell
python -m unittest tools.tests.test_dogmos_contract tools.tests.test_generated_bindings
cargo +1.98.0 test -p dogmos-server --locked --target i686-pc-windows-msvc --test control_plane cross_process_handshake_echo_single_client_and_shutdown -- --exact --nocapture
```

Expected: contract/binding drift failures and `left: 0, right: 32` in the focused Rust test.

- [ ] **Step 2: Make the control-plane fixture install the metadata it asserts**

Before mixture lifecycle setup, encode and send 32 valid dense gas metadata records using the existing protocol helper. Assert the counted response is 32 before asserting the snapshot:

```rust
let gas_entries = test_gas_metadata(32);
let mut gas_request = Vec::new();
encode_gas_metadata_batch(&gas_entries, &mut gas_request).unwrap();
client
	.round_trip_into(OperationKind::GasMetadataInstall, &gas_request, &mut processed)
	.unwrap();
assert_eq!(u32::from_le_bytes(processed), 32);
```

Extract `test_gas_metadata(count)` next to the fixture's existing setup helpers; use finite positive specific heats, dense IDs, unique keys, and no fire role/products.

- [ ] **Step 3: Regenerate bindings with the maintained generator**

Run from `crates/dogmos-byond` because the maintained generator writes `bindings.dm` relative to its current directory:

```powershell
cargo +1.98.0 run --quiet --locked --target i686-pc-windows-msvc -p dogmos-byond --example generate_bindings
```

Review the diff. It must add the frontier operations deterministically, preserve sorted proc paths and LF endings, and contain no unrelated ABI deletion.

- [ ] **Step 4: Update protocol-10 fixture names and canonical expectations**

Change stale `protocol_v9` test names to `protocol_v10`, assert protocol 10 in `test_dogmos_contract.py`, and replace the canonical-byte SHA-256 only after printing and reviewing the exact canonical JSON generated by the test helper. Do not change `dogmos-build-manifest.toml` unless code and manifest actually disagree.

- [ ] **Step 5: Prove the baseline repair**

Run:

```powershell
python -m unittest discover -s tools/tests -p 'test_*.py'
cargo +1.98.0 test --workspace --locked --target i686-pc-windows-msvc
```

Expected: all tooling tests and all i686 workspace test binaries pass.

- [ ] **Step 6: Suggested commit boundary**

```powershell
git add crates/dogmos-byond/bindings.dm crates/dogmos-protocol/tests/compound_commands.rs crates/dogmos-server/tests/control_plane.rs tools/tests/test_dogmos_contract.py
git commit -m "fix: restore protocol 10 contract gates"
```

### Task 2: Repair and separate IPC transport and service-compute benchmarks

**Status:** Complete in `df35f9a`.

**Files:**
- Modify: `crates/dogmos-perf/benches/ipc_round_trip.rs`
- Modify: `tools/benchmark_ipc.ps1`
- Test: `tools/tests/test_perf_contract.py`
- Modify: `docs/performance/ipc-decision.md`

**Interfaces:**
- Consumes: frontier begin/append/commit, turf lifecycle, mixture state, gas metadata, monotonically identified stage requests.
- Produces: `transport_*` cases that need no world setup and `service_*` cases with explicit setup and valid stage epochs.

- [ ] **Step 1: Add a failing benchmark contract test**

Add a source-level test that requires distinct case families and explicit setup markers:

```python
def test_ipc_benchmark_separates_transport_and_service_cases(self):
    source = (ROOT / "crates/dogmos-perf/benches/ipc_round_trip.rs").read_text()
    self.assertIn('name: "transport_scalar_getter"', source)
    self.assertIn('fn prepare_service_world(', source)
    self.assertIn("FrontierCommit", source)
    self.assertIn("next_stage_epoch", source)
```

Run the named Python test and observe it fail.

- [ ] **Step 2: Add one explicit service-world setup function**

Implement `prepare_service_world(&mut DogmosClient) -> Result<ServiceBenchmarkState, Box<dyn Error>>` that installs gas metadata, registers mixtures and turfs, seeds mixture state, applies topology, begins/appends/commits a frontier, and returns the committed frontier epoch plus next stage epoch. Validate every counted response.

Keep transport-only echo/batch cases independent of that state. Prefix case names with `transport_` or `service_` so summaries cannot combine them accidentally.

- [ ] **Step 3: Generate a valid stage request per iteration**

Replace the immutable `Case.request` model for stateful stages with a case runner. Increment `stage_epoch` only after a completed non-pending stage and keep the same request while draining pending chunks:

```rust
while result.pending {
	result = run_stage_chunk(client, frontier_epoch, stage_epoch, work_limit)?;
}
stage_epoch = stage_epoch.checked_add(1).ok_or("stage epoch exhausted")?;
```

Warmup and measured iterations must use the same prepared topology and must report their stage work counts.

- [ ] **Step 4: Make failed repetitions non-publishable**

Have `tools/benchmark_ipc.ps1` write a per-run status record after successful process exit. The summary step must reject missing status records or fewer than `Repetitions` successful CSVs. Preserve exact shim/service PID memory rows separately.

- [ ] **Step 5: Run a shortened proof and then the full control**

```powershell
& .\tools\benchmark_ipc.ps1 -Iterations 2000 -Repetitions 3
& .\tools\benchmark_ipc.ps1 -Iterations 20000 -Repetitions 3
```

Expected: both finish three runs; every service stage reports the expected work count and no `StageConflict`. Check in only a reviewed summary with revision/toolchain/case identity, not raw `tmp/` CSVs.

- [ ] **Step 6: Suggested commit boundary**

```powershell
git add crates/dogmos-perf/benches/ipc_round_trip.rs tools/benchmark_ipc.ps1 tools/tests/test_perf_contract.py docs/performance/ipc-decision.md
git commit -m "test: restore legal IPC performance workloads"
```

### Task 3: Correct workset telemetry and add allocation-count evidence

**Status:** Complete in `b1f88c5`.

**Files:**
- Modify: `crates/dogmos-core/src/world.rs`
- Test: `crates/dogmos-core/tests/frontier_processing.rs`
- Modify: `crates/dogmos-perf/Cargo.toml`
- Create: `crates/dogmos-perf/examples/core_stage_allocations.rs`
- Modify: `docs/performance/README.md`

**Interfaces:**
- Consumes: active stage state and `DogmosWorld::reusable_workset_bytes()`.
- Produces: a documented active-vector-capacity lower bound and per-stage allocation counts for synthetic fixed workloads.

- [ ] **Step 1: Write the telemetry regression test**

Construct a two-node heat stage, advance it until the edge vector exists, and assert the reported lower bound includes:

```rust
state.edges.capacity() * std::mem::size_of::<(u32, u32, f32, f32)>()
```

Expose a `cfg(test)` breakdown helper if the public aggregate cannot make the assertion precise. Observe the test fail against the current `(u32, u32)` accounting.

- [ ] **Step 2: Fix and document the metric**

Use the actual tuple type in the calculation. Rename internal variables/comments to `active_vec_capacity_bytes_lower_bound`; keep the protocol field for compatibility. Document that maps/sets and allocator metadata are excluded and that committed stages currently drop their state.

- [ ] **Step 3: Add a dependency-free allocation-count probe**

Create an example binary with a counting wrapper around `std::alloc::System`. Reset counters after fixture construction and before each stage. Emit CSV containing stage, turf count, allocations, deallocations, allocated bytes, work items, and transcript hash. Use identical synthetic corridor/grid/multiz inputs for each run.

The allocator counter must use atomics only and must not allocate while recording. The emitted transcript hash must cover final numeric state and ordered events.

- [ ] **Step 4: Capture controls**

Run at least three processes for 1,000, 10,000, and 100,000 turfs where practical:

```powershell
1..3 | ForEach-Object {
    cargo +1.98.0 run --release --locked -p dogmos-perf --example core_stage_allocations -- --output "tmp/dogmos-perf/core-control-$_.csv"
    if ($LASTEXITCODE -ne 0) { throw "core allocation control $_ failed" }
}
```

Do not use the counter for wall-time acceptance; use it to rank allocation-removal tasks and verify allocation deltas.

- [ ] **Step 5: Suggested commit boundary**

```powershell
git add crates/dogmos-core/src/world.rs crates/dogmos-core/tests/frontier_processing.rs crates/dogmos-perf/examples/core_stage_allocations.rs docs/performance/README.md
git commit -m "test: measure core stage allocation churn"
```

### Task 4: Remove the per-response 32-bit shim allocation

**Status:** Source and i686 gates complete; paired DreamDaemon candidate capture remains an external
game-repository acceptance gate under the global constraints.

**Files:**
- Modify: `crates/dogmos-byond/src/session.rs`
- Modify: `crates/dogmos-byond/src/lib.rs`
- Test: `crates/dogmos-byond/tests/bounded_io.rs`

**Interfaces:**
- Consumes: `BoundedDogmosClient::round_trip(...) -> Result<&[u8], ClientError>`.
- Produces: `ServiceSession::request_with_response<T, E>(..., decode) -> Result<T, E>` where `E: From<ClientError>`.

- [ ] **Step 1: Add a failing retained-buffer test**

Extend `bounded_io.rs` with a test-only response decoder that records the response pointer for repeated requests and returns a fixed scalar. Assert the pointer stays inside the client's retained response buffer and that the session API returns no owned response vector.

- [ ] **Step 2: Implement the borrowed-response closure**

Use this control flow so timeout termination is preserved:

```rust
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
	match self.client.round_trip(operation, payload, response_capacity, BENCHMARK_REQUEST_TIMEOUT) {
		Ok(response) => decode(response),
		Err(error @ (ClientError::RequestTimeout | ClientError::WorkerStopped)) => {
			self.terminate_service();
			Err(error.into())
		}
		Err(error) => Err(error.into()),
	}
}
```

Keep a zero-response convenience wrapper only if shutdown/cancel call sites become clearer; it must not allocate.

- [ ] **Step 3: Convert production and diagnostic call sites**

Decode fixed arrays, protocol responses, callback fields, and final `ByondValue`s inside the closure while the session mutex guard remains held. Do not return borrowed data outside the guard. Preserve every response-length check and error string.

- [ ] **Step 4: Run focused and i686 gates**

```powershell
cargo +1.98.0 test -p dogmos-byond --locked --target i686-pc-windows-msvc --test bounded_io
cargo +1.98.0 test -p dogmos-byond --locked --target i686-pc-windows-msvc
cargo +1.98.0 clippy -p dogmos-byond --locked --target i686-pc-windows-msvc --all-targets -- -D warnings
```

- [ ] **Step 5: Capture three identical shim/DreamDaemon candidates**

Use the same boot-registration workload identity as the controls. Report DreamDaemon and service memory separately, and compare response allocation counts if the diagnostic harness exposes them. Reject the change if errors, events, or stage transcripts differ.

- [ ] **Step 6: Suggested commit boundary**

```powershell
git add crates/dogmos-byond/src/session.rs crates/dogmos-byond/src/lib.rs crates/dogmos-byond/tests/bounded_io.rs
git commit -m "perf: decode service responses from retained buffers"
```

### Task 5: Make shim validation context lazy and pre-size fixed batches

**Status:** Source and i686 gates complete; paired DreamDaemon allocation/private-byte comparison
remains an external game-repository acceptance gate under the global constraints.

**Files:**
- Modify: `crates/dogmos-byond/src/lib.rs`
- Test: existing `crates/dogmos-byond/src/lib.rs` tests

**Interfaces:**
- Consumes: exact numeric validators and counted fixed-record protocol encoders.
- Produces: identical error text without success-path `String` construction; one final payload allocation per batch.

- [ ] **Step 1: Add representative exact-message tests**

For each decoder family (callback, continuation, mixture, gas metadata, reaction metadata, turf lifecycle, turf gas adjacency, turf heat, heat adjacency, frontier), feed one invalid indexed field and assert the complete message includes family, record index, and field name.

- [ ] **Step 2: Add non-allocating primitive validators**

Keep validators context-free on success and attach index context only in `map_err`, following the existing mixture-adjust pattern:

```rust
let slot = exact_u32(entry[0], "slot")
	.map_err(|error| eyre::eyre!("turf lifecycle entry {index} slot: {error}"))?;
```

For `exact_words4`, validate four words with literal labels or return an indexed primitive error; do not construct four formatted labels before knowing an error exists.

- [ ] **Step 3: Reserve exact batch capacity**

After count validation, initialize each output with checked fixed-record capacity:

```rust
let capacity = 4_usize
	.checked_add(entries.len().checked_mul(RECORD_LEN).ok_or_else(|| eyre!("batch is too large"))?)
	.ok_or_else(|| eyre!("batch is too large"))?;
let mut output = Vec::with_capacity(capacity);
```

Use the actual header/record constants for each protocol family. Assert `output.len() == capacity` after encoding in debug/tests.

- [ ] **Step 4: Run the full decoder and i686 suite**

```powershell
cargo +1.98.0 test -p dogmos-byond --locked --target i686-pc-windows-msvc
cargo +1.98.0 test --workspace --locked --target i686-pc-windows-msvc
```

- [ ] **Step 5: Re-run the identical shim candidate series**

Compare at least three controls and candidates. The change is accepted only with exact error/event equivalence and a measured allocation/private-byte benefit; otherwise retain only clearly readability-neutral capacity fixes.

- [ ] **Step 6: Suggested commit boundary**

```powershell
git add crates/dogmos-byond/src/lib.rs
git commit -m "perf: remove shim validation allocation churn"
```

### Task 6: Hoist mixture-edge cleanup out of lifecycle mutation loops

**Status:** Complete at the Task 6 commit boundary. The exact structural control was captured by the failing
test (two edge-filter passes for two invalidations); the candidate performs one pass and preserves
the expected final topology. No standalone lifecycle wall-time benchmark existed, so the deterministic
pass-count regression is the acceptance evidence for this task.

**Files:**
- Modify: `crates/dogmos-core/src/world.rs`
- Test: `crates/dogmos-core/tests/world_state.rs`
- Test: `crates/dogmos-core/tests/packed_topology.rs`

**Interfaces:**
- Consumes: validated lifecycle batch and `EdgeKey { left, right }`.
- Produces: one mixture-edge retain pass for all replaced/unregistered slots.

- [x] **Step 1: Write the failing one-pass test**

Under `cfg(test)`, count lifecycle edge-filter passes. Build a graph, unregister multiple mixtures in one batch, and assert the batch performs one full edge pass while removing every incident edge and preserving unrelated edges.

- [x] **Step 2: Collect all invalidated slots**

During the already-atomic validation/application flow, insert the slot for each actual unregister and generation replacement into one local set. Do not include idempotent same-generation registrations.

- [x] **Step 3: Retain once**

Replace per-mutation `remove_incident_edges(slot)` calls with:

```rust
if !invalidated_slots.is_empty() {
	self.edges.retain(|key, _| {
		!invalidated_slots.contains(&key.left) && !invalidated_slots.contains(&key.right)
	});
	self.graph = None;
}
```

Use the current graph-cache field name at implementation time. Preserve continuation invalidation and turf detachment semantics.

- [x] **Step 4: Verify behavior and measure bulk teardown**

```powershell
cargo +1.98.0 test -p dogmos-core --locked --target x86_64-pc-windows-msvc --test world_state
cargo +1.98.0 test -p dogmos-core --locked --target i686-pc-windows-msvc
```

Run the synthetic lifecycle benchmark at 1,000/10,000/100,000 slots for controls and candidates. Require identical final topology and a single edge-pass count.

- [x] **Step 5: Suggested commit boundary**

```powershell
git add crates/dogmos-core/src/world.rs crates/dogmos-core/tests/world_state.rs crates/dogmos-core/tests/packed_topology.rs
git commit -m "perf: batch mixture edge cleanup"
```

### Task 7: Remove the per-turf diffusion neighbor allocation

**Files:**
- Modify: `crates/dogmos-core/src/world.rs`
- Test: `crates/dogmos-core/tests/frontier_processing.rs`
- Test: `crates/dogmos-core/tests/numerical_properties.rs`

**Interfaces:**
- Consumes: sorted `PackedTopology::gas_neighbors`, maximum degree six.
- Produces: stack neighbor indices and byte-identical stage results/events.

- [ ] **Step 1: Add allocation/transcript evidence**

Extend the allocation probe with corridor, grid, and six-neighbor multiz diffusion cases. Record allocations after fixture construction and hash final gas vectors.

- [ ] **Step 2: Replace `collect::<Vec<_>>()` with fixed storage**

Use the existing topology maximum constant if exposed; otherwise add one shared core constant:

```rust
let mut neighbors = [0_usize; MAX_TURF_NEIGHBORS];
let mut neighbor_count = 0;
for neighbor in self.topology.gas_neighbors(turf) {
	if let Some(index) = state.index_by_turf.get(&neighbor.handle).copied() {
		neighbors[neighbor_count] = index;
		neighbor_count += 1;
	}
}
```

Iterate `&neighbors[..neighbor_count]` for each gas and derive self-weight from `neighbor_count`.

- [ ] **Step 3: Run numerical and transcript gates**

```powershell
cargo +1.98.0 test -p dogmos-core --locked --target x86_64-pc-windows-msvc --test numerical_properties
cargo +1.98.0 test -p dogmos-core --locked --target x86_64-pc-windows-msvc --test frontier_processing
cargo +1.98.0 test -p dogmos-core --locked --target i686-pc-windows-msvc
```

Expected: exact stage transcript hash and zero per-node neighbor allocations after fixture setup.

- [ ] **Step 4: Suggested commit boundary**

```powershell
git add crates/dogmos-core/src/world.rs crates/dogmos-core/tests/frontier_processing.rs crates/dogmos-core/tests/numerical_properties.rs
git commit -m "perf: keep diffusion neighbors on the stack"
```

### Task 8: Measure and simplify component traversal without changing order

**Files:**
- Modify: `crates/dogmos-core/src/world.rs`
- Test: `crates/dogmos-core/tests/frontier_processing.rs`
- Test: `crates/dogmos-core/tests/world_state.rs`
- Modify: `crates/dogmos-perf/examples/core_stage_allocations.rs`

**Interfaces:**
- Consumes: sorted packed topology and active component membership.
- Produces: deterministic direct-neighbor traversal without a per-node adjacency vector.

- [ ] **Step 1: Establish a threshold to proceed**

From Task 3 controls, proceed only if adjacency construction accounts for a material share of component-stage allocations or time. Record the threshold decision in the reviewed performance summary; otherwise skip this task.

- [ ] **Step 2: Add golden order and rollback tests**

Cover a branching multiz component with firelocks, immutable space, duplicate mixture rejection, cancellation mid-component, and event-capacity overflow. Assert ordered events and exact final numeric state.

- [ ] **Step 3: Borrow stage turf handles**

Split `stage_turf_handles` into a borrowed committed/component path plus an owning fallback only for the all-turfs debug path. Do not clone `stage_component_turfs` when it already owns the component.

- [ ] **Step 4: Traverse packed neighbors directly**

Replace `BTreeMap<u32, Vec<u32>>` adjacency with direct `gas_neighbors(current_handle)` iteration filtered by an active membership map/set. Rely on `PackedTopology`'s tested sorted order; do not add a new sort.

Replace `contains` followed by `insert` with one `if visited.insert(neighbor)`. Preserve the hard turf limit before marking a node visited.

- [ ] **Step 5: Verify and compare**

Run both component test files on x64 and i686, then three allocation/time candidate processes. Require identical hashes/events and improvement beyond control noise.

- [ ] **Step 6: Suggested commit boundary**

```powershell
git add crates/dogmos-core/src/world.rs crates/dogmos-core/tests/frontier_processing.rs crates/dogmos-core/tests/world_state.rs crates/dogmos-perf/examples/core_stage_allocations.rs
git commit -m "perf: traverse packed component topology directly"
```

### Task 9: Recycle service stage state only if profiling justifies it

**Files:**
- Modify: `crates/dogmos-core/src/world.rs`
- Test: `crates/dogmos-core/tests/frontier_processing.rs`
- Test: `crates/dogmos-core/tests/reaction_execution.rs`
- Test: `crates/dogmos-core/tests/world_state.rs`

**Interfaces:**
- Consumes: completed/cancelled stage state.
- Produces: cleared scratch pools whose capacities are retained in `dogmosd`, never exposed to the shim.

- [ ] **Step 1: Gate on measured service churn**

Proceed only if Task 3 shows recurring stage allocation counts or service CPU impact after Tasks 6-8. Do not justify this task with DreamDaemon memory.

- [ ] **Step 2: Add reuse and rollback tests**

Run two identical stages and assert the second does not increase vector capacity or allocation count. Inject cancellation and event overflow between them; assert state remains retryable and no partial records/events commit.

- [ ] **Step 3: Add one scratch owner per stage family**

Keep completed state objects in `DogmosWorld`, clear logical contents/cursors, and move them into the active slot on the next stage. Never clear the only rollback copy before commit succeeds. Maps/sets may initially remain recreated if safe capacity-preserving reset is not clear; optimize vectors first.

- [ ] **Step 4: Verify and measure separately**

Run core/server suites on x64 and the i686 workspace gate. Report `dogmosd` allocation/latency changes separately. Reject increased persistent service memory if the latency/allocation benefit is negligible.

- [ ] **Step 5: Suggested commit boundary**

```powershell
git add crates/dogmos-core/src/world.rs crates/dogmos-core/tests/frontier_processing.rs crates/dogmos-core/tests/reaction_execution.rs crates/dogmos-core/tests/world_state.rs
git commit -m "perf: recycle measured service stage scratch"
```

### Task 10: Optimize server translation scratch only if it remains visible

**Files:**
- Modify: `crates/dogmos-server/src/state.rs`
- Test: `crates/dogmos-server/src/state.rs`

**Interfaces:**
- Consumes: validated wire slices.
- Produces: reusable core mutation vectors cleared on every success/error path.

- [ ] **Step 1: Profile after stage/core changes**

Measure `apply_lifecycle`, adjacency, turf lifecycle, turf heat, heat adjacency, mixture state, and frontier mutations. Skip this task if translation allocation is below the agreed noise/materiality threshold.

- [ ] **Step 2: Add capacity-reuse and atomic-error tests**

For each selected family, apply two same-sized valid batches and one invalid batch. Assert capacity does not grow on the second valid batch and the invalid batch neither mutates world state nor leaves stale entries for the next call.

- [ ] **Step 3: Introduce focused scratch fields**

Add only scratch vectors demonstrated useful by the profile. Clear them before translation, reserve fallibly where the service budget requires it, and pass slices to core. Keep duplicate-edge validation before world mutation.

- [ ] **Step 4: Verify service behavior**

```powershell
cargo +1.98.0 test -p dogmos-server --locked --target x86_64-pc-windows-msvc
cargo +1.98.0 test -p dogmos-server --locked --target i686-pc-windows-msvc
```

- [ ] **Step 5: Suggested commit boundary**

```powershell
git add crates/dogmos-server/src/state.rs
git commit -m "perf: reuse measured server translation scratch"
```

### Task 11: Remove bootstrap frontier duplicate allocation without changing committed order

**Files:**
- Modify: `crates/dogmos-core/src/frontier.rs`
- Test: `crates/dogmos-core/tests/frontier_processing.rs`

**Interfaces:**
- Consumes: chunked begin/append/commit upload.
- Produces: incremental duplicate validation during append and unchanged ordered committed frontier.

- [ ] **Step 1: Add cross-chunk duplicate tests**

Upload the same handle in two non-overlapping append ranges and assert the second append fails atomically. Also cover retry after failure, out-of-order non-overlapping ranges, incomplete upload, and exact committed order.

- [ ] **Step 2: Track upload handles incrementally**

Add an `upload_seen: HashSet<TurfHandle>` cleared/reserved at `begin`. Before writing an append range, validate every handle against both the current set and a temporary set for that chunk; only extend `upload_seen` after the whole chunk passes.

- [ ] **Step 3: Remove `pending()`'s `BTreeSet` pass**

Once append owns duplicate validation, `pending()` checks epoch and completeness only. Keep `commit_validated`'s ordered vector swap and committed-set rebuild. Do not change `remove()` ordering in this task.

- [ ] **Step 4: Verify frontier and event determinism**

```powershell
cargo +1.98.0 test -p dogmos-core --locked --target x86_64-pc-windows-msvc --test frontier_processing
cargo +1.98.0 test -p dogmos-core --locked --target i686-pc-windows-msvc --test frontier_processing
```

- [ ] **Step 5: Suggested commit boundary**

```powershell
git add crates/dogmos-core/src/frontier.rs crates/dogmos-core/tests/frontier_processing.rs
git commit -m "perf: validate frontier duplicates during upload"
```

### Task 12: Qualify the complete change set and decide whether transactional staging warrants rework

**Files:**
- Modify: `docs/performance/README.md`
- Create: `docs/performance/2026-08-30-resource-management-results.md`
- Verify: all changed files

**Interfaces:**
- Consumes: control/candidate artifacts from Tasks 2-11.
- Produces: an evidence-backed acceptance report and a separate go/no-go decision for component transaction redesign.

- [ ] **Step 1: Run repository gates**

```powershell
cargo +1.98.0 fmt --all -- --check
cargo +1.98.0 clippy --workspace --locked --target i686-pc-windows-msvc --all-targets -- -D warnings
cargo +1.98.0 test --workspace --locked --target i686-pc-windows-msvc
cargo +1.98.0 test --locked --target x86_64-pc-windows-msvc -p dogmos-core -p dogmos-protocol -p dogmos-server -p dogmos-process-metrics -p dogmos-identity
python -m unittest discover -s tools/tests -p 'test_*.py'
git diff --check
```

Run the repository feature-matrix script and both supported Linux target builds in an environment with their required linkers. Report unavailable targets as not run, never as pass.

- [ ] **Step 2: Run repeated controls/candidates and equivalence checks**

Use at least three clean processes for identical synthetic core and IPC workloads. For the external paired-game acceptance gate, require three DreamDaemon controls and candidates with exact workload identity, numerical/event equivalence, separate process memory, p50/p95/p99/max, and SSair headroom.

- [ ] **Step 3: Apply acceptance budgets**

Require:

- at least 70% lower Dogmos-attributable DreamDaemon peak private bytes for the architecture migration target;
- no more than 32 MiB fixed shim/mapped address space;
- p95 regression no greater than `max(5%, 2 * control noise)`;
- p99 regression no greater than `max(10%, 2 * control noise)`;
- no numerical, event-order, rollback, timeout, or caller-error regression.

Do not claim the architectural memory target from the micro-optimizations alone; report their incremental effect.

- [ ] **Step 4: Make the transactional-staging decision**

If clone/restore and equalization endpoint copies remain a leading measured cost, write a dedicated design/spec for an indexed transaction scratch arena with disjoint access and atomic commit. If they do not, explicitly close the candidate as not justified. Do not fold that rewrite into this plan's completed changes.

- [ ] **Step 5: Write the result report**

Record exact commands, revision, targets, artifacts, failures, before/after distributions, allocation counts, separate process memory, and skipped external gates. Update `docs/performance/README.md` to link the reviewed result.

- [ ] **Step 6: Suggested commit boundary**

```powershell
git add docs/performance/README.md docs/performance/2026-08-30-resource-management-results.md
git commit -m "docs: record resource management qualification"
```
