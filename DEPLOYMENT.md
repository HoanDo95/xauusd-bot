# Deployment — XAUUSD Bot (GHCR + GitHub Actions + Docker Compose)

## 1. Architecture

```text
GitHub push (main)
      │
      ▼
ci.yml ──► build-push.yml ──► GHCR images ──► deploy.yml
(cargo test,   (Docker Buildx)   xauusd-bot-api     (self-hosted runner
 npm lint/build)                  xauusd-bot-web      on xauusd-bot server)
                                                        │
                                                        ▼
                                           docker compose pull + up
                                                        │
                              ┌─────────────────────────┴─────────────────────────┐
                              ▼                                                   ▼
                     xauusd-api (Rust Axum)                              xauusd-web (Nginx)
                     - scanner 24/7                                       - serves Next.js out/
                     - Telegram alerts                                    - proxies /api → api:8090
                     - data/bot_state.json
```

Two containers, two log streams:

- `docker compose logs -f api` — Rust engine: scanner, market feed, news filter, Telegram, storage.
- `docker compose logs -f web` — Nginx access/error logs only.

Rust log lines now include module targets and thread IDs, e.g.
`gold_signal_bot::storage`, `gold_signal_bot::market_feed`, `ThreadId(13)`.

> The Rust engine intentionally stays as **one** process (API + scanner + scheduler).
> Splitting it into `api` and `worker` containers would duplicate the in-memory candle
> buffer and race on `data/bot_state.json`. A true split requires SQLite/WAL + a queue;
> that is a separate architectural change, not a deployment change.

## 2. Images

| Image | Build context | Dockerfile |
|---|---|---|
| `ghcr.io/hoando95/xauusd-bot-api` | `rust-backend/` | `rust-backend/Dockerfile` |
| `ghcr.io/hoando95/xauusd-bot-web` | repo root | `Dockerfile.web` |

Tags pushed: `main` and the full commit SHA. Rollback with `TAG=<old-sha>`.

If your GitHub owner is not `hoando95`, update:

- `OWNER_LC` in `.github/workflows/build-push.yml`
- image names in `docker-compose.yml` and `docker-compose.ghcr.yml`

## 3. Create the GitHub repo and first push

Repo root must be this directory (`gold-signal-bot/`), because workflows use
`rust-backend/` and `Dockerfile.web` relative to the root.

```bash
cd gold-signal-bot
git init
git add .
git commit -m "feat: XAUUSD signal bot with containerized deployment"
git branch -M main
git remote add origin git@github.com:HoanDo95/xauusd-bot.git
git push -u origin main
```

CI runs first. When it is green, `build-push` pushes both images to GHCR.

## 4. GHCR visibility / pull auth

GHCR packages are private by default. Choose one:

### Option A — Public packages (simplest)
GitHub → your profile → Packages → `xauusd-bot-api` → Package settings →
Change visibility → Public. Repeat for `xauusd-bot-web`.
No server login is needed.

### Option B — Private packages + PAT
Create a classic PAT with `read:packages`. Add it as repo secret
`GHCR_READ_TOKEN`. `deploy.yml` logs in only when that secret exists.

## 5. Self-hosted runner on the server

On the GitHub repo: **Settings → Actions → Runners → New self-hosted runner**
(Linux x64). Copy the registration token (valid ~1 hour).

On `xauusd-bot`:

```bash
ssh jason@xauusd-bot
cd /home/jason/xauusd-bot

REPO_URL="https://github.com/HoanDo95/xauusd-bot" \
RUNNER_TOKEN="<token from GitHub>" \
bash scripts/setup-github-runner.sh
```

The script:

- downloads the latest `actions/runner` into `~/actions-runner`
- registers it with label `xauusd-bot`
- installs a **systemd --user** unit `github-runner.service`
- starts it (linger is already enabled, so it survives logout/reboot)

Check:

```bash
systemctl --user status github-runner
journalctl --user -u github-runner -f
```

Runner requirements on the server: `docker` (user `jason` is in the `docker`
group), `rsync`, `curl`, `python3` — all present.

## 6. First deploy after GHCR is populated

The server is already migrated to Docker. To pull the CI-built images:

```bash
cd /home/jason/xauusd-bot
TAG=main docker compose -f docker-compose.yml -f docker-compose.ghcr.yml pull
TAG=main docker compose -f docker-compose.yml -f docker-compose.ghcr.yml up -d
docker compose ps
curl -s http://127.0.0.1:3000/api/health
```

`deploy.yml` does exactly this automatically with `TAG=<commit-sha>`.

## 7. Local build (no registry)

```bash
cd gold-signal-bot
docker compose -f docker-compose.yml -f docker-compose.build.yml up -d --build
```

## 8. Server operations

```bash
cd /home/jason/xauusd-bot

docker compose ps
docker compose logs -f api
docker compose logs -f web
docker compose restart api
docker compose down            # keeps ./data and .env

# Rollback to a previous SHA
TAG=<old-sha> docker compose -f docker-compose.yml -f docker-compose.ghcr.yml up -d
```

Persistent paths:

- `./data/bot_state.json` — config + signal history + optional daily quota/dedup state.
  The API rewrites it atomically and sets `chmod 600` on Unix.
- `./rust-backend/.env` — Telegram token/chat ID and runtime config. Keep it
  `chmod 600` and never commit it.
