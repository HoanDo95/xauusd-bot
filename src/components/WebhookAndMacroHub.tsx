'use client';

import React, { useState, useEffect } from 'react';
import {
  Webhook,
  Clock,
  Sparkles,
  ShieldCheck,
  ShieldAlert,
  Copy,
  Check,
  Send,
  RefreshCw,
  TrendingUp,
  TrendingDown,
  AlertTriangle,
  Code2,
  Terminal,
} from 'lucide-react';
import { BotConfig, DailyMacroReport, NewsCalendarData } from '../types';

interface WebhookAndMacroHubProps {
  config: BotConfig;
  newsData: NewsCalendarData | null;
  onRefreshCalendar: () => Promise<void>;
  onSignalCreated?: () => void;
}

export const WebhookAndMacroHub: React.FC<WebhookAndMacroHubProps> = ({
  config,
  newsData,
  onRefreshCalendar,
  onSignalCreated,
}) => {
  const [activeTab, setActiveTab] = useState<'cron' | 'webhook'>('cron');
  const [dailyReport, setDailyReport] = useState<DailyMacroReport | null>(null);
  const [isLoadingReport, setIsLoadingReport] = useState<boolean>(false);
  const [isTriggeringCron, setIsTriggeringCron] = useState<boolean>(false);
  const [cronSuccessMsg, setCronSuccessMsg] = useState<string | null>(null);

  // Webhook Simulator state
  const [webhookAction, setWebhookAction] = useState<'BUY' | 'SELL' | 'CANDLE'>('BUY');
  const [webhookPrice, setWebhookPrice] = useState<number>(2684.50);
  const [webhookComment, setWebhookComment] = useState<string>('TradingView PineScript: Pinbar Rejection at Daily Support');
  const [isSendingWebhook, setIsSendingWebhook] = useState<boolean>(false);
  const [webhookResponse, setWebhookResponse] = useState<any | null>(null);
  const [copiedUrl, setCopiedUrl] = useState<boolean>(false);
  const [copiedPine, setCopiedPine] = useState<boolean>(false);

  const webhookUrl =
    typeof window !== 'undefined'
      ? `${window.location.origin}/api/webhook/tradingview`
      : '/api/webhook/tradingview';

  // Fetch initial macro report
  const loadMacroReport = async () => {
    setIsLoadingReport(true);
    try {
      const res = await fetch('/api/macro-bias/latest');
      const data = await res.json();
      if (data.daily_report) {
        setDailyReport(data.daily_report);
      }
    } catch (err) {
      console.error('Error fetching macro report:', err);
    } finally {
      setIsLoadingReport(false);
    }
  };

  useEffect(() => {
    loadMacroReport();
  }, []);

  // Trigger manual 07:00 AM Cron run
  const handleTriggerDailyCron = async () => {
    setIsTriggeringCron(true);
    setCronSuccessMsg(null);
    try {
      const res = await fetch('/api/cron/trigger-daily-macro', {
        method: 'POST',
      });
      const data = await res.json();
      if (res.ok && data.report) {
        setDailyReport(data.report);
        setCronSuccessMsg('✅ Đã chạy thành công Daily Cron: Cập nhật ForexFactory & xu hướng EMA50/200 mới nhất!');
        await onRefreshCalendar();
        setTimeout(() => setCronSuccessMsg(null), 4000);
      }
    } catch (err: any) {
      setCronSuccessMsg(`❌ Lỗi: ${err.message}`);
    } finally {
      setIsTriggeringCron(false);
    }
  };

  // Test TradingView Webhook
  const handleSendWebhook = async () => {
    setIsSendingWebhook(true);
    setWebhookResponse(null);
    try {
      const payload = {
        passphrase: 'SECRET_GOLD_QUANT_BOT',
        ticker: 'XAUUSD',
        action: webhookAction,
        price: webhookPrice,
        timeframe: 'M15',
        comment: webhookComment,
        timestamp: Date.now(),
      };

      const res = await fetch('/api/webhook/tradingview', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(payload),
      });

      const data = await res.json();
      setWebhookResponse(data);
      if (data.status === 'signal_executed' && onSignalCreated) {
        onSignalCreated();
      }
    } catch (err: any) {
      setWebhookResponse({ status: 'network_error', message: err.message });
    } finally {
      setIsSendingWebhook(false);
    }
  };

  const pineScriptSample = `//@version=5
strategy("Gold Pinbar & Volume Strategy", overlay=true)
// Gửi webhook alert khi có tín hiệu
if (buyCondition)
    alert('{"passphrase": "SECRET_GOLD_QUANT_BOT", "ticker": "XAUUSD", "action": "BUY", "price": ' + str.tostring(close) + ', "comment": "Bullish Pinbar at Support"}', alert.freq_once_per_bar_close)
if (sellCondition)
    alert('{"passphrase": "SECRET_GOLD_QUANT_BOT", "ticker": "XAUUSD", "action": "SELL", "price": ' + str.tostring(close) + ', "comment": "Bearish Pinbar at Resistance"}', alert.freq_once_per_bar_close)`;

  return (
    <div className="bg-[#12161f] border border-zinc-800 rounded-xl p-5 shadow-lg relative overflow-hidden">
      {/* Top Header & Tab Controls */}
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-4 pb-4 border-b border-zinc-800">
        <div className="flex items-center gap-3">
          <div className="w-10 h-10 rounded-lg bg-amber-500/10 border border-amber-500/30 flex items-center justify-center text-amber-400">
            {activeTab === 'cron' ? <Sparkles className="w-5 h-5" /> : <Webhook className="w-5 h-5" />}
          </div>
          <div>
            <div className="flex items-center gap-2">
              <h3 className="text-sm font-semibold text-zinc-100 uppercase tracking-wide">
                Free Data & Macro Hub (Rust Async Engine)
              </h3>
              <span className="text-[10px] px-2 py-0.5 rounded bg-zinc-800 text-amber-400 border border-amber-500/20 font-mono">
                ForexFactory + EMA50 · Free 100%
              </span>
            </div>
            <p className="text-xs text-zinc-400">
              Đồng bộ lịch tin ForexFactory & tính xu hướng EMA50/200 miễn phí mỗi ngày, đồng thời tiếp nhận tín hiệu từ TradingView Webhook.
            </p>
          </div>
        </div>

        {/* Tab switch buttons */}
        <div className="flex items-center gap-1.5 bg-zinc-900 p-1 rounded-lg border border-zinc-800">
          <button
            onClick={() => setActiveTab('cron')}
            className={`px-3 py-1.5 rounded-md text-xs font-medium transition flex items-center gap-1.5 ${
              activeTab === 'cron'
                ? 'bg-amber-500 text-zinc-950 font-semibold shadow-sm'
                : 'text-zinc-400 hover:text-zinc-200'
            }`}
          >
            <Clock className="w-3.5 h-3.5" />
            <span>Daily Cron 07:00 AM</span>
          </button>
          <button
            onClick={() => setActiveTab('webhook')}
            className={`px-3 py-1.5 rounded-md text-xs font-medium transition flex items-center gap-1.5 ${
              activeTab === 'webhook'
                ? 'bg-amber-500 text-zinc-950 font-semibold shadow-sm'
                : 'text-zinc-400 hover:text-zinc-200'
            }`}
          >
            <Webhook className="w-3.5 h-3.5" />
            <span>TradingView Webhook</span>
          </button>
        </div>
      </div>

      {/* TAB 1: DAILY CRON & EMA MACRO REPORT */}
      {activeTab === 'cron' && (
        <div className="mt-5 space-y-5 animate-fadeIn">
          {/* Header Action Bar */}
          <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 p-3 rounded-lg bg-zinc-900/80 border border-zinc-800">
            <div className="flex items-center gap-2 text-xs">
              <span className="w-2.5 h-2.5 rounded-full bg-emerald-400 animate-pulse" />
              <span className="text-zinc-300 font-medium">Lịch trình tự động:</span>
              <span className="font-mono text-amber-400 font-semibold">07:00:00 Sáng mỗi ngày</span>
              <span className="text-zinc-500 hidden md:inline">|</span>
              <span className="text-zinc-400 hidden md:inline">
                Quét ForexFactory + EMA50/200 miễn phí, không tốn chi phí API/AI
              </span>
            </div>

            <button
              onClick={handleTriggerDailyCron}
              disabled={isTriggeringCron}
              className="text-xs font-semibold px-3 py-1.5 rounded-md bg-amber-500 hover:bg-amber-400 text-zinc-950 transition flex items-center gap-1.5 cursor-pointer disabled:opacity-50 shrink-0"
            >
              <RefreshCw className={`w-3.5 h-3.5 ${isTriggeringCron ? 'animate-spin' : ''}`} />
              <span>{isTriggeringCron ? 'Đang phân tích vĩ mô...' : 'Chạy Quét Thủ Công (07:00 AM Run)'}</span>
            </button>
          </div>

          {cronSuccessMsg && (
            <div className="p-3 rounded-lg bg-emerald-500/10 border border-emerald-500/30 text-xs text-emerald-400 font-medium flex items-center gap-2">
              <Check className="w-4 h-4 shrink-0" />
              <span>{cronSuccessMsg}</span>
            </div>
          )}

          {/* Daily Macro Report Card */}
          {dailyReport && (
            <div className="grid grid-cols-1 lg:grid-cols-12 gap-5">
              {/* Left Column: Bias Badge & Range (5 cols) */}
              <div className="lg:col-span-5 bg-zinc-900/90 border border-zinc-800 rounded-xl p-4 flex flex-col justify-between gap-4">
                <div>
                  <div className="flex items-center justify-between mb-3">
                    <span className="text-[11px] font-semibold text-zinc-400 uppercase tracking-wider">
                      Định Hướng Vĩ Mô Trong Ngày
                    </span>
                    <span className="text-xs font-mono text-zinc-500">
                      Cập nhật: {new Date(dailyReport.updated_at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}
                    </span>
                  </div>

                  {/* Big Bias Badge */}
                  <div className="flex items-center gap-3 p-3 rounded-lg bg-zinc-950 border border-zinc-800">
                    <div
                      className={`w-12 h-12 rounded-xl flex items-center justify-center font-bold text-xl ${
                        dailyReport.macro_bias === 'BULLISH'
                          ? 'bg-emerald-500/20 text-emerald-400 border border-emerald-500/40'
                          : dailyReport.macro_bias === 'BEARISH'
                          ? 'bg-rose-500/20 text-rose-400 border border-rose-500/40'
                          : 'bg-amber-500/20 text-amber-400 border border-amber-500/40'
                      }`}
                    >
                      {dailyReport.macro_bias === 'BULLISH' ? (
                        <TrendingUp className="w-6 h-6" />
                      ) : (
                        <TrendingDown className="w-6 h-6" />
                      )}
                    </div>

                    <div>
                      <div className="flex items-center gap-2">
                        <span className="text-lg font-bold text-white font-mono tracking-tight">
                          {dailyReport.macro_bias}
                        </span>
                        <span className="text-xs px-2 py-0.5 rounded bg-zinc-800 text-zinc-300 font-mono">
                          Độ tin cậy: {dailyReport.confidence_score}%
                        </span>
                      </div>
                      <p className="text-xs text-zinc-400 mt-0.5">
                        {dailyReport.macro_bias === 'BULLISH'
                          ? 'Chỉ cho phép lệnh BUY, tự động loại bỏ mọi tín hiệu SELL ngược dòng.'
                          : 'Chỉ cho phép lệnh SELL, tự động loại bỏ mọi tín hiệu BUY ngược dòng.'}
                      </p>
                    </div>
                  </div>
                </div>

                {/* Range Forecast */}
                <div className="p-3 rounded-lg bg-zinc-950/60 border border-zinc-800/80 space-y-1.5 text-xs">
                  <div className="flex justify-between items-center text-zinc-400">
                    <span>Biên độ giá dự báo trong ngày:</span>
                    <span className="font-mono font-bold text-amber-400 text-sm">
                      {dailyReport.gold_range_forecast}
                    </span>
                  </div>
                  <div className="flex justify-between items-center text-zinc-400 pt-1 border-t border-zinc-800/60">
                    <span>Kill Switch tự động:</span>
                    <span className="font-mono font-semibold text-emerald-400 flex items-center gap-1">
                      <ShieldCheck className="w-3.5 h-3.5" />
                      Đang canh gác 30p trước/sau tin đỏ
                    </span>
                  </div>
                </div>
              </div>

              {/* Right Column: Macro Rationale & Recommended Action (7 cols) */}
              <div className="lg:col-span-7 bg-zinc-900/90 border border-zinc-800 rounded-xl p-4 flex flex-col justify-between gap-4">
                <div>
                  <h4 className="text-xs font-semibold text-zinc-300 uppercase tracking-wider mb-2 flex items-center gap-2">
                    <Sparkles className="w-4 h-4 text-amber-400" />
                    <span>Nhận Định Dòng Tiền & Vĩ Mô Từ EMA 50/200 (Miễn phí 100%)</span>
                  </h4>
                  <p className="text-xs text-zinc-300 leading-relaxed bg-zinc-950/70 p-3 rounded-lg border border-zinc-800/80">
                    "{dailyReport.daily_rationale}"
                  </p>
                </div>

                <div className="space-y-2">
                  <div className="p-3 rounded-lg bg-amber-500/10 border border-amber-500/20 text-xs text-amber-300">
                    <strong className="block text-amber-400 mb-0.5">Chiến Lược Giao Dịch Đề Xuất Hôm Nay:</strong>
                    {dailyReport.recommended_action}
                  </div>

                  <div className="flex flex-wrap items-center gap-2 text-[11px] text-zinc-400">
                    <span className="text-zinc-500">Tin tức đỏ cần né hôm nay:</span>
                    {dailyReport.high_impact_events_today.map((ev, i) => (
                      <span key={i} className="px-2 py-0.5 rounded bg-zinc-800 text-rose-300 border border-rose-500/20 font-mono">
                        {ev}
                      </span>
                    ))}
                  </div>
                </div>
              </div>
            </div>
          )}
        </div>
      )}

      {/* TAB 2: TRADINGVIEW WEBHOOK ENDPOINT & SIMULATOR */}
      {activeTab === 'webhook' && (
        <div className="mt-5 space-y-5 animate-fadeIn">
          {/* Webhook URL Endpoint Box */}
          <div className="p-4 rounded-xl bg-zinc-900 border border-zinc-800 space-y-3">
            <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-2">
              <div>
                <span className="text-xs font-semibold text-zinc-200 flex items-center gap-1.5">
                  <Terminal className="w-4 h-4 text-amber-400" />
                  Axum Webhook URL tiếp nhận cảnh báo (TradingView Webhook URL)
                </span>
                <p className="text-[11px] text-zinc-400 mt-0.5">
                  Dán URL này vào mục Webhook URL trong cửa sổ tạo Alert của TradingView.
                </p>
              </div>
              <button
                onClick={() => {
                  navigator.clipboard.writeText(webhookUrl);
                  setCopiedUrl(true);
                  setTimeout(() => setCopiedUrl(false), 2000);
                }}
                className="text-xs font-mono px-3 py-1.5 rounded-lg bg-zinc-800 hover:bg-zinc-700 text-amber-400 border border-zinc-700 flex items-center gap-1.5 transition self-start sm:self-auto cursor-pointer"
              >
                {copiedUrl ? <Check className="w-3.5 h-3.5" /> : <Copy className="w-3.5 h-3.5" />}
                <span>{copiedUrl ? 'Đã Copy URL' : 'Copy Webhook URL'}</span>
              </button>
            </div>

            <div className="p-2.5 rounded-lg bg-zinc-950 border border-zinc-800/80 font-mono text-xs text-amber-300 break-all select-all">
              {webhookUrl}
            </div>
          </div>

          {/* Interactive Webhook Simulator & Pine Script Preview Grid */}
          <div className="grid grid-cols-1 lg:grid-cols-12 gap-5">
            {/* Left: Webhook Simulator Form (7 cols) */}
            <div className="lg:col-span-7 bg-zinc-900 border border-zinc-800 rounded-xl p-4 space-y-4">
              <div className="flex items-center justify-between border-b border-zinc-800 pb-2">
                <span className="text-xs font-bold text-zinc-200 uppercase tracking-wide flex items-center gap-1.5">
                  <Send className="w-3.5 h-3.5 text-amber-400" />
                  Trình Giả Lập Bắn Webhook TradingView (Test Sandbox)
                </span>
                <span className="text-[11px] text-zinc-500 font-mono">POST /api/webhook</span>
              </div>

              <div className="grid grid-cols-1 sm:grid-cols-3 gap-3">
                {/* Action Type */}
                <div>
                  <label className="text-[11px] font-semibold text-zinc-400 block mb-1">Hành động (Action)</label>
                  <div className="grid grid-cols-3 gap-1">
                    {(['BUY', 'SELL', 'CANDLE'] as const).map((act) => (
                      <button
                        key={act}
                        type="button"
                        onClick={() => setWebhookAction(act)}
                        className={`py-1.5 rounded text-xs font-mono font-bold transition ${
                          webhookAction === act
                            ? act === 'BUY'
                              ? 'bg-emerald-500 text-zinc-950'
                              : act === 'SELL'
                              ? 'bg-rose-500 text-zinc-950'
                              : 'bg-sky-500 text-zinc-950'
                            : 'bg-zinc-800 text-zinc-400 hover:text-zinc-200'
                        }`}
                      >
                        {act}
                      </button>
                    ))}
                  </div>
                </div>

                {/* Price */}
                <div>
                  <label className="text-[11px] font-semibold text-zinc-400 block mb-1">Mức giá ($)</label>
                  <input
                    type="number"
                    step="0.5"
                    value={webhookPrice}
                    onChange={(e) => setWebhookPrice(Number(e.target.value) || 2684.50)}
                    className="w-full bg-zinc-950 border border-zinc-700 rounded px-2.5 py-1.5 text-xs font-mono text-zinc-100 outline-none focus:border-amber-500"
                  />
                </div>

                {/* Comment */}
                <div>
                  <label className="text-[11px] font-semibold text-zinc-400 block mb-1">Cặp giao dịch</label>
                  <input
                    type="text"
                    disabled
                    value="XAUUSD (Gold)"
                    className="w-full bg-zinc-950/60 border border-zinc-800 rounded px-2.5 py-1.5 text-xs font-mono text-zinc-400"
                  />
                </div>
              </div>

              <div>
                <label className="text-[11px] font-semibold text-zinc-400 block mb-1">Ghi chú phân tích (Comment)</label>
                <input
                  type="text"
                  value={webhookComment}
                  onChange={(e) => setWebhookComment(e.target.value)}
                  className="w-full bg-zinc-950 border border-zinc-700 rounded px-2.5 py-1.5 text-xs text-zinc-100 outline-none focus:border-amber-500"
                  placeholder="Mô tả lý do nến rút chân hoặc chạm vùng Order Block"
                />
              </div>

              <div className="flex items-center justify-between pt-1">
                <span className="text-[11px] text-zinc-500">
                  Hệ thống sẽ kiểm tra Kill Switch & tính Lot trước khi phát cảnh báo.
                </span>
                <button
                  onClick={handleSendWebhook}
                  disabled={isSendingWebhook}
                  className="px-4 py-2 rounded-lg bg-amber-500 hover:bg-amber-400 text-zinc-950 text-xs font-bold transition flex items-center gap-1.5 cursor-pointer disabled:opacity-50"
                >
                  <Send className={`w-3.5 h-3.5 ${isSendingWebhook ? 'animate-pulse' : ''}`} />
                  <span>{isSendingWebhook ? 'Đang gửi...' : 'Gửi Webhook Test'}</span>
                </button>
              </div>

              {/* Webhook Execution Result Log */}
              {webhookResponse && (
                <div
                  className={`p-3 rounded-lg border text-xs font-mono space-y-1.5 animate-fadeIn ${
                    webhookResponse.status === 'signal_executed'
                      ? 'bg-emerald-500/10 border-emerald-500/30 text-emerald-300'
                      : webhookResponse.status === 'blocked_by_kill_switch'
                      ? 'bg-rose-500/10 border-rose-500/30 text-rose-300'
                      : 'bg-amber-500/10 border-amber-500/30 text-amber-300'
                  }`}
                >
                  <div className="font-bold flex items-center gap-1.5">
                    {webhookResponse.status === 'signal_executed' ? (
                      <ShieldCheck className="w-4 h-4 text-emerald-400" />
                    ) : (
                      <ShieldAlert className="w-4 h-4 text-rose-400" />
                    )}
                    <span>Trạng thái: {webhookResponse.status}</span>
                  </div>
                  <p className="text-zinc-300 text-[11px] font-sans">
                    {webhookResponse.reason || webhookResponse.message}
                  </p>
                  {webhookResponse.signal && (
                    <div className="pt-1.5 border-t border-zinc-800 text-[11px] space-y-0.5">
                      <div>• Entry: ${webhookResponse.signal.entry_price} | SL: ${webhookResponse.signal.stop_loss} | TP1: ${webhookResponse.signal.take_profit_1}</div>
                      <div>• Lot đề xuất: <strong className="text-amber-400">{webhookResponse.signal.recommended_lot_size} Lot</strong> | Rủi ro: -${webhookResponse.signal.risk_amount_usd} | Lợi nhuận TP1: +${webhookResponse.signal.potential_profit_tp1_usd}</div>
                    </div>
                  )}
                </div>
              )}
            </div>

            {/* Right: Pine Script & JSON Alert Template (5 cols) */}
            <div className="lg:col-span-5 bg-zinc-900 border border-zinc-800 rounded-xl p-4 flex flex-col justify-between gap-3">
              <div>
                <div className="flex items-center justify-between mb-2">
                  <span className="text-xs font-bold text-zinc-300 uppercase tracking-wide flex items-center gap-1.5">
                    <Code2 className="w-3.5 h-3.5 text-amber-400" />
                    Pine Script Alert Code
                  </span>
                  <button
                    onClick={() => {
                      navigator.clipboard.writeText(pineScriptSample);
                      setCopiedPine(true);
                      setTimeout(() => setCopiedPine(false), 2000);
                    }}
                    className="text-[11px] text-zinc-400 hover:text-zinc-200 flex items-center gap-1 cursor-pointer"
                  >
                    {copiedPine ? <Check className="w-3 h-3 text-emerald-400" /> : <Copy className="w-3 h-3" />}
                    <span>{copiedPine ? 'Đã copy' : 'Copy'}</span>
                  </button>
                </div>
                <pre className="p-3 rounded-lg bg-zinc-950 border border-zinc-800 text-[11px] font-mono text-zinc-300 overflow-x-auto leading-relaxed max-h-[220px]">
                  {pineScriptSample}
                </pre>
              </div>

              <div className="p-3 rounded-lg bg-zinc-950/80 border border-zinc-800 text-[11px] text-zinc-400 space-y-1">
                <span className="text-amber-400 font-semibold block">Quy trình xử lý an toàn:</span>
                <p>1. Nhận Alert từ TradingView qua POST HTTP.</p>
                <p>2. Kiểm tra Kill Switch với cửa sổ ±30 phút tin đỏ CPI/NFP.</p>
                <p>3. Kiểm tra tính đồng thuận với EMA50/200 Trend Bias hôm nay.</p>
                <p>4. Tính Lot Size theo 1% vốn và bắn thẳng tới Telegram.</p>
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  );
};
