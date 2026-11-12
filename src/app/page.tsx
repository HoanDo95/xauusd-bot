'use client';

import React, { useCallback, useEffect, useState } from 'react';
import {
  BotConfig,
  Candle,
  DivergenceAlertsResponse,
  LiveViewResponse,
  MacroFlowResponse,
  MarketContext,
  NewsCalendarData,
  ScenarioResponse,
  Signal,
  SupportResistanceZone,
  TrendBias,
  VolumeDeltaSnapshot,
} from '../types';
import {
  fetchCandles,
  fetchConfig,
  fetchDivergenceAlerts,
  fetchLiveView,
  fetchMacroFlow,
  fetchMarketContext,
  fetchNewsCalendar,
  fetchScenarios,
  fetchSignals,
  saveConfig,
  sendScenariosTelegram,
  sendSignalTelegram,
  simulateSetup,
  startScanner,
  stopScanner,
  testTelegram,
  type SimulateSetupType,
} from '../lib/api';
import { Header, type DashboardTab } from '../components/Header';
import { MetricCards } from '../components/MetricCards';
import { MacroContextCard } from '../components/MacroContextCard';
import { MacroFlowCard } from '../components/MacroFlowCard';
import { ScenarioCard } from '../components/ScenarioCard';
import { LiveSetupCard } from '../components/LiveSetupCard';
import { DivergenceAlertsCard } from '../components/DivergenceAlertsCard';
import { HtfChartCard } from '../components/HtfChartCard';
import { CandlestickChart } from '../components/CandlestickChart';
import { SimulateToolbar } from '../components/SimulateToolbar';
import { SignalCard } from '../components/SignalCard';
import { SignalModal } from '../components/SignalModal';
import { ConfigPanel } from '../components/ConfigPanel';
import { NewsKillSwitchBanner } from '../components/NewsKillSwitchBanner';
import { PositionSizeWidget } from '../components/PositionSizeWidget';
import { WebhookAndMacroHub } from '../components/WebhookAndMacroHub';

const DEFAULT_CONFIG: BotConfig = {
  symbol: 'XAU/USD',
  timeframe: 'M15',
  min_risk_reward: 2.0,
  gold_sl_buffer_points: 10.0,
  volume_surge_multiplier: 1.5,
  volume_filter_enabled: true,
  rejection_wick_ratio: 2.0,
  lookback_period: 25,
  volume_ma_period: 20,
  risk_percentage: 1.0,
  account_balance: 10000,
  account_leverage: 100,
  telegram_bot_token: '',
  telegram_chat_id: '',
  is_active: true,
  kill_switch_enabled: true,
  pre_news_buffer_minutes: 30,
  post_news_buffer_minutes: 30,
  macro_bias: 'AUTO',
  order_block_filter: true,
  webhook_passphrase: '',
  max_signals_per_day: 1,
  session_filter_enabled: true,
  session_alerts_enabled: true,
  session_alert_lead_minutes: 60,
  data_source: 'BINANCE_PAXG',
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
  divergence_alert_enabled: true,
  divergence_alert_m15: true,
  divergence_alert_h1: true,
  divergence_alert_h4: true,
  divergence_alert_cooldown_minutes: 60,
  macro_flow_enabled: true,
};

