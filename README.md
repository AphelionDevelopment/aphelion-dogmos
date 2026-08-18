# Dogmos

Rust-based atmospherics for Meridian Rift, a Space Station 13 downstream, using
[byondapi](https://github.com/spacestation13/byondapi-rs).

The compiled binary on Citadel is compiled for Citadel's CPU, which therefore means that it uses [AVX2 fused-multiply-accumulate](https://en.wikipedia.org/wiki/Advanced_Vector_Extensions#Advanced_Vector_Extensions_2).

Binaries in releases are without these optimizations for compatibility. But it runs slower and you might still run into issues, in that case, please build the project yourself.

Dogmos is a maintained downstream of [Auxmos](https://github.com/Putnam3145/auxmos). Build it like any Rust project with Clang 6 or newer and `LIBCLANG_PATH` set to Clang's `bin` directory on Windows. Supported targets are `i686-unknown-linux-gnu` and `i686-pc-windows-msvc`.

Use `cargo test generate_binds` to generate `bindings.dm` for the DM codebase, or use the repository copy generated with the default Dogmos features.

The `master` branch is unstable. Meridian Rift integrations should use the `dogmos` branch and its matching vendored bindings and DLL.
