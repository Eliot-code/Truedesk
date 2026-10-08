# Relay deployment

The relay is one stateless HTTP service: a WebSocket endpoint at `/ws` and a
health check at `/health`. It does not terminate TLS itself; the Compose setup
below puts Caddy in front for automatic HTTPS.

## Requirements

- a server reachable by the clients (any VPS; amd64 or arm64);
- a DNS name pointing at it, such as `relay.example.com`; and
- ports 80 and 443 open (80 is only used for the certificate challenge).

Clients need no open ports. Direct UDP is attempted between clients; the
relay fallback and all signaling share the one HTTPS port.

## Docker Compose (recommended)

The repo's `compose.yaml` runs the relay behind Caddy, which gets and renews a
Let's Encrypt certificate automatically.

```sh
git clone https://github.com/treeot/opendesk.git
cd opendesk
DOMAIN=relay.example.com docker compose up -d
```

Or copy `.env.example` to `.env`, set `DOMAIN`, and run `docker compose up -d`.
Check it with `curl https://relay.example.com/health`, then set each client's
relay to:

```text
wss://relay.example.com/ws
```

Update with `git pull && docker compose up -d --build`. Logs:
`docker compose logs -f relay` (set `RUST_LOG=debug` in `.env` for more).

## Prebuilt image

CI publishes `ghcr.io/treeot/opendesk-relay` (`latest` and version tags from
releases, `edge` from `main`). Run it behind your own TLS proxy:

```sh
docker run -d --restart unless-stopped -p 8080:8080 ghcr.io/treeot/opendesk-relay:latest
```

Forward `wss://` on 443 to the container's port 8080; WebSocket upgrades pass
through unchanged.

## Coolify

1. Create an application from this repository, build pack **Dockerfile**,
   path `crates/relay/Dockerfile`, build context the repository root.
2. Set the domain (e.g. `relay.example.com`). Coolify provisions TLS and
   injects `PORT`, which the relay binds automatically (default 8080).
3. Deploy, then point clients at `wss://relay.example.com/ws`.

## Clients

In the app's home screen, **Relay regions** takes one WebSocket URL per line,
including `/ws`; clients try them in order. The default development value is
`ws://127.0.0.1:8080/ws` (`cargo run -p relay`).

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


## Transport security: use `wss://` for remote relays

Session payloads are end-to-end encrypted regardless of transport, so the relay
never sees plaintext. However, a plain `ws://` relay exposes connection
**metadata** (IP addresses, timing, message sizes, the 9-digit IDs being paired)
to any on-path observer. For any relay reachable over the network, always use a
`wss://` URL (TLS). Coolify (or your reverse proxy) terminates TLS in front of
the relay's single HTTP port, so point clients at `wss://your-relay-host` while
the relay itself listens on plain HTTP behind the proxy. Plain `ws://` is
intended only for `127.0.0.1`/LAN development.
