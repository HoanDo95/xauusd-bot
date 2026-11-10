//! Reproducible historical backtest for the Gold Signal Bot.
//!
//! The binary is intentionally isolated from the live analyzer: it only calls
//! `GoldMarketAnalyzer::analyze_market` with a fixed production-like config and
//! simulates several trade-management variants on top of the emitted signals.
//!
//! Data source: Binance public klines for PAXGUSDT (or another symbol/interval).
//! Candles are cached as JSONL so repeated runs are deterministic and offline.

use chrono::{DateTime, Datelike, FixedOffset, Months, NaiveDate, SecondsFormat, Timelike, Utc};
use chrono_tz::America::New_York;
use gold_signal_bot::analyzer::GoldMarketAnalyzer;
use gold_signal_bot::models::{BotConfig, Candle, SignalType, Timeframe};
use gold_signal_bot::rsi::{detect_divergence, rsi_series, RsiDivergence};
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
/// Phase 1D: M15 lookback for the CVD divergence filter.
const DELTA_LOOKBACK: usize = 20;
/// Phase 1D: minimum absolute delta ratio for the alignment filter.
const DELTA_ALIGN_THRESHOLD: f64 = 0.10;
/// Phase R0: M15 RSI period and standard extreme thresholds.
const RSI_PERIOD: usize = 14;
const RSI_OVERBOUGHT: f64 = 70.0;
const RSI_OVERSOLD: f64 = 30.0;
const RSI_OVERBOUGHT_ALT: f64 = 65.0;
const RSI_OVERSOLD_ALT: f64 = 35.0;
/// Phase R0: divergence pivots are confirmed with two candles on each side
/// inside the last 60 closed M15 candles (15 hours).
const RSI_DIVERGENCE_LOOKBACK: usize = 60;
const RSI_PIVOT_K: usize = 2;
/// Phase R1: recent-window extremes, regime, and slope variants.
const RSI_RECENT_LOOKBACK_SHORT: usize = 10;
const RSI_RECENT_LOOKBACK_LONG: usize = 20;
const RSI_REGIME_LOOKBACK: usize = 3;
const RSI_REGIME_CROSS_LOOKBACK: usize = 5;
const RSI_REGIME_THRESHOLD: f64 = 50.0;
const RSI_SLOPE_LOOKBACK: usize = 3;
const RSI_SLOPE_MIN_DELTA: f64 = 2.0;
/// Phase R0 volume modes. The analyzer surge threshold is 1.5x MA20(volume).
const VOLUME_SURGE_MULTIPLIER: f64 = 1.5;
const VOLUME_EXHAUSTION_MULTIPLIER: f64 = 0.7;
const VOLUME_EXHAUSTION_RANGE_ATR: f64 = 0.8;

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
    TimeOut02Am,
    TimeOut02AmAll,
    Be15,
    Be20,
    Scale50Tp1,
    Scale50Tp1Be15,
    ConfirmationSoft,
    TpStructure,
    TpStructureSoft,
    TpStructureH4,
    HtfH4,
    HtfH1Channel,
    HtfCombined,
    HtfCombinedTpStructure,
    SwingSl,
    SwingSlHtf,
    Partial25At20,
    TrailAfter20,
    Partial25Trail,
    HtfCombinedPartial25Trail,
    Partial10At20,
    Partial15At25,
    TrailAfter30,
    HtfH4Partial10,
    HtfH4Partial15,
    HtfH4Trail30,
    HtfH4Zone,
    HtfH4TpStructureSoft,
    HtfH4D1,
    DeltaAlign,
    DeltaDivergence,
    DeltaBoth,
    HtfH4DeltaAlign,
    HtfH4D1DeltaAlign,
    HtfH4D1DeltaBoth,
    RsiExtreme,
    RsiExtreme6535,
    RsiDivergence,
    RsiBoth,
    HtfH4D1RsiExtreme,
    HtfH4D1RsiBoth,
    VolumeExhaustion,
    VolumeEither,
    RsiDivergenceVolumeExhaustion,
    HtfH4D1RsiVolumeEither,
    RsiRecentExtreme10,
    RsiRecentExtreme20,
    RsiRecentExtreme10_6535,
    RsiRecentExtreme20_6535,
    RsiRecentExtreme20OrDivergence,
    RsiRegime50,
    RsiRegimeCross50,
    RsiSlope3,
    HtfH4D1RsiRecentExtreme10,
    HtfH4D1RsiRecentExtreme20,
    HtfH4D1RsiRecentExtreme20_6535,
    HtfH4D1RsiRecentExtreme20OrDivergence,
    HtfH4D1RsiRegime50,
    HtfH4D1RsiRegimeCross50,
    HtfH4D1RsiSlope3,
}

