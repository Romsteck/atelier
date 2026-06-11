import { useState, useEffect, useCallback } from 'react';
import { Link } from 'react-router-dom';
import {
  ShieldAlert, RefreshCw, ShieldCheck, AlertOctagon, Activity,
  AlertTriangle, AlertCircle, Info, Loader2, XCircle, ChevronRight,
} from 'lucide-react';
import { getSurveillanceOverview } from '../api/client';
import PageHeader from '../components/PageHeader';
import StatCard, { StatSkeleton } from '../components/StatCard';
import useWebSocket from '../hooks/useWebSocket';

const SEVERITIES = [
  { key: 'critical', label: 'Critical', icon: AlertOctagon,  color: 'text-red-300', bg: 'bg-red-500/20 border-red-500/30' },
  { key: 'high',     label: 'High',     icon: AlertTriangle, color: 'text-orange-300', bg: 'bg-orange-500/20 border-orange-500/30' },
  { key: 'medium',   label: 'Medium',   icon: AlertCircle,   color: 'text-yellow-300', bg: 'bg-yellow-500/20 border-yellow-500/30' },
  { key: 'low',      label: 'Low',      icon: Info,          color: 'text-blue-300', bg: 'bg-blue-500/20 border-blue-500/30' },
];

// The three scans every app has — labels come from the backend (business is
// agent-named); only icons/colors are fixed here.
const KIND_META = {
  security:    { icon: ShieldCheck,  color: 'text-fuchsia-300' },
  code_review: { icon: AlertOctagon, color: 'text-red-300' },
  business:    { icon: Activity,     color: 'text-emerald-300' },
};

function timeSince(iso) {
  if (!iso) return '?';
  const ms = Date.now() - new Date(iso).getTime();
  const s = Math.floor(ms / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}min`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h`;
  const d = Math.floor(h / 24);
  return `${d}j`;
}

// Last-run state of one kind: spinner while running, red cross on failure,
// otherwise relative timestamp of the last completed scan.
function RunStatus({ run }) {
  if (!run) return <span className="text-[11px] text-gray-600">Aucun scan</span>;
  if (run.status === 'running') {
    return (
      <span className="text-[11px] text-blue-300 flex items-center gap-1">
        <Loader2 className="w-3 h-3 animate-spin" /> en cours
      </span>
    );
  }
  const when = timeSince(run.finished_at || run.started_at);
  if (run.status === 'failed') {
    return (
      <span className="text-[11px] text-red-300 flex items-center gap-1" title={run.error || 'run failed'}>
        <XCircle className="w-3 h-3" /> échec · {when}
      </span>
    );
  }
  return <span className="text-[11px] text-gray-500">il y a {when}</span>;
}

// One scan line inside an app card — links straight to that kind's tab in the Studio.
function KindRow({ slug, kind }) {
  const meta = KIND_META[kind.kind] || KIND_META.security;
  const KindIcon = meta.icon;
  return (
    <Link
      to={`/studio?app=${slug}&tab=surveillance&kind=${kind.kind}`}
      className="flex items-center gap-2 px-2 py-1.5 rounded-sm hover:bg-gray-700/40 transition"
    >
      <KindIcon className={`w-3.5 h-3.5 shrink-0 ${kind.blank ? 'text-gray-600' : meta.color}`} />
      <span className={`text-xs flex-1 truncate ${kind.blank ? 'text-gray-500 italic' : 'text-gray-300'}`}>
        {kind.label}{kind.blank ? ' (en veille)' : ''}
      </span>
      <span className="flex items-center gap-1">
        {SEVERITIES.map((sev) =>
          kind.open?.[sev.key] > 0 ? (
            <span key={sev.key} className={`text-[11px] px-1.5 py-0.5 rounded-sm border ${sev.bg} ${sev.color}`} title={sev.label}>
              {kind.open[sev.key]}
            </span>
          ) : null
        )}
      </span>
      <RunStatus run={kind.last_run} />
    </Link>
  );
}

