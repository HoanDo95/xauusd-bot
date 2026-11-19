'use client';

import { useEffect, useRef, useState } from 'react';

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

type ScalpFilters = {
  htf_filter_enabled: boolean;
  rsi_h1_guard_enabled: boolean;
  min_confidence: number;
  max_alerts_per_day: number;
  cooldown_minutes: number;
};

type ScalpStats = {
  alerts: number;
  open: number;
  tp1_open: number;
  completed: number;
  wins: number;
  win_rate: number;
  avg_r: number;
  total_r: number;
  pf: number | null;
  max_dd: number;
  alerts_today: number;
  alerts_7d: number;
  last_alert_at?: string | null;
  tp2: number;
  tp1_be: number;
  sl: number;
  timeout: number;
  skipped: Record<string, number>;
};

type JournalEvent = {
  event?: string;
  setup_id?: string;
  created_at?: string;
  scenario_id?: string;
  scenario_name?: string;
  direction?: string;
  bias?: string;
  entry?: number;
  stop_loss?: number;
  take_profit_1?: number;
  take_profit_2?: number;
  risk_points?: number;
  close?: number;
  r_multiple?: number;
  bars_open?: number;
  source?: string;
  reason?: string;
  notes?: string;
};

async function fetchJson<T>(path: string): Promise<T> {
  const response = await fetch(path, { cache: 'no-store' });
  if (!response.ok) {
    throw new Error(`${path} → HTTP ${response.status}`);
  }
  return (await response.json()) as T;
}

function formatTime(value?: string): string {
  if (!value) return '—';
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString('vi-VN', { hour12: false });
}

function formatNumber(value?: number, digits = 2): string {
  return typeof value === 'number' && Number.isFinite(value)
    ? value.toFixed(digits)
    : '—';
}

function formatR(value?: number): string {
  return typeof value === 'number' && Number.isFinite(value)
    ? `${value >= 0 ? '+' : ''}${value.toFixed(2)}R`
    : '—';
}

function noteValue(notes: string | undefined, key: string): string | null {
  if (!notes) return null;
  const match = notes.match(new RegExp(`${key}=([^;]+)`));
  return match ? match[1] : null;
}

