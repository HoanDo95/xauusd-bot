'use client';

import { useCallback, useEffect, useMemo, useState, type ReactNode } from 'react';
import {
  Activity,
  AlertTriangle,
  CheckCircle2,
  Clock3,
  Radar,
  RefreshCw,
  XCircle,
} from 'lucide-react';
import {
  fetchSetupInbox,
  type InboxSignal,
  type SetupInboxEvent,
  type SetupInboxItem,
  type SetupInboxResponse,
} from '../lib/api';

const REFRESH_MS = 15_000;

function timeLabel(value: string | null | undefined): string {
  if (!value) return '—';
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return '—';
  return date.toLocaleString([], {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    day: '2-digit',
    month: '2-digit',
  });
}

function barLabel(value: number | null): string {
  if (!value) return '—';
  return new Date(value).toLocaleString([], {
    hour: '2-digit',
    minute: '2-digit',
    day: '2-digit',
    month: '2-digit',
  });
}

function statusStyle(status: string): string {
  switch (status.toUpperCase()) {
    case 'TRIGGERED':
      return 'bg-emerald-500/15 text-emerald-300 border-emerald-500/30';
    case 'PRE_ALERT':
      return 'bg-amber-500/15 text-amber-300 border-amber-500/30';
    case 'INVALIDATED':
      return 'bg-rose-500/15 text-rose-300 border-rose-500/30';
    case 'WATCHING':
      return 'bg-sky-500/15 text-sky-300 border-sky-500/30';
    default:
      return 'bg-neutral-800 text-neutral-300 border-neutral-700';
  }
}

function sourceStyle(source: string): string {
  switch (source) {
    case 'SETUP_ENGINE':
      return 'bg-emerald-500/15 text-emerald-300 border-emerald-500/30';
    case 'SCANNER_SCENARIO':
      return 'bg-violet-500/15 text-violet-300 border-violet-500/30';
    case 'SIMULATION':
      return 'bg-amber-500/15 text-amber-300 border-amber-500/30';
    default:
      return 'bg-sky-500/15 text-sky-300 border-sky-500/30';
  }
}

function signalStatusStyle(status: string): string {
  switch (status) {
    case 'Active':
      return 'text-sky-300';
    case 'Target1Hit':
    case 'Target2Hit':
      return 'text-emerald-300';
    case 'StopLossHit':
      return 'text-rose-300';
    case 'Cancelled':
      return 'text-neutral-400';
    default:
      return 'text-neutral-300';
  }
}

function eventMeta(event: SetupInboxEvent): {
  label: string;
  icon: ReactNode;
  className: string;
} {
  switch (event.event) {
    case 'setup_pre_alert':
      return {
        label: 'PRE_ALERT',
        icon: <Clock3 className="w-3.5 h-3.5" />,
        className: 'text-amber-300 border-amber-500/30 bg-amber-500/10',
      };
    case 'setup_triggered':
      return {
        label: 'CONFIRMED',
        icon: <CheckCircle2 className="w-3.5 h-3.5" />,
        className: 'text-emerald-300 border-emerald-500/30 bg-emerald-500/10',
      };
    case 'setup_invalidated':
      return {
        label: 'INVALIDATED',
        icon: <XCircle className="w-3.5 h-3.5" />,
        className: 'text-rose-300 border-rose-500/30 bg-rose-500/10',
      };
    case 'setup_expired':
      return {
        label: 'EXPIRED',
        icon: <Clock3 className="w-3.5 h-3.5" />,
        className: 'text-neutral-400 border-neutral-700 bg-neutral-800/60',
      };
    case 'scenario_observed':
      return {
        label: 'OBSERVED',
        icon: <Radar className="w-3.5 h-3.5" />,
        className: 'text-sky-300 border-sky-500/30 bg-sky-500/10',
      };
    default:
      return {
        label: event.event.toUpperCase(),
        icon: <Activity className="w-3.5 h-3.5" />,
        className: 'text-neutral-300 border-neutral-700 bg-neutral-800/60',
      };
  }
}

