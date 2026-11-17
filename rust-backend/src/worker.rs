use crate::action_alerts::candle_is_closed_and_fresh;
use crate::ai_decision::{
    ai_decision_enabled, decision_blocks_or_approves, relaxed_candidate_config,
    review_market_decision,
};
use crate::analyzer::GoldMarketAnalyzer;
use crate::api::{attach_macro_flow_alignment, persist_state, AppState};
use crate::market_context::{
    build_market_context, htf_allows_signal, shift_candles_prices, MarketContext,
};
use crate::market_feed::{
    volume_source_has_buy_sell_delta, MarketDataFeed, MarketFeedSnapshot,
    SOURCE_TWELVEDATA_YAHOO_VOLUME,
};
use crate::models::{BotConfig, Candle, Signal, Timeframe};
use crate::news_filter::ImpactLevel;
use crate::risk_guardrails::{
    buy_rsi_h1_block_reason, counter_macro_block_reason, evaluate_risk_guardrails,
};
use crate::risk_manager::PositionSizer;
use crate::rsi::{divergence_label, rsi_allows_signal, rsi_snapshot};
use crate::scenario_signal::{
    best_confirmed_scenario, confirmed_signal_from_scenario, rsi_extreme_blocks_confirmed,
};
use crate::scenarios::WATCH_SETUP_MAX_BARS;
use crate::scenarios::{anti_chase_blocks_breakout, build_scenarios, Scenario, ScenarioResponse};
use crate::scheduler::DailyScheduler;
use crate::session_alerts::evaluate_and_dispatch;
use crate::signal_review::{review_blocks_signal, review_signal};
use crate::signal_validation::{validate_risk_budget, validate_signal_levels};
use crate::storage::PersistentStore;
use crate::tracker::{evaluate_active_signals, pending_confirmation_ok, PendingConfirmation};
use crate::volume_delta::{compute_delta, delta_allows_signal};
use chrono::{DateTime, Utc};
use std::time::Instant;
use tracing::{info, warn};

/// Cache spot XAU/USD 300s (~288 calls/day), an toàn cho TwelveData free tier.
const SPOT_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(300);

async fn resolve_spot_price(
    market_feed: &MarketDataFeed,
    cache: &mut Option<(f64, Instant)>,
) -> Option<f64> {
    if let Some((price, fetched_at)) = *cache {
        if fetched_at.elapsed() < SPOT_CACHE_TTL {
            return Some(price);
        }
    }

    let price = market_feed.fetch_spot_xau_usd().await?;
    *cache = Some((price, Instant::now()));
    Some(price)
}

/// PAXG basis = spot XAU/USD − (live PAXG − broker offset thủ công).
///
/// Chỉ chấp nhận basis hữu hạn và nằm trong biên hợp lý để một lần spot lỗi
/// (giá 0/NaN/outlier) không thể kéo toàn bộ nến PAXG lệch hàng trăm USD.
fn compute_spot_basis(
    snapshot: &MarketFeedSnapshot,
    spot_price: f64,
    broker_price_offset: f64,
) -> Option<f64> {
    let raw_live = snapshot
        .live_price
        .or_else(|| snapshot.candles.last().map(|candle| candle.close))?;

    if !spot_price.is_finite()
        || spot_price <= 0.0
        || !(500.0..=10_000.0).contains(&spot_price)
        || !raw_live.is_finite()
    {
        return None;
    }

    let basis = spot_price - (raw_live - broker_price_offset);
    if !basis.is_finite() || basis.abs() > (spot_price * 0.02).max(50.0) {
        return None;
    }

    Some(basis.clamp(-100.0, 100.0))
}

/// Căn snapshot Binance PAXG về spot XAU/USD. Trả (spot, basis) nếu áp dụng thành công.
pub async fn apply_spot_basis(
    snapshot: &mut MarketFeedSnapshot,
    config: &BotConfig,
    market_feed: &MarketDataFeed,
    cache: &mut Option<(f64, Instant)>,
) -> Option<(f64, f64)> {
    if !config.use_auto_spot_basis || snapshot.source != "BINANCE_PAXG" {
        return None;
    }

    let spot_price = resolve_spot_price(market_feed, cache).await?;
    let basis = compute_spot_basis(snapshot, spot_price, config.broker_price_offset)?;
    MarketDataFeed::shift_snapshot_prices(snapshot, basis);
    info!("🌐 Spot basis: spot={:.2}, basis={:+.2}", spot_price, basis);
    Some((spot_price, basis))
}

/// Chuẩn hóa `data_source` từ cấu hình cũ: chỉ map giá trị rỗng/không hợp lệ
/// về BINANCE_PAXG; giữ nguyên TWELVEDATA do user chọn (Phase C1).
pub fn normalize_data_source(config: &mut BotConfig) {
    config.data_source = match config.data_source.trim().to_uppercase().as_str() {
        "TWELVEDATA" => "TWELVEDATA",
        "OANDA" => "OANDA",
        _ => "BINANCE_PAXG",
    }
    .to_string();
}

/// Nhãn nguồn giá cho panel "view toàn cảnh" khi HTF lấy từ Binance PAXG.
fn htf_price_source(spot_basis: Option<f64>) -> String {
    if spot_basis.is_some() {
        "BINANCE_PAXG+TWELVEDATA_SPOT".to_string()
    } else {
        "BINANCE_PAXG".to_string()
    }
}

/// Nhãn nguồn giá HTF khi dùng TwelveData; ưu tiên nhãn Yahoo nếu có.
fn twelve_htf_price_source(sources: &[&str]) -> &'static str {
    if sources
        .iter()
        .any(|source| source.contains("+YAHOO_FUTURES_VOLUME"))
    {
        SOURCE_TWELVEDATA_YAHOO_VOLUME
    } else if sources.iter().any(|source| source.contains("+PAXG_VOLUME")) {
        "TWELVEDATA_XAUUSD+PAXG_VOLUME"
    } else {
        "TWELVEDATA_XAUUSD"
    }
}

