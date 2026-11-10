import 'dotenv/config';
import express, { Request, Response } from 'express';
import { createServer as createViteServer } from 'vite';
import path from 'path';
import fs from 'fs';

/* ────────────────────────────────────────────────────────────────────────────
 * Domain types (mirrors rust-backend/src/models.rs)
 * ──────────────────────────────────────────────────────────────────────────── */

type MacroBias = 'BULLISH' | 'BEARISH' | 'NEUTRAL' | 'AUTO';
type TrendBias = 'BULLISH' | 'BEARISH' | 'NEUTRAL';
type DataSource = 'BINANCE_PAXG' | 'TWELVEDATA' | 'FALLBACK_SEED';

interface Candle {
  timestamp: number;
  open: number;
  high: number;
  low: number;
  close: number;
  volume: number;
  /** Binance kline field 9 (taker buy base volume); absent for TwelveData. */
  taker_buy_volume?: number;
  timeframe: string;
}

interface SupportResistanceZone {
  level_type: 'Support' | 'Resistance';
  price: number;
  min_price: number;
  max_price: number;
  touch_count: number;
  strength_score: number;
}

interface Signal {
  id: string;
  symbol: string;
  signal_type: 'BUY' | 'SELL';
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
  status: 'Active' | 'Target1Hit' | 'Target2Hit' | 'StopLossHit' | 'Cancelled';
  created_at: string;
  telegram_message_id?: number | null;
  recommended_lot_size?: number;
  risk_amount_usd?: number;
  potential_profit_tp1_usd?: number;
  potential_profit_tp2_usd?: number;
  dedup_key?: string;
  candle_timestamp?: number;
  is_simulated?: boolean;
  confluence_score?: number;
  entry_type?: 'MARKET' | 'LIMIT' | 'BREAKOUT';
  candle_close_price?: number;
  live_market_price?: number | null;
  retest_limit_price?: number | null;
  data_source?: string;
  is_entry_filled?: boolean;
  delta_ratio?: number | null;
  cvd?: number | null;
  delta?: number | null;
  rsi_value?: number | null;
  rsi_state?: string | null;
  rsi_divergence?: string | null;
}

interface EconomicEvent {
  id: string;
  title: string;
  currency: string;
  impact: 'High' | 'Medium' | 'Low';
  scheduled_time: string;
  forecast?: string;
  previous?: string;
  actual?: string;
}

interface BotConfig {
  symbol: string;
  timeframe: string;
  min_risk_reward: number;
  gold_sl_buffer_points: number;
  volume_surge_multiplier: number;
  rejection_wick_ratio: number;
  lookback_period: number;
  volume_ma_period: number;
  risk_percentage: number;
  account_balance: number;
  account_leverage: number;
  telegram_bot_token: string;
  telegram_chat_id: string;
  is_active: boolean;
  kill_switch_enabled: boolean;
  pre_news_buffer_minutes: number;
  post_news_buffer_minutes: number;
  macro_bias: MacroBias;
  order_block_filter: boolean;
  max_signals_per_day: number;
  session_filter_enabled: boolean;
  session_alerts_enabled: boolean;
  session_alert_lead_minutes: number;
  data_source: 'BINANCE_PAXG' | 'TWELVEDATA';
  webhook_passphrase: string;
  entry_mode: 'MARKET' | 'LIMIT_RETEST' | 'BREAKOUT';
  broker_price_offset: number;
  use_auto_spot_basis: boolean;
  max_entry_drift_points: number;
  max_candle_age_secs: number;
  htf_h4_filter_enabled: boolean;
  htf_d1_filter_enabled: boolean;
  /** Optional AI Chief Risk Officer veto gate. Default false — no behavior change. */
  ai_signal_review_enabled: boolean;
  /** Minimum AI confidence (0.5–1.0) to allow a real signal. Default 0.75. */
  ai_signal_review_min_confidence: number;
  /** AI error/timeout policy: true = pass (fail-open), false = block. Default true. */
  ai_signal_review_fail_open: boolean;
  /** Optional Volume Delta/CVD gate. Default false — no behavior change. */
  volume_delta_filter_enabled: boolean;
  volume_delta_mode: 'ALIGN' | 'DIVERGENCE' | 'BOTH';
  volume_delta_lookback: number;
  volume_delta_min_ratio: number;
  /** Optional hard RSI gate. Default false — no behavior change. */
  rsi_filter_enabled: boolean;
  rsi_period: number;
  rsi_overbought: number;
  rsi_oversold: number;
  rsi_divergence_enabled: boolean;
  rsi_divergence_lookback: number;
  rsi_pivot_k: number;
  rsi_mode: 'EXTREME' | 'DIVERGENCE' | 'BOTH';
}

interface DailyMacroReport {
  date: string;
  macro_bias: TrendBias;
  confidence_score: number;
  gold_range_forecast: string;
  daily_rationale: string;
  high_impact_events_today: string[];
  recommended_action: string;
  updated_at: string;
}

interface PersistedState {
  config?: Partial<BotConfig>;
  signals?: Signal[];
  economic_events?: EconomicEvent[];
  daily_macro_report?: DailyMacroReport | null;
  active_data_source?: DataSource;
  updated_at?: string;
}

interface PositionSizingResult {
  lotSize: number;
  riskAmountUsd: number;
  potentialProfitTp1Usd: number;
  potentialProfitTp2Usd: number;
  estimatedMarginUsd: number;
  riskAssessment: string;
}

/* ────────────────────────────────────────────────────────────────────────────
 * Constants, helpers & persistent state location
 * ──────────────────────────────────────────────────────────────────────────── */

const BINANCE_KLINES_URL = 'https://api.binance.com/api/v3/klines';
const FOREXFACTORY_CALENDAR_URL = 'https://nfs.faireconomy.media/ff_calendar_thisweek.json';
const DATA_DIR = path.resolve(process.cwd(), 'data');
const STATE_FILE = path.join(DATA_DIR, 'bot_state.json');
const VN_OFFSET_MS = 7 * 60 * 60 * 1000;
const MAX_SIGNAL_HISTORY = 500;

