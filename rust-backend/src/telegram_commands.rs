//! Telegram command worker: `/start`, `/stop`, `/view`, `/status`, `/start-entry`, `/help`.
//!
//! Commands are authorized to one admin chat (`TELEGRAM_ADMIN_CHAT_ID`, with
//! `config.telegram_chat_id` as fallback). They only toggle `is_active` or send
//! read-only scenario/status/entry-check views — never quota, signals or trade
//! state.

use crate::api::{
    current_live_view, current_scenarios, run_entry_check, start_scanner_inner, stop_scanner_inner,
    AppState,
};
use crate::storage::PersistentStore;
use crate::telegram::TelegramNotifier;
use chrono::Utc;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tracing::{info, warn};

const POLL_TIMEOUT_SECS: u64 = 25;
const IDLE_SLEEP_SECS: u64 = 1;
const ERROR_SLEEP_SECS: u64 = 5;
const COMMAND_RATE_LIMIT: Duration = Duration::from_secs(2);
/// AI Entry Check is expensive (one provider call), so it gets a longer
/// per-chat cooldown than the cheap scanner/status commands.
const ENTRY_CHECK_RATE_LIMIT: Duration = Duration::from_secs(30);
const HELP_TEXT: &str = "🤖 <b>XAUUSD Bot commands</b>\n\
                         /start — Bật scanner + gửi scenario view\n\
                         /stop — Tạm dừng scanner\n\
                         /view — Gửi lại scenario view\n\
                         /start-entry — AI entry check thủ công (không phải trade signal)\n\
                         /status — Trạng thái bot\n\
                         /help — Danh sách lệnh";

/// Spawn the long-poll worker. It starts by draining old pending updates so a
/// restart does not replay commands issued while the API was down.
pub fn spawn_telegram_command_worker(state: AppState) {
    tokio::spawn(async move {
        info!("🤖 Telegram command worker đã khởi chạy.");
        let env_admin = env_admin_chat_id();
        let mut offset = {
            let telegram = state.telegram.read().await.clone();
            match telegram.get_updates(-1, 0).await {
                Ok(updates) => updates
                    .last()
                    .and_then(update_id)
                    .map(|id| id.saturating_add(1))
                    .unwrap_or(0),
                Err(err) => {
                    warn!(
                        "⚠️ Telegram command worker: bỏ qua update cũ thất bại: {}",
                        err
                    );
                    0
                }
            }
        };
        let mut last_command_at: HashMap<i64, Instant> = HashMap::new();
        let mut last_entry_check_at: HashMap<i64, Instant> = HashMap::new();

        loop {
            let telegram = state.telegram.read().await.clone();
            match telegram.get_updates(offset, POLL_TIMEOUT_SECS).await {
                Ok(updates) => {
                    for update in updates {
                        if let Some(id) = update_id(&update) {
                            offset = offset.max(id.saturating_add(1));
                        }
                        handle_update(
                            &state,
                            env_admin,
                            &mut last_command_at,
                            &mut last_entry_check_at,
                            &update,
                        )
                        .await;
                    }
                    tokio::time::sleep(Duration::from_secs(IDLE_SLEEP_SECS)).await;
                }
                Err(err) => {
                    warn!("⚠️ Telegram getUpdates lỗi: {}", err);
                    tokio::time::sleep(Duration::from_secs(ERROR_SLEEP_SECS)).await;
                }
            }
        }
    });
}

