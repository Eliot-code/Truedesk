---
title: Self-hosting a relay
description: Run your own Truedesk relay with Docker Compose and Caddy, or the prebuilt image behind your own proxy, and point the apps at it.
---

The relay is one small service. It pairs a host and a viewer by ID and
forwards their encrypted traffic. It never has the keys, so it can't read
sessions, but it does see connection metadata (see the
[security model](/docs/security/#what-the-relay-can-see)). Running your own
means that metadata stays with you, and you aren't sharing the test relay's
limits with everyone else.

## What you need

- A server the computers can reach: any small VPS, amd64 or arm64.
- A DNS name pointing at it, such as `relay.example.com`.
- Ports **80** and **443** open. Port 80 only answers the certificate
  challenge.

The computers using Truedesk need no open ports. They connect out to the
relay on 443.

## Docker Compose (recommended)

The repository's `compose.yaml` runs the relay behind
[Caddy](https://caddyserver.com), which gets and renews a Let's Encrypt
certificate by itself.

```sh
git clone https://github.com/Eliot-code/truedesk.git
cd truedesk
DOMAIN=relay.example.com docker compose up -d
```

Instead of the variable you can copy `.env.example` to `.env` and set
`DOMAIN` there. Check that it's up:

```sh
curl https://relay.example.com/health
# {"status":"ok"}
```

- Update: `git pull && docker compose up -d --build`
- Logs: `docker compose logs -f relay` (set `RUST_LOG=debug` in `.env` for more)

The Caddy config is two lines: it serves `$DOMAIN` and passes everything,
WebSocket upgrades included, to the relay on port 8080.

## Prebuilt image

CI publishes `ghcr.io/eliot-code/truedesk-relay`: `latest` and version tags from
releases, `edge` from `main`. Run it behind your own TLS proxy:

```sh
docker run -d --restart unless-stopped -p 8080:8080 \
  -v relay_data:/data -e RELAY_STATE_FILE=/data/state.json \
  ghcr.io/eliot-code/truedesk-relay:latest
```

Forward `wss://` on port 443 to the container's port 8080. The relay doesn't
do TLS itself.

[`docs/DEPLOYMENT.md`](https://github.com/Eliot-code/truedesk/blob/main/docs/DEPLOYMENT.md)
also covers deploying on Coolify.

## Point the apps at it

On the home screen of each app, under **RELAY REGIONS**, enter one WebSocket
URL per line, ending in `/ws`, and click **Save relay list**:

```text
wss://relay.example.com/ws
```

Truedesk tries them in order and uses the first one it can reach. Give the
host and the viewer the same list. In the [browser viewer](/docs/browser-viewer/),
set the same URL under **Relay**.

Use `wss://` for any relay you reach over a network. Plain `ws://` still
keeps the session encrypted, but anyone on the path can see which IDs connect
and when. Keep `ws://` for testing on `127.0.0.1` or a LAN.

## Settings

All optional, as environment variables on the relay.

| Variable | Default | Meaning |
| --- | --- | --- |
| `RELAY_STATE_FILE` | unset | JSON file that keeps ID ownership across restarts. The Compose setup sets it. |
| `RELAY_ID_LEASE_SECS` | `2592000` (30 days) | How long an offline host keeps its ID. |
| `RELAY_TRUSTED_PROXIES` | loopback and private ranges | Comma-separated CIDRs of your reverse proxies, or `none`. |
| `RELAY_MAX_ID_BINDINGS` | `100000` | Most IDs tracked at once. New IDs are refused when full. |
| `RELAY_NEW_ID_BURST` / `RELAY_NEW_ID_REFILL_SECS` | `20` / `3600` | How many new IDs one IP (or IPv6 /64) may claim per window. |
| `RELAY_IDLE_TIMEOUT_SECS` | `60` | Closes a paired connection after this long without traffic. |
| `RELAY_SESSION_CONTROL_BURST` / `RELAY_SESSION_CONTROL_PER_SEC` | `64` / `16` | Control messages allowed per paired connection. |
| `RELAY_MAX_RATE_ENTRIES` | `100000` | Most IPs tracked by each rate limiter. |
| `RELAY_SWEEP_SECS` | `60` | How often expired IDs are cleared and the state file saved. |

**IDs.** The first host to register an ID owns it until it has been offline
for the lease period. The state file stores a SHA-256 hash of each host's
token, not the token. Without a state file, ownership resets when the relay
restarts.

**Client addresses.** Rate limits and logs use the client's address. Behind a
trusted proxy, the relay reads it from `X-Forwarded-For` or `X-Real-IP`. The
defaults suit a relay on a private Docker network behind Caddy or Traefik. If
the relay's port is reachable from a network you don't control, narrow
`RELAY_TRUSTED_PROXIES` to your proxy's address.

## STUN

To find a direct path, the apps ask Google's public STUN server
(`stun.l.google.com:19302`) for their public address. STUN only helps
discover addresses; it never carries session data. Changing it is a client
setting in the source (`AgentConfig.urls` in host-core), not a relay option.
