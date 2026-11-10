'use client';

import React, { useEffect, useState } from 'react';
import { BotConfig } from '../types';

interface Props {
  config: BotConfig;
  onSaveConfig: (newConfig: BotConfig) => Promise<void>;
  onTestTelegram: (token: string, chatId: string) => Promise<{ success: boolean; message: string }>;
}

export const ConfigPanel: React.FC<Props> = ({
  config,
  onSaveConfig,
  onTestTelegram,
}) => {
  const [formData, setFormData] = useState<BotConfig>(config);
  const [isSaving, setIsSaving] = useState(false);
  const [isTesting, setIsTesting] = useState(false);
  const [saveSuccess, setSaveSuccess] = useState(false);
  const [testResult, setTestResult] = useState<{ success: boolean; message: string } | null>(null);

  // Keep the form in sync when the dashboard finishes loading config from disk.
  useEffect(() => {
    setFormData(config);
  }, [config]);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setIsSaving(true);
    setSaveSuccess(false);
    await onSaveConfig(formData);
    setIsSaving(false);
    setSaveSuccess(true);
    setTimeout(() => setSaveSuccess(false), 3000);
  };

  const handleTestPing = async () => {
    setIsTesting(true);
    setTestResult(null);
    const res = await onTestTelegram(formData.telegram_bot_token, formData.telegram_chat_id);
    setIsTesting(false);
    setTestResult(res);
  };

  return (
    <div className="bg-neutral-900 border border-neutral-800 rounded-xl p-6 flex flex-col gap-6">
      <div className="flex flex-col gap-1">
        <h3 className="text-lg font-bold text-white tracking-tight">
          Cấu hình Sniper & Quản lý Rủi ro (1 Lệnh / Ngày)
        </h3>
        <p className="text-xs text-neutral-400">
          Thông số được áp dụng trực tiếp cho Rust Engine & Express fallback: Sniper Quota, khung giờ vàng
          Âu–Mỹ, đệm Stop Loss 5–25$ (M15 8–10$ / H1 15–20$) và tự động giảm Lot Size để giữ nguyên % rủi ro.
        </p>
      </div>

      <form onSubmit={handleSubmit} className="flex flex-col gap-6">
        {/* Section 1: Algorithmic Logic & Risk Filters */}
        <div className="flex flex-col gap-4">
          <div className="text-xs font-bold text-amber-400 uppercase tracking-wider border-b border-neutral-800 pb-2">
            1. Bộ lọc Kỹ thuật & Quản trị Rủi ro R:R
          </div>

          <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
            {/* Min R:R */}
            <div className="flex flex-col gap-1.5">
              <label className="text-xs font-semibold text-neutral-300">
                Tỷ lệ R:R Tối thiểu (Min Risk:Reward)
              </label>
              <div className="relative">
                <input
                  type="number"
                  step="0.1"
                  min="1.5"
                  max="5.0"
                  value={formData.min_risk_reward}
                  onChange={(e) => setFormData({ ...formData, min_risk_reward: parseFloat(e.target.value) || 2.0 })}
                  className="w-full bg-neutral-950 border border-neutral-800 focus:border-amber-500 rounded-lg px-3 py-2 text-xs font-mono text-white outline-none"
                />
                <span className="absolute right-3 top-2 text-xs text-neutral-500 font-mono">1:X</span>
              </div>
              <span className="text-[11px] text-neutral-500">Mặc định 2.0 (tối thiểu 1:2 trở lên)</span>
            </div>

            {/* SL Buffer — expanded 5.0 – 25.0 */}
            <div className="flex flex-col gap-1.5">
              <label className="text-xs font-semibold text-neutral-300">
                Đệm Stop Loss an toàn (Gold SL Buffer)
              </label>
              <div className="relative">
                <input
                  type="number"
                  step="0.5"
                  min="5.0"
                  max="25.0"
                  value={formData.gold_sl_buffer_points}
                  onChange={(e) => setFormData({ ...formData, gold_sl_buffer_points: parseFloat(e.target.value) || 10.0 })}
                  className="w-full bg-neutral-950 border border-neutral-800 focus:border-amber-500 rounded-lg px-3 py-2 text-xs font-mono text-white outline-none"
                />
                <span className="absolute right-3 top-2 text-xs text-neutral-500 font-mono">$ USD</span>
              </div>
              <span className="text-[11px] text-neutral-500">
                Khuyến nghị 8–10$ cho M15 và 15–20$ cho H1. Lot Size tự động giảm tương ứng để giữ nguyên rủi ro %.
              </span>
            </div>

            {/* Entry Mode */}
            <div className="flex flex-col gap-1.5">
              <label className="text-xs font-semibold text-neutral-300">
                Chế độ vào lệnh (Entry Mode)
              </label>
              <select
                value={formData.entry_mode}
                onChange={(e) =>
                  setFormData({ ...formData, entry_mode: e.target.value as BotConfig['entry_mode'] })
                }
                className="w-full bg-neutral-950 border border-neutral-800 focus:border-amber-500 rounded-lg px-3 py-2 text-xs font-mono text-white outline-none cursor-pointer"
              >
                <option value="MARKET">MARKET — khớp giá live nếu drift nhỏ</option>
                <option value="LIMIT_RETEST">LIMIT_RETEST — chờ retest midpoint</option>
                <option value="BREAKOUT">BREAKOUT — chờ phá đỉnh/đáy nến</option>
              </select>
              <span className="text-[11px] text-neutral-500">
                MARKET tự chuyển Limit khi giá live lệch quá ngưỡng drift
              </span>
            </div>

            {/* Broker price offset */}
            <div className="flex flex-col gap-1.5">
              <label className="text-xs font-semibold text-neutral-300">
                Chênh lệch giá Broker (Offset)
              </label>
              <div className="relative">
                <input
                  type="number"
                  step="0.1"
                  min="-100"
                  max="100"
                  value={formData.broker_price_offset}
                  onChange={(e) =>
                    setFormData({ ...formData, broker_price_offset: parseFloat(e.target.value) || 0 })
                  }
                  className="w-full bg-neutral-950 border border-neutral-800 focus:border-amber-500 rounded-lg px-3 py-2 text-xs font-mono text-white outline-none"
                />
                <span className="absolute right-3 top-2 text-xs text-neutral-500 font-mono">$ USD</span>
              </div>
              <span className="text-[11px] text-neutral-500">
                Cộng vào OHLC & giá live để khớp broker (ví dụ +3.5$)
              </span>
            </div>

            {/* Auto PAXG → XAU/USD spot basis */}
            <div className="flex flex-col gap-1.5">
              <label className="text-xs font-semibold text-neutral-300">
                Tự động căn PAXG → XAU/USD spot (TwelveData free)
              </label>
              <label className="flex items-start gap-2 bg-neutral-950 border border-neutral-800 rounded-lg px-3 py-2 cursor-pointer select-none">
                <input
                  type="checkbox"
                  checked={formData.use_auto_spot_basis ?? true}
                  onChange={(e) =>
                    setFormData({ ...formData, use_auto_spot_basis: e.target.checked })
                  }
                  className="accent-violet-500 w-4 h-4 rounded cursor-pointer mt-0.5"
                />
                <span className="text-[11px] text-neutral-300 leading-snug">
                  Căn giá PAXG (Binance) về giá spot XAU/USD tham chiếu
                </span>
              </label>
              <span className="text-[11px] text-neutral-500">
                Cần <span className="font-mono text-neutral-400">TWELVEDATA_API_KEY</span>; tự fallback
                gold-api.com. Cache 300s, offset broker thủ công vẫn được cộng thêm.
              </span>
            </div>

            {/* Max entry drift */}
            <div className="flex flex-col gap-1.5">
              <label className="text-xs font-semibold text-neutral-300">
                Drift tối đa để vào MARKET
              </label>
              <div className="relative">
                <input
                  type="number"
                  step="0.1"
                  min="0.5"
                  max="10"
                  value={formData.max_entry_drift_points}
                  onChange={(e) =>
                    setFormData({ ...formData, max_entry_drift_points: parseFloat(e.target.value) || 2.5 })
                  }
                  className="w-full bg-neutral-950 border border-neutral-800 focus:border-amber-500 rounded-lg px-3 py-2 text-xs font-mono text-white outline-none"
                />
                <span className="absolute right-3 top-2 text-xs text-neutral-500 font-mono">$ USD</span>
              </div>
              <span className="text-[11px] text-neutral-500">
                Vượt ngưỡng → không đuổi giá, chờ Limit tại close nến
              </span>
            </div>

            {/* Max candle age */}
            <div className="flex flex-col gap-1.5">
              <label className="text-xs font-semibold text-neutral-300">
                Tuổi nến tối đa (Candle Age)
              </label>
              <div className="relative">
                <input
                  type="number"
                  step="10"
                  min="30"
                  max="900"
                  value={formData.max_candle_age_secs}
                  onChange={(e) =>
                    setFormData({ ...formData, max_candle_age_secs: parseInt(e.target.value) || 120 })
                  }
                  className="w-full bg-neutral-950 border border-neutral-800 focus:border-amber-500 rounded-lg px-3 py-2 text-xs font-mono text-white outline-none"
                />
                <span className="absolute right-3 top-2 text-xs text-neutral-500 font-mono">Giây</span>
              </div>
              <span className="text-[11px] text-neutral-500">
                Nến đóng quá lâu sẽ bị từ chối để tránh vào lệnh trễ
              </span>
            </div>

            {/* Volume Surge Multiplier */}
            <div className="flex flex-col gap-1.5">
              <label className="text-xs font-semibold text-neutral-300">
                Hệ số Volume Hấp thụ (Surge vs MA20)
              </label>
              <div className="relative">
                <input
                  type="number"
                  step="0.1"
                  min="1.1"
                  max="3.5"
                  value={formData.volume_surge_multiplier}
                  onChange={(e) => setFormData({ ...formData, volume_surge_multiplier: parseFloat(e.target.value) || 1.5 })}
                  className="w-full bg-neutral-950 border border-neutral-800 focus:border-amber-500 rounded-lg px-3 py-2 text-xs font-mono text-white outline-none"
                />
                <span className="absolute right-3 top-2 text-xs text-neutral-500 font-mono">x MA</span>
              </div>
              <span className="text-[11px] text-neutral-500">Lọc nhiễu thanh khoản yếu, tối thiểu 1.5x</span>
            </div>

            {/* Rejection Wick Ratio */}
            <div className="flex flex-col gap-1.5">
              <label className="text-xs font-semibold text-neutral-300">
                Tỷ lệ Râu nến Pinbar (Wick vs Body)
              </label>
              <input
                type="number"
                step="0.1"
                min="1.5"
                max="4.0"
                value={formData.rejection_wick_ratio}
                onChange={(e) => setFormData({ ...formData, rejection_wick_ratio: parseFloat(e.target.value) || 2.0 })}
                className="w-full bg-neutral-950 border border-neutral-800 focus:border-amber-500 rounded-lg px-3 py-2 text-xs font-mono text-white outline-none"
              />
              <span className="text-[11px] text-neutral-500">Râu nến rút chân gấp tối thiểu 2.0 lần thân</span>
            </div>

            {/* S/R Lookback */}
            <div className="flex flex-col gap-1.5">
              <label className="text-xs font-semibold text-neutral-300">
                Chu kỳ dò Đỉnh/Đáy & Order Block
              </label>
              <input
                type="number"
                min="15"
                max="60"
                value={formData.lookback_period}
                onChange={(e) => setFormData({ ...formData, lookback_period: parseInt(e.target.value) || 25 })}
                className="w-full bg-neutral-950 border border-neutral-800 focus:border-amber-500 rounded-lg px-3 py-2 text-xs font-mono text-white outline-none"
              />
              <span className="text-[11px] text-neutral-500">Số nến quét Swing High & Low</span>
            </div>

            {/* Risk % per Trade */}
            <div className="flex flex-col gap-1.5">
              <label className="text-xs font-semibold text-neutral-300">
                Mức Rủi ro / Lệnh (Risk Allocation)
              </label>
              <div className="relative">
                <input
                  type="number"
                  step="0.1"
                  min="0.2"
                  max="5.0"
                  value={formData.risk_percentage}
                  onChange={(e) => setFormData({ ...formData, risk_percentage: parseFloat(e.target.value) || 1.0 })}
                  className="w-full bg-neutral-950 border border-neutral-800 focus:border-amber-500 rounded-lg px-3 py-2 text-xs font-mono text-white outline-none"
                />
                <span className="absolute right-3 top-2 text-xs text-neutral-500 font-mono">% Vốn</span>
              </div>
              <span className="text-[11px] text-neutral-500">Kỷ luật vốn tiêu chuẩn 1.0%</span>
            </div>

            {/* Account Balance */}
            <div className="flex flex-col gap-1.5">
              <label className="text-xs font-semibold text-neutral-300">
                Số dư Tài khoản (Account Balance)
              </label>
              <div className="relative">
                <input
                  type="number"
                  step="500"
                  min="100"
                  value={formData.account_balance || 10000}
                  onChange={(e) => setFormData({ ...formData, account_balance: parseFloat(e.target.value) || 10000 })}
                  className="w-full bg-neutral-950 border border-neutral-800 focus:border-amber-500 rounded-lg px-3 py-2 text-xs font-mono text-white outline-none"
                />
                <span className="absolute right-3 top-2 text-xs text-neutral-500 font-mono">$ USD</span>
              </div>
              <span className="text-[11px] text-neutral-500">Cơ sở tính Lot Size tự động</span>
            </div>
          </div>
        </div>

        {/* Section 2: Daily Sniper Quota & Session Filter */}
        <div className="flex flex-col gap-4">
          <div className="text-xs font-bold text-emerald-400 uppercase tracking-wider border-b border-neutral-800 pb-2 flex items-center justify-between">
            <span>2. Chế độ Bắn Tỉa (Daily Sniper Quota)</span>
            <span className="text-neutral-500 text-[11px] normal-case font-normal">
              Lệnh test simulation không tiêu tốn quota
            </span>
          </div>

          <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
            {/* Max signals per day */}
            <div className="flex flex-col gap-1.5">
              <label className="text-xs font-semibold text-neutral-300">
                Giới hạn Lệnh / Ngày (Daily Sniper Quota)
              </label>
              <div className="relative">
                <input
                  type="number"
                  min="1"
                  max="5"
                  step="1"
                  value={formData.max_signals_per_day ?? 1}
                  onChange={(e) =>
                    setFormData({
                      ...formData,
                      max_signals_per_day: Math.max(1, Math.min(5, parseInt(e.target.value) || 1)),
                    })
                  }
                  className="w-full bg-neutral-950 border border-neutral-800 focus:border-emerald-500 rounded-lg px-3 py-2 text-xs font-mono text-white outline-none"
                />
                <span className="absolute right-3 top-2 text-xs text-neutral-500 font-mono">Lệnh</span>
              </div>
              <span className="text-[11px] text-neutral-500">Mặc định 1 lệnh/ngày — chỉ 1 setup đẹp nhất</span>
            </div>

            {/* Free live feed (read only) */}
            <div className="flex flex-col gap-1.5">
              <label className="text-xs font-semibold text-neutral-300">
                Nguồn dữ liệu Live (Miễn phí)
              </label>
              <div className="w-full bg-sky-500/10 border border-sky-500/30 rounded-lg px-3 py-2 text-xs font-mono text-sky-300">
                {formData.data_source === 'TWELVEDATA'
                  ? '📡 TwelveData XAU/USD (manual OHLCV)'
                  : '📡 Binance PAXG/USDT (OHLCV + Volume)'}
              </div>
              <span className="text-[11px] text-neutral-500">
                OHLCV + volume khuyến nghị Binance; TwelveData không có volume nên chỉ dùng giá spot.
              </span>
            </div>

            {/* Session filter toggle */}
            <div className="flex flex-col gap-1.5">
              <label className="text-xs font-semibold text-neutral-300">
                Bộ lọc Khung Giờ Vàng (Session Filter)
              </label>
              <label className="flex items-start gap-2 bg-neutral-950 border border-neutral-800 rounded-lg px-3 py-2 cursor-pointer select-none">
                <input
                  type="checkbox"
                  checked={formData.session_filter_enabled ?? true}
                  onChange={(e) => setFormData({ ...formData, session_filter_enabled: e.target.checked })}
                  className="accent-emerald-500 w-4 h-4 rounded cursor-pointer mt-0.5"
                />
                <span className="text-[11px] text-neutral-300 leading-snug">
                  Ưu tiên phiên Âu - Mỹ <span className="font-mono text-emerald-400">13:00 - 23:00 VN</span> để chọn 1 lệnh đẹp nhất ngày
                </span>
              </label>
              <span className="text-[11px] text-neutral-500">Tắt nếu muốn quét cả ngày (không khuyến nghị)</span>
            </div>
          </div>

          {/* Pre-session Telegram briefings */}
          <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
            <div className="flex flex-col gap-1.5">
              <label className="text-xs font-semibold text-neutral-300">
                Báo thức trước phiên Âu/Mỹ
              </label>
              <label className="flex items-start gap-2 bg-neutral-950 border border-neutral-800 rounded-lg px-3 py-2 cursor-pointer select-none">
                <input
                  type="checkbox"
                  checked={formData.session_alerts_enabled ?? true}
                  onChange={(e) =>
                    setFormData({ ...formData, session_alerts_enabled: e.target.checked })
                  }
                  className="accent-emerald-500 w-4 h-4 rounded cursor-pointer mt-0.5"
                />
                <span className="text-[11px] text-neutral-300 leading-snug">
                  Gửi briefing Telegram trước giờ mở phiên London / New York (T2–T6, tự động theo DST)
                </span>
              </label>
              <span className="text-[11px] text-neutral-500">
                Chỉ thông tin tham khảo — không tiêu hao quota và không phải lệnh vào trực tiếp
              </span>
            </div>

            <div className="flex flex-col gap-1.5">
              <label className="text-xs font-semibold text-neutral-300">
                Số phút báo trước phiên
              </label>
              <div className="relative">
                <input
                  type="number"
                  min="15"
                  max="120"
                  step="5"
                  value={formData.session_alert_lead_minutes ?? 60}
                  onChange={(e) =>
                    setFormData({
                      ...formData,
                      session_alert_lead_minutes: Math.max(
                        15,
                        Math.min(120, parseInt(e.target.value) || 60)
                      ),
                    })
                  }
                  className="w-full bg-neutral-950 border border-neutral-800 focus:border-emerald-500 rounded-lg px-3 py-2 text-xs font-mono text-white outline-none"
                />
                <span className="absolute right-3 top-2 text-xs text-neutral-500 font-mono">phút</span>
              </div>
              <span className="text-[11px] text-neutral-500">
                Mặc định 60 phút (1 giờ) — cho phép 15–120 phút
              </span>
            </div>
          </div>
        </div>

        {/* Section 3: Macro & News Kill Switch */}
        <div className="flex flex-col gap-4">
          <div className="text-xs font-bold text-rose-400 uppercase tracking-wider border-b border-neutral-800 pb-2 flex items-center justify-between">
            <span>3. Bộ lọc Vĩ mô & News Kill Switch (Bảo vệ tài khoản)</span>
            <span className="text-neutral-500 text-[11px] normal-case font-normal">
              Tránh rủi ro trượt giá và giãn spread khi ra tin NFP, CPI, FOMC
            </span>
          </div>

          <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
            {/* Macro Bias Selector */}
            <div className="flex flex-col gap-1.5">
              <label className="text-xs font-semibold text-neutral-300">
                Định hướng Xu hướng Vĩ mô (Macro Bias)
              </label>
              <select
                value={formData.macro_bias}
                onChange={(e) => setFormData({ ...formData, macro_bias: e.target.value as BotConfig['macro_bias'] })}
                className="w-full bg-neutral-950 border border-neutral-800 focus:border-rose-500 rounded-lg px-3 py-2 text-xs font-mono text-white outline-none cursor-pointer"
              >
                <option value="AUTO">AUTO (Tự động theo EMA 50/200 - Free 100%)</option>
                <option value="BULLISH">BULLISH (Chỉ nhận lệnh BUY, bỏ qua SELL)</option>
                <option value="BEARISH">BEARISH (Chỉ nhận lệnh SELL, bỏ qua BUY)</option>
                <option value="NEUTRAL">NEUTRAL (Nhận cả BUY lẫn SELL)</option>
              </select>
              <span className="text-[11px] text-neutral-500">
                AUTO dùng EMA50 hướng lên → BUY, hướng xuống → SELL; 100% miễn phí
              </span>
            </div>

            {/* Pre-News Buffer */}
            <div className="flex flex-col gap-1.5">
              <label className="text-xs font-semibold text-neutral-300">
                Thời gian ngưng trước tin lớn
              </label>
              <div className="relative">
                <input
                  type="number"
                  min="10"
                  max="120"
                  value={formData.pre_news_buffer_minutes}
                  onChange={(e) => setFormData({ ...formData, pre_news_buffer_minutes: parseInt(e.target.value) || 30 })}
                  className="w-full bg-neutral-950 border border-neutral-800 focus:border-rose-500 rounded-lg px-3 py-2 text-xs font-mono text-white outline-none"
                />
                <span className="absolute right-3 top-2 text-xs text-neutral-500 font-mono">Phút</span>
              </div>
              <span className="text-[11px] text-neutral-500">Mặc định 30 phút trước tin đỏ</span>
            </div>

            {/* Post-News Buffer */}
            <div className="flex flex-col gap-1.5">
              <label className="text-xs font-semibold text-neutral-300">
                Thời gian chờ sau tin lớn
              </label>
              <div className="relative">
                <input
                  type="number"
                  min="10"
                  max="120"
                  value={formData.post_news_buffer_minutes}
                  onChange={(e) => setFormData({ ...formData, post_news_buffer_minutes: parseInt(e.target.value) || 30 })}
                  className="w-full bg-neutral-950 border border-neutral-800 focus:border-rose-500 rounded-lg px-3 py-2 text-xs font-mono text-white outline-none"
                />
                <span className="absolute right-3 top-2 text-xs text-neutral-500 font-mono">Phút</span>
              </div>
              <span className="text-[11px] text-neutral-500">Chờ thị trường triệt tiêu sóng giật sau tin</span>
            </div>
          </div>

          <div className="flex flex-wrap items-center gap-6 pt-1">
            <label className="flex items-center gap-2 cursor-pointer text-xs text-neutral-300 select-none">
              <input
                type="checkbox"
                checked={formData.kill_switch_enabled}
                onChange={(e) => setFormData({ ...formData, kill_switch_enabled: e.target.checked })}
                className="accent-rose-500 w-4 h-4 rounded cursor-pointer"
              />
              <span className="font-semibold text-white">Bật cơ chế News Kill Switch tự động</span>
            </label>

            <label className="flex items-center gap-2 cursor-pointer text-xs text-neutral-300 select-none">
              <input
                type="checkbox"
                checked={formData.order_block_filter}
                onChange={(e) => setFormData({ ...formData, order_block_filter: e.target.checked })}
                className="accent-amber-500 w-4 h-4 rounded cursor-pointer"
              />
              <span className="font-semibold text-white">Bật bộ lọc Khối Lệnh Tổ Chức (Order Block)</span>
            </label>

            <label className="flex items-center gap-2 cursor-pointer text-xs text-neutral-300 select-none">
              <input
                type="checkbox"
                checked={formData.htf_h4_filter_enabled ?? false}
                onChange={(e) =>
                  setFormData({ ...formData, htf_h4_filter_enabled: e.target.checked })
                }
                className="accent-fuchsia-500 w-4 h-4 rounded cursor-pointer"
              />
              <span className="font-semibold text-white">
                Chặn tín hiệu ngược H4 (mặc định OFF)
              </span>
            </label>

            <label className="flex items-center gap-2 cursor-pointer text-xs text-neutral-300 select-none">
              <input
                type="checkbox"
                checked={formData.htf_d1_filter_enabled ?? false}
                onChange={(e) =>
                  setFormData({ ...formData, htf_d1_filter_enabled: e.target.checked })
                }
                className="accent-fuchsia-500 w-4 h-4 rounded cursor-pointer"
              />
              <span className="font-semibold text-white">
                Chặn tín hiệu ngược D1 (mặc định OFF)
              </span>
            </label>
          </div>

          <p className="text-[11px] text-neutral-500 leading-relaxed">
            Bộ lọc H4 chỉ chặn BUY khi H4 BEARISH và SELL khi H4 BULLISH — D1/H1 chỉ hiển thị tham
            khảo. Mặc định tắt để không thay đổi hành vi tín hiệu hiện tại; khi tắt, bot hoạt động
            hoàn toàn như trước.
          </p>

          {/* Volume Delta / CVD (optional, default OFF) */}
          <div className="rounded-xl border border-cyan-500/30 bg-cyan-500/5 p-4 flex flex-col gap-4">
            <div className="flex flex-wrap items-start justify-between gap-3">
              <div className="flex flex-col gap-1">
                <div className="text-xs font-bold text-cyan-300 uppercase tracking-wider">
                  📊 Volume Delta / CVD filter (mặc định OFF)
                </div>
                <p className="text-[11px] text-neutral-400 leading-relaxed max-w-2xl">
                  Đọc dòng tiền taker buy/sell trên Binance. Chỉ chặn tín hiệu thật khi bật; simulation
                  luôn bỏ qua bộ lọc này.
                </p>
              </div>
              <label className="flex items-center gap-2 cursor-pointer text-xs text-neutral-300 select-none">
                <input
                  type="checkbox"
                  checked={formData.volume_delta_filter_enabled ?? false}
                  onChange={(e) =>
                    setFormData({ ...formData, volume_delta_filter_enabled: e.target.checked })
                  }
                  className="accent-cyan-500 w-4 h-4 rounded cursor-pointer"
                />
                <span className="font-semibold text-white">
                  Bật Volume Delta/CVD filter (mặc định OFF)
                </span>
              </label>
            </div>

            <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
              <div className="flex flex-col gap-1.5">
                <label className="text-xs font-semibold text-neutral-300">Chế độ lọc</label>
                <select
                  disabled={!formData.volume_delta_filter_enabled}
                  value={formData.volume_delta_mode ?? 'BOTH'}
                  onChange={(e) =>
                    setFormData({
                      ...formData,
                      volume_delta_mode: e.target.value as BotConfig['volume_delta_mode'],
                    })
                  }
                  className="w-full bg-neutral-950 border border-neutral-800 focus:border-cyan-500 rounded-lg px-3 py-2 text-xs font-mono text-white outline-none cursor-pointer disabled:opacity-40"
                >
                  <option value="ALIGN">ALIGN — delta thuận chiều lệnh</option>
                  <option value="DIVERGENCE">DIVERGENCE — giá/CVD phân kỳ</option>
                  <option value="BOTH">BOTH — ALIGN hoặc DIVERGENCE</option>
                </select>
                <span className="text-[11px] text-neutral-500">
                  ALIGN: BUY cần delta ratio ≥ ngưỡng; SELL ≤ −ngưỡng.
                </span>
              </div>

              <div className="flex flex-col gap-1.5">
                <label className="text-xs font-semibold text-neutral-300">
                  Ngưỡng |Delta Ratio| (ALIGN)
                </label>
                <div className="relative">
                  <input
                    type="number"
                    step="0.01"
                    min="0"
                    max="1"
                    disabled={!formData.volume_delta_filter_enabled}
                    value={formData.volume_delta_min_ratio ?? 0.1}
                    onChange={(e) =>
                      setFormData({
                        ...formData,
                        volume_delta_min_ratio: Math.min(
                          1,
                          Math.max(0, parseFloat(e.target.value) || 0)
                        ),
                      })
                    }
                    className="w-full bg-neutral-950 border border-neutral-800 focus:border-cyan-500 rounded-lg px-3 py-2 text-xs font-mono text-white outline-none disabled:opacity-40"
                  />
                  <span className="absolute right-3 top-2 text-xs text-neutral-500 font-mono">0–1</span>
                </div>
                <span className="text-[11px] text-neutral-500">
                  Mặc định 0.10 (±10% volume là dòng tiền chủ động).
                </span>
              </div>

              <div className="flex flex-col gap-1.5">
                <label className="text-xs font-semibold text-neutral-300">
                  Lookback CVD / Divergence
                </label>
                <div className="relative">
                  <input
                    type="number"
                    step="1"
                    min="5"
                    max="100"
                    disabled={!formData.volume_delta_filter_enabled}
                    value={formData.volume_delta_lookback ?? 20}
                    onChange={(e) =>
                      setFormData({
                        ...formData,
                        volume_delta_lookback: Math.min(
                          100,
                          Math.max(5, parseInt(e.target.value) || 20)
                        ),
                      })
                    }
                    className="w-full bg-neutral-950 border border-neutral-800 focus:border-cyan-500 rounded-lg px-3 py-2 text-xs font-mono text-white outline-none disabled:opacity-40"
                  />
                  <span className="absolute right-3 top-2 text-xs text-neutral-500 font-mono">nến</span>
                </div>
                <span className="text-[11px] text-neutral-500">Mặc định 20 nến M15.</span>
              </div>
            </div>
          </div>

          {/* AI Chief Risk Officer (optional, veto-only) */}
          <div className="rounded-xl border border-indigo-500/30 bg-indigo-500/5 p-4 flex flex-col gap-4">
            <div className="flex flex-wrap items-start justify-between gap-3">
              <div className="flex flex-col gap-1">
                <div className="text-xs font-bold text-indigo-300 uppercase tracking-wider">
                  🤖 AI Chief Risk Officer (tùy chọn — mặc định OFF)
                </div>
                <p className="text-[11px] text-neutral-400 leading-relaxed max-w-2xl">
                  Khi bật, bot gọi OpenCode Go (Gemini 4 Argon) để review mỗi tín hiệu thật trước
                  khi gửi Telegram/lưu. AI chỉ được veto hoặc chọn
                  STANDBY — <strong className="text-neutral-200">không bao giờ đặt SL/TP live</strong>.
                </p>
              </div>
              <label className="flex items-center gap-2 cursor-pointer text-xs text-neutral-300 select-none">
                <input
                  type="checkbox"
                  checked={formData.ai_signal_review_enabled ?? false}
                  onChange={(e) =>
                    setFormData({ ...formData, ai_signal_review_enabled: e.target.checked })
                  }
                  className="accent-indigo-500 w-4 h-4 rounded cursor-pointer"
                />
                <span className="font-semibold text-white">Bật AI Risk Officer</span>
              </label>
            </div>

            <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
              <div className="flex flex-col gap-1.5">
                <label className="text-xs font-semibold text-neutral-300">
                  Confidence tối thiểu để cho qua
                </label>
                <div className="relative">
                  <input
                    type="number"
                    step="0.05"
                    min="0.5"
                    max="1.0"
                    disabled={!formData.ai_signal_review_enabled}
                    value={formData.ai_signal_review_min_confidence ?? 0.75}
                    onChange={(e) =>
                      setFormData({
                        ...formData,
                        ai_signal_review_min_confidence: Math.min(
                          1,
                          Math.max(0.5, parseFloat(e.target.value) || 0.75)
                        ),
                      })
                    }
                    className="w-full bg-neutral-950 border border-neutral-800 focus:border-indigo-500 rounded-lg px-3 py-2 text-xs font-mono text-white outline-none disabled:opacity-40"
                  />
                  <span className="absolute right-3 top-2 text-xs text-neutral-500 font-mono">0.5–1.0</span>
                </div>
                <span className="text-[11px] text-neutral-500">
                  Mặc định 0.75. AI confidence thấp hơn ngưỡng → tín hiệu bị chặn.
                </span>
              </div>

              <div className="flex flex-col gap-1.5">
                <label className="text-xs font-semibold text-neutral-300">
                  Khi AI lỗi / timeout
                </label>
                <label
                  className={`flex items-start gap-2 bg-neutral-950 border border-neutral-800 rounded-lg px-3 py-2 cursor-pointer select-none ${
                    formData.ai_signal_review_enabled ? '' : 'opacity-40'
                  }`}
                >
                  <input
                    type="checkbox"
                    disabled={!formData.ai_signal_review_enabled}
                    checked={formData.ai_signal_review_fail_open ?? true}
                    onChange={(e) =>
                      setFormData({ ...formData, ai_signal_review_fail_open: e.target.checked })
                    }
                    className="accent-indigo-500 w-4 h-4 rounded cursor-pointer mt-0.5"
                  />
                  <span className="text-[11px] text-neutral-300 leading-snug">
                    Fail-open: AI lỗi thì vẫn cho tín hiệu qua (khuyến nghị)
                  </span>
                </label>
                <span className="text-[11px] text-neutral-500">
                  Tắt fail-open = fail-closed: AI không phản hồi thì chặn tín hiệu.
                </span>
              </div>

              <div className="flex flex-col gap-1.5">
                <label className="text-xs font-semibold text-neutral-300">Phạm vi áp dụng</label>
                <div className="bg-neutral-950 border border-neutral-800 rounded-lg px-3 py-2 text-[11px] text-neutral-300 leading-snug">
                  Chỉ tín hiệu thật (scanner, candle webhook, TradingView). Simulation không gọi AI
                  và không tiêu hao quota.
                </div>
                <span className="text-[11px] text-neutral-500">
                  Cần <span className="font-mono text-neutral-400">OPENCODE_GO_API_KEY</span> trong
                  <span className="font-mono text-neutral-400"> rust-backend/.env</span>.
                </span>
              </div>
            </div>
          </div>
        </div>

        {/* Section 4: Telegram Bot Integration */}
        <div className="flex flex-col gap-4">
          <div className="text-xs font-bold text-sky-400 uppercase tracking-wider border-b border-neutral-800 pb-2 flex items-center justify-between">
            <span>4. Tích hợp Telegram Bot API (Bắn Alert tự động)</span>
            <span className="text-neutral-500 text-[11px] normal-case font-normal">
              Tạo bot tại @BotFather và lấy Chat ID từ @userinfobot
            </span>
          </div>

          <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
            <div className="flex flex-col gap-1.5">
              <label className="text-xs font-semibold text-neutral-300">
                Telegram Bot Token
              </label>
              <input
                type="text"
                placeholder="1234567890:ABCdefGhIJKlmNoPQRsTUVwxyZ"
                value={formData.telegram_bot_token}
                onChange={(e) => setFormData({ ...formData, telegram_bot_token: e.target.value })}
                className="w-full bg-neutral-950 border border-neutral-800 focus:border-sky-500 rounded-lg px-3 py-2 text-xs font-mono text-white outline-none"
              />
              <span className="text-[11px] text-neutral-500">Token bảo mật do BotFather cấp</span>
            </div>

            <div className="flex flex-col gap-1.5">
              <label className="text-xs font-semibold text-neutral-300">
                Telegram Chat ID hoặc Channel Username
              </label>
              <input
                type="text"
                placeholder="-100123456789 hoặc @TenKenhCuaBan"
                value={formData.telegram_chat_id}
                onChange={(e) => setFormData({ ...formData, telegram_chat_id: e.target.value })}
                className="w-full bg-neutral-950 border border-neutral-800 focus:border-sky-500 rounded-lg px-3 py-2 text-xs font-mono text-white outline-none"
              />
              <span className="text-[11px] text-neutral-500">ID kênh / nhóm nhận thông báo tín hiệu</span>
            </div>

            <div className="flex flex-col gap-1.5 md:col-span-2">
              <label className="text-xs font-semibold text-neutral-300">
                TradingView Webhook Passphrase (tùy chọn)
              </label>
              <input
                type="password"
                placeholder="Để trống nếu không dùng TradingView Webhook"
                value={formData.webhook_passphrase}
                onChange={(e) => setFormData({ ...formData, webhook_passphrase: e.target.value })}
                className="w-full bg-neutral-950 border border-neutral-800 focus:border-sky-500 rounded-lg px-3 py-2 text-xs font-mono text-white outline-none"
              />
              <span className="text-[11px] text-neutral-500">
                Khi đặt passphrase, mọi alert TradingView phải gửi kèm đúng trường
                <code className="mx-1 text-sky-300">passphrase</code>
                mới được chấp nhận.
              </span>
            </div>
          </div>

          {/* Test connection feedback */}
          {testResult && (
            <div
              className={`p-3 rounded-lg text-xs font-mono border ${
                testResult.success
                  ? 'bg-emerald-500/10 border-emerald-500/30 text-emerald-400'
                  : 'bg-rose-500/10 border-rose-500/30 text-rose-400'
              }`}
            >
              {testResult.message}
            </div>
          )}
        </div>

        {/* Action Buttons */}
        <div className="flex flex-wrap items-center justify-between gap-4 pt-4 border-t border-neutral-800">
          <button
            type="button"
            onClick={handleTestPing}
            disabled={isTesting}
            className="px-4 py-2 bg-neutral-800 hover:bg-neutral-700 border border-neutral-700 text-sky-400 rounded-lg text-xs font-semibold transition-colors cursor-pointer disabled:opacity-50 flex items-center gap-2"
          >
            {isTesting ? 'Đang kiểm tra kết nối...' : 'Kiểm tra kết nối Telegram (Test Ping)'}
          </button>

          <div className="flex items-center gap-3">
            {saveSuccess && (
              <span className="text-xs font-mono text-emerald-400">
                ✓ Đã lưu cấu hình thành công!
              </span>
            )}
            <button
              type="submit"
              disabled={isSaving}
              className="px-5 py-2 text-xs font-bold text-neutral-950 bg-amber-400 hover:bg-amber-300 rounded-lg transition-colors cursor-pointer disabled:opacity-50"
            >
              {isSaving ? 'Đang lưu...' : 'Lưu thông số cấu hình'}
            </button>
          </div>
        </div>
      </form>
    </div>
  );
};
