<div align="center">

# OpenDesk

A self-hosted, open-source remote desktop with end-to-end encryption.
An alternative to [AnyDesk](https://anydesk.com) and [TeamViewer](https://teamviewer.com), written in Rust.

Runs over `wss://` on port 443. The relay forwards encrypted bytes and never sees your screen, keystrokes, or keys.

</div>

> [!CAUTION]
> **Misuse Disclaimer:**
> OpenDesk is for accessing computers you own or have explicit permission to
> control. The developers do not condone or support any unethical or illegal
> use of this software. Misuse, such as unauthorized access, control, or
> invasion of privacy, is strictly against our guidelines. The authors are not
> responsible for any misuse of the application.

---

## Overview

OpenDesk lets one device view and control another over the internet through a relay server you host yourself. Each endpoint seals frames and input with a key that only the two peers hold, derived from a session password. The relay forwards bytes it cannot decrypt, so the encryption is end to end, not only in transit.

Each peer opens one outbound `wss://` connection on port 443. After SPAKE2 key
confirmation, the host and viewer gather and exchange ICE candidates and try a
direct UDP path. If that fails, the same WebSocket connection carries the
encrypted traffic instead. You don't need inbound ports or port forwarding. The
direct attempt needs outbound UDP; on networks that block it, the session falls
back to the relay. The relay assigns one pairing session ID and sends it to
both peers in `Control::Paired`, so it can route their ICE candidate messages
to the same session.

ICE uses Google Public STUN by default (`stun:stun.l.google.com:19302`) to find
server-reflexive addresses. You can substitute a self-hosted STUN service in
the host-core `AgentConfig.urls` configuration. STUN only helps discover
addresses; it does not relay media or receive desktop contents.

## Features and quickstart

- End-to-end encrypted screen, input, and audio
- End-to-end encrypted file transfer and text-only clipboard sync
- Optional unattended access with a stored password and an optional TOTP two-factor code
- Optional start at sign-in, so an unattended machine is reachable again after a restart
- Local address book and recent peer IDs
- Multi-monitor discovery and monitor selection

The host sends its system sound on macOS 14.6+, Windows, and Linux (PulseAudio, or PipeWire via pipewire-pulse). Elsewhere (older macOS, Linux without either server) it sends no sound. It never uses the microphone. Windows and Linux host support is less mature than macOS, which is where most testing has happened.

For the architecture, security model, and self-hosting instructions, see
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md),
[`docs/SECURITY.md`](docs/SECURITY.md), and
[`docs/DEPLOYMENT.md`](docs/DEPLOYMENT.md).

Quickstart:

1. Deploy the relay on any server with Docker and a DNS name:
   `DOMAIN=relay.example.com docker compose up -d` (automatic HTTPS; see
   [`docs/DEPLOYMENT.md`](docs/DEPLOYMENT.md)). Set clients to
   `wss://relay.example.com/ws`.
2. Start hosting on the host device and note its nine-digit peer ID.
3. From the viewer, enter that ID and the matching session password, then
   approve the request on the host unless unattended access is enabled.

