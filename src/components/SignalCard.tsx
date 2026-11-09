'use client';

import React from 'react';
import { Signal } from '../types';

interface Props {
  signal: Signal;
  isSelected?: boolean;
  onSelect: (signal: Signal) => void;
  onOpenDetails: (signal: Signal) => void;
}

export const SignalCard: React.FC<Props> = ({
  signal,
  isSelected,
  onSelect,
  onOpenDetails,
}) => {
  const isBuy = signal.signal_type === 'BUY';

  const statusColor = {
    Active: 'text-amber-400 bg-amber-400/10 border-amber-400/20',
    Target1Hit: 'text-emerald-400 bg-emerald-400/10 border-emerald-400/20',
    Target2Hit: 'text-emerald-400 bg-emerald-400/20 border-emerald-400/40',
    StopLossHit: 'text-rose-400 bg-rose-400/10 border-rose-400/20',
    Cancelled: 'text-neutral-500 bg-neutral-800 border-neutral-700',
  }[signal.status] || 'text-neutral-400 bg-neutral-800 border-neutral-700';

  return (
    <div
      onClick={() => onSelect(signal)}
      className={`p-4 rounded-xl border transition-all cursor-pointer flex flex-col gap-3 ${
        isSelected
          ? 'bg-neutral-850 border-amber-500/50 shadow-md shadow-amber-500/5'
          : 'bg-neutral-900 border-neutral-800 hover:border-neutral-700'
      }`}
    >
      {/* Top Row: Type, Symbol, Timeframe, Status */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2">
          <span
            className={`px-2 py-0.5 rounded text-xs font-bold font-mono tracking-wider ${
              isBuy ? 'bg-emerald-500/20 text-emerald-400' : 'bg-rose-500/20 text-rose-400'
            }`}
          >
            {signal.signal_type}
          </span>
          <span className="text-sm font-semibold text-white font-mono">{signal.symbol}</span>
          <span className="text-xs text-neutral-400 font-mono">· {signal.timeframe}</span>
        </div>

        <div className="flex items-center gap-2">
          <span className={`text-[11px] font-mono font-medium px-2 py-0.5 rounded border ${statusColor}`}>
            {signal.status}
          </span>
          <span className="text-xs text-neutral-500 font-mono">
            {new Date(signal.created_at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}
          </span>
        </div>
      </div>

      {/* Middle Row: Trade Targets */}
      <div className="grid grid-cols-4 gap-2 bg-neutral-950/60 p-2.5 rounded-lg border border-neutral-850 text-xs font-mono">
        <div>
          <div className="text-neutral-500 text-[11px]">Entry</div>
          <div className="text-white font-semibold tabular-nums mt-0.5">${signal.entry_price.toFixed(2)}</div>
        </div>
        <div>
          <div className="text-neutral-500 text-[11px]">Stop Loss</div>
          <div className="text-rose-400 font-semibold tabular-nums mt-0.5">${signal.stop_loss.toFixed(2)}</div>
        </div>
        <div>
          <div className="text-neutral-500 text-[11px]">Take Profit 1</div>
          <div className="text-emerald-400 font-semibold tabular-nums mt-0.5">${signal.take_profit_1.toFixed(2)}</div>
        </div>
        <div>
          <div className="text-neutral-500 text-[11px]">Risk:Reward</div>
          <div className="text-amber-400 font-bold tabular-nums mt-0.5">1:{signal.risk_reward_ratio.toFixed(2)}</div>
        </div>
      </div>

      {/* Position Sizing Recommendation */}
      {signal.recommended_lot_size && (
        <div className="flex items-center justify-between px-2.5 py-1.5 rounded-lg bg-amber-500/10 border border-amber-500/20 text-xs font-mono">
          <span className="text-zinc-400 text-[11px] flex items-center gap-1.5">
            <span className="w-1.5 h-1.5 rounded-full bg-amber-400"></span>
            <span>Khối lượng đề xuất:</span>
          </span>
          <div className="flex items-center gap-2">
            <span className="text-amber-400 font-bold">{signal.recommended_lot_size.toFixed(2)} Lot</span>
            {signal.risk_amount_usd && (
              <span className="text-zinc-400 text-[11px]">
                (Rủi ro: <span className="text-rose-400">-${signal.risk_amount_usd.toFixed(2)}</span> | TP1: <span className="text-emerald-400">+${signal.potential_profit_tp1_usd?.toFixed(2)}</span>)
              </span>
            )}
          </div>
        </div>
      )}

      {/* Rationale & Volume Absorption */}
      <div className="text-xs text-neutral-300 leading-relaxed line-clamp-2">
        {signal.full_rationale}
      </div>

      {/* Footer Details Action */}
      <div className="flex items-center justify-between pt-1 border-t border-neutral-800 text-xs">
        <div className="flex items-center gap-2 text-neutral-400 font-mono text-[11px]">
          <span>Vol: <strong className="text-amber-400">{signal.volume_surge_ratio}x MA</strong></span>
          <span>·</span>
          <span>Risk: <strong className="text-neutral-200">{signal.risk_points.toFixed(1)}$</strong></span>
        </div>

        <button
          onClick={(e) => {
            e.stopPropagation();
            onOpenDetails(signal);
          }}
          className="text-amber-400 hover:text-amber-300 font-medium text-xs transition-colors flex items-center gap-1 cursor-pointer"
        >
          View Telegram Alert
          <span>→</span>
        </button>
      </div>
    </div>
  );
};
