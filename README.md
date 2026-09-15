<div align="center">

# OpenDesk

**A self-hosted, zero-knowledge, open-source remote desktop.**
An open-source alternative to [AnyDesk](https://anydesk.com) and [TeamViewer](https://teamviewer.com), built in Rust.

Runs over `wss://` on port **443** · **End-to-end encrypted** · The relay is a **blind pipe** that never sees your screen, keystrokes, or keys.

</div>

---

## What is OpenDesk?

OpenDesk lets one device view and control another over the internet, through a relay server you host yourself. Frames and input are sealed on each endpoint with a key only the two peers hold, derived from a session password. The relay forwards bytes it cannot decrypt, so the encryption is end-to-end rather than only in transit.

Each peer opens one outbound `wss://` connection on port 443. After SPAKE2 key
confirmation, OpenDesk attempts a direct UDP path using ICE; if that fails, the
same connection remains the encrypted relay fallback. No inbound ports or port
forwarding are needed. The direct attempt requires outbound UDP; strict-UDP
networks fall back to relay automatically.

ICE uses Google Public STUN by default (`stun:stun.l.google.com:19302`) to find
server-reflexive addresses. A self-hosted STUN service can be substituted in
the host-core `AgentConfig.urls` configuration. STUN helps discover addresses;
it is not a media relay and does not receive desktop contents.

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
| Screen capture + H.264 encode (macOS/Windows/Linux via `scap` + `openh264`) | ✅ |
| Remote input: mouse and keyboard (via `enigo`) | 🚧 in progress |
| Desktop UI / app shell (ID, connect, accept dialog, permissions) | 🗺️ planned |
| Address book · unattended access · file transfer · 2FA | 🗺️ planned |

> **PoC note:** connection ID and password are currently hard-coded defaults; screen capture is verified on macOS (Windows/Linux code-complete, not yet runtime-tested). Software H.264 for now; hardware encoders slot in behind a trait later.

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
- **`viewer/`**: Tauri desktop app. All crypto runs in the Rust backend; the WebView only renders (WebCodecs to canvas).

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

## Roadmap

- [ ] Remote input injection (in progress)
- [ ] Desktop app shell: ID display, connect screen, incoming-request accept dialog, permission onboarding, session toolbar
- [ ] Hardware-accelerated encoders (VideoToolbox / NVENC / QSV)
- [ ] Address book, unattended access, 2FA
- [ ] File transfer, clipboard sync, multi-monitor
- [ ] Audio

## Contributing

Issues and PRs welcome. The codebase favors small, focused crates and test-driven changes.

## License

TBD. Intended to be an open-source license (AGPL/GPL or MIT/Apache), to be finalized.
