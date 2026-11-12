use crate::models::{
    BotConfig, Candle, Signal, SignalStatus, SignalType, SupportResistanceZone, Timeframe, ZoneType,
};
use crate::news_filter::MacroBias;
use crate::risk_manager::PositionSizer;
use crate::signal_validation::{validate_risk_budget, validate_signal_levels};
use chrono::Utc;
use tracing::{info, warn};

/// Bộ máy Phân Tích Kỹ Thuật Tự Động cho Vàng (XAU/USD)
pub struct GoldMarketAnalyzer;

impl GoldMarketAnalyzer {
    /// 1. Tự động quét và phát hiện các vùng Hỗ trợ / Kháng cự động từ dữ liệu nến
    pub fn detect_dynamic_sr_zones(
        candles: &[Candle],
        lookback: usize,
        cluster_tolerance: f64, // Ví dụ: 2.0$ cho XAU/USD để gộp các râu nến gần nhau
    ) -> (Vec<SupportResistanceZone>, Vec<SupportResistanceZone>) {
        if candles.len() < lookback || lookback < 5 {
            return (Vec::new(), Vec::new());
        }

        let slice = &candles[candles.len().saturating_sub(lookback)..];
        let mut raw_highs: Vec<f64> = Vec::new();
        let mut raw_lows: Vec<f64> = Vec::new();

        // Tìm Swing High & Swing Low cục bộ (Pivot 2 nến trước, 2 nến sau)
        for i in 2..(slice.len().saturating_sub(2)) {
            let curr = &slice[i];
            let is_swing_high = curr.high > slice[i - 1].high
                && curr.high > slice[i - 2].high
                && curr.high >= slice[i + 1].high
                && curr.high >= slice[i + 2].high;

            let is_swing_low = curr.low < slice[i - 1].low
                && curr.low < slice[i - 2].low
                && curr.low <= slice[i + 1].low
                && curr.low <= slice[i + 2].low;

            if is_swing_high {
                raw_highs.push(curr.high);
            }
            if is_swing_low {
                raw_lows.push(curr.low);
            }
        }

        let timeframe = candles
            .last()
            .map(|c| c.timeframe)
            .unwrap_or(Timeframe::M15);
        let resistances = Self::cluster_price_levels(
            raw_highs,
            ZoneType::Resistance,
            cluster_tolerance,
            timeframe,
        );
        let supports =
            Self::cluster_price_levels(raw_lows, ZoneType::Support, cluster_tolerance, timeframe);

        (supports, resistances)
    }

    /// Gom cụm các mức giá gần nhau thành vùng giá có độ tin cậy (Zone Clustering)
    fn cluster_price_levels(
        mut prices: Vec<f64>,
        zone_type: ZoneType,
        tolerance: f64,
        timeframe: Timeframe,
    ) -> Vec<SupportResistanceZone> {
        if prices.is_empty() {
            return Vec::new();
        }

        prices.sort_by(f64::total_cmp);
        let mut zones: Vec<SupportResistanceZone> = Vec::new();

        let mut current_cluster: Vec<f64> = vec![prices[0]];

        for &price in &prices[1..] {
            let cluster_avg: f64 =
                current_cluster.iter().sum::<f64>() / (current_cluster.len() as f64);
            if (price - cluster_avg).abs() <= tolerance {
                current_cluster.push(price);
            } else {
                zones.push(Self::build_zone_from_cluster(
                    &current_cluster,
                    zone_type,
                    timeframe,
                ));
                current_cluster = vec![price];
            }
        }

        if !current_cluster.is_empty() {
            zones.push(Self::build_zone_from_cluster(
                &current_cluster,
                zone_type,
                timeframe,
            ));
        }

        // Sắp xếp theo số lần chạm (touch count) giảm dần
        zones.sort_by_key(|zone| std::cmp::Reverse(zone.touch_count));
        zones
    }

    fn build_zone_from_cluster(
        cluster: &[f64],
        zone_type: ZoneType,
        timeframe: Timeframe,
    ) -> SupportResistanceZone {
        let avg_price = cluster.iter().sum::<f64>() / (cluster.len() as f64);
        let min_p = cluster.iter().cloned().fold(f64::INFINITY, f64::min);
        let max_p = cluster.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let touch_count = cluster.len();
        let strength = (touch_count as f64 * 0.25).min(1.0);

        SupportResistanceZone {
            level_type: zone_type,
            price: (avg_price * 100.0).round() / 100.0,
            min_price: (min_p * 100.0).round() / 100.0,
            max_price: (max_p * 100.0).round() / 100.0,
            touch_count,
            timeframe,
            strength_score: strength,
        }
    }

    /// 2. Tính khối lượng trung bình (Volume Moving Average)
    pub fn calculate_volume_ma(candles: &[Candle], period: usize) -> f64 {
        if candles.is_empty() || period == 0 {
            return 0.0;
        }
        let take_count = period.min(candles.len());
        let sum: f64 = candles[candles.len() - take_count..]
            .iter()
            .map(|c| c.volume)
            .sum();
        sum / (take_count as f64)
    }

    /// 3. Tính đường Exponential Moving Average (EMA) từ giá đóng cửa.
    ///
    /// Nếu chuỗi ngắn hơn period, dùng trung bình cộng của toàn bộ chuỗi làm seed.
    pub fn calculate_ema(candles: &[Candle], period: usize) -> f64 {
        if candles.is_empty() || period == 0 {
            return 0.0;
        }

        let closes: Vec<f64> = candles.iter().map(|c| c.close).collect();
        let effective_period = period.min(closes.len());
        let seed: f64 = closes[..effective_period].iter().sum::<f64>() / (effective_period as f64);
        let smoothing = 2.0 / (period as f64 + 1.0);

        closes[effective_period..].iter().fold(seed, |ema, &close| {
            close * smoothing + ema * (1.0 - smoothing)
        })
    }

