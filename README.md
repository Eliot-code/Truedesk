<div align="center">

# OpenDesk

**A self-hosted, zero-knowledge, open-source remote desktop.**
An open-source alternative to [AnyDesk](https://anydesk.com) and [TeamViewer](https://teamviewer.com), built in Rust.

Runs over `wss://` on port **443** · **End-to-end encrypted** · The relay is a **blind pipe** that never sees your screen, keystrokes, or keys.

</div>

> [!CAUTION]
> **Misuse Disclaimer:**
> OpenDesk is for accessing computers you own or have explicit permission to
> control. The developers do not condone or support any unethical or illegal
> use of this software. Misuse, such as unauthorized access, control, or
> invasion of privacy, is strictly against our guidelines. The authors are not
> responsible for any misuse of the application.

---

## What is OpenDesk?

OpenDesk lets one device view and control another over the internet, through a relay server you host yourself. Frames and input are sealed on each endpoint with a key only the two peers hold, derived from a session password. The relay forwards bytes it cannot decrypt, so the encryption is end-to-end rather than only in transit.

Each peer opens one outbound `wss://` connection on port 443. After SPAKE2 key
confirmation, both the host and viewer run the Rust transport: they gather and
exchange ICE candidates and attempt a direct UDP path. If that fails, the same
connection remains the encrypted relay fallback. No inbound ports or port
forwarding are needed. The direct attempt requires outbound UDP; strict-UDP
networks fall back to relay automatically. The relay assigns one pairing
session ID and sends it in `Control::Paired` to both peers, so their ICE
candidate messages are routed to the same session.

ICE uses Google Public STUN by default (`stun:stun.l.google.com:19302`) to find
server-reflexive addresses. A self-hosted STUN service can be substituted in
the host-core `AgentConfig.urls` configuration. STUN helps discover addresses;
it is not a media relay and does not receive desktop contents.

## Features and quickstart

- End-to-end encrypted screen, input, and audio payloads
- End-to-end encrypted file transfer and text-only clipboard sync
- Optional unattended access with a stored password and optional TOTP two-factor code
- Local address book and recent peer IDs
- Multi-monitor discovery and monitor selection

Audio is the host's system sound on macOS 14.6+, Windows, and Linux (PulseAudio or PipeWire via pipewire-pulse), and its microphone elsewhere (older macOS, Linux without either server). Windows/Linux host support is still less mature than the macOS-tested path.

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
`ws://127.0.0.1:8080/ws`, and start the host/viewer development commands
described below.

## Why

- **Zero-knowledge by design.** Two crypto layers: TLS on the wire, and an inner ChaCha20-Poly1305 layer end-to-end. Whoever runs the relay sees nothing.
- **Password-authenticated (SPAKE2).** Only someone with your session password derives the key. A malicious relay cannot man-in-the-middle it.
- **Self-hosted and cross-platform.** One relay binary. Host and viewer target macOS, Windows, and Linux through one code path.
- **Small and auditable.** The relay parses one control message type. Everything else is opaque to it.

## Status

OpenDesk is under active development. What works today:

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

> **Note:** screen capture is verified on macOS (Windows/Linux code-complete, not yet runtime-tested). H.264 is hardware-encoded with VideoToolbox on macOS (software openh264 elsewhere, and as fallback). Release bundles are unsigned.

## Architecture

```
┌──────────────────┐        wss://relay:443        ┌──────────────────┐
│   HOST AGENT      │◄─────── outbound only ───────►│      RELAY        │
│  (Rust)           │      (looks like HTTPS)        │  (Rust / axum)    │
│  scap capture     │                                │  blind pipe:      │
│  openh264 encode  │──── sealed frames ────────────►│  no keys,         │
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
- **`crates/host-agent`**: captures the screen, encodes H.264, seals and sends. Receives and injects remote input.
- **`viewer/`**: Tauri desktop app. The Rust backend runs crypto and transport
  (including ICE/direct UDP and relay fallback); the WebView only decodes with
  WebCodecs and captures input. Direct P2P is bidirectional: the host sends
  video and the viewer sends input over the selected transport.

## Build & run

Requires a recent Rust toolchain and Node.js.

```bash
# 1. relay (bind :8080; put Traefik/Caddy in front for wss:443)
cargo run -p relay

# 2. host agent  (last arg is the session password; must match the viewer)
cargo run -p host-agent -- ws://127.0.0.1:8080/ws abc test

# 3. viewer (Tauri desktop app)
cd viewer && npm install && npm run tauri dev
```

### Platform permissions (host)

- **macOS:** grant **Screen Recording** (to share the screen) and **Accessibility** (to accept remote control) in *System Settings → Privacy & Security*. macOS 12.3+.
- **Windows:** Windows 10 1803+ (`Windows.Graphics.Capture`, `SendInput`).
- **Linux:** PipeWire + `xdg-desktop-portal`; input via uinput/XTest (Wayland injection is limited).

## Security model

- **Two layers.** TLS protects the wire to the reverse proxy. A separate inner AEAD layer (ChaCha20-Poly1305) does the end-to-end encryption. TLS alone is not zero-knowledge; the inner layer is what makes it so.
- **Key agreement.** A password-authenticated exchange (SPAKE2) derives a fresh per-session key. A wrong password fails key confirmation, and nothing is decrypted.
- **Blind relay.** The relay holds no keys and parses only the routing control message. Frames and input are opaque to it.
- **Trust the client.** The viewer is a native Tauri app with a verifiable binary, which a server-served JavaScript client cannot match.

### Checking the selected transport

The host logs `INFO transport selected: direct` after successful ICE
nomination, or `INFO transport selected: relay (ICE timeout)` when it falls
back. The ordering is pairing → SPAKE2/key confirmation → ICE signaling over
`Control::Ice` → transport selection → media, so media never starts before key
confirmation.

### Manual end-to-end check

Run two app instances, connect them by device ID and session password, and
allow the incoming request on the host. Confirm that the remote screen renders
and that mouse/keyboard input controls the host. Check both logs for
`transport selected: direct` or `transport selected: relay`.

### File transfer

The file-transfer feature is live during a session. File messages use the
reliable relay `Tag::Data` path, independently of whether media uses Direct or
relay transport, and are sealed end-to-end with a separate HKDF-derived Data
key (so Data and media never reuse AEAD nonces). The receiver requests consent,
streams into a sanitized filename in the OS Downloads directory, verifies the
SHA-256 before finalizing, and reserves unique paths for concurrent transfers.

Remaining deferrals are audio and multi-monitor support.

### Clipboard sync

Clipboard sync shares text-only clipboard contents end-to-end encrypted over the
session Data channel. It has a per-session toggle that is on by default, caps
clipboard text at 1 MB (`MAX_CLIPBOARD`), and suppresses echoes when a received
value is applied locally.

## Roadmap

- [x] Remote input injection
- [x] Desktop app shell: ID display, connect screen, incoming-request accept dialog, permission onboarding, session toolbar
- [x] Address book, unattended access, 2FA
- [x] Clipboard sync, multi-monitor
- [x] System audio (macOS 14.6+, Windows)
- [x] System audio on Linux (PulseAudio / PipeWire monitor)
- [x] Hardware H.264 on macOS (VideoToolbox)
- [ ] Hardware H.264 on Windows (Media Foundation: NVENC / QSV / AMF) and Linux (VAAPI)

## Contributing

Issues and PRs welcome. The codebase favors small, focused crates and test-driven changes.

## License

[GNU Affero General Public License v3.0](LICENSE) (`AGPL-3.0-only`).

You can use, modify, and self-host OpenDesk freely. If you distribute a
modified version, or run one that others use over a network (for example a
modified relay), you must make your source available under the same license.
