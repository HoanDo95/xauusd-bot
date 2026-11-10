//! Reproducible historical backtest for the Gold Signal Bot.
//!
//! The binary is intentionally isolated from the live analyzer: it only calls
//! `GoldMarketAnalyzer::analyze_market` with a fixed production-like config and
//! simulates several trade-management variants on top of the emitted signals.
//!
//! Data source: Binance public klines for PAXGUSDT (or another symbol/interval).
//! Candles are cached as JSONL so repeated runs are deterministic and offline.

use chrono::{DateTime, FixedOffset, Months, NaiveDate, SecondsFormat, Timelike, Utc};
use chrono_tz::America::New_York;
use gold_signal_bot::analyzer::GoldMarketAnalyzer;
use gold_signal_bot::models::{BotConfig, Candle, SignalType, Timeframe};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::error::Error;
use std::fmt;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

const WINDOW_SIZE: usize = 300;
const FETCH_LIMIT: usize = 1000;
const FETCH_SLEEP_MS: u64 = 50;
const MAX_FETCH_RETRIES: usize = 3;
const IN_SAMPLE_FRACTION: f64 = 0.70;
const BINANCE_KLINES_URL: &str = "https://api.binance.com/api/v3/klines";

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("backtest failed: {err}");
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Variant {
    Baseline,
    TpBand,
    PartialBe,
    ConfirmSession,
}

impl Variant {
    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "baseline" => Some(Self::Baseline),
            "tp_band" => Some(Self::TpBand),
            "partial_be" => Some(Self::PartialBe),
            "confirm_session" => Some(Self::ConfirmSession),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::TpBand => "tp_band",
            Self::PartialBe => "partial_be",
            Self::ConfirmSession => "confirm_session",
        }
    }
}

#[derive(Debug, Clone)]
struct Args {
    symbol: String,
    interval: String,
    start: NaiveDate,
    end: NaiveDate,
    cache_path: PathBuf,
    out_path: PathBuf,
    variants: Vec<Variant>,
    early_be: f64,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            symbol: "PAXGUSDT".to_string(),
            interval: "15m".to_string(),
            start: NaiveDate::from_ymd_opt(2021, 11, 10).expect("valid default start date"),
            end: NaiveDate::from_ymd_opt(2026, 11, 10).expect("valid default end date"),
            cache_path: PathBuf::from("rust-backend/data/backtest/PAXGUSDT_M15.jsonl"),
            out_path: PathBuf::from("rust-backend/data/backtest/report.json"),
            variants: vec![
                Variant::Baseline,
                Variant::TpBand,
                Variant::PartialBe,
                Variant::ConfirmSession,
            ],
            early_be: 0.0,
        }
    }
}

