<div align="center">

<img src="viewer/src-tauri/icons/128x128@2x.png" width="96" alt="OpenDesk icon">

# OpenDesk

**Open-source, self-hosted remote desktop with end-to-end encryption.**<br>
An alternative to TeamViewer, AnyDesk and Parsec for Windows, macOS, Linux and the browser.

[![CI](https://github.com/treeot/opendesk/actions/workflows/ci.yml/badge.svg)](https://github.com/treeot/opendesk/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/treeot/opendesk?include_prereleases&label=release)](https://github.com/treeot/opendesk/releases)
[![License: AGPL-3.0](https://img.shields.io/badge/license-AGPL--3.0-blue)](LICENSE)
[![Written in Rust](https://img.shields.io/badge/written%20in-Rust-orange)](https://www.rust-lang.org)

[Website](https://opendesk.treeot.dev) ·
[Docs](https://opendesk.treeot.dev/docs/) ·
[Download](https://github.com/treeot/opendesk/releases) ·
[Browser viewer](https://opendesk.treeot.dev/connect/) ·
[Self-hosting](docs/DEPLOYMENT.md)

</div>

---

OpenDesk lets you see and control another computer through a relay you run
yourself. Screen, input and sound are encrypted with a key only the two
computers hold, so the relay passes along bytes it can't read. Both sides
connect out over `wss://` on port 443: no port forwarding, and a direct UDP
path is used whenever the network allows it.

> [!CAUTION]
> OpenDesk is for computers you own or have permission to control. Using it
> to access someone else's computer without their consent is not supported
> and may be illegal.

## Features

| | |
|---|---|
| 🔒 **End-to-end encrypted** | SPAKE2 password key exchange, ChaCha20-Poly1305 for every frame and keystroke |
| 🛰️ **Self-hosted relay** | One small Rust binary or Docker image; it never sees your screen or keys |
| ⚡ **Direct when possible** | Peer-to-peer UDP with automatic fallback to the relay; latency badge in the toolbar |
| 🖥️ **Hardware H.264** | VideoToolbox on macOS, Media Foundation on Windows, openh264 elsewhere |
| 🔊 **System sound** | macOS 14.6+, Windows, Linux (PulseAudio / PipeWire) |
| 📁 **Files and clipboard** | Encrypted file transfer with consent and hash check; text clipboard sync |
| 🪟 **Multi-monitor** | Pick a monitor, or add a virtual display on macOS and Windows |
| 🌙 **Unattended access** | Stored password, optional 2FA code, start at sign-in with a tray icon |
| 🌐 **Browser viewer** | Connect from any modern browser at [/connect](https://opendesk.treeot.dev/connect/) |
| 🔄 **Auto-update** | Signed updates checked from GitHub Releases |

## Quickstart

1. **Run a relay** on any server with Docker and a DNS name:
   ```bash
   DOMAIN=relay.example.com docker compose up -d
   ```
   HTTPS is automatic. See [`docs/DEPLOYMENT.md`](docs/DEPLOYMENT.md).
2. **Install the app** on both computers from
   [Releases](https://github.com/treeot/opendesk/releases) and set the relay to
   `wss://relay.example.com/ws`.
3. **Host:** click **Start hosting** and note the nine-digit ID.
4. **Viewer:** enter that ID and the session password, then accept on the host
   (or set up unattended access to skip that).

The [getting started guide](https://opendesk.treeot.dev/docs/getting-started/)
covers each step in more detail.

## Platform support

| | Host | Viewer | Notes |
|---|:---:|:---:|---|
| **macOS** 12.3+ | ✅ | ✅ | Most tested. Needs Screen Recording and Accessibility |
| **Windows** 10 1803+ | ✅ | ✅ | Less runtime-tested than macOS |
| **Linux** | ✅ | ✅ | PipeWire + xdg-desktop-portal; Wayland input is limited |
| **Browser** | — | ✅ | Relay only; no audio, files, clipboard or recording yet |

Release builds are not code-signed. The
[install guide](https://opendesk.treeot.dev/docs/getting-started/) shows how to
open them.

## How it works

```
 HOST                         RELAY                         VIEWER
 capture → H.264 → seal  ──►  forwards sealed bytes  ──►  open → decode
 inject input ◄── open   ◄──  (no keys, no plaintext) ◄──  seal ← input
          └───────────── direct UDP when reachable ─────────────┘
```

| Crate | What it does |
|---|---|
| [`crates/protocol`](crates/protocol) | Wire types, SPAKE2 handshake, sealing and key derivation |
| [`crates/relay`](crates/relay) | axum WebSocket server that pairs peers by ID and forwards sealed bytes |
| [`crates/host-core`](crates/host-core) | Transport (ICE, relay fallback), capture, encoding, input injection |
| [`viewer/`](viewer) | Tauri + React desktop app that both hosts and views |
| [`crates/web-core`](crates/web-core) · [`web/`](web) | The browser viewer, compiled to WebAssembly |
| [`site/`](site) | The website and docs (Astro + Starlight, Markdown pages) |

More detail: [architecture](docs/ARCHITECTURE.md) ·
[security model](docs/SECURITY.md) ·
[browser viewer trade-offs](https://opendesk.treeot.dev/docs/browser-viewer/).

## Build from source

Needs a recent Rust toolchain and Node.js.

```bash
cargo run -p relay                              # relay on :8080 (ws://127.0.0.1:8080/ws)
cd viewer && npm install && npm run tauri dev   # desktop app
```

<details>
<summary>Browser viewer and website</summary>

```bash
cd web && npm ci && npm run wasm && npm run build   # needs wasm32-unknown-unknown + wasm-bindgen-cli 0.2.129
cd site && npm ci && npm run build                  # run `npm run wasm` in web/ first
```

Set `OPENDESK_DEFAULT_RELAY` at build time to change the browser viewer's
default relay. Docs pages are Markdown files in `site/src/content/docs/docs/`.

</details>

To run two instances on one machine, see [`docs/TESTING.md`](docs/TESTING.md).

## Roadmap

- [x] Direct UDP with relay fallback, end-to-end encryption
- [x] Hardware H.264 on macOS and Windows
- [x] Files, clipboard, audio, recording, multi-monitor
- [x] Unattended access with 2FA, start at sign-in
- [x] Browser viewer
- [x] Virtual displays on macOS and Windows
- [ ] Hardware H.264 on Linux (VAAPI)
- [ ] Text chat and view-only sessions

## Contributing

Issues and pull requests are welcome. See [`CONTRIBUTING.md`](CONTRIBUTING.md).

## License

[AGPL-3.0-only](LICENSE). You can use, change and self-host OpenDesk freely.
If you distribute a modified version, or run one others use over a network
(a modified relay, for example), you must publish its source under the same
licence.

<details>
<summary>Third-party software: Amyuni virtual display driver (Windows)</summary>

On Windows, OpenDesk can download and install Amyuni Technologies'
`usbmmidd_v2` virtual display driver for you (see
[docs/SECURITY.md](docs/SECURITY.md#virtual-displays-on-windows)). It is
not part of OpenDesk and comes under its own licence:

```text
Copyright 2014-2021 Amyuni Technologies Inc.
https://www.amyuni.com

This software is provided 'as-is', without any express or implied warranty. In no event will the authors be held liable for any damages arising from the use of this software.
By using this software, you accept to be prompted with an advertisement page which is not always under the control of the author. A fully commercial version is available for users who do not wisth to see those advertisements.

Permission is granted to anyone to use this software for any purpose subject to the following restrictions:

1. The origin of this software must not be misrepresented; you must not claim that you wrote the original software.
   If you use this software in a product, an acknowledgment in the product documentation would be required.
2. Altered versions must be plainly marked as such, and must not be misrepresented as being the original software.
3. This notice may not be removed or altered from any distribution.
```

</details>
