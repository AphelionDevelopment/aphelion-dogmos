# Performance audit repairs

The user approved implementation of every finding in the September 5 audit, with tests at the end. Work stays in the existing checkout, uncommitted. No unrelated changes are included.

## Design and constraints

Preserve public DM exports, wire layouts, numerical coefficients, deterministic event order, generation checks, and atomic stage publication. Growing simulation storage remains service-owned. Prefer bounded reusable storage and explicit ownership over additional threads or a new transport.

## Implementation sequence

- [x] Bound direct-reaction reservation by the registered reaction inventory, including profiling and continuation events. Cover zero-event reactions and simultaneous transactions in server tests.
- [x] Retain stage scratch across successful completion and cancellation; use fixed neighbor buffers and reusable indexed membership. Extend allocation evidence to warmed cycles and allocator live-byte high water.
- [x] Make component computation resumable and account for publication work without exposing partial state. Preserve cross-component revision checks and cancellation rollback.
- [x] Share component membership/traversal storage with equalization and excited-group kernels while retaining their distinct traversal and group-limit semantics.
- [x] Index mixture-to-turf and mixture-edge ownership; preserve stable frontier order while applying batched removals without repeated whole-frontier scans.
- [x] Reduce intermediate snapshot conversion allocations and BYOND list operations using the pinned API's bulk list facilities. Preserve malformed-input validation before returning output.
- [x] Bound legacy callback admission and propagate a latched fatal error rather than silently discarding critical work. Keep shutdown and diagnostics available after failure.
- [x] Add reaction, sparse-frontier, lifecycle, warmed-stage, and per-chunk performance coverage.
- [ ] Complete every integration gate. Run pinned formatting, strict Clippy, i686 workspace tests, x64 service tests, feature matrix, tooling and binding drift checks, repeated performance probes, and available paired integration gates.

## Verification and reporting

The audit's three identical release allocation files are the cold control. Capture three candidate files and compare numerical/event transcripts. New warmed and reaction measurements establish additional baselines, not historical speedup claims. Record every failed or unavailable gate explicitly. DreamDaemon memory and live-round acceptance require paired runtime measurements and remain separate from native allocation evidence.

## Implementation outcome

See [the repair and verification report](../../performance/2026-09-05-audit-repairs.md) for the completed repairs, test review, measured tradeoffs, and unrun integration gates. Cancellation retains the outer stage buffers; an in-flight component future releases its private kernel state when cancelled.
