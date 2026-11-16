# Scenario / Early-Entry Backtest

Period: **2022-01-01** to **2023-12-31** (UTC) · symbol **XAUUSD** · interval **15m**
Data: Dukascopy XAUUSD M15 bid (tick volume) cached at `data/backtest/dukascopy/xauusd-m15-bid-2022-2023.csv` (47194 candles loaded, 47194 in period, 22498 missing M15 bars).

Command: `target/release/backtest_scenarios --start 2022-01-01 --end 2023-12-31 --symbol XAUUSD --csv data/backtest/dukascopy/xauusd-m15-bid-2022-2023.csv --cost-r 0.05 --out-json /tmp/p2_duk2223.json --out-md /tmp/p2_duk2223.md`
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
| legacy | 703 | 157 | 383 | 163 | 22.3% | -0.043 | -0.043 | 0.93 | 43.35 | -30.36 | 12.9% | 23.2% |
| htf_only | 584 | 142 | 316 | 126 | 24.3% | 0.021 | 0.021 | 1.04 | 23.96 | 12.11 | 13.0% | 21.6% |
| sl_cap_only | 662 | 162 | 369 | 131 | 24.5% | -0.018 | -0.018 | 0.97 | 38.64 | -11.65 | 13.0% | 19.8% |
| current | 601 | 158 | 340 | 103 | 26.3% | 0.031 | 0.031 | 1.05 | 26.58 | 18.70 | 14.0% | 17.1% |
| quantum_v2 | 588 | 152 | 318 | 118 | 25.9% | 0.062 | 0.062 | 1.11 | 27.01 | 36.61 | 14.0% | 20.1% |
| anti_chase_off | 604 | 159 | 342 | 103 | 26.3% | 0.031 | 0.031 | 1.05 | 26.58 | 18.55 | 13.9% | 17.1% |
| aligned_only | 601 | 158 | 340 | 103 | 26.3% | 0.031 | 0.031 | 1.05 | 26.58 | 18.70 | 14.0% | 17.1% |
| aligned_only_no_anti_chase | 604 | 159 | 342 | 103 | 26.3% | 0.031 | 0.031 | 1.05 | 26.58 | 18.55 | 13.9% | 17.1% |

### Fill accounting

| Variant | Filled at level | Gapped at open | Skipped, no fill | Counter skipped | Entry outside trigger range |
| --- | ---: | ---: | ---: | ---: | ---: |
| legacy | 703 | 0 | 830 | 0 | 1067 |
| htf_only | 584 | 0 | 556 | 0 | 665 |
| sl_cap_only | 662 | 0 | 794 | 0 | 987 |
| current | 601 | 0 | 557 | 0 | 671 |
| quantum_v2 | 587 | 1 | 526 | 0 | 622 |
| anti_chase_off | 604 | 0 | 557 | 0 | 671 |
| aligned_only | 601 | 0 | 557 | 0 | 671 |
| aligned_only_no_anti_chase | 604 | 0 | 557 | 0 | 671 |

## Current variant headline

| Metric | Value |
| --- | ---: |
| Trades | 601 |
| Wins (TP1) | 158 |
| Losses (SL) | 340 |
| Timeouts | 103 |
| Win rate | 26.3% |
| Avg R | 0.031 |
| Expectancy (R/trade) | 0.031 |
| Profit factor | 1.05 |
| Max drawdown (R) | 26.58 |
| Total R | 18.70 |
| TP2 before SL | 14.0% (84 of 601 trades) |
| Timeout rate | 17.1% |

**Uncertainty:** avg R 95% CI [-0.077, 0.140] · t=0.56 · SE=0.055 · n=601.

### Funnel

| Stage | Count |
| --- | ---: |
| M15 bars in period | 46895 |
| Bars skipped while a trade was open | 30435 |
| Setups evaluated | 16460 |
| PRIMARY valid with levels | 16358 |
| PRIMARY skipped by aligned_only (COUNTER) | 0 |
| BUY skipped by RSI H1 < 30 | 806 |
| Trigger found within 8 bars | 1158 |
| Entries filled at level | 601 |
| Entries gapped (filled at open) | 0 |
| Entries skipped, no fill | 557 |
| ... of which gap > 0.5R | 0 |
| Entry inside trigger candle range | 487 |
| Entry outside trigger candle range (diagnostic) | 671 |
| Skipped (no forward data) | 0 |
| Period-end forced exits | 1 |
| Trades simulated | 601 |

## By scenario (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| bearish_breakdown | 195 | 36 | 94 | 65 | 18.5% | 0.083 | 1.16 | 11.77 | 7.7% |
| bullish_bounce | 396 | 120 | 239 | 37 | 30.3% | 0.012 | 1.02 | 17.42 | 17.2% |
| bullish_breakout | 10 | 2 | 7 | 1 | 20.0% | -0.240 | 0.67 | 6.30 | 10.0% |

## By scenario (aligned_only)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| bearish_breakdown | 195 | 36 | 94 | 65 | 18.5% | 0.083 | 1.16 | 11.77 | 7.7% |
| bullish_bounce | 396 | 120 | 239 | 37 | 30.3% | 0.012 | 1.02 | 17.42 | 17.2% |
| bullish_breakout | 10 | 2 | 7 | 1 | 20.0% | -0.240 | 0.67 | 6.30 | 10.0% |

## By session (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| OUTSIDE | 240 | 66 | 135 | 39 | 27.5% | 0.060 | 1.10 | 29.41 | 15.8% |
| VN_13_23 | 361 | 92 | 205 | 64 | 25.5% | 0.012 | 1.02 | 24.28 | 12.7% |

## By macro bias (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| BEARISH | 190 | 36 | 90 | 64 | 18.9% | 0.104 | 1.20 | 11.77 | 7.9% |
| BULLISH | 394 | 120 | 237 | 37 | 30.5% | 0.020 | 1.03 | 17.42 | 17.3% |
| NEUTRAL | 17 | 2 | 13 | 2 | 11.8% | -0.522 | 0.35 | 12.33 | 5.9% |

## By macro alignment (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| ALIGNED | 584 | 156 | 327 | 101 | 26.7% | 0.047 | 1.08 | 26.58 | 14.2% |
| MIXED | 17 | 2 | 13 | 2 | 11.8% | -0.522 | 0.35 | 12.33 | 5.9% |

## By RSI H1 bucket (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 30-65 | 474 | 125 | 263 | 86 | 26.4% | 0.048 | 1.08 | 18.82 | 14.1% |
| <30 | 8 | 3 | 2 | 3 | 37.5% | 0.490 | 2.45 | 1.65 | 12.5% |
| >65 | 119 | 30 | 75 | 14 | 25.2% | -0.066 | 0.90 | 19.12 | 13.5% |

## By trigger direction (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| ABOVE | 10 | 2 | 7 | 1 | 20.0% | -0.240 | 0.67 | 6.30 | 10.0% |
| BELOW | 195 | 36 | 94 | 65 | 18.5% | 0.083 | 1.16 | 11.77 | 7.7% |
| TOUCH | 396 | 120 | 239 | 37 | 30.3% | 0.012 | 1.02 | 17.42 | 17.2% |

## By early-entry confidence (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| <90 | 447 | 111 | 258 | 78 | 24.8% | -0.000 | 1.00 | 22.63 | 11.4% |
| >=90 | 154 | 47 | 82 | 25 | 30.5% | 0.123 | 1.21 | 15.53 | 21.4% |

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
