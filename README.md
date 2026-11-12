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
- **Daily Plan 07:00**: read-only Telegram summary with macro/news context and
  the PRIMARY/ALTERNATE/WATCHLIST view; never consumes quota or inserts a signal.
- **Sniper quota**: max 1 real signal/day (configurable 1–5), Vietnam trading day
  (`UTC+7`), candle dedup key, session filter 13:00–23:00 GMT+7.
- **Outcome tracking**: automatically updates `Active → Target1Hit / Target2Hit /
  StopLossHit` from subsequent closed candles, with conservative same-candle
  handling.
- **Performance stats**: live win rate, average R, net R, profit factor; simulated
  signals are excluded.
- **Telegram alerts**: HTML format with entry, SL, TP1, TP2, R:R, lot size, volume
  rationale, and a risk warning.
- **Telegram commands**: authorized admin chat can use `/start`, `/stop`, `/view`,
  `/status`, `/help`; `/start` sends the reference scenario view without spending
  quota. Set `TELEGRAM_ADMIN_CHAT_ID` (falls back to a numeric `TELEGRAM_CHAT_ID`).
- **Outcome alerts**: follow-up Telegram messages when TP1, TP2, or SL is hit.
- **Dashboard**: dark trading UI, live candles, quota status, signal history,
  config, TradingView webhook sandbox.

## Thuật ngữ

- **Sniper Signal Stream** — chỉ hiển thị **trade signal đã xác nhận** (đã qua
  đủ filter và được ghi vào danh sách tín hiệu). Kịch bản tham khảo không xuất
  hiện trong stream này.
- **Action View** — trả lời câu hỏi “có nên vào lệnh bây giờ?” với một trong bốn
  verdict `BUY` / `SELL` / `WAIT` / `NO_TRADE`. Đây là view read-only, không phải
  lệnh tự động.
- **Scenario View** — các kế hoạch tham khảo `PRIMARY` / `ALTERNATE` /
  `WATCHLIST`, kèm trigger, Entry/SL/TP và R:R. Không phải trade signal.
- **Trigger** — điều kiện kích hoạt kịch bản, ví dụ `M15 close < 3916.51`
  (ABOVE/BELOW dùng giá đóng nến, TOUCH dùng biên độ râu nến).
- **Entry / SL / TP1 / TP2 / R:R** — giá vào, stop loss, hai mục tiêu chốt lời và
  tỷ lệ lợi nhuận/rủi ro.
- **Macro / Flow** — bối cảnh vĩ mô (FRED/Yahoo/CFTC) và dòng tiền; chỉ tư vấn,
  không phải bộ lọc cứng.
- **Divergence** — phân kỳ RSI giữa giá và động lượng; cảnh báo độc lập, không
  phải tín hiệu vào lệnh.
- **Volume proxy** — volume PAXG dùng thay cho volume XAU/USD thật; chỉ nên xem
  là tương đối.

## Free data sources (no paid API keys)

| Data | Source | Key |
|---|---|---|
| Gold OHLCV + volume (default) | Binance `PAXGUSDT` public API | none |
| Gold OHLC + volume proxy | TwelveData `time_series` XAU/USD + Binance `PAXGUSDT` volume | optional `TWELVEDATA_API_KEY` |
| Spot `XAU/USD` reference (basis only) | TwelveData free `/price`, fallback `gold-api.com` | optional |
| US high-impact calendar | ForexFactory public JSON | none |
| Trend | EMA50/200 computed locally | none |
| Daily macro commentary (optional) | OpenCode Go (default), Gemini or OpenRouter | optional |
| AI decision layer (default `DECISION`, legacy `VETO_ONLY`) | OpenCode Go, default `gemini-4-argon` | optional (key required) |
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

### AI Decision Mode (default) + legacy VETO_ONLY

`ai_signal_review_enabled` defaults to **ON** for new/default configs; an
existing `data/bot_state.json` that explicitly stored `false` stays off until
you change it. `ai_review_mode` defaults to **`DECISION`** and accepts
`DECISION` or `VETO_ONLY` (invalid values sanitize to `DECISION`).

**`DECISION` — AI is the primary decision layer.** The Rust engine still owns
every hard risk rule; the AI chooses the direction:

- Candidate discovery is deliberately **relaxed**: `volume_filter_enabled=false`
  and `macro_bias="NEUTRAL"` for the analysis pass, so AI can review setups that
  the old soft filters would have rejected.
- The AI receives the full collected brief: current price/source/volume caveat,
  D1/H4/H1 bias/EMA/channel/ATR/S-R, Macro & Flow scorecard (real yield, DXY,
  gold, GLD/IAU, COT, VIX/GVZ, MMF), RSI M15/H1/H4, Volume Delta/CVD with the
  PAXG-proxy warning, PRIMARY/ALTERNATE/WATCHLIST scenarios, the candidate
  setup, upcoming High-Impact USD events, and Vietnam session time.
