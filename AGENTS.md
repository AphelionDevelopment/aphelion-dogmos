# aphelion-dogmos agent instructions

aphelion-dogmos is the Rust half of Meridian-Rift's Dogmos atmosphere integration. Be direct, inspect existing implementations before changing them, preserve unrelated work, and leave changes uncommitted unless the user explicitly authorizes a commit.

## Required reading

- Routing and authority: [docs/agent/README.md](docs/agent/README.md) and [docs/agent/source-authority.md](docs/agent/source-authority.md).
- Architecture: [docs/agent/architecture-and-ownership.md](docs/agent/architecture-and-ownership.md), [docs/agent/process-boundary-and-protocol.md](docs/agent/process-boundary-and-protocol.md), and [docs/agent/gameplay-events.md](docs/agent/gameplay-events.md).
- Optimization: [docs/agent/performance-and-memory.md](docs/agent/performance-and-memory.md) and [docs/agent/numerical-invariants.md](docs/agent/numerical-invariants.md).
- Native boundary: [docs/agent/ffi-and-generated-bindings.md](docs/agent/ffi-and-generated-bindings.md).
- Gates and releases: [docs/agent/verification.md](docs/agent/verification.md) and [docs/agent/release-and-artifacts.md](docs/agent/release-and-artifacts.md).
- Source updates: [docs/agent/upstream-drift.md](docs/agent/upstream-drift.md).

## Ownership and implementation rules

The current audited crate is a 32-bit in-process BYOND DLL. Its Rust allocations consume DreamDaemon address space. The target architecture separates a thin `dogmos-byond` shim from a 64-bit `dogmosd` service. Route BYOND conversion and main-thread dispatch to `dogmos-byond`, domain rules and numerical kernels to `dogmos-core`, wire types to `dogmos-protocol`, and service lifecycle/state to `dogmos-server`. Only the shim may depend on `byondapi`.

Preserve public DM proc paths and caller-legible errors. No panic may unwind across the BYOND FFI boundary. Inputs and numerical state must be finite and validated; do not change atmosphere coefficients from intuition.

Generated bindings and release manifests are never hand-edited. Regenerate them with maintained tooling and compare exact output. Build BYOND-facing code for `i686-pc-windows-msvc` and `i686-unknown-linux-gnu`; host-only Cargo success is not authoritative.

## Verification boundary

Use the repository's exact pinned toolchain and `--locked`. Run formatting, strict Clippy, tests, supported feature combinations, i686 shim builds, generated-binding drift, and paired artifact verification as applicable. Verify the paired Meridian-Rift integration through its PowerShell DreamMaker/DreamDaemon gates. Report Rust, DM compile, focused tests, boot, full suite, and performance evidence separately.

Memory optimization targets DreamDaemon private/committed bytes and address-space pressure. Report `dogmosd` memory separately; do not combine it with DreamDaemon or optimize harmless 64-bit service RSS. Accept performance changes only from repeated identical workloads with numerical/event equivalence.
