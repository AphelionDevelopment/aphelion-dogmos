# Meridian-MCP profiling findings

This sheet records issues found while using Meridian-MCP for sustained DreamDaemon performance and
memory investigation. It is separate from Dogmos acceptance evidence: a tooling defect cannot be
reported as a Dogmos regression or improvement.

## Tested stack

| Component | Identity |
| --- | --- |
| Meridian-MCP | 0.1.0 |
| Tracy protocol | 82 |
| Tracy helper source | `099df3de3dc37eca4712c06b8320fb9c53596edd` |
| byond-tracy source | `d1ec404737b04b1ea73d6df4a1b477deacdb1900` |
| SpacemanDMM source | `351ddc0ffb2439876d4565ce5130bb6b027ee605` |
| BYOND | 516.1685, Windows x86 DreamDaemon |

## Findings

| ID | Priority | Area | Reproduction and observed evidence | Required improvement | Acceptance test |
| --- | --- | --- | --- | --- | --- |
| MMCP-PROF-001 | P0 | Capture validity | Launch a profiled server, leave the profiler disconnected through an approximately 95-second station boot, then capture for either 5 or 30 seconds. Both captures were reported as successful 217-byte artifacts with three frames, zero zones, and a negative span. | Reject traces with a negative span, zero zones after an active interval, non-positive frame durations, or implausibly small payloads. Return a structured invalid-capture error and retain the raw diagnostic artifact separately. | A saturated or malformed producer stream cannot return `success: true`; the response identifies each failed invariant. |
| MMCP-PROF-002 | P0 | Producer queue | The fixed byond-tracy event queue can saturate while no client is attached. Attaching immediately after launch produced 257,858 zones in five seconds; waiting through boot produced zero-zone captures, and repeated captures did not recover the process. | Expose queue capacity, current depth, dropped events, saturation count, and producer health. Add a supported always-connected capture session or a producer reset/rearm operation. | A server can boot for two minutes before the first requested window and still produce a valid idle trace without relaunching DreamDaemon. |
| MMCP-PROF-003 | P0 | Frame statistics | An immediate five-second capture reported 38 frames, but one boundary frame was `854546561391182 ns`; mean, p99, and span became unusable while the median remained plausible. A continuously attached 150-second capture similarly reported an impossible `69277243870517736 ns` maximum. | Distinguish complete frames from connection-boundary frames. Exclude partial first/last frames from percentiles by default and return excluded-frame counts and reasons. Add plausibility checks against capture duration. | A five-second local capture has only positive complete-frame durations; boundary frames are enumerated separately and cannot dominate p95/p99. No frame duration exceeds the capture duration without a structured clock-domain error. |
| MMCP-PROF-004 | P1 | Capture lifecycle | `dm_tracy_capture` connects for one bounded window and disconnects. A high-event-rate DreamDaemon can fill the producer queue between sequential tool calls, preventing reliable repeated controls. | Add `dm_tracy_capture_start`, window/checkpoint, and stop semantics over one retained connection, or keep the worker connected between bounded exported windows. | Three consecutive 30-second windows after boot contain monotonic timestamps, non-zero zones, and no saturation without restarting the game. |
| MMCP-PROF-005 | P1 | Hook diagnostics | `dm_tracy_status` confirms process, port, and helper identity but does not report whether all three byond-tracy hooks installed, whether the producer is advancing, or why it stopped. | Surface hook initialization result, BYOND build/offset table identity, hook addresses/prologue validation, events produced, and last producer error. | Status distinguishes healthy-idle, disconnected-buffering, saturated, hook-failed, and capture-active states. |
| MMCP-PROF-006 | P1 | Experiment identity | Capture results omit the DMB hash, loaded native module hashes, map, seed, configuration/features, BYOND executable hash, and phase marker. | Emit a capture manifest binding the trace to exact executable and workload identity. Permit user-supplied immutable run metadata and hash it into the result. | Comparing traces with mismatched map, seed, build, module, or scenario identity is rejected before statistics are calculated. |
| MMCP-PROF-007 | P1 | Time windows | Hotspot and frame tools analyze the whole trace only. They cannot select an idle interval after boot or align the same marker-delimited phase across repetitions. | Support timestamp/frame ranges and named phase markers for hotspots, zones, frames, and comparisons. | A boot-plus-idle trace can yield an idle-only p95/p99 and hotspot table using recorded marker boundaries. |
| MMCP-PROF-008 | P1 | Memory correlation | Tracy tools do not sample exact DreamDaemon/service PIDs or align OS memory observations to profiler time. | Add an optional, explicit process-sampling companion with separate roles and timestamps. Never add service memory to DreamDaemon memory. | A capture returns aligned DreamDaemon private/virtual/working-set samples and a distinct optional service series with no combined-memory field. |
| MMCP-PROF-009 | P2 | Statistics semantics | `span_ns`, `frame_count`, `GetFrameCount`, and complete-frame percentile semantics are not stated in the tool response. The capture can report a frame count different from the later full-frame query. | Version and document field semantics; return raw count, complete count, partial count, and analyzed count. | Schema tests prove capture and query counts differ only for explicitly classified partial frames. |
| MMCP-PROF-010 | P2 | Automated controls | There is no MCP-native repetition/noise workflow or minimum-quality gate for three clean controls. | Add a repeated-control command that validates each trace, reports coefficient/range noise, and refuses to establish a budget from invalid or mixed-identity runs. | Three valid controls produce a noise envelope; one malformed run makes the batch incomplete rather than silently widening the budget. |
| MMCP-PROF-011 | P2 | Network evidence | Capture responses currently report network auditing as unavailable even when explicitly modeling a loopback profiler endpoint. | Report the profiler listener and accepted loopback connection directly from owned process state; reserve packet-level audit for environments where it is available. | The result proves the expected loopback endpoint and client connection without claiming unavailable packet capture as complete evidence. |
| MMCP-PROF-012 | P0 | Clock domain | A 150-second continuously attached capture contained 20,904,031 zones and 811 frames, proving that events arrived, but reported a `69277352227889548 ns` span. Hotspots then assigned multi-hour or multi-day durations to procs which completed during a 95-second boot. | Validate and normalize producer timestamps against the Tracy protocol clock before saving. Record clock source, frequency, conversion factor, first/last raw timestamp, and monotonicity failures. Refuse duration statistics when the trace clock is inconsistent with wall duration. | A known 150-second capture reports a span within a documented tolerance of wall time, all durations are non-negative, and a synthetic clock-frequency fixture detects unit or frequency mismatch. |
| MMCP-PROF-013 | P1 | Launch readiness | `dm_tracy_launch` returned `success: true` with a profiler port, but an immediately following 130-second capture failed with `Timed out connecting to the Tracy client`. A later capture connected and received millions of zones from the same process. | Do not report launch readiness until the hook listener is accepting the verified helper, or expose separate process-started, hook-loaded, listener-bound, and capture-ready states plus a bounded wait command. | Launch followed immediately by capture succeeds repeatedly without a retry or reports a structured not-ready state before consuming the requested capture window. |
| MMCP-PROF-014 | P1 | Artifact persistence | Captures targeting a missing output directory returned successful artifact paths and hashes, but the directory and traces were absent when queried after the sequence. Pre-creating the same directory produced a persistent artifact. | Require an existing parent or create it durably outside temporary-cleanup ownership. Verify the final artifact still exists after the response is assembled and document whether stop/relaunch may remove any path. | A capture to a newly created contained directory remains readable by frame and hotspot tools after later captures and after `dm_tracy_stop`; a missing artifact cannot be returned as successful. |
| MMCP-PROF-015 | P0 | Workspace integrity | After the profiling launch/capture/stop sequence, the disposable contained game mirror reported 2,809 tracked deletions, including map sources, build tools, and `__odlint.dm`. The exact responsible MCP lifecycle call has not yet been isolated, so this is evidence of session-level source erosion rather than attribution to one command. The separate implementation worktree and user checkout were not used as runtime roots. | Snapshot the contained tree before every owned runtime, restrict cleanup to an explicit generated-artifact allowlist, and compare the tree after stop. Abort with a structured integrity error on any undeclared source deletion. Add an operation journal identifying which lifecycle action removed each owned path. | Launch, capture, and stop a profiled server repeatedly in a clean worktree; `git status --porcelain` remains unchanged except for explicitly declared, ignored runtime artifacts. A fixture proves tracked source and build files can never enter cleanup targets. |
| MMCP-PROF-016 | P2 | Upgrade identity | The 2026-08-27 upgraded-tool regression still identified Meridian-MCP only as `0.1.0`, the same identity recorded before the upgrade. Functional behavior changed, but the tool response could not bind the result to an exact MCP source revision or binary hash. | Report the MCP executable hash, source revision, dirty state, build profile, and schema version in status and profiling lifecycle responses. | Two locally built `0.1.0` binaries from different revisions have distinguishable identities in captured evidence without relying on installation timestamps or inferred behavior. |
| MMCP-PROF-017 | P1 | Worktree access | `dm_parse_environment` accepted the primary Meridian-Rift checkout but rejected the isolated `.codex` worktree containing the approved Dogmos changes with `path_outside_workspace`, despite the upgraded MCP being described as uncontained. The response did not expose the effective allowed roots or whether the restriction came from MCP policy, host policy, or stale configuration. | Report effective containment mode and allowed roots in status and every path-policy failure. Support explicitly authorizing another worktree of the same repository without requiring source changes in the user's primary checkout. | Parse the primary checkout and an explicitly authorized sibling Codex worktree in one session; both report their resolved roots and repository identities, while an unrelated path still fails with a structured policy source. |
| MMCP-PROF-018 | P1 | Proc lookup | After parsing Meridian-Rift, `dm_search_context` and `dm_document_symbols` found the declared `/datum/dogmos_kennel/ui_data` override in `code/controllers/subsystem/dogmos_kennel.dm`, but `dm_get_proc` for type `/datum/dogmos_kennel` and proc `ui_data` returned `declared: false` and only the inherited implementation. Search metadata also inconsistently reported the symbol parent as `/datum`. | Resolve exact overrides from the canonical symbol owner used by document symbols, and include the candidate paths and normalization decision when exact lookup falls back to inheritance. | A fixture with a parent proc and child override returns the child's body from `dm_get_proc`; search, document symbols, definition, and proc lookup agree on the same owner path. |
| MMCP-PROF-019 | P1 | Standard-run integrity | The disposable full-map `dm_run` workload modified tracked `icons/obj/fluff/map_previews.dmi` from Git blob `e12a762cb21659380f6efdad2d66da72722f7931` to `930ba485c240a0ed39365392eedcfc7713bd523f` while generating condo previews. This is game behavior rather than evidence that MCP deleted the file, but standard run/stop did not surface the new tracked mutation. | Extend the pre/post integrity journal to standard `dm_run`/`dm_stop`, report every tracked mutation separately from MCP-owned artifacts, and identify the runtime phase/output marker nearest the change. Never silently revert it. | A fixture intentionally rewrites a tracked artifact during runtime; `dm_stop` succeeds but returns a structured source-integrity warning naming the path, before/after blobs, and owning process/session. |
| MMCP-PROF-020 | P1 | Native evidence ingestion | A real playtest produced cumulative BYOND proc-profile JSON, sendmaps JSON, a ten-second performance CSV, structured runtime JSONL, and a Dogmos event log. Meridian-MCP could inspect the matching source but had no command to validate, phase-align, summarize, or compare these native artifacts. Manual parsing was required, and the proc profiles could easily have been mistaken for live-stress captures even though their timestamps ended before game start. | Add bounded readers for BYOND proc profiles, sendmaps profiles, performance CSV, structured logs, and user-defined JSONL events. Bind every result to artifact hash and run identity, classify cumulative versus interval data, align world time and wall time, redact player identifiers by default, and support named phase windows plus repeated-run comparison. | Given a fixture with startup profiles and later fire events, the tool identifies the profiles as pre-game cumulative data, returns percentile tables for selected CSV columns, groups runtime signatures, correlates event phases without exposing player identifiers, and rejects comparisons with different build identities. |
| MMCP-PROF-021 | P1 | Fixture and artifact provenance | After refreshing the contained IPC fixture with protocol-v4 binaries, its DM harness no longer called the state-batch proc and its generated bindings did not declare that proc. Adding the intended call made `dm_compile` fail with `undefined proc`, while the previous DMB remained present at its old hash. The compile response correctly reported failure and `dmb_updated: false`, but a later launch request could still be given that stale DMB path. | Maintain a hash-bound fixture manifest covering DM source, generated bindings, native modules, service executable, and DMB. Mark an existing DMB stale after any source/input change or failed compile, and refuse `dm_run` unless the artifact matches the latest successful compile manifest. Provide a fixture-sync check that reports missing generated procs before launch. | Change a fixture source or remove one generated binding while retaining an older DMB. Compilation fails, the old DMB is classified as stale, `dm_run` refuses it with the mismatched input hashes, and restoring all manifest inputs permits a fresh compile and launch. |

