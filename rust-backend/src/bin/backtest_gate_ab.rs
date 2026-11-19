//! Gate A/B research on an existing scenario backtest trade log.
//!
//! This is deliberately a research tool: it replays the same deterministic gate
//! functions used by live (`decision_gates`, `anti_chase`, HTF, macro, BUY RSI)
//! on the trades produced by `backtest_scenarios`. It does not fetch data, send
//! Telegram, place orders or modify production state.

use chrono::{DateTime, Utc};
use gold_signal_bot::decision_gates::confirmed_signal_with_gates;
use gold_signal_bot::macro_history::MacroHistory;
use gold_signal_bot::market_context::{build_market_context, htf_allows_signal};
use gold_signal_bot::market_view::score_scenario;
use gold_signal_bot::models::{BotConfig, Candle, Signal, Timeframe};
use gold_signal_bot::risk_guardrails::{buy_rsi_h1_block_reason, counter_macro_block_reason};
use gold_signal_bot::rsi::rsi_snapshot;
use gold_signal_bot::scenario_signal::signal_from_scenario;
use gold_signal_bot::scenarios::{anti_chase_blocks_breakout, Scenario};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

const M15_MS: i64 = 15 * 60 * 1000;
const H1_MS: i64 = 60 * 60 * 1000;
const H4_MS: i64 = 4 * H1_MS;
const D1_MS: i64 = 24 * H1_MS;

#[derive(Debug, Deserialize)]
struct Report {
    variants: BTreeMap<String, Variant>,
}

#[derive(Debug, Deserialize)]
struct Variant {
    trades: Vec<Trade>,
}

#[derive(Debug, Clone, Deserialize)]
struct Trade {
    trigger_time: String,
    scenario_id: String,
    direction: String,
    trigger_direction: String,
    confidence: u8,
    macro_alignment: String,
    entry: f64,
    stop_loss: f64,
    take_profit_1: f64,
    take_profit_2: f64,
    r_multiple: f64,
    #[serde(default)]
    risk_reward: Option<f64>,
    #[serde(default = "default_true")]
    retest_ok: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Default, Clone, Serialize)]
struct Stats {
    n: usize,
    wins: usize,
    win_rate: f64,
    avg_r: f64,
    total_r: f64,
    pf: f64,
    max_dd: f64,
}

fn stats(values: &[f64]) -> Stats {
    if values.is_empty() {
        return Stats::default();
    }
    let wins = values.iter().filter(|value| **value > 0.0).count();
    let gross: f64 = values.iter().filter(|value| **value > 0.0).sum();
    let loss: f64 = -values.iter().filter(|value| **value < 0.0).sum::<f64>();
    let mut equity: f64 = 0.0;
    let mut peak: f64 = 0.0;
    let mut max_dd: f64 = 0.0;
    for value in values {
        equity += value;
        peak = peak.max(equity);
        max_dd = max_dd.max(peak - equity);
    }
    Stats {
        n: values.len(),
        wins,
        win_rate: wins as f64 / values.len() as f64,
        avg_r: values.iter().sum::<f64>() / values.len() as f64,
        total_r: values.iter().sum(),
        pf: if loss > 0.0 {
            gross / loss
        } else {
            f64::INFINITY
        },
        max_dd,
    }
}

fn aggregate(candles: &[Candle], bucket_ms: i64, timeframe: Timeframe) -> Vec<Candle> {
    let mut out: Vec<Candle> = Vec::new();
    for candle in candles {
        let bucket = candle.timestamp - candle.timestamp.rem_euclid(bucket_ms);
        if out.last().map(|last| last.timestamp) != Some(bucket) {
            out.push(Candle {
                timestamp: bucket,
                open: candle.open,
                high: candle.high,
                low: candle.low,
                close: candle.close,
                volume: candle.volume,
                taker_buy_volume: 0.0,
                timeframe,
            });
        } else if let Some(last) = out.last_mut() {
            last.high = last.high.max(candle.high);
            last.low = last.low.min(candle.low);
            last.close = candle.close;
            last.volume += candle.volume;
        }
    }
    out
}

