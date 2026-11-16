# Scenario / Early-Entry Backtest

Period: **2025-01-01** to **2025-12-31** (UTC) · symbol **XAUUSD** · interval **15m**
Data: Dukascopy XAUUSD M15 bid (tick volume) cached at `data/backtest/dukascopy/xauusd-m15-bid-2025.csv` (23550 candles loaded, 23550 in period, 11302 missing M15 bars).

Command: `target/release/backtest_scenarios --start 2025-01-01 --end 2025-12-31 --symbol XAUUSD --csv data/backtest/dukascopy/xauusd-m15-bid-2025.csv --out-json report_scenarios_dukascopy.json --out-md report_scenarios_dukascopy.md`

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
| legacy | 428 | 130 | 241 | 57 | 30.4% | 0.137 | 0.137 | 1.24 | 22.58 | 58.64 | 18.7% | 13.3% |
| current | 410 | 125 | 229 | 56 | 30.5% | 0.141 | 0.141 | 1.25 | 19.02 | 57.96 | 18.5% | 13.7% |
| anti_chase_off | 410 | 125 | 229 | 56 | 30.5% | 0.141 | 0.141 | 1.25 | 19.02 | 57.96 | 18.5% | 13.7% |
| aligned_only | 410 | 125 | 229 | 56 | 30.5% | 0.141 | 0.141 | 1.25 | 19.02 | 57.96 | 18.5% | 13.7% |
| aligned_only_no_anti_chase | 410 | 125 | 229 | 56 | 30.5% | 0.141 | 0.141 | 1.25 | 19.02 | 57.96 | 18.5% | 13.7% |

### Fill accounting

| Variant | Filled at level | Gapped at open | Skipped, no fill | Counter skipped | Entry outside trigger range |
| --- | ---: | ---: | ---: | ---: | ---: |
| legacy | 428 | 0 | 556 | 0 | 703 |
| current | 410 | 0 | 521 | 0 | 661 |
| anti_chase_off | 410 | 0 | 521 | 0 | 661 |
| aligned_only | 410 | 0 | 521 | 0 | 661 |
| aligned_only_no_anti_chase | 410 | 0 | 521 | 0 | 661 |

## Current variant headline

| Metric | Value |
| --- | ---: |
| Trades | 410 |
| Wins (TP1) | 125 |
| Losses (SL) | 229 |
| Timeouts | 56 |
| Win rate | 30.5% |
| Avg R | 0.141 |
| Expectancy (R/trade) | 0.141 |
| Profit factor | 1.25 |
| Max drawdown (R) | 19.02 |
| Total R | 57.96 |
| TP2 before SL | 18.5% (76 of 410 trades) |
| Timeout rate | 13.7% |

**Uncertainty:** avg R 95% CI [0.009, 0.274] · t=2.10 · SE=0.067 · n=410.

### Funnel

| Stage | Count |
| --- | ---: |
| M15 bars in period | 23251 |
| Bars skipped while a trade was open | 18572 |
| Setups evaluated | 4679 |
| PRIMARY valid with levels | 4185 |
| PRIMARY skipped by aligned_only (COUNTER) | 0 |
| BUY skipped by RSI H1 < 30 | 381 |
| Trigger found within 8 bars | 931 |
| Entries filled at level | 410 |
| Entries gapped (filled at open) | 0 |
| Entries skipped, no fill | 521 |
| ... of which gap > 0.5R | 0 |
| Entry inside trigger candle range | 270 |
| Entry outside trigger candle range (diagnostic) | 661 |
| Skipped (no forward data) | 0 |
| Period-end forced exits | 1 |
| Trades simulated | 410 |

## By scenario (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| bearish_breakdown | 18 | 1 | 12 | 5 | 5.6% | -0.443 | 0.34 | 8.65 | 5.6% |
| bullish_bounce | 375 | 123 | 216 | 36 | 32.8% | 0.152 | 1.26 | 16.41 | 20.0% |
| bullish_breakout | 17 | 1 | 1 | 15 | 5.9% | 0.529 | 6.75 | 1.00 | 0.0% |

## By scenario (aligned_only)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| bearish_breakdown | 18 | 1 | 12 | 5 | 5.6% | -0.443 | 0.34 | 8.65 | 5.6% |
| bullish_bounce | 375 | 123 | 216 | 36 | 32.8% | 0.152 | 1.26 | 16.41 | 20.0% |
| bullish_breakout | 17 | 1 | 1 | 15 | 5.9% | 0.529 | 6.75 | 1.00 | 0.0% |

## By session (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| OUTSIDE | 222 | 72 | 120 | 30 | 32.4% | 0.191 | 1.35 | 24.38 | 19.8% |
| VN_13_23 | 188 | 53 | 109 | 26 | 28.2% | 0.083 | 1.14 | 15.07 | 17.0% |

## By macro bias (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| BEARISH | 13 | 1 | 8 | 4 | 7.7% | -0.327 | 0.47 | 4.93 | 7.7% |
| BULLISH | 377 | 117 | 212 | 48 | 31.0% | 0.148 | 1.26 | 15.31 | 18.0% |
| NEUTRAL | 20 | 7 | 9 | 4 | 35.0% | 0.313 | 1.67 | 5.00 | 35.0% |

## By macro alignment (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| ALIGNED | 390 | 118 | 220 | 52 | 30.3% | 0.133 | 1.23 | 19.02 | 17.7% |
| MIXED | 20 | 7 | 9 | 4 | 35.0% | 0.313 | 1.67 | 5.00 | 35.0% |

## By RSI H1 bucket (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 30-65 | 328 | 98 | 192 | 38 | 29.9% | 0.088 | 1.15 | 25.63 | 17.4% |
| >65 | 82 | 27 | 37 | 18 | 32.9% | 0.354 | 1.77 | 6.30 | 23.2% |

## By trigger direction (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| ABOVE | 17 | 1 | 1 | 15 | 5.9% | 0.529 | 6.75 | 1.00 | 0.0% |
| BELOW | 18 | 1 | 12 | 5 | 5.6% | -0.443 | 0.34 | 8.65 | 5.6% |
| TOUCH | 375 | 123 | 216 | 36 | 32.8% | 0.152 | 1.26 | 16.41 | 20.0% |

## By early-entry confidence (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| <90 | 278 | 88 | 153 | 37 | 31.6% | 0.156 | 1.28 | 9.77 | 18.7% |
| >=90 | 132 | 37 | 76 | 19 | 28.0% | 0.111 | 1.19 | 19.28 | 18.2% |

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
