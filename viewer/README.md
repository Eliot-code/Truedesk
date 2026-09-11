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

# 2. host  (target and password are the hardcoded PoC defaults: abc / test)
cargo run -p host-agent -- ws://127.0.0.1:8080/ws abc test

# 3. viewer  (built as its own Tauri workspace)
cd viewer && npm install && npm run tauri dev
```

The viewer currently uses target `"abc"` and password `"test"` as hardcoded
PoC defaults in `src/main.ts`. There is no user-facing password prompt yet.

## Capture requirements

The host captures the primary display, encodes it as H.264, and the viewer
renders the real host screen.

On macOS 12.3+, grant Screen Recording permission to the terminal or binary
that runs `host-agent` in **System Settings → Privacy & Security → Screen
Recording**. Until permission is granted, `scap::has_permission()` returns
`false`; `host-agent` prints a message and exits without sending anything.

The cross-platform capture path uses `scap` and software `openh264` encoding:

- Windows: Windows 10 version 1803 or newer, using Windows.Graphics.Capture.
- Linux: PipeWire plus `xdg-desktop-portal` for the Wayland screencast portal.

The Windows and Linux paths are code-complete through the same
`scap`/`openh264` implementation, but runtime verification has so far only
been performed on macOS. Encoding is software for now; hardware encoders can
be added later behind the `Encoder` trait.

## Input control requirements

The viewer now controls the host with mouse and keyboard input captured in the
viewer. Events are sealed viewer-to-host with `k_v2h` and injected on the host
via `enigo`.

- **macOS:** Grant **Accessibility** permission to the `host-agent` process in
  **System Settings → Privacy & Security → Accessibility**. This is separate
  from Screen Recording permission. Without Accessibility permission,
  `host-agent` prints guidance and the deep-link, then runs video-only.
- **Windows:** Input works out of the box through `SendInput`.
- **Linux:** Input uses `uinput`/XTest; injection is limited on Wayland.

## Security acceptance behavior

- **Matching password** — SPAKE2 handshake completes, key confirmation
  succeeds on both sides, and the viewer decrypts valid H.264 frames. Each
  frame is encoded before it is sealed with the SPAKE2-derived key, so no
  cleartext frame leaves the host; the relay only forwards opaque bytes.
- **Wrong password** — key confirmation fails: the host panics with
  `"key confirmation FAILED"`, and the viewer's `kex_verify` call throws.
  In both cases the handshake aborts before any frame is sent/decrypted —
  no frames are ever decrypted with a mismatched password. End-to-end
  encryption remains intact: the relay never sees the password, derived key,
  or unsealed frame. Input is E2EE as well: it is sealed viewer-to-host with
  `k_v2h`, so a wrong password blocks input just like it blocks video.

## Not yet implemented (PoC scope)

- A user-facing password prompt in the viewer UI (password/target are
  hardcoded defaults in `src/main.ts`).
- Password rotation/expiry or brute-force throttling on the relay.

Audio capture and multi-monitor selection are also not implemented. Hardware
encoding is deferred; `openh264` is the current software encoder.
