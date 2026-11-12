'use client';

import React from 'react';
import { CotSnapshot, MacroFlowResponse, MacroFlowScorecard, MacroMetric } from '../types';

interface Props {
  data: MacroFlowResponse | null;
  enabled?: boolean;
}

const DISPLAY_METRICS: Array<{ key: string; fallback: string }> = [
  { key: 'real_yield_10y', fallback: 'Real Yield 10Y' },
  { key: 'nominal_10y', fallback: 'US 10Y' },
  { key: 'yield_2y', fallback: 'US 2Y' },
  { key: 'curve_2s10s', fallback: '2s10s' },
  { key: 'dxy', fallback: 'DXY' },
  { key: 'gold', fallback: 'Gold' },
  { key: 'gld', fallback: 'GLD' },
  { key: 'iau', fallback: 'IAU' },
  { key: 'tlt', fallback: 'TLT' },
  { key: 'spy', fallback: 'SPY' },
  { key: 'vix', fallback: 'VIX' },
  { key: 'gvz', fallback: 'GVZ' },
  { key: 'mmf', fallback: 'MMF' },
];

function isFiniteNumber(value: number | null | undefined): value is number {
  return typeof value === 'number' && Number.isFinite(value);
}

function formatValue(metric?: MacroMetric): string {
  if (!metric || !isFiniteNumber(metric.value)) return '—';
  const value = metric.value;
  if (metric.unit === '%') return `${value.toFixed(2)}%`;
  if (metric.unit === 'USD mn') return `${Math.round(value).toLocaleString('en-US')} mn`;
  if (metric.unit === 'USD') {
    return `$${value.toLocaleString('en-US', {
      minimumFractionDigits: 2,
      maximumFractionDigits: 2,
    })}`;
  }
  if (metric.unit === 'contracts') return Math.round(value).toLocaleString('en-US');
  return value.toFixed(2);
}

function formatChange(metric?: MacroMetric): string {
  if (!metric) return '—';
  if (isFiniteNumber(metric.change_5d_pct)) {
    return `${metric.change_5d_pct >= 0 ? '+' : ''}${metric.change_5d_pct.toFixed(2)}%`;
  }
  if (isFiniteNumber(metric.change_5d)) {
    const suffix = metric.unit === '%' ? ' pp' : metric.unit === 'USD mn' ? ' mn' : '';
    return `${metric.change_5d >= 0 ? '+' : ''}${metric.change_5d.toFixed(2)}${suffix}`;
  }
  return '—';
}

function changeValue(metric?: MacroMetric): number | null {
  if (!metric) return null;
  if (isFiniteNumber(metric.change_5d_pct)) return metric.change_5d_pct;
  if (isFiniteNumber(metric.change_5d)) return metric.change_5d;
  return null;
}

function changeTone(value: number | null): string {
  if (value == null || value === 0) return 'text-neutral-500';
  return value > 0 ? 'text-emerald-400' : 'text-rose-400';
}

function formatContracts(value: number, signed = false): string {
  if (!Number.isFinite(value)) return '—';
  const digits = Math.abs(value).toLocaleString('en-US', { maximumFractionDigits: 0 });
  return signed ? `${value >= 0 ? '+' : '-'}${digits}` : digits;
}

function biasTone(bias: string): string {
  if (bias === 'BULLISH') return 'bg-emerald-500/15 text-emerald-300 border-emerald-500/40';
  if (bias === 'BEARISH') return 'bg-rose-500/15 text-rose-300 border-rose-500/40';
  return 'bg-sky-500/15 text-sky-300 border-sky-500/40';
}

function regimeTone(value: string): string {
  switch (value) {
    case 'RISK_ON':
    case 'EASING':
    case 'WEAK':
    case 'LOW':
      return 'bg-emerald-500/10 text-emerald-300 border-emerald-500/30';
    case 'RISK_OFF':
    case 'TIGHTENING':
    case 'STRONG':
    case 'HIGH':
      return 'bg-rose-500/10 text-rose-300 border-rose-500/30';
    default:
      return 'bg-neutral-800 text-neutral-300 border-neutral-700';
  }
}

function flowTone(value: string): string {
  switch (value) {
    case 'CONFIRMING':
      return 'bg-emerald-500/15 text-emerald-300 border-emerald-500/40';
    case 'DIVERGING':
      return 'bg-rose-500/15 text-rose-300 border-rose-500/40';
    case 'INSUFFICIENT':
      return 'bg-neutral-800 text-neutral-400 border-neutral-700';
    default:
      return 'bg-sky-500/10 text-sky-300 border-sky-500/30';
  }
}

