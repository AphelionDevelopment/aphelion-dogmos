# Handoff — performance and resource management audit

Date: `2026-08-30`
Branch: `master`
HEAD at audit time: `ea4e0cebacf186d98831012ca81873500e161fcf` ("Fix O(n) blowups in turf topology mutation during bulk registration")

## Status in one line

An audit was performed by reading code. **No measurements were taken and no files were changed.** The plan below is unstarted, and its phase ordering is by inspection, not evidence.

## Repository state you are inheriting

13 files were already modified and uncommitted when the audit began, and they were left untouched:

```
crates/dogmos-byond/src/lib.rs
crates/dogmos-core/src/frontier.rs
crates/dogmos-core/src/topology.rs
crates/dogmos-core/src/world.rs
crates/dogmos-core/tests/frontier_processing.rs
crates/dogmos-protocol/src/lib.rs
crates/dogmos-protocol/tests/compound_commands.rs
crates/dogmos-protocol/tests/cross_bitness.rs
crates/dogmos-protocol/tests/telemetry.rs
crates/dogmos-server/src/lib.rs
crates/dogmos-server/src/state.rs
dogmos-build-manifest.toml
tools/tests/test_dogmos_contract.py
```

That work is the incremental-frontier-sync feature (`add_frontier`/`remove_frontier`, `FrontierState::add`/`remove`, protocol `FrontierMutate`) plus several optimizations already landed in the working tree. **All line numbers below refer to the working tree, not to `HEAD`.** Re-verify anchors before editing; they will drift.

`HANDOFF.md` (this file) is the only file the audit session created. It is untracked and is not validated by `tools/check_agent_docs.py` (that gate only scans `AGENTS.md` and a fixed allowlist under `docs/agent/`).

## Constraints that bind the next session

From `AGENTS.md` — read it in full before touching anything:

- The audited crate is a **32-bit in-process BYOND DLL**. Rust allocations in `dogmos-byond` consume DreamDaemon address space. This is the memory that matters.
- Report `dogmosd` (64-bit) memory **separately**. Do not combine it with DreamDaemon, and do not optimize harmless 64-bit service RSS.
- Every behavioral change is **test-first**: prove the focused failure, make the smallest correction, rerun focused then wider gates.
- Accept performance changes **only** from repeated identical workloads with numerical/event equivalence.
- Preserve public DM proc paths and caller-legible errors. No panic may unwind across the FFI boundary.
- Protected files needing explicit approval naming exact files: root/workspace `Cargo.toml`, `Cargo.lock`, `.cargo/`, `rust-toolchain.toml`, `.github/workflows/`, dependency/transport choices, artifact/sync/release tooling, Docker, deploy scripts. Broad plan approval does **not** cover these.
- Generated bindings and release manifests are never hand-edited.
- Build BYOND-facing code for `i686-pc-windows-msvc` and `i686-unknown-linux-gnu`. Host-only Cargo success is not authoritative.

## Findings

All findings are from reading code. None are measured. Severity is a judgment call about allocation volume on hot paths, not an observation.

### P0 — DreamDaemon 32-bit heap

**1. `ServiceSession::request` copies every response into a fresh `Vec`.**
`crates/dogmos-byond/src/session.rs:39` — `.map(<[u8]>::to_vec)`. `BoundedDogmosClient::round_trip` (`client.rs:296`) deliberately returns a borrowed slice out of a preallocated `max_control_payload` buffer; this discards that with a malloc+copy on every call. Most callers immediately `try_into()` a fixed-size array and drop the `Vec` (e.g. `lib.rs:803`).

**2. `&format!(...)` field labels allocated on the success path, per field, per entry.**
The fix exists in exactly one place — `crates/dogmos-byond/src/lib.rs:774`, with a comment explaining the reasoning. Every other decoder still allocates: turf lifecycle (1276–1308), turf adjacency (1351–1373), turf heat (1410–1428), heat adjacency (1524–1543), mixture state (912–925), gas metadata (1002–1073), reaction metadata (1160–1233), frontier handles (1621–1631, 1700–1710), and `exact_words4`. A 4,000-turf boot batch at ~6 fields/entry is roughly 24,000 `String` alloc/free pairs in DreamDaemon's heap, purely to name an error that is not occurring.

**3. Batch encoders start from `Vec::new()` and grow by doubling.**
`crates/dogmos-byond/src/lib.rs` lines 605, 789, 934, 1315, 1378, 1440, 1548, 2071. Final size is exactly `4 + entries.len() * RECORD_LEN` and known up front. Two encoders already do this correctly (1086, 1240); the rest fragment the 32-bit heap with a realloc chain per batch.

