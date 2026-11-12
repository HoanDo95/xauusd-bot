'use client';

import React from 'react';
import { ActionVerdict, ActionViewResponse } from '../types';

interface Props {
  data: ActionViewResponse | null;
}

function price(value: number | null | undefined): string {
  if (typeof value !== 'number' || !Number.isFinite(value)) return '—';
  return `$${value.toFixed(2)}`;
}

function timeLabel(value?: string): string {
  if (!value) return '—';
  const parsed = new Date(value);
  if (Number.isNaN(parsed.getTime())) return value;
  return parsed.toLocaleTimeString('vi-VN', {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  });
}

function verdictTone(verdict: ActionVerdict): string {
  switch (verdict) {
    case 'BUY':
      return 'bg-emerald-500/15 text-emerald-300 border-emerald-500/40';
    case 'SELL':
      return 'bg-rose-500/15 text-rose-300 border-rose-500/40';
    case 'WAIT':
      return 'bg-amber-500/15 text-amber-300 border-amber-500/40';
    default:
      return 'bg-neutral-800 text-neutral-300 border-neutral-700';
  }
}

function alignmentTone(alignment: string): string {
  switch ((alignment || '').toUpperCase()) {
    case 'ALIGNED':
      return 'bg-emerald-500/10 text-emerald-300 border-emerald-500/30';
    case 'COUNTER':
      return 'bg-rose-500/10 text-rose-300 border-rose-500/30';
    case 'MIXED':
      return 'bg-amber-500/10 text-amber-300 border-amber-500/30';
    default:
      return 'bg-neutral-800 text-neutral-400 border-neutral-700';
  }
}

function biasTone(bias: string): string {
  switch ((bias || '').toUpperCase()) {
    case 'BULLISH':
      return 'bg-emerald-500/15 text-emerald-300 border-emerald-500/40';
    case 'BEARISH':
      return 'bg-rose-500/15 text-rose-300 border-rose-500/40';
    default:
      return 'bg-amber-500/15 text-amber-300 border-amber-500/40';
  }
}

const CAUTION_COPY: Record<NonNullable<ActionViewResponse['caution']>, string> = {
  COUNTER_MACRO: 'Macro/Flow đang COUNTER — giảm size hoặc chờ xác nhận.',
  FILTER_BLOCKED: 'Một hoặc nhiều filter đang chặn setup — chưa phải điểm vào.',
  VOLUME_PROXY: 'Volume PAXG proxy — không phải volume XAUUSD thật; chỉ nên xem là tương đối.',
};

const CAUTION_TONE: Record<NonNullable<ActionViewResponse['caution']>, string> = {
  COUNTER_MACRO: 'border-rose-500/30 bg-rose-500/5 text-rose-300',
  FILTER_BLOCKED: 'border-amber-500/30 bg-amber-500/5 text-amber-300',
  VOLUME_PROXY: 'border-sky-500/30 bg-sky-500/5 text-sky-300',
};

const Metric: React.FC<{
  label: string;
  value: string;
  valueClass?: string;
  hint?: string;
}> = ({ label, value, valueClass = 'text-white', hint }) => (
  <div className="flex flex-col gap-0.5">
    <span className="text-[10px] uppercase tracking-wider text-neutral-500">{label}</span>
    <span className={`text-sm font-bold tabular-nums ${valueClass}`}>{value}</span>
    {hint && <span className="text-[9px] text-neutral-600">{hint}</span>}
  </div>
);

/**
 * Action View — the dashboard's single "Should I enter now?" card.
 *
 * It is a display-only summary of `/api/action-view`: the backend never sends
 * Telegram, inserts a signal, consumes quota or auto-trades for this endpoint.
 */
