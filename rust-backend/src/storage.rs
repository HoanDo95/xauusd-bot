use crate::models::{BotConfig, DailyMacroReport, Signal};
use chrono::{DateTime, Duration, Timelike, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// Toàn bộ trạng thái bền vững được ghi xuống `data/bot_state.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PersistentState {
    #[serde(default)]
    pub config: BotConfig,
    #[serde(default)]
    pub signals: Vec<Signal>,
    #[serde(default)]
    pub daily_report: Option<DailyMacroReport>,
    /// Dedup keys của các briefing trước phiên đã gửi (mới nhất ở cuối).
    #[serde(default)]
    pub sent_session_alerts: Vec<String>,
}

/// Bộ lưu trữ JSON nguyên tử (atomic write) cho server chạy 24/7.
#[derive(Debug, Clone)]
pub struct PersistentStore {
    path: PathBuf,
}

impl PersistentStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Nạp trạng thái cũ; file thiếu/hỏng sẽ trả về trạng thái mặc định (không panic).
    pub fn load(&self) -> PersistentState {
        if !self.path.exists() {
            info!(
                "ℹ️ Không tìm thấy file trạng thái {}, khởi tạo mặc định.",
                self.path.display()
            );
            return PersistentState::default();
        }

        match fs::read_to_string(&self.path) {
            Ok(content) => match serde_json::from_str::<PersistentState>(&content) {
                Ok(state) => {
                    info!(
                        "✅ Đã nạp {} tín hiệu từ {}",
                        state.signals.len(),
                        self.path.display()
                    );
                    state
                }
                Err(err) => {
                    warn!(
                        "⚠️ File trạng thái {} không hợp lệ ({}), dùng mặc định.",
                        self.path.display(),
                        err
                    );
                    PersistentState::default()
                }
            },
            Err(err) => {
                warn!(
                    "⚠️ Không đọc được file trạng thái {} ({}), dùng mặc định.",
                    self.path.display(),
                    err
                );
                PersistentState::default()
            }
        }
    }

    /// Ghi nguyên tử: serialize -> file tạm -> rename đè file chính.
    pub fn save(&self, state: &PersistentState) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("Không tạo được thư mục {}: {}", parent.display(), e))?;
            }
        }

        let data = serde_json::to_string_pretty(state)
            .map_err(|e| format!("Không serialize được trạng thái bot: {}", e))?;

        let tmp_path = self.path.with_extension("json.tmp");
        fs::write(&tmp_path, data)
            .map_err(|e| format!("Không ghi được file tạm {}: {}", tmp_path.display(), e))?;

        fs::rename(&tmp_path, &self.path).map_err(|e| {
            format!(
                "Không thể thay thế file trạng thái {}: {}",
                self.path.display(),
                e
            )
        })
    }

    /// Khóa ngày giao dịch theo múi giờ Việt Nam (UTC+7), định dạng YYYY-MM-DD.
    pub fn trading_day_key(dt: DateTime<Utc>) -> String {
        (dt + Duration::hours(7)).format("%Y-%m-%d").to_string()
    }

    /// Đếm số tín hiệu THẬT (bỏ qua simulation) đã phát trong ngày giao dịch hiện tại.
    pub fn count_live_signals_today(signals: &[Signal], now: DateTime<Utc>) -> usize {
        let today = Self::trading_day_key(now);
        signals
            .iter()
            .filter(|signal| {
                !signal.is_simulated && Self::trading_day_key(signal.created_at) == today
            })
            .count()
    }

    /// Đã chạm quota số tín hiệu thật/ngày hay chưa.
    pub fn has_reached_daily_quota(
        signals: &[Signal],
        max_per_day: usize,
        now: DateTime<Utc>,
    ) -> bool {
        Self::count_live_signals_today(signals, now) >= max_per_day
    }

    /// Chống bắn lặp trên cùng một cây nến thông qua dedup_key.
    /// Bỏ qua tín hiệu mô phỏng để lệnh test không chặn tín hiệu thật cùng nến.
    pub fn is_duplicate_candle(signals: &[Signal], dedup_key: &str) -> bool {
        !dedup_key.is_empty()
            && signals
                .iter()
                .any(|signal| !signal.is_simulated && signal.dedup_key == dedup_key)
    }

    /// Giữ tối đa 40 dedup key briefing gần nhất (mới nhất ở cuối, cắt từ đầu).
    pub fn prune_session_alerts(keys: &mut Vec<String>) {
        const MAX_SESSION_ALERTS: usize = 40;
        if keys.len() > MAX_SESSION_ALERTS {
            let excess = keys.len() - MAX_SESSION_ALERTS;
            keys.drain(0..excess);
        }
    }

    /// Đã gửi briefing cho phiên/ngày này chưa (chống trùng sau restart).
    pub fn has_sent_session_alert(keys: &[String], key: &str) -> bool {
        keys.iter().any(|sent| sent == key)
    }

    /// Bộ lọc khung giờ vàng 13:00-23:00 GMT+7 (tương đương 06:00-16:00 UTC).
    pub fn is_within_golden_session(now: DateTime<Utc>, session_filter_enabled: bool) -> bool {
        if !session_filter_enabled {
            return true;
        }
        let vietnam_hour = (now.hour() as i64 + 7).rem_euclid(24);
        (13..=23).contains(&vietnam_hour)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{EntryType, SignalStatus, SignalType, Timeframe};
    use chrono::TimeZone;

    fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0)
            .single()
            .expect("valid UTC datetime")
    }

    fn sample_signal(
        id: &str,
        created_at: DateTime<Utc>,
        is_simulated: bool,
        dedup_key: &str,
    ) -> Signal {
        Signal {
            id: id.to_string(),
            symbol: "XAU/USD".to_string(),
            signal_type: SignalType::Buy,
            timeframe: Timeframe::M15,
            entry_price: 2675.0,
            stop_loss: 2665.0,
            take_profit_1: 2695.0,
            take_profit_2: 2705.0,
            risk_points: 10.0,
            reward_points_tp1: 20.0,
            reward_points_tp2: 30.0,
            risk_reward_ratio: 2.0,
            volume_surge_ratio: 1.9,
            recommended_lot_size: 0.1,
            risk_amount_usd: 100.0,
            potential_profit_tp1_usd: 200.0,
            potential_profit_tp2_usd: 300.0,
            price_action_notes: "test".to_string(),
            volume_analysis_notes: "test".to_string(),
            full_rationale: "test".to_string(),
            status: SignalStatus::Active,
            created_at,
            telegram_message_id: None,
            dedup_key: dedup_key.to_string(),
            candle_timestamp: created_at.timestamp_millis(),
            is_simulated,
            confluence_score: 90,
            outcome_at: None,
            entry_type: EntryType::Market,
            candle_close_price: 2675.0,
            live_market_price: None,
            retest_limit_price: None,
            data_source: "BINANCE_PAXG".to_string(),
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
        }
    }

    #[test]
    fn test_daily_quota_one_signal_per_day() {
        let now = at(2026, 11, 9, 12, 0);
        let signals = vec![sample_signal(
            "SIG-1",
            at(2026, 11, 9, 8, 0),
            false,
            "XAU/USD_M15_1",
        )];

        assert_eq!(PersistentStore::count_live_signals_today(&signals, now), 1);
        assert!(PersistentStore::has_reached_daily_quota(&signals, 1, now));
        assert!(!PersistentStore::has_reached_daily_quota(&signals, 2, now));
    }

    #[test]
    fn test_simulation_signal_does_not_consume_quota() {
        let now = at(2026, 11, 9, 12, 0);
        let signals = vec![sample_signal(
            "SIM-1",
            at(2026, 11, 9, 8, 0),
            true,
            "XAU/USD_M15_2",
        )];

        assert_eq!(PersistentStore::count_live_signals_today(&signals, now), 0);
        assert!(!PersistentStore::has_reached_daily_quota(&signals, 1, now));
    }

    #[test]
    fn test_dedup_key_prevents_duplicate_alert() {
        let now = at(2026, 11, 9, 12, 0);
        let signals = vec![sample_signal(
            "SIG-1",
            now,
            false,
            "XAU/USD_M15_1700000000000",
        )];

        assert!(PersistentStore::is_duplicate_candle(
            &signals,
            "XAU/USD_M15_1700000000000"
        ));
        assert!(!PersistentStore::is_duplicate_candle(
            &signals,
            "XAU/USD_M15_1700000900000"
        ));
        assert!(!PersistentStore::is_duplicate_candle(&signals, ""));
    }

    #[test]
    fn test_simulated_signal_does_not_block_real_signal_on_same_candle() {
        let now = at(2026, 11, 9, 12, 0);
        let signals = vec![sample_signal(
            "SIM-1",
            now,
            true,
            "XAU/USD_M15_1700000000000",
        )];

        // Lệnh test cùng nến không được chặn lệnh thật.
        assert!(!PersistentStore::is_duplicate_candle(
            &signals,
            "XAU/USD_M15_1700000000000"
        ));
    }

    #[test]
    fn test_trading_day_uses_vietnam_timezone() {
        // 18:00 UTC ngày 09/11 = 01:00 sáng 10/11 giờ Việt Nam.
        assert_eq!(
            PersistentStore::trading_day_key(at(2026, 11, 9, 18, 0)),
            "2026-11-10"
        );
        // 16:00 UTC ngày 09/11 = 23:00 giờ Việt Nam cùng ngày.
        assert_eq!(
            PersistentStore::trading_day_key(at(2026, 11, 9, 16, 0)),
            "2026-11-09"
        );
    }

    #[test]
    fn test_daily_quota_respects_vietnam_day_boundary() {
        let now = at(2026, 11, 9, 18, 0); // 01:00 ngày 10/11 giờ VN
        let signals = vec![
            sample_signal("SIG-OLD", at(2026, 11, 9, 15, 0), false, "key-old"),
            sample_signal("SIG-NEW", at(2026, 11, 9, 17, 30), false, "key-new"),
        ];

        // Chỉ tín hiệu lúc 17:30 UTC (00:30 VN ngày 10/11) thuộc ngày giao dịch hiện tại.
        assert_eq!(PersistentStore::count_live_signals_today(&signals, now), 1);
    }

    #[test]
    fn test_golden_session_filter() {
        // 06:30 UTC = 13:30 giờ Việt Nam -> trong phiên vàng.
        assert!(PersistentStore::is_within_golden_session(
            at(2026, 11, 9, 6, 30),
            true
        ));
        // 03:00 UTC = 10:00 giờ Việt Nam -> ngoài phiên.
        assert!(!PersistentStore::is_within_golden_session(
            at(2026, 11, 9, 3, 0),
            true
        ));
        // Tắt filter -> luôn cho phép.
        assert!(PersistentStore::is_within_golden_session(
            at(2026, 11, 9, 3, 0),
            false
        ));
    }

    #[test]
    fn test_has_sent_session_alert() {
        let keys = vec![
            "SESSION_LONDON_2026-11-09".to_string(),
            "SESSION_NEW_YORK_2026-11-09".to_string(),
        ];

        assert!(PersistentStore::has_sent_session_alert(
            &keys,
            "SESSION_LONDON_2026-11-09"
        ));
        assert!(!PersistentStore::has_sent_session_alert(
            &keys,
            "SESSION_LONDON_2026-11-10"
        ));
        assert!(!PersistentStore::has_sent_session_alert(&[], "any-key"));
    }

    #[test]
    fn test_prune_session_alerts_keeps_newest_40() {
        let mut keys: Vec<String> = (0..45).map(|index| format!("key-{index:02}")).collect();

        PersistentStore::prune_session_alerts(&mut keys);

        assert_eq!(keys.len(), 40);
        assert_eq!(keys.first().map(String::as_str), Some("key-05"));
        assert_eq!(keys.last().map(String::as_str), Some("key-44"));

        let mut short: Vec<String> = vec!["only".to_string()];
        PersistentStore::prune_session_alerts(&mut short);
        assert_eq!(short, vec!["only".to_string()]);
    }

    #[test]
    fn test_persistent_store_round_trip() {
        let unique = format!(
            "gold-signal-bot-test-{}-{}.json",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        );
        let path = std::env::temp_dir().join(unique);
        let store = PersistentStore::new(&path);

        let mut state = PersistentState::default();
        state.config.max_signals_per_day = 1;
        state.signals.push(sample_signal(
            "SIG-1",
            at(2026, 11, 9, 8, 0),
            false,
            "XAU/USD_M15_1",
        ));
        state
            .sent_session_alerts
            .push("SESSION_LONDON_2026-11-09".to_string());

        store.save(&state).expect("save state");
        let loaded = store.load();

        assert_eq!(loaded.signals.len(), 1);
        assert_eq!(loaded.signals[0].dedup_key, "XAU/USD_M15_1");
        assert_eq!(loaded.config.max_signals_per_day, 1);
        assert_eq!(loaded.sent_session_alerts, state.sent_session_alerts);

        let _ = fs::remove_file(&path);
    }
}
