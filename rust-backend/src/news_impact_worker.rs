//! News impact worker (5-minute cadence).
//!
//! Responsibilities:
//! 1. Refresh `actual` from ForexFactory for High-Impact USD events that just
//!    passed (strict fetch, never the offline mock fallback).
//! 2. Rebuild `data/news_reactions.json` records from closed M15 candles.
//! 3. Run the capped AI News Analyst after `actual` is available.
//!
//! This worker is advisory only. It never touches signals, quota, Telegram,
//! kill switch, score or the Risk Officer prompt.

use crate::api::AppState;
use crate::models::BotConfig;
use crate::news_analyst::analyze_news_impact;
use crate::news_filter::{EconomicEvent, ImpactLevel};
use crate::news_impact::{
    build_record, summarize, upsert_record, GoldRelevance, NewsImpactRecord, RecordContext,
    DEFAULT_REACTION_HORIZON_MINUTES, MAX_NEWS_IMPACT_RECORDS,
};
use crate::scheduler::DailyScheduler;
use chrono::{DateTime, Duration, Utc};
use std::collections::HashMap;
use tokio::time::{interval, Duration as TokioDuration};
use tracing::{info, warn};

const WORKER_INTERVAL_SECS: u64 = 300;
const ACTUAL_REFRESH_WINDOW_MINUTES: i64 = 60;
const ACTUAL_REFRESH_MAX_ATTEMPTS: u8 = 12;
const RECORD_LOOKBACK_HOURS: i64 = 72;
const AI_RETRY_AFTER_MINUTES: i64 = 30;
const AI_MAX_ATTEMPTS_PER_EVENT: u8 = 2;

/// A High/Medium-Impact USD event that just passed and still has no `actual`.
pub(crate) fn needs_actual_refresh(event: &EconomicEvent, now: DateTime<Utc>) -> bool {
    event.impact != ImpactLevel::Low
        && event.currency.eq_ignore_ascii_case("USD")
        && event.actual.is_none()
        && event.scheduled_time <= now
        && event.scheduled_time >= now - Duration::minutes(ACTUAL_REFRESH_WINDOW_MINUTES)
}

/// Cost/quality gate for the AI analyst.
pub(crate) fn eligible_for_ai(
    record: &NewsImpactRecord,
    config: &BotConfig,
    now: DateTime<Utc>,
) -> bool {
    config.news_ai_analyst_enabled
        && config.news_ai_daily_cap > 0
        && record.impact == ImpactLevel::High
        && record.currency.eq_ignore_ascii_case("USD")
        && record.gold_relevance == GoldRelevance::High
        && record.actual.is_some()
        && record.ai.is_none()
        && record.ai_attempts < AI_MAX_ATTEMPTS_PER_EVENT
        && now >= record.scheduled_time + Duration::minutes(5)
        && now <= record.scheduled_time + Duration::hours(24)
        && record
            .ai_attempted_at
            .map(|last| now - last >= Duration::minutes(AI_RETRY_AFTER_MINUTES))
            .unwrap_or(true)
}

async fn refresh_actuals(
    state: &AppState,
    scheduler: &DailyScheduler,
    attempts: &mut HashMap<String, u8>,
) {
    let now = Utc::now();
    let pending: Vec<String> = {
        let news = state.news_filter.read().await;
        news.calendar_events
            .iter()
            .filter(|event| needs_actual_refresh(event, now))
            .filter(|event| {
                attempts.get(&event.id).copied().unwrap_or(0) < ACTUAL_REFRESH_MAX_ATTEMPTS
            })
            .map(|event| event.id.clone())
            .collect()
    };
    if pending.is_empty() {
        return;
    }
    let pending_count = pending.len();

    let Some(fresh) = scheduler.fetch_economic_events_strict().await else {
        for id in pending {
            let entry = attempts.entry(id).or_insert(0);
            *entry = entry.saturating_add(1);
        }
        warn!(
            "⚠️ [News Impact] Không refresh được actual cho {} sự kiện (network/HTTP/parse).",
            pending_count
        );
        return;
    };

    let touched = {
        let mut news = state.news_filter.write().await;
        news.merge_event_details(&fresh)
    };
    for id in pending {
        let entry = attempts.entry(id).or_insert(0);
        *entry = entry.saturating_add(1);
    }
    info!(
        "📰 [News Impact] Đã refresh actual: {} sự kiện chờ, {} event được cập nhật/bổ sung.",
        pending_count, touched
    );
}