    /// 4. Xác định xu hướng tự động MIỄN PHÍ bằng EMA50 (thay cho AI trả phí).
    /// - Giá trên EMA50 và EMA50 dốc lên => BULLISH
    /// - Giá dưới EMA50 và EMA50 dốc xuống => BEARISH
    /// - Còn lại => NEUTRAL (cho phép cả BUY và SELL)
    pub fn detect_ema_trend_bias(candles: &[Candle]) -> MacroBias {
        if candles.len() < 20 {
            return MacroBias::Neutral;
        }

        let ema_now = Self::calculate_ema(candles, 50);
        let ema_prev = Self::calculate_ema(&candles[..candles.len() - 1], 50);
        let last_close = candles.last().map(|c| c.close).unwrap_or(0.0);

        if ema_now <= 0.0 {
            return MacroBias::Neutral;
        }

        if last_close > ema_now && ema_now > ema_prev {
            MacroBias::Bullish
        } else if last_close < ema_now && ema_now < ema_prev {
            MacroBias::Bearish
        } else {
            MacroBias::Neutral
        }
    }

    /// 5. Phân tích Price Action: Nhận diện nến rút chân (Pinbar / Liquidity Rejection)
    ///
    /// Trả về (is_bullish_rejection, is_bearish_rejection)
    pub fn check_pinbar_rejection(candle: &Candle, rejection_wick_ratio: f64) -> (bool, bool) {
        let range = candle.range();
        let body = candle.body_size();
        let lower_wick = candle.lower_wick();
        let upper_wick = candle.upper_wick();

        // Nến Rút Chân Tăng (Bullish Pinbar / Hammer):
        // - Râu dưới dài ít nhất bằng N lần thân nến (mặc định 2.0x)
        // - Râu dưới chiếm trên 50% tổng biên độ cây nến
        // - Râu trên ngắn (chiếm dưới 25% tổng biên độ cây nến)
        let is_bullish_pinbar = lower_wick >= (body * rejection_wick_ratio)
            && (lower_wick / range) >= 0.48
            && (upper_wick / range) <= 0.28;

        // Nến Rút Chân Giảm (Bearish Pinbar / Shooting Star):
        // - Râu trên dài ít nhất bằng N lần thân nến
        // - Râu trên chiếm trên 50% tổng biên độ cây nến
        // - Râu dưới ngắn (chiếm dưới 25% tổng biên độ)
        let is_bearish_pinbar = upper_wick >= (body * rejection_wick_ratio)
            && (upper_wick / range) >= 0.48
            && (lower_wick / range) <= 0.28;

        (is_bullish_pinbar, is_bearish_pinbar)
    }

    /// 6. Điểm hội tụ kỹ thuật (Confluence Score) trong khoảng 80..=98.
    fn calculate_confluence_score(
        volume_surge_ratio: f64,
        touch_count: usize,
        rejection_wick_ratio: f64,
    ) -> u8 {
        let volume_points = ((volume_surge_ratio - 1.0).max(0.0) * 6.0).min(10.0);
        let touch_points = (touch_count.min(4) as f64) * 1.5;
        let wick_points = ((rejection_wick_ratio - 2.0).max(0.0) * 3.0).min(6.0);
        let score = 80.0 + volume_points + touch_points + wick_points;
        score.round().clamp(80.0, 98.0) as u8
    }

    /// 7. Đánh giá toàn diện và tính toán Tự Động R:R, Entry, Stop Loss, Take Profit.
    ///
    /// Wrapper tương thích cho caller cũ (không có giá live, không áp dụng stale guard).
    pub fn analyze_market(candles: &[Candle], config: &BotConfig) -> Option<Signal> {
        Self::analyze_market_with_live_price(candles, config, None, Utc::now().timestamp_millis())
    }

