# OpenDesk CI

`ci.yml` runs on every push and pull request to `main`:

- `rust`: `protocol` and `relay` build, tests, and clippy on Ubuntu.
- `host-core`: host build, loopback tests, and clippy on macOS.
- `tauri`: check and clippy for `viewer/src-tauri` (its own manifest) on macOS.
- `viewer`: Vite build and Vitest suite on Ubuntu (Node 24).
- `host-core-cross` / `tauri-cross`: compile checks for the Windows and Linux
  host code, which the macOS jobs don't build.
- `cargo-deny`: advisories, licenses, bans, and sources.

`release.yml` runs on `v*` tags and builds unsigned bundles for macOS, Linux,
and Windows (see `docs/RELEASE.md`).

Linux jobs are pinned to `ubuntu-24.04` so a runner image change can't
silently rename the apt packages they install.

Supply chain: third-party actions are pinned to commit SHAs (the tag is in a
comment), cargo runs with `--locked`, `deny.toml` allows only crates.io
sources, and Dependabot (`.github/dependabot.yml`) proposes updates as PRs.
