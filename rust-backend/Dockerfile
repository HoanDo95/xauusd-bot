# syntax=docker/dockerfile:1
# Rust backend image — build context: rust-backend/
FROM rust:1.99-slim-bookworm AS builder
WORKDIR /app
# Keep peak memory bounded on the shared 15 GB host.
ENV CARGO_BUILD_JOBS=2 \
    CARGO_INCREMENTAL=0
COPY Cargo.toml Cargo.lock ./
COPY .cargo ./.cargo
COPY src ./src
RUN cargo build --release --locked --bin gold-signal-bot --bin trigger_report --bin news_report --bin funnel_report

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --create-home --home-dir /app --shell /usr/sbin/nologin bot
WORKDIR /app
COPY --from=builder /app/target/release/gold-signal-bot /usr/local/bin/gold-signal-bot
COPY --from=builder /app/target/release/trigger_report /usr/local/bin/trigger_report
COPY --from=builder /app/target/release/news_report /usr/local/bin/news_report
COPY --from=builder /app/target/release/funnel_report /usr/local/bin/funnel_report
RUN mkdir -p /app/data && chown -R bot:bot /app
USER bot
ENV PORT=8090 \
    RUST_LOG=info
EXPOSE 8090
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
  CMD curl -fsS "http://127.0.0.1:${PORT}/api/health" || exit 1
CMD ["gold-signal-bot"]
