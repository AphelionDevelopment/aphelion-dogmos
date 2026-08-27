# Release and artifacts

A Dogmos release is a paired contract, not a standalone DLL. It contains:

- 32-bit Windows `dogmos.dll` and Linux `libdogmos.so` shims plus symbols;
- 64-bit Windows `dogmosd.exe` and Linux `dogmosd` services plus symbols;
- generated `dogmos_bindings.dm`;
- deterministic `dogmos-release-manifest.json`.

The manifest identifies schema, ABI and protocol versions, crate version, source repository and exact 40-character revision, Rust toolchain, byondapi revision, sorted features/fingerprint, bindings hash, platform targets, filenames, and raw-byte hashes. Shim and service must agree on revision, ABI, protocol, feature fingerprint, and executable identity during their authenticated startup handshake.

Build BYOND-facing artifacts for i686 and service artifacts for x86_64 with the exact pinned toolchain, sorted supported features, and `--locked`. Development builds may identify as `development`; release generation rejects that value. Publish full symbols separately while keeping usable crash file/line diagnostics.

Never fetch a mutable branch for production deployment. The paired Meridian-Rift checkout installs artifacts atomically only after manifest, architecture, filename, executable permission, bindings, and hash verification. A missing, truncated, mismatched, or cross-revision member rejects the entire set before game initialization.

At runtime, the shim streams `dogmosd.exe` through Windows CNG SHA-256 before launch and places that
digest in the authenticated startup identity. The service independently hashes its own current
executable before it creates the named pipe. This closes parent-only digest assertion; it does not
replace signed or otherwise trusted release-manifest provenance.

Release workflows, artifact generators/synchronizers, dependency manifests, Cargo lock/toolchain files, Docker, and deployment scripts are protected files under [the root policy](../../AGENTS.md). Name exact files/effects and obtain explicit approval before editing them.
