# Performance repair verification

Source baseline: `46209bccd3703d38db0efedabeebf0f79e950234`. Repairs are uncommitted. The pre-existing `AGENTS.md` edit is preserved.

## Implemented changes

- Direct reactions reserve callback storage from the installed reaction inventory, including profiling, a DM continuation, and already-pending events, instead of reserving the global queue capacity per transaction.
- Diffusion, heat, reaction, and component stages retain reusable buffers. Generation-aware slot indexes replace repeated stage map/set construction. Fixed neighbor arrays remove per-turf temporary allocations.
- Component kernels run from private snapshots and yield during preparation, traversal, balancing, and publication preparation. Heat publication proceeds incrementally. Shared visibility tokens publish a complete stage or connected component atomically; authoritative writes invalidate conflicting pending updates. Empty-neighbor traversal and the heat maximum-row calculation no longer hide a full traversal outside the work budget.
- Equalization uses the captured component membership index rather than reconstructing the same lookup. The algorithms retain their existing group limits, ordering, coefficients, and trace-gas behavior.
- Reverse mixture-edge and mixture-turf indexes make lifecycle cleanup visit affected owners. Frontier deltas use indexed removal with stable, lazily materialized iteration and amortized compaction.
- Snapshot batches decode directly into the numeric output buffer. The shim writes output lists through the pinned BYOND API's bulk list operation and reuses numeric conversion storage.
- Legacy callbacks have bounded item and accounted-byte admission. Overflow latches an error that blocks simulation and callback execution. Diagnostics and shutdown remain available; initialization can reset the latch after shutdown, but cannot bypass failure in the active world.
- The allocation probe covers cold and warmed cycles, zero-event reaction stages, actual allocator live-byte high water, maximum observed chunk time, sparse frontier removal, and lifecycle cleanup.

## Test review

The tests were reviewed against externally observable requirements, rather than changed solely to accept new chunk counts:

- The component test cancels at each yield point, checks that both mixtures become visible together, verifies conservation and exact two-cell equalization, and verifies clean retry. A temporarily injected early-visibility defect failed this test at a partially published diffusion result.
- The heat test checks both temperatures after each chunk, energy conservation, unchanged pending state, and cancellation rollback.
- The reservation test opens multiple transactions with a realistic global capacity. Temporarily restoring the old reservation failed with an observed capacity of 65,536 entries.
- Callback tests exercise exact accounted-byte and item limits, rejected admission, suppression of queued execution after failure, cleanup, and reopening. The FFI lifecycle test first failed on blocked post-shutdown initialization and passed after the guard repair. It records whether a blocked closure executed; a caught panic is not treated as evidence that the closure was blocked.
- Reverse ownership is checked against an independent full scan after reassignment, unregistration, and generation reuse. Frontier tests check exact order after removal, readdition, and compaction.
- A malformed final batch record must return an error rather than a valid prefix. Storage accounting checks complete gas rows and retained capacity after cancellation.
- Pre-change allocation-probe hashes remain an independent numerical/event oracle. The compatibility hash uses the original work count in its header, because additional scheduling work changes that count without changing atmosphere state. New transcript hashes exclude work counts; chunk work bounds are tested separately.

## Verification results

| Gate | Result |
| --- | --- |
| Pinned i686 workspace tests | 403 passed; 2 documentation examples ignored |
| Pinned x64 core/protocol/server tests | 273 passed |
| i686 strict Clippy, all workspace targets | Passed |
| x64 production release Clippy | Passed; debug-only test helpers excluded |
| Formatting and whitespace diff check | Passed |
| Supported feature matrix | All 12 configurations passed |
| Windows i686 release legacy DLL, shim, and x64 service builds | Passed |
| Regenerated production bindings | Exact output unchanged |
| i686-to-x64 IPC | Passed |
| Callback pressure | 10,000 cycles; 10,240,000 callbacks enqueued and drained; final depth 0; reported service and shim sample ranges 0 bytes |
| Process isolation | 512 MiB diagnostic allocation; shim growth 0 bytes; service growth 537,923,584 bytes |
| Native allocation/equivalence probes | Three runs of 135 cases; deterministic counts and state/event hashes match across runs; 108/108 historical comparisons match |
| Python tooling | 41 passed; 1 pre-existing policy failure |
| i686 Linux release shim | Blocked: missing `cc` linker |
| Paired DreamMaker/DreamDaemon and live rounds | Not run against repaired artifacts |

