use crate::analyzer::GoldMarketAnalyzer;
use crate::api::{persist_state, AppState};
use crate::models::{EconomicEvent, MacroBias, SupportResistanceZone};
use crate::news_filter::ImpactLevel;
use crate::storage::PersistentStore;
use chrono::{DateTime, Datelike, Duration, LocalResult, TimeZone, Utc, Weekday};
use chrono_tz::Tz;
use tracing::{info, warn};

/// Phiên giao dịch chính có briefing trước giờ mở cửa.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradingSession {
    London,
    NewYork,
}

impl TradingSession {
    pub fn code(&self) -> &'static str {
        match self {
            TradingSession::London => "LONDON",
            TradingSession::NewYork => "NEW_YORK",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            TradingSession::London => "PHIÊN ÂU (LONDON)",
            TradingSession::NewYork => "PHIÊN MỸ (NEW YORK)",
        }
    }

    pub fn timezone(&self) -> Tz {
        match self {
            TradingSession::London => chrono_tz::Europe::London,
            TradingSession::NewYork => chrono_tz::America::New_York,
        }
    }
}

/// Một briefing đã đến giờ gửi (chưa xét gửi thành công hay chưa).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionDue {
    pub session: TradingSession,
    pub dedup_key: String,
    pub alert_utc: DateTime<Utc>,
    pub open_utc: DateTime<Utc>,
    pub open_local_label: String,
    pub open_vn_label: String,
}

/// Nội dung briefing trước phiên gửi tới Telegram.
#[derive(Debug, Clone)]
pub struct SessionBriefing {
    pub session: TradingSession,
    pub dedup_key: String,
    pub open_local_label: String,
    pub open_vn_label: String,
    pub lead_minutes: i64,
    pub current_price: f64,
    pub data_source: String,
    pub ema50: f64,
    pub ema200: f64,
    pub trend_bias: MacroBias,
    pub nearest_support: Option<SupportResistanceZone>,
    pub nearest_resistance: Option<SupportResistanceZone>,
    pub upcoming_events: Vec<EconomicEvent>,
    pub signals_today: usize,
    pub max_signals_per_day: usize,
}

/// Trả về các briefing trước phiên đang nằm trong cửa sổ 15 phút và chưa gửi.
///
/// Giờ mở cửa cố định 08:00 theo múi giờ địa phương của từng phiên, nên tự động
/// đúng theo DST (London GMT/BST, New York EST/EDT).
pub fn check_due_session_alerts(
    now_utc: DateTime<Utc>,
    lead_minutes: i64,
    sent_keys: &[String],
) -> Vec<SessionDue> {
    let lead = lead_minutes.max(0);
    let mut due = Vec::new();

    for session in [TradingSession::London, TradingSession::NewYork] {
        let tz = session.timezone();
        let local_now = now_utc.with_timezone(&tz);

        if matches!(local_now.weekday(), Weekday::Sat | Weekday::Sun) {
            continue;
        }

        let local_date = local_now.date_naive();
        let Some(naive_open) = local_date.and_hms_opt(8, 0, 0) else {
            continue;
        };

        let open_local = match tz.from_local_datetime(&naive_open) {
            LocalResult::Single(datetime) => datetime,
            // Thời điểm 08:00 không nằm trong khoảng DST gap thực tế; nếu có,
            // chọn mốc sớm nhất để hành vi vẫn deterministic.
            LocalResult::Ambiguous(earliest, _latest) => earliest,
            LocalResult::None => continue,
        };

        let open_utc = open_local.with_timezone(&Utc);
        let alert_utc = open_utc - Duration::minutes(lead);

        let window_end = alert_utc + Duration::minutes(15);
        if now_utc < alert_utc || now_utc >= window_end {
            continue;
        }

        let dedup_key = format!(
            "SESSION_{}_{}",
            session.code(),
            local_date.format("%Y-%m-%d")
        );
        if PersistentStore::has_sent_session_alert(sent_keys, &dedup_key) {
            continue;
        }

        let open_local_label = format!(
            "{} {} ({})",
            open_local.format("%H:%M"),
            tz.name(),
            open_local.format("%Z")
        );
        let open_vn_label = format!(
            "{} VN (UTC+7)",
            (open_utc + Duration::hours(7)).format("%H:%M")
        );

        due.push(SessionDue {
            session,
            dedup_key,
            alert_utc,
            open_utc,
            open_local_label,
            open_vn_label,
        });
    }

    due
}

