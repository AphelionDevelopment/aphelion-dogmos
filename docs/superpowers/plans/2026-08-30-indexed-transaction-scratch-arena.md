# Indexed transaction scratch arena implementation plan

> Status: Approved for autonomous execution on 2026-08-30. The user has standing authorization
> for in-scope edits, protected-file edits when necessary, and commits on the current `master`
> checkout. Do not push.

**Goal:** Replace per-component ordered-map record staging in equalize and excited-groups with a
bounded, fallibly allocated, slot-indexed transaction while preserving exact results, callback
order, and all-or-nothing publication.

**Architecture:** Add an internal generic `IndexedTransaction<T>` in `dogmos-core`. Each touched
mixture stores its handle, initial revision, and one candidate value; authoritative records remain
unchanged until a fully validated commit, so rollback only truncates transaction-local entries and
events. A slot bitset plus slot-to-entry index provides constant-time lookup, and a checked
`split_at_mut` accessor supplies disjoint candidate pairs without unsafe code.

**Constraints:** Preserve public Rust, C ABI, IPC, and DM contracts. Preserve deterministic turf
and event order, immutable mixtures, the equalize hard limit, cancellation atomicity, event-capacity
atomicity, generation/revision validation, duplicate mutable-mixture rejection, and topology
conflict handling. Use the repository-pinned Rust toolchain and `--locked`. Do not modify protected
files unless implementation evidence makes it necessary.

---

## Task 1: Add the indexed transaction primitive

**Files:**

- Create: `crates/dogmos-core/src/transaction.rs`
- Modify: `crates/dogmos-core/src/lib.rs`
- Test: `crates/dogmos-core/src/transaction.rs`

1. Add failing unit tests for first/repeated touch, generation conflict, rollback index clearing,
   checked disjoint pair access, deterministic handle sorting, and overflow/fallible capacity.
2. Implement `IndexedTransaction<T>` with `try_new`, `checkpoint`, `rollback_to`, `contains`,
   `touch`, `candidate`, `candidate_mut`, `candidate_pair_mut`, `entries`, `sort_by_handle`, and a
   vector-capacity lower-bound metric.
3. Use checked arithmetic and `try_reserve_exact`; return a private typed error instead of panicking
   on impossible capacity.
4. Run `cargo test --locked -p dogmos-core transaction` and commit as
   `feat(core): add indexed transaction arena`.

## Task 2: Port equalize staging and atomic commit

**Files:**

- Modify: `crates/dogmos-core/src/world.rs`
- Modify: `crates/dogmos-core/tests/frontier_processing.rs`
- Modify: `crates/dogmos-core/tests/world_state.rs`

1. Add failing integration tests that prove disconnected components cannot mutate the same mixture,
   an interleaved revision change prevents commit, cancellation leaves authoritative records and
   events untouched, and event overflow rolls back earlier components.
2. Replace `StageComponentState::staged_records` with `IndexedTransaction<MixtureRecord>`, created
   fallibly before the stage cursor becomes active and bounded by mixture slots/frontier length.
3. Split equalize calculation from publication. Touch candidates once, update transfer endpoints
   through the checked pair accessor, and checkpoint/truncate candidates and events on component
   failure.
4. Before publication, validate total event capacity, handle generation, and each initial revision.
   Sort entries by handle, then publish changed candidates and append events. Perform no fallible
   operation after the first authoritative write.
5. Keep existing discovery queues and sorted topology traversal unchanged.
6. Run focused equalize/frontier tests and commit as
   `perf(core): stage equalize in indexed transaction`.

## Task 3: Port excited-groups staging

**Files:**

- Modify: `crates/dogmos-core/src/world.rs`
- Modify: `crates/dogmos-core/tests/world_state.rs`

1. Add failing tests for cross-component duplicate mixture rejection and excited-groups cancellation
   atomicity while retaining the existing exact mixing expectations.
2. Route excited-groups candidates through the same transaction and common validated commit path.
3. Remove its per-component `BTreeMap<MixtureHandle, MixtureRecord>` clone/update/insert cycle.
4. Run focused excited-groups tests and commit as
   `perf(core): stage excited groups in indexed transaction`.

## Task 4: Account for memory and qualify allocations

**Files:**

- Modify: `crates/dogmos-core/src/world.rs`
- Modify: `crates/dogmos-core/tests/world_state.rs`
- Modify: `docs/performance/README.md`
- Modify: the active performance result document selected by the probe workflow

1. Add the transaction bitset, slot index, and dense-entry vector capacities to
   `reusable_workset_bytes`; add a focused regression test for the metric.
2. Run three fresh-process candidate probes for 100,000-turf equalize and excited-groups against the
   existing final-control CSV evidence.
3. Accept a migrated stage only if its transcript hash is identical in every run and allocated bytes
   fall by at least 50 percent. Record medians/ranges and whether retained idle service memory changed.
4. If a stage misses the threshold, profile the remaining allocation source and either make a
   bounded in-scope correction or revert only that stage's migration while retaining proven work.
5. Commit the accepted implementation and evidence as
   `docs(perf): qualify indexed transaction arena`.

## Task 5: Full verification and handoff

**Files:**

- Modify: this plan with final status/evidence

1. Run formatting and repository-prescribed static checks.
2. Run the pinned i686 workspace Clippy and test gates with all targets/features required by the
   repository verification guide.
3. Run the x64 `dogmos-core`/`dogmosd` build and test subset, protocol/legal IPC gates, and the
   applicable feature matrix.
4. Run the paired external game/DD qualification only if its documented environment is locally
   available; otherwise record it explicitly as the remaining external acceptance gate.
5. Update this plan with completed/deferred items, commit the verification record as
   `docs(perf): close transaction arena plan`, and leave the repository clean. Do not push.