Evidence: `tmp/performance-repair-tests.log`, `tmp/performance-repair-x64-tests.log`, `tmp/performance-repair-clippy.log`, `tmp/performance-repair-features.log`, `tmp/performance-repair-cross-bitness.log`, `tmp/performance-repair-callback-pressure.log`, `tmp/performance-repair-process-isolation.log`, and `tmp/performance-repair-final-{1,2,3}.csv`.

The native process checks were rerun against matched development artifacts after the final source changes. They are not paired-game acceptance evidence. Final builds are recorded in `tmp/performance-repair-final-windows-build.log`.

Known gate boundaries:

- The Python tooling suite has one failure caused by the pre-existing `AGENTS.md` removal of its protected-file policy. This repair does not restore or otherwise modify that user edit.
- The i686 Linux release shim build cannot link in this Windows environment because `cc` is unavailable.
- Release Clippy was also attempted with test targets: existing tests call debug-only reference helpers and do not compile in that configuration. Production release libraries and binaries are checked separately; executable tests use the supported debug configuration.
- Paired DreamMaker compile, real BYOND native-load/boot, the full DM suite, live-round timing, and DreamDaemon memory acceptance have not been run against these repaired artifacts. Native IPC probes do not exercise BYOND list creation.

## Performance interpretation

| Stage, 100,000-turf corridor | Original cold allocations | Repaired cold allocations | Repaired third-cycle allocations | Original cold bytes | Repaired cold bytes | Repaired third-cycle bytes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| process_turfs | 133,460 | 210 | 1 | 84,086,416 | 96,467,568 | 24 |
| turf_heat | 133,428 | 178 | 1 | 25,997,568 | 37,748,208 | 24 |
| equalize | 119,462 | 84,684 | 67,170 | 60,595,728 | 179,478,067 | 8,526,056 |
| excited_groups | 83,389 | 66,970 | 66,674 | 39,739,768 | 160,976,299 | 11,396,336 |
| react | No control | 66 | 1 | No control | 14,679,888 | 24 |

`react` uses an empty reaction inventory. Warm cycles continue the same world, so they are not matched historical speedup comparisons. All three candidate repetitions produced the same allocation counts and transcript hashes. Sparse removal of 16 frontier entries allocated nothing at all three world sizes; corresponding mixture cleanup allocated 1,568 bytes in 12 allocations.

Retained snapshots trade service memory and cold allocation volume for reduced allocation churn and cooperative processing. In-flight component state is released on cancellation; its next attempt may allocate again. The reusable-storage metric is a lower bound and excludes tree nodes, in-flight futures, and allocator metadata. The allocator high-water measurement includes the benchmark fixture and service-side state; it is not DreamDaemon memory.

A work limit counts logical processing steps. Allocator operations and cleanup do not provide a hard real-time deadline. The maximum chunk measurement reports observed latency, not a guaranteed upper bound. Do not interpret these native results as a production speedup without repeated paired-game workloads and numerical/event equivalence.

## Reproduction

Use the pinned `1.98.0` toolchain and `--locked --offline`.

```text
cargo +1.98.0 test --locked --offline --target i686-pc-windows-msvc --workspace --no-fail-fast
cargo +1.98.0 test --locked --offline --target x86_64-pc-windows-msvc -p dogmos-core -p dogmos-protocol -p dogmos-server --no-fail-fast
cargo +1.98.0 clippy --locked --offline --target i686-pc-windows-msvc --workspace --all-targets -- -D warnings
cargo +1.98.0 build --locked --offline --release --target x86_64-pc-windows-msvc -p dogmos-perf --example core_stage_allocations
core_stage_allocations --output candidate.csv --baseline tmp/performance-repair-baseline.csv
```

The probe writes a companion `candidate.updates.csv`. Compare deterministic counts and hashes separately from `max_chunk_ns` and update timings. For final deployment qualification, build the shim and service together, regenerate their contract with maintained tooling, and run the paired game's compile, boot, focused and full runtime gates before accepting live performance claims.
