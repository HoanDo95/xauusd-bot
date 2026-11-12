//! Read-only Macro & Flow dashboard data.
//!
//! Sources are deliberately free and keyless:
//! - FRED CSV download (`fredgraph.csv`) for rates, real yield, curve, USD and MMF.
//! - Yahoo Finance chart API for DXY, gold, ETFs, SPY, VIX and GVZ.
//! - CFTC legacy futures-only report for weekly COMEX gold positioning.
//!
//! Nothing in this module feeds the signal scanner, filters, quota, Telegram or
//! auto-trading. It only builds a snapshot for the operator dashboard.

use crate::api::AppState;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;
use tokio::task::JoinSet;
use tracing::info;

const FRED_USER_AGENT: &str = "xauusd-bot/1.0";
const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36";
const FETCH_INTERVAL_SECS: u64 = 15 * 60;
const FRED_SERIES: [&str; 7] = [
    "DGS10",
    "DGS2",
    "DFII10",
    "T10Y2Y",
    "DTWEXBGS",
    "VIXCLS",
    "MMMFFAQ027S",
];
const YAHOO_SYMBOLS: [(&str, &str); 8] = [
    ("dxy", "DX-Y.NYB"),
    ("gold", "GC=F"),
    ("gld", "GLD"),
    ("iau", "IAU"),
    ("tlt", "TLT"),
    ("spy", "SPY"),
    ("vix", "^VIX"),
    ("gvz", "^GVZ"),
];

type YahooSeries = (Vec<i64>, Vec<f64>, Vec<Option<f64>>);

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MacroMetric {
    pub key: String,
    pub label: String,
    pub value: Option<f64>,
    pub unit: String,
    pub change_1d: Option<f64>,
    pub change_5d: Option<f64>,
    pub change_5d_pct: Option<f64>,
    pub volume_ratio_20d: Option<f64>,
    pub date: Option<String>,
    pub source: String,
    pub note: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CotSnapshot {
    pub report_date: String,
    pub open_interest: f64,
    pub noncommercial_long: f64,
    pub noncommercial_short: f64,
    pub noncommercial_net: f64,
    pub noncommercial_net_change: f64,
    pub net_pct_oi: f64,
    pub open_interest_change: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MacroFlowScorecard {
    pub macro_bias: String,
    pub confidence: u8,
    pub score: i32,
    pub direction: String,
    pub driver: String,
    pub flow_quality: String,
    pub risk_regime: String,
    pub rate_regime: String,
    pub usd_regime: String,
    pub vol_regime: String,
    pub notes: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MacroFlowSnapshot {
    pub updated_at: String,
    pub fred_ok: bool,
    pub yahoo_ok: bool,
    pub cot_ok: bool,
    pub metrics: Vec<MacroMetric>,
    pub cot: Option<CotSnapshot>,
    pub scorecard: MacroFlowScorecard,
    pub warnings: Vec<String>,
    pub sources: Vec<String>,
}

/// Trả về (last, change 1 observation, change 5 observations, % change 5 observations).
///
/// "5 observations" là giá trị cách hiện tại 5 phiên (cần ít nhất 6 điểm dữ liệu),
/// tương đương cách tính 5d change thông thường của thị trường.
pub fn compute_changes(
    values: &[(String, f64)],
) -> (Option<f64>, Option<f64>, Option<f64>, Option<f64>) {
    let valid: Vec<f64> = values
        .iter()
        .map(|(_, value)| *value)
        .filter(|value| value.is_finite())
        .collect();

    let Some(&last) = valid.last() else {
        return (None, None, None, None);
    };

    let change_1d = if valid.len() >= 2 {
        Some(last - valid[valid.len() - 2])
    } else {
        None
    };

    let (change_5d, change_5d_pct) = if valid.len() >= 6 {
        let base = valid[valid.len() - 6];
        let change = last - base;
        let pct = if base != 0.0 && base.is_finite() {
            Some(change / base * 100.0)
        } else {
            None
        };
        (Some(change), pct)
    } else {
        (None, None)
    };

    (Some(last), change_1d, change_5d, change_5d_pct)
}

/// Parse FRED CSV (`observation_date,SERIES`) và bỏ các dòng thiếu dữ liệu ".".
pub fn parse_fred_csv(body: &str) -> Vec<(String, f64)> {
    body.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let (date, raw_value) = line.split_once(',')?;
            let date = date.trim();
            let raw_value = raw_value.trim().trim_matches('"');
            if date.is_empty() || raw_value.is_empty() || raw_value == "." {
                return None;
            }
            let value = raw_value.parse::<f64>().ok()?;
            if !value.is_finite() {
                return None;
            }
            Some((date.to_string(), value))
        })
        .collect()
}

/// Parse Yahoo chart JSON thành (timestamps, closes, volumes).
///
/// Các điểm thiếu close bị bỏ qua; volume giữ nguyên vị trí tương ứng và có thể None.
pub fn parse_yahoo_chart(payload: &Value) -> Option<YahooSeries> {
    let result = payload.get("chart")?.get("result")?.as_array()?.first()?;
    let timestamps = result.get("timestamp")?.as_array()?;
    let quote = result
        .get("indicators")?
        .get("quote")?
        .as_array()?
        .first()?;
    let closes = quote.get("close")?.as_array()?;
    let volumes = quote.get("volume").and_then(Value::as_array);

    let mut parsed_timestamps = Vec::new();
    let mut parsed_closes = Vec::new();
    let mut parsed_volumes = Vec::new();

    for (index, timestamp) in timestamps.iter().enumerate() {
        let Some(timestamp) = timestamp.as_i64() else {
            continue;
        };
        let Some(close) = closes.get(index).and_then(Value::as_f64) else {
            continue;
        };
        if !close.is_finite() {
            continue;
        }
        let volume = volumes
            .and_then(|values| values.get(index))
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite() && *value >= 0.0);
        parsed_timestamps.push(timestamp);
        parsed_closes.push(close);
        parsed_volumes.push(volume);
    }

    if parsed_closes.is_empty() {
        None
    } else {
        Some((parsed_timestamps, parsed_closes, parsed_volumes))
    }
}