/// Lượng dịch giá cho HTF: ưu tiên spot basis (đã bao gồm broker offset khi
/// auto-basis bật), nếu không có thì dùng broker offset thủ công để H4/D1/H1
/// thẳng hàng với nến M15. Caller chỉ áp dụng khi giá trị hữu hạn và khác 0.
pub fn htf_shift_amount(spot_basis: Option<f64>, broker_price_offset: f64) -> f64 {
    spot_basis.unwrap_or(broker_price_offset)
}

/// Fetch H4/D1/H1 đã đóng cửa và dựng MarketContext. Trả None nếu thiếu H4/D1.
/// Kèm theo nến H4/D1 (đã căn basis) để dashboard vẽ "view toàn cảnh".
pub async fn fetch_market_context(
    market_feed: &MarketDataFeed,
    data_source: &str,
    spot_basis: Option<f64>,
    broker_price_offset: f64,
) -> Option<(MarketContext, Vec<Candle>, Vec<Candle>)> {
    let ((mut h4, h4_source), (mut d1, d1_source), (mut h1, h1_source)) = tokio::join!(
        market_feed.fetch_htf_candles_with_source(Timeframe::H4, 300, data_source),
        market_feed.fetch_daily_htf_candles_with_source(300, data_source),
        market_feed.fetch_htf_candles_with_source(Timeframe::H1, 300, data_source),
    );

    // TwelveData đã là giá XAU/USD spot nên chỉ cộng broker offset thủ công;
    // Binance vẫn dùng spot basis (nếu có) như trước.
    let binance_shift = htf_shift_amount(spot_basis, broker_price_offset);
    let spot_shift = broker_price_offset;
    let shift_for = |source_label: &str| {
        if source_label.starts_with("TWELVEDATA") || source_label.starts_with("OANDA") {
            spot_shift
        } else {
            binance_shift
        }
    };
    for (candles, shift) in [
        (&mut h4, shift_for(&h4_source)),
        (&mut d1, shift_for(&d1_source)),
        (&mut h1, shift_for(&h1_source)),
    ] {
        if shift.is_finite() && shift != 0.0 {
            shift_candles_prices(candles, shift);
        }
    }

    if h4.is_empty() || d1.is_empty() {
        warn!(
            "⚠️ [HTF Context] Thiếu nến H4 ({}) hoặc D1 ({}), giữ bối cảnh cũ.",
            h4.len(),
            d1.len()
        );
        return None;
    }
    if h1.is_empty() {
        warn!("⚠️ [HTF Context] Không có nến H1, panel H1 sẽ trống.");
    }

    let twelve_context = h4_source.starts_with("TWELVEDATA");
    let oanda_context = h4_source.starts_with("OANDA");
    let price_source = if oanda_context {
        h4_source.clone()
    } else if twelve_context {
        twelve_htf_price_source(&[&h4_source, &d1_source, &h1_source]).to_string()
    } else {
        htf_price_source(spot_basis)
    };
    let context_spot_basis = if twelve_context || oanda_context {
        None
    } else {
        spot_basis
    };

    Some((
        build_market_context(&d1, &h4, &h1, &price_source, context_spot_basis),
        h4,
        d1,
    ))
}

/// Hard risk validation for an AI-approved candidate. The AI only chooses the
/// direction; SL/TP ordering, R:R and the risk budget remain deterministic.
fn validate_ai_hard_risk(signal: &Signal, config: &BotConfig) -> Result<(), String> {
    validate_signal_levels(
        signal.signal_type,
        signal.entry_price,
        signal.stop_loss,
        signal.take_profit_1,
        signal.take_profit_2,
        config.min_risk_reward,
    )?;

    let sizing = PositionSizer::calculate_lot_size(
        config.account_balance,
        config.risk_percentage,
        signal.risk_points,
        signal.reward_points_tp1,
        signal.reward_points_tp2,
        signal.entry_price,
        config.account_leverage,
    );
    validate_risk_budget(config, &sizing)
}

/// Run the deterministic confirmation gates for one scenario and build the
/// filled MARKET signal. Shared by the stored active setup and the normal
/// pre-close scenario path.
fn confirmed_signal_with_gates(
    scenario: &Scenario,
    config: &BotConfig,
    candles: &[Candle],
    live_price: Option<f64>,
    rsi_value: Option<f64>,
    now: DateTime<Utc>,
) -> Option<Signal> {
    if rsi_extreme_blocks_confirmed(
        &scenario.bias,
        rsi_value,
        config.rsi_oversold,
        config.rsi_overbought,
    ) {
        info!(
            "⏳ [Confirmed Signal] {} bị chặn: RSI M15 {:.1} đã quá bán/quá mua — chờ hồi, không đuổi.",
            scenario.id,
            rsi_value.unwrap_or_default()
        );
        return None;
    }

    let signal = confirmed_signal_from_scenario(scenario, config, candles, live_price, now)?;
    let last_index = candles.len().saturating_sub(1);
    if config.quantum_trigger_confirmation_enabled
        && !pending_confirmation_ok(
            &signal,
            candles,
            last_index,
            config.quantum_volume_confirm_ratio,
        )
    {
        info!(
            "⏳ [Confirmed Signal] {} trigger chưa xác nhận volume/pattern — chờ nến sau.",
            scenario.id
        );
        return None;
    }

    info!(
        "🚨 [Confirmed Signal] Scenario {} ({}) confidence {}% — entry {:.2} MARKET sau M15 close {:.2}.",
        scenario.role,
        scenario.id,
        scenario.confidence,
        signal.entry_price,
        candles.last().map(|candle| candle.close).unwrap_or_default()
    );
    Some(signal)
}

async fn clear_active_watch_setup(state: &AppState) {
    let had_setup = state.active_watch_setup.write().await.take().is_some();
    if had_setup {
        if let Err(err) = persist_state(state).await {
            warn!(
                "⚠️ [Setup Watching] Không lưu được trạng thái clear setup: {}",
                err
            );
        }
    }
}

