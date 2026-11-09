# syntax=docker/dockerfile:1
# Rust backend image — build context: rust-backend/
FROM rust:1.99-slim-bookworm AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --create-home --home-dir /app --shell /usr/sbin/nologin bot
WORKDIR /app
COPY --from=builder /app/target/release/gold-signal-bot /usr/local/bin/gold-signal-bot
RUN mkdir -p /app/data && chown -R bot:bot /app
USER bot
ENV PORT=8090 \
    RUST_LOG=info
EXPOSE 8090
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
  CMD curl -fsS "http://127.0.0.1:${PORT}/api/health" || exit 1
CMD ["gold-signal-bot"]
