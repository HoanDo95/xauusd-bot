'use client';

import React, { useState } from 'react';
import { NewsCalendarData } from '../types';

interface Props {
  newsData: NewsCalendarData | null;
  onToggleKillSwitch: () => void;
}

export const NewsKillSwitchBanner: React.FC<Props> = ({
  newsData,
  onToggleKillSwitch,
}) => {
  const [showCalendar, setShowCalendar] = useState(false);

  if (!newsData) return null;

  const isBlocked = newsData.isKillSwitchActive;
  const effectiveBias =
    newsData.effectiveBias ?? (newsData.macroBias === 'AUTO' ? 'NEUTRAL' : newsData.macroBias);
  const macroBiasColor = {
    BULLISH: 'text-emerald-400 bg-emerald-500/10 border-emerald-500/30',
    BEARISH: 'text-rose-400 bg-rose-500/10 border-rose-500/30',
    NEUTRAL: 'text-sky-400 bg-sky-500/10 border-sky-500/30',
  }[effectiveBias] || 'text-neutral-400 bg-neutral-800 border-neutral-700';

  return (
    <div className="flex flex-col gap-2">
      {/* Banner Strip */}
      <div
        className={`px-4 py-2.5 rounded-xl border flex flex-wrap items-center justify-between gap-3 text-xs transition-colors ${
          isBlocked
            ? 'bg-rose-950/40 border-rose-600/50 text-rose-200'
            : 'bg-neutral-900 border-neutral-800 text-neutral-300'
        }`}
      >
        {/* Left: Kill Switch Status */}
        <div className="flex items-center gap-3">
          <div className="flex items-center gap-2">
            <span
              className={`w-2 h-2 rounded-full ${
                isBlocked
                  ? 'bg-rose-500 animate-ping'
                  : newsData.killSwitchEnabled
                  ? 'bg-emerald-500'
                  : 'bg-neutral-500'
              }`}
            ></span>
            <span className="font-bold font-mono uppercase tracking-wider text-white">
              {isBlocked ? 'KILL SWITCH ACTIVE' : 'MACRO & NEWS SHIELD'}
            </span>
          </div>

          <span className="text-neutral-400 hidden sm:inline">|</span>

          <span className="text-neutral-300 line-clamp-1 font-mono text-[11px]">
            {newsData.killSwitchReason}
          </span>
        </div>

        {/* Right: Macro Bias Badge & Calendar Toggle */}
        <div className="flex items-center gap-3">
          <div className="flex items-center gap-1.5 font-mono text-[11px]">
            <span className="text-neutral-500">Macro Bias:</span>
            <span className={`px-2 py-0.5 rounded font-bold border ${macroBiasColor}`}>
              {newsData.macroBias === 'AUTO' ? `AUTO → ${effectiveBias}` : newsData.macroBias}
            </span>
            {typeof newsData.ema50 === 'number' && newsData.ema50 > 0 && (
              <span className="text-neutral-500 hidden md:inline">
                EMA50 ${newsData.ema50.toFixed(1)}
              </span>
            )}
          </div>

          <button
            onClick={() => setShowCalendar(!showCalendar)}
            className="text-amber-400 hover:text-amber-300 font-mono text-[11px] underline cursor-pointer"
          >
            {showCalendar ? 'Ẩn Lịch Tin' : 'Xem Lịch Tin Kinh Tế (CPI/NFP/FOMC)'}
          </button>
        </div>
      </div>

      {/* Expandable Economic Calendar Box */}
      {showCalendar && (
        <div className="bg-neutral-950 border border-neutral-800 rounded-xl p-4 flex flex-col gap-3 text-xs font-mono">
          <div className="flex items-center justify-between border-b border-neutral-850 pb-2">
            <span className="text-white font-bold">LỊCH SỰ KIỆN KINH TẾ QUAN TRỌNG (USD IMPACT)</span>
            <span className="text-neutral-500 text-[11px]">
              Tự động tạm dừng tín hiệu 30 phút trước và sau tin đỏ
            </span>
          </div>

          <div className="overflow-x-auto">
            <table className="w-full text-left">
              <thead>
                <tr className="text-neutral-500 border-b border-neutral-900 pb-1">
                  <th className="py-1">Sự kiện</th>
                  <th className="py-1">Thời gian</th>
                  <th className="py-1">Tác động</th>
                  <th className="py-1 text-right">Dự báo</th>
                  <th className="py-1 text-right">Trước đó</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-neutral-900">
                {newsData.events.map((ev) => {
                  const evDate = new Date(ev.scheduled_time);
                  const isHigh = ev.impact === 'High';
                  return (
                    <tr key={ev.id} className="text-neutral-300">
                      <td className="py-2 font-medium text-white">{ev.title}</td>
                      <td className="py-2 text-neutral-400">
                        {evDate.toLocaleDateString([], { month: 'numeric', day: 'numeric' })}{' '}
                        {evDate.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}
                      </td>
                      <td className="py-2">
                        <span
                          className={`px-1.5 py-0.5 rounded text-[10px] font-bold ${
                            isHigh ? 'bg-rose-500/20 text-rose-400' : 'bg-amber-500/20 text-amber-400'
                          }`}
                        >
                          {ev.impact}
                        </span>
                      </td>
                      <td className="py-2 text-right text-neutral-400">{ev.forecast || '—'}</td>
                      <td className="py-2 text-right text-neutral-500">{ev.previous || '—'}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </div>
      )}
    </div>
  );
};
