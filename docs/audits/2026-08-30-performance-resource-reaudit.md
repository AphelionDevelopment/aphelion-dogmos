# aphelion-dogmos Performance and Resource-Management Reaudit

Date: 2026-08-30  
Audited revision: `37585190239e75232eb5243000ce3d4c916ff7d1` (`master`, no tracked changes before this report)  
Scope: the current aphelion-dogmos checkout, including the service architecture and the retained in-process BYOND implementation. Meridian-Rift and a live DreamDaemon round were not audited in this pass.

## Executive result

The previous local optimization work remains functionally green, but the current revision is not ready for another performance implementation pass without first repairing resource correctness and observability.

The highest-priority issue is in the chunked service stage path: it computes the remaining callback capacity but does not pass that capacity into the core stage. A component can therefore commit and publish more events than the server can enqueue. The enqueue helper then drains only the available count, leaving the rest hidden inside the world. This violates the repository's all-or-nothing callback/backpressure contract.

The retained 32-bit path has two separate lifecycle risks. Its main-thread callback channel is deliberately unbounded and its byte metric counts only the boxed closure pointer, not captured vectors or batches. Its shutdown path also leaves reaction registries holding BYOND values across world reuse. Both can consume or retain DreamDaemon address space, and the current telemetry cannot quantify the callback risk.

Fresh core allocation probes also found a deterministic regression since the qualified transaction-arena revision. At 100,000 turfs, equalize allocation counts are 13.83-14.00% higher and allocated bytes are 3.12-3.24% higher; excited-groups allocation counts are 3.22-3.50% higher and allocated bytes are 2.22-2.29% higher. Inspection identifies the new per-component queue clone and ordered published-mixture set as the leading candidates, but causality has not yet been isolated with a counter or profile.

The maintained IPC benchmark remains functional and within the previous unmatched latency envelope. That is a local transport/functionality result, not production acceptance and not a matched control/candidate comparison. No current DreamDaemon private-byte, address-space, SSair headroom, or live-round evidence was collected.

## Method and evidence boundary

This reaudit:

- read the repository's routing, architecture, process-boundary, gameplay-event, numerical, FFI, performance, verification, release, and upstream-drift guidance;
- compared current `HEAD` with the previously qualified transaction-arena artifacts;
- inspected stage commit, callback enqueue, client/worker teardown, frontier storage, legacy callback, reaction, gas, turf, and heat-worker lifecycles;
- ran current-head formatting, tooling tests, service-safe x64 tests, the authoritative i686 workspace test, three fresh allocation probes, and the maintained three-repetition IPC benchmark;
- did not run strict Clippy, the complete feature matrix, Linux targets, DreamMaker, DreamDaemon, Tracy, a representative full-map workload, or the paired Meridian-Rift qualification matrix.

Inspection findings are not presented as measured production impact. Service memory and DreamDaemon memory remain separate throughout this report.

## Current-head verification

Pinned compiler: `rustc 1.98.0 (88d9e12ae 2026-08-18)`.

| Gate | Result | Interpretation |
| --- | --- | --- |
| `cargo +1.98.0 fmt --all -- --check` | Pass | Formatting only. |
| `python -m unittest discover -s tools/tests -p 'test_*.py'` | Pass: 42 tests | Tooling and checked-in contract tests are green. |
| `cargo +1.98.0 test --locked --target x86_64-pc-windows-msvc -p dogmos-core -p dogmos-protocol -p dogmos-server -p dogmos-process-metrics -p dogmos-identity` | Pass | Service-safe host packages passed. Target-gated cross-process integration remains covered by i686. |
| `cargo +1.98.0 test --workspace --locked --target i686-pc-windows-msvc` | Pass | The authoritative Windows workspace test is green, including BYOND shim and cross-process tests. |
| Three `core_stage_allocations` release probes | Pass and byte-identical | All three 36-row CSVs have SHA-256 `5786EDEB954A1BF4205A11554AEBB5CA5A3E1EBC2CAB7836C74A00B1064471A3`. |
| `tools/benchmark_ipc.ps1 -Iterations 20000 -Repetitions 3` | Pass | Local transport/functionality series only; not a matched production acceptance run. |

