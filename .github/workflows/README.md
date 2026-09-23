# OpenDesk CI

The workflow keeps platform-sensitive checks separate and independent:

- `rust` runs the cross-platform `protocol` and `relay` build, tests, and clippy on Ubuntu.
- `host-core` runs the macOS-dependent host build, live loopback tests, and clippy on macOS.
- `tauri` checks and lints the standalone `viewer/src-tauri` manifest on macOS.
- `viewer` installs the locked Node 20 dependencies and runs the Vite build and Vitest suite on Ubuntu.

The scoped `cargo fmt --check -p protocol -p relay` was tested locally but currently reports existing formatting differences. It is intentionally not a CI step because this task must not modify Rust source files; the Rust clippy checks remain enabled with warnings denied.
