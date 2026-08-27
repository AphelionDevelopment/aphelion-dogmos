# Process boundary and protocol

A Rust DLL does not escape its host process. Any in-process DLL allocation, including Rust heap capacity reserved by `dogmos.dll`, remains a DreamDaemon allocation and contributes to its constrained address space. Memory relief requires authoritative growing state to live in the separate 64-bit `dogmosd` process.

The shim uses a small local control channel for launch, authenticated handshake, health, shutdown, and diagnostics. A fixed-size shared-memory request/reply window is permitted only when measurement shows the control channel misses the latency budget. Never map the gas arena, graphs, scratch collections, callback history, or other dynamically growing service state into DreamDaemon. Total shim and mapped transport space is fixed and bounded.

Wire values are explicitly little-endian and fixed width. Every frame includes protocol version, operation kind, request ID, world generation/nonce, payload length, and deadline. Wire types contain no pointers, `usize`, Rust-layout enums, `String`, `Vec`, or allocator ownership. Reject unknown kinds, invalid lengths, stale generations, mismatched replies, duplicate continuations, and corrupt data.

The shim performs bounded waits and fails closed. Startup retry is allowed before world registration. Mid-round service death, timeout, or contract mismatch is fatal because a replacement service does not possess the authoritative gas state. Do not silently restart, reconstruct empty atmosphere, or fall back to an in-process arena.

On Windows, do not claim a bounded wait merely because `deadline_ns` is populated. The pinned
`interprocess` named-pipe backend rejects receive/send timeout configuration and implements reads
with `ReadFileEx`. Use `BoundedDogmosClient`: it owns a dedicated I/O worker, waits through a bounded
channel, cancels overlapped pipe I/O with `CancelIoEx`, and cancels synchronous writes with
`CancelSynchronousIo`. A timeout permanently poisons the client; the owning session closes the
kill-on-close job and terminates the service rather than accepting a late reply. Keep the stalled-read
i686 test as a required gate. A server-side elapsed-time check alone cannot release DreamMaker from a
stuck read.

`deadline_ns` is a relative service-processing budget, not a wall-clock timestamp. Check it before
dispatch and at bounded intervals inside long native stages. A cancellable stage writes only to
service-owned scratch space, checks the deadline again, and commits atomically; expired work must not
partially advance mixture revisions. The shim's channel timeout independently bounds DreamMaker and
terminates the authoritative service on failure.

Events carry typed numeric handles and generations. DreamDaemon resolves them on its main thread and rejects stale identities. If a synchronous DM reaction is required, the service releases simulation locks and returns a single-use, generation-bound continuation before DM code runs. Never wait for DM while holding a world, mixture, or graph lock.

Callback/event storage is service-owned and negotiated at handshake. Enqueue complete batches
atomically; when a batch cannot fit, return typed backpressure and do not partially enqueue or drop
critical events. Drain through a fixed shim buffer with monotonically increasing sequence numbers,
bounded batch size, remaining depth, high water, and rejection telemetry. A diagnostic event kind
may qualify the transport, but production migration requires explicit event kinds and equivalence
tests for every existing main-thread gameplay effect. Protocol v3's exact 64-byte envelope and
implemented event inventory are retained by protocol v4 and defined in
[Gameplay events](gameplay-events.md).

Protocol v4 adds the fixed-width mixture-state batch required to seed nonzero authoritative service
state. Every record carries one slot/generation handle, an expected revision, temperature, volume,
and all 32 gas slots. The service validates the complete counted batch, rejects stale revisions,
duplicates, invalid physical values, and reserved fields, then commits every record or none. Slot
generation tombstones survive unregister so an older or equal generation cannot target a reused
slot. Registered empty state starts at the 2.7 K legacy temperature floor with zero volume; seeded
state accepts finite non-negative volume and enforces the same temperature floor. State conflicts
return stable error codes rather than collapsing into a generic malformed-request response.

Batch only measured sequences. Write-only commutative operations may coalesce; immediate reads, reactions, and causal gameplay events are ordering barriers. A generic remote evaluator or bytecode interface is not permitted.