async fn handle_update(
    state: &AppState,
    env_admin: Option<i64>,
    last_command_at: &mut HashMap<i64, Instant>,
    last_entry_check_at: &mut HashMap<i64, Instant>,
    update: &serde_json::Value,
) {
    let Some(message) = update.get("message") else {
        return;
    };
    let Some(chat_id) = message
        .get("chat")
        .and_then(|chat| chat.get("id"))
        .and_then(|value| value.as_i64())
    else {
        return;
    };
    let Some(text) = message.get("text").and_then(|value| value.as_str()) else {
        return;
    };
    let Some(command) = parse_command(text) else {
        return;
    };

    let config = state.config.read().await.clone();
    let admin_chat = resolve_admin_chat(env_admin, &config.telegram_chat_id);

    let Some(admin_chat) = admin_chat else {
        info!(
            "⚠️ Telegram command từ chat {} nhưng chưa cấu hình admin.",
            chat_id
        );
        let notifier = notifier_for_chat(&config.telegram_bot_token, chat_id);
        let reply = unauthorized_message(chat_id);
        if let Err(err) = notifier.send_html_message(&reply).await {
            warn!("⚠️ Không gửi được hướng dẫn admin chat: {}", err);
        }
        return;
    };

    if chat_id != admin_chat {
        info!("🚫 Telegram command từ chat không được phép: {}", chat_id);
        return;
    }

    if is_rate_limited(last_command_at, chat_id) {
        return;
    }

    let notifier = notifier_for_chat(&config.telegram_bot_token, admin_chat);
    match command.as_str() {
        "/start" => {
            let result = start_scanner_inner(state).await;
            let config_chat = config.telegram_chat_id.trim().parse::<i64>().ok();
            if !result.telegram_sent || config_chat != Some(admin_chat) {
                if let Err(err) = notifier
                    .send_scenario_summary(&result.live_view, &result.scenarios)
                    .await
                {
                    warn!("⚠️ Telegram /start không gửi được scenario view: {}", err);
                }
            }
            let reply = if result.was_active {
                "Scanner đã bật sẵn — đã gửi lại scenario view."
            } else {
                "▶️ Scanner đã bật. Scenario view đã gửi."
            };
            if let Err(err) = notifier.send_html_message(reply).await {
                warn!("⚠️ Telegram /start reply lỗi: {}", err);
            }
        }
        "/stop" => {
            let reply = match stop_scanner_inner(state).await {
                Ok(()) => "⏸ Scanner đã tạm dừng.".to_string(),
                Err(err) => {
                    warn!("⚠️ Telegram /stop không lưu được trạng thái: {}", err);
                    "⚠️ Scanner đã dừng trong RAM nhưng ghi trạng thái xuống đĩa thất bại."
                        .to_string()
                }
            };
            if let Err(err) = notifier.send_html_message(&reply).await {
                warn!("⚠️ Telegram /stop reply lỗi: {}", err);
            }
        }
        "/view" => {
            let live_view = current_live_view(state).await;
            let scenarios = current_scenarios(state).await;
            if let Err(err) = notifier.send_scenario_summary(&live_view, &scenarios).await {
                warn!("⚠️ Telegram /view không gửi được scenario view: {}", err);
                let _ = notifier
                    .send_html_message("⚠️ Không gửi được scenario view.")
                    .await;
            }
        }
        "/status" => {
            let snapshot = build_status_snapshot(state).await;
            let text = format_status_message(&snapshot);
            if let Err(err) = notifier.send_html_message(&text).await {
                warn!("⚠️ Telegram /status reply lỗi: {}", err);
            }
        }
        "/start-entry" => {
            if is_rate_limited_with(last_entry_check_at, chat_id, ENTRY_CHECK_RATE_LIMIT) {
                info!(
                    "⏳ Telegram /start-entry bị giới hạn 30s cho chat {}.",
                    chat_id
                );
                return;
            }
            match run_entry_check(state).await {
                Ok(result) => {
                    if let Err(err) = notifier.send_entry_check(&result).await {
                        warn!(
                            "⚠️ Telegram /start-entry không gửi được entry check: {}",
                            err
                        );
                        let _ = notifier
                            .send_html_message("⚠️ Không gửi được AI Entry Check.")
                            .await;
                    }
                }
                Err(err) => {
                    warn!("⚠️ Telegram /start-entry không khả dụng: {}", err);
                    if let Err(send_err) = notifier
                        .send_html_message(
                            "⚠️ AI Entry Check không khả dụng. Kiểm tra AI Decision Mode/API key rồi thử lại.",
                        )
                        .await
                    {
                        warn!("⚠️ Telegram /start-entry reply lỗi: {}", send_err);
                    }
                }
            }
        }
        "/help" => {
            if let Err(err) = notifier.send_html_message(HELP_TEXT).await {
                warn!("⚠️ Telegram /help reply lỗi: {}", err);
            }
        }
        _ => {
            if let Err(err) = notifier.send_html_message(HELP_TEXT).await {
                warn!("⚠️ Telegram unknown-command reply lỗi: {}", err);
            }
        }
    }
}

