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
                     - scanner 24/7                                       - serves Vite dist/
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

- `./data/bot_state.json` — config + signal history + daily quota/dedup state.
- `./rust-backend/.env` — Telegram token/chat ID and runtime config.
- `./data` must be owned by UID 1000 (compose runs the API as `1000:1000`).

## 9. Optional log UI (Dozzle)

For browser-based log tracing without SSH:

```bash
docker compose -f docker-compose.yml -f docker-compose.observability.yml up -d dozzle
```

Then open `http://xauusd-bot:8081` inside the tailnet.
Dozzle mounts the Docker socket read-only; keep it tailnet-only.

## 10. Tailscale Serve

The host Tailscale Serve proxies `https://xauusd-bot.tailec308e.ts.net/` →
`http://127.0.0.1:3000` (the `web` container). It is tailnet-only, no public
exposure. If you later need a public TradingView webhook, enable Tailscale
Funnel explicitly or put Cloudflare Tunnel in front.

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

`data/bot_state.json` holds config, signal history, quota and dedup state.
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
snapshots. `backups/` is excluded from deploys and git.

## 13. Webhook passphrase

Set `webhook_passphrase` in **Sniper & Risk Config**. When non-empty, every
TradingView alert must include the same value:

```json
{"passphrase":"<your-secret>","ticker":"XAUUSD","action":"BUY","price":4080.5}
```

The dashboard webhook sandbox and Pine Script sample read the same config value.
Leave it empty to disable the check (not recommended for public endpoints).

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

## 15. Runtime config vs environment

Entry and session behaviour is **dashboard config**, persisted in
`data/bot_state.json`, not environment variables:

- `entry_mode` (`MARKET` / `LIMIT_RETEST` / `BREAKOUT`), `broker_price_offset`,
  `max_entry_drift_points`, `max_candle_age_secs`.
- `session_alerts_enabled`, `session_alert_lead_minutes`, `session_filter_enabled`.

Change them in **Sniper & Risk Config** and save; no container restart is needed.
Environment variables are only for secrets/runtime wiring (Telegram, webhook
passphrase, TwelveData key, AI provider/model, `PORT`, `RUST_LOG`). After editing
`rust-backend/.env`, restart the API so the new environment is loaded:

```bash
docker compose restart api
docker compose logs -f api
```

## 16. PAXG → XAU/USD spot basis calibration

Binance `PAXGUSDT` remains the only OHLCV + volume feed. `TWELVEDATA_API_KEY` is
optional and now powers **only** the XAU/USD spot reference used to calibrate the
PAXG premium/discount; TwelveData `time_series` has no volume and is not used for
candles. The spot quote is cached 300s (~288 calls/day) and falls back to
`https://api.gold-api.com/price/XAU` when TwelveData fails.

Set it in `rust-backend/.env`, then restart the API so the environment is reloaded:

```bash
# rust-backend/.env
TWELVEDATA_API_KEY="your_twelvedata_api_key_here"

docker compose restart api
docker compose logs -f api   # look for: 🌐 Spot basis: spot=..., basis=+...
```

`use_auto_spot_basis` (default true) is dashboard config in
`data/bot_state.json`; no restart is needed to toggle it. A legacy persisted
`data_source = "TWELVEDATA"` is migrated back to `BINANCE_PAXG` on startup.
`broker_price_offset` still applies on top. If both spot providers fail,
`spotBasis` is `null` and the engine continues with Binance PAXG + the manual
offset.