    /// Phân tích với giá live và timestamp hiện tại để chống tín hiệu trễ/đuổi giá.
    pub fn analyze_market_with_live_price(
        candles: &[Candle],
        config: &BotConfig,
        live_price: Option<f64>,
        now_ms: i64,
    ) -> Option<Signal> {
        if candles.len() < config.lookback_period + 5 {
            return None;
        }

        let current_candle = candles.last()?;
        let previous_candles = &candles[..candles.len() - 1];

        // Stale guard: chỉ áp dụng khi có giá live để không phá vỡ test dữ liệu lịch sử.
        if live_price.is_some() {
            let candle_close_time =
                current_candle.timestamp + timeframe_millis(current_candle.timeframe);
            let max_age_ms = config.max_candle_age_secs.saturating_mul(1000);
            if now_ms - candle_close_time > max_age_ms {
                info!(
                    "⏱️ [Analyzer] Bỏ qua nến quá cũ: đóng lúc {}, hiện tại {} (tuổi {}ms > {}ms).",
                    candle_close_time,
                    now_ms,
                    now_ms - candle_close_time,
                    max_age_ms
                );
                return None;
            }
        }

        // Lọc nhiễu: bỏ qua nến Doji đứng im (biên độ quá nhỏ).
        if current_candle.range() < 1.2 {
            return None;
        }

        // Định hướng xu hướng: AUTO dùng EMA50, ngược lại đọc cấu hình.
        let effective_bias = if config.macro_bias.eq_ignore_ascii_case("AUTO") {
            Self::detect_ema_trend_bias(previous_candles)
        } else {
            MacroBias::from_str_ci(&config.macro_bias)
        };
        let allow_buy = effective_bias != MacroBias::Bearish;
        let allow_sell = effective_bias != MacroBias::Bullish;

        // Bước A: Tính Volume MA & Hệ số hấp thụ
        let volume_ma = Self::calculate_volume_ma(previous_candles, config.volume_ma_period);
        let volume_surge_ratio = if volume_ma > 0.0 {
            current_candle.volume / volume_ma
        } else {
            1.0
        };

        // Lọc nhiễu: Bỏ qua nếu khối lượng không đủ hấp thụ dòng tiền.
        // Nguồn không có volume (TwelveData không proxy được PAXG) có thể tắt filter.
        let is_volume_valid = if config.volume_filter_enabled {
            volume_surge_ratio >= config.volume_surge_multiplier
        } else {
            true
        };

        // Bước B: Quét vùng hỗ trợ / kháng cự động
        let (supports, resistances) = Self::detect_dynamic_sr_zones(
            previous_candles,
            config.lookback_period,
            2.5, // 2.5$ biên độ cluster cho Vàng
        );

        // Bước C: Kiểm tra mô hình Nến rút chân
        let (is_bullish_pinbar, is_bearish_pinbar) =
            Self::check_pinbar_rejection(current_candle, config.rejection_wick_ratio);

        // Biên độ chấp nhận kiểm tra lại vùng cản (Zone Retest Tolerance)
        let zone_touch_tolerance = 2.5; // Trong phạm vi 2.5$

        // ================= KỊCH BẢN 1: MUA (BUY SIGNAL) =================
        if allow_buy && is_bullish_pinbar && is_volume_valid {
            // Tìm vùng hỗ trợ gần nhất mà nến rút chân chạm vào hoặc đâm râu xuyên qua
            let nearest_support = supports.iter().find(|zone| {
                let diff_from_low = (current_candle.low - zone.price).abs();
                let diff_from_zone_min = (current_candle.low - zone.min_price).abs();
                diff_from_low <= zone_touch_tolerance
                    || diff_from_zone_min <= zone_touch_tolerance
                    || (current_candle.low <= zone.max_price
                        && current_candle.close >= zone.min_price)
            });

            if let Some(support) = nearest_support {
                // Stop Loss an toàn: Cài dưới đáy nến và dưới mép dưới vùng hỗ trợ, có cộng thêm đệm an toàn
                let base_sl = current_candle.low.min(support.min_price);
                let stop_loss = round2(base_sl - config.gold_sl_buffer_points);

                // Vùng retest = midpoint giữa close và đáy nến/vùng hỗ trợ.
                let retest_limit_price = round2(
                    (current_candle.close + current_candle.low.max(support.min_price)) / 2.0,
                );
                let (entry_price, entry_type, is_entry_filled) = select_entry(
                    true,
                    current_candle,
                    stop_loss,
                    retest_limit_price,
                    live_price,
                    config,
                )?;
                let risk_points = round2((entry_price - stop_loss).abs());

                // Stop loss hợp lý (> 3$ và <= 45$ cho vàng — hỗ trợ SL Buffer tới 20-25 giá)
                if (3.0..=45.0).contains(&risk_points) {
                    let reward_tp1 =
                        ((risk_points * config.min_risk_reward) * 100.0).round() / 100.0;
                    let take_profit_1 = ((entry_price + reward_tp1) * 100.0).round() / 100.0;

                    // TP2: kháng cự gần nhất nằm ngoài TP1, nếu không có thì fallback >= 3R.
                    let take_profit_2 = select_tp2(
                        true,
                        entry_price,
                        take_profit_1,
                        risk_points,
                        &resistances,
                        config.min_risk_reward,
                    );

                    let reward_tp2 = round2((take_profit_2 - entry_price).abs());
                    let risk_reward_ratio = round2(reward_tp1 / risk_points);

                    if risk_reward_ratio >= config.min_risk_reward {
                        info!(
                            "🟢 TÍN HIỆU MUA XAU/USD PHÁT HIỆN: Entry={}, SL={}, TP1={}, R:R={}",
                            entry_price, stop_loss, take_profit_1, risk_reward_ratio
                        );

                        // Tính toán quản lý vốn tự động
                        let pos_sizing = PositionSizer::calculate_lot_size(
                            config.account_balance,
                            config.risk_percentage,
                            risk_points,
                            reward_tp1,
                            reward_tp2,
                            entry_price,
                            config.account_leverage,
                        );

                        // Chặn tín hiệu nếu MIN_LOT đẩy rủi ro vượt ngân sách cấu hình.
                        if let Err(reason) = validate_risk_budget(config, &pos_sizing) {
                            warn!(
                                "🛑 [Analyzer] Bỏ qua tín hiệu BUY do vượt ngân sách rủi ro: {}",
                                reason
                            );
                            return None;
                        }

                        let wick_ratio = if current_candle.body_size() > 0.0 {
                            current_candle.lower_wick() / current_candle.body_size()
                        } else {
                            0.0
                        };
                        let confluence_score = Self::calculate_confluence_score(
                            volume_surge_ratio,
                            support.touch_count,
                            wick_ratio,
                        );
                        let dedup_key = format!(
                            "{}_{}_{}",
                            config.symbol, config.timeframe, current_candle.timestamp
                        );

                        if let Err(reason) = validate_signal_levels(
                            SignalType::Buy,
                            entry_price,
                            stop_loss,
                            take_profit_1,
                            take_profit_2,
                            config.min_risk_reward,
                        ) {
                            warn!("🛑 [Analyzer] Bỏ qua tín hiệu BUY không hợp lệ: {}", reason);
                            return None;
                        }

                        return Some(Signal {
                            id: format!("SIG-{}-{}", Utc::now().timestamp_millis(), "BUY"),
                            symbol: config.symbol.clone(),
                            signal_type: SignalType::Buy,
                            timeframe: config.timeframe,
                            entry_price,
                            stop_loss,
                            take_profit_1,
                            take_profit_2,
                            risk_points,
                            reward_points_tp1: reward_tp1,
                            reward_points_tp2: reward_tp2,
                            risk_reward_ratio,
                            volume_surge_ratio: ((volume_surge_ratio * 100.0).round()) / 100.0,
                            recommended_lot_size: pos_sizing.lot_size,
                            risk_amount_usd: pos_sizing.risk_amount_usd,
                            potential_profit_tp1_usd: pos_sizing.potential_profit_tp1_usd,
                            potential_profit_tp2_usd: pos_sizing.potential_profit_tp2_usd,
                            price_action_notes: format!(
                                "Nến rút chân Bullish Pinbar (râu dưới {:.1}$ chiếm {:.0}% biên độ) phản ứng tại Hỗ trợ động ${:.2}",
                                current_candle.lower_wick(),
                                (current_candle.lower_wick() / current_candle.range()) * 100.0,
                                support.price
                            ),
                            volume_analysis_notes: format!(
                                "Volume đạt {:.2}x MA{}, dòng tiền tổ chức hấp thụ lực bán tại đáy cản (Liquidity Absorption)",
                                volume_surge_ratio, config.volume_ma_period
                            ),
                            full_rationale: format!(
                                "Nến từ chối giảm tạo râu dưới dài tại vùng Hỗ trợ ${:.2} (đã retest {} lần). Khối lượng tăng vọt {:.2}x xác nhận phe mua nhập cuộc bảo vệ vùng giá. Tỷ lệ R:R đạt 1:{:.2}. Khối lượng đề xuất: {:.2} lot (Rủi ro: -${:.2}, TP1: +${:.2}).",
                                support.price, support.touch_count, volume_surge_ratio, risk_reward_ratio,
                                pos_sizing.lot_size, pos_sizing.risk_amount_usd, pos_sizing.potential_profit_tp1_usd
                            ),
                            status: SignalStatus::Active,
                            created_at: Utc::now(),
                            telegram_message_id: None,
                            dedup_key,
                            candle_timestamp: current_candle.timestamp,
                            is_simulated: false,
                            confluence_score,
                            outcome_at: None,
                            entry_type,
                            candle_close_price: round2(current_candle.close),
                            live_market_price: live_price,
                            retest_limit_price: Some(retest_limit_price),
                            data_source: config.data_source.clone(),
                            is_entry_filled,
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
                        });
                    }
                }
            }
        }

        // ================= KỊCH BẢN 2: BÁN (SELL SIGNAL) =================
        if allow_sell && is_bearish_pinbar && is_volume_valid {
            // Tìm vùng kháng cự gần nhất mà nến rút chân chạm vào hoặc đâm râu xuyên qua
            let nearest_resistance = resistances.iter().find(|zone| {
                let diff_from_high = (current_candle.high - zone.price).abs();
                let diff_from_zone_max = (current_candle.high - zone.max_price).abs();
                diff_from_high <= zone_touch_tolerance
                    || diff_from_zone_max <= zone_touch_tolerance
                    || (current_candle.high >= zone.min_price
                        && current_candle.close <= zone.max_price)
            });

            if let Some(resistance) = nearest_resistance {
                // Stop Loss an toàn: Cài trên đỉnh nến và trên mép trên vùng kháng cự, có cộng thêm đệm an toàn
                let base_sl = current_candle.high.max(resistance.max_price);
                let stop_loss = round2(base_sl + config.gold_sl_buffer_points);

                // Vùng retest = midpoint giữa close và đỉnh nến/vùng kháng cự.
                let retest_limit_price = round2(
                    (current_candle.close + current_candle.high.min(resistance.max_price)) / 2.0,
                );
                let (entry_price, entry_type, is_entry_filled) = select_entry(
                    false,
                    current_candle,
                    stop_loss,
                    retest_limit_price,
                    live_price,
                    config,
                )?;
                let risk_points = round2((stop_loss - entry_price).abs());

                if (3.0..=45.0).contains(&risk_points) {
                    let reward_tp1 =
                        ((risk_points * config.min_risk_reward) * 100.0).round() / 100.0;
                    let take_profit_1 = ((entry_price - reward_tp1) * 100.0).round() / 100.0;

                    // TP2: hỗ trợ gần nhất nằm ngoài TP1, nếu không có thì fallback >= 3R.
                    let take_profit_2 = select_tp2(
                        false,
                        entry_price,
                        take_profit_1,
                        risk_points,
                        &supports,
                        config.min_risk_reward,
                    );

                    let reward_tp2 = round2((entry_price - take_profit_2).abs());
                    let risk_reward_ratio = round2(reward_tp1 / risk_points);

                    if risk_reward_ratio >= config.min_risk_reward {
                        info!(
                            "🔴 TÍN HIỆU BÁN XAU/USD PHÁT HIỆN: Entry={}, SL={}, TP1={}, R:R={}",
                            entry_price, stop_loss, take_profit_1, risk_reward_ratio
                        );

                        // Tính toán quản lý vốn tự động
                        let pos_sizing = PositionSizer::calculate_lot_size(
                            config.account_balance,
                            config.risk_percentage,
                            risk_points,
                            reward_tp1,
                            reward_tp2,
                            entry_price,
                            config.account_leverage,
                        );

                        // Chặn tín hiệu nếu MIN_LOT đẩy rủi ro vượt ngân sách cấu hình.
                        if let Err(reason) = validate_risk_budget(config, &pos_sizing) {
                            warn!(
                                "🛑 [Analyzer] Bỏ qua tín hiệu SELL do vượt ngân sách rủi ro: {}",
                                reason
                            );
                            return None;
                        }

                        let wick_ratio = if current_candle.body_size() > 0.0 {
                            current_candle.upper_wick() / current_candle.body_size()
                        } else {
                            0.0
                        };
                        let confluence_score = Self::calculate_confluence_score(
                            volume_surge_ratio,
                            resistance.touch_count,
                            wick_ratio,
                        );
                        let dedup_key = format!(
                            "{}_{}_{}",
                            config.symbol, config.timeframe, current_candle.timestamp
                        );

                        if let Err(reason) = validate_signal_levels(
                            SignalType::Sell,
                            entry_price,
                            stop_loss,
                            take_profit_1,
                            take_profit_2,
                            config.min_risk_reward,
                        ) {
                            warn!(
                                "🛑 [Analyzer] Bỏ qua tín hiệu SELL không hợp lệ: {}",
                                reason
                            );
                            return None;
                        }

                        return Some(Signal {
                            id: format!("SIG-{}-{}", Utc::now().timestamp_millis(), "SELL"),
                            symbol: config.symbol.clone(),
                            signal_type: SignalType::Sell,
                            timeframe: config.timeframe,
                            entry_price,
                            stop_loss,
                            take_profit_1,
                            take_profit_2,
                            risk_points,
                            reward_points_tp1: reward_tp1,
                            reward_points_tp2: reward_tp2,
                            risk_reward_ratio,
                            volume_surge_ratio: ((volume_surge_ratio * 100.0).round()) / 100.0,
                            recommended_lot_size: pos_sizing.lot_size,
                            risk_amount_usd: pos_sizing.risk_amount_usd,
                            potential_profit_tp1_usd: pos_sizing.potential_profit_tp1_usd,
                            potential_profit_tp2_usd: pos_sizing.potential_profit_tp2_usd,
                            price_action_notes: format!(
                                "Nến rút chân Bearish Pinbar (râu trên {:.1}$ chiếm {:.0}% biên độ) phản ứng tại Kháng cự động ${:.2}",
                                current_candle.upper_wick(),
                                (current_candle.upper_wick() / current_candle.range()) * 100.0,
                                resistance.price
                            ),
                            volume_analysis_notes: format!(
                                "Volume đạt {:.2}x MA{}, dòng tiền tổ chức từ chối vùng giá cao (Smart Money Exhaustion)",
                                volume_surge_ratio, config.volume_ma_period
                            ),
                            full_rationale: format!(
                                "Nến từ chối tăng tạo râu trên dài tại Kháng cự ${:.2} (đã kiểm tra {} lần). Khối lượng đột biến {:.2}x xác nhận lực bán áp đảo. Tỷ lệ R:R đạt 1:{:.2}. Khối lượng đề xuất: {:.2} lot (Rủi ro: -${:.2}, TP1: +${:.2}).",
                                resistance.price, resistance.touch_count, volume_surge_ratio, risk_reward_ratio,
                                pos_sizing.lot_size, pos_sizing.risk_amount_usd, pos_sizing.potential_profit_tp1_usd
                            ),
                            status: SignalStatus::Active,
                            created_at: Utc::now(),
                            telegram_message_id: None,
                            dedup_key,
                            candle_timestamp: current_candle.timestamp,
                            is_simulated: false,
                            confluence_score,
                            outcome_at: None,
                            entry_type,
                            candle_close_price: round2(current_candle.close),
                            live_market_price: live_price,
                            retest_limit_price: Some(retest_limit_price),
                            data_source: config.data_source.clone(),
                            is_entry_filled,
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
                        });
                    }
                }
            }
        }

        None
    }
}