## Confirmed useful behavior

- Containment, helper hash verification, fixed loopback ports, and MCP-owned process shutdown worked.
- DreamMaker compilation returned exact executable, output, artifact hash, duration, warnings, and errors.
- Immediate profiler attachment produced real procedure zones, proving that the hook/helper protocol can
  work on the tested BYOND build.
- Process memory can be sampled independently by exact PID while MCP owns DreamDaemon; this is the
  correct basis for keeping DreamDaemon and a future Dogmos service separate.

## Investigation run log

| Experiment | Wall window | Result | Classification |
| --- | --- | --- | --- |
| Delayed first capture | 5 and 30 seconds after boot | 217 bytes, three frames, zero zones, negative span | Invalid; queue/lifecycle diagnostic only |
| Immediate attachment | 5 seconds after launch | 257,858 zones and 38 frames, but an impossible boundary frame | Hook transport proven; timing statistics invalid |
| Continuous attachment | 150 seconds across boot and idle | 103,952,005 bytes, 20,904,031 zones, 811 frames, impossible 69-quadrillion-nanosecond span | Queue workaround proven; clock statistics invalid |
| Consecutive capture 1 | 30 seconds during boot | 4,953,442 zones and 213 frames, impossible span | Event evidence only; not an idle control |
| Consecutive capture 2 | 30 seconds during boot | 7,224,645 zones and 202 frames, impossible span | Event evidence only; not an idle control |
| Consecutive capture 3 | 30 seconds after initialization | 217 bytes, three frames, zero zones, negative span | Invalid; reconnect did not remain healthy |
| Exact-PID memory sampling during capture sequence | Three separate 30-second windows | 115, 116, and 115 DreamDaemon samples; service process absent and therefore not combined | Valid process evidence; first two windows include boot growth, so only the third is near-idle |
| Exact-PID idle-memory controls | Three new 30-second windows after `Initializations complete` | 114 samples each; all three reported `2183696384` private bytes and `2166235136` committed-private bytes | Valid zero-observed-noise idle-memory control for this process; no timing budget inferred |
| Minimal DreamDaemon call_ext fixture | Marker-delimited baseline, 100,000-call loop, 512 MiB service allocation, release, and shutdown | MCP reliably launched, matched output markers, retained exact DreamDaemon PID, and stopped the process; separate exact-PID sampling showed the service arena remained outside DreamDaemon | Valid cross-process call-path and isolation evidence; timing came from the fixture's monotonic boundary clock, not Tracy |
| Parent-death fault injection | Stop MCP-owned DreamDaemon while the logged dogmosd child is active | `dm_stop` terminated DreamDaemon and the Windows kill-on-job-close guard removed the exact service PID within five seconds | Valid lifecycle evidence |
| Full-map operation capture | Parse and diagnose a disposable `PERFORMANCE_TESTS` build, launch MetaStation, wait through 159-second initialization and a 60-game-second marker window, then stop | Source diagnostics were clean; bounded output waits exposed the original nested-callback runtime, then reliably found start/complete markers after the disposable repair | Valid source/runtime orchestration evidence; operation data came from Dogmos telemetry rather than Tracy |
| Bounded callback-pressure fixture | Parse and diagnose the contained fixture, launch four fresh DreamDaemon processes, wait on baseline/saturated/drained/complete markers, and stop | `dm_wait_for_output` reliably synchronized all phases; three paired exact-PID samples showed the 65,536-event backlog in `dogmosd`, and all four runs drained ordered events without runtime errors | Valid standard-run orchestration and memory evidence; no Tracy timing claim |
| Player-driven atmosphere stress | Parse the matching contained source and assess one Minimal Runtime Station run with fusion-test canisters, clustered tritium fire, and a later plasma fire | Native BYOND profiles ended before game start; 33 performance rows and 584 Kennel events covered live stress; manual analysis found no Dogmos runtime and exposed missing equalize/post-process CSV columns | Valid playtest smoke and telemetry-design evidence; not a repeated timing or memory qualification |

