'use client';

import React from 'react';
import { BotConfig, LiveViewResponse } from '../types';

interface Props {
  config: BotConfig;
  liveView: LiveViewResponse | null;
  isTogglingScanner: boolean;
  onToggleScanner: () => void;
  onRefresh: () => void;
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

function candleTimeLabel(value?: number): string {
  if (typeof value !== 'number' || !Number.isFinite(value) || value <= 0) return '—';
  const parsed = new Date(value);
  if (Number.isNaN(parsed.getTime())) return '—';
  return parsed.toLocaleString('vi-VN', {
    hour: '2-digit',
    minute: '2-digit',
    day: '2-digit',
    month: '2-digit',
  });
}

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

const FilterChip: React.FC<{ ok: boolean; label: string; reason?: string }> = ({
  ok,
  label,
  reason,
}) => (
  <span
    title={reason}
    className={`inline-flex items-center gap-1.5 px-2 py-1 rounded-md border text-[10px] font-mono font-semibold ${
      ok
        ? 'bg-emerald-500/10 text-emerald-300 border-emerald-500/30'
        : 'bg-rose-500/10 text-rose-300 border-rose-500/30'
    }`}
  >
    <span className={`w-1.5 h-1.5 rounded-full ${ok ? 'bg-emerald-400' : 'bg-rose-400'}`} />
    {label}
    <span className="font-normal opacity-80">{ok ? 'OK' : 'BLOCKED'}</span>
  </span>
);

/**
 * Live Setup preview — read-only Entry/SL/TP card plus the scanner Start/Stop
 * control. The preview never sends Telegram, stores a signal, or spends quota.
 */
export const LiveSetupCard: React.FC<Props> = ({
  config,
  liveView,
  isTogglingScanner,
  onToggleScanner,
  onRefresh,
}) => {
  const running = config.is_active;
  const signal = liveView?.status === 'setup' ? liveView.signal : undefined;
  const filters = liveView?.filters;

  const blockedReasons = [
    filters && !filters.htf_ok ? `H4/D1: ${filters.htf_reason}` : null,
    filters && !filters.rsi_ok ? `RSI: ${filters.rsi_reason}` : null,
    filters && !filters.delta_ok ? `Volume Delta: ${filters.delta_reason}` : null,
  ].filter((reason): reason is string => Boolean(reason));

  return (
    <div className="bg-neutral-900 border border-neutral-800 rounded-xl p-4 flex flex-col gap-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="flex flex-col gap-1">
          <div className="flex flex-wrap items-center gap-2">
            <h3 className="text-sm font-bold text-white tracking-tight uppercase font-mono">
              Live Setup — Entry / SL / TP
            </h3>
            <span
              className={`inline-flex items-center gap-1.5 px-2 py-0.5 rounded-md border text-[10px] font-mono font-bold tracking-wider ${
                running
                  ? 'bg-emerald-500/10 text-emerald-300 border-emerald-500/30'
                  : 'bg-neutral-800 text-neutral-400 border-neutral-700'
              }`}
            >
              <span
                className={`w-1.5 h-1.5 rounded-full ${
                  running ? 'bg-emerald-400 animate-pulse' : 'bg-neutral-500'
                }`}
              />
              {running ? 'SCANNER RUNNING' : 'SCANNER PAUSED'}
            </span>
          </div>
          <p className="text-[11px] text-neutral-500">
            Preview read-only từ engine: không gửi Telegram, không lưu tín hiệu, không tiêu hao
            quota.
          </p>
        </div>

        <div className="flex flex-wrap items-center gap-2">
          <button
            onClick={onRefresh}
            className="px-3 py-1.5 text-xs font-semibold rounded-lg border border-neutral-700 bg-neutral-950 text-neutral-300 hover:text-white hover:border-neutral-600 transition-colors cursor-pointer"
            title="Tải lại live view ngay (không tốn quota)"
          >
            Làm mới
          </button>
          <button
            onClick={onToggleScanner}
            disabled={isTogglingScanner}
            className={`px-3.5 py-1.5 text-xs font-bold rounded-lg border transition-colors cursor-pointer disabled:cursor-not-allowed disabled:opacity-60 ${
              running
                ? 'bg-rose-500/10 text-rose-300 border-rose-500/40 hover:bg-rose-500/20'
                : 'bg-emerald-500/10 text-emerald-300 border-emerald-500/40 hover:bg-emerald-500/20'
            }`}
          >
            {isTogglingScanner ? 'Đang lưu...' : running ? 'Stop Scanner' : 'Start Scanner'}
          </button>
        </div>
      </div>

      {!running && (
        <div className="rounded-lg border border-neutral-700 bg-neutral-950/60 px-3 py-2 text-xs font-mono text-neutral-300">
          Bấm Start để bắt đầu quét.
          {liveView
            ? ' Đang giữ kết quả gần nhất bên dưới.'
            : ' Chưa có dữ liệu live view.'}
        </div>
      )}

      {liveView?.status === 'setup' && signal && filters ? (
        <div className="flex flex-col gap-3">
          <div className="flex flex-wrap items-center justify-between gap-2">
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
                  {signal.entry_type !== 'MARKET'
                    ? ` · ${signal.is_entry_filled ? 'FILLED' : 'PENDING'}`
                    : ''}
                </span>
              )}
              {typeof signal.confluence_score === 'number' && (
                <span className="px-1.5 py-0.5 rounded text-[10px] font-mono bg-sky-500/10 text-sky-300 border border-sky-500/30">
                  Confluence {signal.confluence_score}/100
                </span>
              )}
            </div>
            <span className="text-[10px] text-neutral-500 font-mono">
              Tạo lúc {timeLabel(signal.created_at)}
            </span>
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

          <div className="flex flex-wrap items-center gap-2">
            <FilterChip ok={filters.htf_ok} label="H4/D1" reason={filters.htf_reason} />
            <FilterChip
              ok={filters.rsi_ok}
              label={`RSI ${
                typeof filters.rsi_value === 'number' ? filters.rsi_value.toFixed(1) : 'n/a'
              }${filters.rsi_state ? ` · ${filters.rsi_state}` : ''}${
                filters.rsi_divergence && filters.rsi_divergence !== 'None'
                  ? ` · ${filters.rsi_divergence} div`
                  : ''
              }`}
              reason={filters.rsi_reason}
            />
            <FilterChip
              ok={filters.delta_ok}
              label={`Volume Delta ${
                typeof filters.delta_ratio === 'number'
                  ? `${filters.delta_ratio >= 0 ? '+' : ''}${filters.delta_ratio.toFixed(2)}`
                  : 'n/a'
              }`}
              reason={filters.delta_reason}
            />
            <span
              title={
                filters.ai_review_enabled
                  ? 'AI review đang bật trong cấu hình; preview không gọi AI để tránh latency/chi phí.'
                  : 'AI review đang tắt trong cấu hình.'
              }
              className={`inline-flex items-center gap-1.5 px-2 py-1 rounded-md border text-[10px] font-mono font-semibold ${
                filters.ai_review_enabled
                  ? 'bg-sky-500/10 text-sky-300 border-sky-500/30'
                  : 'bg-neutral-800 text-neutral-400 border-neutral-700'
              }`}
            >
              <span
                className={`w-1.5 h-1.5 rounded-full ${
                  filters.ai_review_enabled ? 'bg-sky-400' : 'bg-neutral-500'
                }`}
              />
              AI review {filters.ai_review_enabled ? 'ON' : 'OFF'}
            </span>
          </div>

          {blockedReasons.length > 0 && (
            <div className="flex flex-col gap-1 rounded-lg border border-rose-500/20 bg-rose-500/5 px-3 py-2 text-[10px] font-mono text-rose-300">
              <span className="font-bold uppercase tracking-wider">
                Scanner sẽ chặn setup này
              </span>
              {blockedReasons.map((reason) => (
                <p key={reason}>{reason}</p>
              ))}
            </div>
          )}
        </div>
      ) : liveView?.status === 'no_setup' ? (
        <div className="rounded-lg border border-neutral-800 bg-neutral-950/60 px-3 py-3 flex flex-col gap-1.5 text-xs font-mono">
          <p className="text-neutral-200">
            {liveView.message || 'Chưa đủ điều kiện Pinbar + Volume + vùng S/R.'}
          </p>
          <p className="text-neutral-500">
            Nến gần nhất:{' '}
            <span className="text-neutral-300">{candleTimeLabel(liveView.candle_timestamp)}</span>
            {' · '}Close:{' '}
            <span className="text-white tabular-nums">{price(liveView.last_close)}</span>
          </p>
        </div>
      ) : liveView?.status === 'no_data' ? (
        <div className="rounded-lg border border-neutral-800 bg-neutral-950/60 px-3 py-3 flex flex-col gap-1.5 text-xs font-mono">
          <p className="text-neutral-200">
            {liveView.message || 'Chưa đủ nến để phân tích.'}
          </p>
          {typeof liveView.candles === 'number' && (
            <p className="text-neutral-500">
              Nến khả dụng: <span className="text-neutral-300">{liveView.candles}</span>
            </p>
          )}
        </div>
      ) : running ? (
        <p className="text-xs text-neutral-500 font-mono">Đang tải live view...</p>
      ) : null}

      <div className="flex flex-wrap items-center justify-between gap-2 border-t border-neutral-800 pt-3 text-[10px] font-mono">
        <span className="text-neutral-400">
          Preview only — không gửi Telegram, không tốn quota
        </span>
        <span className="text-neutral-600">
          {liveView?.source ? `Nguồn: ${liveView.source}` : 'Nguồn: —'}
          {liveView?.volumeSource ? ` · Vol: ${liveView.volumeSource}` : ''}
          {liveView?.generated_at ? ` · ${timeLabel(liveView.generated_at)}` : ''}
        </span>
      </div>
    </div>
  );
};
