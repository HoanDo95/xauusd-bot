//! Portfolio-level risk guardrails for live trade signals.
//!
//! These checks are deterministic and independent of the AI. They run at the
//! final signal gate, after the per-trade hard risk checks, so a stream of
//! otherwise-valid signals cannot stack unbounded exposure.

use crate::models::{BotConfig, Signal, SignalStatus, SignalType};
use chrono::{DateTime, Duration, Utc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiskGuardrailDecision {
    pub allowed: bool,
    pub reason: Option<String>,
}

impl RiskGuardrailDecision {
    fn allow() -> Self {
        Self {
            allowed: true,
            reason: None,
        }
    }

    fn block(reason: impl Into<String>) -> Self {
        Self {
            allowed: false,
            reason: Some(reason.into()),
        }
    }
}

fn vn_date(value: DateTime<Utc>) -> chrono::NaiveDate {
    (value + Duration::hours(7)).date_naive()
}

fn is_same_vn_day(left: DateTime<Utc>, right: DateTime<Utc>) -> bool {
    vn_date(left) == vn_date(right)
}

fn risk_points(signal: &Signal) -> f64 {
    (signal.entry_price - signal.stop_loss).abs()
}

fn realized_r(signal: &Signal) -> f64 {
    let risk = risk_points(signal);
    if !risk.is_finite() || risk <= 0.0 {
        return 0.0;
    }
    match signal.status {
        SignalStatus::StopLossHit => -1.0,
        SignalStatus::Target1Hit => (signal.take_profit_1 - signal.entry_price).abs() / risk,
        SignalStatus::Target2Hit => (signal.take_profit_2 - signal.entry_price).abs() / risk,
        SignalStatus::Cancelled | SignalStatus::Active => 0.0,
    }
}

fn is_live_exposure(signal: &Signal) -> bool {
    !signal.is_simulated
        && matches!(
            signal.status,
            SignalStatus::Active
                | SignalStatus::Target1Hit
                | SignalStatus::Target2Hit
                | SignalStatus::StopLossHit
        )
}

fn is_open(signal: &Signal) -> bool {
    !signal.is_simulated
        && matches!(
            signal.status,
            SignalStatus::Active | SignalStatus::Target1Hit
        )
}

/// Evaluate portfolio guardrails for a candidate signal.
///
/// `candidate_side` and `candidate_risk_usd` are optional so the same helper
/// can be used for a pre-check before the signal is built. A `None` candidate
/// risk only checks the existing portfolio state.
pub fn evaluate_risk_guardrails(
    config: &BotConfig,
    signals: &[Signal],
    now: DateTime<Utc>,
    candidate_side: Option<SignalType>,
    candidate_risk_usd: Option<f64>,
) -> RiskGuardrailDecision {
    if !config.risk_guardrails_enabled {
        return RiskGuardrailDecision::allow();
    }

    let open_count = signals.iter().filter(|signal| is_open(signal)).count();
    if config.max_concurrent_signals > 0 && open_count >= config.max_concurrent_signals {
        return RiskGuardrailDecision::block(format!(
            "Risk guardrail: đã có {} tín hiệu đang mở (tối đa {}).",
            open_count, config.max_concurrent_signals
        ));
    }

    let today_signals = signals
        .iter()
        .filter(|signal| is_live_exposure(signal) && is_same_vn_day(signal.created_at, now));

    let realized_r_today: f64 = signals
        .iter()
        .filter(|signal| !signal.is_simulated)
        .filter(|signal| {
            matches!(
                signal.status,
                SignalStatus::Target1Hit | SignalStatus::Target2Hit | SignalStatus::StopLossHit
            )
        })
        .filter(|signal| {
            let closed_at = signal.outcome_at.unwrap_or(signal.created_at);
            is_same_vn_day(closed_at, now)
        })
        .map(realized_r)
        .sum();

    if config.max_daily_loss_r > 0.0 && realized_r_today <= -config.max_daily_loss_r {
        return RiskGuardrailDecision::block(format!(
            "Risk guardrail: lỗ realised hôm nay {:.2}R đã chạm giới hạn -{:.2}R.",
            realized_r_today, config.max_daily_loss_r
        ));
    }

    let risk_used_today: f64 = today_signals.map(|signal| signal.risk_amount_usd).sum();
    let candidate_risk = candidate_risk_usd
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(0.0);
    let risk_limit = config.account_balance * config.max_daily_risk_percent / 100.0;
    if config.max_daily_risk_percent > 0.0
        && risk_limit > 0.0
        && risk_used_today + candidate_risk > risk_limit + 1e-9
    {
        return RiskGuardrailDecision::block(format!(
            "Risk guardrail: risk hôm nay ${:.2} + candidate ${:.2} vượt giới hạn ${:.2} ({:.1}% tài khoản).",
            risk_used_today, candidate_risk, risk_limit, config.max_daily_risk_percent
        ));
    }

    if config.same_direction_cooldown_minutes > 0 {
        if let Some(side) = candidate_side {
            let latest_same_direction = signals
                .iter()
                .filter(|signal| !signal.is_simulated)
                .filter(|signal| signal.signal_type == side)
                .filter(|signal| signal.status != SignalStatus::Cancelled)
                .filter(|signal| is_same_vn_day(signal.created_at, now))
                .max_by_key(|signal| signal.created_at);
            if let Some(latest) = latest_same_direction {
                let elapsed = now - latest.created_at;
                if elapsed < Duration::minutes(config.same_direction_cooldown_minutes) {
                    return RiskGuardrailDecision::block(format!(
                        "Risk guardrail: tín hiệu {} gần nhất mới {:.0} phút trước (cooldown {} phút).",
                        side.as_str(),
                        elapsed.num_seconds() as f64 / 60.0,
                        config.same_direction_cooldown_minutes
                    ));
                }
            }
        }
    }

    RiskGuardrailDecision::allow()
}

/// Backtest 2025: COUNTER-macro PRIMARY trades averaged -0.50R. Block them
/// deterministically unless the operator explicitly disables the filter.
pub fn counter_macro_block_reason(config: &BotConfig, signal: &Signal) -> Option<String> {
    if config.counter_macro_filter_enabled
        && signal
            .macro_flow_alignment
            .as_deref()
            .is_some_and(|alignment| alignment.eq_ignore_ascii_case("COUNTER"))
    {
        Some("COUNTER macro — backtest 2025 negative; bỏ qua entry.".to_string())
    } else {
        None
    }
}

/// Backtest 2025: BUY entries with RSI H1 < 30 averaged -0.32R, mostly
/// bullish_bounce catching a falling knife. Block BUY in deep H1 oversold.
pub fn buy_rsi_h1_block_reason(
    config: &BotConfig,
    signal: &Signal,
    rsi_h1: Option<f64>,
) -> Option<String> {
    if signal.signal_type != SignalType::Buy || config.min_rsi_h1_for_buy <= 0.0 {
        return None;
    }
    let value = rsi_h1.filter(|value| value.is_finite())?;
    if value < config.min_rsi_h1_for_buy {
        Some(format!(
            "BUY RSI H1 {:.2} < {:.2} — không mua đuổi vùng quá bán.",
            value, config.min_rsi_h1_for_buy
        ))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::EntryType;
    use chrono::TimeZone;

    fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0)
            .single()
            .expect("valid datetime")
    }

    fn signal(
        status: SignalStatus,
        side: SignalType,
        created_at: DateTime<Utc>,
        risk_usd: f64,
        simulated: bool,
    ) -> Signal {
        let (entry, stop_loss, take_profit_1, take_profit_2) = match side {
            SignalType::Buy => (4000.0, 3990.0, 4020.0, 4030.0),
            SignalType::Sell => (4000.0, 4010.0, 3980.0, 3970.0),
        };
        Signal {
            id: "SIG-GUARD".to_string(),
            symbol: "XAU/USD".to_string(),
            signal_type: side,
            timeframe: crate::models::Timeframe::M15,
            entry_price: entry,
            stop_loss,
            take_profit_1,
            take_profit_2,
            risk_points: 10.0,
            reward_points_tp1: 20.0,
            reward_points_tp2: 30.0,
            risk_reward_ratio: 2.0,
            volume_surge_ratio: 1.0,
            recommended_lot_size: 0.01,
            risk_amount_usd: risk_usd,
            potential_profit_tp1_usd: risk_usd * 2.0,
            potential_profit_tp2_usd: risk_usd * 3.0,
            price_action_notes: String::new(),
            volume_analysis_notes: String::new(),
            full_rationale: String::new(),
            status,
            created_at,
            telegram_message_id: None,
            dedup_key: format!("GUARD-{}-{}", side.as_str(), created_at.timestamp_millis()),
            candle_timestamp: created_at.timestamp_millis(),
            is_simulated: simulated,
            confluence_score: 80,
            outcome_at: None,
            entry_type: EntryType::Market,
            candle_close_price: entry,
            live_market_price: Some(entry),
            retest_limit_price: None,
            data_source: "TEST".to_string(),
            is_entry_filled: true,
            ai_verdict: None,
            ai_confidence: None,
            ai_reason: None,
            delta_ratio: None,
            cvd: None,
            delta: None,
            rsi_value: None,
            rsi_state: None,
            rsi_divergence: None,
            macro_flow_alignment: None,
            macro_flow_note: None,
            cancel_reason: None,
            management_note: None,
        }
    }

    fn config() -> BotConfig {
        BotConfig {
            account_balance: 10_000.0,
            risk_percentage: 1.0,
            risk_guardrails_enabled: true,
            max_concurrent_signals: 2,
            max_daily_risk_percent: 3.0,
            max_daily_loss_r: 2.0,
            same_direction_cooldown_minutes: 60,
            ..BotConfig::default()
        }
    }

    #[test]
    fn allows_when_no_signals() {
        let decision = evaluate_risk_guardrails(
            &config(),
            &[],
            at(2026, 11, 16, 10, 0),
            Some(SignalType::Buy),
            Some(10.0),
        );
        assert!(decision.allowed);
    }

    #[test]
    fn blocks_when_max_concurrent_reached() {
        let now = at(2026, 11, 16, 10, 0);
        let signals = vec![
            signal(SignalStatus::Active, SignalType::Buy, now, 10.0, false),
            signal(SignalStatus::Target1Hit, SignalType::Sell, now, 10.0, false),
        ];
        let decision =
            evaluate_risk_guardrails(&config(), &signals, now, Some(SignalType::Buy), Some(10.0));
        assert!(!decision.allowed);
        assert!(decision.reason.unwrap().contains("đang mở"));
    }

    #[test]
    fn blocks_when_daily_risk_exceeded() {
        let now = at(2026, 11, 16, 10, 0);
        let signals = vec![
            signal(SignalStatus::Target2Hit, SignalType::Buy, now, 150.0, false),
            signal(
                SignalStatus::Target2Hit,
                SignalType::Sell,
                now,
                160.0,
                false,
            ),
        ];
        let decision =
            evaluate_risk_guardrails(&config(), &signals, now, Some(SignalType::Buy), Some(10.0));
        assert!(!decision.allowed);
        assert!(decision.reason.unwrap().contains("risk hôm nay"));
    }

    #[test]
    fn blocks_when_daily_loss_r_exceeded() {
        let now = at(2026, 11, 16, 10, 0);
        let mut first = signal(SignalStatus::StopLossHit, SignalType::Buy, now, 10.0, false);
        first.outcome_at = Some(now);
        let mut second = signal(
            SignalStatus::StopLossHit,
            SignalType::Sell,
            now,
            10.0,
            false,
        );
        second.outcome_at = Some(now);
        let decision = evaluate_risk_guardrails(
            &config(),
            &[first, second],
            now,
            Some(SignalType::Buy),
            Some(10.0),
        );
        assert!(!decision.allowed);
        assert!(decision.reason.unwrap().contains("lỗ realised"));
    }

    #[test]
    fn blocks_same_direction_cooldown() {
        let now = at(2026, 11, 16, 10, 0);
        let last = signal(
            SignalStatus::Active,
            SignalType::Sell,
            now - Duration::minutes(10),
            10.0,
            false,
        );
        let decision =
            evaluate_risk_guardrails(&config(), &[last], now, Some(SignalType::Sell), Some(10.0));
        assert!(!decision.allowed);
        assert!(decision.reason.unwrap().contains("cooldown"));
    }

    #[test]
    fn allows_opposite_direction_during_cooldown() {
        let now = at(2026, 11, 16, 10, 0);
        let last = signal(
            SignalStatus::Active,
            SignalType::Sell,
            now - Duration::minutes(10),
            10.0,
            false,
        );
        let decision =
            evaluate_risk_guardrails(&config(), &[last], now, Some(SignalType::Buy), Some(10.0));
        assert!(decision.allowed);
    }

    #[test]
    fn ignores_simulated_signals() {
        let now = at(2026, 11, 16, 10, 0);
        let signals = vec![
            signal(SignalStatus::Active, SignalType::Buy, now, 10.0, true),
            signal(SignalStatus::Active, SignalType::Buy, now, 10.0, true),
        ];
        let decision =
            evaluate_risk_guardrails(&config(), &signals, now, Some(SignalType::Buy), Some(10.0));
        assert!(decision.allowed);
    }

    #[test]
    fn blocks_counter_macro_when_enabled() {
        let now = at(2026, 11, 16, 10, 0);
        let mut candidate = signal(SignalStatus::Active, SignalType::Sell, now, 10.0, false);
        candidate.macro_flow_alignment = Some("COUNTER".to_string());
        assert!(counter_macro_block_reason(&config(), &candidate).is_some());

        let mut disabled = config();
        disabled.counter_macro_filter_enabled = false;
        assert!(counter_macro_block_reason(&disabled, &candidate).is_none());
    }

    #[test]
    fn blocks_buy_when_rsi_h1_is_deep_oversold() {
        let now = at(2026, 11, 16, 10, 0);
        let buy = signal(SignalStatus::Active, SignalType::Buy, now, 10.0, false);
        assert!(buy_rsi_h1_block_reason(&config(), &buy, Some(25.0)).is_some());
        assert!(buy_rsi_h1_block_reason(&config(), &buy, Some(35.0)).is_none());

        let sell = signal(SignalStatus::Active, SignalType::Sell, now, 10.0, false);
        assert!(buy_rsi_h1_block_reason(&config(), &sell, Some(25.0)).is_none());

        let mut disabled = config();
        disabled.min_rsi_h1_for_buy = 0.0;
        assert!(buy_rsi_h1_block_reason(&disabled, &buy, Some(25.0)).is_none());
    }

    #[test]
    fn disabled_guardrails_allow_everything() {
        let now = at(2026, 11, 16, 10, 0);
        let signals = vec![signal(
            SignalStatus::Active,
            SignalType::Buy,
            now,
            999.0,
            false,
        )];
        let mut config = config();
        config.risk_guardrails_enabled = false;
        let decision =
            evaluate_risk_guardrails(&config, &signals, now, Some(SignalType::Buy), Some(999.0));
        assert!(decision.allowed);
    }
}
