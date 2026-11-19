//! Reproducible backtest for the deterministic scenario / early-entry engine.
//!
//! The binary replays closed M15 candles, rebuilds the exact D1/H4/H1 context
//! that the live scanner uses, calls `build_scenarios`, takes the PRIMARY valid
//! scenario, waits for its M15 trigger, and simulates SL/TP1/TP2 forward.
//!
//! Data source: Binance public PAXGUSDT M15 klines (XAU/USD proxy). Candles are
//! cached as JSONL so repeated runs are deterministic and offline.
//!
//! This binary is intentionally isolated: it does not modify the production
//! scanner, does not call any AI endpoint, and makes no network request when the
//! local cache already covers the requested period.

use chrono::{DateTime, Days, FixedOffset, NaiveDate, SecondsFormat, Timelike, Utc};
use gold_signal_bot::analyzer::GoldMarketAnalyzer;
use gold_signal_bot::decision_trace::zone_quality;
use gold_signal_bot::macro_flow::{MacroFlowScorecard, MacroFlowSnapshot};
use gold_signal_bot::market_context::{build_market_context, htf_allows_signal};
use gold_signal_bot::models::{BotConfig, Candle, MacroBias, SignalType, Timeframe};
use gold_signal_bot::scenarios::{build_scenarios, scenario_triggered, Scenario, ScenarioResponse};
use gold_signal_bot::trigger_journal::regime_label;
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::error::Error;
use std::fmt;
use std::fmt::Write as FmtWrite;
use std::fs::{self, File};
use std::io::Write as IoWrite;
use std::io::{BufRead, BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::time::Duration as StdDuration;

const WINDOW_SIZE: usize = 300;
const FETCH_LIMIT: usize = 1000;
const FETCH_SLEEP_MS: u64 = 50;
const MAX_FETCH_RETRIES: usize = 3;
const BINANCE_KLINES_URL: &str = "https://api.binance.com/api/v3/klines";

const M15_MS: i64 = 15 * 60 * 1000;
const H1_MS: i64 = 60 * 60 * 1000;
const H4_MS: i64 = 4 * H1_MS;
const D1_MS: i64 = 24 * H1_MS;

/// Wait up to eight closed M15 candles for the scenario trigger to print.
const TRIGGER_WAIT_BARS: usize = 8;
/// Research: wait up to this many M15 bars for a breakout/breakdown retest.
const RETEST_WAIT_BARS: usize = 8;
/// Research: SL buffer beyond the retest candle high/low.
const RETEST_BUFFER_USD: f64 = 3.0;
/// Forward simulation horizon: 96 M15 candles = 24 hours.
const MAX_HOLD_BARS: usize = 96;
/// The live context is built from 300 HTF candles.
const CONTEXT_CANDLES: usize = 300;

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("scenario backtest failed: {err}");
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------------------
// Errors and CLI
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum AppError {
    Message(String),
    Io(std::io::Error),
    Http(reqwest::Error),
    Json(serde_json::Error),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Message(message) => write!(f, "{message}"),
            Self::Io(err) => write!(f, "I/O error: {err}"),
            Self::Http(err) => write!(f, "HTTP error: {err}"),
            Self::Json(err) => write!(f, "JSON error: {err}"),
        }
    }
}

impl Error for AppError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Http(err) => Some(err),
            Self::Json(err) => Some(err),
            Self::Message(_) => None,
        }
    }
}

impl From<std::io::Error> for AppError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<reqwest::Error> for AppError {
    fn from(value: reqwest::Error) -> Self {
        Self::Http(value)
    }
}

impl From<serde_json::Error> for AppError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

impl From<String> for AppError {
    fn from(value: String) -> Self {
        Self::Message(value)
    }
}

impl From<&str> for AppError {
    fn from(value: &str) -> Self {
        Self::Message(value.to_string())
    }
}

type Result<T> = std::result::Result<T, AppError>;

#[derive(Debug, Clone)]
struct Args {
    symbol: String,
    start: NaiveDate,
    end: NaiveDate,
    limit_days: Option<usize>,
    cache_path: PathBuf,
    csv_path: Option<PathBuf>,
    cost_r: f64,
    quantum_volume_ratio: f64,
    confirmation_mode: ConfirmationMode,
    out_json: PathBuf,
    out_md: PathBuf,
    dump_triggers: PathBuf,
}

impl Default for Args {
    fn default() -> Self {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        Self {
            symbol: "PAXGUSDT".to_string(),
            start: NaiveDate::from_ymd_opt(2025, 1, 1).expect("valid default start date"),
            end: NaiveDate::from_ymd_opt(2025, 12, 31).expect("valid default end date"),
            limit_days: None,
            cache_path: manifest_dir.join("data/backtest/PAXGUSDT_M15.jsonl"),
            csv_path: None,
            cost_r: 0.0,
            quantum_volume_ratio: 1.2,
            confirmation_mode: ConfirmationMode::Directional,
            out_json: manifest_dir.join("report_scenarios.json"),
            out_md: manifest_dir.join("report_scenarios.md"),
            dump_triggers: manifest_dir.join("data/backtest/exness_raw_triggers.jsonl"),
        }
    }
}

fn print_usage() {
    println!(
        "Usage: backtest_scenarios [options]\n\
         \n\
         Options:\n\
           --start <YYYY-MM-DD>    Start date, UTC inclusive (default: 2025-01-01)\n\
           --end <YYYY-MM-DD>      End date, UTC inclusive (default: 2025-12-31)\n\
           --limit-days <N>        Cap the run at N days from --start\n\
           --symbol <SYMBOL>       Binance symbol (default: PAXGUSDT)\n\
           --cache <PATH>          JSONL candle cache path\n\
           --csv <PATH>            Read M15 CSV (timestamp,open,high,low,close,volume) instead of Binance\n\
           --cost-r <R>            Subtract R cost per trade (default: 0.0)\n\
           --quantum-volume-ratio <R>  Trigger confirmation volume threshold (default: 1.2)\n\
           --confirmation-mode <MODE>  volume | directional | directional_close | directional_body (default: directional)\n\
           --out-json <PATH>       JSON report path (default: rust-backend/report_scenarios.json)\n\
           --out-md <PATH>         Markdown report path (default: rust-backend/report_scenarios.md)\n\
           --help                  Show this help\n\
         \n\
         The binary only fetches Binance when the local cache does not cover the period.\n\
         No AI calls, no API keys, no production module changes."
    );
}

fn require_value(iter: &mut impl Iterator<Item = String>, flag: &str) -> Result<String> {
    iter.next()
        .ok_or_else(|| AppError::Message(format!("missing value for {flag}")))
}

fn parse_args() -> Result<Args> {
    let mut args = Args::default();
    let mut iter = std::env::args().skip(1);

    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--start" => {
                let value = require_value(&mut iter, &flag)?;
                args.start = NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d")
                    .map_err(|_| AppError::Message(format!("invalid --start date: {value}")))?;
            }
            "--end" => {
                let value = require_value(&mut iter, &flag)?;
                args.end = NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d")
                    .map_err(|_| AppError::Message(format!("invalid --end date: {value}")))?;
            }
            "--limit-days" => {
                let value = require_value(&mut iter, &flag)?;
                let parsed = value
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| AppError::Message(format!("invalid --limit-days: {value}")))?;
                if parsed == 0 {
                    return Err(AppError::Message(
                        "--limit-days must be greater than zero".to_string(),
                    ));
                }
                args.limit_days = Some(parsed);
            }
            "--symbol" => {
                let value = require_value(&mut iter, &flag)?.trim().to_ascii_uppercase();
                if value.is_empty() || !value.chars().all(|c| c.is_ascii_alphanumeric()) {
                    return Err(AppError::Message(format!("invalid symbol: {value}")));
                }
                args.symbol = value;
            }
            "--cache" => {
                args.cache_path = PathBuf::from(require_value(&mut iter, &flag)?);
            }
            "--csv" => {
                args.csv_path = Some(PathBuf::from(require_value(&mut iter, &flag)?));
            }
            "--cost-r" => {
                let value = require_value(&mut iter, &flag)?;
                args.cost_r = value
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| AppError::Message(format!("invalid --cost-r: {value}")))?;
                if !args.cost_r.is_finite() || args.cost_r < 0.0 {
                    return Err(AppError::Message(
                        "--cost-r must be finite and >= 0".to_string(),
                    ));
                }
            }
            "--quantum-volume-ratio" => {
                let value = require_value(&mut iter, &flag)?;
                args.quantum_volume_ratio = value.trim().parse::<f64>().map_err(|_| {
                    AppError::Message(format!("invalid --quantum-volume-ratio: {value}"))
                })?;
                if !args.quantum_volume_ratio.is_finite() || args.quantum_volume_ratio <= 0.0 {
                    return Err(AppError::Message(
                        "--quantum-volume-ratio must be finite and > 0".to_string(),
                    ));
                }
            }
            "--confirmation-mode" => {
                let value = require_value(&mut iter, &flag)?;
                args.confirmation_mode = ConfirmationMode::parse(&value).ok_or_else(|| {
                    AppError::Message(format!("invalid --confirmation-mode: {value}"))
                })?;
            }
            "--out-json" => {
                args.out_json = PathBuf::from(require_value(&mut iter, &flag)?);
            }
            "--out-md" => {
                args.out_md = PathBuf::from(require_value(&mut iter, &flag)?);
            }
            "--dump-triggers" => {
                args.dump_triggers = PathBuf::from(require_value(&mut iter, &flag)?);
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            other => {
                return Err(AppError::Message(format!(
                    "unknown argument: {other} (use --help)"
                )))
            }
        }
    }

    if let Some(days) = args.limit_days {
        let last = args
            .start
            .checked_add_days(Days::new((days - 1) as u64))
            .ok_or_else(|| {
                AppError::Message("--limit-days overflows the date range".to_string())
            })?;
        if last < args.end {
            args.end = last;
        }
    }

    if args.start > args.end {
        return Err(AppError::Message(
            "--start must be earlier than or equal to --end".to_string(),
        ));
    }

    Ok(args)
}

fn date_start_ms(date: NaiveDate) -> i64 {
    date.and_hms_opt(0, 0, 0)
        .expect("midnight is valid")
        .and_utc()
        .timestamp_millis()
}

fn date_end_ms(date: NaiveDate) -> i64 {
    date.and_hms_milli_opt(23, 59, 59, 999)
        .expect("end of day is valid")
        .and_utc()
        .timestamp_millis()
}

// ---------------------------------------------------------------------------
// Data download and cache
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct FetchOutcome {
    candles: Vec<Candle>,
    requests: usize,
}

fn value_to_f64(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.parse::<f64>().ok()))
}

fn value_to_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|s| s.parse::<i64>().ok()))
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn round4(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

fn is_valid_candle(candle: &Candle) -> bool {
    candle.timestamp > 0
        && candle.open.is_finite()
        && candle.high.is_finite()
        && candle.low.is_finite()
        && candle.close.is_finite()
        && candle.volume.is_finite()
        && candle.taker_buy_volume.is_finite()
        && candle.high >= candle.low
        && candle.volume >= 0.0
        && candle.taker_buy_volume >= 0.0
}

/// Parse one Binance kline row. Field 9 is the taker buy base asset volume.
fn parse_binance_kline(row: &Value, timeframe: Timeframe) -> Option<Candle> {
    let items = row.as_array()?;
    if items.len() < 7 {
        return None;
    }

    let timestamp = value_to_i64(&items[0])?;
    let open = value_to_f64(&items[1])?;
    let high = value_to_f64(&items[2])?;
    let low = value_to_f64(&items[3])?;
    let close = value_to_f64(&items[4])?;
    let volume = value_to_f64(&items[5])?;
    let taker_buy_volume = items
        .get(9)
        .and_then(value_to_f64)
        .filter(|value| value.is_finite() && *value >= 0.0)
        .map(round2)
        .unwrap_or(0.0);

    let candle = Candle {
        timestamp,
        open,
        high,
        low,
        close,
        volume,
        taker_buy_volume,
        timeframe,
    };

    is_valid_candle(&candle).then_some(candle)
}

async fn fetch_binance_klines(
    client: &reqwest::Client,
    symbol: &str,
    interval: &str,
    start_ms: i64,
    end_ms: i64,
    timeframe: Timeframe,
) -> Result<FetchOutcome> {
    let mut candles: Vec<Candle> = Vec::new();
    let mut cursor = start_ms;
    let mut requests = 0usize;
    let mut progress_bucket = 0usize;

    while cursor <= end_ms {
        let url = format!(
            "{BINANCE_KLINES_URL}?symbol={symbol}&interval={interval}&startTime={cursor}&endTime={end_ms}&limit={FETCH_LIMIT}"
        );

        let mut retries = 0usize;
        let payload = loop {
            let response = client.get(&url).send().await?;
            requests += 1;
            let status = response.status();

            if status.is_success() {
                break response.json::<Value>().await?;
            }

            let retryable =
                status.as_u16() == 429 || status.as_u16() == 418 || status.is_server_error();
            if retryable && retries < MAX_FETCH_RETRIES {
                retries += 1;
                let wait_ms = 1_000 * retries as u64;
                eprintln!(
                    "  warning: Binance HTTP {status}; retrying in {wait_ms}ms ({retries}/{MAX_FETCH_RETRIES})"
                );
                tokio::time::sleep(StdDuration::from_millis(wait_ms)).await;
                continue;
            }

            return Err(AppError::Message(format!(
                "Binance HTTP {status} for {symbol} {interval}"
            )));
        };

        let rows = payload
            .as_array()
            .ok_or_else(|| AppError::Message("Binance response is not an array".to_string()))?;

        if rows.is_empty() {
            break;
        }

        let mut last_timestamp: Option<i64> = None;
        let mut new_candles = 0usize;

        for row in rows {
            let Some(candle) = parse_binance_kline(row, timeframe) else {
                continue;
            };
            if candle.timestamp < start_ms || candle.timestamp > end_ms {
                continue;
            }
            last_timestamp = Some(
                last_timestamp.map_or(candle.timestamp, |previous| previous.max(candle.timestamp)),
            );
            candles.push(candle);
            new_candles += 1;
        }

        if new_candles == 0 {
            break;
        }

        let Some(last_timestamp) = last_timestamp else {
            break;
        };
        cursor = last_timestamp.saturating_add(1);

        let bucket = candles.len() / 10_000;
        if bucket > progress_bucket {
            progress_bucket = bucket;
            println!(
                "  fetched {} candles (through {})",
                candles.len(),
                format_ts(last_timestamp)
            );
        }

        // A short page means Binance has no more candles in the requested range.
        if rows.len() < FETCH_LIMIT {
            break;
        }

        tokio::time::sleep(StdDuration::from_millis(FETCH_SLEEP_MS)).await;
    }

    candles.sort_by_key(|candle| candle.timestamp);
    candles.dedup_by_key(|candle| candle.timestamp);

    Ok(FetchOutcome { candles, requests })
}

fn cache_covers(candles: &[Candle], start_ms: i64, end_ms: i64, step_ms: i64) -> bool {
    let (Some(first), Some(last)) = (candles.first(), candles.last()) else {
        return false;
    };
    let tolerance = step_ms.saturating_mul(2);
    first.timestamp <= start_ms.saturating_add(tolerance)
        && last.timestamp.saturating_add(tolerance) >= end_ms
}

fn load_cache(path: &Path, timeframe: Timeframe) -> Result<Vec<Candle>> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut candles = Vec::new();

    for (line_number, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Candle>(&line) {
            Ok(candle) if candle.timeframe == timeframe && is_valid_candle(&candle) => {
                candles.push(candle);
            }
            Ok(candle) => {
                eprintln!(
                    "  warning: skipping cache line {} (timeframe {:?}, invalid values)",
                    line_number + 1,
                    candle.timeframe
                );
            }
            Err(err) => {
                eprintln!(
                    "  warning: skipping malformed cache line {}: {err}",
                    line_number + 1
                );
            }
        }
    }

    candles.sort_by_key(|candle| candle.timestamp);
    candles.dedup_by_key(|candle| candle.timestamp);
    Ok(candles)
}

