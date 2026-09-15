# Self-hosting the OpenDesk relay

The relay is a single stateless HTTP server. All client traffic (WebSocket over
an HTTP upgrade) rides **one HTTP port**, so any reverse-proxy platform that
terminates TLS on 443 and forwards to a container port can host it. No raw
TCP/UDP ports, unlike RustDesk's 21115–21119.

## Coolify (recommended)

Coolify runs Traefik and manages HTTPS on 443 for you.

1. Create a new **Application** from this git repo.
2. Build pack: **Dockerfile**, path `crates/relay/Dockerfile`.
3. Set the app's domain (e.g. `relay.example.com`). Coolify provisions the
   TLS cert and routes `https://relay.example.com` (port 443) to the
   container.
4. Coolify injects `PORT`; the relay binds it automatically. Leave the
   exposed port as the container's `PORT` (default 8080).
5. Deploy. Clients connect to `wss://relay.example.com/ws`.

No extra ports, no Caddy/Nginx config — Coolify's proxy handles TLS + routing.

## Plain Docker (any VPS, behind your own TLS proxy)

```bash
docker build -f crates/relay/Dockerfile -t opendesk-relay .
docker run -e PORT=8080 -p 8080:8080 opendesk-relay
```

Put any TLS-terminating reverse proxy (Caddy, Traefik, Nginx) in front,
forwarding `wss` on 443 to the container's `8080`. WebSocket upgrades pass
through unchanged.

## Notes

- The relay is a **blind pipe**: it holds no keys and never sees plaintext.
  Running it on untrusted infrastructure does not expose session content.
- It is stateless — scale horizontally only with sticky routing per peer pair
  (a single instance is fine for most self-hosting).
- Sessions are P2P-first: clients exchange ICE candidates through the paired
  relay WebSocket after key confirmation, then use direct UDP when possible.
  Only outbound UDP is required; no inbound firewall rule or forwarded port is
  needed. Networks that block UDP fall back to the relay on `wss://` port 443.
- ICE uses Google Public STUN by default (`stun:stun.l.google.com:19302`). Set
  the host-core `AgentConfig.urls` to a self-hosted STUN endpoint when desired.
- The host reports `INFO transport selected: direct` or
  `INFO transport selected: relay (ICE timeout)` in its logs.
