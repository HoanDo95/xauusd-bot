'use client';

import React, { useState } from 'react';
import { Scenario, ScenarioMarket, ScenarioResponse } from '../types';

interface Props {
  data: ScenarioResponse | null;
  onRefresh: () => void;
  /** Manual reference-scenario snapshot send. Never a trade signal. */
  onSendTelegram: () => Promise<{ success: boolean; message?: string }>;
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

function macroTone(alignment: string): string {
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

function confidenceTone(confidence: number): string {
  if (confidence >= 70) return 'bg-emerald-400';
  if (confidence >= 50) return 'bg-amber-400';
  return 'bg-rose-400';
}

const Metric: React.FC<{ label: string; value: string; valueClass?: string; hint?: string }> = ({
  label,
  value,
  valueClass = 'text-white',
  hint,
}) => (
  <div className="flex flex-col gap-0.5">
    <span className="text-[10px] uppercase tracking-wider text-neutral-500">{label}</span>
    <span className={`text-sm font-bold tabular-nums ${valueClass}`}>{value}</span>
    {hint && <span className="text-[9px] text-neutral-600">{hint}</span>}
  </div>
);

const BiasChip: React.FC<{ label: string; bias: string }> = ({ label, bias }) => (
  <span
    className={`inline-flex items-center gap-1 px-1.5 py-0.5 rounded border text-[9px] font-mono font-bold ${biasTone(
      bias
    )}`}
  >
    <span className="text-[8px] font-normal opacity-70">{label}</span>
    {bias || 'UNKNOWN'}
  </span>
);

const Level: React.FC<{ label: string; value: string; valueClass?: string }> = ({
  label,
  value,
  valueClass = 'text-neutral-200',
}) => (
  <div className="flex flex-col gap-0.5">
    <span className="text-[9px] uppercase tracking-wider text-neutral-500">{label}</span>
    <span className={`text-[11px] font-bold tabular-nums ${valueClass}`}>{value}</span>
  </div>
);

type ScenarioRole = 'PRIMARY' | 'ALTERNATE';

const WatchlistRow: React.FC<{ scenario: Scenario }> = ({ scenario }) => {
  const confidence = Math.max(0, Math.min(100, scenario.confidence || 0));
  return (
    <div className="flex items-center justify-between gap-2 rounded-md border border-neutral-800 bg-neutral-950/40 px-2.5 py-1.5 text-[10px] font-mono">
      <div className="flex min-w-0 items-center gap-2">
        <span
          className={`shrink-0 px-1.5 py-0.5 rounded border text-[9px] font-bold ${biasTone(
            scenario.bias
          )}`}
        >
          {scenario.bias || 'UNKNOWN'}
        </span>
        <span className="truncate text-neutral-300">{scenario.name}</span>
      </div>
      <div className="flex shrink-0 items-center gap-2">
        <span className={scenario.valid ? 'text-emerald-400' : 'text-neutral-500'}>
          {scenario.valid ? 'valid' : 'not valid'}
        </span>
        <span className="text-neutral-500">{confidence}%</span>
      </div>
    </div>
  );
};

const MarketStrip: React.FC<{ market: ScenarioMarket; currentPrice: number | null }> = ({
  market,
  currentPrice,
}) => (
  <div className="grid grid-cols-2 md:grid-cols-3 xl:grid-cols-6 gap-3 bg-neutral-950/60 border border-neutral-800 rounded-lg p-3 font-mono">
    <Metric label="Giá hiện tại" value={price(currentPrice)} />
    <div className="flex flex-col gap-1">
      <span className="text-[10px] uppercase tracking-wider text-neutral-500">D1 / H4 / H1</span>
      <div className="flex flex-wrap gap-1">
        <BiasChip label="D1" bias={market.d1_bias} />
        <BiasChip label="H4" bias={market.h4_bias} />
        <BiasChip label="H1" bias={market.h1_bias} />
      </div>
    </div>
    <Metric label="ATR" value={`$${market.h4_atr.toFixed(2)}`} />
    <div className="flex flex-col gap-0.5">
      <span className="text-[10px] uppercase tracking-wider text-neutral-500">S/R gần</span>
      <span className="text-[11px] font-bold tabular-nums text-emerald-400">
        {price(market.nearest_support)}
      </span>
      <span className="text-[11px] font-bold tabular-nums text-rose-400">
        {price(market.nearest_resistance)}
      </span>
    </div>
    <Metric
      label="RSI M15"
      value={typeof market.rsi_m15 === 'number' ? market.rsi_m15.toFixed(1) : '—'}
      hint={market.rsi_state || '—'}
    />
    <Metric
      label="Macro"
      value={`${market.macro_bias || 'UNKNOWN'} · ${market.macro_confidence}%`}
    />
  </div>
);

const ScenarioTile: React.FC<{ scenario: Scenario; role: ScenarioRole }> = ({
  scenario,
  role,
}) => {
  const confidence = Math.max(0, Math.min(100, scenario.confidence || 0));
  const isPrimary = role === 'PRIMARY';
  const roleLabel = isPrimary ? '🎯 PRIMARY' : '🔁 ALTERNATE';
  const roleTone = isPrimary
    ? 'bg-emerald-500/15 text-emerald-300 border-emerald-500/40'
    : 'bg-amber-500/15 text-amber-300 border-amber-500/40';
  const borderTone = isPrimary
    ? 'border-emerald-500/40 bg-emerald-500/[0.03]'
    : 'border-amber-500/40 bg-amber-500/[0.03]';

  return (
    <div className={`rounded-lg border p-3 flex flex-col gap-3 ${borderTone}`}>
      <div className="flex items-start justify-between gap-2">
        <div className="flex flex-col gap-1">
          <div className="flex flex-wrap items-center gap-1.5">
            <span
              className={`px-1.5 py-0.5 rounded border text-[9px] font-mono font-bold ${roleTone}`}
            >
              {roleLabel}
            </span>
            <span className="text-[9px] font-mono text-neutral-500">
              priority {scenario.priority ?? '—'}
            </span>
          </div>
          <h4 className="text-xs font-bold text-white leading-snug">{scenario.name}</h4>
          <span
            className={`self-start px-1.5 py-0.5 rounded border text-[9px] font-mono font-bold ${biasTone(
              scenario.bias
            )}`}
          >
            {scenario.bias || 'UNKNOWN'}
          </span>
        </div>
        <span
          className={`shrink-0 px-1.5 py-0.5 rounded border text-[9px] font-mono font-bold ${
            scenario.valid
              ? 'bg-emerald-500/15 text-emerald-300 border-emerald-500/40'
              : 'bg-amber-500/10 text-amber-300 border-amber-500/30'
          }`}
        >
          {scenario.valid ? 'VALID' : 'NOT VIABLE'}
        </span>
      </div>

      <div className="flex flex-col gap-1">
        <p className="text-[11px] font-semibold text-neutral-200 leading-snug">
          {scenario.trigger}
        </p>
        <p className="text-[10px] text-neutral-500">{scenario.condition}</p>
      </div>

      <div className="grid grid-cols-2 gap-2 font-mono">
        <Level label="Entry" value={price(scenario.entry)} valueClass="text-white" />
        <Level label="SL" value={price(scenario.stop_loss)} valueClass="text-rose-400" />
        <Level label="TP1" value={price(scenario.take_profit_1)} valueClass="text-emerald-400" />
        <Level label="TP2" value={price(scenario.take_profit_2)} valueClass="text-emerald-300" />
        <Level
          label="R:R"
          value={
            typeof scenario.risk_reward === 'number'
              ? `1:${scenario.risk_reward.toFixed(2)}`
              : '—'
          }
          valueClass="text-sky-300"
        />
        <div className="flex flex-col gap-0.5">
          <span className="text-[9px] uppercase tracking-wider text-neutral-500">Macro</span>
          <span
            className={`self-start px-1.5 py-0.5 rounded border text-[9px] font-bold ${macroTone(
              scenario.macro_alignment
            )}`}
          >
            {scenario.macro_alignment || 'UNKNOWN'}
          </span>
        </div>
      </div>

      <div className="flex flex-col gap-1">
        <div className="flex items-center justify-between text-[9px] font-mono text-neutral-500">
          <span>Confidence</span>
          <span className="text-neutral-300">{confidence}%</span>
        </div>
        <div className="h-1.5 w-full rounded-full bg-neutral-800 overflow-hidden">
          <div
            className={`h-full rounded-full ${confidenceTone(confidence)}`}
            style={{ width: `${Math.max(2, confidence)}%` }}
          />
        </div>
      </div>

      <div className="flex flex-col gap-1 border-t border-neutral-800/80 pt-2 text-[10px] leading-relaxed">
        <p className="text-neutral-400">
          <span className="text-neutral-500">Invalidation:</span> {scenario.invalidation}
        </p>
        {(scenario.notes ?? []).map((note, index) => (
          <p key={`${scenario.id}-note-${index}`} className="text-neutral-500">
            • {note}
          </p>
        ))}
      </div>
    </div>
  );
};

/**
 * "Scenario View" — four read-only reference scenarios for the current market.
 * It is deliberately independent from the strict signal engine, so the card can
 * show ideas while `/api/live-view` is `no_setup`. The manual Telegram button
 * sends the same reference snapshot; it never stores a signal or spends quota.
 */
export const ScenarioCard: React.FC<Props> = ({ data, onRefresh, onSendTelegram }) => {
  const market = data?.market;
  const scenarios = data?.scenarios ?? [];
  const primary =
    scenarios.find((scenario) => (scenario.role || '').toUpperCase() === 'PRIMARY') ??
    scenarios[0];
  const alternate =
    scenarios.find(
      (scenario) => (scenario.role || '').toUpperCase() === 'ALTERNATE' && scenario !== primary
    ) ?? scenarios.find((scenario) => scenario !== primary);
  const watchlist = scenarios.filter(
    (scenario) => scenario !== primary && scenario !== alternate
  );
  const [telegramState, setTelegramState] = useState<'idle' | 'sending' | 'sent' | 'error'>(
    'idle'
  );

  const handleSendTelegram = async () => {
    setTelegramState('sending');
    const result = await onSendTelegram();
    setTelegramState(result.success ? 'sent' : 'error');
  };

  return (
    <div
      id="scenario-view"
      className="bg-neutral-900 border border-neutral-800 rounded-xl p-4 flex flex-col gap-4"
    >
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="flex flex-col gap-1">
          <div className="flex flex-wrap items-center gap-2">
            <h3 className="text-sm font-bold text-white tracking-tight uppercase font-mono">
              Kịch bản thị trường — tham khảo, không phải tín hiệu
            </h3>
            <span className="px-1.5 py-0.5 rounded text-[9px] font-mono uppercase tracking-wider bg-neutral-800 text-neutral-400 border border-neutral-700">
              Read-only
            </span>
            <span className="px-1.5 py-0.5 rounded text-[9px] font-mono uppercase tracking-wider bg-sky-500/10 text-sky-300 border border-sky-500/30">
              1 chính + 1 đối chiếu + watchlist
            </span>
          </div>
          <p className="text-[11px] text-neutral-500">
            PRIMARY là kịch bản ưu tiên, ALTERNATE là kịch bản đối chiếu, WATCHLIST là các kịch
            bản phụ. Xếp hạng minh bạch từ confidence, VALID, macro/flow, H4 bias, R:R và khoảng
            cách giá. Không phải entry đã xác nhận.
          </p>
        </div>

        <div className="flex flex-wrap items-center gap-2">
          <button
            onClick={onRefresh}
            className="px-3 py-1.5 text-xs font-semibold rounded-lg border border-neutral-700 bg-neutral-950 text-neutral-300 hover:text-white hover:border-neutral-600 transition-colors cursor-pointer"
            title="Tải lại 4 kịch bản tham khảo (không tốn quota)"
          >
            Làm mới kịch bản
          </button>
          <button
            onClick={() => {
              void handleSendTelegram();
            }}
            disabled={telegramState === 'sending'}
            className="px-3 py-1.5 text-xs font-semibold rounded-lg border border-sky-500/40 bg-sky-500/10 text-sky-300 hover:bg-sky-500/20 transition-colors cursor-pointer disabled:cursor-not-allowed disabled:opacity-60"
            title="Gửi snapshot kịch bản tham khảo tới Telegram (không phải tín hiệu, không tốn quota)"
          >
            {telegramState === 'sending' ? 'Đang gửi...' : 'Gửi Telegram'}
          </button>
          {telegramState === 'sent' && (
            <span className="text-[10px] font-mono text-emerald-400">Đã gửi Telegram</span>
          )}
          {telegramState === 'error' && (
            <span className="text-[10px] font-mono text-rose-400">Chưa gửi được</span>
          )}
        </div>
      </div>

      {data?.warnings && data.warnings.length > 0 && (
        <div className="flex flex-wrap gap-2 text-[10px] font-mono text-amber-300">
          {data.warnings.map((warning, index) => (
            <span
              key={`${warning}-${index}`}
              className="px-2 py-1 rounded-md bg-amber-500/10 border border-amber-500/30"
            >
              {warning}
            </span>
          ))}
        </div>
      )}

      {!data ? (
        <p className="text-xs text-neutral-500 font-mono">Đang tải kịch bản thị trường...</p>
      ) : data.status === 'no_data' ? (
        <div className="rounded-lg border border-neutral-800 bg-neutral-950/60 px-3 py-3 flex flex-col gap-1.5 text-xs font-mono">
          <p className="text-neutral-200">Chưa đủ nến để dựng kịch bản tham khảo.</p>
          <p className="text-neutral-500">
            Card sẽ tự cập nhật khi backend có đủ nến cho lookback hiện tại.
          </p>
        </div>
      ) : (
        <>
          {market && <MarketStrip market={market} currentPrice={data.price} />}
          {(primary || alternate) && (
            <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
              {primary && <ScenarioTile scenario={primary} role="PRIMARY" />}
              {alternate && <ScenarioTile scenario={alternate} role="ALTERNATE" />}
            </div>
          )}
          {watchlist.length > 0 && (
            <details className="rounded-lg border border-neutral-800 bg-neutral-950/40 px-3 py-2">
              <summary className="cursor-pointer text-[11px] font-mono text-neutral-400">
                👀 Watchlist ({watchlist.length}) — kịch bản phụ, chưa phải ưu tiên
              </summary>
              <div className="mt-2 grid grid-cols-1 md:grid-cols-2 gap-1.5">
                {watchlist.map((scenario) => (
                  <WatchlistRow key={scenario.id} scenario={scenario} />
                ))}
              </div>
            </details>
          )}
        </>
      )}

      <div className="flex flex-wrap items-center justify-between gap-2 border-t border-neutral-800 pt-3 text-[10px] font-mono">
        <span className="text-neutral-400">
          Kịch bản tham khảo — không phải entry đã xác nhận. Gửi Telegram chỉ là snapshot tham
          khảo, không phải tín hiệu.
        </span>
        <span className="text-neutral-600">
          {data?.source ? `Nguồn: ${data.source}` : 'Nguồn: —'}
          {data?.volume_source ? ` · Vol: ${data.volume_source}` : ''}
          {data?.generated_at ? ` · ${timeLabel(data.generated_at)}` : ''}
        </span>
      </div>
    </div>
  );
};
