# Scenario / Early-Entry Backtest

Period: **2025-01-01** to **2025-12-31** (UTC) · symbol **XAUUSD** · interval **15m**
Data: Dukascopy XAUUSD M15 bid (tick volume) cached at `data/backtest/dukascopy/xauusd-m15-bid-2025.csv` (23550 candles loaded, 23550 in period, 11302 missing M15 bars).

Command: `target/release/backtest_scenarios --start 2025-01-01 --end 2025-12-31 --symbol XAUUSD --csv data/backtest/dukascopy/xauusd-m15-bid-2025.csv --cost-r 0.05 --out-json /tmp/p2_duk.json --out-md /tmp/p2_duk.md`
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
| quantum_v2 | ON | no | HTF-merged levels; SL cap 1xATR; weighted confluence v2 (30/25/20/15/10). |
| anti_chase_off | OFF | no | HTF-merged levels; SL cap 1×ATR; anti-chase OFF; BUY RSI H1 guard ON. |
| aligned_only | ON | yes | HTF-merged levels; SL cap 1×ATR; skip COUNTER PRIMARY. |
| aligned_only_no_anti_chase | OFF | yes | HTF-merged levels; SL cap 1×ATR; skip COUNTER PRIMARY; anti-chase OFF. |

| Variant | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Expectancy | PF | Max DD (R) | Total R | TP2 before SL | Timeout rate |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| legacy | 429 | 129 | 238 | 62 | 30.1% | 0.096 | 0.096 | 1.16 | 30.08 | 41.32 | 18.2% | 14.4% |
| htf_only | 401 | 140 | 226 | 35 | 34.9% | 0.138 | 0.138 | 1.23 | 28.04 | 55.15 | 21.4% | 8.7% |
| sl_cap_only | 414 | 135 | 228 | 51 | 32.6% | 0.128 | 0.128 | 1.22 | 23.00 | 52.95 | 18.1% | 12.3% |
| current | 400 | 140 | 224 | 36 | 35.0% | 0.141 | 0.141 | 1.24 | 27.95 | 56.42 | 21.5% | 9.0% |
| quantum_v2 | 390 | 129 | 218 | 43 | 33.1% | 0.137 | 0.137 | 1.23 | 30.09 | 53.58 | 18.5% | 11.0% |
| anti_chase_off | 399 | 140 | 223 | 36 | 35.1% | 0.144 | 0.144 | 1.24 | 26.92 | 57.47 | 21.6% | 9.0% |
| aligned_only | 400 | 140 | 224 | 36 | 35.0% | 0.141 | 0.141 | 1.24 | 27.95 | 56.42 | 21.5% | 9.0% |
| aligned_only_no_anti_chase | 399 | 140 | 223 | 36 | 35.1% | 0.144 | 0.144 | 1.24 | 26.92 | 57.47 | 21.6% | 9.0% |

### Fill accounting

| Variant | Filled at level | Gapped at open | Skipped, no fill | Counter skipped | Entry outside trigger range |
| --- | ---: | ---: | ---: | ---: | ---: |
| legacy | 429 | 0 | 542 | 0 | 690 |
| htf_only | 401 | 0 | 400 | 0 | 520 |
| sl_cap_only | 414 | 0 | 514 | 0 | 652 |
| current | 400 | 0 | 397 | 0 | 517 |
| quantum_v2 | 390 | 0 | 348 | 0 | 450 |
| anti_chase_off | 399 | 0 | 400 | 0 | 519 |
| aligned_only | 400 | 0 | 397 | 0 | 517 |
| aligned_only_no_anti_chase | 399 | 0 | 400 | 0 | 519 |

## Current variant headline

| Metric | Value |
| --- | ---: |
| Trades | 400 |
| Wins (TP1) | 140 |
| Losses (SL) | 224 |
| Timeouts | 36 |
| Win rate | 35.0% |
| Avg R | 0.141 |
| Expectancy (R/trade) | 0.141 |
| Profit factor | 1.24 |
| Max drawdown (R) | 27.95 |
| Total R | 56.42 |
| TP2 before SL | 21.5% (86 of 400 trades) |
| Timeout rate | 9.0% |

