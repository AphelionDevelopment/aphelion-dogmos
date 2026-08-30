# aphelion-dogmos Performance and Resource-Management Audit

Date: 2026-08-30  
Audited revision: `36c9a676c19875fe360df8b8b08dd392d0f70a7b` (`master`, clean before this report)  
Scope: this repository only. No Meridian-Rift source or runtime was audited.

## Executive result

The inherited `HANDOFF.md` found real allocation and complexity problems, but it cannot be used as the implementation order. It audited `ea4e0ce`, took no measurements, and predated the commit that placed its subject changes and the handoff itself on `master`.

The current checkout has three higher-priority evidence failures:

1. The checked-in generated bindings and release-contract expectations have drifted from protocol 10 and the new frontier operations.
2. The authoritative i686 workspace gate fails in `cross_process_handshake_echo_single_client_and_shutdown`: a snapshot reports the truthful installed gas count of zero while the fixture expects 32 without installing gas metadata.
3. The maintained IPC benchmark aborts at its first simulation-stage case with `Server(StageConflict)` because it does not establish the frontier/stage preconditions now required by the real service.

Until these are repaired, the repository cannot produce a trustworthy before/after performance series. Baseline and measurement repair is therefore phase 0, ahead of every optimization.

After those blockers, the best low-risk work is still in the 32-bit shim: eliminate the response `Vec` allocation, make success-path field labels lazy, and reserve exact batch payload sizes. Core work should begin with the batched mixture-edge cleanup and the per-node diffusion allocation. The component/equalization rewrite remains last because it carries the greatest deterministic-order and rollback risk.

## Method

The audit:

- read `HANDOFF.md`, `AGENTS.md`, the complete `docs/agent/` guidance, performance contracts, workload definitions, and current crate manifests;
- inspected allocation, collection, clone, retain, topology, stage, callback, translation, process-metric, reaction, and numerical-kernel paths at current `HEAD`;
- compared the handoff revision to current `HEAD`;
- ran formatting, tooling tests, strict i686 Clippy, the i686 workspace test, targeted x64 tests, and a reduced three-repetition IPC benchmark request;
- did not run DreamMaker, DreamDaemon, Tracy, Linux targets, the full feature matrix, or a representative full-map workload.

The severity labels below rank what blocks reliable optimization and what can pressure the 32-bit DreamDaemon process. They do not treat stable 64-bit `dogmosd` RSS as a defect.

## Verification evidence

| Gate | Result | Interpretation |
| --- | --- | --- |
| `cargo +1.98.0 fmt --all -- --check` | Pass | Formatting only. |
| `python -m unittest discover -s tools/tests -p 'test_*.py'` | Fail: 3 of 41 | Stale protocol/canonical-hash expectations and generated binding drift. |
| `cargo +1.98.0 clippy --workspace --locked --target i686-pc-windows-msvc --all-targets -- -D warnings` | Pass | Static i686 Rust gate. |
| `cargo +1.98.0 test --workspace --locked --target i686-pc-windows-msvc` | Fail | 3 control-plane tests passed, then the gas-count assertion failed; later test binaries did not run. |
| Targeted x64 core/protocol/server/process/identity tests | Pass | The selected service-safe packages passed; x64 workspace-wide testing is intentionally invalid because BYOND dependencies are i686-only. |
| `tools/benchmark_ipc.ps1 -Iterations 2000 -Repetitions 3` | Fail in run 1 | Transport cases through the snapshot ran; the first simulation-stage case returned `StageConflict`, so there is no valid three-run series. |

The partial benchmark output is diagnostic only. For example, its scalar getter p50/p95/p99 was 32.1/75.9/110.8 microseconds and its 1,024-lifecycle batch p50/p95/p99 was 187.4/267.3/326.9 microseconds. These are one shortened run, not acceptance evidence and not comparable to the checked-in 20,000-iteration series.

## Findings

### P0 — evidence and release-contract blockers

#### A1. Generated bindings and contract fixtures do not describe current protocol 10

- `dogmos-build-manifest.toml` declares protocol 10.
- `tools/tests/test_dogmos_contract.py` still expects protocol 9 and the old canonical manifest hash.
- `crates/dogmos-byond/bindings.dm` differs from deterministic generator output and lacks the new frontier add/remove exports.
- Several protocol test names still say `protocol_v9` while exercising the protocol-10 layout.

This is not a performance defect, but it invalidates the release/identity gate that performance artifacts rely on. Regenerate; do not hand-edit generated bindings.

#### A2. The authoritative i686 workspace gate is red at clean `master`