The IPC series used the exact current source fingerprint and paired i686 shim/x64 service. The 1,024-mixture service stage reported 2,048 work items in every sample. Its p50 range was 679.9-696.9 microseconds, with worst p95 939.6 microseconds and worst p99 1,097.2 microseconds. The previous qualified unmatched series reported p50 697.1-781.7, worst p95 929.4, and worst p99 1,099.8 microseconds. The current worst p95 is 1.1% higher than the prior worst and remains within the existing 5% local budget, but the runs were taken at different revisions and times without a same-session control noise series.

One-shot process samples remained small and separated by process: the i686 shim private-byte samples were 933,888, 933,888, and 1,007,616 bytes; the x64 service private-byte samples were 1,970,176, 1,961,984, and 1,961,984 bytes. These standalone helper-process values do not establish DreamDaemon footprint or live service RSS behavior.

## Allocation regression evidence

The current three probe files were byte-identical. The table compares their 100,000-turf rows with the previously qualified arena artifact at revision `e9f57269dd8620b0534ca59d30515fa992b96383`.

| Stage | Topology | Qualified allocations | Current allocations | Change | Qualified bytes | Current bytes | Change |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Equalize | Corridor | 119,462 | 136,128 | +13.95% | 60,595,728 | 62,557,472 | +3.24% |
| Equalize | Grid | 115,828 | 131,844 | +13.83% | 60,092,408 | 61,964,568 | +3.12% |
| Equalize | Three-layer multiz | 119,057 | 135,720 | +14.00% | 60,548,332 | 62,509,572 | +3.24% |
| Excited groups | Corridor | 259,144 | 267,476 | +3.22% | 42,783,920 | 43,764,592 | +2.29% |
| Excited groups | Grid | 258,650 | 266,982 | +3.22% | 42,739,152 | 43,719,824 | +2.29% |
| Excited groups | Three-layer multiz | 225,209 | 233,101 | +3.50% | 41,732,584 | 42,658,856 | +2.22% |

Work-item counts and transcript hashes were deterministic across the three current runs. They are not expected to match the older revision after the component-stage correctness changes, so this comparison establishes resource regression, not behavioral equivalence between revisions.

## Findings

### P0 - chunked stages can commit more callbacks than the server can enqueue

Evidence:

- `crates/dogmos-server/src/state.rs:1331-1363` computes `event_limit` from remaining callback capacity, reserves only `world_events`, and calls `DogmosWorld::process_stage_chunk_cancellable` without that limit.
- The non-chunked test helper at `crates/dogmos-server/src/state.rs:1390-1420` uses `process_stage_cancellable_with_event_limit`, proving the intended capacity-aware path already exists for monolithic stages.
- `crates/dogmos-core/src/world.rs:3201-3211` validates component events against the world's fixed `max_events`, not the server's current remaining callback capacity.
- `crates/dogmos-core/src/world.rs:2157-2161` drains only `min(pending events, maximum)`. Excess committed events remain inside the world after the server reports the stage result.

Impact:

A chunked equalize, excited-groups, heat, or reaction component can commit authoritative state and advance stage progress even though its complete callback batch cannot fit the server queue. The server enqueues only the available prefix and leaves the remainder hidden for a later call. This breaks the documented atomic callback/backpressure boundary and can produce delayed event delivery, misleading stage telemetry, and retained world-event storage under sustained queue pressure.

Required resolution:

Thread the dynamic event budget through the chunked core call without making it part of cursor identity. Validate each component's complete staged event batch against that dynamic budget before publishing its transaction. A pre-filled-queue test must prove that an event-producing component returns backpressure without committing that component, draining a prefix, or advancing its cursor.

### P1 - callback enqueue has fallible operations after world events are drained

Evidence:

- `crates/dogmos-server/src/state.rs:1655-1668` drains world events before queue depth, continuation capacity, sequence, and encoding checks complete.
- `crates/dogmos-server/src/state.rs:1754-1780` inserts pending continuations before the destination callback queue's final `try_reserve_exact`.
- Direct reaction execution creates a transaction and mutates core state before `enqueue_world_events_at` returns successfully (`crates/dogmos-server/src/state.rs:1040-1072`).