For local development, run `cargo run -p relay`, use
`ws://127.0.0.1:8080/ws`, and start the app as described in
[Build and run](#build-and-run).

## Browser viewer

If you can't install anything on the computer you're sitting at, open
<https://opendesk.treeot.dev>, enter the other computer's ID and
password (and its 2FA code, if it asks for one), and connect. The other
computer still needs the desktop app to host.

The page can only view and control. It always goes through the relay, since
browsers can't take part in the direct UDP path, and it has no audio, file
transfer, clipboard sync or recording yet. It needs a browser with WebCodecs:
a recent Chrome, Edge, Firefox or Safari.

The crypto is the same Rust code as the desktop app (`crates/web-core`,
compiled to WebAssembly), so the relay still sees only sealed bytes. The
trade-off is where that code comes from: GitHub Pages serves the page fresh
on every visit, so you are trusting GitHub and this repository each time you
load it, not a binary you built or checked once. If that matters for your
setup, use the desktop app, or build `web/` yourself and host it.

To build it: `cd web && npm ci && npm run wasm && npm run build` (needs the
`wasm32-unknown-unknown` target and `wasm-bindgen-cli` 0.2.129). Set
`OPENDESK_DEFAULT_RELAY` at build time to choose the default relay.

## Why OpenDesk

- **The relay can't read your session.** TLS protects the wire, and an inner ChaCha20-Poly1305 layer encrypts payloads end to end. Whoever runs the relay sees only metadata.
- **Password-authenticated keys (SPAKE2).** Only someone with the session password derives the key, and a malicious relay cannot man-in-the-middle the exchange.
- **Self-hosted and cross-platform.** You run one relay binary. Host and viewer share one code path on macOS, Windows, and Linux.
- **Small enough to audit.** The relay parses only the control messages it needs for pairing and routing. Session payloads are opaque to it.

## Status

OpenDesk is under active development. Current state:

| Area | Status |
|------|--------|
| Relay over `wss://` :443 (blind pipe) | ✅ |
| P2P-first ICE transport with relay fallback | ✅ |
| End-to-end encryption (ChaCha20-Poly1305) | ✅ |
| Password-authenticated key exchange (SPAKE2) | ✅ |
| Clipboard sync (text-only, E2E encrypted) | ✅ |
| Screen capture + H.264 encode (macOS/Windows/Linux via `scap` + `openh264`) | ✅ |
| Remote input: mouse and keyboard (via `enigo`) | ✅ |
| Desktop UI / app shell (ID, connect, accept dialog, permissions) | ✅ |
| File transfer with hashing and consent UI | ✅ |
| Address book · unattended access · TOTP 2FA | ✅ |
| System audio (Opus, 48 kHz; mic fallback) · multi-monitor selection | ✅ |

> [!NOTE]
> Screen capture is verified on macOS. The Windows and Linux code is complete but not yet runtime-tested. H.264 uses hardware encoding through VideoToolbox on macOS and Media Foundation on Windows (NVENC / QSV / AMF; not yet runtime-tested). Linux uses software openh264, which is also the fallback on the other platforms. Release bundles are unsigned.

## Architecture

```
┌──────────────────┐        wss://relay:443        ┌──────────────────┐
│   HOST            │◄─────── outbound only ───────►│      RELAY        │
│  (desktop app)    │      (looks like HTTPS)        │  (Rust / axum)    │
│  scap capture     │                                │  blind pipe:      │
│  H.264 (HW/sw)    │──── sealed frames ────────────►│  no keys,         │
│  enigo inject     │◄─── sealed input ─────────────│  no plaintext,    │
│  SPAKE2 + AEAD    │                                │  routes by ID     │
└──────────────────┘                                └────────┬─────────┘
                                                              │ forwards
                                                              ▼ opaque bytes
                                                     ┌──────────────────┐
                                                     │      VIEWER       │
                                                     │  (Tauri + TS)     │
                                                     │  WebCodecs decode │
                                                     │  crypto in Rust   │
                                                     │  backend          │
                                                     └──────────────────┘
```

- **`crates/protocol`**: shared wire types. Frame tags, control messages, ChaCha20-Poly1305 seal/open, SPAKE2 handshake, HKDF directional keys, input event codec.
- **`crates/relay`**: axum WebSocket server. Pairs a host and viewer by ID and forwards sealed bytes it cannot read.
- **`crates/host-core`**: session setup and transport, plus the host side: captures the screen, encodes H.264, seals and sends frames, and injects remote input.
- **`viewer/`**: Tauri desktop app that both hosts and views. The Rust backend
  runs crypto and transport (including ICE/direct UDP and relay fallback); the
  WebView only decodes with WebCodecs and captures input. Direct P2P works in
  both directions: the host sends video and the viewer sends input over the
  selected transport.
- **`crates/web-core`** and **`web/`**: the browser viewer. web-core is the
  viewer's session logic built on `protocol`, compiled to WebAssembly; `web/`
  is the page around it.

## Build and run

Requires a recent Rust toolchain and Node.js.

```bash
# 1. relay (bind :8080; put Traefik/Caddy in front for wss:443)
cargo run -p relay

# 2. desktop app (hosts and views; run a second instance to test locally)
cd viewer && npm install && npm run tauri dev
```

To run two instances on one machine, see [`docs/TESTING.md`](docs/TESTING.md).

### Platform permissions (host)

- **macOS:** grant **Screen Recording** (to share the screen) and **Accessibility** (to accept remote control) in *System Settings → Privacy & Security*. macOS 12.3+.
- **Windows:** Windows 10 1803+ (`Windows.Graphics.Capture`, `SendInput`).
- **Linux:** PipeWire + `xdg-desktop-portal`; input via uinput/XTest (Wayland injection is limited).

## Security model

- **Two layers.** TLS protects the connection to the reverse proxy. A separate inner AEAD layer (ChaCha20-Poly1305) encrypts end to end. TLS alone would let the relay operator read traffic; the inner layer prevents that.
- **Key agreement.** SPAKE2, a password-authenticated exchange, derives a fresh key for each session. With a wrong password, key confirmation fails and nothing is decrypted.
- **Blind relay.** The relay holds no keys and parses only the routing control messages. Frames and input are opaque to it.
- **Native client.** The viewer is a native Tauri app you can build and verify yourself, unlike a JavaScript client that a server can change on every page load. The [browser viewer](#browser-viewer) is exactly that kind of client; use it knowing the trade-off.

### Checking the selected transport

The host logs `INFO transport selected: direct` after successful ICE
nomination, or `INFO transport selected: relay (ICE timeout)` when it falls
back. The order is pairing → SPAKE2/key confirmation → ICE signaling over
`Control::Ice` → transport selection → media, so media never starts before key
confirmation.

### Manual end-to-end check

Run two app instances, connect them by device ID and session password, and
allow the incoming request on the host. Confirm that the remote screen renders
and that mouse and keyboard input control the host. Check both logs for
`transport selected: direct` or `transport selected: relay`.

### File transfer

You can send files during a session. File messages always use the reliable
relay `Tag::Data` path, whether media goes Direct or through the relay. They
are sealed end to end with a separate HKDF-derived Data key, so Data and media
never reuse AEAD nonces. The receiver asks for consent, streams into a
sanitized filename in the OS Downloads directory, verifies the SHA-256 before
finalizing, and reserves unique paths for concurrent transfers.

### Clipboard sync

Clipboard sync sends text-only clipboard contents, end-to-end encrypted, over
the session Data channel. A per-session toggle controls it (on by default).
Clipboard text is capped at 1 MB (`MAX_CLIPBOARD`), and a value received from
the peer is not echoed back after it is applied locally.

## Roadmap

- [x] Remote input injection
- [x] Desktop app shell: ID display, connect screen, incoming-request accept dialog, permission onboarding, session toolbar
- [x] Address book, unattended access, 2FA
- [x] Clipboard sync, multi-monitor
- [x] System audio (macOS 14.6+, Windows)
- [x] System audio on Linux (PulseAudio / PipeWire monitor)
- [x] Hardware H.264 on macOS (VideoToolbox)
- [x] Hardware H.264 on Windows (Media Foundation: NVENC / QSV / AMF)
- [ ] Hardware H.264 on Linux (VAAPI)

## Contributing

Issues and PRs are welcome. The codebase prefers small, focused crates and test-driven changes. See [`CONTRIBUTING.md`](CONTRIBUTING.md).

## License

[GNU Affero General Public License v3.0](LICENSE) (`AGPL-3.0-only`).

You can use, modify, and self-host OpenDesk freely. If you distribute a
modified version, or run one that others use over a network (for example a
modified relay), you must make your source available under the same license.
