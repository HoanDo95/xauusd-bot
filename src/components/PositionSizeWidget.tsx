'use client';

import React, { useState, useEffect } from 'react';
import { Calculator, ShieldAlert, TrendingUp, DollarSign, Scale, ArrowRight, Check, HelpCircle } from 'lucide-react';
import { BotConfig } from '../types';

interface PositionSizeWidgetProps {
  config: BotConfig;
  currentGoldPrice: number;
  onUpdateConfig: (newConfig: Partial<BotConfig>) => void;
}

export const PositionSizeWidget: React.FC<PositionSizeWidgetProps> = ({
  config,
  currentGoldPrice,
  onUpdateConfig,
}) => {
  const [balance, setBalance] = useState<number>(config.account_balance || 10000);
  const [riskPercent, setRiskPercent] = useState<number>(config.risk_percentage || 1.0);
  const [slPoints, setSlPoints] = useState<number>(config.gold_sl_buffer_points || 8.5);
  const [synced, setSynced] = useState<boolean>(false);
  const [showFormula, setShowFormula] = useState<boolean>(false);

  // Sync state if props change from outside
  useEffect(() => {
    if (config.account_balance && config.account_balance !== balance) {
      setBalance(config.account_balance);
    }
    if (config.risk_percentage && config.risk_percentage !== riskPercent) {
      setRiskPercent(config.risk_percentage);
    }
    if (
      typeof config.gold_sl_buffer_points === 'number' &&
      config.gold_sl_buffer_points !== slPoints
    ) {
      setSlPoints(config.gold_sl_buffer_points);
    }
  }, [config.account_balance, config.risk_percentage, config.gold_sl_buffer_points]);

  // Mathematics corresponding to Rust risk_manager.rs PositionSizer
  // Contract size: 1.0 Lot = 100 Ounces
  // 1.0$ move on 1.0 lot = $100 USD PnL
  const OUNCES_PER_LOT = 100.0;
  const clampedBalance = Math.max(100, balance);
  const clampedRisk = Math.max(0.1, Math.min(5.0, riskPercent));
  const clampedSl = Math.max(1.0, slPoints);

  const allowedRiskUsd = (clampedBalance * (clampedRisk / 100));
  const lossPerLot = clampedSl * OUNCES_PER_LOT;
  const rawLot = allowedRiskUsd / lossPerLot;
  const lotSize = Math.max(0.01, Math.min(50.0, Math.floor(rawLot * 100) / 100));

  const actualRiskUsd = Math.round(lotSize * lossPerLot * 100) / 100;
  const rewardTp1Pts = clampedSl * (config.min_risk_reward || 2.0);
  const potentialProfitTp1Usd = Math.round(lotSize * rewardTp1Pts * OUNCES_PER_LOT * 100) / 100;
  const potentialProfitTp2Usd = Math.round(lotSize * (clampedSl * 3.0) * OUNCES_PER_LOT * 100) / 100;

  const leverage = config.account_leverage || 100;
  const price = currentGoldPrice > 1000 ? currentGoldPrice : 2684.50;
  const marginUsd = Math.round(((lotSize * OUNCES_PER_LOT * price) / leverage) * 100) / 100;

  const handleSyncToBot = () => {
    onUpdateConfig({
      account_balance: clampedBalance,
      risk_percentage: clampedRisk,
    });
    setSynced(true);
    setTimeout(() => setSynced(false), 2200);
  };

  const balancePresets = [5000, 10000, 25000, 50000, 100000];

  return (
    <div className="bg-[#12161f] border border-zinc-800 rounded-xl p-5 shadow-lg relative overflow-hidden">
      {/* Background ambient decorative glow */}
      <div className="absolute top-0 right-0 w-64 h-64 bg-amber-500/5 rounded-full blur-3xl pointer-events-none" />

      {/* Header */}
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 pb-4 border-b border-zinc-800/80">
        <div className="flex items-center gap-3">
          <div className="w-9 h-9 rounded-lg bg-amber-500/10 border border-amber-500/30 flex items-center justify-center text-amber-400">
            <Calculator className="w-5 h-5" />
          </div>
          <div>
            <div className="flex items-center gap-2">
              <h3 className="text-sm font-semibold text-zinc-100 uppercase tracking-wide">
                Quản Lý Vốn & Tính Khối Lượng Tự Động (Position Sizing)
              </h3>
              <span className="text-[10px] px-2 py-0.5 rounded bg-zinc-800 text-amber-400 border border-amber-500/20 font-mono">
                Rust Engine Core
              </span>
            </div>
            <p className="text-xs text-zinc-400">
              Dynamic position sizing algorithm (100 oz/lot standard contract, automated risk allocation).
              Tăng SL buffer lên 15–20$ sẽ tự động giảm Lot để giữ nguyên rủi ro %.
            </p>
          </div>
        </div>

        <div className="flex items-center gap-2">
          <button
            onClick={() => setShowFormula(!showFormula)}
            className="text-xs text-zinc-400 hover:text-zinc-200 flex items-center gap-1 px-2.5 py-1.5 rounded bg-zinc-800/60 border border-zinc-700/60 transition"
            title="Xem công thức toán học Rust"
          >
            <HelpCircle className="w-3.5 h-3.5 text-amber-400" />
            <span>Công thức tính</span>
          </button>
          <button
            onClick={handleSyncToBot}
            className={`text-xs font-medium flex items-center gap-1.5 px-3 py-1.5 rounded transition ${
              synced
                ? 'bg-emerald-500/20 border border-emerald-500 text-emerald-400'
                : 'bg-amber-500 hover:bg-amber-400 text-zinc-950 font-semibold shadow-sm'
            }`}
          >
            {synced ? (
              <>
                <Check className="w-3.5 h-3.5" />
                <span>Đã lưu vào Bot!</span>
              </>
            ) : (
              <>
                <ArrowRight className="w-3.5 h-3.5" />
                <span>Đồng bộ cấu hình Bot</span>
              </>
            )}
          </button>
        </div>
      </div>

      {/* Formula Explanation Drawer */}
      {showFormula && (
        <div className="mt-4 p-3.5 rounded-lg bg-zinc-900/90 border border-zinc-800 text-xs text-zinc-300 font-mono space-y-1.5 animate-fadeIn">
          <div className="text-amber-400 font-semibold flex items-center gap-1.5">
            <Scale className="w-3.5 h-3.5" />
            <span>Công thức chuẩn hóa trong rust-backend/src/risk_manager.rs:</span>
          </div>
          <p>1. <span className="text-zinc-400">Risk USD</span> = Số dư ({clampedBalance.toLocaleString()}$) × ({clampedRisk}% / 100) = <span className="text-amber-300">${allowedRiskUsd.toFixed(2)}</span></p>
          <p>2. <span className="text-zinc-400">1.0 Lot XAU/USD</span> = 100 Ounces vàng → Mỗi $1 giá chạy = $100 PnL</p>
          <p>3. <span className="text-zinc-400">Lot Khuyến Nghị</span> = Risk USD / (SL Distance × 100) = {allowedRiskUsd.toFixed(2)} / ({clampedSl.toFixed(1)} × 100) = <span className="text-emerald-400 font-bold">{lotSize} Lot</span> (làm tròn xuống 0.01 để bảo vệ vốn tuyệt đối)</p>
        </div>
      )}

      {/* Interactive Controls & Output Columns */}
      <div className="grid grid-cols-1 lg:grid-cols-12 gap-5 mt-4">
        {/* Left: Input Parameters (7 cols) */}
        <div className="lg:col-span-7 space-y-4">
          {/* 1. Account Balance */}
          <div>
            <div className="flex items-center justify-between mb-1.5">
              <label className="text-xs font-medium text-zinc-300 flex items-center gap-1.5">
                <DollarSign className="w-3.5 h-3.5 text-zinc-400" />
                <span>Tổng số dư tài khoản giao dịch (Account Balance)</span>
              </label>
              <span className="text-xs font-mono font-semibold text-amber-400">
                ${clampedBalance.toLocaleString()} USD
              </span>
            </div>
            <div className="relative">
              <span className="absolute left-3 top-1/2 -translate-y-1/2 text-zinc-500 font-mono text-sm">$</span>
              <input
                type="number"
                min="100"
                step="500"
                value={balance}
                onChange={(e) => setBalance(Number(e.target.value) || 0)}
                className="w-full pl-7 pr-3 py-2 bg-zinc-900 border border-zinc-700 rounded-lg text-sm font-mono text-zinc-100 focus:outline-none focus:border-amber-500 transition"
                placeholder="10000"
              />
            </div>
            {/* Quick Presets */}
            <div className="flex flex-wrap items-center gap-1.5 mt-2">
              <span className="text-[11px] text-zinc-500 mr-1">Mẫu nhanh:</span>
              {balancePresets.map((p) => (
                <button
                  key={p}
                  type="button"
                  onClick={() => setBalance(p)}
                  className={`text-[11px] px-2 py-0.5 rounded font-mono transition ${
                    balance === p
                      ? 'bg-amber-500/20 text-amber-400 border border-amber-500/40'
                      : 'bg-zinc-800/80 text-zinc-400 hover:text-zinc-200 border border-zinc-700/50'
                  }`}
                >
                  ${(p / 1000).toFixed(0)}k
                </button>
              ))}
            </div>
          </div>

          {/* 2. Risk Percentage */}
          <div>
            <div className="flex items-center justify-between mb-1.5">
              <label className="text-xs font-medium text-zinc-300 flex items-center gap-1.5">
                <ShieldAlert className="w-3.5 h-3.5 text-zinc-400" />
                <span>Tỷ lệ rủi ro cho phép (% Risk / Lệnh)</span>
              </label>
              <span className="text-xs font-mono font-semibold text-zinc-100">
                {clampedRisk.toFixed(1)}% = <span className="text-amber-400">${allowedRiskUsd.toFixed(2)} USD</span>
              </span>
            </div>
            <div className="flex items-center gap-3">
              <input
                type="range"
                min="0.2"
                max="3.0"
                step="0.1"
                value={riskPercent}
                onChange={(e) => setRiskPercent(Number(e.target.value))}
                className="flex-1 accent-amber-500 h-1.5 bg-zinc-800 rounded cursor-pointer"
              />
              <div className="flex gap-1">
                {[0.5, 1.0, 2.0].map((r) => (
                  <button
                    key={r}
                    type="button"
                    onClick={() => setRiskPercent(r)}
                    className={`text-[11px] px-2 py-1 rounded font-mono transition ${
                      riskPercent === r
                        ? 'bg-amber-500 text-zinc-950 font-bold'
                        : 'bg-zinc-800 text-zinc-400 hover:text-zinc-200 border border-zinc-700/50'
                    }`}
                  >
                    {r}%
                  </button>
                ))}
              </div>
            </div>
          </div>

          {/* 3. Stop Loss Distance */}
          <div>
            <div className="flex items-center justify-between mb-1.5">
              <label className="text-xs font-medium text-zinc-300 flex items-center gap-1.5">
                <TrendingUp className="w-3.5 h-3.5 text-zinc-400" />
                <span>Khoảng cách Stop Loss mặc định (SL Distance)</span>
              </label>
              <span className="text-xs font-mono text-zinc-400">
                {clampedSl.toFixed(1)}$ ({Math.round(clampedSl * 10)} pips)
              </span>
            </div>
            <div className="grid grid-cols-4 gap-2">
              {[8.0, 10.0, 15.0, 20.0].map((sl) => (
                <button
                  key={sl}
                  type="button"
                  onClick={() => setSlPoints(sl)}
                  className={`py-1.5 px-2 rounded text-xs font-mono text-center border transition ${
                    slPoints === sl
                      ? 'bg-amber-500/10 border-amber-500/50 text-amber-300 font-semibold'
                      : 'bg-zinc-900 border-zinc-800 text-zinc-400 hover:border-zinc-700'
                  }`}
                >
                  {sl}$ SL
                </button>
              ))}
            </div>
          </div>
        </div>

        {/* Right: Calculated Lot Size & Risk Engine Card (5 cols) */}
        <div className="lg:col-span-5 flex flex-col justify-between bg-zinc-900/80 border border-zinc-800 rounded-lg p-4">
          <div>
            <div className="text-[11px] font-semibold text-zinc-400 uppercase tracking-wider mb-1">
              Khối lượng lệnh khuyến nghị (Lot Size)
            </div>
            <div className="flex items-baseline gap-2 mb-3">
              <span className="text-3xl font-extrabold font-mono text-amber-400 tracking-tight">
                {lotSize.toFixed(2)}
              </span>
              <span className="text-sm font-semibold text-zinc-400">Standard Lot</span>
              <span className="ml-auto text-[11px] text-zinc-400 font-mono">
                (≈ {(lotSize * 100).toFixed(0)} oz vàng)
              </span>
            </div>

            {/* Risk Breakdown Table */}
            <div className="space-y-2 text-xs border-t border-zinc-800/80 pt-3">
              <div className="flex justify-between items-center">
                <span className="text-zinc-400">Rủi ro tối đa nếu dính SL:</span>
                <span className="font-mono font-semibold text-rose-400">
                  -${actualRiskUsd.toFixed(2)} ({((actualRiskUsd / clampedBalance) * 100).toFixed(2)}%)
                </span>
              </div>
              <div className="flex justify-between items-center">
                <span className="text-zinc-400">Lợi nhuận mục tiêu TP1 (1:2):</span>
                <span className="font-mono font-semibold text-emerald-400">
                  +${potentialProfitTp1Usd.toFixed(2)}
                </span>
              </div>
              <div className="flex justify-between items-center">
                <span className="text-zinc-400">Lợi nhuận kỳ vọng TP2 (1:3):</span>
                <span className="font-mono font-semibold text-emerald-300">
                  +${potentialProfitTp2Usd.toFixed(2)}
                </span>
              </div>
              <div className="flex justify-between items-center pt-1 border-t border-zinc-800/60">
                <span className="text-zinc-500">Ký quỹ ước tính (Margin 1:100):</span>
                <span className="font-mono text-zinc-300">
                  ${marginUsd.toFixed(2)}
                </span>
              </div>
            </div>
          </div>

          {/* Assessment Badge */}
          <div className="mt-4 pt-3 border-t border-zinc-800/80">
            <div
              className={`text-[11px] p-2 rounded flex items-center gap-2 ${
                clampedRisk <= 1.0
                  ? 'bg-emerald-500/10 border border-emerald-500/20 text-emerald-400'
                  : clampedRisk <= 2.0
                  ? 'bg-amber-500/10 border border-amber-500/20 text-amber-300'
                  : 'bg-rose-500/10 border border-rose-500/20 text-rose-400'
              }`}
            >
              <ShieldAlert className="w-3.5 h-3.5 shrink-0" />
              <span>
                {clampedRisk <= 1.0
                  ? 'Kỷ luật cao: Đạt tiêu chuẩn cấp vốn quỹ Prop Firm (FTMO/MFF 1%)'
                  : clampedRisk <= 2.0
                  ? 'Mức rủi ro trung bình: Phù hợp tài khoản cá nhân có kinh nghiệm'
                  : 'Cảnh báo: Rủi ro > 2% có thể gây sụt giảm tài khoản lớn (Drawdown)'}
              </span>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
};