fn single_scenario_response(
    scenario: &Scenario,
    last_candle: &Candle,
    source: &str,
    volume_source: &str,
    now: DateTime<Utc>,
) -> ScenarioResponse {
    ScenarioResponse {
        status: "active_watch".to_string(),
        generated_at: now.to_rfc3339(),
        price: Some(last_candle.close),
        source: source.to_string(),
        volume_source: volume_source.to_string(),
        market: None,
        scenarios: vec![scenario.clone()],
        warnings: vec![],
    }
}

/// Build a **confirmed** signal from a valid scenario when the last closed M15
/// candle has already triggered it. This is the only path that may send a full
/// Entry/SL/TP/Lot alert; the pre-trigger early-entry full signal is removed.
async fn build_confirmed_scenario_signal(
    state: &AppState,
    candles: &[Candle],
    config: &BotConfig,
    live_price: Option<f64>,
) -> Option<(Signal, ScenarioResponse)> {
    let last_candle = candles.last()?;
    let now = Utc::now();

    // Only a freshly closed M15 candle may produce a full signal. Without this
    // guard a delayed feed could emit a "confirmed" alert on a candle that
    // closed many minutes ago, which is exactly the late-chase behaviour the
    // two-level pipeline is meant to prevent.
    if !candle_is_closed_and_fresh(
        last_candle,
        now.timestamp_millis(),
        config.max_candle_age_secs,
    ) {
        return None;
    }

    let rsi = rsi_snapshot(
        candles,
        config.rsi_period,
        config.rsi_overbought,
        config.rsi_oversold,
        config.rsi_divergence_lookback,
        config.rsi_pivot_k,
    );
    let rsi_value = rsi.value;
    let mut context = state.market_context.read().await.clone();
    if let Some(context) = context.as_mut() {
        context.rsi_m15 = Some(rsi);
    }
    let macro_flow = state.macro_flow.read().await.clone();
    let active_data_source = state.active_data_source.read().await.clone();
    let active_volume_source = state.active_volume_source.read().await.clone();

    // If a SETUP WATCHING alert is active, its frozen levels own the next
    // confirmed signal. Do not rebuild a different scenario while it is valid.
    let active_setup = state.active_watch_setup.read().await.clone();
    if let Some(setup) = active_setup {
        if setup.is_triggered(last_candle) {
            let signal = confirmed_signal_with_gates(
                &setup.scenario,
                config,
                candles,
                live_price,
                rsi_value,
                now,
            );
            clear_active_watch_setup(state).await;
            let scenarios = single_scenario_response(
                &setup.scenario,
                last_candle,
                &active_data_source,
                &active_volume_source,
                now,
            );
            return signal.map(|signal| (signal, scenarios));
        }
        if setup.is_expired(now, WATCH_SETUP_MAX_BARS) || setup.is_invalidated(last_candle, None) {
            clear_active_watch_setup(state).await;
        } else {
            return None;
        }
    }

    // Build the setup from the candles *before* the confirming candle. Using
    // the confirming candle would move the trigger to the next support and the
    // breakout we just saw would never match its own scenario.
    let setup_candles = &candles[..candles.len().saturating_sub(1)];
    let setup_price = setup_candles.last().map(|candle| candle.close);
    let scenarios = build_scenarios(
        setup_candles,
        config,
        setup_price,
        context.as_ref(),
        macro_flow.as_ref(),
        &active_data_source,
        &active_volume_source,
        now.timestamp_millis(),
    );
    let scenario = best_confirmed_scenario(&scenarios, last_candle)?;
    let signal =
        confirmed_signal_with_gates(scenario, config, candles, live_price, rsi_value, now)?;
    Some((signal, scenarios))
}