fn closed_before(candles: &[Candle], timeframe_ms: i64, as_of_ms: i64) -> &[Candle] {
    let end =
        candles.partition_point(|candle| candle.timestamp.saturating_add(timeframe_ms) <= as_of_ms);
    &candles[..end]
}

fn parse_ms(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.with_timezone(&Utc).timestamp_millis())
}

fn load_candles(path: &PathBuf) -> Result<Vec<Candle>, String> {
    let text = fs::read_to_string(path).map_err(|err| format!("read {}: {err}", path.display()))?;
    let mut candles: Vec<Candle> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<Candle>(line).ok())
        .filter(|candle| candle.timeframe == Timeframe::M15)
        .collect();
    candles.sort_by_key(|candle| candle.timestamp);
    Ok(candles)
}

fn scenario_from_trade(trade: &Trade) -> Scenario {
    let bias = if trade.direction.eq_ignore_ascii_case("SELL") {
        "BEARISH"
    } else {
        "BULLISH"
    };
    Scenario {
        id: trade.scenario_id.clone(),
        name: trade.scenario_id.clone(),
        bias: bias.to_string(),
        trigger: "research".to_string(),
        condition: "research".to_string(),
        entry: Some(trade.entry),
        stop_loss: Some(trade.stop_loss),
        take_profit_1: Some(trade.take_profit_1),
        take_profit_2: Some(trade.take_profit_2),
        risk_reward: trade.risk_reward,
        trigger_price: Some(trade.entry),
        trigger_direction: trade.trigger_direction.clone(),
        trigger_timeframe: "M15".to_string(),
        invalidation: "research".to_string(),
        confidence: trade.confidence,
        macro_alignment: trade.macro_alignment.clone(),
        valid: true,
        role: "PRIMARY".to_string(),
        priority: trade.confidence as i32,
        notes: vec![],
    }
}

fn base_config() -> BotConfig {
    BotConfig {
        timeframe: Timeframe::M15,
        symbol: "XAU/USD".to_string(),
        min_risk_reward: 2.0,
        early_entry_max_distance_r: 10.0,
        quantum_trigger_confirmation_enabled: false,
        htf_h4_filter_enabled: true,
        htf_d1_filter_enabled: true,
        counter_macro_filter_enabled: true,
        min_rsi_h1_for_buy: 30.0,
        anti_chase_filter_enabled: true,
        ..BotConfig::default()
    }
}

fn signal_without_rsi_confirmation(
    scenario: &Scenario,
    config: &BotConfig,
    candles: &[Candle],
    live_price: f64,
) -> Option<Signal> {
    let mut permissive = config.clone();
    permissive.rsi_oversold = 0.0;
    permissive.rsi_overbought = 100.0;
    permissive.quantum_trigger_confirmation_enabled = false;
    confirmed_signal_with_gates(
        scenario,
        &permissive,
        candles,
        Some(live_price),
        Some(50.0),
        Utc::now(),
    )
    .signal
}

#[derive(Debug, Clone, Serialize)]
struct GateRow {
    gate: String,
    passed: usize,
    blocked: usize,
    pass_rate: f64,
    passed_stats: Stats,
    blocked_stats: Stats,
}

#[derive(Debug, Clone, Serialize)]
struct TradeAnalysis {
    trigger_time: String,
    scenario_id: String,
    direction: String,
    r_multiple: f64,
    confidence: u8,
    score: u8,
    retest_ok: bool,
    rsi_h1: Option<f64>,
    rsi_h4: Option<f64>,
    distance_ema_atr: f64,
    macro_bias: Option<String>,
    macro_alignment: Option<String>,
    macro_real_yield_change_5d: Option<f64>,
    macro_dollar_change_5d_pct: Option<f64>,
    passed: BTreeMap<String, bool>,
}

