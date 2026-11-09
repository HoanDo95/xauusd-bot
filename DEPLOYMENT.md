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
