# XAUUSD Bot — Sniper 1 Signal / Day

A production-oriented XAU/USD signal engine with **100% free market data**, a hard
**1 real signal/day** quota, Telegram delivery, and a trader dashboard.

> **Risk disclaimer:** This software is for research and signal automation only.
> It is **not financial advice**. Leveraged gold trading can lose more than your
> deposit. Verify every signal yourself and never risk money you cannot afford
> to lose.

## What it does

- **Rust engine** (`rust-backend/`): Tokio + Axum, scans closed M15/H1 candles 24/7.
- **Selectable data source**: Binance `PAXGUSDT` (default, native volume) or
  TwelveData XAU/USD OHLC with a Binance PAXG volume proxy; RSI is computed
  locally from the selected closes.
- **Dynamic S/R zones**: swing-high/low clustering, not fixed lines.
- **Pinbar + volume absorption**: rejection wick + volume ≥ 1.5× MA20.
- **EMA50/200 trend filter**: `BULLISH`, `BEARISH`, `NEUTRAL`, or `AUTO`.
- **Entry / SL / TP1 / TP2 / R:R / lot size**: SL buffer 5–25 USD, TP1 at min R:R
  (default 1:2), TP2 at 1:3, lot size scaled to the risk %.
- **Entry execution modes** (dashboard config): `MARKET` uses the live price when it
  is within `max_entry_drift_points` (default 2.5 USD) of the last closed candle;
  `LIMIT_RETEST` waits for the S/R retest; `BREAKOUT` places a stop entry 0.5 USD
  beyond the signal candle. `broker_price_offset` shifts every price to match your
  broker feed.
- **Freshness & isolation guards**: stale closed candles are rejected
  (`max_candle_age_secs`, default 120s); `FALLBACK_SEED` data is never analysed or
  tracked; simulated signals never consume the daily quota and are excluded from
  stats and outcome Telegram alerts.
- **Session pre-alerts**: Telegram briefing 1h before London and New York opens
  (DST-aware), no weekend alerts, persisted dedup so a restart does not resend.
- **Sniper quota**: max 1 real signal/day (configurable 1–5), Vietnam trading day
  (`UTC+7`), candle dedup key, session filter 13:00–23:00 GMT+7.
- **Outcome tracking**: automatically updates `Active → Target1Hit / Target2Hit /
  StopLossHit` from subsequent closed candles, with conservative same-candle
  handling.
- **Performance stats**: live win rate, average R, net R, profit factor; simulated
  signals are excluded.
- **Telegram alerts**: HTML format with entry, SL, TP1, TP2, R:R, lot size, volume
  rationale, and a risk warning.
- **Outcome alerts**: follow-up Telegram messages when TP1, TP2, or SL is hit.
- **Dashboard**: dark trading UI, live candles, quota status, signal history,
  config, TradingView webhook sandbox.

## Free data sources (no paid API keys)

| Data | Source | Key |
|---|---|---|
| Gold OHLCV + volume (default) | Binance `PAXGUSDT` public API | none |
| Gold OHLC + volume proxy | TwelveData `time_series` XAU/USD + Binance `PAXGUSDT` volume | optional `TWELVEDATA_API_KEY` |
| Spot `XAU/USD` reference (basis only) | TwelveData free `/price`, fallback `gold-api.com` | optional |
| US high-impact calendar | ForexFactory public JSON | none |
| Trend | EMA50/200 computed locally | none |
| Daily macro commentary (optional) | OpenCode Go (default), Gemini or OpenRouter | optional |
| AI signal risk review (optional, veto-only) | OpenCode Go, default `gemini-4-argon` | optional |
| Alerts | Telegram Bot API | `@BotFather` token |

### PAXG → XAU/USD spot basis calibration

By default, Binance `PAXGUSDT` is the OHLCV + volume source, but PAXG can trade at a
**$5–$11 premium/discount** versus spot XAU/USD. With
`use_auto_spot_basis` enabled (default true), each Binance-mode scan:

1. Fetches the spot reference from TwelveData
   `https://api.twelvedata.com/price?symbol=XAU/USD&apikey=...` when
   `TWELVEDATA_API_KEY` is set.