/// Parse dòng `"GOLD - COMMODITY EXCHANGE INC."` trong CFTC `deafut.txt`.
///
/// Layout legacy futures-only (0-based):
/// 0 name, 2 report date, 7 open interest, 8/9 non-commercial long/short,
/// 37/38/39 open-interest change and non-commercial long/short changes.
pub fn parse_cot_gold_csv(body: &str) -> Option<CotSnapshot> {
    let line = body.lines().find(|line| {
        line.trim_start()
            .starts_with("\"GOLD - COMMODITY EXCHANGE INC.\"")
    })?;
    let fields: Vec<&str> = line.split(',').map(str::trim).collect();
    let field = |index: usize| {
        fields
            .get(index)
            .copied()
            .unwrap_or("")
            .trim_matches('"')
            .trim()
    };

    let report_date = field(2);
    if report_date.is_empty() {
        return None;
    }

    let open_interest = field(7).parse::<f64>().ok()?;
    let noncommercial_long = field(8).parse::<f64>().ok()?;
    let noncommercial_short = field(9).parse::<f64>().ok()?;
    let open_interest_change = field(37).parse::<f64>().ok()?;
    let long_change = field(38).parse::<f64>().ok()?;
    let short_change = field(39).parse::<f64>().ok()?;

    if !open_interest.is_finite()
        || open_interest <= 0.0
        || !noncommercial_long.is_finite()
        || !noncommercial_short.is_finite()
        || !open_interest_change.is_finite()
        || !long_change.is_finite()
        || !short_change.is_finite()
    {
        return None;
    }

    let noncommercial_net = noncommercial_long - noncommercial_short;
    let noncommercial_net_change = long_change - short_change;

    Some(CotSnapshot {
        report_date: report_date.to_string(),
        open_interest,
        noncommercial_long,
        noncommercial_short,
        noncommercial_net,
        noncommercial_net_change,
        net_pct_oi: noncommercial_net / open_interest * 100.0,
        open_interest_change,
    })
}

fn metric_by_key<'a>(metrics: &'a [MacroMetric], key: &str) -> Option<&'a MacroMetric> {
    metrics.iter().find(|metric| metric.key == key)
}

fn sign(value: f64) -> i32 {
    if value > 0.0 {
        1
    } else if value < 0.0 {
        -1
    } else {
        0
    }
}

fn non_zero_sign(value: Option<f64>) -> Option<i32> {
    value.map(sign).filter(|value| *value != 0)
}

