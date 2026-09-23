# OpenDesk architecture

OpenDesk is a Rust remote-desktop system with a Tauri desktop client and a
small self-hosted WebSocket relay. A session has two endpoints:

- **Host:** captures the desktop, encodes video, captures microphone audio,
  and applies remote input.
- **Viewer:** displays the host desktop, sends mouse/keyboard input, and
  provides session controls.

## Components

### `crates/protocol`

The shared wire protocol defines frame tags, control messages, SPAKE2 key
exchange and confirmation, ChaCha20-Poly1305 sealing/opening, HKDF key
derivation, input events, clipboard messages, file-transfer messages, and
audio frames.

### `crates/relay`

The relay is an Axum WebSocket server. It exposes `/ws`, pairs a host and
viewer by peer ID, assigns a session ID, and forwards binary frames between
the two sockets. It listens on one HTTP port, taken from `PORT` and defaulting
to `8080`. TLS is normally terminated by the reverse proxy or hosting
platform in front of it.

The relay parses the initial register/connect control messages and the pairing
and ICE-signaling messages needed to connect peers. It does not implement
desktop protocol semantics or decrypt sealed session payloads.

### `crates/host-core`

`host-core` owns session establishment and transport selection. It performs
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
   through the same relay WebSocket. This is signaling, not desktop content.
5. ICE attempts a direct UDP path. If nomination succeeds, media and input use
   that direct path. If ICE times out, the session uses the already-open relay
   WebSocket as the fallback. No inbound listening port or port forwarding is
   required.

All signaling and relay fallback traffic uses the relay's single HTTP port;
with a TLS reverse proxy, clients use `wss://` on the public HTTPS port.
Google Public STUN (`stun:stun.l.google.com:19302`) is the default for ICE
server-reflexive candidates. The host-core agent configuration can substitute
a self-hosted STUN service.

## Channel model

Each channel has independent directional keys derived from the SPAKE2 shared
secret. The relay transport carries tagged frames; direct UDP carries the
same sealed protocol frames.

| Function | Protocol path | Key space |
| --- | --- | --- |
| Screen video, microphone audio, and remote input | `Video`, `Audio`, and `Input` transport frames | Directional media keys |
| File transfer and text clipboard sync | Reliable `Data` channel | Directional data keys |
| Monitor list and monitor selection | Sealed `Control` payloads | Directional control keys |

The `Kex` frames, initial pairing controls, and ICE signaling are setup
messages rather than application payloads. The data channel is ordered and
reliable; video, audio, and input use the media path and may be dropped under
backpressure to keep the session responsive.

