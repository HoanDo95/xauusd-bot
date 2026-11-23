'use client';

import React, { useMemo, useState } from 'react';
import { MarketOutlook, OutlookScenario } from '../types';
import { isReferenceOnlyScenario } from '../lib/scenarioQuality';

interface Props {
  data: MarketOutlook | null;
}

function price(value: number | null | undefined): string {
  if (typeof value !== 'number' || !Number.isFinite(value)) return '—';
  return value.toFixed(2);
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

function planStatusTone(status: string): string {
  switch (status) {
    case 'TRIGGERED':
      return 'border-amber-500/40 bg-amber-500/10 text-amber-200';
    case 'RETEST_WINDOW':
      return 'border-sky-500/40 bg-sky-500/10 text-sky-200';
    case 'INVALIDATED':
      return 'border-rose-500/40 bg-rose-500/10 text-rose-200';
    case 'NO_TRADE':
      return 'border-neutral-700 bg-neutral-900 text-neutral-400';
    default:
      return 'border-emerald-500/40 bg-emerald-500/10 text-emerald-200';
  }
}

function planLine(label: string, scenario: OutlookScenario | null): string {
  if (!scenario) return `${label}: chưa có kịch bản hợp lệ.`;
  const reference = isReferenceOnlyScenario(scenario.scenario_id) ? 'REFERENCE — ' : '';
  return [
    `${label}: ${reference}${actionLabel(scenario.action)}`,
    `Zone ${price(scenario.zone_low)}–${price(scenario.zone_high)}`,
    `Trigger ${scenario.trigger}`,
    `SL20 ${price(scenario.stop_loss)}`,
    `TP1 20 ${price(scenario.take_profit_1)}`,
    `TP2 40–50 ${price(scenario.take_profit_2)}`,
    `Conf ${scenario.confidence}%`,
    `Invalidation: ${scenario.invalidation}`,
  ].join(' · ');
}

const PlanBlock: React.FC<{
  label: string;
  scenario: OutlookScenario | null;
  tone: 'primary' | 'alternative';
}> = ({ label, scenario, tone }) => {
  if (!scenario) {
    return (
      <div className="rounded-lg border border-neutral-800 bg-neutral-950/60 p-3 text-[11px] text-neutral-500">
        {label}: chưa có kịch bản hợp lệ.
      </div>
    );
  }
  const referenceOnly = isReferenceOnlyScenario(scenario.scenario_id);
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
    <div className={`rounded-lg border ${border} p-3 flex flex-col gap-2`}>
      <div className="flex flex-wrap items-center justify-between gap-2">
        <span className="text-[10px] font-mono font-bold uppercase tracking-wider text-neutral-400">
          {label}
        </span>
        <span className="flex items-center gap-2">
          {referenceOnly && (
            <span className="px-1.5 py-0.5 rounded border text-[9px] font-mono font-bold bg-amber-500/10 text-amber-300 border-amber-500/30">
              REFERENCE ONLY
            </span>
          )}
          <span className={`text-[11px] font-black font-mono ${actionTone}`}>
            {actionLabel(scenario.action)}
          </span>
        </span>
      </div>
      <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1 text-[11px] font-mono">
        <span className="text-neutral-500">Zone</span>
        <span className="text-white font-bold tabular-nums">
          {price(scenario.zone_low)}–{price(scenario.zone_high)}
        </span>
        <span className="text-neutral-500">Conf</span>
        <span className="text-amber-300 font-bold tabular-nums">{scenario.confidence}%</span>
      </div>
      <p className="text-[11px] text-neutral-300 leading-relaxed">{scenario.trigger}</p>
      <div className="grid grid-cols-2 md:grid-cols-4 gap-2 text-[10px] font-mono">
        <div>
          <div className="text-neutral-500 uppercase">SL 20</div>
          <div className="text-rose-300 font-bold tabular-nums">{price(scenario.stop_loss)}</div>
        </div>
        <div>
          <div className="text-neutral-500 uppercase">TP1 20</div>
          <div className="text-emerald-300 font-bold tabular-nums">
            {price(scenario.take_profit_1)}
          </div>
        </div>
        <div>
          <div className="text-neutral-500 uppercase">TP2 40–50</div>
          <div className="text-emerald-300 font-bold tabular-nums">
            {price(scenario.take_profit_2)}
          </div>
        </div>
        <div>
          <div className="text-neutral-500 uppercase">Invalidation</div>
          <div className="text-neutral-300 leading-snug">{scenario.invalidation}</div>
        </div>
      </div>
      {referenceOnly && (
        <p className="text-[9px] text-amber-300/90 leading-relaxed">
          Reference only — backtest 2022–2026 âm; không phải qualified PRIMARY.
        </p>
      )}
    </div>
  );
};

