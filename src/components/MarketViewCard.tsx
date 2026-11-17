'use client';

import React from 'react';
import { MarketOutlook, OutlookScenario } from '../types';

interface Props {
  data: MarketOutlook | null;
}

function price(value: number | null | undefined): string {
  if (typeof value !== 'number' || !Number.isFinite(value)) return '—';
  return `$${value.toFixed(2)}`;
}

function level(value: number | null | undefined): string {
  if (typeof value !== 'number' || !Number.isFinite(value)) return '—';
  return value.toFixed(2);
}

function timeLabel(value?: string): string {
  if (!value) return '—';
  const parsed = new Date(value);
  if (Number.isNaN(parsed.getTime())) return '—';
  return parsed.toLocaleTimeString('vi-VN', {
    hour: '2-digit',
    minute: '2-digit',
  });
}

function stateTone(state: string): string {
  switch (state) {
    case 'NO_TRADE':
      return 'border-rose-500/40 bg-rose-500/10 text-rose-200';
    case 'SNIPER_TRIGGER':
      return 'border-amber-500/40 bg-amber-500/10 text-amber-200';
    case 'MARKET_VIEW':
      return 'border-emerald-500/40 bg-emerald-500/10 text-emerald-200';
    default:
      return 'border-neutral-700 bg-neutral-800 text-neutral-300';
  }
}

function actionLabel(action: string): string {
  switch (action) {
    case 'WATCH_SELL':
      return 'CANH SELL';
    case 'WATCH_BUY':
      return 'CANH BUY';
    case 'STAND_ASIDE':
      return 'ĐỨNG NGOÀI';
    default:
      return action;
  }
}

function ScenarioBlock({
  label,
  scenario,
  tone,
}: {
  label: string;
  scenario: OutlookScenario;
  tone: 'primary' | 'alternative';
}) {
  const border =
    tone === 'primary'
      ? 'border-emerald-500/30 bg-emerald-500/5'
      : 'border-amber-500/30 bg-amber-500/5';
  const actionTone =
    scenario.action === 'WATCH_SELL'
      ? 'text-rose-300'
      : scenario.action === 'WATCH_BUY'
        ? 'text-emerald-300'
        : 'text-neutral-300';
  return (
    <div className={`rounded-xl border ${border} p-4 flex flex-col gap-3`}>
      <div className="flex flex-wrap items-center justify-between gap-2">
        <span className="text-[10px] font-mono font-bold uppercase tracking-wider text-neutral-400">
          {label}
        </span>
        <span className={`text-xs font-black font-mono tracking-wider ${actionTone}`}>
          {actionLabel(scenario.action)}
        </span>
      </div>
      <div className="flex items-baseline gap-2">
        <span className="text-[10px] uppercase tracking-wider text-neutral-500">Zone</span>
        <span className="text-lg font-black font-mono text-white tabular-nums">
          {level(scenario.zone_low)} – {level(scenario.zone_high)}
        </span>
      </div>
      <p className="text-[11px] text-neutral-300 leading-relaxed">{scenario.trigger}</p>
      <div className="grid grid-cols-3 gap-2 text-[10px]">
        <div>
          <div className="text-neutral-500 uppercase tracking-wider">SL</div>
          <div className="font-mono font-bold text-rose-300">{level(scenario.stop_loss)}</div>
        </div>
        <div>
          <div className="text-neutral-500 uppercase tracking-wider">TP1</div>
          <div className="font-mono font-bold text-emerald-300">
            {level(scenario.take_profit_1)}
          </div>
        </div>
        <div>
          <div className="text-neutral-500 uppercase tracking-wider">TP2</div>
          <div className="font-mono font-bold text-emerald-300">
            {level(scenario.take_profit_2)}
          </div>
        </div>
      </div>
      <div className="flex flex-wrap items-center justify-between gap-2 text-[10px] text-neutral-500">
        <span>
          Confidence <span className="font-mono text-neutral-300">{scenario.confidence}%</span>
        </span>
        <span className="text-right">
          Invalidation: <span className="text-neutral-300">{scenario.invalidation}</span>
        </span>
      </div>
    </div>
  );
}