/// Chọn entry theo `entry_mode`, bảo vệ chống đuổi giá và chống vào lệnh khi giá đã xuyên SL.
fn select_entry(
    is_buy: bool,
    current_candle: &Candle,
    stop_loss: f64,
    retest_limit_price: f64,
    live_price: Option<f64>,
    config: &BotConfig,
) -> Option<(f64, crate::models::EntryType, bool)> {
    use crate::models::EntryType;

    let ref_close = current_candle.close;
    let live = live_price.unwrap_or(ref_close);
    let entry_mode = config.entry_mode.trim().to_uppercase();

    // Entry không được nằm sai phía Stop Loss (BUY phải trên SL, SELL phải dưới SL).
    let through_sl = |entry: f64, label: &str| -> bool {
        let invalid = if is_buy {
            entry <= stop_loss
        } else {
            entry >= stop_loss
        };
        if invalid {
            info!(
                "🛑 [Analyzer] Entry {} {:.2} đã xuyên SL {:.2}, hủy tín hiệu.",
                label, entry, stop_loss
            );
        }
        invalid
    };

    match entry_mode.as_str() {
        "LIMIT_RETEST" => {
            if through_sl(retest_limit_price, "LIMIT_RETEST") {
                return None;
            }
            Some((retest_limit_price, EntryType::Limit, false))
        }
        "BREAKOUT" => {
            let entry = round2(if is_buy {
                current_candle.high + 0.5
            } else {
                current_candle.low - 0.5
            });
            if through_sl(entry, "BREAKOUT") {
                return None;
            }
            Some((entry, EntryType::Breakout, false))
        }
        // MARKET (và fallback an toàn cho mode lạ).
        _ => {
            if (live - ref_close).abs() <= config.max_entry_drift_points {
                if through_sl(live, "MARKET") {
                    return None;
                }
                let entry = round2(live);
                if through_sl(entry, "MARKET") {
                    return None;
                }
                Some((entry, EntryType::Market, true))
            } else {
                if through_sl(live, "MARKET") {
                    return None;
                }
                // Drift quá lớn: không đuổi giá, chờ giá quay về close của nến Pinbar.
                let entry = round2(ref_close);
                if through_sl(entry, "MARKET_FALLBACK") {
                    return None;
                }
                Some((entry, EntryType::Limit, false))
            }
        }
    }
}

