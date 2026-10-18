# Truedesk architecture

Truedesk is a Rust remote desktop with a Tauri desktop client and a small
self-hosted WebSocket relay. A session has two endpoints:

- **Host:** captures the desktop, encodes video, captures system audio
  (never the microphone), and applies remote input.
- **Viewer:** displays the host desktop, sends mouse/keyboard input, and
  provides session controls.

## Components

### `crates/protocol`

The shared wire protocol: frame tags, control messages, SPAKE2 key
exchange and confirmation, ChaCha20-Poly1305 sealing/opening, HKDF key
derivation, input events, clipboard messages, file-transfer messages, and
audio frames.

### `crates/relay`

The relay is an Axum WebSocket server. It exposes `/ws`, pairs a host and
viewer by peer ID, assigns a session ID, and forwards binary frames between
the two sockets. It listens on one HTTP port, taken from `PORT` and defaulting
to `8080`. A reverse proxy or hosting platform in front of it normally
terminates TLS.

The relay parses the initial register/connect control messages and the pairing
and ICE-signaling messages needed to connect peers. It does not
interpret desktop protocol messages or decrypt sealed session payloads.

### `crates/host-core`

`host-core` handles session setup and transport selection. It completes
SPAKE2 key confirmation before starting capture or input injection, gathers
ICE candidates, attempts a direct UDP connection, and keeps the relay-backed
transport as the fallback. Its reliable WebSocket channel separates ordered
data from lossy media/input delivery.

The host role controls ICE nomination. The viewer role accepts the nominated
connection.

### `viewer/`

The viewer is a Tauri application. Its Rust backend uses `host-core` for
relay connection, cryptography, and transport; the WebView renders the UI,
decodes video frames, captures input, displays monitor/audio state, and asks
the user for file-transfer consent. Local configuration includes the relay
URL, peer ID, recent IDs, contacts, and optional unattended-access settings.

## Connection model

1. Both endpoints open an outbound WebSocket to the relay's `/ws` endpoint.
2. The host registers its peer ID. The viewer sends a connect request for
   that ID. The relay pairs the sockets and sends each endpoint a session ID.
3. The endpoints exchange SPAKE2 messages and key-confirmation MACs. The
   password is not sent to the relay.
4. After confirmation, the endpoints exchange ICE credentials and candidates
   through the same relay WebSocket. These are signaling messages, not desktop content.
5. ICE attempts a direct UDP path. If nomination succeeds, media and input use
   that direct path. If ICE times out, the session uses the already-open relay
   WebSocket as the fallback. No inbound listening port or port forwarding is
   required.
6. The viewer toolbar shows which path is in use and its round-trip time.
   On the direct path the host measures it from UDP feedback and passes it
   on; on the relay the viewer sends sealed pings that the host answers.
   Relayed hosts from beta.5 and earlier don't answer, so the badge shows
   the path with no time.

All signaling and relay fallback traffic uses the relay's single HTTP port;
with a TLS reverse proxy, clients use `wss://` on the public HTTPS port.
Google Public STUN (`stun:stun.l.google.com:19302`) is the default for ICE
server-reflexive candidates. You can substitute a self-hosted STUN service in
the host-core agent configuration.

## Channel model

Each channel has independent directional keys derived from the SPAKE2 shared
secret. The relay transport carries tagged frames; direct UDP carries the
same sealed protocol frames.

| Function | Protocol path | Key space |
| --- | --- | --- |
| Screen video, audio, and remote input | `Video`, `Audio`, and `Input` transport frames | Directional media keys |
| File transfer, text clipboard sync, chat, remote browsing and TCP tunnels | Reliable `Data` channel | Directional data keys |
| Monitor list and monitor selection | Sealed `Control` payloads | Directional control keys |

The `Kex` frames, initial pairing controls, and ICE signaling are setup
messages, not application payloads. The data channel is ordered and
reliable; video, audio, and input use the media path and may be dropped under
backpressure to keep the session responsive.
