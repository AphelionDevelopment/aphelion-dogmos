# FFI and generated bindings

Every BYOND-bound proc is a panic boundary. Convert and validate `ByondValue` inputs at the edge, call typed domain/protocol code, and translate errors into caller-legible BYOND failures. A Rust panic is caught, attributed to the exported proc, recorded through bounded diagnostics/telemetry, and returned as an error; it never unwinds into DreamDaemon.

Keep public DM proc paths stable. Raw binds that require a DM compatibility wrapper retain the established `__` convention until the generated contract replaces it. Main-thread callbacks and DM references stay inside the shim; core/server code receives only typed handles and immutable metadata.

`dogmos_bindings.dm`, contract defines, release manifests, and binding inventories are generated bindings or generated contract artifacts. Never hand-edit them. Regenerate them from the reviewed Rust revision with the maintained tool, compare normalized proc paths/symbols and then exact deterministic bytes, and review any public addition/removal as an ABI change.

Generated output uses stable ordering and one LF ending. Release generation rejects a development source revision. The game consumes bindings only together with the matching shim, service, ABI/protocol metadata, feature fingerprint, and hashes.

FFI tests cover malformed types, missing arguments, non-finite numbers, stale handles, panics with string/non-string payloads, service timeout/death, response mismatch, and reentrancy continuations. A generated binding-count check is useful drift evidence but does not replace native-load boot verification.
