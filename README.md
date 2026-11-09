# Gold Signal Bot — Sniper 1 Lệnh / Ngày

XAU/USD signal engine with **100% free data feeds** and a hard **1 real signal/day** quota.

## What runs where

- **Rust engine:** `rust-backend/` (Tokio/Axum, persistent `data/bot_state.json`, Binance PAXG + ForexFactory feeds).
- **Express/Vite fallback:** `server.ts` mirrors the same free feeds, quota, dedup key, EMA50 trend and `risk_points <= 45.0`.
- **Dashboard (single source of truth):** `src/app/page.tsx` (Next.js App Router).
- **Vite entry:** `src/main.tsx` → `src/App.tsx` → re-exports `src/app/page.tsx`.
- **Next.js config:** `next.config.mjs` rewrites `/api/*` to `RUST_BACKEND_URL` (default `http://127.0.0.1:8080`).

## Run locally (Express + Vite, no API keys required)

```bash
npm install
npm run dev
# dashboard: http://localhost:3000
```

## Verify

```bash
npm run lint   # tsc --noEmit, zero TypeScript errors
npm run build  # vite production bundle
```

## Free data sources

- **Candles:** Binance Public API `PAXGUSDT` (no key) with automatic removal of the still-forming candle.
- **Optional spot XAU/USD:** TwelveData Free (`TWELVEDATA_API_KEY`, fallback to Binance).
- **News calendar:** ForexFactory public JSON (`ff_calendar_thisweek.json`) with safe offline fallback.
- **Alerts:** Telegram Bot API (free via `@BotFather`).
- **Trend:** EMA50/EMA200, computed locally. No paid AI tokens.

## Sniper rules

- `max_signals_per_day = 1` (configurable 1–5).
- Session filter defaults to **13:00–23:00 Vietnam time**.
- `gold_sl_buffer_points` supports **5.0–25.0** (8–10 on M15, 15–20 on H1); lot size shrinks automatically to preserve risk %.
- Duplicate closed candles are blocked by `dedup_key = {symbol}_{timeframe}_{candle_timestamp}`.
- Dashboard simulation signals are marked `is_simulated = true` and **never consume the daily quota**.