fn load_triggers(path: &PathBuf) -> Result<Vec<Trade>, Box<dyn std::error::Error>> {
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut trades = Vec::new();
    let mut skipped = 0usize;
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(&line)?;
        let Some(r_multiple) = value.get("r_multiple").and_then(|value| value.as_f64()) else {
            skipped += 1;
            continue;
        };
        let mut trade: Trade = serde_json::from_value(value)?;
        trade.r_multiple = r_multiple;
        trades.push(trade);
    }
    println!(
        "loaded {} triggers from {} (skipped {} without outcome)",
        trades.len(),
        path.display(),
        skipped
    );
    Ok(trades)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut report_path = PathBuf::from("data/backtest/exness_retest_variant2.json");
    let mut cache_path = PathBuf::from("data/backtest/XAUUSDm_M15.jsonl");
    let mut out_path = PathBuf::from("data/backtest/gate_ab.json");
    let mut variant = "quantum_v3_htf_retest".to_string();
    let mut macro_dir: Option<PathBuf> = None;
    let mut triggers_path: Option<PathBuf> = None;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--report" => report_path = PathBuf::from(args.next().ok_or("--report value")?),
            "--cache" => cache_path = PathBuf::from(args.next().ok_or("--cache value")?),
            "--out" => out_path = PathBuf::from(args.next().ok_or("--out value")?),
            "--variant" => variant = args.next().ok_or("--variant value")?,
            "--macro-dir" => {
                macro_dir = Some(PathBuf::from(args.next().ok_or("--macro-dir value")?))
            }
            "--triggers" => {
                triggers_path = Some(PathBuf::from(args.next().ok_or("--triggers value")?))
            }
            "--help" => {
                println!("Usage: backtest_gate_ab [--report PATH] [--triggers PATH] [--cache PATH] [--macro-dir PATH] [--out PATH] [--variant NAME]");
                return Ok(());
            }
            other => return Err(format!("unknown flag {other}").into()),
        }
    }

    let trades: Vec<Trade> = if let Some(path) = triggers_path {
        load_triggers(&path)?
    } else {
        let report: Report = serde_json::from_str(&fs::read_to_string(&report_path)?)?;
        report
            .variants
            .get(&variant)
            .ok_or_else(|| format!("variant {variant} not found"))?
            .trades
            .clone()
    };
    let candles = load_candles(&cache_path)?;
    if candles.is_empty() {
        return Err("no M15 candles".into());
    }

    let config = base_config();
    let macro_history = macro_dir
        .as_deref()
        .map(MacroHistory::load)
        .transpose()
        .map_err(|err| format!("macro history: {err}"))?;
    let gate_names = [
        "confirmation",
        "rsi_extreme",
        "anti_chase",
        "htf",
        "macro_counter",
        "buy_rsi",
        "macro_history",
        "all",
        "all_with_macro",
    ];
    let mut passed_map: BTreeMap<String, Vec<f64>> = gate_names
        .iter()
        .map(|name| (name.to_string(), Vec::new()))
        .collect();
    let mut blocked_map: BTreeMap<String, Vec<f64>> = gate_names
        .iter()
        .map(|name| (name.to_string(), Vec::new()))
        .collect();
    let mut analyses = Vec::new();
    let mut missing_trigger = 0usize;
    let mut empty_context = 0usize;
    let mut base_signal_none = 0usize;
    let mut pending_signal_none = 0usize;

    for trade in &trades {
        let Some(trigger_close_ms) = parse_ms(&trade.trigger_time) else {
            missing_trigger += 1;
            continue;
        };
        let trigger_open_ms = trigger_close_ms - M15_MS;
        let Some(trigger_idx) = candles
            .iter()
            .position(|candle| candle.timestamp == trigger_open_ms)
        else {
            missing_trigger += 1;
            continue;
        };
        let m15 = &candles[..=trigger_idx];
        let Some(trigger_candle) = m15.last() else {
            continue;
        };
        let as_of_ms = trigger_close_ms;
        let d1_all = aggregate(m15, D1_MS, Timeframe::H4);
        let h4_all = aggregate(m15, H4_MS, Timeframe::H4);
        let h1_all = aggregate(m15, H1_MS, Timeframe::H1);
        let d1 = closed_before(&d1_all, D1_MS, as_of_ms);
        let h4 = closed_before(&h4_all, H4_MS, as_of_ms);
        let h1 = closed_before(&h1_all, H1_MS, as_of_ms);
        if d1.is_empty() || h4.is_empty() || h1.is_empty() {
            empty_context += 1;
            continue;
        }
        let context = build_market_context(d1, h4, h1, "EXNESS", None);
        let rsi_m15 = rsi_snapshot(m15, 14, 70.0, 30.0, 40, 2);
        let rsi_h1 = context.rsi_h1.as_ref().and_then(|rsi| rsi.value);
        let rsi_h4 = context.rsi_h4.as_ref().and_then(|rsi| rsi.value);
        let scenario = scenario_from_trade(trade);
        let Some(base_signal) =
            signal_without_rsi_confirmation(&scenario, &config, m15, trigger_candle.close)
        else {
            base_signal_none += 1;
            continue;
        };
        let Some(pending_signal) = signal_from_scenario(
            &scenario,
            &config,
            m15,
            Some(trigger_candle.close),
            Utc::now(),
        ) else {
            pending_signal_none += 1;
            continue;
        };
        let daily_change = if context.d1.prev_close.is_finite() && context.d1.prev_close > 0.0 {
            Some((trigger_candle.close - context.d1.prev_close) / context.d1.prev_close * 100.0)
        } else {
            None
        };
        let anti_chase = anti_chase_blocks_breakout(
            pending_signal.signal_type,
            pending_signal.entry_type,
            daily_change,
            rsi_h1,
            rsi_m15.divergence,
            &config,
        );
        let htf = htf_allows_signal(
            base_signal.signal_type.as_str(),
            &context,
            config.htf_h4_filter_enabled,
            config.htf_d1_filter_enabled,
        );
        let macro_block = counter_macro_block_reason(&config, &base_signal);
        let buy_rsi = buy_rsi_h1_block_reason(&config, &base_signal, rsi_h1);

        let mut permissive = config.clone();
        permissive.rsi_oversold = 0.0;
        permissive.rsi_overbought = 100.0;
        let confirmation = confirmed_signal_with_gates(
            &scenario,
            &BotConfig {
                quantum_trigger_confirmation_enabled: true,
                ..permissive.clone()
            },
            m15,
            Some(trigger_candle.close),
            Some(50.0),
            Utc::now(),
        )
        .signal
        .is_some();
        let rsi_gate = confirmed_signal_with_gates(
            &scenario,
            &permissive,
            m15,
            Some(trigger_candle.close),
            rsi_m15.value,
            Utc::now(),
        )
        .signal
        .is_some();

        let macro_point = macro_history
            .as_ref()
            .map(|history| history.at(trigger_close_ms, &trade.direction));
        let macro_history_pass = macro_point
            .as_ref()
            .map(|point| !point.alignment.eq_ignore_ascii_case("COUNTER"))
            .unwrap_or(true);
        let mut passed = BTreeMap::new();
        passed.insert("confirmation".to_string(), confirmation);
        passed.insert("rsi_extreme".to_string(), rsi_gate);
        passed.insert("anti_chase".to_string(), anti_chase.is_none());
        passed.insert("htf".to_string(), htf.0);
        passed.insert("macro_counter".to_string(), macro_block.is_none());
        passed.insert("buy_rsi".to_string(), buy_rsi.is_none());
        let all_core = confirmation
            && rsi_gate
            && anti_chase.is_none()
            && htf.0
            && macro_block.is_none()
            && buy_rsi.is_none();
        passed.insert("macro_history".to_string(), macro_history_pass);
        passed.insert("all".to_string(), all_core);
        passed.insert("all_with_macro".to_string(), all_core && macro_history_pass);

        for name in gate_names {
            if passed.get(name).copied().unwrap_or(false) {
                passed_map.get_mut(name).unwrap().push(trade.r_multiple);
            } else {
                blocked_map.get_mut(name).unwrap().push(trade.r_multiple);
            }
        }

        let ema50 = gold_signal_bot::analyzer::GoldMarketAnalyzer::calculate_ema(m15, 50);
        let atr = context.h4.atr14.max(0.0001);
        let distance_ema_atr = (trigger_candle.close - ema50).abs() / atr;
        let score = score_scenario(
            &scenario,
            Some(&context),
            gold_signal_bot::market_strategist::volume_surge_ratio(m15, 20),
        );
        analyses.push(TradeAnalysis {
            trigger_time: trade.trigger_time.clone(),
            scenario_id: trade.scenario_id.clone(),
            direction: trade.direction.clone(),
            r_multiple: trade.r_multiple,
            confidence: trade.confidence,
            score: score.total,
            retest_ok: trade.retest_ok,
            rsi_h1,
            rsi_h4,
            distance_ema_atr,
            macro_bias: macro_point.as_ref().map(|point| point.bias.clone()),
            macro_alignment: macro_point.as_ref().map(|point| point.alignment.clone()),
            macro_real_yield_change_5d: macro_point
                .as_ref()
                .and_then(|point| point.real_yield_change_5d),
            macro_dollar_change_5d_pct: macro_point
                .as_ref()
                .and_then(|point| point.dollar_change_5d_pct),
            passed,
        });
    }

    println!(
        "skip: missing_trigger={missing_trigger} empty_context={empty_context} base_signal_none={base_signal_none} pending_signal_none={pending_signal_none} analyzed={}",
        analyses.len()
    );
    let baseline = stats(
        &analyses
            .iter()
            .map(|analysis| analysis.r_multiple)
            .collect::<Vec<_>>(),
    );
    let rows: Vec<GateRow> = gate_names
        .iter()
        .map(|name| {
            let passed = passed_map.get(*name).cloned().unwrap_or_default();
            let blocked = blocked_map.get(*name).cloned().unwrap_or_default();
            GateRow {
                gate: name.to_string(),
                passed: passed.len(),
                blocked: blocked.len(),
                pass_rate: if passed.len() + blocked.len() > 0 {
                    passed.len() as f64 / (passed.len() + blocked.len()) as f64
                } else {
                    0.0
                },
                passed_stats: stats(&passed),
                blocked_stats: stats(&blocked),
            }
        })
        .collect();

    let output = json!({
        "report": report_path,
        "variant": variant,
        "candles": candles.len(),
        "baseline": baseline,
        "gates": rows,
        "trades": analyses,
    });
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&out_path, serde_json::to_string_pretty(&output)?)?;
    println!("wrote {}", out_path.display());
    if let Some(rows) = output["gates"].as_array() {
        for row in rows {
            println!(
                "{}: pass {} / block {} ({:.1}%) | pass avgR {:.3} PF {:.2} | block avgR {:.3} PF {:.2}",
                row["gate"].as_str().unwrap_or("?"),
                row["passed"].as_u64().unwrap_or(0),
                row["blocked"].as_u64().unwrap_or(0),
                row["pass_rate"].as_f64().unwrap_or(0.0) * 100.0,
                row["passed_stats"]["avg_r"].as_f64().unwrap_or(0.0),
                row["passed_stats"]["pf"].as_f64().unwrap_or(0.0),
                row["blocked_stats"]["avg_r"].as_f64().unwrap_or(0.0),
                row["blocked_stats"]["pf"].as_f64().unwrap_or(0.0),
            );
        }
    }
    Ok(())
}