/// Quét briefing đến giờ, gửi Telegram và lưu dedup key bền vững.
/// Trả về số briefing đã gửi thành công.
pub async fn evaluate_and_dispatch(state: &AppState) -> usize {
    let config = state.config.read().await.clone();
    if !config.session_alerts_enabled {
        return 0;
    }

    let now = Utc::now();
    let sent_keys = state.sent_session_alerts.read().await.clone();
    let due_alerts = check_due_session_alerts(now, config.session_alert_lead_minutes, &sent_keys);
    if due_alerts.is_empty() {
        return 0;
    }

    // Chụp snapshot một lần, không giữ lock khi gọi Telegram.
    let candles = state.candles.read().await.clone();
    let live_price = *state.live_price.read().await;
    let data_source = state.active_data_source.read().await.clone();
    let signals = state.signals.read().await.clone();
    let calendar_events = state.news_filter.read().await.calendar_events.clone();
    let telegram = state.telegram.read().await.clone();

    let current_price = live_price.unwrap_or_else(|| {
        candles
            .last()
            .map(|candle| candle.close)
            .unwrap_or_default()
    });

    // Không gửi briefing từ nến fallback/offline hoặc khi chưa có dữ liệu giá.
    if candles.is_empty() || data_source == "FALLBACK_SEED" || current_price <= 0.0 {
        warn!(
            "⚠️ [Session Alert] Bỏ qua briefing {} vì chưa có dữ liệu giá thật (source={}).",
            due_alerts
                .iter()
                .map(|due| due.dedup_key.as_str())
                .collect::<Vec<_>>()
                .join(","),
            data_source
        );
        return 0;
    }

    let (supports, resistances) =
        GoldMarketAnalyzer::detect_dynamic_sr_zones(&candles, config.lookback_period.max(5), 2.5);
    let ema50 = GoldMarketAnalyzer::calculate_ema(&candles, 50);
    let ema200 = GoldMarketAnalyzer::calculate_ema(&candles, 200);
    let trend_bias = GoldMarketAnalyzer::detect_ema_trend_bias(&candles);
    let signals_today = PersistentStore::count_live_signals_today(&signals, now);
    let upcoming_events = collect_upcoming_high_impact_events(&calendar_events, now);

    let mut sent_count = 0;

    for due in due_alerts {
        let briefing = SessionBriefing {
            session: due.session,
            dedup_key: due.dedup_key.clone(),
            open_local_label: due.open_local_label.clone(),
            open_vn_label: due.open_vn_label.clone(),
            lead_minutes: config.session_alert_lead_minutes,
            current_price,
            data_source: data_source.clone(),
            ema50,
            ema200,
            trend_bias,
            nearest_support: nearest_support(&supports, current_price),
            nearest_resistance: nearest_resistance(&resistances, current_price),
            upcoming_events: upcoming_events.clone(),
            signals_today,
            max_signals_per_day: config.max_signals_per_day,
        };

        match telegram.send_session_alert(&briefing).await {
            Ok(message_id) => {
                {
                    let mut keys = state.sent_session_alerts.write().await;
                    if !PersistentStore::has_sent_session_alert(&keys, &due.dedup_key) {
                        keys.push(due.dedup_key.clone());
                        PersistentStore::prune_session_alerts(&mut keys);
                    }
                }

                if let Err(err) = persist_state(state).await {
                    warn!(
                        "⚠️ [Session Alert] Đã gửi {} nhưng không lưu được trạng thái: {}",
                        due.dedup_key, err
                    );
                }

                info!(
                    "🌍 [Session Alert] Đã gửi briefing {} ({}) | Telegram message_id={}",
                    due.session.display_name(),
                    due.dedup_key,
                    message_id
                );
                sent_count += 1;
            }
            Err(err) => {
                // Không đánh dấu đã gửi để worker thử lại trong cửa sổ 15 phút.
                warn!(
                    "⚠️ [Session Alert] Gửi briefing {} thất bại ({}), sẽ thử lại trong cửa sổ 15 phút.",
                    due.dedup_key, err
                );
            }
        }
    }

    sent_count
}

/// High-Impact USD events trong 12 giờ tới, sắp xếp sớm → muộn, tối đa 6 tin.
fn collect_upcoming_high_impact_events(
    events: &[EconomicEvent],
    now: DateTime<Utc>,
) -> Vec<EconomicEvent> {
    let horizon = now + Duration::hours(12);
    let mut upcoming: Vec<EconomicEvent> = events
        .iter()
        .filter(|event| {
            event.impact == ImpactLevel::High
                && event.currency.eq_ignore_ascii_case("USD")
                && event.scheduled_time >= now
                && event.scheduled_time <= horizon
        })
        .cloned()
        .collect();

    upcoming.sort_by_key(|event| event.scheduled_time);
    upcoming.truncate(6);
    upcoming
}

fn nearest_support(
    zones: &[SupportResistanceZone],
    current_price: f64,
) -> Option<SupportResistanceZone> {
    zones
        .iter()
        .filter(|zone| zone.price <= current_price)
        .max_by(|left, right| left.price.total_cmp(&right.price))
        .or_else(|| {
            zones.iter().min_by(|left, right| {
                (left.price - current_price)
                    .abs()
                    .total_cmp(&(right.price - current_price).abs())
            })
        })
        .cloned()
}

