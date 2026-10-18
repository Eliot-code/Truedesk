# Truedesk

The Truedesk desktop app, built with Tauri. One app both hosts incoming
remote-control sessions and connects to another Truedesk instance as a viewer.
Cryptography, session keys, and the transport stay in the Rust backend. Both
the host and the viewer gather and exchange ICE candidates and try direct UDP,
falling back to the encrypted relay. Direct P2P carries traffic in both
directions. The WebView only decodes video with WebCodecs and captures user
input.

## Run

Start the relay, then run the app:

```bash
cargo run -p relay
cd viewer
npm install
npm run tauri dev
```

Each app generates and saves its own 9-digit device ID. The backend generates
a fresh one-time session password, which you can rotate from Home. No IDs or
passwords are hardcoded.

## Permissions

To host a controllable session on macOS, grant Truedesk both permissions in
System Settings → Privacy & Security:

- Screen Recording, to capture the display.
- Accessibility, to inject remote mouse and keyboard input.

The Permissions screen shows the current status and opens the matching
settings pane. The host must Allow an incoming request before capture or
input starts.

## Manual end-to-end check

1. Run two app instances, or run one instance on each of two machines, with
   both connected to the same relay.
2. On the host instance, note its generated ID and session password.
3. On the other instance, enter that ID and password and select Connect.
4. On the host, verify the requester and select Allow.
5. Confirm that the viewer sees the host screen and that mouse/keyboard input
   is delivered when the required permissions are granted.
6. Check the logs for `transport selected: direct` or `transport selected:
   relay`, then select Disconnect and verify both instances return to Home.
