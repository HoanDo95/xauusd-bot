use crate::models::{Candle, Signal, SignalStatus, SignalType};
use chrono::Utc;
use serde::{Deserialize, Serialize};

/// Hiệu suất tích lũy của các tín hiệu THẬT (bỏ qua lệnh simulation).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SignalStats {
    pub total: usize,
    pub live: usize,
    pub simulated: usize,
    pub active: usize,
    pub tp1_hits: usize,
    pub tp2_hits: usize,
    pub sl_hits: usize,
    pub cancelled: usize,
    pub closed: usize,
    /// Tỷ lệ thắng trên các lệnh đã đóng (TP1/TP2 tính là thắng).
    pub win_rate: f64,
    /// R trung bình trên mỗi lệnh đã đóng.
    pub avg_r: f64,
    /// Tổng R tích lũy.
    pub net_r: f64,
    /// Gross win R / Gross loss R. 999.0 nghĩa là chưa có lệnh thua.
    pub profit_factor: f64,
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// Theo dõi các tín hiệu đang mở và cập nhật TP1/TP2/SL từ nến ĐÃ ĐÓNG CỬA.
///
/// Quy tắc an toàn:
/// - Chỉ xét nến có timestamp SAU cây nến sinh tín hiệu (không look-ahead).
/// - Nếu cùng một cây nến vừa quét SL vừa chạm TP1 → tính SL (kịch bản xấu nhất).
/// - Sau khi đã chạm TP1, nếu sau đó chạm SL thì giữ TP1Hit (đã chốt lời),
///   nhưng nếu chạm TP2 trước thì nâng lên TP2Hit.
pub fn evaluate_active_signals(signals: &mut [Signal], candles: &[Candle]) -> Vec<Signal> {
    if candles.is_empty() {
        return Vec::new();
    }

    let mut updates = Vec::new();

    for signal in signals.iter_mut() {
        if !matches!(
            signal.status,
            SignalStatus::Active | SignalStatus::Target1Hit
        ) {
            continue;
        }

        let cutoff = if signal.candle_timestamp > 0 {
            signal.candle_timestamp
        } else {
            signal.created_at.timestamp_millis()
        };

        let mut status = signal.status;
        let mut changed = false;

        for candle in candles.iter().filter(|candle| candle.timestamp > cutoff) {
            match signal.signal_type {
                SignalType::Buy => {
                    let hit_sl = candle.low <= signal.stop_loss;
                    let hit_tp1 = candle.high >= signal.take_profit_1;
                    let hit_tp2 = candle.high >= signal.take_profit_2;

                    if hit_sl && hit_tp1 {
                        // Cùng một nến vừa quét SL vừa chạm TP1: thận trọng tính SL.
                        if status != SignalStatus::Target1Hit {
                            status = SignalStatus::StopLossHit;
                            changed = true;
                        }
                        break;
                    }
                    if hit_sl {
                        if status != SignalStatus::Target1Hit {
                            status = SignalStatus::StopLossHit;
                            changed = true;
                        }
                        break;
                    }
                    if hit_tp2 {
                        if status != SignalStatus::Target2Hit {
                            status = SignalStatus::Target2Hit;
                            changed = true;
                        }
                        break;
                    }
                    if hit_tp1 && status == SignalStatus::Active {
                        status = SignalStatus::Target1Hit;
                        changed = true;
                    }
                }
                SignalType::Sell => {
                    let hit_sl = candle.high >= signal.stop_loss;
                    let hit_tp1 = candle.low <= signal.take_profit_1;
                    let hit_tp2 = candle.low <= signal.take_profit_2;

                    if hit_sl && hit_tp1 {
                        if status != SignalStatus::Target1Hit {
                            status = SignalStatus::StopLossHit;
                            changed = true;
                        }
                        break;
                    }
                    if hit_sl {
                        if status != SignalStatus::Target1Hit {
                            status = SignalStatus::StopLossHit;
                            changed = true;
                        }
                        break;
                    }
                    if hit_tp2 {
                        if status != SignalStatus::Target2Hit {
                            status = SignalStatus::Target2Hit;
                            changed = true;
                        }
                        break;
                    }
                    if hit_tp1 && status == SignalStatus::Active {
                        status = SignalStatus::Target1Hit;
                        changed = true;
                    }
                }
            }
        }

        if changed {
            signal.status = status;
            signal.outcome_at = Some(Utc::now());
            updates.push(signal.clone());
        }
    }

    updates
}