export default function DashboardPage() {
  const [currentTab, setCurrentTab] = useState<DashboardTab>('monitor');

  const [candles, setCandles] = useState<Candle[]>([]);
  const [supports, setSupports] = useState<SupportResistanceZone[]>([]);
  const [resistances, setResistances] = useState<SupportResistanceZone[]>([]);
  const [volumeMA, setVolumeMA] = useState<number>(1450);
  const [currentPrice, setCurrentPrice] = useState<number>(2684.5);
  const [ema50, setEma50] = useState<number>(0);
  const [trendBias, setTrendBias] = useState<TrendBias>('NEUTRAL');
  const [dataSource, setDataSource] = useState<string>('BINANCE_PAXG');
  const [volumeSource, setVolumeSource] = useState<string>('BINANCE_PAXG');
  const [spotPrice, setSpotPrice] = useState<number | null>(null);
  const [spotBasis, setSpotBasis] = useState<number | null>(null);

  const [signals, setSignals] = useState<Signal[]>([]);
  const [signalsToday, setSignalsToday] = useState<number>(0);
  const [maxSignalsPerDay, setMaxSignalsPerDay] = useState<number>(1);
  const [activeSignal, setActiveSignal] = useState<Signal | null>(null);
  const [selectedTimeframe, setSelectedTimeframe] = useState<string>('M15');
  const [inspectingSignal, setInspectingSignal] = useState<Signal | null>(null);
  const [newsData, setNewsData] = useState<NewsCalendarData | null>(null);
  const [marketContext, setMarketContext] = useState<MarketContext | null>(null);
  const [volumeDelta, setVolumeDelta] = useState<VolumeDeltaSnapshot | null>(null);
  const [divergenceAlerts, setDivergenceAlerts] = useState<DivergenceAlertsResponse>({
    enabled: false,
    timeframes: [],
    alerts: [],
  });
  const [liveView, setLiveView] = useState<LiveViewResponse | null>(null);
  const [macroFlow, setMacroFlow] = useState<MacroFlowResponse | null>(null);
  const [scenarios, setScenarios] = useState<ScenarioResponse | null>(null);

  const [config, setConfig] = useState<BotConfig>(DEFAULT_CONFIG);
  const [isConfigLoaded, setIsConfigLoaded] = useState(false);
  const [isLoading, setIsLoading] = useState(false);
  const [isTogglingScanner, setIsTogglingScanner] = useState(false);
  const [statusMessage, setStatusMessage] = useState<string | null>(null);

  // Fetch live closed candles, dynamic S/R and EMA trend.
  const loadCandles = useCallback(async () => {
    try {
      const data = await fetchCandles();
      if (Array.isArray(data.candles)) {
        setCandles(data.candles);
        setSupports(data.supports || []);
        setResistances(data.resistances || []);
        setVolumeMA(data.volumeMA || 1450);
        if (data.currentPrice) setCurrentPrice(data.currentPrice);
        if (typeof data.ema50 === 'number') setEma50(data.ema50);
        if (data.trendBias) setTrendBias(data.trendBias);
        if (data.dataSource) setDataSource(data.dataSource);
        if (data.volumeSource) setVolumeSource(data.volumeSource);
        setSpotPrice(typeof data.spotPrice === 'number' ? data.spotPrice : null);
        setSpotBasis(typeof data.spotBasis === 'number' ? data.spotBasis : null);
        setVolumeDelta(data.volumeDelta ?? null);
      }
    } catch (err) {
      console.error('Lỗi tải dữ liệu nến:', err);
    }
  }, []);

  // Fetch signal history + today's Sniper quota counters.
  const loadSignals = useCallback(async () => {
    try {
      const data = await fetchSignals();
      if (Array.isArray(data.signals)) {
        setSignals(data.signals);
        setSignalsToday(data.signalsToday ?? 0);
        setMaxSignalsPerDay(data.maxSignalsPerDay ?? 1);
        setActiveSignal((prev) => prev ?? data.signals[0] ?? null);
      }
    } catch (err) {
      console.error('Lỗi tải danh sách tín hiệu:', err);
    }
  }, []);

  // Fetch persisted bot configuration.
  const loadConfig = useCallback(async () => {
    try {
      const data = await fetchConfig();
      setConfig({ ...DEFAULT_CONFIG, ...data });
      if (data.timeframe) setSelectedTimeframe(data.timeframe);
      if (data.max_signals_per_day) setMaxSignalsPerDay(data.max_signals_per_day);
    } catch (err) {
      console.error('Lỗi tải cấu hình:', err);
    } finally {
      setIsConfigLoaded(true);
    }
  }, []);

  // Fetch ForexFactory calendar, Kill Switch state and EMA trend.
  const loadNewsCalendar = useCallback(async () => {
    try {
      const data = await fetchNewsCalendar();
      setNewsData(data);
      if (data.dataSource) setDataSource(data.dataSource);
      if (typeof data.ema50 === 'number') setEma50(data.ema50);
      if (data.effectiveBias) setTrendBias(data.effectiveBias);
      else if (data.trendBias) setTrendBias(data.trendBias);
    } catch (err) {
      console.error('Lỗi tải lịch tin kinh tế:', err);
    }
  }, []);

  // Fetch read-only D1/H4/H1 context for the "view toàn cảnh" panel.
  const loadMarketContext = useCallback(async () => {
    try {
      const data = await fetchMarketContext();
      setMarketContext(data);
    } catch (err) {
      console.error('Lỗi tải bối cảnh D1/H4/H1:', err);
    }
  }, []);

  // Fetch standalone RSI divergence alerts (advisory; no quota impact).
  const loadDivergenceAlerts = useCallback(async () => {
    try {
      const data = await fetchDivergenceAlerts();
      setDivergenceAlerts(data);
    } catch (err) {
      console.error('Lỗi tải cảnh báo phân kỳ RSI:', err);
    }
  }, []);

  // Fetch the read-only live setup preview (never sends Telegram or spends quota).
  const loadLiveView = useCallback(async () => {
    try {
      const data = await fetchLiveView();
      setLiveView(data);
    } catch (err) {
      console.error('Lỗi tải live view:', err);
    }
  }, []);

  // Fetch the read-only macro/flow snapshot (FRED/Yahoo/CFTC; no quota impact).
  const loadMacroFlow = useCallback(async () => {
    try {
      const data = await fetchMacroFlow();
      setMacroFlow(data);
    } catch (err) {
      console.error('Lỗi tải Macro & Flow:', err);
    }
  }, []);

  // Fetch the four read-only reference scenarios (never an entry confirmation).
  const loadScenarios = useCallback(async () => {
    try {
      const data = await fetchScenarios();
      setScenarios(data);
    } catch (err) {
      console.error('Lỗi tải kịch bản thị trường:', err);
    }
  }, []);

  useEffect(() => {
    void loadCandles();
    void loadSignals();
    void loadConfig();
    void loadNewsCalendar();
    void loadMarketContext();

    // Keep the desk in sync with the 60-second backend scanner.
    const timer = setInterval(() => {
      void loadCandles();
      void loadSignals();
      void loadNewsCalendar();
      void loadMarketContext();
    }, 60_000);

    return () => clearInterval(timer);
  }, [loadCandles, loadSignals, loadConfig, loadNewsCalendar, loadMarketContext]);

  // Divergence alerts refresh every 30 seconds, independent of trade signals.
  useEffect(() => {
    void loadDivergenceAlerts();
    const timer = setInterval(() => {
      void loadDivergenceAlerts();
    }, 30_000);

    return () => clearInterval(timer);
  }, [loadDivergenceAlerts]);

  // Live setup preview polls every 30s only while the scanner is active.
  // Pausing stops the polling; the endpoint itself stays read-only and callable.
  useEffect(() => {
    if (!isConfigLoaded || !config.is_active) return;

    void loadLiveView();
    const timer = setInterval(() => {
      void loadLiveView();
    }, 30_000);

    return () => clearInterval(timer);
  }, [isConfigLoaded, config.is_active, loadLiveView]);

  // Macro & Flow is independent of the scanner; refresh once a minute.
  useEffect(() => {
    void loadMacroFlow();
    const timer = setInterval(() => {
      void loadMacroFlow();
    }, 60_000);

    return () => clearInterval(timer);
  }, [loadMacroFlow]);

  // Reference scenarios are independent of the scanner and refresh once a minute.
  useEffect(() => {
    void loadScenarios();
    const timer = setInterval(() => {
      void loadScenarios();
    }, 60_000);

    return () => clearInterval(timer);
  }, [loadScenarios]);

  // Handle a manual simulation (never consumes the daily Sniper quota).
  const handleSimulate = async (type: SimulateSetupType) => {
    setIsLoading(true);
    setStatusMessage('🧪 Đang chạy simulation: quét Pinbar + Volume + S/R, không tốn quota...');
    try {
      const data = await simulateSetup(type);

      await Promise.all([loadCandles(), loadSignals(), loadNewsCalendar()]);

      if (data.status === 'signal_triggered' && data.signal) {
        setSignals((prev) => [data.signal as Signal, ...prev.filter((s) => s.id !== data.signal?.id)]);
        setActiveSignal(data.signal);
        setStatusMessage(
          `🧪 [SIMULATION - Không tốn Quota] Tín hiệu ${data.signal.signal_type} demo! R:R 1:${data.signal.risk_reward_ratio}`
        );
      } else {
        setStatusMessage(data.message || 'Nến mới đã thêm vào buffer. Chưa đủ điều kiện Pinbar + Volume.');
      }
    } catch (err) {
      setStatusMessage(`Lỗi simulation: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setIsLoading(false);
    }
  };

  // Persist config to `data/bot_state.json` via the backend.
  const handleSaveConfig = async (newConfig: BotConfig) => {
    try {
      const data = await saveConfig(newConfig);
      const savedConfig = data.config || newConfig;
      setConfig({ ...DEFAULT_CONFIG, ...savedConfig });
      if (savedConfig.timeframe) setSelectedTimeframe(savedConfig.timeframe);
      if (data.config?.max_signals_per_day) {
        setMaxSignalsPerDay(data.config.max_signals_per_day);
      }
      await Promise.all([loadSignals(), loadNewsCalendar()]);
    } catch (err) {
      console.error('Lỗi lưu cấu hình:', err);
    }
  };

  // Start/Stop the scanner via the dedicated scanner endpoints.
  // Start also sends the read-only reference scenario view to Telegram; neither
  // action inserts a trade signal or consumes the Sniper quota.
  const handleToggleScanner = async () => {
    setIsTogglingScanner(true);
    try {
      if (config.is_active) {
        await stopScanner();
        setConfig((prev) => ({ ...prev, is_active: false }));
        setStatusMessage('Scanner đã tắt');
      } else {
        const data = await startScanner();
        setConfig((prev) => ({ ...prev, is_active: true }));
        if (data.liveView) setLiveView(data.liveView);
        if (data.scenarios) setScenarios(data.scenarios);
        if (data.telegramSent) {
          setStatusMessage('Scanner started — scenario view sent to Telegram');
        } else {
          setStatusMessage(
            `Scanner started — chưa gửi được Telegram${
              data.telegramError ? `: ${data.telegramError}` : ''
            }`
          );
        }
      }
      await Promise.all([loadLiveView(), loadScenarios()]);
    } catch (err) {
      setStatusMessage(
        `Lỗi bật/tắt scanner: ${err instanceof Error ? err.message : String(err)}`
      );
    } finally {
      setIsTogglingScanner(false);
    }
  };

  // Manual "Gửi Telegram" on the Scenario View card. Reference snapshot only:
  // no quota, no signal persistence, no trade alert.
  const handleSendScenariosTelegram = async () => {
    try {
      const data = await sendScenariosTelegram();
      if (data.telegramSent) {
        return { success: true, message: data.message || 'Đã gửi Telegram' };
      }
      return { success: false, message: data.message || 'Chưa gửi được Telegram' };
    } catch (err) {
      return { success: false, message: err instanceof Error ? err.message : String(err) };
    }
  };

  // Persist the selected timeframe and reload candles from the backend feed.
  const handleTimeframeChange = async (timeframe: string) => {
    setSelectedTimeframe(timeframe);
    if (timeframe !== config.timeframe) {
      await handleSaveConfig({ ...config, timeframe });
    }
    await loadCandles();
  };

  // Test Telegram connection.
  const handleTestTelegram = async (token: string, chatId: string) => {
    try {
      const data = await testTelegram(token, chatId);
      if (data.status === 'success') {
        return { success: true, message: data.message };
      }
      return { success: false, message: data.message || 'Lỗi gửi tin nhắn' };
    } catch (err) {
      return { success: false, message: err instanceof Error ? err.message : String(err) };
    }
  };

  // Send a single signal alert to Telegram.
  const handleSendSignalToTelegram = async (signal: Signal) => {
    try {
      const data = await sendSignalTelegram(
        signal,
        config.telegram_bot_token,
        config.telegram_chat_id
      );
      if (data.status === 'success') {
        return { success: true, message: data.message };
      }
      return { success: false, message: data.message };
    } catch (err) {
      return { success: false, message: err instanceof Error ? err.message : String(err) };
    }
  };

  const quotaReached = signalsToday >= maxSignalsPerDay;

  return (
    <div className="min-h-screen bg-neutral-950 text-neutral-100 flex flex-col font-sans">
      {/* Top Bar Header with Sniper Quota & Free Feed badges */}
      <Header
        currentTab={currentTab}
        onTabChange={setCurrentTab}
        goldPrice={currentPrice}
        config={config}
        signalsToday={signalsToday}
        dataSource={dataSource}
        onOpenTestModal={() => setCurrentTab('config')}
      />

      {/* Main Content Area */}
      <main className="flex-1 max-w-7xl w-full mx-auto p-4 md:p-6 flex flex-col gap-5">
        {/* News Kill Switch & Macro Bias Banner */}
        <NewsKillSwitchBanner
          newsData={newsData}
          onToggleKillSwitch={() =>
            handleSaveConfig({ ...config, kill_switch_enabled: !config.kill_switch_enabled })
          }
        />

        {/* KPI, Sniper Quota & Free Live Feed Bar */}
        <MetricCards
          signals={signals}
          supports={supports}
          resistances={resistances}
          currentPrice={currentPrice}
          config={config}
          signalsToday={signalsToday}
          dataSource={dataSource}
          volumeSource={volumeSource}
          trendBias={trendBias}
          ema50={ema50}
          spotPrice={spotPrice}
          spotBasis={spotBasis}
        />

        {/* Tab 1: Live Trading Desk & Candlestick Monitor */}
        {currentTab === 'monitor' && (
          <div className="flex flex-col gap-6">
            {/* Live setup preview + Start/Stop scanner (read-only, quota-free) */}
            <LiveSetupCard
              config={config}
              liveView={liveView}
              isTogglingScanner={isTogglingScanner}
              onToggleScanner={() => {
                void handleToggleScanner();
              }}
              onRefresh={() => {
                void loadLiveView();
              }}
            />

            {/* Read-only macro/flow context (FRED/Yahoo/CFTC) */}
            <MacroFlowCard data={macroFlow} enabled={config.macro_flow_enabled ?? true} />

            {/* Read-only D1/H4/H1 "view toàn cảnh" context */}
            <MacroContextCard context={marketContext} volumeDelta={volumeDelta} />

            {/* 4 read-only reference scenarios — never an entry confirmation */}
            <ScenarioCard
              data={scenarios}
              onRefresh={() => {
                void loadScenarios();
              }}
              onSendTelegram={handleSendScenariosTelegram}
            />

            {/* Standalone RSI divergence alerts — advisory, never an entry */}
            <DivergenceAlertsCard
              context={marketContext}
              enabled={divergenceAlerts.enabled}
              timeframes={divergenceAlerts.timeframes}
              alerts={divergenceAlerts.alerts}
            />

            {/* Closed H4 candles — context only, never an entry trigger */}
            <HtfChartCard />

            {/* Manual Simulation Controls (explicitly quota-free) */}
            <SimulateToolbar
              onSimulate={handleSimulate}
              isLoading={isLoading}
              statusMessage={statusMessage}
            />

            {/* Split View: Candlestick Chart (2/3) + Real-time Signal Feed (1/3) */}
            <div className="grid grid-cols-1 lg:grid-cols-3 gap-6 items-start">
              {/* Candlestick & Volume Chart */}
              <div className="lg:col-span-2">
                <CandlestickChart
                  candles={candles}
                  supports={supports}
                  resistances={resistances}
                  volumeMA={volumeMA}
                  activeSignal={activeSignal}
                  selectedTimeframe={selectedTimeframe}
                  onTimeframeChange={handleTimeframeChange}
                />
              </div>

              {/* Live Signal Feed Panel */}
              <div className="flex flex-col gap-3">
                <div className="flex items-center justify-between pb-1">
                  <div className="flex items-center gap-2">
                    <span className="w-2 h-2 rounded-full bg-amber-400"></span>
                    <h3 className="text-sm font-bold text-white uppercase tracking-wider font-mono">
                      Sniper Signal Stream
                    </h3>
                  </div>
                  <span className="text-xs text-neutral-500 font-mono">
                    {signals.length} Setups
                  </span>
                </div>

                <div className="flex flex-col gap-3 max-h-[480px] overflow-y-auto pr-1">
                  {signals.length === 0 ? (
                    <div className="p-8 text-center text-xs text-neutral-500 bg-neutral-900 border border-neutral-800 rounded-xl">
                      Chưa có tín hiệu nào. Dùng các nút <span className="text-amber-400 font-mono">[SIMULATION]</span> phía trên để test nến Pinbar tại vùng hỗ trợ/kháng cự mà không tốn quota.
                    </div>
                  ) : (
                    signals.map((sig) => (
                      <SignalCard
                        key={sig.id}
                        signal={sig}
                        isSelected={activeSignal?.id === sig.id}
                        onSelect={(s) => setActiveSignal(s)}
                        onOpenDetails={(s) => setInspectingSignal(s)}
                      />
                    ))
                  )}
                </div>
              </div>
            </div>

            {/* Automated Position Sizing & Lot Calculator */}
            <PositionSizeWidget
              config={config}
              currentGoldPrice={currentPrice}
              onUpdateConfig={(partial) => handleSaveConfig({ ...config, ...partial })}
            />
          </div>
        )}

        {/* Tab 2: Full Signal Alerts Feed & Table */}
        {currentTab === 'signals' && (
          <div className="bg-neutral-900 border border-neutral-800 rounded-xl p-5 flex flex-col gap-4">
            <div className="flex flex-wrap items-center justify-between gap-4">
              <div>
                <h3 className="text-base font-bold text-white tracking-tight">
                  Lịch sử Tín hiệu & Trạng thái Bắn Alert Telegram
                </h3>
                <p className="text-xs text-neutral-400 mt-0.5">
                  Toàn bộ tín hiệu với Entry, Stop Loss 5–25$, Take Profit, R:R và cờ simulation (không tính quota).
                </p>
              </div>

              <div className="flex flex-wrap items-center gap-2 text-xs font-mono">
                <span
                  className={`px-2.5 py-1 rounded-md border ${
                    quotaReached
                      ? 'bg-amber-500/10 text-amber-300 border-amber-500/30'
                      : 'bg-emerald-500/10 text-emerald-400 border-emerald-500/20'
                  }`}
                >
                  🎯 Quota: {signalsToday}/{maxSignalsPerDay} lệnh hôm nay
                </span>
                <span className="px-2.5 py-1 bg-emerald-500/10 text-emerald-400 border border-emerald-500/20 rounded-md">
                  BUY: {signals.filter((s) => s.signal_type === 'BUY').length}
                </span>
                <span className="px-2.5 py-1 bg-rose-500/10 text-rose-400 border border-rose-500/20 rounded-md">
                  SELL: {signals.filter((s) => s.signal_type === 'SELL').length}
                </span>
              </div>
            </div>

            {/* Table */}
            <div className="overflow-x-auto">
              <table className="w-full text-left text-xs font-mono border-collapse">
                <thead>
                  <tr className="border-b border-neutral-800 text-neutral-400 bg-neutral-950/40">
                    <th className="py-3 px-3">Thời gian</th>
                    <th className="py-3 px-3">Lệnh</th>
                    <th className="py-3 px-3">Khung</th>
                    <th className="py-3 px-3 text-right">Entry</th>
                    <th className="py-3 px-3 text-right">Stop Loss</th>
                    <th className="py-3 px-3 text-right">TP1 (1:2)</th>
                    <th className="py-3 px-3 text-right">TP2 (1:3)</th>
                    <th className="py-3 px-3 text-right">R:R</th>
                    <th className="py-3 px-3 text-right">Volume</th>
                    <th className="py-3 px-3 text-center">Trạng thái</th>
                    <th className="py-3 px-3 text-right">Chi tiết</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-neutral-850">
                  {signals.map((sig) => (
                    <tr
                      key={sig.id}
                      className="hover:bg-neutral-850/50 transition-colors cursor-pointer"
                      onClick={() => setInspectingSignal(sig)}
                    >
                      <td className="py-3 px-3 text-neutral-400">
                        {new Date(sig.created_at).toLocaleTimeString([], {
                          hour: '2-digit',
                          minute: '2-digit',
                          second: '2-digit',
                        })}
                      </td>
                      <td className="py-3 px-3">
                        <span
                          className={`px-2 py-0.5 rounded font-bold ${
                            sig.signal_type === 'BUY'
                              ? 'bg-emerald-500/20 text-emerald-400'
                              : 'bg-rose-500/20 text-rose-400'
                          }`}
                        >
                          {sig.signal_type}
                        </span>
                        {sig.is_simulated && (
                          <span className="ml-1.5 px-1.5 py-0.5 rounded text-[9px] bg-amber-500/20 text-amber-300 border border-amber-500/30">
                            SIM
                          </span>
                        )}
                      </td>
                      <td className="py-3 px-3 text-neutral-300">{sig.timeframe}</td>
                      <td className="py-3 px-3 text-right text-white font-bold tabular-nums">
                        ${sig.entry_price.toFixed(2)}
                      </td>
                      <td className="py-3 px-3 text-right text-rose-400 tabular-nums">
                        ${sig.stop_loss.toFixed(2)}
                      </td>
                      <td className="py-3 px-3 text-right text-emerald-400 tabular-nums">
                        ${sig.take_profit_1.toFixed(2)}
                      </td>
                      <td className="py-3 px-3 text-right text-emerald-300 tabular-nums">
                        ${sig.take_profit_2.toFixed(2)}
                      </td>
                      <td className="py-3 px-3 text-right text-amber-400 font-bold tabular-nums">
                        1:{sig.risk_reward_ratio.toFixed(2)}
                      </td>
                      <td className="py-3 px-3 text-right text-neutral-300 tabular-nums">
                        {sig.volume_surge_ratio}x MA
                      </td>
                      <td className="py-3 px-3 text-center">
                        <span className="px-2 py-0.5 rounded text-[11px] bg-neutral-800 text-neutral-300 border border-neutral-700">
                          {sig.status}
                        </span>
                      </td>
                      <td className="py-3 px-3 text-right">
                        <button
                          onClick={(e) => {
                            e.stopPropagation();
                            setInspectingSignal(sig);
                          }}
                          className="text-amber-400 hover:text-amber-300 font-medium cursor-pointer"
                        >
                          Xem Alert
                        </button>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </div>
        )}

        {/* Tab 3: ForexFactory News, EMA Macro & TradingView Webhook */}
        {currentTab === 'webhook' && (
          <WebhookAndMacroHub
            config={config}
            newsData={newsData}
            onRefreshCalendar={loadNewsCalendar}
            onSignalCreated={() => {
              void loadSignals();
              void loadCandles();
            }}
          />
        )}

        {/* Tab 4: Sniper & Risk Settings */}
        {currentTab === 'config' && (
          <ConfigPanel
            config={config}
            onSaveConfig={handleSaveConfig}
            onTestTelegram={handleTestTelegram}
          />
        )}
      </main>

      <footer className="border-t border-neutral-800 bg-neutral-950/60 px-4 py-4 text-center text-[11px] leading-relaxed text-neutral-500">
        <p>
          <strong className="text-neutral-400">Risk disclaimer:</strong> Tín hiệu chỉ mang tính
          tham khảo, không phải lời khuyên đầu tư. Giao dịch vàng có đòn bẩy và rủi ro cao; bạn có
          thể mất toàn bộ vốn. Luôn tự kiểm chứng và chỉ sử dụng số vốn bạn chấp nhận mất.
        </p>
        <p className="mt-1 font-mono text-[10px] text-neutral-600">
          XAUUSD Bot · Sniper 1 signal/day · Not financial advice
        </p>
      </footer>

      {/* Detail Modal */}
      <SignalModal
        signal={inspectingSignal}
        onClose={() => setInspectingSignal(null)}
        onSendToTelegram={handleSendSignalToTelegram}
      />
    </div>
  );
}
