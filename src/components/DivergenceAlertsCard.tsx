'use client';

import React from 'react';
import { DivergenceAlert, MarketContext, RsiSnapshot } from '../types';

interface Props {
  context: MarketContext | null;
  enabled: boolean;
  timeframes: string[];
  alerts: DivergenceAlert[];
}

function rsiValue(snapshot?: RsiSnapshot | null): string {
  if (typeof snapshot?.value !== 'number' || !Number.isFinite(snapshot.value)) return 'n/a';
  return snapshot.value.toFixed(1);
}

function divergenceClass(divergence?: string | null): string {
  switch ((divergence || '').toUpperCase()) {
    case 'BULLISH':
      return 'bg-emerald-500/15 text-emerald-300 border-emerald-500/40';
    case 'BEARISH':
      return 'bg-rose-500/15 text-rose-300 border-rose-500/40';
    default:
      return 'bg-neutral-950 text-neutral-500 border-neutral-800';
  }
}

function directionClass(direction?: string | null): string {
  return (direction || '').toUpperCase() === 'BULLISH'
    ? 'bg-emerald-500/15 text-emerald-300 border-emerald-500/40'
    : 'bg-rose-500/15 text-rose-300 border-rose-500/40';
}

function detectedLabel(value: string): string {
  const parsed = new Date(value);
  if (Number.isNaN(parsed.getTime())) return value;
  return parsed.toLocaleTimeString('vi-VN', {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  });
}

const TimeframeState: React.FC<{
  label: string;
  snapshot?: RsiSnapshot | null;
  active: boolean;
}> = ({ label, snapshot, active }) => {
  const divergence = snapshot?.divergence || 'None';
  const hasDivergence = divergence !== 'None';

  return (
    <div
      className={`flex flex-col gap-1.5 rounded-lg border px-2.5 py-2 ${
        active ? 'border-neutral-800 bg-neutral-950/60' : 'border-neutral-800/60 bg-neutral-950/30'
      }`}
    >
      <div className="flex items-center justify-between gap-1">
        <span className="text-[9px] font-mono uppercase tracking-wider text-neutral-500">
          {label}
        </span>
        <span
          className={`px-1.5 py-0.5 rounded text-[8px] font-mono font-bold uppercase border ${
            active
              ? 'bg-amber-500/10 text-amber-300 border-amber-500/30'
              : 'bg-neutral-900 text-neutral-600 border-neutral-800'
          }`}
        >
          {active ? 'ON' : 'OFF'}
        </span>
      </div>
      <div className="flex items-baseline justify-between gap-1">
        <span className="text-base font-bold font-mono text-white tabular-nums leading-none">
          {rsiValue(snapshot)}
        </span>
        <span className="text-[9px] font-mono text-neutral-500">{snapshot?.state || 'n/a'}</span>
      </div>
      <span
        className={`px-1.5 py-0.5 rounded text-[9px] font-mono border text-center truncate ${divergenceClass(
          divergence
        )}`}
      >
        {hasDivergence ? `${divergence} Divergence` : 'No divergence'}
      </span>
    </div>
  );
};

/**
 * Standalone RSI divergence monitor. Advisory momentum warnings only —
 * it never shows entry/SL/TP and never consumes the Sniper quota.
 */
export const DivergenceAlertsCard: React.FC<Props> = ({
  context,
  enabled,
  timeframes,
  alerts,
}) => {
  const isActive = (timeframe: string) => enabled && timeframes.includes(timeframe);

  return (
    <div className="bg-neutral-900 border border-neutral-800 rounded-xl p-4 flex flex-col gap-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="flex flex-col gap-0.5">
          <div className="flex flex-wrap items-center gap-2">
            <h3 className="text-sm font-bold text-white tracking-tight uppercase font-mono">
              Cảnh báo phân kỳ RSI
            </h3>
            <span
              className={`px-1.5 py-0.5 rounded text-[9px] font-mono uppercase tracking-wider border ${
                enabled
                  ? 'bg-amber-500/10 text-amber-300 border-amber-500/30'
                  : 'bg-neutral-800 text-neutral-400 border-neutral-700'
              }`}
            >
              {enabled ? 'ON' : 'OFF'}
            </span>
          </div>
          <p className="text-[11px] text-neutral-500">
            M15/H1/H4 · RSI Wilder(14) · cooldown riêng cho từng khung và hướng phân kỳ.
          </p>
        </div>

        <span className="px-2 py-1 rounded-md bg-amber-500/10 border border-amber-500/30 text-[10px] font-mono font-semibold text-amber-200">
          Cảnh báo động lượng — không phải entry
        </span>
      </div>

      {context ? (
        <div className="grid grid-cols-3 gap-2">
          <TimeframeState label="M15" snapshot={context.rsi_m15} active={isActive('M15')} />
          <TimeframeState label="H1" snapshot={context.rsi_h1} active={isActive('H1')} />
          <TimeframeState label="H4" snapshot={context.rsi_h4} active={isActive('H4')} />
        </div>
      ) : (
        <p className="text-xs text-neutral-500 font-mono">
          Đang chờ bối cảnh D1/H4/H1 từ Rust engine để hiển thị trạng thái phân kỳ hiện tại.
        </p>
      )}

      <div className="flex flex-col gap-2">
        <div className="flex items-center justify-between text-[10px] font-mono">
          <span className="text-neutral-400 uppercase tracking-wider">
            Cảnh báo gần đây
          </span>
          <span className="text-neutral-600">{alerts.length}/50</span>
        </div>

        {alerts.length === 0 ? (
          <div className="rounded-lg border border-neutral-800 bg-neutral-950/40 px-3 py-4 text-center text-[11px] text-neutral-500 font-mono">
            {enabled
              ? 'Chưa phát hiện phân kỳ RSI nào trên M15/H1/H4.'
              : 'Cảnh báo phân kỳ RSI đang tắt trong Cấu hình.'}
          </div>
        ) : (
          <div className="flex flex-col gap-1.5 max-h-64 overflow-y-auto pr-1">
            {alerts.map((alert) => (
              <div
                key={alert.id}
                className="rounded-lg border border-neutral-800 bg-neutral-950/50 px-3 py-2 flex flex-col gap-1"
              >
                <div className="flex flex-wrap items-center gap-2 text-[10px] font-mono">
                  <span className="text-neutral-500 tabular-nums" title={alert.detected_at}>
                    {detectedLabel(alert.detected_at)}
                  </span>
                  <span className="px-1.5 py-0.5 rounded bg-neutral-900 border border-neutral-800 text-neutral-300">
                    {alert.timeframe}
                  </span>
                  <span
                    className={`px-1.5 py-0.5 rounded border font-bold uppercase ${directionClass(
                      alert.direction
                    )}`}
                  >
                    {alert.direction}
                  </span>
                  <span className="text-white tabular-nums">
                    {Number.isFinite(alert.price) ? `$${alert.price.toFixed(2)}` : '—'}
                  </span>
                  <span className="text-neutral-400 tabular-nums">
                    RSI {Number.isFinite(alert.rsi) ? alert.rsi.toFixed(1) : '—'}
                  </span>
                </div>
                <p className="text-[11px] text-neutral-400 leading-snug">{alert.note}</p>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
};