/**
 * Always-available H1 Market View. Read-only: never an entry, never a signal.
 * The primary/alternative zones come from the deterministic scenario builder.
 */
export const MarketViewCard: React.FC<Props> = ({ data }) => {
  if (!data) {
    return (
      <div className="bg-neutral-900 border border-neutral-800 rounded-xl p-4 flex flex-col gap-2">
        <h3 className="text-sm font-bold text-white tracking-tight uppercase font-mono">
          🧭 Market View — luôn có kịch bản
        </h3>
        <p className="text-xs text-neutral-500 font-mono">Đang tải market view...</p>
      </div>
    );
  }

  return (
    <div className="bg-neutral-900 border border-neutral-800 rounded-xl p-4 flex flex-col gap-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="flex flex-col gap-1">
          <h3 className="text-sm font-bold text-white tracking-tight uppercase font-mono">
            🧭 Market View — luôn có kịch bản
          </h3>
          <p className="text-[11px] text-neutral-500">
            H1 outlook · levels từ scenario builder · không phải entry
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <span
            className={`px-2.5 py-1 rounded-md border text-[10px] font-mono font-black tracking-wider ${stateTone(
              data.bot_state
            )}`}
          >
            {data.bot_state}
          </span>
          <span className="text-[10px] text-neutral-500 font-mono">
            {timeLabel(data.generated_at)} → {timeLabel(data.valid_until)}
          </span>
        </div>
      </div>

      {data.no_trade_reason ? (
        <div className="rounded-lg border border-rose-500/40 bg-rose-500/10 px-3 py-2 text-xs text-rose-200">
          ⛔ <strong>NO_TRADE:</strong> {data.no_trade_reason}
        </div>
      ) : null}

      <div className="flex flex-wrap items-center gap-2 text-[10px] font-mono text-neutral-400">
        <span className="px-2 py-0.5 rounded border border-neutral-700 bg-neutral-800 text-neutral-200">
          {data.structure}
        </span>
        <span>D1 {data.d1_bias}</span>
        <span>·</span>
        <span>H4 {data.h4_bias}</span>
        <span>·</span>
        <span>H1 {data.h1_bias}</span>
        <span>·</span>
        <span>Giá {price(data.price)}</span>
        <span>·</span>
        <span>H4 ATR {level(data.h4_atr)}</span>
        <span>·</span>
        <span>Session {data.session}</span>
        {data.spread_proxy_atr != null ? (
          <>
            <span>·</span>
            <span>Spread proxy {data.spread_proxy_atr.toFixed(2)}×</span>
          </>
        ) : null}
      </div>

      <p className="text-xs text-neutral-200 leading-relaxed">{data.market_view}</p>

      <div className="grid grid-cols-1 lg:grid-cols-2 gap-3">
        {data.primary ? (
          <ScenarioBlock label="🎯 Primary" scenario={data.primary} tone="primary" />
        ) : (
          <div className="rounded-xl border border-neutral-800 bg-neutral-950 p-4 text-xs text-neutral-500">
            🎯 Chưa có PRIMARY hợp lệ — đứng ngoài, chờ cấu trúc mới.
          </div>
        )}
        {data.alternative ? (
          <ScenarioBlock label="🔄 Alternative" scenario={data.alternative} tone="alternative" />
        ) : (
          <div className="rounded-xl border border-neutral-800 bg-neutral-950 p-4 text-xs text-neutral-500">
            🔄 Chưa có ALTERNATIVE — chỉ theo dõi PRIMARY.
          </div>
        )}
      </div>

      <div className="rounded-lg border border-neutral-800 bg-neutral-950 px-3 py-2">
        <div className="text-[10px] font-mono font-bold uppercase tracking-wider text-neutral-500 mb-1">
          ⚠️ Risk notes
        </div>
        <ul className="flex flex-col gap-1">
          {data.risk_notes.map((note, index) => (
            <li key={`${index}-${note}`} className="text-[11px] text-neutral-400">
              • {note}
            </li>
          ))}
        </ul>
      </div>

      <div className="text-[10px] text-neutral-500 font-mono uppercase tracking-wider">
        Kế hoạch tham khảo — chưa phải entry. Bot không tự vào lệnh.
      </div>
    </div>
  );
};