`crates/dogmos-server/tests/control_plane.rs:388` expects `snapshot.gas_count == 32`, but that cross-process fixture never installs gas metadata. `ServiceState::snapshot` correctly derives the value from the installed registry and returns zero when none exists. The test must either install a 32-entry registry before lifecycle/state operations or explicitly expect zero. Installing metadata is preferable because the fixture claims to exercise a 32-gas production snapshot.

This failure is deterministic on a focused rerun. It is a fixture/precondition defect unless separate evidence shows the service dropped a valid metadata request.

#### A3. The maintained IPC benchmark no longer establishes a legal simulation stage

`crates/dogmos-perf/benches/ipc_round_trip.rs:223-234` sends `ProcessTurfs` with `frontier_epoch = 1`, but the benchmark only registers mixtures and adjacency. It does not register turfs, upload/commit a frontier, install gas metadata, or seed production-equivalent stage state. The service now enforces those preconditions and returns `StageConflict`.

The benchmark also reuses one fixed stage request across warmup and measurement. Its setup and epoch policy must be explicit so the benchmark measures either transport-only echo behavior or real service compute, never an accidental mixture of the two.

#### A4. Workset telemetry undercounts and changes meaning across stage lifetime

`DogmosWorld::reusable_workset_bytes` counts `StageHeatState::edges` as `(u32, u32)` even though each entry is `(u32, u32, f32, f32)`, undercounting the vector payload by 50%. It also omits the heap storage of stage maps and sets. More importantly, each stage state is created at stage start and consumed/dropped at commit, so the metric is nonzero only while a stage is active and does not represent retained reusable capacity.

The field can remain a documented lower bound, but its tuple size must be corrected and telemetry/tests must state whether values are active logical payload, retained capacity, or allocator/process measurements. It cannot currently validate allocation-reuse claims.

### P1 — 32-bit shim allocation pressure

#### B1. Every service response is copied into a fresh `Vec`

`crates/dogmos-byond/src/session.rs:25-46` converts the borrowed slice returned from the client's fixed response buffer with `to_vec()`. Most callers decode a fixed-width response immediately and discard the allocation. This directly affects DreamDaemon's 32-bit heap.

Use a closure-based borrowed-response API, or fixed caller-owned response arrays where that makes call sites clearer. Preserve timeout poisoning and service termination before returning the error.

#### B2. Success-path diagnostic labels allocate per field

The handoff's finding is correct but incomplete. Hot decoders construct `&format!(...)` labels for mixture state, gas/reaction metadata, turf lifecycle/adjacency/heat, heat adjacency, frontier handles, and `exact_words4`. The same pattern also exists in callback values, continuation tokens, and mixture lifecycle decoding.

The existing lazy error mapping near the mixture-adjust decoder is the local precedent. Preserve exact caller-legible field context with tests; do not replace messages with context-free numeric errors.

#### B3. Known-size batch encoders grow from zero capacity

The current BYOND encoders for continuation adjustments, mixture state, turf lifecycle, turf adjacency, turf heat, heat adjacency, diagnostic batches, and mixture lifecycle create `Vec::new()` even though the counted fixed-record final length is known. The protocol encoders commonly reserve correctly; the shim adapters should do the same after their count bounds are validated.

This removes realloc chains but not the necessary final payload allocation. It should be evaluated together with B1/B2 in a repeated DreamDaemon boot-registration workload.

#### B4. Callback decoding has an additional missed label-allocation loop

`decode_production_callback_batch` correctly reserves its output field vector, but it formats `callback value {index}` for all four values of every event on the success path. A full 1,023-event drain creates 4,092 temporary strings in DreamDaemon. This belongs in the lazy-label task and needs a saturated-batch error-message regression test.

### P1 — core complexity and per-stage churn

#### C1. Batched mixture unregister still scans every edge once per mutation

`DogmosWorld::apply_lifecycle` calls `remove_incident_edges` inside both replacement and unregister branches. That helper retains over the full `BTreeMap`. A mass batch is therefore O(changed slots x total mixture edges). The turf-detachment scan was already hoisted; the mixture-edge scan was not.

Collect affected mixture slots during validation/application and retain the edge map once. The set must include replaced registrations as well as unregisters. Add an inspection-safe counter or focused benchmark that proves one edge pass for a batch without timing assertions.

#### C2. Diffusion allocates a neighbor `Vec` per processed turf

`compute_stage_diffusion_node` collects at most six dense neighbor indices into a heap vector and then scans it for all gas slots. A stack `[usize; 6]` plus length preserves the necessary repeated iteration without allocation. `PackedTopology` already bounds and sorts neighbors.

