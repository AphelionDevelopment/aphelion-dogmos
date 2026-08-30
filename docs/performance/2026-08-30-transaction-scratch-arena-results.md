# Indexed transaction scratch arena qualification results

## Outcome

The indexed transaction arena passes its core allocation and deterministic-transcript acceptance
gates for equalize and excited-groups. It also closes a correctness hole: the component transaction
now rejects a mutable mixture shared by disconnected turf components instead of allowing the later
component to overwrite the earlier staged candidate.

This qualification does not replace the external DreamDaemon/DD acceptance matrix. No claim is made
here about DreamDaemon private bytes, SSair headroom, or production service latency.

## Qualified revision and artifacts

- Code revision measured: `e9f57269dd8620b0534ca59d30515fa992b96383`
- Rust: `1.98.0`
- Control: `tmp/dogmos-perf/core-final-candidate-{1,2,3}.csv` (ignored)
- Arena candidate: `tmp/dogmos-perf/core-arena-candidate-{1,2,3}.csv` (ignored)

All three arena CSVs contain 36 rows and have SHA-256
`019355931FA73B76C5329096859D28B1946010F5C872A6B48E63970D56CD9539`.

## Core allocation results

Every 100,000-turf transcript hash exactly matches its corresponding packed-topology control:

| Stage | Topology | Control allocations | Arena allocations | Control bytes | Arena bytes | Byte reduction |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Equalize | Corridor | 161,935 | 119,462 | 196,675,040 | 60,595,728 | 69.19% |
| Equalize | Grid | 157,651 | 115,828 | 195,054,608 | 60,092,408 | 69.19% |
| Equalize | Three-layer multiz | 161,527 | 119,057 | 196,683,956 | 60,548,332 | 69.22% |
| Excited groups | Corridor | 243,281 | 259,144 | 150,413,920 | 42,783,920 | 71.56% |
| Excited groups | Grid | 242,787 | 258,650 | 150,369,152 | 42,739,152 | 71.58% |
| Excited groups | Three-layer multiz | 214,300 | 225,209 | 149,154,600 | 41,732,584 | 72.02% |

Equalize reduces both allocation count and bytes. Excited-groups increases allocation count by
10,909-15,863 while reducing allocated bytes by more than 106 MiB. The accepted gate is allocated
bytes because the rewrite replaces large per-entry tree-owned record clones with one bounded dense
candidate array; allocation counts remain useful for later profiling but do not override the passed
byte gate.

The post-stage reusable-vector lower bound remains identical to the control for all six rows:
4,366,912-4,397,756 bytes for equalize and zero for excited-groups. The transaction vectors are
included while a component stage is active and are dropped at commit, so the arena adds no retained
post-stage core workset.

## Atomicity and ownership evidence

Focused tests cover first/repeated touch, generation conflict, fallible capacity, rollback index
clearing, checked disjoint candidate access, deterministic commit ordering, event-overflow rollback,
cross-component duplicate mutable-mixture rejection, and commit-time revision revalidation. Existing
equalize and excited-groups transcript, decompression, cancellation, and hard-limit tests remain
green. Authoritative mixture records and public events are not mutated until the complete component
transaction passes validation.

## Local verification

The following commands exited 0 against the committed implementation:

```powershell
cargo +1.98.0 fmt --all -- --check
cargo +1.98.0 clippy --workspace --locked --target i686-pc-windows-msvc --all-targets -- -D warnings
cargo +1.98.0 test --workspace --locked --target i686-pc-windows-msvc
cargo +1.98.0 test --locked --target x86_64-pc-windows-msvc -p dogmos-core -p dogmos-protocol -p dogmos-server -p dogmos-process-metrics -p dogmos-identity
tools/check_feature_matrix.ps1 -Target i686-pc-windows-msvc
python -m unittest discover -s tools/tests -p 'test_*.py'
tools/benchmark_ipc.ps1 -Iterations 20000 -Repetitions 3
```

The Python run completed 42 tests. The feature matrix checked no features, each supported individual
feature, defaults, and defaults plus Tracy. All three legal IPC processes completed; every
1,024-mixture service-stage sample reported 2,048 work items. The service-stage p50 range was
697.1-781.7 microseconds, with worst p95 929.4 microseconds and worst p99 1,099.8 microseconds. This
unmatched local series is a transport/functionality gate, not production latency acceptance.

## Remaining external gates

- Three matched DreamDaemon control/candidate workloads with exact map, seed, BYOND version, and
  workload hash.
- Separate DreamDaemon and service private-byte series.
- DreamDaemon p50/p95/p99/max and SSair headroom.
- The paired external game/DD qualification matrix from the repository verification guide.
