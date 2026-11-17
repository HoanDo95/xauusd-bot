# Trigger Journal Spec v1

## Purpose

The trigger journal answers one question:

> **Did the trigger actually work, regardless of whether confirmation, AI or
> risk approved a signal?**

It is an append-only analysis log, not live trade state. It never sends
Telegram, inserts a signal, consumes quota or places an order.

## File

- Path: `data/trigger_journal.jsonl`
- One JSON object per line.
- Append-only. Do not edit historical lines; write a new event instead.
- Schema version: `1` on every line.
- Rotation: keep the file under ~50 MB (roughly tens of thousands of lines).
  Archive older lines if needed; the report reads the active file.

## Event types

| Event | When | Key fields |
|---|---|---|
| `scenario_observed` | Once per M15 close, even outside session | scenario, trigger, confidence, valid, distance, session, regime |
| `setup_created` | A SETUP WATCHING alert was sent; levels are frozen | setup_id, scenario, entry/SL/TP, distance, watch_distance |
| `setup_closed` | Watched setup expired/invalidated/triggered before a signal | setup_id, reason, bars_open |
| `triggered` | M15 close crossed the frozen trigger | setup_id, signal_id, close, fill, confirmation_pass/reason |
| `ai_review` | AI Decision returned or was unavailable | setup_id, ai_action, ai_confidence, ai_model, ai_blocked, ai_reason |
| `outcome` | Hypothetical TP1/SL/TIMEOUT completed | setup_id, outcome, r_multiple, MFE/MAE at 1/4/8/24 bars |

`triggered` is recorded **even when confirmation fails or AI returns STANDBY**.
That is the point: the raw trigger must be measurable.

## Outcome rules

- Start from the M15 candle **after** the confirming candle. The confirming
  candle's close is the decision point; the trade is forward from there.
- Forward window: **24 M15 bars** (6 hours).
- SELL: TP1 hit when `low <= take_profit_1`; SL hit when `high >= stop_loss`.
- BUY: TP1 hit when `high >= take_profit_1`; SL hit when `low <= stop_loss`.
- If TP1 and SL are both inside the same candle, count **SL first**
  (conservative).
- `TIMEOUT` marks the close at bar 24.
- `r_multiple`:
  - TP1 = `|TP1 − entry| / |entry − SL|` (positive)
  - SL = `-1.0`
  - TIMEOUT = signed close mark-to-market / risk
- `DATA_GAP` means the trigger candle is no longer in the candle history; it is
  excluded from win-rate/expectancy.
- `MFE/MAE` are favorable/adverse excursions in R at 1/4/8/24 bars. A missing
  checkpoint means the outcome closed before that bar.

## Metrics

- **Watching trigger rate** = watched setups whose frozen trigger was hit /
  watched setups created.
- **Scanner-only triggers** = triggers without a prior SETUP WATCHING alert.
- **Win rate** = TP1 / (TP1 + SL + TIMEOUT).
- **Avg R / expectancy** = mean `r_multiple` over completed outcomes.
- **Profit factor** = sum(positive R) / |sum(negative R)|. `n/a` when there are
  no losses.
- **Max drawdown (R)** = peak-to-trough of cumulative R, ordered by trigger
  time.
- **AI confidence buckets**: `<0.60`, `0.60–0.70`, `0.70–0.80`, `0.80–0.90`,
  `>=0.90`, `NO_AI`.
- **Confirmation buckets**: `CONFIRMED` / `REJECTED`.

## Decision rule (pre-registered)

Do not tune parameters mid-test. After **100–200 completed trigger outcomes**
(or 6–8 weeks, whichever comes first):

1. Keep the current spec if the **95% bootstrap CI of expectancy after 0.05R
   cost is above 0** and max drawdown is within the agreed cap (e.g. 15R).
2. If the point estimate is positive but the CI includes 0, extend to the next
   pre-registered sample size; do not change rules.
3. If expectancy is negative, prune the negative buckets (scenario, regime,
   session, AI confidence) and re-test.
4. If AI confidence buckets are not monotonic (e.g. `>=0.90` does not beat
   `0.70–0.80`), stop using confidence as a gate and keep AI as context/veto.
5. No auto-trade until the forward test passes.

## API

```text
GET /api/trigger-journal?limit=200
GET /api/trigger-journal/summary
```

Both are read-only. The summary is computed from the JSONL file on request.

## Report CLI

```bash
cd rust-backend
cargo run --bin trigger_report -- --path data/trigger_journal.jsonl

# On the production server (the report binary is in the API image):
docker compose exec api trigger_report --path data/trigger_journal.jsonl
```

The report prints the overview, buckets by scenario/regime/session/AI
confidence/confirmation, and the methodology.

## Change control

- Adding optional fields: keep schema `1`.
- Changing event meaning, outcome rules or metric definitions: bump to schema
  `2` and document the migration.
- Never rewrite historical outcomes to make a bucket look better.