## Protocol-v3 standard-run validation

The upgraded Meridian-MCP parsed the contained 18-symbol IPC benchmark DME, then launched three
fresh standard DreamDaemon processes and synchronized on baseline, callback-saturated,
callback-drained, and complete markers. All three runs exposed the exact DreamDaemon and service
PIDs, retained ordered output through completion, and stopped cleanly. Exact-PID process memory
remained a PowerShell measurement responsibility. No new MCP blocker appeared in this workflow; the
resulting 60-sample-per-role evidence is recorded in `docs/performance/ipc-decision.md`.

## 2026-08-27 upgraded-tool regression

The upgraded Meridian-MCP was tested against a new disposable worktree at exact Meridian-Rift
revision `1623a76079a6617598498eaf7f5778f8564ed314`. Before launch, the worktree had exactly two
intentional tracked modifications: the synchronized Dogmos DLL and generated bindings. After
`dm_tracy_launch`, an immediate capture attempt, and `dm_tracy_stop`, the same two modifications
remained and there were zero tracked deletions. The 2,809-file erosion recorded in MMCP-PROF-015 did
not recur, so that issue is provisionally fixed for this scenario. Repetition and a dedicated
cleanup fixture are still required before closing it.

Launch readiness remains open. `dm_tracy_launch` reported success for DreamDaemon PID 18732 and
profiler port 50212, but an immediately following five-second `dm_tracy_capture` returned `Timed out
connecting to the Tracy client.` No trace existed for frame analysis. This is a direct reproduction
of MMCP-PROF-013 on the upgraded tool.