impl Variant {
    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "baseline" => Some(Self::Baseline),
            "tp_band" => Some(Self::TpBand),
            "partial_be" => Some(Self::PartialBe),
            "confirm_session" => Some(Self::ConfirmSession),
            "time_out_02am" => Some(Self::TimeOut02Am),
            "time_out_02am_all" => Some(Self::TimeOut02AmAll),
            "be_15" => Some(Self::Be15),
            "be_20" => Some(Self::Be20),
            "scale_50_tp1" => Some(Self::Scale50Tp1),
            "scale_50_tp1_be_15" => Some(Self::Scale50Tp1Be15),
            "confirmation_soft" => Some(Self::ConfirmationSoft),
            "tp_structure" => Some(Self::TpStructure),
            "tp_structure_soft" => Some(Self::TpStructureSoft),
            "tp_structure_h4" => Some(Self::TpStructureH4),
            "htf_h4" => Some(Self::HtfH4),
            "htf_h1_channel" => Some(Self::HtfH1Channel),
            "htf_combined" => Some(Self::HtfCombined),
            "htf_combined_tp_structure" => Some(Self::HtfCombinedTpStructure),
            "swing_sl" => Some(Self::SwingSl),
            "swing_sl_htf" => Some(Self::SwingSlHtf),
            "partial_25_at_20" => Some(Self::Partial25At20),
            "trail_after_20" => Some(Self::TrailAfter20),
            "partial_25_trail" => Some(Self::Partial25Trail),
            "htf_combined_partial_25_trail" => Some(Self::HtfCombinedPartial25Trail),
            "partial_10_at_20" => Some(Self::Partial10At20),
            "partial_15_at_25" => Some(Self::Partial15At25),
            "trail_after_30" => Some(Self::TrailAfter30),
            "htf_h4_partial_10" => Some(Self::HtfH4Partial10),
            "htf_h4_partial_15" => Some(Self::HtfH4Partial15),
            "htf_h4_trail_30" => Some(Self::HtfH4Trail30),
            "htf_h4_zone" => Some(Self::HtfH4Zone),
            "htf_h4_tp_structure_soft" => Some(Self::HtfH4TpStructureSoft),
            "htf_h4_d1" => Some(Self::HtfH4D1),
            "delta_align" => Some(Self::DeltaAlign),
            "delta_divergence" => Some(Self::DeltaDivergence),
            "delta_both" => Some(Self::DeltaBoth),
            "htf_h4_delta_align" => Some(Self::HtfH4DeltaAlign),
            "htf_h4_d1_delta_align" => Some(Self::HtfH4D1DeltaAlign),
            "htf_h4_d1_delta_both" => Some(Self::HtfH4D1DeltaBoth),
            "rsi_extreme" => Some(Self::RsiExtreme),
            "rsi_extreme_65_35" => Some(Self::RsiExtreme6535),
            "rsi_divergence" => Some(Self::RsiDivergence),
            "rsi_both" => Some(Self::RsiBoth),
            "htf_h4_d1_rsi_extreme" => Some(Self::HtfH4D1RsiExtreme),
            "htf_h4_d1_rsi_both" => Some(Self::HtfH4D1RsiBoth),
            "volume_exhaustion" => Some(Self::VolumeExhaustion),
            "volume_either" => Some(Self::VolumeEither),
            "rsi_divergence_volume_exhaustion" => Some(Self::RsiDivergenceVolumeExhaustion),
            "htf_h4_d1_rsi_volume_either" => Some(Self::HtfH4D1RsiVolumeEither),
            "rsi_recent_extreme_10" => Some(Self::RsiRecentExtreme10),
            "rsi_recent_extreme_20" => Some(Self::RsiRecentExtreme20),
            "rsi_recent_extreme_10_65_35" => Some(Self::RsiRecentExtreme10_6535),
            "rsi_recent_extreme_20_65_35" => Some(Self::RsiRecentExtreme20_6535),
            "rsi_recent_extreme_20_or_divergence" => Some(Self::RsiRecentExtreme20OrDivergence),
            "rsi_regime_50" => Some(Self::RsiRegime50),
            "rsi_regime_cross_50" => Some(Self::RsiRegimeCross50),
            "rsi_slope_3" => Some(Self::RsiSlope3),
            "htf_h4_d1_rsi_recent_extreme_10" => Some(Self::HtfH4D1RsiRecentExtreme10),
            "htf_h4_d1_rsi_recent_extreme_20" => Some(Self::HtfH4D1RsiRecentExtreme20),
            "htf_h4_d1_rsi_recent_extreme_20_65_35" => Some(Self::HtfH4D1RsiRecentExtreme20_6535),
            "htf_h4_d1_rsi_recent_extreme_20_or_divergence" => {
                Some(Self::HtfH4D1RsiRecentExtreme20OrDivergence)
            }
            "htf_h4_d1_rsi_regime_50" => Some(Self::HtfH4D1RsiRegime50),
            "htf_h4_d1_rsi_regime_cross_50" => Some(Self::HtfH4D1RsiRegimeCross50),
            "htf_h4_d1_rsi_slope_3" => Some(Self::HtfH4D1RsiSlope3),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::TpBand => "tp_band",
            Self::PartialBe => "partial_be",
            Self::ConfirmSession => "confirm_session",
            Self::TimeOut02Am => "time_out_02am",
            Self::TimeOut02AmAll => "time_out_02am_all",
            Self::Be15 => "be_15",
            Self::Be20 => "be_20",
            Self::Scale50Tp1 => "scale_50_tp1",
            Self::Scale50Tp1Be15 => "scale_50_tp1_be_15",
            Self::ConfirmationSoft => "confirmation_soft",
            Self::TpStructure => "tp_structure",
            Self::TpStructureSoft => "tp_structure_soft",
            Self::TpStructureH4 => "tp_structure_h4",
            Self::HtfH4 => "htf_h4",
            Self::HtfH1Channel => "htf_h1_channel",
            Self::HtfCombined => "htf_combined",
            Self::HtfCombinedTpStructure => "htf_combined_tp_structure",
            Self::SwingSl => "swing_sl",
            Self::SwingSlHtf => "swing_sl_htf",
            Self::Partial25At20 => "partial_25_at_20",
            Self::TrailAfter20 => "trail_after_20",
            Self::Partial25Trail => "partial_25_trail",
            Self::HtfCombinedPartial25Trail => "htf_combined_partial_25_trail",
            Self::Partial10At20 => "partial_10_at_20",
            Self::Partial15At25 => "partial_15_at_25",
            Self::TrailAfter30 => "trail_after_30",
            Self::HtfH4Partial10 => "htf_h4_partial_10",
            Self::HtfH4Partial15 => "htf_h4_partial_15",
            Self::HtfH4Trail30 => "htf_h4_trail_30",
            Self::HtfH4Zone => "htf_h4_zone",
            Self::HtfH4TpStructureSoft => "htf_h4_tp_structure_soft",
            Self::HtfH4D1 => "htf_h4_d1",
            Self::DeltaAlign => "delta_align",
            Self::DeltaDivergence => "delta_divergence",
            Self::DeltaBoth => "delta_both",
            Self::HtfH4DeltaAlign => "htf_h4_delta_align",
            Self::HtfH4D1DeltaAlign => "htf_h4_d1_delta_align",
            Self::HtfH4D1DeltaBoth => "htf_h4_d1_delta_both",
            Self::RsiExtreme => "rsi_extreme",
            Self::RsiExtreme6535 => "rsi_extreme_65_35",
            Self::RsiDivergence => "rsi_divergence",
            Self::RsiBoth => "rsi_both",
            Self::HtfH4D1RsiExtreme => "htf_h4_d1_rsi_extreme",
            Self::HtfH4D1RsiBoth => "htf_h4_d1_rsi_both",
            Self::VolumeExhaustion => "volume_exhaustion",
            Self::VolumeEither => "volume_either",
            Self::RsiDivergenceVolumeExhaustion => "rsi_divergence_volume_exhaustion",
            Self::HtfH4D1RsiVolumeEither => "htf_h4_d1_rsi_volume_either",
            Self::RsiRecentExtreme10 => "rsi_recent_extreme_10",
            Self::RsiRecentExtreme20 => "rsi_recent_extreme_20",
            Self::RsiRecentExtreme10_6535 => "rsi_recent_extreme_10_65_35",
            Self::RsiRecentExtreme20_6535 => "rsi_recent_extreme_20_65_35",
            Self::RsiRecentExtreme20OrDivergence => "rsi_recent_extreme_20_or_divergence",
            Self::RsiRegime50 => "rsi_regime_50",
            Self::RsiRegimeCross50 => "rsi_regime_cross_50",
            Self::RsiSlope3 => "rsi_slope_3",
            Self::HtfH4D1RsiRecentExtreme10 => "htf_h4_d1_rsi_recent_extreme_10",
            Self::HtfH4D1RsiRecentExtreme20 => "htf_h4_d1_rsi_recent_extreme_20",
            Self::HtfH4D1RsiRecentExtreme20_6535 => "htf_h4_d1_rsi_recent_extreme_20_65_35",
            Self::HtfH4D1RsiRecentExtreme20OrDivergence => {
                "htf_h4_d1_rsi_recent_extreme_20_or_divergence"
            }
            Self::HtfH4D1RsiRegime50 => "htf_h4_d1_rsi_regime_50",
            Self::HtfH4D1RsiRegimeCross50 => "htf_h4_d1_rsi_regime_cross_50",
            Self::HtfH4D1RsiSlope3 => "htf_h4_d1_rsi_slope_3",
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
    cost_usd: f64,
    /// Walk-forward analysis is always included in the report; the flag is
    /// accepted so the requested invocation is explicit.
    walk_forward: bool,
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
            cost_usd: 0.0,
            walk_forward: false,
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
           --variants <LIST>       Comma-separated: baseline,tp_band,partial_be,confirm_session,\n\
                                   time_out_02am,time_out_02am_all,be_15,be_20,\n\
                                   scale_50_tp1,scale_50_tp1_be_15,confirmation_soft,tp_structure,\n\
                                   tp_structure_soft,tp_structure_h4,htf_h4,htf_h1_channel,\n\
                                   htf_combined,htf_combined_tp_structure,swing_sl,swing_sl_htf,\n\
                                   partial_25_at_20,trail_after_20,partial_25_trail,\n\
                                   htf_combined_partial_25_trail,partial_10_at_20,partial_15_at_25,\n\
                                   trail_after_30,htf_h4_partial_10,htf_h4_partial_15,htf_h4_trail_30,\n\
                                   htf_h4_zone,htf_h4_tp_structure_soft,htf_h4_d1,\n\
                                   delta_align,delta_divergence,delta_both,htf_h4_delta_align,\n\
                                   htf_h4_d1_delta_align,htf_h4_d1_delta_both,\n\
                                   rsi_extreme,rsi_extreme_65_35,rsi_divergence,rsi_both,\n\
                                   htf_h4_d1_rsi_extreme,htf_h4_d1_rsi_both,volume_exhaustion,\n\
                                   volume_either,rsi_divergence_volume_exhaustion,\n\
                                   htf_h4_d1_rsi_volume_either,rsi_recent_extreme_10,\n\
                                   rsi_recent_extreme_20,rsi_recent_extreme_10_65_35,\n\
                                   rsi_recent_extreme_20_65_35,rsi_recent_extreme_20_or_divergence,\n\
                                   rsi_regime_50,rsi_regime_cross_50,rsi_slope_3,\n\
                                   htf_h4_d1_rsi_recent_extreme_10,htf_h4_d1_rsi_recent_extreme_20,\n\
                                   htf_h4_d1_rsi_recent_extreme_20_65_35,\n\
                                   htf_h4_d1_rsi_recent_extreme_20_or_divergence,\n\
                                   htf_h4_d1_rsi_regime_50,htf_h4_d1_rsi_regime_cross_50,\n\
                                   htf_h4_d1_rsi_slope_3\n\
           --early-be <POINTS>     Early break-even distance for partial_be (default: 0 = off)\n\
           --cost-usd <USD>        Round-turn spread+slippage cost in USD/oz, subtracted\n\
                                   as cost_usd / risk from every trade R (default: 0)\n\
           --walk-forward          Explicitly enable walk-forward reporting (always on)\n\
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
            "--cost-usd" => {
                let value = require_value(&mut iter, &flag)?;
                let parsed = value
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| AppError::Message(format!("invalid --cost-usd value: {value}")))?;
                if !parsed.is_finite() || parsed < 0.0 {
                    return Err(AppError::Message(
                        "--cost-usd must be a finite non-negative number".to_string(),
                    ));
                }
                args.cost_usd = parsed;
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            "--walk-forward" => {
                args.walk_forward = true;
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

fn timeframe_millis(timeframe: Timeframe) -> i64 {
    match timeframe {
        Timeframe::M15 => 15 * 60 * 1000,
        Timeframe::H1 => 60 * 60 * 1000,
        Timeframe::H4 => 4 * 60 * 60 * 1000,
    }
}

/// Aggregate M15 candles into fixed UTC buckets (H1 or H4).
///
/// Timestamps are bucket starts. Open = first open, high = max, low = min,
/// close = last close, volume = sum. Input must be sorted by timestamp.
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

/// Index of the last candle whose close time is at or before `as_of_ms`.
///
/// Candle timestamps are open times, so this deliberately uses
/// `timestamp + timeframe` to prevent using an in-progress higher-timeframe
/// candle at signal time.
fn last_closed_index(candles: &[Candle], timeframe: Timeframe, as_of_ms: i64) -> Option<usize> {
    let tf_ms = timeframe_millis(timeframe);
    let count = candles.partition_point(|candle| candle.timestamp + tf_ms <= as_of_ms);
    count.checked_sub(1)
}

// ---------------------------------------------------------------------------
// Phase 1B: D1/H4/H1 context
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HtfFrame {
    D1,
    H4,
    H1,
}

impl HtfFrame {
    fn millis(self) -> i64 {
        match self {
            Self::D1 => 24 * 60 * 60 * 1000,
            Self::H4 => 4 * 60 * 60 * 1000,
            Self::H1 => 60 * 60 * 1000,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct HtfCandle {
    timestamp: i64,
    high: f64,
    low: f64,
    close: f64,
}

trait OhlcBar {
    fn high(&self) -> f64;
    fn low(&self) -> f64;
    fn close(&self) -> f64;
}

impl OhlcBar for Candle {
    fn high(&self) -> f64 {
        self.high
    }

    fn low(&self) -> f64 {
        self.low
    }

    fn close(&self) -> f64 {
        self.close
    }
}

impl OhlcBar for HtfCandle {
    fn high(&self) -> f64 {
        self.high
    }

    fn low(&self) -> f64 {
        self.low
    }

    fn close(&self) -> f64 {
        self.close
    }
}

/// Aggregate M15 candles into UTC-aligned D1/H4/H1 buckets.
///
/// D1 buckets start at 00:00 UTC, H4 at 00/04/08/12/16/20 UTC, and H1 at the
/// top of each UTC hour. Input must be sorted by timestamp.
fn aggregate_htf(candles: &[Candle], bucket_ms: i64) -> Vec<HtfCandle> {
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

        while let Some(next) = iter.peek() {
            if next.timestamp.div_euclid(bucket_ms) * bucket_ms != bucket {
                break;
            }
            let next = iter.next().expect("peeked candle");
            high = high.max(next.high);
            low = low.min(next.low);
            close = next.close;
        }

        aggregated.push(HtfCandle {
            timestamp: bucket,
            high,
            low,
            close,
        });
    }

    aggregated
}

fn ema_series(values: &[f64], period: usize) -> Vec<Option<f64>> {
    let mut series = vec![None; values.len()];
    if period == 0 || values.len() < period {
        return series;
    }

    let seed = values[..period].iter().sum::<f64>() / period as f64;
    series[period - 1] = Some(seed);
    let alpha = 2.0 / (period as f64 + 1.0);
    let mut previous = seed;

    for (index, value) in values.iter().enumerate().skip(period) {
        previous = value * alpha + previous * (1.0 - alpha);
        series[index] = Some(previous);
    }

    series
}

/// Simple ATR14: arithmetic mean of the last 14 true ranges.
fn atr14_at<T: OhlcBar>(candles: &[T], index: usize) -> Option<f64> {
    if index < 14 || index >= candles.len() {
        return None;
    }

    let start = index + 1 - 14;
    let mut true_range_sum = 0.0;
    for pair in candles[start - 1..=index].windows(2) {
        let previous_close = pair[0].close();
        let candle = &pair[1];
        let true_range = (candle.high() - candle.low())
            .max((candle.high() - previous_close).abs())
            .max((candle.low() - previous_close).abs());
        true_range_sum += true_range;
    }

    Some(true_range_sum / 14.0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bias {
    Bullish,
    Bearish,
    Neutral,
}

struct HtfSeries {
    frame: HtfFrame,
    candles: Vec<HtfCandle>,
    ema50: Vec<Option<f64>>,
    ema200: Vec<Option<f64>>,
    atr14: Vec<Option<f64>>,
}

impl HtfSeries {
    fn build(m15: &[Candle], frame: HtfFrame) -> Self {
        Self::from_aggregated(frame, aggregate_htf(m15, frame.millis()))
    }

    fn from_aggregated(frame: HtfFrame, candles: Vec<HtfCandle>) -> Self {
        let closes: Vec<f64> = candles.iter().map(|candle| candle.close).collect();
        let ema50 = ema_series(&closes, 50);
        let ema200 = ema_series(&closes, 200);
        let atr14 = (0..candles.len())
            .map(|index| atr14_at(&candles, index))
            .collect();

        Self {
            frame,
            candles,
            ema50,
            ema200,
            atr14,
        }
    }

    /// Last candle fully closed at or before `as_of_ms`.
    fn last_closed_index(&self, as_of_ms: i64) -> Option<usize> {
        let count = self
            .candles
            .partition_point(|candle| candle.timestamp + self.frame.millis() <= as_of_ms);
        count.checked_sub(1)
    }

    fn bias(&self, index: usize) -> Bias {
        let (Some(close), Some(ema50), Some(ema200)) = (
            self.candles.get(index).map(|candle| candle.close),
            self.ema50.get(index).copied().flatten(),
            self.ema200.get(index).copied().flatten(),
        ) else {
            return Bias::Neutral;
        };

        if close > ema50 && ema50 > ema200 {
            Bias::Bullish
        } else if close < ema50 && ema50 < ema200 {
            Bias::Bearish
        } else {
            Bias::Neutral
        }
    }

    fn channel_position(&self, index: usize, period: usize) -> Option<f64> {
        if period == 0 || index + 1 < period || index >= self.candles.len() {
            return None;
        }

        let start = index + 1 - period;
        let window = &self.candles[start..=index];
        let high = window
            .iter()
            .map(|candle| candle.high)
            .fold(f64::NEG_INFINITY, f64::max);
        let low = window
            .iter()
            .map(|candle| candle.low)
            .fold(f64::INFINITY, f64::min);
        let range = high - low;
        if range <= 1e-9 {
            return Some(0.5);
        }

        let close = self.candles[index].close;
        Some(((close - low) / range).clamp(0.0, 1.0))
    }

    fn atr14(&self, index: usize) -> Option<f64> {
        self.atr14.get(index).copied().flatten()
    }

    /// Pivot highs/lows (2 candles each side) in the last `lookback` candles.
    fn swing_prices(&self, index: usize, lookback: usize) -> (Vec<f64>, Vec<f64>) {
        let mut swing_highs = Vec::new();
        let mut swing_lows = Vec::new();
        if lookback < 5 || index + 1 < lookback {
            return (swing_highs, swing_lows);
        }

        let start = index + 1 - lookback;
        let window = &self.candles[start..=index];
        for pivot in 2..window.len().saturating_sub(2) {
            let current = &window[pivot];
            let is_swing_high = current.high > window[pivot - 1].high
                && current.high > window[pivot - 2].high
                && current.high >= window[pivot + 1].high
                && current.high >= window[pivot + 2].high;
            let is_swing_low = current.low < window[pivot - 1].low
                && current.low < window[pivot - 2].low
                && current.low <= window[pivot + 1].low
                && current.low <= window[pivot + 2].low;

            if is_swing_high {
                swing_highs.push(current.high);
            }
            if is_swing_low {
                swing_lows.push(current.low);
            }
        }

        (swing_highs, swing_lows)
    }
}

#[derive(Debug, Clone, Copy)]
struct HtfSnapshot {
    d1_bias: Bias,
    h4_bias: Bias,
    h4_channel: Option<f64>,
    h1_channel: Option<f64>,
    near_h4_support: bool,
    near_h4_resistance: bool,
}

struct HtfContext {
    d1: HtfSeries,
    h4: HtfSeries,
    h1: HtfSeries,
}

impl HtfContext {
    fn new(m15: &[Candle]) -> Self {
        Self {
            d1: HtfSeries::build(m15, HtfFrame::D1),
            h4: HtfSeries::build(m15, HtfFrame::H4),
            h1: HtfSeries::build(m15, HtfFrame::H1),
        }
    }

    /// Context as of the M15 signal close. `price` is the signal candle close.
    fn snapshot(&self, signal_close_ms: i64, price: f64) -> HtfSnapshot {
        let d1_bias = self
            .d1
            .last_closed_index(signal_close_ms)
            .map(|index| self.d1.bias(index))
            .unwrap_or(Bias::Neutral);

        let h4_index = self.h4.last_closed_index(signal_close_ms);
        let h4_bias = h4_index
            .map(|index| self.h4.bias(index))
            .unwrap_or(Bias::Neutral);
        let h4_channel = h4_index.and_then(|index| self.h4.channel_position(index, 20));

        let h1_channel = self
            .h1
            .last_closed_index(signal_close_ms)
            .and_then(|index| self.h1.channel_position(index, 20));

        let (near_h4_support, near_h4_resistance) = match h4_index {
            Some(index) => match self.h4.atr14(index) {
                Some(atr) if atr > 1e-9 => {
                    let tolerance = 0.5 * atr;
                    let (swing_highs, swing_lows) = self.h4.swing_prices(index, 50);
                    let near_support = swing_lows
                        .iter()
                        .any(|level| (price - level).abs() <= tolerance);
                    let near_resistance = swing_highs
                        .iter()
                        .any(|level| (price - level).abs() <= tolerance);
                    (near_support, near_resistance)
                }
                _ => (false, false),
            },
            None => (false, false),
        };

        HtfSnapshot {
            d1_bias,
            h4_bias,
            h4_channel,
            h1_channel,
            near_h4_support,
            near_h4_resistance,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HtfFilter {
    H4,
    H1Channel,
    Combined,
    H4Zone,
    H4D1,
}

fn htf_h4_bias_allows(
    raw: &RawSignal,
    snapshot: &HtfSnapshot,
) -> std::result::Result<(), &'static str> {
    if raw.is_buy() && snapshot.h4_bias == Bias::Bearish {
        Err("htf_h4_bearish")
    } else if !raw.is_buy() && snapshot.h4_bias == Bias::Bullish {
        Err("htf_h4_bullish")
    } else {
        Ok(())
    }
}

fn htf_filter_allows(
    filter: HtfFilter,
    raw: &RawSignal,
    snapshot: &HtfSnapshot,
) -> std::result::Result<(), &'static str> {
    let is_buy = raw.is_buy();

    match filter {
        HtfFilter::H4 => htf_h4_bias_allows(raw, snapshot),
        HtfFilter::H1Channel => {
            let Some(position) = snapshot.h1_channel else {
                return Err("htf_h1_channel_missing");
            };
            if is_buy && position > 0.30 {
                Err("htf_h1_channel_not_low")
            } else if !is_buy && position < 0.70 {
                Err("htf_h1_channel_not_high")
            } else {
                Ok(())
            }
        }
        HtfFilter::Combined => {
            htf_h4_bias_allows(raw, snapshot)?;

            let Some(position) = snapshot.h1_channel else {
                return Err("htf_h1_channel_missing");
            };
            if is_buy && position > 0.30 {
                return Err("htf_h1_channel_not_low");
            }
            if !is_buy && position < 0.70 {
                return Err("htf_h1_channel_not_high");
            }

            if is_buy && snapshot.d1_bias == Bias::Bearish && !snapshot.near_h4_support {
                return Err("htf_d1_countertrend_no_zone");
            }
            if !is_buy && snapshot.d1_bias == Bias::Bullish && !snapshot.near_h4_resistance {
                return Err("htf_d1_countertrend_no_zone");
            }

            Ok(())
        }
        HtfFilter::H4Zone => {
            htf_h4_bias_allows(raw, snapshot)?;

            let Some(position) = snapshot.h4_channel else {
                return Err("htf_h4_zone_missing");
            };
            if is_buy && position > 0.40 {
                Err("htf_h4_zone_not_low")
            } else if !is_buy && position < 0.60 {
                Err("htf_h4_zone_not_high")
            } else {
                Ok(())
            }
        }
        HtfFilter::H4D1 => {
            htf_h4_bias_allows(raw, snapshot)?;

            if is_buy && snapshot.d1_bias == Bias::Bearish {
                return Err("htf_d1_bearish");
            }
            if !is_buy && snapshot.d1_bias == Bias::Bullish {
                return Err("htf_d1_bullish");
            }

            Ok(())
        }
    }
}

fn is_at_or_after_vn_02(timestamp_ms: i64) -> bool {
    let Some(datetime) = DateTime::<Utc>::from_timestamp_millis(timestamp_ms) else {
        return false;
    };
    let vietnam = datetime.with_timezone(&vietnam_offset());
    vietnam.hour() >= 2
}

fn is_later_vn_day(timestamp_ms: i64, reference_ms: i64) -> bool {
    match (vietnam_date(timestamp_ms), vietnam_date(reference_ms)) {
        (Some(date), Some(reference)) => date > reference,
        _ => false,
    }
}

/// True when the last three M15 ranges are each below half of ATR14(M15),
/// or price is within +/-5 USD of entry while the last three volumes decline.
fn is_choppy(candles: &[Candle], index: usize, entry_price: f64) -> bool {
    if index < 14 || index >= candles.len() {
        return false;
    }

    let start = index + 1 - 14;
    let mut true_range_sum = 0.0;
    for i in start..=index {
        let candle = &candles[i];
        let previous_close = candles[i - 1].close;
        let true_range = (candle.high - candle.low)
            .max((candle.high - previous_close).abs())
            .max((candle.low - previous_close).abs());
        true_range_sum += true_range;
    }
    let atr14 = true_range_sum / 14.0;

    let ranges_small = (index - 2..=index).all(|i| candles[i].range() < 0.5 * atr14);
    let close_near_entry = (candles[index].close - entry_price).abs() <= 5.0;
    let volumes_declining = candles[index].volume < candles[index - 1].volume
        && candles[index - 1].volume < candles[index - 2].volume;

    ranges_small || (close_near_entry && volumes_declining)
}

fn nearest_above(levels: &[f64], price: f64) -> Option<f64> {
    levels
        .iter()
        .copied()
        .filter(|level| *level > price)
        .min_by(f64::total_cmp)
}

fn nearest_below(levels: &[f64], price: f64) -> Option<f64> {
    levels
        .iter()
        .copied()
        .filter(|level| *level < price)
        .max_by(f64::total_cmp)
}

/// Build TP1/TP2 from H1/H4 structure with Phase 1A guardrails.
///
/// TP1 is the nearest H1 level beyond entry. TP2 is the nearest H4 level
/// beyond TP1. If no H1 level exists, fall back to the 2R/3R TP band. If H1
/// exists but no H4 level exists beyond TP1, fall back to a banded 3R TP2
/// (at least TP1 + 5, capped at 40). Distances are measured from entry.
fn tp_structure_levels(
    entry: f64,
    stop_loss: f64,
    is_buy: bool,
    h1_levels: &[f64],
    h4_levels: &[f64],
) -> std::result::Result<Levels, &'static str> {
    let risk = (entry - stop_loss).abs();
    if risk <= 1e-9 {
        return Err("zero_risk");
    }
    if 2.0 * risk > 40.0 {
        return Err("risk_gt_20");
    }

    let h1_level = if is_buy {
        nearest_above(h1_levels, entry)
    } else {
        nearest_below(h1_levels, entry)
    };
    let Some(h1_level) = h1_level else {
        return tp_band_levels(entry, stop_loss, is_buy);
    };

    let mut tp1_distance = (h1_level - entry).abs();
    if tp1_distance < 20.0 {
        return Err("tp1_lt_20");
    }
    if tp1_distance > 40.0 {
        tp1_distance = 40.0;
    }
    let take_profit_1 = round2(if is_buy {
        entry + tp1_distance
    } else {
        entry - tp1_distance
    });

    let h4_level = if is_buy {
        nearest_above(h4_levels, take_profit_1)
    } else {
        nearest_below(h4_levels, take_profit_1)
    };

    let mut tp2_distance = match h4_level {
        Some(level) => (level - entry).abs(),
        None => (3.0 * risk).max(tp1_distance + 5.0).min(40.0),
    };
    if tp2_distance > 40.0 {
        tp2_distance = 40.0;
    }
    if tp2_distance <= tp1_distance {
        let fallback = (tp1_distance + 5.0).min(40.0);
        if fallback <= tp1_distance {
            return Err("tp2_le_tp1");
        }
        tp2_distance = fallback;
    }

    let take_profit_2 = round2(if is_buy {
        entry + tp2_distance
    } else {
        entry - tp2_distance
    });

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

/// Phase 1B soft TP structure: H1 TP1 with 2R fallback, H4 TP2 with 3R fallback.
///
/// Unlike `tp_structure`, a TP1 candidate closer than 20 USD falls back to 2R
/// instead of rejecting, and only the 2R <= 40 risk cap can reject a trade.
fn tp_structure_soft_levels(
    entry: f64,
    stop_loss: f64,
    is_buy: bool,
    h1_levels: &[f64],
    h4_levels: &[f64],
) -> std::result::Result<Levels, &'static str> {
    let risk = (entry - stop_loss).abs();
    if risk <= 1e-9 {
        return Err("zero_risk");
    }
    if 2.0 * risk > 40.0 {
        return Err("risk_gt_20");
    }

    let h1_level = if is_buy {
        nearest_above(h1_levels, entry)
    } else {
        nearest_below(h1_levels, entry)
    };
    let mut tp1_distance = match h1_level {
        Some(level) => {
            let distance = (level - entry).abs();
            if distance < 20.0 {
                2.0 * risk
            } else {
                distance.min(40.0)
            }
        }
        None => 2.0 * risk,
    };
    // TP2 must be at least TP1 + 5 while both distances are capped at 40.
    // A TP1 beyond 35 therefore cannot produce an ordered TP2; clamp it to
    // 35 instead of rejecting, preserving the "reject only on 2R > 40" rule.
    if tp1_distance > 35.0 {
        tp1_distance = 35.0;
    }
    tp1_distance = round2(tp1_distance);

    let take_profit_1 = round2(if is_buy {
        entry + tp1_distance
    } else {
        entry - tp1_distance
    });

    let h4_level = if is_buy {
        nearest_above(h4_levels, take_profit_1)
    } else {
        nearest_below(h4_levels, take_profit_1)
    };
    let min_tp2_distance = round2(tp1_distance + 5.0);
    let mut tp2_distance = match h4_level {
        Some(level) => round2((level - entry).abs()),
        None => round2(3.0 * risk),
    };
    tp2_distance = tp2_distance.min(40.0);
    if tp2_distance + 1e-9 < min_tp2_distance {
        tp2_distance = min_tp2_distance;
    }
    if tp2_distance > 40.0 {
        tp2_distance = 40.0;
    }

    let take_profit_2 = round2(if is_buy {
        entry + tp2_distance
    } else {
        entry - tp2_distance
    });

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

/// Phase 1B H4-only TP structure: nearest H4 level beyond entry for TP1, the
/// next H4 level for TP2, with the same soft 2R/3R fallbacks as
/// `tp_structure_soft_levels`.
fn tp_structure_h4_levels(
    entry: f64,
    stop_loss: f64,
    is_buy: bool,
    h4_levels: &[f64],
) -> std::result::Result<Levels, &'static str> {
    let risk = (entry - stop_loss).abs();
    if risk <= 1e-9 {
        return Err("zero_risk");
    }
    if 2.0 * risk > 40.0 {
        return Err("risk_gt_20");
    }

    let h4_level = if is_buy {
        nearest_above(h4_levels, entry)
    } else {
        nearest_below(h4_levels, entry)
    };
    let mut tp1_distance = match h4_level {
        Some(level) => {
            let distance = (level - entry).abs();
            if distance < 20.0 {
                2.0 * risk
            } else {
                distance.min(40.0)
            }
        }
        None => 2.0 * risk,
    };
    // See tp_structure_soft_levels: TP1 + 5 must fit under the 40 cap.
    if tp1_distance > 35.0 {
        tp1_distance = 35.0;
    }
    tp1_distance = round2(tp1_distance);

    let take_profit_1 = round2(if is_buy {
        entry + tp1_distance
    } else {
        entry - tp1_distance
    });

    let next_h4_level = if is_buy {
        nearest_above(h4_levels, take_profit_1)
    } else {
        nearest_below(h4_levels, take_profit_1)
    };
    let min_tp2_distance = round2(tp1_distance + 5.0);
    let mut tp2_distance = match next_h4_level {
        Some(level) => round2((level - entry).abs()),
        None => round2(3.0 * risk),
    };
    tp2_distance = tp2_distance.min(40.0);
    if tp2_distance + 1e-9 < min_tp2_distance {
        tp2_distance = min_tp2_distance;
    }
    if tp2_distance > 40.0 {
        tp2_distance = 40.0;
    }

    let take_profit_2 = round2(if is_buy {
        entry + tp2_distance
    } else {
        entry - tp2_distance
    });

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

/// H1/H4 zones on the signal side, using only candles fully closed at the
/// signal candle close.
fn structure_side_levels(
    raw: &RawSignal,
    candles: &[Candle],
    h1: &[Candle],
    h4: &[Candle],
    step_ms: i64,
) -> std::result::Result<(Vec<f64>, Vec<f64>), &'static str> {
    let signal_candle = candles.get(raw.index).ok_or("missing_signal_candle")?;
    let signal_close_ms = signal_candle.timestamp + step_ms;

    let mut h1_levels = Vec::new();
    if let Some(index) = last_closed_index(h1, Timeframe::H1, signal_close_ms) {
        let start = (index + 1).saturating_sub(100);
        let (supports, resistances) =
            GoldMarketAnalyzer::detect_dynamic_sr_zones(&h1[start..=index], 50, 5.0);
        let zones = if raw.is_buy() { resistances } else { supports };
        h1_levels.extend(zones.iter().map(|zone| zone.price));
    }

    let mut h4_levels = Vec::new();
    if let Some(index) = last_closed_index(h4, Timeframe::H4, signal_close_ms) {
        let start = (index + 1).saturating_sub(100);
        let (supports, resistances) =
            GoldMarketAnalyzer::detect_dynamic_sr_zones(&h4[start..=index], 50, 10.0);
        let zones = if raw.is_buy() { resistances } else { supports };
        h4_levels.extend(zones.iter().map(|zone| zone.price));
    }

    Ok((h1_levels, h4_levels))
}

fn structure_levels_for_signal(
    raw: &RawSignal,
    candles: &[Candle],
    h1: &[Candle],
    h4: &[Candle],
    step_ms: i64,
) -> std::result::Result<Levels, &'static str> {
    let (h1_levels, h4_levels) = structure_side_levels(raw, candles, h1, h4, step_ms)?;
    tp_structure_levels(
        raw.entry_price,
        raw.stop_loss,
        raw.is_buy(),
        &h1_levels,
        &h4_levels,
    )
}

fn soft_structure_levels_for_signal(
    raw: &RawSignal,
    candles: &[Candle],
    h1: &[Candle],
    h4: &[Candle],
    step_ms: i64,
) -> std::result::Result<Levels, &'static str> {
    let (h1_levels, h4_levels) = structure_side_levels(raw, candles, h1, h4, step_ms)?;
    tp_structure_soft_levels(
        raw.entry_price,
        raw.stop_loss,
        raw.is_buy(),
        &h1_levels,
        &h4_levels,
    )
}

fn h4_structure_levels_for_signal(
    raw: &RawSignal,
    candles: &[Candle],
    h4: &[Candle],
    step_ms: i64,
) -> std::result::Result<Levels, &'static str> {
    let signal_candle = candles.get(raw.index).ok_or("missing_signal_candle")?;
    let signal_close_ms = signal_candle.timestamp + step_ms;

    let mut h4_levels = Vec::new();
    if let Some(index) = last_closed_index(h4, Timeframe::H4, signal_close_ms) {
        let start = (index + 1).saturating_sub(100);
        let (supports, resistances) =
            GoldMarketAnalyzer::detect_dynamic_sr_zones(&h4[start..=index], 50, 10.0);
        let zones = if raw.is_buy() { resistances } else { supports };
        h4_levels.extend(zones.iter().map(|zone| zone.price));
    }

    tp_structure_h4_levels(raw.entry_price, raw.stop_loss, raw.is_buy(), &h4_levels)
}

/// Adaptive swing stop for Phase 1B.
///
/// Uses the 20 closed M15 candles ending at the signal candle. The stop is the
/// swing extreme +/- 2.5 USD, widened to 0.8 x ATR14 when it is on the wrong
/// side of entry or tighter than 0.5 x ATR14. Risk above 20 USD (2R > 40) is
/// rejected; the analyzer TP1/TP2 are kept and R is recomputed from the new SL.
fn swing_sl_levels(
    candles: &[Candle],
    raw: &RawSignal,
) -> std::result::Result<Levels, &'static str> {
    if raw.index < 19 || raw.index >= candles.len() {
        return Err("swing_lookback_missing");
    }

    let window = &candles[raw.index + 1 - 20..=raw.index];
    let is_buy = raw.is_buy();
    let entry = raw.entry_price;

    let mut stop_loss = if is_buy {
        let swing_low = window
            .iter()
            .map(|candle| candle.low)
            .fold(f64::INFINITY, f64::min);
        round2(swing_low - 2.5)
    } else {
        let swing_high = window
            .iter()
            .map(|candle| candle.high)
            .fold(f64::NEG_INFINITY, f64::max);
        round2(swing_high + 2.5)
    };

    let atr14 = atr14_at(candles, raw.index).ok_or("swing_atr_missing")?;
    let mut risk = (entry - stop_loss).abs();
    let wrong_side = if is_buy {
        stop_loss >= entry
    } else {
        stop_loss <= entry
    };
    if wrong_side || risk < 0.5 * atr14 {
        stop_loss = round2(if is_buy {
            entry - 0.8 * atr14
        } else {
            entry + 0.8 * atr14
        });
        risk = (entry - stop_loss).abs();
    }

    if risk > 30.0 {
        return Err("swing_risk_gt_30");
    }
    if 2.0 * risk > 40.0 {
        return Err("risk_gt_20");
    }

    let levels = Levels {
        entry,
        stop_loss,
        take_profit_1: raw.take_profit_1,
        take_profit_2: raw.take_profit_2,
    };

    if levels.is_ordered(is_buy) {
        Ok(levels)
    } else {
        Err("invalid_levels")
    }
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
// Phase 1D: volume delta / CVD filters
// ---------------------------------------------------------------------------

/// Aggressive-buyer volume minus aggressive-seller volume for one candle.
///
/// Binance reports taker buy base volume, so taker sell volume is
/// `volume - taker_buy_volume`; the difference simplifies to
/// `2 * taker_buy_volume - volume`.
fn candle_delta(candle: &Candle) -> f64 {
    2.0 * candle.taker_buy_volume - candle.volume
}

/// Delta normalized by candle volume, in [-1, 1] for valid Binance data.
fn delta_ratio(candle: &Candle) -> f64 {
    if candle.volume > 0.0 {
        candle_delta(candle) / candle.volume
    } else {
        0.0
    }
}

/// Cumulative delta through each candle (CVD[0] = delta of the first candle).
fn cumulative_delta(candles: &[Candle]) -> Vec<f64> {
    let mut cvd = Vec::with_capacity(candles.len());
    let mut running = 0.0f64;
    for candle in candles {
        running += candle_delta(candle);
        cvd.push(running);
    }
    cvd
}

/// Phase 1D alignment gate: the signal candle must show aggressive flow on
/// the trade side (BUY delta ratio >= +0.10, SELL <= -0.10).
fn delta_alignment_allows(
    is_buy: bool,
    candles: &[Candle],
    index: usize,
) -> std::result::Result<(), &'static str> {
    let Some(candle) = candles.get(index) else {
        return Err("delta_align");
    };
    let ratio = delta_ratio(candle);
    let allowed = if is_buy {
        ratio >= DELTA_ALIGN_THRESHOLD
    } else {
        ratio <= -DELTA_ALIGN_THRESHOLD
    };

    if allowed {
        Ok(())
    } else {
        Err("delta_align")
    }
}

/// Phase 1D divergence gate over the last 20 M15 candles.
///
/// BUY: price fell while CVD rose (bullish absorption).
/// SELL: price rose while CVD fell (bearish exhaustion).
fn delta_divergence_allows(
    is_buy: bool,
    candles: &[Candle],
    cvd: &[f64],
    index: usize,
) -> std::result::Result<(), &'static str> {
    if index < DELTA_LOOKBACK || index >= candles.len() || cvd.len() != candles.len() {
        return Err("delta_divergence");
    }

    let price_change = candles[index].close - candles[index - DELTA_LOOKBACK].close;
    let cvd_change = cvd[index] - cvd[index - DELTA_LOOKBACK];
    let allowed = if is_buy {
        price_change < 0.0 && cvd_change > 0.0
    } else {
        price_change > 0.0 && cvd_change < 0.0
    };

    if allowed {
        Ok(())
    } else {
        Err("delta_divergence")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeltaFilter {
    Align,
    Divergence,
    Both,
}

/// `delta_both` is an OR gate. When neither leg passes, the rejection is
/// counted as `delta_align` so the report keeps a single reason per signal.
fn delta_filter_allows(
    filter: DeltaFilter,
    is_buy: bool,
    candles: &[Candle],
    cvd: &[f64],
    index: usize,
) -> std::result::Result<(), &'static str> {
    match filter {
        DeltaFilter::Align => delta_alignment_allows(is_buy, candles, index),
        DeltaFilter::Divergence => delta_divergence_allows(is_buy, candles, cvd, index),
        DeltaFilter::Both => {
            if delta_alignment_allows(is_buy, candles, index).is_ok()
                || delta_divergence_allows(is_buy, candles, cvd, index).is_ok()
            {
                Ok(())
            } else {
                Err("delta_align")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Phase R0: RSI and volume-mode filters
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum RsiGate {
    Extreme {
        overbought: f64,
        oversold: f64,
    },
    Divergence,
    Both {
        overbought: f64,
        oversold: f64,
    },
    RecentExtreme {
        overbought: f64,
        oversold: f64,
        lookback: usize,
    },
    RecentExtremeOrDivergence {
        overbought: f64,
        oversold: f64,
        lookback: usize,
    },
    Regime {
        lookback: usize,
        threshold: f64,
    },
    RegimeCross {
        lookback: usize,
        threshold: f64,
    },
    Slope {
        lookback: usize,
        min_delta: f64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VolumeFilter {
    Exhaustion,
    Either,
}

fn rsi_extreme_allows(
    is_buy: bool,
    rsi_value: Option<f64>,
    overbought: f64,
    oversold: f64,
) -> std::result::Result<(), &'static str> {
    let Some(value) = rsi_value else {
        return Err("rsi_extreme");
    };

    let allowed = if is_buy {
        value <= oversold
    } else {
        value >= overbought
    };

    if allowed {
        Ok(())
    } else {
        Err("rsi_extreme")
    }
}

fn rsi_recent_extreme_allows(
    is_buy: bool,
    rsi: &[Option<f64>],
    index: usize,
    lookback: usize,
    overbought: f64,
    oversold: f64,
) -> std::result::Result<(), &'static str> {
    if lookback == 0 || index >= rsi.len() || index + 1 < lookback {
        return Err("rsi_recent_extreme");
    }

    let start = index + 1 - lookback;
    let touched = rsi[start..=index].iter().flatten().copied().any(|value| {
        value.is_finite()
            && if is_buy {
                value <= oversold
            } else {
                value >= overbought
            }
    });

    if touched {
        Ok(())
    } else {
        Err("rsi_recent_extreme")
    }
}

fn finite_rsi_at(rsi: &[Option<f64>], index: usize) -> Option<f64> {
    rsi.get(index)
        .copied()
        .flatten()
        .filter(|value| value.is_finite())
}

fn rsi_regime_allows(
    is_buy: bool,
    rsi: &[Option<f64>],
    index: usize,
    lookback: usize,
    threshold: f64,
) -> std::result::Result<(), &'static str> {
    if lookback == 0 {
        return Err("rsi_regime");
    }
    let Some(current) = finite_rsi_at(rsi, index) else {
        return Err("rsi_regime");
    };
    let Some(prior) = index
        .checked_sub(lookback)
        .and_then(|prior_index| finite_rsi_at(rsi, prior_index))
    else {
        return Err("rsi_regime");
    };

    let allowed = if is_buy {
        current > threshold && current > prior
    } else {
        current < threshold && current < prior
    };

    if allowed {
        Ok(())
    } else {
        Err("rsi_regime")
    }
}

fn rsi_regime_cross_allows(
    is_buy: bool,
    rsi: &[Option<f64>],
    index: usize,
    lookback: usize,
    threshold: f64,
) -> std::result::Result<(), &'static str> {
    if lookback == 0 || index >= rsi.len() || index < lookback {
        return Err("rsi_regime_cross");
    }

    let start = index - lookback;
    let crossed = rsi[start..=index].windows(2).any(|pair| {
        let (Some(previous), Some(current)) = (pair[0], pair[1]) else {
            return false;
        };
        if !previous.is_finite() || !current.is_finite() {
            return false;
        }
        if is_buy {
            previous <= threshold && current > threshold
        } else {
            previous >= threshold && current < threshold
        }
    });

    if crossed {
        Ok(())
    } else {
        Err("rsi_regime_cross")
    }
}

fn rsi_slope_allows(
    is_buy: bool,
    rsi: &[Option<f64>],
    index: usize,
    lookback: usize,
    min_delta: f64,
) -> std::result::Result<(), &'static str> {
    if lookback == 0 {
        return Err("rsi_slope");
    }
    let Some(current) = finite_rsi_at(rsi, index) else {
        return Err("rsi_slope");
    };
    let Some(prior) = index
        .checked_sub(lookback)
        .and_then(|prior_index| finite_rsi_at(rsi, prior_index))
    else {
        return Err("rsi_slope");
    };

    let delta = if is_buy {
        current - prior
    } else {
        prior - current
    };

    if delta >= min_delta {
        Ok(())
    } else {
        Err("rsi_slope")
    }
}

fn rsi_divergence_allows(
    is_buy: bool,
    divergence: RsiDivergence,
) -> std::result::Result<(), &'static str> {
    let allowed = match divergence {
        RsiDivergence::Bullish => is_buy,
        RsiDivergence::Bearish => !is_buy,
        RsiDivergence::None => false,
    };

    if allowed {
        Ok(())
    } else {
        Err("rsi_divergence")
    }
}

/// Evaluate an RSI gate at the signal candle.
///
/// `rsi_both` is an OR gate; when neither leg passes the rejection is counted
/// once as `rsi_extreme`, mirroring `delta_both`'s single-reason reporting.
fn rsi_gate_allows(
    gate: RsiGate,
    is_buy: bool,
    candles: &[Candle],
    rsi: &[Option<f64>],
    index: usize,
) -> std::result::Result<(), &'static str> {
    let divergence = if index < candles.len() && index < rsi.len() {
        detect_divergence(
            &candles[..=index],
            &rsi[..=index],
            RSI_DIVERGENCE_LOOKBACK,
            RSI_PIVOT_K,
        )
    } else {
        RsiDivergence::None
    };

    match gate {
        RsiGate::Extreme {
            overbought,
            oversold,
        } => rsi_extreme_allows(
            is_buy,
            rsi.get(index).copied().flatten(),
            overbought,
            oversold,
        ),
        RsiGate::Divergence => rsi_divergence_allows(is_buy, divergence),
        RsiGate::Both {
            overbought,
            oversold,
        } => {
            let extreme_passes = rsi_extreme_allows(
                is_buy,
                rsi.get(index).copied().flatten(),
                overbought,
                oversold,
            )
            .is_ok();
            if extreme_passes || rsi_divergence_allows(is_buy, divergence).is_ok() {
                Ok(())
            } else {
                Err("rsi_extreme")
            }
        }
        RsiGate::RecentExtreme {
            overbought,
            oversold,
            lookback,
        } => rsi_recent_extreme_allows(is_buy, rsi, index, lookback, overbought, oversold),
        RsiGate::RecentExtremeOrDivergence {
            overbought,
            oversold,
            lookback,
        } => {
            let recent_passes =
                rsi_recent_extreme_allows(is_buy, rsi, index, lookback, overbought, oversold)
                    .is_ok();
            if recent_passes || rsi_divergence_allows(is_buy, divergence).is_ok() {
                Ok(())
            } else {
                Err("rsi_recent_extreme")
            }
        }
        RsiGate::Regime {
            lookback,
            threshold,
        } => rsi_regime_allows(is_buy, rsi, index, lookback, threshold),
        RsiGate::RegimeCross {
            lookback,
            threshold,
        } => rsi_regime_cross_allows(is_buy, rsi, index, lookback, threshold),
        RsiGate::Slope {
            lookback,
            min_delta,
        } => rsi_slope_allows(is_buy, rsi, index, lookback, min_delta),
    }
}

/// Volume MA over the `period` closed candles before `index`, matching the
/// analyzer's use of `previous_candles` for its surge ratio.
fn volume_ma_before(candles: &[Candle], index: usize, period: usize) -> Option<f64> {
    if period == 0 || index < period || index > candles.len() {
        return None;
    }

    let start = index - period;
    let sum: f64 = candles[start..index]
        .iter()
        .map(|candle| candle.volume)
        .sum();
    Some(sum / period as f64)
}

fn volume_surge_at(candles: &[Candle], index: usize) -> bool {
    let Some(ma20) = volume_ma_before(candles, index, 20) else {
        return false;
    };
    if ma20 <= 0.0 {
        return false;
    }

    candles
        .get(index)
        .map(|candle| candle.volume >= VOLUME_SURGE_MULTIPLIER * ma20)
        .unwrap_or(false)
}

/// Low-volume reversal candle: volume <= 0.7 x MA20 and range <= 0.8 x ATR14.
fn volume_exhaustion_at(candles: &[Candle], index: usize) -> bool {
    let Some(ma20) = volume_ma_before(candles, index, 20) else {
        return false;
    };
    if ma20 <= 0.0 {
        return false;
    }
    let Some(candle) = candles.get(index) else {
        return false;
    };
    let Some(atr14) = atr14_at(candles, index) else {
        return false;
    };

    candle.volume <= VOLUME_EXHAUSTION_MULTIPLIER * ma20
        && candle.range() <= VOLUME_EXHAUSTION_RANGE_ATR * atr14
}

fn volume_filter_allows(
    filter: VolumeFilter,
    candles: &[Candle],
    index: usize,
) -> std::result::Result<(), &'static str> {
    match filter {
        VolumeFilter::Exhaustion => {
            if volume_exhaustion_at(candles, index) {
                Ok(())
            } else {
                Err("volume_exhaustion")
            }
        }
        VolumeFilter::Either => {
            if volume_surge_at(candles, index) || volume_exhaustion_at(candles, index) {
                Ok(())
            } else {
                Err("volume_either")
            }
        }
    }
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
    #[serde(rename = "TRAIL")]
    Trail,
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
    entry_index: usize,
    exit_index: usize,
    signal_type: SignalType,
    entry_price: f64,
    stop_loss: f64,
    take_profit_1: f64,
    take_profit_2: f64,
    exit_price: f64,
    outcome: Outcome,
    r_multiple: f64,
    cost_usd: f64,
    mfe_usd: f64,
    mae_usd: f64,
    mfe_before_tp1_usd: f64,
    mfe_after_tp1_usd: f64,
    duration_candles: usize,
    risk_points: f64,
}