export const ActionCard: React.FC<Props> = ({ data }) => {
  if (!data) {
    return (
      <div className="bg-neutral-900 border border-neutral-800 rounded-xl p-4 flex flex-col gap-2">
        <h3 className="text-sm font-bold text-white tracking-tight uppercase font-mono">
          View hành động — nên vào lệnh bây giờ?
        </h3>
        <p className="text-xs text-neutral-500 font-mono">Đang tải action view...</p>
      </div>
    );
  }

  const signal = data.signal;
  const primary = data.primaryScenario;

  return (
    <div className="bg-neutral-900 border border-neutral-800 rounded-xl p-4 flex flex-col gap-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="flex flex-col gap-1">
          <div className="flex flex-wrap items-center gap-2">
            <h3 className="text-sm font-bold text-white tracking-tight uppercase font-mono">
              View hành động — nên vào lệnh bây giờ?
            </h3>
            <span className="px-1.5 py-0.5 rounded text-[9px] font-mono uppercase tracking-wider bg-neutral-800 text-neutral-400 border border-neutral-700">
              Read-only
            </span>
            <span className="px-1.5 py-0.5 rounded text-[9px] font-mono uppercase tracking-wider bg-sky-500/10 text-sky-300 border border-sky-500/30">
              Không auto-trade
            </span>
          </div>
          <p className="text-[11px] text-neutral-500">
            Tổng hợp live setup + kịch bản chính + macro/flow. Chỉ báo cáo, không gửi Telegram,
            không lưu tín hiệu, không tiêu hao quota.
          </p>
        </div>
        <span className="text-[10px] font-mono text-neutral-600">
          {data.generated_at ? `Cập nhật ${timeLabel(data.generated_at)}` : 'Chưa có thời điểm'}
        </span>
      </div>

      <div className="flex flex-wrap items-center gap-3">
        <span
          className={`px-4 py-2 rounded-lg border text-2xl font-black font-mono tracking-widest ${verdictTone(
            data.verdict
          )}`}
        >
          {data.verdict}
        </span>
        <div className="flex min-w-0 flex-col gap-1">
          <h4 className="text-base font-bold text-white leading-snug">{data.headline}</h4>
          <p className="text-xs text-neutral-400 leading-relaxed">{data.reason}</p>
        </div>
      </div>

      {data.caution && (
        <div
          className={`rounded-lg border px-3 py-2 text-[11px] font-mono font-semibold ${
            CAUTION_TONE[data.caution]
          }`}
        >
          ⚠️ {CAUTION_COPY[data.caution]}
        </div>
      )}

      <div className="flex flex-wrap items-center gap-2 text-[10px] font-mono">
        <span className="px-2 py-1 rounded-md border border-neutral-700 bg-neutral-950/60 text-neutral-400">
          Volume quality:{' '}
          <strong className="text-neutral-200">{data.volumeQuality}</strong>
        </span>
        {data.volumeWarning && (
          <span className="px-2 py-1 rounded-md border border-amber-500/30 bg-amber-500/5 text-amber-300">
            ⚠️ {data.volumeWarning}
          </span>
        )}
        {data.macroFlow && (
          <span className="inline-flex flex-wrap items-center gap-2 px-2 py-1 rounded-md border border-neutral-700 bg-neutral-950/60 text-neutral-400">
            <span>🌐 Macro/Flow</span>
            <span
              className={`px-1.5 py-0.5 rounded border font-bold ${alignmentTone(
                data.macroFlow.alignment
              )}`}
            >
              {data.macroFlow.alignment}
            </span>
            <span>
              Bias <strong className="text-neutral-200">{data.macroFlow.bias}</strong>
            </span>
            <span>
              Confidence <strong className="text-neutral-200">{data.macroFlow.confidence}%</strong>
            </span>
            <span>
              Flow <strong className="text-neutral-200">{data.macroFlow.flowQuality}</strong>
            </span>
          </span>
        )}
      </div>

      {signal ? (
        <div className="flex flex-col gap-3">
          <div className="flex flex-wrap items-center gap-2">
            <span
              className={`px-2.5 py-1 rounded-md text-xs font-bold font-mono tracking-wider border ${
                signal.signal_type === 'BUY'
                  ? 'bg-emerald-500/20 text-emerald-400 border-emerald-500/30'
                  : 'bg-rose-500/20 text-rose-400 border-rose-500/30'
              }`}
            >
              {signal.signal_type}
            </span>
            <span className="text-sm font-semibold text-white font-mono">{signal.symbol}</span>
            <span className="text-xs text-neutral-400 font-mono">· {signal.timeframe}</span>
            {signal.entry_type && (
              <span className="px-1.5 py-0.5 rounded text-[10px] font-mono bg-sky-500/15 text-sky-300 border border-sky-500/30">
                {signal.entry_type}
              </span>
            )}
          </div>

          <div className="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-6 gap-3 bg-neutral-950/60 p-3 rounded-lg border border-neutral-800 font-mono">
            <Metric label="Entry" value={price(signal.entry_price)} />
            <Metric label="Stop Loss" value={price(signal.stop_loss)} valueClass="text-rose-400" />
            <Metric
              label="TP1"
              value={price(signal.take_profit_1)}
              valueClass="text-emerald-400"
            />
            <Metric
              label="TP2"
              value={price(signal.take_profit_2)}
              valueClass="text-emerald-300"
            />
            <Metric
              label="R:R"
              value={`1:${signal.risk_reward_ratio.toFixed(2)}`}
              valueClass="text-sky-300"
            />
            <Metric
              label="Lot"
              value={
                typeof signal.recommended_lot_size === 'number'
                  ? `${signal.recommended_lot_size.toFixed(2)} lot`
                  : '—'
              }
              hint={
                typeof signal.risk_amount_usd === 'number'
                  ? `Risk ${price(signal.risk_amount_usd)}`
                  : undefined
              }
            />
          </div>
        </div>
      ) : primary ? (
        <div className="flex flex-col gap-3 rounded-lg border border-neutral-800 bg-neutral-950/60 p-3">
          <div className="flex flex-wrap items-center gap-2">
            <span
              className={`px-2 py-0.5 rounded border text-[10px] font-mono font-bold ${biasTone(
                primary.bias
              )}`}
            >
              {primary.bias || 'UNKNOWN'}
            </span>
            <span className="text-xs font-bold text-white">{primary.name}</span>
            <span
              className={`px-1.5 py-0.5 rounded border text-[9px] font-mono font-bold ${
                primary.valid
                  ? 'bg-emerald-500/15 text-emerald-300 border-emerald-500/40'
                  : 'bg-amber-500/10 text-amber-300 border-amber-500/30'
              }`}
            >
              {primary.valid ? 'VALID' : 'NOT VALID'}
            </span>
            <span className="text-[10px] font-mono text-neutral-500">
              Confidence {primary.confidence}%
            </span>
          </div>

          <p className="text-[11px] font-semibold text-neutral-200 leading-snug">
            {primary.trigger}
          </p>

          <div className="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-6 gap-3 font-mono">
            <Metric label="Entry" value={price(primary.entry)} />
            <Metric label="Stop Loss" value={price(primary.stop_loss)} valueClass="text-rose-400" />
            <Metric
              label="TP1"
              value={price(primary.take_profit_1)}
              valueClass="text-emerald-400"
            />
            <Metric
              label="TP2"
              value={price(primary.take_profit_2)}
              valueClass="text-emerald-300"
            />
            <Metric
              label="R:R"
              value={
                typeof primary.risk_reward === 'number'
                  ? `1:${primary.risk_reward.toFixed(2)}`
                  : '—'
              }
              valueClass="text-sky-300"
            />
            <Metric label="Macro" value={primary.macro_alignment || 'UNKNOWN'} />
          </div>

          <p className="text-[10px] text-neutral-400 leading-relaxed">
            <span className="text-neutral-500">Invalidation:</span> {primary.invalidation}
          </p>
        </div>
      ) : (
        <p className="text-xs text-neutral-500 font-mono">
          Không có signal hoặc kịch bản chính để hiển thị.
        </p>
      )}

      <div className="flex flex-wrap items-center justify-between gap-2 border-t border-neutral-800 pt-3 text-[10px] font-mono">
        <span className="text-neutral-400">
          View hành động — không phải lệnh tự động; bot không tự vào lệnh.
        </span>
        <a
          href="#scenario-view"
          className="text-sky-300 hover:text-sky-200 underline underline-offset-2 transition-colors"
        >
          Xem kịch bản chi tiết →
        </a>
      </div>

      <div className="flex flex-wrap items-center justify-between gap-2 text-[10px] font-mono text-neutral-600">
        <span>
          Nguồn: {data.source || '—'}
          {data.volumeSource ? ` · Vol: ${data.volumeSource}` : ''}
        </span>
        <span>{data.generated_at ? timeLabel(data.generated_at) : '—'}</span>
      </div>
    </div>
  );
};
