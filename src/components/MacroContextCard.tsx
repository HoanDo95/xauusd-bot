'use client';

import React from 'react';
import { MarketContext, RsiSnapshot, TimeframeContext, VolumeDeltaSnapshot } from '../types';

interface Props {
  context: MarketContext | null;
  volumeDelta?: VolumeDeltaSnapshot | null;
}

function price(value: number | null | undefined): string {
  if (typeof value !== 'number' || !Number.isFinite(value) || value <= 0) return '—';
  return `$${value.toFixed(2)}`;
}

function room(close: number, level: number | null, side: 'support' | 'resistance'): string {
  if (typeof level !== 'number' || !Number.isFinite(level) || close <= 0) return '—';
  const distance = side === 'support' ? close - level : level - close;
  if (distance < 0) return '—';
  return `$${distance.toFixed(2)}`;
}

function biasClass(bias: string): string {
  switch (bias) {
    case 'BULLISH':
      return 'bg-emerald-500/15 text-emerald-300 border-emerald-500/40';
    case 'BEARISH':
      return 'bg-rose-500/15 text-rose-300 border-rose-500/40';
    default:
      return 'bg-sky-500/15 text-sky-300 border-sky-500/40';
  }
}

function channelColor(position: number): string {
  if (position >= 0.67) return 'bg-rose-400';
  if (position <= 0.33) return 'bg-emerald-400';
  return 'bg-amber-400';
}

function timeframeLabel(context: TimeframeContext): string {
  return context.timeframe || '—';
}

function deltaColor(value: number): string {
  if (value > 0) return 'text-emerald-400';
  if (value < 0) return 'text-rose-400';
  return 'text-neutral-400';
}

function signed(value: number, digits: number): string {
  if (!Number.isFinite(value)) return '—';
  return `${value >= 0 ? '+' : ''}${value.toFixed(digits)}`;
}

const DeltaBadge: React.FC<{ delta?: VolumeDeltaSnapshot | null }> = ({ delta }) => {
  if (!delta) return null;
  const ratio = Number.isFinite(delta.delta_ratio) ? delta.delta_ratio : 0;

  return (
    <div className="flex flex-wrap items-center gap-2 rounded-lg border border-neutral-800 bg-neutral-950/60 px-3 py-2 text-[10px] font-mono">
      <span className="text-neutral-500 uppercase tracking-wider">M15 Volume Delta</span>
      <span className={`px-1.5 py-0.5 rounded bg-neutral-900 border border-neutral-800 tabular-nums ${deltaColor(delta.delta)}`}>
        Delta {signed(delta.delta, 0)}
      </span>
      <span className={`px-1.5 py-0.5 rounded bg-neutral-900 border border-neutral-800 tabular-nums ${deltaColor(ratio)}`}>
        Ratio {signed(ratio, 2)}
      </span>
      <span className={`px-1.5 py-0.5 rounded bg-neutral-900 border border-neutral-800 tabular-nums ${deltaColor(delta.cvd)}`}>
        CVD {signed(delta.cvd, 0)}
      </span>
      <span className="text-neutral-600">taker buy − sell · lookback window</span>
    </div>
  );
};

function rsiStateClass(state: string | null | undefined): string {
  switch ((state || '').toUpperCase()) {
    case 'OVERSOLD':
      return 'bg-emerald-500/15 text-emerald-300 border-emerald-500/40';
    case 'OVERBOUGHT':
      return 'bg-rose-500/15 text-rose-300 border-rose-500/40';
    default:
      return 'bg-neutral-800 text-neutral-300 border-neutral-700';
  }
}

function rsiDivergenceClass(divergence: string | null | undefined): string {
  switch ((divergence || '').toUpperCase()) {
    case 'BULLISH':
      return 'bg-emerald-500/10 text-emerald-300 border-emerald-500/30';
    case 'BEARISH':
      return 'bg-rose-500/10 text-rose-300 border-rose-500/30';
    default:
      return 'bg-neutral-900 text-neutral-500 border-neutral-800';
  }
}