export default function ScalpPage() {
  const [health, setHealth] = useState<ScalpHealth | null>(null);
  const [config, setConfig] = useState<ScalpConfig | null>(null);
  const [journal, setJournal] = useState<JournalEvent[]>([]);
  const [filters, setFilters] = useState<ScalpFilters | null>(null);
  const [filterForm, setFilterForm] = useState<ScalpFilters | null>(null);
  const [filterMessage, setFilterMessage] = useState<string | null>(null);
  const [stats, setStats] = useState<ScalpStats | null>(null);
  const [error, setError] = useState<string | null>(null);
  const filterDirty = useRef(false);

  useEffect(() => {
    let cancelled = false;
    const load = async () => {
      try {
        const [h, c, j, f, s] = await Promise.all([
          fetchJson<ScalpHealth>('/scalp-api/api/health'),
          fetchJson<ScalpConfig>('/scalp-api/api/config'),
          fetchJson<JournalEvent[]>('/scalp-api/api/trigger-journal?limit=1000'),
          fetchJson<ScalpFilters>('/scalp-api/api/scalp-filters'),
          fetchJson<ScalpStats>('/scalp-api/api/scalp-stats'),
        ]);
        if (!cancelled) {
          setHealth(h);
          setConfig(c);
          setJournal(j);
          setFilters(f);
          setStats(s);
          if (!filterDirty.current) setFilterForm(f);
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

  const updateFilter = (patch: Partial<ScalpFilters>) => {
    filterDirty.current = true;
    setFilterForm((current) => (current ? { ...current, ...patch } : current));
  };

  const saveFilters = async () => {
    if (!filterForm) return;
    setFilterMessage('Saving…');
    try {
      const response = await fetch('/scalp-api/api/scalp-filters', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(filterForm),
      });
      const body = (await response.json()) as {
        status?: string;
        message?: string;
        filters?: ScalpFilters;
      };
      if (!response.ok) {
        throw new Error(body.message ?? `HTTP ${response.status}`);
      }
      const saved = body.filters ?? filterForm;
      setFilters(saved);
      setFilterForm(saved);
      filterDirty.current = false;
      setFilterMessage('Saved');
    } catch (err) {
      setFilterMessage(err instanceof Error ? err.message : 'Save failed');
    }
  };

  const warnings = health?.configWarnings ?? [];
  const scalpEvents = journal.filter((event) =>
    (event.event ?? '').startsWith('scalp_ob_')
  );
  const alerts = scalpEvents
    .filter((event) => event.event === 'scalp_ob_alert')
    .slice()
    .reverse();
  const outcomes = new Map<string, JournalEvent>();
  const tp1Setups = new Set<string>();
  for (const event of scalpEvents) {
    if (!event.setup_id) continue;
    if (event.event === 'scalp_ob_outcome') outcomes.set(event.setup_id, event);
    if (event.event === 'scalp_ob_tp1') tp1Setups.add(event.setup_id);
  }
  const recentEvents = scalpEvents.slice(-20).reverse();

  return (
    <main className="min-h-screen bg-neutral-950 text-neutral-100 p-4 md:p-8">
      <div className="mx-auto max-w-6xl flex flex-col gap-5">
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
            Service scalp riêng, alert-only. OB-filtered retest research. Không auto-trade,
            không dùng quota GOLD SIGNAL của bot swing.
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
                <span>Strategy</span>
                <span className="rounded-full border border-teal-500/40 bg-teal-500/10 px-2 py-0.5 text-teal-200">
                  OB retest
                </span>
              </div>
              <div className="flex items-center justify-between">
                <span>Alerts</span>
                <span className="rounded-full border border-neutral-700 px-2 py-0.5">
                  research only
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

        <section className="grid gap-3 md:grid-cols-3">
          <div className="rounded-xl border border-neutral-800 bg-neutral-900/60 p-4">
            <div className="text-[11px] uppercase text-neutral-500">Alerts</div>
            <div className="mt-1 text-2xl font-semibold">{stats?.alerts ?? 0}</div>
            <div className="mt-1 text-xs text-neutral-400">
              today {stats?.alerts_today ?? 0} · 7d {stats?.alerts_7d ?? 0}
            </div>
          </div>
          <div className="rounded-xl border border-neutral-800 bg-neutral-900/60 p-4">
            <div className="text-[11px] uppercase text-neutral-500">Win rate</div>
            <div className="mt-1 text-2xl font-semibold text-emerald-300">
              {stats && stats.completed > 0 ? `${stats.win_rate.toFixed(1)}%` : '—'}
            </div>
            <div className="mt-1 text-xs text-neutral-400">
              {stats?.wins ?? 0}/{stats?.completed ?? 0} completed
            </div>
          </div>
          <div className="rounded-xl border border-neutral-800 bg-neutral-900/60 p-4">
            <div className="text-[11px] uppercase text-neutral-500">Avg R</div>
            <div
              className={`mt-1 text-2xl font-semibold ${
                (stats?.avg_r ?? 0) >= 0 ? 'text-emerald-300' : 'text-rose-300'
              }`}
            >
              {formatR(stats?.avg_r)}
            </div>
            <div className="mt-1 text-xs text-neutral-400">
              total {formatR(stats?.total_r)}
            </div>
          </div>
          <div className="rounded-xl border border-neutral-800 bg-neutral-900/60 p-4">
            <div className="text-[11px] uppercase text-neutral-500">Profit factor</div>
            <div className="mt-1 text-2xl font-semibold">
              {stats?.pf != null ? stats.pf.toFixed(2) : '—'}
            </div>
            <div className="mt-1 text-xs text-neutral-400">
              maxDD {stats?.max_dd ? formatR(-stats.max_dd) : '0.00R'}
            </div>
          </div>
          <div className="rounded-xl border border-neutral-800 bg-neutral-900/60 p-4">
            <div className="text-[11px] uppercase text-neutral-500">Open</div>
            <div className="mt-1 text-2xl font-semibold">{stats?.open ?? 0}</div>
            <div className="mt-1 text-xs text-neutral-400">
              TP1/BE open: {stats?.tp1_open ?? 0}
            </div>
          </div>
          <div className="rounded-xl border border-neutral-800 bg-neutral-900/60 p-4">
            <div className="text-[11px] uppercase text-neutral-500">Last alert</div>
            <div className="mt-1 text-sm font-semibold text-neutral-200">
              {formatTime(stats?.last_alert_at ?? undefined)}
            </div>
            <div className="mt-1 text-xs text-neutral-400">
              TP2 {stats?.tp2 ?? 0} · TP1/BE {stats?.tp1_be ?? 0} · SL {stats?.sl ?? 0} · TO{' '}
              {stats?.timeout ?? 0}
            </div>
          </div>
        </section>

        <section className="rounded-xl border border-neutral-800 bg-neutral-900/60 p-4">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <div>
              <div className="text-xs font-semibold text-neutral-300">Scalp filters</div>
              <div className="mt-1 text-[11px] text-neutral-500">
                Active: HTF {filters?.htf_filter_enabled ? 'on' : 'off'} · RSI guard{' '}
                {filters?.rsi_h1_guard_enabled ? 'on' : 'off'} · conf ≥{' '}
                {filters?.min_confidence ?? '—'} · max {filters?.max_alerts_per_day ?? '—'} ›
                day · cooldown {filters?.cooldown_minutes ?? '—'}m
              </div>
            </div>
            <button
              type="button"
              onClick={saveFilters}
              disabled={!filterForm}
              className="rounded-lg border border-teal-500/40 bg-teal-500/10 px-3 py-2 text-xs font-semibold text-teal-200 hover:border-teal-400 disabled:opacity-40"
            >
              Save filters
            </button>
          </div>

          <div className="mt-4 grid gap-3 md:grid-cols-5">
            <label className="flex items-center gap-2 text-xs text-neutral-300">
              <input
                type="checkbox"
                checked={filterForm?.htf_filter_enabled ?? false}
                onChange={(e) => updateFilter({ htf_filter_enabled: e.target.checked })}
                className="accent-teal-500 h-4 w-4"
              />
              HTF H4/D1
            </label>
            <label className="flex items-center gap-2 text-xs text-neutral-300">
              <input
                type="checkbox"
                checked={filterForm?.rsi_h1_guard_enabled ?? false}
                onChange={(e) => updateFilter({ rsi_h1_guard_enabled: e.target.checked })}
                className="accent-teal-500 h-4 w-4"
              />
              RSI H1 guard
            </label>
            <label className="flex flex-col gap-1 text-[11px] text-neutral-500">
              Min confidence
              <input
                type="number"
                min={0}
                max={100}
                value={filterForm?.min_confidence ?? 60}
                onChange={(e) => updateFilter({ min_confidence: Number(e.target.value) })}
                className="rounded-lg border border-neutral-700 bg-neutral-950 px-2 py-1.5 font-mono text-xs text-neutral-100"
              />
            </label>
            <label className="flex flex-col gap-1 text-[11px] text-neutral-500">
              Max alerts/day
              <input
                type="number"
                min={1}
                max={10}
                value={filterForm?.max_alerts_per_day ?? 3}
                onChange={(e) => updateFilter({ max_alerts_per_day: Number(e.target.value) })}
                className="rounded-lg border border-neutral-700 bg-neutral-950 px-2 py-1.5 font-mono text-xs text-neutral-100"
              />
            </label>
            <label className="flex flex-col gap-1 text-[11px] text-neutral-500">
              Cooldown (min)
              <input
                type="number"
                min={5}
                max={240}
                value={filterForm?.cooldown_minutes ?? 45}
                onChange={(e) => updateFilter({ cooldown_minutes: Number(e.target.value) })}
                className="rounded-lg border border-neutral-700 bg-neutral-950 px-2 py-1.5 font-mono text-xs text-neutral-100"
              />
            </label>
          </div>
          {filterMessage && (
            <div className="mt-3 text-xs text-neutral-400">{filterMessage}</div>
          )}
        </section>

        <section className="rounded-xl border border-neutral-800 bg-neutral-900/60 p-4">
          <div className="text-xs font-semibold text-neutral-300">Skipped reasons</div>
          <div className="mt-2 flex flex-wrap gap-2 text-[11px]">
            {Object.entries(stats?.skipped ?? {}).length === 0 ? (
              <span className="text-neutral-500">—</span>
            ) : (
              Object.entries(stats?.skipped ?? {}).map(([reason, count]) => (
                <span
                  key={reason}
                  className="rounded-full border border-neutral-700 px-2 py-0.5 text-neutral-300"
                >
                  {reason}: {count}
                </span>
              ))
            )}
          </div>
        </section>

        <section className="rounded-xl border border-neutral-800 bg-neutral-900/60 p-4">
          <div className="flex items-center justify-between gap-2">
            <div className="text-xs font-semibold text-neutral-300">Scalp OB alerts</div>
            <div className="text-[11px] text-neutral-500">
              {alerts.length} alert(s) · journal {journal.length} lines
            </div>
          </div>

          {alerts.length === 0 ? (
            <div className="mt-3 rounded-lg border border-dashed border-neutral-700 p-4 text-xs text-neutral-400">
              Chưa có alert. Worker đang chờ PRIMARY trigger → retest → OB ≤ 1.0 ATR.
            </div>
          ) : (
            <div className="mt-3 overflow-x-auto">
              <table className="w-full min-w-[980px] text-left text-xs">
                <thead className="text-neutral-500">
                  <tr>
                    <th className="py-2 pr-3 font-medium">Time</th>
                    <th className="py-2 pr-3 font-medium">Setup</th>
                    <th className="py-2 pr-3 font-medium">Dir</th>
                    <th className="py-2 pr-3 font-medium">Entry</th>
                    <th className="py-2 pr-3 font-medium">SL</th>
                    <th className="py-2 pr-3 font-medium">TP1</th>
                    <th className="py-2 pr-3 font-medium">TP2</th>
                    <th className="py-2 pr-3 font-medium">OB</th>
                    <th className="py-2 pr-3 font-medium">Risk / lot</th>
                    <th className="py-2 pr-3 font-medium">Outcome</th>
                  </tr>
                </thead>
                <tbody>
                  {alerts.map((alert, index) => {
                    const outcome = alert.setup_id ? outcomes.get(alert.setup_id) : undefined;
                    const tp1 = alert.setup_id ? tp1Setups.has(alert.setup_id) : false;
                    const ob = noteValue(alert.notes, 'ob_distance_atr');
                    const riskUsd = noteValue(alert.notes, 'risk_usd');
                    const lot = noteValue(alert.notes, 'lot');
                    return (
                      <tr
                        key={`${alert.setup_id ?? 'alert'}-${index}`}
                        className="border-t border-neutral-800/70"
                      >
                        <td className="py-2 pr-3 text-neutral-400">
                          {formatTime(alert.created_at)}
                        </td>
                        <td className="py-2 pr-3 font-mono text-[11px] text-neutral-300">
                          {alert.setup_id ?? '—'}
                        </td>
                        <td className="py-2 pr-3 font-semibold">
                          {alert.direction ?? '—'}
                        </td>
                        <td className="py-2 pr-3 font-mono">{formatNumber(alert.entry)}</td>
                        <td className="py-2 pr-3 font-mono">
                          {formatNumber(alert.stop_loss)}
                        </td>
                        <td className="py-2 pr-3 font-mono">
                          {formatNumber(alert.take_profit_1)}
                        </td>
                        <td className="py-2 pr-3 font-mono">
                          {formatNumber(alert.take_profit_2)}
                        </td>
                        <td className="py-2 pr-3 font-mono text-teal-300">
                          {ob ? `${ob}×ATR` : '—'}
                        </td>
                        <td className="py-2 pr-3 font-mono text-neutral-400">
                          {riskUsd ? `$${riskUsd} · ${lot ?? '—'} lot` : '—'}
                        </td>
                        <td className="py-2 pr-3">
                          {outcome ? (
                            <span
                              className={
                                (outcome.r_multiple ?? 0) >= 0
                                  ? 'text-emerald-300'
                                  : 'text-rose-300'
                              }
                            >
                              {outcome.reason ?? outcome.event}{' '}
                              {formatR(outcome.r_multiple)}
                            </span>
                          ) : tp1 ? (
                            <span className="text-amber-300">TP1 / BE</span>
                          ) : (
                            <span className="text-neutral-500">OPEN</span>
                          )}
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          )}
        </section>

        <section className="rounded-xl border border-neutral-800 bg-neutral-900/60 p-4">
          <div className="text-xs font-semibold text-neutral-300">Recent scalp events</div>
          {recentEvents.length === 0 ? (
            <div className="mt-3 text-xs text-neutral-500">
              Chưa có event scalp_ob_*.
            </div>
          ) : (
            <div className="mt-3 overflow-x-auto">
              <table className="w-full min-w-[720px] text-left text-xs">
                <thead className="text-neutral-500">
                  <tr>
                    <th className="py-2 pr-3 font-medium">Time</th>
                    <th className="py-2 pr-3 font-medium">Event</th>
                    <th className="py-2 pr-3 font-medium">Setup</th>
                    <th className="py-2 pr-3 font-medium">Reason</th>
                    <th className="py-2 pr-3 font-medium">Notes</th>
                  </tr>
                </thead>
                <tbody>
                  {recentEvents.map((event, index) => (
                    <tr
                      key={`${event.event ?? 'event'}-${event.setup_id ?? index}-${index}`}
                      className="border-t border-neutral-800/70"
                    >
                      <td className="py-2 pr-3 text-neutral-400">
                        {formatTime(event.created_at)}
                      </td>
                      <td className="py-2 pr-3 font-mono text-[11px] text-neutral-300">
                        {event.event ?? '—'}
                      </td>
                      <td className="py-2 pr-3 font-mono text-[11px] text-neutral-400">
                        {event.setup_id ?? '—'}
                      </td>
                      <td className="py-2 pr-3 text-neutral-400">
                        {event.reason ?? '—'}
                      </td>
                      <td className="py-2 pr-3 text-neutral-500">
                        {event.notes ?? '—'}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
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
