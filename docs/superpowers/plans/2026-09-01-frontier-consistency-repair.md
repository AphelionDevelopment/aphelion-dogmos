# Frontier Consistency Repair Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep DreamMaker and `dogmosd` frontier/topology state consistent across invalid turf lifecycle state, rejected mutations, resumable stages, and fatal service shutdown.

**Architecture:** DreamMaker validates and stages frontier changes before publication, then commits local epochs only after service acceptance. `dogmos-core` applies the same active-stage barrier to mixture lifecycle teardown as every other topology mutator. A DM failure latch prevents post-failure FFI calls while the controlled reboot proceeds.

**Tech Stack:** Rust 1.98.0, Dream Maker, Dogmos fixed-width IPC, RIFT controller.

**Spec:** `docs/superpowers/specs/2026-09-01-frontier-consistency-repair-design.md`

## Global Constraints

- Preserve unrelated dirty work in the Meridian-Rift checkout.
- Do not hand-edit generated bindings, contract defines, release manifests, or installed native artifacts.
- Use the pinned Rust 1.98.0 toolchain and `--locked`.
- Build BYOND-facing Rust for `i686-pc-windows-msvc` and the service/core for `x86_64-pc-windows-msvc`.
- Treat the original fail-closed diagnostic as authoritative and never provide an in-process atmosphere fallback.

---

### Task 1: Native active-stage lifecycle barrier

**Files:**
- Modify: `crates/dogmos-core/src/world.rs`
- Test: `crates/dogmos-core/src/world.rs`

**Interfaces:**
- Consumes: `DogmosWorld::apply_lifecycle(&[LifecycleMutation])` and `StageConflictReason::ActiveStageMutation`.
- Produces: lifecycle batches return `WorldError::StageConflict` without changing mixture or topology state while `stage_cursor` is active.

- [ ] Add a focused test that starts a resumable stage, attempts a mixture unregister, and asserts `ActiveStageMutation`, unchanged topology revision, and unchanged mixture state.
- [ ] Run the focused test and confirm it fails because `apply_lifecycle` currently mutates during the active stage.
- [ ] Add the minimal active-stage guard to `apply_lifecycle`.
- [ ] Rerun the focused test and the `dogmos-core` test package.

### Task 2: DM frontier validation and atomic publication

**Files:**
- Modify: `C:/Users/Zoe/Documents/GitHub/Meridian-Rift/modular_aphelion/modules/dogmos/code/service_backend.dm`
- Modify: `C:/Users/Zoe/Documents/GitHub/Meridian-Rift/modular_aphelion/modules/dogmos/code/service_backend_test.dm`
- Modify only if required by the caller contract: `C:/Users/Zoe/Documents/GitHub/Meridian-Rift/code/controllers/subsystem/air.dm`

**Interfaces:**
- Consumes: active turf registration state, `dogmos_frontier_add`, `dogmos_frontier_remove`, and the current four-word frontier epoch.
- Produces: explicit success/failure from frontier synchronization and chunk publication; local epoch and committed frontier change only after accepted responses.

- [ ] Add a DM unit test proving an invalid active turf cannot change the frontier epoch or committed frontier.
- [ ] Add a DM unit test proving a rejected/malformed incremental response cannot publish the candidate epoch.
- [ ] Run the focused tests and confirm their expected failures.
- [ ] Add normal-path catch-up registration, complete pair validation, diagnostic context, candidate epochs, fixed-width response validation, and explicit failure returns.
- [ ] Update callers to fail closed once and stop the current atmosphere pass when synchronization fails.
- [ ] Rerun both focused tests and nearby Dogmos frontier/lifecycle tests.

### Task 3: Fail-closed quiescence

**Files:**
- Modify: `C:/Users/Zoe/Documents/GitHub/Meridian-Rift/modular_aphelion/modules/dogmos/code/dogmos.dm`
- Modify: `C:/Users/Zoe/Documents/GitHub/Meridian-Rift/modular_aphelion/modules/dogmos/code/service_backend.dm`
- Modify: `C:/Users/Zoe/Documents/GitHub/Meridian-Rift/modular_aphelion/modules/dogmos/code/service_backend_test.dm`

**Interfaces:**
- Consumes: `service_ready`, SSair fatal-stage handling, and Dogmos FFI entry points.
- Produces: a recovery-preserved failure latch that prevents further native requests after the authoritative stage failure.

- [ ] Add a focused test that latches fail-closed state and exercises stage, mixture, registration, and turf update entry points without advancing request telemetry or pending state.
- [ ] Run the focused test and confirm existing entry points continue after `CRASH()` or reach FFI paths.
- [ ] Add and recover/reset the failure latch, then return explicitly from unavailable-service branches.
- [ ] Rerun the focused test and the existing malformed-stage/fail-closed test.

### Task 4: Source verification

**Files:**
- Verify all modified source and test files.

**Interfaces:**
- Consumes: completed Tasks 1-3.
- Produces: formatted, lint-clean, compiler-accepted source with focused and wider tests passing.

- [ ] Run `cargo +1.98.0 fmt --all -- --check`.
- [ ] Run strict locked Clippy for supported changed packages and targets.
- [ ] Run locked x86_64 core/server tests and i686 workspace tests.
- [ ] Reparse Meridian-Rift through Meridian-MCP if available and inspect diagnostics separately.
- [ ] Run focused RIFT Dogmos tests, fast DreamMaker compile, and applicable wider DM suite.
- [ ] Run `git diff --check` and inspect every changed file against unrelated dirty work.

### Task 5: Paired artifact synchronization and runtime qualification

**Files:**
- Generated and installed only through maintained tooling: `C:/Users/Zoe/Documents/GitHub/Meridian-Rift/code/__DEFINES/dogmos_bindings.dm`, `C:/Users/Zoe/Documents/GitHub/Meridian-Rift/code/__DEFINES/dogmos_contract.dm`, `C:/Users/Zoe/Documents/GitHub/Meridian-Rift/dogmos.lock.json`, and the paired platform artifacts.

**Interfaces:**
- Consumes: one exact verified aphelion-dogmos revision.
- Produces: a matched shim/service/bindings/contract installation suitable for the next playtest.

- [ ] Build the release i686 shim and x86_64 service with the pinned toolchain.
- [ ] Generate a deterministic paired release through maintained tooling.
- [ ] Verify the staging manifest, architectures, hashes, bindings, and source revision.
- [ ] Install the complete pair atomically through the maintained synchronizer.
- [ ] Run contract verification, full-map boot, bounded soak, and process cleanup checks.
- [ ] Commit the approved source and generated changes only after applicable gates pass.
