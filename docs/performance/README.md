# Dogmos performance evidence

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