fn nearest_resistance(
    zones: &[SupportResistanceZone],
    current_price: f64,
) -> Option<SupportResistanceZone> {
    zones
        .iter()
        .filter(|zone| zone.price >= current_price)
        .min_by(|left, right| left.price.total_cmp(&right.price))
        .or_else(|| {
            zones.iter().min_by(|left, right| {
                (left.price - current_price)
                    .abs()
                    .total_cmp(&(right.price - current_price).abs())
            })
        })
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn utc(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .expect("valid UTC datetime")
    }

    #[test]
    fn winter_alerts_use_standard_time() {
        let london = check_due_session_alerts(utc(2026, 11, 9, 7, 0), 60, &[]);
        assert_eq!(london.len(), 1);
        assert_eq!(london[0].session, TradingSession::London);
        assert_eq!(london[0].dedup_key, "SESSION_LONDON_2026-11-09");
        assert_eq!(london[0].open_utc, utc(2026, 11, 9, 8, 0));
        assert_eq!(london[0].alert_utc, utc(2026, 11, 9, 7, 0));
        assert_eq!(london[0].open_local_label, "08:00 Europe/London (GMT)");
        assert_eq!(london[0].open_vn_label, "15:00 VN (UTC+7)");

        let new_york = check_due_session_alerts(utc(2026, 11, 9, 12, 0), 60, &[]);
        assert_eq!(new_york.len(), 1);
        assert_eq!(new_york[0].session, TradingSession::NewYork);
        assert_eq!(new_york[0].dedup_key, "SESSION_NEW_YORK_2026-11-09");
        assert_eq!(new_york[0].open_utc, utc(2026, 11, 9, 13, 0));
        assert_eq!(new_york[0].alert_utc, utc(2026, 11, 9, 12, 0));
        assert_eq!(new_york[0].open_local_label, "08:00 America/New_York (EST)");
        assert_eq!(new_york[0].open_vn_label, "20:00 VN (UTC+7)");
    }

    #[test]
    fn summer_alerts_use_daylight_saving_time() {
        let london = check_due_session_alerts(utc(2026, 7, 15, 6, 0), 60, &[]);
        assert_eq!(london.len(), 1);
        assert_eq!(london[0].session, TradingSession::London);
        assert_eq!(london[0].open_utc, utc(2026, 7, 15, 7, 0));
        assert_eq!(london[0].alert_utc, utc(2026, 7, 15, 6, 0));
        assert!(london[0].open_local_label.ends_with("(BST)"));

        let new_york = check_due_session_alerts(utc(2026, 7, 15, 11, 0), 60, &[]);
        assert_eq!(new_york.len(), 1);
        assert_eq!(new_york[0].session, TradingSession::NewYork);
        assert_eq!(new_york[0].open_utc, utc(2026, 7, 15, 12, 0));
        assert_eq!(new_york[0].alert_utc, utc(2026, 7, 15, 11, 0));
        assert!(new_york[0].open_local_label.ends_with("(EDT)"));
    }

    #[test]
    fn weekends_have_no_alerts() {
        // Thứ Bảy và Chủ Nhật, thử đúng giờ alert của cả hai phiên.
        for day in [14, 15] {
            assert!(check_due_session_alerts(utc(2026, 11, day, 7, 0), 60, &[]).is_empty());
            assert!(check_due_session_alerts(utc(2026, 11, day, 12, 0), 60, &[]).is_empty());
        }
    }

    #[test]
    fn already_sent_key_is_skipped() {
        let sent = vec!["SESSION_LONDON_2026-11-09".to_string()];
        assert!(check_due_session_alerts(utc(2026, 11, 9, 7, 0), 60, &sent).is_empty());
    }

    #[test]
    fn outside_fifteen_minute_window_is_skipped() {
        // Trước alert 1 phút và đúng alert + 15 phút đều không còn due.
        assert!(check_due_session_alerts(utc(2026, 11, 9, 6, 59), 60, &[]).is_empty());
        assert!(check_due_session_alerts(utc(2026, 11, 9, 7, 15), 60, &[]).is_empty());
        assert!(check_due_session_alerts(utc(2026, 11, 9, 12, 15), 60, &[]).is_empty());
        // Trong cửa sổ 15 phút vẫn due.
        assert_eq!(
            check_due_session_alerts(utc(2026, 11, 9, 7, 14), 60, &[]).len(),
            1
        );
    }

    #[test]
    fn custom_lead_minutes_shift_alert_time() {
        let alerts = check_due_session_alerts(utc(2026, 11, 9, 6, 30), 90, &[]);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].session, TradingSession::London);
        assert_eq!(alerts[0].alert_utc, utc(2026, 11, 9, 6, 30));
    }
}