- `./data` must be owned by UID 1000 (compose runs the API as `1000:1000`).

## 9. Optional log UI (Dozzle)

For browser-based log tracing without SSH:

```bash
docker compose -f docker-compose.yml -f docker-compose.observability.yml up -d dozzle
```

Then open `http://xauusd-bot:8081` inside the tailnet.
Dozzle mounts the Docker socket read-only; keep it tailnet-only.

## 10. Tailscale Serve / Funnel

The host Tailscale Serve proxies `https://xauusd-bot.tailec308e.ts.net/` →
`http://127.0.0.1:3000` (the `web` container). By default it is tailnet-only.

For remote access from a machine outside the tailnet, enable **Tailscale
Funnel** and keep Basic auth on:

```bash
# On the server
tailscale funnel --bg 3000
tailscale funnel status

# Disable public exposure again
tailscale funnel --https=443 off
```

Funnel publishes the same HTTPS URL to the internet; it does **not** open a
router port. Always set `DASHBOARD_USER`/`DASHBOARD_PASSWORD` in
`/home/jason/xauusd-bot/.env` before enabling Funnel, and set a strong
`WEBHOOK_PASSPHRASE` in `rust-backend/.env` because webhook paths are exempt
from Basic auth. See section 13b for the auth setup.

Verify from outside:

```bash
curl -I https://xauusd-bot.tailec308e.ts.net/                    # 401
curl -u operator:<password> -I https://xauusd-bot.tailec308e.ts.net/  # 200
```

## 11. CI on the self-hosted runner (current mode)

