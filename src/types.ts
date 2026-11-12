export interface Candle {
  timestamp: number;
  open: number;
  high: number;
  low: number;
  close: number;
  volume: number;
  timeframe: string;
}

export interface SupportResistanceZone {
  level_type: 'Support' | 'Resistance';
  price: number;
  min_price: number;
  max_price: number;
  touch_count: number;
  strength_score: number;
}

export type SignalType = 'BUY' | 'SELL';
export type SignalStatus = 'Active' | 'Target1Hit' | 'Target2Hit' | 'StopLossHit' | 'Cancelled';
export type EntryType = 'MARKET' | 'LIMIT' | 'BREAKOUT';
export type EntryMode = 'MARKET' | 'LIMIT_RETEST' | 'BREAKOUT';

export type MacroBias = 'BULLISH' | 'BEARISH' | 'NEUTRAL' | 'AUTO';
export type TrendBias = 'BULLISH' | 'BEARISH' | 'NEUTRAL';

/** Read-only higher-timeframe snapshot (D1/H4/H1) from the Rust engine. */
export interface TimeframeContext {
  timeframe: string;
  bias: TrendBias;
  close: number;
  ema50: number;
  ema200: number;
  /** Donchian(20) position 0..1 (0 = lower band, 1 = upper band). */
  channel_position: number;
  atr14: number;
  nearest_support: number | null;
  nearest_resistance: number | null;
  candles: number;
  /** Timestamp (ms) of the latest closed candle in this timeframe. */
  last_candle_timestamp?: number;
}

/** Point-in-time Wilder RSI + divergence label. */
export interface RsiSnapshot {
  value: number | null;
  state: string;
  divergence: 'None' | 'Bullish' | 'Bearish' | string;
}

export interface MarketContext {
  d1: TimeframeContext;
  h4: TimeframeContext;
  h1: TimeframeContext;
  updated_at: string;
  price_source: string;
  spot_basis: number | null;
  /** RSI of the signal candle (computed live by the scanner/API). */
  rsi_m15?: RsiSnapshot | null;
  /** H1 RSI context (fixed 70/30 semantics). */
  rsi_h1?: RsiSnapshot | null;
  /** H4 RSI context (fixed 70/30 semantics). */
  rsi_h4?: RsiSnapshot | null;
}

/**
 * Standalone RSI divergence alert. Advisory momentum warning only:
 * it never contains entry/SL/TP and never consumes the Sniper quota.
 */
export interface DivergenceAlert {
  id: string;
  symbol: string;
  timeframe: 'M15' | 'H1' | 'H4' | string;
  direction: 'Bullish' | 'Bearish' | string;
  price: number;
  rsi: number;
  /** Timestamp (ms) of the HTF/M15 candle the alert refers to. */
  candle_timestamp: number;
  detected_at: string;
  /** Short Vietnamese explanation. */
  note: string;
}

/** GET /api/divergence-alerts response. */
export interface DivergenceAlertsResponse {
  enabled: boolean;
  timeframes: string[];
  /** Newest first, capped at 50 by the backend. */
  alerts: DivergenceAlert[];
}

/** One read-only macro/flow metric from FRED or Yahoo Finance. */
export interface MacroMetric {
  key: string;
  label: string;
  value: number | null;
  unit: string;
  change_1d: number | null;
  change_5d: number | null;
  change_5d_pct: number | null;
  volume_ratio_20d: number | null;
  date: string | null;
  source: string;
  note: string;
}

/** CFTC legacy futures-only COMEX gold positioning snapshot. */
export interface CotSnapshot {
  report_date: string;
  open_interest: number;
  noncommercial_long: number;
  noncommercial_short: number;
  noncommercial_net: number;
  noncommercial_net_change: number;
  net_pct_oi: number;
  open_interest_change: number;
}

/** Transparent macro/flow scorecard; context only, never an entry. */
export interface MacroFlowScorecard {
  macro_bias: 'BULLISH' | 'BEARISH' | 'NEUTRAL' | string;
  confidence: number;
  score: number;
  direction: 'UP' | 'DOWN' | 'MIXED' | string;
  driver: string;
  flow_quality: 'CONFIRMING' | 'DIVERGING' | 'MIXED' | 'INSUFFICIENT' | string;
  risk_regime: 'RISK_ON' | 'RISK_OFF' | 'NEUTRAL' | string;
  rate_regime: 'EASING' | 'TIGHTENING' | 'NEUTRAL' | string;
  usd_regime: 'STRONG' | 'WEAK' | 'NEUTRAL' | string;
  vol_regime: 'HIGH' | 'NORMAL' | 'LOW' | string;
  notes: string[];
}

