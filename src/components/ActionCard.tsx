'use client';

import React, { useState } from 'react';
import { ActionVerdict, ActionViewResponse, AiDecision, EntryCheckResult } from '../types';
import { runAiEntryCheck } from '../lib/api';

interface Props {
  data: ActionViewResponse | null;
  /** Latest AI Decision (DECISION mode). Display-only; deterministic verdict stays primary. */
  aiDecision?: AiDecision | null;
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

function triggerCondition(direction: string, triggerPrice: number): string {
  const value = triggerPrice.toFixed(2);
  switch ((direction || '').toUpperCase()) {
    case 'ABOVE':
      return `M15 close > ${value}`;
    case 'BELOW':
      return `M15 close < ${value}`;
    case 'TOUCH':
      return `M15 touch ${value}`;
    default:
      return `M15 ${value}`;
  }
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

function aiActionTone(action: string): string {
  switch ((action || '').toUpperCase()) {
    case 'BUY':
      return 'bg-emerald-500/15 text-emerald-300 border-emerald-500/40';
    case 'SELL':
      return 'bg-rose-500/15 text-rose-300 border-rose-500/40';
    case 'STANDBY':
      return 'bg-amber-500/15 text-amber-300 border-amber-500/40';
    default:
      return 'bg-neutral-800 text-neutral-300 border-neutral-700';
  }
}

function proximityTone(proximity: string): string {
  switch ((proximity || '').toUpperCase()) {
    case 'NEAR':
      return 'bg-emerald-500/15 text-emerald-300 border-emerald-500/40';
    case 'MID':
      return 'bg-amber-500/15 text-amber-300 border-amber-500/40';
    case 'FAR':
      return 'bg-rose-500/15 text-rose-300 border-rose-500/40';
    default:
      return 'bg-neutral-800 text-neutral-400 border-neutral-700';
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
export const ActionCard: React.FC<Props> = ({ data, aiDecision }) => {
  const [entryCheck, setEntryCheck] = useState<EntryCheckResult | null>(null);
  const [entryCheckError, setEntryCheckError] = useState<string | null>(null);
  const [entryCheckLoading, setEntryCheckLoading] = useState(false);

  const handleEntryCheck = async () => {
    setEntryCheckLoading(true);
    setEntryCheckError(null);
    const response = await runAiEntryCheck();
    setEntryCheckLoading(false);
    if (response.status === 'ok' && response.result) {
      setEntryCheck(response.result);
      return;
    }
    setEntryCheck(null);
    setEntryCheckError(response.message || 'AI Entry Check không khả dụng.');
  };

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
  const ai = aiDecision;
  const aiConfidence =
    ai && Number.isFinite(ai.confidence_score)
      ? `${Math.round(Math.min(1, Math.max(0, ai.confidence_score)) * 100)}%`
      : '—';
  const aiAction = (ai?.action || '').toUpperCase();
  const aiDiffersFromVerdict =
    (aiAction === 'BUY' || aiAction === 'SELL') && aiAction !== data.verdict;
  const entryCheckAction = (entryCheck?.decision.action || '').toUpperCase();
  const entryCheckConfidence =
    entryCheck && Number.isFinite(entryCheck.decision.confidence_score)
      ? `${Math.round(
          Math.min(1, Math.max(0, entryCheck.decision.confidence_score)) * 100
        )}%`
      : '—';
  const entryPlan = entryCheck?.entry_plan ?? null;
  const proximity = (data.proximity || 'UNKNOWN').toUpperCase();
  const distanceLabel = price(data.distanceToTrigger);
  const distanceAtrLabel =
    typeof data.distanceAtr === 'number' && Number.isFinite(data.distanceAtr)
      ? `${data.distanceAtr.toFixed(2)} ATR`
      : '— ATR';

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
        <div className="flex flex-col items-end gap-2">
          <span className="text-[10px] font-mono text-neutral-600">
            {data.generated_at ? `Cập nhật ${timeLabel(data.generated_at)}` : 'Chưa có thời điểm'}
          </span>
          <button
            type="button"
            onClick={handleEntryCheck}
            disabled={entryCheckLoading}
            className="px-2.5 py-1 rounded-md border border-indigo-500/40 bg-indigo-500/10 text-[10px] font-mono font-bold uppercase tracking-wider text-indigo-200 hover:bg-indigo-500/20 disabled:cursor-wait disabled:opacity-60 transition-colors"
          >
            {entryCheckLoading ? 'Đang check…' : 'AI Entry Check'}
          </button>
        </div>
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

      {ai && (
        <div className="flex flex-col gap-1.5 rounded-lg border border-indigo-500/30 bg-indigo-500/5 px-3 py-2.5">
          <div className="flex flex-wrap items-center gap-2 text-[11px] font-mono">
            <span
              className={`px-2 py-0.5 rounded border font-bold ${aiActionTone(aiAction)}`}
            >
              🧠 AI: {aiAction || 'UNKNOWN'} · {aiConfidence}
            </span>
            {ai.selected_scenario && (
              <span className="text-neutral-400">
                Selected <strong className="text-neutral-200">{ai.selected_scenario}</strong>
              </span>
            )}
            {ai.macro_flow_alignment && (
              <span className="text-neutral-400">
                Macro <strong className="text-neutral-200">{ai.macro_flow_alignment}</strong>
              </span>
            )}
          </div>
          <p className="text-[11px] text-neutral-300 leading-relaxed">
            <span className="text-neutral-500">Reason:</span> {ai.reason}
          </p>
          {ai.execution_rule && (
            <p className="text-[10px] text-neutral-400">
              <span className="text-neutral-500">Execution rule:</span> {ai.execution_rule}
            </p>
          )}
          {aiDiffersFromVerdict && (
            <p className="text-[10px] font-mono text-amber-300">
              AI {aiAction} khác verdict deterministic {data.verdict} — hiển thị cả hai, không tự
              động override.
            </p>
          )}
          <p className="text-[9px] text-neutral-500">
            AI là lớp quyết định chính; code vẫn giữ hard risk (SL/TP, R:R, risk budget, quota,
            session, news, dedup). Không auto-trade.
          </p>
        </div>
      )}

      {entryCheckError && (
        <div className="rounded-lg border border-rose-500/30 bg-rose-500/5 px-3 py-2 text-[11px] font-mono font-semibold text-rose-300">
          ⚠️ {entryCheckError}
        </div>
      )}

      {entryCheck && (
        <div className="flex flex-col gap-2 rounded-lg border border-indigo-500/30 bg-indigo-500/5 px-3 py-2.5">
          <div className="flex flex-wrap items-center gap-2 text-[11px] font-mono">
            <span
              className={`px-2 py-0.5 rounded border font-bold ${aiActionTone(
                entryCheckAction
              )}`}
            >
              🧠 AI Entry Check: {entryCheckAction || 'UNKNOWN'} · {entryCheckConfidence}
            </span>
            {entryCheck.decision.selected_scenario && (
              <span className="text-neutral-400">
                Selected{' '}
                <strong className="text-neutral-200">
                  {entryCheck.decision.selected_scenario}
                </strong>
              </span>
            )}
            {entryPlan && (
              <span
                className={`px-1.5 py-0.5 rounded border text-[9px] font-bold ${
                  entryPlan.valid
                    ? 'bg-emerald-500/15 text-emerald-300 border-emerald-500/40'
                    : 'bg-amber-500/10 text-amber-300 border-amber-500/30'
                }`}
              >
                Plan {entryPlan.source} · {entryPlan.valid ? 'VALID' : 'INVALID'}
                {entryPlan.readiness != null ? ` · readiness ${entryPlan.readiness}%` : ''}
              </span>
            )}
            {entryPlan?.early_entry && (
              <span className="px-1.5 py-0.5 rounded border text-[9px] font-bold bg-indigo-500/15 text-indigo-300 border-indigo-500/40">
                EARLY · trigger chưa xác nhận
              </span>
            )}
          </div>
          <p className="text-[11px] text-neutral-300 leading-relaxed">
            <span className="text-neutral-500">Reason:</span> {entryCheck.decision.reason}
          </p>
          {entryPlan ? (
            <>
              <div className="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-6 gap-3 font-mono">
                <Metric label="Entry" value={price(entryPlan.entry)} />
                <Metric
                  label="Stop Loss"
                  value={price(entryPlan.stop_loss)}
                  valueClass="text-rose-400"
                />
                <Metric
                  label="TP1"
                  value={price(entryPlan.take_profit_1)}
                  valueClass="text-emerald-400"
                />
                <Metric
                  label="TP2"
                  value={price(entryPlan.take_profit_2)}
                  valueClass="text-emerald-300"
                />
                <Metric
                  label="R:R"
                  value={
                    Number.isFinite(entryPlan.risk_reward)
                      ? `1:${entryPlan.risk_reward.toFixed(2)}`
                      : '—'
                  }
                  valueClass="text-sky-300"
                />
                <Metric label="Side" value={entryPlan.side} />
              </div>
              <p className="text-[10px] text-neutral-400 leading-relaxed">
                <span className="text-neutral-500">Note:</span> {entryPlan.note}
              </p>
            </>
          ) : (
            <p className="text-[10px] font-mono text-neutral-500">
              AI không chọn candidate/scenario có level hợp lệ — không có entry plan tham khảo.
            </p>
          )}
          <p className="text-[9px] text-neutral-500">
            Check thủ công — không phải trade signal tự động; không gửi Telegram, không lưu tín
            hiệu, không tiêu hao quota.
          </p>
        </div>
      )}

      <div className="flex flex-col gap-1">
        <div
          className={`inline-flex w-fit flex-wrap items-center gap-2 rounded-md border px-2.5 py-1 text-[11px] font-mono font-bold ${proximityTone(
            proximity
          )}`}
        >
          <span>Cách trigger: {distanceLabel}</span>
          <span>· {distanceAtrLabel}</span>
          <span>· {proximity}</span>
        </div>
        <p className="text-[10px] text-neutral-500">
          NEAR = có thể canh; MID/FAR = chờ trigger, không đuổi.
        </p>
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

          {typeof primary.trigger_price === 'number' &&
            Number.isFinite(primary.trigger_price) && (
              <p className="text-[10px] font-mono text-neutral-400">
                Trigger alert:{' '}
                <span
                  className={`font-bold ${
                    (data.triggerAlertEnabled ?? true) ? 'text-emerald-300' : 'text-neutral-500'
                  }`}
                >
                  {(data.triggerAlertEnabled ?? true) ? 'ON' : 'OFF'}
                </span>{' '}
                · {triggerCondition(primary.trigger_direction, primary.trigger_price)}
              </p>
            )}

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