/// Chấm điểm minh bạch từ các metric đang có; không dùng dữ liệu thiếu.
pub fn score_macro_flow(metrics: &[MacroMetric], cot: Option<&CotSnapshot>) -> MacroFlowScorecard {
    let mut score = 0_i32;
    let mut available_inputs = 0_usize;
    let mut contributions: Vec<(i32, String)> = Vec::new();

    let real_yield_change = metric_by_key(metrics, "real_yield_10y").and_then(|m| m.change_5d);
    if let Some(change) = real_yield_change {
        available_inputs += 1;
        if change <= -0.02 {
            score += 1;
            contributions.push((1, "real yield ↓".to_string()));
        } else if change >= 0.02 {
            score -= 1;
            contributions.push((-1, "real yield ↑".to_string()));
        }
    }

    let dxy_change_pct = metric_by_key(metrics, "dxy").and_then(|m| m.change_5d_pct);
    if let Some(change) = dxy_change_pct {
        available_inputs += 1;
        if change <= -0.2 {
            score += 1;
            contributions.push((1, "DXY ↓".to_string()));
        } else if change >= 0.2 {
            score -= 1;
            contributions.push((-1, "DXY ↑".to_string()));
        }
    }

    let gold_change_pct = metric_by_key(metrics, "gold").and_then(|m| m.change_5d_pct);
    if let Some(change) = gold_change_pct {
        available_inputs += 1;
        if change >= 0.5 {
            score += 1;
            contributions.push((1, "gold ↑".to_string()));
        } else if change <= -0.5 {
            score -= 1;
            contributions.push((-1, "gold ↓".to_string()));
        }
    }

    let gld_change_pct = metric_by_key(metrics, "gld").and_then(|m| m.change_5d_pct);
    let iau_change_pct = metric_by_key(metrics, "iau").and_then(|m| m.change_5d_pct);
    let etf_change_pct = match (gld_change_pct, iau_change_pct) {
        (Some(gld), Some(iau)) => Some((gld + iau) / 2.0),
        (Some(gld), None) => Some(gld),
        (None, Some(iau)) => Some(iau),
        (None, None) => None,
    };
    if let Some(change) = etf_change_pct {
        available_inputs += 1;
        if change >= 0.5 {
            score += 1;
            contributions.push((1, "ETF ↑".to_string()));
        } else if change <= -0.5 {
            score -= 1;
            contributions.push((-1, "ETF ↓".to_string()));
        }
    }

    let cot_net_change = cot.map(|snapshot| snapshot.noncommercial_net_change);
    if let Some(snapshot) = cot {
        available_inputs += 1;
        let mut cot_score = 0_i32;
        if snapshot.noncommercial_net > 0.0 {
            cot_score += 1;
        } else if snapshot.noncommercial_net < 0.0 {
            cot_score -= 1;
        }
        if snapshot.noncommercial_net_change > 0.0 {
            cot_score += 1;
        } else if snapshot.noncommercial_net_change < 0.0 {
            cot_score -= 1;
        }
        cot_score = cot_score.clamp(-2, 2);
        score += cot_score;
        if cot_score > 0 {
            contributions.push((cot_score, "COT ↑".to_string()));
        } else if cot_score < 0 {
            contributions.push((cot_score, "COT ↓".to_string()));
        }
    }

    let insufficient = available_inputs < 3;
    let (macro_bias, direction) = if insufficient {
        ("NEUTRAL", "MIXED")
    } else if score >= 2 {
        ("BULLISH", "UP")
    } else if score <= -2 {
        ("BEARISH", "DOWN")
    } else {
        ("NEUTRAL", "MIXED")
    };

    let mut confidence = 40 + 10 * score.abs() + 5 * available_inputs as i32;
    if insufficient {
        confidence = confidence.min(40);
    }
    let confidence = confidence.clamp(0, 100) as u8;

    let gold_sign = non_zero_sign(gold_change_pct);
    let etf_sign = non_zero_sign(etf_change_pct);
    let cot_sign = non_zero_sign(cot_net_change);
    let flow_quality = if insufficient {
        "INSUFFICIENT"
    } else {
        let diverging = (gold_sign.is_some() && etf_sign.is_some() && gold_sign != etf_sign)
            || (gold_sign.is_some() && cot_sign.is_some() && gold_sign != cot_sign)
            || (etf_sign.is_some() && cot_sign.is_some() && etf_sign != cot_sign);
        if diverging {
            "DIVERGING"
        } else if gold_sign.is_some() && etf_sign.is_some() && cot_sign.is_some() {
            "CONFIRMING"
        } else {
            "MIXED"
        }
    };

    let vix = metric_by_key(metrics, "vix")
        .and_then(|m| m.value)
        .or_else(|| metric_by_key(metrics, "vix_fred").and_then(|m| m.value));
    let gvz = metric_by_key(metrics, "gvz").and_then(|m| m.value);
    let spy_change_pct = metric_by_key(metrics, "spy").and_then(|m| m.change_5d_pct);

    let risk_regime = if vix.is_some_and(|value| value > 20.0)
        || spy_change_pct.is_some_and(|value| value <= -1.0)
    {
        "RISK_OFF"
    } else if vix.is_some_and(|value| value < 15.0)
        && spy_change_pct.is_some_and(|value| value >= 0.0)
    {
        "RISK_ON"
    } else {
        "NEUTRAL"
    };

    let rate_regime = match real_yield_change {
        Some(change) if change <= -0.02 => "EASING",
        Some(change) if change >= 0.02 => "TIGHTENING",
        _ => "NEUTRAL",
    };

    let usd_regime = match dxy_change_pct {
        Some(change) if change <= -0.2 => "WEAK",
        Some(change) if change >= 0.2 => "STRONG",
        _ => "NEUTRAL",
    };

    let vol_regime =
        if gvz.is_some_and(|value| value > 25.0) || vix.is_some_and(|value| value > 25.0) {
            "HIGH"
        } else if gvz.is_some_and(|value| value < 15.0) && vix.is_some_and(|value| value < 15.0) {
            "LOW"
        } else {
            "NORMAL"
        };

    contributions.sort_by_key(|(contribution, _)| std::cmp::Reverse(contribution.abs()));
    let driver = if contributions.is_empty() {
        "không có tín hiệu rõ".to_string()
    } else {
        contributions
            .iter()
            .take(2)
            .map(|(_, label)| label.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };

    let mut notes = vec![
        format!(
            "Điểm macro/flow = {} từ {} nhóm dữ liệu khả dụng; bias {} (confidence {}%).",
            score, available_inputs, macro_bias, confidence
        ),
        "ETF flow chỉ là proxy giá/khối lượng từ GLD/IAU — không phải holdings thật; cần dữ liệu issuer để xác nhận.".to_string(),
        "COT là dữ liệu tuần của CFTC: công bố thứ Sáu, số liệu tính đến thứ Ba — có độ trễ so với giá.".to_string(),
        "Skew quyền chọn chỉ được đại diện bởi GVZ/VIX; full options skew không khả dụng từ nguồn miễn phí.".to_string(),
        "Đây là bối cảnh macro/flow, không phải tín hiệu entry.".to_string(),
    ];
    if insufficient {
        notes.insert(
            0,
            "Chưa đủ 3 nhóm dữ liệu để kết luận — chỉ hiển thị tham khảo.".to_string(),
        );
    }

    MacroFlowScorecard {
        macro_bias: macro_bias.to_string(),
        confidence,
        score,
        direction: direction.to_string(),
        driver,
        flow_quality: flow_quality.to_string(),
        risk_regime: risk_regime.to_string(),
        rate_regime: rate_regime.to_string(),
        usd_regime: usd_regime.to_string(),
        vol_regime: vol_regime.to_string(),
        notes,
    }
}

fn yahoo_date(timestamp: i64) -> String {
    DateTime::<Utc>::from_timestamp(timestamp, 0)
        .map(|date| date.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| timestamp.to_string())
}

fn volume_ratio_20d(volumes: &[Option<f64>]) -> Option<f64> {
    let last = volumes.last().copied().flatten()?;
    if !last.is_finite() || last <= 0.0 {
        return None;
    }
    let prior: Vec<f64> = volumes
        .iter()
        .rev()
        .skip(1)
        .take(20)
        .filter_map(|volume| *volume)
        .filter(|volume| volume.is_finite() && *volume > 0.0)
        .collect();
    if prior.is_empty() {
        return None;
    }
    let average = prior.iter().sum::<f64>() / prior.len() as f64;
    (average > 0.0).then_some(last / average)
}

fn metric_from_fred(
    key: &str,
    label: &str,
    unit: &str,
    series: &str,
    note: &str,
    values: &[(String, f64)],
) -> MacroMetric {
    let (value, change_1d, change_5d, change_5d_pct) = compute_changes(values);
    MacroMetric {
        key: key.to_string(),
        label: label.to_string(),
        value,
        unit: unit.to_string(),
        change_1d,
        change_5d,
        change_5d_pct,
        volume_ratio_20d: None,
        date: values.last().map(|(date, _)| date.clone()),
        source: format!("FRED:{}", series),
        note: note.to_string(),
    }
}

fn metric_from_yahoo(
    key: &str,
    label: &str,
    unit: &str,
    symbol: &str,
    note: &str,
    series: Option<&YahooSeries>,
) -> MacroMetric {
    let values: Vec<(String, f64)> = series
        .map(|(timestamps, closes, _)| {
            timestamps
                .iter()
                .zip(closes.iter())
                .map(|(timestamp, close)| (yahoo_date(*timestamp), *close))
                .collect()
        })
        .unwrap_or_default();
    let (value, change_1d, change_5d, change_5d_pct) = compute_changes(&values);
    MacroMetric {
        key: key.to_string(),
        label: label.to_string(),
        value,
        unit: unit.to_string(),
        change_1d,
        change_5d,
        change_5d_pct,
        volume_ratio_20d: series.and_then(|(_, _, volumes)| volume_ratio_20d(volumes)),
        date: values.last().map(|(date, _)| date.clone()),
        source: format!("YAHOO:{}", symbol),
        note: note.to_string(),
    }
}

fn encode_yahoo_symbol(symbol: &str) -> String {
    symbol
        .replace('%', "%25")
        .replace('^', "%5E")
        .replace('=', "%3D")
}

async fn fetch_fred_series(
    client: Client,
    series: &'static str,
) -> (&'static str, Result<Vec<(String, f64)>, String>) {
    let url = format!(
        "https://fred.stlouisfed.org/graph/fredgraph.csv?id={}",
        series
    );
    let result = async {
        let response = client
            .get(&url)
            .header(reqwest::header::USER_AGENT, FRED_USER_AGENT)
            .send()
            .await
            .map_err(|err| format!("request: {}", err.without_url()))?;
        if !response.status().is_success() {
            return Err(format!("HTTP {}", response.status()));
        }
        let body = response
            .text()
            .await
            .map_err(|err| format!("body: {}", err.without_url()))?;
        let values = parse_fred_csv(&body);
        if values.is_empty() {
            return Err("no data".to_string());
        }
        Ok(values)
    }
    .await;
    (series, result)
}

async fn fetch_fred_group(
    client: &Client,
) -> (HashMap<&'static str, Vec<(String, f64)>>, Vec<String>) {
    let mut set = JoinSet::new();
    for series in FRED_SERIES {
        set.spawn(fetch_fred_series(client.clone(), series));
    }

    let mut data = HashMap::new();
    let mut warnings = Vec::new();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok((series, Ok(values))) => {
                data.insert(series, values);
            }
            Ok((series, Err(message))) => {
                warnings.push(format!("FRED {}: {}", series, message));
            }
            Err(err) => warnings.push(format!("FRED task: {}", err)),
        }
    }
    (data, warnings)
}

