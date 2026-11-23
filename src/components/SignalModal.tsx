'use client';

import React, { useState } from 'react';
import { Signal } from '../types';

interface Props {
  signal: Signal | null;
  onClose: () => void;
  onSendToTelegram: (signal: Signal) => Promise<{ success: boolean; message?: string }>;
}

export const SignalModal: React.FC<Props> = ({
  signal,
  onClose,
  onSendToTelegram,
}) => {
  const [copied, setCopied] = useState(false);
  const [isSending, setIsSending] = useState(false);
  const [dispatchStatus, setDispatchStatus] = useState<string | null>(null);

  if (!signal) return null;

  const isBuy = signal.signal_type === 'BUY';
  const actionIcon = isBuy ? '🟢' : '🔴';
  const actionText = isBuy ? 'BUY / LONG' : 'SELL / SHORT';


  const simulationPrefix = signal.is_simulated
    ? '🧪 [SIMULATION - KHÔNG TÍNH LỆNH THẬT]\n'
    : '';
  const confluenceLine =
    typeof signal.confluence_score === 'number'
      ? `\n🎯 Điểm hội tụ kỹ thuật (Confluence): ${signal.confluence_score}/100`
      : '';
  const aiConfidencePct = Math.round(
    Math.min(1, Math.max(0, signal.ai_confidence ?? 0)) * 100
  );
  const aiTelegramLine = signal.ai_verdict
    ? `\n🤖 AI Risk Officer: ${signal.ai_verdict} · confidence ${aiConfidencePct}%\n• ${signal.ai_reason ?? ''}`
    : '';
  const volumeDeltaLine =
    typeof signal.delta_ratio === 'number'
      ? `\n📊 Volume Delta: ${signal.delta_ratio.toFixed(2)}${
          typeof signal.cvd === 'number' ? ` | CVD: ${signal.cvd.toFixed(0)}` : ''
        }`
      : '';
  const rsiDivergence =
    signal.rsi_divergence && signal.rsi_divergence !== 'None'
      ? ` · ${signal.rsi_divergence} Divergence`
      : '';
  const rsiTelegramLine =
    typeof signal.rsi_value === 'number'
      ? `\n📐 RSI M15: ${signal.rsi_value.toFixed(1)}${
          signal.rsi_state ? ` · ${signal.rsi_state}` : ''
        }${rsiDivergence}`
      : '';
  const macroTelegramLine = signal.macro_flow_alignment
    ? `\n🌐 Macro/Flow: ${signal.macro_flow_alignment}${
        signal.macro_flow_note ? ` — ${signal.macro_flow_note}` : ''
      }`
    : '';

  const baseTelegramMessage = `${simulationPrefix}🚨 GOLD SIGNAL ALERT | ${signal.symbol} 🚨
━━━━━━━━━━━━━━━━━━━━━━
📊 Lệnh: ${actionIcon} ${actionText}
⏱ Khung: ${signal.timeframe}
💵 Entry: $${signal.entry_price.toFixed(2)}
🎯 Take Profit 1 (1:2): $${signal.take_profit_1.toFixed(2)} (+${signal.reward_points_tp1.toFixed(1)} pts)
🚀 Take Profit 2 (1:3): $${signal.take_profit_2.toFixed(2)} (+${signal.reward_points_tp2.toFixed(1)} pts)
🛑 Stop Loss: $${signal.stop_loss.toFixed(2)} (-${signal.risk_points.toFixed(1)} pts)
⚖️ Tỷ lệ R:R: 1:${signal.risk_reward_ratio.toFixed(2)}
⚠️ View-only: không tính lot/risk.${confluenceLine}
━━━━━━━━━━━━━━━━━━━━━━
🔍 Phân tích Kỹ thuật & Volume:
• Price Action: ${signal.price_action_notes}
• Volume Absorption: ${signal.volume_analysis_notes}${volumeDeltaLine}${rsiTelegramLine}${macroTelegramLine}
• Đánh giá: ${signal.full_rationale}${aiTelegramLine}
━━━━━━━━━━━━━━━━━━━━━━
⚠️ Quản lý rủi ro tối đa 1-2% tài khoản. Chúc bạn giao dịch kỷ luật!
⚡ Engine: Gold Sniper Bot v1.2 (Tokio/Axum + EMA50 Free Trend)`;

  const handleCopy = () => {
    navigator.clipboard.writeText(baseTelegramMessage);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  const handleSendTelegram = async () => {
    setIsSending(true);
    setDispatchStatus(null);
    const res = await onSendToTelegram(signal);
    setIsSending(false);
    if (res.success) {
      setDispatchStatus('✅ Đã bắn tín hiệu thành công tới Telegram channel!');
    } else {
      setDispatchStatus(`❌ Lỗi: ${res.message || 'Kiểm tra cấu hình Bot Token & Chat ID trong mục Risk & Settings'}`);
    }
  };

  return (
    <div className="fixed inset-0 z-50 bg-black/75 backdrop-blur-sm flex items-center justify-center p-4">
      <div className="bg-neutral-900 border border-neutral-750 rounded-2xl w-full max-w-2xl max-h-[90vh] overflow-y-auto shadow-2xl flex flex-col">
        {/* Modal Header */}
        <div className="flex items-center justify-between p-5 border-b border-neutral-800">
          <div className="flex flex-wrap items-center gap-3">
            <span
              className={`px-2.5 py-1 rounded text-xs font-bold font-mono ${
                isBuy ? 'bg-emerald-500/20 text-emerald-400' : 'bg-rose-500/20 text-rose-400'
              }`}
            >
              {signal.signal_type} SIGNAL
            </span>
            <span className="text-lg font-bold text-white font-mono">{signal.symbol}</span>
            <span className="text-xs text-neutral-400 font-mono">· {signal.timeframe}</span>
            {signal.entry_type && (
              <span className="px-2 py-0.5 rounded text-[10px] font-bold font-mono bg-sky-500/15 text-sky-300 border border-sky-500/30">
                {signal.entry_type}
                {signal.entry_type !== 'MARKET'
                  ? ` · ${signal.is_entry_filled ? 'FILLED' : 'PENDING'}`
                  : ''}
              </span>
            )}
            {signal.is_simulated && (
              <span className="px-2 py-0.5 rounded text-[10px] font-bold font-mono bg-amber-500/20 text-amber-300 border border-amber-500/40">
                SIMULATION · KHÔNG TÍNH LỆNH THẬT
              </span>
            )}
            {typeof signal.confluence_score === 'number' && (
              <span className="px-2 py-0.5 rounded text-[10px] font-bold font-mono bg-sky-500/15 text-sky-300 border border-sky-500/30">
                CONFLUENCE {signal.confluence_score}/100
              </span>
            )}
          </div>

          <button
            onClick={onClose}
            className="text-neutral-400 hover:text-white transition-colors text-xl p-1 leading-none cursor-pointer"
          >
            ✕
          </button>
        </div>

        {/* Modal Body */}
        <div className="p-5 flex flex-col gap-5">
          {/* Risk-Reward Geometry Card */}
          <div className="grid grid-cols-2 sm:grid-cols-4 gap-3 bg-neutral-950 p-4 rounded-xl border border-neutral-800 text-xs font-mono">
            <div>
              <span className="text-neutral-500 block">Entry Point</span>
              <span className="text-white text-base font-bold tabular-nums">
                ${signal.entry_price.toFixed(2)}
              </span>
            </div>
            <div>
              <span className="text-neutral-500 block">Stop Loss (Risk)</span>
              <span className="text-rose-400 text-base font-bold tabular-nums">
                ${signal.stop_loss.toFixed(2)}
              </span>
              <span className="text-[10px] text-rose-500/80 block">(-{signal.risk_points.toFixed(1)}$ buffer)</span>
            </div>
            <div>
              <span className="text-neutral-500 block">Target 1 (1:2 R:R)</span>
              <span className="text-emerald-400 text-base font-bold tabular-nums">
                ${signal.take_profit_1.toFixed(2)}
              </span>
              <span className="text-[10px] text-emerald-500/80 block">(+{signal.reward_points_tp1.toFixed(1)}$)</span>
            </div>
            <div>
              <span className="text-neutral-500 block">Target 2 (1:3 R:R)</span>
              <span className="text-emerald-300 text-base font-bold tabular-nums">
                ${signal.take_profit_2.toFixed(2)}
              </span>
              <span className="text-[10px] text-emerald-400/80 block">(+{signal.reward_points_tp2.toFixed(1)}$)</span>
            </div>
          </div>

          {/* Entry Price Correctness Metadata */}
          {(signal.entry_type || signal.data_source) && (
            <div className="flex flex-wrap items-center gap-x-5 gap-y-1 px-1 text-[11px] font-mono text-neutral-400">
              {typeof signal.candle_close_price === 'number' && (
                <span>
                  Pinbar close:{' '}
                  <strong className="text-white">${signal.candle_close_price.toFixed(2)}</strong>
                </span>
              )}
              {typeof signal.live_market_price === 'number' && (
                <span>
                  Live:{' '}
                  <strong className="text-sky-300">${signal.live_market_price.toFixed(2)}</strong>
                </span>
              )}
              {typeof signal.retest_limit_price === 'number' && (
                <span>
                  Retest:{' '}
                  <strong className="text-amber-300">${signal.retest_limit_price.toFixed(2)}</strong>
                </span>
              )}
              {signal.data_source && (
                <span>
                  Source: <strong className="text-neutral-200">{signal.data_source}</strong>
                </span>
              )}
            </div>
          )}

          {/* AI Chief Risk Officer (veto-only, advisory fields) */}
          {signal.ai_verdict && (
            <div className="rounded-xl border border-indigo-500/30 bg-indigo-500/10 p-3.5 flex flex-col gap-2 text-xs">
              <div className="flex flex-wrap items-center gap-2 font-mono">
                <span className="text-indigo-300 font-bold">🤖 AI Chief Risk Officer</span>
                <span
                  className={`px-2 py-0.5 rounded text-[10px] font-bold ${
                    signal.ai_verdict === 'STANDBY'
                      ? 'bg-amber-500/20 text-amber-300 border border-amber-500/40'
                      : signal.ai_verdict === 'BUY'
                        ? 'bg-emerald-500/20 text-emerald-300 border border-emerald-500/40'
                        : signal.ai_verdict === 'SELL'
                          ? 'bg-rose-500/20 text-rose-300 border border-rose-500/40'
                          : 'bg-neutral-800 text-neutral-300 border border-neutral-700'
                  }`}
                >
                  {signal.ai_verdict}
                </span>
                <span className="text-neutral-400">
                  confidence <strong className="text-indigo-200">{aiConfidencePct}%</strong>
                </span>
                <span className="text-[10px] text-neutral-500">veto-only · không đặt SL/TP live</span>
              </div>
              {signal.ai_reason && (
                <p className="text-neutral-300 leading-relaxed">
                  <span className="text-indigo-300 font-mono">•</span> {signal.ai_reason}
                </p>
              )}
            </div>
          )}

          {/* Macro/Flow alignment (advisory, never blocks the signal) */}
          {signal.macro_flow_alignment && (
            <div className="rounded-xl border border-teal-500/30 bg-teal-500/10 p-3.5 flex flex-col gap-2 text-xs">
              <div className="flex flex-wrap items-center gap-2 font-mono">
                <span className="text-teal-300 font-bold">🌐 Macro/Flow</span>
                <span
                  className={`px-2 py-0.5 rounded text-[10px] font-bold border ${
                    signal.macro_flow_alignment === 'ALIGNED'
                      ? 'bg-emerald-500/20 text-emerald-300 border-emerald-500/40'
                      : signal.macro_flow_alignment === 'COUNTER'
                        ? 'bg-rose-500/20 text-rose-300 border-rose-500/40'
                        : signal.macro_flow_alignment === 'MIXED'
                          ? 'bg-amber-500/20 text-amber-300 border-amber-500/40'
                          : 'bg-neutral-800 text-neutral-300 border-neutral-700'
                  }`}
                >
                  {signal.macro_flow_alignment}
                </span>
                <span className="text-[10px] text-neutral-500">
                  bối cảnh vĩ mô · không tự động chặn
                </span>
              </div>
              {signal.macro_flow_note && (
                <p className="text-neutral-300 leading-relaxed">{signal.macro_flow_note}</p>
              )}
            </div>
          )}

          {/* Volume Delta/CVD (only present when the optional filter ran) */}
          {typeof signal.delta_ratio === 'number' && (            <div className="rounded-xl border border-cyan-500/30 bg-cyan-500/10 p-3.5 flex flex-wrap items-center gap-x-5 gap-y-2 text-xs font-mono">
              <span className="text-cyan-300 font-bold">📊 Volume Delta</span>
              <span
                className={
                  signal.delta_ratio > 0
                    ? 'text-emerald-400'
                    : signal.delta_ratio < 0
                      ? 'text-rose-400'
                      : 'text-neutral-300'
                }
              >
                Ratio{' '}
                <strong>
                  {signal.delta_ratio >= 0 ? '+' : ''}
                  {signal.delta_ratio.toFixed(2)}
                </strong>
              </span>
              {typeof signal.delta === 'number' && (
                <span
                  className={
                    signal.delta > 0
                      ? 'text-emerald-400'
                      : signal.delta < 0
                        ? 'text-rose-400'
                        : 'text-neutral-300'
                  }
                >
                  Delta{' '}
                  <strong>
                    {signal.delta >= 0 ? '+' : ''}
                    {signal.delta.toFixed(0)}
                  </strong>
                </span>
              )}
              {typeof signal.cvd === 'number' && (
                <span
                  className={
                    signal.cvd > 0
                      ? 'text-emerald-400'
                      : signal.cvd < 0
                        ? 'text-rose-400'
                        : 'text-neutral-300'
                  }
                >
                  CVD{' '}
                  <strong>
                    {signal.cvd >= 0 ? '+' : ''}
                    {signal.cvd.toFixed(0)}
                  </strong>
                </span>
              )}
              <span className="text-[10px] text-neutral-500">
                taker buy − sell · chỉ hiển thị khi filter bật
              </span>
            </div>
          )}

          {/* RSI M15 context (always present for real signals) */}
          {typeof signal.rsi_value === 'number' && (
            <div className="rounded-xl border border-fuchsia-500/30 bg-fuchsia-500/10 p-3.5 flex flex-wrap items-center gap-x-5 gap-y-2 text-xs font-mono">
              <span className="text-fuchsia-300 font-bold">📐 RSI M15</span>
              <span className="text-neutral-200">
                Value <strong className="tabular-nums">{signal.rsi_value.toFixed(1)}</strong>
              </span>
              {signal.rsi_state && (
                <span
                  className={`px-2 py-0.5 rounded text-[10px] font-bold border ${
                    signal.rsi_state === 'Oversold'
                      ? 'bg-emerald-500/20 text-emerald-300 border-emerald-500/40'
                      : signal.rsi_state === 'Overbought'
                        ? 'bg-rose-500/20 text-rose-300 border-rose-500/40'
                        : 'bg-neutral-800 text-neutral-300 border-neutral-700'
                  }`}
                >
                  {signal.rsi_state}
                </span>
              )}
              {signal.rsi_divergence && signal.rsi_divergence !== 'None' && (
                <span
                  className={`px-2 py-0.5 rounded text-[10px] font-bold border ${
                    signal.rsi_divergence === 'Bullish'
                      ? 'bg-emerald-500/10 text-emerald-300 border-emerald-500/30'
                      : signal.rsi_divergence === 'Bearish'
                        ? 'bg-rose-500/10 text-rose-300 border-rose-500/30'
                        : 'bg-neutral-900 text-neutral-400 border-neutral-800'
                  }`}
                >
                  {signal.rsi_divergence} Divergence
                </span>
              )}
              <span className="text-[10px] text-neutral-500">
                Wilder · context/AI input · không tự vào lệnh
              </span>
            </div>
          )}

          {/* Analysis Breakdown */}
          <div className="flex flex-col gap-2">
            <h4 className="text-xs font-bold text-neutral-400 uppercase tracking-wider">
              Thuật toán xác nhận tín hiệu (Signal Logic)
            </h4>
            <div className="bg-neutral-950/70 p-3.5 rounded-xl border border-neutral-800 text-xs text-neutral-300 flex flex-col gap-2 leading-relaxed">
              <div>
                <strong className="text-amber-400 font-mono">1. Price Action:</strong> {signal.price_action_notes}
              </div>
              <div>
                <strong className="text-amber-400 font-mono">2. Volume Surge:</strong> {signal.volume_analysis_notes}
              </div>
              <div>
                <strong className="text-amber-400 font-mono">3. Math R:R:</strong> Rủi ro {signal.risk_points.toFixed(1)}$ so với Lợi nhuận kỳ vọng TP1 {signal.reward_points_tp1.toFixed(1)}$ (Tỷ lệ 1:{signal.risk_reward_ratio.toFixed(2)} thỏa mãn quy tắc tối thiểu 1:2).
              </div>
              {signal.dedup_key && (
                <div>
                  <strong className="text-amber-400 font-mono">4. Dedup Key:</strong>{' '}
                  <span className="text-neutral-400">{signal.dedup_key}</span> — chống bắn lặp cùng một cây nến.
                </div>
              )}
            </div>
          </div>

          {/* Telegram Preview Box */}
          <div className="flex flex-col gap-2">
            <div className="flex items-center justify-between">
              <h4 className="text-xs font-bold text-neutral-400 uppercase tracking-wider flex items-center gap-1.5">
                <span className="w-2 h-2 rounded-full bg-sky-400"></span>
                Telegram Alert Format (Preview)
              </h4>
              <button
                onClick={handleCopy}
                className="text-xs text-amber-400 hover:text-amber-300 transition-colors font-mono cursor-pointer"
              >
                {copied ? '✓ Đã Copy' : 'Copy Message'}
              </button>
            </div>

            <pre className="p-4 bg-neutral-950 border border-neutral-800 rounded-xl text-neutral-200 text-xs font-mono whitespace-pre-wrap leading-relaxed select-all">
              {baseTelegramMessage}
            </pre>
          </div>

          {dispatchStatus && (
            <div className="p-3 rounded-lg text-xs font-mono bg-neutral-950 border border-neutral-800">
              {dispatchStatus}
            </div>
          )}
        </div>

        {/* Modal Actions */}
        <div className="flex items-center justify-end gap-3 p-4 border-t border-neutral-800 bg-neutral-950/40">
          <button
            onClick={onClose}
            className="px-4 py-2 text-xs font-medium text-neutral-400 hover:text-white rounded-lg transition-colors cursor-pointer"
          >
            Đóng
          </button>
          <button
            onClick={handleSendTelegram}
            disabled={isSending}
            className="px-4 py-2 text-xs font-bold text-neutral-950 bg-amber-400 hover:bg-amber-300 rounded-lg transition-colors cursor-pointer disabled:opacity-50 flex items-center gap-2"
          >
            {isSending ? 'Đang gửi...' : 'Bắn Alert qua Telegram'}
          </button>
        </div>
      </div>
    </div>
  );
};
