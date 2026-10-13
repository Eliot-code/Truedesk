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
| 13 | With unattended on, tick **Start OpenDesk when I sign in**, then sign out and back in | No window opens; a tray icon appears and the viewer can connect without accept. Closing the window keeps hosting; tray **Quit** stops it |
| 14 | Linux only: in a session with no keyring running (no GNOME Keyring or KWallet), save unattended access | Saving fails with "couldn't … system keyring" and a hint; if it was already on, the card says it isn't working and connections need accept |
| 15 | Copy the host's `config.json` to the other machine, restart the app there | It shows a different ID from the host; both stay online |
| 16 | On the host, **Get a new ID**, confirm | The ID changes and the host is online under it; the old ID no longer connects |
| 17 | Mac host: in the viewer's monitor menu pick **Add display…** | The host shows "The viewer added a virtual display"; the viewer switches to a new display about the size of its window, and clicks land where they should on it |
| 18 | Pick **Add display…** again, then once more | A second display is added; the third try says the session can't add more |
| 19 | With a virtual display shown, click **Remove**, then end the session with another still added | The stream goes back to the main display and the removed one disappears; after Disconnect the host has no extra displays left (System Settings → Displays) |
| 20 | Untick **Let viewers add virtual displays** on the host, reconnect | The viewer's monitor menu has no **Add display…** |
| 21 | Headless host (no monitor or dummy plug), then connect. Mac: with the setting on, and again with it off | Setting on: the viewer sees a 1920x1080 virtual display. Off, or a Windows host: the viewer stops with "That computer has no screen connected…" instead of the Screen Recording message |

`cargo run -p host-core --example virtual_display` on a Mac adds a 1280x720
display and a 2560x1440 HiDPI one for two seconds each and removes them.

macOS asks for **Screen Recording** and **Accessibility** permission the first
time; restart the app after granting them. When you report an issue, include
the OS versions, the number of any failing step, and the app log:

- macOS: `~/Library/Logs/com.opendesk.app/`
- Windows: `%LOCALAPPDATA%\com.opendesk.app\logs\`
- Linux: `~/.local/share/com.opendesk.app/logs/`

The relay logs to stderr; set `RUST_LOG=debug` for more detail.