fn print_usage() {
    println!(
        "Usage: backtest [options]\n\
         \n\
         Options:\n\
           --symbol <SYMBOL>       Binance symbol (default: PAXGUSDT)\n\
           --interval <INTERVAL>   Kline interval: 15m, 1h, 4h (default: 15m)\n\
           --start <YYYY-MM-DD>    Start date, UTC (default: 2021-11-10)\n\
           --end <YYYY-MM-DD>      End date, UTC inclusive (default: 2026-11-10)\n\
           --cache <PATH>          JSONL candle cache path\n\
           --out <PATH>            JSON report path\n\
           --variants <LIST>       Comma-separated: baseline,tp_band,partial_be,confirm_session\n\
           --early-be <POINTS>     Early break-even distance for partial_be (default: 0 = off)\n\
           --help                  Show this help"
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
            "--symbol" => {
                let value = require_value(&mut iter, &flag)?;
                let value = value.trim().to_ascii_uppercase();
                if value.is_empty() || !value.chars().all(|c| c.is_ascii_alphanumeric()) {
                    return Err(AppError::Message(format!("invalid symbol: {value}")));
                }
                args.symbol = value;
            }
            "--interval" => {
                args.interval = require_value(&mut iter, &flag)?.trim().to_string();
            }
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
            "--cache" => {
                args.cache_path = PathBuf::from(require_value(&mut iter, &flag)?);
            }
            "--out" => {
                args.out_path = PathBuf::from(require_value(&mut iter, &flag)?);
            }
            "--variants" => {
                let value = require_value(&mut iter, &flag)?;
                let mut variants = Vec::new();
                for part in value.split(',') {
                    let part = part.trim();
                    if part.is_empty() {
                        continue;
                    }
                    let variant = Variant::parse(part).ok_or_else(|| {
                        AppError::Message(format!("unknown backtest variant: {part}"))
                    })?;
                    if !variants.contains(&variant) {
                        variants.push(variant);
                    }
                }
                if variants.is_empty() {
                    return Err(AppError::Message("--variants list is empty".to_string()));
                }
                args.variants = variants;
            }
            "--early-be" => {
                let value = require_value(&mut iter, &flag)?;
                let parsed = value.trim().parse::<f64>().map_err(|_| {
                    AppError::Message(format!("invalid --early-be distance: {value}"))
                })?;
                if !parsed.is_finite() || parsed < 0.0 {
                    return Err(AppError::Message(
                        "--early-be must be a finite non-negative number".to_string(),
                    ));
                }
                args.early_be = parsed;
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

    if args.start > args.end {
        return Err(AppError::Message(
            "--start must be earlier than or equal to --end".to_string(),
        ));
    }

    Ok(args)
}

// ---------------------------------------------------------------------------
// Data download and cache
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct FetchOutcome {
    candles: Vec<Candle>,
    requests: usize,
}

fn timeframe_for_interval(interval: &str) -> Result<(Timeframe, i64)> {
    match interval {
        "15m" => Ok((Timeframe::M15, 15 * 60 * 1000)),
        "1h" => Ok((Timeframe::H1, 60 * 60 * 1000)),
        "4h" => Ok((Timeframe::H4, 4 * 60 * 60 * 1000)),
        _ => Err(AppError::Message(format!(
            "unsupported interval: {interval} (expected 15m, 1h, or 4h)"
        ))),
    }
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

fn round_to(value: f64, digits: i32) -> f64 {
    let factor = 10f64.powi(digits);
    (value * factor).round() / factor
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
                tokio::time::sleep(Duration::from_millis(wait_ms)).await;
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

        tokio::time::sleep(Duration::from_millis(FETCH_SLEEP_MS)).await;
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

fn count_gaps(candles: &[Candle], step_ms: i64) -> (usize, usize) {
    let mut gaps = 0usize;
    let mut missing = 0usize;
    for pair in candles.windows(2) {
        let delta = pair[1].timestamp.saturating_sub(pair[0].timestamp);
        if delta > step_ms {
            gaps += 1;
            missing += ((delta / step_ms).saturating_sub(1)) as usize;
        }
    }
    (gaps, missing)
}

// ---------------------------------------------------------------------------
// Signal generation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct RawSignal {
    index: usize,
    signal_type: SignalType,
    entry_price: f64,
    stop_loss: f64,
    take_profit_1: f64,
    take_profit_2: f64,
}

impl RawSignal {
    fn is_buy(self) -> bool {
        self.signal_type == SignalType::Buy
    }

    fn levels(self) -> Levels {
        Levels {
            entry: self.entry_price,
            stop_loss: self.stop_loss,
            take_profit_1: self.take_profit_1,
            take_profit_2: self.take_profit_2,
        }
    }
}

fn vietnam_offset() -> FixedOffset {
    FixedOffset::east_opt(7 * 3600).expect("valid UTC+7 offset")
}

/// Golden session 13:00-23:59 Vietnam time, matching the live
/// `is_within_golden_session` hour range (13..=23).
fn is_in_golden_session(timestamp_ms: i64) -> bool {
    let Some(datetime) = DateTime::<Utc>::from_timestamp_millis(timestamp_ms) else {
        return false;
    };
    let vietnam = datetime.with_timezone(&vietnam_offset());
    (13..=23).contains(&vietnam.hour())
}

fn vietnam_date(timestamp_ms: i64) -> Option<NaiveDate> {
    DateTime::<Utc>::from_timestamp_millis(timestamp_ms)
        .map(|datetime| datetime.with_timezone(&vietnam_offset()).date_naive())
}

fn backtest_config() -> BotConfig {
    BotConfig {
        lookback_period: 30,
        gold_sl_buffer_points: 10.0,
        min_risk_reward: 2.0,
        volume_surge_multiplier: 1.5,
        rejection_wick_ratio: 2.0,
        macro_bias: "AUTO".to_string(),
        session_filter_enabled: true,
        max_signals_per_day: 1,
        risk_percentage: 1.0,
        account_balance: 10_000.0,
        account_leverage: 100.0,
        entry_mode: "MARKET".to_string(),
        data_source: "BINANCE_PAXG".to_string(),
        ..BotConfig::default()
    }
}

fn generate_signals(candles: &[Candle], config: &BotConfig) -> Vec<RawSignal> {
    if candles.len() < WINDOW_SIZE {
        return Vec::new();
    }

    let mut signals = Vec::new();
    let mut used_days: HashSet<NaiveDate> = HashSet::new();
    let mut used_dedup_keys: HashSet<String> = HashSet::new();

    for index in (WINDOW_SIZE - 1)..candles.len() {
        if index % 20_000 == 0 {
            println!(
                "  analyzed {index}/{} candles, {} signal(s) so far",
                candles.len(),
                signals.len()
            );
        }

        let signal_candle = &candles[index];
        if !is_in_golden_session(signal_candle.timestamp) {
            continue;
        }

        let Some(day) = vietnam_date(signal_candle.timestamp) else {
            continue;
        };
        if used_days.contains(&day) {
            continue;
        }

        let window = &candles[index + 1 - WINDOW_SIZE..=index];
        let Some(signal) = GoldMarketAnalyzer::analyze_market(window, config) else {
            continue;
        };

        if !used_dedup_keys.insert(signal.dedup_key.clone()) {
            continue;
        }
        used_days.insert(day);

        signals.push(RawSignal {
            index,
            signal_type: signal.signal_type,
            entry_price: signal.entry_price,
            stop_loss: signal.stop_loss,
            take_profit_1: signal.take_profit_1,
            take_profit_2: signal.take_profit_2,
        });
    }

    signals
}

// ---------------------------------------------------------------------------
// Trade simulation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
enum Outcome {
    #[serde(rename = "TP1")]
    Tp1,
    #[serde(rename = "TP2")]
    Tp2,
    #[serde(rename = "SL")]
    Sl,
    #[serde(rename = "BE")]
    Be,
    #[serde(rename = "TIME_EXIT")]
    TimeExit,
}

impl Outcome {
    fn is_win(self) -> bool {
        matches!(self, Self::Tp1 | Self::Tp2)
    }
}

#[derive(Debug, Clone, Copy)]
struct Levels {
    entry: f64,
    stop_loss: f64,
    take_profit_1: f64,
    take_profit_2: f64,
}

impl Levels {
    fn risk(self) -> f64 {
        (self.entry - self.stop_loss).abs()
    }

    fn r_multiple(self, is_buy: bool, price: f64) -> f64 {
        let risk = self.risk();
        if risk <= 0.0 {
            return 0.0;
        }
        if is_buy {
            (price - self.entry) / risk
        } else {
            (self.entry - price) / risk
        }
    }

    fn is_ordered(self, is_buy: bool) -> bool {
        if self.risk() <= 1e-9 {
            return false;
        }
        if is_buy {
            self.stop_loss < self.entry
                && self.take_profit_1 > self.entry
                && self.take_profit_2 > self.take_profit_1
        } else {
            self.stop_loss > self.entry
                && self.take_profit_1 < self.entry
                && self.take_profit_2 < self.take_profit_1
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct TradeResult {
    outcome: Outcome,
    exit_price: f64,
    r_multiple: f64,
}

#[derive(Debug, Clone)]
struct SimulatedTrade {
    signal_timestamp: i64,
    entry_timestamp: i64,
    exit_timestamp: i64,
    signal_type: SignalType,
    entry_price: f64,
    stop_loss: f64,
    take_profit_1: f64,
    take_profit_2: f64,
    exit_price: f64,
    outcome: Outcome,
    r_multiple: f64,
    duration_candles: usize,
    risk_points: f64,
}

#[derive(Debug, Clone, Serialize)]
struct TradeRecord {
    signal_timestamp: i64,
    signal_time_utc: String,
    entry_timestamp: i64,
    entry_time_utc: String,
    exit_timestamp: i64,
    exit_time_utc: String,
    signal_type: SignalType,
    entry_price: f64,
    stop_loss: f64,
    take_profit_1: f64,
    take_profit_2: f64,
    exit_price: f64,
    outcome: Outcome,
    r_multiple: f64,
    risk_points: f64,
    duration_candles: usize,
}

impl From<&SimulatedTrade> for TradeRecord {
    fn from(trade: &SimulatedTrade) -> Self {
        Self {
            signal_timestamp: trade.signal_timestamp,
            signal_time_utc: format_ts(trade.signal_timestamp),
            entry_timestamp: trade.entry_timestamp,
            entry_time_utc: format_ts(trade.entry_timestamp),
            exit_timestamp: trade.exit_timestamp,
            exit_time_utc: format_ts(trade.exit_timestamp),
            signal_type: trade.signal_type,
            entry_price: trade.entry_price,
            stop_loss: trade.stop_loss,
            take_profit_1: trade.take_profit_1,
            take_profit_2: trade.take_profit_2,
            exit_price: trade.exit_price,
            outcome: trade.outcome,
            r_multiple: round_to(trade.r_multiple, 4),
            risk_points: round_to(trade.risk_points, 4),
            duration_candles: trade.duration_candles,
        }
    }
}

fn hit_stop(candle: &Candle, is_buy: bool, stop: f64) -> bool {
    if is_buy {
        candle.low <= stop
    } else {
        candle.high >= stop
    }
}

fn hit_target(candle: &Candle, is_buy: bool, target: f64) -> bool {
    if is_buy {
        candle.high >= target
    } else {
        candle.low <= target
    }
}

fn is_at_or_after_us_close(timestamp_ms: i64, step_ms: i64) -> bool {
    let Some(datetime) = DateTime::<Utc>::from_timestamp_millis(timestamp_ms + step_ms) else {
        return false;
    };
    let new_york = datetime.with_timezone(&New_York);
    let minutes = new_york.hour() * 60 + new_york.minute();
    minutes >= 16 * 60 + 45
}

/// No new entries in the 30 minutes before the 08:00 New York data open.
/// In UTC this is 12:30-13:00 during EST and 11:30-12:00 during EDT.
fn is_pre_us_open_blackout(entry_timestamp_ms: i64) -> bool {
    let Some(datetime) = DateTime::<Utc>::from_timestamp_millis(entry_timestamp_ms) else {
        return false;
    };
    let new_york = datetime.with_timezone(&New_York);
    let minutes = new_york.hour() * 60 + new_york.minute();
    (7 * 60 + 30..8 * 60).contains(&minutes)
}

fn build_trade(
    candles: &[Candle],
    raw: &RawSignal,
    entry_index: usize,
    exit_index: usize,
    levels: Levels,
    result: TradeResult,
) -> SimulatedTrade {
    let entry_timestamp = candles[entry_index].timestamp;
    let exit_timestamp = candles[exit_index].timestamp;

    SimulatedTrade {
        signal_timestamp: candles[raw.index].timestamp,
        entry_timestamp,
        exit_timestamp,
        signal_type: raw.signal_type,
        entry_price: levels.entry,
        stop_loss: levels.stop_loss,
        take_profit_1: levels.take_profit_1,
        take_profit_2: levels.take_profit_2,
        exit_price: result.exit_price,
        outcome: result.outcome,
        r_multiple: result.r_multiple,
        duration_candles: exit_index.saturating_sub(entry_index),
        risk_points: levels.risk(),
    }
}

fn simulate_standard(
    candles: &[Candle],
    entry_index: usize,
    raw: &RawSignal,
    levels: Levels,
    time_exit_at_us_close: bool,
    step_ms: i64,
) -> Option<SimulatedTrade> {
    if !levels.is_ordered(raw.is_buy()) || entry_index + 1 >= candles.len() {
        return None;
    }

    let is_buy = raw.is_buy();
    let tp1_r = levels.r_multiple(is_buy, levels.take_profit_1);
    let mut tp1_hit = false;

    for index in (entry_index + 1)..candles.len() {
        let candle = &candles[index];

        // Conservative intra-candle priority: stop loss before targets.
        if hit_stop(candle, is_buy, levels.stop_loss) {
            if tp1_hit {
                return Some(build_trade(
                    candles,
                    raw,
                    entry_index,
                    index,
                    levels,
                    TradeResult {
                        outcome: Outcome::Tp1,
                        exit_price: levels.take_profit_1,
                        r_multiple: tp1_r,
                    },
                ));
            }
            return Some(build_trade(
                candles,
                raw,
                entry_index,
                index,
                levels,
                TradeResult {
                    outcome: Outcome::Sl,
                    exit_price: levels.stop_loss,
                    r_multiple: -1.0,
                },
            ));
        }

        if hit_target(candle, is_buy, levels.take_profit_2) {
            let tp2_r = levels.r_multiple(is_buy, levels.take_profit_2);
            return Some(build_trade(
                candles,
                raw,
                entry_index,
                index,
                levels,
                TradeResult {
                    outcome: Outcome::Tp2,
                    exit_price: levels.take_profit_2,
                    r_multiple: tp2_r,
                },
            ));
        }

        if hit_target(candle, is_buy, levels.take_profit_1) {
            tp1_hit = true;
        }

        if time_exit_at_us_close && is_at_or_after_us_close(candle.timestamp, step_ms) {
            if tp1_hit {
                return Some(build_trade(
                    candles,
                    raw,
                    entry_index,
                    index,
                    levels,
                    TradeResult {
                        outcome: Outcome::Tp1,
                        exit_price: levels.take_profit_1,
                        r_multiple: tp1_r,
                    },
                ));
            }
            let close_r = levels.r_multiple(is_buy, candle.close);
            return Some(build_trade(
                candles,
                raw,
                entry_index,
                index,
                levels,
                TradeResult {
                    outcome: Outcome::TimeExit,
                    exit_price: candle.close,
                    r_multiple: close_r,
                },
            ));
        }
    }

    let exit_index = candles.len() - 1;
    let last_close = candles
        .last()
        .map(|candle| candle.close)
        .unwrap_or(levels.entry);

    if tp1_hit {
        return Some(build_trade(
            candles,
            raw,
            entry_index,
            exit_index,
            levels,
            TradeResult {
                outcome: Outcome::Tp1,
                exit_price: levels.take_profit_1,
                r_multiple: tp1_r,
            },
        ));
    }

    Some(build_trade(
        candles,
        raw,
        entry_index,
        exit_index,
        levels,
        TradeResult {
            outcome: Outcome::TimeExit,
            exit_price: last_close,
            r_multiple: levels.r_multiple(is_buy, last_close),
        },
    ))
}

fn simulate_partial_be(
    candles: &[Candle],
    entry_index: usize,
    raw: &RawSignal,
    levels: Levels,
    early_be: f64,
) -> Option<SimulatedTrade> {
    if !levels.is_ordered(raw.is_buy()) || entry_index + 1 >= candles.len() {
        return None;
    }

    let is_buy = raw.is_buy();
    let tp1_r = levels.r_multiple(is_buy, levels.take_profit_1);
    let tp2_r = levels.r_multiple(is_buy, levels.take_profit_2);
    let mut tp1_hit = false;
    let mut break_even_active = false;

    for index in (entry_index + 1)..candles.len() {
        let candle = &candles[index];

        if early_be > 0.0 && !tp1_hit && !break_even_active {
            let early_target = if is_buy {
                levels.entry + early_be
            } else {
                levels.entry - early_be
            };
            if hit_target(candle, is_buy, early_target) {
                break_even_active = true;
            }
        }

        let current_stop = if break_even_active {
            levels.entry
        } else {
            levels.stop_loss
        };

        // Conservative intra-candle priority: stop before targets.
        if hit_stop(candle, is_buy, current_stop) {
            if tp1_hit {
                return Some(build_trade(
                    candles,
                    raw,
                    entry_index,
                    index,
                    levels,
                    TradeResult {
                        outcome: Outcome::Be,
                        exit_price: levels.entry,
                        r_multiple: 0.5 * tp1_r,
                    },
                ));
            }
            if break_even_active {
                return Some(build_trade(
                    candles,
                    raw,
                    entry_index,
                    index,
                    levels,
                    TradeResult {
                        outcome: Outcome::Be,
                        exit_price: levels.entry,
                        r_multiple: 0.0,
                    },
                ));
            }
            return Some(build_trade(
                candles,
                raw,
                entry_index,
                index,
                levels,
                TradeResult {
                    outcome: Outcome::Sl,
                    exit_price: levels.stop_loss,
                    r_multiple: -1.0,
                },
            ));
        }

        if !tp1_hit && hit_target(candle, is_buy, levels.take_profit_1) {
            tp1_hit = true;
            break_even_active = true;
        }

        if tp1_hit && hit_target(candle, is_buy, levels.take_profit_2) {
            return Some(build_trade(
                candles,
                raw,
                entry_index,
                index,
                levels,
                TradeResult {
                    outcome: Outcome::Tp2,
                    exit_price: levels.take_profit_2,
                    r_multiple: 0.5 * tp1_r + 0.5 * tp2_r,
                },
            ));
        }
    }

    let exit_index = candles.len() - 1;
    let last_close = candles
        .last()
        .map(|candle| candle.close)
        .unwrap_or(levels.entry);
    let close_r = levels.r_multiple(is_buy, last_close);
    let r_multiple = if tp1_hit {
        0.5 * tp1_r + 0.5 * close_r
    } else {
        close_r
    };

    Some(build_trade(
        candles,
        raw,
        entry_index,
        exit_index,
        levels,
        TradeResult {
            outcome: Outcome::TimeExit,
            exit_price: last_close,
            r_multiple,
        },
    ))
}

fn tp_band_levels(
    entry: f64,
    stop_loss: f64,
    is_buy: bool,
) -> std::result::Result<Levels, &'static str> {
    let risk = (entry - stop_loss).abs();
    if risk <= 1e-9 {
        return Err("zero_risk");
    }
    if 2.0 * risk > 40.0 {
        return Err("risk_gt_20");
    }

    let tp1_distance = (2.0 * risk).max(20.0).clamp(20.0, 30.0);
    let tp2_distance = (3.0 * risk).max(tp1_distance + 5.0).clamp(30.0, 40.0);

    if tp2_distance <= tp1_distance {
        return Err("tp2_le_tp1");
    }

    let (take_profit_1, take_profit_2) = if is_buy {
        (round2(entry + tp1_distance), round2(entry + tp2_distance))
    } else {
        (round2(entry - tp1_distance), round2(entry - tp2_distance))
    };

    let levels = Levels {
        entry,
        stop_loss,
        take_profit_1,
        take_profit_2,
    };

    if levels.is_ordered(is_buy) {
        Ok(levels)
    } else {
        Err("invalid_levels")
    }
}

fn confirm_levels(
    raw: &RawSignal,
    candles: &[Candle],
    step_ms: i64,
) -> std::result::Result<Levels, &'static str> {
    let next_index = raw.index + 1;
    let Some(next_candle) = candles.get(next_index) else {
        return Err("no_confirmation_candle");
    };
    let Some(signal_candle) = candles.get(raw.index) else {
        return Err("missing_signal_candle");
    };

    let entry_timestamp = next_candle.timestamp + step_ms;
    if is_pre_us_open_blackout(entry_timestamp) {
        return Err("pre_us_open_blackout");
    }

    let is_buy = raw.is_buy();
    let confirmed = if is_buy {
        next_candle.close > signal_candle.high
    } else {
        next_candle.close < signal_candle.low
    };
    if !confirmed {
        return Err("no_confirmation");
    }

    let levels = Levels {
        entry: round2(next_candle.close),
        stop_loss: raw.stop_loss,
        take_profit_1: raw.take_profit_1,
        take_profit_2: raw.take_profit_2,
    };

    if levels.risk() <= 1e-9 {
        return Err("zero_risk");
    }
    if is_buy {
        if levels.entry <= levels.stop_loss {
            return Err("entry_through_sl");
        }
        if levels.entry >= levels.take_profit_1 {
            return Err("entry_beyond_tp1");
        }
    } else {
        if levels.entry >= levels.stop_loss {
            return Err("entry_through_sl");
        }
        if levels.entry <= levels.take_profit_1 {
            return Err("entry_beyond_tp1");
        }
    }
    if !levels.is_ordered(is_buy) {
        return Err("invalid_levels");
    }

    Ok(levels)
}

// ---------------------------------------------------------------------------
// Metrics and reports
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
struct Metrics {
    trades: usize,
    wins: usize,
    losses: usize,
    be: usize,
    time_exits: usize,
    win_rate_pct: f64,
    avg_r: f64,
    net_r: f64,
    profit_factor: Option<f64>,
    max_drawdown_r: f64,
    expectancy_r: f64,
    avg_duration_candles: f64,
}

impl Metrics {
    fn empty() -> Self {
        Self {
            trades: 0,
            wins: 0,
            losses: 0,
            be: 0,
            time_exits: 0,
            win_rate_pct: 0.0,
            avg_r: 0.0,
            net_r: 0.0,
            profit_factor: None,
            max_drawdown_r: 0.0,
            expectancy_r: 0.0,
            avg_duration_candles: 0.0,
        }
    }
}

fn compute_metrics<'a>(trades: impl IntoIterator<Item = &'a SimulatedTrade>) -> Metrics {
    let trades: Vec<&SimulatedTrade> = trades.into_iter().collect();
    let total = trades.len();
    if total == 0 {
        return Metrics::empty();
    }

    let wins = trades.iter().filter(|trade| trade.outcome.is_win()).count();
    let losses = trades
        .iter()
        .filter(|trade| trade.outcome == Outcome::Sl)
        .count();
    let be = trades
        .iter()
        .filter(|trade| trade.outcome == Outcome::Be)
        .count();
    let time_exits = trades
        .iter()
        .filter(|trade| trade.outcome == Outcome::TimeExit)
        .count();

    let net_r: f64 = trades.iter().map(|trade| trade.r_multiple).sum();
    let gross_profit: f64 = trades
        .iter()
        .map(|trade| trade.r_multiple)
        .filter(|value| *value > 0.0)
        .sum();
    let gross_loss: f64 = trades
        .iter()
        .map(|trade| trade.r_multiple)
        .filter(|value| *value < 0.0)
        .map(f64::abs)
        .sum();

    let profit_factor = if gross_loss > 0.0 {
        Some(gross_profit / gross_loss)
    } else if gross_profit > 0.0 {
        None
    } else {
        Some(0.0)
    };

    let mut cumulative = 0.0f64;
    let mut peak = 0.0f64;
    let mut max_drawdown = 0.0f64;
    for trade in &trades {
        cumulative += trade.r_multiple;
        peak = peak.max(cumulative);
        max_drawdown = max_drawdown.max(peak - cumulative);
    }

    let duration_total: usize = trades.iter().map(|trade| trade.duration_candles).sum();
    let avg_r = net_r / total as f64;

    Metrics {
        trades: total,
        wins,
        losses,
        be,
        time_exits,
        win_rate_pct: round_to(wins as f64 / total as f64 * 100.0, 2),
        avg_r: round_to(avg_r, 4),
        net_r: round_to(net_r, 4),
        profit_factor: profit_factor.map(|value| round_to(value, 4)),
        max_drawdown_r: round_to(max_drawdown, 4),
        expectancy_r: round_to(avg_r, 4),
        avg_duration_candles: round_to(duration_total as f64 / total as f64, 2),
    }
}

#[derive(Debug, Clone, Serialize)]
struct VariantReport {
    name: String,
    signals: usize,
    trades: usize,
    rejected: usize,
    reject_reasons: BTreeMap<String, usize>,
    overall: Metrics,
    in_sample: Metrics,
    out_of_sample: Metrics,
    trade_log: Vec<TradeRecord>,
}

#[derive(Debug, Clone, Serialize)]
struct ReportConfig {
    lookback_period: usize,
    window_size: usize,
    gold_sl_buffer_points: f64,
    min_risk_reward: f64,
    volume_surge_multiplier: f64,
    rejection_wick_ratio: f64,
    macro_bias: String,
    session_filter: String,
    max_signals_per_day: usize,
    early_be: f64,
    news_filter: String,
    same_candle_policy: String,
}

#[derive(Debug, Clone, Serialize)]
struct Report {
    generated_at: String,
    symbol: String,
    interval: String,
    requested_start: String,
    requested_end: String,
    actual_start: String,
    actual_end: String,
    candles: usize,
    data_source: String,
    cache_path: String,
    fetch_requests: usize,
    gaps: usize,
    missing_candles_estimate: usize,
    split_timestamp: i64,
    split_time_utc: String,
    config: ReportConfig,
    notes: Vec<String>,
    variants: Vec<VariantReport>,
}

fn run_variant(
    variant: Variant,
    candles: &[Candle],
    signals: &[RawSignal],
    split_timestamp: i64,
    early_be: f64,
    step_ms: i64,
) -> VariantReport {
    let mut trades = Vec::new();
    let mut reject_reasons: BTreeMap<String, usize> = BTreeMap::new();

    for raw in signals {
        let is_buy = raw.is_buy();
        let levels = match variant {
            Variant::Baseline | Variant::PartialBe => {
                let levels = raw.levels();
                if !levels.is_ordered(is_buy) {
                    *reject_reasons
                        .entry("invalid_levels".to_string())
                        .or_default() += 1;
                    continue;
                }
                levels
            }
            Variant::TpBand => match tp_band_levels(raw.entry_price, raw.stop_loss, is_buy) {
                Ok(levels) => levels,
                Err(reason) => {
                    *reject_reasons.entry(reason.to_string()).or_default() += 1;
                    continue;
                }
            },
            Variant::ConfirmSession => match confirm_levels(raw, candles, step_ms) {
                Ok(levels) => levels,
                Err(reason) => {
                    *reject_reasons.entry(reason.to_string()).or_default() += 1;
                    continue;
                }
            },
        };

        let entry_index = match variant {
            Variant::ConfirmSession => raw.index + 1,
            _ => raw.index,
        };

        if entry_index + 1 >= candles.len() {
            *reject_reasons
                .entry("no_data_after_entry".to_string())
                .or_default() += 1;
            continue;
        }

        let trade = match variant {
            Variant::Baseline | Variant::TpBand => {
                simulate_standard(candles, entry_index, raw, levels, false, step_ms)
            }
            Variant::PartialBe => simulate_partial_be(candles, entry_index, raw, levels, early_be),
            Variant::ConfirmSession => {
                simulate_standard(candles, entry_index, raw, levels, true, step_ms)
            }
        };

        match trade {
            Some(trade) => trades.push(trade),
            None => {
                *reject_reasons
                    .entry("simulation_failed".to_string())
                    .or_default() += 1;
            }
        }
    }

    let overall = compute_metrics(&trades);
    let in_sample = compute_metrics(
        trades
            .iter()
            .filter(|trade| trade.entry_timestamp < split_timestamp),
    );
    let out_of_sample = compute_metrics(
        trades
            .iter()
            .filter(|trade| trade.entry_timestamp >= split_timestamp),
    );

    let rejected = reject_reasons.values().sum();

    VariantReport {
        name: variant.name().to_string(),
        signals: signals.len(),
        trades: trades.len(),
        rejected,
        reject_reasons,
        overall,
        in_sample,
        out_of_sample,
        trade_log: trades.iter().map(TradeRecord::from).collect(),
    }
}

fn format_ts(timestamp_ms: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(timestamp_ms)
        .map(|datetime| datetime.to_rfc3339_opts(SecondsFormat::Secs, true))
        .unwrap_or_else(|| timestamp_ms.to_string())
}

fn print_row(name: &str, split: &str, metrics: &Metrics) {
    let profit_factor = metrics
        .profit_factor
        .map(|value| format!("{value:.2}"))
        .unwrap_or_else(|| "inf".to_string());

    println!(
        "{name:<18} {split:<6} {:>6} {:>5} {:>5} {:>4} {:>5} {:>6.1} {:>7.3} {:>8.2} {profit_factor:>7} {:>7.2} {:>7.3} {:>7.1}",
        metrics.trades,
        metrics.wins,
        metrics.losses,
        metrics.be,
        metrics.time_exits,
        metrics.win_rate_pct,
        metrics.avg_r,
        metrics.net_r,
        metrics.max_drawdown_r,
        metrics.expectancy_r,
        metrics.avg_duration_candles,
    );
}

fn print_table(report: &Report) {
    println!();
    println!(
        "{:<18} {:<6} {:>6} {:>5} {:>5} {:>4} {:>5} {:>6} {:>7} {:>8} {:>7} {:>7} {:>7} {:>7}",
        "variant",
        "split",
        "trades",
        "wins",
        "loss",
        "BE",
        "time",
        "win%",
        "avgR",
        "netR",
        "PF",
        "maxDD",
        "expR",
        "avgDur"
    );

    for variant in &report.variants {
        print_row(&variant.name, "ALL", &variant.overall);
        print_row("", "IS", &variant.in_sample);
        print_row("", "OOS", &variant.out_of_sample);
    }
}

fn write_report(path: &Path, report: &Report) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let json = serde_json::to_string_pretty(report)?;
    fs::write(path, json)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Orchestration
// ---------------------------------------------------------------------------

async fn run() -> Result<()> {
    let args = parse_args()?;
    let (timeframe, step_ms) = timeframe_for_interval(&args.interval)?;

    let requested_start_ms = args
        .start
        .and_hms_opt(0, 0, 0)
        .expect("valid start midnight")
        .and_utc()
        .timestamp_millis();
    let requested_end_ms = args
        .end
        .and_hms_opt(23, 59, 59)
        .expect("valid end time")
        .and_utc()
        .timestamp_millis();
    let now_ms = Utc::now().timestamp_millis();

    if requested_start_ms >= requested_end_ms {
        return Err(AppError::Message(
            "requested start/end range is empty".to_string(),
        ));
    }
    if requested_start_ms >= now_ms {
        return Err(AppError::Message(
            "requested start is in the future".to_string(),
        ));
    }

    let effective_end_ms = requested_end_ms.min(now_ms);
    let config = backtest_config();

    println!(
        "Backtest {} {} from {} to {} (step {}m)",
        args.symbol,
        args.interval,
        args.start,
        args.end,
        step_ms / 60_000
    );

    let mut candles = load_cache(&args.cache_path, timeframe)?;
    let mut fetch_requests = 0usize;
    let mut data_source = "cache".to_string();
    let mut used_partial_cache = false;

    if cache_covers(&candles, requested_start_ms, effective_end_ms, step_ms) {
        println!(
            "Using cached candles: {} rows from {}",
            candles.len(),
            args.cache_path.display()
        );
    } else {
        if candles.is_empty() {
            println!("Cache missing or empty; downloading from Binance.");
        } else {
            println!(
                "Cache does not cover the requested range; downloading from Binance (existing {} rows).",
                candles.len()
            );
        }

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("gold-signal-bot-backtest/0.1")
            .build()?;

        let fallback_start_date = args
            .end
            .checked_sub_months(Months::new(36))
            .unwrap_or(args.start);
        let fallback_start_ms = fallback_start_date
            .and_hms_opt(0, 0, 0)
            .expect("valid fallback midnight")
            .and_utc()
            .timestamp_millis();

        let mut candidates = vec![requested_start_ms];
        if fallback_start_ms > requested_start_ms && fallback_start_ms < effective_end_ms {
            candidates.push(fallback_start_ms);
        }

        let mut fetched: Option<FetchOutcome> = None;
        let mut last_error: Option<AppError> = None;

        for (attempt_index, candidate_start) in candidates.iter().enumerate() {
            if attempt_index > 0 {
                eprintln!(
                    "warning: retrying with a 3-year range starting {} (requested {})",
                    format_ts(*candidate_start),
                    args.start
                );
            }

            match fetch_binance_klines(
                &client,
                &args.symbol,
                &args.interval,
                *candidate_start,
                effective_end_ms,
                timeframe,
            )
            .await
            {
                Ok(outcome) if !outcome.candles.is_empty() => {
                    fetch_requests += outcome.requests;
                    println!(
                        "Downloaded {} candles using {} request(s).",
                        outcome.candles.len(),
                        outcome.requests
                    );
                    fetched = Some(outcome);
                    break;
                }
                Ok(outcome) => {
                    fetch_requests += outcome.requests;
                    let message = format!(
                        "Binance returned no candles for {}",
                        format_ts(*candidate_start)
                    );
                    eprintln!("warning: {message}");
                    last_error = Some(AppError::Message(message));
                }
                Err(err) => {
                    eprintln!("warning: download failed: {err}");
                    last_error = Some(err);
                }
            }
        }

        match fetched {
            Some(outcome) => {
                candles = outcome.candles;
                data_source = "binance".to_string();
                write_cache(&args.cache_path, &candles)?;
                println!("Candle cache written to {}", args.cache_path.display());
            }
            None if candles.len() >= WINDOW_SIZE + 2 => {
                eprintln!(
                    "warning: download failed; continuing with the existing partial cache ({} candles).",
                    candles.len()
                );
                data_source = "cache_partial".to_string();
                used_partial_cache = true;
            }
            None => {
                return Err(last_error.unwrap_or_else(|| {
                    AppError::Message(
                        "unable to download candles and no cache available".to_string(),
                    )
                }));
            }
        }
    }

    candles.retain(|candle| {
        candle.timestamp >= requested_start_ms && candle.timestamp <= effective_end_ms
    });

    if candles.len() < WINDOW_SIZE + 2 {
        return Err(AppError::Message(format!(
            "not enough candles for a {} candle window: {}",
            WINDOW_SIZE,
            candles.len()
        )));
    }

    let actual_start_ms = candles
        .first()
        .map(|candle| candle.timestamp)
        .unwrap_or(requested_start_ms);
    let actual_end_ms = candles
        .last()
        .map(|candle| candle.timestamp)
        .unwrap_or(effective_end_ms);
    let split_timestamp =
        actual_start_ms + ((actual_end_ms - actual_start_ms) as f64 * IN_SAMPLE_FRACTION) as i64;
    let (gaps, missing_candles) = count_gaps(&candles, step_ms);

    println!(
        "Data: {} candles, {} .. {}, {} gap(s), ~{} missing candle(s), source={}",
        candles.len(),
        format_ts(actual_start_ms),
        format_ts(actual_end_ms),
        gaps,
        missing_candles,
        data_source
    );

    let signals = generate_signals(&candles, &config);
    println!(
        "Signals: {} (session + 1/day quota, before variant filters)",
        signals.len()
    );
    if signals.len() < 30 {
        println!(
            "note: fewer than 30 signals; metrics are statistically thin (no live thresholds were tuned)."
        );
    }

    let mut variants = Vec::new();
    for variant in &args.variants {
        let report = run_variant(
            *variant,
            &candles,
            &signals,
            split_timestamp,
            args.early_be,
            step_ms,
        );
        println!(
            "Variant {}: signals={}, trades={}, rejected={} {:?}",
            report.name, report.signals, report.trades, report.rejected, report.reject_reasons
        );
        variants.push(report);
    }

    let mut notes = vec![
        "News filter disabled: no historical 5-year economic-calendar data is bundled.".to_string(),
        "Session filter 13:00-23:59 Vietnam time (UTC+7), matching live hour range 13..=23.".to_string(),
        "One analyzer signal per Vietnam trading day; variant-specific rejects do not backfill the daily quota.".to_string(),
        "Baseline/TP band: TP1 is banked; a later SL settles at TP1, TP2 upgrades only if it is hit first; same-candle SL+TP is SL.".to_string(),
        "Partial BE: 50% at TP1, remaining stop at break-even; a BE outcome can carry a positive R (0.5 x TP1 R).".to_string(),
        "Confirm session: next-candle close beyond the signal candle high/low, entry at that close, original SL/TP, exit at 16:45 America/New_York.".to_string(),
        "Confirm session entry blackout: 07:30-08:00 New York (12:30-13:00 UTC in EST, 11:30-12:00 UTC in EDT).".to_string(),
        "Confirm session time exits settle at TP1 when TP1 was already hit; otherwise the outcome is TIME_EXIT at the 16:45 close.".to_string(),
        "Open trades remaining at the end of the dataset are closed at the final candle close with TIME_EXIT.".to_string(),
        "In-sample = first 70% of data by entry time; out-of-sample = final 30%.".to_string(),
        "taker_buy_volume is cached for future order-flow work but is not used by the current analyzer or variants.".to_string(),
    ];
    if args.early_be > 0.0 {
        notes.push(format!(
            "partial_be used early break-even at +{} points before TP1.",
            args.early_be
        ));
    }
    if used_partial_cache {
        notes.push("Binance download failed; metrics use a partial local cache.".to_string());
    }
    if actual_start_ms > requested_start_ms + step_ms * 2 {
        notes.push(format!(
            "Requested start {} is earlier than the actual data start {} (Binance availability or fallback range).",
            args.start,
            format_ts(actual_start_ms)
        ));
    }
    if actual_end_ms + step_ms * 2 < effective_end_ms {
        notes.push(format!(
            "Actual data ends at {} but the requested/window end is {}.",
            format_ts(actual_end_ms),
            format_ts(effective_end_ms)
        ));
    }

    let report = Report {
        generated_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        symbol: args.symbol,
        interval: args.interval,
        requested_start: format_ts(requested_start_ms),
        requested_end: format_ts(requested_end_ms),
        actual_start: format_ts(actual_start_ms),
        actual_end: format_ts(actual_end_ms),
        candles: candles.len(),
        data_source,
        cache_path: args.cache_path.display().to_string(),
        fetch_requests,
        gaps,
        missing_candles_estimate: missing_candles,
        split_timestamp,
        split_time_utc: format_ts(split_timestamp),
        config: ReportConfig {
            lookback_period: config.lookback_period,
            window_size: WINDOW_SIZE,
            gold_sl_buffer_points: config.gold_sl_buffer_points,
            min_risk_reward: config.min_risk_reward,
            volume_surge_multiplier: config.volume_surge_multiplier,
            rejection_wick_ratio: config.rejection_wick_ratio,
            macro_bias: config.macro_bias,
            session_filter: "13:00-23:59 UTC+7".to_string(),
            max_signals_per_day: config.max_signals_per_day,
            early_be: args.early_be,
            news_filter: "disabled (no historical calendar)".to_string(),
            same_candle_policy: "SL wins over TP on the same candle".to_string(),
        },
        notes,
        variants,
    };

    write_report(&args.out_path, &report)?;
    print_table(&report);
    println!();
    println!("JSON report: {}", args.out_path.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    const STEP_MS: i64 = 15 * 60 * 1000;

    fn candle(timestamp: i64, open: f64, high: f64, low: f64, close: f64) -> Candle {
        Candle {
            timestamp,
            open,
            high,
            low,
            close,
            volume: 100.0,
            taker_buy_volume: 0.0,
            timeframe: Timeframe::M15,
        }
    }

    fn raw_signal(
        index: usize,
        signal_type: SignalType,
        entry_price: f64,
        stop_loss: f64,
        take_profit_1: f64,
        take_profit_2: f64,
    ) -> RawSignal {
        RawSignal {
            index,
            signal_type,
            entry_price,
            stop_loss,
            take_profit_1,
            take_profit_2,
        }
    }

    fn utc_ms(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
        Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .expect("valid test datetime")
            .timestamp_millis()
    }

    #[test]
    fn parse_binance_kline_reads_taker_buy_volume_field_nine() {
        let row = serde_json::json!([
            1_700_000_000_000_i64,
            "2000.10",
            "2010.50",
            "1995.00",
            "2005.25",
            "123.45",
            1_700_000_899_999_i64,
            "0",
            10,
            "77.77",
            "0",
            "0"
        ]);

        let parsed = parse_binance_kline(&row, Timeframe::M15).expect("valid kline");
        assert_eq!(parsed.timestamp, 1_700_000_000_000);
        assert!((parsed.taker_buy_volume - 77.77).abs() < 1e-9);
    }

    #[test]
    fn tp_band_levels_clamp_and_reject_wide_risk() {
        let buy = tp_band_levels(100.0, 90.0, true).expect("buy levels");
        assert!((buy.take_profit_1 - 120.0).abs() < 1e-9);
        assert!((buy.take_profit_2 - 130.0).abs() < 1e-9);

        let sell = tp_band_levels(100.0, 110.0, false).expect("sell levels");
        assert!((sell.take_profit_1 - 80.0).abs() < 1e-9);
        assert!((sell.take_profit_2 - 70.0).abs() < 1e-9);

        assert!(matches!(
            tp_band_levels(100.0, 79.0, true),
            Err("risk_gt_20")
        ));

        let tight = tp_band_levels(100.0, 97.0, true).expect("tight risk levels");
        assert!((tight.take_profit_1 - 120.0).abs() < 1e-9);
        assert!((tight.take_profit_2 - 130.0).abs() < 1e-9);
    }

    #[test]
    fn baseline_same_candle_stop_and_target_is_stop_loss() {
        let candles = vec![
            candle(0, 100.0, 101.0, 99.0, 100.0),
            candle(STEP_MS, 100.0, 111.0, 94.0, 100.0),
        ];
        let raw = raw_signal(0, SignalType::Buy, 100.0, 95.0, 110.0, 115.0);

        let trade =
            simulate_standard(&candles, 0, &raw, raw.levels(), false, STEP_MS).expect("trade");
        assert_eq!(trade.outcome, Outcome::Sl);
        assert_eq!(trade.r_multiple, -1.0);
    }

    #[test]
    fn baseline_tp1_then_later_stop_settles_at_tp1() {
        let candles = vec![
            candle(0, 100.0, 101.0, 99.0, 100.0),
            candle(STEP_MS, 100.0, 111.0, 99.0, 110.0),
            candle(2 * STEP_MS, 110.0, 111.0, 94.0, 95.0),
        ];
        let raw = raw_signal(0, SignalType::Buy, 100.0, 95.0, 110.0, 115.0);

        let trade =
            simulate_standard(&candles, 0, &raw, raw.levels(), false, STEP_MS).expect("trade");
        assert_eq!(trade.outcome, Outcome::Tp1);
        assert!((trade.r_multiple - 2.0).abs() < 1e-9);
    }

    #[test]
    fn baseline_tp1_then_tp2_upgrades_to_tp2() {
        let candles = vec![
            candle(0, 100.0, 101.0, 99.0, 100.0),
            candle(STEP_MS, 100.0, 111.0, 99.0, 110.0),
            candle(2 * STEP_MS, 110.0, 116.0, 109.0, 115.0),
        ];
        let raw = raw_signal(0, SignalType::Buy, 100.0, 95.0, 110.0, 115.0);

        let trade =
            simulate_standard(&candles, 0, &raw, raw.levels(), false, STEP_MS).expect("trade");
        assert_eq!(trade.outcome, Outcome::Tp2);
        assert!((trade.r_multiple - 3.0).abs() < 1e-9);
    }

    #[test]
    fn partial_be_banks_half_at_tp1_then_exits_at_break_even() {
        let candles = vec![
            candle(0, 100.0, 101.0, 99.0, 100.0),
            candle(STEP_MS, 100.0, 111.0, 101.0, 110.0),
            candle(2 * STEP_MS, 105.0, 106.0, 100.0, 101.0),
        ];
        let raw = raw_signal(0, SignalType::Buy, 100.0, 95.0, 110.0, 115.0);

        let trade = simulate_partial_be(&candles, 0, &raw, raw.levels(), 0.0).expect("trade");
        assert_eq!(trade.outcome, Outcome::Be);
        assert!((trade.r_multiple - 1.0).abs() < 1e-9);
    }

    #[test]
    fn partial_be_runs_remaining_half_to_tp2() {
        let candles = vec![
            candle(0, 100.0, 101.0, 99.0, 100.0),
            candle(STEP_MS, 100.0, 111.0, 101.0, 110.0),
            candle(2 * STEP_MS, 110.0, 116.0, 101.0, 115.0),
        ];
        let raw = raw_signal(0, SignalType::Buy, 100.0, 95.0, 110.0, 115.0);

        let trade = simulate_partial_be(&candles, 0, &raw, raw.levels(), 0.0).expect("trade");
        assert_eq!(trade.outcome, Outcome::Tp2);
        assert!((trade.r_multiple - 2.5).abs() < 1e-9);
    }

    #[test]
    fn partial_be_early_break_even_can_exit_at_zero_before_tp1() {
        let candles = vec![
            candle(0, 100.0, 101.0, 99.0, 100.0),
            candle(STEP_MS, 100.0, 109.0, 101.0, 108.0),
            candle(2 * STEP_MS, 108.0, 108.5, 100.0, 100.5),
        ];
        let raw = raw_signal(0, SignalType::Buy, 100.0, 95.0, 110.0, 115.0);

        let trade = simulate_partial_be(&candles, 0, &raw, raw.levels(), 8.0).expect("trade");
        assert_eq!(trade.outcome, Outcome::Be);
        assert_eq!(trade.r_multiple, 0.0);
    }

    #[test]
    fn confirm_session_time_exits_at_new_york_close() {
        // 2024-01-10 is EST (UTC-5). Entry candle opens at 20:45 UTC (15:45 ET),
        // and the 21:30 UTC candle closes exactly at 16:45 New York time.
        let candles = vec![
            candle(utc_ms(2024, 1, 10, 20, 45), 100.0, 100.2, 99.8, 100.0),
            candle(utc_ms(2024, 1, 10, 21, 0), 100.0, 100.6, 99.9, 100.5),
            candle(utc_ms(2024, 1, 10, 21, 15), 100.5, 100.8, 100.1, 100.4),
            candle(utc_ms(2024, 1, 10, 21, 30), 100.4, 100.7, 100.0, 100.2),
        ];
        let raw = raw_signal(0, SignalType::Buy, 100.0, 95.0, 110.0, 115.0);

        let trade =
            simulate_standard(&candles, 0, &raw, raw.levels(), true, STEP_MS).expect("trade");
        assert_eq!(trade.outcome, Outcome::TimeExit);
        assert_eq!(trade.exit_timestamp, candles[3].timestamp);
        assert!((trade.exit_price - 100.2).abs() < 1e-9);
        assert!((trade.r_multiple - 0.04).abs() < 1e-9);
    }

    #[test]
    fn pre_us_open_blackout_matches_winter_and_summer_utc_windows() {
        assert!(is_pre_us_open_blackout(utc_ms(2024, 1, 10, 12, 30)));
        assert!(is_pre_us_open_blackout(utc_ms(2024, 1, 10, 12, 45)));
        assert!(!is_pre_us_open_blackout(utc_ms(2024, 1, 10, 13, 0)));

        assert!(is_pre_us_open_blackout(utc_ms(2024, 7, 10, 11, 30)));
        assert!(is_pre_us_open_blackout(utc_ms(2024, 7, 10, 11, 45)));
        assert!(!is_pre_us_open_blackout(utc_ms(2024, 7, 10, 12, 0)));
    }

    #[test]
    fn golden_session_matches_live_hour_range() {
        assert!(is_in_golden_session(utc_ms(2024, 1, 10, 6, 0)));
        assert!(!is_in_golden_session(utc_ms(2024, 1, 10, 5, 45)));
        assert!(is_in_golden_session(utc_ms(2024, 1, 10, 16, 45)));
        assert!(!is_in_golden_session(utc_ms(2024, 1, 10, 17, 0)));
    }

    #[test]
    fn metrics_compute_expectancy_profit_factor_and_drawdown() {
        let trades = vec![
            SimulatedTrade {
                signal_timestamp: 1,
                entry_timestamp: 1,
                exit_timestamp: 2,
                signal_type: SignalType::Buy,
                entry_price: 100.0,
                stop_loss: 95.0,
                take_profit_1: 110.0,
                take_profit_2: 115.0,
                exit_price: 110.0,
                outcome: Outcome::Tp1,
                r_multiple: 2.0,
                duration_candles: 2,
                risk_points: 5.0,
            },
            SimulatedTrade {
                signal_timestamp: 3,
                entry_timestamp: 3,
                exit_timestamp: 4,
                signal_type: SignalType::Buy,
                entry_price: 100.0,
                stop_loss: 95.0,
                take_profit_1: 110.0,
                take_profit_2: 115.0,
                exit_price: 95.0,
                outcome: Outcome::Sl,
                r_multiple: -1.0,
                duration_candles: 2,
                risk_points: 5.0,
            },
            SimulatedTrade {
                signal_timestamp: 5,
                entry_timestamp: 5,
                exit_timestamp: 6,
                signal_type: SignalType::Buy,
                entry_price: 100.0,
                stop_loss: 95.0,
                take_profit_1: 110.0,
                take_profit_2: 115.0,
                exit_price: 115.0,
                outcome: Outcome::Tp2,
                r_multiple: 3.0,
                duration_candles: 2,
                risk_points: 5.0,
            },
        ];

        let metrics = compute_metrics(&trades);
        assert_eq!(metrics.trades, 3);
        assert_eq!(metrics.wins, 2);
        assert_eq!(metrics.losses, 1);
        assert!((metrics.net_r - 4.0).abs() < 1e-9);
        assert!((metrics.expectancy_r - 4.0 / 3.0).abs() < 1e-4);
        assert!((metrics.profit_factor.expect("profit factor") - 5.0).abs() < 1e-9);
        assert!((metrics.max_drawdown_r - 1.0).abs() < 1e-9);
    }

    #[test]
    fn cache_coverage_requires_both_ends() {
        let candles = vec![
            candle(1_000, 1.0, 2.0, 0.5, 1.5),
            candle(10_000, 1.5, 2.5, 1.0, 2.0),
        ];
        assert!(cache_covers(&candles, 1_000, 10_000, 100));
        assert!(!cache_covers(&candles, 1_000, 20_000, 100));
        assert!(!cache_covers(&candles, 0, 10_000, 100));
    }
}