const RegimeBadge: React.FC<{ label: string; value: string }> = ({ label, value }) => (
  <div className="flex items-center justify-between gap-2 rounded-lg border border-neutral-800 bg-neutral-950/60 px-2.5 py-2">
    <span className="text-[9px] uppercase tracking-wider text-neutral-500 font-mono">{label}</span>
    <span className={`px-1.5 py-0.5 rounded text-[9px] font-mono font-bold border ${regimeTone(value)}`}>
      {value}
    </span>
  </div>
);

const MetricTile: React.FC<{ metric?: MacroMetric; fallback: string }> = ({ metric, fallback }) => {
  const change = changeValue(metric);
  return (
    <div
      className="rounded-lg border border-neutral-800 bg-neutral-950/60 px-2.5 py-2 flex flex-col gap-1 min-w-0"
      title={metric ? `${metric.source}${metric.date ? ` · ${metric.date}` : ''}${metric.note ? `\n${metric.note}` : ''}` : undefined}
    >
      <span className="text-[9px] font-mono uppercase tracking-wider text-neutral-500 truncate">
        {metric?.label || fallback}
      </span>
      <span className="text-sm font-bold font-mono text-white tabular-nums leading-none">
        {formatValue(metric)}
      </span>
      <div className="flex items-center justify-between gap-1 text-[9px] font-mono">
        <span className={`tabular-nums ${changeTone(change)}`}>5d {formatChange(metric)}</span>
        {isFiniteNumber(metric?.volume_ratio_20d) && (
          <span className="text-neutral-600">vol {metric.volume_ratio_20d.toFixed(1)}x</span>
        )}
      </div>
    </div>
  );
};

const ScoreSummary: React.FC<{ scorecard: MacroFlowScorecard }> = ({ scorecard }) => {
  const confidence = Math.min(100, Math.max(0, scorecard.confidence));
  const confidenceColor =
    confidence >= 70 ? 'bg-emerald-400' : confidence >= 40 ? 'bg-amber-400' : 'bg-rose-400';

  return (
    <div className="rounded-lg border border-neutral-800 bg-neutral-950/60 p-3 flex flex-col gap-2.5">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <span className="text-[10px] font-mono uppercase tracking-wider text-neutral-400">
          Gold bias
        </span>
        <span className={`px-2 py-0.5 rounded text-[10px] font-mono font-bold border ${biasTone(scorecard.macro_bias)}`}>
          {scorecard.macro_bias}
        </span>
      </div>

      <div className="flex items-baseline justify-between gap-2 font-mono">
        <span className="text-2xl font-bold text-white tabular-nums leading-none">
          {confidence}%
        </span>
        <span className="text-[10px] text-neutral-500">confidence</span>
      </div>
      <div className="h-1.5 w-full rounded-full bg-neutral-800 overflow-hidden">
        <div
          className={`h-full rounded-full ${confidenceColor}`}
          style={{ width: `${Math.max(2, confidence)}%` }}
        />
      </div>
      <div className="flex items-center justify-between gap-2 text-[10px] font-mono">
        <span className="text-neutral-500">
          score <span className="text-neutral-200 tabular-nums">{scorecard.score >= 0 ? `+${scorecard.score}` : scorecard.score}</span>
        </span>
        <span className="text-neutral-500">
          direction <span className="text-neutral-200">{scorecard.direction}</span>
        </span>
      </div>
    </div>
  );
};

const FlowSummary: React.FC<{ scorecard: MacroFlowScorecard }> = ({ scorecard }) => (
  <div className="rounded-lg border border-neutral-800 bg-neutral-950/60 p-3 flex flex-col gap-2.5">
    <div className="flex flex-wrap items-center justify-between gap-2">
      <span className="text-[10px] font-mono uppercase tracking-wider text-neutral-400">
        Flow quality
      </span>
      <span className={`px-2 py-0.5 rounded text-[10px] font-mono font-bold border ${flowTone(scorecard.flow_quality)}`}>
        {scorecard.flow_quality}
      </span>
    </div>
    <div className="flex flex-col gap-1">
      <span className="text-[9px] font-mono uppercase tracking-wider text-neutral-500">Driver</span>
      <span className="text-[11px] font-mono text-neutral-200 leading-snug">{scorecard.driver}</span>
    </div>
    <p className="text-[10px] text-neutral-500 leading-snug">
      Giá vàng, ETF proxy và COT cùng hướng = CONFIRMING; lệch hướng = DIVERGING.
    </p>
  </div>
);