function numeric(value: unknown, fallback: number): number {
  const n = Number(value);
  return Number.isFinite(n) ? n : fallback;
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

function clampInt(value: unknown, fallback: number, min: number, max: number): number {
  return Math.round(clamp(numeric(value, fallback), min, max));
}

function booleanOr(value: unknown, fallback: boolean): boolean {
  return typeof value === 'boolean' ? value : fallback;
}

/** Calendar day key in the Vietnam trading timezone (UTC+7). */
function vnDateKey(input: Date | string | number = new Date()): string {
  const date = input instanceof Date ? input : new Date(input);
  if (Number.isNaN(date.getTime())) return '';
  return new Date(date.getTime() + VN_OFFSET_MS).toISOString().slice(0, 10);
}

function formatVnTime(input: string): string {
  const date = new Date(input);
  if (Number.isNaN(date.getTime())) return '--:--';
  const vn = new Date(date.getTime() + VN_OFFSET_MS);
  return `${String(vn.getUTCHours()).padStart(2, '0')}:${String(vn.getUTCMinutes()).padStart(2, '0')}`;
}

/** Active high-liquidity session 13:00 – 23:00 Vietnam time (GMT+7). */
function isWithinActiveSession(now: Date = new Date()): boolean {
  const vn = new Date(now.getTime() + VN_OFFSET_MS);
  const hour = vn.getUTCHours();
  return hour >= 13 && hour < 23;
}

function buildDedupKey(symbol: string, timeframe: string, candleTimestamp: number): string {
  return `${symbol}_${timeframe}_${candleTimestamp}`;
}

function dataSourceLabel(source: DataSource): string {
  switch (source) {
    case 'BINANCE_PAXG':
      return 'Binance PAXG/USDT (Free 24/7)';
    case 'TWELVEDATA':
      return 'TwelveData XAU/USD (Free 800/day)';
    default:
      return 'Offline Fallback (Seed)';
  }
}

/* ────────────────────────────────────────────────────────────────────────────
 * Default configuration, fallback seeds & in-memory state
 * ──────────────────────────────────────────────────────────────────────────── */

const DEFAULT_CONFIG: BotConfig = {
  symbol: 'XAU/USD',
  timeframe: 'M15',
  min_risk_reward: 2.0,
  gold_sl_buffer_points: 10.0,
  volume_surge_multiplier: 1.5,
  rejection_wick_ratio: 2.0,
  lookback_period: 25,
  volume_ma_period: 20,
  risk_percentage: 1.0,
  account_balance: 10000,
  account_leverage: 100,
  telegram_bot_token: process.env.TELEGRAM_BOT_TOKEN || '',
  telegram_chat_id: process.env.TELEGRAM_CHAT_ID || '',
  is_active: true,
  kill_switch_enabled: true,
  pre_news_buffer_minutes: 30,
  post_news_buffer_minutes: 30,
  macro_bias: 'AUTO',
  order_block_filter: true,
  max_signals_per_day: 1,
  session_filter_enabled: true,
  session_alerts_enabled: true,
  session_alert_lead_minutes: 60,
  data_source: 'BINANCE_PAXG',
  webhook_passphrase: process.env.WEBHOOK_PASSPHRASE || '',
  entry_mode: 'MARKET',
  broker_price_offset: 0,
  use_auto_spot_basis: true,
  max_entry_drift_points: 2.5,
  max_candle_age_secs: 120,
  htf_h4_filter_enabled: false,
  htf_d1_filter_enabled: false,
  ai_signal_review_enabled: false,
  ai_signal_review_min_confidence: 0.75,
  ai_signal_review_fail_open: true,
  volume_delta_filter_enabled: false,
  volume_delta_mode: 'BOTH',
  volume_delta_lookback: 20,
  volume_delta_min_ratio: 0.1,
  rsi_filter_enabled: false,
  rsi_period: 14,
  rsi_overbought: 70,
  rsi_oversold: 30,
  rsi_divergence_enabled: true,
  rsi_divergence_lookback: 40,
  rsi_pivot_k: 2,
  rsi_mode: 'BOTH',
};

function normalizeConfig(input: Partial<BotConfig>): BotConfig {
  const base = DEFAULT_CONFIG;
  const bias: MacroBias =
    input.macro_bias === 'BULLISH' ||
    input.macro_bias === 'BEARISH' ||
    input.macro_bias === 'NEUTRAL' ||
    input.macro_bias === 'AUTO'
      ? input.macro_bias
      : base.macro_bias;

  const timeframe =
    input.timeframe === 'H1' || input.timeframe === 'H4' || input.timeframe === 'M15'
      ? input.timeframe
      : base.timeframe;

  const entryMode: BotConfig['entry_mode'] =
    input.entry_mode === 'LIMIT_RETEST' ||
    input.entry_mode === 'BREAKOUT' ||
    input.entry_mode === 'MARKET'
      ? input.entry_mode
      : base.entry_mode;

  const rawVolumeDeltaMode =
    typeof input.volume_delta_mode === 'string'
      ? input.volume_delta_mode.trim().toUpperCase()
      : '';
  const volumeDeltaMode: BotConfig['volume_delta_mode'] =
    rawVolumeDeltaMode === 'ALIGN' ||
    rawVolumeDeltaMode === 'DIVERGENCE' ||
    rawVolumeDeltaMode === 'BOTH'
      ? (rawVolumeDeltaMode as BotConfig['volume_delta_mode'])
      : base.volume_delta_mode;

  const rawRsiMode =
    typeof input.rsi_mode === 'string' ? input.rsi_mode.trim().toUpperCase() : '';
  const rsiMode: BotConfig['rsi_mode'] =
    rawRsiMode === 'EXTREME' || rawRsiMode === 'DIVERGENCE' || rawRsiMode === 'BOTH'
      ? (rawRsiMode as BotConfig['rsi_mode'])
      : base.rsi_mode;

  const rsiOverbought = clamp(
    numeric(input.rsi_overbought, base.rsi_overbought),
    50,
    100
  );
  const rsiOversold = clamp(numeric(input.rsi_oversold, base.rsi_oversold), 0, 50);
  const [normalizedOverbought, normalizedOversold] =
    rsiOversold >= rsiOverbought ? [70, 30] : [rsiOverbought, rsiOversold];

  return {
    symbol: typeof input.symbol === 'string' && input.symbol.trim() ? input.symbol.trim() : base.symbol,
    timeframe,
    min_risk_reward: clamp(numeric(input.min_risk_reward, base.min_risk_reward), 1.5, 5.0),
    gold_sl_buffer_points: clamp(
      numeric(input.gold_sl_buffer_points, base.gold_sl_buffer_points),
      5.0,
      25.0
    ),
    volume_surge_multiplier: clamp(
      numeric(input.volume_surge_multiplier, base.volume_surge_multiplier),
      1.1,
      3.5
    ),
    rejection_wick_ratio: clamp(
      numeric(input.rejection_wick_ratio, base.rejection_wick_ratio),
      1.5,
      4.0
    ),
    lookback_period: clampInt(input.lookback_period, base.lookback_period, 15, 60),
    volume_ma_period: clampInt(input.volume_ma_period, base.volume_ma_period, 10, 50),
    risk_percentage: clamp(numeric(input.risk_percentage, base.risk_percentage), 0.2, 5.0),
    account_balance: clamp(numeric(input.account_balance, base.account_balance), 100, 1_000_000_000),
    account_leverage: clamp(numeric(input.account_leverage, base.account_leverage), 1, 1000),
    telegram_bot_token:
      typeof input.telegram_bot_token === 'string'
        ? input.telegram_bot_token
        : process.env.TELEGRAM_BOT_TOKEN || '',
    telegram_chat_id:
      typeof input.telegram_chat_id === 'string'
        ? input.telegram_chat_id
        : process.env.TELEGRAM_CHAT_ID || '',
    is_active: booleanOr(input.is_active, base.is_active),
    kill_switch_enabled: booleanOr(input.kill_switch_enabled, base.kill_switch_enabled),
    pre_news_buffer_minutes: clampInt(
      input.pre_news_buffer_minutes,
      base.pre_news_buffer_minutes,
      5,
      180
    ),
    post_news_buffer_minutes: clampInt(
      input.post_news_buffer_minutes,
      base.post_news_buffer_minutes,
      5,
      180
    ),
    macro_bias: bias,
    order_block_filter: booleanOr(input.order_block_filter, base.order_block_filter),
    max_signals_per_day: clampInt(input.max_signals_per_day, base.max_signals_per_day, 1, 5),
    session_filter_enabled: booleanOr(
      input.session_filter_enabled,
      base.session_filter_enabled
    ),
    session_alerts_enabled: booleanOr(
      input.session_alerts_enabled,
      base.session_alerts_enabled
    ),
    session_alert_lead_minutes: clampInt(
      input.session_alert_lead_minutes,
      base.session_alert_lead_minutes,
      15,
      120
    ),
    data_source: input.data_source === 'TWELVEDATA' ? 'TWELVEDATA' : 'BINANCE_PAXG',
    webhook_passphrase:
      typeof input.webhook_passphrase === 'string'
        ? input.webhook_passphrase
        : process.env.WEBHOOK_PASSPHRASE || '',
    entry_mode: entryMode,
    broker_price_offset: clamp(
      numeric(input.broker_price_offset, base.broker_price_offset),
      -100,
      100
    ),
    use_auto_spot_basis: booleanOr(
      input.use_auto_spot_basis,
      base.use_auto_spot_basis
    ),
    max_entry_drift_points: clamp(
      numeric(input.max_entry_drift_points, base.max_entry_drift_points),
      0.5,
      10
    ),
    max_candle_age_secs: clampInt(
      input.max_candle_age_secs,
      base.max_candle_age_secs,
      30,
      900
    ),
    htf_h4_filter_enabled: booleanOr(
      input.htf_h4_filter_enabled,
      base.htf_h4_filter_enabled
    ),
    htf_d1_filter_enabled: booleanOr(
      input.htf_d1_filter_enabled,
      base.htf_d1_filter_enabled
    ),
    ai_signal_review_enabled: booleanOr(
      input.ai_signal_review_enabled,
      base.ai_signal_review_enabled
    ),
    ai_signal_review_min_confidence: clamp(
      numeric(
        input.ai_signal_review_min_confidence,
        base.ai_signal_review_min_confidence
      ),
      0.5,
      1.0
    ),
    ai_signal_review_fail_open: booleanOr(
      input.ai_signal_review_fail_open,
      base.ai_signal_review_fail_open
    ),
    volume_delta_filter_enabled: booleanOr(
      input.volume_delta_filter_enabled,
      base.volume_delta_filter_enabled
    ),
    volume_delta_mode: volumeDeltaMode,
    volume_delta_lookback: clampInt(
      input.volume_delta_lookback,
      base.volume_delta_lookback,
      5,
      100
    ),
    volume_delta_min_ratio: clamp(
      numeric(input.volume_delta_min_ratio, base.volume_delta_min_ratio),
      0,
      1
    ),
    rsi_filter_enabled: booleanOr(
      input.rsi_filter_enabled,
      base.rsi_filter_enabled
    ),
    rsi_period: clampInt(input.rsi_period, base.rsi_period, 2, 100),
    rsi_overbought: normalizedOverbought,
    rsi_oversold: normalizedOversold,
    rsi_divergence_enabled: booleanOr(
      input.rsi_divergence_enabled,
      base.rsi_divergence_enabled
    ),
    rsi_divergence_lookback: clampInt(
      input.rsi_divergence_lookback,
      base.rsi_divergence_lookback,
      10,
      200
    ),
    rsi_pivot_k: clampInt(input.rsi_pivot_k, base.rsi_pivot_k, 1, 10),
    rsi_mode: rsiMode,
  };
}

function createFallbackEconomicEvents(): EconomicEvent[] {
  const now = Date.now();
  return [
    {
      id: 'EVT-CPI-01',
      title: 'US Core CPI m/m (Lạm phát lõi)',
      currency: 'USD',
      impact: 'High',
      scheduled_time: new Date(now + 2 * 3600 * 1000).toISOString(),
      forecast: '0.3%',
      previous: '0.3%',
    },
    {
      id: 'EVT-NFP-02',
      title: 'US Non-Farm Payrolls (Bảng lương phi nông nghiệp)',
      currency: 'USD',
      impact: 'High',
      scheduled_time: new Date(now + 26 * 3600 * 1000).toISOString(),
      forecast: '175K',
      previous: '216K',
    },
    {
      id: 'EVT-FOMC-03',
      title: 'FOMC Rate Decision (Quyết định Lãi suất FED)',
      currency: 'USD',
      impact: 'High',
      scheduled_time: new Date(now + 72 * 3600 * 1000).toISOString(),
      forecast: '5.25%',
      previous: '5.50%',
    },
    {
      id: 'EVT-PMI-04',
      title: 'US ISM Manufacturing PMI',
      currency: 'USD',
      impact: 'Medium',
      scheduled_time: new Date(now - 3 * 3600 * 1000).toISOString(),
      forecast: '49.2',
      previous: '48.5',
      actual: '49.8',
    },
  ];
}

/** Offline-only seed candles. Never used to generate live signals. */
function generateGoldCandles(count = 100): Candle[] {
  const candles: Candle[] = [];
  let price = 2682.4;
  const now = Date.now();
  const stepMs = 15 * 60 * 1000;

  for (let i = 0; i < count; i++) {
    const ts = now - (count - i) * stepMs;
    const noise = Math.sin(i * 0.4) * 3.5 + ((i % 5) - 2) * 1.1;
    const open = price;
    let close = Math.round((open + noise) * 100) / 100;
    if (close < 2650) close = 2655;
    if (close > 2720) close = 2715;

    let high = Math.round((Math.max(open, close) + 1.2 + (i % 4) * 0.7) * 100) / 100;
    let low = Math.round((Math.min(open, close) - 1.1 - (i % 3) * 0.8) * 100) / 100;
    const baseVol = 1250 + Math.round(Math.abs(Math.cos(i * 0.3) * 800));
    const volume = i === count - 1 ? Math.round(baseVol * 1.85) : baseVol;

    if (i === count - 3) low = 2671.2;
    if (i === count - 1) {
      low = 2670.5;
      close = 2684.5;
    }

    candles.push({
      timestamp: ts,
      open,
      high,
      low,
      close,
      volume,
      timeframe: 'M15',
    });

    price = close;
  }
  return candles;
}

function createSeedSignals(): Signal[] {
  const now = Date.now();
  const firstTs = now - 26 * 3600 * 1000;
  const secondTs = now - 31 * 3600 * 1000;
  const firstCandleTs = Math.floor(firstTs / (15 * 60 * 1000)) * 15 * 60 * 1000;
  const secondCandleTs = Math.floor(secondTs / (15 * 60 * 1000)) * 15 * 60 * 1000;

  return [
    {
      id: 'SIG-1741520000000-BUY',
      symbol: 'XAU/USD',
      signal_type: 'BUY',
      timeframe: 'M15',
      entry_price: 2674.5,
      stop_loss: 2664.0,
      take_profit_1: 2695.5,
      take_profit_2: 2706.0,
      risk_points: 10.5,
      reward_points_tp1: 21.0,
      reward_points_tp2: 31.5,
      risk_reward_ratio: 2.0,
      volume_surge_ratio: 1.85,
      recommended_lot_size: 0.09,
      risk_amount_usd: 94.5,
      potential_profit_tp1_usd: 189.0,
      potential_profit_tp2_usd: 283.5,
      price_action_notes:
        'Nến rút chân Bullish Pinbar phản ứng mạnh tại vùng Hỗ trợ động $2,670.00 (râu dưới chiếm 58% biên độ)',
      volume_analysis_notes:
        'Volume đạt 1.85x so với MA20, xác nhận Smart Money hấp thụ cản dưới',
      full_rationale:
        'Phe mua bảo vệ quyết liệt vùng hỗ trợ $2,670.00 sau khi test 3 lần. Volume đột biến xác nhận lực cầu đảo chiều. R:R đạt chuẩn 1:2.0.',
      status: 'Target1Hit',
      created_at: new Date(firstTs).toISOString(),
      telegram_message_id: 8812,
      dedup_key: buildDedupKey('XAU/USD', 'M15', firstCandleTs),
      candle_timestamp: firstCandleTs,
      is_simulated: false,
      confluence_score: 84,
    },
    {
      id: 'SIG-1741509000000-SELL',
      symbol: 'XAU/USD',
      signal_type: 'SELL',
      timeframe: 'M15',
      entry_price: 2712.8,
      stop_loss: 2722.5,
      take_profit_1: 2693.4,
      take_profit_2: 2683.7,
      risk_points: 9.7,
      reward_points_tp1: 19.4,
      reward_points_tp2: 29.1,
      risk_reward_ratio: 2.0,
      volume_surge_ratio: 2.15,
      recommended_lot_size: 0.1,
      risk_amount_usd: 97.0,
      potential_profit_tp1_usd: 194.0,
      potential_profit_tp2_usd: 291.0,
      price_action_notes:
        'Shooting Star Pinbar râu trên dài 62% từ chối mức Kháng cự tâm lý $2,715.00',
      volume_analysis_notes:
        'Volume tăng 2.15x MA20, lực bán chốt lời diện rộng áp đảo tại đỉnh cản',
      full_rationale:
        'Nến từ chối giá cao tại kháng cự đỉnh ngày. Khối lượng đột biến cho thấy phe bán áp đảo hoàn toàn. R:R đạt 1:2.0.',
      status: 'Target2Hit',
      created_at: new Date(secondTs).toISOString(),
      telegram_message_id: 8794,
      dedup_key: buildDedupKey('XAU/USD', 'M15', secondCandleTs),
      candle_timestamp: secondCandleTs,
      is_simulated: false,
      confluence_score: 78,
    },
  ];
}

let config: BotConfig = { ...DEFAULT_CONFIG };
let candlesBuffer: Candle[] = generateGoldCandles(100);
let signalsList: Signal[] = createSeedSignals();
let economicEvents: EconomicEvent[] = createFallbackEconomicEvents();
let activeDataSource: DataSource = 'BINANCE_PAXG';
let dailyMacroReport: DailyMacroReport = {
  date: vnDateKey(),
  macro_bias: 'NEUTRAL',
  confidence_score: 70,
  gold_range_forecast: '$2,660.00 - $2,710.00',
  daily_rationale:
    'Báo cáo khởi tạo: đang chờ dữ liệu nến Binance PAXG/USDT và lịch tin ForexFactory để tính EMA50/200.',
  high_impact_events_today: [],
  recommended_action:
    'Chờ đồng bộ dữ liệu thị trường. Bot vẫn tuân thủ khóa 1 lệnh/ngày và Kill Switch tin đỏ.',
  updated_at: new Date().toISOString(),
};

/* ────────────────────────────────────────────────────────────────────────────
 * Persistent storage — data/bot_state.json (atomic write)
 * ──────────────────────────────────────────────────────────────────────────── */

function saveState(): void {
  try {
    fs.mkdirSync(DATA_DIR, { recursive: true });
    const payload: PersistedState = {
      config,
      signals: signalsList.slice(0, MAX_SIGNAL_HISTORY),
      economic_events: economicEvents,
      daily_macro_report: dailyMacroReport,
      active_data_source: activeDataSource,
      updated_at: new Date().toISOString(),
    };
    const tmpFile = `${STATE_FILE}.tmp`;
    fs.writeFileSync(tmpFile, JSON.stringify(payload, null, 2), 'utf-8');
    fs.renameSync(tmpFile, STATE_FILE);
  } catch (err) {
    console.warn('[Storage] Failed to persist bot state:', err instanceof Error ? err.message : err);
  }
}

function loadPersistedState(): void {
  try {
    if (!fs.existsSync(STATE_FILE)) {
      console.log('[Storage] No previous bot_state.json — starting with defaults.');
      return;
    }
    const raw = fs.readFileSync(STATE_FILE, 'utf-8');
    const parsed = JSON.parse(raw) as PersistedState;

    if (parsed.config) {
      config = normalizeConfig({ ...config, ...parsed.config });
    }
    if (Array.isArray(parsed.signals) && parsed.signals.length > 0) {
      signalsList = parsed.signals.slice(0, MAX_SIGNAL_HISTORY);
    }
    if (Array.isArray(parsed.economic_events) && parsed.economic_events.length > 0) {
      economicEvents = parsed.economic_events;
    }
    if (parsed.daily_macro_report) {
      dailyMacroReport = parsed.daily_macro_report;
    }
    if (
      parsed.active_data_source === 'BINANCE_PAXG' ||
      parsed.active_data_source === 'TWELVEDATA' ||
      parsed.active_data_source === 'FALLBACK_SEED'
    ) {
      activeDataSource = parsed.active_data_source;
    }

    console.log(
      `[Storage] Loaded ${signalsList.length} signals, ${economicEvents.length} events from ${STATE_FILE}`
    );
  } catch (err) {
    console.warn('[Storage] Failed to load bot state:', err instanceof Error ? err.message : err);
  }
}

/* ────────────────────────────────────────────────────────────────────────────
 * Free live market data — Binance PAXG/USDT & ForexFactory JSON
 * ──────────────────────────────────────────────────────────────────────────── */

async function fetchJson<T>(url: string, timeoutMs = 9000): Promise<T> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const res = await fetch(url, {
      signal: controller.signal,
      headers: {
        Accept: 'application/json',
        'User-Agent': 'GoldSignalBot/1.2 (+free-data)',
      },
    });
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    return (await res.json()) as T;
  } finally {
    clearTimeout(timer);
  }
}

