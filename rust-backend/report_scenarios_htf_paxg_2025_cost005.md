# Scenario / Early-Entry Backtest

Period: **2025-01-01** to **2025-12-31** (UTC) · symbol **PAXGUSDT** · interval **15m**
Data: Binance public klines (PAXGUSDT XAU/USD proxy) cached at `/home/jason/Downloads/Be-foundation/tool-xauusd/gold-signal-bot/rust-backend/data/backtest/PAXGUSDT_M15.jsonl` (175329 candles loaded, 35040 in period, 0 missing M15 bars).

Command: `target/release/backtest_scenarios --start 2025-01-01 --end 2025-12-31 --cost-r 0.05 --out-json report_scenarios_htf_paxg_2025_cost005.json --out-md report_scenarios_htf_paxg_2025_cost005.md`
Cost assumption: **0.05R per trade** (already subtracted from all R metrics).

## Fill model

| Rule | Value |
| --- | --- |
| TOUCH/LIMIT fill window | 8 closed M15 bars after the trigger |
| BREAKOUT gap fill | Trigger-candle open only when slippage <= 0.5R |
| Fill-bar handling | Simulation starts on the M15 bar after the fill bar. |
| Same-candle rule | SL wins when SL and TP print in the same M15 candle. |
| Summary | TOUCH/LIMIT entries must trade at the scenario entry within 8 M15 bars; BREAKOUT entries fill at the level, or at the trigger-candle open on a gap <= 0.5R, otherwise skip. |

## Variant comparison

| Variant | Anti-chase | Skip COUNTER | Description |
| --- | --- | --- | --- |
| legacy | ON | no | M15-only levels; no SL cap; bearish_rejection ON; no BUY RSI H1 guard; all valid PRIMARY. |
| htf_only | ON | no | HTF-merged levels; no SL cap; bearish_rejection OFF; BUY RSI H1 guard ON. |
| sl_cap_only | ON | no | M15-only levels; SL cap 1×ATR; bearish_rejection OFF; BUY RSI H1 guard ON. |
| current | ON | no | HTF-merged levels; SL cap 1×ATR; bearish_rejection OFF; BUY RSI H1 guard ON. |
| anti_chase_off | OFF | no | HTF-merged levels; SL cap 1×ATR; anti-chase OFF; BUY RSI H1 guard ON. |
| aligned_only | ON | yes | HTF-merged levels; SL cap 1×ATR; skip COUNTER PRIMARY. |
| aligned_only_no_anti_chase | OFF | yes | HTF-merged levels; SL cap 1×ATR; skip COUNTER PRIMARY; anti-chase OFF. |

| Variant | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Expectancy | PF | Max DD (R) | Total R | TP2 before SL | Timeout rate |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| legacy | 621 | 164 | 329 | 128 | 26.4% | 0.037 | 0.037 | 1.06 | 35.28 | 22.80 | 16.6% | 20.6% |
| htf_only | 598 | 181 | 321 | 96 | 30.3% | 0.096 | 0.096 | 1.17 | 35.31 | 57.27 | 19.6% | 16.1% |
| sl_cap_only | 630 | 191 | 331 | 108 | 30.3% | 0.112 | 0.112 | 1.20 | 18.01 | 70.80 | 20.3% | 17.1% |
| current | 607 | 189 | 329 | 89 | 31.1% | 0.110 | 0.110 | 1.19 | 36.21 | 66.46 | 20.9% | 14.7% |
| anti_chase_off | 607 | 189 | 329 | 89 | 31.1% | 0.110 | 0.110 | 1.19 | 36.21 | 66.46 | 20.9% | 14.7% |
| aligned_only | 607 | 189 | 329 | 89 | 31.1% | 0.110 | 0.110 | 1.19 | 36.21 | 66.46 | 20.9% | 14.7% |
| aligned_only_no_anti_chase | 607 | 189 | 329 | 89 | 31.1% | 0.110 | 0.110 | 1.19 | 36.21 | 66.46 | 20.9% | 14.7% |

### Fill accounting

| Variant | Filled at level | Gapped at open | Skipped, no fill | Counter skipped | Entry outside trigger range |
| --- | ---: | ---: | ---: | ---: | ---: |
| legacy | 621 | 0 | 778 | 0 | 980 |
| htf_only | 597 | 1 | 548 | 0 | 690 |
| sl_cap_only | 630 | 0 | 771 | 0 | 969 |
| current | 606 | 1 | 552 | 0 | 695 |
| anti_chase_off | 606 | 1 | 546 | 0 | 688 |
| aligned_only | 606 | 1 | 552 | 0 | 695 |
| aligned_only_no_anti_chase | 606 | 1 | 546 | 0 | 688 |

## Current variant headline

| Metric | Value |
| --- | ---: |
| Trades | 607 |
| Wins (TP1) | 189 |
| Losses (SL) | 329 |
| Timeouts | 89 |
| Win rate | 31.1% |
| Avg R | 0.110 |
| Expectancy (R/trade) | 0.110 |
| Profit factor | 1.19 |
| Max drawdown (R) | 36.21 |
| Total R | 66.46 |
| TP2 before SL | 20.9% (127 of 607 trades) |
| Timeout rate | 14.7% |

**Uncertainty:** avg R 95% CI [0.000, 0.219] · t=1.96 · SE=0.056 · n=607.

### Funnel