/** GET /api/macro-flow snapshot (read-only). */
export interface MacroFlowSnapshot {
  updated_at: string;
  fred_ok: boolean;
  yahoo_ok: boolean;
  cot_ok: boolean;
  metrics: MacroMetric[];
  cot: CotSnapshot | null;
  scorecard: MacroFlowScorecard;
  warnings: string[];
  sources: string[];
}

export interface MacroFlowResponse {
  status: 'ok' | 'loading' | 'disabled';
  snapshot?: MacroFlowSnapshot;
}

/**
 * Filter checks evaluated by GET /api/live-view. Display-only: the preview never
 * applies them, so a setup can still show a blocked filter that the scanner would skip.
 */
export interface LiveViewFilters {
  htf_ok: boolean;
  htf_reason: string;
  rsi_ok: boolean;
  rsi_reason: string;
  delta_ok: boolean;
  delta_reason: string;
  /** Config flag only — the preview never calls the AI review. */
  ai_review_enabled: boolean;
  rsi_value?: number | null;
  rsi_state?: string | null;
  rsi_divergence?: string | null;
  delta_ratio?: number | null;
  cvd?: number | null;
}

/**
 * GET /api/live-view response. Read-only preview: never sends Telegram, stores a
 * signal, or consumes the daily Sniper quota.
 */
export interface LiveViewResponse {
  status: 'setup' | 'no_setup' | 'no_data';
  signal?: Signal;
  filters?: LiveViewFilters;
  message?: string;
  /** Present for no_data: number of candles currently buffered. */
  candles?: number;
  /** Present for no_setup: timestamp (ms) of the last closed candle. */
  candle_timestamp?: number;
  /** Present for no_setup: close price of the last closed candle. */
  last_close?: number;
  source?: string;
  volumeSource?: string;
  generated_at?: string;
}

/** Aggressive taker flow of the latest M15 candle + CVD over the lookback window. */
export interface VolumeDeltaSnapshot {
  buy_volume: number;
  sell_volume: number;
  delta: number;
  delta_ratio: number;
  cvd: number;
  cvd_change: number;
}

/** GET /api/candles response: closed OHLCV + dynamic S/R + EMA + volume delta. */
export interface CandlesResponse {
  candles: Candle[];
  supports: SupportResistanceZone[];
  resistances: SupportResistanceZone[];
  volumeMA: number;
  currentPrice: number;
  lastClosedPrice?: number;
  ema50?: number;
  ema200?: number;
  trendBias?: TrendBias;
  dataSource?: string;
  volumeSource?: string;
  priceSource?: string;
  spotPrice?: number | null;
  spotBasis?: number | null;
  volumeDelta?: VolumeDeltaSnapshot;
}