fn load_csv(path: &Path, timeframe: Timeframe) -> Result<Vec<Candle>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut candles = Vec::new();

    for (line_number, line) in reader.lines().enumerate() {
        let line = line?;
        if line_number == 0 || line.trim().is_empty() {
            continue;
        }
        let mut columns = line.split(',');
        let timestamp = columns
            .next()
            .and_then(|value| value.trim().parse::<i64>().ok());
        let open = columns
            .next()
            .and_then(|value| value.trim().parse::<f64>().ok());
        let high = columns
            .next()
            .and_then(|value| value.trim().parse::<f64>().ok());
        let low = columns
            .next()
            .and_then(|value| value.trim().parse::<f64>().ok());
        let close = columns
            .next()
            .and_then(|value| value.trim().parse::<f64>().ok());
        let volume = columns
            .next()
            .and_then(|value| value.trim().parse::<f64>().ok())
            .unwrap_or(0.0);

        let (Some(timestamp), Some(open), Some(high), Some(low), Some(close)) =
            (timestamp, open, high, low, close)
        else {
            eprintln!("  warning: skipping malformed CSV line {}", line_number + 1);
            continue;
        };
        let candle = Candle {
            timestamp,
            open,
            high,
            low,
            close,
            volume,
            taker_buy_volume: 0.0,
            timeframe,
        };
        if is_valid_candle(&candle) {
            candles.push(candle);
        } else {
            eprintln!("  warning: skipping invalid CSV line {}", line_number + 1);
        }
    }

    candles.sort_by_key(|candle| candle.timestamp);
    candles.dedup_by_key(|candle| candle.timestamp);
    Ok(candles)
}

fn write_cache(path: &Path, candles: &[Candle]) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("candles.jsonl");
    let tmp_path = path.with_file_name(format!("{file_name}.tmp"));

    let file = File::create(&tmp_path)?;
    let mut writer = BufWriter::new(file);
    for candle in candles {
        serde_json::to_writer(&mut writer, candle)?;
        writer.write_all(b"\n")?;
    }
    writer.flush()?;
    fs::rename(&tmp_path, path)?;
    Ok(())
}

fn m15_atr14(candles: &[Candle], index: usize) -> f64 {
    if index == 0 {
        return 0.0;
    }
    let start = index.saturating_sub(13);
    let mut sum = 0.0;
    let mut count = 0usize;
    for i in start..=index {
        let candle = &candles[i];
        let prev_close = if i > 0 {
            candles[i - 1].close
        } else {
            candle.close
        };
        let tr = (candle.high - candle.low)
            .max((candle.high - prev_close).abs())
            .max((candle.low - prev_close).abs());
        if tr.is_finite() {
            sum += tr;
            count += 1;
        }
    }
    if count == 0 {
        0.0
    } else {
        sum / count as f64
    }
}

fn write_trigger_dump(path: &Path, triggers: &[TriggerRecord]) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let file = File::create(path)?;
    let mut writer = BufWriter::new(file);
    for trigger in triggers {
        serde_json::to_writer(&mut writer, trigger)?;
        writer.write_all(b"\n")?;
    }
    writer.flush()?;
    println!("Wrote {} ({} raw triggers)", path.display(), triggers.len());
    Ok(())
}

async fn load_or_fetch(args: &Args, start_ms: i64, end_ms: i64) -> Result<(Vec<Candle>, usize)> {
    let mut candles = load_cache(&args.cache_path, Timeframe::M15)?;
    let mut requests = 0usize;

    if !cache_covers(&candles, start_ms, end_ms, M15_MS) {
        println!(
            "Cache {} does not cover {} .. {}; fetching Binance {} M15...",
            args.cache_path.display(),
            format_ts(start_ms),
            format_ts(end_ms),
            args.symbol
        );
        let client = reqwest::Client::builder()
            .timeout(StdDuration::from_secs(30))
            .build()?;
        let outcome = fetch_binance_klines(
            &client,
            &args.symbol,
            "15m",
            start_ms,
            end_ms,
            Timeframe::M15,
        )
        .await?;
        requests = outcome.requests;

        if outcome.candles.is_empty() {
            return Err(AppError::Message(format!(
                "Binance returned no M15 candles for {} between {} and {}",
                args.symbol,
                format_ts(start_ms),
                format_ts(end_ms)
            )));
        }

        candles.extend(outcome.candles);
        candles.sort_by_key(|candle| candle.timestamp);
        candles.dedup_by_key(|candle| candle.timestamp);
        write_cache(&args.cache_path, &candles)?;
        println!(
            "Cached {} candles at {}",
            candles.len(),
            args.cache_path.display()
        );
    }

    Ok((candles, requests))
}

// ---------------------------------------------------------------------------
// Time and candle helpers
// ---------------------------------------------------------------------------

fn format_ts(timestamp_ms: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(timestamp_ms)
        .map(|datetime| datetime.to_rfc3339_opts(SecondsFormat::Secs, true))
        .unwrap_or_else(|| timestamp_ms.to_string())
}

fn vietnam_offset() -> FixedOffset {
    FixedOffset::east_opt(7 * 3600).expect("valid UTC+7 offset")
}

/// VN 13:00-23:00 session used by the report breakdown.
fn session_bucket(timestamp_ms: i64) -> &'static str {
    let Some(datetime) = DateTime::<Utc>::from_timestamp_millis(timestamp_ms) else {
        return "UNKNOWN";
    };
    let vietnam = datetime.with_timezone(&vietnam_offset());
    let minutes = vietnam.hour() * 60 + vietnam.minute();
    if (13 * 60..23 * 60).contains(&minutes) {
        "VN_13_23"
    } else {
        "OUTSIDE"
    }
}

fn rsi_bucket(value: Option<f64>) -> String {
    match value {
        Some(value) if value < 30.0 => "<30".to_string(),
        Some(value) if value <= 65.0 => "30-65".to_string(),
        Some(_) => ">65".to_string(),
        None => "unknown".to_string(),
    }
}

/// Aggregate M15 candles into fixed UTC buckets (H1, H4, D1).
///
/// Timestamps are bucket starts. Open = first open, high = max, low = min,
/// close = last close, volume = sum. Input must be sorted by timestamp.
///
/// `models::Timeframe` has no D1 variant, so D1 candles carry `H4` metadata.
/// `build_market_context("D1", ...)` and `detect_ema_trend_bias` never read the
/// metadata field; the D1 bucket size is supplied explicitly by the caller.
fn aggregate_candles(candles: &[Candle], bucket_ms: i64, timeframe: Timeframe) -> Vec<Candle> {
    if bucket_ms <= 0 {
        return Vec::new();
    }

    let mut aggregated = Vec::new();
    let mut iter = candles.iter().peekable();

    while let Some(first) = iter.next() {
        let bucket = first.timestamp.div_euclid(bucket_ms) * bucket_ms;
        let mut high = first.high;
        let mut low = first.low;
        let mut close = first.close;
        let mut volume = first.volume;
        let mut taker_buy_volume = first.taker_buy_volume;

        while let Some(next) = iter.peek() {
            if next.timestamp.div_euclid(bucket_ms) * bucket_ms != bucket {
                break;
            }
            let next = iter.next().expect("peeked candle");
            high = high.max(next.high);
            low = low.min(next.low);
            close = next.close;
            volume += next.volume;
            taker_buy_volume += next.taker_buy_volume;
        }

        aggregated.push(Candle {
            timestamp: bucket,
            open: first.open,
            high,
            low,
            close,
            volume,
            taker_buy_volume,
            timeframe,
        });
    }

    aggregated
}

/// Last `max_len` candles fully closed at or before `as_of_ms`.
///
/// Candle timestamps are open times, so this deliberately uses
/// `timestamp + timeframe_ms` to prevent using an in-progress HTF candle.
fn closed_slice(candles: &[Candle], timeframe_ms: i64, as_of_ms: i64, max_len: usize) -> &[Candle] {
    let end =
        candles.partition_point(|candle| candle.timestamp.saturating_add(timeframe_ms) <= as_of_ms);
    if end == 0 {
        return &[];
    }
    let start = end.saturating_sub(max_len);
    &candles[start..end]
}

fn count_missing_bars(candles: &[Candle], step_ms: i64) -> usize {
    candles
        .windows(2)
        .map(|pair| {
            let delta = pair[1].timestamp.saturating_sub(pair[0].timestamp);
            if delta > step_ms {
                (delta / step_ms).saturating_sub(1) as usize
            } else {
                0
            }
        })
        .sum()
}

// ---------------------------------------------------------------------------
// Scenario inputs
// ---------------------------------------------------------------------------

fn macro_flow_for(d1: &[Candle], now_ms: i64) -> MacroFlowSnapshot {
    let bias = GoldMarketAnalyzer::detect_ema_trend_bias(d1);
    let macro_bias = bias.as_str().to_string();
    let direction = match bias {
        MacroBias::Bullish => "UP",
        MacroBias::Bearish => "DOWN",
        MacroBias::Neutral => "MIXED",
    };

    MacroFlowSnapshot {
        updated_at: format_ts(now_ms),
        fred_ok: false,
        yahoo_ok: false,
        cot_ok: false,
        metrics: Vec::new(),
        cot: None,
        scorecard: MacroFlowScorecard {
            macro_bias,
            confidence: 80,
            score: 0,
            direction: direction.to_string(),
            driver: "BACKTEST_D1_EMA".to_string(),
            // Metrics are intentionally empty. UNKNOWN keeps the alignment
            // driven solely by the deterministic D1 EMA bias instead of
            // pretending the flow data is sufficient.
            flow_quality: "UNKNOWN".to_string(),
            risk_regime: "NEUTRAL".to_string(),
            rate_regime: "NEUTRAL".to_string(),
            usd_regime: "NEUTRAL".to_string(),
            vol_regime: "NORMAL".to_string(),
            notes: Vec::new(),
        },
        warnings: Vec::new(),
        sources: Vec::new(),
    }
}

fn primary_scenario(response: &ScenarioResponse) -> Option<&Scenario> {
    response.scenarios.iter().find(|scenario| {
        scenario.role.eq_ignore_ascii_case("PRIMARY")
            && scenario.valid
            && scenario
                .entry
                .is_some_and(|value| value.is_finite() && value > 0.0)
            && scenario
                .stop_loss
                .is_some_and(|value| value.is_finite() && value > 0.0)
            && scenario
                .take_profit_1
                .is_some_and(|value| value.is_finite() && value > 0.0)
            && scenario
                .take_profit_2
                .is_some_and(|value| value.is_finite() && value > 0.0)
            && scenario
                .risk_reward
                .is_some_and(|value| value.is_finite() && value > 0.0)
    })
}

fn side_for_scenario(scenario: &Scenario) -> Option<SignalType> {
    match scenario.bias.trim().to_ascii_uppercase().as_str() {
        "BULLISH" => Some(SignalType::Buy),
        "BEARISH" => Some(SignalType::Sell),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy)]
struct ScenarioLevels {
    entry: f64,
    stop_loss: f64,
    take_profit_1: f64,
    take_profit_2: f64,
    risk_reward: f64,
}

fn levels_from_scenario(scenario: &Scenario, side: SignalType) -> Option<ScenarioLevels> {
    let entry = scenario.entry?;
    let stop_loss = scenario.stop_loss?;
    let take_profit_1 = scenario.take_profit_1?;
    let take_profit_2 = scenario.take_profit_2?;
    let risk_reward = scenario.risk_reward?;

    let risk = (entry - stop_loss).abs();
    if !risk.is_finite() || risk <= 1e-9 || !risk_reward.is_finite() || risk_reward <= 0.0 {
        return None;
    }

    let ordered = match side {
        SignalType::Buy => {
            stop_loss < entry && entry < take_profit_1 && take_profit_1 < take_profit_2
        }
        SignalType::Sell => {
            stop_loss > entry && entry > take_profit_1 && take_profit_1 > take_profit_2
        }
    };
    if !ordered {
        return None;
    }

    Some(ScenarioLevels {
        entry,
        stop_loss,
        take_profit_1,
        take_profit_2,
        risk_reward,
    })
}

