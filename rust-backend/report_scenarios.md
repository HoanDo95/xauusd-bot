# Scenario / Early-Entry Backtest

Period: **2025-01-01** to **2025-12-31** (UTC) · symbol **PAXGUSDT** · interval **15m**
Data: Binance public klines cached at `/home/jason/Downloads/Be-foundation/tool-xauusd/gold-signal-bot/rust-backend/data/backtest/PAXGUSDT_M15.jsonl` (175329 candles loaded, 35040 in period, 0 missing M15 bars).

Command: `target/release/backtest_scenarios --start 2025-01-01 --end 2025-12-31`

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
| legacy | ON | no | Anti-chase ON; bearish_rejection ON; no BUY RSI H1 guard; all valid PRIMARY. |
| current | ON | no | Anti-chase ON; bearish_rejection OFF; BUY RSI H1 guard ON; all valid PRIMARY. |
| anti_chase_off | OFF | no | Anti-chase OFF; bearish_rejection OFF; BUY RSI H1 guard ON; all valid PRIMARY. |
| aligned_only | ON | yes | Anti-chase ON; bearish_rejection OFF; BUY RSI H1 guard ON; skip COUNTER PRIMARY. |
| aligned_only_no_anti_chase | OFF | yes | Anti-chase OFF; bearish_rejection OFF; BUY RSI H1 guard ON; skip COUNTER PRIMARY. |

| Variant | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Expectancy | PF | Max DD (R) | Total R | TP2 before SL | Timeout rate |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| legacy | 617 | 163 | 331 | 123 | 26.4% | 0.082 | 0.082 | 1.15 | 27.41 | 50.28 | 16.7% | 19.9% |
| current | 584 | 161 | 302 | 121 | 27.6% | 0.124 | 0.124 | 1.23 | 16.81 | 72.33 | 17.6% | 20.7% |
| anti_chase_off | 582 | 160 | 301 | 121 | 27.5% | 0.124 | 0.124 | 1.23 | 17.98 | 72.40 | 17.3% | 20.8% |
| aligned_only | 584 | 161 | 302 | 121 | 27.6% | 0.124 | 0.124 | 1.23 | 16.81 | 72.33 | 17.6% | 20.7% |
| aligned_only_no_anti_chase | 582 | 160 | 301 | 121 | 27.5% | 0.124 | 0.124 | 1.23 | 17.98 | 72.40 | 17.3% | 20.8% |

### Fill accounting

| Variant | Filled at level | Gapped at open | Skipped, no fill | Counter skipped | Entry outside trigger range |
| --- | ---: | ---: | ---: | ---: | ---: |
| legacy | 617 | 0 | 785 | 0 | 980 |
| current | 584 | 0 | 780 | 0 | 973 |
| anti_chase_off | 582 | 0 | 781 | 0 | 971 |
| aligned_only | 584 | 0 | 780 | 0 | 973 |
| aligned_only_no_anti_chase | 582 | 0 | 781 | 0 | 971 |

## Current variant headline

| Metric | Value |
| --- | ---: |
| Trades | 584 |
| Wins (TP1) | 161 |
| Losses (SL) | 302 |
| Timeouts | 121 |
| Win rate | 27.6% |
| Avg R | 0.124 |
| Expectancy (R/trade) | 0.124 |
| Profit factor | 1.23 |
| Max drawdown (R) | 16.81 |
| Total R | 72.33 |
| TP2 before SL | 17.6% (103 of 584 trades) |
| Timeout rate | 20.7% |

**Uncertainty:** avg R 95% CI [0.017, 0.231] · t=2.27 · SE=0.054 · n=584.

### Funnel