async function fetchBinanceCandles(timeframe: string): Promise<Candle[]> {
  const interval = timeframe === 'H1' ? '1h' : timeframe === 'H4' ? '4h' : '15m';
  const url = `${BINANCE_KLINES_URL}?symbol=PAXGUSDT&interval=${interval}&limit=100`;
  const rows = await fetchJson<unknown[][]>(url);
  const now = Date.now();

  const candles: Candle[] = [];
  for (const row of rows) {
    if (!Array.isArray(row) || row.length < 7) continue;
    const closeTime = Number(row[6]);
    // Anti-repaint: keep only fully closed candles.
    if (!Number.isFinite(closeTime) || closeTime > now) continue;

    const candle: Candle = {
      timestamp: Number(row[0]),
      open: Number.parseFloat(String(row[1])),
      high: Number.parseFloat(String(row[2])),
      low: Number.parseFloat(String(row[3])),
      close: Number.parseFloat(String(row[4])),
      volume: Number.parseFloat(String(row[5])),
      taker_buy_volume: Number.parseFloat(String(row[9] ?? 0)),
      timeframe,
    };

    if (
      Number.isFinite(candle.timestamp) &&
      Number.isFinite(candle.open) &&
      Number.isFinite(candle.high) &&
      Number.isFinite(candle.low) &&
      Number.isFinite(candle.close)
    ) {
      candles.push(candle);
    }
  }

  return candles.slice(-100);
}

async function fetchTwelveDataCandles(timeframe: string): Promise<Candle[]> {
  const apiKey = process.env.TWELVEDATA_API_KEY;
  if (!apiKey) throw new Error('TWELVEDATA_API_KEY is not configured');

  const interval = timeframe === 'H1' ? '1h' : timeframe === 'H4' ? '4h' : '15min';
  const url = `https://api.twelvedata.com/time_series?symbol=XAU/USD&interval=${interval}&outputsize=100&apikey=${encodeURIComponent(apiKey)}`;
  const payload = await fetchJson<{
    status?: string;
    message?: string;
    values?: Array<{
      datetime: string;
      open: string;
      high: string;
      low: string;
      close: string;
      volume?: string;
    }>;
  }>(url);

  if (payload.status === 'error' || !Array.isArray(payload.values)) {
    throw new Error(payload.message || 'Invalid TwelveData response');
  }

  // TwelveData returns newest first and includes the still-forming candle.
  const values = [...payload.values].reverse();
  values.pop();

  const candles: Candle[] = [];
  for (const value of values) {
    const iso = value.datetime.includes('T')
      ? value.datetime
      : `${value.datetime.replace(' ', 'T')}Z`;
    const timestamp = Date.parse(iso);
    if (!Number.isFinite(timestamp)) continue;

    const candle: Candle = {
      timestamp,
      open: Number.parseFloat(value.open),
      high: Number.parseFloat(value.high),
      low: Number.parseFloat(value.low),
      close: Number.parseFloat(value.close),
      volume: Number.parseFloat(value.volume || '0'),
      timeframe,
    };
    if (
      Number.isFinite(candle.open) &&
      Number.isFinite(candle.high) &&
      Number.isFinite(candle.low) &&
      Number.isFinite(candle.close)
    ) {
      candles.push(candle);
    }
  }

  return candles.slice(-100);
}

async function refreshMarketCandles(): Promise<{ success: boolean; source: DataSource }> {
  const preferred: DataSource =
    config.data_source === 'TWELVEDATA' && process.env.TWELVEDATA_API_KEY
      ? 'TWELVEDATA'
      : 'BINANCE_PAXG';
  const attempts: Array<'BINANCE_PAXG' | 'TWELVEDATA'> =
    preferred === 'TWELVEDATA' ? ['TWELVEDATA', 'BINANCE_PAXG'] : ['BINANCE_PAXG'];

  for (const source of attempts) {
    try {
      const live =
        source === 'TWELVEDATA'
          ? await fetchTwelveDataCandles(config.timeframe)
          : await fetchBinanceCandles(config.timeframe);

      if (live.length >= 20) {
        candlesBuffer = live;
        activeDataSource = source;
        return { success: true, source };
      }
      console.warn(`[MarketFeed] ${source} returned only ${live.length} closed candles.`);
    } catch (err) {
      console.warn(
        `[MarketFeed] ${source} unavailable:`,
        err instanceof Error ? err.message : err
      );
    }
  }

  activeDataSource = 'FALLBACK_SEED';
  return { success: false, source: activeDataSource };
}

async function fetchForexFactoryEvents(): Promise<EconomicEvent[] | null> {
  try {
    const raw = await fetchJson<
      Array<{
        title?: string;
        country?: string;
        currency?: string;
        date?: string;
        impact?: string;
        forecast?: string;
        previous?: string;
        actual?: string;
      }>
    >(FOREXFACTORY_CALENDAR_URL);

    if (!Array.isArray(raw)) throw new Error('Unexpected ForexFactory payload');

    const events: EconomicEvent[] = [];
    for (let index = 0; index < raw.length; index++) {
      const item = raw[index];
      const currency = String(item.country || item.currency || '').toUpperCase();
      if (currency !== 'USD') continue;

      const impactRaw = String(item.impact || '').toLowerCase();
      const impact: EconomicEvent['impact'] | null =
        impactRaw === 'high' ? 'High' : impactRaw === 'medium' ? 'Medium' : null;
      if (!impact) continue;

      const scheduled = item.date ? new Date(item.date) : null;
      if (!scheduled || Number.isNaN(scheduled.getTime())) continue;

      events.push({
        id: `FF-${index}-${scheduled.getTime()}`,
        title: item.title || 'US Economic Event',
        currency: 'USD',
        impact,
        scheduled_time: scheduled.toISOString(),
        forecast: item.forecast || '',
        previous: item.previous || '',
        actual: item.actual || '',
      });
    }

    events.sort(
      (a, b) => new Date(a.scheduled_time).getTime() - new Date(b.scheduled_time).getTime()
    );

    if (events.length === 0) throw new Error('No USD High/Medium events found');
    return events;
  } catch (err) {
    console.warn(
      '[NewsFeed] ForexFactory unavailable, keeping fallback calendar:',
      err instanceof Error ? err.message : err
    );
    return null;
  }
}

