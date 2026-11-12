import type {
  AiSignalReview,
  BotConfig,
  Candle,
  CandlesResponse,
  DailyMacroReport,
  EconomicEvent,
  MacroBias,
  MarketContext,
  NewsCalendarData,
  Signal,
  TrendBias,
  TradingViewWebhookPayload,
} from '../types';

export type SimulateSetupType = 'BUY_PINBAR' | 'SELL_PINBAR' | 'RANDOM_CANDLE';

export interface HtfCandlesResponse {
  timeframe: string;
  candles: Candle[];
}

export interface SignalsResponse {
  total: number;
  signals: Signal[];
  signalsToday: number;
  maxSignalsPerDay: number;
  dailyQuotaReached: boolean;
}

export interface ConfigResponse {
  status: string;
  message: string;
  config: BotConfig;
}

export interface HealthResponse {
  status: string;
  symbol: string;
  timeframe: string;
  signalsCount: number;
  candlesCount: number;
  isActive: boolean;
  signalsToday: number;
  maxSignalsPerDay: number;
  dailyQuotaReached: boolean;
  dataSource: string;
  dataSourceLabel: string;
  volumeSource?: string;
  priceSource?: string;
  spotPrice?: number | null;
  spotBasis?: number | null;
  livePrice?: number | null;
  ema50?: number;
  trendBias?: TrendBias;
  killSwitchActive?: boolean;
  /** Optional H4 trend filter state (default false). */
  h4FilterEnabled?: boolean;
  /** Optional AI Chief Risk Officer veto gate (default false). */
  aiReviewEnabled?: boolean;
  /** Optional Volume Delta/CVD gate state (default false). */
  volumeDeltaFilterEnabled?: boolean;
  /** Optional hard RSI gate state (default false). */
  rsiFilterEnabled?: boolean;
  /** True only when the live feed is fresh and not a fallback seed. */
  feedOk?: boolean;
  /** Age of the last scanner tick in seconds (null before the first scan). */
  lastScanAgeSecs?: number | null;
  /** True when a Telegram bot token is configured (never returned). */
  telegramConfigured?: boolean;
  /** True when a webhook passphrase is configured (never returned). */
  webhookPassphraseConfigured?: boolean;
}

export interface AiReviewResponse {
  status: 'success' | 'disabled' | 'unavailable' | 'error';
  review?: AiSignalReview;
  message?: string;
}

export interface MacroBiasResponse {
  macro_bias: MacroBias;
  effective_bias: TrendBias;
  ema50?: number;
  trend_bias?: TrendBias;
  daily_report: DailyMacroReport | null;
  is_kill_switch_active: boolean;
  active_blocking_event: EconomicEvent | null;
  kill_switch_enabled: boolean;
}

export interface DailyMacroTriggerResponse {
  status: string;
  message: string;
  report: DailyMacroReport;
  events_synced: number;
}

export interface SimulateSetupResponse {
  status: 'signal_triggered' | 'candle_added';
  simulated?: boolean;
  candle?: Candle;
  signal?: Signal;
  message?: string;
}

export interface TelegramTestResponse {
  status: 'success' | 'error';
  message: string;
}

export interface TradingViewWebhookResponse {
  status: string;
  action_taken?: string;
  message?: string;
  reason?: string;
  signal?: Signal;
}

export interface LotCalculationResponse {
  lotSize: number;
  riskAmountUsd: number;
  potentialProfitTp1Usd: number;
  potentialProfitTp2Usd: number;
  estimatedMarginUsd: number;
  riskAssessment: string;
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, {
    ...init,
    headers: {
      'Content-Type': 'application/json',
      ...(init?.headers as Record<string, string> | undefined),
    },
  });

  const payload = (await res.json().catch(() => ({}))) as { message?: string } & T;
  if (!res.ok) {
    throw new Error(payload?.message || `API request failed: ${path} (${res.status})`);
  }
  return payload as T;
}

/** GET /api/candles — live closed OHLCV candles + dynamic S/R + EMA trend. */
export function fetchCandles(): Promise<CandlesResponse> {
  return request<CandlesResponse>('/api/candles');
}

/**
 * GET /api/context — read-only D1/H4/H1 market context.
 * The Rust engine returns 204 until the higher-timeframe feed is ready;
 * the Express fallback has no context endpoint, so resolve to null.
 */
export async function fetchMarketContext(): Promise<MarketContext | null> {
  const res = await fetch('/api/context');
  if (res.status === 204) return null;
  if (!res.ok) throw new Error(`API request failed: /api/context (${res.status})`);
  const contentType = res.headers.get('content-type') || '';
  if (!contentType.includes('application/json')) return null;
  return (await res.json()) as MarketContext;
}

