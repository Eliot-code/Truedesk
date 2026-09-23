# Relay deployment

This guide deploys the OpenDesk relay as one HTTP service. The relay itself
serves a WebSocket endpoint at `/ws`; it does not terminate TLS. Put it behind
Coolify, Caddy, Traefik, or another reverse proxy that provides HTTPS and
WebSocket upgrades.

## Requirements

- a host reachable by the clients;
- a public DNS name, such as `relay.example.com`;
- TLS at the reverse proxy, so clients can use `wss://`; and
- one exposed application port.

No separate relay media port is required. Direct UDP is attempted between
clients, but the relay fallback and all signaling share the one HTTP port.

## Coolify / Docker deployment

The repository includes [`crates/relay/Dockerfile`](../crates/relay/Dockerfile).
Create a Coolify application from this repository and set its build context to
the repository root. Use that Dockerfile and configure the application to
listen on the container port `8080`.

Coolify normally injects `PORT`; the binary reads it and binds to
`0.0.0.0:$PORT`. If the platform does not inject it, set:

```text
PORT=8080
```

Expose only the platform's public HTTPS route. Configure the route for
WebSocket upgrades and point `relay.example.com` at it. Coolify should
terminate TLS and proxy the public `wss://relay.example.com/ws` request to the
container's `http://relay:8080/ws` endpoint.

## Minimal steps

1. Create the DNS record for the relay hostname.
2. Create a Coolify application from this repository and select
   `crates/relay/Dockerfile`.
3. Set the application port to `8080` (or use the value supplied through
   `PORT`).
4. Enable HTTPS for the hostname and confirm that WebSocket upgrades are
   allowed.
5. Deploy and test the endpoint with a WebSocket-capable client at
   `wss://relay.example.com/ws`.
6. In each OpenDesk client, set its relay URL to the complete WebSocket URL,
   including `/ws`, for example:

   ```text
   wss://relay.example.com/ws
   ```

   The viewer stores this setting locally and uses it for both hosting and
   connecting. The default development value is `ws://127.0.0.1:8080/ws`.
7. Start hosting on one device, then connect to its nine-digit peer ID from
   another device using the session password.

The viewer exposes the relay setting through its Tauri `set_relay` command;
the UI surface for changing that setting may vary while the application is in
development. A local development relay can be run with `cargo run -p relay`
and reached at `ws://127.0.0.1:8080/ws`.

## STUN

ICE uses Google Public STUN at `stun:stun.l.google.com:19302` by default to
discover server-reflexive addresses. STUN does not relay desktop contents.
Public STUN is the default because it requires no additional service and is
only used for candidate discovery; the encrypted payload still uses the
direct nominated UDP path or the OpenDesk relay fallback.

Deployments that require control of candidate discovery can substitute a
self-hosted STUN URL in the host-core `AgentConfig.urls` configuration. That
configuration is part of the client code, not a relay environment variable.

## Operations and security

Keep the relay current, restrict access to the host, and monitor connection
logs without assuming that relay logs contain session contents. The relay can
see routing and network metadata, but it has no session decryption keys. Use
`wss://` in production; plain `ws://` is appropriate only for trusted local
development or an already-protected network.

