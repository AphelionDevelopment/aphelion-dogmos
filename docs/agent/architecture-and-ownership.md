# Architecture and ownership

## Current audited state

The root `dogmos` package builds a `cdylib` loaded into 32-bit DreamDaemon. Process-global arenas, graphs, workers, scratch buffers, gas/reaction registries, and callback queues therefore share DreamDaemon's address space. DM owns datum/turf identity, subsystem scheduling, machinery, gameplay effects, logging, administration, and UI.

This current state is the migration source, not the intended ownership boundary.

The service prototype now constructs a BYOND-free `dogmos_core::world::DogmosWorld`. It owns the
generation-checked mixture slots, immutable numeric gas-metadata registry, adjacency map, validated
diffusion graph, reusable input/output buffers, snapshots, and atomic stage commit. Core also owns an
immutable reaction registry with fixed-width IDs, validated gas requirements, and a compact numeric
priority order. `dogmos-server` translates fixed-width protocol values and owns the transport/event
outbox; it no longer owns a duplicate mixture arena. Reaction execution and continuations, Katmos,
turf heat, the DM registration adapter, and the complete production command surface remain in the
legacy root crate and are not yet migrated.

## Target state

| Component | Owns | Must not own |
| --- | --- | --- |
| `dogmos-byond` | BYOND value conversion, connection lifecycle, bounded IPC windows, response validation, main-thread event dispatch | Growing world state, numerical arenas, DM policy |
| `dogmos-protocol` | Fixed-width handles, versioned messages, codecs, stable error/event kinds | `byondapi`, pointers, allocator-owned wire fields, world logic |
| `dogmos-core` | Gas/mixture state, reactions, graphs, numerical kernels, typed commands/events, world generation | `ByondValue`, DM refs, transport, process globals |
| `dogmos-server` / `dogmosd` | Authoritative 64-bit `DogmosWorld`, scheduling, workers, bounded event outbox, health and metrics | BYOND references, player/admin policy, automatic empty-state restart |

Only `dogmos-byond` may depend on `byondapi`. Public protocol/core types use fixed-width integers and explicit generations rather than `usize` or raw slots. A mixture or turf handle is valid only for its world generation; slot reuse must not make a stale request target new state.

`tools/check_dependency_direction.py` enforces that `dogmos-core` and `dogmos-protocol` do not
depend on `byondapi`, contain DM-call identifiers, or expose pointer-sized public numeric state
fields such as `usize`. The CI tooling-test suite runs this guard while extraction is in progress.

DM remains authoritative for datum identity, public proc compatibility, subsystem cadence, machinery decisions, atom movement, gameplay consequences, logs, rights, and TGUI. `dogmosd` returns typed facts/events; the shim validates and dispatches them but does not invent game policy.

Move code by responsibility, not filename. Extract pure math before moving orchestration. Keep an adapter only while differential transcript tests prove old and new paths equivalent. Do not duplicate a growing arena in the shim as a fallback.
