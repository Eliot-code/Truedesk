# OpenDesk Viewer

Tauri-based viewer for the OpenDesk remote desktop PoC. It connects to the
relay over WebSocket, runs a SPAKE2 password-authenticated key exchange
against the host-agent, and decrypts incoming frames.

Crypto (SPAKE2 handshake + key confirmation + AEAD decrypt) runs entirely in
the Rust Tauri backend, not in the webview's TypeScript — the viewer's JS
never holds a key.

## Build layout

- The root Cargo workspace (`opendesk/Cargo.toml`) covers `crates/*`
  (`relay`, `host-agent`, `protocol`, etc).
- `viewer/src-tauri` is its **own**, separate Cargo workspace — it is not a
  member of the root workspace. Build/run it via `npm run tauri dev` (or
  `cargo run --manifest-path viewer/src-tauri/Cargo.toml` if you need to
  invoke Cargo directly).

## Run recipe

Run each of these in its own terminal, in order:

```bash
# 1. relay
cargo run -p relay

# 2. host  (last arg is the password; must match the viewer)
cargo run -p host-agent -- ws://127.0.0.1:8080/ws abc test

# 3. viewer  (built as its own Tauri workspace)
cd viewer && npm install && npm run tauri dev
```

The viewer currently uses password `"test"` and target `"abc"` as hardcoded
PoC defaults in `src/main.ts`. There is no user-facing password prompt yet —
to test against a different password/target, edit those defaults or pass
matching values to `host-agent` on the command line above.

## Security acceptance behavior

- **Matching password** — SPAKE2 handshake completes, key confirmation
  succeeds on both sides, and the viewer starts receiving and attempting to
  decrypt (AEAD `open()`) frames. The frame payload is still a dummy `0xAB`
  buffer, not real H.264, so the browser's `VideoDecoder` will error on it —
  that is **expected** at this stage of the PoC. The thing actually being
  proven is that AEAD `open()` succeeds, i.e. both sides derived the same
  session key.
- **Wrong password** — key confirmation fails: the host panics with
  `"key confirmation FAILED"`, and the viewer's `kex_verify` call throws.
  In both cases the handshake aborts before any frame is sent/decrypted —
  no frames are ever decrypted with a mismatched password. This is the
  end-to-end encryption guarantee: the relay only ever forwards opaque
  bytes and never sees the password or the derived key.

## Not yet implemented (PoC scope)

- Real screen capture / H.264 encoding on the host (frames are a fixed dummy
  payload).
- Real input injection.
- A user-facing password prompt in the viewer UI (password/target are
  hardcoded defaults in `src/main.ts`).
- Password rotation/expiry or brute-force throttling on the relay.