2. Falls back to the free `https://api.gold-api.com/price/XAU` endpoint if
   TwelveData is missing or fails.
3. Computes `basis = spot − (live PAXG − broker_price_offset)`, clamps it to
   ±100 USD, and shifts every closed OHLC candle plus the live price by that
   basis (rounded to 2 decimals).
4. Logs `🌐 Spot basis: spot=4111.17, basis=+5.07` and exposes
   `spotPrice`, `spotBasis` and `priceSource` on `/api/health` and
   `/api/candles`.

The spot quote is cached for **300 seconds** (~288 calls/day, safely inside the
TwelveData free tier of 800/day). In `BINANCE_PAXG` mode, TwelveData
`time_series` is only a fallback for the signal-timeframe feed when Binance
fails. In `TWELVEDATA` mode, OHLC comes from `time_series`
(`15min` / `1h` / `4h` / `1day`, `timezone=UTC`, `order=ASC`) and volume is
merged from Binance PAXG by matching candle timestamps; RSI is computed locally
from the TwelveData closes. TwelveData responses are cached per timeframe
(M15 300s, H1/H4 900s, D1 3600s), keeping usage at roughly 500 requests/day on
the free tier. If the proxy has no matching timestamps, the feed reports
`volumeSource = "NONE"`; turn off `volume_filter_enabled` in the dashboard if
you want signals without volume. A persisted `data_source` is preserved on
startup; only empty/unknown values fall back to `BINANCE_PAXG`. The manual
`broker_price_offset` remains supported and is applied before the auto basis;
turn `use_auto_spot_basis` off to use only the manual offset. If both spot
providers fail, the engine keeps Binance PAXG candles with the manual offset
and reports `spotBasis: null`. Defaults remain `data_source = "BINANCE_PAXG"`
and `volume_filter_enabled = true`.

### Optional AI macro report

AI is **optional** and only decorates the once-a-day (07:00) macro report. Core
signal generation — EMA50/200 trend, S/R zones, pinbar, volume and quota — never
calls an AI API.

- `AI_PROVIDER` is optional and case-insensitive: `OPENCODE_GO`, `GEMINI`,
  `OPENROUTER`, or `NONE`. Leave it empty to auto-detect:
  `OPENCODE_GO_API_KEY` → OpenCode Go, else `GEMINI_API_KEY` → Gemini, else
  `OPENROUTER_API_KEY` → OpenRouter, else no AI. `NONE` forces the free path.
- **OpenCode Go** (recommended) uses the OpenAI-compatible endpoint
  `https://opencode.ai/inference/go/openai/v1` (override with
  `OPENCODE_GO_BASE_URL`) and `OPENCODE_GO_MODEL` (default `gemini-4-argon`,
  best reasoning). Tested alternatives: `deepseek-v4.1-flash`
  (cheap/reasoning) and `glm-5.3-flash` (fast/cheap). Requests send
  `User-Agent: xauusd-bot/1.0` and a per-day
  `x-opencode-session: xauusd-macro-<UTC date>` header.
- Gemini uses its OpenAI-compatible endpoint
  (`https://generativelanguage.googleapis.com/v1beta/openai`) with
  `GEMINI_MODEL` (default `gemini-2.5-flash`).
- OpenRouter is supported with `OPENROUTER_MODEL` (default
  `anthropic/claude-3.5-sonnet`, the previous behaviour).
- The scheduled AI report is **one call per day** at 07:00 (the manual Daily
  Cron trigger makes one extra call if pressed). No key or a failed AI call =
  deterministic EMA50/200 report. Signals never depend on the AI provider.
- The key lives only in the environment / `rust-backend/.env`; it is never
  logged or committed.
- The daily report bias is **advisory only**. The cron never writes it into
  `config.macro_bias`, so `AUTO` keeps following EMA50/200 dynamically; an
  explicit `BULLISH`/`BEARISH`/`NEUTRAL` choice is preserved.

### Optional AI Chief Risk Officer (veto-only)

`ai_signal_review_enabled` is **OFF by default**, so signal generation and
Telegram delivery are unchanged unless you turn it on in **Sniper & Risk
Config**. When enabled, the Rust engine asks OpenCode Go (default model
`gemini-4-argon`, override with `AI_SIGNAL_REVIEW_MODEL`) to review each **real**
signal before it is stored or sent to Telegram:

