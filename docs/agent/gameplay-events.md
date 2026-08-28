# Gameplay event wire contract

`dogmosd` reports typed computation results; DreamMaker validates targets and owns every gameplay
effect. The service event queue is bounded and authoritative. The i686 shim uses one reusable 64 KiB
response buffer and must not retain a decoded event or construct a persistent BYOND event list.

Protocol v3 introduced the fixed 64-byte event retained by protocol v7: sequence `u64`, kind `u16`,
flags `u16`, subject and target slot/generation handles, four finite `f64` values, and an auxiliary
`u32`. The 24-byte batch header plus 1,023 complete events fits in the fixed shim buffer. Unknown
kinds, flag bits, auxiliary enum values, non-finite values, sequence gaps, and stale generations fail
closed.

The implemented wire kinds are diagnostic, reaction finished, pressure difference, decompression
floor rip, firelock consideration, turf destruction request, DM reaction continuation, and reaction
profiled. Reaction-finished auxiliary values are plasma, hydrogen, tritium, and freon. A profiled
reaction event instead carries the dense registered reaction ID in the auxiliary field and its
service execution cost in value 0. Profiling is opt-in per reaction request, uses a finite
non-negative threshold, and emits no profiling event below that threshold.

Reaction values preserve the current DM callback arguments:

| Reaction | Value 0 | Value 1 | Value 2 | Value 3 |
| --- | --- | --- | --- | --- |
| Plasma | fire amount in mol | post-reaction temperature in K | zero | zero |
| Hydrogen | burned fuel in mol | post-reaction temperature in K | zero | zero |
| Tritium | burned fuel in mol | released energy in J | mixture volume in L | post-reaction temperature in K |
| Freon | reaction result amount in mol | pre-reaction temperature in K | post-reaction temperature in K | zero |

DreamMaker times a continued DM reaction locally because that work does not execute in `dogmosd`.
It records both native and DM reaction costs through the same bounded Kennel history after resolving
the typed holder and reaction identity on the main thread.

Do not add a visual-update kind until the game repository defines a complete bounded visual payload
or versioned snapshot token. A trigger-only callback is incorrect after the authoritative gas state
moves out of DreamMaker.

Build simulation state and its complete event batch in scratch storage. Validate capacity, deadline,
values, and handles before committing either. Backpressure rejects the whole batch; no critical event
may be partially enqueued, overwritten, or dropped. Each event is self-contained so DreamMaker can
apply it immediately without retaining an incomplete group across drains.

Only DreamDaemon memory is the footprint acceptance target. Report the `dogmosd` queue and process
memory separately, and never add service memory to DreamDaemon measurements.
