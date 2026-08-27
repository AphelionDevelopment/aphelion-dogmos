# Dogmos full-map operation pressure

Status: one valid round-start capture and one valid settled-idle delta; repetitions and stress
workloads still required.

## Workload identity and limitations

The capture used Meridian-Rift revision `1623a76079a6617598498eaf7f5778f8564ed314`, BYOND
516.1685, MetaStation, and a disposable `PERFORMANCE_TESTS` build. The DMB SHA-256 was
`DA4E260F4392A6568E78374B78953800A619083AB7195830FE6AEF6026C34B02`; the corrected in-process
Dogmos DLL SHA-256 was `C70C334444C3A816C4696CA0A6D5F7BDE783BAC565CF8FBC4CD9217A16CDC552`.

The detailed window began at round start and ran for 60 game seconds. Late asynchronous condo
preview initialization and its pre-existing decorative-burning runtimes overlapped the window, so
this is a heavy round-start/initialization workload rather than an idle control. Aggregate counters
include all earlier boot and registration work. Do not compare its wall timing to another map seed
or call it an idle baseline.

The first attempt also exposed a tgstation performance-harness defect: the nested `_addtimer`
callback in `queue_performance_tests()` runtime-failed while inspecting `world.gc_destroyed`. The
disposable run used a direct `OnRoundstart` callback. No profiling-hook change was added to shipped
game code.

## Operation inventory

The valid JSON artifact contains 1,846,192 aggregate calls and 882,530 calls made while detailed
capture was enabled. The rolling transcript returned the most recent 512 calls and reported
882,018 older detailed calls as dropped from the returned window. There were no recorded FFI
errors.

| Class | Aggregate calls | Share |
| --- | ---: | ---: |
| Scalar read | 914,674 | 49.54% |
| Other | 464,608 | 25.17% |
| Graph update | 346,996 | 18.80% |
| Scalar write | 117,309 | 6.35% |
| Mixture transaction | 2,329 | 0.13% |
| Simulation stage | 276 | 0.01% |

The largest individual bindings were:

| Binding | Class | Calls |
| --- | --- | ---: |
| `/turf/proc/update_air_ref` | other | 350,483 |
| `/turf/proc/__update_auxtools_turf_adjacency_info` | graph update | 262,147 |
| `/datum/gas_mixture/proc/return_temperature` | scalar read | 193,419 |
| `/datum/gas_mixture/proc/return_volume` | scalar read | 161,120 |
| `/datum/gas_mixture/proc/__get_moles` | scalar read | 141,348 |
| `/datum/gas_mixture/proc/__get_gases` | scalar read | 141,005 |
| `/datum/gas_mixture/proc/compare` | scalar read | 109,893 |
| `/datum/gas_mixture/proc/copy_from` | other | 108,203 |
| `/datum/gas_mixture/proc/return_pressure` | scalar read | 95,109 |
| `/datum/gas_mixture/proc/__gasmixture_register` | graph update | 77,521 |

The final 512-call transcript was dominated by repeated scalar reads. Its most common adjacent pair
was pressure-to-pressure (164), followed by temperature-to-temperature (59),
pressure-to-temperature (41), total-moles-to-pressure (19), and temperature-to-pressure (19).
This is direct evidence for snapshot/vector reads and typed multi-property queries; it does not
support one synchronous IPC request per getter.

## IPC feasibility calculation

Applying the measured live DreamDaemon mean of 43.6618-47.7127 microseconds to all 882,530 detailed
calls predicts 38.5-42.1 seconds of serialized IPC time over this window. Applying it only to the
914,674 aggregate scalar reads predicts 39.9-43.6 seconds. These are intentionally simple lower
bounds: they omit service compute, p95/p99 tails, callbacks, and write barriers.

The selected API therefore keeps the named pipe but rejects fine-grained remoting. Registration and
adjacency setup need bulk frames; common scalar read runs need mixture snapshots or typed
multi-property responses; existing FDM, equalization, heat, and callback-drain stages remain coarse
barrier commands.

## Memory and queue findings

The snapshot reported 70,214 live mixture slots, a 70,216 high-water mark, 67,515 gas-graph nodes,
68,805 heat-graph nodes, and an audited i686 allocation floor of 132,405,000 bytes for the modeled
mixture/graph structures. Only 47 mixtures spilled past inline mole storage.