- It returns `BUY`, `SELL` or `STANDBY`, `confidence_score`, a selected
  scenario and a short reason. A `STANDBY`, opposite direction, or confidence
  below `ai_signal_review_min_confidence` means **no trade signal**.
- After AI approval, the soft HTF/Volume Delta/RSI filters are **skipped** (the
  AI already weighed them as context). Hard checks still run: SL/TP ordering,
  minimum R:R, risk budget, daily quota, session window, news kill switch and
  dedup. The AI never sets or changes live SL/TP.
- **Fail-closed on AI failure**: if `OPENCODE_GO_API_KEY` is missing, the
  provider errors/times out, or the JSON is malformed, there is no trade signal
  in `DECISION` mode regardless of `ai_signal_review_fail_open`.
- **Start** runs one AI decision and sends an `AI MARKET BRIEF` to Telegram with
  the action/confidence/selected scenario/reason plus the compact scenario body.
  Start still never inserts a signal or consumes quota. If AI is unavailable it
  sends the scenario summary and returns `aiError`; it does not block Start.
  The response includes `aiDecision` and `aiError`, and `/api/action-view`
  exposes the latest decision as `aiDecision`.
- One AI decision call per real signal attempt (quota still caps real signals at
  1/day by default). Simulated setups never call the AI and never consume quota.
- Accepted signals carry `ai_verdict`, `ai_confidence` and `ai_reason`.

**`VETO_ONLY` — legacy behaviour.** The AI reviews each real deterministic
signal and may only approve, contradict or return `STANDBY`; an opposite
direction or low confidence blocks the signal. `ai_signal_review_fail_open`
(default `true`) applies **only** here: AI error/timeout lets the deterministic
signal proceed, and setting it to `false` makes VETO_ONLY fail-closed. Start
keeps sending the plain scenario summary.

**RSI 70/30 meaning:** `RSI >= 70` is **Overbought** and `RSI <= 30` is
**Oversold**. These flag an extended move, not an automatic reversal; a
bullish/bearish divergence is a warning, not an entry. RSI remains context/AI
input unless the optional hard `rsi_filter_enabled` gate is explicitly turned
on.

Manual test endpoint: `POST /api/ai/review-signal` with
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
- Both AI modes receive the RSI block plus the 3-layer rules (H4/H1
  supply/demand confluence + RSI extreme/divergence + volume confirmation).
  Missing or contradictory layers should push the AI to `STANDBY`. In
  `DECISION` mode RSI is context the AI weighs; in `VETO_ONLY` it can still only
  veto. Neither mode ever sets live SL/TP.
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
- The AI prompt includes the macro/flow block and may lower confidence or choose
  `STANDBY` when the setup is counter-macro.
- `GET /api/live-view` returns the same alignment in its `macroFlow` block; the
  Live Setup card shows an ALIGNED/COUNTER/MIXED/UNKNOWN chip.
- **No hard macro filter**: alignment is advisory only; it does not block a
  signal in code. AI veto remains the only macro-related block.

### Scenario View + Start-to-Telegram (Phase C4)

The dashboard has a **Scenario View** card with four reference scenarios even
when the strict signal engine says `no_setup`:

```text
Bullish breakout · Bearish breakdown · Bullish bounce · Bearish rejection
```

The API ranks them transparently instead of showing four equal ideas:

- `priority = confidence`
  `+100` when `valid`, `+30`/`-30` for macro ALIGNED/COUNTER, `+20` when the
  scenario bias matches H4, `+10` for R:R ≥ `min_risk_reward`, and `+10` when
  price is within `1.5 × ATR` of the entry.
- Sorted by priority, then confidence, then validity.
- `PRIMARY` is the highest-priority scenario. `ALTERNATE` is the first
  subsequent scenario with the opposite bias (or the second scenario). All
  remaining ideas are `WATCHLIST`.
- Every scenario still shows trigger, entry, SL, TP1/TP2, R:R, invalidation,
  confidence, macro/flow alignment and `VALID`/`NOT VIABLE`. The card shows
  PRIMARY/ALTERNATE as two large cards and WATCHLIST as a compact expandable
  list. This is read-only reference planning, not a confirmed signal; the card
  never inserts a signal or consumes quota.

Pressing **Start** now:

1. Enables `is_active`.
2. Builds the live view + scenario view immediately.
3. In `DECISION` mode, runs one AI decision from the full brief. In `VETO_ONLY`
   mode it does not call the AI on Start (legacy behaviour).
