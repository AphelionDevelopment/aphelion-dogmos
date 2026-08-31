# Component-stage allocation recovery results

Date: 2026-08-31

## Scope

This measurement covers removal of the component queue clone and replacement of the ordered published-mixture set with a slot-indexed generation marker. It does not qualify DreamDaemon memory or live-round behavior.

The control and candidate used Rust 1.98.0 with locked, offline dependencies. Each core-stage artifact was captured three times in the same session. All three control files are byte-identical with SHA-256 `5A7179FD52E070C70A9991A66AF47DB9E033353CB3CAFEB3503420B866156895`; all three candidate files are byte-identical with SHA-256 `CBB6468F4AC4331AB784BCD2F64456E125D019AC4BF91E07DFC3C60E0B59DBF9`.

Artifacts are under `tmp/dogmos-perf/component-recovery-{control,candidate}-{1,2,3}.csv`.

## Deterministic 100,000-turf results

Transcript hashes, work-item counts, and event behavior match exactly between control and candidate. Allocation counts and allocated bytes improved in every component-stage row.

| Stage | Topology | Allocations, control | Allocations, candidate | Allocated bytes, control | Allocated bytes, candidate |
| --- | --- | ---: | ---: | ---: | ---: |
| Equalize | Corridor | 136,128 | 119,462 | 62,557,472 | 60,595,728 |
| Excited groups | Corridor | 267,476 | 259,144 | 43,764,592 | 42,783,920 |
| Equalize | Grid | 131,844 | 115,828 | 61,964,568 | 60,092,408 |
| Excited groups | Grid | 266,982 | 258,650 | 43,719,824 | 42,739,152 |
| Equalize | Multi-z | 135,720 | 119,057 | 62,509,572 | 60,548,332 |
| Excited groups | Multi-z | 233,101 | 225,209 | 42,658,856 | 41,732,584 |

The candidate allocation and allocated-byte values equal the previously qualified `e9f5726` artifact for all six rows.

## Workset telemetry

The CSV separates peak active vector-capacity lower bound from post-stage retained vector-capacity lower bound. The candidate peak is 800,000 bytes higher for each 100,000-turf component row because it now truthfully counts the 100,000-entry `Vec<Option<u32>>` generation marker. The control omitted the ordered set's node, bucket, and allocator overhead, so its lower bound was incomplete. Post-stage retained lower bounds are unchanged.

Both lower bounds exclude map/set node or bucket overhead and allocator metadata. They must not be interpreted as process RSS or private bytes.

## IPC controls

The maintained IPC benchmark ran three 20,000-iteration controls and three candidates in the same session. Artifacts and per-process memory samples are under `tmp/dogmos-perf/ipc-control` and `tmp/dogmos-perf/ipc-candidate`.

The affected `service_simulation_stage_1024_mixtures_32_gases` case stayed within the 5% budget:

| Percentile | Control median | Candidate median | Delta |
| --- | ---: | ---: | ---: |
| p50 | 1,226,900 ns | 1,256,000 ns | +2.37% |
| p95 | 1,444,000 ns | 1,458,900 ns | +1.03% |
| p99 | 1,643,100 ns | 1,686,200 ns | +2.62% |

The service private-byte median was 1,978,368 bytes for control and 1,966,080 bytes for candidate. The 32-bit shim private-byte median was 937,984 bytes for control and 1,007,616 bytes for candidate. These are startup samples from short synthetic IPC processes, not DreamDaemon measurements.

Several transport-only p95 medians moved more than 5% while their code and payloads were unchanged. Those shifts are treated as scheduler noise rather than evidence about the component-stage change; the affected service-stage case is the acceptance signal for this task.

## Result

The component-stage allocation regression is recovered locally with deterministic numerical/event equivalence and focused generation, capacity-rejection, retry, cancellation, and cross-component tests. Live DreamDaemon qualification remains a separate release gate.
