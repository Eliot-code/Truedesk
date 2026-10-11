# Relay deployment

The relay is a single stateless HTTP service: a WebSocket endpoint at `/ws` and a
health check at `/health`. It does not terminate TLS itself; the Compose setup
below puts Caddy in front for automatic HTTPS.

## Requirements

- a server reachable by the clients (any VPS; amd64 or arm64);
- a DNS name pointing at it, such as `relay.example.com`; and
- ports 80 and 443 open (port 80 serves only the certificate challenge).

Clients need no open ports. Clients try direct UDP between themselves; the
relay fallback and all signaling share the one HTTPS port.

## Docker Compose (recommended)

The repo's `compose.yaml` runs the relay behind Caddy, which obtains and renews a
Let's Encrypt certificate.

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

1. Create an application from this repository with build pack
   **Dockerfile**, **Base Directory** `/` (the repo root; the build needs the
   whole Cargo workspace), **Dockerfile Location** `/crates/relay/Dockerfile`,
   and **Ports Exposes** `8080`. Putting the Dockerfile path in Base Directory
   fails with `mkdir: can't create directory ... File exists`.
2. Set the domain (e.g. `relay.example.com`). Coolify provisions TLS and
   injects `PORT`, which the relay binds automatically (default 8080).
3. Deploy, then point clients at `wss://relay.example.com/ws`.

## Clients

In the app's home screen, **Relay regions** takes one WebSocket URL per line,
including `/ws`; clients try them in order. The default development value is
`ws://127.0.0.1:8080/ws` (`cargo run -p relay`).

## STUN

ICE uses Google Public STUN at `stun:stun.l.google.com:19302` by default to
discover server-reflexive addresses. It's the default because it needs no
extra service and only takes part in candidate discovery. STUN never relays
desktop contents: the encrypted payload still goes over the nominated direct
UDP path or the OpenDesk relay fallback.

To control candidate discovery yourself, substitute a self-hosted STUN URL in
the host-core `AgentConfig.urls` configuration. That setting lives in the
client code, not in a relay environment variable.

## Operations and security

Keep the relay up to date, restrict access to the server, and monitor the
connection logs. Don't expect session contents in those logs: the relay sees
routing and network metadata, but it has no session decryption keys.

### Use `wss://` for remote relays

Session payloads are end-to-end encrypted on any transport, so the relay never
sees plaintext. A plain `ws://` relay still exposes connection metadata (IP
addresses, timing, message sizes, the 9-digit IDs being paired) to anyone on
the network path. For a relay reachable over the network, use a `wss://` URL
(TLS). Coolify or your reverse proxy terminates TLS in front of the relay's
single HTTP port, so point clients at `wss://your-relay-host` while the relay
itself listens on plain HTTP behind the proxy. Use plain `ws://` only for
`127.0.0.1`/LAN development or an already-protected network.
