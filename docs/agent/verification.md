# Verification matrix

| Evidence | Required use | Boundary |
| --- | --- | --- |
| Focused unit/property test | Every behavioral correction, written and observed failing first | Proves only the named break |
| `cargo fmt --all -- --check` | Rust source changes | Formatting only |
| Strict Clippy with `--locked` | Changed packages on their supported target | Static Rust gate, not runtime integration |
| i686 workspace/shim tests | BYOND-facing Rust | Required; a host-only pass is insufficient |
| x86_64 core/server tests | Service/core/protocol/performance crates | Required after the split |
| Feature matrix | Feature ownership or shared modules | Use supported combinations, not intentionally invalid all-features |
| Generated-binding/manifest drift | Exports, features, releases | Deterministic contract gate |
| PowerShell DreamMaker compile | Paired Meridian-Rift checkout | Compiler acceptance |
| PowerShell focused DM tests | Changed integration behavior | Iteration evidence only |
| PowerShell boot probe | Native load and initialization | Requires initialization marker and runtime review |
| Full DM suite | Integration completion | Required when DM behavior can regress broadly |
| Repeated process/Tracy workload | Memory or performance claim | Separate DreamDaemon/service measurements and equivalence |

Use the exact repository-pinned Rust toolchain and `--locked`. The authoritative pre-split Windows baseline is:

```powershell
cargo +1.98.0 test --workspace --locked --target i686-pc-windows-msvc
if ($LASTEXITCODE -ne 0) { throw 'Rust tests failed.' }
```

Run DreamMaker and DreamDaemon through the paired game repository's maintained PowerShell entry points. Use Meridian-MCP for DM parsing, navigation, diagnostics, and Tracy analysis, not as a substitute for those build/test gates.

Report exact commands, tool/target versions, scope, exit/result artifacts, warnings, runtime signatures, and gates not run. Distinguish executable tests from ignored doc tests. Never call a focused run, parser success, process liveness, or a plain host `cargo test` complete evidence.

Before handoff, run `git diff --check`, inspect every protected-file diff separately, confirm source/contract revisions and hashes, and leave changes uncommitted unless the user authorizes otherwise.