const CotBlock: React.FC<{ cot: CotSnapshot }> = ({ cot }) => (
  <div className="rounded-lg border border-amber-500/20 bg-amber-500/[0.03] p-3 flex flex-col gap-2.5">
    <div className="flex flex-wrap items-center justify-between gap-2">
      <span className="text-[10px] font-mono uppercase tracking-wider text-amber-300">
        CFTC COT — COMEX Gold
      </span>
      <span className="text-[9px] font-mono text-neutral-500">
        report {cot.report_date} · weekly lag
      </span>
    </div>
    <div className="grid grid-cols-2 md:grid-cols-4 gap-2 text-[11px] font-mono">
      <div className="flex flex-col gap-0.5">
        <span className="text-neutral-500">Non-commercial net</span>
        <span className={`font-bold tabular-nums ${cot.noncommercial_net >= 0 ? 'text-emerald-400' : 'text-rose-400'}`}>
          {formatContracts(cot.noncommercial_net, true)}
        </span>
      </div>
      <div className="flex flex-col gap-0.5">
        <span className="text-neutral-500">Weekly change</span>
        <span className={`font-bold tabular-nums ${cot.noncommercial_net_change >= 0 ? 'text-emerald-400' : 'text-rose-400'}`}>
          {formatContracts(cot.noncommercial_net_change, true)}
        </span>
      </div>
      <div className="flex flex-col gap-0.5">
        <span className="text-neutral-500">Net % open interest</span>
        <span className="font-bold text-neutral-200 tabular-nums">{cot.net_pct_oi.toFixed(1)}%</span>
      </div>
      <div className="flex flex-col gap-0.5">
        <span className="text-neutral-500">Open interest</span>
        <span className="font-bold text-neutral-200 tabular-nums">{formatContracts(cot.open_interest)}</span>
        <span className={`text-[9px] tabular-nums ${cot.open_interest_change >= 0 ? 'text-emerald-400' : 'text-rose-400'}`}>
          {formatContracts(cot.open_interest_change, true)} tuần
        </span>
      </div>
    </div>
  </div>
);

const CompactState: React.FC<{ title: string; message: string }> = ({ title, message }) => (
  <div className="bg-neutral-900 border border-neutral-800 rounded-xl p-4 flex flex-col gap-2">
    <div className="flex flex-wrap items-center gap-2">
      <h3 className="text-sm font-bold text-white tracking-tight">
        Macro &amp; Flow — bối cảnh trước khi vào lệnh
      </h3>
      <span className="px-1.5 py-0.5 rounded text-[9px] font-mono uppercase tracking-wider bg-neutral-800 text-neutral-400 border border-neutral-700">
        Read-only
      </span>
    </div>
    <p className="text-[11px] text-neutral-500 font-mono">{title}</p>
    <p className="text-[11px] text-neutral-500 leading-relaxed">{message}</p>
  </div>
);

/**
 * Top-down macro/flow context. Read-only and advisory: it never changes signal
 * generation, filters, quota, Telegram alerts or auto-trading.
 */
