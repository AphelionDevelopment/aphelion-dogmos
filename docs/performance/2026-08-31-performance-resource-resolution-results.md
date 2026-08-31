# Performance and resource-management resolution results

Date: 2026-08-31

## Scope and source state

This handoff records the uncommitted implementation of
`docs/superpowers/plans/2026-08-30-performance-resource-resolution-plan.md` against source revision
`d8dc92d8fd3d268abd89463c5818f174bf077b48`. No protected manifest, workflow, dependency,
transport, release, generated-binding, or deployment file was changed.

The result separates repository-local Rust evidence, synthetic performance evidence, and the
paired Meridian-Rift runtime gate. It does not treat process liveness, a DreamMaker compile, or a
service RSS sample as DreamDaemon performance qualification.

## Finding status

| Audit finding | Status | Resolution |
| --- | --- | --- |
| Chunked stages ignored live callback capacity | Resolved locally | The core validates the active event ceiling before component publication; the server passes its live remaining capacity and maps overflow to backpressure. Exact-fit, overflow, retry, cancellation, and earlier-component commit behavior are tested. |
| Callback conversion drained authoritative events before fallible allocation | Resolved locally | Callback batches are prepared from borrowed events, destination and continuation capacity is reserved first, and the final commit performs no fallible allocation or encoding. Injected reservation/commit failures retain one authoritative event owner and retry once in order. |
| Legacy reaction ownership survived shutdown | Resolved locally | Both the Rust reaction registry and `REACTION_INFO` are cleared at shutdown. Reuse tests prove an old identifier resolves only to its replacement. |
| Legacy callback bytes counted only a boxed handle | Resolved locally | Each callback carries a producer-supplied heap-capacity lower bound. Current, high-water, and cumulative lower-bound counters are separate from item counters and reset at the world boundary. |
| Legacy callback admission remained unbounded | Deferred at the mandatory checkpoint | The paired checkout exercises the service backend, not the retained in-process callback workload. No evidence-derived item or byte limit was invented and critical-event behavior was not changed. See `2026-08-30-callback-queue-pressure-results.md`. |
| Component-stage allocation regression | Resolved for the measured Rust workload | The component queue is moved rather than cloned and published membership uses a slot-indexed generation marker. All six 100,000-turf rows returned to the qualified allocation/byte values with exact transcript and work equivalence. See `2026-08-31-component-stage-recovery-results.md`. |
| Successful shim decode paths allocated indexed labels | Resolved locally | Valid callback and snapshot scalars no longer format labels. Overflow failures retain exact index context and allocation probes cover successful full batches. |
| Bounded client teardown could detach a worker | Resolved locally | The client has bounded explicit close/cancel/join behavior and `ServiceSession` calls it only after the child is interruptible. Repeated timeout cycles verify worker and Windows handle cleanup. |
| Legacy heat work occupied a Rayon worker for the world lifetime | Resolved locally | Heat processing uses an owned named thread, explicit shutdown message, bounded join, and caller-legible shutdown errors. Reuse tests prove the worker and scratch owner are released before rearm. |
| Legacy gas/turf shutdown retained large arena capacity | Resolved locally | Gas, free-slot, turf graph, map, planetary atmosphere, and heat owners are taken at shutdown and explicitly recreated before a new world. Metrics report zero capacity while stopped. |
| Gas-slot reuse can scan free slots times all turfs | Skipped by evidence | No representative profile identifies the path as material. Reference-count synchronization state was not added without the plan's prerequisite trace. |
| Frontier telemetry omitted committed storage | Resolved in core diagnostics | Upload scratch retains its old meaning. Committed length, capacities, and a vector-entry lower bound are exposed separately; hash-table control bytes and allocator metadata remain documented exclusions. No wire or protocol field changed. |

## Deterministic performance evidence

Three control and three candidate core-stage CSVs were captured in one session. Every control was
byte-identical with SHA-256
`5A7179FD52E070C70A9991A66AF47DB9E033353CB3CAFEB3503420B866156895`; every candidate was
byte-identical with SHA-256
`CBB6468F4AC4331AB784BCD2F64456E125D019AC4BF91E07DFC3C60E0B59DBF9`.

