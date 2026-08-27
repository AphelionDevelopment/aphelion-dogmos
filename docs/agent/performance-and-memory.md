# Performance and memory

The footprint target is DreamDaemon. Record its private committed bytes, virtual size, working set/peak, committed/reserved/free regions, largest contiguous free region, and allocation failures. Record `dogmosd` RSS/private bytes and CPU separately. Never add the processes together when deciding whether BYOND memory pressure improved.

The 64-bit service heap may scale with the modeled world. Its memory is constrained only by leak prevention, bounded queues, lifecycle cleanup, and measured CPU/cache benefits. Do not spend complexity shrinking stable service RSS while DreamDaemon latency or maintainability worsens.

The shim plus mapped IPC views must be fixed-size and independent of turf, mixture, reaction, or callback-history cardinality. The qualification target is at least 70% lower Dogmos-attributable DreamDaemon peak private bytes and no more than 32 MiB of shim/mapped address space on the agreed stress workload.

Use identical maps, seeds, features, BYOND versions, durations, and scenarios. Run at least three clean controls and three candidates. Report noise, p50/p95/p99/max latency, SSair budget overruns, IPC operation counts, batch sizes, queue depth/age, and separate process memory. A candidate passes only with numerical/event equivalence and without sustained tick-budget regression.

Optimize service CPU from profiles. Current candidates include repeated reaction lookups, per-cycle collection allocation, quadratic front draining, linear free-slot search, full-bound scratch clearing, edge deduplication, and blanket Rayon splitting. Establish a focused benchmark or allocation count first. Wall-time benchmarks on shared CI are evidence, not stable merge gates.

Normal telemetry uses counters and high-water marks. Expensive histograms, allocator scans, and address-space walks run only during explicit diagnostics so measurement does not become the hot-path allocation problem.
