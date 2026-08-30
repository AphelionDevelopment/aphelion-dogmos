# Resource management qualification results

## Outcome

The local Windows qualification gates pass for the protocol, shim, core, service, Python contracts,
and feature matrix. The accepted changes remove deterministic allocation and scan costs without
changing measured core transcripts. Tasks 9 and 10 were rejected by their evidence gates. The
remaining component transaction cost justifies a separate indexed scratch-arena design.

The architecture-level production acceptance is not complete. This repository run did not execute
three matched DreamDaemon controls/candidates or collect SSair headroom, so it does not establish the
70% Dogmos-attributable DreamDaemon private-byte reduction target.

## Qualified revision and artifacts

- Code revision measured: `277098aabc6d72ae1342a29129cc045de1e9ebf5`
- Rust: `1.98.0`
- Feature fingerprint: `7ee0a12425df8b2d85eb47f08d19655a3a217eadec26e9d290da78f6de17bcff`
- Core controls: `tmp/dogmos-perf/core-control-{1,2,3}.csv` (ignored)
- Final core candidates: `tmp/dogmos-perf/core-final-candidate-{1,2,3}.csv` (ignored)
- IPC candidates/status/memory: `tmp/dogmos-perf/ipc/` (ignored)

All three final core CSVs contain 36 rows and have SHA-256
`C569CEDAC3DAC2C96C1E0A2371DF8DEA32E4189CC99F7775A54C9C4B22FAA22C`.

## Core allocation results

Every candidate transcript hash matches its corresponding control across corridor, grid, and
three-layer multiz fixtures at 1,000, 10,000, and 100,000 turfs.

At 100,000 turfs:

| Stage | Control allocations | Candidate allocations | Allocations removed | Candidate allocated bytes |
| --- | ---: | ---: | ---: | ---: |
| Process turfs | 133,396 | 33,396 | 100,000 | 76,692,176 |
| Equalize | 264,873-269,491 | 157,651-161,935 | 107,222-107,556 | 195,054,608-196,683,956 |
| Excited groups | 330,967-359,948 | 214,300-243,281 | 116,667 | 149,154,600-150,413,920 |

Lifecycle invalidation also changed from one full mixture-edge retain per invalidated slot to one
retain per batch. The regression test captured two passes for the old two-invalidation case and one
pass for the candidate while preserving unrelated edges.

## IPC latency and process memory

Three fresh candidate processes ran 20,000 operations per transport shape and 500 complete logical
stages after warmup. Every logical 1,024-turf stage reported 2,048 work items.

| Case | Candidate p50 range (us) | Worst p95 (us) | Worst p99 (us) | Recorded control p95 / p99 (us) |
| --- | ---: | ---: | ---: | ---: |
| Scalar getter | 31.8-33.0 | 76.3 | 109.4 | 84.2 / 124.3 |
| 1,024-operation batch | 33.6-33.8 | 56.8 | 102.8 | 75.8 / 120.0 |
| 32-gas mixture snapshot | 32.4-33.1 | 80.8 | 115.5 | 79.7 / 120.4 |
| 1,024-turf service stage | 701.7-716.3 | 879.7 | 1,037.0 | 949.3 / 1,122.5 |

The snapshot p95 increase is 1.4%, within the 5% acceptance budget; every other listed p95 and all
listed p99 values improved. Maxima remain scheduler-sensitive and ranged up to 2.16 ms for the
service stage.

The three one-shot process samples were:

| Role | Private bytes | Virtual bytes | Working set bytes |
| --- | ---: | ---: | ---: |
| i686 shim | 933,888-1,007,616 | 18,247,680-19,558,400 | 4,808,704-4,833,280 |
| x64 service | 1,953,792-1,974,272 | 4,354,215,936-4,354,248,704 | 4,034,560-4,055,040 |

The shim virtual mapping remains below the 32 MiB fixed-address-space budget. These standalone
samples do not substitute for DreamDaemon private-byte evidence.

## Verification

The following commands exited 0:

```powershell
cargo +1.98.0 fmt --all -- --check
cargo +1.98.0 clippy --workspace --locked --target i686-pc-windows-msvc --all-targets -- -D warnings
cargo +1.98.0 test --workspace --locked --target i686-pc-windows-msvc
cargo +1.98.0 test --locked --target x86_64-pc-windows-msvc -p dogmos-core -p dogmos-protocol -p dogmos-server -p dogmos-process-metrics -p dogmos-identity
python -m unittest discover -s tools/tests -p 'test_*.py'
tools/check_feature_matrix.ps1 -Target i686-pc-windows-msvc
tools/benchmark_ipc.ps1 -Iterations 20000 -Repetitions 3
```

The Python command ran 42 tests. The i686 workspace and x64 targeted suites reported no failures.
The feature matrix checked no features, every individual supported feature, defaults, and defaults
plus Tracy.

The installed `i686-unknown-linux-gnu` and `x86_64-unknown-linux-gnu` Rust targets were not built:
this Windows host has neither `i686-linux-gnu-gcc` nor `x86_64-linux-gnu-gcc`. Those CI/Linux linker
gates are not run, not passed.

## Deferred external gates

- Three matched DreamDaemon control/candidate workloads with exact map, seed, BYOND version, and
  workload hash.
- Separate DreamDaemon and service private-byte series, not a combined total.
- DreamDaemon p50/p95/p99/max and SSair headroom.
- The 70% Dogmos-attributable DreamDaemon peak-private-byte architecture target.

## Transaction staging decision

The candidate remains justified because equalize still allocates about 195-197 MB per 100,000-turf
stage after traversal cleanup. The implementation is deliberately deferred to
`2026-08-30-transaction-scratch-arena-design.md`; it requires its own tests, allocation acceptance,
and production qualification rather than being folded into this completed change set.