/* ────────────────────────────────────────────────────────────────────────────
 * Indicators & quantitative analyzer (mirrors rust-backend/src/analyzer.rs)
 * ──────────────────────────────────────────────────────────────────────────── */

function calculateEMA(candles: Candle[], period: number): number {
  if (candles.length === 0) return 0;
  const effectivePeriod = Math.min(period, candles.length);
  const multiplier = 2 / (effectivePeriod + 1);
  let ema = candles[0].close;
  for (let i = 1; i < candles.length; i++) {
    ema = candles[i].close * multiplier + ema * (1 - multiplier);
  }
  return Math.round(ema * 100) / 100;
}

function detectEmaTrendBias(candles: Candle[]): TrendBias {
  if (candles.length < 10) return 'NEUTRAL';
  const lastClose = candles[candles.length - 1].close;
  const emaNow = calculateEMA(candles, 50);
  const priorSlice = candles.slice(0, Math.max(2, candles.length - 3));
  const emaPrior = calculateEMA(priorSlice, 50);

  if (lastClose > emaNow && emaNow >= emaPrior) return 'BULLISH';
  if (lastClose < emaNow && emaNow <= emaPrior) return 'BEARISH';
  return 'NEUTRAL';
}

function resolveEffectiveBias(cfg: BotConfig, candles: Candle[]): TrendBias {
  if (cfg.macro_bias === 'AUTO') return detectEmaTrendBias(candles);
  return cfg.macro_bias;
}

function detectDynamicSR(candles: Candle[], lookback = 25, tolerance = 2.5) {
  const slice = candles.slice(Math.max(0, candles.length - lookback));
  const rawHighs: number[] = [];
  const rawLows: number[] = [];

  for (let i = 2; i < slice.length - 2; i++) {
    const c = slice[i];
    if (
      c.high > slice[i - 1].high &&
      c.high > slice[i - 2].high &&
      c.high >= slice[i + 1].high &&
      c.high >= slice[i + 2].high
    ) {
      rawHighs.push(c.high);
    }
    if (
      c.low < slice[i - 1].low &&
      c.low < slice[i - 2].low &&
      c.low <= slice[i + 1].low &&
      c.low <= slice[i + 2].low
    ) {
      rawLows.push(c.low);
    }
  }

  function cluster(prices: number[], type: 'Support' | 'Resistance'): SupportResistanceZone[] {
    if (prices.length === 0) return [];
    prices.sort((a, b) => a - b);
    const zones: SupportResistanceZone[] = [];
    let currentCluster = [prices[0]];

    for (let i = 1; i < prices.length; i++) {
      const avg = currentCluster.reduce((a, b) => a + b, 0) / currentCluster.length;
      if (Math.abs(prices[i] - avg) <= tolerance) {
        currentCluster.push(prices[i]);
      } else {
        const minP = Math.min(...currentCluster);
        const maxP = Math.max(...currentCluster);
        zones.push({
          level_type: type,
          price: Math.round(avg * 100) / 100,
          min_price: Math.round(minP * 100) / 100,
          max_price: Math.round(maxP * 100) / 100,
          touch_count: currentCluster.length,
          strength_score: Math.min(1.0, currentCluster.length * 0.25),
        });
        currentCluster = [prices[i]];
      }
    }

    if (currentCluster.length > 0) {
      const avg = currentCluster.reduce((a, b) => a + b, 0) / currentCluster.length;
      zones.push({
        level_type: type,
        price: Math.round(avg * 100) / 100,
        min_price: Math.round(Math.min(...currentCluster) * 100) / 100,
        max_price: Math.round(Math.max(...currentCluster) * 100) / 100,
        touch_count: currentCluster.length,
        strength_score: Math.min(1.0, currentCluster.length * 0.25),
      });
    }

    zones.sort((a, b) => b.touch_count - a.touch_count);
    return zones;
  }

  return {
    supports: cluster(rawLows, 'Support'),
    resistances: cluster(rawHighs, 'Resistance'),
  };
}

function calculateVolumeMA(candles: Candle[], period = 20): number {
  if (candles.length === 0) return 0;
  const take = Math.min(period, candles.length);
  const subset = candles.slice(candles.length - take);
  return subset.reduce((acc, c) => acc + c.volume, 0) / take;
}

/** Mirrors rust-backend/src/volume_delta.rs for the Express dev fallback. */
function computeVolumeDelta(candles: Candle[], lookback = 20) {
  const latest = candles[candles.length - 1];
  if (!latest) {
    return { buy_volume: 0, sell_volume: 0, delta: 0, delta_ratio: 0, cvd: 0, cvd_change: 0 };
  }

  const candleDelta = (candle: Candle) => {
    const buy = Math.min(candle.taker_buy_volume ?? 0, candle.volume);
    const sell = candle.volume - buy;
    return buy - sell;
  };

  const buy = Math.min(latest.taker_buy_volume ?? 0, latest.volume);
  const sell = latest.volume - buy;
  const delta = buy - sell;
  const deltaRatio = latest.volume > 0 ? delta / latest.volume : 0;

  const take = Math.min(Math.max(1, lookback), candles.length);
  const window = candles.slice(candles.length - take);
  const deltas = window.map(candleDelta);
  const cvd = deltas.reduce((acc, value) => acc + value, 0);
  const cvdChange = deltas.length > 1 ? cvd - deltas[0] : 0;

  return {
    buy_volume: buy,
    sell_volume: sell,
    delta,
    delta_ratio: deltaRatio,
    cvd,
    cvd_change: cvdChange,
  };
}

function checkPinbar(candle: Candle, wickRatio = 2.0) {
  const range = Math.max(0.001, candle.high - candle.low);
  const body = Math.abs(candle.close - candle.open);
  const lowerWick = Math.min(candle.open, candle.close) - candle.low;
  const upperWick = candle.high - Math.max(candle.open, candle.close);

  const isBullish =
    lowerWick >= body * wickRatio &&
    lowerWick / range >= 0.48 &&
    upperWick / range <= 0.28;

  const isBearish =
    upperWick >= body * wickRatio &&
    upperWick / range >= 0.48 &&
    lowerWick / range <= 0.28;

  return { isBullish, isBearish, lowerWick, upperWick, body, range };
}

function computeConfluenceScore(
  touchCount: number,
  volumeSurgeRatio: number,
  wickRatio: number,
  riskReward: number
): number {
  let score = 35;
  score += Math.min(25, Math.max(0, (volumeSurgeRatio - 1.0) * 25));
  score += Math.min(20, Math.max(0, touchCount * 5));
  score += Math.min(10, Math.max(0, (wickRatio - 1.5) * 8));
  score += Math.min(10, Math.max(0, (riskReward - 2.0) * 10));
  return Math.round(clamp(score, 0, 100));
}

function markSimulation(signal: Signal, isSimulated: boolean): Signal {
  if (isSimulated) {
    signal.is_simulated = true;
    signal.price_action_notes = `[SIMULATION - Không tốn Quota] ${signal.price_action_notes}`;
    signal.full_rationale = `[SIMULATION - Không tốn Quota] ${signal.full_rationale}`;
  }
  return signal;
}

