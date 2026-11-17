'use client';

import React from 'react';
import { ArrowDownRight, ArrowUpRight, Minus, Newspaper, Sparkles } from 'lucide-react';
import { NewsImpactRecord, NewsImpactResponse } from '../types';

interface Props {
  data: NewsImpactResponse | null;
}

function formatPoints(value?: number | null): string {
  if (typeof value !== 'number' || !Number.isFinite(value)) return 'n/a';
  return `${value >= 0 ? '+' : ''}${value.toFixed(2)}`;
}

function formatPct(value?: number | null): string {
  if (typeof value !== 'number' || !Number.isFinite(value)) return 'n/a';
  return `${(value * 100).toFixed(1)}%`;
}

function formatTime(value: string): string {
  const parsed = new Date(value);
  if (Number.isNaN(parsed.getTime())) return value;
  return parsed.toLocaleString('vi-VN', {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  });
}

function reactionClass(reaction: string): string {
  switch ((reaction || '').toUpperCase()) {
    case 'UP':
      return 'bg-emerald-500/15 text-emerald-300 border-emerald-500/40';
    case 'DOWN':
      return 'bg-rose-500/15 text-rose-300 border-rose-500/40';
    case 'FLAT':
      return 'bg-sky-500/15 text-sky-300 border-sky-500/40';
    default:
      return 'bg-neutral-950 text-neutral-500 border-neutral-800';
  }
}

function leanClass(lean: string): string {
  switch ((lean || '').toUpperCase()) {
    case 'UP':
      return 'text-emerald-300';
    case 'DOWN':
      return 'text-rose-300';
    default:
      return 'text-neutral-500';
  }
}

function reactionIcon(reaction: string) {
  switch ((reaction || '').toUpperCase()) {
    case 'UP':
      return <ArrowUpRight className="w-3 h-3" />;
    case 'DOWN':
      return <ArrowDownRight className="w-3 h-3" />;
    default:
      return <Minus className="w-3 h-3" />;
  }
}

function recordTitle(record: NewsImpactRecord): string {
  return [
    record.conditional_reason,
    record.reaction_reason,
    record.ai?.reasoning ? `AI: ${record.ai.reasoning}` : '',
  ]
    .filter(Boolean)
    .join(' · ');
}

const EVENT_CLASS_LABELS: Record<string, string> = {
  INFLATION: 'Lạm phát',
  LABOR: 'Lao động',
  GROWTH: 'Tăng trưởng',
  CONSUMPTION: 'Tiêu dùng',
  HOUSING: 'Nhà ở',
  MANUFACTURING: 'Sản xuất',
  SENTIMENT: 'Tâm lý',
  FED: 'Fed',
  GEOPOLITICAL: 'Địa chính trị',
  OTHER: 'Khác',
};