The disposable inputs were hash-bound as follows:

| Artifact | SHA-256 |
| --- | --- |
| `dogmos.dll` | `6971B36955024D515287E2451C3A6FBEE743DEB0D991D7BC5135A3922E238105` |
| generated bindings | `051D138C4062309D387AFDEF8F9519D956BC6640F93BBFBAC042519C8D36003B` |
| `tgstation.dmb` | `73014E3CB85259DF0636FCD205C08F5D6E72875857E43DD42EB375447F669900` |

## Evidence handling

Raw traces and process samples remain under ignored `tmp/dogmos-perf/`. A trace is clean only when it
has a positive span, non-zero zones, enough complete frames for the requested percentile, no queue
saturation evidence, and the full workload identity. Failed reproductions are diagnostic evidence,
not performance baselines.

## 2026-08-27 remediation implementation status

The `over64-byond-ci-recovery` Meridian-MCP worktree now contains code and fixture coverage for the
following boundaries. This is an implementation disposition, not new performance acceptance
evidence; live BYOND qualification remains pending until the repeated integration gates produce
valid traces from the exact tested MCP binary.

| Finding | Current disposition |
| --- | --- |
| MMCP-PROF-001 | Implemented and fixture-verified. Invalid helper or Rust-revalidated captures return a structured `invalid_capture` result, are retained under `.meridian-tracy-diagnostics`, and cannot enter authoritative capture records or statistics. |
| MMCP-PROF-002, 004, 005 | Implemented and native-fixture-verified. The collector retains a drain worker, performs one bounded retry for transient attach failures, exposes explicit transition/worker/retry state, and reports named queue, hook, prologue, offset-table, saturation, drop, and producer telemetry. Startup does not become ready until producer progress and queue health are both valid. Delayed live capture remains to be proven. |
| MMCP-PROF-003, 006, 007, 009, 010, 011, 012 | Existing implementations remain covered by the expanded qualification contract: complete/partial frame classification, phase windows, immutable experiment identity, repeated controls, owned-loopback network evidence, and raw/trace clock validation are required before evidence is accepted. |
| MMCP-PROF-008 | Partially implemented. Exact owned-process memory series remain separated by process role and align to capture time. A distinct future `dogmosd` series still requires the cross-process service implementation and must not be inferred from current in-process runs. |
| MMCP-PROF-013 | Code path remediated and native-fixture-verified. Launch readiness now requires valid queue/hook telemetry; capture reconnection is bounded and reports whether the capture window started and whether drain recovery succeeded. Immediate live BYOND repetition is still required before closure. |
| MMCP-PROF-014 | Implemented and Rust-fixture-verified. Publication uses reserved atomic trace/sidecar paths under an explicit existing experiment directory, and missing or colliding destinations cannot be reported as successful artifacts. |
| MMCP-PROF-015 | Implemented and Rust-fixture-verified. Each experiment uses a durable `.meridian-tracy-session.json` journal with pre/post lifecycle checkpoints, exact owned-output exceptions, atomic journal writes, structured integrity failures, and an unfinished-session recovery gate. Repeated clean-worktree live runs remain required before closure. |
| MMCP-PROF-016 | Implemented and Rust-fixture-verified. Every tool result and profiling artifact includes the MCP source revision, dirty state, build target/profile, executable SHA-256, and derived build ID; comparison rejects different MCP build IDs. CI supplies the authoritative GitHub revision. |
| MMCP-PROF-017, 018 | Not changed by this remediation. Worktree authorization diagnostics and exact child-override proc lookup remain separate open findings. |
| MMCP-PROF-019 | Newly observed after remediation. Tracy experiment journaling does not yet cover tracked mutations produced by standard `dm_run` workloads. |
| MMCP-PROF-020 | Open. Native BYOND profiling and structured runtime artifacts still require manual phase and identity validation. |
| MMCP-PROF-021 | Newly observed after remediation. Compile failures retain the previous DMB without a launch-time source/artifact freshness gate. |

