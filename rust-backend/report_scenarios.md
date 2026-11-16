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
| current | ON | no | Anti-chase ON; all valid PRIMARY scenarios. |
| anti_chase_off | OFF | no | Anti-chase OFF; all valid PRIMARY scenarios. |
| aligned_only | ON | yes | Anti-chase ON; skip PRIMARY scenarios with macro_alignment = COUNTER. |
| aligned_only_no_anti_chase | OFF | yes | Anti-chase OFF; skip PRIMARY scenarios with macro_alignment = COUNTER. |

| Variant | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Expectancy | PF | Max DD (R) | Total R | TP2 before SL | Timeout rate |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| current | 638 | 159 | 354 | 125 | 24.9% | 0.029 | 0.029 | 1.05 | 28.13 | 18.46 | 15.8% | 19.6% |
| anti_chase_off | 630 | 159 | 346 | 125 | 25.2% | 0.044 | 0.044 | 1.08 | 27.13 | 27.52 | 16.0% | 19.8% |
| aligned_only | 617 | 163 | 331 | 123 | 26.4% | 0.082 | 0.082 | 1.15 | 27.41 | 50.28 | 16.7% | 19.9% |
| aligned_only_no_anti_chase | 615 | 162 | 330 | 123 | 26.3% | 0.082 | 0.082 | 1.15 | 29.41 | 50.34 | 16.4% | 20.0% |

### Fill accounting

| Variant | Filled at level | Gapped at open | Skipped, no fill | Counter skipped | Entry outside trigger range |
| --- | ---: | ---: | ---: | ---: | ---: |
| current | 638 | 0 | 781 | 0 | 983 |
| anti_chase_off | 630 | 0 | 777 | 0 | 976 |
| aligned_only | 617 | 0 | 785 | 239 | 980 |
| aligned_only_no_anti_chase | 615 | 0 | 786 | 189 | 978 |

## Current variant headline

| Metric | Value |
| --- | ---: |
| Trades | 638 |
| Wins (TP1) | 159 |
| Losses (SL) | 354 |
| Timeouts | 125 |
| Win rate | 24.9% |
| Avg R | 0.029 |
| Expectancy (R/trade) | 0.029 |
| Profit factor | 1.05 |
| Max drawdown (R) | 28.13 |
| Total R | 18.46 |
| TP2 before SL | 15.8% (101 of 638 trades) |
| Timeout rate | 19.6% |

**Uncertainty:** avg R 95% CI [-0.072, 0.130] · t=0.56 · SE=0.051 · n=638.

### Funnel

| Stage | Count |
| --- | ---: |
| M15 bars in period | 35040 |
| Bars skipped while a trade was open | 28713 |
| Setups evaluated | 6327 |
| PRIMARY valid with levels | 5794 |
| PRIMARY skipped by aligned_only (COUNTER) | 0 |
| Trigger found within 8 bars | 1419 |
| Entries filled at level | 638 |
| Entries gapped (filled at open) | 0 |
| Entries skipped, no fill | 781 |
| ... of which gap > 0.5R | 0 |
| Entry inside trigger candle range | 436 |
| Entry outside trigger candle range (diagnostic) | 983 |
| Skipped (no forward data) | 0 |
| Period-end forced exits | 1 |
| Trades simulated | 638 |

## By scenario (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| bearish_breakdown | 28 | 6 | 12 | 10 | 21.4% | 0.038 | 1.07 | 3.54 | 3.6% |
| bearish_rejection | 53 | 7 | 44 | 2 | 13.2% | -0.567 | 0.32 | 31.79 | 7.5% |
| bullish_bounce | 525 | 140 | 295 | 90 | 26.7% | 0.051 | 1.09 | 25.15 | 17.3% |
| bullish_breakout | 32 | 6 | 3 | 23 | 18.8% | 0.645 | 5.34 | 1.82 | 15.6% |

## By scenario (aligned_only)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| bearish_breakdown | 28 | 6 | 12 | 10 | 21.4% | 0.038 | 1.07 | 3.54 | 3.6% |
| bearish_rejection | 13 | 1 | 12 | 0 | 7.7% | -0.769 | 0.17 | 10.00 | 0.0% |
| bullish_bounce | 542 | 149 | 304 | 89 | 27.5% | 0.067 | 1.12 | 26.23 | 17.9% |
| bullish_breakout | 34 | 7 | 3 | 24 | 20.6% | 0.668 | 5.77 | 1.82 | 14.7% |

## By session (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| OUTSIDE | 356 | 89 | 199 | 68 | 25.0% | 0.031 | 1.05 | 26.08 | 14.9% |
| VN_13_23 | 282 | 70 | 155 | 57 | 24.8% | 0.026 | 1.05 | 19.76 | 17.0% |

## By macro bias (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| BEARISH | 41 | 7 | 24 | 10 | 17.1% | -0.218 | 0.66 | 8.95 | 2.4% |
| BULLISH | 597 | 152 | 330 | 115 | 25.5% | 0.046 | 1.08 | 23.19 | 16.8% |

## By macro alignment (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| ALIGNED | 598 | 153 | 322 | 123 | 25.6% | 0.064 | 1.12 | 26.34 | 16.2% |
| COUNTER | 40 | 6 | 32 | 2 | 15.0% | -0.501 | 0.38 | 21.79 | 10.0% |

## By RSI H1 bucket (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 30-65 | 470 | 118 | 259 | 93 | 25.1% | 0.041 | 1.07 | 20.15 | 16.4% |
| <30 | 49 | 7 | 36 | 6 | 14.3% | -0.366 | 0.51 | 24.34 | 8.2% |
| >65 | 119 | 34 | 59 | 26 | 28.6% | 0.144 | 1.27 | 12.53 | 16.8% |

## By trigger direction (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| ABOVE | 32 | 6 | 3 | 23 | 18.8% | 0.645 | 5.34 | 1.82 | 15.6% |
| BELOW | 28 | 6 | 12 | 10 | 21.4% | 0.038 | 1.07 | 3.54 | 3.6% |
| TOUCH | 578 | 147 | 339 | 92 | 25.4% | -0.006 | 0.99 | 37.28 | 16.4% |

## By early-entry confidence (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| <90 | 431 | 103 | 249 | 79 | 23.9% | -0.035 | 0.94 | 33.59 | 14.4% |
| >=90 | 207 | 56 | 105 | 46 | 27.1% | 0.162 | 1.31 | 13.26 | 18.8% |

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
