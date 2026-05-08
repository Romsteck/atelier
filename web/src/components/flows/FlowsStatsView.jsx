import { useState, useEffect, useCallback, useMemo } from 'react';
import {
  Loader2, AlertCircle, Workflow, Activity, CheckCircle2,
  Clock, HardDrive, TrendingUp, TrendingDown, Plug, Layers,
  ChevronUp, ChevronDown,
} from 'lucide-react';

async function api(path) {
  const res = await fetch(`/api${path}`, { credentials: 'include' });
  if (!res.ok) throw new Error(`${res.status} ${res.statusText}`);
  return res.json();
}

const PERIODS = [
  { key: '24h', label: '24h' },
  { key: '7d',  label: '7 jours' },
  { key: '30d', label: '30 jours' },
  { key: 'all', label: 'Tout' },
];

function fmtMs(ms) {
  if (ms == null) return '—';
  if (ms < 1) return '<1ms';
  if (ms < 1000) return `${Math.round(ms)}ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(2)}s`;
  if (ms < 3_600_000) return `${(ms / 60_000).toFixed(1)}min`;
  return `${(ms / 3_600_000).toFixed(2)}h`;
}

function fmtBytes(b) {
  if (b == null) return '—';
  if (b < 1024) return `${b} B`;
  if (b < 1024 * 1024) return `${(b / 1024).toFixed(1)} KB`;
  if (b < 1024 * 1024 * 1024) return `${(b / 1024 / 1024).toFixed(1)} MB`;
  return `${(b / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

function fmtPct(rate) {
  if (rate == null) return '—';
  return `${(rate * 100).toFixed(1)}%`;
}

function fmtTime(iso) {
  if (!iso) return '';
  try { return new Date(iso).toLocaleString(); } catch { return iso; }
}

function shortDate(iso) {
  if (!iso) return '';
  try {
    const d = new Date(iso);
    return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
  } catch { return iso; }
}

function shortHour(iso) {
  if (!iso) return '';
  try {
    const d = new Date(iso);
    return d.toLocaleTimeString(undefined, { hour: '2-digit' });
  } catch { return iso; }
}

// ─── KPI card ───────────────────────────────────────────────────────────

function KpiCard({ icon: Icon, label, value, sub, color = 'text-blue-400' }) {
  return (
    <div className="bg-gray-800 border border-gray-700 rounded-lg p-4 flex items-start gap-3">
      <div className={`w-9 h-9 rounded-md bg-gray-900/60 flex items-center justify-center ${color} shrink-0`}>
        <Icon className="w-4 h-4" />
      </div>
      <div className="min-w-0 flex-1">
        <div className="text-[10px] uppercase tracking-wider text-gray-500">{label}</div>
        <div className="text-2xl font-semibold text-white tabular-nums truncate">{value}</div>
        {sub && <div className="text-[11px] text-gray-500 truncate">{sub}</div>}
      </div>
    </div>
  );
}

// ─── Activity sparkline (SVG) ───────────────────────────────────────────

function ActivitySpark({ buckets, period }) {
  if (!buckets || buckets.length === 0) {
    return <div className="text-xs text-gray-500 italic">Aucune activité</div>;
  }
  const w = 600;
  const h = 90;
  const padTop = 6;
  const padBottom = 18;
  const max = Math.max(1, ...buckets.map(b => b.success_count + b.failed_count));
  const barW = (w / buckets.length) * 0.85;
  const gap = (w / buckets.length) * 0.15;
  const fmt = period === '24h' ? shortHour : shortDate;
  const tickIdx = period === '24h' ? [0, 6, 12, 18, buckets.length - 1] :
                   period === '7d' ? buckets.map((_, i) => i) :
                   [0, Math.floor(buckets.length / 2), buckets.length - 1];

  return (
    <div className="bg-gray-900/40 border border-gray-700 rounded-lg p-3">
      <svg viewBox={`0 0 ${w} ${h}`} className="w-full" preserveAspectRatio="none">
        {buckets.map((b, i) => {
          const total = b.success_count + b.failed_count;
          const barH = total > 0 ? ((h - padTop - padBottom) * total) / max : 0;
          const failedH = total > 0 ? (barH * b.failed_count) / total : 0;
          const successH = barH - failedH;
          const x = i * (barW + gap) + gap / 2;
          const baseY = h - padBottom;
          return (
            <g key={i}>
              {successH > 0 && (
                <rect x={x} y={baseY - barH} width={barW} height={successH} fill="#10b981" opacity="0.7" />
              )}
              {failedH > 0 && (
                <rect x={x} y={baseY - failedH} width={barW} height={failedH} fill="#ef4444" opacity="0.85" />
              )}
              {tickIdx.includes(i) && (
                <text x={x + barW / 2} y={h - 4} fontSize="9" fill="#6b7280" textAnchor="middle">
                  {fmt(b.bucket_start)}
                </text>
              )}
            </g>
          );
        })}
      </svg>
      <div className="flex items-center gap-3 text-[10px] text-gray-500 mt-1">
        <span className="flex items-center gap-1"><span className="w-2 h-2 bg-emerald-500 rounded-sm" /> Succès</span>
        <span className="flex items-center gap-1"><span className="w-2 h-2 bg-red-500 rounded-sm" /> Échec</span>
      </div>
    </div>
  );
}

// ─── Top list ───────────────────────────────────────────────────────────

function TopList({ title, icon: Icon, rows, formatValue, valueLabel, scope, onSelectFlow }) {
  if (!rows || rows.length === 0) return null;
  const maxVal = Math.max(1, ...rows.map(r => r.value));
  return (
    <div className="bg-gray-800 border border-gray-700 rounded-lg p-3">
      <div className="flex items-center gap-2 mb-2 text-[11px] font-semibold uppercase tracking-wider text-gray-400">
        <Icon className="w-3.5 h-3.5" /> {title}
      </div>
      <div className="space-y-1.5">
        {rows.map((r, i) => {
          const pct = (r.value / maxVal) * 100;
          const clickable = !!onSelectFlow;
          return (
            <button
              key={`${r.app_slug ?? ''}/${r.flow_name}/${i}`}
              type="button"
              onClick={clickable ? () => onSelectFlow(r.app_slug, r.flow_name) : undefined}
              className={`w-full text-left rounded transition-colors ${clickable ? 'hover:bg-gray-700/40 cursor-pointer' : 'cursor-default'}`}
            >
              <div className="flex items-baseline gap-2 px-1">
                <span className="text-[11px] font-mono text-gray-200 truncate flex-1">
                  {scope === 'global' && r.app_slug && (
                    <span className="text-gray-500">{r.app_slug}/</span>
                  )}
                  {r.flow_name}
                </span>
                <span className="text-[11px] font-mono text-gray-400 tabular-nums shrink-0">{formatValue(r.value)}</span>
              </div>
              <div className="h-1 rounded bg-gray-900/60 overflow-hidden mx-1 mt-0.5">
                <div className="h-full bg-blue-400/60" style={{ width: `${pct}%` }} />
              </div>
              {valueLabel && (
                <div className="text-[9px] text-gray-500 px-1 mt-0.5">{r.count} run{r.count > 1 ? 's' : ''}</div>
              )}
            </button>
          );
        })}
      </div>
    </div>
  );
}

// ─── Per-flow table (sortable) ──────────────────────────────────────────

function PerFlowTable({ rows, scope, onSelectFlow }) {
  const [sortKey, setSortKey] = useState('count');
  const [sortDesc, setSortDesc] = useState(true);

  const sorted = useMemo(() => {
    const r = [...(rows || [])];
    r.sort((a, b) => {
      const av = a[sortKey] ?? 0;
      const bv = b[sortKey] ?? 0;
      if (typeof av === 'string') return sortDesc ? bv.localeCompare(av) : av.localeCompare(bv);
      return sortDesc ? bv - av : av - bv;
    });
    return r;
  }, [rows, sortKey, sortDesc]);

  function head(key, label) {
    const active = sortKey === key;
    return (
      <th
        onClick={() => { if (active) setSortDesc(!sortDesc); else { setSortKey(key); setSortDesc(true); } }}
        className={`text-left text-[10px] uppercase tracking-wider px-2 py-2 cursor-pointer select-none ${active ? 'text-blue-400' : 'text-gray-500 hover:text-gray-300'}`}
      >
        <span className="inline-flex items-center gap-1">
          {label}
          {active && (sortDesc ? <ChevronDown className="w-3 h-3" /> : <ChevronUp className="w-3 h-3" />)}
        </span>
      </th>
    );
  }

  if (!rows || rows.length === 0) return <div className="text-xs text-gray-500 italic px-2 py-4">Aucun flux dans la période</div>;

  return (
    <div className="bg-gray-800 border border-gray-700 rounded-lg overflow-hidden">
      <table className="w-full text-[12px]">
        <thead className="bg-gray-900/40 border-b border-gray-700">
          <tr>
            {head('flow_name', 'Flux')}
            {scope === 'global' && head('app_slug', 'App')}
            {head('count', 'Runs')}
            {head('success_rate', 'Succès')}
            {head('avg_ms', 'Avg')}
            {head('p50_ms', 'p50')}
            {head('p95_ms', 'p95')}
            {head('p99_ms', 'p99')}
            {head('total_bytes', 'Données')}
          </tr>
        </thead>
        <tbody>
          {sorted.map((r, i) => {
            const failedAny = r.failed_count > 0;
            const clickable = !!onSelectFlow;
            return (
              <tr
                key={`${r.app_slug ?? ''}/${r.flow_name}/${i}`}
                onClick={clickable ? () => onSelectFlow(r.app_slug, r.flow_name) : undefined}
                className={`border-b border-gray-700/60 last:border-0 ${clickable ? 'hover:bg-gray-700/30 cursor-pointer' : ''}`}
              >
                <td className="px-2 py-1.5 font-mono text-gray-200 truncate max-w-[220px]">{r.flow_name}</td>
                {scope === 'global' && <td className="px-2 py-1.5 font-mono text-gray-400">{r.app_slug || '—'}</td>}
                <td className="px-2 py-1.5 text-gray-200 tabular-nums">{r.count}</td>
                <td className={`px-2 py-1.5 tabular-nums ${failedAny ? 'text-red-400' : 'text-emerald-400'}`}>
                  {fmtPct(r.success_rate)}
                </td>
                <td className="px-2 py-1.5 text-gray-300 tabular-nums">{fmtMs(r.avg_ms)}</td>
                <td className="px-2 py-1.5 text-gray-400 tabular-nums">{fmtMs(r.p50_ms)}</td>
                <td className="px-2 py-1.5 text-gray-400 tabular-nums">{fmtMs(r.p95_ms)}</td>
                <td className="px-2 py-1.5 text-gray-400 tabular-nums">{fmtMs(r.p99_ms)}</td>
                <td className="px-2 py-1.5 text-gray-400 tabular-nums">{fmtBytes(r.total_bytes)}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

// ─── Main view ──────────────────────────────────────────────────────────

export default function FlowsStatsView({ scope = 'global', slug, onSelectFlow }) {
  const [period, setPeriod] = useState('7d');
  const [stats, setStats] = useState(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const path = scope === 'app'
        ? `/apps/${encodeURIComponent(slug)}/flows/_stats?period=${period}`
        : `/flows/_stats?period=${period}`;
      const r = await api(path);
      setStats(r.stats);
    } catch (e) {
      setError(e.message);
    } finally {
      setLoading(false);
    }
  }, [scope, slug, period]);

  useEffect(() => { load(); }, [load]);

  return (
    <div className="h-full overflow-auto p-4 space-y-4">
      <div className="flex items-center justify-between flex-wrap gap-3">
        <div className="flex items-center gap-2">
          {PERIODS.map(p => (
            <button
              key={p.key}
              onClick={() => setPeriod(p.key)}
              className={`px-2.5 py-1 text-[11px] font-medium rounded ${period === p.key ? 'bg-blue-500 text-white' : 'bg-gray-800 text-gray-400 hover:bg-gray-700 hover:text-white'}`}
            >
              {p.label}
            </button>
          ))}
        </div>
        {loading && <Loader2 className="w-4 h-4 text-gray-400 animate-spin" />}
      </div>

      {error && (
        <div className="px-4 py-2 bg-red-500/10 border border-red-500/30 rounded text-red-400 text-xs flex items-center gap-2">
          <AlertCircle className="w-3.5 h-3.5" /> {error}
        </div>
      )}

      {stats && (
        <>
          {/* KPI header */}
          <div className="grid grid-cols-2 lg:grid-cols-4 gap-3">
            <KpiCard
              icon={Activity}
              label="Total runs"
              value={stats.kpi.total_runs.toLocaleString()}
              sub={`${stats.kpi.success_count} ok · ${stats.kpi.failed_count} échec`}
              color="text-blue-400"
            />
            <KpiCard
              icon={CheckCircle2}
              label="Taux de succès"
              value={fmtPct(stats.kpi.success_rate)}
              sub={stats.kpi.failed_count > 0 ? `${stats.kpi.failed_count} échec` : 'Aucun échec'}
              color={stats.kpi.failed_count > 0 ? 'text-amber-400' : 'text-emerald-400'}
            />
            <KpiCard
              icon={Clock}
              label="Temps cumulé"
              value={fmtMs(stats.kpi.total_duration_ms)}
              sub="Somme des durées"
              color="text-violet-400"
            />
            <KpiCard
              icon={HardDrive}
              label="Données transférées"
              value={fmtBytes(stats.kpi.total_bytes)}
              sub="Input + output"
              color="text-fuchsia-400"
            />
          </div>

          {/* Activity */}
          <div>
            <div className="flex items-center gap-2 mb-2 text-[11px] font-semibold uppercase tracking-wider text-gray-400">
              <Activity className="w-3.5 h-3.5" /> Activité
            </div>
            <ActivitySpark buckets={stats.activity} period={period} />
          </div>

          {/* Failures section */}
          {(stats.recent_failures.length > 0 || stats.step_hotspots.length > 0) && (
            <div className="grid grid-cols-1 lg:grid-cols-2 gap-3">
              {/* Recent failures */}
              <div className="bg-gray-800 border border-red-500/20 rounded-lg p-3">
                <div className="flex items-center gap-2 mb-2 text-[11px] font-semibold uppercase tracking-wider text-red-400">
                  <AlertCircle className="w-3.5 h-3.5" /> Derniers échecs
                </div>
                {stats.recent_failures.length === 0 ? (
                  <div className="text-xs text-gray-500 italic">Aucun échec sur la période 🎉</div>
                ) : (
                  <div className="space-y-1.5">
                    {stats.recent_failures.map(f => (
                      <button
                        key={f.run_id}
                        type="button"
                        onClick={() => onSelectFlow && onSelectFlow(f.app_slug, f.flow_name, f.run_id)}
                        className="w-full text-left bg-red-500/5 hover:bg-red-500/10 rounded px-2 py-1.5 transition-colors"
                      >
                        <div className="flex items-baseline gap-2 mb-0.5">
                          <span className="font-mono text-[11px] text-red-300 truncate">
                            {scope === 'global' && f.app_slug && <span className="text-gray-500">{f.app_slug}/</span>}
                            {f.flow_name}
                          </span>
                          {f.failed_step_id && (
                            <span className="text-[10px] font-mono text-red-400 bg-red-500/10 px-1 rounded">@{f.failed_step_id}</span>
                          )}
                          <span className="text-[10px] text-gray-500 ml-auto">{fmtTime(f.started_at)}</span>
                        </div>
                        {f.error_message && (
                          <div className="text-[10px] text-gray-400 font-mono truncate">{f.error_message}</div>
                        )}
                      </button>
                    ))}
                  </div>
                )}
              </div>

              {/* Step hotspots */}
              <div className="bg-gray-800 border border-gray-700 rounded-lg p-3">
                <div className="flex items-center gap-2 mb-2 text-[11px] font-semibold uppercase tracking-wider text-gray-400">
                  <TrendingDown className="w-3.5 h-3.5 text-amber-400" /> Steps à risque
                </div>
                {stats.step_hotspots.length === 0 ? (
                  <div className="text-xs text-gray-500 italic">Aucun step défaillant</div>
                ) : (
                  <div className="space-y-1">
                    {stats.step_hotspots.map((h, i) => (
                      <div key={i} className="flex items-baseline gap-2 px-1 py-1 text-[11px]">
                        <span className="font-mono text-gray-300 truncate flex-1">
                          {scope === 'global' && h.app_slug && <span className="text-gray-500">{h.app_slug}/</span>}
                          <span className="text-gray-200">{h.flow_name}</span>
                          <span className="text-gray-500">/</span>
                          <span className="text-amber-400">{h.step_id}</span>
                        </span>
                        <span className="text-gray-400 tabular-nums">{h.failure_count}</span>
                      </div>
                    ))}
                  </div>
                )}
              </div>
            </div>
          )}

          {/* Top lists grid */}
          <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-3">
            <TopList
              title="Plus exécutés"
              icon={TrendingUp}
              rows={stats.top_by_count}
              formatValue={(v) => `${v.toFixed(0)}`}
              scope={scope}
              onSelectFlow={onSelectFlow}
            />
            <TopList
              title="Plus lents (avg)"
              icon={Clock}
              rows={stats.top_by_avg_duration}
              formatValue={fmtMs}
              valueLabel
              scope={scope}
              onSelectFlow={onSelectFlow}
            />
            <TopList
              title="Plus coûteux (cumulé)"
              icon={Layers}
              rows={stats.top_by_total_time}
              formatValue={fmtMs}
              valueLabel
              scope={scope}
              onSelectFlow={onSelectFlow}
            />
            <TopList
              title="Plus volumineux"
              icon={HardDrive}
              rows={stats.top_by_bytes}
              formatValue={fmtBytes}
              valueLabel
              scope={scope}
              onSelectFlow={onSelectFlow}
            />
          </div>

          {/* Per-app + per-connector (global only) */}
          {scope === 'global' && (stats.per_app.length > 0 || stats.per_connector.length > 0) && (
            <div className="grid grid-cols-1 lg:grid-cols-2 gap-3">
              {stats.per_app.length > 0 && (
                <div className="bg-gray-800 border border-gray-700 rounded-lg overflow-hidden">
                  <div className="px-3 py-2 border-b border-gray-700 text-[11px] font-semibold uppercase tracking-wider text-gray-400 flex items-center gap-2">
                    <Workflow className="w-3.5 h-3.5" /> Par application
                  </div>
                  <table className="w-full text-[12px]">
                    <thead className="bg-gray-900/40 border-b border-gray-700">
                      <tr>
                        <th className="text-left px-3 py-1.5 text-[10px] uppercase text-gray-500">App</th>
                        <th className="text-left px-3 py-1.5 text-[10px] uppercase text-gray-500">Runs</th>
                        <th className="text-left px-3 py-1.5 text-[10px] uppercase text-gray-500">Succès</th>
                        <th className="text-left px-3 py-1.5 text-[10px] uppercase text-gray-500">Temps</th>
                        <th className="text-left px-3 py-1.5 text-[10px] uppercase text-gray-500">Données</th>
                      </tr>
                    </thead>
                    <tbody>
                      {stats.per_app.map(r => (
                        <tr key={r.slug} className="border-b border-gray-700/60 last:border-0 hover:bg-gray-700/30">
                          <td className="px-3 py-1.5 font-mono text-gray-200">{r.slug}</td>
                          <td className="px-3 py-1.5 text-gray-200 tabular-nums">{r.run_count}</td>
                          <td className={`px-3 py-1.5 tabular-nums ${r.failed_count > 0 ? 'text-amber-400' : 'text-emerald-400'}`}>
                            {fmtPct(r.success_rate)}
                          </td>
                          <td className="px-3 py-1.5 text-gray-400 tabular-nums">{fmtMs(r.total_duration_ms)}</td>
                          <td className="px-3 py-1.5 text-gray-400 tabular-nums">{fmtBytes(r.total_bytes)}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
              {stats.per_connector.length > 0 && (
                <div className="bg-gray-800 border border-gray-700 rounded-lg overflow-hidden">
                  <div className="px-3 py-2 border-b border-gray-700 text-[11px] font-semibold uppercase tracking-wider text-gray-400 flex items-center gap-2">
                    <Plug className="w-3.5 h-3.5" /> Par connecteur
                  </div>
                  <table className="w-full text-[12px]">
                    <thead className="bg-gray-900/40 border-b border-gray-700">
                      <tr>
                        <th className="text-left px-3 py-1.5 text-[10px] uppercase text-gray-500">Connecteur</th>
                        <th className="text-left px-3 py-1.5 text-[10px] uppercase text-gray-500">Ops</th>
                        <th className="text-left px-3 py-1.5 text-[10px] uppercase text-gray-500">Temps</th>
                        <th className="text-left px-3 py-1.5 text-[10px] uppercase text-gray-500">Données</th>
                      </tr>
                    </thead>
                    <tbody>
                      {stats.per_connector.map(r => (
                        <tr key={r.connector} className="border-b border-gray-700/60 last:border-0 hover:bg-gray-700/30">
                          <td className="px-3 py-1.5 font-mono text-gray-200">{r.connector}</td>
                          <td className="px-3 py-1.5 text-gray-200 tabular-nums">{r.op_count}</td>
                          <td className="px-3 py-1.5 text-gray-400 tabular-nums">{fmtMs(r.total_duration_ms)}</td>
                          <td className="px-3 py-1.5 text-gray-400 tabular-nums">{fmtBytes(r.total_bytes)}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
            </div>
          )}

          {/* Per-flow table */}
          <div>
            <div className="flex items-center gap-2 mb-2 text-[11px] font-semibold uppercase tracking-wider text-gray-400">
              <Workflow className="w-3.5 h-3.5" /> Tous les flux
            </div>
            <PerFlowTable rows={stats.per_flow} scope={scope} onSelectFlow={onSelectFlow} />
          </div>
        </>
      )}
    </div>
  );
}
