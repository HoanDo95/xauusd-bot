'use client';

import React from 'react';
import { BotConfig } from '../types';

export type DashboardTab = 'monitor' | 'signals' | 'webhook' | 'config';

interface Props {
  currentTab: DashboardTab;
  onTabChange: (tab: DashboardTab) => void;
  goldPrice: number;
  config: BotConfig;
  signalsToday: number;
  dataSource?: string;
  onOpenTestModal: () => void;
}

function feedLabel(dataSource?: string): string {
  if (dataSource === 'TWELVEDATA') return 'TwelveData XAU/USD (Free)';
  if (dataSource === 'FALLBACK_SEED' || dataSource === 'OFFLINE') {
    return 'Offline Fallback (Seed)';
  }
  return 'Binance PAXG/USDT (Free 24/7)';
}

export const Header: React.FC<Props> = ({
  currentTab,
  onTabChange,
  goldPrice,
  config,
  signalsToday,
  dataSource,
  onOpenTestModal,
}) => {
  const quotaEnabled = config.daily_quota_enabled ?? false;
  const maxSignals = Math.max(1, config.max_signals_per_day || 1);
  const quotaReached = quotaEnabled && signalsToday >= maxSignals;

  const tabs: Array<{ id: DashboardTab; label: string }> = [
    { id: 'monitor', label: 'Live Trading Desk' },
    { id: 'signals', label: 'Signal Alerts' },
    { id: 'webhook', label: 'News & Webhook' },
    { id: 'config', label: 'Sniper Config' },
  ];

  return (
    <header className="sticky top-0 z-40 flex flex-wrap items-center justify-between gap-x-6 gap-y-2 px-4 py-3 md:px-6 border-b border-neutral-800 bg-neutral-950/90 backdrop-blur">
      {/* Zone 1: Brand Wordmark */}
      <div className="flex items-center gap-3 whitespace-nowrap shrink-0">
        <div className="w-8 h-8 rounded-lg bg-amber-500/10 border border-amber-500/30 flex items-center justify-center text-amber-400 font-bold text-sm shadow-sm">
          Au
        </div>
        <div className="flex flex-col">
          <span className="text-sm font-bold tracking-tight text-white uppercase">
            Gold Sniper Bot
          </span>
          <span className="text-[10px] text-neutral-500 font-mono">
            {quotaEnabled ? `${maxSignals} Signals / Day` : 'Unlimited Signals'} · XAU/USD
          </span>
        </div>
      </div>

      {/* Zone 2: Navigation Links / View Tabs */}
      <nav className="order-3 lg:order-none w-full lg:w-auto flex items-center justify-center gap-2 md:gap-6 text-sm font-medium text-neutral-400">
        {tabs.map((tab) => (
          <button
            key={tab.id}
            onClick={() => onTabChange(tab.id)}
            className={`px-2 py-1 hover:text-white transition-colors whitespace-nowrap shrink-0 cursor-pointer rounded-md ${
              currentTab === tab.id ? 'text-amber-400 font-semibold bg-amber-500/5' : ''
            }`}
          >
            {tab.label}
          </button>
        ))}
      </nav>

      {/* Zone 3: Sniper Quota, Free Feed Badge, Ticker & Action */}
      <div className="flex flex-wrap items-center justify-end gap-2 shrink-0">
        <div
          className={`flex items-center gap-1.5 px-2.5 py-1 rounded-lg border text-[11px] font-mono font-semibold whitespace-nowrap ${
            quotaReached
              ? 'bg-amber-500/10 text-amber-300 border-amber-500/30'
              : 'bg-emerald-500/10 text-emerald-400 border-emerald-500/30'
          }`}
          title={
            quotaEnabled
              ? 'Số lệnh THẬT đã phát hôm nay / giới hạn đã bật. Lệnh simulation không tính quota.'
              : 'Không giới hạn số lệnh/ngày. Dedup theo nến vẫn chặn alert trùng.'
          }
        >
          <span>
            {quotaEnabled
              ? `🎯 Quota Hôm Nay: ${signalsToday}/${maxSignals} Lệnh`
              : '🎯 Quota: Không giới hạn'}
          </span>
          {quotaEnabled && (
            <span className={quotaReached ? 'text-amber-400' : 'text-emerald-500'}>
              {quotaReached ? '· Quota Reached' : '· Ready'}
            </span>
          )}
        </div>

        <div
          className="hidden xl:flex items-center gap-1.5 px-2.5 py-1 rounded-lg border border-sky-500/30 bg-sky-500/10 text-sky-300 text-[11px] font-mono whitespace-nowrap"
          title="Nguồn dữ liệu nến Vàng miễn phí đang hoạt động"
        >
          📡 Live Feed: {feedLabel(dataSource ?? config.data_source)}
        </div>

        <div className="hidden sm:flex items-baseline gap-1.5 px-3 py-1 bg-neutral-900 border border-neutral-800 rounded-lg text-xs font-mono">
          <span className="text-neutral-500">XAU/USD:</span>
          <span className="text-amber-400 font-semibold tabular-nums">
            ${goldPrice.toFixed(2)}
          </span>
        </div>

        <button
          onClick={onOpenTestModal}
          className="px-3.5 py-1.5 text-xs font-semibold text-neutral-950 bg-amber-400 hover:bg-amber-300 rounded-lg transition-colors whitespace-nowrap shrink-0 cursor-pointer shadow-sm shadow-amber-500/20"
        >
          Test Alert
        </button>
      </div>
    </header>
  );
};