/// Chọn TP2 an toàn: mức cản gần nhất nằm ngoài TP1 + $0.01, nếu không có
/// thì fallback tối thiểu 3R và luôn xa hơn TP1 ít nhất $0.5.
fn select_tp2(
    is_buy: bool,
    entry_price: f64,
    take_profit_1: f64,
    risk_points: f64,
    zones: &[SupportResistanceZone],
    min_risk_reward: f64,
) -> f64 {
    if is_buy {
        let candidate = zones
            .iter()
            .map(|zone| round2(zone.price - 1.0))
            .filter(|price| *price > take_profit_1 + 0.01)
            .min_by(f64::total_cmp);

        if let Some(price) = candidate {
            return price;
        }

        let fallback_distance = (3.0 * risk_points)
            .max((take_profit_1 - entry_price).abs() + 0.5)
            .max(min_risk_reward * risk_points + 0.5);
        round2(entry_price + fallback_distance)
    } else {
        let candidate = zones
            .iter()
            .map(|zone| round2(zone.price + 1.0))
            .filter(|price| *price < take_profit_1 - 0.01)
            .max_by(f64::total_cmp);

        if let Some(price) = candidate {
            return price;
        }

        let fallback_distance = (3.0 * risk_points)
            .max((entry_price - take_profit_1).abs() + 0.5)
            .max(min_risk_reward * risk_points + 0.5);
        round2(entry_price - fallback_distance)
    }
}

