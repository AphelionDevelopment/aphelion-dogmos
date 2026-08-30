# Dogmos performance evidence

Reviewed 2026-08-30 resource-management results are in
[`2026-08-30-resource-management-results.md`](2026-08-30-resource-management-results.md). The
separate transaction follow-up is specified in
[`2026-08-30-transaction-scratch-arena-design.md`](2026-08-30-transaction-scratch-arena-design.md).

This directory defines reproducible workloads and acceptance budgets for the current in-process
Dogmos backend and the later 64-bit service. DreamDaemon memory and service-process memory are
always recorded separately. Only DreamDaemon footprint is used for the BYOND memory target.

Every live result records the exact map, seed, Rust revision, feature set, BYOND version, duration,
and SHA-256 of the workload file. Results with different identities are not comparable. Raw output
belongs under ignored `tmp/dogmos-perf/<revision>/<run-id>/`; checked-in documents contain only the
workload contract, measured noise budget, and reviewed summaries.

Use `tools/perf/Invoke-DogmosWorkload.ps1 -ValidateOnly` to validate the corpus. Use
`tools/perf/Measure-DogmosProcesses.ps1` to sample exact DreamDaemon and optional `dogmosd` PIDs.
Use `tools/perf/Compare-DogmosPerformance.ps1` to reject incompatible runs and calculate deltas.
DreamMaker source discovery and Tracy capture must go through Meridian-MCP after
`dm_parse_environment`; PowerShell owns process sampling and checked-in build/test entry points.

The live workload profiles require explicit in-game markers. A profile is not accepted merely
because DreamDaemon remained alive: every listed marker and correctness assertion must be recorded.

## Core stage allocation probe

Run the native core-only allocation probe in a fresh process:

```powershell
cargo +1.98.0 run --release --locked -p dogmos-perf --example core_stage_allocations -- --output "tmp/dogmos-perf/core-control.csv"
```

The probe constructs corridor, grid, and three-layer multiz fixtures at 1,000, 10,000, and 100,000
turfs. It resets an atomic `System` allocator wrapper after each fixture is built and measures one
complete process-turfs, turf-heat, equalize, or excited-groups stage. The CSV reports allocations,
deallocations, allocated and deallocated bytes, charged work items, a final-state and ordered-event
transcript hash, and the active reusable-vector capacity lower bound. Allocation counts rank
allocation-removal work; they are not wall-time acceptance evidence.

`reusable_workset_bytes` is a lower bound over active vector capacities. It includes the complete
four-field heat-edge tuple and excludes maps, sets, and allocator metadata. Stage-owned state is
dropped when that stage commits; retained event capacity can remain visible until events are
drained.

Three fresh release processes on 2026-08-30 produced byte-identical 36-row controls with SHA-256
`9C28A0C6D019941F66237EB99B221DCF8BA9C29D2E68852829938C1B520EFE30`. At 100,000 turfs,
process-turfs made 133,396 allocations and allocated 79,892,176 bytes for every topology.
Turf-heat made 133,425-133,428 allocations and allocated 24,949,008-29,142,704 bytes. Equalize was
the largest byte allocator at 197,006,624-198,765,028 bytes and 264,873-269,491 allocations;
excited-groups made 330,967-359,948 allocations and allocated 157,116,200-158,375,520 bytes. These
controls prioritize component traversal and per-turf diffusion churn while preserving distinct
transcript hashes for each topology and stage.

Replacing the process-turfs neighbor `Vec` with the topology's six-entry stack bound produced three
byte-identical 36-row candidate runs with SHA-256
`7F4733C1E9BC9FECD003F352D34A9F05721308C723F839A31E83BBE2A44A36B4`. Every process-turfs
transcript hash matched its control. Each fixture eliminated exactly one allocation and 32 allocated
bytes per turf: the 100,000-turf cases fell from 133,396 to 33,396 allocations and from 79,892,176
to 76,692,176 allocated bytes for all three topologies.

Direct packed-topology traversal for component stages produced three byte-identical 36-row candidate
runs with SHA-256 `C569CEDAC3DAC2C96C1E0A2371DF8DEA32E4189CC99F7775A54C9C4B22FAA22C`.
Every transcript hash matched the preceding candidate. At 100,000 turfs, equalize removed
107,222-107,556 allocations and 1,952,016-2,082,224 allocated bytes, while excited-groups removed
116,667 allocations and 7,961,600 allocated bytes for every topology. These reductions exceed the
zero-noise allocation controls and retain sorted packed-topology traversal order.

Stage-state recycling and server translation scratch were reviewed but not implemented. Retaining
stage vectors would pin large buffers while leaving the measured per-entry tree/set and transaction
allocations intact. In the legal IPC control, increasing a lifecycle batch from 1 to 1,024 records
raised p50 round-trip latency from 31.4 microseconds to 54.7 microseconds; this whole-path delta is
small beside the 739.5 microsecond p50 1,024-turf service stage and does not justify persistent
per-family translation buffers without more granular profile evidence.
