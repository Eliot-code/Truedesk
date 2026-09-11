# OpenDesk relay — single-HTTP-port server for reverse-proxy platforms (Coolify, etc).
# The platform terminates TLS on 443 and proxies to this container's PORT.

# --- build ---
FROM rust:1-slim AS build
WORKDIR /src
COPY . .
RUN cargo build --release -p relay

# --- runtime ---
FROM debian:bookworm-slim
RUN useradd -m -u 10001 relay
COPY --from=build /src/target/release/relay /usr/local/bin/relay
USER relay

# The platform injects $PORT; default 8080. Only ONE HTTP port is exposed —
# all traffic (WebSocket over HTTP upgrade) rides this single port.
ENV PORT=8080
EXPOSE 8080

CMD ["relay"]