export const MacroFlowCard: React.FC<Props> = ({ data, enabled = true }) => {
  if (!enabled || data?.status === 'disabled') {
    return (
      <CompactState
        title="Dashboard đang tắt"
        message="Bật “Macro & Flow dashboard (read-only)” trong Cấu hình để xem bối cảnh vĩ mô và dòng tiền. Mục này không ảnh hưởng tín hiệu, quota hay Telegram."
      />
    );
  }

  if (!data || data.status !== 'ok' || !data.snapshot) {
    return (
      <CompactState
        title="Đang tải snapshot FRED / Yahoo / CFTC..."
        message="Worker read-only làm mới mỗi 15 phút. Trong lúc chờ, các tín hiệu kỹ thuật bên dưới vẫn hoạt động bình thường."
      />
    );
  }

  const snapshot = data.snapshot;
  const scorecard = snapshot.scorecard;
  const metrics = new Map(snapshot.metrics.map((metric) => [metric.key, metric]));
  const updated = new Date(snapshot.updated_at);
  const updatedLabel = Number.isNaN(updated.getTime())
    ? snapshot.updated_at
    : updated.toLocaleTimeString('vi-VN', { hour: '2-digit', minute: '2-digit', second: '2-digit' });

  return (
    <div className="bg-neutral-900 border border-neutral-800 rounded-xl p-4 flex flex-col gap-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="flex flex-col gap-0.5">
          <div className="flex flex-wrap items-center gap-2">
            <h3 className="text-sm font-bold text-white tracking-tight uppercase font-mono">
              Macro &amp; Flow — bối cảnh trước khi vào lệnh
            </h3>
            <span className="px-1.5 py-0.5 rounded text-[9px] font-mono uppercase tracking-wider bg-neutral-800 text-neutral-400 border border-neutral-700">
              Read-only
            </span>
          </div>
          <p className="text-[11px] text-neutral-500">
            FRED + Yahoo Finance + CFTC · cập nhật {updatedLabel} · bối cảnh vĩ mô và dòng tiền, không phải entry.
          </p>
        </div>

        <div className="flex flex-wrap items-center gap-1.5 text-[9px] font-mono">
          <span className={`px-1.5 py-0.5 rounded border ${snapshot.fred_ok ? 'bg-emerald-500/10 text-emerald-300 border-emerald-500/30' : 'bg-rose-500/10 text-rose-300 border-rose-500/30'}`}>
            FRED {snapshot.fred_ok ? 'OK' : 'lỗi'}
          </span>
          <span className={`px-1.5 py-0.5 rounded border ${snapshot.yahoo_ok ? 'bg-emerald-500/10 text-emerald-300 border-emerald-500/30' : 'bg-rose-500/10 text-rose-300 border-rose-500/30'}`}>
            Yahoo {snapshot.yahoo_ok ? 'OK' : 'lỗi'}
          </span>
          <span className={`px-1.5 py-0.5 rounded border ${snapshot.cot_ok ? 'bg-emerald-500/10 text-emerald-300 border-emerald-500/30' : 'bg-amber-500/10 text-amber-300 border-amber-500/30'}`}>
            COT {snapshot.cot_ok ? 'OK' : 'thiếu'}
          </span>
        </div>
      </div>

      <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
        <ScoreSummary scorecard={scorecard} />
        <FlowSummary scorecard={scorecard} />
      </div>

      <div className="grid grid-cols-2 md:grid-cols-4 gap-2">
        <RegimeBadge label="Risk" value={scorecard.risk_regime} />
        <RegimeBadge label="Rates" value={scorecard.rate_regime} />
        <RegimeBadge label="USD" value={scorecard.usd_regime} />
        <RegimeBadge label="Volatility" value={scorecard.vol_regime} />
      </div>

      <div className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-5 gap-2">
        {DISPLAY_METRICS.map(({ key, fallback }) => (
          <MetricTile key={key} metric={metrics.get(key)} fallback={fallback} />
        ))}
      </div>

      {snapshot.cot && <CotBlock cot={snapshot.cot} />}

      <div className="grid grid-cols-1 lg:grid-cols-2 gap-3">
        <div className="rounded-lg border border-neutral-800 bg-neutral-950/40 p-3 flex flex-col gap-1.5">
          <span className="text-[9px] font-mono uppercase tracking-wider text-neutral-500">
            Ghi chú
          </span>
          <ul className="list-disc pl-4 text-[10px] text-neutral-400 leading-relaxed flex flex-col gap-1">
            {scorecard.notes.map((note, index) => (
              <li key={`${index}-${note.slice(0, 12)}`}>{note}</li>
            ))}
          </ul>
        </div>

        <div className="flex flex-col gap-3">
          {snapshot.warnings.length > 0 && (
            <div className="rounded-lg border border-amber-500/30 bg-amber-500/5 p-3 flex flex-col gap-1">
              <span className="text-[9px] font-mono uppercase tracking-wider text-amber-300">
                Cảnh báo nguồn dữ liệu ({snapshot.warnings.length})
              </span>
              <ul className="list-disc pl-4 text-[10px] text-amber-200/80 leading-relaxed max-h-24 overflow-y-auto">
                {snapshot.warnings.map((warning, index) => (
                  <li key={`${index}-${warning.slice(0, 12)}`}>{warning}</li>
                ))}
              </ul>
            </div>
          )}

          <div className="rounded-lg border border-neutral-800 bg-neutral-950/40 p-3 flex flex-col gap-1">
            <span className="text-[9px] font-mono uppercase tracking-wider text-neutral-500">
              Nguồn
            </span>
            <p className="text-[10px] text-neutral-500 leading-relaxed">
              {snapshot.sources.join(' · ')}
            </p>
          </div>
        </div>
      </div>

      <p className="text-[10px] font-mono text-neutral-500 border-t border-neutral-800 pt-2">
        Macro/flow context — không phải entry. ETF flow là proxy giá/volume; COT trễ tuần; skew chỉ là proxy GVZ/VIX.
      </p>
    </div>
  );
};