Across corridor, grid, and multi-z 100,000-turf equalize and excited-group rows, the candidate
reduced allocation counts and bytes in every row and matched the previously qualified `e9f5726`
values. Transcript hashes, work counts, and event behavior were unchanged. The candidate's peak
vector-capacity lower bound is 800,000 bytes higher because the new 100,000-entry generation marker
is now counted; the control omitted ordered-set node, bucket, and allocator overhead. Retained
vector-capacity lower bounds are unchanged.

The same-session maintained IPC series kept the affected
`service_simulation_stage_1024_mixtures_32_gases` medians within the 5 percent budget: p50 +2.37
percent, p95 +1.03 percent, and p99 +2.62 percent. Short-process service and shim samples remained
separate and are not DreamDaemon memory results. Transport-only percentile movement was treated as
scheduler noise because those paths and payloads were unchanged.

## Fresh local qualification

Pinned compiler: `rustc 1.98.0 (88d9e12ae 2026-08-18)`.

| Gate | Result |
| --- | --- |
| `cargo +1.98.0 fmt --all -- --check` | Pass |
| strict workspace Clippy, i686, all targets, locked | Pass with `-D warnings` |
| full locked i686 workspace tests | Pass; executable tests have zero failures, two documentation tests remain intentionally ignored |
| locked x64 core/protocol/server/process-metrics/identity tests | Pass |
| maintained i686 feature matrix | Pass for all 12 supported configurations |
| `python -m unittest discover -s tools/tests -p 'test_*.py'` | Pass: 42 tests |
| generated `crates/dogmos-byond/bindings.dm` comparison | Exact; SHA-256 before and after `01F09EDDA244D4B606CCA40FF51B375F366F1DB82EC1837E8B4DC1C85EA9F771` |
| `git diff --check` | Pass |

Candidate Windows artifact hashes used for the attempted paired gate were:

- i686 `dogmos_byond.dll`: `A7E98CDADB84ACED96766C7D09B1697F02B99C185D67875D2169E36D21EE33F2`;
- x86_64 `dogmosd.exe`: `E74D2D13A16425DD9F3D00DF9DE6311C51893CE87C30035D5475AF07ECA6734F`.

## Paired Meridian-Rift gate

The maintained focused runner compiled the paired checkout with BYOND 516.1687: 0 errors and two
expected debug/test warnings. Runtime qualification did not start. The generated DM contract
requires source revision `a09f26ab8dffc6d6a1c50274ea1ced5dc5953ab7`; the candidate correctly
reported `d8dc92d8fd3d268abd89463c5818f174bf077b48`, so the Dogmos subsystem failed closed before
service initialization. The later mixture runtimes were consequences of that rejected
initialization and are not evidence of an arena or numerical regression. The run produced no fresh
unit-test result artifact and is recorded as failed, not partial success.

The temporary focus include, DME edit, map selector, and processes were removed or restored by the
runner. An earlier interrupted attempt had already overwritten the paired checkout's two installed
binaries before its cleanup ran. Their pre-run hashes are recorded by the dirty lock manifest as:

- `dogmos.dll`: `FE648E6CAE59F47AE2F152F25BB6AC1ECE1ACA158C37AE45D62BB48E4FAA86B9`;
- `dogmosd.exe`: `5604A8815557E42F74A8ECF2D41F19825BB67A834DC4604774C78F3E91D9F4DE`.

No matching local copy, symbol file, release bundle, repository object, or user-profile backup was
found. A clean rebuild of the recorded source revision reproduced the artifact sizes and identity
but not the PE/PDB-derived hashes. The protected lock manifest was not changed. Exact restoration
therefore requires the original artifact pair, or explicit authorization to regenerate and install
a coherent paired contract through the maintained release/sync tooling.

## Unrun or externally blocked acceptance gates

- Linux cross-target builds were not rerun in this Windows session.
- Paired focused tests, boot probe, hard-restart/world-reuse probe, and full DM suite are blocked by
  the source/contract mismatch.
- Three paired control and three paired candidate live workloads were not run.
- No live DreamDaemon private/committed/address-space series, separate live `dogmosd` RSS series,
  SSair headroom series, or callback queue live high-water series was captured.
- The production target of at least 70 percent lower DreamDaemon Dogmos-attributable peak private
  bytes is not claimed.

All implementation changes remain uncommitted for review.