4. Sends Telegram: `DECISION` sends an **AI MARKET BRIEF** with the AI verdict
   plus the compact PRIMARY/ALTERNATE/WATCHLIST body; if AI is unavailable it
   sends the scenario summary and returns `aiError`. `VETO_ONLY` sends the plain
   scenario summary. Start never inserts a signal or consumes quota.
5. Returns the same view plus `aiDecision`/`aiError` to the dashboard.

The Scenario View card also has **Gửi Telegram** to resend the current snapshot
on demand. Stop only pauses the scanner; it does not close open signals or reset
quota.

### Action View — “Should I enter now?”

The monitor tab starts with an **Action View** card backed by
`GET /api/action-view`. It combines the live setup, the primary reference
scenario and the macro/flow snapshot into one read-only verdict:

| Verdict | Meaning |
|---|---|
| `BUY` / `SELL` | A live setup exists and the HTF/RSI/Delta display filters all pass. Entry, SL, TP1/TP2, R:R and lot size are shown. |
| `WAIT` | A setup exists but at least one filter is blocked (`FILTER_BLOCKED`), or `no_setup` with a valid primary scenario — wait for the trigger. |
| `NO_TRADE` | No valid scenario, or not enough candles (`no_data`). |

- `caution` is `COUNTER_MACRO` when macro/flow opposes the signal or primary
  scenario, `FILTER_BLOCKED` when a display filter blocks the setup, and
  `VOLUME_PROXY` for an actionable signal whose volume is a PAXG proxy.
- `volumeQuality` maps `BINANCE_PAXG_PROXY → PAXG_PROXY`,
  `BINANCE_PAXG → PAXG`, `NONE → NONE`, otherwise `UNKNOWN`. For PAXG/PAXG
  proxy the API also returns: “Volume là proxy PAXG, không phải volume XAUUSD
  thật; chỉ nên xem là tương đối.”
- Proximity fields: `distanceToTrigger` (USD), `distanceAtr` (H4 ATR units) and
  `proximity` (`NEAR` ≤ 0.5 ATR, `MID` ≤ 1.5 ATR, `FAR` > 1.5 ATR, `UNKNOWN`
  when price/trigger/ATR is missing). The card shows
  `Cách trigger: $25.46 · 0.85 ATR · MID` with green/amber/rose tones and the
  note “NEAR = có thể canh; MID/FAR = chờ trigger, không đuổi.”
- The **Sniper Signal Stream** empty state reuses Action View + the PRIMARY
  scenario to show what to wait for instead of a dead panel. It states that the
  stream only lists confirmed trade signals and points to Action View / Scenario
  View for reference plans.
- In `DECISION` mode the response also includes the latest cached
  `aiDecision` (action/confidence/selected scenario/reason). The Action Card
  shows it prominently but keeps the deterministic verdict visible; when the AI
  action differs from a `WAIT`/`NO_TRADE` verdict both labels are shown and the
  AI never silently overrides the code verdict.
- The endpoint is **read-only**: no Telegram, no signal insertion, no Sniper
  quota, no AI call (it only reads the last cached decision) and no
  auto-trading. It never blocks a real signal beyond the existing scanner
  filters; it only reports them. The card is labelled “View hành động — không
  phải lệnh tự động; bot không tự vào lệnh.”

### Daily Plan 07:00 (Telegram, read-only)

Once a day at **07:00 server local time**, after the news sync and macro report
update, the scheduler sends a read-only daily plan to Telegram when
`daily_plan_enabled` is true (default):

```text
📅 KẾ HOẠCH NGÀY 2026-11-12 · XAU/USD
Macro: BEARISH · 95% · CONFIRMING
D1 BEARISH · H4 BEARISH · H1 BEARISH · ATR H4 30.0
Tin đỏ USD: CPI 20:30 · NFP 21:30 (tối đa 4 tin)

🎯 PRIMARY — …
🔁 ALTERNATE — …
👀 Watchlist: …

Action: WAIT — chờ trigger của PRIMARY.
Đây là kế hoạch tham khảo, không phải trade signal đã xác nhận.
```

- Uses the `DailyMacroReport` already computed at 07:00 (date, macro bias,
  confidence, high-impact USD events) plus the current live view and scenarios.
- Reuses the same PRIMARY/ALTERNATE/WATCHLIST body as the Start-to-Telegram
  summary, escaped HTML and capped at 3,500 characters.
- **Read-only/advisory**: no quota consumption, no signal insertion, no scanner
  state change, no AI review and no auto-trading.
- Toggle: `daily_plan_enabled` (default true) in **Sniper & Risk Config**.
  `/api/health` exposes `dailyPlanEnabled`. With an empty Telegram token/chat the
  simulator returns `Ok(1001)` and logs that no message was sent.