/// Spawn the 5-minute news-impact worker.
pub fn spawn_news_impact_worker(state: AppState) {
    tokio::spawn(async move {
        info!("📰 [News Impact] Worker khởi động (chu kỳ 5 phút, advisory-only).");
        tokio::time::sleep(TokioDuration::from_secs(30)).await;
        let scheduler = DailyScheduler::from_env();
        let mut ticker = interval(TokioDuration::from_secs(WORKER_INTERVAL_SECS));
        let mut actual_attempts: HashMap<String, u8> = HashMap::new();

        loop {
            ticker.tick().await;
            let config = state.config.read().await.clone();
            if !config.news_reaction_enabled {
                continue;
            }

            refresh_actuals(&state, &scheduler, &mut actual_attempts).await;

            let now = Utc::now();
            let events = state.news_filter.read().await.calendar_events.clone();
            let candles = state.candles.read().await.clone();
            let h4_atr = state
                .market_context
                .read()
                .await
                .as_ref()
                .map(|context| context.h4.atr14);
            let price_source = state.active_data_source.read().await.clone();
            let volume_source = state.active_volume_source.read().await.clone();
            let horizon = if config.news_reaction_horizon_minutes > 0 {
                config.news_reaction_horizon_minutes
            } else {
                DEFAULT_REACTION_HORIZON_MINUTES
            };

            let mut records = state.news_impact.read().await.clone();
            let mut changed = false;
            let cutoff = now - Duration::hours(RECORD_LOOKBACK_HOURS);

            for event in events
                .iter()
                .filter(|event| event.scheduled_time <= now && event.scheduled_time >= cutoff)
            {
                let existing = records
                    .iter()
                    .find(|record| record.event_id == event.id)
                    .cloned();
                // Freeze completed records: once the 240-minute window is over,
                // a changing H4 ATR must not relabel a historical reaction.
                // AI may still attach later through the separate AI loop below.
                if existing
                    .as_ref()
                    .map(|record| record.status == "COMPLETE")
                    .unwrap_or(false)
                {
                    continue;
                }
                let mut record = build_record(
                    event,
                    existing.as_ref(),
                    &RecordContext {
                        candles: &candles,
                        h4_atr,
                        price_source: &price_source,
                        volume_source: &volume_source,
                        horizon_minutes: horizon,
                        now,
                    },
                );
                if let Some(existing) = existing.as_ref() {
                    // `updated_at` changes every tick; compare content first so
                    // an unchanged record does not rewrite the JSON file every
                    // five minutes.
                    record.updated_at = existing.updated_at;
                    if &record == existing {
                        continue;
                    }
                    record.updated_at = now;
                }
                upsert_record(&mut records, record);
                changed = true;
            }

            if config.news_ai_analyst_enabled && config.news_ai_daily_cap > 0 {
                let today = now.date_naive();
                let mut calls_today: usize = records
                    .iter()
                    .filter(|record| {
                        record
                            .ai_attempted_at
                            .map(|time| time.date_naive() == today)
                            .unwrap_or(false)
                    })
                    .map(|record| record.ai_attempts as usize)
                    .sum();

                for record in records.iter_mut() {
                    if calls_today >= config.news_ai_daily_cap {
                        break;
                    }
                    if !eligible_for_ai(record, &config, now) {
                        continue;
                    }

                    record.ai_attempted_at = Some(now);
                    record.ai_attempts = record.ai_attempts.saturating_add(1);
                    calls_today += 1;
                    changed = true;

                    match analyze_news_impact(record).await {
                        Some(view) => {
                            info!(
                                "📰 [News Impact] AI {} → {} {}% ({})",
                                record.event_id,
                                view.gold_lean.as_str(),
                                view.confidence,
                                view.model
                            );
                            record.ai = Some(view);
                        }
                        None => {
                            warn!(
                                "⚠️ [News Impact] AI không trả về view hợp lệ cho {} (attempt {}/{}).",
                                record.event_id, record.ai_attempts, AI_MAX_ATTEMPTS_PER_EVENT
                            );
                        }
                    }
                }
            }

            if !changed {
                continue;
            }

            records.sort_by_key(|record| record.scheduled_time);
            if records.len() > MAX_NEWS_IMPACT_RECORDS {
                let excess = records.len() - MAX_NEWS_IMPACT_RECORDS;
                records.drain(0..excess);
            }
            *state.news_impact.write().await = records.clone();

            let store = state.news_impact_store.clone();
            let to_save = records.clone();
            match tokio::task::spawn_blocking(move || store.save(&to_save)).await {
                Ok(Ok(())) => {}
                Ok(Err(err)) => warn!("⚠️ [News Impact] Không lưu được record: {}", err),
                Err(err) => warn!("⚠️ [News Impact] Join error khi lưu record: {}", err),
            }

            let summary = summarize(&records);
            info!(
                "📰 [News Impact] records={} measured={} up={} down={} flat={} unknown={} · AI {}/{}",
                summary.total_events,
                summary.measured_events,
                summary.up,
                summary.down,
                summary.flat,
                summary.unknown,
                summary.ai_correct,
                summary.ai_total
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::news_impact::{EventClass, Lean, NewsAiView, Reaction};
    use chrono::TimeZone;

    fn at(hour: u32, minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 17, hour, minute, 0)
            .single()
            .expect("valid time")
    }

    fn event(scheduled_time: DateTime<Utc>) -> EconomicEvent {
        EconomicEvent {
            id: "FF-1".to_string(),
            title: "Core CPI m/m".to_string(),
            currency: "USD".to_string(),
            impact: ImpactLevel::High,
            scheduled_time,
            forecast: Some("0.3%".to_string()),
            previous: Some("0.2%".to_string()),
            actual: None,
        }
    }

    #[test]
    fn actual_refresh_only_for_recent_high_usd_missing_actual() {
        let now = at(13, 45);
        assert!(needs_actual_refresh(&event(at(13, 30)), now));
        assert!(!needs_actual_refresh(&event(at(12, 0)), now));
        assert!(!needs_actual_refresh(&event(at(14, 0)), now));

        let mut with_actual = event(at(13, 30));
        with_actual.actual = Some("0.4%".to_string());
        assert!(!needs_actual_refresh(&with_actual, now));

        let mut medium = event(at(13, 30));
        medium.impact = ImpactLevel::Medium;
        assert!(needs_actual_refresh(&medium, now));
    }

    #[test]
    fn ai_requires_high_usd_actual_and_relevance() {
        let now = at(14, 0);
        let config = BotConfig::default();
        let mut record = NewsImpactRecord {
            event_id: "FF-1".to_string(),
            impact: ImpactLevel::High,
            currency: "USD".to_string(),
            gold_relevance: GoldRelevance::High,
            actual: Some("0.5%".to_string()),
            scheduled_time: at(13, 30),
            ..NewsImpactRecord::default()
        };
        assert!(eligible_for_ai(&record, &config, now));

        record.actual = None;
        assert!(!eligible_for_ai(&record, &config, now));

        record.actual = Some("0.5%".to_string());
        record.gold_relevance = GoldRelevance::Medium;
        assert!(!eligible_for_ai(&record, &config, now));

        record.gold_relevance = GoldRelevance::High;
        record.ai = Some(NewsAiView {
            gold_lean: Lean::Down,
            ..NewsAiView::default()
        });
        assert!(!eligible_for_ai(&record, &config, now));
    }

    #[test]
    fn ai_respects_retry_window_and_attempt_cap() {
        let now = at(14, 0);
        let config = BotConfig::default();
        let mut record = NewsImpactRecord {
            event_id: "FF-1".to_string(),
            impact: ImpactLevel::High,
            currency: "USD".to_string(),
            gold_relevance: GoldRelevance::High,
            actual: Some("0.5%".to_string()),
            scheduled_time: at(13, 30),
            ai_attempted_at: Some(at(13, 55)),
            ai_attempts: 1,
            ..NewsImpactRecord::default()
        };
        assert!(!eligible_for_ai(&record, &config, now));

        record.ai_attempted_at = Some(at(13, 20));
        assert!(eligible_for_ai(&record, &config, now));

        record.ai_attempts = 2;
        assert!(!eligible_for_ai(&record, &config, now));
    }

    #[test]
    fn ai_never_runs_before_five_minutes_after_event() {
        let now = at(13, 32);
        let config = BotConfig::default();
        let record = NewsImpactRecord {
            event_id: "FF-1".to_string(),
            impact: ImpactLevel::High,
            currency: "USD".to_string(),
            gold_relevance: GoldRelevance::High,
            actual: Some("0.5%".to_string()),
            scheduled_time: at(13, 30),
            ..NewsImpactRecord::default()
        };
        assert!(!eligible_for_ai(&record, &config, now));
    }

    #[test]
    fn reaction_fields_do_not_affect_ai_eligibility() {
        let now = at(14, 0);
        let config = BotConfig::default();
        let record = NewsImpactRecord {
            event_id: "FF-1".to_string(),
            impact: ImpactLevel::High,
            currency: "USD".to_string(),
            gold_relevance: GoldRelevance::High,
            actual: Some("0.5%".to_string()),
            scheduled_time: at(13, 30),
            event_class: EventClass::Inflation,
            reaction: Reaction::Down,
            ..NewsImpactRecord::default()
        };
        assert!(eligible_for_ai(&record, &config, now));
    }
}
