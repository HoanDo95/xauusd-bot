'use client';

import React from 'react';

interface Props {
  onSimulate: (type: 'BUY_PINBAR' | 'SELL_PINBAR' | 'RANDOM_CANDLE') => void;
  isLoading: boolean;
  statusMessage: string | null;
}

export const SimulateToolbar: React.FC<Props> = ({
  onSimulate,
  isLoading,
  statusMessage,
}) => {
  return (
    <div className="bg-neutral-900 border border-neutral-800 rounded-xl p-3 flex flex-col gap-2.5">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-[11px] font-bold text-amber-300 bg-amber-500/10 border border-amber-500/30 rounded-md px-2 py-1 font-mono">
            🧪 [SIMULATION - Không tốn Quota 1 lệnh/ngày]
          </span>

          <button
            onClick={() => onSimulate('BUY_PINBAR')}
            disabled={isLoading}
            className="px-3 py-1.5 bg-emerald-500/10 hover:bg-emerald-500/20 border border-emerald-500/30 text-emerald-400 rounded-lg text-xs font-semibold transition-colors flex items-center gap-1.5 cursor-pointer disabled:opacity-50"
          >
            <span className="w-1.5 h-1.5 rounded-full bg-emerald-400"></span>
            [SIMULATION] Bullish Pinbar at Support
          </button>

          <button
            onClick={() => onSimulate('SELL_PINBAR')}
            disabled={isLoading}
            className="px-3 py-1.5 bg-rose-500/10 hover:bg-rose-500/20 border border-rose-500/30 text-rose-400 rounded-lg text-xs font-semibold transition-colors flex items-center gap-1.5 cursor-pointer disabled:opacity-50"
          >
            <span className="w-1.5 h-1.5 rounded-full bg-rose-400"></span>
            [SIMULATION] Bearish Pinbar at Resistance
          </button>

          <button
            onClick={() => onSimulate('RANDOM_CANDLE')}
            disabled={isLoading}
            className="px-3 py-1.5 bg-neutral-800 hover:bg-neutral-700 border border-neutral-700 text-neutral-300 rounded-lg text-xs font-medium transition-colors cursor-pointer disabled:opacity-50"
          >
            [SIMULATION] Normal Candle
          </button>
        </div>

        <span className="text-[11px] text-neutral-500 font-mono">
          Manual sandbox · không ảnh hưởng quota lệnh thật
        </span>
      </div>

      {statusMessage && (
        <div className="text-xs font-mono text-neutral-400 truncate">
          {statusMessage}
        </div>
      )}
    </div>
  );
};
