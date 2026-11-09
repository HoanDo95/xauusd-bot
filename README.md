# XAUUSD Bot — Sniper 1 Signal / Day

A production-oriented XAU/USD signal engine with **100% free market data**, a hard
**1 real signal/day** quota, Telegram delivery, and a trader dashboard.

> **Risk disclaimer:** This software is for research and signal automation only.
> It is **not financial advice**. Leveraged gold trading can lose more than your
> deposit. Verify every signal yourself and never risk money you cannot afford
> to lose.

## What it does

- **Rust engine** (`rust-backend/`): Tokio + Axum, scans closed M15/H1 candles 24/7.
- **Dynamic S/R zones**: swing-high/low clustering, not fixed lines.
- **Pinbar + volume absorption**: rejection wick + volume ≥ 1.5× MA20.
- **EMA50/200 trend filter**: `BULLISH`, `BEARISH`, `NEUTRAL`, or `AUTO`.
- **Entry / SL / TP1 / TP2 / R:R / lot size**: SL buffer 5–25 USD, TP1 at min R:R
  (default 1:2), TP2 at 1:3, lot size scaled to the risk %.
- **Sniper quota**: max 1 real signal/day (configurable 1–5), Vietnam trading day
  (`UTC+7`), candle dedup key, session filter 13:00–23:00 GMT+7.
- **Outcome tracking**: automatically updates `Active → Target1Hit / Target2Hit /
  StopLossHit` from subsequent closed candles, with conservative same-candle
  handling.
- **Performance stats**: live win rate, average R, net R, profit factor; simulated
  signals are excluded.
- **Telegram alerts**: HTML format with entry, SL, TP1, TP2, R:R, lot size, volume
  rationale, and a risk warning.
- **Dashboard**: dark trading UI, live candles, quota status, signal history,
  config, TradingView webhook sandbox.

## Free data sources (no paid API keys)

| Data | Source | Key |
|---|---|---|
| Gold OHLCV + volume | Binance `PAXGUSDT` public API | none |
| Spot `XAU/USD` (optional) | TwelveData free tier | optional |
| US high-impact calendar | ForexFactory public JSON | none |
| Trend | EMA50/200 computed locally | none |
| Alerts | Telegram Bot API | `@BotFather` token |

## Architecture

```text
GitHub push main
   │
   ▼
ci.yml (self-hosted)  →  build-push.yml (GHCR)  →  deploy.yml (self-hosted)
   │                         │                          │
   └─ cargo test             └─ api + web images        └─ docker compose pull/up
      npm lint/build                                       on xauusd-bot server
                                                               │
                                            ┌──────────────────┴──────────────────┐
                                            ▼                                     ▼
                                  xauusd-api (Rust Axum)                 xauusd-web (Nginx)
                                  scanner + Telegram + state             static dashboard + /api proxy
```

- `docker-compose.yml` — api + web, healthchecks, log rotation, memory limits.
- `docker-compose.ghcr.yml` — pull-only production overlay.
- `docker-compose.build.yml` — local build overlay.
- `docker-compose.observability.yml` — optional Dozzle log UI.

See **[DEPLOYMENT.md](DEPLOYMENT.md)** for the full server/runner/GHCR runbook.

## Run locally (no API keys)

```bash
npm install
npm run dev
# dashboard: http://localhost:3000

cd rust-backend
cp .env.example .env      # add Telegram token/chat id if you want alerts
cargo run --release       # API on :8090
```

Verify:

```bash
cd rust-backend && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
cd .. && npm run lint && npm run build
```

## Docker (local)

```bash
cp rust-backend/.env.example rust-backend/.env   # fill Telegram credentials
docker compose -f docker-compose.yml -f docker-compose.build.yml up -d --build
docker compose ps
curl -s http://127.0.0.1:3000/api/health
```

The API container runs as UID/GID `1000:1000` by default so it can write
`./data/bot_state.json`. Override with `HOST_UID`/`HOST_GID` if needed.

## Production

```bash
TAG=main docker compose -f docker-compose.yml -f docker-compose.ghcr.yml pull
TAG=main docker compose -f docker-compose.yml -f docker-compose.ghcr.yml up -d
```

The deploy workflow does this automatically with `TAG=<commit-sha>`, preserving
`rust-backend/.env` and `data/`. Rollback:

```bash
TAG=<old-sha> docker compose -f docker-compose.yml -f docker-compose.ghcr.yml up -d
```

## Key API endpoints

| Method | Endpoint | Purpose |
|---|---|---|
| `GET` | `/api/health` | Health, feed source, daily quota, kill switch |
| `GET` | `/api/candles` | Closed candles, S/R zones, EMA50, trend |
| `GET` | `/api/signals` | Signal history + performance stats |
| `GET` | `/api/stats` | Win rate, avg R, net R, profit factor |
| `GET/POST` | `/api/config` | Read/update sanitized bot config |
| `POST` | `/api/calculate-lot` | Lot size calculator |
| `GET` | `/api/news-calendar` | ForexFactory events + kill switch |
| `POST` | `/api/simulate-setup` | Simulated signal (never consumes live quota) |
| `POST` | `/api/telegram/test` | Telegram connection test |
| `POST` | `/api/webhook/tradingview` | TradingView alert (optional passphrase) |

## Security notes

- The dashboard is intended for **Tailscale-only** access. If you expose it
  publicly, put authentication in front of it (Cloudflare Access / Nginx basic
  auth) because `/api/config` returns the Telegram token.
- Set `webhook_passphrase` in the dashboard; TradingView alerts must include the
  same `passphrase` field.
- Revoke a leaked `@BotFather` token immediately and update it in the dashboard
  **and** `rust-backend/.env`.
- Deploy workflow never overwrites `.env` or `data/`.

## Backup

```bash
bash scripts/backup-state.sh          # keeps the newest 14 state snapshots
systemctl --user enable --now xauusd-backup.timer
```

## License

Proprietary — all rights reserved. See [LICENSE](LICENSE).
