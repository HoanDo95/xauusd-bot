'use client';

import React, { useEffect, useMemo, useState } from 'react';
import { Candle } from '../types';
import { fetchHtfCandles } from '../lib/api';

const MAX_DISPLAY_CANDLES = 60;
const REFRESH_MS = 5 * 60 * 1000;

/**
 * Closed H4 candles from the Rust HTF feed. Read-only "view toàn cảnh":
 * it never triggers an entry and is not part of the M15 signal logic.
 */
export const HtfChartCard: React.FC = () => {
  const [candles, setCandles] = useState<Candle[]>([]);
  const [updatedAt, setUpdatedAt] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;

    const load = async () => {
      try {
        const data = await fetchHtfCandles('H4', 120);
        if (cancelled) return;
        setCandles(data.candles);
        setUpdatedAt(new Date().toISOString());
        setError(null);
      } catch (err) {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : String(err));
        }
      }
    };

    void load();
    const timer = setInterval(() => {
      void load();
    }, REFRESH_MS);

    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, []);

  const view = useMemo(() => {
    const slice = candles.slice(Math.max(0, candles.length - MAX_DISPLAY_CANDLES));
    if (slice.length === 0) {
      return { slice, minPrice: 0, maxPrice: 0 };
    }

    let minPrice = Number.POSITIVE_INFINITY;
    let maxPrice = Number.NEGATIVE_INFINITY;
    for (const candle of slice) {
      if (Number.isFinite(candle.low)) minPrice = Math.min(minPrice, candle.low);
      if (Number.isFinite(candle.high)) maxPrice = Math.max(maxPrice, candle.high);
    }
    if (!Number.isFinite(minPrice) || !Number.isFinite(maxPrice)) {
      return { slice, minPrice: 0, maxPrice: 0 };
    }

    const padding = (maxPrice - minPrice) * 0.08 || 4;
    return {
      slice,
      minPrice: minPrice - padding,
      maxPrice: maxPrice + padding,
    };
  }, [candles]);

  const chartWidth = 900;
  const chartHeight = 280;
  const topPad = 18;
  const bottomPad = 24;
  const rightPad = 60;
  const priceHeight = chartHeight - topPad - bottomPad;
  const plotWidth = chartWidth - rightPad;

  const priceToY = (price: number) => {
    const range = view.maxPrice - view.minPrice || 1;
    return topPad + (1 - (price - view.minPrice) / range) * priceHeight;
  };

  const spacing = view.slice.length > 0 ? plotWidth / view.slice.length : 20;
  const bodyWidth = Math.max(3, spacing * 0.62);
  const lastCandle = view.slice[view.slice.length - 1];
  const updatedLabel = updatedAt
    ? new Date(updatedAt).toLocaleTimeString('vi-VN', {
        hour: '2-digit',
        minute: '2-digit',
      })
    : null;

  return (
    <div className="bg-neutral-900 border border-neutral-800 rounded-xl p-4 flex flex-col gap-3">
      <div className="flex flex-wrap items-start justify-between gap-3 border-b border-neutral-800 pb-3">
        <div className="flex flex-col gap-0.5">
          <div className="flex flex-wrap items-center gap-2">
            <h3 className="text-sm font-bold text-white tracking-tight uppercase font-mono">
              H4 View toàn cảnh — không phải điểm entry
            </h3>
            <span className="px-1.5 py-0.5 rounded text-[9px] font-mono uppercase tracking-wider bg-neutral-800 text-neutral-400 border border-neutral-700">
              Read-only
            </span>
          </div>
          <p className="text-[11px] text-neutral-500">
            Nến H4 đã đóng từ Binance PAXG, tự động làm mới mỗi 5 phút. Chỉ dùng để nhìn xu hướng lớn.
          </p>
        </div>

        <div className="flex flex-wrap items-center gap-2 text-[10px] font-mono">
          {lastCandle && (
            <span className="px-2 py-1 rounded-md bg-neutral-950 border border-neutral-800 text-neutral-300 tabular-nums">
              close ${lastCandle.close.toFixed(2)}
            </span>
          )}
          {updatedLabel && <span className="text-neutral-600">cập nhật {updatedLabel}</span>}
        </div>
      </div>

      {view.slice.length === 0 ? (
        <div className="h-[220px] flex items-center justify-center rounded-lg border border-dashed border-neutral-800 bg-neutral-950/40 text-xs font-mono text-neutral-500">
          {error
            ? `Không tải được nến H4: ${error}`
            : 'Đang chờ nến H4 từ Rust engine (endpoint /api/htf-candles).'}
        </div>
      ) : (
        <div className="relative w-full overflow-x-auto select-none">
          <svg
            viewBox={`0 0 ${chartWidth} ${chartHeight}`}
            className="w-full h-auto min-w-[700px]"
            role="img"
            aria-label="H4 candlestick context chart"
          >
            {[0, 0.25, 0.5, 0.75, 1].map((pct) => {
              const price = view.maxPrice - (view.maxPrice - view.minPrice) * pct;
              const y = topPad + pct * priceHeight;
              return (
                <g key={pct}>
                  <line
                    x1="0"
                    y1={y}
                    x2={plotWidth}
                    y2={y}
                    stroke="#262626"
                    strokeWidth="1"
                    strokeDasharray="3 3"
                  />
                  <text
                    x={plotWidth + 6}
                    y={y + 4}
                    fill="#737373"
                    fontSize="10"
                    fontFamily="var(--font-mono)"
                  >
                    ${price.toFixed(1)}
                  </text>
                </g>
              );
            })}

            {view.slice.map((candle, index) => {
              const x = index * spacing + spacing / 2;
              const openY = priceToY(candle.open);
              const closeY = priceToY(candle.close);
              const highY = priceToY(candle.high);
              const lowY = priceToY(candle.low);
              const bullish = candle.close >= candle.open;
              const color = bullish ? '#10b981' : '#f43f5e';

              return (
                <g key={candle.timestamp || index}>
                  <line x1={x} y1={highY} x2={x} y2={lowY} stroke={color} strokeWidth="1.2" />
                  <rect
                    x={x - bodyWidth / 2}
                    y={Math.min(openY, closeY)}
                    width={bodyWidth}
                    height={Math.max(2, Math.abs(openY - closeY))}
                    fill={color}
                    rx="1"
                  />
                </g>
              );
            })}
          </svg>
        </div>
      )}

      <div className="flex flex-wrap items-center justify-between gap-2 text-[10px] font-mono text-neutral-500">
        <span>
          {view.slice.length} nến H4 · khung lớn chỉ tham khảo, tín hiệu entry vẫn nằm trên M15/H1
        </span>
        <span className="text-neutral-600">Volume Delta/CVD filter: tùy chọn, mặc định OFF</span>
      </div>
    </div>
  );
};