function SetupCard({ setup }: { setup: SetupInboxItem }) {
  const trigger = setup.trigger_price ?? null;
  const distance = setup.distance_now;
  const level = (value: number | null) => (value == null ? '—' : value.toFixed(2));

  return (
    <div className="rounded-xl border border-neutral-800 bg-neutral-900/70 p-4 flex flex-col gap-3">
      <div className="flex flex-wrap items-start justify-between gap-2">
        <div className="flex flex-col gap-1">
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-sm font-bold text-white">{setup.name}</span>
            <span className="text-[10px] font-mono uppercase tracking-wider text-neutral-500">
              {setup.role} · {setup.bias} · {setup.confidence}%
            </span>
            {setup.stale && (
              <span className="px-2 py-0.5 rounded border border-amber-500/40 bg-amber-500/10 text-[10px] font-bold text-amber-300">
                STALE
              </span>
            )}
          </div>
          <div className="text-[11px] font-mono text-neutral-400">
            #{setup.setup_id} · updated {timeLabel(setup.updated_at)}
          </div>
        </div>
        <span
          className={`px-2 py-0.5 rounded border text-[10px] font-bold ${statusStyle(setup.status)}`}
        >
          {setup.status}
        </span>
      </div>

      <div className="grid grid-cols-2 md:grid-cols-4 gap-2 text-[11px] font-mono">
        <div className="rounded-lg border border-neutral-800 bg-neutral-950/60 p-2">
          <div className="text-[9px] uppercase tracking-wider text-neutral-500">Trigger</div>
          <div className="text-neutral-200">
            {trigger == null ? '—' : `${setup.trigger_direction} ${trigger.toFixed(2)}`}
          </div>
        </div>
        <div className="rounded-lg border border-neutral-800 bg-neutral-950/60 p-2">
          <div className="text-[9px] uppercase tracking-wider text-neutral-500">Distance now</div>
          <div className={setup.stale ? 'text-amber-300' : 'text-neutral-200'}>
            {distance == null ? '—' : `${distance.toFixed(2)} pts`}
            {setup.distance_atr != null ? ` · ${setup.distance_atr.toFixed(2)} ATR` : ''}
          </div>
        </div>
        <div className="rounded-lg border border-neutral-800 bg-neutral-950/60 p-2">
          <div className="text-[9px] uppercase tracking-wider text-neutral-500">Watch window</div>
          <div className="text-neutral-200">{setup.watch_distance.toFixed(2)} pts</div>
        </div>
        <div className="rounded-lg border border-neutral-800 bg-neutral-950/60 p-2">
          <div className="text-[9px] uppercase tracking-wider text-neutral-500">Session</div>
          <div className={setup.session === 'IN' ? 'text-emerald-300' : 'text-amber-300'}>
            {setup.session}
          </div>
        </div>
      </div>

      <div className="grid grid-cols-2 md:grid-cols-4 gap-2 text-[11px] font-mono">
        <div>
          <span className="text-neutral-500">Entry </span>
          <span className="text-white">{level(setup.entry)}</span>
        </div>
        <div>
          <span className="text-neutral-500">SL </span>
          <span className="text-rose-300">{level(setup.stop_loss)}</span>
        </div>
        <div>
          <span className="text-neutral-500">TP1 </span>
          <span className="text-emerald-300">{level(setup.take_profit_1)}</span>
        </div>
        <div>
          <span className="text-neutral-500">TP2 </span>
          <span className="text-emerald-200">{level(setup.take_profit_2)}</span>
        </div>
      </div>

      <div className="flex flex-wrap items-center gap-1.5">
        {setup.flags.length === 0 ? (
          <span className="text-[10px] font-mono text-neutral-500">Không có flag</span>
        ) : (
          setup.flags.map((flag) => (
            <span
              key={flag}
              className={`px-1.5 py-0.5 rounded border text-[9px] font-bold ${
                flag === 'COUNTER_TREND' || flag === 'HIGH_RISK'
                  ? 'border-amber-500/30 bg-amber-500/10 text-amber-300'
                  : flag === 'STRUCTURE_INVALID'
                    ? 'border-rose-500/30 bg-rose-500/10 text-rose-300'
                    : 'border-neutral-700 bg-neutral-800 text-neutral-300'
              }`}
            >
              {flag}
            </span>
          ))
        )}
      </div>

      <div className="text-[10px] font-mono text-neutral-500">
        Invalidation: {setup.invalidation || '—'}
      </div>
    </div>
  );
}