// ---------------------------------------------------------------------------
// Fill model and trade simulation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
enum FillKind {
    #[serde(rename = "LEVEL")]
    Level,
    #[serde(rename = "GAP")]
    Gap,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct FillResult {
    fill_index: usize,
    fill_price: f64,
    kind: FillKind,
    slippage: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum FillOutcome {
    Filled(FillResult),
    SkippedNoFill,
    SkippedGapTooLarge,
}

/// Resolve the actual fill for a triggered scenario.
///
/// TOUCH/LIMIT: the entry level must trade in the next `TRIGGER_WAIT_BARS`
/// closed M15 candles. The trigger bar itself is excluded because the trigger
/// is only known at its close. No touch => no trade.
///
/// ABOVE/BELOW BREAKOUT: the level fills at `scenario.entry` unless the trigger
/// candle opens beyond it. A gap fill at the candle open is accepted only when
/// slippage is at most `0.5R`; otherwise the trade is skipped.
/// Trigger confirmation: require a directional close/rejection and a
/// volume expansion before a scenario trigger is accepted.
fn trigger_confirmation_ok(
    side: SignalType,
    trigger_direction: &str,
    candles: &[Candle],
    index: usize,
    volume_ratio: f64,
    mode: ConfirmationMode,
) -> bool {
    let Some(candle) = candles.get(index) else {
        return false;
    };
    let previous = index
        .checked_sub(1)
        .and_then(|previous| candles.get(previous));

    let period = 20;
    let start = index.saturating_sub(period);
    let window = &candles[start..index];
    let average_volume = if window.is_empty() {
        0.0
    } else {
        window.iter().map(|candle| candle.volume).sum::<f64>() / window.len() as f64
    };
    let volume_ok = average_volume > 0.0 && candle.volume >= average_volume * volume_ratio.max(1.0);
    if !volume_ok {
        return false;
    }

    let direction = trigger_direction.trim().to_ascii_uppercase();
    let range = candle.high - candle.low;
    let body = (candle.close - candle.open).abs();
    let close_position = if range > 0.0 {
        (candle.close - candle.low) / range
    } else {
        0.5
    };
    let directional_ok = match (side, direction.as_str()) {
        (SignalType::Buy, "ABOVE") => candle.close > candle.open,
        (SignalType::Sell, "BELOW") => candle.close < candle.open,
        _ => false,
    };

    match (side, direction.as_str()) {
        (SignalType::Buy | SignalType::Sell, "TOUCH") => {
            let (bullish_pinbar, bearish_pinbar) =
                GoldMarketAnalyzer::check_pinbar_rejection(candle, 2.0);
            let (bullish_engulfing, bearish_engulfing) = previous
                .map(|previous| GoldMarketAnalyzer::check_engulfing(previous, candle))
                .unwrap_or((false, false));
            match side {
                SignalType::Buy => bullish_pinbar || bullish_engulfing,
                SignalType::Sell => bearish_pinbar || bearish_engulfing,
            }
        }
        (SignalType::Buy, "ABOVE") | (SignalType::Sell, "BELOW") => match mode {
            ConfirmationMode::Volume => true,
            ConfirmationMode::Directional => directional_ok,
            ConfirmationMode::DirectionalClose => {
                directional_ok
                    && if side == SignalType::Buy {
                        close_position >= 0.70
                    } else {
                        close_position <= 0.30
                    }
            }
            ConfirmationMode::DirectionalBody => {
                directional_ok && range > 0.0 && body / range >= 0.50
            }
        },
        _ => false,
    }
}

/// Research: convert a breakout/breakdown trigger into a retest entry.
///
/// Wait up to `RETEST_WAIT_BARS` M15 bars for price to retest the broken level
/// and reject (bearish close below for SELL, bullish close above for BUY).
/// Entry stays at the broken level; SL sits beyond the retest candle high/low
/// plus `RETEST_BUFFER_USD`; TP1/TP2 are 2R/3R. Returns `None` when no retest
/// appears or price runs too far before retesting.
fn build_retest_levels(
    side: SignalType,
    trigger_direction: &str,
    levels: ScenarioLevels,
    trigger_idx: usize,
    m15: &[Candle],
    end_idx: usize,
) -> Option<(usize, ScenarioLevels)> {
    let direction = trigger_direction.trim().to_ascii_uppercase();
    if direction != "BELOW" && direction != "ABOVE" {
        return None;
    }
    let entry = levels.entry;
    let original_risk = (levels.entry - levels.stop_loss).abs();
    if !original_risk.is_finite() || original_risk <= 0.0 {
        return None;
    }
    let last = (trigger_idx + RETEST_WAIT_BARS).min(end_idx);
    for (j, candle) in m15.iter().enumerate().take(last + 1).skip(trigger_idx + 1) {
        match side {
            SignalType::Sell if direction == "BELOW" => {
                if candle.high >= entry && candle.close < entry && candle.close < candle.open {
                    let stop_loss = candle.high.max(entry) + RETEST_BUFFER_USD;
                    let risk = (entry - stop_loss).abs();
                    if !risk.is_finite() || risk <= 0.0 {
                        return None;
                    }
                    return Some((
                        j,
                        ScenarioLevels {
                            entry,
                            stop_loss,
                            take_profit_1: entry - (risk * 2.0 + 0.01),
                            take_profit_2: entry - (risk * 3.0 + 0.01),
                            risk_reward: 2.0,
                        },
                    ));
                }
                if candle.low < entry - original_risk {
                    return None;
                }
            }
            SignalType::Buy if direction == "ABOVE" => {
                if candle.low <= entry && candle.close > entry && candle.close > candle.open {
                    let stop_loss = candle.low.min(entry) - RETEST_BUFFER_USD;
                    let risk = (stop_loss - entry).abs();
                    if !risk.is_finite() || risk <= 0.0 {
                        return None;
                    }
                    return Some((
                        j,
                        ScenarioLevels {
                            entry,
                            stop_loss,
                            take_profit_1: entry + (risk * 2.0 + 0.01),
                            take_profit_2: entry + (risk * 3.0 + 0.01),
                            risk_reward: 2.0,
                        },
                    ));
                }
                if candle.high > entry + original_risk {
                    return None;
                }
            }
            _ => {}
        }
    }
    None
}

fn resolve_fill(
    side: SignalType,
    trigger_direction: &str,
    levels: ScenarioLevels,
    trigger_index: usize,
    m15: &[Candle],
    end_idx: usize,
) -> FillOutcome {
    let scenario_risk = (levels.entry - levels.stop_loss).abs();
    if !scenario_risk.is_finite() || scenario_risk <= 1e-9 {
        return FillOutcome::SkippedNoFill;
    }

    let direction = trigger_direction.trim().to_ascii_uppercase();
    match (side, direction.as_str()) {
        (SignalType::Buy | SignalType::Sell, "TOUCH") => {
            let last = (trigger_index + TRIGGER_WAIT_BARS).min(end_idx);
            for (index, candle) in m15
                .iter()
                .enumerate()
                .take(last + 1)
                .skip(trigger_index + 1)
            {
                if candle.low <= levels.entry && levels.entry <= candle.high {
                    return FillOutcome::Filled(FillResult {
                        fill_index: index,
                        fill_price: levels.entry,
                        kind: FillKind::Level,
                        slippage: 0.0,
                    });
                }
            }
            FillOutcome::SkippedNoFill
        }
        (SignalType::Buy, "ABOVE") => {
            let trigger_candle = &m15[trigger_index];
            if trigger_candle.open > levels.entry {
                let slippage = trigger_candle.open - levels.entry;
                if slippage <= 0.5 * scenario_risk {
                    FillOutcome::Filled(FillResult {
                        fill_index: trigger_index,
                        fill_price: trigger_candle.open,
                        kind: FillKind::Gap,
                        slippage,
                    })
                } else {
                    FillOutcome::SkippedGapTooLarge
                }
            } else {
                FillOutcome::Filled(FillResult {
                    fill_index: trigger_index,
                    fill_price: levels.entry,
                    kind: FillKind::Level,
                    slippage: 0.0,
                })
            }
        }
        (SignalType::Sell, "BELOW") => {
            let trigger_candle = &m15[trigger_index];
            if trigger_candle.open < levels.entry {
                let slippage = levels.entry - trigger_candle.open;
                if slippage <= 0.5 * scenario_risk {
                    FillOutcome::Filled(FillResult {
                        fill_index: trigger_index,
                        fill_price: trigger_candle.open,
                        kind: FillKind::Gap,
                        slippage,
                    })
                } else {
                    FillOutcome::SkippedGapTooLarge
                }
            } else {
                FillOutcome::Filled(FillResult {
                    fill_index: trigger_index,
                    fill_price: levels.entry,
                    kind: FillKind::Level,
                    slippage: 0.0,
                })
            }
        }
        _ => FillOutcome::SkippedNoFill,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
enum ExitReason {
    #[serde(rename = "TP1")]
    Tp1,
    #[serde(rename = "SL")]
    StopLoss,
    #[serde(rename = "TIMEOUT")]
    Timeout,
}

#[derive(Debug, Clone, Copy)]
struct SimResult {
    exit_reason: ExitReason,
    exit_price: f64,
    r_multiple: f64,
    tp2_before_sl: bool,
    bars_held: usize,
}

/// Signed R for an exit price. Used for timeout exits; SL is fixed at -1R and
/// TP1 uses the scenario's own R:R for level fills.
fn r_multiple(side: SignalType, entry: f64, stop_loss: f64, exit_price: f64) -> Option<f64> {
    let risk = (entry - stop_loss).abs();
    if !risk.is_finite() || risk <= 1e-9 {
        return None;
    }
    let reward = match side {
        SignalType::Buy => exit_price - entry,
        SignalType::Sell => entry - exit_price,
    };
    Some(reward / risk)
}

/// Simulate a trade on closed M15 candles *after* the fill candle.
///
/// Headline exit: first SL or TP1. Same-candle SL+TP1 counts as SL
/// (conservative). If neither prints within the horizon, exit at the last close.
///
/// `tp2_before_sl` is a separate hypothetical runner: it answers "would TP2
/// have printed before SL/timeout?" even when the headline trade exited at TP1.
fn simulate_trade(
    side: SignalType,
    levels: ScenarioLevels,
    fill_price: f64,
    forward: &[Candle],
) -> Option<SimResult> {
    let bars = forward.len().min(MAX_HOLD_BARS);
    if bars == 0 {
        return None;
    }

    let risk = (fill_price - levels.stop_loss).abs();
    if !risk.is_finite() || risk <= 1e-9 {
        return None;
    }

    let mut headline: Option<(usize, ExitReason, f64)> = None;
    for (offset, candle) in forward.iter().take(bars).enumerate() {
        let (sl_hit, tp1_hit) = match side {
            SignalType::Buy => (
                candle.low <= levels.stop_loss,
                candle.high >= levels.take_profit_1,
            ),
            SignalType::Sell => (
                candle.high >= levels.stop_loss,
                candle.low <= levels.take_profit_1,
            ),
        };

        // Conservative same-candle rule: SL is evaluated before TP1.
        if sl_hit {
            headline = Some((offset, ExitReason::StopLoss, levels.stop_loss));
            break;
        }
        if tp1_hit {
            headline = Some((offset, ExitReason::Tp1, levels.take_profit_1));
            break;
        }
    }

    let (exit_offset, exit_reason, exit_price) = match headline {
        Some(value) => value,
        None => {
            let last = &forward[bars - 1];
            (bars - 1, ExitReason::Timeout, last.close)
        }
    };

    let r_multiple = match exit_reason {
        ExitReason::Tp1 => {
            if (fill_price - levels.entry).abs() < 1e-9 {
                levels.risk_reward
            } else {
                let reward = (levels.take_profit_1 - fill_price).abs();
                let r = reward / risk;
                if !r.is_finite() || r <= 0.0 {
                    return None;
                }
                r
            }
        }
        ExitReason::StopLoss => -1.0,
        ExitReason::Timeout => r_multiple(side, fill_price, levels.stop_loss, exit_price)?,
    };

    let mut tp2_before_sl = false;
    for candle in forward.iter().take(bars) {
        let (sl_hit, tp2_hit) = match side {
            SignalType::Buy => (
                candle.low <= levels.stop_loss,
                candle.high >= levels.take_profit_2,
            ),
            SignalType::Sell => (
                candle.high >= levels.stop_loss,
                candle.low <= levels.take_profit_2,
            ),
        };
        if sl_hit {
            break;
        }
        if tp2_hit {
            tp2_before_sl = true;
            break;
        }
    }

    Some(SimResult {
        exit_reason,
        exit_price,
        r_multiple,
        tp2_before_sl,
        bars_held: exit_offset + 1,
    })
}

// ---------------------------------------------------------------------------
// Metrics
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone)]
struct Stats {
    trades: usize,
    wins: usize,
    losses: usize,
    timeouts: usize,
    tp2_hits: usize,
    total_r: f64,
    r_sum_sq: f64,
    gross_profit: f64,
    gross_loss: f64,
    max_drawdown_r: f64,
    equity: f64,
    peak: f64,
}

#[derive(Debug, Clone, Serialize)]
struct StatsSummary {
    trades: usize,
    wins: usize,
    losses: usize,
    timeouts: usize,
    win_rate: f64,
    avg_r: f64,
    expectancy_r: f64,
    profit_factor: Option<f64>,
    max_drawdown_r: f64,
    total_r: f64,
    tp2_hits: usize,
    tp2_before_sl_rate: f64,
    tp2_rate_of_wins: f64,
    timeout_rate: f64,
    std_r: Option<f64>,
    standard_error_r: Option<f64>,
    t_stat: Option<f64>,
    ci95_low_r: Option<f64>,
    ci95_high_r: Option<f64>,
}

impl Stats {
    fn record(&mut self, trade: &TradeRecord) {
        self.record_values(trade.exit_reason, trade.r_multiple, trade.tp2_before_sl);
    }

    fn record_values(&mut self, reason: ExitReason, r: f64, tp2_before_sl: bool) {
        self.trades += 1;
        match reason {
            ExitReason::Tp1 => self.wins += 1,
            ExitReason::StopLoss => self.losses += 1,
            ExitReason::Timeout => self.timeouts += 1,
        }
        if tp2_before_sl {
            self.tp2_hits += 1;
        }

        self.total_r += r;
        self.r_sum_sq += r * r;
        if r > 0.0 {
            self.gross_profit += r;
        } else if r < 0.0 {
            self.gross_loss += -r;
        }

        self.equity += r;
        if self.equity > self.peak {
            self.peak = self.equity;
        }
        let drawdown = self.peak - self.equity;
        if drawdown > self.max_drawdown_r {
            self.max_drawdown_r = drawdown;
        }
    }

    fn summary(&self) -> StatsSummary {
        let trades = self.trades as f64;
        let win_rate = if self.trades == 0 {
            0.0
        } else {
            self.wins as f64 / trades
        };
        let avg_r = if self.trades == 0 {
            0.0
        } else {
            self.total_r / trades
        };
        let tp2_before_sl_rate = if self.trades == 0 {
            0.0
        } else {
            self.tp2_hits as f64 / trades
        };
        let tp2_rate_of_wins = if self.wins == 0 {
            0.0
        } else {
            self.tp2_hits as f64 / self.wins as f64
        };
        let timeout_rate = if self.trades == 0 {
            0.0
        } else {
            self.timeouts as f64 / trades
        };
        let profit_factor = if self.gross_loss > 1e-12 {
            Some(self.gross_profit / self.gross_loss)
        } else {
            None
        };
        let (std_r, standard_error_r, t_stat, ci95_low_r, ci95_high_r) = if self.trades >= 2 {
            let n = self.trades as f64;
            let variance = ((self.r_sum_sq - n * avg_r * avg_r) / (n - 1.0)).max(0.0);
            let std = variance.sqrt();
            let se = std / n.sqrt();
            let t = if se > 0.0 { avg_r / se } else { 0.0 };
            (
                Some(std),
                Some(se),
                Some(t),
                Some(avg_r - 1.96 * se),
                Some(avg_r + 1.96 * se),
            )
        } else {
            (None, None, None, None, None)
        };

        StatsSummary {
            trades: self.trades,
            wins: self.wins,
            losses: self.losses,
            timeouts: self.timeouts,
            win_rate: round4(win_rate),
            avg_r: round4(avg_r),
            expectancy_r: round4(avg_r),
            profit_factor: profit_factor.map(round4),
            max_drawdown_r: round4(self.max_drawdown_r),
            total_r: round4(self.total_r),
            tp2_hits: self.tp2_hits,
            tp2_before_sl_rate: round4(tp2_before_sl_rate),
            tp2_rate_of_wins: round4(tp2_rate_of_wins),
            timeout_rate: round4(timeout_rate),
            std_r: std_r.map(round4),
            standard_error_r: standard_error_r.map(round4),
            t_stat: t_stat.map(round4),
            ci95_low_r: ci95_low_r.map(round4),
            ci95_high_r: ci95_high_r.map(round4),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct TradeRecord {
    signal_time: String,
    trigger_time: String,
    fill_time: String,
    exit_time: String,
    scenario_id: String,
    scenario_name: String,
    direction: String,
    trigger_direction: String,
    confidence: u8,
    macro_alignment: String,
    entry: f64,
    fill_price: f64,
    fill_kind: FillKind,
    slippage: f64,
    slippage_r: f64,
    stop_loss: f64,
    take_profit_1: f64,
    take_profit_2: f64,
    risk: f64,
    risk_reward: f64,
    exit_reason: ExitReason,
    exit_price: f64,
    r_multiple: f64,
    tp2_before_sl: bool,
    bars_held: usize,
    session: String,
    macro_bias: String,
    rsi_h1: Option<f64>,
    rsi_h1_bucket: String,
    early_entry_eligible: bool,
    entry_within_trigger_range: bool,
    period_end_forced_exit: bool,
}

/// Raw trigger record for gate A/B. Unlike `TradeRecord`, this includes
/// triggers that later failed retest/fill. It is written to a separate JSONL
/// file so the main report stays compact.
#[derive(Debug, Clone, Serialize)]
struct TriggerRecord {
    trigger_time: String,
    original_trigger_time: String,
    retest_time: Option<String>,
    original_entry: f64,
    original_stop_loss: f64,
    original_take_profit_1: f64,
    original_take_profit_2: f64,
    retest_entry: Option<f64>,
    retest_stop_loss: Option<f64>,
    retest_take_profit_1: Option<f64>,
    retest_take_profit_2: Option<f64>,
    m15_atr: f64,
    zone_touch_count: Option<usize>,
    zone_width_atr: Option<f64>,
    scenario_id: String,
    scenario_name: String,
    direction: String,
    trigger_direction: String,
    confidence: u8,
    macro_alignment: String,
    entry: f64,
    stop_loss: f64,
    take_profit_1: f64,
    take_profit_2: f64,
    risk_reward: f64,
    retest_ok: bool,
    exit_reason: Option<ExitReason>,
    r_multiple: Option<f64>,
    bars_held: Option<usize>,
    session: String,
    macro_bias: String,
    rsi_h1: Option<f64>,
    regime: String,
    h4_atr: f64,
}

#[derive(Debug, Default)]
struct Breakdowns {
    by_scenario: BTreeMap<String, Stats>,
    by_session: BTreeMap<String, Stats>,
    by_macro_bias: BTreeMap<String, Stats>,
    by_macro_alignment: BTreeMap<String, Stats>,
    by_rsi_h1: BTreeMap<String, Stats>,
    by_trigger_direction: BTreeMap<String, Stats>,
    by_direction: BTreeMap<String, Stats>,
    by_early_entry: BTreeMap<String, Stats>,
}

impl Breakdowns {
    fn record(&mut self, trade: &TradeRecord) {
        self.by_scenario
            .entry(trade.scenario_id.clone())
            .or_default()
            .record(trade);
        self.by_session
            .entry(trade.session.clone())
            .or_default()
            .record(trade);
        self.by_macro_bias
            .entry(trade.macro_bias.clone())
            .or_default()
            .record(trade);
        self.by_macro_alignment
            .entry(trade.macro_alignment.clone())
            .or_default()
            .record(trade);
        self.by_rsi_h1
            .entry(trade.rsi_h1_bucket.clone())
            .or_default()
            .record(trade);
        self.by_trigger_direction
            .entry(trade.trigger_direction.clone())
            .or_default()
            .record(trade);
        self.by_direction
            .entry(trade.direction.clone())
            .or_default()
            .record(trade);
        let early_entry = if trade.early_entry_eligible {
            ">=90"
        } else {
            "<90"
        };
        self.by_early_entry
            .entry(early_entry.to_string())
            .or_default()
            .record(trade);
    }
}

#[derive(Debug, Clone, Serialize)]
struct BreakdownReport {
    by_scenario: BTreeMap<String, StatsSummary>,
    by_session: BTreeMap<String, StatsSummary>,
    by_macro_bias: BTreeMap<String, StatsSummary>,
    by_macro_alignment: BTreeMap<String, StatsSummary>,
    by_rsi_h1: BTreeMap<String, StatsSummary>,
    by_trigger_direction: BTreeMap<String, StatsSummary>,
    by_direction: BTreeMap<String, StatsSummary>,
    by_early_entry: BTreeMap<String, StatsSummary>,
}

fn summarize_map(map: &BTreeMap<String, Stats>) -> BTreeMap<String, StatsSummary> {
    map.iter()
        .map(|(key, stats)| (key.clone(), stats.summary()))
        .collect()
}

impl Breakdowns {
    fn report(&self) -> BreakdownReport {
        BreakdownReport {
            by_scenario: summarize_map(&self.by_scenario),
            by_session: summarize_map(&self.by_session),
            by_macro_bias: summarize_map(&self.by_macro_bias),
            by_macro_alignment: summarize_map(&self.by_macro_alignment),
            by_rsi_h1: summarize_map(&self.by_rsi_h1),
            by_trigger_direction: summarize_map(&self.by_trigger_direction),
            by_direction: summarize_map(&self.by_direction),
            by_early_entry: summarize_map(&self.by_early_entry),
        }
    }
}

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
struct PeriodInfo {
    start: String,
    end: String,
    start_ms: i64,
    end_ms: i64,
    limit_days: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
struct DataInfo {
    symbol: String,
    interval: String,
    source: String,
    cache_path: String,
    candles_loaded: usize,
    candles_in_period: usize,
    missing_m15_bars_in_period: usize,
    first_timestamp: Option<String>,
    last_timestamp: Option<String>,
    fetch_requests: usize,
}

#[derive(Debug, Clone, Serialize)]
struct ConfigInfo {
    min_risk_reward: f64,
    early_entry_enabled: bool,
    early_entry_min_confidence: u8,
    lookback_period: usize,
    window_size: usize,
    context_candles: usize,
    trigger_wait_bars: usize,
    max_hold_bars: usize,
    one_open_trade: bool,
}

#[derive(Debug, Clone, Serialize)]
struct FillModelInfo {
    touch_fill_window_bars: usize,
    breakout_max_gap_r: f64,
    fill_bar_offset: String,
    same_candle_rule: String,
    description: String,
}

#[derive(Debug, Default, Clone, Serialize)]
struct Funnel {
    bars_in_period: usize,
    bars_skipped_while_open: usize,
    setups_evaluated: usize,
    primary_valid: usize,
    primary_skipped_counter: usize,
    htf_filter_skipped: usize,
    retest_skipped: usize,
    buy_rsi_h1_skipped: usize,
    trigger_found: usize,
    trigger_confirmation_skipped: usize,
    skipped_no_forward: usize,
    entries_filled_at_level: usize,
    entries_gapped: usize,
    entries_skipped_no_fill: usize,
    entries_skipped_gap_too_large: usize,
    entry_within_trigger_range: usize,
    entry_outside_trigger_range: usize,
    period_end_forced_exits: usize,
    trades: usize,
}

#[derive(Debug, Clone, Serialize)]
struct VariantConfig {
    name: String,
    description: String,
    anti_chase_filter_enabled: bool,
    aligned_only: bool,
    htf_filters: bool,
    breakdown_retest: bool,
    buy_rsi_h1_guard: bool,
    bearish_rejection_enabled: bool,
    htf_levels_enabled: bool,
    structural_sl_atr_cap: f64,
    quantum_weighted_score_enabled: bool,
    trigger_confirmation_enabled: bool,
    quantum_volume_confirm_ratio: f64,
    confirmation_mode: String,
}

#[derive(Debug, Clone, Serialize)]
struct VariantReport {
    config: VariantConfig,
    funnel: Funnel,
    overall: StatsSummary,
    breakdowns: BreakdownReport,
    trades: Vec<TradeRecord>,
}

#[derive(Debug, Clone, Serialize)]
struct ComparisonRow {
    variant: String,
    trades: usize,
    wins: usize,
    losses: usize,
    timeouts: usize,
    win_rate: f64,
    avg_r: f64,
    expectancy_r: f64,
    profit_factor: Option<f64>,
    max_drawdown_r: f64,
    total_r: f64,
    tp2_before_sl_rate: f64,
    timeout_rate: f64,
    entries_filled_at_level: usize,
    entries_gapped: usize,
    entries_skipped_no_fill: usize,
    counter_skipped: usize,
    entry_outside_trigger_range: usize,
}

#[derive(Debug, Clone, Serialize)]
struct Report {
    generated_at: String,
    command: String,
    period: PeriodInfo,
    data: DataInfo,
    config: ConfigInfo,
    cost_r: f64,
    fill_model: FillModelInfo,
    assumptions: Vec<String>,
    comparison: Vec<ComparisonRow>,
    variants: BTreeMap<String, VariantReport>,
}

fn assumptions() -> Vec<String> {
    vec![
        "PAXGUSDT is a free proxy for XAU/USD; the PAXG premium/discount is not modeled.".to_string(),
        "The macro layer is deterministic only: D1 EMA bias, confidence 80, score 0, no metrics/COT/news and no AI review.".to_string(),
        "One open trade at a time; new setups are skipped while a trade is open, and setup evaluation resumes on the M15 bar after the exit bar.".to_string(),
        "TOUCH/LIMIT entries require the scenario entry level to trade in the next 8 closed M15 bars after the trigger; if it never trades, the trade is skipped.".to_string(),
        "BREAKOUT entries fill at the scenario entry level; if the trigger candle opens beyond the level, they fill at the candle open only when slippage is <= 0.5R, otherwise they are skipped.".to_string(),
        "Forward simulation starts on the M15 candle after the fill candle; the fill candle itself is not evaluated for SL/TP.".to_string(),
        "No news filter or general slippage is modeled; --cost-r subtracts a fixed R cost per trade, and only the breakout gap rule above is applied.".to_string(),
        "Headline exit is TP1; TP2-before-SL is a separate hypothetical runner that ignores the TP1 exit.".to_string(),
        "If SL and TP print in the same M15 candle, SL wins (conservative).".to_string(),
        "Win rate = TP1 wins / all simulated trades; timeouts are neither wins nor losses. Profit factor and avg R include timeout exits.".to_string(),
        "HTF context uses only closed D1/H4/H1 candles; the scenario window uses the last 300 closed M15 candles.".to_string(),
        "Trades still open at the period end are force-closed at the last close and flagged as period_end_forced_exit.".to_string(),
        "Session breakdown uses VN time (UTC+7), 13:00-23:00 vs outside, measured at the fill bar.".to_string(),
        "RSI H1 bucket is measured at the signal bar (when the PRIMARY scenario is built), not at the fill bar.".to_string(),
        "entry_outside_trigger_range is retained as a diagnostic of the trigger candle only; it no longer implies an assumed fill.".to_string(),
        "No network is used when the local JSONL cache covers the requested period; no API keys or AI endpoints are used.".to_string(),
    ]
}

fn config_info(config: &BotConfig) -> ConfigInfo {
    ConfigInfo {
        min_risk_reward: config.min_risk_reward,
        early_entry_enabled: config.early_entry_enabled,
        early_entry_min_confidence: config.early_entry_min_confidence,
        lookback_period: config.lookback_period,
        window_size: WINDOW_SIZE,
        context_candles: CONTEXT_CANDLES,
        trigger_wait_bars: TRIGGER_WAIT_BARS,
        max_hold_bars: MAX_HOLD_BARS,
        one_open_trade: true,
    }
}

fn fill_model_info() -> FillModelInfo {
    FillModelInfo {
        touch_fill_window_bars: TRIGGER_WAIT_BARS,
        breakout_max_gap_r: 0.5,
        fill_bar_offset: "Simulation starts on the M15 bar after the fill bar.".to_string(),
        same_candle_rule: "SL wins when SL and TP print in the same M15 candle.".to_string(),
        description: "TOUCH/LIMIT entries must trade at the scenario entry within 8 M15 bars; BREAKOUT entries fill at the level, or at the trigger-candle open on a gap <= 0.5R, otherwise skip.".to_string(),
    }
}

fn comparison_row(name: &str, variant: &VariantReport) -> ComparisonRow {
    ComparisonRow {
        variant: name.to_string(),
        trades: variant.overall.trades,
        wins: variant.overall.wins,
        losses: variant.overall.losses,
        timeouts: variant.overall.timeouts,
        win_rate: variant.overall.win_rate,
        avg_r: variant.overall.avg_r,
        expectancy_r: variant.overall.expectancy_r,
        profit_factor: variant.overall.profit_factor,
        max_drawdown_r: variant.overall.max_drawdown_r,
        total_r: variant.overall.total_r,
        tp2_before_sl_rate: variant.overall.tp2_before_sl_rate,
        timeout_rate: variant.overall.timeout_rate,
        entries_filled_at_level: variant.funnel.entries_filled_at_level,
        entries_gapped: variant.funnel.entries_gapped,
        entries_skipped_no_fill: variant.funnel.entries_skipped_no_fill,
        counter_skipped: variant.funnel.primary_skipped_counter,
        entry_outside_trigger_range: variant.funnel.entry_outside_trigger_range,
    }
}

fn fmt_pf(profit_factor: Option<f64>, trades: usize) -> String {
    if trades == 0 {
        "n/a".to_string()
    } else {
        profit_factor.map_or_else(|| "inf".to_string(), |value| format!("{value:.2}"))
    }
}

fn fmt_pct(value: f64) -> String {
    format!("{:.1}%", value * 100.0)
}

fn render_markdown(report: &Report) -> String {
    let mut output = String::new();
    let current = report
        .variants
        .get("current")
        .expect("current variant is always present");
    let aligned_only = report
        .variants
        .get("aligned_only")
        .expect("aligned_only variant is always present");

    writeln!(output, "# Scenario / Early-Entry Backtest").expect("write to string");
    writeln!(output).expect("write to string");
    writeln!(
        output,
        "Period: **{}** to **{}** (UTC) · symbol **{}** · interval **{}**",
        report.period.start, report.period.end, report.data.symbol, report.data.interval
    )
    .expect("write to string");
    writeln!(
        output,
        "Data: {} cached at `{}` ({} candles loaded, {} in period, {} missing M15 bars).",
        report.data.source,
        report.data.cache_path,
        report.data.candles_loaded,
        report.data.candles_in_period,
        report.data.missing_m15_bars_in_period
    )
    .expect("write to string");
    writeln!(output).expect("write to string");
    writeln!(output, "Command: `{}`", report.command).expect("write to string");
    writeln!(
        output,
        "Cost assumption: **{:.2}R per trade** (already subtracted from all R metrics).",
        report.cost_r
    )
    .expect("write to string");
    writeln!(output).expect("write to string");

    writeln!(output, "## Fill model").expect("write to string");
    writeln!(output).expect("write to string");
    writeln!(output, "| Rule | Value |").expect("write to string");
    writeln!(output, "| --- | --- |").expect("write to string");
    writeln!(
        output,
        "| TOUCH/LIMIT fill window | {} closed M15 bars after the trigger |",
        report.fill_model.touch_fill_window_bars
    )
    .expect("write to string");
    writeln!(
        output,
        "| BREAKOUT gap fill | Trigger-candle open only when slippage <= {:.1}R |",
        report.fill_model.breakout_max_gap_r
    )
    .expect("write to string");
    writeln!(
        output,
        "| Fill-bar handling | {} |",
        report.fill_model.fill_bar_offset
    )
    .expect("write to string");
    writeln!(
        output,
        "| Same-candle rule | {} |",
        report.fill_model.same_candle_rule
    )
    .expect("write to string");
    writeln!(output, "| Summary | {} |", report.fill_model.description).expect("write to string");
    writeln!(output).expect("write to string");

    writeln!(output, "## Variant comparison").expect("write to string");
    writeln!(output).expect("write to string");
    writeln!(
        output,
        "| Variant | Anti-chase | Skip COUNTER | Description |"
    )
    .expect("write to string");
    writeln!(output, "| --- | --- | --- | --- |").expect("write to string");
    for row in &report.comparison {
        if let Some(variant) = report.variants.get(&row.variant) {
            writeln!(
                output,
                "| {} | {} | {} | {} |",
                row.variant,
                if variant.config.anti_chase_filter_enabled {
                    "ON"
                } else {
                    "OFF"
                },
                if variant.config.aligned_only {
                    "yes"
                } else {
                    "no"
                },
                variant.config.description
            )
            .expect("write to string");
        }
    }
    writeln!(output).expect("write to string");
    write_comparison_markdown(&mut output, &report.comparison);

    writeln!(output, "## Current variant headline").expect("write to string");
    writeln!(output).expect("write to string");
    write_headline_markdown(&mut output, &current.overall);
    if let (Some(low), Some(high), Some(t_stat), Some(standard_error)) = (
        current.overall.ci95_low_r,
        current.overall.ci95_high_r,
        current.overall.t_stat,
        current.overall.standard_error_r,
    ) {
        writeln!(
            output,
            "**Uncertainty:** avg R 95% CI [{low:.3}, {high:.3}] · t={t_stat:.2} · SE={standard_error:.3} · n={}.",
            current.overall.trades
        )
        .expect("write to string");
        writeln!(output).expect("write to string");
    }
    write_funnel_markdown(
        &mut output,
        &current.funnel,
        report.config.trigger_wait_bars,
    );

    write_breakdown_markdown(
        &mut output,
        "By scenario (current)",
        &current.breakdowns.by_scenario,
    );
    write_breakdown_markdown(
        &mut output,
        "By scenario (aligned_only)",
        &aligned_only.breakdowns.by_scenario,
    );
    write_breakdown_markdown(
        &mut output,
        "By session (current)",
        &current.breakdowns.by_session,
    );
    write_breakdown_markdown(
        &mut output,
        "By macro bias (current)",
        &current.breakdowns.by_macro_bias,
    );
    write_breakdown_markdown(
        &mut output,
        "By macro alignment (current)",
        &current.breakdowns.by_macro_alignment,
    );
    write_breakdown_markdown(
        &mut output,
        "By RSI H1 bucket (current)",
        &current.breakdowns.by_rsi_h1,
    );
    write_breakdown_markdown(
        &mut output,
        "By trigger direction (current)",
        &current.breakdowns.by_trigger_direction,
    );
    write_breakdown_markdown(
        &mut output,
        "By early-entry confidence (current)",
        &current.breakdowns.by_early_entry,
    );

    writeln!(output, "## Caveats").expect("write to string");
    writeln!(output).expect("write to string");
    for assumption in &report.assumptions {
        writeln!(output, "- {assumption}").expect("write to string");
    }
    writeln!(output).expect("write to string");
    writeln!(
        output,
        "_This is a research report, not financial advice. The PAXG proxy and the simplified fill model can still differ from real XAU/USD execution._"
    )
    .expect("write to string");

    output
}

fn write_headline_markdown(output: &mut String, overall: &StatsSummary) {
    writeln!(output, "| Metric | Value |").expect("write to string");
    writeln!(output, "| --- | ---: |").expect("write to string");
    writeln!(output, "| Trades | {} |", overall.trades).expect("write to string");
    writeln!(output, "| Wins (TP1) | {} |", overall.wins).expect("write to string");
    writeln!(output, "| Losses (SL) | {} |", overall.losses).expect("write to string");
    writeln!(output, "| Timeouts | {} |", overall.timeouts).expect("write to string");
    writeln!(output, "| Win rate | {} |", fmt_pct(overall.win_rate)).expect("write to string");
    writeln!(output, "| Avg R | {:.3} |", overall.avg_r).expect("write to string");
    writeln!(
        output,
        "| Expectancy (R/trade) | {:.3} |",
        overall.expectancy_r
    )
    .expect("write to string");
    writeln!(
        output,
        "| Profit factor | {} |",
        fmt_pf(overall.profit_factor, overall.trades)
    )
    .expect("write to string");
    writeln!(
        output,
        "| Max drawdown (R) | {:.2} |",
        overall.max_drawdown_r
    )
    .expect("write to string");
    writeln!(output, "| Total R | {:.2} |", overall.total_r).expect("write to string");
    writeln!(
        output,
        "| TP2 before SL | {} ({} of {} trades) |",
        fmt_pct(overall.tp2_before_sl_rate),
        overall.tp2_hits,
        overall.trades
    )
    .expect("write to string");
    writeln!(
        output,
        "| Timeout rate | {} |",
        fmt_pct(overall.timeout_rate)
    )
    .expect("write to string");
    writeln!(output).expect("write to string");
}

fn write_funnel_markdown(output: &mut String, funnel: &Funnel, trigger_wait_bars: usize) {
    writeln!(output, "### Funnel").expect("write to string");
    writeln!(output).expect("write to string");
    writeln!(output, "| Stage | Count |").expect("write to string");
    writeln!(output, "| --- | ---: |").expect("write to string");
    writeln!(output, "| M15 bars in period | {} |", funnel.bars_in_period)
        .expect("write to string");
    writeln!(
        output,
        "| Bars skipped while a trade was open | {} |",
        funnel.bars_skipped_while_open
    )
    .expect("write to string");
    writeln!(output, "| Setups evaluated | {} |", funnel.setups_evaluated)
        .expect("write to string");
    writeln!(
        output,
        "| PRIMARY valid with levels | {} |",
        funnel.primary_valid
    )
    .expect("write to string");
    writeln!(
        output,
        "| PRIMARY skipped by aligned_only (COUNTER) | {} |",
        funnel.primary_skipped_counter
    )
    .expect("write to string");
    writeln!(
        output,
        "| PRIMARY skipped by HTF H4/D1 filter | {} |",
        funnel.htf_filter_skipped
    )
    .expect("write to string");
    writeln!(
        output,
        "| BUY skipped by RSI H1 < 30 | {} |",
        funnel.buy_rsi_h1_skipped
    )
    .expect("write to string");
    writeln!(
        output,
        "| Trigger found within {} bars | {} |",
        trigger_wait_bars, funnel.trigger_found
    )
    .expect("write to string");
    writeln!(
        output,
        "| Triggers skipped by trigger confirmation | {} |",
        funnel.trigger_confirmation_skipped
    )
    .expect("write to string");
    writeln!(
        output,
        "| Entries filled at level | {} |",
        funnel.entries_filled_at_level
    )
    .expect("write to string");
    writeln!(
        output,
        "| Entries gapped (filled at open) | {} |",
        funnel.entries_gapped
    )
    .expect("write to string");
    writeln!(
        output,
        "| Entries skipped, no fill | {} |",
        funnel.entries_skipped_no_fill
    )
    .expect("write to string");
    writeln!(
        output,
        "| ... of which gap > 0.5R | {} |",
        funnel.entries_skipped_gap_too_large
    )
    .expect("write to string");
    writeln!(
        output,
        "| Entry inside trigger candle range | {} |",
        funnel.entry_within_trigger_range
    )
    .expect("write to string");
    writeln!(
        output,
        "| Entry outside trigger candle range (diagnostic) | {} |",
        funnel.entry_outside_trigger_range
    )
    .expect("write to string");
    writeln!(
        output,
        "| Skipped (no forward data) | {} |",
        funnel.skipped_no_forward
    )
    .expect("write to string");
    writeln!(
        output,
        "| Period-end forced exits | {} |",
        funnel.period_end_forced_exits
    )
    .expect("write to string");
    writeln!(output, "| Trades simulated | {} |", funnel.trades).expect("write to string");
    writeln!(output).expect("write to string");
}

fn write_comparison_markdown(output: &mut String, rows: &[ComparisonRow]) {
    writeln!(
        output,
        "| Variant | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Expectancy | PF | Max DD (R) | Total R | TP2 before SL | Timeout rate |"
    )
    .expect("write to string");
    writeln!(
        output,
        "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"
    )
    .expect("write to string");
    for row in rows {
        writeln!(
            output,
            "| {} | {} | {} | {} | {} | {} | {:.3} | {:.3} | {} | {:.2} | {:.2} | {} | {} |",
            row.variant,
            row.trades,
            row.wins,
            row.losses,
            row.timeouts,
            fmt_pct(row.win_rate),
            row.avg_r,
            row.expectancy_r,
            fmt_pf(row.profit_factor, row.trades),
            row.max_drawdown_r,
            row.total_r,
            fmt_pct(row.tp2_before_sl_rate),
            fmt_pct(row.timeout_rate)
        )
        .expect("write to string");
    }
    writeln!(output).expect("write to string");

    writeln!(output, "### Fill accounting").expect("write to string");
    writeln!(output).expect("write to string");
    writeln!(
        output,
        "| Variant | Filled at level | Gapped at open | Skipped, no fill | Counter skipped | Entry outside trigger range |"
    )
    .expect("write to string");
    writeln!(output, "| --- | ---: | ---: | ---: | ---: | ---: |").expect("write to string");
    for row in rows {
        writeln!(
            output,
            "| {} | {} | {} | {} | {} | {} |",
            row.variant,
            row.entries_filled_at_level,
            row.entries_gapped,
            row.entries_skipped_no_fill,
            row.counter_skipped,
            row.entry_outside_trigger_range
        )
        .expect("write to string");
    }
    writeln!(output).expect("write to string");
}

fn write_breakdown_markdown(
    output: &mut String,
    title: &str,
    map: &BTreeMap<String, StatsSummary>,
) {
    writeln!(output, "## {title}").expect("write to string");
    writeln!(output).expect("write to string");
    writeln!(
        output,
        "| Key | Trades | Wins | Losses | Timeouts | Win rate | Avg R | Profit factor | Max DD (R) | TP2 before SL |"
    )
    .expect("write to string");
    writeln!(
        output,
        "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"
    )
    .expect("write to string");
    for (key, stats) in map {
        writeln!(
            output,
            "| {} | {} | {} | {} | {} | {} | {:.3} | {} | {:.2} | {} |",
            key,
            stats.trades,
            stats.wins,
            stats.losses,
            stats.timeouts,
            fmt_pct(stats.win_rate),
            stats.avg_r,
            fmt_pf(stats.profit_factor, stats.trades),
            stats.max_drawdown_r,
            fmt_pct(stats.tp2_before_sl_rate)
        )
        .expect("write to string");
    }
    writeln!(output).expect("write to string");
}

fn write_reports(args: &Args, report: &Report) -> Result<()> {
    if let Some(parent) = args.out_json.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let file = File::create(&args.out_json)?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, report)?;
    writer.write_all(b"\n")?;
    writer.flush()?;

    if let Some(parent) = args.out_md.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    fs::write(&args.out_md, render_markdown(report))?;
    Ok(())
}

fn print_table(title: &str, map: &BTreeMap<String, StatsSummary>) {
    println!("\n{title}");
    println!(
        "{:<24} {:>7} {:>6} {:>7} {:>7} {:>7} {:>7} {:>8}",
        "key", "trades", "wins", "losses", "win%", "avgR", "PF", "maxDD"
    );
    for (key, stats) in map {
        println!(
            "{:<24} {:>7} {:>6} {:>7} {:>6.1} {:>7.3} {:>7} {:>8.2}",
            key,
            stats.trades,
            stats.wins,
            stats.losses,
            stats.win_rate * 100.0,
            stats.avg_r,
            fmt_pf(stats.profit_factor, stats.trades),
            stats.max_drawdown_r
        );
    }
}

fn print_summary(report: &Report) {
    let current = report
        .variants
        .get("current")
        .expect("current variant is always present");
    let aligned_only = report
        .variants
        .get("aligned_only")
        .expect("aligned_only variant is always present");

    println!("\n=== Variant comparison ===");
    println!(
        "{:<32} {:>7} {:>6} {:>7} {:>7} {:>7} {:>7} {:>7} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8}",
        "variant",
        "trades",
        "wins",
        "losses",
        "timeouts",
        "win%",
        "avgR",
        "PF",
        "maxDD",
        "totalR",
        "TP2%",
        "filled",
        "gapped",
        "skip"
    );
    for row in &report.comparison {
        println!(
            "{:<32} {:>7} {:>6} {:>7} {:>7} {:>6.1} {:>7.3} {:>7} {:>8.2} {:>8.2} {:>7.1} {:>7} {:>7} {:>7}",
            row.variant,
            row.trades,
            row.wins,
            row.losses,
            row.timeouts,
            row.win_rate * 100.0,
            row.avg_r,
            fmt_pf(row.profit_factor, row.trades),
            row.max_drawdown_r,
            row.total_r,
            row.tp2_before_sl_rate * 100.0,
            row.entries_filled_at_level,
            row.entries_gapped,
            row.entries_skipped_no_fill
        );
    }
    for row in &report.comparison {
        if row.counter_skipped > 0 {
            println!("{}: counter_skipped={}", row.variant, row.counter_skipped);
        }
    }

    println!("\n=== Current variant ===");
    let overall = &current.overall;
    println!(
        "trades={} wins={} losses={} timeouts={} win_rate={} avg_R={:.3} expectancy_R={:.3} PF={} maxDD_R={:.2} TP2_before_SL={} timeout_rate={}",
        overall.trades,
        overall.wins,
        overall.losses,
        overall.timeouts,
        fmt_pct(overall.win_rate),
        overall.avg_r,
        overall.expectancy_r,
        fmt_pf(overall.profit_factor, overall.trades),
        overall.max_drawdown_r,
        fmt_pct(overall.tp2_before_sl_rate),
        fmt_pct(overall.timeout_rate)
    );
    if let (Some(low), Some(high), Some(t_stat)) =
        (overall.ci95_low_r, overall.ci95_high_r, overall.t_stat)
    {
        println!("uncertainty: avg_R_95CI=[{low:.3}, {high:.3}] t={t_stat:.2}");
    }
    println!(
        "funnel: bars={} setups={} primary_valid={} counter_skipped={} triggered={} filled_level={} gapped={} skipped_no_fill={} gap_too_large={} outside_trigger={} trades={} period_end_exits={}",
        current.funnel.bars_in_period,
        current.funnel.setups_evaluated,
        current.funnel.primary_valid,
        current.funnel.primary_skipped_counter,
        current.funnel.trigger_found,
        current.funnel.entries_filled_at_level,
        current.funnel.entries_gapped,
        current.funnel.entries_skipped_no_fill,
        current.funnel.entries_skipped_gap_too_large,
        current.funnel.entry_outside_trigger_range,
        current.funnel.trades,
        current.funnel.period_end_forced_exits
    );

    print_table("By scenario (current)", &current.breakdowns.by_scenario);
    print_table("By session (current)", &current.breakdowns.by_session);
    print_table("By macro bias (current)", &current.breakdowns.by_macro_bias);
    print_table(
        "By macro alignment (current)",
        &current.breakdowns.by_macro_alignment,
    );
    print_table("By RSI H1 (current)", &current.breakdowns.by_rsi_h1);
    print_table(
        "By trigger direction (current)",
        &current.breakdowns.by_trigger_direction,
    );
    print_table(
        "By early-entry confidence (current)",
        &current.breakdowns.by_early_entry,
    );
    print_table(
        "By scenario (aligned_only)",
        &aligned_only.breakdowns.by_scenario,
    );
}

// ---------------------------------------------------------------------------
// Main run
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfirmationMode {
    Volume,
    Directional,
    DirectionalClose,
    DirectionalBody,
}

impl ConfirmationMode {
    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "volume" | "volume_only" => Some(Self::Volume),
            "directional" => Some(Self::Directional),
            "directional_close" => Some(Self::DirectionalClose),
            "directional_body" => Some(Self::DirectionalBody),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Volume => "volume",
            Self::Directional => "directional",
            Self::DirectionalClose => "directional_close",
            Self::DirectionalBody => "directional_body",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct RunOptions {
    anti_chase_filter_enabled: bool,
    aligned_only: bool,
    htf_filters: bool,
    breakdown_retest: bool,
    buy_rsi_h1_guard: bool,
    bearish_rejection_enabled: bool,
    htf_levels_enabled: bool,
    structural_sl_atr_cap: f64,
    quantum_weighted_score_enabled: bool,
    trigger_confirmation_enabled: bool,
    quantum_volume_confirm_ratio: f64,
    confirmation_mode: ConfirmationMode,
    cost_r: f64,
    dump_triggers: bool,
}

struct RunResult {
    funnel: Funnel,
    overall: StatsSummary,
    breakdowns: BreakdownReport,
    trades: Vec<TradeRecord>,
    triggers: Vec<TriggerRecord>,
}

fn run_variant(
    m15: &[Candle],
    h1_all: &[Candle],
    h4_all: &[Candle],
    d1_all: &[Candle],
    start_idx: usize,
    end_idx: usize,
    options: RunOptions,
) -> RunResult {
    let config = BotConfig {
        min_risk_reward: 2.0,
        anti_chase_filter_enabled: options.anti_chase_filter_enabled,
        bearish_rejection_enabled: options.bearish_rejection_enabled,
        htf_levels_enabled: options.htf_levels_enabled,
        structural_sl_atr_cap: options.structural_sl_atr_cap,
        quantum_weighted_score_enabled: options.quantum_weighted_score_enabled,
        trigger_confirmation_enabled: options.trigger_confirmation_enabled,
        quantum_volume_confirm_ratio: options.quantum_volume_confirm_ratio,
        min_rsi_h1_for_buy: if options.buy_rsi_h1_guard { 30.0 } else { 0.0 },
        early_entry_enabled: true,
        early_entry_min_confidence: 90,
        lookback_period: 30,
        ..BotConfig::default()
    };

    let mut funnel = Funnel {
        bars_in_period: end_idx - start_idx + 1,
        ..Funnel::default()
    };
    let mut overall = Stats::default();
    let mut breakdowns = Breakdowns::default();
    let mut trades: Vec<TradeRecord> = Vec::new();
    let mut trigger_records: Vec<TriggerRecord> = Vec::new();
    let mut busy_until: Option<usize> = None;
    let mut seen_triggers: HashSet<(String, i64, usize)> = HashSet::new();

    for i in start_idx..=end_idx {
        if let Some(busy) = busy_until {
            if i <= busy {
                funnel.bars_skipped_while_open += 1;
                continue;
            }
        }
        funnel.setups_evaluated += 1;

        let as_of_ms = m15[i].timestamp.saturating_add(M15_MS);
        let window = &m15[i + 1 - WINDOW_SIZE..=i];
        let d1 = closed_slice(d1_all, D1_MS, as_of_ms, CONTEXT_CANDLES);
        let h4 = closed_slice(h4_all, H4_MS, as_of_ms, CONTEXT_CANDLES);
        let h1 = closed_slice(h1_all, H1_MS, as_of_ms, CONTEXT_CANDLES);
        let context = build_market_context(d1, h4, h1, "BACKTEST", None);
        let macro_flow = macro_flow_for(d1, as_of_ms);

        let response = build_scenarios(
            window,
            &config,
            Some(m15[i].close),
            Some(&context),
            Some(&macro_flow),
            "BACKTEST",
            "NONE",
            as_of_ms,
        );

        let Some(scenario) = primary_scenario(&response) else {
            continue;
        };
        let Some(side) = side_for_scenario(scenario) else {
            continue;
        };
        let Some(mut levels) = levels_from_scenario(scenario, side) else {
            continue;
        };
        funnel.primary_valid += 1;

        if options.htf_filters {
            let (allowed, _reason) = htf_allows_signal(side.as_str(), &context, true, true);
            if !allowed {
                funnel.htf_filter_skipped += 1;
                continue;
            }
        }

        let rsi_h1 = context.rsi_h1.as_ref().and_then(|snapshot| snapshot.value);
        if options.buy_rsi_h1_guard
            && side == SignalType::Buy
            && rsi_h1.is_some_and(|value| value < 30.0)
        {
            funnel.buy_rsi_h1_skipped += 1;
            continue;
        }

        if options.aligned_only && scenario.macro_alignment.eq_ignore_ascii_case("COUNTER") {
            funnel.primary_skipped_counter += 1;
            continue;
        }

        let last_trigger = (i + TRIGGER_WAIT_BARS).min(end_idx);
        let mut trigger_index: Option<usize> = None;
        for (j, candle) in m15.iter().enumerate().take(last_trigger + 1).skip(i + 1) {
            if scenario_triggered(scenario, candle) {
                if options.trigger_confirmation_enabled
                    && !trigger_confirmation_ok(
                        side,
                        &scenario.trigger_direction,
                        m15,
                        j,
                        options.quantum_volume_confirm_ratio,
                        options.confirmation_mode,
                    )
                {
                    funnel.trigger_confirmation_skipped += 1;
                    continue;
                }
                // Dedup the same scenario id + entry price within one M15 candle.
                let key = (
                    scenario.id.clone(),
                    (levels.entry * 100.0).round() as i64,
                    j,
                );
                if seen_triggers.insert(key) {
                    trigger_index = Some(j);
                }
                break;
            }
        }

        let Some(mut trigger_idx) = trigger_index else {
            continue;
        };
        funnel.trigger_found += 1;

        let original_trigger_idx = trigger_idx;
        let original_levels = levels;
        let mut retest_fill: Option<FillResult> = None;
        let mut retest_ok = true;
        let retest_direction = scenario.trigger_direction.trim().to_ascii_uppercase();
        if options.breakdown_retest && matches!(retest_direction.as_str(), "BELOW" | "ABOVE") {
            match build_retest_levels(
                side,
                &scenario.trigger_direction,
                levels,
                trigger_idx,
                m15,
                end_idx,
            ) {
                Some((retest_idx, retest_levels)) => {
                    levels = retest_levels;
                    trigger_idx = retest_idx;
                    retest_fill = Some(FillResult {
                        fill_index: retest_idx,
                        fill_price: levels.entry,
                        kind: FillKind::Level,
                        slippage: 0.0,
                    });
                }
                None => {
                    funnel.retest_skipped += 1;
                    retest_ok = false;
                }
            }
        }

        if options.dump_triggers {
            let outcome = if trigger_idx < end_idx {
                let forward_end = (trigger_idx + MAX_HOLD_BARS).min(end_idx);
                let forward = &m15[trigger_idx + 1..=forward_end];
                simulate_trade(side, levels, levels.entry, forward)
            } else {
                None
            };
            trigger_records.push(TriggerRecord {
                trigger_time: format_ts(m15[trigger_idx].timestamp + M15_MS),
                original_trigger_time: format_ts(m15[original_trigger_idx].timestamp + M15_MS),
                retest_time: retest_ok.then(|| format_ts(m15[trigger_idx].timestamp + M15_MS)),
                original_entry: original_levels.entry,
                original_stop_loss: original_levels.stop_loss,
                original_take_profit_1: original_levels.take_profit_1,
                original_take_profit_2: original_levels.take_profit_2,
                retest_entry: retest_ok.then_some(levels.entry),
                retest_stop_loss: retest_ok.then_some(levels.stop_loss),
                retest_take_profit_1: retest_ok.then_some(levels.take_profit_1),
                retest_take_profit_2: retest_ok.then_some(levels.take_profit_2),
                m15_atr: m15_atr14(m15, original_trigger_idx),
                zone_touch_count: zone_quality(
                    &m15[..=original_trigger_idx],
                    original_levels.entry,
                    context.h4.atr14,
                )
                .map(|(touches, _)| touches),
                zone_width_atr: zone_quality(
                    &m15[..=original_trigger_idx],
                    original_levels.entry,
                    context.h4.atr14,
                )
                .map(|(_, width)| width),
                scenario_id: scenario.id.clone(),
                scenario_name: scenario.name.clone(),
                direction: side.as_str().to_string(),
                trigger_direction: scenario.trigger_direction.clone(),
                confidence: scenario.confidence,
                macro_alignment: scenario.macro_alignment.clone(),
                entry: levels.entry,
                stop_loss: levels.stop_loss,
                take_profit_1: levels.take_profit_1,
                take_profit_2: levels.take_profit_2,
                risk_reward: levels.risk_reward,
                retest_ok,
                exit_reason: outcome.map(|result| result.exit_reason),
                r_multiple: outcome.map(|result| result.r_multiple),
                bars_held: outcome.map(|result| result.bars_held),
                session: session_bucket(m15[trigger_idx].timestamp + M15_MS).to_string(),
                macro_bias: macro_flow.scorecard.macro_bias.clone(),
                rsi_h1: context.rsi_h1.as_ref().and_then(|snapshot| snapshot.value),
                regime: regime_label(&context.d1.bias, &context.h4.bias, &context.h1.bias),
                h4_atr: context.h4.atr14,
            });
        }
        if !retest_ok {
            continue;
        }

        let trigger_candle = &m15[trigger_idx];
        let entry_within_trigger_range =
            levels.entry >= trigger_candle.low && levels.entry <= trigger_candle.high;
        if entry_within_trigger_range {
            funnel.entry_within_trigger_range += 1;
        } else {
            funnel.entry_outside_trigger_range += 1;
        }

        let fill = match retest_fill {
            Some(fill) => fill,
            None => match resolve_fill(
                side,
                &scenario.trigger_direction,
                levels,
                trigger_idx,
                m15,
                end_idx,
            ) {
                FillOutcome::Filled(fill) => fill,
                FillOutcome::SkippedNoFill => {
                    funnel.entries_skipped_no_fill += 1;
                    continue;
                }
                FillOutcome::SkippedGapTooLarge => {
                    funnel.entries_skipped_no_fill += 1;
                    funnel.entries_skipped_gap_too_large += 1;
                    continue;
                }
            },
        };
        match fill.kind {
            FillKind::Level => funnel.entries_filled_at_level += 1,
            FillKind::Gap => funnel.entries_gapped += 1,
        }

        let forward_end = (fill.fill_index + MAX_HOLD_BARS).min(end_idx);
        if fill.fill_index >= forward_end {
            funnel.skipped_no_forward += 1;
            continue;
        }
        let forward = &m15[fill.fill_index + 1..=forward_end];
        let Some(sim) = simulate_trade(side, levels, fill.fill_price, forward) else {
            funnel.skipped_no_forward += 1;
            continue;
        };

        let exit_index = fill.fill_index + sim.bars_held;
        let exit_candle = &m15[exit_index];
        let rsi_h1 = context.rsi_h1.as_ref().and_then(|snapshot| snapshot.value);
        let macro_bias = macro_flow.scorecard.macro_bias.clone();
        let session = session_bucket(m15[fill.fill_index].timestamp + M15_MS).to_string();
        let scenario_risk = (levels.entry - levels.stop_loss).abs();
        let fill_risk = (fill.fill_price - levels.stop_loss).abs();
        let risk_reward = if fill.kind == FillKind::Level {
            levels.risk_reward
        } else if fill_risk > 1e-9 {
            (levels.take_profit_1 - fill.fill_price).abs() / fill_risk
        } else {
            0.0
        };
        let period_end_forced_exit =
            sim.exit_reason == ExitReason::Timeout && fill.fill_index + MAX_HOLD_BARS > end_idx;
        if period_end_forced_exit {
            funnel.period_end_forced_exits += 1;
        }

        let trade = TradeRecord {
            signal_time: format_ts(as_of_ms),
            trigger_time: format_ts(m15[trigger_idx].timestamp + M15_MS),
            fill_time: format_ts(m15[fill.fill_index].timestamp + M15_MS),
            exit_time: format_ts(exit_candle.timestamp + M15_MS),
            scenario_id: scenario.id.clone(),
            scenario_name: scenario.name.clone(),
            direction: side.as_str().to_string(),
            trigger_direction: scenario.trigger_direction.clone(),
            confidence: scenario.confidence,
            macro_alignment: scenario.macro_alignment.clone(),
            entry: round2(levels.entry),
            fill_price: round2(fill.fill_price),
            fill_kind: fill.kind,
            slippage: round2(fill.slippage),
            slippage_r: round4(if scenario_risk > 1e-9 {
                fill.slippage / scenario_risk
            } else {
                0.0
            }),
            stop_loss: round2(levels.stop_loss),
            take_profit_1: round2(levels.take_profit_1),
            take_profit_2: round2(levels.take_profit_2),
            risk: round2(fill_risk),
            risk_reward: round4(risk_reward),
            exit_reason: sim.exit_reason,
            exit_price: round2(sim.exit_price),
            r_multiple: round4(sim.r_multiple - options.cost_r),
            tp2_before_sl: sim.tp2_before_sl,
            bars_held: sim.bars_held,
            session,
            macro_bias,
            rsi_h1: rsi_h1.map(round2),
            rsi_h1_bucket: rsi_bucket(rsi_h1),
            early_entry_eligible: scenario.confidence >= config.early_entry_min_confidence,
            entry_within_trigger_range,
            period_end_forced_exit,
        };

        overall.record(&trade);
        breakdowns.record(&trade);
        trades.push(trade);
        busy_until = Some(exit_index);
    }

    funnel.trades = trades.len();
    RunResult {
        funnel,
        overall: overall.summary(),
        breakdowns: breakdowns.report(),
        trades,
        triggers: trigger_records,
    }
}

async fn run() -> Result<()> {
    let args = parse_args()?;
    let start_ms = date_start_ms(args.start);
    let end_ms = date_end_ms(args.end);

    println!(
        "Scenario backtest {} .. {} UTC · {} M15",
        args.start, args.end, args.symbol
    );

    let (candles, fetch_requests) = if let Some(csv_path) = &args.csv_path {
        (load_csv(csv_path, Timeframe::M15)?, 0)
    } else {
        load_or_fetch(&args, start_ms, end_ms).await?
    };
    let m15: Vec<Candle> = candles
        .into_iter()
        .filter(|candle| candle.timeframe == Timeframe::M15)
        .collect();

    if m15.is_empty() {
        return Err(AppError::Message(
            "no M15 candles available after loading the cache".to_string(),
        ));
    }

    let first_in_period = m15.partition_point(|candle| candle.timestamp < start_ms);
    let end_exclusive = m15.partition_point(|candle| candle.timestamp <= end_ms);
    if first_in_period >= end_exclusive {
        return Err(AppError::Message(format!(
            "no M15 candles in the requested period {} .. {}",
            args.start, args.end
        )));
    }
    let end_idx = end_exclusive - 1;
    let start_idx = first_in_period.max(WINDOW_SIZE - 1);
    if start_idx > end_idx {
        return Err(AppError::Message(format!(
            "not enough M15 history before {} (need {} warmup candles)",
            args.start, WINDOW_SIZE
        )));
    }

    let period_candles = &m15[first_in_period..=end_idx];
    let h1_all = aggregate_candles(&m15, H1_MS, Timeframe::H1);
    let h4_all = aggregate_candles(&m15, H4_MS, Timeframe::H4);
    let d1_all = aggregate_candles(&m15, D1_MS, Timeframe::H4);

    let base_config = BotConfig {
        min_risk_reward: 2.0,
        anti_chase_filter_enabled: true,
        early_entry_enabled: true,
        early_entry_min_confidence: 90,
        lookback_period: 30,
        ..BotConfig::default()
    };

    let variant_specs = [
        (
            "legacy",
            "M15-only levels; no SL cap; bearish_rejection ON; no BUY RSI H1 guard; all valid PRIMARY.",
            true,
            false,
            false,
            true,
            false,
            0.0,
        ),
        (
            "htf_only",
            "HTF-merged levels; no SL cap; bearish_rejection OFF; BUY RSI H1 guard ON.",
            true,
            false,
            true,
            false,
            true,
            0.0,
        ),
        (
            "sl_cap_only",
            "M15-only levels; SL cap 1×ATR; bearish_rejection OFF; BUY RSI H1 guard ON.",
            true,
            false,
            true,
            false,
            false,
            1.0,
        ),
        (
            "current",
            "HTF-merged levels; SL cap 1×ATR; bearish_rejection OFF; BUY RSI H1 guard ON.",
            true,
            false,
            true,
            false,
            true,
            1.0,
        ),
        (
            "anti_chase_off",
            "HTF-merged levels; SL cap 1×ATR; anti-chase OFF; BUY RSI H1 guard ON.",
            false,
            false,
            true,
            false,
            true,
            1.0,
        ),
        (
            "aligned_only",
            "HTF-merged levels; SL cap 1×ATR; skip COUNTER PRIMARY.",
            true,
            true,
            true,
            false,
            true,
            1.0,
        ),
        (
            "aligned_only_no_anti_chase",
            "HTF-merged levels; SL cap 1×ATR; skip COUNTER PRIMARY; anti-chase OFF.",
            false,
            true,
            true,
            false,
            true,
            1.0,
        ),
    ];

    let mut variants: BTreeMap<String, VariantReport> = BTreeMap::new();
    for (
        name,
        description,
        anti_chase,
        aligned_only,
        buy_rsi_h1_guard,
        bearish_rejection_enabled,
        htf_levels_enabled,
        structural_sl_atr_cap,
    ) in variant_specs
    {
        println!("Running variant {name}...");
        let result = run_variant(
            &m15,
            &h1_all,
            &h4_all,
            &d1_all,
            start_idx,
            end_idx,
            RunOptions {
                anti_chase_filter_enabled: anti_chase,
                aligned_only,
                htf_filters: false,
                breakdown_retest: false,
                buy_rsi_h1_guard,
                bearish_rejection_enabled,
                htf_levels_enabled,
                structural_sl_atr_cap,
                quantum_weighted_score_enabled: false,
                trigger_confirmation_enabled: false,
                quantum_volume_confirm_ratio: args.quantum_volume_ratio,
                confirmation_mode: args.confirmation_mode,
                cost_r: args.cost_r,
                dump_triggers: false,
            },
        );
        variants.insert(
            name.to_string(),
            VariantReport {
                config: VariantConfig {
                    name: name.to_string(),
                    description: description.to_string(),
                    anti_chase_filter_enabled: anti_chase,
                    aligned_only,
                    htf_filters: false,
                    breakdown_retest: false,
                    buy_rsi_h1_guard,
                    bearish_rejection_enabled,
                    htf_levels_enabled,
                    structural_sl_atr_cap,
                    quantum_weighted_score_enabled: false,
                    trigger_confirmation_enabled: false,
                    quantum_volume_confirm_ratio: args.quantum_volume_ratio,
                    confirmation_mode: args.confirmation_mode.as_str().to_string(),
                },
                funnel: result.funnel,
                overall: result.overall,
                breakdowns: result.breakdowns,
                trades: result.trades,
            },
        );
    }

    println!("Running variant quantum_v2...");
    let quantum = run_variant(
        &m15,
        &h1_all,
        &h4_all,
        &d1_all,
        start_idx,
        end_idx,
        RunOptions {
            anti_chase_filter_enabled: true,
            aligned_only: false,
            htf_filters: false,
            breakdown_retest: false,
            buy_rsi_h1_guard: true,
            bearish_rejection_enabled: false,
            htf_levels_enabled: true,
            structural_sl_atr_cap: 1.0,
            quantum_weighted_score_enabled: true,
            trigger_confirmation_enabled: false,
            quantum_volume_confirm_ratio: args.quantum_volume_ratio,
            confirmation_mode: args.confirmation_mode,
            cost_r: args.cost_r,
            dump_triggers: false,
        },
    );
    variants.insert(
        "quantum_v2".to_string(),
        VariantReport {
            config: VariantConfig {
                name: "quantum_v2".to_string(),
                description:
                    "HTF-merged levels; SL cap 1xATR; weighted confluence v2 (30/25/20/15/10)."
                        .to_string(),
                anti_chase_filter_enabled: true,
                aligned_only: false,
                htf_filters: false,
                breakdown_retest: false,
                buy_rsi_h1_guard: true,
                bearish_rejection_enabled: false,
                htf_levels_enabled: true,
                structural_sl_atr_cap: 1.0,
                quantum_weighted_score_enabled: true,
                trigger_confirmation_enabled: false,
                quantum_volume_confirm_ratio: args.quantum_volume_ratio,
                confirmation_mode: args.confirmation_mode.as_str().to_string(),
            },
            funnel: quantum.funnel,
            overall: quantum.overall,
            breakdowns: quantum.breakdowns,
            trades: quantum.trades,
        },
    );

    println!("Running variant quantum_v3...");
    let quantum_v3 = run_variant(
        &m15,
        &h1_all,
        &h4_all,
        &d1_all,
        start_idx,
        end_idx,
        RunOptions {
            anti_chase_filter_enabled: true,
            aligned_only: false,
            htf_filters: false,
            breakdown_retest: false,
            buy_rsi_h1_guard: true,
            bearish_rejection_enabled: false,
            htf_levels_enabled: true,
            structural_sl_atr_cap: 1.0,
            quantum_weighted_score_enabled: true,
            trigger_confirmation_enabled: true,
            quantum_volume_confirm_ratio: args.quantum_volume_ratio,
            confirmation_mode: args.confirmation_mode,
            cost_r: args.cost_r,
            dump_triggers: false,
        },
    );
    variants.insert(
        "quantum_v3".to_string(),
        VariantReport {
            config: VariantConfig {
                name: "quantum_v3".to_string(),
                description:
                    "HTF levels; SL cap; weighted v2 + engulfing confirmation + volume threshold from --quantum-volume-ratio."
                        .to_string(),
                anti_chase_filter_enabled: true,
                aligned_only: false,
                htf_filters: false,
                breakdown_retest: false,
                buy_rsi_h1_guard: true,
                bearish_rejection_enabled: false,
                htf_levels_enabled: true,
                structural_sl_atr_cap: 1.0,
                quantum_weighted_score_enabled: true,
                trigger_confirmation_enabled: true,
                quantum_volume_confirm_ratio: args.quantum_volume_ratio,
                confirmation_mode: args.confirmation_mode.as_str().to_string(),
            },
            funnel: quantum_v3.funnel,
            overall: quantum_v3.overall,
            breakdowns: quantum_v3.breakdowns,
            trades: quantum_v3.trades,
        },
    );

    println!("Running variant quantum_v3_htf...");
    let quantum_v3_htf = run_variant(
        &m15,
        &h1_all,
        &h4_all,
        &d1_all,
        start_idx,
        end_idx,
        RunOptions {
            anti_chase_filter_enabled: true,
            aligned_only: false,
            htf_filters: true,
            breakdown_retest: false,
            buy_rsi_h1_guard: true,
            bearish_rejection_enabled: false,
            htf_levels_enabled: true,
            structural_sl_atr_cap: 1.0,
            quantum_weighted_score_enabled: true,
            trigger_confirmation_enabled: true,
            quantum_volume_confirm_ratio: args.quantum_volume_ratio,
            confirmation_mode: args.confirmation_mode,
            cost_r: args.cost_r,
            dump_triggers: false,
        },
    );
    variants.insert(
        "quantum_v3_htf".to_string(),
        VariantReport {
            config: VariantConfig {
                name: "quantum_v3_htf".to_string(),
                description: "quantum_v3 + production HTF H4/D1 filters (no counter-HTF entries)."
                    .to_string(),
                anti_chase_filter_enabled: true,
                aligned_only: false,
                htf_filters: true,
                breakdown_retest: false,
                buy_rsi_h1_guard: true,
                bearish_rejection_enabled: false,
                htf_levels_enabled: true,
                structural_sl_atr_cap: 1.0,
                quantum_weighted_score_enabled: true,
                trigger_confirmation_enabled: true,
                quantum_volume_confirm_ratio: args.quantum_volume_ratio,
                confirmation_mode: args.confirmation_mode.as_str().to_string(),
            },
            funnel: quantum_v3_htf.funnel,
            overall: quantum_v3_htf.overall,
            breakdowns: quantum_v3_htf.breakdowns,
            trades: quantum_v3_htf.trades,
        },
    );

    println!("Running variant quantum_v3_htf_retest...");
    let quantum_v3_htf_retest = run_variant(
        &m15,
        &h1_all,
        &h4_all,
        &d1_all,
        start_idx,
        end_idx,
        RunOptions {
            anti_chase_filter_enabled: true,
            aligned_only: false,
            htf_filters: true,
            breakdown_retest: true,
            buy_rsi_h1_guard: true,
            bearish_rejection_enabled: false,
            htf_levels_enabled: true,
            structural_sl_atr_cap: 1.0,
            quantum_weighted_score_enabled: true,
            trigger_confirmation_enabled: true,
            quantum_volume_confirm_ratio: args.quantum_volume_ratio,
            confirmation_mode: args.confirmation_mode,
            cost_r: args.cost_r,
            dump_triggers: true,
        },
    );
    if !quantum_v3_htf_retest.triggers.is_empty() {
        write_trigger_dump(&args.dump_triggers, &quantum_v3_htf_retest.triggers)?;
    }
    variants.insert(
        "quantum_v3_htf_retest".to_string(),
        VariantReport {
            config: VariantConfig {
                name: "quantum_v3_htf_retest".to_string(),
                description:
                    "quantum_v3_htf + retest entry for breakout/breakdown (8-bar wait, 3 USD buffer, 2R/3R)."
                        .to_string(),
                anti_chase_filter_enabled: true,
                aligned_only: false,
                htf_filters: true,
                breakdown_retest: true,
                buy_rsi_h1_guard: true,
                bearish_rejection_enabled: false,
                htf_levels_enabled: true,
                structural_sl_atr_cap: 1.0,
                quantum_weighted_score_enabled: true,
                trigger_confirmation_enabled: true,
                quantum_volume_confirm_ratio: args.quantum_volume_ratio,
                confirmation_mode: args.confirmation_mode.as_str().to_string(),
            },
            funnel: quantum_v3_htf_retest.funnel,
            overall: quantum_v3_htf_retest.overall,
            breakdowns: quantum_v3_htf_retest.breakdowns,
            trades: quantum_v3_htf_retest.trades,
        },
    );

    println!("Running variant scalp_raw_retest...");
    let scalp_raw_retest = run_variant(
        &m15,
        &h1_all,
        &h4_all,
        &d1_all,
        start_idx,
        end_idx,
        RunOptions {
            anti_chase_filter_enabled: false,
            aligned_only: false,
            htf_filters: false,
            breakdown_retest: true,
            buy_rsi_h1_guard: false,
            bearish_rejection_enabled: false,
            htf_levels_enabled: true,
            structural_sl_atr_cap: 1.0,
            quantum_weighted_score_enabled: false,
            trigger_confirmation_enabled: false,
            quantum_volume_confirm_ratio: args.quantum_volume_ratio,
            confirmation_mode: args.confirmation_mode,
            cost_r: args.cost_r,
            dump_triggers: true,
        },
    );
    if !scalp_raw_retest.triggers.is_empty() {
        let scalp_dump = args
            .dump_triggers
            .with_file_name("exness_scalp_raw_triggers.jsonl");
        write_trigger_dump(&scalp_dump, &scalp_raw_retest.triggers)?;
    }
    variants.insert(
        "scalp_raw_retest".to_string(),
        VariantReport {
            config: VariantConfig {
                name: "scalp_raw_retest".to_string(),
                description:
                    "scalp universe: retest entry, no HTF/RSI/confirmation filters, 8-bar wait."
                        .to_string(),
                anti_chase_filter_enabled: false,
                aligned_only: false,
                htf_filters: false,
                breakdown_retest: true,
                buy_rsi_h1_guard: false,
                bearish_rejection_enabled: false,
                htf_levels_enabled: true,
                structural_sl_atr_cap: 1.0,
                quantum_weighted_score_enabled: false,
                trigger_confirmation_enabled: false,
                quantum_volume_confirm_ratio: args.quantum_volume_ratio,
                confirmation_mode: args.confirmation_mode.as_str().to_string(),
            },
            funnel: scalp_raw_retest.funnel,
            overall: scalp_raw_retest.overall,
            breakdowns: scalp_raw_retest.breakdowns,
            trades: scalp_raw_retest.trades,
        },
    );

    let comparison: Vec<ComparisonRow> = [
        "legacy",
        "htf_only",
        "sl_cap_only",
        "current",
        "quantum_v2",
        "quantum_v3",
        "quantum_v3_htf",
        "quantum_v3_htf_retest",
        "scalp_raw_retest",
        "anti_chase_off",
        "aligned_only",
        "aligned_only_no_anti_chase",
    ]
    .iter()
    .filter_map(|name| {
        variants
            .get(*name)
            .map(|variant| comparison_row(name, variant))
    })
    .collect();

    let data_source = if args.csv_path.is_some() {
        "Dukascopy XAUUSD M15 bid (tick volume)"
    } else {
        "Binance public klines (PAXGUSDT XAU/USD proxy)"
    };
    let data_cache_path = args
        .csv_path
        .as_ref()
        .unwrap_or(&args.cache_path)
        .display()
        .to_string();

    let report = Report {
        generated_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        command: std::env::args().collect::<Vec<_>>().join(" "),
        period: PeriodInfo {
            start: args.start.to_string(),
            end: args.end.to_string(),
            start_ms,
            end_ms,
            limit_days: args.limit_days,
        },
        data: DataInfo {
            symbol: args.symbol.clone(),
            interval: "15m".to_string(),
            source: data_source.to_string(),
            cache_path: data_cache_path.clone(),
            candles_loaded: m15.len(),
            candles_in_period: period_candles.len(),
            missing_m15_bars_in_period: count_missing_bars(period_candles, M15_MS),
            first_timestamp: period_candles
                .first()
                .map(|candle| format_ts(candle.timestamp)),
            last_timestamp: period_candles
                .last()
                .map(|candle| format_ts(candle.timestamp)),
            fetch_requests,
        },
        config: config_info(&base_config),
        cost_r: args.cost_r,
        fill_model: fill_model_info(),
        assumptions: assumptions(),
        comparison,
        variants,
    };

    write_reports(&args, &report)?;
    print_summary(&report);
    println!("\nWrote {}", args.out_json.display());
    println!("Wrote {}", args.out_md.display());

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests (pure only; no network, no filesystem)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn utc_ms(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
        NaiveDate::from_ymd_opt(year, month, day)
            .expect("valid date")
            .and_hms_opt(hour, minute, 0)
            .expect("valid time")
            .and_utc()
            .timestamp_millis()
    }

    fn candle(timestamp: i64, open: f64, high: f64, low: f64, close: f64) -> Candle {
        Candle {
            timestamp,
            open,
            high,
            low,
            close,
            volume: 1.0,
            taker_buy_volume: 0.5,
            timeframe: Timeframe::M15,
        }
    }

    fn buy_levels() -> ScenarioLevels {
        ScenarioLevels {
            entry: 2000.0,
            stop_loss: 1990.0,
            take_profit_1: 2020.0,
            take_profit_2: 2040.0,
            risk_reward: 2.0,
        }
    }

    fn sell_levels() -> ScenarioLevels {
        ScenarioLevels {
            entry: 2000.0,
            stop_loss: 2010.0,
            take_profit_1: 1980.0,
            take_profit_2: 1960.0,
            risk_reward: 2.0,
        }
    }

    #[test]
    fn aggregate_candles_builds_h1_ohlcv() {
        let mut candles = vec![
            candle(utc_ms(2025, 1, 1, 0, 0), 100.0, 101.0, 99.0, 100.5),
            candle(utc_ms(2025, 1, 1, 0, 15), 100.5, 102.0, 100.0, 101.0),
            candle(utc_ms(2025, 1, 1, 0, 30), 101.0, 103.0, 100.5, 102.0),
            candle(utc_ms(2025, 1, 1, 0, 45), 102.0, 102.5, 101.0, 101.5),
        ];
        for (index, value) in candles.iter_mut().enumerate() {
            value.volume = index as f64 + 1.0;
            value.taker_buy_volume = (index as f64 + 1.0) / 2.0;
        }

        let h1 = aggregate_candles(&candles, H1_MS, Timeframe::H1);
        assert_eq!(h1.len(), 1);
        let bar = &h1[0];
        assert_eq!(bar.timestamp, utc_ms(2025, 1, 1, 0, 0));
        assert_eq!(bar.timeframe, Timeframe::H1);
        assert!((bar.open - 100.0).abs() < 1e-9);
        assert!((bar.high - 103.0).abs() < 1e-9);
        assert!((bar.low - 99.0).abs() < 1e-9);
        assert!((bar.close - 101.5).abs() < 1e-9);
        assert!((bar.volume - 10.0).abs() < 1e-9);
        assert!((bar.taker_buy_volume - 5.0).abs() < 1e-9);
    }

    #[test]
    fn aggregate_candles_aligns_h4_and_d1_to_utc_boundaries() {
        let candles = vec![
            candle(utc_ms(2025, 1, 1, 0, 0), 100.0, 101.0, 99.0, 100.0),
            candle(utc_ms(2025, 1, 1, 3, 45), 100.0, 102.0, 98.0, 101.0),
            candle(utc_ms(2025, 1, 1, 4, 0), 101.0, 103.0, 100.0, 102.0),
            candle(utc_ms(2025, 1, 1, 7, 45), 102.0, 104.0, 101.0, 103.0),
            candle(utc_ms(2025, 1, 1, 12, 0), 103.0, 105.0, 102.0, 104.0),
            candle(utc_ms(2025, 1, 1, 20, 0), 104.0, 106.0, 103.0, 105.0),
        ];

        let h4 = aggregate_candles(&candles, H4_MS, Timeframe::H4);
        assert_eq!(h4.len(), 4);
        assert_eq!(h4[0].timestamp, utc_ms(2025, 1, 1, 0, 0));
        assert!((h4[0].high - 102.0).abs() < 1e-9);
        assert!((h4[0].low - 98.0).abs() < 1e-9);
        assert!((h4[0].close - 101.0).abs() < 1e-9);
        assert_eq!(h4[1].timestamp, utc_ms(2025, 1, 1, 4, 0));
        assert!((h4[1].open - 101.0).abs() < 1e-9);
        assert_eq!(h4[2].timestamp, utc_ms(2025, 1, 1, 12, 0));
        assert_eq!(h4[3].timestamp, utc_ms(2025, 1, 1, 20, 0));

        let d1 = aggregate_candles(&candles, D1_MS, Timeframe::H4);
        assert_eq!(d1.len(), 1);
        assert_eq!(d1[0].timestamp, utc_ms(2025, 1, 1, 0, 0));
        assert!((d1[0].open - 100.0).abs() < 1e-9);
        assert!((d1[0].high - 106.0).abs() < 1e-9);
        assert!((d1[0].low - 98.0).abs() < 1e-9);
        assert!((d1[0].close - 105.0).abs() < 1e-9);
    }

    #[test]
    fn closed_slice_never_uses_an_in_progress_htf_candle() {
        let h1 = vec![
            candle(utc_ms(2025, 1, 2, 9, 0), 100.0, 101.0, 99.0, 100.0),
            candle(utc_ms(2025, 1, 2, 10, 0), 100.0, 101.0, 99.0, 100.0),
            candle(utc_ms(2025, 1, 2, 11, 0), 100.0, 101.0, 99.0, 100.0),
        ];

        let before_noon = closed_slice(&h1, H1_MS, utc_ms(2025, 1, 2, 11, 30), 300);
        assert_eq!(before_noon.len(), 2);
        assert_eq!(
            before_noon.last().expect("last candle").timestamp,
            utc_ms(2025, 1, 2, 10, 0)
        );

        let at_noon = closed_slice(&h1, H1_MS, utc_ms(2025, 1, 2, 12, 0), 300);
        assert_eq!(at_noon.len(), 3);

        let capped = closed_slice(&h1, H1_MS, utc_ms(2025, 1, 2, 12, 0), 2);
        assert_eq!(capped.len(), 2);
        assert_eq!(capped[0].timestamp, utc_ms(2025, 1, 2, 10, 0));
    }

    #[test]
    fn r_multiple_is_signed_for_both_directions() {
        assert!((r_multiple(SignalType::Buy, 2000.0, 1990.0, 2020.0).unwrap() - 2.0).abs() < 1e-9);
        assert!((r_multiple(SignalType::Sell, 2000.0, 2010.0, 1980.0).unwrap() - 2.0).abs() < 1e-9);
        assert!((r_multiple(SignalType::Buy, 2000.0, 1990.0, 1995.0).unwrap() + 0.5).abs() < 1e-9);
        assert!((r_multiple(SignalType::Sell, 2000.0, 2010.0, 2005.0).unwrap() + 0.5).abs() < 1e-9);
        assert!(r_multiple(SignalType::Buy, 2000.0, 2000.0, 2010.0).is_none());
    }

    #[test]
    fn simulate_buy_hits_tp1() {
        let forward = [
            candle(utc_ms(2025, 1, 1, 0, 15), 2000.0, 2010.0, 1998.0, 2005.0),
            candle(utc_ms(2025, 1, 1, 0, 30), 2005.0, 2021.0, 2002.0, 2015.0),
        ];
        let result =
            simulate_trade(SignalType::Buy, buy_levels(), 2000.0, &forward).expect("simulated");
        assert_eq!(result.exit_reason, ExitReason::Tp1);
        assert!((result.r_multiple - 2.0).abs() < 1e-9);
        assert_eq!(result.bars_held, 2);
        assert!(!result.tp2_before_sl);
    }

    #[test]
    fn simulate_buy_hits_stop_loss() {
        let forward = [candle(
            utc_ms(2025, 1, 1, 0, 15),
            2000.0,
            2005.0,
            1989.0,
            1995.0,
        )];
        let result =
            simulate_trade(SignalType::Buy, buy_levels(), 2000.0, &forward).expect("simulated");
        assert_eq!(result.exit_reason, ExitReason::StopLoss);
        assert!((result.r_multiple + 1.0).abs() < 1e-9);
        assert_eq!(result.bars_held, 1);
        assert!(!result.tp2_before_sl);
    }

    #[test]
    fn simulate_same_candle_sl_and_tp_counts_as_sl() {
        let forward = [candle(
            utc_ms(2025, 1, 1, 0, 15),
            2000.0,
            2025.0,
            1985.0,
            2000.0,
        )];
        let result =
            simulate_trade(SignalType::Buy, buy_levels(), 2000.0, &forward).expect("simulated");
        assert_eq!(result.exit_reason, ExitReason::StopLoss);
        assert!((result.r_multiple + 1.0).abs() < 1e-9);
        assert!(!result.tp2_before_sl);
    }

    #[test]
    fn simulate_sell_hits_tp1() {
        let forward = [candle(
            utc_ms(2025, 1, 1, 0, 15),
            2000.0,
            2005.0,
            1979.0,
            1990.0,
        )];
        let result =
            simulate_trade(SignalType::Sell, sell_levels(), 2000.0, &forward).expect("simulated");
        assert_eq!(result.exit_reason, ExitReason::Tp1);
        assert!((result.r_multiple - 2.0).abs() < 1e-9);
        assert!(!result.tp2_before_sl);
    }

    #[test]
    fn simulate_timeout_uses_signed_close_r() {
        let forward = [candle(
            utc_ms(2025, 1, 1, 0, 15),
            2000.0,
            2010.0,
            1995.0,
            2005.0,
        )];
        let result =
            simulate_trade(SignalType::Buy, buy_levels(), 2000.0, &forward).expect("simulated");
        assert_eq!(result.exit_reason, ExitReason::Timeout);
        assert!((result.r_multiple - 0.5).abs() < 1e-9);
        assert_eq!(result.bars_held, 1);
        assert!(!result.tp2_before_sl);
    }

    #[test]
    fn simulate_tracks_tp2_before_sl_after_tp1() {
        let forward = [
            candle(utc_ms(2025, 1, 1, 0, 15), 2000.0, 2021.0, 2000.0, 2015.0),
            candle(utc_ms(2025, 1, 1, 0, 30), 2015.0, 2041.0, 2015.0, 2030.0),
        ];
        let result =
            simulate_trade(SignalType::Buy, buy_levels(), 2000.0, &forward).expect("simulated");
        assert_eq!(result.exit_reason, ExitReason::Tp1);
        assert_eq!(result.bars_held, 1);
        assert!(result.tp2_before_sl);
    }

    #[test]
    fn simulate_does_not_count_tp2_after_sl() {
        let forward = [
            candle(utc_ms(2025, 1, 1, 0, 15), 2000.0, 2021.0, 2000.0, 2015.0),
            candle(utc_ms(2025, 1, 1, 0, 30), 2015.0, 2016.0, 1989.0, 1995.0),
            candle(utc_ms(2025, 1, 1, 0, 45), 1995.0, 2041.0, 1995.0, 2030.0),
        ];
        let result =
            simulate_trade(SignalType::Buy, buy_levels(), 2000.0, &forward).expect("simulated");
        assert_eq!(result.exit_reason, ExitReason::Tp1);
        assert!(!result.tp2_before_sl);
    }

    #[test]
    fn simulate_returns_none_without_forward_candles() {
        assert!(simulate_trade(SignalType::Buy, buy_levels(), 2000.0, &[]).is_none());
    }

    #[test]
    fn resolve_fill_touch_waits_for_the_entry_level_to_trade() {
        let m15 = [
            candle(utc_ms(2025, 1, 1, 0, 0), 2002.0, 2006.0, 2001.0, 2004.0),
            candle(utc_ms(2025, 1, 1, 0, 15), 2004.0, 2006.0, 2001.0, 2003.0),
            candle(utc_ms(2025, 1, 1, 0, 30), 2003.0, 2004.0, 1999.0, 2001.0),
        ];
        let outcome = resolve_fill(
            SignalType::Buy,
            "TOUCH",
            buy_levels(),
            0,
            &m15,
            m15.len() - 1,
        );
        match outcome {
            FillOutcome::Filled(fill) => {
                assert_eq!(fill.fill_index, 2);
                assert!((fill.fill_price - 2000.0).abs() < 1e-9);
                assert_eq!(fill.kind, FillKind::Level);
                assert!((fill.slippage).abs() < 1e-9);
            }
            other => panic!("expected fill, got {other:?}"),
        }
    }

    #[test]
    fn resolve_fill_touch_skips_when_entry_never_trades() {
        let m15 = [
            candle(utc_ms(2025, 1, 1, 0, 0), 2002.0, 2006.0, 2001.0, 2004.0),
            candle(utc_ms(2025, 1, 1, 0, 15), 2004.0, 2007.0, 2001.0, 2005.0),
        ];
        let outcome = resolve_fill(
            SignalType::Buy,
            "TOUCH",
            buy_levels(),
            0,
            &m15,
            m15.len() - 1,
        );
        assert_eq!(outcome, FillOutcome::SkippedNoFill);
    }

    #[test]
    fn resolve_fill_breakout_fills_at_level_without_gap() {
        let m15 = [candle(
            utc_ms(2025, 1, 1, 0, 0),
            1998.0,
            2005.0,
            1997.0,
            2003.0,
        )];
        let outcome = resolve_fill(
            SignalType::Buy,
            "ABOVE",
            buy_levels(),
            0,
            &m15,
            m15.len() - 1,
        );
        match outcome {
            FillOutcome::Filled(fill) => {
                assert_eq!(fill.fill_index, 0);
                assert!((fill.fill_price - 2000.0).abs() < 1e-9);
                assert_eq!(fill.kind, FillKind::Level);
            }
            other => panic!("expected fill, got {other:?}"),
        }
    }

    #[test]
    fn resolve_fill_breakout_gap_at_open_within_half_r() {
        let m15 = [candle(
            utc_ms(2025, 1, 1, 0, 0),
            2003.0,
            2007.0,
            2002.0,
            2006.0,
        )];
        let outcome = resolve_fill(
            SignalType::Buy,
            "ABOVE",
            buy_levels(),
            0,
            &m15,
            m15.len() - 1,
        );
        match outcome {
            FillOutcome::Filled(fill) => {
                assert!((fill.fill_price - 2003.0).abs() < 1e-9);
                assert_eq!(fill.kind, FillKind::Gap);
                assert!((fill.slippage - 3.0).abs() < 1e-9);
            }
            other => panic!("expected gap fill, got {other:?}"),
        }
    }

    #[test]
    fn resolve_fill_breakout_skips_gap_above_half_r() {
        let m15 = [candle(
            utc_ms(2025, 1, 1, 0, 0),
            2006.0,
            2009.0,
            2005.0,
            2008.0,
        )];
        let outcome = resolve_fill(
            SignalType::Buy,
            "ABOVE",
            buy_levels(),
            0,
            &m15,
            m15.len() - 1,
        );
        assert_eq!(outcome, FillOutcome::SkippedGapTooLarge);
    }

    #[test]
    fn resolve_fill_sell_breakout_gap_at_open_within_half_r() {
        let m15 = [candle(
            utc_ms(2025, 1, 1, 0, 0),
            1998.0,
            1999.0,
            1994.0,
            1995.0,
        )];
        let outcome = resolve_fill(
            SignalType::Sell,
            "BELOW",
            sell_levels(),
            0,
            &m15,
            m15.len() - 1,
        );
        match outcome {
            FillOutcome::Filled(fill) => {
                assert!((fill.fill_price - 1998.0).abs() < 1e-9);
                assert_eq!(fill.kind, FillKind::Gap);
                assert!((fill.slippage - 2.0).abs() < 1e-9);
            }
            other => panic!("expected gap fill, got {other:?}"),
        }
    }

    #[test]
    fn simulate_gap_fill_recomputes_r_from_the_actual_fill() {
        let forward = [candle(
            utc_ms(2025, 1, 1, 0, 15),
            2003.0,
            2021.0,
            2004.0,
            2015.0,
        )];
        let result =
            simulate_trade(SignalType::Buy, buy_levels(), 2003.0, &forward).expect("simulated");
        assert_eq!(result.exit_reason, ExitReason::Tp1);
        let expected = (2020.0 - 2003.0) / (2003.0 - 1990.0);
        assert!((result.r_multiple - expected).abs() < 1e-9);
    }

    #[test]
    fn stats_compute_profit_factor_and_max_drawdown() {
        let mut stats = Stats::default();
        stats.record_values(ExitReason::Tp1, 2.0, true);
        stats.record_values(ExitReason::StopLoss, -1.0, false);
        stats.record_values(ExitReason::StopLoss, -1.0, false);
        stats.record_values(ExitReason::Tp1, 2.0, false);

        let summary = stats.summary();
        assert_eq!(summary.trades, 4);
        assert_eq!(summary.wins, 2);
        assert_eq!(summary.losses, 2);
        assert_eq!(summary.timeouts, 0);
        assert!((summary.win_rate - 0.5).abs() < 1e-9);
        assert!((summary.avg_r - 0.5).abs() < 1e-9);
        assert!((summary.total_r - 2.0).abs() < 1e-9);
        assert!((summary.profit_factor.expect("profit factor") - 2.0).abs() < 1e-9);
        assert!((summary.max_drawdown_r - 2.0).abs() < 1e-9);
        assert!((summary.tp2_before_sl_rate - 0.25).abs() < 1e-9);
    }

    #[test]
    fn stats_keep_timeouts_separate_from_wins() {
        let mut stats = Stats::default();
        stats.record_values(ExitReason::Timeout, 0.5, false);

        let summary = stats.summary();
        assert_eq!(summary.trades, 1);
        assert_eq!(summary.wins, 0);
        assert_eq!(summary.losses, 0);
        assert_eq!(summary.timeouts, 1);
        assert!((summary.timeout_rate - 1.0).abs() < 1e-9);
        assert!(summary.profit_factor.is_none());
    }

    #[test]
    fn session_bucket_uses_vietnam_time() {
        assert_eq!(session_bucket(utc_ms(2025, 1, 2, 6, 0)), "VN_13_23");
        assert_eq!(session_bucket(utc_ms(2025, 1, 2, 15, 59)), "VN_13_23");
        assert_eq!(session_bucket(utc_ms(2025, 1, 2, 16, 0)), "OUTSIDE");
        assert_eq!(session_bucket(utc_ms(2025, 1, 2, 5, 59)), "OUTSIDE");
    }

    #[test]
    fn rsi_bucket_uses_requested_boundaries() {
        assert_eq!(rsi_bucket(Some(29.9)).as_str(), "<30");
        assert_eq!(rsi_bucket(Some(30.0)).as_str(), "30-65");
        assert_eq!(rsi_bucket(Some(65.0)).as_str(), "30-65");
        assert_eq!(rsi_bucket(Some(65.1)).as_str(), ">65");
        assert_eq!(rsi_bucket(None).as_str(), "unknown");
    }
}
