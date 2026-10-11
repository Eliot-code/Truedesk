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
| 2 | Accept | Viewer shows the host screen within a few seconds |
| 3 | Move mouse, click, type (incl. symbols like `@#{}`) | Host reacts; characters match |
| 4 | Copy text on each side | Clipboard syncs both ways (toggle on) |
| 5 | Send a file both ways | Consent prompt, file arrives, size matches |
| 6 | Switch monitor (if host has 2+) | Stream switches |
| 7 | Play a video/song on the host | Viewer hears it; mute works. macOS asks for System Audio Recording once. On Linux or macOS < 14.6, speak near the host mic instead |
| 8 | Wrong password | Rejected, viewer does **not** retry |
| 9 | Quit host app mid-session | Viewer shows reconnecting, gives up after ~30 s |
| 10 | Kill host network for 10 s, restore | Viewer reconnects; host re-registers |
| 11 | Unattended on (+ 2FA) | Connects without accept; wrong TOTP rejected |
| 12 | Record session, stop | `.webm` saved and plays |

macOS asks for **Screen Recording** and **Accessibility** permission the first
time; restart the app after granting them. When you report an issue, include
the OS versions, the number of any failing step, and the app log:

- macOS: `~/Library/Logs/com.opendesk.app/`
- Windows: `%LOCALAPPDATA%\com.opendesk.app\logs\`
- Linux: `~/.local/share/com.opendesk.app/logs/`

The relay logs to stderr; set `RUST_LOG=debug` for more detail.