- The AI may return `BUY`, `SELL` or `STANDBY` plus an advisory `entry_zone`,
  `dynamic_sl`, `tp`, `macro_trend_h1_h4` and `reason`.
- It is **veto-only**: a `STANDBY` verdict, an opposite direction, or confidence
  below `ai_signal_review_min_confidence` (default `0.75`, range `0.5–1.0`)
  blocks the signal. The AI can never change the live SL/TP or lot size.
- The review prompt includes D1/H4/H1 bias, EMA50/200, channel position, ATR,
  S/R, current price/spot basis, candidate side/entry/SL/TP/RR/lot, volume,
  RSI M15/H1/H4 context, Vietnam session time and upcoming High-Impact USD
  events.
- **Fail-open by default**: if `OPENCODE_GO_API_KEY` is missing, the request
  errors/times out, or the JSON is malformed, the signal proceeds. Set
  `ai_signal_review_fail_open=false` in the dashboard for fail-closed behaviour.
- One review call per real signal (the daily Sniper quota still caps real
  signals at 1/day by default). Simulated setups never call the AI and never
  consume quota.
- Blocked signals are not stored or sent, and `ai_verdict`, `ai_confidence` and
  `ai_reason` are attached to accepted signals and shown in Telegram and the
  dashboard modal.
- Manual test endpoint: `POST /api/ai/review-signal` with
  `{"signal": {...}, "context": {...}}` (or a raw signal). It returns
  `{"status":"success","review":{...}}`, sends no Telegram and consumes no quota.

### RSI context (Phase R1/R2)

RSI is **advisory/context first**. The Rust engine computes Wilder RSI on
closed candles and exposes it everywhere the bot makes or explains a decision:

- `GET /api/context` returns `rsi_m15`, `rsi_h1` and `rsi_h4` snapshots
  (`value`, `state`, `divergence`). The dashboard "View toàn cảnh" panel shows
  M15/H1/H4 with Oversold/Overbought colouring and divergence badges.
- Every real signal carries `rsi_value`, `rsi_state` and `rsi_divergence`; they
  are shown in the signal modal and appended to Telegram alerts.
- The AI Risk Officer prompt includes the RSI block plus the 3-layer rules
  (H4/H1 supply/demand confluence + RSI extreme/divergence + volume
  confirmation). Missing or contradictory layers should push the AI to
  `STANDBY`. AI remains veto-only and never sets live SL/TP.
- The optional hard gate (`rsi_filter_enabled`) is **OFF by default**, so live
  behaviour is unchanged. When enabled it supports `EXTREME`, `DIVERGENCE` or
  `BOTH` and only applies to real signals; simulations skip it.

Phase R0 backtest evidence (why the default stays OFF):

| Variant | Trades | Net R | Profit factor |
|---|---|---|---|
| `rsi_extreme` (70/30 at trigger candle) | 0/871 | — | — |
| `rsi_extreme_65_35` | 1 | — | — |
| `rsi_divergence` | 69 | −4.76R | 0.94 |
| `htf_h4_d1_rsi_both` | 42 | +7.93R | 1.31 |

`htf_h4_d1_rsi_both` was profitable in-sample (+7.93R, PF 1.31) and OOS
(+5.78R, PF 2.00), but that is a single variant and not enough to change the
default. **Recommendation:** keep RSI as context/AI input; do not enable the
hard filter without new forward-test evidence.

Phase R1 then tested whether the RSI extreme should be measured over a recent
window instead of the trigger candle, plus regime/slope variants:

| Variant | Trades | Net R | PF | MaxDD | OOS Net R | OOS PF |
|---|---:|---:|---:|---:|---:|---:|
| `rsi_recent_extreme_10` | 10 | −2.28 | 0.75 | 6.17 | +1.94 | 3.00 |
| `rsi_recent_extreme_20` | 30 | −7.79 | 0.71 | 18.57 | +6.90 | 8.00 |
| `rsi_recent_extreme_10_65_35` | 26 | +8.29 | 1.53 | 5.13 | +4.92 | 6.00 |
| `rsi_recent_extreme_20_65_35` | 78 | −6.06 | 0.93 | 20.00 | +9.69 | 2.67 |
| `rsi_regime_cross_50` | 376 | +54.01 | 1.25 | 38.00 | +47.52 | 2.53 |
| `htf_h4_d1_rsi_regime_50` | 204 | +53.69 | 1.44 | 16.00 | +14.40 | 2.07 |
| `htf_h4_d1` (reference) | 541 | +209.85 | 1.66 | 31.00 | +54.93 | 2.14 |