function runAnalysis(
  candles: Candle[],
  cfg: BotConfig,
  options: { isSimulated?: boolean } = {}
): Signal | null {
  const isSimulated = options.isSimulated === true;
  if (candles.length < cfg.lookback_period + 5) return null;

  const currentCandle = candles[candles.length - 1];
  const previousCandles = candles.slice(0, candles.length - 1);

  // Tier 2: session filter & news kill switch (live signals only; simulations
  // are an explicit sandbox and must remain testable at any hour).
  if (!isSimulated && cfg.session_filter_enabled && !isWithinActiveSession()) {
    console.log('[Session Filter] Ngoài khung giờ vàng 13:00-23:00 VN — bỏ qua quét live.');
    return null;
  }
  if (!isSimulated && cfg.kill_switch_enabled) {
    const kill = getKillSwitchStatus();
    if (kill.active) {
      console.log(`[News Kill Switch] ${kill.reason}`);
      return null;
    }
  }

  const candleRange = Math.max(0.0001, currentCandle.high - currentCandle.low);
  if (candleRange < 1.2) {
    console.log('[Analyzer] Bỏ qua nến Doji có biên độ < 1.2$.');
    return null;
  }

  const volumeMA = calculateVolumeMA(previousCandles, cfg.volume_ma_period);
  const volumeSurgeRatio = volumeMA > 0 ? currentCandle.volume / volumeMA : 1.0;
  const isVolumeValid = volumeSurgeRatio >= cfg.volume_surge_multiplier;

  const { supports, resistances } = detectDynamicSR(
    previousCandles,
    cfg.lookback_period,
    2.5
  );
  const pinbar = checkPinbar(currentCandle, cfg.rejection_wick_ratio);
  const effectiveBias: TrendBias = isSimulated
    ? 'NEUTRAL'
    : resolveEffectiveBias(cfg, candles);
  const ema50 = calculateEMA(candles, 50);
  const dedupKey = buildDedupKey(cfg.symbol, cfg.timeframe, currentCandle.timestamp);
  const tolerance = 3.0;

  // BUY SETUP
  if (pinbar.isBullish && isVolumeValid) {
    if (effectiveBias === 'BEARISH') {
      console.log('[Trend Filter] Bỏ qua BUY vì EMA/macro bias đang BEARISH.');
      return null;
    }

    const support = supports.find(
      (zone) =>
        Math.abs(currentCandle.low - zone.price) <= tolerance ||
        Math.abs(currentCandle.low - zone.min_price) <= tolerance ||
        (currentCandle.low <= zone.max_price && currentCandle.close >= zone.min_price)
    );

    if (support) {
      const entryPrice = currentCandle.close;
      const baseSL = Math.min(currentCandle.low, support.min_price);
      const stopLoss = Math.round((baseSL - cfg.gold_sl_buffer_points) * 100) / 100;
      const riskPoints = Math.round((entryPrice - stopLoss) * 100) / 100;

      // Expanded SL ceiling: 45.0$ supports a 20–25$ buffer plus pinbar height.
      if (riskPoints >= 3.0 && riskPoints <= 45.0) {
        const rewardTP1 = Math.round(riskPoints * cfg.min_risk_reward * 100) / 100;
        const tp1 = Math.round((entryPrice + rewardTP1) * 100) / 100;
        const nextResistance = resistances.find((zone) => zone.price > tp1);
        const tp2 = nextResistance
          ? Math.round((nextResistance.price - 1.0) * 100) / 100
          : Math.round((entryPrice + riskPoints * 3.0) * 100) / 100;
        const rewardTP2 = Math.round((tp2 - entryPrice) * 100) / 100;
        const rr = Math.round((rewardTP1 / riskPoints) * 100) / 100;

        if (rr >= cfg.min_risk_reward) {
          const sizing = calculateLotSize(
            cfg.account_balance,
            cfg.risk_percentage,
            riskPoints,
            rewardTP1,
            rewardTP2,
            entryPrice,
            cfg.account_leverage
          );

          const wickRatio =
            pinbar.body > 0 ? pinbar.lowerWick / pinbar.body : pinbar.lowerWick;

          const signal: Signal = {
            id: `${isSimulated ? 'SIM' : 'SIG'}-${Date.now()}-BUY`,
            symbol: cfg.symbol,
            signal_type: 'BUY',
            timeframe: cfg.timeframe,
            entry_price: entryPrice,
            stop_loss: stopLoss,
            take_profit_1: tp1,
            take_profit_2: tp2,
            risk_points: riskPoints,
            reward_points_tp1: rewardTP1,
            reward_points_tp2: rewardTP2,
            risk_reward_ratio: rr,
            volume_surge_ratio: Math.round(volumeSurgeRatio * 100) / 100,
            recommended_lot_size: sizing.lotSize,
            risk_amount_usd: sizing.riskAmountUsd,
            potential_profit_tp1_usd: sizing.potentialProfitTp1Usd,
            potential_profit_tp2_usd: sizing.potentialProfitTp2Usd,
            price_action_notes: `Nến rút chân Bullish Pinbar (râu dưới ${pinbar.lowerWick.toFixed(1)}$ chiếm ${((pinbar.lowerWick / pinbar.range) * 100).toFixed(0)}% biên độ) phản ứng tại Hỗ trợ động $${support.price}`,
            volume_analysis_notes: `Volume đạt ${volumeSurgeRatio.toFixed(2)}x MA${cfg.volume_ma_period}, dòng tiền tổ chức hấp thụ lực bán tại đáy cản (Liquidity Absorption)`,
            full_rationale: `Nến từ chối giảm tạo râu dưới dài tại vùng Hỗ trợ $${support.price} (đã retest ${support.touch_count} lần). Khối lượng tăng vọt ${volumeSurgeRatio.toFixed(2)}x xác nhận phe mua bảo vệ vùng giá. Xu hướng ${effectiveBias} (EMA50 $${ema50}). Tỷ lệ R:R đạt 1:${rr}. Khối lượng đề xuất: ${sizing.lotSize} lot (Rủi ro: -$${sizing.riskAmountUsd}, TP1: +$${sizing.potentialProfitTp1Usd}).`,
            status: 'Active',
            created_at: new Date().toISOString(),
            dedup_key: dedupKey,
            candle_timestamp: currentCandle.timestamp,
            is_simulated: isSimulated,
            confluence_score: computeConfluenceScore(
              support.touch_count,
              volumeSurgeRatio,
              wickRatio,
              rr
            ),
          };

          return markSimulation(signal, isSimulated);
        }
      }
    }
  }

  // SELL SETUP
  if (pinbar.isBearish && isVolumeValid) {
    if (effectiveBias === 'BULLISH') {
      console.log('[Trend Filter] Bỏ qua SELL vì EMA/macro bias đang BULLISH.');
      return null;
    }

    const resistance = resistances.find(
      (zone) =>
        Math.abs(currentCandle.high - zone.price) <= tolerance ||
        Math.abs(currentCandle.high - zone.max_price) <= tolerance ||
        (currentCandle.high >= zone.min_price && currentCandle.close <= zone.max_price)
    );

    if (resistance) {
      const entryPrice = currentCandle.close;
      const baseSL = Math.max(currentCandle.high, resistance.max_price);
      const stopLoss = Math.round((baseSL + cfg.gold_sl_buffer_points) * 100) / 100;
      const riskPoints = Math.round((stopLoss - entryPrice) * 100) / 100;

      if (riskPoints >= 3.0 && riskPoints <= 45.0) {
        const rewardTP1 = Math.round(riskPoints * cfg.min_risk_reward * 100) / 100;
        const tp1 = Math.round((entryPrice - rewardTP1) * 100) / 100;
        const nextSupport = supports.find((zone) => zone.price < tp1);
        const tp2 = nextSupport
          ? Math.round((nextSupport.price + 1.0) * 100) / 100
          : Math.round((entryPrice - riskPoints * 3.0) * 100) / 100;
        const rewardTP2 = Math.round((entryPrice - tp2) * 100) / 100;
        const rr = Math.round((rewardTP1 / riskPoints) * 100) / 100;

        if (rr >= cfg.min_risk_reward) {
          const sizing = calculateLotSize(
            cfg.account_balance,
            cfg.risk_percentage,
            riskPoints,
            rewardTP1,
            rewardTP2,
            entryPrice,
            cfg.account_leverage
          );

          const wickRatio =
            pinbar.body > 0 ? pinbar.upperWick / pinbar.body : pinbar.upperWick;

          const signal: Signal = {
            id: `${isSimulated ? 'SIM' : 'SIG'}-${Date.now()}-SELL`,
            symbol: cfg.symbol,
            signal_type: 'SELL',
            timeframe: cfg.timeframe,
            entry_price: entryPrice,
            stop_loss: stopLoss,
            take_profit_1: tp1,
            take_profit_2: tp2,
            risk_points: riskPoints,
            reward_points_tp1: rewardTP1,
            reward_points_tp2: rewardTP2,
            risk_reward_ratio: rr,
            volume_surge_ratio: Math.round(volumeSurgeRatio * 100) / 100,
            recommended_lot_size: sizing.lotSize,
            risk_amount_usd: sizing.riskAmountUsd,
            potential_profit_tp1_usd: sizing.potentialProfitTp1Usd,
            potential_profit_tp2_usd: sizing.potentialProfitTp2Usd,
            price_action_notes: `Nến rút chân Bearish Pinbar (râu trên ${pinbar.upperWick.toFixed(1)}$ chiếm ${((pinbar.upperWick / pinbar.range) * 100).toFixed(0)}% biên độ) phản ứng tại Kháng cự động $${resistance.price}`,
            volume_analysis_notes: `Volume đạt ${volumeSurgeRatio.toFixed(2)}x MA${cfg.volume_ma_period}, dòng tiền tổ chức từ chối vùng giá cao (Smart Money Exhaustion)`,
            full_rationale: `Nến từ chối tăng tạo râu trên dài tại Kháng cự $${resistance.price} (đã test ${resistance.touch_count} lần). Khối lượng đột biến ${volumeSurgeRatio.toFixed(2)}x xác nhận lực bán áp đảo. Xu hướng ${effectiveBias} (EMA50 $${ema50}). Tỷ lệ R:R đạt 1:${rr}. Khối lượng đề xuất: ${sizing.lotSize} lot (Rủi ro: -$${sizing.riskAmountUsd}, TP1: +$${sizing.potentialProfitTp1Usd}).`,
            status: 'Active',
            created_at: new Date().toISOString(),
            dedup_key: dedupKey,
            candle_timestamp: currentCandle.timestamp,
            is_simulated: isSimulated,
            confluence_score: computeConfluenceScore(
              resistance.touch_count,
              volumeSurgeRatio,
              wickRatio,
              rr
            ),
          };

          return markSimulation(signal, isSimulated);
        }
      }
    }
  }

  return null;
}

/* ────────────────────────────────────────────────────────────────────────────
 * Quota, dedup, kill switch & macro report
 * ──────────────────────────────────────────────────────────────────────────── */

function countLiveSignalsToday(signals: Signal[], now: Date = new Date()): number {
  const today = vnDateKey(now);
  return signals.filter(
    (signal) => !signal.is_simulated && vnDateKey(signal.created_at) === today
  ).length;
}

function hasReachedDailyQuota(): boolean {
  return countLiveSignalsToday(signalsList) >= Math.max(1, config.max_signals_per_day);
}

function isDuplicateSignal(dedupKey: string): boolean {
  return signalsList.some(
    (signal) => !signal.is_simulated && signal.dedup_key === dedupKey
  );
}

function quotaSnapshot() {
  const signalsToday = countLiveSignalsToday(signalsList);
  const maxSignalsPerDay = Math.max(1, config.max_signals_per_day);
  return {
    signalsToday,
    maxSignalsPerDay,
    dailyQuotaReached: signalsToday >= maxSignalsPerDay,
  };
}

function getKillSwitchStatus(now: number = Date.now()): {
  active: boolean;
  event: EconomicEvent | null;
  reason: string;
} {
  if (!config.kill_switch_enabled) {
    return {
      active: false,
      event: null,
      reason: 'News Kill Switch đang tắt theo cấu hình.',
    };
  }

  for (const event of economicEvents) {
    if (event.impact !== 'High') continue;
    const eventTime = new Date(event.scheduled_time).getTime();
    if (!Number.isFinite(eventTime)) continue;

    const preMs = config.pre_news_buffer_minutes * 60 * 1000;
    const postMs = config.post_news_buffer_minutes * 60 * 1000;
    if (now >= eventTime - preMs && now <= eventTime + postMs) {
      const diffMin = Math.round((eventTime - now) / 60000);
      return {
        active: true,
        event,
        reason:
          diffMin > 0
            ? `🛑 Tự động ngưng giao dịch: Sắp ra tin [${event.title}] sau ${diffMin} phút. Tránh rủi ro giãn spread & trượt giá!`
            : `🛑 Tự động ngưng giao dịch: Tin [${event.title}] vừa ra ${Math.abs(diffMin)} phút trước. Đang chờ giá ổn định!`,
      };
    }
  }

  return {
    active: false,
    event: null,
    reason:
      'Thị trường an toàn: Không có tin đỏ USD High Impact trong phạm vi vùng cấm ±30 phút.',
  };
}