### P1 — per-tick allocation in core simulation

**4. `remove_incident_edges` is the un-fixed twin of the bug fixed in `ea4e0ce`.**
`crates/dogmos-core/src/world.rs:4980` does `self.edges.retain(...)` — a full `BTreeMap` traversal — and is called *inside* the per-mutation loop at `world.rs:1047` and `world.rs:1054`. That is O(unregisters x total edges), the exact shape the batched `unregistering` set at `world.rs:1029` was introduced to eliminate for turfs. The turf half was hoisted; the mixture-edge half was not.

**5. `compute_stage_diffusion_node` heap-allocates a <=6-element `Vec` per turf per tick.**
`crates/dogmos-core/src/world.rs:2507`. Same shape as the `nth()` fixes at 2693 and 2967, but this one is `collect()`ed because it is iterated `MAX_GAS_SLOTS` times. It needs a buffer; it does not need a heap buffer. `[usize; MAX_TURF_NEIGHBORS]` plus a count is a stack array.

**6. Equalize and excited-groups rebuild a `BTreeMap<u32, Vec<u32>>` adjacency map per component, per tick.**
`crates/dogmos-core/src/world.rs:3739` and `world.rs:3873`. One `Vec` allocation per node plus a B-tree, reconstructing what `PackedTopology` already answers in O(<=6) via direct slot index. `process_ready_stage_component` calls these once per component.

**7. `stage_equalization_transfer` clones two full `MixtureRecord`s per flow edge.**
`crates/dogmos-core/src/world.rs:4176`–4180 — clone source, clone target, mutate, re-`insert` both into the `BTreeMap`. A component of N turfs produces ~N flows, so ~2N record clones (each carrying `[f32; MAX_GAS_SLOTS]`) plus 2N B-tree inserts, to work around not holding two disjoint `&mut` into a `BTreeMap`.

**8. `stage_turf_handles()` clones the component vector on every call.**
`crates/dogmos-core/src/world.rs:4935`. `process_ready_stage_component` already clones the queue into `stage_component_turfs` at `world.rs:3028`; each of `process_equalize` / `process_excited_groups` / `process_turf_heat` then clones it again on entry (3721, 3863, 4201).

**9. `process_ready_stage_component` does a three-pass clone-restore-restage.**
`crates/dogmos-core/src/world.rs:3033`–3060: clone every mixture in the component (`before`), run, clone them all again (`after`), write `before` back, stage `after`. Three full record copies per mixture per component per tick. The staging semantics are load-bearing — this is a rework, not a deletion.

### P1 — server translation layer (64-bit RSS; churn, not a memory target)

**10. Every `apply_*` allocates a throwaway wire-to-core `Vec`.**
`crates/dogmos-server/src/state.rs` lines 797, 825, 846, 889, 898, 920, 937, 962, plus `add_frontier`/`remove_frontier` at 369 and 384. The `dogmos-protocol` decoder already allocated a `Vec` per batch (`crates/dogmos-protocol/src/lib.rs` 1381, 1421, 1504, 1565, 1626, 1789, 1855, 2499), so each batched command allocates and frees two batch-sized vectors per request. `ServiceState` already has the right pattern in `pending_callback_scratch` / `pending_continuation_scratch`; it is simply not applied here. `apply_turf_adjacency` is worst at three (869, 889, 898).

### P2

**11. `FrontierState::remove` is O(committed) per incremental call.**
`crates/dogmos-core/src/frontier.rs:247` — `committed.retain()` scans the whole frontier to drop a handful of handles. `add()` was built specifically so the steady-state path avoids full-frontier-sized work; `remove()` still does it. It also allocates a `HashSet` per call for a typically single-digit set.

**12. `pending()` allocates a fresh `BTreeSet` over the whole staging vector.**
`crates/dogmos-core/src/frontier.rs:161`. `commit_validated`'s doc comment notes the double pass was removed, but the remaining single pass is still O(n log n) with an allocation on a full-map bootstrap. Duplicate detection could ride the existing `received_bits` / `committed_set` machinery.

**13. Double `BTreeSet` lookup in the equalize BFS.**
`crates/dogmos-core/src/world.rs:3921` — `visited.contains(&neighbor)` then `visited.insert(neighbor)`. One `insert` returning `bool` does both. Innermost loop.