async fn fetch_yahoo_series(
    client: Client,
    key: &'static str,
    symbol: &'static str,
) -> (&'static str, Result<YahooSeries, String>) {
    let url = format!(
        "https://query1.finance.yahoo.com/v8/finance/chart/{}?range=3mo&interval=1d",
        encode_yahoo_symbol(symbol)
    );
    let result = async {
        let response = client
            .get(&url)
            .header(reqwest::header::USER_AGENT, BROWSER_USER_AGENT)
            .header(reqwest::header::ACCEPT, "application/json,text/plain,*/*")
            .send()
            .await
            .map_err(|err| format!("request: {}", err.without_url()))?;
        if !response.status().is_success() {
            return Err(format!("HTTP {}", response.status()));
        }
        let payload: Value = response
            .json()
            .await
            .map_err(|err| format!("json: {}", err.without_url()))?;
        parse_yahoo_chart(&payload).ok_or_else(|| "no chart data".to_string())
    }
    .await;
    (key, result)
}

async fn fetch_yahoo_group(client: &Client) -> (HashMap<&'static str, YahooSeries>, Vec<String>) {
    let mut set = JoinSet::new();
    for (key, symbol) in YAHOO_SYMBOLS {
        set.spawn(fetch_yahoo_series(client.clone(), key, symbol));
    }

    let mut data = HashMap::new();
    let mut warnings = Vec::new();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok((key, Ok(series))) => {
                data.insert(key, series);
            }
            Ok((key, Err(message))) => {
                warnings.push(format!("YAHOO {}: {}", key, message));
            }
            Err(err) => warnings.push(format!("YAHOO task: {}", err)),
        }
    }
    (data, warnings)
}