fn timeframe_millis(timeframe: Timeframe) -> i64 {
    match timeframe {
        Timeframe::M15 => 15 * 60 * 1000,
        Timeframe::H1 => 60 * 60 * 1000,
        Timeframe::H4 => 4 * 60 * 60 * 1000,
    }
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE_TS: i64 = 1_700_000_000_000;
    const STEP_MS: i64 = 15 * 60 * 1000;

    fn candle(timestamp: i64, open: f64, high: f64, low: f64, close: f64, volume: f64) -> Candle {
        Candle {
            timestamp,
            open,
            high,
            low,
            close,
            volume,
            taker_buy_volume: 0.0,
            timeframe: Timeframe::M15,
        }
    }

    /// Chuỗi nến zigzag tạo swing high 2690.5 và swing low 2669.5 ổn định.
    fn zigzag_candles(count: usize) -> Vec<Candle> {
        (0..count)
            .map(|i| {
                let level = match i % 4 {
                    0 => 2670.0,
                    2 => 2690.0,
                    _ => 2680.0,
                };
                candle(
                    BASE_TS + i as i64 * STEP_MS,
                    level,
                    level + 0.5,
                    level - 0.5,
                    level,
                    1000.0,
                )
            })
            .collect()
    }

    fn bullish_pinbar(timestamp: i64) -> Candle {
        candle(timestamp, 2674.0, 2675.6, 2669.5, 2675.0, 2000.0)
    }

    fn buy_config(gold_sl_buffer_points: f64) -> BotConfig {
        BotConfig {
            gold_sl_buffer_points,
            lookback_period: 30,
            volume_ma_period: 20,
            min_risk_reward: 2.0,
            volume_surge_multiplier: 1.5,
            rejection_wick_ratio: 2.0,
            macro_bias: "BULLISH".to_string(),
            account_balance: 10_000.0,
            risk_percentage: 1.0,
            ..BotConfig::default()
        }
    }

    fn expect_buy_signal(gold_sl_buffer_points: f64) -> Signal {
        let mut candles = zigzag_candles(60);
        let signal_ts = BASE_TS + 60 * STEP_MS;
        candles.push(bullish_pinbar(signal_ts));

        GoldMarketAnalyzer::analyze_market(&candles, &buy_config(gold_sl_buffer_points))
            .expect("expected a valid BUY signal")
    }

    #[test]
    fn test_detect_dynamic_sr_zones() {
        let candles = zigzag_candles(60);
        let (supports, resistances) =
            GoldMarketAnalyzer::detect_dynamic_sr_zones(&candles, 30, 2.5);

        assert!(!supports.is_empty(), "expected support zones");
        assert!(!resistances.is_empty(), "expected resistance zones");

        let support = supports.first().expect("support zone");
        let resistance = resistances.first().expect("resistance zone");
        assert!(support.price < resistance.price);
        assert!(support.touch_count > 1);
        assert!(resistance.touch_count > 1);
        assert!(support.min_price <= support.price);
        assert!(support.max_price >= support.price);
    }

    #[test]
    fn test_calculate_ema_tracks_rising_series() {
        let candles: Vec<Candle> = (0..60)
            .map(|i| {
                let close = 2600.0 + i as f64;
                candle(
                    BASE_TS + i as i64 * STEP_MS,
                    close,
                    close + 1.0,
                    close - 1.0,
                    close,
                    1000.0,
                )
            })
            .collect();

        let ema = GoldMarketAnalyzer::calculate_ema(&candles, 50);
        assert!(ema > 2600.0);
        assert!(ema < 2659.0);
        assert_eq!(
            GoldMarketAnalyzer::detect_ema_trend_bias(&candles),
            MacroBias::Bullish
        );
    }

    #[test]
    fn test_detect_ema_trend_bias_bearish_series() {
        let candles: Vec<Candle> = (0..60)
            .map(|i| {
                let close = 2700.0 - i as f64;
                candle(
                    BASE_TS + i as i64 * STEP_MS,
                    close,
                    close + 1.0,
                    close - 1.0,
                    close,
                    1000.0,
                )
            })
            .collect();

        assert_eq!(
            GoldMarketAnalyzer::detect_ema_trend_bias(&candles),
            MacroBias::Bearish
        );
    }

    #[test]
    fn test_analyze_market_buy_signal_with_10_point_sl() {
        let signal = expect_buy_signal(10.0);

        assert_eq!(signal.signal_type, SignalType::Buy);
        assert_eq!(signal.symbol, "XAU/USD");
        assert!((signal.risk_points - 15.5).abs() < 1e-6);
        assert!((signal.stop_loss - 2659.5).abs() < 1e-6);
        assert_eq!(signal.recommended_lot_size, 0.06);
        // Rủi ro thực tế sau khi làm tròn lot: 0.06 * 15.5 * 100 = $93 (< $100 max).
        assert!((signal.risk_amount_usd - 93.0).abs() < 1e-6);
        assert!(signal.risk_amount_usd <= 10_000.0 * 0.01 + 1e-6);
        assert_eq!(
            signal.dedup_key,
            format!("XAU/USD_M15_{}", signal.candle_timestamp)
        );
        assert!(!signal.is_simulated);
        assert!((80..=98).contains(&signal.confluence_score));
    }

    #[test]
    fn test_analyze_market_buy_signal_with_20_point_sl() {
        let signal_10 = expect_buy_signal(10.0);
        let signal_20 = expect_buy_signal(20.0);

        // SL Buffer 20 giá vượt ngưỡng cũ 25.0 nhưng vẫn phải được chấp nhận (<= 45.0).
        assert_eq!(signal_20.signal_type, SignalType::Buy);
        assert!((signal_20.risk_points - 25.5).abs() < 1e-6);
        assert!(signal_20.risk_points > 25.0);

        // Lot size phải giảm để rủi ro tiền mặt không vượt quá 1% tài khoản $10,000.
        assert_eq!(signal_20.recommended_lot_size, 0.03);
        assert!(signal_20.recommended_lot_size < signal_10.recommended_lot_size);
        assert!(signal_20.risk_amount_usd <= 10_000.0 * 0.01 + 1e-6);
    }

    #[test]
    fn test_noise_candle_is_rejected() {
        let mut candles = zigzag_candles(60);
        let signal_ts = BASE_TS + 60 * STEP_MS;
        // Biên độ 0.8 < 1.2 -> nhiễu, không được phát tín hiệu.
        candles.push(candle(signal_ts, 2674.0, 2674.4, 2673.6, 2674.0, 2000.0));

        assert!(
            GoldMarketAnalyzer::analyze_market(&candles, &buy_config(10.0)).is_none(),
            "tiny range candle must be ignored"
        );
    }

    #[test]
    fn test_volume_filter_disabled_allows_zero_volume_signal() {
        let mut candles = zigzag_candles(60);
        let signal_ts = BASE_TS + 60 * STEP_MS;
        // Cùng mẫu Pinbar nhưng volume = 0 (nguồn không có volume).
        candles.push(candle(signal_ts, 2674.0, 2675.6, 2669.5, 2675.0, 0.0));

        let enabled = buy_config(10.0);
        assert!(
            GoldMarketAnalyzer::analyze_market(&candles, &enabled).is_none(),
            "volume filter ON must reject zero-volume candle"
        );

        let disabled = BotConfig {
            volume_filter_enabled: false,
            ..buy_config(10.0)
        };
        let signal = GoldMarketAnalyzer::analyze_market(&candles, &disabled)
            .expect("volume filter OFF must allow the setup");
        assert_eq!(signal.signal_type, SignalType::Buy);
        assert_eq!(signal.volume_surge_ratio, 0.0);
    }

    #[test]
    fn test_macro_bias_filters_counter_trend_buy() {
        let mut candles = zigzag_candles(60);
        let signal_ts = BASE_TS + 60 * STEP_MS;
        candles.push(bullish_pinbar(signal_ts));

        let config = BotConfig {
            macro_bias: "BEARISH".to_string(),
            ..buy_config(10.0)
        };

        assert!(GoldMarketAnalyzer::analyze_market(&candles, &config).is_none());
    }

    fn analyze_buy_with_live(live_price: f64, now_ms: i64, config: BotConfig) -> Option<Signal> {
        let mut candles = zigzag_candles(60);
        let signal_ts = BASE_TS + 60 * STEP_MS;
        candles.push(bullish_pinbar(signal_ts));

        GoldMarketAnalyzer::analyze_market_with_live_price(
            &candles,
            &config,
            Some(live_price),
            now_ms,
        )
    }

    fn fresh_now() -> i64 {
        // 1 giây sau khi nến Pinbar đóng cửa.
        BASE_TS + 60 * STEP_MS + STEP_MS + 1000
    }

    #[test]
    fn entry_market_uses_live_price_when_drift_within_limit() {
        let signal = analyze_buy_with_live(2676.0, fresh_now(), buy_config(10.0))
            .expect("expected BUY signal");

        assert_eq!(signal.entry_price, 2676.0);
        assert_eq!(signal.entry_type, crate::models::EntryType::Market);
        assert!(signal.is_entry_filled);
        assert_eq!(signal.candle_close_price, 2675.0);
        assert_eq!(signal.live_market_price, Some(2676.0));
        assert_eq!(signal.data_source, "BINANCE_PAXG");
    }

    #[test]
    fn entry_market_falls_back_to_limit_when_drift_exceeds() {
        let signal = analyze_buy_with_live(2690.0, fresh_now(), buy_config(10.0))
            .expect("expected BUY signal");

        assert_eq!(signal.entry_price, 2675.0);
        assert_eq!(signal.entry_type, crate::models::EntryType::Limit);
        assert!(!signal.is_entry_filled);
        assert_eq!(signal.retest_limit_price, Some(2672.25));
    }

    #[test]
    fn entry_market_cancels_when_live_already_through_sl() {
        let signal = analyze_buy_with_live(2650.0, fresh_now(), buy_config(10.0));
        assert!(signal.is_none());
    }

    #[test]
    fn entry_limit_retest_uses_midpoint() {
        let config = BotConfig {
            entry_mode: "LIMIT_RETEST".to_string(),
            ..buy_config(10.0)
        };
        let signal =
            analyze_buy_with_live(2676.0, fresh_now(), config).expect("expected BUY signal");

        assert_eq!(signal.entry_price, 2672.25);
        assert_eq!(signal.retest_limit_price, Some(2672.25));
        assert_eq!(signal.entry_type, crate::models::EntryType::Limit);
        assert!(!signal.is_entry_filled);
        assert!((signal.risk_points - 12.75).abs() < 1e-6);
    }

    #[test]
    fn entry_breakout_uses_candle_high_plus_half_point() {
        let config = BotConfig {
            entry_mode: "BREAKOUT".to_string(),
            ..buy_config(10.0)
        };
        let signal =
            analyze_buy_with_live(2676.0, fresh_now(), config).expect("expected BUY signal");

        assert_eq!(signal.entry_price, 2676.1);
        assert_eq!(signal.entry_type, crate::models::EntryType::Breakout);
        assert!(!signal.is_entry_filled);
    }

    #[test]
    fn stale_candle_is_rejected_when_live_price_provided() {
        let stale_now = BASE_TS + 60 * STEP_MS + STEP_MS + 120_000 + 1;
        let signal = analyze_buy_with_live(2675.0, stale_now, buy_config(10.0));
        assert!(signal.is_none());
    }

    #[test]
    fn stale_guard_is_not_applied_without_live_price() {
        // analyze_market() wrapper truyền live_price = None -> dữ liệu lịch sử vẫn phân tích được.
        let signal = expect_buy_signal(10.0);
        assert_eq!(signal.entry_type, crate::models::EntryType::Market);
        assert_eq!(signal.live_market_price, None);
    }

    fn zone(price: f64) -> SupportResistanceZone {
        SupportResistanceZone {
            level_type: ZoneType::Resistance,
            price,
            min_price: price - 0.5,
            max_price: price + 0.5,
            touch_count: 1,
            timeframe: Timeframe::M15,
            strength_score: 0.25,
        }
    }

    #[test]
    fn select_tp2_buy_skips_resistance_too_close_to_tp1() {
        let zones = vec![zone(2420.5), zone(2430.0), zone(2425.0)];

        let tp2 = select_tp2(true, 2400.0, 2420.0, 10.0, &zones, 2.0);

        // 2420.5 - 1.0 = 2419.5 is not beyond TP1 + 0.01 and must be skipped.
        // The nearest valid resistance is 2425.0 -> TP2 = 2424.0.
        assert!((tp2 - 2424.0).abs() < 1e-9);
        assert!(tp2 > 2420.0 + 0.01);
    }

    #[test]
    fn select_tp2_sell_skips_support_too_close_to_tp1() {
        let zones = vec![zone(2379.5), zone(2370.0), zone(2375.0)];

        let tp2 = select_tp2(false, 2400.0, 2380.0, 10.0, &zones, 2.0);

        // 2379.5 + 1.0 = 2380.5 is not beyond TP1 - 0.01 and must be skipped.
        // The nearest valid support is 2375.0 -> TP2 = 2376.0.
        assert!((tp2 - 2376.0).abs() < 1e-9);
        assert!(tp2 < 2380.0 - 0.01);
    }

    #[test]
    fn select_tp2_falls_back_to_three_r_without_zones() {
        assert!((select_tp2(true, 2400.0, 2420.0, 10.0, &[], 2.0) - 2430.0).abs() < 1e-9);
        assert!((select_tp2(false, 2400.0, 2380.0, 10.0, &[], 2.0) - 2370.0).abs() < 1e-9);
    }

    #[test]
    fn generated_buy_signal_keeps_tp2_beyond_tp1() {
        let signal = expect_buy_signal(10.0);

        assert!(signal.take_profit_2 > signal.take_profit_1 + 0.01);
        assert!(signal.reward_points_tp2 > signal.reward_points_tp1 + 0.01);
    }

    #[test]
    fn entry_market_cancels_when_live_through_sl_within_max_drift() {
        // SL = 2669.5 - 4.0 = 2665.5; live 2665.0 is through SL but only 10 points
        // away from the candle close, exactly at the configured drift limit.
        let config = BotConfig {
            max_entry_drift_points: 10.0,
            ..buy_config(4.0)
        };

        let signal = analyze_buy_with_live(2665.0, fresh_now(), config);
        assert!(signal.is_none());
    }

    #[test]
    fn small_account_wide_sl_is_rejected_by_risk_budget() {
        // $100 balance at 1% => $1 max risk. A 25.5-point SL forces MIN_LOT 0.01,
        // whose actual risk is $25.50 — far above the budget.
        let config = BotConfig {
            account_balance: 100.0,
            risk_percentage: 1.0,
            ..buy_config(20.0)
        };
        let mut candles = zigzag_candles(60);
        candles.push(bullish_pinbar(BASE_TS + 60 * STEP_MS));

        assert!(GoldMarketAnalyzer::analyze_market(&candles, &config).is_none());
    }
}