const RsiCell: React.FC<{ label: string; snapshot?: RsiSnapshot | null }> = ({
  label,
  snapshot,
}) => {
  const value =
    typeof snapshot?.value === 'number' && Number.isFinite(snapshot.value)
      ? snapshot.value.toFixed(1)
      : 'n/a';
  const state = snapshot?.state || 'n/a';
  const divergence = snapshot?.divergence || 'None';
  const hasDivergence = divergence !== 'None' && divergence !== 'n/a';

  return (
    <div className="flex flex-col gap-1.5 rounded-lg border border-neutral-800 bg-neutral-900/80 px-2.5 py-2">
      <div className="flex items-center justify-between gap-1">
        <span className="text-[9px] font-mono uppercase tracking-wider text-neutral-500">{label}</span>
        <span className={`px-1.5 py-0.5 rounded text-[9px] font-mono font-bold border ${rsiStateClass(state)}`}>
          {state}
        </span>
      </div>
      <span className="text-base font-bold font-mono text-white tabular-nums leading-none">
        {value}
      </span>
      <span
        className={`px-1.5 py-0.5 rounded text-[9px] font-mono border text-center truncate ${rsiDivergenceClass(
          divergence
        )}`}
        title={hasDivergence ? `${divergence} divergence` : 'No divergence'}
      >
        {hasDivergence ? `${divergence} Divergence` : 'No divergence'}
      </span>
    </div>
  );
};

const RsiPanel: React.FC<{
  m15?: RsiSnapshot | null;
  h1?: RsiSnapshot | null;
  h4?: RsiSnapshot | null;
}> = ({ m15, h1, h4 }) => (
  <div className="rounded-lg border border-neutral-800 bg-neutral-950/60 px-3 py-2.5 flex flex-col gap-2">
    <div className="flex flex-wrap items-center justify-between gap-2 text-[10px] font-mono">
      <span className="text-neutral-400 uppercase tracking-wider">📐 RSI Wilder (context)</span>
      <span className="text-neutral-600">M15 trigger · H1/H4 fixed 70/30 · advisory</span>
    </div>
    <div className="grid grid-cols-3 gap-2">
      <RsiCell label="M15" snapshot={m15} />
      <RsiCell label="H1" snapshot={h1} />
      <RsiCell label="H4" snapshot={h4} />
    </div>
  </div>
);

const TimeframePanel: React.FC<{ context: TimeframeContext; highlight?: boolean }> = ({
  context,
  highlight = false,
}) => {
  const position = Math.min(1, Math.max(0, Number.isFinite(context.channel_position) ? context.channel_position : 0.5));
  const percent = Math.round(position * 100);

  return (
    <div
      className={`rounded-lg border p-3.5 flex flex-col gap-3 ${
        highlight
          ? 'border-amber-500/50 bg-amber-500/[0.04] ring-1 ring-amber-500/20'
          : 'border-neutral-800 bg-neutral-950/40'
      }`}
    >
      <div className="flex items-center justify-between gap-2">
        <div className="flex items-center gap-2">
          <span className="text-sm font-bold font-mono text-white tracking-wider">
            {timeframeLabel(context)}
          </span>
          {highlight && (
            <span className="px-1.5 py-0.5 rounded text-[9px] font-mono font-bold uppercase tracking-wider bg-amber-500/20 text-amber-300 border border-amber-500/30">
              H4 filter
            </span>
          )}
        </div>
        <span className={`px-2 py-0.5 rounded text-[10px] font-mono font-bold border ${biasClass(context.bias)}`}>
          {context.bias}
        </span>
      </div>

      <div className="flex items-baseline justify-between gap-2">
        <span className="text-lg font-bold font-mono text-white tabular-nums">
          {price(context.close)}
        </span>
        <span className="text-[10px] text-neutral-500 font-mono">{context.candles} nến</span>
      </div>

      <div className="grid grid-cols-2 gap-2 text-[11px] font-mono">
        <div className="flex flex-col">
          <span className="text-neutral-500">EMA50</span>
          <span className="text-neutral-200 tabular-nums">{price(context.ema50)}</span>
        </div>
        <div className="flex flex-col">
          <span className="text-neutral-500">EMA200</span>
          <span className="text-neutral-200 tabular-nums">{price(context.ema200)}</span>
        </div>
        <div className="flex flex-col">
          <span className="text-neutral-500">ATR14</span>
          <span className="text-neutral-200 tabular-nums">{price(context.atr14)}</span>
        </div>
        <div className="flex flex-col">
          <span className="text-neutral-500">Vị trí kênh</span>
          <span className="text-neutral-200 tabular-nums">{percent}%</span>
        </div>
      </div>

      <div className="flex flex-col gap-1">
        <div className="h-1.5 w-full rounded-full bg-neutral-800 overflow-hidden">
          <div
            className={`h-full rounded-full ${channelColor(position)}`}
            style={{ width: `${Math.max(2, percent)}%` }}
          />
        </div>
        <div className="flex justify-between text-[9px] font-mono text-neutral-600">
          <span>Đáy kênh</span>
          <span>Đỉnh kênh</span>
        </div>
      </div>

      <div className="grid grid-cols-2 gap-2 text-[11px] font-mono border-t border-neutral-800/80 pt-2">
        <div className="flex flex-col">
          <span className="text-neutral-500">Hỗ trợ gần</span>
          <span className="text-emerald-400 tabular-nums">{price(context.nearest_support)}</span>
          <span className="text-[9px] text-neutral-600">
            room {room(context.close, context.nearest_support, 'support')}
          </span>
        </div>
        <div className="flex flex-col">
          <span className="text-neutral-500">Kháng cự gần</span>
          <span className="text-rose-400 tabular-nums">{price(context.nearest_resistance)}</span>
          <span className="text-[9px] text-neutral-600">
            room {room(context.close, context.nearest_resistance, 'resistance')}
          </span>
        </div>
      </div>
    </div>
  );
};