function AppCard({ app }) {
  return (
    <div className="bg-gray-800/50 border border-gray-700/50 rounded-lg overflow-hidden">
      <Link
        to={`/studio?app=${app.slug}&tab=surveillance`}
        className="flex items-center gap-2 px-3 py-2 border-b border-gray-700/50 hover:bg-gray-800 transition"
      >
        <span className="text-sm text-gray-50 font-medium truncate">{app.name}</span>
        <span className="text-xs text-gray-500">{app.slug}</span>
        <span className="flex-1" />
        {app.open_total > 0 ? (
          <span className="text-xs px-1.5 py-0.5 rounded-sm border bg-amber-500/20 border-amber-500/30 text-amber-300">
            {app.open_total} ouverte{app.open_total > 1 ? 's' : ''}
          </span>
        ) : (
          <span className="text-xs text-emerald-300">RAS</span>
        )}
        <ChevronRight className="w-4 h-4 text-gray-600 shrink-0" />
      </Link>
      <div className="p-1.5 space-y-0.5">
        {app.kinds.map((k) => (
          <KindRow key={k.kind} slug={app.slug} kind={k} />
        ))}
      </div>
    </div>
  );
}

// Global recap dashboard: aggregated totals + one card per app. The detail
// (findings list, dismiss/resolve, live console) lives in the per-app Studio tab.
export default function Surveillance() {
  const [overview, setOverview] = useState(null);
  const [loading, setLoading] = useState(false);
  const [err, setErr] = useState(null);

  const reload = useCallback(() => {
    setLoading(true);
    setErr(null);
    getSurveillanceOverview()
      .then((res) => setOverview(res.data || null))
      .catch((e) => {
        if (e.response?.status === 503) {
          setErr('Surveillance désactivée (Postgres injoignable au boot).');
        } else {
          setErr(e.response?.data?.error || e.message);
        }
      })
      .finally(() => setLoading(false));
  }, []);

  useEffect(() => { reload(); }, [reload]);

  // Live updates: any finding/run event across apps refreshes the snapshot.
  useWebSocket({
    'surveillance:event': () => reload(),
  });

  const totals = overview?.totals;
  const apps = [...(overview?.apps || [])].sort(
    (a, b) => b.open_total - a.open_total || a.name.localeCompare(b.name)
  );

  return (
    <div className="h-full flex flex-col">
      <PageHeader title="Surveillance IA" icon={ShieldAlert}>
        <button
          onClick={reload}
          disabled={loading}
          className="px-2 py-1 text-xs text-gray-300 hover:text-gray-50 border border-gray-700 hover:border-gray-600 rounded-sm flex items-center gap-1 disabled:opacity-50"
        >
          <RefreshCw className={`w-3 h-3 ${loading ? 'animate-spin' : ''}`} />
          Actualiser
        </button>
      </PageHeader>

      <div className="flex-1 overflow-y-auto p-4 space-y-4">
        {err && (
          <div className="p-3 bg-red-900/30 border border-red-700/50 text-red-300 rounded-sm text-sm">
            {err}
          </div>
        )}

        {!err && !overview && (
          <div className="grid grid-cols-2 md:grid-cols-3 xl:grid-cols-6 gap-3">
            {Array.from({ length: 6 }).map((_, i) => <StatSkeleton key={i} />)}
          </div>
        )}

        {/* Keep the last-known snapshot visible when a refresh fails — only an
            initial-load failure shows the error banner alone. */}
        {overview && (
          <>
            <div className="grid grid-cols-2 md:grid-cols-3 xl:grid-cols-6 gap-3">
              {SEVERITIES.map((sev) => (
                <StatCard
                  key={sev.key}
                  icon={sev.icon}
                  label={sev.label}
                  value={totals?.open?.[sev.key] ?? 0}
                  color={totals?.open?.[sev.key] > 0 ? sev.color : 'text-gray-500'}
                />
              ))}
              <StatCard
                icon={Loader2}
                label="Scans en cours"
                value={totals?.running ?? 0}
                color={totals?.running > 0 ? 'text-blue-300' : 'text-gray-500'}
              />
              <StatCard
                icon={XCircle}
                label="Échecs"
                value={totals?.failed ?? 0}
                color={totals?.failed > 0 ? 'text-red-300' : 'text-gray-500'}
              />
            </div>

            {apps.length === 0 ? (
              <div className="text-center py-12 text-gray-500 text-sm">Aucune app.</div>
            ) : (
              <div className="grid md:grid-cols-2 xl:grid-cols-3 gap-4">
                {apps.map((app) => <AppCard key={app.slug} app={app} />)}
              </div>
            )}
          </>
        )}
      </div>
    </div>
  );
}
