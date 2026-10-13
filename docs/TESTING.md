# Manual testing

Automated tests cover the protocol, relay, and host/viewer sessions end to end
(`cargo test --workspace`). Screen capture, input injection, and audio need a
real desktop session, so test them by hand before a release.

## One machine (two instances)

Debug builds read `OPENDESK_DATA_DIR`, so a second instance gets its own
identity and config.

1. Relay: `cargo run -p relay` (listens on `ws://127.0.0.1:8080/ws`, the
   default relay URL).
2. Instance A (host): `cd viewer && npm run tauri dev`. It starts hosting on
   launch; note the 9-digit ID and session password on the home screen.
3. Instance B (viewer), in another terminal, while A's dev server is running:
   `OPENDESK_DATA_DIR=/tmp/opendesk-b viewer/src-tauri/target/debug/viewer`
4. In B, enter A's ID and password, then accept on A.

On one machine you are viewing your own screen, so the stream shows itself
recursively. That's expected.

## Two machines

Run the relay where both machines can reach it, then point both apps at it
(home screen → **Relay regions**, e.g. `ws://<relay-ip>:8080/ws` on a LAN or your
`wss://` deployment).

| # | Step | Expected |
|---|------|----------|
| 1 | Open the app on both. Viewer: enter host ID + password, Connect | Host shows accept dialog |
| 2 | Accept | Viewer shows the host screen within a few seconds; the toolbar shows Direct or Relayed and a round-trip time |
| 3 | Move mouse, click, type (incl. symbols like `@#{}`) | Host reacts; characters match |
| 4 | Turn clipboard sync on in the toolbar (it starts off), copy text on each side | Clipboard syncs both ways |
| 5 | Send a file both ways | Consent prompt, file arrives, size matches |
| 6 | Switch monitor (if host has 2+) | Stream switches |
| 7 | Play a video or song on the host | Viewer hears it; mute works. macOS asks for System Audio Recording once. macOS < 14.6 and Linux without PulseAudio/PipeWire send no audio |
| 8 | Wrong password | Rejected, viewer does **not** retry |
| 9 | Click Disconnect on the host | Viewer shows "The host ended the session." and doesn't reconnect |
| 10 | Kill host network for 10 s, restore | Viewer reconnects; host re-registers. With the network gone for good, both sides notice within ~45 s |
| 11 | Unattended on, then 2FA (scan the QR, confirm a code) | Connects without accept; wrong code rejected. A device on the Known devices list still gets the accept prompt |
| 12 | Record session, stop | "Recording saved to …" appears; the `.webm` plays |

macOS asks for **Screen Recording** and **Accessibility** permission the first
time; restart the app after granting them. When you report an issue, include
the OS versions, the number of any failing step, and the app log:

- macOS: `~/Library/Logs/com.opendesk.app/`
- Windows: `%LOCALAPPDATA%\com.opendesk.app\logs\`
- Linux: `~/.local/share/com.opendesk.app/logs/`

The relay logs to stderr; set `RUST_LOG=debug` for more detail.