/**
 * "View toàn cảnh" — D1/H4/H1 context. Read-only and advisory: it never opens
 * a trade on its own; the H4 filter is opt-in via ConfigPanel (default OFF).
 */
export const MacroContextCard: React.FC<Props> = ({ context, volumeDelta }) => {
  if (!context) {
    return (
      <div className="bg-neutral-900 border border-neutral-800 rounded-xl p-4 flex flex-col gap-3">
        <div className="text-sm font-bold text-white tracking-tight">
          View toàn cảnh — không tự vào lệnh
        </div>
        <DeltaBadge delta={volumeDelta} />
        <p className="text-xs text-neutral-500 font-mono">
          Đang chờ bối cảnh D1/H4/H1 từ Rust engine (204). Dữ liệu sẽ xuất hiện sau khi feed HTF
          sẵn sàng.
        </p>
      </div>
    );
  }

  const updated = new Date(context.updated_at);
  const updatedLabel = Number.isNaN(updated.getTime())
    ? context.updated_at
    : updated.toLocaleTimeString('vi-VN', { hour: '2-digit', minute: '2-digit', second: '2-digit' });

  return (
    <div className="bg-neutral-900 border border-neutral-800 rounded-xl p-4 flex flex-col gap-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="flex flex-col gap-0.5">
          <div className="flex flex-wrap items-center gap-2">
            <h3 className="text-sm font-bold text-white tracking-tight uppercase font-mono">
              View toàn cảnh — không tự vào lệnh
            </h3>
            <span className="px-1.5 py-0.5 rounded text-[9px] font-mono uppercase tracking-wider bg-neutral-800 text-neutral-400 border border-neutral-700">
              Read-only
            </span>
          </div>
          <p className="text-[11px] text-neutral-500">
            D1/H4/H1: xu hướng EMA50/200, kênh Donchian(20), ATR14 và vùng S/R gần nhất. H4 chỉ
            chặn tín hiệu khi bật cờ trong Cấu hình.
          </p>
        </div>

        <div className="flex flex-wrap items-center gap-2 text-[10px] font-mono">
          <span className="px-2 py-1 rounded-md bg-neutral-950 border border-neutral-800 text-neutral-400">
            {context.price_source}
          </span>
          {typeof context.spot_basis === 'number' && (
            <span className="px-2 py-1 rounded-md bg-violet-500/10 border border-violet-500/30 text-violet-300 tabular-nums">
              basis {context.spot_basis >= 0 ? '+' : '-'}${Math.abs(context.spot_basis).toFixed(2)}
            </span>
          )}
          <span className="text-neutral-600">cập nhật {updatedLabel}</span>
        </div>
      </div>

      <DeltaBadge delta={volumeDelta} />

      <RsiPanel m15={context.rsi_m15} h1={context.rsi_h1} h4={context.rsi_h4} />

      <div className="grid grid-cols-1 md:grid-cols-3 gap-3">
        <TimeframePanel context={context.d1} />
        <TimeframePanel context={context.h4} highlight />
        <TimeframePanel context={context.h1} />
      </div>
    </div>
  );
};
