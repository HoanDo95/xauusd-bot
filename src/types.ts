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
}

export interface MarketContext {
  d1: TimeframeContext;
  h4: TimeframeContext;
  h1: TimeframeContext;
  updated_at: string;
  price_source: string;
  spot_basis: number | null;
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
