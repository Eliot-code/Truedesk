# Contributing to OpenDesk

Thanks for helping out! Issues and pull requests are welcome.

## Getting started

1. Fork the repo and create a branch from `main`.
2. Read [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) and
   [`docs/SECURITY.md`](docs/SECURITY.md) before touching the protocol,
   crypto, or relay.
3. Keep changes small and focused, one concern per pull request.

## Development

- Rust workspace: `crates/protocol`, `crates/relay`, `crates/host-core`.
- Desktop app: `viewer/` (React) and `viewer/src-tauri/` (Tauri backend; it is
  **not** part of the Cargo workspace, so build it with its own manifest).
- Builds need CMake (libopus is compiled from source). Linux also needs
  `libpipewire-0.3-dev`, `libspa-0.2-dev`, `libasound2-dev`, `libxdo-dev`, and
  `clang`.

## Before opening a pull request

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --manifest-path viewer/src-tauri/Cargo.toml --all-targets -- -D warnings
cd viewer && npm ci && npx vitest run && npm run build
```

Add or update tests for behavior changes. CI runs these checks on macOS,
Linux, and Windows.

## Commit messages

- Short and imperative, one line, under ~60 characters:
  `fix(relay): close surviving peer`.
- Optional prefix: `feat`, `fix`, `docs`, `ci`, `chore`, `refactor`, `test`, `perf`.
- Add a body only when the *why* isn't obvious from the diff. Keep it brief.

## License

OpenDesk is licensed under [AGPL-3.0-only](LICENSE). By submitting a pull
request, you agree that your contribution is licensed under the same terms.

## Security issues

Please don't open a public issue for vulnerabilities. Report them privately
through GitHub's private vulnerability reporting; see
[`docs/SECURITY.md`](docs/SECURITY.md#reporting-a-vulnerability).