/// Background Scanner 24/7 — quét mỗi 60 giây.
///
/// `initial_spot_cache` giữ lại kết quả spot/basis vừa fetch lúc bootstrap để
/// tick đầu tiên không gọi thêm TwelveData ngoài cửa sổ cache 300s.
pub fn spawn_scanner(state: AppState, initial_spot_cache: Option<(f64, Instant)>) {
    tokio::spawn(async move {
        info!("⏱️ Sniper Worker 24/7 đã khởi chạy (chu kỳ 60 giây)...");
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        let mut cached_spot = initial_spot_cache;

        loop {
            interval.tick().await;
            *state.last_scan_at.write().await = Some(Utc::now());

            // 6.1 Luôn refresh nến thật đã đóng cửa (kể cả khi bot tạm dừng).
            let config = state.config.read().await.clone();
            state
                .market_feed
                .set_yahoo_futures_volume_enabled(config.yahoo_futures_volume_enabled);
            let mut snapshot = state
                .market_feed
                .fetch_market_snapshot(
                    config.timeframe,
                    300,
                    &config.data_source,
                    config.broker_price_offset,
                )
                .await;
            *state.active_volume_source.write().await = snapshot.volume_source.clone();

            // 6.1b Căn PAXG về spot XAU/USD trước khi tracker/analyzer nhìn thấy giá.
            let applied_spot =
                apply_spot_basis(&mut snapshot, &config, &state.market_feed, &mut cached_spot)
                    .await;
            let (scan_spot_price, scan_spot_basis) = match applied_spot {
                Some((spot, basis)) => {
                    *state.spot_basis_at.write().await = Some(Utc::now());
                    (Some(spot), Some(basis))
                }
                None => {
                    // Spot lỗi nhưng vẫn có PAXG thật: dùng lại basis tốt gần nhất
                    // (tối đa 15 phút) thay vì phân tích giá PAXG thô.
                    if config.use_auto_spot_basis && snapshot.source == "BINANCE_PAXG" {
                        let existing_basis = *state.spot_basis.read().await;
                        let basis_at = *state.spot_basis_at.read().await;
                        let basis_age_secs = basis_at.map(|at| (Utc::now() - at).num_seconds());
                        let basis_is_fresh = basis_age_secs.map(|age| age <= 900).unwrap_or(false);
                        match existing_basis.filter(|basis| basis.is_finite() && basis_is_fresh) {
                            Some(basis) => {
                                MarketDataFeed::shift_snapshot_prices(&mut snapshot, basis);
                                let existing_spot = *state.spot_price.read().await;
                                warn!(
                                    "⚠️ [Scanner] Không lấy được spot mới — dùng lại basis {:+} (tuổi {}s).",
                                    basis,
                                    basis_age_secs.unwrap_or(0)
                                );
                                (existing_spot, Some(basis))
                            }
                            None => {
                                *state.active_data_source.write().await =
                                    "SPOT_BASIS_UNAVAILABLE".to_string();
                                warn!(
                                    "⚠️ [Scanner] Không có spot basis hợp lệ (<15 phút) — bỏ qua tracker & phân tích, không dùng PAXG thô."
                                );
                                continue;
                            }
                        }
                    } else {
                        (None, None)
                    }
                }
            };
            *state.spot_price.write().await = scan_spot_price;
            *state.spot_basis.write().await = scan_spot_basis;

            // Không bao giờ phân tích/tracker trên nến giả fallback.
            if snapshot.source == "FALLBACK_SEED" {
                *state.active_data_source.write().await = "FALLBACK_SEED".to_string();
                warn!(
                    "⚠️ [Scanner] Cả Binance và TwelveData đều lỗi — giữ nguyên nến cũ, bỏ qua tracker & phân tích."
                );
                continue;
            }

            let fresh_candles = snapshot.candles;
            if !fresh_candles.is_empty() {
                // Cập nhật TP1/TP2/SL cho các tín hiệu đang mở trước khi ghi đè nến.
                let updates = {
                    let mut signals = state.signals.write().await;
                    evaluate_active_signals(
                        &mut signals,
                        &fresh_candles,
                        PendingConfirmation {
                            enabled: config.quantum_trigger_confirmation_enabled,
                            volume_ratio: config.quantum_volume_confirm_ratio,
                        },
                    )
                };
                *state.candles.write().await = fresh_candles;
                *state.live_price.write().await = snapshot.live_price;
                *state.active_data_source.write().await = snapshot.source.clone();
                if !updates.is_empty() {
                    info!("📈 Đã cập nhật {} tín hiệu theo TP/SL", updates.len());
                    for update in updates.iter().filter(|signal| !signal.is_simulated) {
                        let telegram = state.telegram.read().await.clone();
                        if let Err(err) = telegram.send_outcome_alert(update).await {
                            warn!("⚠️ Không gửi được Telegram outcome: {}", err);
                        }
                    }
                    if let Err(err) = persist_state(&state).await {
                        warn!("⚠️ Không lưu được kết quả TP/SL: {}", err);
                    }
                }
            }

            // 6.2 Bot đang hoạt động?
            if !config.is_active {
                continue;
            }

            // 6.2b Hard weekend gate: no trade signals or entry alerts on
            // Saturday/Sunday, even when the session filter is disabled.
            if PersistentStore::is_market_closed_weekend(Utc::now()) {
                continue;
            }

            let now = Utc::now();
            let signals_snapshot = state.signals.read().await.clone();

            // 6.3 Daily quota là tùy chọn. Mặc định TẮT: phát mọi setup đủ điều kiện.
            if config.daily_quota_enabled
                && PersistentStore::has_reached_daily_quota(
                    &signals_snapshot,
                    config.max_signals_per_day,
                    now,
                )
            {
                continue;
            }

            // 6.4 Chỉ quét trong khung giờ thanh khoản Âu-Mỹ (13:00-23:00 GMT+7).
            if !PersistentStore::is_within_golden_session(now, config.session_filter_enabled) {
                continue;
            }

            // 6.5 News Kill Switch: không vào lệnh quanh tin High Impact USD.
            let is_market_safe = state
                .news_filter
                .read()
                .await
                .check_kill_switch(now, &config);
            if !is_market_safe {
                continue;
            }

            // 6.6 Dedup: cùng một cây nến chỉ được đánh giá/phát tín hiệu tối đa 1 lần.
            let candles = state.candles.read().await.clone();
            let latest_timestamp = match candles.last() {
                Some(candle) => candle.timestamp,
                None => continue,
            };
            if PersistentStore::is_duplicate_candle_timestamp(&signals_snapshot, latest_timestamp) {
                continue;
            }

            // 6.7 Phân tích và phát tín hiệu nếu đủ 5 tầng lọc.
            // Ở DECISION mode, soft filters (volume/macro bias) được nới lỏng để
            // AI có thể review candidate; hard risk vẫn do code kiểm tra.
            let live_price = *state.live_price.read().await;
            let active_volume_source = state.active_volume_source.read().await.clone();
            let decision_mode = ai_decision_enabled(&config);
            let candidate_config = if decision_mode {
                relaxed_candidate_config(&config)
            } else {
                config.clone()
            };
            // A persisted SETUP WATCHING owns the next signal until it triggers
            // or invalidates. Skip the pinbar analyzer so a different candidate
            // cannot fire while the user is watching a frozen setup.
            let setup_owns_signal = {
                let active = state.active_watch_setup.read().await;
                active.as_ref().is_some_and(|setup| {
                    candles.last().is_some_and(|candle| {
                        !setup.is_expired(now, WATCH_SETUP_MAX_BARS)
                            && !setup.is_invalidated(candle, None)
                    })
                })
            };
            let mut candidate = if setup_owns_signal {
                None
            } else {
                GoldMarketAnalyzer::analyze_market_with_live_price(
                    &candles,
                    &candidate_config,
                    live_price,
                    Utc::now().timestamp_millis(),
                )
            };
            let mut confirmed_scenarios: Option<ScenarioResponse> = None;
            if candidate.is_none() && decision_mode {
                if let Some((signal, scenarios)) =
                    build_confirmed_scenario_signal(&state, &candles, &config, live_price).await
                {
                    candidate = Some(signal);
                    confirmed_scenarios = Some(scenarios);
                }
            }

            if let Some(mut signal) = candidate {
                if PersistentStore::is_duplicate_candle(&signals_snapshot, &signal.dedup_key) {
                    continue;
                }

                // 6.7a RSI M15 (advisory/context) — luôn gắn vào tín hiệu.
                let rsi = rsi_snapshot(
                    &candles,
                    config.rsi_period,
                    config.rsi_overbought,
                    config.rsi_oversold,
                    config.rsi_divergence_lookback,
                    config.rsi_pivot_k,
                );
                signal.rsi_value = rsi.value;
                signal.rsi_state = Some(rsi.state.clone());
                signal.rsi_divergence = Some(divergence_label(rsi.divergence));

                if decision_mode {
                    // 6.7b AI Decision Mode: build the full brief and let the AI
                    // choose BUY/SELL/STANDBY. No AI response = fail-closed.
                    let mut context = state.market_context.read().await.clone();
                    if let Some(context) = context.as_mut() {
                        context.rsi_m15 = Some(rsi.clone());
                    }
                    let events = state.news_filter.read().await.calendar_events.clone();
                    let macro_flow = state.macro_flow.read().await.clone();
                    let active_data_source = state.active_data_source.read().await.clone();
                    let delta = compute_delta(&candles, config.volume_delta_lookback);
                    let scenarios = if let Some(scenarios) = confirmed_scenarios.take() {
                        scenarios
                    } else {
                        build_scenarios(
                            &candles,
                            &config,
                            live_price,
                            context.as_ref(),
                            macro_flow.as_ref(),
                            &active_data_source,
                            &active_volume_source,
                            Utc::now().timestamp_millis(),
                        )
                    };

                    // Macro/flow alignment is advisory context for the AI and UI.
                    attach_macro_flow_alignment(&mut signal, macro_flow.as_ref());

                    let delta_for_ai = if volume_source_has_buy_sell_delta(&active_volume_source) {
                        Some(&delta)
                    } else {
                        None
                    };
                    let decision = review_market_decision(
                        Some(&signal),
                        context.as_ref(),
                        macro_flow.as_ref(),
                        Some(&scenarios),
                        Some(&rsi),
                        delta_for_ai,
                        &events,
                        &config,
                    )
                    .await;

                    let Some(decision) = decision else {
                        warn!(
                            "🛑 [AI Decision] Không có quyết định cho {} {} — fail-closed, bỏ qua tín hiệu.",
                            signal.signal_type, signal.symbol
                        );
                        continue;
                    };
                    *state.last_ai_decision.write().await = Some(decision.clone());

                    signal.ai_verdict = Some(decision.action.clone());
                    signal.ai_confidence = Some(decision.confidence_score);
                    signal.ai_reason = Some(decision.reason.clone());
                    if decision.macro_flow_alignment.is_some() {
                        signal.macro_flow_alignment = decision.macro_flow_alignment.clone();
                    }
                    if decision.macro_flow_note.is_some() {
                        signal.macro_flow_note = decision.macro_flow_note.clone();
                    }
                    if volume_source_has_buy_sell_delta(&active_volume_source) {
                        signal.delta_ratio = Some(delta.delta_ratio);
                        signal.cvd = Some(delta.cvd);
                        signal.delta = Some(delta.delta);
                    }

                    let (blocked, reason) = decision_blocks_or_approves(
                        &signal,
                        &decision,
                        config.ai_signal_review_min_confidence,
                    );
                    if blocked {
                        warn!(
                            "🧠 [AI Decision] Bỏ qua {} {}: {}",
                            signal.signal_type, signal.symbol, reason
                        );
                        continue;
                    }
                    info!(
                        "✅ [AI Decision] Cho qua {} {} — {}",
                        signal.signal_type, signal.symbol, reason
                    );

                    // Deterministic anti-chase gate: after an exhausted daily
                    // move, do not chase a breakout even if the AI approved it.
                    let daily_change_pct = context.as_ref().and_then(|context| {
                        let prev_close = context.d1.prev_close;
                        (prev_close.is_finite() && prev_close > 0.0).then(|| {
                            let current = live_price.unwrap_or(context.d1.close);
                            (current - prev_close) / prev_close * 100.0
                        })
                    });
                    let rsi_h1 = context
                        .as_ref()
                        .and_then(|context| context.rsi_h1.as_ref())
                        .and_then(|snapshot| snapshot.value);
                    if let Some(reason) = anti_chase_blocks_breakout(
                        signal.signal_type,
                        signal.entry_type,
                        daily_change_pct,
                        rsi_h1,
                        rsi.divergence,
                        &config,
                    ) {
                        warn!(
                            "🛑 [Anti-chase] Bỏ qua {} {}: {}",
                            signal.signal_type, signal.symbol, reason
                        );
                        continue;
                    }

                    // AI has weighed HTF/volume/RSI/macro as context. Skip the
                    // soft filters but keep every hard risk rule.
                    if let Err(reason) = validate_ai_hard_risk(&signal, &config) {
                        warn!(
                            "🛑 [AI Decision] Hard risk từ chối {} {}: {}",
                            signal.signal_type, signal.symbol, reason
                        );
                        continue;
                    }
                } else {
                    // 6.7b Bộ lọc H4/D1 tùy chọn (mặc định OFF).
                    if config.htf_h4_filter_enabled || config.htf_d1_filter_enabled {
                        let context = state.market_context.read().await;
                        if let Some(context) = context.as_ref() {
                            let (allowed, reason) = htf_allows_signal(
                                signal.signal_type.as_str(),
                                context,
                                config.htf_h4_filter_enabled,
                                config.htf_d1_filter_enabled,
                            );
                            if !allowed {
                                warn!(
                                    "🛑 [HTF Filter] Bỏ qua {} {}: {}",
                                    signal.signal_type, signal.symbol, reason
                                );
                                continue;
                            }
                        }
                    }

                    // 6.7c Volume Delta/CVD tùy chọn (mặc định OFF).
                    // Chỉ áp dụng cho tín hiệu thật; simulation bỏ qua hoàn toàn.
                    if config.volume_delta_filter_enabled && !signal.is_simulated {
                        if !volume_source_has_buy_sell_delta(&active_volume_source) {
                            warn!(
                                "⚠️ [Volume Delta] Nguồn {} không có buy/sell delta — bỏ qua filter.",
                                active_volume_source
                            );
                        } else {
                            let (allowed, reason, snapshot) =
                                delta_allows_signal(signal.signal_type.as_str(), &candles, &config);
                            if !allowed {
                                warn!(
                                    "🛑 [Volume Delta] Bỏ qua {} {}: {}",
                                    signal.signal_type, signal.symbol, reason
                                );
                                continue;
                            }
                            signal.delta_ratio = Some(snapshot.delta_ratio);
                            signal.cvd = Some(snapshot.cvd);
                            signal.delta = Some(snapshot.delta);
                            info!("📊 [Volume Delta] {} — {}", signal.id, reason);
                        }
                    }

                    // 6.7d RSI filter tùy chọn (mặc định OFF).
                    // Chỉ áp dụng cho tín hiệu thật; simulation bỏ qua hoàn toàn.
                    if config.rsi_filter_enabled && !signal.is_simulated {
                        let (allowed, reason) =
                            rsi_allows_signal(signal.signal_type.as_str(), &rsi, &config);
                        if !allowed {
                            warn!(
                                "🛑 [RSI Filter] Bỏ qua {} {}: {}",
                                signal.signal_type, signal.symbol, reason
                            );
                            continue;
                        }
                        info!("📐 [RSI Filter] {} — {}", signal.id, reason);
                    }

                    // 6.7e Macro & Flow alignment (advisory) — luôn gắn, kể cả khi
                    // AI review tắt. Không bao giờ chặn tín hiệu.
                    let macro_flow = state.macro_flow.read().await.clone();
                    attach_macro_flow_alignment(&mut signal, macro_flow.as_ref());

                    // 6.7f AI Chief Risk Officer VETO_ONLY (hành vi cũ).
                    // AI chỉ được veto/standby; không bao giờ sửa SL/TP live.
                    if config.ai_signal_review_enabled {
                        let mut context = state.market_context.read().await.clone();
                        if let Some(context) = context.as_mut() {
                            context.rsi_m15 = Some(rsi.clone());
                        }
                        let events = state.news_filter.read().await.calendar_events.clone();

                        match review_signal(
                            &signal,
                            context.as_ref(),
                            macro_flow.as_ref(),
                            &config,
                            &events,
                        )
                        .await
                        {
                            Some(review) => {
                                signal.ai_verdict = Some(review.action.clone());
                                signal.ai_confidence = Some(review.confidence_score);
                                signal.ai_reason = Some(review.reason.clone());
                                if review.macro_flow_alignment.is_some() {
                                    signal.macro_flow_alignment =
                                        review.macro_flow_alignment.clone();
                                }
                                if review.macro_flow_note.is_some() {
                                    signal.macro_flow_note = review.macro_flow_note.clone();
                                }

                                let (blocked, reason) = review_blocks_signal(
                                    &signal,
                                    &review,
                                    config.ai_signal_review_min_confidence,
                                );
                                if blocked {
                                    warn!(
                                        "🤖 [AI Risk Officer] Bỏ qua {} {}: {}",
                                        signal.signal_type, signal.symbol, reason
                                    );
                                    continue;
                                }
                                info!(
                                    "✅ [AI Risk Officer] Cho qua {} {} — {}",
                                    signal.signal_type, signal.symbol, reason
                                );
                            }
                            None => {
                                if config.ai_signal_review_fail_open {
                                    warn!(
                                        "⚠️ [AI Risk Officer] Không có review cho {} {} — fail-open, cho tín hiệu qua.",
                                        signal.signal_type, signal.symbol
                                    );
                                } else {
                                    warn!(
                                        "🛑 [AI Risk Officer] Không có review cho {} {} — fail-closed, bỏ qua tín hiệu.",
                                        signal.signal_type, signal.symbol
                                    );
                                    continue;
                                }
                            }
                        }
                    }
                }

                // 6.7f Cổng cuối: dedup/quota phải được kiểm tra nguyên tử với
                // bước gửi Telegram + lưu tín hiệu. Giữ khóa tới hết vòng lặp.
                let _gate = state.signal_gate.lock().await;
                let signals_now = state.signals.read().await.clone();
                if PersistentStore::is_duplicate_candle(&signals_now, &signal.dedup_key) {
                    warn!(
                        "🛑 [Scanner] Dedup sau phân tích — bỏ qua {} {}",
                        signal.signal_type, signal.symbol
                    );
                    continue;
                }
                if config.daily_quota_enabled
                    && PersistentStore::has_reached_daily_quota(
                        &signals_now,
                        config.max_signals_per_day,
                        Utc::now(),
                    )
                {
                    warn!(
                        "🛑 [Scanner] Quota ngày đã đạt sau phân tích — bỏ qua {} {}",
                        signal.signal_type, signal.symbol
                    );
                    continue;
                }

                if let Some(reason) = counter_macro_block_reason(&config, &signal) {
                    warn!(
                        "🛑 [Macro Filter] Bỏ qua {} {}: {}",
                        signal.signal_type, signal.symbol, reason
                    );
                    continue;
                }
                let rsi_h1 = state
                    .market_context
                    .read()
                    .await
                    .as_ref()
                    .and_then(|context| context.rsi_h1.as_ref())
                    .and_then(|snapshot| snapshot.value);
                if let Some(reason) = buy_rsi_h1_block_reason(&config, &signal, rsi_h1) {
                    warn!(
                        "🛑 [RSI H1 Guard] Bỏ qua {} {}: {}",
                        signal.signal_type, signal.symbol, reason
                    );
                    continue;
                }

                let guardrail = evaluate_risk_guardrails(
                    &config,
                    &signals_now,
                    Utc::now(),
                    Some(signal.signal_type),
                    Some(signal.risk_amount_usd),
                );
                if !guardrail.allowed {
                    warn!(
                        "🛑 [Risk Guardrail] Bỏ qua {} {}: {}",
                        signal.signal_type,
                        signal.symbol,
                        guardrail.reason.unwrap_or_else(|| "blocked".to_string())
                    );
                    continue;
                }

                info!(
                    "🚨 [Sniper Scanner] Tín hiệu {} {} | Entry={} | SL={} | Confluence={}/100",
                    signal.signal_type,
                    signal.symbol,
                    signal.entry_price,
                    signal.stop_loss,
                    signal.confluence_score
                );

                let telegram = state.telegram.read().await.clone();
                if let Ok(msg_id) = telegram.send_signal_alert(&signal).await {
                    signal.telegram_message_id = Some(msg_id);
                }

                signal.is_simulated = false;
                state.signals.write().await.insert(0, signal);

                if let Err(err) = persist_state(&state).await {
                    warn!("⚠️ Không lưu được trạng thái sau tín hiệu: {}", err);
                }
            }
        }
    });
}

