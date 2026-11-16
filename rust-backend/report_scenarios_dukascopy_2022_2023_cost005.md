# Scenario / Early-Entry Backtest

Period: **2022-01-01** to **2023-12-31** (UTC) · symbol **XAUUSD** · interval **15m**
Data: Dukascopy XAUUSD M15 bid (tick volume) cached at `data/backtest/dukascopy/xauusd-m15-bid-2022-2023.csv` (47194 candles loaded, 47194 in period, 22498 missing M15 bars).

Command: `target/release/backtest_scenarios --start 2022-01-01 --end 2023-12-31 --symbol XAUUSD --csv data/backtest/dukascopy/xauusd-m15-bid-2022-2023.csv --cost-r 0.05 --out-json report_scenarios_dukascopy_2022_2023_cost005.json --out-md report_scenarios_dukascopy_2022_2023_cost005.md`
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
| legacy | 703 | 157 | 383 | 163 | 22.3% | -0.043 | -0.043 | 0.93 | 43.35 | -30.36 | 12.9% | 23.2% |
| current | 623 | 132 | 328 | 163 | 21.2% | -0.034 | -0.034 | 0.94 | 40.35 | -21.03 | 11.2% | 26.2% |
| anti_chase_off | 625 | 133 | 329 | 163 | 21.3% | -0.033 | -0.033 | 0.94 | 40.35 | -20.88 | 11.4% | 26.1% |
| aligned_only | 623 | 132 | 328 | 163 | 21.2% | -0.034 | -0.034 | 0.94 | 40.35 | -21.03 | 11.2% | 26.2% |
| aligned_only_no_anti_chase | 625 | 133 | 329 | 163 | 21.3% | -0.033 | -0.033 | 0.94 | 40.35 | -20.88 | 11.4% | 26.1% |

### Fill accounting

| Variant | Filled at level | Gapped at open | Skipped, no fill | Counter skipped | Entry outside trigger range |
| --- | ---: | ---: | ---: | ---: | ---: |
| legacy | 703 | 0 | 830 | 0 | 1067 |
| current | 623 | 0 | 762 | 0 | 945 |
| anti_chase_off | 625 | 0 | 762 | 0 | 945 |
| aligned_only | 623 | 0 | 762 | 0 | 945 |
| aligned_only_no_anti_chase | 625 | 0 | 762 | 0 | 945 |

## Current variant headline

| Metric | Value |
| --- | ---: |
| Trades | 623 |
| Wins (TP1) | 132 |
| Losses (SL) | 328 |
| Timeouts | 163 |
| Win rate | 21.2% |
| Avg R | -0.034 |
| Expectancy (R/trade) | -0.034 |
| Profit factor | 0.94 |
| Max drawdown (R) | 40.35 |
| Total R | -21.03 |
| TP2 before SL | 11.2% (70 of 623 trades) |
| Timeout rate | 26.2% |

**Uncertainty:** avg R 95% CI [-0.131, 0.063] · t=-0.68 · SE=0.050 · n=623.

### Funnel

| Stage | Count |
| --- | ---: |
| M15 bars in period | 46895 |
| Bars skipped while a trade was open | 34769 |
| Setups evaluated | 12126 |
| PRIMARY valid with levels | 12014 |
| PRIMARY skipped by aligned_only (COUNTER) | 0 |
| BUY skipped by RSI H1 < 30 | 755 |
| Trigger found within 8 bars | 1385 |
| Entries filled at level | 623 |
| Entries gapped (filled at open) | 0 |
| Entries skipped, no fill | 762 |
| ... of which gap > 0.5R | 0 |
| Entry inside trigger candle range | 440 |
| Entry outside trigger candle range (diagnostic) | 945 |
| Skipped (no forward data) | 0 |
| Period-end forced exits | 1 |
| Trades simulated | 623 |

## By scenario (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| bearish_breakdown | 201 | 27 | 79 | 95 | 13.4% | 0.031 | 1.07 | 14.88 | 2.5% |
| bullish_bounce | 390 | 104 | 245 | 41 | 26.7% | -0.083 | 0.88 | 46.57 | 16.4% |
| bullish_breakout | 32 | 1 | 4 | 27 | 3.1% | 0.152 | 1.82 | 1.33 | 3.1% |

## By scenario (aligned_only)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| bearish_breakdown | 201 | 27 | 79 | 95 | 13.4% | 0.031 | 1.07 | 14.88 | 2.5% |
| bullish_bounce | 390 | 104 | 245 | 41 | 26.7% | -0.083 | 0.88 | 46.57 | 16.4% |
| bullish_breakout | 32 | 1 | 4 | 27 | 3.1% | 0.152 | 1.82 | 1.33 | 3.1% |

## By session (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| OUTSIDE | 285 | 67 | 144 | 74 | 23.5% | 0.012 | 1.02 | 36.51 | 12.3% |
| VN_13_23 | 338 | 65 | 184 | 89 | 19.2% | -0.073 | 0.88 | 29.91 | 10.4% |

## By macro bias (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| BEARISH | 198 | 27 | 77 | 94 | 13.6% | 0.040 | 1.09 | 14.88 | 2.5% |
| BULLISH | 408 | 103 | 242 | 63 | 25.2% | -0.064 | 0.90 | 42.65 | 15.4% |
| NEUTRAL | 17 | 2 | 9 | 6 | 11.8% | -0.165 | 0.73 | 6.54 | 11.8% |

## By macro alignment (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| ALIGNED | 606 | 130 | 319 | 157 | 21.4% | -0.030 | 0.95 | 40.35 | 11.2% |
| MIXED | 17 | 2 | 9 | 6 | 11.8% | -0.165 | 0.73 | 6.54 | 11.8% |

## By RSI H1 bucket (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 30-65 | 503 | 113 | 273 | 117 | 22.5% | -0.019 | 0.97 | 24.33 | 11.5% |
| <30 | 9 | 1 | 5 | 3 | 11.1% | -0.129 | 0.78 | 5.25 | 0.0% |
| >65 | 111 | 18 | 50 | 43 | 16.2% | -0.094 | 0.82 | 24.33 | 10.8% |

## By trigger direction (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| ABOVE | 32 | 1 | 4 | 27 | 3.1% | 0.152 | 1.82 | 1.33 | 3.1% |
| BELOW | 201 | 27 | 79 | 95 | 13.4% | 0.031 | 1.07 | 14.88 | 2.5% |
| TOUCH | 390 | 104 | 245 | 41 | 26.7% | -0.083 | 0.88 | 46.57 | 16.4% |

## By early-entry confidence (current)

| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| <90 | 466 | 88 | 248 | 130 | 18.9% | -0.083 | 0.86 | 47.20 | 9.0% |
| >=90 | 157 | 44 | 80 | 33 | 28.0% | 0.112 | 1.20 | 11.04 | 17.8% |

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