fn update_id(update: &serde_json::Value) -> Option<i64> {
    update.get("update_id").and_then(|value| value.as_i64())
}

/// Extract the first command token, strip a `@botname` suffix and lowercase it.
/// Plain text and non-command messages are ignored.
fn parse_command(text: &str) -> Option<String> {
    let token = text.split_whitespace().next()?;
    if !token.starts_with('/') {
        return None;
    }
    let command = token.split('@').next().unwrap_or(token).to_lowercase();
    (!command.is_empty()).then_some(command)
}

fn env_admin_chat_id() -> Option<i64> {
    std::env::var("TELEGRAM_ADMIN_CHAT_ID")
        .ok()
        .and_then(|value| value.trim().parse::<i64>().ok())
}

/// Explicit env admin wins; otherwise the configured Telegram chat ID is used
/// when it is a numeric chat ID.
fn resolve_admin_chat(env_admin: Option<i64>, config_chat_id: &str) -> Option<i64> {
    env_admin.or_else(|| config_chat_id.trim().parse::<i64>().ok())
}

fn unauthorized_message(chat_id: i64) -> String {
    format!(
        "Bot chưa cấu hình TELEGRAM_ADMIN_CHAT_ID.\n\
         Chat ID của bạn là {chat_id}. Thêm TELEGRAM_ADMIN_CHAT_ID={chat_id} vào rust-backend/.env rồi restart API."
    )
}

fn notifier_for_chat(bot_token: &str, chat_id: i64) -> TelegramNotifier {
    TelegramNotifier::new(bot_token.to_string(), chat_id.to_string())
}

fn is_rate_limited(last_command_at: &mut HashMap<i64, Instant>, chat_id: i64) -> bool {
    is_rate_limited_with(last_command_at, chat_id, COMMAND_RATE_LIMIT)
}

fn is_rate_limited_with(
    last_command_at: &mut HashMap<i64, Instant>,
    chat_id: i64,
    limit: Duration,
) -> bool {
    let now = Instant::now();
    if let Some(last) = last_command_at.get(&chat_id) {
        if now.saturating_duration_since(*last) < limit {
            return true;
        }
    }
    last_command_at.insert(chat_id, now);
    false
}

struct StatusSnapshot {
    scanner_active: bool,
    data_source: String,
    volume_source: String,
    feed_ok: bool,
    last_scan_age_secs: Option<i64>,
    macro_bias: String,
    macro_confidence: u8,
    flow_quality: String,
    signals_today: usize,
    max_signals_per_day: usize,
    daily_quota_enabled: bool,
    divergence_enabled: bool,
    ai_review_enabled: bool,
}

async fn build_status_snapshot(state: &AppState) -> StatusSnapshot {
    let config = state.config.read().await.clone();
    let signals = state.signals.read().await.clone();
    let active_data_source = state.active_data_source.read().await.clone();
    let active_volume_source = state.active_volume_source.read().await.clone();
    let last_scan_at = *state.last_scan_at.read().await;
    let macro_flow = state.macro_flow.read().await.clone();
    let now = Utc::now();
    let last_scan_age_secs = last_scan_at.map(|at| (now - at).num_seconds().max(0));
    let feed_ok = feed_is_ok(&active_data_source, last_scan_age_secs);

    let (macro_bias, macro_confidence, flow_quality) = macro_flow
        .map(|snapshot| {
            (
                snapshot.scorecard.macro_bias,
                snapshot.scorecard.confidence,
                snapshot.scorecard.flow_quality,
            )
        })
        .unwrap_or_else(|| ("UNKNOWN".to_string(), 0, "—".to_string()));

    StatusSnapshot {
        scanner_active: config.is_active,
        data_source: active_data_source,
        volume_source: active_volume_source,
        feed_ok,
        last_scan_age_secs,
        macro_bias,
        macro_confidence,
        flow_quality,
        signals_today: PersistentStore::count_live_signals_today(&signals, now),
        max_signals_per_day: config.max_signals_per_day,
        daily_quota_enabled: config.daily_quota_enabled,
        divergence_enabled: config.divergence_alert_enabled,
        ai_review_enabled: config.ai_signal_review_enabled,
    }
}

