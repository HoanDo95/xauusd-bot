'use client';

import { useEffect, useState } from 'react';

type ScalpHealth = {
  status?: string;
  profile?: string;
  timeframe?: string;
  feedOk?: boolean;
  configGuardOk?: boolean;
  configWarnings?: Array<{ code?: string; severity?: string; message?: string }>;
  telegramConfigured?: boolean;
  signalsToday?: number;
  filledToday?: number;
  maxSignalsPerDay?: number;
  dailyQuotaReached?: boolean;
  configHash?: string;
  livePrice?: number | null;
  lastScanAgeSecs?: number | null;
};

type ScalpConfig = {
  account_balance?: number;
  risk_percentage?: number;
  max_daily_risk_percent?: number;
  max_daily_loss_r?: number;
  max_signals_per_day?: number;
  daily_quota_enabled?: boolean;
  session_filter_enabled?: boolean;
  ai_signal_review_min_confidence?: number;
  timeframe?: string;
};

async function fetchJson<T>(path: string): Promise<T> {
  const response = await fetch(path, { cache: 'no-store' });
  if (!response.ok) {
    throw new Error(`${path} → HTTP ${response.status}`);
  }
  return (await response.json()) as T;
}

export default function ScalpPage() {
  const [health, setHealth] = useState<ScalpHealth | null>(null);
  const [config, setConfig] = useState<ScalpConfig | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    const load = async () => {
      try {
        const [h, c] = await Promise.all([
          fetchJson<ScalpHealth>('/scalp-api/api/health'),
          fetchJson<ScalpConfig>('/scalp-api/api/config'),
        ]);
        if (!cancelled) {
          setHealth(h);
          setConfig(c);
          setError(null);
        }
      } catch (err) {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : 'Không tải được scalp service');
        }
      }
    };
    void load();
    const timer = window.setInterval(load, 15000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, []);

  const warnings = health?.configWarnings ?? [];

  return (
    <main className="min-h-screen bg-neutral-950 text-neutral-100 p-4 md:p-8">
      <div className="mx-auto max-w-5xl flex flex-col gap-5">
        <header className="flex flex-col gap-2 border-b border-neutral-800 pb-4">
          <div className="flex items-center justify-between gap-3">
            <div>
              <div className="text-[11px] uppercase tracking-[0.2em] text-teal-300">
                XAU-USD Scalp
              </div>
              <h1 className="text-2xl md:text-3xl font-bold">Scalp Research Service</h1>
            </div>
            <a
              href="/"
              className="rounded-lg border border-neutral-700 px-3 py-2 text-xs text-neutral-300 hover:border-teal-400 hover:text-teal-200"
            >
              ← Swing dashboard
            </a>
          </div>
          <p className="text-sm text-neutral-400 max-w-3xl">
            Service scalp riêng, alert-only. ICT/OB short-swing strategy đang ở giai đoạn
            research/backtest; chưa gửi alert live. Không auto-trade, không dùng quota
            GOLD SIGNAL của bot swing.
          </p>
        </header>

        {error && (
          <div className="rounded-xl border border-rose-500/40 bg-rose-500/10 p-4 text-sm text-rose-200">
            {error}
          </div>
        )}

        <section className="grid gap-3 md:grid-cols-3">
          <div className="rounded-xl border border-neutral-800 bg-neutral-900/60 p-4">
            <div className="text-[11px] uppercase text-neutral-500">Status</div>
            <div className="mt-1 text-xl font-semibold text-teal-300">
              {health?.status ?? '—'}
            </div>
            <div className="mt-1 text-xs text-neutral-400">
              profile: {health?.profile ?? '—'} · {health?.timeframe ?? '—'}
            </div>
          </div>
          <div className="rounded-xl border border-neutral-800 bg-neutral-900/60 p-4">
            <div className="text-[11px] uppercase text-neutral-500">Feed / Scan</div>
            <div className="mt-1 text-xl font-semibold">
              {health?.feedOk ? 'OK' : 'DEGRADED'}
            </div>
            <div className="mt-1 text-xs text-neutral-400">
              last scan: {health?.lastScanAgeSecs ?? '—'}s
            </div>
          </div>
          <div className="rounded-xl border border-neutral-800 bg-neutral-900/60 p-4">
            <div className="text-[11px] uppercase text-neutral-500">Config guard</div>
            <div
              className={`mt-1 text-xl font-semibold ${
                health?.configGuardOk ? 'text-emerald-300' : 'text-rose-300'
              }`}
            >
              {health?.configGuardOk ? 'OK' : 'ATTENTION'}
            </div>
            <div className="mt-1 text-xs text-neutral-400">
              {warnings.length} warning(s)
            </div>
          </div>
        </section>

        <section className="grid gap-3 md:grid-cols-2">
          <div className="rounded-xl border border-neutral-800 bg-neutral-900/60 p-4">
            <div className="text-xs font-semibold text-neutral-300">Risk / quota</div>
            <dl className="mt-3 grid grid-cols-2 gap-2 text-xs">
              <dt className="text-neutral-500">Account</dt>
              <dd className="text-right font-mono">
                ${config?.account_balance?.toFixed(0) ?? '—'}
              </dd>
              <dt className="text-neutral-500">Risk / lệnh</dt>
              <dd className="text-right font-mono">{config?.risk_percentage ?? '—'}%</dd>
              <dt className="text-neutral-500">Max daily risk</dt>
              <dd className="text-right font-mono">
                {config?.max_daily_risk_percent ?? '—'}%
              </dd>
              <dt className="text-neutral-500">Max daily loss</dt>
              <dd className="text-right font-mono">{config?.max_daily_loss_r ?? '—'}R</dd>
              <dt className="text-neutral-500">Quota</dt>
              <dd className="text-right font-mono">
                {health?.filledToday ?? 0}/{health?.maxSignalsPerDay ?? '—'} fills
              </dd>
              <dt className="text-neutral-500">Telegram</dt>
              <dd className="text-right font-mono">
                {health?.telegramConfigured ? 'configured' : 'missing'}
              </dd>
            </dl>
          </div>

          <div className="rounded-xl border border-neutral-800 bg-neutral-900/60 p-4">
            <div className="text-xs font-semibold text-neutral-300">Research state</div>
            <div className="mt-3 flex flex-col gap-2 text-xs text-neutral-400">
              <div className="flex items-center justify-between">
                <span>ICT/OB strategy</span>
                <span className="rounded-full border border-amber-500/40 bg-amber-500/10 px-2 py-0.5 text-amber-200">
                  research
                </span>
              </div>
              <div className="flex items-center justify-between">
                <span>Live alerts</span>
                <span className="rounded-full border border-neutral-700 px-2 py-0.5">
                  off
                </span>
              </div>
              <div className="flex items-center justify-between">
                <span>Auto-trade</span>
                <span className="rounded-full border border-neutral-700 px-2 py-0.5">
                  off
                </span>
              </div>
              <div className="mt-1 text-[11px] text-neutral-500">
                config hash: {health?.configHash?.slice(0, 12) ?? '—'}…
              </div>
            </div>
          </div>
        </section>

        {warnings.length > 0 && (
          <section className="rounded-xl border border-amber-500/30 bg-amber-500/5 p-4">
            <div className="text-xs font-semibold text-amber-200">Config warnings</div>
            <ul className="mt-2 flex flex-col gap-1 text-xs text-amber-100/80">
              {warnings.map((warning, index) => (
                <li key={`${warning.code ?? 'warning'}-${index}`}>
                  <span className="font-mono text-amber-300">{warning.severity}</span>{' '}
                  {warning.code}: {warning.message}
                </li>
              ))}
            </ul>
          </section>
        )}

        <footer className="text-[11px] text-neutral-500">
          Scalp service is isolated from the swing bot: separate data/state/journal,
          separate Telegram channel, shared dashboard auth. No live alerts until the
          strategy/backtest is accepted.
        </footer>
      </div>
    </main>
  );
}