The current in-process callback channel is still unbounded. It enqueued 67,875 items and reached a queue-depth
high water of 65,012 before draining to four at snapshot time. `callback_owned_bytes` is a current
value, not a byte high-water, and counts only boxed trait-object handles rather than captured heap
payload, so it understates the peak. The cross-process prototype now bounds a 65,536-event service
queue, rejects overflow atomically, and moved its measured 2.6 MB backlog into `dogmosd` while
DreamDaemon private bytes increased only 147,456-151,552 bytes. That queue currently carries
diagnostic events only; production migration still requires typed equivalents for every gameplay
callback and must not drop critical work.

## Serializer regression found during capture

The first release artifact was invalid JSON because `debug_assert_eq!(json.pop(), Some('}'))`
performed a required mutation only in debug builds. A release-specific regression test now proves
the diagnostic fields remain inside the root object, and the accepted rerun parsed successfully.
Raw artifacts remain ignored under `tmp/dogmos-perf/`.

## Settled-idle delta

A second performance build used a 60-game-second warmup after round start, wrote a valid pre-window
snapshot, enabled detailed capture for another 60 game seconds, and wrote a valid post-window
snapshot. `Initializations complete` occurred during the warmup, before detailed capture began. The
DMB SHA-256 for this harness was
`0EAE45889254D8C7660EE26F981888C1ACF20753AC80F6B32FD6410C89584E1E` and the Dogmos DLL identity
remained `C70C334444C3A816C4696CA0A6D5F7BDE783BAC565CF8FBC4CD9217A16CDC552`.

Subtracting the pre-window aggregate counters from the post-window counters yielded 635,421 calls;
the detailed sequence counter recorded 635,420 because the post-window snapshot call increments its
own aggregate counter. Every one of the four coarse SSair stages ran exactly 116 times.

| Class | Idle-window calls | Share |
| --- | ---: | ---: |
| Scalar read | 544,456 | 85.68% |
| Scalar write | 50,138 | 7.89% |
| Other | 24,560 | 3.87% |
| Graph update | 13,864 | 2.18% |
| Mixture transaction | 1,939 | 0.31% |
| Simulation stage | 464 | 0.07% |

The busiest idle bindings were `return_temperature` (187,869), `return_pressure` (128,348),
`__get_moles` (81,084), `return_volume` (57,258), `__get_gases` (28,431), `is_immutable`
(27,577), and `total_moles` (24,372). Registration and unregister each occurred 6,911 times,
demonstrating that temporary-mixture lifecycle traffic also needs a typed bulk path. The rolling
transcript again showed repeated pressure, temperature, total-moles, comparison, and gas-vector read
sequences.

This is approximately 5,478 calls and 4,694 scalar reads per observed SSair cycle. Applying the
43.6618-47.7127 microsecond live mean predicts 239-261 ms of raw IPC per cycle, or 27.7-30.3 seconds
over the window, before service compute or tail latency. By contrast, retaining four coarse stage
round trips costs about 0.34 ms per cycle at the measured worst p95. The transport passes; the
fine-grained API does not.

The idle window enqueued 3,449 callbacks while queue depth began and ended at three with no enqueue
failures. The observed 63,611 queue-depth high water came from earlier process activity, because the
current telemetry does not expose a resettable per-window high-water or captured-payload bytes.

## Resulting command boundary

The measured workload now has concrete prototype frames: fixed mixture snapshots, counted
register/unregister mutations, counted adjacency mutations, and the four typed simulation stages.
Initial release cross-bitness measurements kept 64-operation lifecycle and adjacency batches below
92 microseconds at worst p95 across three transport-only runs. The follow-up service prototype now
owns generational mixture slots and adjacency, and a real 1,024-mixture x 32-gas diffusion stage
measured 182.5 microseconds median and 254.9 microseconds p95 in one exploratory standalone run.
Three real DreamDaemon runs put the 64-mixture stage at 47.38-55.76 microseconds mean end to end.
The next acceptance gate must add complete atmosphere behavior and real typed gameplay callback
events, then repeat the idle capture and add stress scenarios. DreamMaker must retain only handles,
compatibility-facing values, and bounded callback/event data; authoritative mixture and graph
storage belongs in `dogmosd`.
