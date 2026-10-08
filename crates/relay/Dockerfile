# OpenDesk relay: one HTTP port (WebSocket at /ws, health at /health).
# Put a TLS proxy in front (see compose.yaml + deploy/Caddyfile, or Coolify).

# Build and runtime share a Debian release so the binary's glibc matches.
FROM rust:1-slim-trixie AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p relay \
 && cp target/release/relay /relay

FROM debian:trixie-slim
RUN useradd --system --uid 10001 relay
COPY --from=build /relay /usr/local/bin/relay
USER relay

# Platforms like Coolify inject $PORT; default 8080.
ENV PORT=8080 RUST_LOG=info
EXPOSE 8080
CMD ["relay"]