| Stage | Count |
| --- | ---: |
| M15 bars in period | 35040 |
| Bars skipped while a trade was open | 27602 |
| Setups evaluated | 7438 |
| PRIMARY valid with levels | 6631 |
| PRIMARY skipped by aligned_only (COUNTER) | 0 |
| BUY skipped by RSI H1 < 30 | 508 |
| Trigger found within 8 bars | 1364 |
| Entries filled at level | 584 |
| Entries gapped (filled at open) | 0 |
| Entries skipped, no fill | 780 |
| ... of which gap > 0.5R | 0 |
| Entry inside trigger candle range | 391 |
| Entry outside trigger candle range (diagnostic) | 973 |
| Skipped (no forward data) | 0 |
| Period-end forced exits | 1 |
| Trades simulated | 584 |

## By scenario (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| bearish_breakdown | 29 | 6 | 13 | 10 | 20.7% | 0.002 | 1.00 | 3.54 | 3.5% |
| bullish_bounce | 522 | 148 | 286 | 88 | 28.3% | 0.094 | 1.17 | 22.77 | 18.6% |
| bullish_breakout | 33 | 7 | 3 | 23 | 21.2% | 0.701 | 6.30 | 1.82 | 15.2% |

## By scenario (aligned_only)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| bearish_breakdown | 29 | 6 | 13 | 10 | 20.7% | 0.002 | 1.00 | 3.54 | 3.5% |
| bullish_bounce | 522 | 148 | 286 | 88 | 28.3% | 0.094 | 1.17 | 22.77 | 18.6% |
| bullish_breakout | 33 | 7 | 3 | 23 | 21.2% | 0.701 | 6.30 | 1.82 | 15.2% |

## By session (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| OUTSIDE | 328 | 90 | 168 | 70 | 27.4% | 0.137 | 1.26 | 19.86 | 16.2% |
| VN_13_23 | 256 | 71 | 134 | 51 | 27.7% | 0.107 | 1.20 | 15.42 | 19.5% |

## By macro bias (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| BEARISH | 29 | 6 | 13 | 10 | 20.7% | 0.002 | 1.00 | 3.54 | 3.5% |
| BULLISH | 555 | 155 | 289 | 111 | 27.9% | 0.130 | 1.24 | 18.14 | 18.4% |

## By macro alignment (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| ALIGNED | 584 | 161 | 302 | 121 | 27.6% | 0.124 | 1.23 | 16.81 | 17.6% |

## By RSI H1 bucket (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 30-65 | 487 | 123 | 268 | 96 | 25.3% | 0.043 | 1.08 | 19.62 | 16.6% |
| >65 | 97 | 38 | 34 | 25 | 39.2% | 0.527 | 2.38 | 4.88 | 22.7% |

## By trigger direction (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| ABOVE | 33 | 7 | 3 | 23 | 21.2% | 0.701 | 6.30 | 1.82 | 15.2% |
| BELOW | 29 | 6 | 13 | 10 | 20.7% | 0.002 | 1.00 | 3.54 | 3.5% |
| TOUCH | 522 | 148 | 286 | 88 | 28.3% | 0.094 | 1.17 | 22.77 | 18.6% |

## By early-entry confidence (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| <90 | 367 | 103 | 192 | 72 | 28.1% | 0.107 | 1.20 | 14.15 | 16.9% |
| >=90 | 217 | 58 | 110 | 49 | 26.7% | 0.152 | 1.29 | 13.81 | 18.9% |

## Caveats

- PAXGUSDT is a free proxy for XAU/USD; the PAXG premium/discount is not modeled.
- The macro layer is deterministic only: D1 EMA bias, confidence 80, score 0, no metrics/COT/news and no AI review.
- One open trade at a time; new setups are skipped while a trade is open, and setup evaluation resumes on the M15 bar after the exit bar.
- TOUCH/LIMIT entries require the scenario entry level to trade in the next 8 closed M15 bars after the trigger; if it never trades, the trade is skipped.
- BREAKOUT entries fill at the scenario entry level; if the trigger candle opens beyond the level, they fill at the candle open only when slippage is <= 0.5R, otherwise they are skipped.
- Forward simulation starts on the M15 candle after the fill candle; the fill candle itself is not evaluated for SL/TP.
- No spread, commission, news filter or general slippage is modeled; only the breakout gap rule above is applied.
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
