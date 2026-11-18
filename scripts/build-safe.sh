#!/usr/bin/env bash
# Memory-safe verification/build for the shared 15 GB host.
#
# Why: cargo/next can spawn enough parallel workers to exhaust RAM while the
# live bot, Exness MT5 terminal, and Docker are running. This script keeps
# cargo at 2 jobs, disables incremental compilation, caps the Node heap, and
# uses the static export that production actually serves.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
export CARGO_INCREMENTAL=0
export NODE_OPTIONS="${NODE_OPTIONS:---max-old-space-size=2048}"
export NEXT_TELEMETRY_DISABLED=1

echo "==> Rust: fmt"
(cd rust-backend && cargo fmt --check)

echo "==> Rust: clippy (jobs=${CARGO_BUILD_JOBS})"
(cd rust-backend && cargo clippy --all-targets -- -D warnings)

echo "==> Rust: tests (jobs=${CARGO_BUILD_JOBS})"
(cd rust-backend && cargo test --locked)

echo "==> Web: lint"
npm run lint

echo "==> Web: static build (NODE_OPTIONS=${NODE_OPTIONS})"
npm run build:static

echo "==> Done."
