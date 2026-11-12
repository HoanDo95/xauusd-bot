# Security — XAUUSD Bot

This bot is a single-operator signal system. The default deployment is reachable
only over a private Tailscale tailnet; that network boundary is **not** a
substitute for application authentication.

## Current controls

- Dashboard/API are bound to `127.0.0.1` on the host and exposed through nginx.
- Optional HTTP Basic auth for the dashboard/API:
  - Set `DASHBOARD_USER` and `DASHBOARD_PASSWORD` in the compose `.env`.
  - Webhooks and health endpoints are exempt; webhooks use the webhook passphrase.
  - Leave both unset to keep the previous Tailscale-only behavior.
- `GET/POST /api/config` redacts `telegram_bot_token` and `webhook_passphrase`.
  Blank or masked input keeps the stored secret; enter a new value to rotate it.
- Webhook passphrase comparison is constant-time. A long random passphrase is
  strongly recommended; an empty passphrase disables webhook authentication.
- `data/bot_state.json` and backups are written with mode `0600`; backup umask is
  `077`.
- nginx sends `X-Content-Type-Options`, `X-Frame-Options`, `Referrer-Policy`,
  `Permissions-Policy`, hides the nginx version, limits request bodies to 256k
  and rate-limits `/api/` to 10 r/s (burst 20).
- Telegram/TwelveData/OpenCode request URLs are redacted from logs and API errors.
- Docker: API runs with `cap_drop: [ALL]`, both services have `pids_limit` and
  `no-new-privileges`.

## Rotate immediately if a secret was exposed

A Telegram bot token that appears in chat, logs, screenshots, or a public repo is
compromised. Rotate it:

1. Open Telegram → `@BotFather` → `/revoke` → select the bot → copy the new token.
2. Update `rust-backend/.env` (`TELEGRAM_BOT_TOKEN=...`) **or** paste the new
   token into Dashboard → Sniper & Risk Config and save (the field is write-only).
3. Restart the API container:
   ```bash
   cd /home/jason/xauusd-bot
   docker compose restart api
   ```
4. Rotate the webhook passphrase with a long random value, update TradingView,
   and confirm `/api/health` shows `webhookPassphraseConfigured: true`.
5. Rotate TwelveData/OpenCode keys if they were ever printed.

## Deployment hardening checklist

- Keep the API/web ports bound to `127.0.0.1`; use Tailscale Serve for TLS.
- Enable `DASHBOARD_USER`/`DASHBOARD_PASSWORD` unless every tailnet device is trusted.
- Set a non-empty `WEBHOOK_PASSPHRASE` if TradingView webhooks are used.
- Do not enable Dozzle (Docker socket) unless it is authenticated and tailnet-only.
- Keep `.env`, `data/`, and `backups/` out of git; verify with `git status`.
- CI runs only on `push`/manual dispatch. Do not run fork/PR code on the
  self-hosted runner: it has Docker access and can read the deploy directory.
- Review `npm audit`/`cargo audit` periodically; rebuild images after base-image
  updates.

## Known gaps

- There is no per-user login or role system beyond optional nginx Basic auth.
- The legacy `server.ts` Express fallback is for local development; do not expose
  it on an untrusted network.
- Self-hosted GitHub Actions remain a supply-chain trust boundary. If the repo
  accepts external contributors, move PR CI to an isolated hosted runner.