#### C3. Component stages rebuild multiple ordered maps and vectors per component

Equalize and excited-groups clone the selected turf vector, construct node/adjacency `BTreeMap`s, allocate a vector per adjacency node, and build further maps/sets for BFS, parents, staged records, and decompression. The handoff correctly identified the duplicate adjacency representation and double `visited` lookup. It understated the total temporary-collection churn.

Direct `PackedTopology` traversal can remove the adjacency map, but only if membership and iteration order remain deterministic. This work requires transcript/event equivalence and a component-scale allocation benchmark before implementation.

#### C4. Transactional component staging copies every affected mixture repeatedly

`process_ready_stage_component` clones the component, clones every mixture into `before`, runs the stage, clones every mixture into `after`, restores `before`, and later commits `after`. `stage_equalization_transfer` then clones both endpoint records for each flow and reinserts them into the staging map.

The copies enforce rollback and avoid aliased mutable borrows. They are not dead code. A future indexed scratch transaction can reduce them, but it is the highest-risk optimization in this audit and must be isolated behind golden event/numeric transcripts and cancellation/overflow rollback tests.

#### C5. Stage scratch collections are discarded rather than reused

`StageDiffusionState`, `StageHeatState`, `StageReactionState`, and `StageComponentState` are constructed at stage start and consumed at commit. Their `Vec`, `BTreeMap`, and `BTreeSet` allocations are dropped every completed stage, despite telemetry naming the storage a reusable workset.

This is `dogmosd` churn, not a DreamDaemon footprint target. It may still affect stage latency and allocator high-water behavior. Measure allocation counts first; if significant, recycle cleared state objects while preserving fail-closed cancellation and event-capacity rollback.

### P2 — lower-confidence or lower-value candidates

#### D1. Server wire-to-core translation allocates throwaway vectors

The handoff's server finding is correct. Each `apply_*` method collects translated records, and turf adjacency creates a duplicate-edge set plus two vectors. These allocations are in 64-bit `dogmosd`. Optimize them only after profiling shows material latency or churn; they do not advance the 70% DreamDaemon memory target.

Reusable scratch vectors are safer than changing protocol decoders to expose transport-owned storage. They must be cleared on every error path and must not make validation partially mutating.

#### D2. Frontier duplicate/removal work is linear but order-sensitive

`FrontierState::pending` allocates an ordered set over the full bootstrap frontier. `remove` allocates a hash set for the delta and retains the committed vector once. The latter is O(committed), not O(committed x removed), because removals are already batched.

Track upload duplicates incrementally to remove the bootstrap validation allocation. Do not use swap-remove for committed handles: stage iteration order is observable through deterministic event order. Retain-based removal should remain unless a measured frontier-removal workload justifies an order-preserving index structure.

#### D3. A few debug-only fallback paths allocate heavily but do not affect release service stages

The monolithic `process_turf_heat` and fallback diffusion graph builders are behind `cfg(debug_assertions)`. Their collections matter to debug tests and developer feedback, but they are not production release hot paths. Do not prioritize them from release-memory inspection alone.

## Previously unaudited areas

- `reactions.rs`: native reaction arithmetic does not allocate on its steady-state success path. Gas-key lookup is an ordered-map lookup per named gas, which is a CPU candidate only if reaction profiling identifies it.
- `metadata.rs`: allocations build immutable registries at installation. They are bounded (maximum 32 gases) and not per tick.
- `numerics/conduction.rs` and `numerics/diffusion.rs`: graph construction allocates canonical topology, but the reusable `*_into` kernels do not allocate per numerical iteration. The current chunked world stages do not consistently retain their surrounding scratch state, which is the larger resource issue.
- process metrics: Windows sampling uses direct process APIs and fixed structs; no new steady-state allocation issue was found. The semantic undercount is in core workset telemetry (A4).
- cross-bitness probe: it is a qualification fixture, not a representative allocator benchmark.

## Recommended order

1. Repair generated contracts, the i686 fixture, and the IPC benchmark.
2. Correct telemetry semantics and add allocation/work counters so later claims are measurable.
3. Capture three valid controls before changing hot paths.
4. Apply the low-risk 32-bit shim changes as separately reviewable units.
5. Hoist mixture-edge cleanup and replace the per-node diffusion heap vector.
6. Re-measure, then choose between component traversal work and service scratch reuse based on profiles.
7. Leave transactional equalize staging and frontier storage redesign until dedicated evidence shows they are necessary.

The detailed tasks and gates are in `docs/superpowers/plans/2026-08-30-performance-resource-management-plan.md`.
