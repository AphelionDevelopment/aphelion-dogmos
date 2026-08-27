# Meridian-MCP issues update - 2026-08-27

This is the short handoff sheet for the Meridian-MCP maintainers. The canonical evidence ledger is
[Meridian-MCP profiling findings](meridian-mcp-profiling-findings.md); issue IDs below remain stable
between updates. These are tooling findings, not Dogmos performance regressions.

## Current open items

| ID | Priority | Current issue | Requested next change |
| --- | --- | --- | --- |
| MMCP-PROF-017 | P1 | The MCP accepted the primary Meridian-Rift checkout but rejected an explicitly selected sibling Codex worktree. Path-policy errors did not identify the effective allowed roots or policy owner. | Report containment mode and allowed roots, and support explicit authorization of sibling worktrees from the same repository. |
| MMCP-PROF-018 | P1 | Exact child proc overrides can appear in search and document-symbol results while `dm_get_proc` returns only the inherited proc. | Make search, definition, document-symbol, and proc lookup agree on the canonical override owner. |
| MMCP-PROF-019 | P1 | Standard `dm_run` workloads can modify tracked game assets without a post-run source-integrity warning. | Extend integrity journaling to standard run/stop and report tracked mutations without reverting them. |
| MMCP-PROF-020 | P1 | BYOND proc profiles, performance CSV, sendmaps data, JSONL events, and runtime logs still require manual identity and phase validation. | Add bounded native-evidence readers with hashes, time semantics, phase selection, privacy defaults, and comparison guards. |
| MMCP-PROF-021 | P1 | A contained IPC fixture drifted across upgrades: the harness omitted its protocol-v4 state call and generated bindings omitted that proc. After the source call was restored, `dm_compile` failed correctly, but the previous DMB remained available at the same path. | Hash-bind fixture inputs to the DMB, classify the old DMB as stale after input changes or compile failure, refuse to launch it, and add a generated-binding completeness check. |

## Latest reproduction: fixture and stale DMB

1. Fresh protocol-v4 `dogmos_byond.dll` and `dogmosd.exe` were copied into the contained fixture.
2. Inspection found that `dogmos_ipc_benchmark.dm` no longer called
   `dogmos_ipc_benchmark_state_batch`, so a nominal v4 smoke would have processed only zero-gas
   mixtures.
3. Restoring the call exposed that `dogmos_ipc_benchmark_bindings.dm` also lacked the proc.
4. `dm_compile` returned one `undefined proc` error, `success: false`, and `dmb_updated: false`.
5. The older DMB remained present with SHA-256
   `A85696CB769EAA3433AB8F9A2D1ACEE8E94839FB22A5DC80B284EA0494B6AF11`.

Local Windows binding generation also omitted the new state export and two existing callback
exports even though Rust macro expansion contained their inventory entries. The disposable fixture
therefore used one explicit state wrapper for the smoke. This is not an approved production binding
generation path; production bindings remain subject to the existing Linux generation and hash
comparison gate.

The compile response was accurate; the missing protection is a launch-time proof that the DMB was
built successfully from the current source, bindings, and native-artifact set.

## Previously remediated profiling items

The updated Windows qualification closed the tested scenarios for MMCP-PROF-001, 002, 004, 005,
013, 014, 015, and 016. Those fixes covered invalid capture rejection, persistent drain/producer
health, launch readiness, atomic artifact publication, workspace-integrity journals, and exact MCP
build identity. MMCP-PROF-003, 006-012 remain requirements of the evidence contract even where code
and fixtures exist; Linux live qualification remains CI-owned.