function buildDailyMacroReport(
  candles: Candle[],
  events: EconomicEvent[]
): DailyMacroReport {
  const trend = detectEmaTrendBias(candles);
  const ema50 = calculateEMA(candles, 50);
  const ema200 = calculateEMA(candles, 200);
  const lastClose = candles[candles.length - 1]?.close || 0;
  const recent = candles.slice(-24);
  const averageRange =
    recent.length > 0
      ? recent.reduce((sum, candle) => sum + (candle.high - candle.low), 0) / recent.length
      : 8.0;
  const forecastLow = Math.max(0, lastClose - averageRange * 2.5);
  const forecastHigh = lastClose + averageRange * 2.5;

  const today = vnDateKey();
  const highImpactToday = events.filter(
    (event) => event.impact === 'High' && vnDateKey(event.scheduled_time) === today
  );

  const rationale =
    `EMA50 = $${ema50.toFixed(2)} · EMA200 = $${ema200.toFixed(2)} · Giá đóng cửa $${lastClose.toFixed(2)}. ` +
    `Xu hướng hiện tại: ${trend}. Dữ liệu nến đóng cửa lấy miễn phí từ Binance PAXG/USDT, ` +
    `lịch tin USD High/Medium lấy từ ForexFactory JSON — không dùng AI trả phí.`;

  const recommendedAction =
    trend === 'BULLISH'
      ? 'ƯU TIÊN BUY THUẬN XU HƯỚNG: Chỉ canh Pinbar rút chân tại Hỗ trợ động. Bỏ qua SELL ngược EMA50.'
      : trend === 'BEARISH'
        ? 'ƯU TIÊN SELL THUẬN XU HƯỚNG: Chỉ canh Pinbar râu trên tại Kháng cự động. Bỏ qua BUY ngược EMA50.'
        : 'NEUTRAL: EMA50 đi ngang. Chờ đủ hội tụ S/R + Pinbar + Volume, hoặc giữ nguyên tiền mặt nếu nhiễu.';

  return {
    date: today,
    macro_bias: trend,
    confidence_score: Math.round(
      clamp(65 + (candles.length >= 50 ? 10 : 0) + (highImpactToday.length > 0 ? 5 : 0), 0, 95)
    ),
    gold_range_forecast: `$${forecastLow.toFixed(2)} - $${forecastHigh.toFixed(2)}`,
    daily_rationale: rationale,
    high_impact_events_today: highImpactToday.map(
      (event) => `${event.title} (${formatVnTime(event.scheduled_time)})`
    ),
    recommended_action: recommendedAction,
    updated_at: new Date().toISOString(),
  };
}

/* ────────────────────────────────────────────────────────────────────────────
 * Position sizing & Telegram dispatch
 * ──────────────────────────────────────────────────────────────────────────── */

function calculateLotSize(
  accountBalance: number,
  riskPercentage: number,
  slDistancePoints: number,
  rewardTp1Points: number,
  rewardTp2Points: number,
  goldPrice: number,
  leverage = 100
): PositionSizingResult {
  const balance = Math.max(100, accountBalance);
  const riskPct = clamp(riskPercentage, 0.1, 5.0);
  const slPoints = Math.max(1.0, slDistancePoints);
  const ouncesPerLot = 100.0;
  const riskAmountUsd = Math.round(balance * (riskPct / 100) * 100) / 100;
  const lossPerLot = slPoints * ouncesPerLot;
  const rawLot = riskAmountUsd / lossPerLot;
  const lotSize = Math.max(0.01, Math.min(50.0, Math.floor(rawLot * 100) / 100));
  const actualRiskUsd = Math.round(lotSize * lossPerLot * 100) / 100;
  const potentialProfitTp1Usd =
    Math.round(lotSize * rewardTp1Points * ouncesPerLot * 100) / 100;
  const potentialProfitTp2Usd =
    Math.round(lotSize * rewardTp2Points * ouncesPerLot * 100) / 100;
  const marginUsd =
    Math.round(((lotSize * ouncesPerLot * goldPrice) / leverage) * 100) / 100;

  let assessment = 'Bảo toàn vốn tối ưu (Rủi ro < 1.0%)';
  if (riskPct >= 2.0) {
    assessment = 'Khuyến nghị: Rủi ro cao, chỉ dành cho tài khoản nhỏ';
  } else if (riskPct >= 1.0) {
    assessment = 'Chuẩn mực Quản trị Quỹ Prop Firm (1.0% rủi ro)';
  }

  return {
    lotSize,
    riskAmountUsd: actualRiskUsd,
    potentialProfitTp1Usd,
    potentialProfitTp2Usd,
    estimatedMarginUsd: marginUsd,
    riskAssessment: assessment,
  };
}

async function dispatchTelegramAlert(
  signal: Signal,
  botToken: string,
  chatId: string
): Promise<{ success: boolean; messageId?: number; error?: string }> {
  if (!botToken || !chatId) {
    return { success: false, error: 'Chưa cấu hình Telegram Bot Token hoặc Chat ID' };
  }

  const actionIcon = signal.signal_type === 'BUY' ? '🟢' : '🔴';
  const actionText = signal.signal_type === 'BUY' ? 'BUY / LONG' : 'SELL / SHORT';
  const simulationPrefix = signal.is_simulated
    ? '🧪 <b>[SIMULATION - KHÔNG TỐN QUOTA 1 LỆNH/NGÀY]</b>\n'
    : '';

  const lotStr = signal.recommended_lot_size
    ? `${signal.recommended_lot_size} Lot`
    : 'Theo tỷ lệ 1% vốn';
  const riskUsdStr = signal.risk_amount_usd
    ? ` | -$${signal.risk_amount_usd.toFixed(2)}`
    : '';
  const tp1UsdStr = signal.potential_profit_tp1_usd
    ? ` | +$${signal.potential_profit_tp1_usd.toFixed(2)}`
    : '';
  const tp2UsdStr = signal.potential_profit_tp2_usd
    ? ` | +$${signal.potential_profit_tp2_usd.toFixed(2)}`
    : '';
  const confluenceStr =
    typeof signal.confluence_score === 'number'
      ? `\n🎯 <b>Confluence Score:</b> ${signal.confluence_score}/100`
      : '';

  const text = `${simulationPrefix}🚨 <b>GOLD SIGNAL ALERT | ${signal.symbol}</b> 🚨
━━━━━━━━━━━━━━━━━━━━━━
📊 <b>Lệnh:</b> ${actionIcon} <code>${actionText}</code>
⏱ <b>Khung:</b> <code>${signal.timeframe}</code>
💵 <b>Entry:</b> <code>$${signal.entry_price.toFixed(2)}</code>
🎯 <b>Take Profit 1 (1:2):</b> <code>$${signal.take_profit_1.toFixed(2)}</code> (+${signal.reward_points_tp1.toFixed(1)} pts${tp1UsdStr})
🚀 <b>Take Profit 2 (1:3):</b> <code>$${signal.take_profit_2.toFixed(2)}</code> (+${signal.reward_points_tp2.toFixed(1)} pts${tp2UsdStr})
🛑 <b>Stop Loss:</b> <code>$${signal.stop_loss.toFixed(2)}</code> (-${signal.risk_points.toFixed(1)} pts${riskUsdStr})
⚖️ <b>Tỷ lệ R:R:</b> <b>1:${signal.risk_reward_ratio.toFixed(2)}</b>${confluenceStr}
💼 <b>Quản lý vốn đề xuất:</b> <code>${lotStr}</code>
━━━━━━━━━━━━━━━━━━━━━━
🔍 <b>Phân tích Kỹ thuật & Volume:</b>
• <b>Price Action:</b> ${signal.price_action_notes}
• <b>Volume Absorption:</b> ${signal.volume_analysis_notes}
• <b>Đánh giá:</b> ${signal.full_rationale}
━━━━━━━━━━━━━━━━━━━━━━
⚠️ <i>Quản lý rủi ro tối đa 1-2% tài khoản. Chúc bạn giao dịch kỷ luật!</i>
⚡ <i>Engine: Gold Sniper Bot v1.2 (Tokio/Axum + EMA50 Free Trend)</i>`;

  try {
    const url = `https://api.telegram.org/bot${botToken}/sendMessage`;
    const res = await fetch(url, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        chat_id: chatId,
        text,
        parse_mode: 'HTML',
        disable_web_page_preview: true,
      }),
    });

    const data = (await res.json()) as {
      ok?: boolean;
      description?: string;
      result?: { message_id?: number };
    };
    if (res.ok && data.ok) {
      return { success: true, messageId: data.result?.message_id };
    }
    return { success: false, error: data.description || 'Lỗi gửi tin nhắn Telegram' };
  } catch (err) {
    return {
      success: false,
      error: err instanceof Error ? err.message : 'Lỗi kết nối tới Telegram API',
    };
  }
}

/* ────────────────────────────────────────────────────────────────────────────
 * 24/7 scanner worker
 * ──────────────────────────────────────────────────────────────────────────── */

let scanInFlight = false;

async function runScheduledScan(): Promise<void> {
  if (scanInFlight) return;
  scanInFlight = true;

  try {
    if (!config.is_active) return;

    await refreshMarketCandles();

    // Fallback seed candles are for UI continuity only — never send live alerts.
    if (activeDataSource === 'FALLBACK_SEED') return;
    if (hasReachedDailyQuota()) return;

    const latestCandle = candlesBuffer[candlesBuffer.length - 1];
    if (!latestCandle) return;

    const dedupKey = buildDedupKey(
      config.symbol,
      config.timeframe,
      latestCandle.timestamp
    );
    if (isDuplicateSignal(dedupKey)) return;
    if (config.session_filter_enabled && !isWithinActiveSession()) return;

    const kill = getKillSwitchStatus();
    if (kill.active) return;

    const signal = runAnalysis(candlesBuffer, config);
    if (!signal) return;

    if (config.telegram_bot_token && config.telegram_chat_id) {
      const telegramResult = await dispatchTelegramAlert(
        signal,
        config.telegram_bot_token,
        config.telegram_chat_id
      );
      if (telegramResult.success) {
        signal.telegram_message_id = telegramResult.messageId;
      }
    }

    signalsList.unshift(signal);
    if (signalsList.length > MAX_SIGNAL_HISTORY) {
      signalsList = signalsList.slice(0, MAX_SIGNAL_HISTORY);
    }
    saveState();

    console.log(
      `[Scanner] 🎯 Live signal ${signal.signal_type} @ $${signal.entry_price} | confluence ${signal.confluence_score}/100 | quota ${quotaSnapshot().signalsToday}/${quotaSnapshot().maxSignalsPerDay}`
    );
  } catch (err) {
    console.warn('[Scanner] Cycle failed:', err instanceof Error ? err.message : err);
  } finally {
    scanInFlight = false;
  }
}

async function initializeEngine(): Promise<void> {
  console.log('[Init] Syncing free live data (Binance PAXG + ForexFactory)...');

  const [events, market] = await Promise.all([
    fetchForexFactoryEvents(),
    refreshMarketCandles(),
  ]);

  if (events && events.length > 0) {
    economicEvents = events;
  }
  if (!market.success) {
    console.warn('[Init] Live feed unavailable — dashboard will use offline seed candles.');
  }

  dailyMacroReport = buildDailyMacroReport(candlesBuffer, economicEvents);
  saveState();

  console.log(
    `[Init] Feed=${activeDataSource} (${dataSourceLabel(activeDataSource)}) · candles=${candlesBuffer.length} · events=${economicEvents.length} · EMA50=${calculateEMA(candlesBuffer, 50)}`
  );

  if (market.success) {
    await runScheduledScan();
  }
}

/* ────────────────────────────────────────────────────────────────────────────
 * Express API
 * ──────────────────────────────────────────────────────────────────────────── */