| Stage | Count |
| --- | ---: |
| M15 bars in period | 35040 |
| Bars skipped while a trade was open | 25904 |
| Setups evaluated | 9136 |
| PRIMARY valid with levels | 8931 |
| PRIMARY skipped by aligned_only (COUNTER) | 0 |
| BUY skipped by RSI H1 < 30 | 574 |
| Trigger found within 8 bars | 1159 |
| Entries filled at level | 606 |
| Entries gapped (filled at open) | 1 |
| Entries skipped, no fill | 552 |
| ... of which gap > 0.5R | 0 |
| Entry inside trigger candle range | 464 |
| Entry outside trigger candle range (diagnostic) | 695 |
| Skipped (no forward data) | 0 |
| Period-end forced exits | 1 |
| Trades simulated | 607 |

## By scenario (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| bearish_breakdown | 36 | 1 | 25 | 10 | 2.8% | -0.490 | 0.34 | 17.65 | 0.0% |
| bullish_bounce | 548 | 182 | 291 | 75 | 33.2% | 0.154 | 1.27 | 25.54 | 22.1% |
| bullish_breakout | 23 | 6 | 13 | 4 | 26.1% | -0.020 | 0.97 | 5.25 | 26.1% |

## By scenario (aligned_only)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| bearish_breakdown | 36 | 1 | 25 | 10 | 2.8% | -0.490 | 0.34 | 17.65 | 0.0% |
| bullish_bounce | 548 | 182 | 291 | 75 | 33.2% | 0.154 | 1.27 | 25.54 | 22.1% |
| bullish_breakout | 23 | 6 | 13 | 4 | 26.1% | -0.020 | 0.97 | 5.25 | 26.1% |

## By session (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| OUTSIDE | 338 | 104 | 182 | 52 | 30.8% | 0.114 | 1.20 | 35.27 | 19.5% |
| VN_13_23 | 269 | 85 | 147 | 37 | 31.6% | 0.104 | 1.18 | 13.81 | 22.7% |

## By macro bias (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| BEARISH | 36 | 1 | 25 | 10 | 2.8% | -0.490 | 0.34 | 17.65 | 0.0% |
| BULLISH | 571 | 188 | 304 | 79 | 32.9% | 0.147 | 1.26 | 22.76 | 22.2% |

## By macro alignment (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| ALIGNED | 607 | 189 | 329 | 89 | 31.1% | 0.110 | 1.19 | 36.21 | 20.9% |

## By RSI H1 bucket (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 30-65 | 471 | 123 | 271 | 77 | 26.1% | -0.025 | 0.96 | 47.49 | 15.9% |
| <30 | 2 | 0 | 2 | 0 | 0.0% | -1.050 | 0.00 | 2.10 | 0.0% |
| >65 | 134 | 66 | 56 | 12 | 49.2% | 0.599 | 2.36 | 5.10 | 38.8% |

## By trigger direction (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| ABOVE | 23 | 6 | 13 | 4 | 26.1% | -0.020 | 0.97 | 5.25 | 26.1% |
| BELOW | 36 | 1 | 25 | 10 | 2.8% | -0.490 | 0.34 | 17.65 | 0.0% |
| TOUCH | 548 | 182 | 291 | 75 | 33.2% | 0.154 | 1.27 | 25.54 | 22.1% |

## By early-entry confidence (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| <90 | 409 | 134 | 224 | 51 | 32.8% | 0.130 | 1.22 | 30.91 | 23.7% |
| >=90 | 198 | 55 | 105 | 38 | 27.8% | 0.067 | 1.12 | 12.71 | 15.2% |

## Caveats

- PAXGUSDT is a free proxy for XAU/USD; the PAXG premium/discount is not modeled.
- The macro layer is deterministic only: D1 EMA bias, confidence 80, score 0, no metrics/COT/news and no AI review.
- One open trade at a time; new setups are skipped while a trade is open, and setup evaluation resumes on the M15 bar after the exit bar.
- TOUCH/LIMIT entries require the scenario entry level to trade in the next 8 closed M15 bars after the trigger; if it never trades, the trade is skipped.
- BREAKOUT entries fill at the scenario entry level; if the trigger candle opens beyond the level, they fill at the candle open only when slippage is <= 0.5R, otherwise they are skipped.
- Forward simulation starts on the M15 candle after the fill candle; the fill candle itself is not evaluated for SL/TP.
- No news filter or general slippage is modeled; --cost-r subtracts a fixed R cost per trade, and only the breakout gap rule above is applied.
- Headline exit is TP1; TP2-before-SL is a separate hypothetical runner that ignores the TP1 exit.
- If SL and TP print in the same M15 candle, SL wins (conservative).
- Win rate = TP1 wins / all simulated trades; timeouts are neither wins nor losses. Profit factor and avg R include timeout exits.
- HTF context uses only closed D1/H4/H1 candles; the scenario window uses the last 300 closed M15 candles.
- Trades still open at the period end are force-closed at the last close and flagged as period_end_forced_exit.
- Session breakdown uses VN time (UTC+7), 13:00-23:00 vs outside, measured at the fill bar.
- RSI H1 bucket is measured at the signal bar (when the PRIMARY scenario is built), not at the fill bar.
- entry_outside_trigger_range is retained as a diagnostic of the trigger candle only; it no longer implies an assumed fill.
- No network is used when the local JSONL cache covers the requested period; no API keys or AI endpoints are used.

_This is a research report, not financial advice. The PAXG proxy and the simplified fill model can still differ from real XAU/USD execution._
