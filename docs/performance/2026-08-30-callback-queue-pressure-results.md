# Legacy callback queue ownership results

Date: 2026-08-31

## Scope

This result covers the retained 32-bit in-process callback queue in `auxcallback`. It does not
describe the `dogmosd` service callback queue and does not combine DreamDaemon memory with service
RSS.

## Implemented telemetry

Each queued callback now owns an envelope containing the closure and an explicit heap-capacity
lower bound supplied by its producer. The queue adds `size_of::<DeferredFunc>()` to that estimate.
Allocator metadata, the concrete closure allocation, shared allocations, opaque `eyre::Report`
storage, and BYOND-owned memory are excluded.

The exposed ownership fields are:

- `owned_bytes_lower_bound_current`;
- `owned_bytes_lower_bound_high_water`;
- `owned_bytes_lower_bound_enqueued`.

Item enqueue/drain totals, current depth, depth high water, and enqueue failures remain separate.
All counters saturate. Draining or cleanup releases current ownership, and starting a new world
resets the per-world metrics. Repository producers cannot bypass accounting through a raw channel
sender.

Heap-capacity accounting was added for the FDM nested pressure batches, Katmos pressure batches,
reaction requirement vectors, and heat-worker error strings. Fixed scalar and BYOND-value captures
report zero dynamic bytes. Opaque error-report internals remain an explicit lower-bound omission.

## Local verification

Pinned compiler: `rustc 1.98.0 (88d9e12ae 2026-08-18)`.

| Gate | Result |
| --- | --- |
| `cargo +1.98.0 test --locked --target i686-pc-windows-msvc -p auxcallback` | Pass: 5 tests |
| `cargo +1.98.0 test --locked --target i686-pc-windows-msvc -p dogmos --features turf_processing,katmos,superconductivity` | Pass: 44 tests |
| `cargo +1.98.0 clippy --locked --target i686-pc-windows-msvc -p auxcallback --all-targets -- -D warnings` | Pass |
| `cargo +1.98.0 clippy --locked --target x86_64-pc-windows-msvc -p dogmos-server --all-targets -- -D warnings` | Pass |
| `cargo +1.98.0 fmt --all -- --check` | Pass |

The focused regression queues a closure owning a `Vec<u64>` with capacity 128. Current ownership
increases by at least the vector capacity plus the deferred-function handle, returns to the starting
value after drain, and leaves cumulative ownership monotonic. A separate test covers saturation and
world cleanup/reset.

## Required live series

No representative retained-legacy pressure workload is available in the paired Meridian-Rift
checkout. Its maintained Dogmos tests and liveness soak exercise the service backend and therefore
cannot measure this in-process queue. Substituting that service workload would produce the wrong
process and ownership evidence.

The following Task 4 acceptance evidence remains unrun:

- three identical retained-legacy DreamDaemon pressure runs;
- enqueue/drain counts and callback event equivalence;
- item and owned-byte current/high-water series;
- DreamDaemon private/committed bytes and address-space pressure;
- the exact paired DM response to a callback admission failure.

`dogmosd` private bytes/RSS, if collected later, must be reported in a separate column and is not a
substitute for DreamDaemon memory.

## Policy checkpoint

No callback capacity is selected here. The queue remains unbounded and preserves the existing
critical-event behavior. Task 5 is blocked until a retained-legacy workload supplies measured high
water and the paired DM failure/recovery behavior is explicitly approved.