export const NewsImpactCard: React.FC<Props> = ({ data }) => {
  const summary = data?.summary;
  const records = data?.records ?? [];
  const enabled = data?.enabled ?? true;

  return (
    <div className="rounded-xl border border-neutral-800 bg-neutral-900 p-4 flex flex-col gap-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex items-center gap-2">
          <Newspaper className="w-4 h-4 text-amber-400" />
          <span className="text-xs font-bold uppercase tracking-wider text-white">
            News Impact — đo phản ứng vàng sau tin
          </span>
          <span className="px-1.5 py-0.5 rounded border border-amber-500/30 bg-amber-500/10 text-[9px] font-mono font-bold uppercase text-amber-300">
            Advisory
          </span>
        </div>
        <span className="text-[10px] text-neutral-500">
          {enabled ? 'Worker 5 phút · không ảnh hưởng tín hiệu' : 'Đang tắt trong cấu hình'}
        </span>
      </div>

      {!enabled ? (
        <p className="text-[11px] text-neutral-500">
          News Impact đang tắt. Bật trong Cấu hình → Sniper &amp; Risk Settings.
        </p>
      ) : (
        <>
          <div className="grid grid-cols-2 md:grid-cols-4 gap-2">
            <div className="rounded-lg border border-neutral-800 bg-neutral-950/60 px-3 py-2">
              <div className="text-[9px] font-mono uppercase tracking-wider text-neutral-500">
                Đã đo
              </div>
              <div className="text-lg font-bold font-mono text-white tabular-nums">
                {summary?.measured_events ?? 0}
                <span className="text-[10px] text-neutral-500 font-normal">
                  /{summary?.total_events ?? 0}
                </span>
              </div>
            </div>
            <div className="rounded-lg border border-neutral-800 bg-neutral-950/60 px-3 py-2">
              <div className="text-[9px] font-mono uppercase tracking-wider text-neutral-500">
                UP / DOWN / FLAT
              </div>
              <div className="text-sm font-bold font-mono text-white tabular-nums">
                <span className="text-emerald-300">{summary?.up ?? 0}</span>
                <span className="text-neutral-600"> / </span>
                <span className="text-rose-300">{summary?.down ?? 0}</span>
                <span className="text-neutral-600"> / </span>
                <span className="text-sky-300">{summary?.flat ?? 0}</span>
              </div>
            </div>
            <div className="rounded-lg border border-neutral-800 bg-neutral-950/60 px-3 py-2">
              <div className="text-[9px] font-mono uppercase tracking-wider text-neutral-500">
                Avg |+60m|
              </div>
              <div className="text-sm font-bold font-mono text-white tabular-nums">
                {formatPoints(summary?.avg_abs_move_60m)}
                <span className="text-[10px] text-neutral-500 font-normal"> pts</span>
              </div>
            </div>
            <div className="rounded-lg border border-neutral-800 bg-neutral-950/60 px-3 py-2">
              <div className="text-[9px] font-mono uppercase tracking-wider text-neutral-500">
                Lean hit / AI hit
              </div>
              <div className="text-sm font-bold font-mono text-white tabular-nums">
                {formatPct(summary?.conditional_hit_rate)}
                <span className="text-[9px] text-neutral-500 font-normal">
                  {' '}
                  ({summary?.conditional_correct ?? 0}/{summary?.conditional_total ?? 0})
                </span>
                <span className="text-neutral-600"> / </span>
                <span className="text-amber-300">{formatPct(summary?.ai_hit_rate)}</span>
                <span className="text-[9px] text-neutral-500 font-normal">
                  {' '}
                  ({summary?.ai_correct ?? 0}/{summary?.ai_total ?? 0})
                </span>
              </div>
            </div>
          </div>

          {records.length === 0 ? (
            <p className="text-[11px] text-neutral-500">
              Chưa có sự kiện nào được đo. Worker sẽ ghi nhận sau khi có tin High/Medium USD đi qua
              và đủ nến M15.
            </p>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full text-left text-[11px]">
                <thead>
                  <tr className="text-neutral-500 border-b border-neutral-800">
                    <th className="py-1.5 pr-2 font-medium">Thời gian</th>
                    <th className="py-1.5 pr-2 font-medium">Sự kiện</th>
                    <th className="py-1.5 pr-2 font-medium">Actual / Forecast</th>
                    <th className="py-1.5 pr-2 font-medium">Lean</th>
                    <th className="py-1.5 pr-2 font-medium">Reaction</th>
                    <th className="py-1.5 pr-2 font-medium text-right">+60m</th>
                    <th className="py-1.5 font-medium">AI</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-neutral-900">
                  {records.slice(0, 8).map((record) => (
                    <tr
                      key={record.event_id}
                      className="text-neutral-300 align-top"
                      title={recordTitle(record)}
                    >
                      <td className="py-2 pr-2 whitespace-nowrap text-neutral-500 font-mono">
                        {formatTime(record.scheduled_time)}
                      </td>
                      <td className="py-2 pr-2">
                        <div className="font-medium text-white max-w-[240px] truncate">
                          {record.title}
                        </div>
                        <div className="text-[9px] text-neutral-500 font-mono">
                          {EVENT_CLASS_LABELS[record.event_class] ?? record.event_class} ·{' '}
                          {record.gold_relevance}
                        </div>
                      </td>
                      <td className="py-2 pr-2 font-mono whitespace-nowrap">
                        <span className="text-white">{record.actual ?? 'n/a'}</span>
                        <span className="text-neutral-600"> / </span>
                        <span className="text-neutral-400">{record.forecast ?? 'n/a'}</span>
                      </td>
                      <td className={`py-2 pr-2 font-mono font-bold ${leanClass(record.conditional_lean)}`}>
                        {record.conditional_lean}
                      </td>
                      <td className="py-2 pr-2">
                        <span
                          className={`inline-flex items-center gap-1 px-1.5 py-0.5 rounded border text-[9px] font-mono font-bold ${reactionClass(
                            record.reaction
                          )}`}
                        >
                          {reactionIcon(record.reaction)}
                          {record.reaction}
                        </span>
                      </td>
                      <td
                        className={`py-2 pr-2 text-right font-mono tabular-nums ${
                          (record.return_60m ?? 0) >= 0 ? 'text-emerald-300' : 'text-rose-300'
                        }`}
                      >
                        {formatPoints(record.return_60m)}
                      </td>
                      <td className="py-2">
                        {record.ai ? (
                          <span className="inline-flex items-center gap-1 text-[9px] font-mono">
                            <Sparkles className="w-3 h-3 text-amber-400" />
                            <span className={leanClass(record.ai.gold_lean)}>
                              {record.ai.gold_lean}
                            </span>
                            <span className="text-neutral-500">{record.ai.confidence}%</span>
                          </span>
                        ) : (
                          <span className="text-[9px] text-neutral-600">n/a</span>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}

          <p className="text-[10px] leading-relaxed text-neutral-500">
            Chỉ đo lường, không dùng để chặn/duyệt lệnh. Reaction lấy từ nến M15 đã đóng; ngưỡng
            UP/DOWN = 0.3× H4 ATR. AI chỉ chạy sau khi có actual, tối đa{' '}
            {data?.aiDailyCap ?? 6} sự kiện/ngày. Nguồn volume:{' '}
            <span className="text-neutral-400">{records[0]?.volume_source ?? 'n/a'}</span>.
            {(summary?.measured_events ?? 0) < 30 && (
              <span className="text-amber-300/80">
                {' '}
                Mẫu còn nhỏ (&lt;30 sự kiện) — chưa đủ để kết luận hit rate.
              </span>
            )}
          </p>
        </>
      )}
    </div>
  );
};