Recent-extreme unlocks trades but does not create a robust edge: 10→20 lookback
adds trades at negative marginal expectancy, and 70/30 vs 65/35 is not stable.
Regime/slope variants are momentum-aligned and cut 57–64% of trades while
lowering expectancy and raising full-sample drawdown. `htf_h4_d1` remains the
best quality/return profile; no RSI gate beats it on net R or PF. **Phase R1
conclusion: keep RSI context/AI-only.** If a drawdown-reduction experiment is
wanted, `htf_h4_d1_rsi_regime_50` / `_regime_cross_50` can be forward-tested with
the explicit expectation of lower profit and lower PF.

### Macro & Flow (Phase M1/M2, read-only/advisory)

The dashboard has a **Macro & Flow** card that answers the top-down questions
before a technical setup:

- **Macro context/regime**: FRED DGS10, DGS2, DFII10 real yield, 2s10s curve,
  broad dollar, VIX, MMF assets.
- **Capital flows**: Yahoo DXY, gold futures, GLD/IAU (ETF price/volume proxy),
  TLT, SPY, VIX, GVZ.
- **COT**: CFTC COMEX gold non-commercial net, weekly change, open interest.
- **Scorecard**: macro bias, confidence (max 95%), score, direction, driver,
  flow quality, risk/rate/USD/volatility regimes, and Vietnamese notes.

The card is **read-only**: it never changes signal generation, quota or Telegram
trade alerts. ETF flow is a price/volume proxy (not issuer holdings); COT is
weekly and lagged; GVZ/VIX is an options-skew proxy, not full skew.

Phase M2 adds **advisory alignment** to real signals and the Live Setup preview:

```text
BUY  + macro BULLISH  → ALIGNED
BUY  + macro BEARISH  → COUNTER
SELL + macro BEARISH  → ALIGNED
SELL + macro BULLISH  → COUNTER
```

- Every real signal carries `macro_flow_alignment` / `macro_flow_note`; Telegram
  and the signal modal show them.
- The AI Risk Officer prompt includes the macro/flow block and may lower
  confidence or choose `STANDBY` when the setup is counter-macro.
- `GET /api/live-view` returns the same alignment in its `macroFlow` block; the
  Live Setup card shows an ALIGNED/COUNTER/MIXED/UNKNOWN chip.
- **No hard macro filter**: alignment is advisory only; it does not block a
  signal in code. AI veto remains the only macro-related block.

### Entry execution (dashboard config)

| `entry_mode` | Behaviour |
|---|---|
| `MARKET` | Uses the live price if `|live − last close| ≤ max_entry_drift_points` (default 2.5 USD). If drift is larger, it does **not** chase: it waits at the candle close with a limit entry. If live price already crossed the SL, the signal is cancelled. |
| `LIMIT_RETEST` | Places a limit order at the detected support/resistance retest price. |
| `BREAKOUT` | Places a stop entry 0.5 USD beyond the signal candle high (buy) or low (sell). |

`broker_price_offset` (USD, −100…100) is added to candles, live price and signal
levels so the dashboard matches the broker feed; with `use_auto_spot_basis`, the
PAXG→spot basis is layered on top automatically. A closed candle older than
`max_candle_age_secs` (30–900s, default 120s) is rejected. When both market feeds
fail, the engine labels data `FALLBACK_SEED`, shows it for the UI, and **never**
analyses or tracks it. Simulated setups are stored with `is_simulated=true` and
never consume real quota or appear in stats/outcome alerts.

### Session pre-alerts (dashboard config)

When `session_alerts_enabled` is true, the engine sends a Telegram briefing
`session_alert_lead_minutes` (15–120, default 60) before the 08:00 local open of
London and New York. Session clocks use `Europe/London` and `America/New_York`, so
DST is handled automatically. Alerts are skipped on weekends, and the dedup key is
persisted in `data/bot_state.json` so restarting the API does not resend a briefing.

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

