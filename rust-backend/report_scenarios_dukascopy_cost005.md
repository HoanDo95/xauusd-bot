# Scenario / Early-Entry Backtest

Period: **2025-01-01** to **2025-12-31** (UTC) · symbol **XAUUSD** · interval **15m**
Data: Dukascopy XAUUSD M15 bid (tick volume) cached at `data/backtest/dukascopy/xauusd-m15-bid-2025.csv` (23550 candles loaded, 23550 in period, 11302 missing M15 bars).

Command: `target/release/backtest_scenarios --start 2025-01-01 --end 2025-12-31 --symbol XAUUSD --csv data/backtest/dukascopy/xauusd-m15-bid-2025.csv --cost-r 0.05 --out-json report_scenarios_dukascopy_cost005.json --out-md report_scenarios_dukascopy_cost005.md`
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
| legacy | ON | no | Anti-chase ON; bearish_rejection ON; no BUY RSI H1 guard; all valid PRIMARY. |
| current | ON | no | Anti-chase ON; bearish_rejection OFF; BUY RSI H1 guard ON; all valid PRIMARY. |
| anti_chase_off | OFF | no | Anti-chase OFF; bearish_rejection OFF; BUY RSI H1 guard ON; all valid PRIMARY. |
| aligned_only | ON | yes | Anti-chase ON; bearish_rejection OFF; BUY RSI H1 guard ON; skip COUNTER PRIMARY. |
| aligned_only_no_anti_chase | OFF | yes | Anti-chase OFF; bearish_rejection OFF; BUY RSI H1 guard ON; skip COUNTER PRIMARY. |

| Variant | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Expectancy | PF | Max DD (R) | Total R | TP2 before SL | Timeout rate |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| legacy | 428 | 130 | 241 | 57 | 30.4% | 0.087 | 0.087 | 1.15 | 30.08 | 37.24 | 18.7% | 13.3% |
| current | 410 | 125 | 229 | 56 | 30.5% | 0.091 | 0.091 | 1.15 | 25.92 | 37.46 | 18.5% | 13.7% |
| anti_chase_off | 410 | 125 | 229 | 56 | 30.5% | 0.091 | 0.091 | 1.15 | 25.92 | 37.46 | 18.5% | 13.7% |
| aligned_only | 410 | 125 | 229 | 56 | 30.5% | 0.091 | 0.091 | 1.15 | 25.92 | 37.46 | 18.5% | 13.7% |
| aligned_only_no_anti_chase | 410 | 125 | 229 | 56 | 30.5% | 0.091 | 0.091 | 1.15 | 25.92 | 37.46 | 18.5% | 13.7% |

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
| Avg R | 0.091 |
| Expectancy (R/trade) | 0.091 |
| Profit factor | 1.15 |
| Max drawdown (R) | 25.92 |
| Total R | 37.46 |
| TP2 before SL | 18.5% (76 of 410 trades) |
| Timeout rate | 13.7% |

**Uncertainty:** avg R 95% CI [-0.041, 0.224] · t=1.35 · SE=0.067 · n=410.

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
| bearish_breakdown | 18 | 1 | 12 | 5 | 5.6% | -0.493 | 0.30 | 9.50 | 5.6% |
| bullish_bounce | 375 | 123 | 216 | 36 | 32.8% | 0.102 | 1.17 | 21.04 | 20.0% |
| bullish_breakout | 17 | 1 | 1 | 15 | 5.9% | 0.479 | 5.52 | 1.05 | 0.0% |

## By scenario (aligned_only)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| bearish_breakdown | 18 | 1 | 12 | 5 | 5.6% | -0.493 | 0.30 | 9.50 | 5.6% |
| bullish_bounce | 375 | 123 | 216 | 36 | 32.8% | 0.102 | 1.17 | 21.04 | 20.0% |
| bullish_breakout | 17 | 1 | 1 | 15 | 5.9% | 0.479 | 5.52 | 1.05 | 0.0% |

## By session (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| OUTSIDE | 222 | 72 | 120 | 30 | 32.4% | 0.141 | 1.25 | 28.08 | 19.8% |
| VN_13_23 | 188 | 53 | 109 | 26 | 28.2% | 0.033 | 1.05 | 17.21 | 17.0% |

## By macro bias (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| BEARISH | 13 | 1 | 8 | 4 | 7.7% | -0.377 | 0.42 | 5.53 | 7.7% |
| BULLISH | 377 | 117 | 212 | 48 | 31.0% | 0.098 | 1.17 | 20.39 | 18.0% |
| NEUTRAL | 20 | 7 | 9 | 4 | 35.0% | 0.263 | 1.53 | 5.25 | 35.0% |

## By macro alignment (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| ALIGNED | 390 | 118 | 220 | 52 | 30.3% | 0.083 | 1.14 | 25.92 | 17.7% |
| MIXED | 20 | 7 | 9 | 4 | 35.0% | 0.263 | 1.53 | 5.25 | 35.0% |

## By RSI H1 bucket (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 30-65 | 328 | 98 | 192 | 38 | 29.9% | 0.038 | 1.06 | 33.28 | 17.4% |
| >65 | 82 | 27 | 37 | 18 | 32.9% | 0.304 | 1.63 | 7.25 | 23.2% |

## By trigger direction (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| ABOVE | 17 | 1 | 1 | 15 | 5.9% | 0.479 | 5.52 | 1.05 | 0.0% |
| BELOW | 18 | 1 | 12 | 5 | 5.6% | -0.493 | 0.30 | 9.50 | 5.6% |
| TOUCH | 375 | 123 | 216 | 36 | 32.8% | 0.102 | 1.17 | 21.04 | 20.0% |

## By early-entry confidence (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| <90 | 278 | 88 | 153 | 37 | 31.6% | 0.106 | 1.18 | 12.02 | 18.7% |
| >=90 | 132 | 37 | 76 | 19 | 28.0% | 0.061 | 1.10 | 21.18 | 18.2% |

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