fn feed_is_ok(active_data_source: &str, last_scan_age_secs: Option<i64>) -> bool {
    if active_data_source.eq_ignore_ascii_case("FALLBACK_SEED")
        || active_data_source.eq_ignore_ascii_case("SPOT_BASIS_UNAVAILABLE")
    {
        return false;
    }
    last_scan_age_secs.is_some_and(|age| age <= 180)
}

fn format_status_message(snapshot: &StatusSnapshot) -> String {
    let scanner = if snapshot.scanner_active {
        "RUNNING"
    } else {
        "PAUSED"
    };
    let feed = if snapshot.feed_ok { "OK" } else { "DEGRADED" };
    let last_scan = snapshot
        .last_scan_age_secs
        .map(|age| format!("{age}s ago"))
        .unwrap_or_else(|| "—".to_string());
    let signals_line = if snapshot.daily_quota_enabled {
        format!(
            "Signals today: {}/{}",
            snapshot.signals_today, snapshot.max_signals_per_day
        )
    } else {
        format!("Signals today: {} (không giới hạn)", snapshot.signals_today)
    };

    format!(
        "📊 XAUUSD Bot\n\
         Scanner: {scanner}\n\
         Data: {} · volume {}\n\
         Feed: {feed} · last scan {last_scan}\n\
         Macro: {} · {}% · {}\n\
         {signals_line}\n\
         Divergence: {}\n\
         AI review: {}",
        TelegramNotifier::escape_html(&snapshot.data_source),
        TelegramNotifier::escape_html(&snapshot.volume_source),
        TelegramNotifier::escape_html(&snapshot.macro_bias),
        snapshot.macro_confidence,
        TelegramNotifier::escape_html(&snapshot.flow_quality),
        if snapshot.divergence_enabled {
            "ON"
        } else {
            "OFF"
        },
        if snapshot.ai_review_enabled {
            "ON"
        } else {
            "OFF"
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_command_strips_bot_suffix_and_lowercases() {
        assert_eq!(parse_command("/start"), Some("/start".to_string()));
        assert_eq!(
            parse_command("/START@GoldSignalBot"),
            Some("/start".to_string())
        );
        assert_eq!(
            parse_command("/status@GoldSignalBot extra"),
            Some("/status".to_string())
        );
        assert_eq!(
            parse_command("/start-entry@GoldSignalBot"),
            Some("/start-entry".to_string())
        );
        assert_eq!(parse_command("hello"), None);
        assert_eq!(parse_command("view"), None);
    }

    #[test]
    fn admin_chat_prefers_env_then_falls_back_to_config_chat_id() {
        assert_eq!(resolve_admin_chat(Some(-100111), "-100222"), Some(-100111));
        assert_eq!(resolve_admin_chat(None, "-100222"), Some(-100222));
        assert_eq!(resolve_admin_chat(None, "not-a-number"), None);
        assert_eq!(resolve_admin_chat(None, ""), None);
    }

    #[test]
    fn unauthorized_message_contains_chat_id_and_env_instruction() {
        let message = unauthorized_message(123456);
        assert!(message.contains("Bot chưa cấu hình TELEGRAM_ADMIN_CHAT_ID."));
        assert!(message.contains("Chat ID của bạn là 123456."));
        assert!(message.contains("TELEGRAM_ADMIN_CHAT_ID=123456"));
        assert!(message.contains("restart API"));
    }

    #[test]
    fn rate_limit_ignores_commands_within_two_seconds() {
        let mut last = HashMap::new();
        assert!(!is_rate_limited(&mut last, 42));
        assert!(is_rate_limited(&mut last, 42));
        assert!(!is_rate_limited(&mut last, 43));
    }

    #[test]
    fn entry_check_uses_longer_rate_limit() {
        let mut last = HashMap::new();
        assert!(!is_rate_limited_with(&mut last, 42, ENTRY_CHECK_RATE_LIMIT));
        assert!(is_rate_limited_with(&mut last, 42, ENTRY_CHECK_RATE_LIMIT));
        assert!(ENTRY_CHECK_RATE_LIMIT >= Duration::from_secs(30));
    }

    #[test]
    fn status_message_has_compact_shape() {
        let snapshot = StatusSnapshot {
            scanner_active: true,
            data_source: "TWELVEDATA".to_string(),
            volume_source: "BINANCE_PAXG_PROXY".to_string(),
            feed_ok: true,
            last_scan_age_secs: Some(42),
            macro_bias: "BEARISH".to_string(),
            macro_confidence: 95,
            flow_quality: "CONFIRMING".to_string(),
            signals_today: 0,
            max_signals_per_day: 1,
            daily_quota_enabled: true,
            divergence_enabled: true,
            ai_review_enabled: false,
        };

        let message = format_status_message(&snapshot);
        for expected in [
            "📊 XAUUSD Bot",
            "Scanner: RUNNING",
            "Data: TWELVEDATA · volume BINANCE_PAXG_PROXY",
            "Feed: OK · last scan 42s ago",
            "Macro: BEARISH · 95% · CONFIRMING",
            "Signals today: 0/1",
            "Divergence: ON",
            "AI review: OFF",
        ] {
            assert!(
                message.contains(expected),
                "missing status line: {expected}"
            );
        }
    }

    #[test]
    fn status_message_shows_unlimited_when_daily_quota_disabled() {
        let snapshot = StatusSnapshot {
            scanner_active: true,
            data_source: "BINANCE_PAXG".to_string(),
            volume_source: "BINANCE_PAXG".to_string(),
            feed_ok: true,
            last_scan_age_secs: Some(12),
            macro_bias: "BULLISH".to_string(),
            macro_confidence: 80,
            flow_quality: "CONFIRMING".to_string(),
            signals_today: 3,
            max_signals_per_day: 1,
            daily_quota_enabled: false,
            divergence_enabled: true,
            ai_review_enabled: true,
        };

        let message = format_status_message(&snapshot);
        assert!(message.contains("Signals today: 3 (không giới hạn)"));
        assert!(!message.contains("Signals today: 3/1"));
    }

    #[test]
    fn status_message_handles_paused_and_missing_feed() {
        let snapshot = StatusSnapshot {
            scanner_active: false,
            data_source: "FALLBACK_SEED".to_string(),
            volume_source: "NONE".to_string(),
            feed_ok: false,
            last_scan_age_secs: None,
            macro_bias: "UNKNOWN".to_string(),
            macro_confidence: 0,
            flow_quality: "—".to_string(),
            signals_today: 1,
            max_signals_per_day: 1,
            daily_quota_enabled: true,
            divergence_enabled: false,
            ai_review_enabled: true,
        };

        let message = format_status_message(&snapshot);
        assert!(message.contains("Scanner: PAUSED"));
        assert!(message.contains("Feed: DEGRADED · last scan —"));
        assert!(message.contains("Macro: UNKNOWN · 0% · —"));
        assert!(message.contains("Signals today: 1/1"));
        assert!(message.contains("Divergence: OFF"));
        assert!(message.contains("AI review: ON"));
    }

    #[test]
    fn help_text_lists_all_commands() {
        for command in [
            "/start",
            "/stop",
            "/view",
            "/start-entry",
            "/status",
            "/help",
        ] {
            assert!(
                HELP_TEXT.contains(command),
                "missing help command: {command}"
            );
        }
    }
}