async fn fetch_cot(client: Client) -> Result<CotSnapshot, String> {
    let url = "https://www.cftc.gov/dea/newcot/deafut.txt";
    let mut last_status = None;

    // Cloudflare occasionally serves a challenge on the first request from a new
    // TLS connection but allows the retry on the pooled connection. Retrying is
    // cheap (at most 3 attempts every 15 minutes) and keeps the dashboard useful.
    for attempt in 0..3 {
        let response = client
            .get(url)
            .header(reqwest::header::USER_AGENT, BROWSER_USER_AGENT)
            .header(
                reqwest::header::ACCEPT,
                "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
            )
            .header(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9")
            .header(
                "sec-ch-ua",
                "\"Chromium\";v=\"124\", \"Google Chrome\";v=\"124\", \"Not-A.Brand\";v=\"99\"",
            )
            .header("sec-ch-ua-mobile", "?0")
            .header("sec-ch-ua-platform", "\"Windows\"")
            .header("sec-fetch-dest", "document")
            .header("sec-fetch-mode", "navigate")
            .header("sec-fetch-site", "none")
            .header("sec-fetch-user", "?1")
            .header("upgrade-insecure-requests", "1")
            .header(
                reqwest::header::REFERER,
                "https://www.cftc.gov/MarketReports/CommitmentsofTraders/index.htm",
            )
            .header(reqwest::header::ACCEPT_ENCODING, "identity")
            .header(reqwest::header::CACHE_CONTROL, "no-cache")
            .send()
            .await
            .map_err(|err| format!("request: {}", err.without_url()))?;

        if response.status().is_success() {
            let body = response
                .text()
                .await
                .map_err(|err| format!("body: {}", err.without_url()))?;
            return parse_cot_gold_csv(&body)
                .ok_or_else(|| "GOLD row not found or malformed".to_string());
        }

        last_status = Some(response.status());
        if attempt < 2 {
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }

    let status = last_status
        .map(|status| status.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    Err(format!("HTTP {}", status))
}

/// Fetch toàn bộ snapshot. Lỗi từng nguồn chỉ thêm warning, không làm hỏng snapshot.
pub async fn fetch_macro_flow(client: &Client) -> MacroFlowSnapshot {
    let (fred_data, mut warnings) = fetch_fred_group(client).await;
    let (yahoo_data, yahoo_warnings) = fetch_yahoo_group(client).await;
    warnings.extend(yahoo_warnings);

    let cot_result = fetch_cot(client.clone()).await;
    let cot = match &cot_result {
        Ok(snapshot) => Some(snapshot.clone()),
        Err(message) => {
            warnings.push(format!("CFTC: {}", message));
            None
        }
    };

    let mut metrics = Vec::new();
    let fred_specs: [(&str, &str, &str, &str, &str); 7] = [
        (
            "real_yield_10y",
            "Real Yield 10Y",
            "%",
            "DFII10",
            "Lợi suất thực 10Y — driver chính của vàng.",
        ),
        (
            "nominal_10y",
            "US 10Y Yield",
            "%",
            "DGS10",
            "Lợi suất danh nghĩa 10Y.",
        ),
        (
            "yield_2y",
            "US 2Y Yield",
            "%",
            "DGS2",
            "Lợi suất danh nghĩa 2Y.",
        ),
        (
            "curve_2s10s",
            "2s10s Curve",
            "%",
            "T10Y2Y",
            "Chênh lệch lợi suất 10Y - 2Y.",
        ),
        (
            "broad_dollar",
            "Broad Dollar (DTWEXBGS)",
            "index",
            "DTWEXBGS",
            "Chỉ số USD thương mại đa tiền tệ của Fed.",
        ),
        (
            "vix_fred",
            "VIX (FRED)",
            "index",
            "VIXCLS",
            "VIX đóng cửa từ FRED (dự phòng cho Yahoo).",
        ),
        (
            "mmf",
            "Money Market Fund Assets (Q)",
            "USD mn",
            "MMMFFAQ027S",
            "Tổng tài sản MMF, dữ liệu quý — độ trễ cao.",
        ),
    ];
    for (key, label, unit, series, note) in fred_specs {
        let empty = Vec::new();
        let values = fred_data.get(series).unwrap_or(&empty);
        metrics.push(metric_from_fred(key, label, unit, series, note, values));
    }

    let yahoo_specs: [(&str, &str, &str, &str, &str); 8] = [
        (
            "dxy",
            "DXY",
            "index",
            "DX-Y.NYB",
            "US Dollar Index — nghịch biến với vàng.",
        ),
        (
            "gold",
            "Gold Futures",
            "USD",
            "GC=F",
            "Giá vàng tương lai front-month.",
        ),
        (
            "gld",
            "GLD ETF",
            "USD",
            "GLD",
            "ETF flow proxy: price/volume, not holdings",
        ),
        (
            "iau",
            "IAU ETF",
            "USD",
            "IAU",
            "ETF flow proxy: price/volume, not holdings",
        ),
        (
            "tlt",
            "TLT (20Y+ Treasuries)",
            "USD",
            "TLT",
            "Trái phiếu dài hạn — proxy lãi suất dài hạn.",
        ),
        (
            "spy",
            "SPY (S&P 500)",
            "USD",
            "SPY",
            "S&P 500 — thước đo risk appetite.",
        ),
        (
            "vix",
            "VIX",
            "index",
            "^VIX",
            "Options skew proxy; full skew unavailable",
        ),
        (
            "gvz",
            "GVZ (Gold VIX)",
            "index",
            "^GVZ",
            "Options skew proxy; full skew unavailable",
        ),
    ];
    for (key, label, unit, symbol, note) in yahoo_specs {
        metrics.push(metric_from_yahoo(
            key,
            label,
            unit,
            symbol,
            note,
            yahoo_data.get(key),
        ));
    }

    if let Some(snapshot) = cot.as_ref() {
        metrics.push(MacroMetric {
            key: "cot_net".to_string(),
            label: "COT Non-commercial Net".to_string(),
            value: Some(snapshot.noncommercial_net),
            unit: "contracts".to_string(),
            change_1d: None,
            change_5d: Some(snapshot.noncommercial_net_change),
            change_5d_pct: None,
            volume_ratio_20d: None,
            date: Some(snapshot.report_date.clone()),
            source: "CFTC".to_string(),
            note: "CFTC weekly, released Friday, data as of Tuesday".to_string(),
        });
    }

    let fred_ok = metrics
        .iter()
        .any(|metric| metric.source.starts_with("FRED:") && metric.value.is_some());
    let yahoo_ok = metrics
        .iter()
        .any(|metric| metric.source.starts_with("YAHOO:") && metric.value.is_some());
    let cot_ok = cot.is_some();

    let scorecard = score_macro_flow(&metrics, cot.as_ref());

    MacroFlowSnapshot {
        updated_at: Utc::now().to_rfc3339(),
        fred_ok,
        yahoo_ok,
        cot_ok,
        metrics,
        cot,
        scorecard,
        warnings,
        sources: vec![
            "FRED: DGS10, DGS2, DFII10, T10Y2Y, DTWEXBGS, VIXCLS, MMMFFAQ027S".to_string(),
            "Yahoo Finance chart API: DX-Y.NYB, GLD, IAU, TLT, SPY, GC=F, ^VIX, ^GVZ".to_string(),
            "CFTC: https://www.cftc.gov/dea/newcot/deafut.txt".to_string(),
        ],
    }
}

/// Worker read-only: fetch ngay lập tức, sau đó làm mới mỗi 15 phút.
pub fn spawn_macro_flow_worker(state: AppState) {
    tokio::spawn(async move {
        let client = Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_else(|_| Client::new());

        info!("🌐 [Macro & Flow] Worker read-only khởi chạy (refresh mỗi 15 phút)...");
        loop {
            let enabled = state.config.read().await.macro_flow_enabled;
            if !enabled {
                tokio::time::sleep(Duration::from_secs(60)).await;
                continue;
            }

            let snapshot = fetch_macro_flow(&client).await;
            info!(
                "📊 [Macro & Flow] bias={} score={} confidence={} flow={} warnings={}",
                snapshot.scorecard.macro_bias,
                snapshot.scorecard.score,
                snapshot.scorecard.confidence,
                snapshot.scorecard.flow_quality,
                snapshot.warnings.len()
            );
            *state.macro_flow.write().await = Some(snapshot);
            *state.macro_flow_updated_at.write().await = Some(Utc::now());

            tokio::time::sleep(Duration::from_secs(FETCH_INTERVAL_SECS)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn metric(
        key: &str,
        value: f64,
        change_5d: Option<f64>,
        change_5d_pct: Option<f64>,
    ) -> MacroMetric {
        MacroMetric {
            key: key.to_string(),
            label: key.to_string(),
            value: Some(value),
            unit: "%".to_string(),
            change_1d: None,
            change_5d,
            change_5d_pct,
            volume_ratio_20d: None,
            date: Some("2026-11-10".to_string()),
            source: "TEST".to_string(),
            note: String::new(),
        }
    }

    fn cot(net: f64, net_change: f64) -> CotSnapshot {
        CotSnapshot {
            report_date: "2026-11-03".to_string(),
            open_interest: 400_000.0,
            noncommercial_long: 250_000.0,
            noncommercial_short: 50_000.0,
            noncommercial_net: net,
            noncommercial_net_change: net_change,
            net_pct_oi: net / 400_000.0 * 100.0,
            open_interest_change: 2_000.0,
        }
    }

    #[test]
    fn compute_changes_returns_last_one_and_five_observation_deltas() {
        let values: Vec<(String, f64)> = [100.0, 101.0, 102.0, 103.0, 104.0, 105.0]
            .iter()
            .enumerate()
            .map(|(index, value)| (format!("2026-11-{:02}", index + 1), *value))
            .collect();

        let (last, change_1d, change_5d, change_5d_pct) = compute_changes(&values);
        assert_eq!(last, Some(105.0));
        assert_eq!(change_1d, Some(1.0));
        assert_eq!(change_5d, Some(5.0));
        assert_eq!(change_5d_pct, Some(5.0));
    }

    #[test]
    fn compute_changes_handles_short_or_empty_series() {
        let empty: Vec<(String, f64)> = Vec::new();
        assert_eq!(compute_changes(&empty), (None, None, None, None));

        let one = vec![("2026-11-01".to_string(), 100.0)];
        assert_eq!(compute_changes(&one), (Some(100.0), None, None, None));

        let five: Vec<(String, f64)> = (1..=5)
            .map(|index| (format!("2026-11-0{}", index), 100.0 + index as f64))
            .collect();
        let (last, change_1d, change_5d, change_5d_pct) = compute_changes(&five);
        assert_eq!(last, Some(105.0));
        assert_eq!(change_1d, Some(1.0));
        assert_eq!(change_5d, None);
        assert_eq!(change_5d_pct, None);
    }

    #[test]
    fn parse_fred_csv_skips_header_and_missing_values() {
        let body = "observation_date,DGS10\n2026-11-07,4.20\n2026-11-08,.\n2026-11-09,4.25\n";
        let values = parse_fred_csv(body);
        assert_eq!(
            values,
            vec![
                ("2026-11-07".to_string(), 4.20),
                ("2026-11-09".to_string(), 4.25)
            ]
        );
    }

    #[test]
    fn parse_yahoo_chart_keeps_valid_points_and_optional_volume() {
        let payload = json!({
            "chart": {
                "result": [{
                    "timestamp": [1_700_000_000_i64, 1_700_086_400_i64, 1_700_172_800_i64],
                    "indicators": {
                        "quote": [{
                            "close": [100.0, null, 102.0],
                            "volume": [10.0, 20.0, null]
                        }]
                    }
                }]
            }
        });

        let (timestamps, closes, volumes) =
            parse_yahoo_chart(&payload).expect("valid chart payload");
        assert_eq!(timestamps, vec![1_700_000_000, 1_700_172_800]);
        assert_eq!(closes, vec![100.0, 102.0]);
        assert_eq!(volumes, vec![Some(10.0), None]);
    }

    #[test]
    fn parse_yahoo_chart_rejects_empty_or_malformed_payloads() {
        assert!(parse_yahoo_chart(&json!({})).is_none());
        assert!(parse_yahoo_chart(&json!({"chart": {"result": []}})).is_none());
        assert!(parse_yahoo_chart(&json!({
            "chart": {"result": [{"timestamp": [], "indicators": {"quote": [{"close": []}]}}]}
        }))
        .is_none());
    }

    #[test]
    fn parse_cot_gold_csv_uses_legacy_field_layout() {
        let mut fields = vec![String::new(); 47];
        fields[0] = "\"GOLD - COMMODITY EXCHANGE INC.\"".to_string();
        fields[1] = "261103".to_string();
        fields[2] = "2026-11-03".to_string();
        fields[3] = "088691".to_string();
        fields[4] = "CMX".to_string();
        fields[5] = "01".to_string();
        fields[6] = "088".to_string();
        fields[7] = "399169".to_string();
        fields[8] = "220530".to_string();
        fields[9] = "40982".to_string();
        fields[37] = "2359".to_string();
        fields[38] = "-3194".to_string();
        fields[39] = "3709".to_string();

        let snapshot = parse_cot_gold_csv(&fields.join(",")).expect("valid gold row");
        assert_eq!(snapshot.report_date, "2026-11-03");
        assert_eq!(snapshot.open_interest, 399_169.0);
        assert_eq!(snapshot.noncommercial_long, 220_530.0);
        assert_eq!(snapshot.noncommercial_short, 40_982.0);
        assert_eq!(snapshot.noncommercial_net, 179_548.0);
        assert_eq!(snapshot.noncommercial_net_change, -6_903.0);
        assert_eq!(snapshot.open_interest_change, 2_359.0);
        assert!((snapshot.net_pct_oi - 44.980_6).abs() < 0.001);
    }

    #[test]
    fn parse_cot_gold_csv_rejects_missing_or_negative_open_interest() {
        assert!(parse_cot_gold_csv("no gold row here").is_none());

        let mut fields = vec!["0".to_string(); 47];
        fields[0] = "\"GOLD - COMMODITY EXCHANGE INC.\"".to_string();
        fields[2] = "2026-11-03".to_string();
        fields[7] = "-1".to_string();
        assert!(parse_cot_gold_csv(&fields.join(",")).is_none());
    }

    #[test]
    fn score_is_bullish_and_confirming_when_all_inputs_align() {
        let metrics = vec![
            metric("real_yield_10y", 2.0, Some(-0.05), Some(-2.0)),
            metric("dxy", 100.0, Some(-0.4), Some(-0.4)),
            metric("gold", 4_000.0, Some(40.0), Some(1.0)),
            metric("gld", 400.0, Some(3.2), Some(0.8)),
            metric("iau", 50.0, Some(0.3), Some(0.6)),
            metric("vix", 16.0, None, None),
            metric("gvz", 18.0, None, None),
            metric("spy", 600.0, Some(3.0), Some(0.5)),
        ];
        let cot = cot(180_000.0, 5_000.0);

        let scorecard = score_macro_flow(&metrics, Some(&cot));
        assert_eq!(scorecard.macro_bias, "BULLISH");
        assert_eq!(scorecard.direction, "UP");
        assert_eq!(scorecard.flow_quality, "CONFIRMING");
        assert_eq!(scorecard.rate_regime, "EASING");
        assert_eq!(scorecard.usd_regime, "WEAK");
        assert_eq!(scorecard.risk_regime, "NEUTRAL");
        assert_eq!(scorecard.vol_regime, "NORMAL");
        assert!(scorecard.score >= 2);
        assert_eq!(scorecard.confidence, 100);
        assert!(scorecard.driver.contains("COT ↑"));
        assert!(scorecard.driver.contains("real yield ↓"));
    }

    #[test]
    fn score_is_bearish_and_confirming_on_mirror_inputs() {
        let metrics = vec![
            metric("real_yield_10y", 2.5, Some(0.05), Some(2.0)),
            metric("dxy", 105.0, Some(0.5), Some(0.5)),
            metric("gold", 3_900.0, Some(-40.0), Some(-1.0)),
            metric("gld", 390.0, Some(-3.2), Some(-0.8)),
            metric("iau", 49.0, Some(-0.3), Some(-0.6)),
            metric("vix", 26.0, None, None),
            metric("gvz", 30.0, None, None),
            metric("spy", 590.0, Some(-9.0), Some(-1.5)),
        ];
        let cot = cot(-180_000.0, -5_000.0);

        let scorecard = score_macro_flow(&metrics, Some(&cot));
        assert_eq!(scorecard.macro_bias, "BEARISH");
        assert_eq!(scorecard.direction, "DOWN");
        assert_eq!(scorecard.flow_quality, "CONFIRMING");
        assert_eq!(scorecard.rate_regime, "TIGHTENING");
        assert_eq!(scorecard.usd_regime, "STRONG");
        assert_eq!(scorecard.risk_regime, "RISK_OFF");
        assert_eq!(scorecard.vol_regime, "HIGH");
        assert!(scorecard.score <= -2);
    }

    #[test]
    fn score_is_neutral_and_insufficient_with_fewer_than_three_inputs() {
        let metrics = vec![
            metric("real_yield_10y", 2.0, Some(-0.05), Some(-2.0)),
            metric("dxy", 100.0, Some(-0.4), Some(-0.4)),
        ];

        let scorecard = score_macro_flow(&metrics, None);
        assert_eq!(scorecard.macro_bias, "NEUTRAL");
        assert_eq!(scorecard.direction, "MIXED");
        assert_eq!(scorecard.flow_quality, "INSUFFICIENT");
        assert!(scorecard.confidence <= 40);
        assert!(scorecard
            .notes
            .iter()
            .any(|note| note.contains("Chưa đủ 3")));
    }

    #[test]
    fn score_detects_divergence_between_price_and_flow() {
        let metrics = vec![
            metric("real_yield_10y", 2.0, Some(-0.05), Some(-2.0)),
            metric("dxy", 100.0, Some(-0.4), Some(-0.4)),
            metric("gold", 4_000.0, Some(40.0), Some(1.0)),
            metric("gld", 400.0, Some(-4.0), Some(-1.0)),
            metric("iau", 50.0, Some(-0.5), Some(-1.0)),
            metric("vix", 14.0, None, None),
            metric("gvz", 14.0, None, None),
            metric("spy", 600.0, Some(1.0), Some(0.2)),
        ];

        let scorecard = score_macro_flow(&metrics, None);
        assert_eq!(scorecard.flow_quality, "DIVERGING");
        assert_eq!(scorecard.risk_regime, "RISK_ON");
        assert_eq!(scorecard.vol_regime, "LOW");
    }

    #[test]
    fn score_without_any_metrics_is_insufficient() {
        let scorecard = score_macro_flow(&[], None);
        assert_eq!(scorecard.macro_bias, "NEUTRAL");
        assert_eq!(scorecard.flow_quality, "INSUFFICIENT");
        assert_eq!(scorecard.confidence, 40);
        assert_eq!(scorecard.score, 0);
        assert_eq!(scorecard.driver, "không có tín hiệu rõ");
    }
}
