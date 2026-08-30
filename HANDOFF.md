# Handoff — DM-side Dogmos improvements

Date: `2026-08-30`

This handoff supersedes the original read-only performance audit that previously occupied this
file. That audit is preserved in Git history, but its implementation priorities are stale.

## Assignment

Continue in the existing Meridian-Rift `dogmos` checkout and specialize in the DreamMaker side of
Dogmos: SSair scheduling, lifecycle and topology synchronization, the DM compatibility API, typed
gameplay-event dispatch, recovery, diagnostics, Kennel behavior, and player/admin-visible effects.

Read [DM_SIDE_AGENT_INSTRUCTIONS.md](DM_SIDE_AGENT_INSTRUCTIONS.md) before taking any action in the
game repository. It is the operating sheet for this assignment.

## Repositories and inspected state

| Repository | Branch | Inspected revision | State at handoff inspection |
| --- | --- | --- | --- |
| `aphelion-dogmos` | `master` | `c471ff482d4927378457a8075c2f9a954b6b5354` | Clean before these handoff documents |
| `Meridian-Rift` | `dogmos` | `dfd461a55f500ef20bab641b4908456ebd0ab7cf` | Clean |

Do not create another checkout or generated worktree. Do not mix standalone Rust qualification with
game-integration claims. Preserve unrelated changes if either checkout is no longer clean.

## What the standalone session completed

The standalone audit and implementation plans were carried through locally on `aphelion-dogmos`:

- direct packed-topology traversal replaced avoidable component adjacency reconstruction;
- process-turfs removed its per-turf neighbor-vector allocation;
- lifecycle invalidation performs one mixture-edge retain per batch;
- equalize and excited-groups now use a bounded indexed transaction arena;
- the arena keeps authoritative mixtures untouched until validated commit, revalidates revisions,
  preserves ordered events, and rejects mutable mixtures shared across disconnected components;
- pinned i686 and x64 Rust gates, the feature matrix, Python contracts, and legal IPC benchmark
  completed successfully.

The transaction result is in
[`docs/performance/2026-08-30-transaction-scratch-arena-results.md`](docs/performance/2026-08-30-transaction-scratch-arena-results.md).
At 100,000 turfs, allocated bytes fell 69.19-69.22% for equalize and 71.56-72.02% for
excited-groups with exact transcript hashes. These are core-process results, not DreamDaemon
production acceptance.

The remaining architecture gate is the matched game workload: three clean DreamDaemon controls and
candidates, separate DreamDaemon and `dogmosd` memory series, exact workload identity,
numerical/event equivalence, latency percentiles, and SSair headroom.

## Confirmed Meridian-Rift integration surface

Meridian-MCP parsed `tgstation.dme` successfully at the inspected revision. The main integration
surfaces are:

- `modular_aphelion/modules/dogmos/code/dogmos.dm` — subsystem initialization and shutdown;
- `modular_aphelion/modules/dogmos/code/service_backend.dm` — lifecycle batches, service-backed
  mixture compatibility, frontier/stage dispatch, callback validation, and gameplay dispatch;
- `modular_aphelion/modules/dogmos/code/service_backend_test.dm` — service lifecycle, callback
  identity, topology barrier, and related focused tests;
- `code/controllers/subsystem/air.dm` — SSair scheduling/recovery and Dogmos telemetry state;
- `code/controllers/subsystem/dogmos_kennel.dm` and `dogmos_kennel_events.dm` — admin controls,
  bounded histories, overlays, targets, and diagnostics;
- `code/modules/atmospherics/gasmixtures/**` and
  `code/modules/atmospherics/environmental/**` — the narrow fork-owned compatibility exception;
- `code/modules/unit_tests/dogmos_*.dm` — DM-visible behavior and regression coverage;
- `tgui/packages/tgui/interfaces/DogmosKennel*` — the admin-facing status and control UI;
- `code/__DEFINES/dogmos_bindings.dm` and `dogmos_contract.dm` — generated, never hand-edited.

Semantic inspection confirmed these current behaviors:

- `flush_turf_registration_batch()` enforces lifecycle-before-topology order and defers while a
  committed frontier is active.
- `process_turf_equalize_auxtools()` and `process_excited_groups_auxtools()` route through the
  service stage API.
- `dispatch_general_callback()` validates sequence and turf generations before applying pressure,
  decompression, firelock, reaction, or destruction effects on the DM main thread.
- direct reaction callbacks remain transaction-scoped; turf-stage reaction continuations return to
  the bounded general queue.
- recovery tests require the same service PID and world generation rather than a second world.
- the Kennel keeps bounded histories and weak-reference-backed jump targets, and exposes separate
  DreamDaemon and service process metrics.

The cached SpacemanDMM snapshot contained a large inherited diagnostic baseline. Parser success is
not a clean DreamChecker result and is not a DreamMaker compile. Filter diagnostics to changed files
and compare before/after instead of claiming the repository has zero diagnostics.

## Recommended first objective

Perform a DM-side audit before broad changes, then write a separate implementation plan. Focus on
the complete cost and correctness path from SSair frontier creation through service-stage chunks,
bounded callback drains, gameplay effects, cache invalidation, recovery, and Kennel reporting.

Prioritize evidence in this order:

1. Establish the paired DreamDaemon control workload and current local gates.
2. Audit SSair work budgeting, callback-drain budgeting, and topology deferral for starvation or
   work that escapes the charged budget.
3. Audit lifecycle/topology batching and recovery for ordering, stale state, duplicated work, and
   full-world scans.
4. Audit DM mixture snapshot/cache call sites for read-after-write barriers, invalidation gaps, and
   avoidable BYOND list construction.
5. Audit typed callback consumers for stale-target policy, exact order, bounded work, durable
   diagnostics, and visible gameplay equivalence.
6. Profile Kennel data production and overlays for producer-side bounding and DreamDaemon-only
   memory savings. Do not optimize `dogmosd` RSS for appearance.

Treat these as investigation lanes, not pre-confirmed bugs. Measure or prove each finding before
implementing it.

## Completion boundary

A DM-side change is not complete with parser output or a focused unit test alone. The expected
ladder is semantic reparse, changed-file diagnostics, focused DM tests, contract drift verification,
fresh DreamMaker compile, two-process boot, wider DM suite, and repeated matched performance runs
when making a performance claim. Report every unrun gate explicitly.

No push is authorized by this handoff. Commit only when the user explicitly authorizes commits for
the Meridian-Rift task.