/**
 * GET /api/htf-candles — closed H4/D1 candles for the "view toàn cảnh" chart.
 * The Express dev fallback has no HTF endpoint; resolve to an empty list there.
 */
export async function fetchHtfCandles(
  timeframe: 'H4' | 'D1' = 'H4',
  limit = 120
): Promise<HtfCandlesResponse> {
  const params = new URLSearchParams({
    timeframe,
    limit: String(limit),
  });
  const res = await fetch(`/api/htf-candles?${params.toString()}`);
  if (!res.ok) return { timeframe, candles: [] };
  const contentType = res.headers.get('content-type') || '';
  if (!contentType.includes('application/json')) return { timeframe, candles: [] };
  const payload = (await res.json()) as Partial<HtfCandlesResponse>;
  return {
    timeframe: payload.timeframe || timeframe,
    candles: Array.isArray(payload.candles) ? payload.candles : [],
  };
}

/** GET /api/signals — signal history plus today's Sniper quota counters. */
export function fetchSignals(): Promise<SignalsResponse> {
  return request<SignalsResponse>('/api/signals');
}

/** GET /api/config — current bot configuration. */
export function fetchConfig(): Promise<BotConfig> {
  return request<BotConfig>('/api/config');
}

/** POST /api/config — persist bot configuration to disk. */
export function saveConfig(config: BotConfig): Promise<ConfigResponse> {
  return request<ConfigResponse>('/api/config', {
    method: 'POST',
    body: JSON.stringify(config),
  });
}

/** GET /api/news-calendar — ForexFactory USD events + Kill Switch state. */
export function fetchNewsCalendar(): Promise<NewsCalendarData> {
  return request<NewsCalendarData>('/api/news-calendar');
}

/** GET /api/health — engine, feed, quota and trend status. */
export function fetchHealth(): Promise<HealthResponse> {
  return request<HealthResponse>('/api/health');
}

/** GET /api/macro-bias/latest — latest macro report & effective EMA trend. */
export function fetchMacroBias(): Promise<MacroBiasResponse> {
  return request<MacroBiasResponse>('/api/macro-bias/latest');
}

/** POST /api/cron/trigger-daily-macro — sync ForexFactory and rebuild the EMA report. */
export function triggerDailyMacroScan(): Promise<DailyMacroTriggerResponse> {
  return request<DailyMacroTriggerResponse>('/api/cron/trigger-daily-macro', {
    method: 'POST',
  });
}

/** POST /api/simulate-setup — manual test setup; never consumes the daily quota. */
export function simulateSetup(type: SimulateSetupType): Promise<SimulateSetupResponse> {
  return request<SimulateSetupResponse>('/api/simulate-setup', {
    method: 'POST',
    body: JSON.stringify({ type }),
  });
}

/** POST /api/telegram/test — send a Telegram test ping. */
export function testTelegram(token: string, chatId: string): Promise<TelegramTestResponse> {
  return request<TelegramTestResponse>('/api/telegram/test', {
    method: 'POST',
    body: JSON.stringify({ token, chatId }),
  });
}

/** POST /api/telegram/send-signal — dispatch one specific signal alert. */
export function sendSignalTelegram(
  signal: Signal,
  token: string,
  chatId: string
): Promise<TelegramTestResponse & { messageId?: number }> {
  return request<TelegramTestResponse & { messageId?: number }>('/api/telegram/send-signal', {
    method: 'POST',
    body: JSON.stringify({ signal, token, chatId }),
  });
}

/**
 * POST /api/ai/review-signal — manual AI Chief Risk Officer review.
 * Veto-only and side-effect free: never sends Telegram or consumes the daily quota.
 */
export function reviewSignal(
  signal: Signal,
  context?: MarketContext | null
): Promise<AiReviewResponse> {
  return request<AiReviewResponse>('/api/ai/review-signal', {
    method: 'POST',
    body: JSON.stringify(context ? { signal, context } : { signal }),
  });
}

/** POST /api/calculate-lot — server-side position sizing. */
export function calculateLot(payload: {
  account_balance: number;
  risk_percentage: number;
  sl_distance: number;
  tp1_distance?: number;
  tp2_distance?: number;
  current_price?: number;
  leverage?: number;
}): Promise<LotCalculationResponse> {
  return request<LotCalculationResponse>('/api/calculate-lot', {
    method: 'POST',
    body: JSON.stringify(payload),
  });
}

/** POST /api/webhook/tradingview — forward a TradingView alert into the engine. */
export function sendTradingViewWebhook(
  payload: TradingViewWebhookPayload
): Promise<TradingViewWebhookResponse> {
  return request<TradingViewWebhookResponse>('/api/webhook/tradingview', {
    method: 'POST',
    body: JSON.stringify(payload),
  });
}