export interface Signal {
  id: string;
  symbol: string;
  signal_type: SignalType;
  timeframe: string;
  entry_price: number;
  stop_loss: number;
  take_profit_1: number;
  take_profit_2: number;
  risk_points: number;
  reward_points_tp1: number;
  reward_points_tp2: number;
  risk_reward_ratio: number;
  volume_surge_ratio: number;
  price_action_notes: string;
  volume_analysis_notes: string;
  full_rationale: string;
  status: SignalStatus;
  created_at: string;
  outcome_at?: string | null;
  telegram_message_id?: number | null;
  recommended_lot_size?: number;
  risk_amount_usd?: number;
  potential_profit_tp1_usd?: number;
  potential_profit_tp2_usd?: number;
  /** "{symbol}_{timeframe}_{candle_timestamp}" — prevents re-alerting the same closed candle. */
  dedup_key?: string;
  /** Timestamp of the closed candle that produced this signal. */
  candle_timestamp?: number;
  /** True for manual dashboard test setups. Simulated signals never consume the daily quota. */
  is_simulated?: boolean;
  /** Technical confluence score 0..100 (S/R touches + volume + wick + R:R). */
  confluence_score?: number;
  /** How the order is entered: market fill, limit retest, or breakout. */
  entry_type?: EntryType;
  /** Close price of the pinbar candle that produced the signal. */
  candle_close_price?: number;
  /** Live market price at signal generation time. */
  live_market_price?: number | null;
  /** Suggested limit retest price (midpoint between close and wick/zone). */
  retest_limit_price?: number | null;
  /** Price data source used for this signal. */
  data_source?: string;
  /** Whether a limit/breakout entry has been filled. */
  is_entry_filled?: boolean;
  /** AI Chief Risk Officer verdict: "BUY" | "SELL" | "STANDBY". Absent when review is off. */
  ai_verdict?: string | null;
  /** AI confidence 0..1 (advisory only; AI never sets live SL/TP). */
  ai_confidence?: number | null;
  /** Short AI risk explanation (advisory only). */
  ai_reason?: string | null;
  /** Signal-candle delta ratio (only present when the Volume Delta filter is enabled). */
  delta_ratio?: number | null;
  /** CVD over the configured lookback window. */
  cvd?: number | null;
  /** Absolute buy-minus-sell volume of the signal candle. */
  delta?: number | null;
  /** RSI (Wilder) of the signal candle — advisory/context. */
  rsi_value?: number | null;
  /** "Overbought" | "Neutral" | "Oversold". */
  rsi_state?: string | null;
  /** "None" | "Bullish" | "Bearish". */
  rsi_divergence?: string | null;
}

/** AI Chief Risk Officer review — veto-only, never live SL/TP. */
export interface AiSignalReview {
  action: 'BUY' | 'SELL' | 'STANDBY' | string;
  confidence_score: number;
  entry_zone?: string | null;
  dynamic_sl?: number | null;
  tp?: number | null;
  macro_trend_h1_h4?: string | null;
  /** RSI M15 echoed by the AI (nested object parsed server-side). */
  rsi_value?: number | null;
  rsi_state?: string | null;
  rsi_divergence?: string | null;
  /** Advisory execution rule proposed by the AI. */
  execution_rule?: string | null;
  reason: string;
  provider: string;
  model: string;
}

export interface PositionSizeCalculation {
  lotSize: number;
  riskAmountUsd: number;
  potentialProfitTp1Usd: number;
  potentialProfitTp2Usd: number;
  estimatedMarginUsd: number;
  riskAssessment: string;
}

export interface EconomicEvent {
  id: string;
  title: string;
  currency: string;
  impact: 'High' | 'Medium' | 'Low';
  scheduled_time: string;
  forecast?: string;
  previous?: string;
  actual?: string;
}

export interface NewsCalendarData {
  events: EconomicEvent[];
  isKillSwitchActive: boolean;
  activeEvent: EconomicEvent | null;
  killSwitchReason: string;
  killSwitchEnabled: boolean;
  macroBias: MacroBias;
  effectiveBias?: TrendBias;
  ema50?: number;
  trendBias?: TrendBias;
  dataSource?: string;
  sessionFilterEnabled?: boolean;
  sessionWindowOpen?: boolean;
}