/// HTF Context Worker — refresh D1/H4/H1 mỗi 15 phút (read-only, không vào lệnh).
pub fn spawn_htf_context(state: AppState) {
    tokio::spawn(async move {
        info!("🧭 [HTF Context] Worker làm mới D1/H4/H1 mỗi 15 phút...");
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(15 * 60)).await;
            let config = state.config.read().await.clone();
            state
                .market_feed
                .set_yahoo_futures_volume_enabled(config.yahoo_futures_volume_enabled);
            let spot_basis = *state.spot_basis.read().await;
            if let Some((context, h4, d1)) = fetch_market_context(
                &state.market_feed,
                &config.data_source,
                spot_basis,
                config.broker_price_offset,
            )
            .await
            {
                *state.market_context.write().await = Some(context);
                *state.htf_h4_candles.write().await = h4;
                *state.htf_d1_candles.write().await = d1;
                info!("✅ [HTF Context] Đã cập nhật bối cảnh D1/H4/H1.");
            }
        }
    });
}

/// Daily Cron 07:00 AM — đồng bộ ForexFactory + báo cáo EMA.
pub fn spawn_daily_scheduler(state: AppState, scheduler: DailyScheduler) {
    tokio::spawn(async move {
        info!("⏰ [Daily Scheduler] Khởi động Cron Job 07:00 AM mỗi ngày...");
        loop {
            let sleep_seconds = DailyScheduler::seconds_until_next_seven_am();
            info!(
                "⏳ [Daily Scheduler] Lần chạy tiếp theo sau {} giây.",
                sleep_seconds
            );
            tokio::time::sleep(std::time::Duration::from_secs(sleep_seconds)).await;

            let now = Utc::now();
            let weekend = PersistentStore::is_market_closed_weekend(now);
            let events = if weekend {
                scheduler.fetch_next_week_economic_events().await
            } else {
                scheduler.fetch_daily_economic_events().await
            };
            {
                let mut nf = state.news_filter.write().await;
                nf.update_events(events.clone());
            }

            let d1_candles = state.htf_d1_candles.read().await.clone();
            let report_candles = if d1_candles.len() >= 200 {
                d1_candles
            } else {
                state.candles.read().await.clone()
            };
            let live_price = *state.live_price.read().await;
            let macro_flow = state.macro_flow.read().await.clone();
            let mut report = scheduler
                .fetch_daily_macro_bias_from_context(
                    &report_candles,
                    live_price,
                    macro_flow.as_ref(),
                )
                .await;
            report.high_impact_events_today = events
                .iter()
                .filter(|event| event.impact == ImpactLevel::High)
                .take(6)
                .map(|event| {
                    format!(
                        "{} ({})",
                        event.title,
                        event.scheduled_time.format("%d/%m %H:%M UTC")
                    )
                })
                .collect();

            // Bias của báo cáo chỉ mang tính tham khảo. KHÔNG ghi đè config.macro_bias:
            // giữ nguyên BULLISH/BEARISH/NEUTRAL do user chọn, và giữ AUTO để EMA50/200
            // tiếp tục được đánh giá động ở mỗi tín hiệu.
            info!(
                "🤖 [Daily Scheduler] Nguồn báo cáo: {} | bias tư vấn: {} (config.macro_bias giữ nguyên).",
                scheduler.report_source(),
                report.macro_bias.as_str()
            );
            {
                let mut rep = state.daily_report.write().await;
                *rep = Some(report.clone());
            }
            if let Err(err) = persist_state(&state).await {
                warn!("⚠️ Không lưu được Daily Macro report: {}", err);
            }

            // 07:00: weekday = daily plan; weekend = news + weekly assessment.
            // Both are read-only/advisory, no quota, no signal insertion.
            let config = state.config.read().await.clone();
            let live_view = crate::api::current_live_view(&state).await;
            let scenarios = crate::api::current_scenarios(&state).await;
            let telegram = state.telegram.read().await.clone();
            if weekend {
                if config.weekend_brief_enabled {
                    match telegram
                        .send_weekend_brief(&live_view, &scenarios, &events, &report)
                        .await
                    {
                        Ok(message_id) => info!(
                            "🌙 [Weekend Brief 07:00] Đã gửi tin tức + đánh giá tuần tới (message_id={}).",
                            message_id
                        ),
                        Err(err) => warn!("⚠️ Không gửi được weekend brief: {}", err),
                    }
                } else {
                    info!("🌙 [Weekend Brief 07:00] Weekend brief đang tắt.");
                }
            } else if config.daily_plan_enabled {
                match telegram
                    .send_daily_plan(&live_view, &scenarios, &events, &report)
                    .await
                {
                    Ok(message_id) => info!(
                        "📅 [Daily Scheduler 07:00 AM] Đã gửi kế hoạch ngày tới Telegram (message_id={}).",
                        message_id
                    ),
                    Err(err) => warn!("⚠️ Không gửi được daily plan Telegram: {}", err),
                }
            } else {
                info!("📅 [Daily Scheduler 07:00 AM] Daily plan Telegram đang tắt.");
            }
            info!("✅ [Daily Scheduler 07:00 AM] Đã cập nhật lịch tin & xu hướng EMA.");
        }
    });
}

