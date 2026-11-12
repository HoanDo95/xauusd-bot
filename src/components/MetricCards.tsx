'use client';

import React from 'react';
import { BotConfig, Signal, SupportResistanceZone, TrendBias } from '../types';

interface Props {
  signals: Signal[];
  supports: SupportResistanceZone[];
  resistances: SupportResistanceZone[];
  currentPrice: number;
  config: BotConfig;
  signalsToday: number;
  dataSource?: string;
  volumeSource?: string;
  trendBias?: TrendBias;
  ema50?: number;
  spotPrice?: number | null;
  spotBasis?: number | null;
}

function feedLabel(dataSource?: string, volumeSource?: string): string {
  if (dataSource === 'TWELVEDATA') {
    if (volumeSource === 'BINANCE_PAXG_PROXY') return 'TwelveData XAU/USD + PAXG volume';
    if (volumeSource === 'NONE') return 'TwelveData XAU/USD (no volume)';
    return 'TwelveData XAU/USD (Free)';
  }
  if (dataSource === 'FALLBACK_SEED' || dataSource === 'OFFLINE') {
    return 'Offline Fallback (Seed)';
  }
  return 'Binance PAXG/USDT (Free 24/7)';
}

export const MetricCards: React.FC<Props> = ({
  signals,
  supports,
  resistances,
  currentPrice,
  config,
  signalsToday,
  dataSource,
  volumeSource,
  trendBias,
  ema50,
  spotPrice,
  spotBasis,
}) => {
  // Statistics calculations — only REAL signals count toward performance.
  const liveSignals = signals.filter((s) => !s.is_simulated);
  const closedStatuses = ['Target1Hit', 'Target2Hit', 'StopLossHit', 'Cancelled'];
  const closedSignals = liveSignals.filter((s) => closedStatuses.includes(s.status));
  const wins = liveSignals.filter(
    (s) => s.status === 'Target1Hit' || s.status === 'Target2Hit',
  ).length;
  const losses = liveSignals.filter((s) => s.status === 'StopLossHit').length;
  const closed = closedSignals.length;
  const winRate = closed > 0 ? ((wins / closed) * 100).toFixed(1) : '—';
  const activeCount = liveSignals.filter((s) => s.status === 'Active').length;

  const rMultiple = (s: Signal): number => {
    const risk = Math.abs(s.entry_price - s.stop_loss);
    if (risk <= 0) return 0;
    if (s.status === 'Target1Hit') return Math.abs(s.take_profit_1 - s.entry_price) / risk;
    if (s.status === 'Target2Hit') return Math.abs(s.take_profit_2 - s.entry_price) / risk;
    if (s.status === 'StopLossHit') return -1;
    return 0;
  };
  const avgR =
    closed > 0
      ? (closedSignals.reduce((acc, s) => acc + rMultiple(s), 0) / closed).toFixed(2)
      : '—';

  const quotaEnabled = config.daily_quota_enabled ?? false;
  const maxSignals = Math.max(1, config.max_signals_per_day || 1);
  const quotaReached = quotaEnabled && signalsToday >= maxSignals;

  const nearestSupport = supports.length ? supports[0].price : currentPrice - 8;
  const nearestResistance = resistances.length ? resistances[0].price : currentPrice + 8;

  const trendColor =
    trendBias === 'BULLISH'
      ? 'text-emerald-400'
      : trendBias === 'BEARISH'
        ? 'text-rose-400'
        : 'text-sky-400';

  return (
    <div className="flex flex-col gap-3">
      {/* Prominent Sniper Quota & Free Live Feed Badges */}
      <div className="flex flex-wrap items-center gap-2.5">
        <div
          className={`flex items-center gap-2 px-3.5 py-2 rounded-xl border text-xs font-mono font-bold shadow-sm ${
            quotaReached
              ? 'bg-amber-500/10 text-amber-300 border-amber-500/40 shadow-amber-500/5'
              : 'bg-emerald-500/10 text-emerald-400 border-emerald-500/40 shadow-emerald-500/5'
          }`}
        >
          <span className="text-base leading-none">🎯</span>
          <span>
            {quotaEnabled
              ? `Quota Hôm Nay: ${signalsToday}/${maxSignals} Lệnh`
              : 'Quota: Không giới hạn'}
          </span>
          {quotaEnabled && (
            <span
              className={`px-1.5 py-0.5 rounded text-[10px] uppercase tracking-wider ${
                quotaReached ? 'bg-amber-500/20 text-amber-300' : 'bg-emerald-500/20 text-emerald-300'
              }`}
            >
              {quotaReached ? 'Quota Reached' : 'Ready'}
            </span>
          )}
        </div>

        <div className="flex items-center gap-2 px-3.5 py-2 rounded-xl border border-sky-500/40 bg-sky-500/10 text-sky-300 text-xs font-mono font-semibold shadow-sm shadow-sky-500/5">
          <span className="text-base leading-none">📡</span>
          <span>
            Live Feed: {feedLabel(dataSource ?? config.data_source, volumeSource)}
          </span>
        </div>

        {typeof ema50 === 'number' && ema50 > 0 && (
          <div className="flex items-center gap-2 px-3.5 py-2 rounded-xl border border-neutral-800 bg-neutral-900 text-neutral-300 text-xs font-mono">
            <span>EMA50:</span>
            <span className="text-white font-bold tabular-nums">${ema50.toFixed(2)}</span>
            <span className={`font-bold ${trendColor}`}>{trendBias ?? 'NEUTRAL'}</span>
          </div>
        )}

        {typeof spotBasis === 'number' && (
          <div
            className="flex items-center gap-2 px-3.5 py-2 rounded-xl border border-violet-500/40 bg-violet-500/10 text-violet-300 text-xs font-mono"
            title="PAXG (Binance) đã được căn về giá spot XAU/USD tham chiếu; cache 300 giây."
          >
            <span className="text-base leading-none">🌐</span>
            <span>
              Spot basis:{' '}
              <span className="font-bold tabular-nums">
                {spotBasis >= 0 ? '+' : '-'}${Math.abs(spotBasis).toFixed(2)}
              </span>
            </span>
            {typeof spotPrice === 'number' && (
              <span className="text-violet-400/80 tabular-nums">spot ${spotPrice.toFixed(2)}</span>
            )}
          </div>
        )}
      </div>

      <div className="grid grid-cols-2 md:grid-cols-4 gap-3">
        {/* Metric 1: Win Rate & Completed Trades */}
        <div className="bg-neutral-900 border border-neutral-800 rounded-xl p-3.5">
          <div className="text-xs text-neutral-400 font-medium">Live Win Rate (Closed Signals)</div>
          <div className="flex items-baseline gap-2 mt-1">
            <span className="text-2xl font-bold font-mono text-emerald-400 tabular-nums">
              {winRate}{winRate !== '—' ? '%' : ''}
            </span>
            <span className="text-xs text-neutral-500 font-mono">
              {wins}W · {losses}L · {closed} closed
            </span>
          </div>
          <div className="text-xs text-neutral-500 mt-1">
            Avg R: <span className="text-amber-400 font-mono">{avgR}</span> · Simulation excluded
          </div>
        </div>

        {/* Metric 2: Active Signals / Queue */}
        <div className="bg-neutral-900 border border-neutral-800 rounded-xl p-3.5">
          <div className="text-xs text-neutral-400 font-medium">Active Setups in Play</div>
          <div className="flex items-baseline gap-2 mt-1">
            <span className="text-2xl font-bold font-mono text-amber-400 tabular-nums">
              {activeCount}
            </span>
            <span className="text-xs text-neutral-500 font-mono">
              / {liveSignals.length} Live Signals
            </span>
          </div>
          <div className="text-xs text-neutral-500 mt-1">
            Sniper scanner · M15 & H1 · {feedLabel(dataSource ?? config.data_source, volumeSource)}
          </div>
        </div>

        {/* Metric 3: Dynamic Support & Resistance Range */}
        <div className="bg-neutral-900 border border-neutral-800 rounded-xl p-3.5">
          <div className="text-xs text-neutral-400 font-medium">Dynamic S/R Liquidity Channel</div>
          <div className="flex items-baseline gap-2 mt-1">
            <span className="text-sm font-semibold font-mono text-emerald-400 tabular-nums">
              ${nearestSupport.toFixed(1)}
            </span>
            <span className="text-neutral-600 text-xs">—</span>
            <span className="text-sm font-semibold font-mono text-rose-400 tabular-nums">
              ${nearestResistance.toFixed(1)}
            </span>
          </div>
          <div className="text-xs text-neutral-500 mt-1">
            Pivot range: ${(nearestResistance - nearestSupport).toFixed(1)} spread
          </div>
        </div>

        {/* Metric 4: Daily Sniper Limit & SL Buffer */}
        <div className="bg-neutral-900 border border-neutral-800 rounded-xl p-3.5">
          <div className="text-xs text-neutral-400 font-medium">Sniper Limit & SL Buffer</div>
          <div className="flex items-baseline gap-2 mt-1">
            <span className={`text-2xl font-bold font-mono tabular-nums ${quotaReached ? 'text-amber-400' : 'text-emerald-400'}`}>
              {quotaEnabled ? maxSignals : '∞'}
            </span>
            <span className="text-xs text-neutral-500 font-mono">
              {quotaEnabled ? 'lệnh / ngày' : 'không giới hạn'}
            </span>
            <span className="ml-auto text-xs font-bold font-mono text-amber-400 tabular-nums">
              {config.gold_sl_buffer_points.toFixed(1)}$
            </span>
          </div>
          <div className="text-xs text-neutral-500 mt-1">
            SL 5–25$ · M15 8–10$ · H1 15–20$ · Lot tự động
          </div>
        </div>
      </div>
    </div>
  );
};