function SignalRow({ signal }: { signal: InboxSignal }) {
  return (
    <tr className="border-b border-neutral-800/70 hover:bg-neutral-850/40">
      <td className="py-2.5 px-2 text-[10px] font-mono text-neutral-400">
        {timeLabel(signal.created_at)}
      </td>
      <td className="py-2.5 px-2">
        <span
          className={`px-1.5 py-0.5 rounded border text-[9px] font-bold ${sourceStyle(signal.source)}`}
        >
          {signal.source.replace('_', ' ')}
        </span>
      </td>
      <td className="py-2.5 px-2">
        <span
          className={`px-1.5 py-0.5 rounded text-[10px] font-bold ${
            signal.side === 'BUY'
              ? 'bg-emerald-500/15 text-emerald-300'
              : 'bg-rose-500/15 text-rose-300'
          }`}
        >
          {signal.side}
        </span>
      </td>
      <td className="py-2.5 px-2 text-right font-mono text-white">{signal.entry.toFixed(2)}</td>
      <td className="py-2.5 px-2 text-right font-mono text-rose-300">
        {signal.stop_loss.toFixed(2)}
      </td>
      <td className="py-2.5 px-2 text-right font-mono text-emerald-300">
        {signal.take_profit_1.toFixed(2)}
      </td>
      <td className="py-2.5 px-2 text-right font-mono text-emerald-200">
        {signal.take_profit_2.toFixed(2)}
      </td>
      <td className={`py-2.5 px-2 text-[10px] font-mono font-bold ${signalStatusStyle(signal.status)}`}>
        {signal.status}
      </td>
      <td className="py-2.5 px-2 text-[10px] font-mono text-neutral-400">
        {signal.ai_verdict ?? '—'}
        {signal.ai_confidence != null
          ? ` ${Math.round(signal.ai_confidence * 100)}%`
          : ''}
      </td>
    </tr>
  );
}

