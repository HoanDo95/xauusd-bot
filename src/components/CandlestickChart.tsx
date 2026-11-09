'use client';

import React, { useState, useMemo } from 'react';
import { Candle, SupportResistanceZone, Signal } from '../types';

interface Props {
  candles: Candle[];
  supports: SupportResistanceZone[];
  resistances: SupportResistanceZone[];
  volumeMA: number;
  activeSignal: Signal | null;
  selectedTimeframe: string;
  onTimeframeChange: (tf: string) => void;
}

export const CandlestickChart: React.FC<Props> = ({
  candles,
  supports,
  resistances,
  volumeMA,
  activeSignal,
  selectedTimeframe,
  onTimeframeChange,
}) => {
  const [hoveredIndex, setHoveredIndex] = useState<number | null>(null);
  const [showSR, setShowSR] = useState(true);
  const [showTargets, setShowTargets] = useState(true);

  // Compute price bounds
  const { minPrice, maxPrice, maxVol, displayCandles } = useMemo(() => {
    // Show last 35 candles for clean viewing
    const slice = candles.slice(Math.max(0, candles.length - 35));
    if (!slice.length) {
      return { minPrice: 2650, maxPrice: 2720, maxVol: 5000, displayCandles: [] };
    }

    let min = Math.min(...slice.map((c) => c.low));
    let max = Math.max(...slice.map((c) => c.high));
    const mVol = Math.max(...slice.map((c) => c.volume), volumeMA * 1.5, 1000);

    // If active signal, ensure SL and TP are within view
    if (activeSignal && showTargets) {
      min = Math.min(min, activeSignal.stop_loss, activeSignal.take_profit_1);
      max = Math.max(max, activeSignal.stop_loss, activeSignal.take_profit_2);
    }

    // Add 2% padding
    const pad = (max - min) * 0.08 || 4;
    return {
      minPrice: Math.floor(min - pad),
      maxPrice: Math.ceil(max + pad),
      maxVol: mVol,
      displayCandles: slice,
    };
  }, [candles, activeSignal, showTargets, volumeMA]);

  const chartWidth = 900;
  const chartHeight = 360;
  const volHeight = 70;
  const topPad = 20;
  const botPad = 25;
  const priceHeight = chartHeight - topPad - botPad;

  const priceToY = (price: number) => {
    const range = maxPrice - minPrice || 1;
    return topPad + (1 - (price - minPrice) / range) * priceHeight;
  };

  const volToY = (vol: number) => {
    const ratio = Math.min(1, vol / (maxVol || 1));
    return chartHeight + volHeight - 5 - ratio * (volHeight - 15);
  };

  const candleSpacing = displayCandles.length > 0 ? chartWidth / displayCandles.length : 20;
  const candleBodyWidth = Math.max(5, candleSpacing * 0.65);

  const hoveredCandle = hoveredIndex !== null && displayCandles[hoveredIndex] ? displayCandles[hoveredIndex] : displayCandles[displayCandles.length - 1];

  return (
    <div className="bg-neutral-900 border border-neutral-800 rounded-xl p-4 flex flex-col gap-3">
      {/* Chart Header Bar */}
      <div className="flex flex-wrap items-center justify-between gap-4 pb-2 border-b border-neutral-800">
        <div className="flex items-center gap-3">
          <div className="flex items-baseline gap-2">
            <span className="text-base font-semibold text-white tracking-tight">XAU/USD · Gold Spot</span>
            <span className="text-xs text-neutral-400 font-mono">15m / 1h Aggregated</span>
          </div>

          {hoveredCandle && (
            <div className="hidden lg:flex items-center gap-3 text-xs font-mono text-neutral-300 pl-4 border-l border-neutral-800">
              <span>O: <strong className="text-white">${hoveredCandle.open.toFixed(2)}</strong></span>
              <span>H: <strong className="text-white">${hoveredCandle.high.toFixed(2)}</strong></span>
              <span>L: <strong className="text-white">${hoveredCandle.low.toFixed(2)}</strong></span>
              <span>C: <strong className={hoveredCandle.close >= hoveredCandle.open ? 'text-emerald-400' : 'text-rose-400'}>${hoveredCandle.close.toFixed(2)}</strong></span>
              <span>Vol: <strong className="text-amber-400">{hoveredCandle.volume.toLocaleString()}</strong></span>
            </div>
          )}
        </div>

        {/* Chart View Controls */}
        <div className="flex items-center gap-2">
          {/* Timeframe selector */}
          <div className="flex items-center bg-neutral-950 p-0.5 rounded-lg border border-neutral-800 text-xs font-mono">
            {['M15', 'H1'].map((tf) => (
              <button
                key={tf}
                onClick={() => onTimeframeChange(tf)}
                className={`px-2.5 py-1 rounded transition-colors ${
                  selectedTimeframe === tf ? 'bg-amber-500/20 text-amber-400 font-semibold' : 'text-neutral-400 hover:text-neutral-200'
                }`}
              >
                {tf}
              </button>
            ))}
          </div>

          {/* Toggle S/R */}
          <button
            onClick={() => setShowSR(!showSR)}
            className={`px-2.5 py-1 rounded-lg text-xs font-medium border transition-colors ${
              showSR ? 'bg-neutral-800 border-neutral-700 text-neutral-200' : 'bg-transparent border-neutral-800 text-neutral-500'
            }`}
          >
            S/R Zones
          </button>

          {/* Toggle Targets */}
          <button
            onClick={() => setShowTargets(!showTargets)}
            className={`px-2.5 py-1 rounded-lg text-xs font-medium border transition-colors ${
              showTargets ? 'bg-amber-500/10 border-amber-500/30 text-amber-400' : 'bg-transparent border-neutral-800 text-neutral-500'
            }`}
          >
            Trade Levels
          </button>
        </div>
      </div>

      {/* SVG Canvas Chart */}
      <div className="relative w-full overflow-x-auto select-none">
        <svg
          viewBox={`0 0 ${chartWidth} ${chartHeight + volHeight}`}
          className="w-full h-auto min-w-[700px] overflow-visible"
          onMouseLeave={() => setHoveredIndex(null)}
        >
          {/* Background Grid Lines */}
          {[0, 0.25, 0.5, 0.75, 1].map((pct, idx) => {
            const priceVal = minPrice + (maxPrice - minPrice) * (1 - pct);
            const y = topPad + pct * priceHeight;
            return (
              <g key={idx}>
                <line
                  x1="0"
                  y1={y}
                  x2={chartWidth - 55}
                  y2={y}
                  stroke="#262626"
                  strokeWidth="1"
                  strokeDasharray="3 3"
                />
                <text
                  x={chartWidth - 50}
                  y={y + 4}
                  fill="#737373"
                  fontSize="10"
                  fontFamily="var(--font-mono)"
                >
                  ${priceVal.toFixed(1)}
                </text>
              </g>
            );
          })}

          {/* Dynamic Support Zones */}
          {showSR &&
            supports.slice(0, 2).map((sup, idx) => {
              const y = priceToY(sup.price);
              const yMin = priceToY(sup.min_price);
              const yMax = priceToY(sup.max_price);
              const h = Math.max(4, Math.abs(yMin - yMax));
              return (
                <g key={`sup-${idx}`}>
                  <rect
                    x="0"
                    y={Math.min(yMin, yMax)}
                    width={chartWidth - 55}
                    height={h}
                    fill="rgba(16, 185, 129, 0.08)"
                    stroke="rgba(16, 185, 129, 0.4)"
                    strokeWidth="1"
                    strokeDasharray="4 2"
                  />
                  <text
                    x="10"
                    y={y - 4}
                    fill="#34d399"
                    fontSize="10"
                    fontWeight="600"
                    fontFamily="var(--font-mono)"
                  >
                    SUPPORT ${sup.price.toFixed(2)} ({sup.touch_count} retests)
                  </text>
                </g>
              );
            })}

          {/* Dynamic Resistance Zones */}
          {showSR &&
            resistances.slice(0, 2).map((res, idx) => {
              const y = priceToY(res.price);
              const yMin = priceToY(res.min_price);
              const yMax = priceToY(res.max_price);
              const h = Math.max(4, Math.abs(yMin - yMax));
              return (
                <g key={`res-${idx}`}>
                  <rect
                    x="0"
                    y={Math.min(yMin, yMax)}
                    width={chartWidth - 55}
                    height={h}
                    fill="rgba(244, 63, 94, 0.08)"
                    stroke="rgba(244, 63, 94, 0.4)"
                    strokeWidth="1"
                    strokeDasharray="4 2"
                  />
                  <text
                    x="10"
                    y={y + 12}
                    fill="#fb7185"
                    fontSize="10"
                    fontWeight="600"
                    fontFamily="var(--font-mono)"
                  >
                    RESISTANCE ${res.price.toFixed(2)} ({res.touch_count} retests)
                  </text>
                </g>
              );
            })}

          {/* Active Trade Levels Overlay (Entry, SL, TP1, TP2) */}
          {showTargets && activeSignal && (
            <g className="transition-opacity duration-300">
              {/* Entry Line */}
              <line
                x1="0"
                y1={priceToY(activeSignal.entry_price)}
                x2={chartWidth - 55}
                y2={priceToY(activeSignal.entry_price)}
                stroke="#38bdf8"
                strokeWidth="1.5"
                strokeDasharray="4 3"
              />
              <text
                x={chartWidth - 160}
                y={priceToY(activeSignal.entry_price) - 4}
                fill="#38bdf8"
                fontSize="10"
                fontWeight="700"
                fontFamily="var(--font-mono)"
              >
                ENTRY ${activeSignal.entry_price.toFixed(2)}
              </text>

              {/* Stop Loss Line */}
              <line
                x1="0"
                y1={priceToY(activeSignal.stop_loss)}
                x2={chartWidth - 55}
                y2={priceToY(activeSignal.stop_loss)}
                stroke="#f43f5e"
                strokeWidth="1.5"
                strokeDasharray="3 3"
              />
              <text
                x={chartWidth - 170}
                y={priceToY(activeSignal.stop_loss) + 12}
                fill="#f43f5e"
                fontSize="10"
                fontWeight="700"
                fontFamily="var(--font-mono)"
              >
                SL ${activeSignal.stop_loss.toFixed(2)} (-{activeSignal.risk_points.toFixed(1)}$)
              </text>

              {/* Take Profit 1 */}
              <line
                x1="0"
                y1={priceToY(activeSignal.take_profit_1)}
                x2={chartWidth - 55}
                y2={priceToY(activeSignal.take_profit_1)}
                stroke="#10b981"
                strokeWidth="1.5"
                strokeDasharray="4 2"
              />
              <text
                x={chartWidth - 170}
                y={priceToY(activeSignal.take_profit_1) - 4}
                fill="#10b981"
                fontSize="10"
                fontWeight="700"
                fontFamily="var(--font-mono)"
              >
                TP1 (1:2) ${activeSignal.take_profit_1.toFixed(2)}
              </text>

              {/* Take Profit 2 */}
              <line
                x1="0"
                y1={priceToY(activeSignal.take_profit_2)}
                x2={chartWidth - 55}
                y2={priceToY(activeSignal.take_profit_2)}
                stroke="#059669"
                strokeWidth="1.5"
              />
              <text
                x={chartWidth - 170}
                y={priceToY(activeSignal.take_profit_2) - 4}
                fill="#059669"
                fontSize="10"
                fontWeight="700"
                fontFamily="var(--font-mono)"
              >
                TP2 (1:3) ${activeSignal.take_profit_2.toFixed(2)}
              </text>
            </g>
          )}

          {/* Candlestick Glyphs & Volumes */}
          {displayCandles.map((c, i) => {
            const x = i * candleSpacing + candleSpacing / 2;
            const openY = priceToY(c.open);
            const closeY = priceToY(c.close);
            const highY = priceToY(c.high);
            const lowY = priceToY(c.low);
            const isBullish = c.close >= c.open;
            const color = isBullish ? '#10b981' : '#f43f5e';

            const bodyTop = Math.min(openY, closeY);
            const bodyH = Math.max(2, Math.abs(openY - closeY));

            // Pinbar detection for visual annotation
            const range = Math.max(0.01, c.high - c.low);
            const body = Math.abs(c.close - c.open);
            const lowerWick = Math.min(c.open, c.close) - c.low;
            const upperWick = c.high - Math.max(c.open, c.close);
            const isBullPin = lowerWick >= body * 1.9 && lowerWick / range >= 0.48;
            const isBearPin = upperWick >= body * 1.9 && upperWick / range >= 0.48;

            // Volume bar
            const volY = volToY(c.volume);
            const volH = chartHeight + volHeight - 5 - volY;

            return (
              <g
                key={c.timestamp || i}
                onMouseEnter={() => setHoveredIndex(i)}
                className="cursor-pointer"
              >
                {/* Candle Wick (High to Low) */}
                <line
                  x1={x}
                  y1={highY}
                  x2={x}
                  y2={lowY}
                  stroke={color}
                  strokeWidth="1.2"
                />

                {/* Candle Body */}
                <rect
                  x={x - candleBodyWidth / 2}
                  y={bodyTop}
                  width={candleBodyWidth}
                  height={bodyH}
                  fill={isBullish ? '#10b981' : '#f43f5e'}
                  rx="1"
                />

                {/* Volume Bar */}
                <rect
                  x={x - candleBodyWidth / 2}
                  y={volY}
                  width={candleBodyWidth}
                  height={Math.max(1, volH)}
                  fill={isBullish ? 'rgba(16, 185, 129, 0.35)' : 'rgba(244, 63, 94, 0.35)'}
                />

                {/* Pinbar marker indicator */}
                {isBullPin && (
                  <g>
                    <polygon
                      points={`${x},${lowY + 16} ${x - 4},${lowY + 23} ${x + 4},${lowY + 23}`}
                      fill="#34d399"
                    />
                    <text
                      x={x}
                      y={lowY + 32}
                      textAnchor="middle"
                      fill="#34d399"
                      fontSize="9"
                      fontWeight="700"
                      fontFamily="var(--font-mono)"
                    >
                      PINBAR
                    </text>
                  </g>
                )}

                {isBearPin && (
                  <g>
                    <polygon
                      points={`${x},${highY - 16} ${x - 4},${highY - 23} ${x + 4},${highY - 23}`}
                      fill="#fb7185"
                    />
                    <text
                      x={x}
                      y={highY - 25}
                      textAnchor="middle"
                      fill="#fb7185"
                      fontSize="9"
                      fontWeight="700"
                      fontFamily="var(--font-mono)"
                    >
                      PINBAR
                    </text>
                  </g>
                )}
              </g>
            );
          })}

          {/* Volume Section Separator Line */}
          <line
            x1="0"
            y1={chartHeight}
            x2={chartWidth - 55}
            y2={chartHeight}
            stroke="#262626"
            strokeWidth="1"
          />

          {/* Volume MA20 Line */}
          <line
            x1="0"
            y1={volToY(volumeMA)}
            x2={chartWidth - 55}
            y2={volToY(volumeMA)}
            stroke="#f59e0b"
            strokeWidth="1.2"
            strokeDasharray="3 2"
          />
          <text
            x="10"
            y={chartHeight + 14}
            fill="#f59e0b"
            fontSize="10"
            fontWeight="600"
            fontFamily="var(--font-mono)"
          >
            VOL MA20: {volumeMA.toLocaleString()} (SURGE ABSORPTION THRESHOLD)
          </text>
        </svg>
      </div>

      {/* Chart Footer / Quick Legend */}
      <div className="flex flex-wrap items-center justify-between text-xs text-neutral-400 pt-1">
        <div className="flex items-center gap-4">
          <span className="flex items-center gap-1.5">
            <span className="w-2.5 h-2.5 rounded-sm bg-emerald-500"></span>
            Bullish Candlestick
          </span>
          <span className="flex items-center gap-1.5">
            <span className="w-2.5 h-2.5 rounded-sm bg-rose-500"></span>
            Bearish Candlestick
          </span>
          <span className="flex items-center gap-1.5">
            <span className="w-3 h-0.5 bg-emerald-400"></span>
            Support Zone
          </span>
          <span className="flex items-center gap-1.5">
            <span className="w-3 h-0.5 bg-rose-400"></span>
            Resistance Zone
          </span>
          <span className="flex items-center gap-1.5">
            <span className="w-3 h-0.5 bg-amber-500 border-dashed border-b"></span>
            Volume MA20 Filter
          </span>
        </div>
        <div className="text-neutral-500 font-mono">
          Hover over candles to view full OHLC & Absorption details
        </div>
      </div>
    </div>
  );
};