impl SimulatedTrade {
    fn cost_r(&self) -> f64 {
        if self.risk_points <= 1e-9 {
            0.0
        } else {
            self.cost_usd / self.risk_points
        }
    }

    fn net_r(&self) -> f64 {
        self.r_multiple - self.cost_r()
    }

    fn mfe_r(&self) -> f64 {
        if self.risk_points <= 1e-9 {
            0.0
        } else {
            self.mfe_usd / self.risk_points
        }
    }

    fn mae_r(&self) -> f64 {
        if self.risk_points <= 1e-9 {
            0.0
        } else {
            self.mae_usd / self.risk_points
        }
    }
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
    cost_usd: f64,
    cost_r: f64,
    net_r_multiple: f64,
    mfe_usd: f64,
    mae_usd: f64,
    mfe_r: f64,
    mae_r: f64,
    mfe_before_tp1_usd: f64,
    mfe_after_tp1_usd: f64,
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
            cost_usd: round_to(trade.cost_usd, 4),
            cost_r: round_to(trade.cost_r(), 4),
            net_r_multiple: round_to(trade.net_r(), 4),
            mfe_usd: round_to(trade.mfe_usd, 4),
            mae_usd: round_to(trade.mae_usd, 4),
            mfe_r: round_to(trade.mfe_r(), 4),
            mae_r: round_to(trade.mae_r(), 4),
            mfe_before_tp1_usd: round_to(trade.mfe_before_tp1_usd, 4),
            mfe_after_tp1_usd: round_to(trade.mfe_after_tp1_usd, 4),
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
        entry_index,
        exit_index,
        signal_type: raw.signal_type,
        entry_price: levels.entry,
        stop_loss: levels.stop_loss,
        take_profit_1: levels.take_profit_1,
        take_profit_2: levels.take_profit_2,
        exit_price: result.exit_price,
        outcome: result.outcome,
        r_multiple: result.r_multiple,
        cost_usd: 0.0,
        mfe_usd: 0.0,
        mae_usd: 0.0,
        mfe_before_tp1_usd: 0.0,
        mfe_after_tp1_usd: 0.0,
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

fn soft_confirm_levels(
    raw: &RawSignal,
    candles: &[Candle],
) -> std::result::Result<Levels, &'static str> {
    let next_index = raw.index + 1;
    let Some(next_candle) = candles.get(next_index) else {
        return Err("no_confirmation_candle");
    };
    let Some(signal_candle) = candles.get(raw.index) else {
        return Err("missing_signal_candle");
    };

    // Soft confirmation: the next close only has to stay inside the signal
    // candle body range, not break its high/low like confirm_session.
    if next_candle.close < signal_candle.low || next_candle.close > signal_candle.high {
        return Err("no_confirmation");
    }

    let is_buy = raw.is_buy();
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
// Phase 1A isolated risk management
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimeExitRule {
    None,
    Vn02Choppy,
    Vn02All,
}

#[derive(Debug, Clone, Copy)]
struct Management {
    /// Move SL to entry once MFE reaches this USD distance. The stop applies
    /// from the next M15 candle (no optimistic same-candle BE fill).
    be_trigger_usd: Option<f64>,
    /// Close 50% at TP1 and leave the remainder running.
    partial_at_tp1: bool,
    time_exit: TimeExitRule,
}

impl Management {
    const BASELINE: Self = Self {
        be_trigger_usd: None,
        partial_at_tp1: false,
        time_exit: TimeExitRule::None,
    };

    fn be(trigger_usd: f64) -> Self {
        Self {
            be_trigger_usd: Some(trigger_usd),
            ..Self::BASELINE
        }
    }

    fn scale(be_trigger_usd: Option<f64>) -> Self {
        Self {
            be_trigger_usd,
            partial_at_tp1: true,
            ..Self::BASELINE
        }
    }