async function startServer() {
  const app = express();
  const PORT = Number(process.env.PORT) || 3000;

  app.use(express.json({ limit: '1mb' }));
  loadPersistedState();

  /* API 1: Health & system status */
  app.get(['/api/health', '/health'], (_req: Request, res: Response) => {
    const kill = getKillSwitchStatus();
    const livePrice = candlesBuffer[candlesBuffer.length - 1]?.close ?? null;
    res.json({
      status: 'healthy',
      service: 'Gold Signal Bot (Express Free Engine)',
      symbol: config.symbol,
      timeframe: config.timeframe,
      signalsCount: signalsList.length,
      candlesCount: candlesBuffer.length,
      isActive: config.is_active,
      ...quotaSnapshot(),
      dataSource: activeDataSource,
      dataSourceLabel: dataSourceLabel(activeDataSource),
      livePrice,
      ema50: calculateEMA(candlesBuffer, 50),
      trendBias: detectEmaTrendBias(candlesBuffer),
      killSwitchActive: kill.active,
      h4FilterEnabled: config.htf_h4_filter_enabled,
      d1FilterEnabled: config.htf_d1_filter_enabled,
      aiReviewEnabled: config.ai_signal_review_enabled,
      volumeDeltaFilterEnabled: config.volume_delta_filter_enabled,
      rsiFilterEnabled: config.rsi_filter_enabled,
      tradingviewWebhook: '/api/webhook/tradingview',
    });
  });

  /* API 2: Current closed candles + dynamic S/R + EMA trend */
  app.get('/api/candles', (_req: Request, res: Response) => {
    const { supports, resistances } = detectDynamicSR(
      candlesBuffer,
      config.lookback_period,
      2.5
    );
    const volumeMA = calculateVolumeMA(candlesBuffer, config.volume_ma_period);
    const lastCandle = candlesBuffer[candlesBuffer.length - 1];
    const lastClosedPrice = lastCandle?.close ?? 2684.5;

    res.json({
      count: candlesBuffer.length,
      candles: candlesBuffer,
      supports,
      resistances,
      volumeMA: Math.round(volumeMA),
      currentPrice: lastClosedPrice,
      lastClosedPrice,
      ema50: calculateEMA(candlesBuffer, 50),
      ema200: calculateEMA(candlesBuffer, 200),
      trendBias: detectEmaTrendBias(candlesBuffer),
      dataSource: activeDataSource,
      dataSourceLabel: dataSourceLabel(activeDataSource),
      volumeDelta: computeVolumeDelta(candlesBuffer, config.volume_delta_lookback),
    });
  });

  /* API 3: Signal history + daily quota */
  app.get('/api/signals', (_req: Request, res: Response) => {
    res.json({
      total: signalsList.length,
      signals: signalsList,
      ...quotaSnapshot(),
    });
  });

  /* API 4: Config get/update (persisted to data/bot_state.json) */
  app.get('/api/config', (_req: Request, res: Response) => {
    res.json(config);
  });

  app.post('/api/config', (req: Request, res: Response) => {
    const previousDataSource = config.data_source;
    config = normalizeConfig({ ...config, ...(req.body || {}) });
    saveState();

    if (config.data_source !== previousDataSource) {
      void refreshMarketCandles();
    }

    res.json({
      status: 'success',
      message: 'Cấu hình Sniper đã được lưu bền vững xuống data/bot_state.json!',
      config,
    });
  });

  /* API 4.2: Position sizing calculator */
  app.post('/api/calculate-lot', (req: Request, res: Response) => {
    const body = req.body || {};
    const lastCandle = candlesBuffer[candlesBuffer.length - 1];
    const balance = numeric(body.account_balance, config.account_balance);
    const riskPct = numeric(body.risk_percentage, config.risk_percentage);
    const sl = numeric(body.sl_distance, config.gold_sl_buffer_points);
    const tp1 = numeric(body.tp1_distance, sl * config.min_risk_reward);
    const tp2 = numeric(body.tp2_distance, sl * 3.0);
    const goldPrice = numeric(body.current_price, lastCandle?.close || 2684.5);
    const leverage = numeric(body.leverage, config.account_leverage);

    res.json(calculateLotSize(balance, riskPct, sl, tp1, tp2, goldPrice, leverage));
  });

  /* API 4.5: ForexFactory calendar & Kill Switch */
  app.get('/api/news-calendar', (_req: Request, res: Response) => {
    const kill = getKillSwitchStatus();
    res.json({
      events: economicEvents,
      isKillSwitchActive: kill.active,
      activeEvent: kill.event,
      killSwitchReason: kill.reason,
      killSwitchEnabled: config.kill_switch_enabled,
      macroBias: config.macro_bias,
      effectiveBias: resolveEffectiveBias(config, candlesBuffer),
      ema50: calculateEMA(candlesBuffer, 50),
      trendBias: detectEmaTrendBias(candlesBuffer),
      dataSource: activeDataSource,
      sessionFilterEnabled: config.session_filter_enabled,
      sessionWindowOpen: isWithinActiveSession(),
    });
  });

  /* API 4.6: Latest macro bias / EMA report */
  app.get('/api/macro-bias/latest', (_req: Request, res: Response) => {
    const kill = getKillSwitchStatus();
    res.json({
      macro_bias: config.macro_bias,
      effective_bias: resolveEffectiveBias(config, candlesBuffer),
      ema50: calculateEMA(candlesBuffer, 50),
      trend_bias: detectEmaTrendBias(candlesBuffer),
      daily_report: dailyMacroReport,
      is_kill_switch_active: kill.active,
      active_blocking_event: kill.event,
      kill_switch_enabled: config.kill_switch_enabled,
    });
  });

  /* API 4.7: Manual Daily Macro Cron — ForexFactory sync + EMA50/200 report */
  app.post('/api/cron/trigger-daily-macro', async (_req: Request, res: Response) => {
    const events = await fetchForexFactoryEvents();
    if (events && events.length > 0) {
      economicEvents = events;
    }

    await refreshMarketCandles();
    dailyMacroReport = buildDailyMacroReport(candlesBuffer, economicEvents);
    saveState();

    res.json({
      status: 'success',
      message:
        'Đã đồng bộ ForexFactory & tính lại xu hướng EMA50/200 miễn phí (không gọi AI trả phí).',
      report: dailyMacroReport,
      events_synced: economicEvents.length,
      data_source: activeDataSource,
    });
  });

  /* API 4.8: TradingView Webhook — quota, dedup, kill switch & trend filter */
  const handleTradingViewWebhook = async (req: Request, res: Response) => {
    const payload = req.body || {};
    const now = Date.now();
    const action = String(payload.action || 'BUY').toUpperCase();
    const ticker = String(payload.ticker || 'XAUUSD').toUpperCase();
    const lastCandle = candlesBuffer[candlesBuffer.length - 1];
    const price = numeric(payload.price, lastCandle?.close || 2684.5);

    if (
      config.webhook_passphrase &&
      String(payload.passphrase || '') !== config.webhook_passphrase
    ) {
      return res.status(401).json({
        status: 'unauthorized',
        action_taken: 'DROPPED',
        reason: 'Webhook passphrase không hợp lệ.',
      });
    }

    // Candle ingestion is always accepted (it is market data, not a trade).
    if (action === 'CANDLE') {
      const candle: Candle = {
        timestamp: numeric(payload.timestamp, now),
        open: numeric(payload.open, price),
        high: numeric(payload.high, price + 1.2),
        low: numeric(payload.low, price - 1.2),
        close: numeric(payload.close, price),
        volume: numeric(payload.volume, 1600),
        timeframe: String(payload.timeframe || config.timeframe),
      };
      candlesBuffer.push(candle);
      if (candlesBuffer.length > MAX_SIGNAL_HISTORY) candlesBuffer.shift();
      saveState();

      return res.status(200).json({
        status: 'candle_recorded',
        message: 'Nến TradingView đã lưu vào buffer phân tích thị trường.',
        candle,
      });
    }

    if (!config.is_active) {
      return res.status(200).json({
        status: 'bot_inactive',
        action_taken: 'DROPPED',
        reason: 'Bot đang ở trạng thái TẠM DỪNG. Bật is_active để nhận tín hiệu.',
      });
    }

    if (action !== 'BUY' && action !== 'SELL') {
      return res.status(400).json({
        status: 'invalid_action',
        action_taken: 'DROPPED',
        reason: 'TradingView action phải là BUY, SELL hoặc CANDLE.',
      });
    }

    const quota = quotaSnapshot();
    if (quota.dailyQuotaReached) {
      return res.status(200).json({
        status: 'blocked_by_daily_quota',
        action_taken: 'DROPPED',
        reason: `🎯 Đã đạt Daily Sniper Quota (${quota.signalsToday}/${quota.maxSignalsPerDay} lệnh hôm nay). Bot chỉ nhận tối đa ${quota.maxSignalsPerDay} lệnh thật/ngày.`,
        ...quota,
      });
    }

    const kill = getKillSwitchStatus(now);
    if (kill.active) {
      return res.status(200).json({
        status: 'blocked_by_kill_switch',
        action_taken: 'DROPPED',
        reason: kill.reason,
        event: kill.event?.title || 'US High Impact News',
      });
    }

    const effectiveBias = resolveEffectiveBias(config, candlesBuffer);
    if (action === 'BUY' && effectiveBias === 'BEARISH') {
      return res.status(200).json({
        status: 'blocked_by_macro_bias',
        action_taken: 'DROPPED',
        reason:
          'Trend/Kill Bias hôm nay là BEARISH (EMA50 hoặc cấu hình). Hệ thống bỏ qua BUY để bảo toàn vốn.',
        current_bias: effectiveBias,
      });
    }
    if (action === 'SELL' && effectiveBias === 'BULLISH') {
      return res.status(200).json({
        status: 'blocked_by_macro_bias',
        action_taken: 'DROPPED',
        reason:
          'Trend/Kill Bias hôm nay là BULLISH (EMA50 hoặc cấu hình). Hệ thống bỏ qua SELL để bảo toàn vốn.',
        current_bias: effectiveBias,
      });
    }

    const candleTimestamp = lastCandle?.timestamp || now;
    const dedupKey = buildDedupKey(ticker, config.timeframe, candleTimestamp);
    if (isDuplicateSignal(dedupKey)) {
      return res.status(200).json({
        status: 'blocked_by_dedup',
        action_taken: 'DROPPED',
        reason: 'Cây nến này đã phát tín hiệu thật trước đó (dedup_key trùng).',
        dedup_key: dedupKey,
      });
    }

    const isBuy = action === 'BUY';
    const slBuffer = clamp(config.gold_sl_buffer_points, 5.0, 25.0);
    const stopLoss = isBuy
      ? Math.round((price - slBuffer) * 100) / 100
      : Math.round((price + slBuffer) * 100) / 100;
    const riskPoints = Math.round(Math.abs(price - stopLoss) * 100) / 100;
    const rewardTP1 = Math.round(riskPoints * config.min_risk_reward * 100) / 100;
    const rewardTP2 = Math.round(riskPoints * 3.0 * 100) / 100;
    const tp1 = isBuy
      ? Math.round((price + rewardTP1) * 100) / 100
      : Math.round((price - rewardTP1) * 100) / 100;
    const tp2 = isBuy
      ? Math.round((price + rewardTP2) * 100) / 100
      : Math.round((price - rewardTP2) * 100) / 100;
    const rr = Math.round((rewardTP1 / riskPoints) * 100) / 100;

    const sizing = calculateLotSize(
      config.account_balance,
      config.risk_percentage,
      riskPoints,
      rewardTP1,
      rewardTP2,
      price,
      config.account_leverage
    );

    const signal: Signal = {
      id: `TV-${now}-${action}`,
      symbol: ticker,
      signal_type: isBuy ? 'BUY' : 'SELL',
      timeframe: String(payload.timeframe || config.timeframe),
      entry_price: price,
      stop_loss: stopLoss,
      take_profit_1: tp1,
      take_profit_2: tp2,
      risk_points: riskPoints,
      reward_points_tp1: rewardTP1,
      reward_points_tp2: rewardTP2,
      risk_reward_ratio: rr,
      volume_surge_ratio: 1.95,
      recommended_lot_size: sizing.lotSize,
      risk_amount_usd: sizing.riskAmountUsd,
      potential_profit_tp1_usd: sizing.potentialProfitTp1Usd,
      potential_profit_tp2_usd: sizing.potentialProfitTp2Usd,
      price_action_notes:
        payload.comment || `TradingView Webhook Alert kích hoạt tại $${price.toFixed(2)}`,
      volume_analysis_notes:
        'Xác nhận hấp thụ lực giá theo tín hiệu Pine Script từ TradingView',
      full_rationale: `TradingView Alert [${action}] thực thi an toàn ngoài vùng cấm tin tức. Khối lượng tự động: ${sizing.lotSize} lot (Rủi ro: -$${sizing.riskAmountUsd}, TP1: +$${sizing.potentialProfitTp1Usd}). Tỷ lệ R:R chuẩn 1:${rr}.`,
      status: 'Active',
      created_at: new Date().toISOString(),
      dedup_key: dedupKey,
      candle_timestamp: candleTimestamp,
      is_simulated: false,
      confluence_score: computeConfluenceScore(2, 1.95, 2.0, rr),
    };

    if (config.telegram_bot_token && config.telegram_chat_id) {
      const telegramResult = await dispatchTelegramAlert(
        signal,
        config.telegram_bot_token,
        config.telegram_chat_id
      );
      if (telegramResult.success) {
        signal.telegram_message_id = telegramResult.messageId;
      }
    }

    signalsList.unshift(signal);
    if (signalsList.length > MAX_SIGNAL_HISTORY) {
      signalsList = signalsList.slice(0, MAX_SIGNAL_HISTORY);
    }
    saveState();

    return res.status(201).json({
      status: 'signal_executed',
      message:
        'Tín hiệu TradingView đã vượt qua Sniper Quota, News Kill Switch & Trend Filter, đồng thời được lưu bền vững.',
      signal,
      ...quotaSnapshot(),
    });
  };

  app.post('/api/webhook/tradingview', handleTradingViewWebhook);
  app.post('/api/webhook', handleTradingViewWebhook);

  /* API 5: Telegram test ping (aliases match both Rust & legacy clients) */
  const handleTelegramTest = async (req: Request, res: Response) => {
    const body = req.body || {};
    const token = body.token || config.telegram_bot_token;
    const chatId = body.chatId || body.chat_id || config.telegram_chat_id;

    if (!token || !chatId) {
      res.status(400).json({
        status: 'error',
        message: 'Vui lòng cung cấp Bot Token và Chat ID trước khi test!',
      });
      return;
    }

    const testText = `🔔 <b>[Gold Sniper Bot - Test Alert]</b>
━━━━━━━━━━━━━━━━━━━━━━
✅ Kết nối Telegram Bot API thành công!
• Hệ thống: <b>Gold Sniper 1 Lệnh/Ngày + Free Live Data</b>
• Cặp tài sản: <code>${config.symbol}</code>
• Nguồn dữ liệu: <code>${dataSourceLabel(activeDataSource)}</code>
• Thuật toán: <i>Dynamic S/R + Pinbar Rejection + Volume Absorption + EMA50 Trend</i>
• R:R Tối thiểu: <b>1:${config.min_risk_reward}</b>
• Daily Sniper Quota: <b>${quotaSnapshot().signalsToday}/${quotaSnapshot().maxSignalsPerDay} lệnh</b>
━━━━━━━━━━━━━━━━━━━━━━
⚡ <i>Sẵn sàng tiếp nhận và bắn tín hiệu tự động 24/7 với chi phí API 0đ.</i>`;

    try {
      const tgRes = await fetch(`https://api.telegram.org/bot${token}/sendMessage`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          chat_id: chatId,
          text: testText,
          parse_mode: 'HTML',
        }),
      });

      const data = (await tgRes.json()) as { ok?: boolean; description?: string };
      if (tgRes.ok && data.ok) {
        res.json({ status: 'success', message: 'Đã gửi tin nhắn test thành công tới Telegram!' });
      } else {
        res.status(400).json({ status: 'error', message: data.description || 'Lỗi gửi Telegram' });
      }
    } catch (err) {
      res.status(500).json({
        status: 'error',
        message: err instanceof Error ? err.message : 'Lỗi mạng khi kết nối Telegram',
      });
    }
  };

  app.post('/api/telegram/test', handleTelegramTest);
  app.post('/api/test-telegram', handleTelegramTest);

  /* API 5.2: Send a specific signal to Telegram (Signal Modal button) */
  app.post('/api/telegram/send-signal', async (req: Request, res: Response) => {
    const body = req.body || {};
    const signal = body.signal as Signal | undefined;
    if (!signal) {
      res.status(400).json({ status: 'error', message: 'Missing signal in request body' });
      return;
    }

    const token = body.token || config.telegram_bot_token;
    const chatId = body.chatId || body.chat_id || config.telegram_chat_id;
    const result = await dispatchTelegramAlert(signal, token, chatId);
    if (result.success) {
      res.json({ status: 'success', message: 'Đã gửi tín hiệu tới Telegram!', messageId: result.messageId });
    } else {
      res.status(400).json({ status: 'error', message: result.error || 'Lỗi gửi Telegram' });
    }
  });

  /* API 6: Manual simulation — ALWAYS is_simulated=true, never consumes quota */
  app.post('/api/simulate-setup', async (req: Request, res: Response) => {
    const setupType = String(req.body?.type || 'BUY_PINBAR') as
      | 'BUY_PINBAR'
      | 'SELL_PINBAR'
      | 'RANDOM_CANDLE';
    const last = candlesBuffer[candlesBuffer.length - 1];
    const ts = Date.now();
    const { supports, resistances } = detectDynamicSR(
      candlesBuffer,
      config.lookback_period,
      2.5
    );
    const volMA = Math.max(calculateVolumeMA(candlesBuffer, config.volume_ma_period), 100);

    let newCandle: Candle;

    if (setupType === 'BUY_PINBAR') {
      const supportPrice = supports.length ? supports[0].price : (last?.close || 2684.5) - 6;
      const open = supportPrice + 2.5;
      const low = supportPrice - 1.2;
      const close = supportPrice + 3.8;
      const high = close + 0.6;
      const volume = Math.round(volMA * (config.volume_surge_multiplier + 0.35));

      newCandle = {
        timestamp: ts,
        open: Math.round(open * 100) / 100,
        high: Math.round(high * 100) / 100,
        low: Math.round(low * 100) / 100,
        close: Math.round(close * 100) / 100,
        volume,
        timeframe: config.timeframe,
      };
    } else if (setupType === 'SELL_PINBAR') {
      const resistancePrice = resistances.length
        ? resistances[0].price
        : (last?.close || 2684.5) + 6;
      const open = resistancePrice - 2.5;
      const high = resistancePrice + 1.4;
      const close = resistancePrice - 3.5;
      const low = close - 0.5;
      const volume = Math.round(volMA * (config.volume_surge_multiplier + 0.4));

      newCandle = {
        timestamp: ts,
        open: Math.round(open * 100) / 100,
        high: Math.round(high * 100) / 100,
        low: Math.round(low * 100) / 100,
        close: Math.round(close * 100) / 100,
        volume,
        timeframe: config.timeframe,
      };
    } else {
      const delta = (Math.random() - 0.49) * 4.0;
      const open = last?.close || 2684.5;
      const close = Math.round((open + delta) * 100) / 100;
      const high = Math.round((Math.max(open, close) + Math.random() * 1.5) * 100) / 100;
      const low = Math.round((Math.min(open, close) - Math.random() * 1.5) * 100) / 100;
      const volume = Math.round(volMA * (0.8 + Math.random() * 0.5));

      newCandle = {
        timestamp: ts,
        open,
        high,
        low,
        close,
        volume,
        timeframe: config.timeframe,
      };
    }

    candlesBuffer.push(newCandle);
    if (candlesBuffer.length > MAX_SIGNAL_HISTORY) {
      candlesBuffer.shift();
    }

    // Simulation bypasses session/news filters and forces NEUTRAL trend, so the
    // operator can verify both BUY and SELL flows at any time of day.
    const signal = runAnalysis(candlesBuffer, config, { isSimulated: true });

    if (signal) {
      if (config.telegram_bot_token && config.telegram_chat_id) {
        const telegramResult = await dispatchTelegramAlert(
          signal,
          config.telegram_bot_token,
          config.telegram_chat_id
        );
        if (telegramResult.success) {
          signal.telegram_message_id = telegramResult.messageId;
        }
      }

      signalsList.unshift(signal);
      if (signalsList.length > MAX_SIGNAL_HISTORY) {
        signalsList = signalsList.slice(0, MAX_SIGNAL_HISTORY);
      }
      saveState();

      return res.json({
        status: 'signal_triggered',
        simulated: true,
        message:
          '🧪 [SIMULATION - Không tốn Quota 1 lệnh/ngày] Tín hiệu test đã được tạo và không ảnh hưởng quota thật.',
        candle: newCandle,
        signal,
        ...quotaSnapshot(),
      });
    }

    saveState();
    return res.json({
      status: 'candle_added',
      simulated: true,
      candle: newCandle,
      message:
        'Nến test đã thêm vào buffer. Chưa hội tụ đủ điều kiện Pinbar + Volume tại vùng cản (không tốn quota).',
      ...quotaSnapshot(),
    });
  });

  /* Vite integration for local development / production SPA */
  if (process.env.NODE_ENV === 'production') {
    app.use(express.static(path.resolve(process.cwd(), 'dist')));
    app.get('*', (_req: Request, res: Response) => {
      res.sendFile(path.resolve(process.cwd(), 'dist', 'index.html'));
    });
  } else {
    const vite = await createViteServer({
      server: { middlewareMode: true },
      appType: 'spa',
    });
    app.use(vite.middlewares);
  }

  app.listen(PORT, '0.0.0.0', () => {
    console.log(`[Gold Signal Bot Server] Running on http://0.0.0.0:${PORT}`);
  });

  // Kick off live data sync and the 24/7 scanner (60-second cadence).
  void initializeEngine();
  setInterval(() => {
    void runScheduledScan();
  }, 60_000);
}

startServer().catch((err) => {
  console.error('Failed to start server:', err);
});
