# Meridian-Rift Dogmos playtest assessment

This assessment covers the 2026-08-27 Minimal Runtime Station playtest at Meridian-Rift revision
`1623a76079a6617598498eaf7f5778f8564ed314`. It is an in-process i686 Dogmos baseline, not evidence
for the diagnostic 32-bit shim and 64-bit `dogmosd` service prototype.

## Build and evidence identity

| Item | Identity |
| --- | --- |
| Meridian-Rift revision | `1623a76079a6617598498eaf7f5778f8564ed314` |
| BYOND | 516.1685, Windows x86 DreamDaemon |
| Playtest DMB | `73014E3CB85259DF0636FCD205C08F5D6E72875857E43DD42EB375447F669900` |
| In-process `dogmos.dll` | `C70C334444C3A816C4696CA0A6D5F7BDE783BAC565CF8FBC4CD9217A16CDC552` |
| Performance CSV | `A44DF465E6E1FAE6B76A4105E199035663042E07A319812C2FA5168763916A7E` |
| Kennel log | `1C2CA75261D43AF455BE7C3100E47EC3C7FFD1481F12CDB5CFBC05926EBD23A3` |
| Runtime log | `FAA8A73ED215F02D092C7C81280DABB1F9398789E3BC89D851F0DB0A33EC4E03` |

The three BYOND procedure profiles were written before game start. They measure initialization, not
the later fire workloads. The ten-second performance CSV and Dogmos Kennel log cover the live
workload. No exact-PID memory series was captured, so this run cannot support a DreamDaemon memory
reduction claim. No `dogmosd` process participated and no service memory should be inferred.

## Workload

- One player on Minimal Runtime Station.
- Nine fusion-test canisters created over approximately nine seconds.
- A clustered tritium-fire and explosion workload in one area.
- A later plasma-fire workload in the Thunderdome.
- Reaction profiling enabled shortly before the tritium workload.

This is a useful adversarial smoke and attribution run. It is not a repeated control/candidate
experiment and has no stable seed or scripted phase markers.

## Observations

The 33 performance rows span 310.5 game seconds. Values are the subsystem's exported moving costs,
not raw per-call samples.

| Metric | Average | p95 | Maximum |
| --- | ---: | ---: | ---: |
| Active-turf cost, ms | 6.57 | 19.00 | 39.15 |
| Excited-group cost, ms | 1.38 | 3.32 | 10.31 |
| High-pressure cost, ms | 0.44 | 1.17 | 4.38 |
| Hotspot cost, ms | 0.95 | 4.18 | 13.67 |
| Turf-heat cost, ms | 4.11 | 9.40 | 12.24 |
| Pipenet cost, ms | 6.35 | 11.50 | 20.81 |
| Map tick usage | 0.96 | 1.55 | 1.98 |

The largest active-turf and hotspot values align with the tritium explosion interval. The final
sample, during the later fire workload, reached 39.15 ms active-turf, 10.31 ms excited-group, and
13.63 ms pipenet moving cost. These stages can span subsystem resumptions, so they demonstrate
pressure rather than one uninterrupted frame duration.

Heat telemetry stayed internally consistent: every reported edge attempt was applied, lock
contention was zero, the heat graph ranged from 53,132 to 57,582 nodes, and cumulative registration
operations ranged from 53,988 to 59,390. This does not prove numerical equivalence, but it provides
no evidence of a lost-edge or contention failure during the run.

Reaction profiling recorded 540 calls:

| Reaction | Count | Total measured time, ms | Mean, ms | p95, ms | p99, ms | Maximum, ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Tritium combustion | 495 | 507.48 | 1.025 | 1.68 | 2.89 | 4.69 |
| Plasma combustion | 45 | 51.11 | 1.136 | 2.09 | 2.38 | 2.39 |

The Kennel retained bounded UI history while writing all qualifying samples to disk. The run also
recorded a peak fire group of 147 turfs and 15 breach events totaling 1,309.7 moles lost. Profiling
was configured at 0.5 ms, which selected most reactions in this stress. The timings include the
native reaction call but exclude the later DM event-list and log-write work, so they are not the full
instrumentation cost.

The runtime log contains 49 runtime entries: 48 paired burning-component errors on decorative
structures during startup and one missing-observer-landmark error. All occurred before the tested
Dogmos fire workload, and no runtime entry names Dogmos or an atmosphere processing proc. They must
remain a separate baseline issue rather than be attributed to Dogmos.

## Resulting changes and next gates

The playtest exposed an attribution gap in the CSV: `SSair.cost_equalize` and
`SSair.cost_post_process` already existed but were not exported. Meridian-Rift now adds
`air_equalize_cost` and `air_post_process_cost` as scalar CSV columns. This does not retain a new DM
list or datum and therefore does not create continuing DreamDaemon memory use.

The next controlled run should:

1. Use a scripted phase marker for idle, tritium fire, plasma fire, breach, recovery, and shutdown.
2. Capture at least three in-process controls and three service candidates with identical map, seed,
   artifact hashes, and action counts.
3. Record exact-PID DreamDaemon memory and a separate `dogmosd` series.
4. Capture the new equalize and post-process columns, callback depth/high-water/failures, and a valid
   phase-window Tracy trace.
5. Compare event order, reaction results, gas totals, temperatures, and pressure outcomes before any
   authoritative migration decision.