Impact:

Allocation, sequence, continuation-capacity, encoding, or destination-queue failure can leave world events drained, continuation records installed without corresponding queued callbacks, or a reaction transaction alive after an error. Some paths are rare and require fault injection to reproduce, but the ordering is inspection-proven and conflicts with the repository's fallible, all-or-nothing ownership rules.

Required resolution:

Preflight and reserve all server-owned storage before mutating queue-visible state. Translate and validate from borrowed pending events, then commit the drain, continuation insertion, sequence advance, and callback enqueue as one infallible section. Add deterministic failure seams for allocation/capacity tests; do not depend on real out-of-memory behavior.

### P1 - the 32-bit callback queue is unbounded and its byte telemetry is not ownership telemetry

Evidence:

- `crates/auxcallback/src/lib.rs:53-85` uses `flume::unbounded` and explicitly relies on infinite queue capacity to avoid dropping work.
- `crates/auxcallback/src/lib.rs:93-105` reports owned bytes as queue depth multiplied by `size_of::<DeferredFunc>()`.
- Producers in `src/turfs/processing.rs`, `src/turfs/katmos.rs`, `src/turfs/superconduct.rs`, and `src/gas/types.rs` capture heap-backed vectors, batches, errors, and BYOND values in queued closures. Those captures are not included in the metric.

Impact:

If background producers outrun the DreamDaemon main-thread drain, the legacy in-process DLL can grow without a bound in DreamDaemon's 32-bit address space. The current metric can remain nearly flat per item while captured payload capacity grows substantially. This is inspection-proven exposure; no live queue-pressure series was taken.

Required resolution:

First make every repository-owned producer report the exact lower-bound bytes it transfers into the closure and expose item/byte current and high-water gauges. Then define a bounded, all-or-nothing batch admission contract. Critical gameplay callbacks must not be silently dropped: capacity rejection has to stop or fail the producing stage and surface through a caller-legible fatal/backpressure path. The final policy requires paired Meridian-Rift behavior and live pressure verification.

### P1 - legacy shutdown retains reaction BYOND values across world reuse

Evidence:

- `src/reaction.rs:37-45` stores reaction implementations in thread-local `REACTION_VALUES`; BYOND-side entries own `ByondValue`.
- `src/reaction.rs:162-168` inserts or replaces entries during reaction registration.
- `src/gas/types.rs:16`, `:324`, and `:368` maintain a separate static `REACTION_INFO` map.
- `src/lib.rs:339-354` calls `destroy_gas_info_structs` during shutdown, but `src/gas/types.rs:226-245` clears neither reaction registry.

Impact:

A loaded DLL reused for another BYOND world can retain stale DM reaction references, removed reaction entries, and their Rust heap storage. It can also expose stale registry contents during reinitialization. This is an ownership/lifecycle defect independent of whether its steady-state memory is large.

Required resolution:

Clear both reaction registries after worker shutdown and before gas metadata is rearmed. Add a two-world lifecycle test that registers BYOND-side and Rust-side reactions, shuts down, proves the registries empty, then reinitializes without stale lookup behavior.

### P1 - current component correctness state regressed core allocation counts and bytes

Evidence:

- `crates/dogmos-core/src/world.rs:633-666` adds a `BTreeSet<MixtureHandle>` to retain mixtures published by earlier disconnected components.
- `crates/dogmos-core/src/world.rs:3130-3144` clones the complete component queue into `stage_component_turfs` and drops that clone after every component.
- The fresh probes measured the deterministic regression in the table above.
- `DogmosWorld::reusable_workset_bytes` counts `StageComponentState::queue` but not the concurrently alive `stage_component_turfs` clone (`crates/dogmos-core/src/world.rs:907-955`). It also intentionally excludes the ordered set, so the current probe does not report the component peak that it is trying to explain.

Impact:

The regression occurs in 64-bit `dogmosd`, not DreamDaemon. It consumes allocator work and stage latency headroom but does not directly threaten the 32-bit address-space target. The new state enforces important cross-component correctness and must not be removed without an equivalent invariant.

Required resolution:

Make the probe observe peak active stage storage first. Reuse or move the component queue instead of cloning it. Replace the per-entry ordered-set allocation with a bounded slot-indexed generation marker or equivalent deterministic structure. Preserve incremental component commit, cross-component duplicate-mixture rejection, cancellation rollback, event order, and numeric/event transcripts.

### P1 - remaining shim error labels allocate on successful hot paths

Evidence:

- `crates/dogmos-byond/src/lib.rs:542-546` formats `callback value {index}` for all four values of every decoded event. A full 1,023-event drain creates 4,092 temporary strings.
- `crates/dogmos-byond/src/lib.rs:893-897` formats one gas label per mixture snapshot entry.

Impact:

These allocations occur in the 32-bit DreamDaemon process on successful decode. They survived the previous broad lazy-context cleanup and are a small, low-risk direct footprint/churn target.

Required resolution:

Use fixed validation on the success path and format the indexed label only when conversion fails. Preserve the exact indexed error context with focused tests, including finite `f64` values that overflow when narrowed to `f32`.

### P2 - `BoundedDogmosClient::drop` can detach a cancelling I/O worker

Evidence:

- `crates/dogmos-byond/src/client.rs:370-380` closes the sender and requests cancellation, but joins only if the worker is already finished at the immediate second check.
- The bounded-I/O tests poll the worker before drop; they do not prove drop itself leaves no worker or handle behind.

Impact:

A slow cancellation can outlive the client value with its join handle detached. Normal `ServiceSession` teardown kills and waits for the service first, reducing the common-path risk, but the public client lifecycle is not self-contained.

Required resolution:

Provide an explicit bounded close/join operation and call it from service termination after the pipe/process has been made interruptible. Test repeated connect-timeout-drop and request-timeout-drop cycles for zero live workers and stable handle/private-byte counts. Do not make `Drop` wait without a hard bound.

### P2 - the legacy heat worker occupies a Rayon worker for the world lifetime

Evidence:

- `src/turfs/superconduct.rs:777-794` starts a Rayon task that blocks indefinitely on a channel receive and owns reusable heat scratch.
- `src/turfs/superconduct.rs:54-72` stops it with two busy-yield loops and has no join handle; timeout panics at five seconds.

Impact:

The legacy in-process path permanently consumes one global Rayon worker and relies on an atomic flag rather than a joined ownership boundary. The retained scratch is intentional while the world is active, but worker-pool capacity and shutdown completion are not directly observable.

Required resolution:

Move the long-lived receiver to a named dedicated thread with an owned join handle and explicit stop signal, or remove it when the service path fully owns heat processing. Verify no task, channel payload, or scratch survives a world-reuse cycle.

### P2 - legacy gas-slot reuse can scan the full turf graph once per free candidate

Evidence:

- `src/gas.rs:235-254` searches the free-slot vector in reverse while holding its write lock.
- Each candidate calls `gas_mix_is_referenced`; `src/turfs.rs:447-452` scans every turf graph node.
- `src/gas.rs:304-314` also performs a linear duplicate check on unregister.

Impact:

Worst-case registration is O(free slots x turfs), and the lock prevents concurrent free-list progress. No representative registration profile was captured, so this is a complexity finding rather than a measured hot path.

Required resolution:

Maintain an authoritative per-slot turf-reference count or generation-safe referenced bitmap updated by turf registration, mixture changes, and turf destruction. Reuse must remain impossible while any turf names the slot. Gate implementation on a live or representative registration trace showing material scan cost.

### P2 - legacy shutdown clears large arenas without releasing their capacity

Evidence:

- `src/gas.rs:77-94` reserves 240,000 gas slots and clears the vectors on shutdown.
- `src/turfs.rs:385-403` reserves 650,250 nodes, 1,300,500 edges, and 650,250 map entries, then calls collection `clear` methods on shutdown.
- Heat state uses `take` and does release its owner at `src/turfs/superconduct.rs:71`.

Impact:

The gas and turf collections keep their large allocations alive between world shutdown and reinitialization. Reinitialization replaces them, but the teardown boundary does not actually release the documented arenas. This matters most for hard-restart/world-reuse behavior inside 32-bit DreamDaemon.

Required resolution:

Replace the gas and turf owners with `None` at shutdown and recreate them during explicit world preparation. Add lifecycle metrics/tests proving active counts and capacities return to zero before rearm.

### P2 - frontier telemetry omits persistent frontier storage

Evidence:

- `crates/dogmos-core/src/frontier.rs:35-48` owns both committed frontier storage and upload scratch.
- `frontier_upload_bytes` at `crates/dogmos-core/src/frontier.rs:276-279` counts only staging, received bits, and upload-set lower bounds. It omits `committed`, `committed_set`, hash buckets, and allocator metadata.

Impact:

The field name is defensible as upload-only storage, but current telemetry has no separate lower bound for the persistent committed frontier. Resource dashboards can therefore miss a major service-owned collection after upload completes.

Required resolution:

Keep upload and committed storage separate. Add a documented `frontier_storage_bytes_lower_bound` and committed-count/capacity fields rather than changing the meaning of the existing metric.

## Resolution order

1. Repair chunked callback-capacity propagation and enqueue atomicity before further stage optimization.
2. Clear legacy reaction ownership on shutdown.
3. Make legacy callback byte ownership truthful, capture a representative queue-pressure series, then implement bounded all-or-nothing admission with the paired DM failure path.
4. Remove the remaining 32-bit shim label allocations.
5. Instrument peak component storage, recover the measured core regression, and rerun identical allocation/IPC series.
6. Close client and heat-worker lifecycle gaps and release legacy arenas at shutdown.
7. Implement gas-slot reference accounting only if representative evidence confirms registration scan cost.
8. Add persistent frontier-storage telemetry.
9. Run full local gates, then the paired Meridian-Rift DreamMaker/DreamDaemon and live workload acceptance matrix with DreamDaemon and service memory reported separately.

The test-first implementation tasks, checkpoints, and acceptance commands are defined in `docs/superpowers/plans/2026-08-30-performance-resource-resolution-plan.md`.

## Unrun acceptance gates

This report does not claim production completion. The following evidence remains required after implementation:

- strict i686 Clippy and the supported feature matrix;
- Linux cross-target builds where supported;
- generated-binding drift and paired-artifact verification;
- paired Meridian-Rift PowerShell DreamMaker compile and DreamDaemon boot/full-suite gates;
- three identical control and three identical candidate live workloads with exact revision, map, seed, BYOND version, configuration, operation counts, callback counts, and transcript/event equivalence;
- separate DreamDaemon private/committed bytes and address-space series, separate `dogmosd` private/RSS series, stage p50/p95/p99/max, SSair headroom, and callback queue item/owned-byte high water.

## 2026-08-31 resolution status

The approved plan was implemented through its repository-local and synthetic-performance gates.
Detailed evidence is recorded in
`docs/performance/2026-08-31-performance-resource-resolution-results.md`, with callback-pressure and
component-stage measurements in their dedicated dated result files.

The callback-capacity propagation, prepared atomic callback commit, legacy reaction cleanup,
callback ownership telemetry, shim success-path allocation removal, component-stage allocation
recovery, bounded client close, owned heat thread, arena release/rearm, and committed-frontier
diagnostics are resolved locally. The gas-slot reference-count change was skipped because its
required representative hot-path evidence was absent. Bounded legacy callback admission remains at
the mandatory policy checkpoint because the paired checkout does not provide a retained-legacy
pressure workload or an approved DM rejection path.

Fresh formatting, strict i686 Clippy, full i686 workspace tests, x64 core/service tests, the
supported feature matrix, 42 Python tool tests, deterministic generated-binding comparison, and
diff whitespace checks passed. The measured component rows recovered the qualified allocation
values with exact transcript/work equivalence, and the affected same-session IPC percentiles stayed
within the 5 percent budget.

Production qualification remains externally blocked. The paired Meridian-Rift contract names
source `a09f26ab8dffc6d6a1c50274ea1ced5dc5953ab7`, while the candidate reports
`d8dc92d8fd3d268abd89463c5818f174bf077b48`; the maintained runtime gate therefore rejected the
candidate before initialization. No DreamDaemon memory reduction or live-round equivalence is
claimed. The paired checkout also requires restoration or authorized regeneration of the two
installed native artifacts, as described in the implementation results.