/// Tính hiệu suất từ lịch sử tín hiệu. Chỉ tính lệnh THẬT (`!is_simulated`).
pub fn compute_stats(signals: &[Signal]) -> SignalStats {
    let mut stats = SignalStats {
        total: signals.len(),
        ..SignalStats::default()
    };

    for signal in signals {
        if signal.is_simulated {
            stats.simulated += 1;
            continue;
        }

        stats.live += 1;
        match signal.status {
            SignalStatus::Active => stats.active += 1,
            SignalStatus::Target1Hit => {
                stats.tp1_hits += 1;
                stats.closed += 1;
            }
            SignalStatus::Target2Hit => {
                stats.tp2_hits += 1;
                stats.closed += 1;
            }
            SignalStatus::StopLossHit => {
                stats.sl_hits += 1;
                stats.closed += 1;
            }
            SignalStatus::Cancelled => {
                stats.cancelled += 1;
                stats.closed += 1;
            }
        }
    }

    let wins = stats.tp1_hits + stats.tp2_hits;
    stats.win_rate = if stats.closed > 0 {
        round2((wins as f64 / stats.closed as f64) * 100.0)
    } else {
        0.0
    };

    let mut gross_win = 0.0;
    let mut gross_loss = 0.0;
    let mut net = 0.0;
    let mut closed = 0usize;

    for signal in signals.iter().filter(|signal| !signal.is_simulated) {
        let risk = (signal.entry_price - signal.stop_loss).abs();
        if risk <= f64::EPSILON {
            continue;
        }

        let r = match signal.status {
            SignalStatus::Target1Hit => (signal.take_profit_1 - signal.entry_price).abs() / risk,
            SignalStatus::Target2Hit => (signal.take_profit_2 - signal.entry_price).abs() / risk,
            SignalStatus::StopLossHit => -1.0,
            SignalStatus::Cancelled => 0.0,
            SignalStatus::Active => continue,
        };

        net += r;
        closed += 1;
        if r >= 0.0 {
            gross_win += r;
        } else {
            gross_loss += -r;
        }
    }

    stats.net_r = round2(net);
    stats.avg_r = if closed > 0 {
        round2(net / closed as f64)
    } else {
        0.0
    };
    stats.profit_factor = if gross_loss > 0.0 {
        round2(gross_win / gross_loss)
    } else if gross_win > 0.0 {
        999.0
    } else {
        0.0
    };

    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Timeframe;
    use chrono::TimeZone;

    fn signal(
        signal_type: SignalType,
        entry: f64,
        stop_loss: f64,
        tp1: f64,
        tp2: f64,
        candle_timestamp: i64,
    ) -> Signal {
        Signal {
            id: "SIG-TEST".to_string(),
            symbol: "XAU/USD".to_string(),
            signal_type,
            timeframe: Timeframe::M15,
            entry_price: entry,
            stop_loss,
            take_profit_1: tp1,
            take_profit_2: tp2,
            risk_points: (entry - stop_loss).abs(),
            reward_points_tp1: (tp1 - entry).abs(),
            reward_points_tp2: (tp2 - entry).abs(),
            risk_reward_ratio: 2.0,
            volume_surge_ratio: 1.8,
            recommended_lot_size: 0.07,
            risk_amount_usd: 100.0,
            potential_profit_tp1_usd: 200.0,
            potential_profit_tp2_usd: 300.0,
            price_action_notes: "test".to_string(),
            volume_analysis_notes: "test".to_string(),
            full_rationale: "test".to_string(),
            status: SignalStatus::Active,
            created_at: Utc
                .timestamp_millis_opt(candle_timestamp)
                .single()
                .unwrap_or_else(Utc::now),
            telegram_message_id: None,
            dedup_key: "XAU/USD_M15_1".to_string(),
            candle_timestamp,
            is_simulated: false,
            confluence_score: 90,
            outcome_at: None,
        }
    }

    fn candle(timestamp: i64, open: f64, high: f64, low: f64, close: f64) -> Candle {
        Candle {
            timestamp,
            open,
            high,
            low,
            close,
            volume: 1000.0,
            timeframe: Timeframe::M15,
        }
    }

    #[test]
    fn buy_signal_hits_tp1_then_tp2() {
        let mut signals = vec![signal(SignalType::Buy, 100.0, 95.0, 110.0, 115.0, 1)];
        let candles = vec![
            candle(2, 100.0, 111.0, 99.0, 110.5),
            candle(3, 110.5, 116.0, 110.0, 115.5),
        ];

        let updates = evaluate_active_signals(&mut signals, &candles);

        assert_eq!(updates.len(), 1);
        assert_eq!(signals[0].status, SignalStatus::Target2Hit);
        assert!(signals[0].outcome_at.is_some());
    }

    #[test]
    fn buy_signal_hits_sl() {
        let mut signals = vec![signal(SignalType::Buy, 100.0, 95.0, 110.0, 115.0, 1)];
        let candles = vec![candle(2, 100.0, 101.0, 94.0, 95.5)];

        evaluate_active_signals(&mut signals, &candles);

        assert_eq!(signals[0].status, SignalStatus::StopLossHit);
    }

    #[test]
    fn same_candle_hitting_sl_and_tp1_is_conservative_sl() {
        let mut signals = vec![signal(SignalType::Buy, 100.0, 95.0, 110.0, 115.0, 1)];
        let candles = vec![candle(2, 100.0, 111.0, 94.0, 100.0)];

        evaluate_active_signals(&mut signals, &candles);

        assert_eq!(signals[0].status, SignalStatus::StopLossHit);
    }

    #[test]
    fn sell_signal_hits_tp1() {
        let mut signals = vec![signal(SignalType::Sell, 100.0, 105.0, 90.0, 85.0, 1)];
        let candles = vec![candle(2, 100.0, 101.0, 89.0, 90.5)];

        evaluate_active_signals(&mut signals, &candles);

        assert_eq!(signals[0].status, SignalStatus::Target1Hit);
    }

    #[test]
    fn does_not_use_candles_before_or_at_signal_candle() {
        let mut signals = vec![signal(SignalType::Buy, 100.0, 95.0, 110.0, 115.0, 10)];
        let candles = vec![candle(10, 100.0, 111.0, 94.0, 100.0)];

        let updates = evaluate_active_signals(&mut signals, &candles);

        assert_eq!(updates.len(), 0);
        assert_eq!(signals[0].status, SignalStatus::Active);
    }

    #[test]
    fn stats_ignore_simulated_and_compute_r() {
        let mut tp1 = signal(SignalType::Buy, 100.0, 95.0, 110.0, 115.0, 1);
        tp1.status = SignalStatus::Target1Hit;

        let mut sl = signal(SignalType::Buy, 100.0, 95.0, 110.0, 115.0, 2);
        sl.status = SignalStatus::StopLossHit;

        let mut sim = signal(SignalType::Buy, 100.0, 95.0, 110.0, 115.0, 3);
        sim.status = SignalStatus::Target2Hit;
        sim.is_simulated = true;

        let stats = compute_stats(&[tp1, sl, sim]);

        assert_eq!(stats.total, 3);
        assert_eq!(stats.live, 2);
        assert_eq!(stats.simulated, 1);
        assert_eq!(stats.closed, 2);
        assert_eq!(stats.win_rate, 50.0);
        assert_eq!(stats.avg_r, 0.5);
        assert_eq!(stats.net_r, 1.0);
        assert_eq!(stats.profit_factor, 2.0);
    }
}