This account currently cannot start GitHub-hosted jobs (billing block:
"recent account payments have failed or your spending limit needs to be
increased"). Therefore `ci.yml` and `build-push.yml` run on the same
`xauusd-bot` self-hosted runner:

- CI: cargo fmt/clippy/test + npm lint/build on the server.
- Build: `docker build` on the server daemon, then `docker push` to GHCR.
- Deploy: pull the new tags and restart compose.

Once billing is fixed, switch `runs-on` back to `ubuntu-latest` in
`ci.yml` and `build-push.yml` to offload builds from the server.

## 12. Daily state backup

`data/bot_state.json` holds config, signal history, optional quota and dedup state.
Install the user timer once on the server:

```bash
cd /home/jason/xauusd-bot
mkdir -p ~/.config/systemd/user
cp deploy/systemd/xauusd-backup.service deploy/systemd/xauusd-backup.timer ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now xauusd-backup.timer
systemctl --user start xauusd-backup.service   # test immediately
ls -lt backups | head
```

The timer runs daily at 03:15 (randomized ±5 min) and keeps the newest 14
snapshots. `backups/` is excluded from deploys and git. The script uses
`umask 077`, creates `backups/` with mode `700`, and forces each snapshot to
`chmod 600` so state secrets are not world-readable.

Rotate the Telegram bot token and webhook passphrase after any exposure (shared
logs, screenshots, backups, or a public repository). Update the token in the
dashboard and `rust-backend/.env`, then restart the API.

## 13. Webhook passphrase

Set `webhook_passphrase` in **Sniper & Risk Config**. When non-empty, every
TradingView alert must include the same value:

```json
{"passphrase":"<your-secret>","ticker":"XAUUSD","action":"BUY","price":4080.5}
```

Use a long random value (at least 16 characters; 32+ recommended). The API
compares it in constant time. The dashboard webhook sandbox and Pine Script
sample read the same config value. Leave it empty only for Tailscale-only use;
empty disables the check and is not recommended for public endpoints. Rotate the
passphrase after any exposure.

## 13b. Optional dashboard Basic auth

The web container can protect the dashboard and `/api/` with HTTP Basic auth.
It is **opt-in**; leave both variables unset to keep the Tailscale-only behavior.

```bash
# /home/jason/xauusd-bot/.env (compose project env, never committed)
DASHBOARD_USER="operator"
DASHBOARD_PASSWORD="use-a-long-random-password"

cd /home/jason/xauusd-bot
docker compose up -d web
docker compose logs web | grep '\[auth\]'
```

Verify:

```bash
curl -I http://127.0.0.1:3000/                 # 401 without credentials
curl -I -u operator:<password> http://127.0.0.1:3000/   # 200
curl -I http://127.0.0.1:3000/healthz          # 200 (exempt)
```

The web image also includes an opaque `AuthShield`: it covers the viewport
until `/api/health` succeeds, so the dashboard is not visible behind the
browser's native Basic auth dialog. Dashboard/API responses send
`Cache-Control: no-store`; SPA and API 401 responses send
`Clear-Site-Data: "cache"` to purge stale cached HTML. If a browser still shows
an old cached dashboard after an upgrade, hard-refresh once.

Webhook paths (`/api/webhook`, `/api/webhook/tradingview`,
`/api/candles/webhook`) and health endpoints are exempt so TradingView can keep
posting; they rely on the webhook passphrase instead. The deploy smoke test uses
`/healthz` and the API container's direct health endpoint, so it works with auth
enabled.

## 14. Optional AI macro report

AI is optional and only used for the daily 07:00 macro commentary. Core signals
always use the free EMA50/200 heuristic, so the bot works fully without any AI
key. Configure it in `rust-backend/.env` and restart the API container:

```bash
# "" = auto-detect, "none" = disabled, or opencode_go | gemini | openrouter
AI_PROVIDER=""
# OpenCode Go (recommended): OpenAI-compatible, default model Gemini 4 Argon.
OPENCODE_GO_API_KEY=""
OPENCODE_GO_BASE_URL="https://opencode.ai/inference/go/openai/v1"
OPENCODE_GO_MODEL="gemini-4-argon"
# Model alternatives: "deepseek-v4.1-flash" (cheap/reasoning),
# "glm-5.3-flash" (fast/cheap).
# AI model for DECISION / VETO_ONLY signal review:
AI_SIGNAL_REVIEW_MODEL="gemini-4-argon"
GEMINI_API_KEY=""
GEMINI_MODEL="gemini-2.5-flash"
OPENROUTER_API_KEY=""
OPENROUTER_MODEL="anthropic/claude-3.5-sonnet"

docker compose restart api
docker compose logs -f api   # logs provider/model, never the key
```

- Auto-detect order: `OPENCODE_GO_API_KEY` first, then `GEMINI_API_KEY`, then
  `OPENROUTER_API_KEY`. `AI_PROVIDER=none` disables AI explicitly.
- OpenCode Go uses the OpenAI-compatible endpoint
  `https://opencode.ai/inference/go/openai/v1` (override with
  `OPENCODE_GO_BASE_URL`). Default model is `gemini-4-argon`; tested
  alternatives are `deepseek-v4.1-flash` and `glm-5.3-flash`. Requests include
  `User-Agent: xauusd-bot/1.0` and `x-opencode-session: xauusd-macro-<UTC date>`.
- The scheduler makes one AI call per day at 07:00 (pressing the manual Daily
  Cron trigger adds one more). No key, `AI_PROVIDER=none`, or an AI error falls
  back to the deterministic EMA50/200 report; signals are unaffected.
- Keep the API key only in the environment / `rust-backend/.env`. It is never
  logged or committed; edit `.env` on the server and restart the API container.
- Gemini uses the OpenAI-compatible endpoint
  (`https://generativelanguage.googleapis.com/v1beta/openai`).
- The daily report bias is advisory only. It is shown in the dashboard but is
  **never** written to `config.macro_bias`, so `AUTO` stays dynamic and an
  explicit `BULLISH`/`BEARISH`/`NEUTRAL` setting is preserved.

### AI Decision Mode (default) + legacy VETO_ONLY

AI review defaults to **ON** for new/default configs and is separate from the
daily macro report. `ai_review_mode` defaults to **`DECISION`**; `VETO_ONLY`
restores the old veto-only behaviour. An existing `data/bot_state.json` with
`ai_signal_review_enabled=false` stays off until you change it. Configure both
in **Sniper & Risk Config**, not in env.

- Model: `AI_SIGNAL_REVIEW_MODEL` (default `gemini-4-argon`); base URL:
  `OPENCODE_GO_BASE_URL`; key: the existing `OPENCODE_GO_API_KEY`.
- **429 resilience:** OpenCode Go may return 429 when the upstream Gemini TPM
  quota is exhausted. The client retries twice with `Retry-After`/backoff, then
  falls back to `AI_SIGNAL_REVIEW_FALLBACK_MODEL` (default
  `deepseek-v4.1-flash`). Set it to `off` to disable the fallback. The model
  actually used is stored in the decision/review `model` field.
- **DECISION:** AI is the primary BUY/SELL/STANDBY layer. Candidate discovery
  relaxes `volume_filter_enabled` and `macro_bias` so AI can review setups the
  old soft filters rejected. After AI approval the HTF/Volume Delta/RSI soft
  filters are skipped, but code still enforces SL/TP ordering, minimum R:R,
  risk budget, optional daily quota, session, news and dedup. AI never sets live SL/TP.
- **DECISION is fail-closed:** missing key, provider error/timeout, or malformed
  JSON means no trade signal, regardless of `ai_signal_review_fail_open`.
- **Early entry (90% rule):** when no pinbar candidate exists, a valid
  `PRIMARY`/`ALTERNATE` scenario with `confidence >= early_entry_min_confidence`
  (default `90`) can become a pending signal after AI approval. Entry stays at
  the scenario trigger (`BREAKOUT`/`LIMIT`, `PENDING`), never a market chase.
  Set `early_entry_enabled=false` to disable.
- **Anti-chase:** daily change `<= anti_chase_daily_change_pct` (default `-2.0%`)
  plus RSI H1 `< anti_chase_rsi_h1` (default `25`) or M15 Bullish divergence
  invalidates bearish breakout/breakdown and promotes the pullback/rejection.
  The bullish mirror uses `+2.0%` and RSI H1 `>75`. Set
  `anti_chase_filter_enabled=false` to disable.
- **Confidence cap:** `>=90%` only when macro/D1/H4 agree, RSI H1 is `30–65`,
  and there is no opposite M15 divergence; otherwise cap at 85/75/65. The score
  is confluence/readiness, not win probability.
- **Structural SL:** breakout/breakdown SL is anchored beyond the recent M15
  swing high/low plus a buffer; if risk exceeds 45 points the scenario is
  invalid, not tightened into the retest zone.
- **Pullback max overshoot:** a `LIMIT` fill is cancelled when the fill candle
  overshoots entry by more than `0.5R` without a rejection close. The bot waits
  for a recalculated S/R zone instead of filling near the SL.
- **Pullback confirmation:** `bearish_rejection` with RSI M15 `>=70` at
  resistance (mirror: `bullish_bounce` with RSI M15 `<=30` at support) gets a
  +10 confluence bonus and can reach `>=90` when macro/D1-H4 and RSI H1 agree.
- **PAXG volume:** weak proxy, weighted below 10%, never the main reason for a
  signal. True tick volume requires a paid OANDA/FXCM/CME feed.
- **Portfolio risk guardrails:** `risk_guardrails_enabled` defaults `true`;
  `max_concurrent_signals=2`, `max_daily_risk_percent=3.0`,
  `max_daily_loss_r=2.0`, `same_direction_cooldown_minutes=60`. They run at the
  final signal gate for the scanner and both webhook paths; simulated signals
  are ignored. After TP1 the outcome alert suggests break-even, partial profit
  and M15-structure trailing.
- **Backtest-derived strategy filters:** `counter_macro_filter_enabled=true`
  skips COUNTER-macro entries; `bearish_rejection_enabled=false` disables the
  negative-expectancy rejection scenario; `min_rsi_h1_for_buy=30.0` blocks BUY
  entries in deep H1 oversold. COUNTER PRIMARY scenarios are replaced by an
  ALIGNED scenario, or marked non-tradable if none exists.
- **Scenario geometry:** `htf_levels_enabled=true` merges H4/H1/D1 S/R and EMA
  levels into the scenario level set; a level confirmed by 2+ HTF timeframes is
  preferred for breakout/breakdown triggers. `structural_sl_atr_cap=1.0` caps a
  breakout/breakdown structural stop at 1× H4 ATR from the broken level, so an
  entry that has already travelled away from the swing cannot carry a 5× ATR
  stop. `rsi_d1` is now part of `/api/context`, the dashboard RSI panel and the
  AI prompt. Cost-adjusted A/B (0.05R): PAXG 2025 +0.037R → +0.110R, Dukascopy
  2025 +0.096R → +0.141R, Dukascopy 2022–2023 -0.043R → +0.031R. These values
  still need forward-testing and a full spread/commission model.
- **VETO_ONLY:** AI may only approve, contradict or return `STANDBY`.
  `ai_signal_review_fail_open` (default `true`) applies only here; `false`
  makes it fail-closed.
- Dashboard controls: `ai_review_mode`, `ai_signal_review_min_confidence`
  (`0.5–1.0`, default `0.75`), `ai_signal_review_fail_open` (VETO_ONLY only),
  `early_entry_enabled` (default `true`), `early_entry_min_confidence`
  (`50–100`, default `90`), `anti_chase_filter_enabled` (default `true`),
  `anti_chase_daily_change_pct` (default `-2.0`) and `anti_chase_rsi_h1`
  (default `25`).
- One decision/review per real signal attempt. **Unlimited signals/day by
  default**; enable `daily_quota_enabled` to cap real signals at
  `max_signals_per_day` (1–5). Multiple signals still each pass AI Decision +
  hard risk + session/news + per-candle dedup. Simulated setups skip AI and quota.
- **Start:** in DECISION mode it runs one AI decision and sends an
  `AI MARKET BRIEF` (verdict + compact scenario body) to Telegram; it never
  inserts a signal or spends quota. If AI is unavailable, Start still succeeds,
  sends the scenario summary and returns `aiError`. The response includes
  `aiDecision`/`aiError`; `/api/action-view` exposes the latest `aiDecision`.
- `POST /api/ai/review-signal` validates a signal manually without sending
  Telegram or consuming quota. `GET /api/health` exposes `aiReviewEnabled` and
  `aiReviewMode`.
- `POST /api/ai/entry-check` (Telegram `/start-entry`) is an **on-demand** AI
  entry check: it runs the same brief as Start, returns the AI action plus a
  reference plan from the candidate/selected scenario (`CANDIDATE`, `PRIMARY`,
  `ALTERNATE`, `WATCHLIST`), and validates levels with the same SL/TP ordering,
  R:R and risk-budget rules. It never inserts/sends a trade signal, never sends
  Telegram by itself, never consumes quota and never trades. Unsafe levels return
  `valid=false` with the reason; AI STANDBY may still show the PRIMARY scenario
  as a clearly-labelled reference. Scenario plans expose `readiness` and
  `early_entry=true` when the confidence clears the early-entry threshold before
  the trigger confirms; entry remains the scenario trigger. Success is HTTP 200
  `{"status":"ok","result":{...}}`; unavailable/disabled is HTTP 503
  `{"status":"unavailable","message":"..."}`. The command is admin-only and
  rate-limited to one per 30 seconds per chat.

RSI 70/30 is a context label, not an automatic reversal: `>=70` Overbought,
`<=30` Oversold; divergence is a warning, not an entry.

## 15. Runtime config vs environment

Entry and session behaviour is **dashboard config**, persisted in
`data/bot_state.json`, not environment variables:

- `entry_mode` (`MARKET` / `LIMIT_RETEST` / `BREAKOUT`), `broker_price_offset`,
  `max_entry_drift_points`, `max_candle_age_secs`.
- `session_alerts_enabled`, `session_alert_lead_minutes`, `session_filter_enabled`.
- `daily_quota_enabled` (**default OFF = unlimited**), `max_signals_per_day`
  (1–5, only used when the toggle is on). Per-candle dedup remains active.
- `ai_signal_review_enabled`, `ai_review_mode` (`DECISION` default /
  `VETO_ONLY`), `ai_signal_review_min_confidence`,
  `ai_signal_review_fail_open` (VETO_ONLY only; AI key/model stay in
  `rust-backend/.env`).
- `rsi_filter_enabled` (**default OFF**), `rsi_period`, `rsi_overbought`,
  `rsi_oversold`, `rsi_divergence_enabled`, `rsi_divergence_lookback`,
  `rsi_pivot_k`, `rsi_mode`. No new environment variables are required; toggle
  these in **Sniper & Risk Config**. RSI M15/H1/H4 is always shown and sent to
  the AI as context even while the hard filter is off.

RSI is advisory/context by default. Phase R0 showed the 70/30 trigger-candle
gate produced 0/871 trades and the divergence variant lost 4.76R, so enabling
the hard filter is a deliberate opt-in after forward testing, not a deploy step.

Change them in **Sniper & Risk Config** and save; no container restart is needed.
Environment variables are only for secrets/runtime wiring (Telegram, webhook
passphrase, TwelveData key, AI provider/model, `PORT`, `RUST_LOG`). After editing
`rust-backend/.env`, restart the API so the new environment is loaded:

```bash
docker compose restart api
docker compose logs -f api
```

## 16. XAU/USD data source: TwelveData vs Binance PAXG

Two modes are supported (dashboard config, persisted in `data/bot_state.json`):

| Mode | Price | Volume |
|---|---|---|
| `BINANCE_PAXG` (default) | Binance `PAXGUSDT` + spot basis | Binance PAXG |
| `TWELVEDATA` | TwelveData `XAU/USD` OHLC | Yahoo GC=F volume (fallback PAXG → NONE) |

`TWELVEDATA` mode:

- OHLC from `time_series` (`15min` / `1h` / `4h` / `1day`, `timezone=UTC`,
  `order=ASC`); RSI is computed locally from those closes.
- Volume is merged from Yahoo GC=F (COMEX futures) by matching candle
  timestamps; Binance PAXG is the fallback, then `NONE`. If no timestamps
  match, `volumeSource = "NONE"`; turn off `volume_filter_enabled` in the
  dashboard to keep generating price-action signals without volume.
- Cached per timeframe: M15 300s, H1/H4 900s, D1 3600s (~500 requests/day,
  inside the free 800/day tier).
- Health shows `dataSource`, `priceSource`, `volumeSource`, `spotBasis: null`.
- Switch back to Binance at any time via `data_source = BINANCE_PAXG`.

`BINANCE_PAXG` mode keeps the spot-basis calibration: the TwelveData `/price`
reference is cached 300s and shifts PAXG OHLC toward XAU/USD; `spotBasis` is
`null` if both spot providers fail and the engine continues with PAXG + manual
offset.

Set the key in `rust-backend/.env`, then restart the API:

```bash
# rust-backend/.env
TWELVEDATA_API_KEY="your_twelvedata_api_key_here"

docker compose restart api
docker compose logs -f api   # look for: TwelveData XAU/USD ... / Đã ghép volume ...
```

### Yahoo GC=F futures volume (default for `TWELVEDATA`)

TwelveData has no volume. The feed merges COMEX gold futures volume from
Yahoo's public chart endpoint:

```text
TwelveData XAU/USD OHLC
        + Yahoo GC=F volume (15m/1h/1d; H1→H4 aggregate)
        ↓
volumeSource = YAHOO_FUTURES_PROXY
        ↓ fallback
BINANCE_PAXG_PROXY → NONE
```

Important implementation details:

- Use `period1` / `period2`; the `range=` variant returns HTTP 429 from the
  production network.
- A 10-minute cache, two retries with backoff, and a 30-minute circuit breaker
  after three consecutive failures prevent Yahoo rate-limit spam.
- Only closed bars are merged. Yahoo D1 bars open at 05:00/06:00 UTC (00:00 ET)
  and are normalised to UTC midnight before matching TwelveData D1 candles.
- Yahoo GC=F has no taker buy/sell split; `taker_buy_volume=0` and delta/CVD is
  disabled for `YAHOO_FUTURES_PROXY`.
- The AI prompt weights GC=F volume below 20% and never as the main reason for
  a signal. PAXG remains the fallback and is weighted below 10%.

Runtime kill switch:

```bash
# /home/jason/xauusd-bot/rust-backend/.env
YAHOO_FUTURES_VOLUME_ENABLED=true
```

`false` is a hard startup kill (requires `docker compose up -d api`). The
dashboard also exposes `yahoo_futures_volume_enabled` for a runtime toggle; it
is ANDed with the env flag, so the env var can always force-disable Yahoo.

Verify after deploy:

```bash
curl -s http://127.0.0.1:8090/api/health | jq '{volumeSource, yahooFuturesVolumeEnabled}'
curl -s http://127.0.0.1:8090/api/context | jq '.price_source'
```

Expected: `volumeSource="YAHOO_FUTURES_PROXY"` and
`price_source="TWELVEDATA_XAUUSD+YAHOO_FUTURES_VOLUME"`. If Yahoo fails, the
same fields show `BINANCE_PAXG_PROXY` / `TWELVEDATA_XAUUSD+PAXG_VOLUME`.

### OANDA tick volume (optional, free demo)

Create a free OANDA demo account, copy the v20 API token, then set:

```bash
# rust-backend/.env
OANDA_API_TOKEN="your_oanda_v20_token"
OANDA_ENV="practice"          # or "live"
```

Select `data_source=OANDA` in the dashboard (or `POST /api/config`). The feed
uses `XAU_USD` M15/H1/H4/D1 candles with `volume` = tick volume. OANDA has no
taker buy/sell split, so `volumeSource=OANDA_TICK`, delta/CVD is disabled, and
the AI treats volume as relative context only. If OANDA fails, the feed falls
back to TwelveData when `TWELVEDATA_API_KEY` is set, then Binance PAXG.

### Quantum weighted score v2 (A/B)

`quantum_weighted_score_enabled` defaults **false**. When enabled, scenario
confidence uses a 30/25/20/15/10 confluence model:

- trend alignment (D1/H4/H1 + EMA50 slope)
- key-level proximity + swing structure (HH/HL/LH/LL)
- volume surge
- RSI H1 range
- macro alignment

Anti-chase caps the score at 75 when price is more than `1.5× ATR` from EMA50
M15. A/B at 0.05R cost:

| Dataset | current | quantum_v2 |
|---|---:|---:|
| PAXG 2025 | +0.110R | **+0.123R** |
| Dukascopy 2025 | **+0.141R** | +0.137R |
| Dukascopy 2022–2023 | +0.031R | **+0.062R** |

The result is mixed, so the flag stays off until forward-test/approval. Turn it
on from the dashboard or `POST /api/config` only for an explicit A/B run.

### Trigger journal (analysis-only)

Every setup, trigger, AI review and hypothetical outcome is appended to
`data/trigger_journal.jsonl` (schema v1). It records triggers even when
confirmation/AI/risk rejected the signal, so raw trigger expectancy can be
measured without selection bias. Read-only endpoints:
`GET /api/trigger-journal?limit=200` and `GET /api/trigger-journal/summary`.
Run the report with
`cargo run --bin trigger_report -- --path data/trigger_journal.jsonl`
or, on the server, `docker compose exec api trigger_report --path data/trigger_journal.jsonl`.
See [`TRIGGER_JOURNAL_SPEC.md`](TRIGGER_JOURNAL_SPEC.md) for the schema,
outcome rules and pre-registered decision rule.

### BotState + always-on Market View (Phase 1)

- **NO_TRADE** — hard gates only: weekend/market closed, high-impact USD news
  ±`pre/post_news_buffer_minutes` (default 30), stale/fallback data, and the
  spread proxy `|live − last M15 close| / H4 ATR > max_spread_proxy_atr`
  (default 1.0). RSI/EMA/macro never silence the view.
- **MARKET_VIEW** — always available 24/5 on the dashboard via
  `GET /api/market-view`: structure (trend/pullback/range/reversal), PRIMARY and
  ALTERNATIVE zones, invalidation and risk notes. Levels come from the scenario
  builder; the view is a reference plan, not an entry.
- **SNIPER_TRIGGER** — a live confirmed signal exists; only this state can show
  a full GOLD SIGNAL with Entry/SL/TP/Lot after confirmation + AI + hard risk.
- **Session split** — the 13:00–23:00 session gate applies to sniper/watching
  only. Market View stays available outside the session.
- **Telegram** — `market_view_worker` sends on a new H1 close during the session,
  only when the view changed materially and after `market_view_cooldown_minutes`
  (default 120). Dashboard polling is every 60s.
- **AI roles** — Phase 1 does not change the Risk Officer prompt. The strategist
  prompt for Market View is Phase 2 and must not replace the risk veto.

### Two-level notification pipeline

- **SETUP WATCHING** — sent **once per setup** when price approaches a valid
  PRIMARY trigger (`action_watch_distance_atr`, default `1.0× H4 ATR`, capped at
  `action_watch_max_distance_points`, default 15 points; min PRIMARY confidence
  `action_watch_min_confidence`, default 60). It contains the zone, trigger,
  current price and distance only. It must not contain Entry/SL/TP/Lot and
  explicitly says `CHƯA phải entry`.
- **One-shot lifecycle** — the setup (scenario + frozen trigger/SL/TP +
  `created_at`) is persisted in `bot_state.json`. After one alert the worker
  stays silent until the setup triggers (scanner sends GOLD SIGNAL), reaches its
  structural stop, or expires after 8 M15 bars. The zone-bucket cooldown
  (`action_trigger_cooldown_minutes`, default 120) is a backstop. Watching
  alerts only run in the 13:00–23:00 GMT+7 session.
- **GOLD SIGNAL ALERT** — sent only after a closed M15 candle triggers the
  frozen watched setup, the confirmation gate passes, and AI + hard risk
  approve. Only this alert contains Entry/SL/TP/Lot.
- **AI prompt alignment** — when `early_entry_enabled=false`, the AI is told
  that pre-trigger early entry is disabled and a closed M15 trigger is the entry
  gate; scenario confidence is context, not a 90% permission threshold.
- **Fresh-candle rule** — the confirming M15 candle must be within
  `max_candle_age_secs` (180s in production) of its close. The setup scenario is
  built from the candles *before* the confirming candle; the same setup is
  passed to AI review. One live signal is allowed per candle timestamp.
- **Feed cadence** — the TwelveData M15 cache is 45s so the 60s feed tick
  fetches the just-closed bar instead of serving a 5-minute-old cache entry.
- `early_entry_enabled` defaults **false**; the scanner no longer sends a full
  early-entry signal before the M15 close. The field is kept only for the
  `/start-entry` scenario fallback.
- The old action-trigger alert with Entry/SL/TP is retired; the action worker
  now sends SETUP WATCHING alerts instead.

### Quantum trigger confirmation (P3, A/B)

`quantum_trigger_confirmation_enabled` defaults **false**; when enabled, the
candidate analyzer accepts bullish/bearish engulfing in addition to pinbar, and
the v2 score uses the M15 RSI/SMA14 cross plus
`quantum_volume_confirm_ratio` (default `1.2`).

The live tracker and the scenario backtest now share the same confirmation gate:
breakouts require a directional close plus volume expansion; limit/touch fills
require pinbar/engulfing plus volume. A/B at 0.05R cost:

| Dataset | current | quantum_v3 |
|---|---:|---:|
| PAXG 2025 | +0.110R · PF 1.19 · DD 36.21 | **+0.172R · PF 1.32 · DD 12.87** |
| Dukascopy 2025 | **+0.141R · PF 1.24 · DD 27.95** | +0.091R · PF 1.16 · DD 14.15 |
| Dukascopy 2022–2023 | +0.031R · PF 1.05 · DD 26.58 | **+0.077R · PF 1.14 · DD 13.72** |

`quantum_v3` produces ~40% fewer signals and much lower drawdown; Dukascopy
2025 average R is lower. Keep both flags off by default and enable them only
for a deliberate forward-test.

### Pending-order staleness guard

- A pending `LIMIT`/`BREAKOUT` signal is cancelled when price moves more than
  `1.5R` away from the entry before a fill, or after 8 closed M15 bars without
  a fill. The `cancel_reason` explains which rule fired.
- Early entry is only allowed when the live price is within
  `early_entry_max_distance_r` (default `1.0R`) of the scenario trigger.
  Otherwise the bot waits for a fresh scenario instead of creating a stale
  pending order.
- Telegram shows `PENDING` plus `KHÔNG market sell/chase` with the pending
  entry. The alert is a pending order, not a market fill.

### Signal journal cleanup (operator)

The signal journal is separate from the manual AI entry check. To close all
currently open (`Active`) signals after a test run:

```bash
curl -X POST http://127.0.0.1:8090/api/signals/close-all
# {"status":"ok","closed":5,"purged":0,"remaining":23}
```

To remove the whole test journal and start a clean forward-test baseline:

```bash
curl -X POST "http://127.0.0.1:8090/api/signals/close-all?purge=true"
```

Both endpoints only update the local journal; they never send Telegram, create
a signal or touch the scanner/quota. `POST /api/ai/entry-check` (and Telegram
`/start-entry`) is a view-only AI check: it does not insert a signal, does not
update the scanner's `last_ai_decision`, and returns `is_signal=false` /
`source=MANUAL_ENTRY_CHECK`.

## 16b. Divergence alerts and Live Setup preview

- `divergence_alert_enabled` (default true) sends standalone RSI divergence
  alerts for M15/H1/H4. They are advisory only, never consume the daily quota,
  and use a 60-minute cooldown per timeframe + direction.
- `GET /api/divergence-alerts` returns recent in-memory alerts.
- `GET /api/live-view` returns a read-only Entry/SL/TP/R:R preview (or
  `no_setup`); it never sends Telegram, stores a signal or consumes quota.
- The dashboard Live Setup card has the Start/Stop control for `is_active`.
- Macro/flow alignment (ALIGNED/COUNTER/MIXED/UNKNOWN) is attached to real
  signals and the Live Setup preview. It is advisory only: no code-level hard
  macro filter. The AI receives the macro/flow block and may
  lower confidence or STANDBY.
- Scenario View (`GET /api/scenarios`) returns four ranked reference scenarios
  even when the signal engine says `no_setup`. Ranking is transparent:
  `priority = confidence + 100 (valid) + 30/-30 (macro ALIGNED/COUNTER) + 20
  (H4 bias match) + 10 (R:R >= min) + 10 (price within 1.5×ATR of entry)`.
  The highest-priority idea is `PRIMARY`; the first opposite-bias idea is
  `ALTERNATE`; the rest are `WATCHLIST`. The dashboard shows two large cards plus
  a compact watchlist, and the Telegram summary uses the same three roles.
  Pressing Start (`POST /api/scanner/start`) sends the scenario summary to
  Telegram and returns the same view to the dashboard; it never inserts a signal
  or consumes quota. `POST /api/scenarios/telegram` resends on demand.
- Telegram commands are handled by a long-poll worker: `/start`, `/stop`,
  `/view`, `/start-entry`, `/status`, `/help`. Authorize one admin chat in
  `rust-backend/.env`:

  ```bash
  TELEGRAM_ADMIN_CHAT_ID="-1001234567890"
  ```

  If unset, a numeric `TELEGRAM_CHAT_ID` is used. With neither configured, the
  bot replies with the sender's chat ID and the env line to add, without running
  the command. `/start` sends the scenario summary and does **not** consume
  quota; `/stop` only pauses the scanner. `/start-entry` runs a manual AI entry
  check (AI action + reference plan) and does **not** insert a signal, send an
  automatic trade alert, consume quota or auto-trade. Commands are rate-limited
  to one per 2 seconds per chat (`/start-entry`: one per 30 seconds), and old
  pending updates are drained on startup.

## 16c. Action View

- `GET /api/action-view` returns one read-only verdict: `BUY`, `SELL`, `WAIT`
  or `NO_TRADE`, plus the live signal (when present), the primary scenario,
  macro/flow alignment, volume quality and a short Vietnamese reason.
- `volumeQuality` maps `BINANCE_PAXG_PROXY → PAXG_PROXY`,
  `BINANCE_PAXG → PAXG`, `NONE → NONE`, otherwise `UNKNOWN`. PAXG/PAXG-proxy
  responses include a warning that volume is a proxy, not real XAU/USD volume.
- The endpoint is **read-only and quota-free**: no Telegram send, no signal
  insertion/persistence, no AI review, no Sniper quota consumption and no
  auto-trading. It reuses the existing live-view/scenario builders and reports
  their filters; it does not add a hard macro filter or change analyzer logic.
- Proximity fields: `distanceToTrigger` (USD), `distanceAtr` (H4 ATR units) and
  `proximity` (`NEAR` ≤ 0.5 ATR, `MID` ≤ 1.5 ATR, `FAR` > 1.5 ATR, `UNKNOWN`).
  Current price priority is live signal entry → last close → scenario price;
  trigger priority is PRIMARY `trigger_price` → signal entry. The dashboard shows
  `Cách trigger: $25.46 · 0.85 ATR · MID` with a “chờ trigger, không đuổi” note.
- The dashboard Action View card polls the endpoint on load and every 30 seconds.
- When the **Sniper Signal Stream** is empty, it shows “Chưa có trade signal đã
  xác nhận” plus the Action View verdict and PRIMARY trigger/levels instead of a
  dead panel. The stream itself only lists confirmed trade signals.

## 16d. Action Trigger alerts

- `action_trigger_alert_enabled` (default true) runs a read-only worker every
  30 seconds. It alerts only when the PRIMARY scenario is `valid`, has a
  structured trigger price, and the latest **closed M15 candle** hits it:
  `ABOVE` = close above, `BELOW` = close below, `TOUCH` = wick range includes
  the level.
- Trigger levels are the same ones shown in Scenario View: breakout/breakdown
  use the buffered entry level; bounce/rejection use nearest support/resistance.
  Synthetic fallback feeds are ignored, and a candle older than
  `max_candle_age_secs` after close is skipped.
- Dedup key is `{scenario_id}:{direction}:{trigger_price}` with
  `action_trigger_cooldown_minutes` (default 30, range 5–1440). Cooldown state is
  in memory only (capped at 50 entries), so a restart may re-alert once.
- The Telegram message includes entry/SL/TP1/TP2/R:R and a PAXG volume-proxy
  caveat, and explicitly says it is a scenario trigger alert, **not a confirmed
  trade signal**.
- This path never inserts a signal, consumes the daily Sniper quota, persists
  trade state, calls AI review or auto-trades. `/api/health` exposes
  `actionTriggerAlertEnabled`; `/api/action-view` exposes
  `triggerAlertEnabled`.

## 16e. Daily Plan 07:00 (Telegram, read-only)

- At **07:00 server local time**, after the ForexFactory sync and the daily macro
  report update/persist, the scheduler sends one daily plan to Telegram when
  `daily_plan_enabled` is true (default). Toggle it in **Sniper & Risk Config**;
  `/api/health` exposes `dailyPlanEnabled`.
- The message contains the report date, macro bias/confidence/flow quality, D1/H4/H1
  bias + H4 ATR, up to four high-impact USD events, the same
  PRIMARY/ALTERNATE/WATCHLIST body as the Start-to-Telegram summary, and an
  `Action: WAIT/NO_TRADE` advisory footer. HTML is escaped and capped at 3,500
  characters.
- **Read-only/advisory**: it does not consume the Sniper quota, insert or persist a
  signal, change scanner state, call AI review or auto-trade. It is not a
  confirmed trade signal.
- If the Telegram token/chat is empty, the simulator returns `Ok(1001)` and logs
  that no message was sent; the scheduler continues normally.

## 16f. Weekend mode (Saturday/Sunday, Vietnam time)

- Gold is closed on Saturday/Sunday. A hard weekend gate runs independently of
  `session_filter_enabled`: no trade signals, action-trigger alerts, divergence
  alerts, session briefings or entry checks.
- `/start` enables the scanner but sends only a short market-closed message;
  `/view` and `/start-entry` reply market closed. `POST /api/ai/entry-check`
  returns HTTP 503 with the weekend reason.
- At **07:00 Sat/Sun**, the scheduler sends a weekend brief instead of the daily
  plan: next week's High-Impact USD events from ForexFactory
  `ff_calendar_nextweek.json`, macro/flow, D1/H4/H1 context, RSI, key S/R and a
  weekly bias assessment. It contains no entry/SL/TP and is not a trade signal.
- Toggle: `weekend_brief_enabled` (default true). `/api/health` exposes
  `marketOpen=false` and `weekendBriefEnabled` on weekends.
- The weekend news sync updates the news filter with next week's events so the
  Monday session starts with the correct calendar.
