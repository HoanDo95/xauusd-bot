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
  /** EMA50 change over the last 3 closed candles. */
  ema50_slope_3?: number;
  /** "HH_HL" | "LH_LL" | "MIXED" | "UNKNOWN". */
  structure?: string;
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
  /** D1 RSI context (fixed 70/30 semantics). */
  rsi_d1?: RsiSnapshot | null;
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
 * Advisory alignment between a candidate signal and the macro/flow snapshot.
 * Display-only: it never blocks a signal or changes SL/TP/quota.
 */
export interface MacroFlowAlignment {
  bias: 'BULLISH' | 'BEARISH' | 'NEUTRAL' | 'UNKNOWN' | string;
  /** 0..95 */
  confidence: number;
  score: number;
  flowQuality: 'CONFIRMING' | 'DIVERGING' | 'MIXED' | 'INSUFFICIENT' | 'UNKNOWN' | string;
  alignment: 'ALIGNED' | 'COUNTER' | 'MIXED' | 'UNKNOWN' | string;
  /** Short Vietnamese explanation. */
  note: string;
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
  /** Advisory macro/flow alignment (present when a snapshot exists). */
  macroFlow?: MacroFlowAlignment;
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

/**
 * One read-only reference scenario from GET /api/scenarios. It is never an
 * executable signal: no Telegram, no quota, no persistence.
 */
export interface Scenario {
  id: string;
  name: string;
  bias: 'BULLISH' | 'BEARISH' | 'NEUTRAL' | string;
  /** Human-readable trigger condition. */
  trigger: string;
  /** Short condition text. */
  condition: string;
  entry: number | null;
  stop_loss: number | null;
  take_profit_1: number | null;
  take_profit_2: number | null;
  risk_reward: number | null;
  /** Structured trigger price watched by the read-only action-trigger worker. */
  trigger_price: number | null;
  /** ABOVE | BELOW | TOUCH | NONE. */
  trigger_direction: 'ABOVE' | 'BELOW' | 'TOUCH' | 'NONE' | string;
  /** Timeframe used to evaluate the trigger (M15). */
  trigger_timeframe: string;
  invalidation: string;
  /** 0..95 */
  confidence: number;
  macro_alignment: 'ALIGNED' | 'COUNTER' | 'MIXED' | 'UNKNOWN' | string;
  /** Levels ordered + R:R >= min + risk within range. */
  valid: boolean;
  /** Display ranking: PRIMARY | ALTERNATE | WATCHLIST. */
  role: 'PRIMARY' | 'ALTERNATE' | 'WATCHLIST' | string;
  /** Transparent ranking score; higher is stronger. */
  priority: number;
  /** Vietnamese notes, including why the scenario is invalid. */
  notes: string[];
}

/** Read-only market snapshot that accompanies the reference scenarios. */
export interface ScenarioMarket {
  h4_bias: string;
  h1_bias: string;
  d1_bias: string;
  h4_atr: number;
  nearest_support: number | null;
  nearest_resistance: number | null;
  rsi_m15: number | null;
  rsi_state: string;
  rsi_h1?: number | null;
  rsi_h4?: number | null;
  rsi_d1?: number | null;
  daily_change_pct?: number | null;
  macro_bias: string;
  macro_confidence: number;
}

/** GET /api/scenarios response. Read-only; never an entry confirmation. */
export interface ScenarioResponse {
  status: 'ok' | 'no_data';
  generated_at: string;
  price: number | null;
  source: string;
  volume_source: string;
  market: ScenarioMarket | null;
  scenarios: Scenario[];
  warnings: string[];
}

/** Action verdict from GET /api/action-view. Never an automatic order. */
export type ActionVerdict = 'BUY' | 'SELL' | 'WAIT' | 'NO_TRADE';

/**
 * GET /api/action-view response — a single read-only "Should I enter now?" view.
 * It only reports the live view + primary scenario + macro/flow context; it never
 * sends Telegram, inserts a signal, consumes quota or places an order.
 */
export interface ActionViewResponse {
  verdict: ActionVerdict;
  headline: string;
  reason: string;
  caution: 'COUNTER_MACRO' | 'FILTER_BLOCKED' | 'VOLUME_PROXY' | null;
  signal: Signal | null;
  primaryScenario: Scenario | null;
  /** Absolute USD distance from current price to the PRIMARY trigger (null when unknown). */
  distanceToTrigger?: number | null;
  /** distanceToTrigger divided by H4 ATR (null when price/trigger/ATR is missing). */
  distanceAtr?: number | null;
  /** NEAR <= 0.5 ATR, MID <= 1.5 ATR, FAR > 1.5 ATR, UNKNOWN when missing. */
  proximity?: 'NEAR' | 'MID' | 'FAR' | 'UNKNOWN' | string;
  macroFlow: MacroFlowAlignment | null;
  volumeQuality: 'PAXG_PROXY' | 'PAXG' | 'FUTURES_PROXY' | 'EXNESS_TICK' | 'OANDA_TICK' | 'NONE' | 'XAUUSD' | 'UNKNOWN';
  volumeWarning: string | null;
  /** Whether the read-only PRIMARY-scenario trigger Telegram alert is enabled. */
  triggerAlertEnabled?: boolean;
  /** Latest AI Decision (DECISION mode) — advisory, never an automatic order. */
  aiDecision?: AiDecision | null;
  source: string;
  volumeSource: string;
  generated_at: string;
}

/** Three operating states: NO_TRADE / MARKET_VIEW / SNIPER_TRIGGER. */
export type BotState = 'NO_TRADE' | 'MARKET_VIEW' | 'SNIPER_TRIGGER';

/** Reference scenario inside the H1 Market View. Levels are a plan, not an entry. */
export interface H1Confirmation {
  status: 'CONFIRMED' | 'PENDING' | 'INVALID' | 'UNKNOWN' | string;
  bias: string;
  close?: number | null;
  trigger_price?: number | null;
  atr14?: number | null;
  momentum_atr?: number | null;
  note: string;
  m15_entry_zone_low?: number | null;
  m15_entry_zone_high?: number | null;
  m15_entry_note: string;
}

export interface OutlookScenario {
  role: string;
  scenario_id: string;
  action: 'WATCH_SELL' | 'WATCH_BUY' | 'STAND_ASIDE' | string;
  /** Scenario passed level ordering, R:R and risk validation. */
  valid: boolean;
  zone_low: number;
  zone_high: number;
  trigger: string;
  stop_loss: number | null;
  take_profit_1: number | null;
  take_profit_2: number | null;
  confidence: number;
  invalidation: string;
  /** Advisory H1 confirmation + M15 retest plan; never a signal by itself. */
  h1_confirmation?: H1Confirmation | null;
}

/**
 * GET /api/market-view — always-available H1 Market Outlook.
 * Deterministic levels from the scenario builder; never an automatic order.
 */
export interface StrategistView {
  generated_at: string;
  model: string;
  prompt_version: string;
  market_view: string;
  structure: string;
  primary_scenario_id: string | null;
  primary_action: string | null;
  alternative_scenario_id: string | null;
  risk_notes: string[];
}

/** Deterministic 60/30/10 score for the PRIMARY scenario. */
export interface MarketScore {
  structure: number;
  trigger: number;
  macro_score: number;
  total: number;
  notes: string[];
}

export interface MarketOutlook {
  bot_state: BotState;
  generated_at: string;
  valid_until: string;
  /** ACTIVE | TRIGGERED | RETEST_WINDOW | INVALIDATED | NO_TRADE */
  plan_status?: string;
  structure: string;
  market_view: string;
  primary: OutlookScenario | null;
  alternative: OutlookScenario | null;
  risk_notes: string[];
  no_trade_reason: string | null;
  session: 'IN' | 'OUT' | string;
  spread_proxy_atr: number | null;
  d1_bias: string;
  h4_bias: string;
  h1_bias: string;
  price: number | null;
  h4_atr: number | null;
  /** Optional AI Market Strategist narrative. Levels remain deterministic. */
  strategist?: StrategistView | null;
  /** Deterministic 60/30/10 score for the PRIMARY scenario. */
  score?: MarketScore | null;
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
  /** "ALIGNED" | "COUNTER" | "MIXED" | "UNKNOWN" — advisory only, never blocks. */
  macro_flow_alignment?: string | null;
  /** Short Vietnamese macro/flow note (advisory only). */
  macro_flow_note?: string | null;
}

/** AI Decision Mode verdict — primary BUY/SELL/STANDBY, never live SL/TP. */
export interface AiDecision {
  action: 'BUY' | 'SELL' | 'STANDBY' | string;
  /** 0..1 */
  confidence_score: number;
  selected_scenario: 'PRIMARY' | 'ALTERNATE' | 'WATCHLIST' | 'NONE' | null | string;
  reason: string;
  execution_rule?: string | null;
  macro_flow_alignment?: 'ALIGNED' | 'COUNTER' | 'MIXED' | 'UNKNOWN' | string | null;
  macro_flow_note?: string | null;
  provider: string;
  model: string;
}

/**
 * Reference entry plan from a manual AI Entry Check. Derived from the
 * deterministic candidate or a scenario; never an inserted trade signal.
 */
export interface EntryPlan {
  side: 'BUY' | 'SELL' | string;
  source: 'CANDIDATE' | 'PRIMARY' | 'ALTERNATE' | 'WATCHLIST' | string;
  entry: number;
  stop_loss: number;
  take_profit_1: number;
  take_profit_2: number;
  risk_reward: number;
  valid: boolean;
  /** Scenario confidence / candidate confluence, when known. */
  readiness?: number | null;
  /** True when the scenario cleared the early-entry threshold before trigger confirmation. */
  early_entry?: boolean;
  note: string;
}

/**
 * POST /api/ai/entry-check response. On-demand only: no Telegram send, no
 * signal insertion/persistence, no quota consumption and no auto-trading.
 */
export interface EntryCheckResult {
  decision: AiDecision;
  live_view: LiveViewResponse;
  scenarios: ScenarioResponse;
  entry_plan: EntryPlan | null;
  generated_at: string;
  /** Cached H1 AI Market Strategist view (advisory; never an entry/signal). */
  market_strategist?: StrategistView | null;
  /** Deterministic panorama: volatility, structure, zones, scenarios, news. */
  panorama?: MarketPanorama | null;
}

export interface MarketPanorama {
  generated_at: string;
  price: number | null;
  session: string;
  weekend: boolean;
  source: string;
  volume_source: string;
  volatility: PanoramaVolatility;
  structure: PanoramaStructure;
  zones: PanoramaZones;
  scenarios: PanoramaScenario[];
  macro_block: PanoramaMacro | null;
  news: PanoramaNews;
  score: MarketScore | null;
}

export interface PanoramaVolatility {
  h4_atr: number | null;
  atr_pct: number | null;
  daily_change_pct: number | null;
  spread_usd: number | null;
  spread_proxy_atr: number | null;
  regime: string;
  m15_range: number | null;
}

export interface PanoramaStructure {
  d1_bias: string;
  h4_bias: string;
  h1_bias: string;
  d1_structure: string;
  h4_structure: string;
  h1_structure: string;
  h4_atr: number | null;
  price_vs_ema50_m15: number | null;
}

export interface PanoramaZones {
  nearest_support: number | null;
  nearest_resistance: number | null;
  supports: PanoramaZone[];
  resistances: PanoramaZone[];
  htf_levels: PanoramaHtfLevel[];
}

export interface PanoramaZone {
  price: number;
  min_price: number;
  max_price: number;
  touch_count: number;
  strength: number;
  distance_usd: number;
  distance_atr: number | null;
}

export interface PanoramaHtfLevel {
  timeframe: string;
  kind: string;
  price: number;
  distance_atr: number | null;
}

export interface PanoramaScenario {
  role: string;
  scenario_id: string;
  name: string;
  bias: string;
  action: string;
  valid: boolean;
  zone_low: number;
  zone_high: number;
  trigger_price: number | null;
  trigger_direction: string;
  trigger_timeframe: string;
  entry: number | null;
  stop_loss: number | null;
  take_profit_1: number | null;
  take_profit_2: number | null;
  risk_reward: number | null;
  confidence: number;
  macro_alignment: string;
  invalidation: string;
  h1_confirmation?: H1Confirmation | null;
}

export interface PanoramaMacro {
  bias: string;
  confidence: number;
  score: number;
  flow_quality: string;
  alignment: string;
  note: string;
}

export interface PanoramaNews {
  next_high_impact_title: string | null;
  minutes_until: number | null;
  kill_switch_active: boolean;
  kill_switch_reason: string;
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
  /** AI view of macro/flow alignment (advisory; falls back to deterministic snapshot). */
  macro_flow_alignment?: string | null;
  macro_flow_note?: string | null;
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
  /** Server-computed relative to the kill-switch window. */
  status?: 'PAST' | 'ACTIVE' | 'UPCOMING' | string;
  minutesUntil?: number;
}

export interface NewsCalendarData {
  events: EconomicEvent[];
  isKillSwitchActive: boolean;
  activeEvent: EconomicEvent | null;
  killSwitchReason: string;
  killSwitchEnabled: boolean;
  macroBias: MacroBias;
  /** Next future High-Impact USD event, for the news banner. */
  nextHighImpactEvent?: EconomicEvent | null;
  nextHighImpactInMinutes?: number | null;
  effectiveBias?: TrendBias;
  ema50?: number;
  trendBias?: TrendBias;
  dataSource?: string;
  sessionFilterEnabled?: boolean;
  sessionWindowOpen?: boolean;
}

export type NewsImpactEventClass =
  | 'INFLATION'
  | 'LABOR'
  | 'GROWTH'
  | 'CONSUMPTION'
  | 'HOUSING'
  | 'MANUFACTURING'
  | 'SENTIMENT'
  | 'FED'
  | 'GEOPOLITICAL'
  | 'OTHER';

export type NewsImpactGoldRelevance = 'HIGH' | 'MEDIUM' | 'LOW';
export type NewsImpactLean = 'UP' | 'DOWN' | 'UNCLEAR';
export type NewsImpactReaction = 'UP' | 'DOWN' | 'FLAT' | 'UNKNOWN';
export type NewsImpactHawkishDovish = 'HAWKISH' | 'DOVISH' | 'NEUTRAL' | 'UNKNOWN';

export interface NewsAiView {
  generated_at: string;
  model: string;
  prompt_version: string;
  event_class: NewsImpactEventClass;
  gold_relevance: NewsImpactGoldRelevance;
  hawkish_dovish: NewsImpactHawkishDovish;
  gold_lean: NewsImpactLean;
  confidence: number;
  conditions?: string | null;
  reasoning: string;
}

export interface NewsImpactRecord {
  schema: number;
  event_id: string;
  title: string;
  currency: string;
  impact: 'High' | 'Medium' | 'Low';
  scheduled_time: string;
  event_class: NewsImpactEventClass;
  gold_relevance: NewsImpactGoldRelevance;
  forecast?: string | null;
  previous?: string | null;
  actual?: string | null;
  surprise?: number | null;
  surprise_pct?: number | null;
  conditional_lean: NewsImpactLean;
  conditional_reason: string;
  status: 'UPCOMING' | 'MEASURING' | 'COMPLETE' | 'UNKNOWN' | string;
  baseline_price?: number | null;
  baseline_time?: string | null;
  return_15m?: number | null;
  return_60m?: number | null;
  return_240m?: number | null;
  return_15m_pct?: number | null;
  return_60m_pct?: number | null;
  return_240m_pct?: number | null;
  mfe_points?: number | null;
  mae_points?: number | null;
  volume_ratio?: number | null;
  h4_atr?: number | null;
  reaction: NewsImpactReaction;
  reaction_reason: string;
  price_source: string;
  volume_source: string;
  ai?: NewsAiView | null;
  ai_attempted_at?: string | null;
  ai_attempts: number;
  created_at: string;
  updated_at: string;
}

export interface NewsImpactBucket {
  event_class: NewsImpactEventClass;
  count: number;
  up: number;
  down: number;
  flat: number;
  unknown: number;
  avg_abs_move_60m?: number | null;
  conditional_hit_rate?: number | null;
  ai_hit_rate?: number | null;
}

export interface NewsImpactSummary {
  total_events: number;
  measured_events: number;
  up: number;
  down: number;
  flat: number;
  unknown: number;
  avg_abs_move_15m?: number | null;
  avg_abs_move_60m?: number | null;
  avg_abs_move_240m?: number | null;
  conditional_total: number;
  conditional_correct: number;
  conditional_hit_rate?: number | null;
  ai_total: number;
  ai_correct: number;
  ai_hit_rate?: number | null;
  majority_down_share?: number | null;
  by_class: NewsImpactBucket[];
}

export interface NewsImpactResponse {
  records: NewsImpactRecord[];
  summary: NewsImpactSummary;
  updatedAt?: string | null;
  enabled?: boolean;
  aiEnabled?: boolean;
  aiDailyCap?: number;
  horizonMinutes?: number;
  advisoryOnly?: boolean;
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
  /** Maximum real (non-simulated) signals per calendar day when the quota is enabled. Default 1. */
  max_signals_per_day: number;
  /** Optional daily cap. Default false — the bot sends every setup that passes all gates. */
  daily_quota_enabled: boolean;
  /** Portfolio-level risk guardrails: max concurrent, daily risk, daily loss R, same-direction cooldown. */
  risk_guardrails_enabled?: boolean;
  /** Max live open signals (0 = unlimited). Default 2. */
  max_concurrent_signals?: number;
  /** Max total risk USD per day as % of account (0 = unlimited). Default 3.0. */
  max_daily_risk_percent?: number;
  /** Max realized loss per day in R (0 = unlimited). Default 2.0. */
  max_daily_loss_r?: number;
  /** Cooldown between same-direction signals in minutes (0 = off). Default 60. */
  same_direction_cooldown_minutes?: number;
  /** Block COUNTER-macro entries (backtest 2025 negative). Default true. */
  counter_macro_filter_enabled?: boolean;
  /** Enable bearish_rejection scenario. Backtest 2025 negative; default false. */
  bearish_rejection_enabled?: boolean;
  /** Minimum RSI H1 for BUY entries (0 = off). Default 30. */
  min_rsi_h1_for_buy?: number;
  /** Merge H4/H1/D1 S/R levels into scenarios. Default true. */
  htf_levels_enabled?: boolean;
  /** Cap structural SL at N × ATR from the broken level (0 = off). Default 1.0. */
  structural_sl_atr_cap?: number;
  /** Use Yahoo GC=F COMEX futures volume proxy. Default true. */
  yahoo_futures_volume_enabled?: boolean;
  /** Prefer the high-liquidity Vietnam session 13:00 – 23:00 (GMT+7). */
  session_filter_enabled: boolean;
  /** Send informational Telegram briefings before London/New York opens (Mon–Fri, DST-aware). */
  session_alerts_enabled: boolean;
  /** Minutes before session open to send the briefing (15–120). */
  session_alert_lead_minutes: number;
  /** Active free market data source. */
  data_source?: 'BINANCE_PAXG' | 'TWELVEDATA' | 'OANDA' | 'EXNESS';
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
  /** AI review enabled. Default ON for new configs; persisted false stays false. */
  ai_signal_review_enabled?: boolean;
  /** DECISION = AI chooses BUY/SELL/STANDBY; VETO_ONLY = legacy veto-only review. Default DECISION. */
  ai_review_mode?: 'DECISION' | 'VETO_ONLY';
  /** Minimum AI confidence (0.5–1.0) required to let a signal through. Default 0.75. */
  ai_signal_review_min_confidence?: number;
  /** VETO_ONLY only: true = pass on AI error (fail-open, default), false = block. */
  ai_signal_review_fail_open?: boolean;
  /** Allow AI to approve a strong scenario before its M15 trigger confirms. Default true. */
  early_entry_enabled?: boolean;
  /** Minimum scenario confidence (50–100) for early approval. Default 90. */
  early_entry_min_confidence?: number;
  /** Max distance from live price to trigger for early entry, in R (0 = off). Default 1.0. */
  early_entry_max_distance_r?: number;
  /** Enable weighted confluence v2 (30/25/20/15/10). Default false until A/B. */
  quantum_weighted_score_enabled?: boolean;
  /** Trigger confirmation: volume + directional close/pinbar/engulfing/RSI cross. Default true. */
  trigger_confirmation_enabled?: boolean;
  /** Volume ratio used by the trigger confirmation score. Default 1.2. */
  quantum_volume_confirm_ratio?: number;
  /** Block breakout/breakdown entries after an exhausted daily move. Default true. */
  anti_chase_filter_enabled?: boolean;
  /** Daily change percent that activates anti-chase. Default -2.0. */
  anti_chase_daily_change_pct?: number;
  /** RSI H1 extreme that activates anti-chase. Default 25.0. */
  anti_chase_rsi_h1?: number;
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
  /** Telegram alert when the PRIMARY scenario trigger is hit on a closed M15 candle. Default true. */
  action_trigger_alert_enabled?: boolean;
  /** Shadow retest research: journal/Telegram only, never a signal or quota. Default true. */
  retest_shadow_enabled?: boolean;
  /** Retest scalp research: alert-only, default false. Never a signal or quota. */
  retest_scalp_enabled?: boolean;
  /** Max structural SL distance for the retest scalp profile (USD, default 20). */
  retest_scalp_max_sl_usd?: number;
  /** TP1 distance from entry for the retest scalp profile (USD, default 20). */
  retest_scalp_tp1_usd?: number;
  /** TP2 distance from entry for the retest scalp profile (USD, default 40). */
  retest_scalp_tp2_usd?: number;
  /** Suggested risk percentage for the retest scalp lot (default 0.5). */
  retest_scalp_risk_percentage?: number;
  /** Strategy v2 shadow: raw trigger + 1x H4 ATR SL, journal-only. Default false. */
  strategy_v2_shadow_enabled?: boolean;
  /** Session shadow: confirmed scenario in all sessions, journal-only. Default false. */
  session_shadow_enabled?: boolean;
  /** Minimum minutes between alerts for the same scenario + direction + zone (5–1440, default 120). */
  action_trigger_cooldown_minutes?: number;
  /** Max distance from price to trigger for SETUP WATCHING, in H4 ATR. Default 1.0. */
  action_watch_distance_atr?: number;
  /** Absolute distance cap (points) for SETUP WATCHING. Default 15. */
  action_watch_max_distance_points?: number;
  /** Minimum PRIMARY confidence for SETUP WATCHING. Default 60. */
  action_watch_min_confidence?: number;
  /** Append-only trigger journal for raw trigger efficacy. Default true. */
  trigger_journal_enabled?: boolean;
  /** Send MARKET VIEW to Telegram on H1 close during the session. Default true. */
  market_view_telegram_enabled?: boolean;
  /** Enable the AI Market Strategist for the H1 Market View. Default false (A/B). */
  market_strategist_enabled?: boolean;
  /** Minimum minutes between MARKET VIEW Telegram messages (15–1440, default 120). */
  market_view_cooldown_minutes?: number;
  /** Spread proxy threshold (|live − M15 close| / H4 ATR) for NO_TRADE. Default 1.0. */
  max_spread_proxy_atr?: number;
  /** Read-only Macro & Flow dashboard (FRED/Yahoo/CFTC). Default true; never affects signals. */
  macro_flow_enabled?: boolean;
  /** Send the read-only 07:00 daily plan to Telegram. Default true; no quota impact. */
  daily_plan_enabled?: boolean;
  /** Send the weekend brief (news + next-week assessment) at 07:00 Sat/Sun. Default true. */
  weekend_brief_enabled?: boolean;
  /** News impact/reaction journal (advisory, measurement-only). Default true. */
  news_reaction_enabled?: boolean;
  /** AI News Impact Analyst (post-actual only, capped). Default true. */
  news_ai_analyst_enabled?: boolean;
  /** Maximum distinct High-Impact events analyzed by AI per UTC day (0 = off). Default 6. */
  news_ai_daily_cap?: number;
  /** Reaction measurement/status horizon in minutes (60–240, default 240). */
  news_reaction_horizon_minutes?: number;
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