/**
 * Compact, copyable discretionary trade plan from the always-on Market View.
 * Read-only: never a sniper signal, never an automatic order.
 */
export const TradePlanCard: React.FC<Props> = ({ data }) => {
  const [copied, setCopied] = useState(false);

  const copyText = useMemo(() => {
    if (!data) return '';
    return [
      `XAU/USD Trade Plan — ${data.bot_state}`,
      `Plan status: ${data.plan_status ?? 'ACTIVE'} · valid until ${data.valid_until}`,
      `Structure: ${data.structure} · Session: ${data.session} · D1 ${data.d1_bias} / H4 ${data.h4_bias} / H1 ${data.h1_bias}`,
      `Price: ${price(data.price)} · H4 ATR: ${price(data.h4_atr)}`,
      data.no_trade_reason ? `NO_TRADE: ${data.no_trade_reason}` : '',
      planLine('PRIMARY', data.primary),
      data.alternative ? planLine('ALTERNATIVE', data.alternative) : '',
      'Discretionary plan — not a sniper confirmation. Manage risk/size.',
    ]
      .filter(Boolean)
      .join('\n');
  }, [data]);

  const handleCopy = async () => {
    if (!copyText) return;
    try {
      await navigator.clipboard.writeText(copyText);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch {
      setCopied(false);
    }
  };

  if (!data) {
    return (
      <div className="rounded-xl border border-neutral-800 bg-neutral-900 p-4">
        <h3 className="text-sm font-bold text-white tracking-tight uppercase font-mono">
          📋 Trade Plan hôm nay
        </h3>
        <p className="text-xs text-neutral-500 font-mono mt-1">Đang tải Market View...</p>
      </div>
    );
  }

  return (
    <div className="rounded-xl border border-sky-500/30 bg-sky-500/5 p-4 flex flex-col gap-3">
      <div className="flex flex-wrap items-start justify-between gap-2">
        <div className="flex flex-col gap-0.5">
          <div className="flex flex-wrap items-center gap-2">
            <h3 className="text-sm font-bold text-white tracking-tight uppercase font-mono">
              📋 Trade Plan hôm nay
            </h3>
            <span className="px-1.5 py-0.5 rounded border border-sky-500/40 bg-sky-500/10 text-[9px] font-mono font-bold uppercase tracking-wider text-sky-200">
              Discretionary
            </span>
            <span className="px-1.5 py-0.5 rounded border border-neutral-700 bg-neutral-900 text-[9px] font-mono uppercase tracking-wider text-neutral-400">
              {data.bot_state}
            </span>
            <span
              className={`px-1.5 py-0.5 rounded border text-[9px] font-mono font-bold uppercase tracking-wider ${planStatusTone(
                data.plan_status ?? 'ACTIVE'
              )}`}
            >
              {data.plan_status ?? 'ACTIVE'}
            </span>
          </div>
          <p className="text-[11px] text-neutral-400">
            Kế hoạch tham khảo từ Market View · không phải sniper signal · không tự vào lệnh
          </p>
        </div>
        <button
          type="button"
          onClick={() => {
            void handleCopy();
          }}
          className="px-2.5 py-1 rounded-md border border-sky-500/40 bg-sky-500/10 text-[10px] font-mono font-bold uppercase tracking-wider text-sky-200 hover:bg-sky-500/20 transition-colors"
        >
          {copied ? 'Đã copy' : 'Copy plan'}
        </button>
      </div>

      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[10px] font-mono text-neutral-400">
        <span>{data.structure}</span>
        <span>·</span>
        <span>Session {data.session}</span>
        <span>·</span>
        <span>D1 {data.d1_bias}</span>
        <span>·</span>
        <span>H4 {data.h4_bias}</span>
        <span>·</span>
        <span>H1 {data.h1_bias}</span>
        <span>·</span>
        <span>Giá {price(data.price)}</span>
        {data.score ? (
          <>
            <span>·</span>
            <span>
              Score <span className="text-white font-bold">{data.score.total}/100</span>
            </span>
          </>
        ) : null}
      </div>

      {data.no_trade_reason ? (
        <div className="rounded-lg border border-rose-500/40 bg-rose-500/10 px-3 py-2 text-[11px] text-rose-200">
          ⛔ <strong>NO_TRADE:</strong> {data.no_trade_reason}
        </div>
      ) : null}

      <PlanBlock label="🎯 Primary" scenario={data.primary} tone="primary" />
      {data.alternative ? (
        <PlanBlock label="🔄 Alternative" scenario={data.alternative} tone="alternative" />
      ) : null}

      <p className="text-[10px] text-neutral-500 font-mono">
        Trigger chỉ có hiệu lực khi nến M15 đóng cửa xác nhận. Tự quản lý risk/size; đây không phải
        lời khuyên đầu tư.
      </p>
    </div>
  );
};