/// Session Pre-Alert Worker — briefing thông tin trước phiên Âu/Mỹ, mỗi 30 giây.
pub fn spawn_session_alerts(state: AppState) {
    tokio::spawn(async move {
        info!("🌍 [Session Alerts] Worker briefing trước phiên đã khởi chạy (chu kỳ 30 giây)...");
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));

        loop {
            interval.tick().await;
            let sent = evaluate_and_dispatch(&state).await;
            if sent > 0 {
                info!("✅ [Session Alerts] Đã gửi {} briefing trước phiên.", sent);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Candle, Timeframe};

    fn snapshot(live_price: Option<f64>, last_close: f64) -> MarketFeedSnapshot {
        MarketFeedSnapshot {
            candles: vec![Candle {
                timestamp: 1,
                open: last_close,
                high: last_close,
                low: last_close,
                close: last_close,
                volume: 100.0,
                taker_buy_volume: 0.0,
                timeframe: Timeframe::M15,
            }],
            source: "BINANCE_PAXG".to_string(),
            live_price,
            volume_source: "BINANCE_PAXG".to_string(),
        }
    }

    #[test]
    fn spot_basis_uses_live_price_and_removes_manual_offset() {
        let snapshot = snapshot(Some(4106.10), 4100.0);
        let basis = compute_spot_basis(&snapshot, 4111.17, 2.0).expect("finite basis");
        assert!((basis - 7.07).abs() < 1e-9);
    }

    #[test]
    fn spot_basis_falls_back_to_last_close_and_clamps() {
        // spot 9000, PAXG 8850 -> basis 150 (2% = 180) hợp lệ, clamp về +100.
        let snapshot = snapshot(None, 8850.0);
        let basis = compute_spot_basis(&snapshot, 9000.0, 0.0).expect("finite basis");
        assert_eq!(basis, 100.0);

        assert_eq!(compute_spot_basis(&snapshot, 0.0, 0.0), None);
        assert_eq!(compute_spot_basis(&snapshot, f64::NAN, 0.0), None);
    }

    #[test]
    fn spot_basis_accepts_plausible_difference() {
        let snapshot = snapshot(Some(4095.0), 4095.0);
        let basis = compute_spot_basis(&snapshot, 4100.0, 0.0).expect("plausible basis");
        assert!((basis - 5.0).abs() < 1e-9);
    }

    #[test]
    fn spot_basis_rejects_implausible_difference() {
        let snapshot = snapshot(Some(4000.0), 4000.0);
        // 500 > max(4500*2%=90, 50) -> outlier.
        assert_eq!(compute_spot_basis(&snapshot, 4500.0, 0.0), None);
        // 100 > max(4100*2%=82, 50) -> outlier.
        assert_eq!(compute_spot_basis(&snapshot, 4100.0, 0.0), None);
    }

    #[test]
    fn spot_basis_rejects_bad_spot_price() {
        let snapshot = snapshot(Some(4000.0), 4000.0);
        for bad in [
            0.0,
            -1.0,
            499.99,
            10_000.01,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ] {
            assert_eq!(
                compute_spot_basis(&snapshot, bad, 0.0),
                None,
                "spot {bad} must be rejected"
            );
        }
    }

    #[test]
    fn htf_shift_uses_spot_basis_else_broker_offset() {
        assert!((htf_shift_amount(Some(5.07), 2.0) - 5.07).abs() < 1e-9);
        assert!((htf_shift_amount(None, 2.0) - 2.0).abs() < 1e-9);
        assert_eq!(htf_shift_amount(None, 0.0), 0.0);
        assert!(htf_shift_amount(None, f64::NAN).is_nan());
        assert!(htf_shift_amount(Some(f64::INFINITY), 2.0).is_infinite());
    }

    #[test]
    fn htf_price_source_labels_cover_binance_and_twelvedata_modes() {
        assert_eq!(htf_price_source(None), "BINANCE_PAXG");
        assert_eq!(htf_price_source(Some(5.07)), "BINANCE_PAXG+TWELVEDATA_SPOT");
        assert_eq!(
            twelve_htf_price_source(&["TWELVEDATA_XAUUSD+YAHOO_FUTURES_VOLUME"]),
            "TWELVEDATA_XAUUSD+YAHOO_FUTURES_VOLUME"
        );
        assert_eq!(
            twelve_htf_price_source(&["TWELVEDATA_XAUUSD+PAXG_VOLUME"]),
            "TWELVEDATA_XAUUSD+PAXG_VOLUME"
        );
        assert_eq!(
            twelve_htf_price_source(&["TWELVEDATA_XAUUSD"]),
            "TWELVEDATA_XAUUSD"
        );
    }

    #[test]
    fn data_source_normalization_preserves_twelvedata() {
        let mut twelve = BotConfig {
            data_source: "  twelvedata ".to_string(),
            ..BotConfig::default()
        };
        normalize_data_source(&mut twelve);
        assert_eq!(twelve.data_source, "TWELVEDATA");

        let mut already_binance = BotConfig::default();
        normalize_data_source(&mut already_binance);
        assert_eq!(already_binance.data_source, "BINANCE_PAXG");

        let mut oanda = BotConfig {
            data_source: " oanda ".to_string(),
            ..BotConfig::default()
        };
        normalize_data_source(&mut oanda);
        assert_eq!(oanda.data_source, "OANDA");

        let mut unknown = BotConfig {
            data_source: "garbage".to_string(),
            ..BotConfig::default()
        };
        normalize_data_source(&mut unknown);
        assert_eq!(unknown.data_source, "BINANCE_PAXG");

        let mut empty = BotConfig {
            data_source: String::new(),
            ..BotConfig::default()
        };
        normalize_data_source(&mut empty);
        assert_eq!(empty.data_source, "BINANCE_PAXG");
    }
}