The updated integration contract performs an immediate capture, waits through the drain interval,
then requires three valid steady-state captures with positive raw and trace ranges, complete frames,
non-zero zones, zero queue saturation/drops, one exact MCP build identity, and a finalized integrity
journal. Passing fixture tests does not substitute for that live gate.

### Live qualification result

The final Windows qualification passed on BYOND 516.1687 with Meridian-MCP build ID
`60b2266e44c419d1a36ed56ec70b8200fa9de992b8dae64fc0aadc2a51730559` and executable SHA-256
`e7e1255e8335f3932191b5fa6f457ad74387a208561014ccbb6a67cb65c06a69`. It completed one immediate
30-second capture, retained the drain worker for 120 seconds, then completed three consecutive
30-second steady-state captures. All four trace contracts passed with non-zero zones, positive raw
and normalized time ranges, at least three complete frames, zero saturation/drops, exact helper and
MCP identity, restored drain-worker health, clean source-integrity checkpoints, and a finalized
journal. The final trace reported 299 complete frames and the known fixture proc was found by both
hotspot and exact-zone queries. This closes the tested Windows scenarios for MMCP-PROF-001, 002,
004, 005, 013, 014, 015, and 016. Ubuntu native and live Linux BYOND qualification remain CI-owned
and are not implied by this Windows result.