export interface BotConfig {
  symbol: string;
  timeframe: string;
  min_risk_reward: number;
  /** Stop-loss buffer in gold dollars. New range: 5.0 – 25.0 (8–10 on M15, 15–20 on H1). */
  gold_sl_buffer_points: number;
  volume_surge_multiplier: number;
  /** Volume Absorption filter. Default true; turn off when the feed has no volume. */
  volume_filter_enabled: boolean;
  rejection_wick_ratio: number;
  lookback_period: number;
  volume_ma_period: number;
  risk_percentage: number;
  account_balance?: number;
  account_leverage?: number;
  telegram_bot_token: string;
  telegram_chat_id: string;
  is_active: boolean;
  kill_switch_enabled: boolean;
  pre_news_buffer_minutes: number;
  post_news_buffer_minutes: number;
  macro_bias: MacroBias;
  order_block_filter: boolean;
  /** Optional TradingView webhook passphrase (empty = not enforced). */
  webhook_passphrase: string;
  /** Maximum real (non-simulated) signals per calendar day. Default 1 — Sniper mode. */
  max_signals_per_day: number;
  /** Prefer the high-liquidity Vietnam session 13:00 – 23:00 (GMT+7). */
  session_filter_enabled: boolean;
  /** Send informational Telegram briefings before London/New York opens (Mon–Fri, DST-aware). */
  session_alerts_enabled: boolean;
  /** Minutes before session open to send the briefing (15–120). */
  session_alert_lead_minutes: number;
  /** Active free market data source. */
  data_source?: 'BINANCE_PAXG' | 'TWELVEDATA';
  /** Entry strategy: immediate market fill, limit retest, or breakout. */
  entry_mode: EntryMode;
  /** Broker price offset in USD applied to candles and live price. */
  broker_price_offset: number;
  /** Auto-calibrate Binance PAXG to the TwelveData/gold-api XAU/USD spot reference. */
  use_auto_spot_basis: boolean;
  /** Maximum allowed live-vs-close drift (USD) before falling back to a limit order. */
  max_entry_drift_points: number;
  /** Reject signals when the closed candle is older than this many seconds. */
  max_candle_age_secs: number;
  /** Optional H4 trend filter — blocks counter-H4 signals only when true. Default OFF. */
  htf_h4_filter_enabled: boolean;
  htf_d1_filter_enabled: boolean;
  /** Optional AI Chief Risk Officer veto gate. Default OFF — no behavior change when false. */
  ai_signal_review_enabled?: boolean;
  /** Minimum AI confidence (0.5–1.0) required to let a signal through. Default 0.75. */
  ai_signal_review_min_confidence?: number;
  /** When AI errors/times out: true = pass (fail-open, default), false = block. */
  ai_signal_review_fail_open?: boolean;
  /** Optional Volume Delta/CVD gate. Default OFF — no behavior change when false. */
  volume_delta_filter_enabled: boolean;
  /** ALIGN = aggressive flow on trade side; DIVERGENCE = price/CVD divergence; BOTH = OR. */
  volume_delta_mode: 'ALIGN' | 'DIVERGENCE' | 'BOTH';
  /** CVD/divergence lookback in candles (5–100). */
  volume_delta_lookback: number;
  /** Minimum |delta_ratio| for ALIGN mode (0–1). */
  volume_delta_min_ratio: number;
  /** Optional hard RSI gate. Default OFF — no behavior change when false. */
  rsi_filter_enabled?: boolean;
  /** Wilder RSI period (2–100, default 14). */
  rsi_period?: number;
  /** Overbought threshold (50–100, default 70). */
  rsi_overbought?: number;
  /** Oversold threshold (0–50, default 30). */
  rsi_oversold?: number;
  /** Enable the divergence component of the RSI gate. Default true. */
  rsi_divergence_enabled?: boolean;
  /** Divergence pivot lookback (10–200, default 40). */
  rsi_divergence_lookback?: number;
  /** Pivot confirmation candles per side (1–10, default 2). */
  rsi_pivot_k?: number;
  /** EXTREME | DIVERGENCE | BOTH. */
  rsi_mode?: 'EXTREME' | 'DIVERGENCE' | 'BOTH';
  /** Standalone RSI divergence alerts. Default true; independent of trade signals/quota. */
  divergence_alert_enabled?: boolean;
  /** Alert on M15 divergence (default true). */
  divergence_alert_m15?: boolean;
  /** Alert on H1 divergence (default true). */
  divergence_alert_h1?: boolean;
  /** Alert on H4 divergence (default true). */
  divergence_alert_h4?: boolean;
  /** Minimum minutes between alerts for the same timeframe + direction (5–1440, default 60). */
  divergence_alert_cooldown_minutes?: number;
  /** Read-only Macro & Flow dashboard (FRED/Yahoo/CFTC). Default true; never affects signals. */
  macro_flow_enabled?: boolean;
}

export interface DailyMacroReport {
  date: string;
  macro_bias: TrendBias;
  confidence_score: number;
  gold_range_forecast: string;
  daily_rationale: string;
  high_impact_events_today: string[];
  recommended_action: string;
  updated_at: string;
}

export interface TradingViewWebhookPayload {
  passphrase?: string;
  ticker: string;
  action: 'BUY' | 'SELL' | 'CANDLE';
  price: number;
  open?: number;
  high?: number;
  low?: number;
  close?: number;
  volume?: number;
  timeframe?: string;
  comment?: string;
  timestamp?: number;
}