### Action Trigger alerts (Phase C7)

The PRIMARY reference scenario now carries a structured trigger
(`trigger_price`, `trigger_direction`, `trigger_timeframe`) and a read-only
worker checks the latest closed M15 candle every 30 seconds:

- `ABOVE` fires when the candle close is above the trigger price, `BELOW` when
  the close is below it, and `TOUCH` when the candle's wick range includes the
  level.
- Trigger levels match Scenario View: breakout/breakdown use the buffered entry
  level, while bounce/rejection use the nearest support/resistance.
- Only a `valid` PRIMARY scenario with a trigger price is eligible. Fallback or
  unavailable feeds are ignored, and a candle older than
  `max_candle_age_secs` after its close time is skipped.
- Dedup key: `{scenario_id}:{direction}:{trigger_price}`. The cooldown is
  `action_trigger_cooldown_minutes` (default 30, range 5–1440). State is kept in
  memory only (max 50 entries).
- Config toggles: `action_trigger_alert_enabled` (default true) and
  `action_trigger_cooldown_minutes` (default 30). `/api/health` exposes
  `actionTriggerAlertEnabled`, and `/api/action-view` exposes
  `triggerAlertEnabled` for the Action View card.
- The Telegram message states clearly that it is a **scenario trigger alert,
  not a confirmed trade signal**. It never inserts a signal, consumes Sniper
  quota, persists trade state, calls AI review or auto-trades.

### Telegram bot commands

The API runs a long-poll command worker. Commands are accepted only from the
admin chat:

```bash
# rust-backend/.env
TELEGRAM_ADMIN_CHAT_ID="-1001234567890"
```

If `TELEGRAM_ADMIN_CHAT_ID` is empty, a numeric `TELEGRAM_CHAT_ID` is used as the
fallback. When neither is available, the bot replies with the sender's chat ID
and the env line to add; it does not execute the command.

| Command | Behaviour |
|---|---|
| `/start` | Enable the scanner, send the scenario summary, then reply. Does **not** consume quota or insert a signal. |
| `/stop` | Pause the scanner. Open signals and quota are untouched. |
| `/view` | Re-send the current live view + scenario summary. |
| `/status` | Compact status: scanner, data/volume source, feed age, macro, quota, divergence, AI review. |
| `/help` | List commands. |

Commands support a `@botname` suffix (for example `/status@MyGoldBot`), are
rate-limited to one per 2 seconds per chat, and unknown commands return `/help`.
The worker drains old pending updates on startup and advances the update offset
for every update so commands are not replayed.

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
  telegram_commands.rs  market_context.rs  rsi.rs  volume_delta.rs  signal_review.rs
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
| `GET` | `/api/health` | Health, live price, feed/volume source, spot basis/price source, daily quota, kill switch, `aiReviewMode` |
| `GET` | `/api/candles` | Live + last closed price, S/R zones, EMA50/200, trend, `dataSource`, `volumeSource`, `spotPrice`, `spotBasis`, `priceSource` |
| `GET` | `/api/context` | Read-only D1/H4/H1 context + `rsi_m15`/`rsi_h1`/`rsi_h4` (204 until HTF ready) |
| `GET` | `/api/signals` | Signal history + performance stats |
| `GET` | `/api/stats` | Win rate, avg R, net R, profit factor (real signals only) |
| `GET/POST` | `/api/config` | Read/update bot config (secrets redacted; blank keeps current) |
| `POST` | `/api/calculate-lot` | Lot size calculator; returns snake_case + camelCase fields |
| `GET` | `/api/news-calendar` | ForexFactory events + kill switch |
| `POST` | `/api/simulate-setup` | Simulated signal (never consumes live quota) |
| `POST` | `/api/ai/review-signal` | Manual AI Risk Officer review (no Telegram, no quota) |
| `GET` | `/api/scenarios` | Four ranked reference scenarios (`PRIMARY`/`ALTERNATE`/`WATCHLIST`); no quota |
| `GET` | `/api/live-view` | Read-only setup preview (Entry/SL/TP/R:R) or `no_setup`; no quota |
| `GET` | `/api/action-view` | Read-only BUY/SELL/WAIT/NO_TRADE verdict + PRIMARY proximity + latest `aiDecision`; no quota |
| `POST` | `/api/scanner/start` | Enable scanner + DECISION AI brief / VETO_ONLY scenario summary (no signal/quota) |
| `POST` | `/api/scanner/stop` | Pause scanner (does not close open signals or reset quota) |
| `POST` | `/api/scenarios/telegram` | Re-send the reference scenario snapshot (no quota) |
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