### Source layout (standardized)

```text
rust-backend/src/
  main.rs                 # thin bootstrap only
  worker.rs               # 60s scanner, 15m HTF, 07:00 cron, session alerts
  api/
    mod.rs                # AppState + router + shared filter helpers
    health.rs  signals.rs  market.rs  config.rs
    webhooks.rs  ai.rs  news.rs  telegram.rs  simulation.rs
  analyzer.rs  models.rs  storage.rs  market_feed.rs
  news_filter.rs  scheduler.rs  telegram.rs  risk_manager.rs
  market_context.rs  rsi.rs  volume_delta.rs  signal_review.rs
  tracker.rs  session_alerts.rs

src/app/
  layout.tsx  page.tsx  globals.css   # single Next.js App Router runtime
src/components/                       # dashboard client components
src/lib/api.ts                        # typed API client
server.ts                             # legacy Express fallback only
```

The dashboard is **Next.js App Router only**; the old Vite entrypoints were
removed. Production builds a static `out/` bundle (`npm run build:static`) and
nginx serves it while proxying `/api` to the Rust engine.

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
cd .. && npm run lint && npm run build && npm run build:static
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
| `GET` | `/api/health` | Health, live price, feed/volume source, spot basis/price source, daily quota, kill switch |
| `GET` | `/api/candles` | Live + last closed price, S/R zones, EMA50/200, trend, `dataSource`, `volumeSource`, `spotPrice`, `spotBasis`, `priceSource` |
| `GET` | `/api/context` | Read-only D1/H4/H1 context + `rsi_m15`/`rsi_h1`/`rsi_h4` (204 until HTF ready) |
| `GET` | `/api/signals` | Signal history + performance stats |
| `GET` | `/api/stats` | Win rate, avg R, net R, profit factor (real signals only) |
| `GET/POST` | `/api/config` | Read/update bot config (secrets redacted; blank keeps current) |
| `POST` | `/api/calculate-lot` | Lot size calculator; returns snake_case + camelCase fields |
| `GET` | `/api/news-calendar` | ForexFactory events + kill switch |
| `POST` | `/api/simulate-setup` | Simulated signal (never consumes live quota) |
| `POST` | `/api/ai/review-signal` | Manual AI Risk Officer review (no Telegram, no quota) |
| `POST` | `/api/telegram/test` | Telegram connection test |
| `POST` | `/api/telegram/send-signal` | Send one existing signal to Telegram |
| `POST` | `/api/webhook/tradingview` | TradingView alert (optional passphrase) |

## Security notes

- `GET /api/config` and the `POST /api/config` response **redact** the Telegram
  bot token and webhook passphrase. They are never returned by the API.
- Leaving the Telegram token or webhook passphrase blank in the dashboard keeps
  the currently stored secret; enter a new value only when you want to rotate it.
- Set a long, random, non-empty `webhook_passphrase`; TradingView alerts must
  include the same `passphrase` field. Empty means the webhook check is disabled.
- Rotate the `@BotFather` token immediately if it was ever exposed (logs, chat,
  screenshot, public repo), then update it in the dashboard **and**
  `rust-backend/.env`.
- The dashboard is intended for **Tailscale-only** access; Tailscale remains the
  perimeter. Optional HTTP Basic auth is built in: set `DASHBOARD_USER` and
  `DASHBOARD_PASSWORD` in the compose `.env` and restart `web`. Health and
  TradingView webhook paths stay exempt (webhooks use the passphrase).
- CI runs only on `push`/manual dispatch. Do not run fork/PR code on the
  self-hosted runner: it has Docker access and can read the deploy directory.
- The persistent state file is written with `0600` permissions on Unix, and
  backups use `umask 077` + `chmod 600`.
- Deploy workflow never overwrites `.env` or `data/`.

## Backup

```bash
bash scripts/backup-state.sh          # keeps the newest 14 state snapshots
systemctl --user enable --now xauusd-backup.timer
```

## License

Proprietary — all rights reserved. See [LICENSE](LICENSE).