    fn time_exit(rule: TimeExitRule) -> Self {
        Self {
            time_exit: rule,
            ..Self::BASELINE
        }
    }
}

fn time_exit_eligible(
    rule: TimeExitRule,
    candles: &[Candle],
    index: usize,
    entry_timestamp: i64,
    entry_price: f64,
) -> bool {
    let candle = &candles[index];
    match rule {
        TimeExitRule::None => false,
        TimeExitRule::Vn02All => {
            is_later_vn_day(candle.timestamp, entry_timestamp)
                && is_at_or_after_vn_02(candle.timestamp)
        }
        TimeExitRule::Vn02Choppy => {
            is_later_vn_day(candle.timestamp, entry_timestamp)
                && is_at_or_after_vn_02(candle.timestamp)
                && is_choppy(candles, index, entry_price)
        }
    }
}

fn simulate_managed(
    candles: &[Candle],
    entry_index: usize,
    raw: &RawSignal,
    levels: Levels,
    management: Management,
) -> Option<SimulatedTrade> {
    if !levels.is_ordered(raw.is_buy()) || entry_index + 1 >= candles.len() {
        return None;
    }

    let is_buy = raw.is_buy();
    let entry_timestamp = candles[entry_index].timestamp;
    let tp1_r = levels.r_multiple(is_buy, levels.take_profit_1);
    let tp2_r = levels.r_multiple(is_buy, levels.take_profit_2);
    let mut tp1_hit = false;
    let mut be_active = false;
    let mut mfe_trigger = 0.0f64;

    for index in (entry_index + 1)..candles.len() {
        let candle = &candles[index];
        let current_stop = if be_active {
            levels.entry
        } else {
            levels.stop_loss
        };

        // Conservative intra-candle priority: stop before targets.
        if hit_stop(candle, is_buy, current_stop) {
            if tp1_hit {
                if management.partial_at_tp1 {
                    let r_multiple = if be_active {
                        0.5 * tp1_r
                    } else {
                        0.5 * tp1_r - 0.5
                    };
                    return Some(build_trade(
                        candles,
                        raw,
                        entry_index,
                        index,
                        levels,
                        TradeResult {
                            outcome: Outcome::Tp1,
                            exit_price: current_stop,
                            r_multiple,
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
                    outcome: if be_active { Outcome::Be } else { Outcome::Sl },
                    exit_price: current_stop,
                    r_multiple: if be_active { 0.0 } else { -1.0 },
                },
            ));
        }

        if hit_target(candle, is_buy, levels.take_profit_2) {
            let r_multiple = if management.partial_at_tp1 {
                0.5 * tp1_r + 0.5 * tp2_r
            } else {
                tp2_r
            };
            return Some(build_trade(
                candles,
                raw,
                entry_index,
                index,
                levels,
                TradeResult {
                    outcome: Outcome::Tp2,
                    exit_price: levels.take_profit_2,
                    r_multiple,
                },
            ));
        }

        if !tp1_hit && hit_target(candle, is_buy, levels.take_profit_1) {
            tp1_hit = true;
        }

        if time_exit_eligible(
            management.time_exit,
            candles,
            index,
            entry_timestamp,
            levels.entry,
        ) {
            let close_r = levels.r_multiple(is_buy, candle.close);
            if tp1_hit {
                if management.partial_at_tp1 {
                    return Some(build_trade(
                        candles,
                        raw,
                        entry_index,
                        index,
                        levels,
                        TradeResult {
                            outcome: Outcome::TimeExit,
                            exit_price: candle.close,
                            r_multiple: 0.5 * tp1_r + 0.5 * close_r,
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
                    outcome: Outcome::TimeExit,
                    exit_price: candle.close,
                    r_multiple: close_r,
                },
            ));
        }

        // BE activation is evaluated on completed candles and becomes active
        // on the following candle, avoiding optimistic same-candle BE fills.
        let favorable = if is_buy {
            candle.high - levels.entry
        } else {
            levels.entry - candle.low
        };
        mfe_trigger = mfe_trigger.max(favorable.max(0.0));
        if !be_active {
            if let Some(trigger) = management.be_trigger_usd {
                if mfe_trigger >= trigger {
                    be_active = true;
                }
            }
        }
    }

    let exit_index = candles.len() - 1;
    let last_close = candles
        .last()
        .map(|candle| candle.close)
        .unwrap_or(levels.entry);
    let close_r = levels.r_multiple(is_buy, last_close);

    if tp1_hit {
        if management.partial_at_tp1 {
            return Some(build_trade(
                candles,
                raw,
                entry_index,
                exit_index,
                levels,
                TradeResult {
                    outcome: Outcome::TimeExit,
                    exit_price: last_close,
                    r_multiple: 0.5 * tp1_r + 0.5 * close_r,
                },
            ));
        }
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
            r_multiple: close_r,
        },
    ))
}

/// Phase 1C partial/trailing management flags.
#[derive(Debug, Clone, Copy)]
struct PartialRule {
    /// Fraction of the position closed at the trigger.
    fraction: f64,
    /// Favorable excursion in USD from entry that books the partial.
    trigger_usd: f64,
}

impl PartialRule {
    const fn new(fraction: f64, trigger_usd: f64) -> Self {
        Self {
            fraction,
            trigger_usd,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct TrailRule {
    /// MFE in USD required before the trail activates.
    trigger_usd: f64,
    /// Distance kept behind the high/low-water mark.
    distance_usd: f64,
    /// Price quantization step in USD.
    step_usd: f64,
}

impl TrailRule {
    const fn new(trigger_usd: f64, distance_usd: f64, step_usd: f64) -> Self {
        Self {
            trigger_usd,
            distance_usd,
            step_usd,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct PartialTrailManagement {
    partial: Option<PartialRule>,
    trail: Option<TrailRule>,
}

fn blended_r(fraction: f64, partial_taken: bool, partial_r: f64, remainder_r: f64) -> f64 {
    if partial_taken {
        fraction * partial_r + (1.0 - fraction) * remainder_r
    } else {
        remainder_r
    }
}

/// Baseline management plus optional partial booking and trailing-stop rules.
///
/// A partial is booked before TP1 only when its USD trigger is closer to entry
/// than TP1. The trailing stop is computed from completed candles and becomes
/// binding on the next M15 candle, so no same-candle optimistic fill is
/// possible. After TP1, a stop-out settles at TP1 unless the trail has
/// ratcheted beyond TP1, preserving baseline "TP1 banked" behavior without
/// introducing a hard TP1 stop.
fn simulate_partial_trail(
    candles: &[Candle],
    entry_index: usize,
    raw: &RawSignal,
    levels: Levels,
    management: PartialTrailManagement,
) -> Option<SimulatedTrade> {
    if !levels.is_ordered(raw.is_buy()) || entry_index + 1 >= candles.len() {
        return None;
    }

    let is_buy = raw.is_buy();
    let initial_stop = levels.stop_loss;
    let take_profit_1 = levels.take_profit_1;
    let take_profit_2 = levels.take_profit_2;
    let tp1_r = levels.r_multiple(is_buy, take_profit_1);
    let tp2_r = levels.r_multiple(is_buy, take_profit_2);
    let partial = management.partial;
    let partial_fraction = partial.map_or(0.0, |rule| rule.fraction.clamp(0.0, 1.0));
    let partial_r = partial.map_or(0.0, |rule| rule.trigger_usd / levels.risk());
    let partial_trigger = partial.map(|rule| {
        if is_buy {
            levels.entry + rule.trigger_usd
        } else {
            levels.entry - rule.trigger_usd
        }
    });
    let trigger_before_tp1 = partial_trigger.is_some_and(|trigger| {
        if is_buy {
            trigger < take_profit_1
        } else {
            trigger > take_profit_1
        }
    });

    let mut tp1_hit = false;
    let mut partial_taken = false;
    let mut mfe = 0.0f64;
    let mut trail_stop = initial_stop;

    for index in (entry_index + 1)..candles.len() {
        let candle = &candles[index];

        let current_stop = if is_buy {
            initial_stop.max(trail_stop)
        } else {
            initial_stop.min(trail_stop)
        };

        // Conservative intra-candle priority: stop before targets.
        if hit_stop(candle, is_buy, current_stop) {
            let trail_improves_stop = if is_buy {
                current_stop > initial_stop + 1e-9
            } else {
                current_stop < initial_stop - 1e-9
            };
            let trail_beyond_tp1 = if is_buy {
                current_stop > take_profit_1 + 1e-9
            } else {
                current_stop < take_profit_1 - 1e-9
            };

            let (outcome, exit_price, remainder_r) = if tp1_hit {
                if trail_improves_stop && trail_beyond_tp1 {
                    (
                        Outcome::Trail,
                        current_stop,
                        levels.r_multiple(is_buy, current_stop),
                    )
                } else {
                    (Outcome::Tp1, take_profit_1, tp1_r)
                }
            } else if trail_improves_stop {
                (
                    Outcome::Trail,
                    current_stop,
                    levels.r_multiple(is_buy, current_stop),
                )
            } else {
                (Outcome::Sl, initial_stop, -1.0)
            };

            return Some(build_trade(
                candles,
                raw,
                entry_index,
                index,
                levels,
                TradeResult {
                    outcome,
                    exit_price,
                    r_multiple: blended_r(partial_fraction, partial_taken, partial_r, remainder_r),
                },
            ));
        }

        // The partial trigger is booked before TP1 only when it is closer to
        // entry than TP1.
        if let Some(trigger) = partial_trigger {
            if !partial_taken
                && !tp1_hit
                && trigger_before_tp1
                && hit_target(candle, is_buy, trigger)
            {
                partial_taken = true;
            }
        }

        if hit_target(candle, is_buy, take_profit_2) {
            return Some(build_trade(
                candles,
                raw,
                entry_index,
                index,
                levels,
                TradeResult {
                    outcome: Outcome::Tp2,
                    exit_price: take_profit_2,
                    r_multiple: blended_r(partial_fraction, partial_taken, partial_r, tp2_r),
                },
            ));
        }

        if !tp1_hit && hit_target(candle, is_buy, take_profit_1) {
            tp1_hit = true;
        }

        let favorable = if is_buy {
            candle.high - levels.entry
        } else {
            levels.entry - candle.low
        };
        mfe = mfe.max(favorable.max(0.0));

        if let Some(rule) = management.trail {
            if mfe >= rule.trigger_usd && rule.step_usd > 0.0 {
                let quantized_stop = if is_buy {
                    ((levels.entry + mfe) / rule.step_usd).floor() * rule.step_usd
                        - rule.distance_usd
                } else {
                    ((levels.entry - mfe) / rule.step_usd).ceil() * rule.step_usd
                        + rule.distance_usd
                };
                trail_stop = if is_buy {
                    quantized_stop.max(initial_stop)
                } else {
                    quantized_stop.min(initial_stop)
                };
            }
        }
    }

    let exit_index = candles.len() - 1;
    let last_close = candles
        .last()
        .map(|candle| candle.close)
        .unwrap_or(levels.entry);
    let close_r = levels.r_multiple(is_buy, last_close);

    if tp1_hit {
        return Some(build_trade(
            candles,
            raw,
            entry_index,
            exit_index,
            levels,
            TradeResult {
                outcome: Outcome::Tp1,
                exit_price: take_profit_1,
                r_multiple: blended_r(partial_fraction, partial_taken, partial_r, tp1_r),
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
            r_multiple: blended_r(partial_fraction, partial_taken, partial_r, close_r),
        },
    ))
}

/// Attach MFE/MAE and cost-adjusted fields to a simulated trade.
///
/// MFE/MAE are measured in USD from entry across all candles from the first
/// post-entry candle through the exit candle. MFE before TP1 includes the
/// candle that first touches TP1; MFE after TP1 starts on the next candle.
fn attach_excursions(candles: &[Candle], trade: &mut SimulatedTrade, cost_usd: f64) {
    let is_buy = trade.signal_type == SignalType::Buy;
    let entry = trade.entry_price;
    let mut mfe = 0.0f64;
    let mut mae = 0.0f64;
    let mut mfe_before_tp1 = 0.0f64;
    let mut mfe_after_tp1 = 0.0f64;
    let mut tp1_hit = false;

    let start = trade.entry_index.saturating_add(1);
    let end = trade.exit_index.min(candles.len().saturating_sub(1));

    if start <= end {
        for candle in &candles[start..=end] {
            let favorable = if is_buy {
                candle.high - entry
            } else {
                entry - candle.low
            }
            .max(0.0);
            let adverse = if is_buy {
                entry - candle.low
            } else {
                candle.high - entry
            }
            .max(0.0);

            mfe = mfe.max(favorable);
            mae = mae.max(adverse);

            if !tp1_hit {
                mfe_before_tp1 = mfe_before_tp1.max(favorable);
                if hit_target(candle, is_buy, trade.take_profit_1) {
                    tp1_hit = true;
                }
            } else {
                mfe_after_tp1 = mfe_after_tp1.max(favorable);
            }
        }
    }

    trade.mfe_usd = mfe;
    trade.mae_usd = mae;
    trade.mfe_before_tp1_usd = mfe_before_tp1;
    trade.mfe_after_tp1_usd = mfe_after_tp1;
    trade.cost_usd = cost_usd;
}

// ---------------------------------------------------------------------------
// Metrics and reports
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
struct MfeBucket {
    threshold_usd: f64,
    reached: usize,
    pct_of_trades: f64,
    ended_tp1: usize,
    ended_tp2: usize,
    ended_sl: usize,
    ended_be: usize,
    ended_time_exit: usize,
    ended_trail: usize,
    reversed: usize,
}

#[derive(Debug, Clone, Serialize)]
struct Metrics {
    trades: usize,
    wins: usize,
    losses: usize,
    tp1: usize,
    tp2: usize,
    be: usize,
    time_exits: usize,
    trail: usize,
    win_rate_pct: f64,
    /// Gross R metrics (before costs).
    avg_r: f64,
    gross_r: f64,
    profit_factor: Option<f64>,
    max_drawdown_r: f64,
    expectancy_r: f64,
    /// Net-of-cost R metrics.
    avg_net_r: f64,
    net_r: f64,
    net_r_after_costs: f64,
    profit_factor_after_costs: Option<f64>,
    max_drawdown_r_after_costs: f64,
    expectancy_after_costs_r: f64,
    avg_duration_candles: f64,
    avg_mfe_usd: f64,
    avg_mae_usd: f64,
    avg_mfe_r: f64,
    avg_mae_r: f64,
    mfe_buckets: Vec<MfeBucket>,
}

impl Metrics {
    fn empty() -> Self {
        Self {
            trades: 0,
            wins: 0,
            losses: 0,
            tp1: 0,
            tp2: 0,
            be: 0,
            time_exits: 0,
            trail: 0,
            win_rate_pct: 0.0,
            avg_r: 0.0,
            gross_r: 0.0,
            profit_factor: None,
            max_drawdown_r: 0.0,
            expectancy_r: 0.0,
            avg_net_r: 0.0,
            net_r: 0.0,
            net_r_after_costs: 0.0,
            profit_factor_after_costs: None,
            max_drawdown_r_after_costs: 0.0,
            expectancy_after_costs_r: 0.0,
            avg_duration_candles: 0.0,
            avg_mfe_usd: 0.0,
            avg_mae_usd: 0.0,
            avg_mfe_r: 0.0,
            avg_mae_r: 0.0,
            mfe_buckets: mfe_buckets(&[]),
        }
    }
}

fn profit_factor_from(values: &[f64]) -> Option<f64> {
    let gross_profit: f64 = values.iter().copied().filter(|value| *value > 0.0).sum();
    let gross_loss: f64 = values
        .iter()
        .copied()
        .filter(|value| *value < 0.0)
        .map(f64::abs)
        .sum();

    if gross_loss > 0.0 {
        Some(gross_profit / gross_loss)
    } else if gross_profit > 0.0 {
        None
    } else {
        Some(0.0)
    }
}

fn max_drawdown_from(values: &[f64]) -> f64 {
    let mut cumulative = 0.0f64;
    let mut peak = 0.0f64;
    let mut max_drawdown = 0.0f64;
    for value in values {
        cumulative += value;
        peak = peak.max(cumulative);
        max_drawdown = max_drawdown.max(peak - cumulative);
    }
    max_drawdown
}

fn mfe_buckets(trades: &[&SimulatedTrade]) -> Vec<MfeBucket> {
    const THRESHOLDS: [f64; 6] = [10.0, 15.0, 20.0, 25.0, 30.0, 40.0];

    THRESHOLDS
        .iter()
        .map(|threshold| {
            let reached_trades: Vec<&&SimulatedTrade> = trades
                .iter()
                .filter(|trade| trade.mfe_usd >= *threshold)
                .collect();
            let reached = reached_trades.len();
            let ended_tp1 = reached_trades
                .iter()
                .filter(|trade| trade.outcome == Outcome::Tp1)
                .count();
            let ended_tp2 = reached_trades
                .iter()
                .filter(|trade| trade.outcome == Outcome::Tp2)
                .count();
            let ended_sl = reached_trades
                .iter()
                .filter(|trade| trade.outcome == Outcome::Sl)
                .count();
            let ended_be = reached_trades
                .iter()
                .filter(|trade| trade.outcome == Outcome::Be)
                .count();
            let ended_time_exit = reached_trades
                .iter()
                .filter(|trade| trade.outcome == Outcome::TimeExit)
                .count();
            let ended_trail = reached_trades
                .iter()
                .filter(|trade| trade.outcome == Outcome::Trail)
                .count();

            MfeBucket {
                threshold_usd: *threshold,
                reached,
                pct_of_trades: if trades.is_empty() {
                    0.0
                } else {
                    round_to(reached as f64 / trades.len() as f64 * 100.0, 2)
                },
                ended_tp1,
                ended_tp2,
                ended_sl,
                ended_be,
                ended_time_exit,
                ended_trail,
                reversed: ended_sl + ended_be + ended_time_exit,
            }
        })
        .collect()
}

fn compute_metrics<'a>(trades: impl IntoIterator<Item = &'a SimulatedTrade>) -> Metrics {
    let trades: Vec<&SimulatedTrade> = trades.into_iter().collect();
    let total = trades.len();
    if total == 0 {
        return Metrics::empty();
    }

    let tp1 = trades
        .iter()
        .filter(|trade| trade.outcome == Outcome::Tp1)
        .count();
    let tp2 = trades
        .iter()
        .filter(|trade| trade.outcome == Outcome::Tp2)
        .count();
    let trail = trades
        .iter()
        .filter(|trade| trade.outcome == Outcome::Trail)
        .count();
    let wins = tp1 + tp2 + trail;
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

    let gross_values: Vec<f64> = trades.iter().map(|trade| trade.r_multiple).collect();
    let net_values: Vec<f64> = trades.iter().map(|trade| trade.net_r()).collect();

    let net_r: f64 = net_values.iter().sum();
    let gross_r: f64 = gross_values.iter().sum();
    let avg_r = gross_r / total as f64;
    let avg_net_r = net_r / total as f64;

    let profit_factor = profit_factor_from(&gross_values);
    let profit_factor_after_costs = profit_factor_from(&net_values);
    let max_drawdown = max_drawdown_from(&gross_values);
    let max_drawdown_after_costs = max_drawdown_from(&net_values);

    let duration_total: usize = trades.iter().map(|trade| trade.duration_candles).sum();
    let mfe_total: f64 = trades.iter().map(|trade| trade.mfe_usd).sum();
    let mae_total: f64 = trades.iter().map(|trade| trade.mae_usd).sum();
    let mfe_r_total: f64 = trades.iter().map(|trade| trade.mfe_r()).sum();
    let mae_r_total: f64 = trades.iter().map(|trade| trade.mae_r()).sum();

    Metrics {
        trades: total,
        wins,
        losses,
        tp1,
        tp2,
        be,
        time_exits,
        trail,
        win_rate_pct: round_to(wins as f64 / total as f64 * 100.0, 2),
        avg_r: round_to(avg_r, 4),
        gross_r: round_to(gross_r, 4),
        profit_factor: profit_factor.map(|value| round_to(value, 4)),
        max_drawdown_r: round_to(max_drawdown, 4),
        expectancy_r: round_to(avg_r, 4),
        avg_net_r: round_to(avg_net_r, 4),
        net_r: round_to(net_r, 4),
        net_r_after_costs: round_to(net_r, 4),
        profit_factor_after_costs: profit_factor_after_costs.map(|value| round_to(value, 4)),
        max_drawdown_r_after_costs: round_to(max_drawdown_after_costs, 4),
        expectancy_after_costs_r: round_to(avg_net_r, 4),
        avg_duration_candles: round_to(duration_total as f64 / total as f64, 2),
        avg_mfe_usd: round_to(mfe_total / total as f64, 2),
        avg_mae_usd: round_to(mae_total / total as f64, 2),
        avg_mfe_r: round_to(mfe_r_total / total as f64, 4),
        avg_mae_r: round_to(mae_r_total / total as f64, 4),
        mfe_buckets: mfe_buckets(&trades),
    }
}

#[derive(Debug, Clone, Serialize)]
struct WindowMetrics {
    trades: usize,
    net_r: f64,
    profit_factor: Option<f64>,
    max_drawdown_r: f64,
    expectancy_r: f64,
    win_rate_pct: f64,
    sample_thin: bool,
}

impl WindowMetrics {
    fn from_metrics(metrics: &Metrics) -> Self {
        Self {
            trades: metrics.trades,
            net_r: metrics.net_r_after_costs,
            profit_factor: metrics.profit_factor_after_costs,
            max_drawdown_r: metrics.max_drawdown_r_after_costs,
            expectancy_r: metrics.expectancy_after_costs_r,
            win_rate_pct: metrics.win_rate_pct,
            sample_thin: metrics.trades < 30,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct WalkForwardWindow {
    label: String,
    start: String,
    end: String,
    metrics: WindowMetrics,
}

#[derive(Debug, Clone, Serialize, Default)]
struct WalkForwardSummary {
    windows: usize,
    evaluated_windows: usize,
    positive_windows: usize,
    negative_windows: usize,
    flat_windows: usize,
    best_net_r: f64,
    worst_net_r: f64,
    consistency_pct: f64,
}

fn summarize_walk_forward(windows: &[WalkForwardWindow]) -> WalkForwardSummary {
    let mut summary = WalkForwardSummary {
        windows: windows.len(),
        best_net_r: f64::NEG_INFINITY,
        worst_net_r: f64::INFINITY,
        ..WalkForwardSummary::default()
    };

    for window in windows {
        let metrics = &window.metrics;
        if metrics.trades == 0 {
            continue;
        }
        summary.evaluated_windows += 1;
        if metrics.net_r > 0.0 {
            summary.positive_windows += 1;
        } else if metrics.net_r < 0.0 {
            summary.negative_windows += 1;
        } else {
            summary.flat_windows += 1;
        }
        summary.best_net_r = summary.best_net_r.max(metrics.net_r);
        summary.worst_net_r = summary.worst_net_r.min(metrics.net_r);
    }

    if summary.evaluated_windows == 0 {
        summary.best_net_r = 0.0;
        summary.worst_net_r = 0.0;
    }
    summary.consistency_pct = if summary.evaluated_windows == 0 {
        0.0
    } else {
        round_to(
            summary.positive_windows as f64 / summary.evaluated_windows as f64 * 100.0,
            2,
        )
    };
    summary
}

#[derive(Debug, Clone, Serialize)]
struct RollingWindow {
    label: String,
    is_start: String,
    is_end: String,
    oos_start: String,
    oos_end: String,
    is_metrics: WindowMetrics,
    oos_metrics: WindowMetrics,
}

#[derive(Debug, Clone, Serialize)]
struct TopCandidate {
    name: String,
    trades: usize,
    net_r: f64,
    positive_windows: usize,
    evaluated_windows: usize,
    consistency_pct: f64,
    rolling_is_oos: Vec<RollingWindow>,
}

#[derive(Debug, Clone, Serialize)]
struct VariantReport {
    name: String,
    signals: usize,
    trades: usize,
    rejected: usize,
    reject_reasons: BTreeMap<String, usize>,
    sample_thin: bool,
    overall: Metrics,
    in_sample: Metrics,
    out_of_sample: Metrics,
    yearly: BTreeMap<String, Metrics>,
    walk_forward: Vec<WalkForwardWindow>,
    walk_forward_summary: WalkForwardSummary,
    rolling_is_oos: Vec<RollingWindow>,
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
    cost_usd_per_trade: f64,
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
    top_candidate: Option<TopCandidate>,
    variants: Vec<VariantReport>,
}

#[derive(Debug, Clone)]
struct DateWindow {
    label: String,
    start_date: NaiveDate,
    end_date: NaiveDate,
    start_ms: i64,
    /// Exclusive end timestamp used for filtering.
    end_ms: i64,
}

#[derive(Debug, Clone)]
struct RollingWindowDef {
    label: String,
    is_start_date: NaiveDate,
    is_end_date: NaiveDate,
    oos_start_date: NaiveDate,
    oos_end_date: NaiveDate,
    is_start_ms: i64,
    is_end_ms: i64,
    oos_start_ms: i64,
    oos_end_ms: i64,
}

struct BacktestContext<'a> {
    candles: &'a [Candle],
    h1: &'a [Candle],
    h4: &'a [Candle],
    htf: &'a HtfContext,
    /// Phase 1D cumulative delta, aligned with `candles`.
    cvd: &'a [f64],
    /// Phase R0 Wilder RSI14, aligned with `candles`.
    rsi: &'a [Option<f64>],
    split_timestamp: i64,
    early_be: f64,
    cost_usd: f64,
    step_ms: i64,
    walk_forward: &'a [DateWindow],
    rolling: &'a [RollingWindowDef],
}

#[derive(Debug, Clone, Copy)]
enum LevelSource {
    Analyzer,
    TpBand,
    ConfirmSession,
    ConfirmationSoft,
    TpStructure,
    TpStructureSoft,
    TpStructureH4,
    SwingSl,
}

#[derive(Debug, Clone, Copy)]
enum SimKind {
    Standard,
    UsClose,
    PartialBe,
    Be15,
    Be20,
    Scale50,
    Scale50Be15,
    TimeOutChoppy,
    TimeOutAll,
    Partial25,
    Trail20,
    Partial25Trail,
    Partial10At20,
    Partial15At25,
    Trail30,
}

#[derive(Debug, Clone, Copy)]
struct VariantPlan {
    levels: LevelSource,
    simulation: SimKind,
    htf: Option<HtfFilter>,
    delta: Option<DeltaFilter>,
    rsi: Option<RsiGate>,
    volume: Option<VolumeFilter>,
}

fn variant_plan(variant: Variant) -> VariantPlan {
    let (levels, simulation, htf, delta) = match variant {
        Variant::Baseline => (LevelSource::Analyzer, SimKind::Standard, None, None),
        Variant::TpBand => (LevelSource::TpBand, SimKind::Standard, None, None),
        Variant::PartialBe => (LevelSource::Analyzer, SimKind::PartialBe, None, None),
        Variant::ConfirmSession => (LevelSource::ConfirmSession, SimKind::UsClose, None, None),
        Variant::TimeOut02Am => (LevelSource::Analyzer, SimKind::TimeOutChoppy, None, None),
        Variant::TimeOut02AmAll => (LevelSource::Analyzer, SimKind::TimeOutAll, None, None),
        Variant::Be15 => (LevelSource::Analyzer, SimKind::Be15, None, None),
        Variant::Be20 => (LevelSource::Analyzer, SimKind::Be20, None, None),
        Variant::Scale50Tp1 => (LevelSource::Analyzer, SimKind::Scale50, None, None),
        Variant::Scale50Tp1Be15 => (LevelSource::Analyzer, SimKind::Scale50Be15, None, None),
        Variant::ConfirmationSoft => (LevelSource::ConfirmationSoft, SimKind::Standard, None, None),
        Variant::TpStructure => (LevelSource::TpStructure, SimKind::Standard, None, None),
        Variant::TpStructureSoft => (LevelSource::TpStructureSoft, SimKind::Standard, None, None),
        Variant::TpStructureH4 => (LevelSource::TpStructureH4, SimKind::Standard, None, None),
        Variant::HtfH4 => (
            LevelSource::Analyzer,
            SimKind::Standard,
            Some(HtfFilter::H4),
            None,
        ),
        Variant::HtfH1Channel => (
            LevelSource::Analyzer,
            SimKind::Standard,
            Some(HtfFilter::H1Channel),
            None,
        ),
        Variant::HtfCombined => (
            LevelSource::Analyzer,
            SimKind::Standard,
            Some(HtfFilter::Combined),
            None,
        ),
        Variant::HtfCombinedTpStructure => (
            LevelSource::TpStructureSoft,
            SimKind::Standard,
            Some(HtfFilter::Combined),
            None,
        ),
        Variant::SwingSl => (LevelSource::SwingSl, SimKind::Standard, None, None),
        Variant::SwingSlHtf => (
            LevelSource::SwingSl,
            SimKind::Standard,
            Some(HtfFilter::Combined),
            None,
        ),
        Variant::Partial25At20 => (LevelSource::Analyzer, SimKind::Partial25, None, None),
        Variant::TrailAfter20 => (LevelSource::Analyzer, SimKind::Trail20, None, None),
        Variant::Partial25Trail => (LevelSource::Analyzer, SimKind::Partial25Trail, None, None),
        Variant::HtfCombinedPartial25Trail => (
            LevelSource::Analyzer,
            SimKind::Partial25Trail,
            Some(HtfFilter::Combined),
            None,
        ),
        Variant::Partial10At20 => (LevelSource::Analyzer, SimKind::Partial10At20, None, None),
        Variant::Partial15At25 => (LevelSource::Analyzer, SimKind::Partial15At25, None, None),
        Variant::TrailAfter30 => (LevelSource::Analyzer, SimKind::Trail30, None, None),
        Variant::HtfH4Partial10 => (
            LevelSource::Analyzer,
            SimKind::Partial10At20,
            Some(HtfFilter::H4),
            None,
        ),
        Variant::HtfH4Partial15 => (
            LevelSource::Analyzer,
            SimKind::Partial15At25,
            Some(HtfFilter::H4),
            None,
        ),
        Variant::HtfH4Trail30 => (
            LevelSource::Analyzer,
            SimKind::Trail30,
            Some(HtfFilter::H4),
            None,
        ),
        Variant::HtfH4Zone => (
            LevelSource::Analyzer,
            SimKind::Standard,
            Some(HtfFilter::H4Zone),
            None,
        ),
        Variant::HtfH4TpStructureSoft => (
            LevelSource::TpStructureSoft,
            SimKind::Standard,
            Some(HtfFilter::H4),
            None,
        ),
        Variant::HtfH4D1 => (
            LevelSource::Analyzer,
            SimKind::Standard,
            Some(HtfFilter::H4D1),
            None,
        ),
        Variant::DeltaAlign => (
            LevelSource::Analyzer,
            SimKind::Standard,
            None,
            Some(DeltaFilter::Align),
        ),
        Variant::DeltaDivergence => (
            LevelSource::Analyzer,
            SimKind::Standard,
            None,
            Some(DeltaFilter::Divergence),
        ),
        Variant::DeltaBoth => (
            LevelSource::Analyzer,
            SimKind::Standard,
            None,
            Some(DeltaFilter::Both),
        ),
        Variant::HtfH4DeltaAlign => (
            LevelSource::Analyzer,
            SimKind::Standard,
            Some(HtfFilter::H4),
            Some(DeltaFilter::Align),
        ),
        Variant::HtfH4D1DeltaAlign => (
            LevelSource::Analyzer,
            SimKind::Standard,
            Some(HtfFilter::H4D1),
            Some(DeltaFilter::Align),
        ),
        Variant::HtfH4D1DeltaBoth => (
            LevelSource::Analyzer,
            SimKind::Standard,
            Some(HtfFilter::H4D1),
            Some(DeltaFilter::Both),
        ),
        Variant::RsiExtreme
        | Variant::RsiExtreme6535
        | Variant::RsiDivergence
        | Variant::RsiBoth
        | Variant::VolumeExhaustion
        | Variant::VolumeEither
        | Variant::RsiDivergenceVolumeExhaustion
        | Variant::RsiRecentExtreme10
        | Variant::RsiRecentExtreme20
        | Variant::RsiRecentExtreme10_6535
        | Variant::RsiRecentExtreme20_6535
        | Variant::RsiRecentExtreme20OrDivergence
        | Variant::RsiRegime50
        | Variant::RsiRegimeCross50
        | Variant::RsiSlope3 => (LevelSource::Analyzer, SimKind::Standard, None, None),
        Variant::HtfH4D1RsiExtreme
        | Variant::HtfH4D1RsiBoth
        | Variant::HtfH4D1RsiVolumeEither
        | Variant::HtfH4D1RsiRecentExtreme10
        | Variant::HtfH4D1RsiRecentExtreme20
        | Variant::HtfH4D1RsiRecentExtreme20_6535
        | Variant::HtfH4D1RsiRecentExtreme20OrDivergence
        | Variant::HtfH4D1RsiRegime50
        | Variant::HtfH4D1RsiRegimeCross50
        | Variant::HtfH4D1RsiSlope3 => (
            LevelSource::Analyzer,
            SimKind::Standard,
            Some(HtfFilter::H4D1),
            None,
        ),
    };

    let (rsi, volume) = match variant {
        Variant::RsiExtreme | Variant::HtfH4D1RsiExtreme => (
            Some(RsiGate::Extreme {
                overbought: RSI_OVERBOUGHT,
                oversold: RSI_OVERSOLD,
            }),
            None,
        ),
        Variant::RsiExtreme6535 => (
            Some(RsiGate::Extreme {
                overbought: RSI_OVERBOUGHT_ALT,
                oversold: RSI_OVERSOLD_ALT,
            }),
            None,
        ),
        Variant::RsiDivergence => (Some(RsiGate::Divergence), None),
        Variant::RsiBoth | Variant::HtfH4D1RsiBoth => (
            Some(RsiGate::Both {
                overbought: RSI_OVERBOUGHT,
                oversold: RSI_OVERSOLD,
            }),
            None,
        ),
        Variant::VolumeExhaustion => (None, Some(VolumeFilter::Exhaustion)),
        Variant::VolumeEither => (None, Some(VolumeFilter::Either)),
        Variant::RsiDivergenceVolumeExhaustion => {
            (Some(RsiGate::Divergence), Some(VolumeFilter::Exhaustion))
        }
        Variant::HtfH4D1RsiVolumeEither => (
            Some(RsiGate::Both {
                overbought: RSI_OVERBOUGHT,
                oversold: RSI_OVERSOLD,
            }),
            Some(VolumeFilter::Either),
        ),
        Variant::RsiRecentExtreme10 | Variant::HtfH4D1RsiRecentExtreme10 => (
            Some(RsiGate::RecentExtreme {
                overbought: RSI_OVERBOUGHT,
                oversold: RSI_OVERSOLD,
                lookback: RSI_RECENT_LOOKBACK_SHORT,
            }),
            None,
        ),
        Variant::RsiRecentExtreme20 | Variant::HtfH4D1RsiRecentExtreme20 => (
            Some(RsiGate::RecentExtreme {
                overbought: RSI_OVERBOUGHT,
                oversold: RSI_OVERSOLD,
                lookback: RSI_RECENT_LOOKBACK_LONG,
            }),
            None,
        ),
        Variant::RsiRecentExtreme10_6535 => (
            Some(RsiGate::RecentExtreme {
                overbought: RSI_OVERBOUGHT_ALT,
                oversold: RSI_OVERSOLD_ALT,
                lookback: RSI_RECENT_LOOKBACK_SHORT,
            }),
            None,
        ),
        Variant::RsiRecentExtreme20_6535 | Variant::HtfH4D1RsiRecentExtreme20_6535 => (
            Some(RsiGate::RecentExtreme {
                overbought: RSI_OVERBOUGHT_ALT,
                oversold: RSI_OVERSOLD_ALT,
                lookback: RSI_RECENT_LOOKBACK_LONG,
            }),
            None,
        ),
        Variant::RsiRecentExtreme20OrDivergence
        | Variant::HtfH4D1RsiRecentExtreme20OrDivergence => (
            Some(RsiGate::RecentExtremeOrDivergence {
                overbought: RSI_OVERBOUGHT,
                oversold: RSI_OVERSOLD,
                lookback: RSI_RECENT_LOOKBACK_LONG,
            }),
            None,
        ),
        Variant::RsiRegime50 | Variant::HtfH4D1RsiRegime50 => (
            Some(RsiGate::Regime {
                lookback: RSI_REGIME_LOOKBACK,
                threshold: RSI_REGIME_THRESHOLD,
            }),
            None,
        ),
        Variant::RsiRegimeCross50 | Variant::HtfH4D1RsiRegimeCross50 => (
            Some(RsiGate::RegimeCross {
                lookback: RSI_REGIME_CROSS_LOOKBACK,
                threshold: RSI_REGIME_THRESHOLD,
            }),
            None,
        ),
        Variant::RsiSlope3 | Variant::HtfH4D1RsiSlope3 => (
            Some(RsiGate::Slope {
                lookback: RSI_SLOPE_LOOKBACK,
                min_delta: RSI_SLOPE_MIN_DELTA,
            }),
            None,
        ),
        _ => (None, None),
    };

    VariantPlan {
        levels,
        simulation,
        htf,
        delta,
        rsi,
        volume,
    }
}

fn year_key(timestamp_ms: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(timestamp_ms)
        .map(|datetime| datetime.with_timezone(&vietnam_offset()).year().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn midnight_ms(date: NaiveDate) -> i64 {
    date.and_hms_opt(0, 0, 0)
        .expect("valid midnight")
        .and_utc()
        .timestamp_millis()
}

/// Exclusive timestamp for the instant after the final second of `date`.
fn end_of_day_exclusive_ms(date: NaiveDate) -> i64 {
    date.and_hms_opt(23, 59, 59)
        .expect("valid end of day")
        .and_utc()
        .timestamp_millis()
        .saturating_add(1)
}

/// Fixed one-year windows W1..W5 starting at the requested start date.
///
/// Intermediate windows end at midnight of their boundary date, so a signal at
/// exactly 00:00 on 2022-11-10 belongs to W2. The final window extends through
/// the requested end date.
fn walk_forward_windows(start: NaiveDate, end: NaiveDate) -> Vec<DateWindow> {
    const WINDOW_COUNT: u32 = 5;
    const WINDOW_MONTHS: u32 = 12;

    let range_end_exclusive = end_of_day_exclusive_ms(end);
    let mut windows = Vec::new();

    for index in 0..WINDOW_COUNT {
        let Some(window_start) = start.checked_add_months(Months::new(index * WINDOW_MONTHS))
        else {
            break;
        };
        if window_start > end {
            break;
        }
        let Some(window_end) = start.checked_add_months(Months::new((index + 1) * WINDOW_MONTHS))
        else {
            break;
        };

        let start_ms = midnight_ms(window_start);
        let end_ms = if index + 1 == WINDOW_COUNT {
            range_end_exclusive
        } else {
            midnight_ms(window_end)
        }
        .min(range_end_exclusive);
        if start_ms >= end_ms {
            break;
        }

        windows.push(DateWindow {
            label: format!("W{}", index + 1),
            start_date: window_start,
            end_date: window_end.min(end),
            start_ms,
            end_ms,
        });
    }

    windows
}

/// Rolling 3-year in-sample / 1-year out-of-sample windows, stepped annually.
fn rolling_is_oos_windows(start: NaiveDate, end: NaiveDate) -> Vec<RollingWindowDef> {
    const STEP_MONTHS: u32 = 12;
    const IS_MONTHS: u32 = 36;
    const OOS_MONTHS: u32 = 12;

    let end_midnight = midnight_ms(end);
    let range_end_exclusive = end_of_day_exclusive_ms(end);
    let mut windows = Vec::new();
    let mut index = 0u32;

    while let Some(is_start) = start.checked_add_months(Months::new(index * STEP_MONTHS)) {
        let Some(is_end) = start.checked_add_months(Months::new(index * STEP_MONTHS + IS_MONTHS))
        else {
            break;
        };
        let Some(oos_end) =
            start.checked_add_months(Months::new(index * STEP_MONTHS + IS_MONTHS + OOS_MONTHS))
        else {
            break;
        };

        let is_start_ms = midnight_ms(is_start);
        let is_end_ms = midnight_ms(is_end);
        let oos_start_ms = is_end_ms;
        if oos_start_ms >= end_midnight {
            break;
        }
        let oos_end_ms = if oos_end >= end {
            range_end_exclusive
        } else {
            midnight_ms(oos_end)
        };

        windows.push(RollingWindowDef {
            label: format!("R{}", index + 1),
            is_start_date: is_start,
            is_end_date: is_end,
            oos_start_date: is_end,
            oos_end_date: oos_end.min(end),
            is_start_ms,
            is_end_ms,
            oos_start_ms,
            oos_end_ms,
        });
        index += 1;
    }

    windows
}

fn window_metrics_for_range(
    trades: &[SimulatedTrade],
    start_ms: i64,
    end_ms: i64,
) -> WindowMetrics {
    WindowMetrics::from_metrics(&compute_metrics(trades.iter().filter(|trade| {
        trade.entry_timestamp >= start_ms && trade.entry_timestamp < end_ms
    })))
}

fn build_walk_forward(trades: &[SimulatedTrade], windows: &[DateWindow]) -> Vec<WalkForwardWindow> {
    windows
        .iter()
        .map(|window| WalkForwardWindow {
            label: window.label.clone(),
            start: window.start_date.to_string(),
            end: window.end_date.to_string(),
            metrics: window_metrics_for_range(trades, window.start_ms, window.end_ms),
        })
        .collect()
}

fn build_rolling_is_oos(
    trades: &[SimulatedTrade],
    windows: &[RollingWindowDef],
) -> Vec<RollingWindow> {
    windows
        .iter()
        .map(|window| RollingWindow {
            label: window.label.clone(),
            is_start: window.is_start_date.to_string(),
            is_end: window.is_end_date.to_string(),
            oos_start: window.oos_start_date.to_string(),
            oos_end: window.oos_end_date.to_string(),
            is_metrics: window_metrics_for_range(trades, window.is_start_ms, window.is_end_ms),
            oos_metrics: window_metrics_for_range(trades, window.oos_start_ms, window.oos_end_ms),
        })
        .collect()
}

/// Pick the most consistent non-thin variant: positive walk-forward windows
/// first, full-sample net R as the tie-breaker.
fn select_top_candidate(variants: &[VariantReport]) -> Option<TopCandidate> {
    variants
        .iter()
        .filter(|variant| !variant.sample_thin)
        .max_by(|left, right| {
            left.walk_forward_summary
                .positive_windows
                .cmp(&right.walk_forward_summary.positive_windows)
                .then_with(|| left.overall.net_r.total_cmp(&right.overall.net_r))
        })
        .map(|variant| TopCandidate {
            name: variant.name.clone(),
            trades: variant.trades,
            net_r: variant.overall.net_r,
            positive_windows: variant.walk_forward_summary.positive_windows,
            evaluated_windows: variant.walk_forward_summary.evaluated_windows,
            consistency_pct: variant.walk_forward_summary.consistency_pct,
            rolling_is_oos: variant.rolling_is_oos.clone(),
        })
}

fn run_variant(
    variant: Variant,
    signals: &[RawSignal],
    context: &BacktestContext<'_>,
) -> VariantReport {
    let candles = context.candles;
    let plan = variant_plan(variant);
    let mut trades = Vec::new();
    let mut reject_reasons: BTreeMap<String, usize> = BTreeMap::new();

    for raw in signals {
        let is_buy = raw.is_buy();

        if let Some(filter) = plan.htf {
            let Some(signal_candle) = candles.get(raw.index) else {
                *reject_reasons
                    .entry("missing_signal_candle".to_string())
                    .or_default() += 1;
                continue;
            };
            let snapshot = context.htf.snapshot(
                signal_candle.timestamp + context.step_ms,
                signal_candle.close,
            );
            if let Err(reason) = htf_filter_allows(filter, raw, &snapshot) {
                *reject_reasons.entry(reason.to_string()).or_default() += 1;
                continue;
            }
        }

        if let Some(filter) = plan.delta {
            if let Err(reason) =
                delta_filter_allows(filter, is_buy, candles, context.cvd, raw.index)
            {
                *reject_reasons.entry(reason.to_string()).or_default() += 1;
                continue;
            }
        }

        if let Some(gate) = plan.rsi {
            if let Err(reason) = rsi_gate_allows(gate, is_buy, candles, context.rsi, raw.index) {
                *reject_reasons.entry(reason.to_string()).or_default() += 1;
                continue;
            }
        }

        if let Some(filter) = plan.volume {
            if let Err(reason) = volume_filter_allows(filter, candles, raw.index) {
                *reject_reasons.entry(reason.to_string()).or_default() += 1;
                continue;
            }
        }

        let levels = match plan.levels {
            LevelSource::Analyzer => {
                let levels = raw.levels();
                if !levels.is_ordered(is_buy) {
                    *reject_reasons
                        .entry("invalid_levels".to_string())
                        .or_default() += 1;
                    continue;
                }
                levels
            }
            LevelSource::TpBand => match tp_band_levels(raw.entry_price, raw.stop_loss, is_buy) {
                Ok(levels) => levels,
                Err(reason) => {
                    *reject_reasons.entry(reason.to_string()).or_default() += 1;
                    continue;
                }
            },
            LevelSource::ConfirmSession => match confirm_levels(raw, candles, context.step_ms) {
                Ok(levels) => levels,
                Err(reason) => {
                    *reject_reasons.entry(reason.to_string()).or_default() += 1;
                    continue;
                }
            },
            LevelSource::ConfirmationSoft => match soft_confirm_levels(raw, candles) {
                Ok(levels) => levels,
                Err(reason) => {
                    *reject_reasons.entry(reason.to_string()).or_default() += 1;
                    continue;
                }
            },
            LevelSource::TpStructure => match structure_levels_for_signal(
                raw,
                candles,
                context.h1,
                context.h4,
                context.step_ms,
            ) {
                Ok(levels) => levels,
                Err(reason) => {
                    *reject_reasons.entry(reason.to_string()).or_default() += 1;
                    continue;
                }
            },
            LevelSource::TpStructureSoft => match soft_structure_levels_for_signal(
                raw,
                candles,
                context.h1,
                context.h4,
                context.step_ms,
            ) {
                Ok(levels) => levels,
                Err(reason) => {
                    *reject_reasons.entry(reason.to_string()).or_default() += 1;
                    continue;
                }
            },
            LevelSource::TpStructureH4 => {
                match h4_structure_levels_for_signal(raw, candles, context.h4, context.step_ms) {
                    Ok(levels) => levels,
                    Err(reason) => {
                        *reject_reasons.entry(reason.to_string()).or_default() += 1;
                        continue;
                    }
                }
            }
            LevelSource::SwingSl => match swing_sl_levels(candles, raw) {
                Ok(levels) => levels,
                Err(reason) => {
                    *reject_reasons.entry(reason.to_string()).or_default() += 1;
                    continue;
                }
            },
        };

        let entry_index = match plan.levels {
            LevelSource::ConfirmSession | LevelSource::ConfirmationSoft => raw.index + 1,
            _ => raw.index,
        };

        if entry_index + 1 >= candles.len() {
            *reject_reasons
                .entry("no_data_after_entry".to_string())
                .or_default() += 1;
            continue;
        }

        let trade = match plan.simulation {
            SimKind::Standard => {
                simulate_standard(candles, entry_index, raw, levels, false, context.step_ms)
            }
            SimKind::UsClose => {
                simulate_standard(candles, entry_index, raw, levels, true, context.step_ms)
            }
            SimKind::PartialBe => {
                simulate_partial_be(candles, entry_index, raw, levels, context.early_be)
            }
            SimKind::Be15 => {
                simulate_managed(candles, entry_index, raw, levels, Management::be(15.0))
            }
            SimKind::Be20 => {
                simulate_managed(candles, entry_index, raw, levels, Management::be(20.0))
            }
            SimKind::Scale50 => {
                simulate_managed(candles, entry_index, raw, levels, Management::scale(None))
            }
            SimKind::Scale50Be15 => simulate_managed(
                candles,
                entry_index,
                raw,
                levels,
                Management::scale(Some(15.0)),
            ),
            SimKind::TimeOutChoppy => simulate_managed(
                candles,
                entry_index,
                raw,
                levels,
                Management::time_exit(TimeExitRule::Vn02Choppy),
            ),
            SimKind::TimeOutAll => simulate_managed(
                candles,
                entry_index,
                raw,
                levels,
                Management::time_exit(TimeExitRule::Vn02All),
            ),
            SimKind::Partial25 => simulate_partial_trail(
                candles,
                entry_index,
                raw,
                levels,
                PartialTrailManagement {
                    partial: Some(PartialRule::new(0.25, 20.0)),
                    trail: None,
                },
            ),
            SimKind::Trail20 => simulate_partial_trail(
                candles,
                entry_index,
                raw,
                levels,
                PartialTrailManagement {
                    partial: None,
                    trail: Some(TrailRule::new(20.0, 10.0, 1.0)),
                },
            ),
            SimKind::Partial25Trail => simulate_partial_trail(
                candles,
                entry_index,
                raw,
                levels,
                PartialTrailManagement {
                    partial: Some(PartialRule::new(0.25, 20.0)),
                    trail: Some(TrailRule::new(20.0, 10.0, 1.0)),
                },
            ),
            SimKind::Partial10At20 => simulate_partial_trail(
                candles,
                entry_index,
                raw,
                levels,
                PartialTrailManagement {
                    partial: Some(PartialRule::new(0.10, 20.0)),
                    trail: None,
                },
            ),
            SimKind::Partial15At25 => simulate_partial_trail(
                candles,
                entry_index,
                raw,
                levels,
                PartialTrailManagement {
                    partial: Some(PartialRule::new(0.15, 25.0)),
                    trail: None,
                },
            ),
            SimKind::Trail30 => simulate_partial_trail(
                candles,
                entry_index,
                raw,
                levels,
                PartialTrailManagement {
                    partial: None,
                    trail: Some(TrailRule::new(30.0, 10.0, 1.0)),
                },
            ),
        };

        match trade {
            Some(mut trade) => {
                attach_excursions(candles, &mut trade, context.cost_usd);
                trades.push(trade);
            }
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
            .filter(|trade| trade.entry_timestamp < context.split_timestamp),
    );
    let out_of_sample = compute_metrics(
        trades
            .iter()
            .filter(|trade| trade.entry_timestamp >= context.split_timestamp),
    );

    let mut trades_by_year: BTreeMap<String, Vec<&SimulatedTrade>> = BTreeMap::new();
    for trade in &trades {
        trades_by_year
            .entry(year_key(trade.entry_timestamp))
            .or_default()
            .push(trade);
    }
    let yearly = trades_by_year
        .into_iter()
        .map(|(year, year_trades)| (year, compute_metrics(year_trades)))
        .collect();

    let rejected = reject_reasons.values().sum();
    let walk_forward = build_walk_forward(&trades, context.walk_forward);
    let walk_forward_summary = summarize_walk_forward(&walk_forward);
    let rolling_is_oos = build_rolling_is_oos(&trades, context.rolling);
    let sample_thin = trades.len() < 30;

    VariantReport {
        name: variant.name().to_string(),
        signals: signals.len(),
        trades: trades.len(),
        rejected,
        reject_reasons,
        sample_thin,
        overall,
        in_sample,
        out_of_sample,
        yearly,
        walk_forward,
        walk_forward_summary,
        rolling_is_oos,
        trade_log: trades.iter().map(TradeRecord::from).collect(),
    }
}

fn format_ts(timestamp_ms: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(timestamp_ms)
        .map(|datetime| datetime.to_rfc3339_opts(SecondsFormat::Secs, true))
        .unwrap_or_else(|| timestamp_ms.to_string())
}

fn format_profit_factor(value: Option<f64>) -> String {
    value
        .map(|pf| format!("{pf:.2}"))
        .unwrap_or_else(|| "inf".to_string())
}

fn print_row(name: &str, split: &str, metrics: &Metrics) {
    println!(
        "{name:<24} {split:<4} {:>6} {:>4} {:>4} {:>4} {:>3} {:>4} {:>5} {:>5.1} {:>6.3} {:>8.2} {:>5} {:>6.2} {:>6.3} {:>8.2} {:>5} {:>6.2} {:>6.1} {:>5.1} {:>5.1}",
        metrics.trades,
        metrics.tp1,
        metrics.tp2,
        metrics.losses,
        metrics.be,
        metrics.time_exits,
        metrics.trail,
        metrics.win_rate_pct,
        metrics.avg_r,
        metrics.gross_r,
        format_profit_factor(metrics.profit_factor),
        metrics.max_drawdown_r,
        metrics.avg_net_r,
        metrics.net_r,
        format_profit_factor(metrics.profit_factor_after_costs),
        metrics.max_drawdown_r_after_costs,
        metrics.avg_duration_candles,
        metrics.avg_mfe_usd,
        metrics.avg_mae_usd,
    );
}

fn print_table(report: &Report) {
    println!();
    println!("Gross metrics are before costs; net metrics subtract cost_usd / risk per trade.");
    println!(
        "{:<24} {:<4} {:>6} {:>4} {:>4} {:>4} {:>3} {:>4} {:>5} {:>5} {:>6} {:>8} {:>5} {:>6} {:>6} {:>8} {:>5} {:>6} {:>6} {:>5} {:>5}",
        "variant",
        "split",
        "trades",
        "TP1",
        "TP2",
        "SL",
        "BE",
        "TIME",
        "TRAIL",
        "win%",
        "avgR",
        "grossR",
        "PF",
        "maxDD",
        "avgNet",
        "netR",
        "PFn",
        "maxDDn",
        "dur",
        "MFE$",
        "MAE$"
    );

    for variant in &report.variants {
        let display_name = if variant.sample_thin {
            format!("{}*", variant.name)
        } else {
            variant.name.clone()
        };
        print_row(&display_name, "ALL", &variant.overall);
        print_row("", "IS", &variant.in_sample);
        print_row("", "OOS", &variant.out_of_sample);
    }
    if report.variants.iter().any(|variant| variant.sample_thin) {
        println!();
        println!("* sample-thin: fewer than 30 trades.");
    }
}

fn print_mfe_table(report: &Report) {
    println!();
    println!(
        "MFE sweep (ALL trades): % of trades reaching each favorable excursion and how they ended."
    );
    println!(
        "{:<24} {:>5} {:>7} {:>8} {:>7} {:>7} {:>6} {:>8} {:>8} {:>8}",
        "variant", "+USD", "reached", "%trades", "TP1/2", "SL", "BE", "TIME", "TRAIL", "reversed"
    );

    for variant in &report.variants {
        for bucket in &variant.overall.mfe_buckets {
            let ended_tp = bucket.ended_tp1 + bucket.ended_tp2;
            println!(
                "{:<24} {:>5.0} {:>7} {:>7.1}% {:>7} {:>7} {:>6} {:>8} {:>8} {:>8}",
                variant.name,
                bucket.threshold_usd,
                bucket.reached,
                bucket.pct_of_trades,
                ended_tp,
                bucket.ended_sl,
                bucket.ended_be,
                bucket.ended_time_exit,
                bucket.ended_trail,
                bucket.reversed,
            );
        }
    }
}

fn print_yearly_table(report: &Report) {
    println!();
    println!("Yearly net-of-cost performance (entry year in UTC+7):");
    println!(
        "{:<24} {:>4} {:>6} {:>4} {:>4} {:>4} {:>3} {:>4} {:>5} {:>5} {:>8} {:>8} {:>5} {:>6} {:>6}",
        "variant",
        "year",
        "trades",
        "TP1",
        "TP2",
        "SL",
        "BE",
        "TIME",
        "TRAIL",
        "win%",
        "netR$",
        "avgNet",
        "PFn",
        "maxDDn",
        "dur"
    );

    for variant in &report.variants {
        for (year, metrics) in &variant.yearly {
            println!(
                "{:<24} {:>4} {:>6} {:>4} {:>4} {:>4} {:>3} {:>4} {:>5} {:>5.1} {:>8.2} {:>8.3} {:>5} {:>6.2} {:>6.1}",
                variant.name,
                year,
                metrics.trades,
                metrics.tp1,
                metrics.tp2,
                metrics.losses,
                metrics.be,
                metrics.time_exits,
                metrics.trail,
                metrics.win_rate_pct,
                metrics.net_r_after_costs,
                metrics.avg_net_r,
                format_profit_factor(metrics.profit_factor_after_costs),
                metrics.max_drawdown_r_after_costs,
                metrics.avg_duration_candles,
            );
        }
    }
}

fn print_walk_forward_table(report: &Report) {
    println!();
    println!("Walk-forward stability (fixed 12-month windows, net of costs; thin = <30 trades):");
    println!(
        "{:<24} {:<3} {:<10} {:<10} {:>6} {:>8} {:>6} {:>8} {:>8} {:>5}",
        "variant", "win", "start", "end", "trades", "netR", "PFn", "maxDDn", "expR", "thin"
    );

    for variant in &report.variants {
        for window in &variant.walk_forward {
            let metrics = &window.metrics;
            println!(
                "{:<24} {:<3} {:<10} {:<10} {:>6} {:>8.2} {:>6} {:>8.2} {:>8.3} {:>5}",
                variant.name,
                window.label,
                window.start,
                window.end,
                metrics.trades,
                metrics.net_r,
                format_profit_factor(metrics.profit_factor),
                metrics.max_drawdown_r,
                metrics.expectancy_r,
                if metrics.sample_thin { "yes" } else { "" },
            );
        }
        let summary = &variant.walk_forward_summary;
        println!(
            "  {}: {}/{} positive windows ({:.1}% consistency), worst {:.2}R, best {:.2}R",
            variant.name,
            summary.positive_windows,
            summary.evaluated_windows,
            summary.consistency_pct,
            summary.worst_net_r,
            summary.best_net_r,
        );
    }
    println!();
    println!("Sample-thin variants are flagged but still reported; no parameters are trained on these windows.");
}

fn print_top_candidate(report: &Report) {
    let Some(candidate) = &report.top_candidate else {
        println!();
        println!("Top candidate: none (no variant reached 30 trades).");
        return;
    };

    println!();
    println!(
        "Top candidate (consistency-first): {} | {} trades | netR {:.2} | {}/{} positive windows ({:.1}%)",
        candidate.name,
        candidate.trades,
        candidate.net_r,
        candidate.positive_windows,
        candidate.evaluated_windows,
        candidate.consistency_pct
    );

    if candidate.rolling_is_oos.is_empty() {
        println!("Rolling 3y IS / 1y OOS: no complete window in the requested range.");
        return;
    }

    println!();
    println!("Rolling 3y IS / 1y OOS for the top candidate (net of costs):");
    println!(
        "{:<3} {:<24} {:>6} {:>8} {:>6} {:>6} {:>8} {:>6} {:>8} {:>5}",
        "win",
        "IS period",
        "IS trd",
        "IS netR",
        "IS PF",
        "OOS trd",
        "OOS netR",
        "OOS PF",
        "OOS maxDD",
        "OOS+"
    );

    for window in &candidate.rolling_is_oos {
        let is_metrics = &window.is_metrics;
        let oos_metrics = &window.oos_metrics;
        println!(
            "{:<3} {:<24} {:>6} {:>8.2} {:>6} {:>6} {:>8.2} {:>6} {:>8.2} {:>5}",
            window.label,
            format!("{}..{}", window.is_start, window.is_end),
            is_metrics.trades,
            is_metrics.net_r,
            format_profit_factor(is_metrics.profit_factor),
            oos_metrics.trades,
            oos_metrics.net_r,
            format_profit_factor(oos_metrics.profit_factor),
            oos_metrics.max_drawdown_r,
            if oos_metrics.trades > 0 && oos_metrics.net_r > 0.0 {
                "yes"
            } else {
                "no"
            },
        );
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
    let h1_candles = aggregate_candles(&candles, timeframe_millis(Timeframe::H1), Timeframe::H1);
    let h4_candles = aggregate_candles(&candles, timeframe_millis(Timeframe::H4), Timeframe::H4);
    let htf_context = HtfContext::new(&candles);
    let cvd = cumulative_delta(&candles);
    let rsi14 = rsi_series(&candles, RSI_PERIOD);
    let walk_forward_defs = walk_forward_windows(args.start, args.end);
    let rolling_defs = rolling_is_oos_windows(args.start, args.end);

    println!(
        "Data: {} candles, {} .. {}, {} gap(s), ~{} missing candle(s), source={}",
        candles.len(),
        format_ts(actual_start_ms),
        format_ts(actual_end_ms),
        gaps,
        missing_candles,
        data_source
    );
    println!(
        "Higher timeframes: {} H1 candles, {} H4 candles (UTC bucket starts)",
        h1_candles.len(),
        h4_candles.len()
    );
    println!(
        "Phase 1B context: {} D1, {} H4, {} H1 candles (EMA50/EMA200/ATR14 precomputed)",
        htf_context.d1.candles.len(),
        htf_context.h4.candles.len(),
        htf_context.h1.candles.len()
    );
    println!(
        "Walk-forward: {} yearly window(s), {} rolling 3y IS / 1y OOS window(s)",
        walk_forward_defs.len(),
        rolling_defs.len()
    );
    println!(
        "Phase R0 context: {} RSI14 value(s), divergence lookback {} M15 candles, pivot k={}",
        rsi14.iter().filter(|value| value.is_some()).count(),
        RSI_DIVERGENCE_LOOKBACK,
        RSI_PIVOT_K
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

    // Volume-mode variants need the analyzer's surge requirement disabled so
    // low-volume exhaustion reversals can reach the variant gate.
    let volume_config = BotConfig {
        volume_surge_multiplier: 0.0,
        ..config.clone()
    };
    let needs_volume_universe = args
        .variants
        .iter()
        .any(|variant| variant_plan(*variant).volume.is_some());
    let volume_signals = if needs_volume_universe {
        let signals = generate_signals(&candles, &volume_config);
        println!(
            "Volume-mode signals: {} (analyzer surge disabled; variant volume gate applied after)",
            signals.len()
        );
        signals
    } else {
        Vec::new()
    };

    let context = BacktestContext {
        candles: &candles,
        h1: &h1_candles,
        h4: &h4_candles,
        htf: &htf_context,
        cvd: &cvd,
        rsi: &rsi14,
        split_timestamp,
        early_be: args.early_be,
        cost_usd: args.cost_usd,
        step_ms,
        walk_forward: &walk_forward_defs,
        rolling: &rolling_defs,
    };

    let mut variants = Vec::new();
    for variant in &args.variants {
        let plan = variant_plan(*variant);
        let variant_signals = if plan.volume.is_some() {
            &volume_signals
        } else {
            &signals
        };
        let report = run_variant(*variant, variant_signals, &context);
        println!(
            "Variant {}: signals={}, trades={}, rejected={} {:?}",
            report.name, report.signals, report.trades, report.rejected, report.reject_reasons
        );
        if report.sample_thin {
            println!(
                "  note: {} has {} trade(s) (<30) and is flagged sample-thin.",
                report.name, report.trades
            );
        }
        variants.push(report);
    }
    let top_candidate = select_top_candidate(&variants);
    if let Some(candidate) = &top_candidate {
        println!(
            "Top candidate (consistency-first): {} ({} trades, netR {:.2}, {}/{} positive windows)",
            candidate.name,
            candidate.trades,
            candidate.net_r,
            candidate.positive_windows,
            candidate.evaluated_windows
        );
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
        "time_out_02am: on a later Vietnam calendar date than entry, from 02:00 VN onward, close at candle close only when choppy = (last 3 M15 ranges each < 0.5 x ATR14) OR (close within +/-5 USD of entry AND last 3 volumes strictly declining).".to_string(),
        "time_out_02am_all: on a later Vietnam calendar date than entry, close at candle close from 02:00 VN onward regardless of chop.".to_string(),
        "be_15/be_20: no partial; once MFE reaches 15/20 USD the SL moves to entry on the next M15 candle. BE before TP1 = 0R; TP1 already banked stays TP1.".to_string(),
        "scale_50_tp1: 50% at TP1, remaining 50% keeps the original SL; TP1 then SL = 0.5 x TP1R - 0.5R. scale_50_tp1_be_15 adds an entry stop once MFE >= 15 USD; TP1 then BE = 0.5 x TP1R.".to_string(),
        "confirmation_soft: next M15 close must be inside the signal candle [low, high]; entry at that close, original SL/TP, no US-close time exit and no pre-US-open blackout.".to_string(),
        "tp_structure: H1/H4 are aggregated from M15 once at UTC hour/4-hour boundaries. Only candles fully closed at signal close are used; H1 zones use last 100 H1 bars, lookback 50, tolerance 5.0; H4 zones use tolerance 10.0.".to_string(),
        "tp_structure guardrails: TP1 is the nearest H1 level beyond entry, TP2 the nearest H4 level beyond TP1. Reject if 2 x risk > 40 or TP1 distance < 20; cap TP1/TP2 distance at 40. No H1 level falls back to the 2R/3R TP band; missing H4 beyond TP1 falls back to a banded 3R TP2 (>= TP1 + 5, capped 40).".to_string(),
        "Phase 1B context: D1/H4/H1 are aggregated once from M15 at UTC date/4-hour/hour boundaries. A signal only sees candles fully closed at the M15 signal close (timestamp + timeframe <= signal close).".to_string(),
        "Phase 1B bias: EMA50/EMA200 are incremental (SMA seed over the first 50/200 closes); BULLISH = close > EMA50 > EMA200, BEARISH = close < EMA50 < EMA200, else NEUTRAL. H1/H4 Donchian(20) position = (close - low) / (high - low), clamped 0..1, flat channel = 0.5.".to_string(),
        "Phase 1B H4 proximity: pivot highs/lows (2 candles each side) in the last 50 closed H4 candles; near support/resistance when price is within 0.5 x ATR14(H4) of a swing low/high.".to_string(),
        "htf_h4 blocks BUY only when H4 bias is BEARISH and SELL only when H4 bias is BULLISH; NEUTRAL allows both. htf_h1_channel allows BUY only at Donchian position <= 0.30 and SELL only at >= 0.70; a missing channel rejects with htf_h1_channel_missing.".to_string(),
        "htf_combined = H4 not opposing + H1 channel extreme. If D1 bias opposes, a BUY is allowed only near H4 support and a SELL only near H4 resistance; otherwise htf_d1_countertrend_no_zone. htf_combined_tp_structure adds tp_structure_soft levels to htf_combined.".to_string(),
        "swing_sl: 20 closed M15 candles ending at the signal candle; BUY SL = min(low) - 2.5, SELL SL = max(high) + 2.5 (round 2). If the stop is on the wrong side of entry or risk < 0.5 x ATR14(M15), widen to 0.8 x ATR14 from entry. Reject risk > 30 (swing_risk_gt_30) and 2 x risk > 40 (risk_gt_20); analyzer TP1/TP2 are kept and R is recomputed from the new SL.".to_string(),
        "tp_structure_soft: TP1 = nearest H1 level beyond entry; if closer than 20 USD (or absent) fall back to 2R. TP2 = nearest H4 level beyond TP1, else 3R; cap 40 and force >= TP1 + 5. Because TP2 must also fit under 40, TP1 is clamped to 35 when a candidate would otherwise exceed 35. Reject only zero risk or 2R > 40; fallback and rejection counts are reported.".to_string(),
        "tp_structure_h4: TP1 = nearest H4 level beyond entry with the same <20 -> 2R fallback; TP2 = next H4 level beyond TP1, else 3R, capped 40 and >= TP1 + 5. TP1 is similarly clamped to 35 so TP2 fits under 40. Reject only zero risk or 2R > 40.".to_string(),
        "partial_25_at_20: 25% is booked at +20 USD only when +20 is reached before TP1; the remaining 75% keeps the original SL/TP. If +20 is never reached the variant behaves like baseline. Cost is still charged once against full-trade R.".to_string(),
        "trail_after_20: once MFE >= 20 USD the stop is trailed 10 USD behind the high-water/low-water mark, quantized to 1 USD steps (floor high / ceil low), and never worse than the initial stop. Trail updates on closed candles and binds from the next M15 candle; after TP1 a stop-out settles at TP1 unless the trail has ratcheted beyond it. TRAIL is reported when the trailing stop is the binding exit.".to_string(),
        "partial_25_trail combines both rules; htf_combined_partial_25_trail adds the htf_combined filter. TRAIL outcomes are counted as wins because trailing only activates after +20 USD and never exits worse than the initial stop.".to_string(),
        "partial_10_at_20: 10% is booked at +20 USD only when +20 is reached before TP1; the remaining 90% keeps the original SL/TP. partial_15_at_25 does the same with 15% at +25 USD. If the trigger is never reached the variant behaves like baseline.".to_string(),
        "trail_after_30: once MFE >= 30 USD the stop is trailed 10 USD behind the high-water/low-water mark, quantized to 1 USD steps (floor high / ceil low), and never worse than the initial stop. Trail updates on closed candles and binds from the next M15 candle.".to_string(),
        "htf_h4_partial_10 / htf_h4_partial_15 / htf_h4_trail_30 apply the htf_h4 filter to those management rules. htf_h4_tp_structure_soft combines the htf_h4 filter with tp_structure_soft levels.".to_string(),
        "htf_h4_zone adds H4 Donchian(20) location to htf_h4: BUY only when the last closed H4 channel position is <= 0.40, SELL only when >= 0.60; a missing channel rejects with htf_h4_zone_missing.".to_string(),
        "htf_h4_d1 adds a non-opposing D1 bias to htf_h4: BUY is blocked when D1 is BEARISH and SELL is blocked when D1 is BULLISH; NEUTRAL allows both.".to_string(),
        "Walk-forward W1..W5 are fixed 12-month windows starting at --start. They are a stability check, not parameter training; every window reports trades, net R, PF, maxDD, expectancy and a sample-thin flag (<30 trades).".to_string(),
        "Rolling analysis: 3-year in-sample followed by the next 1-year out-of-sample, stepped annually. The top candidate is selected consistency-first (most positive walk-forward windows, then full-sample net R) among variants with at least 30 trades.".to_string(),
        "Cost model: each trade subtracts cost_usd / risk from gross R; gross and net metrics are reported separately.".to_string(),
        "R fields: gross_r/avg_r/PF/maxDD are pre-cost; net_r/avg_net_r/PF_after_costs/maxDD_after_costs are after cost. net_r_after_costs is kept as an explicit alias of net_r.".to_string(),
        "MFE/MAE are measured in USD from entry through the exit candle; MFE before TP1 includes the first TP1-touch candle and after-TP1 starts on the next candle.".to_string(),
        "Open trades remaining at the end of the dataset are closed at the final candle close with TIME_EXIT.".to_string(),
        "In-sample = first 70% of data by entry time; out-of-sample = final 30%.".to_string(),
        "Phase 1D delta: delta = 2 x taker_buy_volume - volume; delta_ratio = delta / volume (0 when volume is 0). Positive delta means aggressive buyers.".to_string(),
        "delta_align: BUY requires signal-candle delta_ratio >= +0.10, SELL requires <= -0.10; otherwise the signal is rejected as delta_align.".to_string(),
        "delta_divergence: over the last 20 M15 candles, BUY requires price change < 0 and CVD change > 0 (bullish absorption); SELL requires price change > 0 and CVD change < 0 (bearish exhaustion). CVD is cumulative delta.".to_string(),
        "delta_both = delta_align OR delta_divergence. When neither leg passes, the rejection is counted once as delta_align.".to_string(),
        "htf_h4_delta_align / htf_h4_d1_delta_align / htf_h4_d1_delta_both combine the Phase 1C HTF gates with delta. HTF is evaluated before delta, so rejection counts reflect the first failing gate.".to_string(),
        "Phase R0 RSI: Wilder RSI14 is precomputed once for all M15 candles; the first value appears after 14 closed candles. Zero average loss = 100, zero average gain = 0, flat = 50.".to_string(),
        "rsi_extreme: baseline signals plus BUY only when RSI14 <= 30 and SELL only when RSI14 >= 70. rsi_extreme_65_35 uses 35/65 instead. A missing RSI value rejects as rsi_extreme.".to_string(),
        "rsi_divergence: confirmed pivots use 2 candles on each side inside the last 60 closed M15 candles; the last two pivots must be >= 3 candles apart. Bearish = price higher high + RSI lower high; bullish = price lower low + RSI higher low. BUY requires bullish and SELL requires bearish divergence, else rsi_divergence.".to_string(),
        "rsi_both = RSI extreme 70/30 OR matching divergence. When neither leg passes the rejection is counted once as rsi_extreme. htf_h4_d1_rsi_extreme / htf_h4_d1_rsi_both add the htf_h4_d1 gate, evaluated before RSI.".to_string(),
        "Volume mode variants regenerate signals with analyzer volume_surge_multiplier=0.0 so the analyzer does not require a surge; the variant gate is then applied. MA20(volume) uses the 20 closed candles before the signal candle.".to_string(),
        "volume_exhaustion: current volume <= 0.7 x MA20(volume,20) AND current range <= 0.8 x ATR14(M15); otherwise volume_exhaustion. volume_either: surge >= 1.5 x MA20 OR that exhaustion condition; otherwise volume_either.".to_string(),
        "rsi_divergence_volume_exhaustion = rsi_divergence AND volume_exhaustion. htf_h4_d1_rsi_volume_either = htf_h4_d1 AND rsi_both(70/30) AND volume_either. Gate order is HTF, RSI, volume, delta, so rejection counts reflect the first failing gate.".to_string(),
    ];
    if args.early_be > 0.0 {
        notes.push(format!(
            "partial_be used early break-even at +{} points before TP1.",
            args.early_be
        ));
    }
    if args.cost_usd > 0.0 {
        notes.push(format!(
            "Round-turn cost {:.4} USD/oz subtracted from every trade as cost_usd / risk R.",
            args.cost_usd
        ));
    }
    if args.walk_forward {
        notes.push(
            "--walk-forward supplied; yearly windows and rolling IS/OOS are always included."
                .to_string(),
        );
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
            cost_usd_per_trade: args.cost_usd,
            news_filter: "disabled (no historical calendar)".to_string(),
            same_candle_policy: "SL wins over TP on the same candle".to_string(),
        },
        notes,
        top_candidate,
        variants,
    };

    write_report(&args.out_path, &report)?;
    print_table(&report);
    print_mfe_table(&report);
    print_yearly_table(&report);
    print_walk_forward_table(&report);
    print_top_candidate(&report);
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
                entry_index: 0,
                exit_index: 1,
                signal_type: SignalType::Buy,
                entry_price: 100.0,
                stop_loss: 95.0,
                take_profit_1: 110.0,
                take_profit_2: 115.0,
                exit_price: 110.0,
                outcome: Outcome::Tp1,
                r_multiple: 2.0,
                cost_usd: 0.0,
                mfe_usd: 0.0,
                mae_usd: 0.0,
                mfe_before_tp1_usd: 0.0,
                mfe_after_tp1_usd: 0.0,
                duration_candles: 2,
                risk_points: 5.0,
            },
            SimulatedTrade {
                signal_timestamp: 3,
                entry_timestamp: 3,
                exit_timestamp: 4,
                entry_index: 0,
                exit_index: 1,
                signal_type: SignalType::Buy,
                entry_price: 100.0,
                stop_loss: 95.0,
                take_profit_1: 110.0,
                take_profit_2: 115.0,
                exit_price: 95.0,
                outcome: Outcome::Sl,
                r_multiple: -1.0,
                cost_usd: 0.0,
                mfe_usd: 0.0,
                mae_usd: 0.0,
                mfe_before_tp1_usd: 0.0,
                mfe_after_tp1_usd: 0.0,
                duration_candles: 2,
                risk_points: 5.0,
            },
            SimulatedTrade {
                signal_timestamp: 5,
                entry_timestamp: 5,
                exit_timestamp: 6,
                entry_index: 0,
                exit_index: 1,
                signal_type: SignalType::Buy,
                entry_price: 100.0,
                stop_loss: 95.0,
                take_profit_1: 110.0,
                take_profit_2: 115.0,
                exit_price: 115.0,
                outcome: Outcome::Tp2,
                r_multiple: 3.0,
                cost_usd: 0.0,
                mfe_usd: 0.0,
                mae_usd: 0.0,
                mfe_before_tp1_usd: 0.0,
                mfe_after_tp1_usd: 0.0,
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
    fn aggregate_candles_builds_h1_and_h4_ohlcv() {
        let start = utc_ms(2024, 1, 10, 0, 0);
        let candles: Vec<Candle> = (0..8)
            .map(|i| {
                let open = 100.0 + i as f64;
                candle(
                    start + i * STEP_MS,
                    open,
                    open + 2.0,
                    open - 1.0,
                    open + 1.0,
                )
            })
            .collect();

        let h1 = aggregate_candles(&candles, timeframe_millis(Timeframe::H1), Timeframe::H1);
        assert_eq!(h1.len(), 2);
        assert_eq!(h1[0].timestamp, start);
        assert_eq!(h1[0].timeframe, Timeframe::H1);
        assert!((h1[0].open - 100.0).abs() < 1e-9);
        assert!((h1[0].high - 105.0).abs() < 1e-9);
        assert!((h1[0].low - 99.0).abs() < 1e-9);
        assert!((h1[0].close - 104.0).abs() < 1e-9);
        assert!((h1[0].volume - 400.0).abs() < 1e-9);

        assert_eq!(h1[1].timestamp, start + 60 * 60 * 1000);
        assert!((h1[1].open - 104.0).abs() < 1e-9);
        assert!((h1[1].high - 109.0).abs() < 1e-9);
        assert!((h1[1].low - 103.0).abs() < 1e-9);
        assert!((h1[1].close - 108.0).abs() < 1e-9);

        let h4 = aggregate_candles(&candles, timeframe_millis(Timeframe::H4), Timeframe::H4);
        assert_eq!(h4.len(), 1);
        assert_eq!(h4[0].timestamp, start);
        assert_eq!(h4[0].timeframe, Timeframe::H4);
        assert!((h4[0].open - 100.0).abs() < 1e-9);
        assert!((h4[0].high - 109.0).abs() < 1e-9);
        assert!((h4[0].low - 99.0).abs() < 1e-9);
        assert!((h4[0].close - 108.0).abs() < 1e-9);
        assert!((h4[0].volume - 800.0).abs() < 1e-9);
    }

    #[test]
    fn last_closed_index_never_uses_an_in_progress_candle() {
        let h1 = vec![
            candle(utc_ms(2024, 1, 10, 8, 0), 100.0, 101.0, 99.0, 100.0),
            candle(utc_ms(2024, 1, 10, 9, 0), 100.0, 101.0, 99.0, 100.0),
            candle(utc_ms(2024, 1, 10, 10, 0), 100.0, 101.0, 99.0, 100.0),
        ];
        assert_eq!(
            last_closed_index(&h1, Timeframe::H1, utc_ms(2024, 1, 10, 10, 45)),
            Some(1)
        );
        assert_eq!(
            last_closed_index(&h1, Timeframe::H1, utc_ms(2024, 1, 10, 11, 0)),
            Some(2)
        );

        let h4 = vec![
            candle(utc_ms(2024, 1, 10, 4, 0), 100.0, 101.0, 99.0, 100.0),
            candle(utc_ms(2024, 1, 10, 8, 0), 100.0, 101.0, 99.0, 100.0),
            candle(utc_ms(2024, 1, 10, 12, 0), 100.0, 101.0, 99.0, 100.0),
        ];
        assert_eq!(
            last_closed_index(&h4, Timeframe::H4, utc_ms(2024, 1, 10, 15, 45)),
            Some(1)
        );
        assert_eq!(
            last_closed_index(&h4, Timeframe::H4, utc_ms(2024, 1, 10, 16, 0)),
            Some(2)
        );
    }

    fn flat_candles(start: i64, count: usize, range: f64) -> Vec<Candle> {
        (0..count)
            .map(|i| {
                candle(
                    start + i as i64 * STEP_MS,
                    100.0,
                    100.0 + range,
                    100.0 - range,
                    100.0,
                )
            })
            .collect()
    }

    #[test]
    fn time_out_02am_all_closes_on_first_candle_after_two_vn() {
        // 15:00 UTC = 22:00 VN on Jan 10; 19:00 UTC = 02:00 VN on Jan 11.
        let start = utc_ms(2024, 1, 10, 15, 0);
        let candles = flat_candles(start, 20, 0.05);
        let raw = raw_signal(0, SignalType::Buy, 100.0, 95.0, 110.0, 115.0);
        let trade = simulate_managed(
            &candles,
            0,
            &raw,
            raw.levels(),
            Management::time_exit(TimeExitRule::Vn02All),
        )
        .expect("trade");

        assert_eq!(trade.outcome, Outcome::TimeExit);
        assert_eq!(trade.exit_timestamp, utc_ms(2024, 1, 10, 19, 0));
        assert!((trade.r_multiple - 0.0).abs() < 1e-9);
    }

    #[test]
    fn time_out_02am_only_closes_when_chop_condition_holds() {
        let start = utc_ms(2024, 1, 10, 15, 0);
        let mut choppy = flat_candles(start, 20, 0.05);
        // Make the earlier candles wide so the last three tiny ranges are
        // clearly below half of ATR14 at the 02:00 VN candle.
        for candle in choppy.iter_mut().take(14).skip(1) {
            candle.high = 101.0;
            candle.low = 99.0;
        }
        let raw = raw_signal(0, SignalType::Buy, 100.0, 95.0, 110.0, 115.0);
        let trade = simulate_managed(
            &choppy,
            0,
            &raw,
            raw.levels(),
            Management::time_exit(TimeExitRule::Vn02Choppy),
        )
        .expect("trade");
        assert_eq!(trade.exit_timestamp, utc_ms(2024, 1, 10, 19, 0));

        // Wide, constant-range candles are not choppy; the trade runs to the
        // end of the dataset instead of exiting at 02:00 VN.
        let volatile = flat_candles(start, 20, 1.0);
        let trade = simulate_managed(
            &volatile,
            0,
            &raw,
            raw.levels(),
            Management::time_exit(TimeExitRule::Vn02Choppy),
        )
        .expect("trade");
        assert_eq!(trade.exit_timestamp, candles_last_timestamp(&volatile));
        assert_ne!(trade.exit_timestamp, utc_ms(2024, 1, 10, 19, 0));
    }

    fn candles_last_timestamp(candles: &[Candle]) -> i64 {
        candles.last().expect("candles").timestamp
    }

    #[test]
    fn be_15_moves_stop_to_entry_after_mfe_trigger() {
        let candles = vec![
            candle(0, 100.0, 101.0, 99.0, 100.0),
            candle(STEP_MS, 100.0, 116.0, 99.0, 110.0),
            candle(2 * STEP_MS, 110.0, 111.0, 99.0, 99.0),
        ];
        let raw = raw_signal(0, SignalType::Buy, 100.0, 95.0, 120.0, 125.0);
        let trade =
            simulate_managed(&candles, 0, &raw, raw.levels(), Management::be(15.0)).expect("trade");

        assert_eq!(trade.outcome, Outcome::Be);
        assert!((trade.r_multiple - 0.0).abs() < 1e-9);
    }

    #[test]
    fn be_15_does_not_trigger_below_threshold() {
        let candles = vec![
            candle(0, 100.0, 101.0, 99.0, 100.0),
            candle(STEP_MS, 100.0, 114.0, 99.0, 110.0),
            candle(2 * STEP_MS, 110.0, 111.0, 94.0, 95.0),
        ];
        let raw = raw_signal(0, SignalType::Buy, 100.0, 95.0, 120.0, 125.0);
        let trade =
            simulate_managed(&candles, 0, &raw, raw.levels(), Management::be(15.0)).expect("trade");

        assert_eq!(trade.outcome, Outcome::Sl);
        assert!((trade.r_multiple + 1.0).abs() < 1e-9);
    }

    #[test]
    fn be_15_keeps_tp1_banked_when_entry_stop_is_hit_later() {
        let candles = vec![
            candle(0, 100.0, 101.0, 99.0, 100.0),
            candle(STEP_MS, 100.0, 121.0, 99.0, 120.0),
            candle(2 * STEP_MS, 120.0, 121.0, 99.0, 100.0),
        ];
        let raw = raw_signal(0, SignalType::Buy, 100.0, 95.0, 120.0, 125.0);
        let trade =
            simulate_managed(&candles, 0, &raw, raw.levels(), Management::be(15.0)).expect("trade");

        assert_eq!(trade.outcome, Outcome::Tp1);
        assert!((trade.r_multiple - 4.0).abs() < 1e-9);
    }

    #[test]
    fn scale_50_tp1_uses_original_stop_for_remainder() {
        let candles = vec![
            candle(0, 100.0, 101.0, 99.0, 100.0),
            candle(STEP_MS, 100.0, 111.0, 99.0, 110.0),
            candle(2 * STEP_MS, 110.0, 111.0, 94.0, 95.0),
        ];
        let raw = raw_signal(0, SignalType::Buy, 100.0, 95.0, 110.0, 115.0);
        let trade = simulate_managed(&candles, 0, &raw, raw.levels(), Management::scale(None))
            .expect("trade");

        assert_eq!(trade.outcome, Outcome::Tp1);
        assert!((trade.r_multiple - 0.5).abs() < 1e-9);
    }

    #[test]
    fn scale_50_tp1_runs_remainder_to_tp2() {
        let candles = vec![
            candle(0, 100.0, 101.0, 99.0, 100.0),
            candle(STEP_MS, 100.0, 111.0, 99.0, 110.0),
            candle(2 * STEP_MS, 110.0, 116.0, 109.0, 115.0),
        ];
        let raw = raw_signal(0, SignalType::Buy, 100.0, 95.0, 110.0, 115.0);
        let trade = simulate_managed(&candles, 0, &raw, raw.levels(), Management::scale(None))
            .expect("trade");

        assert_eq!(trade.outcome, Outcome::Tp2);
        assert!((trade.r_multiple - 2.5).abs() < 1e-9);
    }

    #[test]
    fn scale_50_tp1_be_15_banks_half_after_tp1() {
        let candles = vec![
            candle(0, 100.0, 101.0, 99.0, 100.0),
            candle(STEP_MS, 100.0, 111.0, 99.0, 110.0),
            candle(2 * STEP_MS, 110.0, 117.0, 109.0, 116.0),
            candle(3 * STEP_MS, 116.0, 117.0, 99.0, 100.0),
        ];
        let raw = raw_signal(0, SignalType::Buy, 100.0, 95.0, 110.0, 125.0);
        let trade = simulate_managed(
            &candles,
            0,
            &raw,
            raw.levels(),
            Management::scale(Some(15.0)),
        )
        .expect("trade");

        assert_eq!(trade.outcome, Outcome::Tp1);
        assert!((trade.r_multiple - 1.0).abs() < 1e-9);
    }

    #[test]
    fn scale_50_tp1_be_15_before_tp1_exits_at_zero() {
        let candles = vec![
            candle(0, 100.0, 101.0, 99.0, 100.0),
            candle(STEP_MS, 100.0, 116.0, 99.0, 110.0),
            candle(2 * STEP_MS, 110.0, 111.0, 99.0, 100.0),
        ];
        let raw = raw_signal(0, SignalType::Buy, 100.0, 95.0, 120.0, 125.0);
        let trade = simulate_managed(
            &candles,
            0,
            &raw,
            raw.levels(),
            Management::scale(Some(15.0)),
        )
        .expect("trade");

        assert_eq!(trade.outcome, Outcome::Be);
        assert!((trade.r_multiple - 0.0).abs() < 1e-9);
    }

    #[test]
    fn confirmation_soft_accepts_close_inside_signal_range() {
        let candles = vec![
            candle(0, 100.0, 101.0, 99.0, 100.0),
            candle(STEP_MS, 100.0, 101.0, 99.0, 100.5),
        ];
        let raw = raw_signal(0, SignalType::Buy, 100.0, 95.0, 110.0, 115.0);
        let levels = soft_confirm_levels(&raw, &candles).expect("soft confirmation");
        assert!((levels.entry - 100.5).abs() < 1e-9);
        assert!((levels.stop_loss - 95.0).abs() < 1e-9);
        assert!((levels.take_profit_1 - 110.0).abs() < 1e-9);

        let outside = vec![
            candle(0, 100.0, 101.0, 99.0, 100.0),
            candle(STEP_MS, 100.0, 102.0, 100.5, 101.5),
        ];
        assert!(matches!(
            soft_confirm_levels(&raw, &outside),
            Err("no_confirmation")
        ));
    }

    #[test]
    fn tp_structure_applies_guardrails_and_fallbacks() {
        assert!(matches!(
            tp_structure_levels(100.0, 90.0, true, &[115.0], &[]),
            Err("tp1_lt_20")
        ));
        assert!(matches!(
            tp_structure_levels(100.0, 79.0, true, &[130.0], &[]),
            Err("risk_gt_20")
        ));

        let capped = tp_structure_levels(100.0, 90.0, true, &[130.0], &[145.0])
            .expect("capped structure levels");
        assert!((capped.take_profit_1 - 130.0).abs() < 1e-9);
        assert!((capped.take_profit_2 - 140.0).abs() < 1e-9);

        let h4_next =
            tp_structure_levels(100.0, 90.0, true, &[125.0], &[130.0]).expect("h4 next level");
        assert!((h4_next.take_profit_1 - 125.0).abs() < 1e-9);
        assert!((h4_next.take_profit_2 - 130.0).abs() < 1e-9);

        let fallback =
            tp_structure_levels(100.0, 90.0, true, &[], &[]).expect("fallback band levels");
        assert!((fallback.take_profit_1 - 120.0).abs() < 1e-9);
        assert!((fallback.take_profit_2 - 130.0).abs() < 1e-9);

        let sell = tp_structure_levels(100.0, 110.0, false, &[70.0], &[60.0])
            .expect("sell structure levels");
        assert!((sell.take_profit_1 - 70.0).abs() < 1e-9);
        assert!((sell.take_profit_2 - 60.0).abs() < 1e-9);

        assert!(matches!(
            tp_structure_levels(100.0, 90.0, true, &[145.0], &[150.0]),
            Err("tp2_le_tp1")
        ));
    }

    fn simulated_trade(
        outcome: Outcome,
        r_multiple: f64,
        risk_points: f64,
        cost_usd: f64,
        mfe_usd: f64,
    ) -> SimulatedTrade {
        SimulatedTrade {
            signal_timestamp: 1,
            entry_timestamp: 1,
            exit_timestamp: 2,
            entry_index: 0,
            exit_index: 1,
            signal_type: SignalType::Buy,
            entry_price: 100.0,
            stop_loss: 100.0 - risk_points,
            take_profit_1: 100.0 + risk_points * 2.0,
            take_profit_2: 100.0 + risk_points * 3.0,
            exit_price: 100.0,
            outcome,
            r_multiple,
            cost_usd,
            mfe_usd,
            mae_usd: 0.0,
            mfe_before_tp1_usd: 0.0,
            mfe_after_tp1_usd: 0.0,
            duration_candles: 2,
            risk_points,
        }
    }

    #[test]
    fn costs_reduce_net_r_and_profit_factor() {
        let trades = vec![
            simulated_trade(Outcome::Tp1, 2.0, 10.0, 0.4, 20.0),
            simulated_trade(Outcome::Sl, -1.0, 10.0, 0.4, 5.0),
        ];
        let metrics = compute_metrics(&trades);
        assert!((metrics.gross_r - 1.0).abs() < 1e-9);
        assert!((metrics.net_r - 0.92).abs() < 1e-9);
        assert!((metrics.net_r_after_costs - 0.92).abs() < 1e-9);
        assert!((metrics.avg_net_r - 0.46).abs() < 1e-9);
        let pf = metrics
            .profit_factor_after_costs
            .expect("net profit factor");
        assert!((pf - 1.96 / 1.04).abs() < 1e-4);
    }

    #[test]
    fn mfe_buckets_count_reached_and_reversed_trades() {
        let trades = vec![
            simulated_trade(Outcome::Sl, -1.0, 10.0, 0.0, 16.0),
            simulated_trade(Outcome::Be, 0.0, 10.0, 0.0, 21.0),
            simulated_trade(Outcome::Tp2, 3.0, 10.0, 0.0, 35.0),
            simulated_trade(Outcome::Tp1, 2.0, 10.0, 0.0, 5.0),
        ];
        let metrics = compute_metrics(&trades);

        let bucket_10 = &metrics.mfe_buckets[0];
        assert_eq!(bucket_10.threshold_usd, 10.0);
        assert_eq!(bucket_10.reached, 3);
        assert!((bucket_10.pct_of_trades - 75.0).abs() < 1e-9);
        assert_eq!(bucket_10.ended_sl, 1);
        assert_eq!(bucket_10.ended_be, 1);
        assert_eq!(bucket_10.ended_tp2, 1);
        assert_eq!(bucket_10.reversed, 2);

        let bucket_20 = &metrics.mfe_buckets[2];
        assert_eq!(bucket_20.reached, 2);
        assert_eq!(bucket_20.reversed, 1);

        let bucket_40 = &metrics.mfe_buckets[5];
        assert_eq!(bucket_40.reached, 0);
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

    // -----------------------------------------------------------------------
    // Phase 1B: multi-timeframe context
    // -----------------------------------------------------------------------

    const DAY_MS: i64 = 24 * 60 * 60 * 1000;

    fn htf_candle(timestamp: i64, close: f64) -> HtfCandle {
        HtfCandle {
            timestamp,
            high: close + 1.0,
            low: close - 1.0,
            close,
        }
    }

    #[test]
    fn aggregate_htf_builds_d1_h4_h1_buckets_from_m15() {
        let start = utc_ms(2024, 1, 10, 0, 0);
        let candles: Vec<Candle> = (0..8)
            .map(|i| {
                let open = 100.0 + i as f64;
                candle(
                    start + i * STEP_MS,
                    open,
                    open + 2.0,
                    open - 1.0,
                    open + 1.0,
                )
            })
            .collect();

        let day = aggregate_htf(&candles, HtfFrame::D1.millis());
        assert_eq!(day.len(), 1);
        assert_eq!(day[0].timestamp, start);
        assert!((day[0].high - 109.0).abs() < 1e-9);
        assert!((day[0].low - 99.0).abs() < 1e-9);
        assert!((day[0].close - 108.0).abs() < 1e-9);

        let h4 = aggregate_htf(&candles, HtfFrame::H4.millis());
        assert_eq!(h4.len(), 1);
        assert_eq!(h4[0].timestamp, start);

        let h1 = aggregate_htf(&candles, HtfFrame::H1.millis());
        assert_eq!(h1.len(), 2);
        assert_eq!(h1[1].timestamp, start + 60 * 60 * 1000);
        assert!((h1[1].close - 108.0).abs() < 1e-9);
    }

    #[test]
    fn htf_bias_uses_incremental_ema50_ema200() {
        let bullish: Vec<HtfCandle> = (0..260)
            .map(|i| htf_candle(i * DAY_MS, 100.0 + i as f64))
            .collect();
        let series = HtfSeries::from_aggregated(HtfFrame::D1, bullish);
        assert_eq!(series.bias(259), Bias::Bullish);
        assert_eq!(series.bias(100), Bias::Neutral);

        let bearish: Vec<HtfCandle> = (0..260)
            .map(|i| htf_candle(i * DAY_MS, 500.0 - i as f64))
            .collect();
        let series = HtfSeries::from_aggregated(HtfFrame::D1, bearish);
        assert_eq!(series.bias(259), Bias::Bearish);
    }

    #[test]
    fn htf_last_closed_index_excludes_in_progress_candle() {
        let start = utc_ms(2024, 1, 10, 0, 0);
        let candles = vec![
            htf_candle(start, 100.0),
            htf_candle(start + DAY_MS, 101.0),
            htf_candle(start + 2 * DAY_MS, 102.0),
        ];
        let series = HtfSeries::from_aggregated(HtfFrame::D1, candles);
        assert_eq!(series.last_closed_index(start + DAY_MS + 1), Some(0));
        assert_eq!(series.last_closed_index(start + 2 * DAY_MS), Some(1));
        assert_eq!(series.last_closed_index(start + 3 * DAY_MS), Some(2));
    }

    #[test]
    fn htf_channel_position_clamps_inside_donchian_range() {
        let start = utc_ms(2024, 1, 10, 0, 0);
        let mut candles: Vec<HtfCandle> = (0..20)
            .map(|i| htf_candle(start + i * 3_600_000, 100.0))
            .collect();
        candles[19].close = 110.0;
        candles[0].low = 90.0;
        candles[19].high = 110.0;
        let series = HtfSeries::from_aggregated(HtfFrame::H1, candles);
        assert_eq!(series.channel_position(19, 20), Some(1.0));

        let mut candles: Vec<HtfCandle> = (0..20)
            .map(|i| htf_candle(start + i * 3_600_000, 100.0))
            .collect();
        candles[19].close = 90.0;
        candles[0].low = 90.0;
        let series = HtfSeries::from_aggregated(HtfFrame::H1, candles);
        assert_eq!(series.channel_position(19, 20), Some(0.0));
    }

    fn htf_snapshot(
        d1_bias: Bias,
        h4_bias: Bias,
        h4_channel: Option<f64>,
        h1_channel: Option<f64>,
        near_h4_support: bool,
        near_h4_resistance: bool,
    ) -> HtfSnapshot {
        HtfSnapshot {
            d1_bias,
            h4_bias,
            h4_channel,
            h1_channel,
            near_h4_support,
            near_h4_resistance,
        }
    }

    #[test]
    fn htf_h4_filter_blocks_only_opposing_bias() {
        let buy = raw_signal(0, SignalType::Buy, 100.0, 95.0, 110.0, 115.0);
        let sell = raw_signal(0, SignalType::Sell, 100.0, 105.0, 90.0, 85.0);

        let bearish = htf_snapshot(
            Bias::Neutral,
            Bias::Bearish,
            Some(0.5),
            Some(0.5),
            false,
            false,
        );
        assert_eq!(
            htf_filter_allows(HtfFilter::H4, &buy, &bearish),
            Err("htf_h4_bearish")
        );
        assert_eq!(htf_filter_allows(HtfFilter::H4, &sell, &bearish), Ok(()));

        let bullish = htf_snapshot(
            Bias::Neutral,
            Bias::Bullish,
            Some(0.5),
            Some(0.5),
            false,
            false,
        );
        assert_eq!(
            htf_filter_allows(HtfFilter::H4, &sell, &bullish),
            Err("htf_h4_bullish")
        );
        assert_eq!(htf_filter_allows(HtfFilter::H4, &buy, &bullish), Ok(()));
    }

    #[test]
    fn htf_h1_channel_requires_low_buy_and_high_sell() {
        let buy = raw_signal(0, SignalType::Buy, 100.0, 95.0, 110.0, 115.0);
        let sell = raw_signal(0, SignalType::Sell, 100.0, 105.0, 90.0, 85.0);

        let low = htf_snapshot(
            Bias::Neutral,
            Bias::Neutral,
            Some(0.5),
            Some(0.25),
            false,
            false,
        );
        assert_eq!(htf_filter_allows(HtfFilter::H1Channel, &buy, &low), Ok(()));
        assert_eq!(
            htf_filter_allows(HtfFilter::H1Channel, &sell, &low),
            Err("htf_h1_channel_not_high")
        );

        let high = htf_snapshot(
            Bias::Neutral,
            Bias::Neutral,
            Some(0.5),
            Some(0.75),
            false,
            false,
        );
        assert_eq!(
            htf_filter_allows(HtfFilter::H1Channel, &buy, &high),
            Err("htf_h1_channel_not_low")
        );
        assert_eq!(
            htf_filter_allows(HtfFilter::H1Channel, &sell, &high),
            Ok(())
        );

        let missing = htf_snapshot(Bias::Neutral, Bias::Neutral, Some(0.5), None, false, false);
        assert_eq!(
            htf_filter_allows(HtfFilter::H1Channel, &buy, &missing),
            Err("htf_h1_channel_missing")
        );
    }

    #[test]
    fn htf_combined_allows_countertrend_only_near_h4_zone() {
        let buy = raw_signal(0, SignalType::Buy, 100.0, 95.0, 110.0, 115.0);
        let sell = raw_signal(0, SignalType::Sell, 100.0, 105.0, 90.0, 85.0);

        let allowed = htf_snapshot(
            Bias::Bearish,
            Bias::Neutral,
            Some(0.5),
            Some(0.2),
            true,
            false,
        );
        assert_eq!(
            htf_filter_allows(HtfFilter::Combined, &buy, &allowed),
            Ok(())
        );

        let blocked = htf_snapshot(
            Bias::Bearish,
            Bias::Neutral,
            Some(0.5),
            Some(0.2),
            false,
            false,
        );
        assert_eq!(
            htf_filter_allows(HtfFilter::Combined, &buy, &blocked),
            Err("htf_d1_countertrend_no_zone")
        );

        let sell_allowed = htf_snapshot(
            Bias::Bullish,
            Bias::Neutral,
            Some(0.5),
            Some(0.8),
            false,
            true,
        );
        assert_eq!(
            htf_filter_allows(HtfFilter::Combined, &sell, &sell_allowed),
            Ok(())
        );

        let opposing_h4 = htf_snapshot(
            Bias::Neutral,
            Bias::Bearish,
            Some(0.5),
            Some(0.2),
            true,
            false,
        );
        assert_eq!(
            htf_filter_allows(HtfFilter::Combined, &buy, &opposing_h4),
            Err("htf_h4_bearish")
        );
    }

    #[test]
    fn htf_h4_zone_requires_low_buy_and_high_sell() {
        let buy = raw_signal(0, SignalType::Buy, 100.0, 95.0, 110.0, 115.0);
        let sell = raw_signal(0, SignalType::Sell, 100.0, 105.0, 90.0, 85.0);

        let low = htf_snapshot(
            Bias::Neutral,
            Bias::Neutral,
            Some(0.40),
            Some(0.5),
            false,
            false,
        );
        assert_eq!(htf_filter_allows(HtfFilter::H4Zone, &buy, &low), Ok(()));
        assert_eq!(
            htf_filter_allows(HtfFilter::H4Zone, &sell, &low),
            Err("htf_h4_zone_not_high")
        );

        let high = htf_snapshot(
            Bias::Neutral,
            Bias::Neutral,
            Some(0.60),
            Some(0.5),
            false,
            false,
        );
        assert_eq!(
            htf_filter_allows(HtfFilter::H4Zone, &buy, &high),
            Err("htf_h4_zone_not_low")
        );
        assert_eq!(htf_filter_allows(HtfFilter::H4Zone, &sell, &high), Ok(()));

        let missing = htf_snapshot(Bias::Neutral, Bias::Neutral, None, Some(0.5), false, false);
        assert_eq!(
            htf_filter_allows(HtfFilter::H4Zone, &buy, &missing),
            Err("htf_h4_zone_missing")
        );

        let opposing_h4 = htf_snapshot(
            Bias::Neutral,
            Bias::Bearish,
            Some(0.2),
            Some(0.5),
            false,
            false,
        );
        assert_eq!(
            htf_filter_allows(HtfFilter::H4Zone, &buy, &opposing_h4),
            Err("htf_h4_bearish")
        );
    }

    #[test]
    fn htf_h4_d1_blocks_only_opposing_daily_bias() {
        let buy = raw_signal(0, SignalType::Buy, 100.0, 95.0, 110.0, 115.0);
        let sell = raw_signal(0, SignalType::Sell, 100.0, 105.0, 90.0, 85.0);

        let d1_bearish = htf_snapshot(
            Bias::Bearish,
            Bias::Neutral,
            Some(0.5),
            Some(0.5),
            false,
            false,
        );
        assert_eq!(
            htf_filter_allows(HtfFilter::H4D1, &buy, &d1_bearish),
            Err("htf_d1_bearish")
        );
        assert_eq!(
            htf_filter_allows(HtfFilter::H4D1, &sell, &d1_bearish),
            Ok(())
        );

        let d1_bullish = htf_snapshot(
            Bias::Bullish,
            Bias::Neutral,
            Some(0.5),
            Some(0.5),
            false,
            false,
        );
        assert_eq!(
            htf_filter_allows(HtfFilter::H4D1, &sell, &d1_bullish),
            Err("htf_d1_bullish")
        );
        assert_eq!(
            htf_filter_allows(HtfFilter::H4D1, &buy, &d1_bullish),
            Ok(())
        );

        let neutral = htf_snapshot(
            Bias::Neutral,
            Bias::Neutral,
            Some(0.5),
            Some(0.5),
            false,
            false,
        );
        assert_eq!(htf_filter_allows(HtfFilter::H4D1, &buy, &neutral), Ok(()));
        assert_eq!(htf_filter_allows(HtfFilter::H4D1, &sell, &neutral), Ok(()));

        let opposing_h4 = htf_snapshot(
            Bias::Neutral,
            Bias::Bearish,
            Some(0.5),
            Some(0.5),
            false,
            false,
        );
        assert_eq!(
            htf_filter_allows(HtfFilter::H4D1, &buy, &opposing_h4),
            Err("htf_h4_bearish")
        );
    }

    // -----------------------------------------------------------------------
    // Phase 1B: adaptive swing stop
    // -----------------------------------------------------------------------

    fn wide_flat_candles(start: i64, count: usize, high: f64, low: f64) -> Vec<Candle> {
        (0..count)
            .map(|i| candle(start + i as i64 * STEP_MS, 100.0, high, low, 100.0))
            .collect()
    }

    #[test]
    fn swing_sl_uses_lookback_extreme_and_widens_to_eight_tenths_atr() {
        let start = utc_ms(2024, 1, 10, 15, 0);
        let candles = wide_flat_candles(start, 20, 110.0, 99.0);
        let raw = raw_signal(19, SignalType::Buy, 100.0, 95.0, 130.0, 140.0);

        let levels = swing_sl_levels(&candles, &raw).expect("swing levels");
        // ATR14 = 11, so the 3.5 risk from swing_low - 2.5 is below 0.5 ATR
        // and is widened to 0.8 * 11 = 8.8 below entry.
        assert!((levels.stop_loss - 91.2).abs() < 1e-9);
        assert!((levels.take_profit_1 - 130.0).abs() < 1e-9);
        assert!((levels.take_profit_2 - 140.0).abs() < 1e-9);
    }

    #[test]
    fn swing_sl_rejects_when_widened_risk_exceeds_cap() {
        let start = utc_ms(2024, 1, 10, 15, 0);
        let candles = wide_flat_candles(start, 20, 160.0, 99.0);
        let raw = raw_signal(19, SignalType::Buy, 100.0, 95.0, 130.0, 140.0);

        assert!(matches!(
            swing_sl_levels(&candles, &raw),
            Err("swing_risk_gt_30")
        ));
        assert!(matches!(
            swing_sl_levels(
                &candles[..19],
                &raw_signal(18, SignalType::Buy, 100.0, 95.0, 130.0, 140.0)
            ),
            Err("swing_lookback_missing")
        ));
    }

    #[test]
    fn swing_sl_rounds_to_two_decimals() {
        let start = utc_ms(2024, 1, 10, 15, 0);
        let candles: Vec<Candle> = (0..20)
            .map(|i| candle(start + i as i64 * STEP_MS, 100.0, 100.5, 98.876, 100.0))
            .collect();
        let raw = raw_signal(19, SignalType::Buy, 100.0, 95.0, 130.0, 140.0);

        let levels = swing_sl_levels(&candles, &raw).expect("swing levels");
        assert!((levels.stop_loss - 96.38).abs() < 1e-9);
    }

    // -----------------------------------------------------------------------
    // Phase 1B: TP structure variants
    // -----------------------------------------------------------------------

    #[test]
    fn tp_structure_soft_falls_back_and_caps_instead_of_rejecting() {
        let low = tp_structure_soft_levels(100.0, 95.0, true, &[115.0], &[125.0])
            .expect("soft fallback levels");
        assert!((low.take_profit_1 - 110.0).abs() < 1e-9);
        assert!((low.take_profit_2 - 125.0).abs() < 1e-9);

        let capped =
            tp_structure_soft_levels(100.0, 95.0, true, &[150.0], &[]).expect("soft capped levels");
        assert!((capped.take_profit_1 - 135.0).abs() < 1e-9);
        assert!((capped.take_profit_2 - 140.0).abs() < 1e-9);

        assert!(matches!(
            tp_structure_soft_levels(100.0, 79.0, true, &[120.0], &[130.0]),
            Err("risk_gt_20")
        ));
    }

    #[test]
    fn tp_structure_h4_uses_next_h4_level_and_fallbacks() {
        let levels = tp_structure_h4_levels(100.0, 95.0, true, &[115.0, 125.0])
            .expect("h4 structure levels");
        assert!((levels.take_profit_1 - 110.0).abs() < 1e-9);
        assert!((levels.take_profit_2 - 115.0).abs() < 1e-9);

        let sell =
            tp_structure_h4_levels(100.0, 105.0, false, &[85.0, 75.0]).expect("h4 sell levels");
        assert!((sell.take_profit_1 - 90.0).abs() < 1e-9);
        assert!((sell.take_profit_2 - 85.0).abs() < 1e-9);
    }

    // -----------------------------------------------------------------------
    // Phase 1B: partial 25% and trailing stops
    // -----------------------------------------------------------------------

    #[test]
    fn partial_25_books_quarter_at_twenty_before_tp1() {
        let candles = vec![
            candle(0, 100.0, 100.5, 99.5, 100.0),
            candle(STEP_MS, 100.0, 121.0, 99.0, 120.0),
            candle(2 * STEP_MS, 120.0, 121.0, 94.0, 95.0),
        ];
        let raw = raw_signal(0, SignalType::Buy, 100.0, 95.0, 130.0, 140.0);
        let management = PartialTrailManagement {
            partial: Some(PartialRule::new(0.25, 20.0)),
            trail: None,
        };

        let trade =
            simulate_partial_trail(&candles, 0, &raw, raw.levels(), management).expect("trade");
        assert_eq!(trade.outcome, Outcome::Sl);
        assert!((trade.r_multiple - 0.25).abs() < 1e-9);
    }

    #[test]
    fn partial_25_does_not_book_after_tp1() {
        let candles = vec![
            candle(0, 100.0, 100.5, 99.5, 100.0),
            candle(STEP_MS, 100.0, 121.0, 99.0, 120.0),
            candle(2 * STEP_MS, 120.0, 121.0, 94.0, 95.0),
        ];
        let raw = raw_signal(0, SignalType::Buy, 100.0, 95.0, 115.0, 140.0);
        let management = PartialTrailManagement {
            partial: Some(PartialRule::new(0.25, 20.0)),
            trail: None,
        };

        let trade =
            simulate_partial_trail(&candles, 0, &raw, raw.levels(), management).expect("trade");
        assert_eq!(trade.outcome, Outcome::Tp1);
        assert!((trade.r_multiple - 3.0).abs() < 1e-9);
    }

    #[test]
    fn partial_trail_keeps_original_stop_after_tp1() {
        // A pullback to TP1 must not become a hard TP1 stop: baseline keeps
        // the original stop and can still reach TP2.
        let candles = vec![
            candle(0, 100.0, 100.5, 99.5, 100.0),
            candle(STEP_MS, 100.0, 111.0, 109.0, 110.0),
            candle(2 * STEP_MS, 110.0, 112.0, 108.0, 109.0),
            candle(3 * STEP_MS, 109.0, 126.0, 108.0, 125.0),
        ];
        let raw = raw_signal(0, SignalType::Buy, 100.0, 95.0, 110.0, 125.0);
        let management = PartialTrailManagement {
            partial: Some(PartialRule::new(0.25, 20.0)),
            trail: None,
        };

        let trade =
            simulate_partial_trail(&candles, 0, &raw, raw.levels(), management).expect("trade");
        assert_eq!(trade.outcome, Outcome::Tp2);
        assert!((trade.r_multiple - 5.0).abs() < 1e-9);
    }

    #[test]
    fn trail_after_twenty_ratchets_in_one_dollar_steps() {
        let candles = vec![
            candle(0, 100.0, 100.5, 99.5, 100.0),
            candle(STEP_MS, 100.0, 121.0, 112.0, 120.0),
            candle(2 * STEP_MS, 120.0, 131.0, 112.0, 130.0),
            candle(3 * STEP_MS, 130.0, 130.0, 120.0, 121.0),
        ];
        let raw = raw_signal(0, SignalType::Buy, 100.0, 95.0, 150.0, 160.0);
        let management = PartialTrailManagement {
            partial: None,
            trail: Some(TrailRule::new(20.0, 10.0, 1.0)),
        };

        let trade =
            simulate_partial_trail(&candles, 0, &raw, raw.levels(), management).expect("trade");
        assert_eq!(trade.outcome, Outcome::Trail);
        assert!((trade.exit_price - 121.0).abs() < 1e-9);
        assert!((trade.r_multiple - 4.2).abs() < 1e-9);
    }

    #[test]
    fn trail_after_twenty_does_not_activate_below_threshold() {
        let candles = vec![
            candle(0, 100.0, 100.5, 99.5, 100.0),
            candle(STEP_MS, 100.0, 119.0, 99.0, 110.0),
            candle(2 * STEP_MS, 110.0, 111.0, 94.0, 95.0),
        ];
        let raw = raw_signal(0, SignalType::Buy, 100.0, 95.0, 130.0, 140.0);
        let management = PartialTrailManagement {
            partial: None,
            trail: Some(TrailRule::new(20.0, 10.0, 1.0)),
        };

        let trade =
            simulate_partial_trail(&candles, 0, &raw, raw.levels(), management).expect("trade");
        assert_eq!(trade.outcome, Outcome::Sl);
        assert!((trade.r_multiple + 1.0).abs() < 1e-9);
    }

    #[test]
    fn partial_25_and_trail_blend_quarter_and_remainder() {
        let candles = vec![
            candle(0, 100.0, 100.5, 99.5, 100.0),
            candle(STEP_MS, 100.0, 121.0, 112.0, 120.0),
            candle(2 * STEP_MS, 120.0, 121.0, 110.0, 111.0),
        ];
        let raw = raw_signal(0, SignalType::Buy, 100.0, 95.0, 150.0, 160.0);
        let management = PartialTrailManagement {
            partial: Some(PartialRule::new(0.25, 20.0)),
            trail: Some(TrailRule::new(20.0, 10.0, 1.0)),
        };

        let trade =
            simulate_partial_trail(&candles, 0, &raw, raw.levels(), management).expect("trade");
        assert_eq!(trade.outcome, Outcome::Trail);
        assert!((trade.exit_price - 111.0).abs() < 1e-9);
        assert!((trade.r_multiple - 2.65).abs() < 1e-9);
    }

    #[test]
    fn partial_10_books_ten_percent_at_twenty_before_tp1() {
        let candles = vec![
            candle(0, 100.0, 100.5, 99.5, 100.0),
            candle(STEP_MS, 100.0, 121.0, 99.0, 120.0),
            candle(2 * STEP_MS, 120.0, 121.0, 94.0, 95.0),
        ];
        let raw = raw_signal(0, SignalType::Buy, 100.0, 95.0, 130.0, 140.0);
        let management = PartialTrailManagement {
            partial: Some(PartialRule::new(0.10, 20.0)),
            trail: None,
        };

        let trade =
            simulate_partial_trail(&candles, 0, &raw, raw.levels(), management).expect("trade");
        assert_eq!(trade.outcome, Outcome::Sl);
        assert!((trade.r_multiple - -0.5).abs() < 1e-9);
    }

    #[test]
    fn partial_15_books_fifteen_percent_at_twenty_five_before_tp1() {
        let candles = vec![
            candle(0, 100.0, 100.5, 99.5, 100.0),
            candle(STEP_MS, 100.0, 126.0, 99.0, 125.0),
            candle(2 * STEP_MS, 125.0, 126.0, 94.0, 95.0),
        ];
        let raw = raw_signal(0, SignalType::Buy, 100.0, 95.0, 130.0, 140.0);
        let management = PartialTrailManagement {
            partial: Some(PartialRule::new(0.15, 25.0)),
            trail: None,
        };

        let trade =
            simulate_partial_trail(&candles, 0, &raw, raw.levels(), management).expect("trade");
        assert_eq!(trade.outcome, Outcome::Sl);
        assert!((trade.r_multiple - -0.1).abs() < 1e-9);
    }

    #[test]
    fn partial_15_does_not_book_after_tp1() {
        let candles = vec![
            candle(0, 100.0, 100.5, 99.5, 100.0),
            candle(STEP_MS, 100.0, 126.0, 99.0, 125.0),
            candle(2 * STEP_MS, 125.0, 126.0, 94.0, 95.0),
        ];
        let raw = raw_signal(0, SignalType::Buy, 100.0, 95.0, 120.0, 140.0);
        let management = PartialTrailManagement {
            partial: Some(PartialRule::new(0.15, 25.0)),
            trail: None,
        };

        let trade =
            simulate_partial_trail(&candles, 0, &raw, raw.levels(), management).expect("trade");
        assert_eq!(trade.outcome, Outcome::Tp1);
        assert!((trade.r_multiple - 4.0).abs() < 1e-9);
    }

    #[test]
    fn trail_after_30_requires_thirty_usd_mfe() {
        let candles = vec![
            candle(0, 100.0, 100.5, 99.5, 100.0),
            candle(STEP_MS, 100.0, 129.0, 99.0, 110.0),
            candle(2 * STEP_MS, 110.0, 111.0, 94.0, 95.0),
        ];
        let raw = raw_signal(0, SignalType::Buy, 100.0, 95.0, 150.0, 160.0);
        let management = PartialTrailManagement {
            partial: None,
            trail: Some(TrailRule::new(30.0, 10.0, 1.0)),
        };

        let trade =
            simulate_partial_trail(&candles, 0, &raw, raw.levels(), management).expect("trade");
        assert_eq!(trade.outcome, Outcome::Sl);
        assert!((trade.r_multiple + 1.0).abs() < 1e-9);
    }

    #[test]
    fn trail_after_30_ratchets_ten_behind_high_water_mark() {
        let candles = vec![
            candle(0, 100.0, 100.5, 99.5, 100.0),
            candle(STEP_MS, 100.0, 131.0, 112.0, 130.0),
            candle(2 * STEP_MS, 130.0, 132.0, 120.0, 121.0),
        ];
        let raw = raw_signal(0, SignalType::Buy, 100.0, 95.0, 150.0, 160.0);
        let management = PartialTrailManagement {
            partial: None,
            trail: Some(TrailRule::new(30.0, 10.0, 1.0)),
        };

        let trade =
            simulate_partial_trail(&candles, 0, &raw, raw.levels(), management).expect("trade");
        assert_eq!(trade.outcome, Outcome::Trail);
        assert!((trade.exit_price - 121.0).abs() < 1e-9);
        assert!((trade.r_multiple - 4.2).abs() < 1e-9);
    }

    #[test]
    fn trail_after_30_ratchets_for_sell_side() {
        let candles = vec![
            candle(0, 100.0, 100.5, 99.5, 100.0),
            candle(STEP_MS, 100.0, 88.0, 69.0, 70.0),
            candle(2 * STEP_MS, 70.0, 80.0, 68.0, 79.0),
        ];
        let raw = raw_signal(0, SignalType::Sell, 100.0, 105.0, 60.0, 50.0);
        let management = PartialTrailManagement {
            partial: None,
            trail: Some(TrailRule::new(30.0, 10.0, 1.0)),
        };

        let trade =
            simulate_partial_trail(&candles, 0, &raw, raw.levels(), management).expect("trade");
        assert_eq!(trade.outcome, Outcome::Trail);
        assert!((trade.exit_price - 79.0).abs() < 1e-9);
        assert!((trade.r_multiple - 4.2).abs() < 1e-9);
    }

    #[test]
    fn trail_outcome_is_counted_as_a_win_and_reported() {
        let trades = vec![simulated_trade(Outcome::Trail, 4.2, 5.0, 0.4, 31.0)];
        let metrics = compute_metrics(&trades);
        assert_eq!(metrics.trail, 1);
        assert_eq!(metrics.wins, 1);
        assert!((metrics.net_r - 4.12).abs() < 1e-9);
    }

    // -----------------------------------------------------------------------
    // Phase 1C: variants and walk-forward
    // -----------------------------------------------------------------------

    #[test]
    fn phase1c_variant_names_round_trip_through_parse() {
        let variants = [
            (Variant::Partial10At20, "partial_10_at_20"),
            (Variant::Partial15At25, "partial_15_at_25"),
            (Variant::TrailAfter30, "trail_after_30"),
            (Variant::HtfH4Partial10, "htf_h4_partial_10"),
            (Variant::HtfH4Partial15, "htf_h4_partial_15"),
            (Variant::HtfH4Trail30, "htf_h4_trail_30"),
            (Variant::HtfH4Zone, "htf_h4_zone"),
            (Variant::HtfH4TpStructureSoft, "htf_h4_tp_structure_soft"),
            (Variant::HtfH4D1, "htf_h4_d1"),
        ];

        for (variant, name) in variants {
            assert_eq!(variant.name(), name);
            assert_eq!(Variant::parse(name), Some(variant));
        }
    }

    #[test]
    fn walk_forward_windows_cover_five_fixed_years() {
        let start = NaiveDate::from_ymd_opt(2021, 11, 10).expect("valid start");
        let end = NaiveDate::from_ymd_opt(2026, 11, 10).expect("valid end");
        let windows = walk_forward_windows(start, end);

        assert_eq!(windows.len(), 5);
        assert_eq!(windows[0].label, "W1");
        assert_eq!(windows[0].start_date.to_string(), "2021-11-10");
        assert_eq!(windows[0].end_date.to_string(), "2022-11-10");
        assert_eq!(windows[4].label, "W5");
        assert_eq!(windows[4].start_date.to_string(), "2025-11-10");
        assert_eq!(windows[4].end_date.to_string(), "2026-11-10");
        assert_eq!(windows[0].end_ms, windows[1].start_ms);
    }

    #[test]
    fn rolling_windows_build_three_year_is_and_one_year_oos() {
        let start = NaiveDate::from_ymd_opt(2021, 11, 10).expect("valid start");
        let end = NaiveDate::from_ymd_opt(2026, 11, 10).expect("valid end");
        let windows = rolling_is_oos_windows(start, end);

        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].label, "R1");
        assert_eq!(windows[0].is_start_date.to_string(), "2021-11-10");
        assert_eq!(windows[0].is_end_date.to_string(), "2024-11-10");
        assert_eq!(windows[0].oos_end_date.to_string(), "2025-11-10");
        assert_eq!(windows[1].is_start_date.to_string(), "2022-11-10");
        assert_eq!(windows[1].oos_end_date.to_string(), "2026-11-10");
    }

    #[test]
    fn window_metrics_use_net_r_and_flag_thin_samples() {
        let trades = vec![simulated_trade(Outcome::Tp1, 2.0, 10.0, 0.4, 20.0)];
        let metrics = window_metrics_for_range(&trades, 0, 2);

        assert_eq!(metrics.trades, 1);
        assert!(metrics.sample_thin);
        assert!((metrics.net_r - 1.96).abs() < 1e-9);
        assert!((metrics.expectancy_r - 1.96).abs() < 1e-9);
    }

    #[test]
    fn walk_forward_summary_counts_positive_and_negative_windows() {
        let window = |label: &str, trades: usize, net_r: f64| WalkForwardWindow {
            label: label.to_string(),
            start: "2021-11-10".to_string(),
            end: "2022-11-10".to_string(),
            metrics: WindowMetrics {
                trades,
                net_r,
                profit_factor: None,
                max_drawdown_r: 0.0,
                expectancy_r: net_r,
                win_rate_pct: 0.0,
                sample_thin: trades < 30,
            },
        };
        let windows = vec![
            window("W1", 10, 2.0),
            window("W2", 10, -1.0),
            window("W3", 0, 0.0),
            window("W4", 10, 0.0),
            window("W5", 10, 3.0),
        ];

        let summary = summarize_walk_forward(&windows);
        assert_eq!(summary.windows, 5);
        assert_eq!(summary.evaluated_windows, 4);
        assert_eq!(summary.positive_windows, 2);
        assert_eq!(summary.negative_windows, 1);
        assert_eq!(summary.flat_windows, 1);
        assert!((summary.consistency_pct - 50.0).abs() < 1e-9);
        assert!((summary.best_net_r - 3.0).abs() < 1e-9);
        assert!((summary.worst_net_r + 1.0).abs() < 1e-9);
    }

    // -----------------------------------------------------------------------
    // Phase 1D: volume delta / CVD filters
    // -----------------------------------------------------------------------

    fn flow_candle(timestamp: i64, close: f64, taker_buy_volume: f64) -> Candle {
        Candle {
            timestamp,
            open: close,
            high: close + 0.5,
            low: close - 0.5,
            close,
            volume: 100.0,
            taker_buy_volume,
            timeframe: Timeframe::M15,
        }
    }

    #[test]
    fn delta_math_matches_taker_flow_and_handles_zero_volume() {
        let buy_flow = flow_candle(0, 100.0, 60.0);
        assert!((candle_delta(&buy_flow) - 20.0).abs() < 1e-9);
        assert!((delta_ratio(&buy_flow) - 0.20).abs() < 1e-9);

        let sell_flow = flow_candle(0, 100.0, 30.0);
        assert!((candle_delta(&sell_flow) + 40.0).abs() < 1e-9);
        assert!((delta_ratio(&sell_flow) + 0.40).abs() < 1e-9);

        let mut zero_volume = flow_candle(0, 100.0, 0.0);
        zero_volume.volume = 0.0;
        assert_eq!(delta_ratio(&zero_volume), 0.0);

        let cvd = cumulative_delta(&[
            flow_candle(0, 100.0, 60.0),
            flow_candle(STEP_MS, 100.0, 40.0),
        ]);
        assert!((cvd[0] - 20.0).abs() < 1e-9);
        assert!(cvd[1].abs() < 1e-9);
    }

    #[test]
    fn delta_alignment_requires_ten_percent_aggressive_flow() {
        assert!(delta_alignment_allows(true, &[flow_candle(0, 100.0, 55.0)], 0).is_ok());
        assert_eq!(
            delta_alignment_allows(true, &[flow_candle(0, 100.0, 54.9)], 0),
            Err("delta_align")
        );
        assert!(delta_alignment_allows(false, &[flow_candle(0, 100.0, 45.0)], 0).is_ok());
        assert_eq!(
            delta_alignment_allows(false, &[flow_candle(0, 100.0, 45.1)], 0),
            Err("delta_align")
        );
        assert_eq!(delta_alignment_allows(true, &[], 0), Err("delta_align"));
    }

    #[test]
    fn delta_divergence_detects_absorption_and_exhaustion() {
        // Price falls while aggressive buyers accumulate: bullish absorption.
        let bullish: Vec<Candle> = (0..=DELTA_LOOKBACK)
            .map(|i| flow_candle(i as i64 * STEP_MS, 100.0 - i as f64 * 0.1, 60.0))
            .collect();
        let bullish_cvd = cumulative_delta(&bullish);
        assert!(delta_divergence_allows(true, &bullish, &bullish_cvd, DELTA_LOOKBACK).is_ok());
        assert_eq!(
            delta_divergence_allows(false, &bullish, &bullish_cvd, DELTA_LOOKBACK),
            Err("delta_divergence")
        );

        // Price rises while aggressive sellers dominate: bearish exhaustion.
        let bearish: Vec<Candle> = (0..=DELTA_LOOKBACK)
            .map(|i| flow_candle(i as i64 * STEP_MS, 100.0 + i as f64 * 0.1, 40.0))
            .collect();
        let bearish_cvd = cumulative_delta(&bearish);
        assert!(delta_divergence_allows(false, &bearish, &bearish_cvd, DELTA_LOOKBACK).is_ok());
        assert_eq!(
            delta_divergence_allows(true, &bearish, &bearish_cvd, DELTA_LOOKBACK),
            Err("delta_divergence")
        );

        // Price and CVD both fall: no bullish divergence.
        let both_fall: Vec<Candle> = (0..=DELTA_LOOKBACK)
            .map(|i| flow_candle(i as i64 * STEP_MS, 100.0 - i as f64 * 0.1, 40.0))
            .collect();
        let both_fall_cvd = cumulative_delta(&both_fall);
        assert_eq!(
            delta_divergence_allows(true, &both_fall, &both_fall_cvd, DELTA_LOOKBACK),
            Err("delta_divergence")
        );
        assert_eq!(
            delta_divergence_allows(true, &bullish, &bullish_cvd, DELTA_LOOKBACK - 1),
            Err("delta_divergence")
        );
    }

    #[test]
    fn delta_both_allows_either_leg_and_reports_align_when_both_fail() {
        // Alignment passes; divergence is not required.
        let aligned = vec![flow_candle(0, 100.0, 60.0)];
        assert!(delta_filter_allows(DeltaFilter::Both, true, &aligned, &[20.0], 0).is_ok());

        // Alignment fails at the signal candle, but bullish absorption passes.
        let mut absorbing: Vec<Candle> = (0..=DELTA_LOOKBACK)
            .map(|i| flow_candle(i as i64 * STEP_MS, 100.0 - i as f64 * 0.1, 60.0))
            .collect();
        absorbing[DELTA_LOOKBACK].taker_buy_volume = 50.0;
        let absorbing_cvd = cumulative_delta(&absorbing);
        assert!(delta_filter_allows(
            DeltaFilter::Both,
            true,
            &absorbing,
            &absorbing_cvd,
            DELTA_LOOKBACK
        )
        .is_ok());

        // Flat price with neutral flow fails both legs.
        let flat: Vec<Candle> = (0..=DELTA_LOOKBACK)
            .map(|i| flow_candle(i as i64 * STEP_MS, 100.0, 50.0))
            .collect();
        let flat_cvd = cumulative_delta(&flat);
        assert_eq!(
            delta_filter_allows(DeltaFilter::Both, true, &flat, &flat_cvd, DELTA_LOOKBACK),
            Err("delta_align")
        );
    }

    #[test]
    fn phase1d_variant_names_round_trip_through_parse() {
        let variants = [
            (Variant::DeltaAlign, "delta_align"),
            (Variant::DeltaDivergence, "delta_divergence"),
            (Variant::DeltaBoth, "delta_both"),
            (Variant::HtfH4DeltaAlign, "htf_h4_delta_align"),
            (Variant::HtfH4D1DeltaAlign, "htf_h4_d1_delta_align"),
            (Variant::HtfH4D1DeltaBoth, "htf_h4_d1_delta_both"),
        ];

        for (variant, name) in variants {
            assert_eq!(variant.name(), name);
            assert_eq!(Variant::parse(name), Some(variant));
        }
        assert_eq!(
            variant_plan(Variant::HtfH4D1DeltaBoth).delta,
            Some(DeltaFilter::Both)
        );
        assert_eq!(variant_plan(Variant::Baseline).delta, None);
    }

    #[test]
    fn run_variant_records_delta_rejection_reasons() {
        let start = utc_ms(2024, 1, 10, 15, 0);
        let candles: Vec<Candle> = (0..40)
            .map(|i| flow_candle(start + i as i64 * STEP_MS, 100.0, 50.0))
            .collect();
        let cvd = cumulative_delta(&candles);
        let htf = HtfContext::new(&candles);
        let context = BacktestContext {
            candles: &candles,
            h1: &[],
            h4: &[],
            htf: &htf,
            cvd: &cvd,
            rsi: &[],
            split_timestamp: start,
            early_be: 0.0,
            cost_usd: 0.0,
            step_ms: STEP_MS,
            walk_forward: &[],
            rolling: &[],
        };
        let raw = raw_signal(25, SignalType::Buy, 100.0, 95.0, 110.0, 115.0);
        let signals = std::slice::from_ref(&raw);

        let align = run_variant(Variant::DeltaAlign, signals, &context);
        assert_eq!(align.trades, 0);
        assert_eq!(align.rejected, 1);
        assert_eq!(align.reject_reasons.get("delta_align"), Some(&1));

        let divergence = run_variant(Variant::DeltaDivergence, signals, &context);
        assert_eq!(divergence.rejected, 1);
        assert_eq!(divergence.reject_reasons.get("delta_divergence"), Some(&1));

        let both = run_variant(Variant::DeltaBoth, signals, &context);
        assert_eq!(both.rejected, 1);
        assert_eq!(both.reject_reasons.get("delta_align"), Some(&1));
    }

    // -----------------------------------------------------------------------
    // Phase R0: RSI + divergence + volume modes
    // -----------------------------------------------------------------------

    fn range_candle(timestamp: i64, close: f64, volume: f64, range: f64) -> Candle {
        Candle {
            timestamp,
            open: close,
            high: close + range / 2.0,
            low: close - range / 2.0,
            close,
            volume,
            taker_buy_volume: 0.0,
            timeframe: Timeframe::M15,
        }
    }

    #[test]
    fn phase_r0_variant_names_round_trip_through_parse() {
        let variants = [
            (Variant::RsiExtreme, "rsi_extreme"),
            (Variant::RsiExtreme6535, "rsi_extreme_65_35"),
            (Variant::RsiDivergence, "rsi_divergence"),
            (Variant::RsiBoth, "rsi_both"),
            (Variant::HtfH4D1RsiExtreme, "htf_h4_d1_rsi_extreme"),
            (Variant::HtfH4D1RsiBoth, "htf_h4_d1_rsi_both"),
            (Variant::VolumeExhaustion, "volume_exhaustion"),
            (Variant::VolumeEither, "volume_either"),
            (
                Variant::RsiDivergenceVolumeExhaustion,
                "rsi_divergence_volume_exhaustion",
            ),
            (
                Variant::HtfH4D1RsiVolumeEither,
                "htf_h4_d1_rsi_volume_either",
            ),
        ];

        for (variant, name) in variants {
            assert_eq!(variant.name(), name);
            assert_eq!(Variant::parse(name), Some(variant));
        }

        assert_eq!(
            variant_plan(Variant::RsiExtreme).rsi,
            Some(RsiGate::Extreme {
                overbought: 70.0,
                oversold: 30.0,
            })
        );
        assert_eq!(
            variant_plan(Variant::RsiExtreme6535).rsi,
            Some(RsiGate::Extreme {
                overbought: 65.0,
                oversold: 35.0,
            })
        );
        assert_eq!(
            variant_plan(Variant::VolumeExhaustion).volume,
            Some(VolumeFilter::Exhaustion)
        );
        assert_eq!(
            variant_plan(Variant::VolumeEither).volume,
            Some(VolumeFilter::Either)
        );
        assert_eq!(
            variant_plan(Variant::HtfH4D1RsiVolumeEither).htf,
            Some(HtfFilter::H4D1)
        );
        assert_eq!(variant_plan(Variant::Baseline).rsi, None);
        assert_eq!(variant_plan(Variant::Baseline).volume, None);
    }

    #[test]
    fn rsi_gate_requires_extreme_or_matching_divergence() {
        let start = utc_ms(2024, 1, 10, 15, 0);
        let candles: Vec<Candle> = (0..20)
            .map(|index| flow_candle(start + index as i64 * STEP_MS, 100.0, 50.0))
            .collect();
        let mut rsi = vec![Some(50.0); candles.len()];

        // 70/30 extreme gate.
        assert!(rsi_extreme_allows(true, Some(30.0), 70.0, 30.0).is_ok());
        assert_eq!(
            rsi_extreme_allows(true, Some(30.1), 70.0, 30.0),
            Err("rsi_extreme")
        );
        assert!(rsi_extreme_allows(false, Some(70.0), 70.0, 30.0).is_ok());
        assert_eq!(
            rsi_extreme_allows(false, Some(69.9), 70.0, 30.0),
            Err("rsi_extreme")
        );
        assert_eq!(
            rsi_extreme_allows(true, None, 70.0, 30.0),
            Err("rsi_extreme")
        );

        // 65/35 extreme gate is tighter.
        assert!(rsi_extreme_allows(true, Some(35.0), 65.0, 35.0).is_ok());
        assert_eq!(
            rsi_extreme_allows(true, Some(35.1), 65.0, 35.0),
            Err("rsi_extreme")
        );
        assert!(rsi_extreme_allows(false, Some(65.0), 65.0, 35.0).is_ok());
        assert_eq!(
            rsi_extreme_allows(false, Some(64.9), 65.0, 35.0),
            Err("rsi_extreme")
        );

        // Divergence must match the trade direction.
        assert!(rsi_divergence_allows(true, RsiDivergence::Bullish).is_ok());
        assert_eq!(
            rsi_divergence_allows(true, RsiDivergence::Bearish),
            Err("rsi_divergence")
        );
        assert_eq!(
            rsi_divergence_allows(false, RsiDivergence::None),
            Err("rsi_divergence")
        );

        // Both gate: extreme passes; neutral without pivots fails as rsi_extreme.
        let both = RsiGate::Both {
            overbought: 70.0,
            oversold: 30.0,
        };
        rsi[19] = Some(25.0);
        assert!(rsi_gate_allows(both, true, &candles, &rsi, 19).is_ok());
        rsi[19] = Some(50.0);
        assert_eq!(
            rsi_gate_allows(both, true, &candles, &rsi, 19),
            Err("rsi_extreme")
        );
        assert_eq!(
            rsi_gate_allows(RsiGate::Divergence, true, &candles, &rsi, 19),
            Err("rsi_divergence")
        );
        assert_eq!(
            rsi_gate_allows(
                RsiGate::Extreme {
                    overbought: 70.0,
                    oversold: 30.0,
                },
                true,
                &candles,
                &[],
                19,
            ),
            Err("rsi_extreme")
        );
    }

    // -----------------------------------------------------------------------
    // Phase R1: recent-window extremes, regime, and slope gates
    // -----------------------------------------------------------------------

    #[test]
    fn rsi_recent_extreme_checks_only_the_lookback_window() {
        let mut rsi = vec![Some(50.0); 40];

        // A touch inside a 10-candle window ending at index 30 (21..=30).
        rsi[21] = Some(25.0);
        assert!(rsi_recent_extreme_allows(true, &rsi, 30, 10, 70.0, 30.0).is_ok());

        // The same touch one candle older falls outside the window.
        rsi[21] = Some(50.0);
        rsi[20] = Some(25.0);
        assert_eq!(
            rsi_recent_extreme_allows(true, &rsi, 30, 10, 70.0, 30.0),
            Err("rsi_recent_extreme")
        );

        // The signal candle itself counts and thresholds are inclusive.
        rsi[20] = Some(50.0);
        rsi[30] = Some(30.0);
        assert!(rsi_recent_extreme_allows(true, &rsi, 30, 10, 70.0, 30.0).is_ok());

        // SELL mirror: overbought touch passes, oversold does not.
        let mut sell_rsi = vec![Some(50.0); 40];
        sell_rsi[29] = Some(75.0);
        assert!(rsi_recent_extreme_allows(false, &sell_rsi, 30, 10, 70.0, 30.0).is_ok());
        assert_eq!(
            rsi_recent_extreme_allows(true, &sell_rsi, 30, 10, 70.0, 30.0),
            Err("rsi_recent_extreme")
        );

        // None/non-finite values are ignored and insufficient history rejects.
        let mut missing = vec![Some(50.0); 40];
        missing[29] = None;
        missing[28] = Some(f64::NAN);
        assert_eq!(
            rsi_recent_extreme_allows(true, &missing, 30, 10, 70.0, 30.0),
            Err("rsi_recent_extreme")
        );
        assert_eq!(
            rsi_recent_extreme_allows(true, &missing, 5, 10, 70.0, 30.0),
            Err("rsi_recent_extreme")
        );
        assert_eq!(
            rsi_recent_extreme_allows(true, &missing, 39, 0, 70.0, 30.0),
            Err("rsi_recent_extreme")
        );
    }

    #[test]
    fn rsi_recent_extreme_10_and_20_lookbacks_differ() {
        let mut rsi = vec![Some(50.0); 40];
        // 15 candles before index 30: outside 10, inside 20.
        rsi[15] = Some(25.0);

        assert_eq!(
            rsi_recent_extreme_allows(true, &rsi, 30, 10, 70.0, 30.0),
            Err("rsi_recent_extreme")
        );
        assert!(rsi_recent_extreme_allows(true, &rsi, 30, 20, 70.0, 30.0).is_ok());
    }

    #[test]
    fn rsi_recent_extreme_respects_65_35_thresholds() {
        let mut rsi = vec![Some(50.0); 20];
        rsi[12] = Some(34.0);

        assert!(rsi_recent_extreme_allows(true, &rsi, 19, 10, 65.0, 35.0).is_ok());
        assert_eq!(
            rsi_recent_extreme_allows(true, &rsi, 19, 10, 70.0, 30.0),
            Err("rsi_recent_extreme")
        );

        // 35.0 exactly is inclusive for BUY.
        rsi[12] = Some(35.0);
        assert!(rsi_recent_extreme_allows(true, &rsi, 19, 10, 65.0, 35.0).is_ok());

        // 66.0 passes 65/35 for SELL but not 70/30.
        rsi[12] = Some(66.0);
        assert!(rsi_recent_extreme_allows(false, &rsi, 19, 10, 65.0, 35.0).is_ok());
        assert_eq!(
            rsi_recent_extreme_allows(false, &rsi, 19, 10, 70.0, 30.0),
            Err("rsi_recent_extreme")
        );
    }

    #[test]
    fn rsi_recent_extreme_or_divergence_accepts_either_leg() {
        let start = utc_ms(2024, 1, 10, 15, 0);
        let mut candles: Vec<Candle> = (0..20)
            .map(|index| candle(start + index as i64 * STEP_MS, 100.0, 100.5, 99.5, 100.0))
            .collect();
        // Lower lows with higher RSI lows -> bullish divergence.
        candles[5].low = 98.0;
        candles[12].low = 97.0;

        let mut rsi = vec![Some(50.0); candles.len()];
        rsi[5] = Some(45.0);
        rsi[12] = Some(55.0);

        let gate = RsiGate::RecentExtremeOrDivergence {
            overbought: 70.0,
            oversold: 30.0,
            lookback: 20,
        };

        // Divergence leg only: BUY passes, SELL does not.
        assert!(rsi_gate_allows(gate, true, &candles, &rsi, 19).is_ok());
        assert_eq!(
            rsi_gate_allows(gate, false, &candles, &rsi, 19),
            Err("rsi_recent_extreme")
        );

        // Recent-extreme leg only: remove the divergence and touch oversold.
        let mut recent_only = rsi.clone();
        recent_only[5] = Some(55.0);
        recent_only[19] = Some(25.0);
        assert!(rsi_gate_allows(gate, true, &candles, &recent_only, 19).is_ok());

        // Neither leg: a single rejection is counted as rsi_recent_extreme.
        let mut neither = recent_only.clone();
        neither[19] = Some(50.0);
        assert_eq!(
            rsi_gate_allows(gate, true, &candles, &neither, 19),
            Err("rsi_recent_extreme")
        );
        assert_eq!(
            rsi_gate_allows(gate, false, &candles, &neither, 19),
            Err("rsi_recent_extreme")
        );
    }

    #[test]
    fn rsi_regime_requires_direction_and_threshold() {
        let mut rsi = vec![Some(50.0); 10];
        rsi[7] = Some(45.0);
        rsi[9] = Some(55.0);

        // BUY: above 50 and rising over the lookback.
        assert!(rsi_regime_allows(true, &rsi, 9, 2, 50.0).is_ok());
        assert_eq!(
            rsi_regime_allows(false, &rsi, 9, 2, 50.0),
            Err("rsi_regime")
        );

        // SELL: below 50 and falling over the lookback.
        rsi[7] = Some(55.0);
        rsi[9] = Some(45.0);
        assert!(rsi_regime_allows(false, &rsi, 9, 2, 50.0).is_ok());
        assert_eq!(rsi_regime_allows(true, &rsi, 9, 2, 50.0), Err("rsi_regime"));

        // Right direction but on the wrong side of the threshold.
        rsi[7] = Some(35.0);
        rsi[9] = Some(45.0);
        assert_eq!(rsi_regime_allows(true, &rsi, 9, 2, 50.0), Err("rsi_regime"));

        // Flat, missing values, and insufficient history all reject.
        let flat = vec![Some(60.0); 10];
        assert_eq!(
            rsi_regime_allows(true, &flat, 9, 2, 50.0),
            Err("rsi_regime")
        );
        let mut missing = rsi.clone();
        missing[9] = None;
        assert_eq!(
            rsi_regime_allows(true, &missing, 9, 2, 50.0),
            Err("rsi_regime")
        );
        assert_eq!(rsi_regime_allows(true, &rsi, 1, 2, 50.0), Err("rsi_regime"));
        assert_eq!(rsi_regime_allows(true, &rsi, 9, 0, 50.0), Err("rsi_regime"));
    }

    #[test]
    fn rsi_regime_cross_detects_crossings_inside_the_window() {
        let mut rsi = vec![Some(50.0); 12];
        rsi[6] = Some(48.0);
        rsi[7] = Some(52.0);

        // The 6->7 up-cross is inside the 5-candle window ending at 10 or 11.
        assert!(rsi_regime_cross_allows(true, &rsi, 10, 5, 50.0).is_ok());
        assert!(rsi_regime_cross_allows(true, &rsi, 11, 5, 50.0).is_ok());

        // A shorter window ending at 10 starts at 7 and misses it.
        assert_eq!(
            rsi_regime_cross_allows(true, &rsi, 10, 3, 50.0),
            Err("rsi_regime_cross")
        );

        // A touch at exactly 50 followed by a move above also counts as a BUY cross.
        let mut touch = vec![Some(50.0); 12];
        touch[10] = Some(51.0);
        assert!(rsi_regime_cross_allows(true, &touch, 11, 5, 50.0).is_ok());

        // SELL cross is the mirror image (values above 50 avoid a BUY up-cross).
        let mut down = vec![Some(50.0); 12];
        down[6] = Some(51.0);
        down[7] = Some(51.0);
        down[8] = Some(52.0);
        down[9] = Some(48.0);
        assert!(rsi_regime_cross_allows(false, &down, 11, 5, 50.0).is_ok());
        assert_eq!(
            rsi_regime_cross_allows(true, &down, 11, 5, 50.0),
            Err("rsi_regime_cross")
        );

        // An older cross falls outside the window; a longer lookback still sees it.
        let mut old_cross = vec![Some(50.0); 12];
        old_cross[2] = Some(48.0);
        old_cross[3] = Some(52.0);
        assert_eq!(
            rsi_regime_cross_allows(true, &old_cross, 11, 5, 50.0),
            Err("rsi_regime_cross")
        );
        assert!(rsi_regime_cross_allows(true, &old_cross, 11, 10, 50.0).is_ok());

        // Insufficient history and a zero lookback reject.
        assert_eq!(
            rsi_regime_cross_allows(true, &rsi, 4, 5, 50.0),
            Err("rsi_regime_cross")
        );
        assert_eq!(
            rsi_regime_cross_allows(true, &rsi, 11, 0, 50.0),
            Err("rsi_regime_cross")
        );
    }

    #[test]
    fn rsi_slope_requires_minimum_delta_in_trade_direction() {
        let mut rsi = vec![Some(50.0); 10];
        rsi[6] = Some(50.0);
        rsi[9] = Some(52.0);

        assert!(rsi_slope_allows(true, &rsi, 9, 3, 2.0).is_ok());
        assert_eq!(rsi_slope_allows(false, &rsi, 9, 3, 2.0), Err("rsi_slope"));

        rsi[9] = Some(51.9);
        assert_eq!(rsi_slope_allows(true, &rsi, 9, 3, 2.0), Err("rsi_slope"));

        rsi[6] = Some(52.0);
        rsi[9] = Some(50.0);
        assert!(rsi_slope_allows(false, &rsi, 9, 3, 2.0).is_ok());
        assert_eq!(rsi_slope_allows(true, &rsi, 9, 3, 2.0), Err("rsi_slope"));

        // Insufficient history, missing values, and zero lookback reject.
        assert_eq!(rsi_slope_allows(true, &rsi, 2, 3, 2.0), Err("rsi_slope"));
        assert_eq!(rsi_slope_allows(true, &rsi, 9, 0, 2.0), Err("rsi_slope"));
        let mut missing = rsi.clone();
        missing[6] = None;
        assert_eq!(
            rsi_slope_allows(true, &missing, 9, 3, 2.0),
            Err("rsi_slope")
        );
    }

    #[test]
    fn phase_r1_variant_names_and_plans_round_trip() {
        let variants = [
            (Variant::RsiRecentExtreme10, "rsi_recent_extreme_10", false),
            (Variant::RsiRecentExtreme20, "rsi_recent_extreme_20", false),
            (
                Variant::RsiRecentExtreme10_6535,
                "rsi_recent_extreme_10_65_35",
                false,
            ),
            (
                Variant::RsiRecentExtreme20_6535,
                "rsi_recent_extreme_20_65_35",
                false,
            ),
            (
                Variant::RsiRecentExtreme20OrDivergence,
                "rsi_recent_extreme_20_or_divergence",
                false,
            ),
            (Variant::RsiRegime50, "rsi_regime_50", false),
            (Variant::RsiRegimeCross50, "rsi_regime_cross_50", false),
            (Variant::RsiSlope3, "rsi_slope_3", false),
            (
                Variant::HtfH4D1RsiRecentExtreme10,
                "htf_h4_d1_rsi_recent_extreme_10",
                true,
            ),
            (
                Variant::HtfH4D1RsiRecentExtreme20,
                "htf_h4_d1_rsi_recent_extreme_20",
                true,
            ),
            (
                Variant::HtfH4D1RsiRecentExtreme20_6535,
                "htf_h4_d1_rsi_recent_extreme_20_65_35",
                true,
            ),
            (
                Variant::HtfH4D1RsiRecentExtreme20OrDivergence,
                "htf_h4_d1_rsi_recent_extreme_20_or_divergence",
                true,
            ),
            (Variant::HtfH4D1RsiRegime50, "htf_h4_d1_rsi_regime_50", true),
            (
                Variant::HtfH4D1RsiRegimeCross50,
                "htf_h4_d1_rsi_regime_cross_50",
                true,
            ),
            (Variant::HtfH4D1RsiSlope3, "htf_h4_d1_rsi_slope_3", true),
        ];

        for (variant, name, is_htf) in variants {
            assert_eq!(variant.name(), name);
            assert_eq!(Variant::parse(name), Some(variant));
            let plan = variant_plan(variant);
            assert!(matches!(plan.levels, LevelSource::Analyzer));
            assert!(matches!(plan.simulation, SimKind::Standard));
            assert!(plan.delta.is_none());
            assert!(plan.volume.is_none());
            assert_eq!(plan.htf, if is_htf { Some(HtfFilter::H4D1) } else { None });
        }

        assert_eq!(
            variant_plan(Variant::RsiRecentExtreme10).rsi,
            Some(RsiGate::RecentExtreme {
                overbought: 70.0,
                oversold: 30.0,
                lookback: 10,
            })
        );
        assert_eq!(
            variant_plan(Variant::RsiRecentExtreme20).rsi,
            Some(RsiGate::RecentExtreme {
                overbought: 70.0,
                oversold: 30.0,
                lookback: 20,
            })
        );
        assert_eq!(
            variant_plan(Variant::RsiRecentExtreme10_6535).rsi,
            Some(RsiGate::RecentExtreme {
                overbought: 65.0,
                oversold: 35.0,
                lookback: 10,
            })
        );
        assert_eq!(
            variant_plan(Variant::HtfH4D1RsiRecentExtreme20_6535).rsi,
            Some(RsiGate::RecentExtreme {
                overbought: 65.0,
                oversold: 35.0,
                lookback: 20,
            })
        );
        assert_eq!(
            variant_plan(Variant::HtfH4D1RsiRecentExtreme20OrDivergence).rsi,
            Some(RsiGate::RecentExtremeOrDivergence {
                overbought: 70.0,
                oversold: 30.0,
                lookback: 20,
            })
        );
        assert_eq!(
            variant_plan(Variant::RsiRegime50).rsi,
            Some(RsiGate::Regime {
                lookback: 3,
                threshold: 50.0,
            })
        );
        assert_eq!(
            variant_plan(Variant::HtfH4D1RsiRegimeCross50).rsi,
            Some(RsiGate::RegimeCross {
                lookback: 5,
                threshold: 50.0,
            })
        );
        assert_eq!(
            variant_plan(Variant::RsiSlope3).rsi,
            Some(RsiGate::Slope {
                lookback: 3,
                min_delta: 2.0,
            })
        );
    }

    #[test]
    fn run_variant_records_phase_r1_rsi_rejection_reasons() {
        let start = utc_ms(2024, 1, 10, 15, 0);
        let candles: Vec<Candle> = (0..40)
            .map(|index| flow_candle(start + index as i64 * STEP_MS, 100.0, 50.0))
            .collect();
        let neutral_rsi = vec![Some(50.0); candles.len()];
        let htf = HtfContext::new(&candles);
        let context = BacktestContext {
            candles: &candles,
            h1: &[],
            h4: &[],
            htf: &htf,
            cvd: &[],
            rsi: &neutral_rsi,
            split_timestamp: start,
            early_be: 0.0,
            cost_usd: 0.0,
            step_ms: STEP_MS,
            walk_forward: &[],
            rolling: &[],
        };
        let raw = raw_signal(25, SignalType::Buy, 100.0, 95.0, 110.0, 115.0);
        let signals = std::slice::from_ref(&raw);

        let recent = run_variant(Variant::RsiRecentExtreme10, signals, &context);
        assert_eq!(recent.trades, 0);
        assert_eq!(recent.reject_reasons.get("rsi_recent_extreme"), Some(&1));

        let regime = run_variant(Variant::RsiRegime50, signals, &context);
        assert_eq!(regime.trades, 0);
        assert_eq!(regime.reject_reasons.get("rsi_regime"), Some(&1));

        let cross = run_variant(Variant::RsiRegimeCross50, signals, &context);
        assert_eq!(cross.trades, 0);
        assert_eq!(cross.reject_reasons.get("rsi_regime_cross"), Some(&1));

        let slope = run_variant(Variant::RsiSlope3, signals, &context);
        assert_eq!(slope.trades, 0);
        assert_eq!(slope.reject_reasons.get("rsi_slope"), Some(&1));

        let or_gate = run_variant(Variant::RsiRecentExtreme20OrDivergence, signals, &context);
        assert_eq!(or_gate.trades, 0);
        assert_eq!(or_gate.reject_reasons.get("rsi_recent_extreme"), Some(&1));

        // A recent oversold touch inside the lookback unlocks the trade.
        let mut touched_rsi = vec![Some(50.0); candles.len()];
        touched_rsi[20] = Some(25.0);
        let touched_context = BacktestContext {
            candles: &candles,
            h1: &[],
            h4: &[],
            htf: &htf,
            cvd: &[],
            rsi: &touched_rsi,
            split_timestamp: start,
            early_be: 0.0,
            cost_usd: 0.0,
            step_ms: STEP_MS,
            walk_forward: &[],
            rolling: &[],
        };
        let recent = run_variant(Variant::RsiRecentExtreme10, signals, &touched_context);
        assert_eq!(recent.trades, 1);
        assert!(recent.reject_reasons.is_empty());
    }

    #[test]
    fn volume_mode_accepts_surge_or_exhaustion_only() {
        let start = utc_ms(2024, 1, 10, 15, 0);
        let base: Vec<Candle> = (0..30)
            .map(|index| range_candle(start + index as i64 * STEP_MS, 100.0, 100.0, 1.0))
            .collect();

        // Low volume and a small range: exhaustion passes, surge does not.
        let mut exhaustion = base.clone();
        exhaustion[29] = range_candle(start + 29 * STEP_MS, 100.0, 50.0, 0.5);
        assert!(volume_filter_allows(VolumeFilter::Exhaustion, &exhaustion, 29).is_ok());
        assert!(volume_filter_allows(VolumeFilter::Either, &exhaustion, 29).is_ok());

        // Surge passes the OR gate even with a wide range; exhaustion alone fails.
        let mut surge = base.clone();
        surge[29] = range_candle(start + 29 * STEP_MS, 100.0, 200.0, 2.0);
        assert!(volume_filter_allows(VolumeFilter::Either, &surge, 29).is_ok());
        assert_eq!(
            volume_filter_allows(VolumeFilter::Exhaustion, &surge, 29),
            Err("volume_exhaustion")
        );

        // Average volume with an average range passes neither gate.
        assert_eq!(
            volume_filter_allows(VolumeFilter::Exhaustion, &base, 29),
            Err("volume_exhaustion")
        );
        assert_eq!(
            volume_filter_allows(VolumeFilter::Either, &base, 29),
            Err("volume_either")
        );

        // Not enough history for MA20 rejects both modes.
        assert_eq!(
            volume_filter_allows(VolumeFilter::Either, &base, 10),
            Err("volume_either")
        );
    }

    #[test]
    fn run_variant_records_rsi_and_volume_rejection_reasons() {
        let start = utc_ms(2024, 1, 10, 15, 0);
        let candles: Vec<Candle> = (0..40)
            .map(|index| flow_candle(start + index as i64 * STEP_MS, 100.0, 50.0))
            .collect();
        let rsi = vec![Some(50.0); candles.len()];
        let htf = HtfContext::new(&candles);
        let context = BacktestContext {
            candles: &candles,
            h1: &[],
            h4: &[],
            htf: &htf,
            cvd: &[],
            rsi: &rsi,
            split_timestamp: start,
            early_be: 0.0,
            cost_usd: 0.0,
            step_ms: STEP_MS,
            walk_forward: &[],
            rolling: &[],
        };
        let raw = raw_signal(25, SignalType::Buy, 100.0, 95.0, 110.0, 115.0);
        let signals = std::slice::from_ref(&raw);

        let extreme = run_variant(Variant::RsiExtreme, signals, &context);
        assert_eq!(extreme.trades, 0);
        assert_eq!(extreme.reject_reasons.get("rsi_extreme"), Some(&1));

        let volume_exhaustion = run_variant(Variant::VolumeExhaustion, signals, &context);
        assert_eq!(volume_exhaustion.trades, 0);
        assert_eq!(
            volume_exhaustion.reject_reasons.get("volume_exhaustion"),
            Some(&1)
        );

        let volume_either = run_variant(Variant::VolumeEither, signals, &context);
        assert_eq!(volume_either.trades, 0);
        assert_eq!(volume_either.reject_reasons.get("volume_either"), Some(&1));

        // RSI is evaluated before volume for combined variants.
        let combined = run_variant(Variant::RsiDivergenceVolumeExhaustion, signals, &context);
        assert_eq!(combined.trades, 0);
        assert_eq!(combined.reject_reasons.get("rsi_divergence"), Some(&1));

        // With RSI extreme passing, the combined H4/D1 variant fails at volume.
        let mut extreme_rsi = vec![Some(50.0); candles.len()];
        extreme_rsi[25] = Some(25.0);
        let combined_context = BacktestContext {
            candles: &candles,
            h1: &[],
            h4: &[],
            htf: &htf,
            cvd: &[],
            rsi: &extreme_rsi,
            split_timestamp: start,
            early_be: 0.0,
            cost_usd: 0.0,
            step_ms: STEP_MS,
            walk_forward: &[],
            rolling: &[],
        };
        let combined = run_variant(Variant::HtfH4D1RsiVolumeEither, signals, &combined_context);
        assert_eq!(combined.trades, 0);
        assert_eq!(combined.reject_reasons.get("volume_either"), Some(&1));
    }
}