## Resolution plan

Unstarted. Sequenced so each step is independently verifiable.

**Phase 1 — shim heap pressure (items 1, 2, 3).** Highest value per unit of risk, and the memory `AGENTS.md` names as the target.
1. Change `ServiceSession::request` to return `Result<&[u8]>` or write into a caller-supplied buffer; update the ~20 call sites, most of which simplify. Fixed-size responses land in a stack array.
2. Replace `&format!(...)` labels with the `.map_err(|e| eyre!("label {index}: {e}"))` shape already used at `lib.rs:777`. Mechanical, one decoder at a time. **Error text must stay caller-legible** — add an assertion test on one representative message per decoder so the wording is provably preserved.
3. `Vec::with_capacity(4 + entries.len() * RECORD_LEN)` in the eight batch encoders.

Evidence required: DreamDaemon private/committed bytes across an identical boot-storm workload, before vs after, reported separately from `dogmosd`.

**Phase 2 — the O(n x m) survivor (item 4).** Hoist `remove_incident_edges` out of the mutation loop exactly as the turf half was hoisted: collect unregistered slots once, then one `edges.retain` for the whole batch. Self-contained. Should be measurable on the same bulk-registration workload as `ea4e0ce`.

**Phase 3 — per-tick core allocations (items 5, 6, 8, 13).**
- 5: stack array in `compute_stage_diffusion_node`.
- 8: return `&[TurfHandle]` from `stage_turf_handles`, or split into a borrowed component path and an owning fallback.
- 6: drop the adjacency `BTreeMap`; query `topology.gas_neighbors` directly against a component-membership set. **Caveat:** the current code sorts neighbor lists, so traversal order is deterministic. `PackedTopology` keeps neighbors sorted via `insert_sorted`, so order should be preserved — but confirm with the transcript-equivalence test rather than by argument.
- 13: fold to a single `insert`.

**Phase 4 — server scratch buffers (item 10).** Add reusable `Vec` fields on `ServiceState` following the `pending_callback_scratch` precedent. Do this *after* Phase 3 so core-side churn measurements are not confounded. The deeper fix — having `dogmos-protocol` decode straight into caller-provided storage, skipping the intermediate representation — is a larger protocol-layer change and should be its own decision, not folded in here.

**Phase 5 — frontier incremental path (items 11, 12).** `remove()` should use `committed_set` plus a swap-remove index map, or accept an O(removed) tombstone with periodic compaction. **This changes frontier ordering, which stage cursors iterate.** It must be gated by the transcript-equivalence and frontier-processing tests, and order stability may forbid swap-remove outright. Investigate before committing to an approach.

**Phase 6 — component staging rework (items 7, 9).** Largest change, real semantic risk. `process_ready_stage_component`'s save/run/capture/restore is a transactional guarantee, and `stage_equalization_transfer`'s clone-mutate-reinsert is how it avoids aliasing. Indexing into a `Vec<MixtureRecord>` with disjoint `split_at_mut` access is the right shape but touches equalize correctness directly. Do it last, or scope it to reusing scratch buffers (the cheap 80%) and leave the aliasing rework for a dedicated session.

## Open decision — needs the user

The audit ended on an unanswered question:

> Start on Phase 1, or get measurements in place first so the phase ordering is evidence-backed?

Nothing was decided. `crates/dogmos-perf` has an `ipc_round_trip` bench and there is a perf contract in `tools/tests/test_perf_contract.py`, but `AGENTS.md` requires repeated identical workloads with numerical/event equivalence plus the paired Meridian-Rift PowerShell DreamMaker/DreamDaemon gates. Phase 1 and Phase 2 are high-confidence from inspection; Phase 3's relative value is not.

## Verification, when you do start

Per `docs/agent/verification.md` and `AGENTS.md`, use the pinned toolchain and `--locked`. Run formatting, strict Clippy, tests, supported feature combinations, i686 shim builds, generated-binding drift, and paired artifact verification as applicable. Verify the paired Meridian-Rift integration through its PowerShell gates. Report Rust, DM compile, focused tests, boot, full suite, and performance evidence **separately**.

## What was not examined

- `crates/dogmos-core/src/reactions.rs`, `metadata.rs`, and the `numerics/` kernels were not audited for allocation behavior.
- `crates/dogmos-perf` and the benchmark harness were only checked for existence, not reviewed.
- The `examples/cross_bitness_probe.rs` path was not read.
- No profiling, no benchmark runs, no build was performed in the audit session.