**Uncertainty:** avg R 95% CI [0.003, 0.279] · t=2.00 · SE=0.070 · n=400.

### Funnel

| Stage | Count |
| --- | ---: |
| M15 bars in period | 23251 |
| Bars skipped while a trade was open | 17954 |
| Setups evaluated | 5297 |
| PRIMARY valid with levels | 5288 |
| PRIMARY skipped by aligned_only (COUNTER) | 0 |
| BUY skipped by RSI H1 < 30 | 444 |
| Trigger found within 8 bars | 797 |
| Entries filled at level | 400 |
| Entries gapped (filled at open) | 0 |
| Entries skipped, no fill | 397 |
| ... of which gap > 0.5R | 0 |
| Entry inside trigger candle range | 280 |
| Entry outside trigger candle range (diagnostic) | 517 |
| Skipped (no forward data) | 0 |
| Period-end forced exits | 0 |
| Trades simulated | 400 |

## By scenario (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| bearish_breakdown | 14 | 0 | 9 | 5 | 0.0% | -0.485 | 0.31 | 6.87 | 0.0% |
| bullish_bounce | 371 | 138 | 206 | 27 | 37.2% | 0.183 | 1.31 | 22.72 | 23.2% |
| bullish_breakout | 15 | 2 | 9 | 4 | 13.3% | -0.316 | 0.52 | 6.76 | 0.0% |

## By scenario (aligned_only)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| bearish_breakdown | 14 | 0 | 9 | 5 | 0.0% | -0.485 | 0.31 | 6.87 | 0.0% |
| bullish_bounce | 371 | 138 | 206 | 27 | 37.2% | 0.183 | 1.31 | 22.72 | 23.2% |
| bullish_breakout | 15 | 2 | 9 | 4 | 13.3% | -0.316 | 0.52 | 6.76 | 0.0% |

## By session (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| OUTSIDE | 184 | 62 | 100 | 22 | 33.7% | 0.159 | 1.28 | 24.33 | 21.7% |
| VN_13_23 | 216 | 78 | 124 | 14 | 36.1% | 0.126 | 1.21 | 15.30 | 21.3% |

## By macro bias (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| BEARISH | 10 | 0 | 6 | 4 | 0.0% | -0.322 | 0.49 | 3.30 | 0.0% |
| BULLISH | 367 | 133 | 203 | 31 | 36.2% | 0.169 | 1.29 | 24.82 | 22.1% |
| NEUTRAL | 23 | 7 | 15 | 1 | 30.4% | -0.110 | 0.84 | 8.83 | 21.7% |

## By macro alignment (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| ALIGNED | 377 | 133 | 209 | 35 | 35.3% | 0.156 | 1.27 | 27.95 | 21.5% |
| MIXED | 23 | 7 | 15 | 1 | 30.4% | -0.110 | 0.84 | 8.83 | 21.7% |

## By RSI H1 bucket (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 30-65 | 293 | 95 | 170 | 28 | 32.4% | 0.078 | 1.13 | 35.38 | 19.8% |
| <30 | 1 | 0 | 0 | 1 | 0.0% | -0.426 | 0.00 | 0.43 | 0.0% |
| >65 | 106 | 45 | 54 | 7 | 42.4% | 0.321 | 1.60 | 13.95 | 26.4% |

## By trigger direction (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| ABOVE | 15 | 2 | 9 | 4 | 13.3% | -0.316 | 0.52 | 6.76 | 0.0% |
| BELOW | 14 | 0 | 9 | 5 | 0.0% | -0.485 | 0.31 | 6.87 | 0.0% |
| TOUCH | 371 | 138 | 206 | 27 | 37.2% | 0.183 | 1.31 | 22.72 | 23.2% |

## By early-entry confidence (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| <90 | 285 | 102 | 158 | 25 | 35.8% | 0.158 | 1.27 | 12.40 | 22.1% |
| >=90 | 115 | 38 | 66 | 11 | 33.0% | 0.099 | 1.16 | 21.13 | 20.0% |

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
