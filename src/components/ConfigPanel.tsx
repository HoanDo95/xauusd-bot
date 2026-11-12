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
          Cấu hình Sniper & Quản lý Rủi ro
        </h3>
        <p className="text-xs text-neutral-400">
          Thông số được áp dụng trực tiếp cho Rust Engine & Express fallback: quota tùy chọn, khung giờ vàng
          Âu–Mỹ, đệm Stop Loss 5–25$ (M15 8–10$ / H1 15–20$), tự động giảm Lot Size để giữ nguyên % rủi ro
          và cảnh báo phân kỳ RSI độc lập (không phải entry, không tiêu hao quota).
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
              <label className="flex items-start gap-2 bg-neutral-950 border border-neutral-800 rounded-lg px-3 py-2 cursor-pointer select-none">
                <input
                  type="checkbox"
                  checked={formData.volume_filter_enabled ?? true}
                  onChange={(e) =>
                    setFormData({ ...formData, volume_filter_enabled: e.target.checked })
                  }
                  className="accent-amber-500 w-4 h-4 rounded cursor-pointer mt-0.5"
                />
                <span className="text-[11px] text-neutral-300 leading-snug">
                  Bật lọc Volume Absorption (tắt nếu nguồn không có volume)
                </span>
              </label>
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

        {/* Section 2: Optional Daily Quota & Session Filter */}
        <div className="flex flex-col gap-4">
          <div className="text-xs font-bold text-emerald-400 uppercase tracking-wider border-b border-neutral-800 pb-2 flex items-center justify-between">
            <span>2. Giới hạn lệnh/ngày (tùy chọn) & Bộ lọc phiên</span>
            <span className="text-neutral-500 text-[11px] normal-case font-normal">
              Lệnh test simulation không tiêu tốn quota
            </span>
          </div>

          <div className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-4 gap-4">
            {/* Daily quota toggle */}
            <div className="flex flex-col gap-1.5">
              <label className="text-xs font-semibold text-neutral-300">
                Giới hạn số lệnh/ngày
              </label>
              <label className="flex items-start gap-2 bg-neutral-950 border border-neutral-800 rounded-lg px-3 py-2 cursor-pointer select-none">
                <input
                  type="checkbox"
                  checked={formData.daily_quota_enabled ?? false}
                  onChange={(e) =>
                    setFormData({ ...formData, daily_quota_enabled: e.target.checked })
                  }
                  className="accent-emerald-500 w-4 h-4 rounded cursor-pointer mt-0.5"
                />
                <span className="text-[11px] text-neutral-300 leading-snug">
                  {formData.daily_quota_enabled ? 'ĐANG BẬT — có giới hạn' : 'ĐANG TẮT — không giới hạn'}
                </span>
              </label>
              <span className="text-[11px] text-neutral-500">
                Mặc định OFF — bot bắn mọi setup đủ điều kiện. Bật để giới hạn 1–5 lệnh/ngày.
              </span>
            </div>

            {/* Max signals per day */}
            <div className="flex flex-col gap-1.5">
              <label className="text-xs font-semibold text-neutral-300">
                Giới hạn Lệnh / Ngày
              </label>
              <div className="relative">
                <input
                  type="number"
                  min="1"
                  max="5"
                  step="1"
                  disabled={!(formData.daily_quota_enabled ?? false)}
                  value={formData.max_signals_per_day ?? 1}
                  onChange={(e) =>
                    setFormData({
                      ...formData,
                      max_signals_per_day: Math.max(1, Math.min(5, parseInt(e.target.value) || 1)),
                    })
                  }
                  className="w-full bg-neutral-950 border border-neutral-800 focus:border-emerald-500 rounded-lg px-3 py-2 text-xs font-mono text-white outline-none disabled:opacity-40 disabled:cursor-not-allowed"
                />
                <span className="absolute right-3 top-2 text-xs text-neutral-500 font-mono">Lệnh</span>
              </div>
              <span className="text-[11px] text-neutral-500">
                Chỉ áp dụng khi toggle bật (1–5 lệnh/ngày).
              </span>
            </div>

            {/* Free live feed (read only) */}
            <div className="flex flex-col gap-1.5">
              <label className="text-xs font-semibold text-neutral-300">
                Nguồn dữ liệu Live (Miễn phí)
              </label>
              <div className="w-full bg-sky-500/10 border border-sky-500/30 rounded-lg px-3 py-2 text-xs font-mono text-sky-300">
                {formData.data_source === 'TWELVEDATA'
                  ? '📡 TwelveData XAU/USD + Binance PAXG volume proxy'
                  : '📡 Binance PAXG/USDT (OHLCV + Volume)'}
              </div>
              <span className="text-[11px] text-neutral-500">
                TwelveData XAU/USD cung cấp OHLC; volume lấy từ Binance PAXG proxy. Nếu proxy lỗi,
                tắt bộ lọc Volume Absorption bên trên.
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
                  Ưu tiên phiên Âu - Mỹ <span className="font-mono text-emerald-400">13:00 - 23:00 VN</span> để tập trung vào khung thanh khoản cao nhất
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

          <label className="flex items-center gap-2 cursor-pointer text-xs text-neutral-300 select-none">
            <input
              type="checkbox"
              checked={formData.macro_flow_enabled ?? true}
              onChange={(e) =>
                setFormData({ ...formData, macro_flow_enabled: e.target.checked })
              }
              className="accent-sky-500 w-4 h-4 rounded cursor-pointer"
            />
            <span className="font-semibold text-white">
              Bật Macro &amp; Flow dashboard (read-only)
            </span>
          </label>
          <p className="text-[11px] text-neutral-500 leading-relaxed -mt-2">
            FRED + Yahoo + CFTC: bối cảnh vĩ mô/dòng tiền trước khi vào lệnh. Chỉ hiển thị, không
            tham gia tạo tín hiệu, bộ lọc, quota hay Telegram.
          </p>

          <label className="flex items-center gap-2 cursor-pointer text-xs text-neutral-300 select-none">
            <input
              type="checkbox"
              checked={formData.daily_plan_enabled ?? true}
              onChange={(e) =>
                setFormData({ ...formData, daily_plan_enabled: e.target.checked })
              }
              className="accent-sky-500 w-4 h-4 rounded cursor-pointer"
            />
            <span className="font-semibold text-white">
              Bật kế hoạch ngày 07:00 qua Telegram
            </span>
          </label>
          <p className="text-[11px] text-neutral-500 leading-relaxed -mt-2">
            Gửi macro/news + PRIMARY/ALTERNATE/WATCHLIST mỗi ngày lúc 07:00 (giờ server). Chỉ đọc:
            không tiêu hao quota, không chèn tín hiệu và không tự vào lệnh.
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

          {/* RSI context + optional hard filter (default OFF) */}
          <div className="rounded-xl border border-rose-500/30 bg-rose-500/5 p-4 flex flex-col gap-4">
            <div className="flex flex-wrap items-start justify-between gap-3">
              <div className="flex flex-col gap-1">
                <div className="text-xs font-bold text-rose-300 uppercase tracking-wider">
                  📐 RSI Wilder — context & bộ lọc tùy chọn (mặc định OFF)
                </div>
                <p className="text-[11px] text-neutral-400 leading-relaxed max-w-2xl">
                  RSI M15/H1/H4 luôn được hiển thị và gửi kèm AI Risk Officer như context. Bộ lọc
                  cứng bên dưới chỉ chặn tín hiệu thật khi được bật; simulation luôn bỏ qua.
                </p>
              </div>
              <label className="flex items-center gap-2 cursor-pointer text-xs text-neutral-300 select-none">
                <input
                  type="checkbox"
                  checked={formData.rsi_filter_enabled ?? false}
                  onChange={(e) =>
                    setFormData({ ...formData, rsi_filter_enabled: e.target.checked })
                  }
                  className="accent-rose-500 w-4 h-4 rounded cursor-pointer"
                />
                <span className="font-semibold text-white">
                  Bật bộ lọc RSI (mặc định OFF)
                </span>
              </label>
            </div>

            <p className="rounded-lg border border-amber-500/40 bg-amber-500/10 px-3 py-2 text-[11px] text-amber-200 leading-relaxed">
              ⚠️ Backtest R0: RSI 70/30 tại nến trigger = 0/871 lệnh; divergence −4.76R. Chỉ bật
              sau khi forward-test có bằng chứng mới.
            </p>

            <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
              <div className="flex flex-col gap-1.5">
                <label className="text-xs font-semibold text-neutral-300">Chế độ lọc RSI</label>
                <select
                  disabled={!formData.rsi_filter_enabled}
                  value={formData.rsi_mode ?? 'BOTH'}
                  onChange={(e) =>
                    setFormData({
                      ...formData,
                      rsi_mode: e.target.value as BotConfig['rsi_mode'],
                    })
                  }
                  className="w-full bg-neutral-950 border border-neutral-800 focus:border-rose-500 rounded-lg px-3 py-2 text-xs font-mono text-white outline-none cursor-pointer disabled:opacity-40"
                >
                  <option value="EXTREME">EXTREME — BUY oversold / SELL overbought</option>
                  <option value="DIVERGENCE">DIVERGENCE — Bullish / Bearish divergence</option>
                  <option value="BOTH">BOTH — EXTREME hoặc DIVERGENCE</option>
                </select>
                <span className="text-[11px] text-neutral-500">
                  BOTH là mặc định khi bật; EXTREME khớp backtest R0 70/30.
                </span>
              </div>

              <div className="flex flex-col gap-1.5">
                <label className="text-xs font-semibold text-neutral-300">Overbought</label>
                <div className="relative">
                  <input
                    type="number"
                    step="1"
                    min="50"
                    max="100"
                    disabled={!formData.rsi_filter_enabled}
                    value={formData.rsi_overbought ?? 70}
                    onChange={(e) =>
                      setFormData({
                        ...formData,
                        rsi_overbought: Math.min(100, Math.max(50, parseFloat(e.target.value) || 70)),
                      })
                    }
                    className="w-full bg-neutral-950 border border-neutral-800 focus:border-rose-500 rounded-lg px-3 py-2 text-xs font-mono text-white outline-none disabled:opacity-40"
                  />
                  <span className="absolute right-3 top-2 text-xs text-neutral-500 font-mono">50–100</span>
                </div>
                <span className="text-[11px] text-neutral-500">Mặc định 70.</span>
              </div>

              <div className="flex flex-col gap-1.5">
                <label className="text-xs font-semibold text-neutral-300">Oversold</label>
                <div className="relative">
                  <input
                    type="number"
                    step="1"
                    min="0"
                    max="50"
                    disabled={!formData.rsi_filter_enabled}
                    value={formData.rsi_oversold ?? 30}
                    onChange={(e) =>
                      setFormData({
                        ...formData,
                        rsi_oversold: Math.min(50, Math.max(0, parseFloat(e.target.value) || 30)),
                      })
                    }
                    className="w-full bg-neutral-950 border border-neutral-800 focus:border-rose-500 rounded-lg px-3 py-2 text-xs font-mono text-white outline-none disabled:opacity-40"
                  />
                  <span className="absolute right-3 top-2 text-xs text-neutral-500 font-mono">0–50</span>
                </div>
                <span className="text-[11px] text-neutral-500">
                  Mặc định 30; backend reset 30/70 nếu oversold ≥ overbought.
                </span>
              </div>

              <div className="flex flex-col gap-1.5">
                <label className="text-xs font-semibold text-neutral-300">Chu kỳ RSI (Wilder)</label>
                <div className="relative">
                  <input
                    type="number"
                    step="1"
                    min="2"
                    max="100"
                    disabled={!formData.rsi_filter_enabled}
                    value={formData.rsi_period ?? 14}
                    onChange={(e) =>
                      setFormData({
                        ...formData,
                        rsi_period: Math.min(100, Math.max(2, parseInt(e.target.value) || 14)),
                      })
                    }
                    className="w-full bg-neutral-950 border border-neutral-800 focus:border-rose-500 rounded-lg px-3 py-2 text-xs font-mono text-white outline-none disabled:opacity-40"
                  />
                  <span className="absolute right-3 top-2 text-xs text-neutral-500 font-mono">nến</span>
                </div>
                <span className="text-[11px] text-neutral-500">Mặc định 14.</span>
              </div>

              <div className="flex flex-col gap-1.5">
                <label className="text-xs font-semibold text-neutral-300">
                  Lookback divergence
                </label>
                <div className="relative">
                  <input
                    type="number"
                    step="1"
                    min="10"
                    max="200"
                    disabled={!formData.rsi_filter_enabled}
                    value={formData.rsi_divergence_lookback ?? 40}
                    onChange={(e) =>
                      setFormData({
                        ...formData,
                        rsi_divergence_lookback: Math.min(
                          200,
                          Math.max(10, parseInt(e.target.value) || 40)
                        ),
                      })
                    }
                    className="w-full bg-neutral-950 border border-neutral-800 focus:border-rose-500 rounded-lg px-3 py-2 text-xs font-mono text-white outline-none disabled:opacity-40"
                  />
                  <span className="absolute right-3 top-2 text-xs text-neutral-500 font-mono">nến</span>
                </div>
                <span className="text-[11px] text-neutral-500">Mặc định 40 nến M15.</span>
              </div>

              <div className="flex flex-col gap-1.5">
                <label className="text-xs font-semibold text-neutral-300">
                  Pivot K (xác nhận mỗi bên)
                </label>
                <div className="relative">
                  <input
                    type="number"
                    step="1"
                    min="1"
                    max="10"
                    disabled={!formData.rsi_filter_enabled}
                    value={formData.rsi_pivot_k ?? 2}
                    onChange={(e) =>
                      setFormData({
                        ...formData,
                        rsi_pivot_k: Math.min(10, Math.max(1, parseInt(e.target.value) || 2)),
                      })
                    }
                    className="w-full bg-neutral-950 border border-neutral-800 focus:border-rose-500 rounded-lg px-3 py-2 text-xs font-mono text-white outline-none disabled:opacity-40"
                  />
                  <span className="absolute right-3 top-2 text-xs text-neutral-500 font-mono">1–10</span>
                </div>
                <span className="text-[11px] text-neutral-500">Mặc định 2 nến mỗi bên.</span>
              </div>
            </div>

            <label
              className={`flex items-start gap-2 bg-neutral-950 border border-neutral-800 rounded-lg px-3 py-2 cursor-pointer select-none ${
                formData.rsi_filter_enabled ? '' : 'opacity-40'
              }`}
            >
              <input
                type="checkbox"
                disabled={!formData.rsi_filter_enabled}
                checked={formData.rsi_divergence_enabled ?? true}
                onChange={(e) =>
                  setFormData({ ...formData, rsi_divergence_enabled: e.target.checked })
                }
                className="accent-rose-500 w-4 h-4 rounded cursor-pointer mt-0.5"
              />
              <span className="text-[11px] text-neutral-300 leading-snug">
                Dùng divergence trong bộ lọc RSI (mặc định bật). Tắt = chỉ xét EXTREME.
              </span>
            </label>
          </div>

          {/* Standalone RSI divergence alerts (advisory, no quota) */}
          <div className="rounded-xl border border-amber-500/30 bg-amber-500/5 p-4 flex flex-col gap-4">
            <div className="flex flex-wrap items-start justify-between gap-3">
              <div className="flex flex-col gap-1">
                <div className="text-xs font-bold text-amber-300 uppercase tracking-wider">
                  🔔 Cảnh báo phân kỳ RSI — độc lập (không phải entry)
                </div>
                <p className="text-[11px] text-neutral-400 leading-relaxed max-w-2xl">
                  Gửi cảnh báo Telegram khi M15/H1/H4 xuất hiện phân kỳ RSI/giá. Cảnh báo chỉ
                  mang tính động lượng: không tạo tín hiệu, không có entry/SL/TP, không tiêu hao
                  quota và không lưu vào lịch sử signals.
                </p>
              </div>
              <label className="flex items-center gap-2 cursor-pointer text-xs text-neutral-300 select-none">
                <input
                  type="checkbox"
                  checked={formData.divergence_alert_enabled ?? true}
                  onChange={(e) =>
                    setFormData({ ...formData, divergence_alert_enabled: e.target.checked })
                  }
                  className="accent-amber-500 w-4 h-4 rounded cursor-pointer"
                />
                <span className="font-semibold text-white">Bật cảnh báo phân kỳ RSI</span>
              </label>
            </div>

            <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
              <label
                className={`flex items-start gap-2 bg-neutral-950 border border-neutral-800 rounded-lg px-3 py-2 cursor-pointer select-none ${
                  (formData.divergence_alert_enabled ?? true) ? '' : 'opacity-40'
                }`}
              >
                <input
                  type="checkbox"
                  disabled={!(formData.divergence_alert_enabled ?? true)}
                  checked={formData.divergence_alert_m15 ?? true}
                  onChange={(e) =>
                    setFormData({ ...formData, divergence_alert_m15: e.target.checked })
                  }
                  className="accent-amber-500 w-4 h-4 rounded cursor-pointer mt-0.5"
                />
                <span className="text-[11px] text-neutral-300 leading-snug uppercase font-mono font-bold">
                  M15
                </span>
              </label>

              <label
                className={`flex items-start gap-2 bg-neutral-950 border border-neutral-800 rounded-lg px-3 py-2 cursor-pointer select-none ${
                  (formData.divergence_alert_enabled ?? true) ? '' : 'opacity-40'
                }`}
              >
                <input
                  type="checkbox"
                  disabled={!(formData.divergence_alert_enabled ?? true)}
                  checked={formData.divergence_alert_h1 ?? true}
                  onChange={(e) =>
                    setFormData({ ...formData, divergence_alert_h1: e.target.checked })
                  }
                  className="accent-amber-500 w-4 h-4 rounded cursor-pointer mt-0.5"
                />
                <span className="text-[11px] text-neutral-300 leading-snug uppercase font-mono font-bold">
                  H1
                </span>
              </label>

              <label
                className={`flex items-start gap-2 bg-neutral-950 border border-neutral-800 rounded-lg px-3 py-2 cursor-pointer select-none ${
                  (formData.divergence_alert_enabled ?? true) ? '' : 'opacity-40'
                }`}
              >
                <input
                  type="checkbox"
                  disabled={!(formData.divergence_alert_enabled ?? true)}
                  checked={formData.divergence_alert_h4 ?? true}
                  onChange={(e) =>
                    setFormData({ ...formData, divergence_alert_h4: e.target.checked })
                  }
                  className="accent-amber-500 w-4 h-4 rounded cursor-pointer mt-0.5"
                />
                <span className="text-[11px] text-neutral-300 leading-snug uppercase font-mono font-bold">
                  H4
                </span>
              </label>

              <div className="flex flex-col gap-1.5">
                <label className="text-xs font-semibold text-neutral-300">
                  Cooldown cảnh báo
                </label>
                <div className="relative">
                  <input
                    type="number"
                    min="5"
                    max="1440"
                    step="5"
                    disabled={!(formData.divergence_alert_enabled ?? true)}
                    value={formData.divergence_alert_cooldown_minutes ?? 60}
                    onChange={(e) =>
                      setFormData({
                        ...formData,
                        divergence_alert_cooldown_minutes: Math.min(
                          1440,
                          Math.max(5, parseInt(e.target.value) || 60)
                        ),
                      })
                    }
                    className="w-full bg-neutral-950 border border-neutral-800 focus:border-amber-500 rounded-lg px-3 py-2 text-xs font-mono text-white outline-none disabled:opacity-40"
                  />
                  <span className="absolute right-3 top-2 text-xs text-neutral-500 font-mono">phút</span>
                </div>
                <span className="text-[11px] text-neutral-500">
                  5–1440 phút, mặc định 60. Áp dụng riêng cho từng khung + hướng phân kỳ.
                </span>
              </div>
            </div>
          </div>

          {/* Action Trigger alerts (read-only, no quota) */}
          <div className="rounded-xl border border-sky-500/30 bg-sky-500/5 p-4 flex flex-col gap-4">
            <div className="flex flex-wrap items-start justify-between gap-3">
              <div className="flex flex-col gap-1">
                <div className="text-xs font-bold text-sky-300 uppercase tracking-wider">
                  🚨 Cảnh báo trigger kịch bản chính — không phải trade signal
                </div>
                <p className="text-[11px] text-neutral-400 leading-relaxed max-w-2xl">
                  Gửi Telegram khi kịch bản PRIMARY valid đạt trigger trên nến M15 đã đóng
                  (ABOVE/BELOW/TOUCH). Chỉ cảnh báo: không chèn tín hiệu, không tiêu hao quota,
                  không lưu trade state và không tự vào lệnh.
                </p>
              </div>
              <label className="flex items-center gap-2 cursor-pointer text-xs text-neutral-300 select-none">
                <input
                  type="checkbox"
                  checked={formData.action_trigger_alert_enabled ?? true}
                  onChange={(e) =>
                    setFormData({ ...formData, action_trigger_alert_enabled: e.target.checked })
                  }
                  className="accent-sky-500 w-4 h-4 rounded cursor-pointer"
                />
                <span className="font-semibold text-white">
                  Bật cảnh báo Telegram khi trigger kịch bản
                </span>
              </label>
            </div>

            <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
              <div className="flex flex-col gap-1.5">
                <label className="text-xs font-semibold text-neutral-300">
                  Cooldown cảnh báo trigger
                </label>
                <div className="relative">
                  <input
                    type="number"
                    min="5"
                    max="1440"
                    step="5"
                    disabled={!(formData.action_trigger_alert_enabled ?? true)}
                    value={formData.action_trigger_cooldown_minutes ?? 30}
                    onChange={(e) =>
                      setFormData({
                        ...formData,
                        action_trigger_cooldown_minutes: Math.min(
                          1440,
                          Math.max(5, parseInt(e.target.value) || 30)
                        ),
                      })
                    }
                    className="w-full bg-neutral-950 border border-neutral-800 focus:border-sky-500 rounded-lg px-3 py-2 text-xs font-mono text-white outline-none disabled:opacity-40"
                  />
                  <span className="absolute right-3 top-2 text-xs text-neutral-500 font-mono">
                    phút
                  </span>
                </div>
                <span className="text-[11px] text-neutral-500">
                  5–1440 phút, mặc định 30. Dedup theo scenario + hướng + mức trigger.
                </span>
              </div>
            </div>
          </div>

          {/* AI Decision Mode / legacy VETO_ONLY */}
          <div className="rounded-xl border border-indigo-500/30 bg-indigo-500/5 p-4 flex flex-col gap-4">
            <div className="flex flex-wrap items-start justify-between gap-3">
              <div className="flex flex-col gap-1">
                <div className="text-xs font-bold text-indigo-300 uppercase tracking-wider">
                  🧠 AI Decision Mode (mặc định BẬT)
                </div>
                <p className="text-[11px] text-neutral-400 leading-relaxed max-w-2xl">
                  Ở chế độ <strong className="text-neutral-200">DECISION</strong>, AI là lớp quyết
                  định chính: nhận full brief (D1/H4/H1, macro/flow, RSI, volume delta, scenarios,
                  news) rồi chọn BUY / SELL / STANDBY. Code chỉ giữ hard risk: SL/TP ordering, R:R,
                  risk budget, quota, session, news, dedup.{' '}
                  <strong className="text-neutral-200">AI không bao giờ đặt SL/TP live</strong>.
                </p>
              </div>
              <label className="flex items-center gap-2 cursor-pointer text-xs text-neutral-300 select-none">
                <input
                  type="checkbox"
                  checked={formData.ai_signal_review_enabled ?? true}
                  onChange={(e) =>
                    setFormData({ ...formData, ai_signal_review_enabled: e.target.checked })
                  }
                  className="accent-indigo-500 w-4 h-4 rounded cursor-pointer"
                />
                <span className="font-semibold text-white">Bật AI</span>
              </label>
            </div>

            <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
              <div className="flex flex-col gap-1.5">
                <label className="text-xs font-semibold text-neutral-300">Chế độ AI</label>
                <select
                  disabled={!formData.ai_signal_review_enabled}
                  value={formData.ai_review_mode ?? 'DECISION'}
                  onChange={(e) =>
                    setFormData({
                      ...formData,
                      ai_review_mode: e.target.value as 'DECISION' | 'VETO_ONLY',
                    })
                  }
                  className="w-full bg-neutral-950 border border-neutral-800 focus:border-indigo-500 rounded-lg px-3 py-2 text-xs font-mono text-white outline-none disabled:opacity-40"
                >
                  <option value="DECISION">DECISION — AI quyết BUY/SELL/STANDBY</option>
                  <option value="VETO_ONLY">VETO_ONLY — hành vi cũ</option>
                </select>
                <span className="text-[11px] text-neutral-500">
                  DECISION: AI quyết định, code validate risk. VETO_ONLY: AI chỉ veto tín hiệu
                  deterministic.
                </span>
              </div>

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
                  Mặc định 0.75. DECISION: AI confidence thấp hơn ngưỡng → không tín hiệu.
                  VETO_ONLY: tín hiệu bị veto.
                </span>
              </div>

              <div className="flex flex-col gap-1.5">
                <label className="text-xs font-semibold text-neutral-300">
                  Duyệt scenario sớm (90% là đủ)
                </label>
                <label className="flex items-center gap-2 bg-neutral-950 border border-neutral-800 rounded-lg px-3 py-2 cursor-pointer select-none">
                  <input
                    type="checkbox"
                    disabled={!formData.ai_signal_review_enabled}
                    checked={formData.early_entry_enabled ?? true}
                    onChange={(e) =>
                      setFormData({ ...formData, early_entry_enabled: e.target.checked })
                    }
                    className="accent-indigo-500 w-4 h-4 rounded cursor-pointer"
                  />
                  <span className="text-[11px] text-neutral-300">
                    Cho phép entry trước khi trigger xác nhận
                  </span>
                </label>
                <div className="relative">
                  <input
                    type="number"
                    step="1"
                    min="50"
                    max="100"
                    disabled={
                      !formData.ai_signal_review_enabled ||
                      !(formData.early_entry_enabled ?? true)
                    }
                    value={formData.early_entry_min_confidence ?? 90}
                    onChange={(e) =>
                      setFormData({
                        ...formData,
                        early_entry_min_confidence: Math.min(
                          100,
                          Math.max(50, parseInt(e.target.value, 10) || 90)
                        ),
                      })
                    }
                    className="w-full bg-neutral-950 border border-neutral-800 focus:border-indigo-500 rounded-lg px-3 py-2 text-xs font-mono text-white outline-none disabled:opacity-40"
                  />
                  <span className="absolute right-3 top-2 text-xs text-neutral-500 font-mono">%</span>
                </div>
                <span className="text-[11px] text-neutral-500">
                  AI được duyệt PRIMARY/ALTERNATE khi confidence ≥ ngưỡng, dù trigger M15 chưa in.
                  Entry vẫn là mức trigger (lệnh chờ), không chase giá.
                </span>
              </div>

              <div className="flex flex-col gap-1.5">
                <label className="text-xs font-semibold text-neutral-300">
                  Anti-chase breakout
                </label>
                <label className="flex items-center gap-2 bg-neutral-950 border border-neutral-800 rounded-lg px-3 py-2 cursor-pointer select-none">
                  <input
                    type="checkbox"
                    checked={formData.anti_chase_filter_enabled ?? true}
                    onChange={(e) =>
                      setFormData({
                        ...formData,
                        anti_chase_filter_enabled: e.target.checked,
                      })
                    }
                    className="accent-indigo-500 w-4 h-4 rounded cursor-pointer"
                  />
                  <span className="text-[11px] text-neutral-300">
                    Chặn breakout sau ngày đã chạy mạnh
                  </span>
                </label>
                <div className="grid grid-cols-2 gap-2">
                  <div className="relative">
                    <input
                      type="number"
                      step="0.1"
                      min="-10"
                      max="-0.5"
                      disabled={!(formData.anti_chase_filter_enabled ?? true)}
                      value={formData.anti_chase_daily_change_pct ?? -2.0}
                      onChange={(e) =>
                        setFormData({
                          ...formData,
                          anti_chase_daily_change_pct: Math.min(
                            -0.5,
                            Math.max(-10, parseFloat(e.target.value) || -2.0)
                          ),
                        })
                      }
                      className="w-full bg-neutral-950 border border-neutral-800 focus:border-indigo-500 rounded-lg px-3 py-2 text-xs font-mono text-white outline-none disabled:opacity-40"
                    />
                    <span className="absolute right-3 top-2 text-[10px] text-neutral-500 font-mono">%</span>
                  </div>
                  <div className="relative">
                    <input
                      type="number"
                      step="0.5"
                      min="10"
                      max="40"
                      disabled={!(formData.anti_chase_filter_enabled ?? true)}
                      value={formData.anti_chase_rsi_h1 ?? 25.0}
                      onChange={(e) =>
                        setFormData({
                          ...formData,
                          anti_chase_rsi_h1: Math.min(
                            40,
                            Math.max(10, parseFloat(e.target.value) || 25.0)
                          ),
                        })
                      }
                      className="w-full bg-neutral-950 border border-neutral-800 focus:border-indigo-500 rounded-lg px-3 py-2 text-xs font-mono text-white outline-none disabled:opacity-40"
                    />
                    <span className="absolute right-3 top-2 text-[10px] text-neutral-500 font-mono">RSI</span>
                  </div>
                </div>
                <span className="text-[11px] text-neutral-500">
                  Daily change ≤ ngưỡng và (RSI H1 cực đoan hoặc M15 divergence ngược chiều) → tắt
                  breakout/breakdown, ưu tiên pullback. Không chase sau ngày đã chạy mạnh.
                </span>
              </div>

              <div className="flex flex-col gap-1.5">
                <label className="text-xs font-semibold text-neutral-300">
                  Khi AI lỗi / timeout
                </label>
                <label
                  className={`flex items-start gap-2 bg-neutral-950 border border-neutral-800 rounded-lg px-3 py-2 cursor-pointer select-none ${
                    formData.ai_signal_review_enabled &&
                    (formData.ai_review_mode ?? 'DECISION') === 'VETO_ONLY'
                      ? ''
                      : 'opacity-40'
                  }`}
                >
                  <input
                    type="checkbox"
                    disabled={
                      !formData.ai_signal_review_enabled ||
                      (formData.ai_review_mode ?? 'DECISION') !== 'VETO_ONLY'
                    }
                    checked={formData.ai_signal_review_fail_open ?? true}
                    onChange={(e) =>
                      setFormData({ ...formData, ai_signal_review_fail_open: e.target.checked })
                    }
                    className="accent-indigo-500 w-4 h-4 rounded cursor-pointer mt-0.5"
                  />
                  <span className="text-[11px] text-neutral-300 leading-snug">
                    VETO_ONLY: fail-open khi AI lỗi (mặc định)
                  </span>
                </label>
                <span className="text-[11px] text-neutral-500">
                  DECISION luôn fail-closed: AI không phản hồi → không tín hiệu, bất kể flag này.
                  Tắt fail-open ở VETO_ONLY cũng chặn tín hiệu.
                </span>
              </div>

              <div className="flex flex-col gap-1.5">
                <label className="text-xs font-semibold text-neutral-300">Phạm vi áp dụng</label>
                <div className="bg-neutral-950 border border-neutral-800 rounded-lg px-3 py-2 text-[11px] text-neutral-300 leading-snug">
                  DECISION nới soft filters (volume/RSI/HTF/macro) để AI review candidate; hard
                  risk vẫn do code chặn. Chỉ tín hiệu thật; simulation không gọi AI/quota.
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
                type="password"
                placeholder="Để trống = giữ token hiện tại"
                value={formData.telegram_bot_token}
                onChange={(e) => setFormData({ ...formData, telegram_bot_token: e.target.value })}
                className="w-full bg-neutral-950 border border-neutral-800 focus:border-sky-500 rounded-lg px-3 py-2 text-xs font-mono text-white outline-none"
              />
              <span className="text-[11px] text-neutral-500">
                Token không còn được trả về từ API; nhập token mới nếu muốn thay đổi.
              </span>
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
                placeholder="Để trống = giữ passphrase hiện tại (nếu đã đặt)"
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
