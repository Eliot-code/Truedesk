# Deploying the relay

See [`docs/DEPLOYMENT.md`](../docs/DEPLOYMENT.md). Quick start on a server
with Docker and a DNS name pointing at it:

```sh
DOMAIN=relay.example.com docker compose up -d
```

This directory holds the Caddy config used by [`compose.yaml`](../compose.yaml).