export function SetupInboxPanel() {
  const [data, setData] = useState<SetupInboxResponse | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const next = await fetchSetupInbox();
      setData(next);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
    const timer = window.setInterval(() => {
      void load();
    }, REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [load]);

  const staleCount = useMemo(
    () => data?.setups.filter((setup) => setup.stale).length ?? 0,
    [data?.setups]
  );
  const confirmedCount = useMemo(
    () => data?.signals.filter((signal) => signal.source === 'SETUP_ENGINE').length ?? 0,
    [data?.signals]
  );

  if (loading && !data) {
    return (
      <div className="rounded-xl border border-neutral-800 bg-neutral-900 p-6 text-sm text-neutral-400">
        Đang tải Setup Inbox...
      </div>
    );
  }

  if (error && !data) {
    return (
      <div className="rounded-xl border border-rose-500/30 bg-rose-500/5 p-6 text-sm text-rose-300">
        Không tải được Setup Inbox: {error}
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-5">
      <div className="rounded-xl border border-neutral-800 bg-neutral-900 p-5">
        <div className="flex flex-wrap items-start justify-between gap-4">
          <div className="flex flex-col gap-1">
            <div className="flex items-center gap-2">
              <Radar className="w-5 h-5 text-sky-400" />
              <h3 className="text-base font-bold text-white tracking-tight">Setup Inbox</h3>
              <span className="px-2 py-0.5 rounded border border-sky-500/30 bg-sky-500/10 text-[10px] font-mono text-sky-300">
                READ-ONLY
              </span>
            </div>
            <p className="text-xs text-neutral-400 max-w-2xl">
              Gom setup theo scenario để tránh rối khi Telegram bắn liên tục. PRE_ALERT/INVALIDATED
              vẫn hiện ở đây kể cả khi quiet mode tắt Telegram.
            </p>
          </div>
          <div className="flex flex-wrap items-center gap-2 text-[11px] font-mono">
            <span className="px-2 py-1 rounded border border-neutral-700 bg-neutral-950 text-neutral-300">
              Giá {data?.price?.toFixed(2) ?? '—'}
            </span>
            <span className="px-2 py-1 rounded border border-neutral-700 bg-neutral-950 text-neutral-300">
              Session {data?.session ?? '—'}
            </span>
            <span className="px-2 py-1 rounded border border-neutral-700 bg-neutral-950 text-neutral-300">
              Bar {barLabel(data?.lastBar ?? null)}
            </span>
            <span
              className={`px-2 py-1 rounded border ${
                data?.quietMode
                  ? 'border-emerald-500/30 bg-emerald-500/10 text-emerald-300'
                  : 'border-amber-500/30 bg-amber-500/10 text-amber-300'
              }`}
            >
              Quiet mode {data?.quietMode ? 'ON' : 'OFF'}
            </span>
            <button
              type="button"
              onClick={() => void load()}
              className="px-2 py-1 rounded border border-neutral-700 bg-neutral-950 text-neutral-300 hover:border-sky-500/50 hover:text-sky-300 flex items-center gap-1"
            >
              <RefreshCw className="w-3 h-3" /> Refresh
            </button>
          </div>
        </div>

        <div className="grid grid-cols-2 md:grid-cols-4 gap-3 mt-4">
          <div className="rounded-lg border border-neutral-800 bg-neutral-950/60 p-3">
            <div className="text-[10px] uppercase tracking-wider text-neutral-500">Setups</div>
            <div className="text-xl font-bold text-white font-mono">{data?.setups.length ?? 0}</div>
          </div>
          <div className="rounded-lg border border-neutral-800 bg-neutral-950/60 p-3">
            <div className="text-[10px] uppercase tracking-wider text-neutral-500">Stale</div>
            <div className={`text-xl font-bold font-mono ${staleCount ? 'text-amber-300' : 'text-emerald-300'}`}>
              {staleCount}
            </div>
          </div>
          <div className="rounded-lg border border-neutral-800 bg-neutral-950/60 p-3">
            <div className="text-[10px] uppercase tracking-wider text-neutral-500">Setup Confirmed</div>
            <div className="text-xl font-bold text-emerald-300 font-mono">{confirmedCount}</div>
          </div>
          <div className="rounded-lg border border-neutral-800 bg-neutral-950/60 p-3">
            <div className="text-[10px] uppercase tracking-wider text-neutral-500">Watch window</div>
            <div className="text-xl font-bold text-neutral-200 font-mono">
              {data?.watchDistance?.toFixed(1) ?? '—'} pts
            </div>
          </div>
        </div>
      </div>

      <div className="flex flex-col gap-3">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            <Activity className="w-4 h-4 text-amber-400" />
            <h4 className="text-sm font-bold text-white uppercase tracking-wider font-mono">
              Active Setups
            </h4>
          </div>
          <span className="text-[11px] font-mono text-neutral-500">
            {data?.setups.length ?? 0} scenario
          </span>
        </div>
        {data?.setups.length ? (
          <div className="grid grid-cols-1 xl:grid-cols-2 gap-3">
            {data.setups.map((setup) => (
              <SetupCard key={setup.setup_id} setup={setup} />
            ))}
          </div>
        ) : (
          <div className="rounded-xl border border-neutral-800 bg-neutral-900 p-5 text-sm text-neutral-400">
            Chưa có setup nào đang được theo dõi.
          </div>
        )}
      </div>

      <div className="rounded-xl border border-neutral-800 bg-neutral-900 p-4">
        <div className="flex items-center justify-between mb-3">
          <div className="flex items-center gap-2">
            <CheckCircle2 className="w-4 h-4 text-emerald-400" />
            <h4 className="text-sm font-bold text-white uppercase tracking-wider font-mono">
              Confirmed Signals
            </h4>
          </div>
          <span className="text-[11px] font-mono text-neutral-500">
            {data?.signals.length ?? 0} signal gần nhất
          </span>
        </div>
        <div className="overflow-x-auto">
          <table className="w-full text-left text-[11px] font-mono border-collapse">
            <thead>
              <tr className="border-b border-neutral-800 text-neutral-500">
                <th className="py-2 px-2">Thời gian</th>
                <th className="py-2 px-2">Nguồn</th>
                <th className="py-2 px-2">Side</th>
                <th className="py-2 px-2 text-right">Entry</th>
                <th className="py-2 px-2 text-right">SL</th>
                <th className="py-2 px-2 text-right">TP1</th>
                <th className="py-2 px-2 text-right">TP2</th>
                <th className="py-2 px-2">Trạng thái</th>
                <th className="py-2 px-2">AI</th>
              </tr>
            </thead>
            <tbody>
              {(data?.signals ?? []).slice(0, 12).map((signal) => (
                <SignalRow key={signal.id} signal={signal} />
              ))}
            </tbody>
          </table>
        </div>
        {!data?.signals.length && (
          <p className="text-xs text-neutral-500 mt-3">Chưa có signal nào.</p>
        )}
      </div>

      <div className="rounded-xl border border-neutral-800 bg-neutral-900 p-4">
        <div className="flex items-center justify-between mb-3">
          <div className="flex items-center gap-2">
            <Clock3 className="w-4 h-4 text-sky-400" />
            <h4 className="text-sm font-bold text-white uppercase tracking-wider font-mono">
              Event Timeline
            </h4>
          </div>
          <span className="text-[11px] font-mono text-neutral-500">
            {data?.events.length ?? 0} event gần nhất
          </span>
        </div>
        <div className="flex flex-col gap-1.5 max-h-[420px] overflow-y-auto pr-1">
          {(data?.events ?? []).slice(0, 30).map((event, index) => {
            const meta = eventMeta(event);
            return (
              <div
                key={`${event.event}-${event.candle_timestamp ?? index}-${index}`}
                className="flex items-start gap-2 rounded-lg border border-neutral-800 bg-neutral-950/50 px-3 py-2"
              >
                <span className={`mt-0.5 px-1.5 py-0.5 rounded border text-[9px] font-bold inline-flex items-center gap-1 ${meta.className}`}>
                  {meta.icon}
                  {meta.label}
                </span>
                <div className="flex flex-col gap-0.5 min-w-0">
                  <div className="text-[11px] font-mono text-neutral-200">
                    {event.scenario_name ?? event.scenario_id ?? event.setup_id ?? 'setup'}
                    {event.reason ? ` · ${event.reason}` : ''}
                  </div>
                  <div className="text-[10px] font-mono text-neutral-500">
                    {timeLabel(event.created_at ?? null)}
                    {event.signal_id ? ` · signal ${event.signal_id}` : ''}
                  </div>
                </div>
              </div>
            );
          })}
          {!data?.events.length && (
            <p className="text-xs text-neutral-500">Chưa có event nào.</p>
          )}
        </div>
      </div>

      {error && (
        <div className="rounded-lg border border-amber-500/30 bg-amber-500/5 p-3 text-[11px] font-mono text-amber-300 flex items-center gap-2">
          <AlertTriangle className="w-4 h-4" />
          Refresh lỗi: {error} — đang hiển thị dữ liệu cũ.
        </div>
      )}
    </div>
  );
}
