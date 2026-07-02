import { useState, useEffect, useCallback, useRef, useMemo } from 'react';
import {
  ShieldAlert, ShieldCheck, AlertOctagon, Activity, Wrench, Radar,
  AlertTriangle, AlertCircle, Info, Loader2, XCircle, ChevronRight,
} from 'lucide-react';
import {
  getSurveillanceOverview, getSurveillanceSweep, startSurveillanceSweep,
  cancelSurveillanceSweep, getSweepSchedule, putSweepSchedule, getSurveillanceTranscript,
  getResolvingFindings,
} from '../api/client';
import { openStudio } from '../lib/openStudio';
import PageHeader from '../components/PageHeader';
import StatCard, { StatSkeleton } from '../components/StatCard';
import SweepLiveView from '../components/surveillance/SweepLiveView';
import SchedulePopover from '../components/surveillance/SchedulePopover';
import { mergeLines } from '../components/surveillance/scanFormat';
import useWebSocket from '../hooks/useWebSocket';
import { useToast, Toast } from '../hooks/useToast';
import { apiErr } from '../utils/apiErr';

const SEVERITIES = [
  { key: 'critical', label: 'Critical', icon: AlertOctagon,  color: 'text-red-700 dark:text-red-300', bg: 'bg-red-500/20 border-red-500/30' },
  { key: 'high',     label: 'High',     icon: AlertTriangle, color: 'text-orange-700 dark:text-orange-300', bg: 'bg-orange-500/20 border-orange-500/30' },
  { key: 'medium',   label: 'Medium',   icon: AlertCircle,   color: 'text-yellow-700 dark:text-yellow-300', bg: 'bg-yellow-500/20 border-yellow-500/30' },
  { key: 'low',      label: 'Low',      icon: Info,          color: 'text-blue-700 dark:text-blue-300', bg: 'bg-blue-500/20 border-blue-500/30' },
];

// The three scans every app has — labels come from the backend (business is
// agent-named); only icons/colors are fixed here.
const KIND_META = {
  security:    { icon: ShieldCheck,  color: 'text-fuchsia-700 dark:text-fuchsia-300' },
  code_review: { icon: AlertOctagon, color: 'text-red-700 dark:text-red-300' },
  business:    { icon: Activity,     color: 'text-emerald-700 dark:text-emerald-300' },
};

const KINDS_ORDER = ['security', 'code_review', 'business'];

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
      <span className="text-[11px] text-blue-700 dark:text-blue-300 flex items-center gap-1">
        <Loader2 className="w-3 h-3 animate-spin" /> en cours
      </span>
    );
  }
  const when = timeSince(run.finished_at || run.started_at);
  if (run.status === 'failed') {
    return (
      <span className="text-[11px] text-red-700 dark:text-red-300 flex items-center gap-1" title={run.error || 'run failed'}>
        <XCircle className="w-3 h-3" /> échec · {when}
      </span>
    );
  }
  return <span className="text-[11px] text-gray-500">il y a {when}</span>;
}

// One scan line inside an app card — links straight to that kind's tab in the Studio.
// `resolving` = a finding of this kind has an open resolution conversation.
function KindRow({ slug, kind, resolving }) {
  const meta = KIND_META[kind.kind] || KIND_META.security;
  const KindIcon = meta.icon;
  return (
    <button
      type="button"
      onClick={() => openStudio(slug, { tab: 'surveillance', kind: kind.kind })}
      className="w-full text-left flex items-center gap-2 px-2 py-1.5 rounded-sm hover:bg-gray-700/40 transition"
    >
      <KindIcon className={`w-3.5 h-3.5 shrink-0 ${kind.blank ? 'text-gray-600' : meta.color}`} />
      <span className={`text-xs flex-1 truncate ${kind.blank ? 'text-gray-500 italic' : 'text-gray-300'}`}>
        {kind.label}{kind.blank ? ' (en veille)' : ''}
      </span>
      {resolving && (
        <span className="flex items-center gap-0.5 text-[11px] text-blue-700 dark:text-blue-300" title="Résolution en cours (conversation agent ouverte)">
          <Wrench className="w-3 h-3 animate-pulse" />
        </span>
      )}
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
    </button>
  );
}

// `resolving` = { count, kinds: Set<kind> } for this app (findings being resolved).
function AppCard({ app, resolving }) {
  const resolvingKinds = resolving?.kinds || new Set();
  return (
    <div className={`bg-gray-800/50 border rounded-lg overflow-hidden ${resolving?.count ? 'border-blue-500/40' : 'border-gray-700/50'}`}>
      <button
        type="button"
        onClick={() => openStudio(app.slug, { tab: 'surveillance' })}
        className="w-full text-left flex items-center gap-2 px-3 py-2 border-b border-gray-700/50 hover:bg-gray-800 transition"
      >
        <span className="text-sm text-gray-50 font-medium truncate">{app.name}</span>
        <span className="text-xs text-gray-500">{app.slug}</span>
        <span className="flex-1" />
        {resolving?.count > 0 && (
          <span className="text-xs px-1.5 py-0.5 rounded-sm border bg-blue-500/20 border-blue-500/30 text-blue-700 dark:text-blue-300 flex items-center gap-1" title={`${resolving.count} finding(s) en cours de résolution`}>
            <Wrench className="w-3 h-3" /> {resolving.count}
          </span>
        )}
        {app.open_total > 0 ? (
          <span className="text-xs px-1.5 py-0.5 rounded-sm border bg-amber-500/20 border-amber-500/30 text-amber-700 dark:text-amber-300">
            {app.open_total} ouverte{app.open_total > 1 ? 's' : ''}
          </span>
        ) : (
          <span className="text-xs text-emerald-700 dark:text-emerald-300">RAS</span>
        )}
        <ChevronRight className="w-4 h-4 text-gray-600 shrink-0" />
      </button>
      <div className="p-1.5 space-y-0.5">
        {app.kinds.map((k) => (
          <KindRow key={k.kind} slug={app.slug} kind={k} resolving={resolvingKinds.has(k.kind)} />
        ))}
      </div>
    </div>
  );
}

// Global recap dashboard: aggregated totals + one card per app. The detail
// (findings list, dismiss/resolve, live console) lives in the per-app Studio tab.
export default function Surveillance() {
  const [overview, setOverview] = useState(null);
  const [err, setErr] = useState(null);
  // Automatic sweep state + live transcripts (keyed by run_id) + schedule config.
  const [sweep, setSweep] = useState(null);
  const [transcripts, setTranscripts] = useState({});
  const [schedule, setSchedule] = useState(null);
  // Findings being resolved right now (open agent conversation) across all apps.
  const [resolving, setResolving] = useState([]);
  const currentRunIdsRef = useRef(new Set());
  const { toast, showToast } = useToast();

  const sweepActive = !!sweep && (sweep.status === 'running' || sweep.status === 'cancelling');

  // No manual refresh button: the overview reloads on every surveillance:event
  // (finding/run change) over WebSocket, on mount, on sweep settle, and on reconnect.
  const reload = useCallback(() => {
    setErr(null);
    getSurveillanceOverview()
      .then((res) => setOverview(res.data || null))
      .catch((e) => {
        if (e.response?.status === 503) {
          setErr('Surveillance désactivée (Postgres injoignable au boot).');
        } else {
          setErr(e.response?.data?.error || e.message);
        }
      });
  }, []);

  const reloadResolving = useCallback(() => {
    getResolvingFindings().then((r) => setResolving(r.data?.resolving || [])).catch(() => {});
  }, []);

  useEffect(() => { reload(); reloadResolving(); }, [reload, reloadResolving]);

  // Hydrate sweep + schedule on mount (a refresh mid-sweep shows the live view).
  const hydrateSweep = useCallback(() => {
    getSurveillanceSweep().then((r) => setSweep(r.data?.sweep || null)).catch(() => {});
  }, []);
  useEffect(() => {
    hydrateSweep();
    getSweepSchedule().then((r) => setSchedule(r.data?.schedule || null)).catch(() => {});
  }, [hydrateSweep]);

  // The 3 run_ids of the app currently being scanned (stable key drives the
  // transcript reset + buffered re-fetch when the sweep advances).
  const current = sweep?.apps?.[sweep?.current_index];
  const currentRunIds = useMemo(
    () => (current ? KINDS_ORDER.map((k) => current[k]?.run_id).filter(Boolean) : []),
    [current],
  );
  const runIdsKey = currentRunIds.join('|');

  useEffect(() => {
    currentRunIdsRef.current = new Set(currentRunIds);
    if (currentRunIds.length === 0 || !current) return;
    setTranscripts({});
    let cancelled = false;
    const slug = current.slug;
    currentRunIds.forEach((rid) => {
      getSurveillanceTranscript(slug, rid)
        .then((r) => { if (!cancelled) setTranscripts((p) => ({ ...p, [rid]: mergeLines(p[rid] || [], r.data?.lines || []) })); })
        .catch(() => {});
    });
    return () => { cancelled = true; };
  }, [runIdsKey]); // eslint-disable-line react-hooks/exhaustive-deps

  // Reload the overview once a sweep settles (it produced/pruned findings).
  const prevStatus = useRef(null);
  useEffect(() => {
    const st = sweep?.status;
    if (prevStatus.current === 'running' && st && st !== 'running' && st !== 'cancelling') reload();
    prevStatus.current = st;
  }, [sweep?.status, reload]);

  // Live updates (handlers see fresh closures via useWebSocket's ref).
  const { epoch } = useWebSocket({
    'surveillance:event': () => { if (!sweepActive) reload(); },
    'surveillance:sweep': (data) => setSweep(data),
    'surveillance:transcript': (data) => {
      if (!data || !currentRunIdsRef.current.has(data.run_id)) return;
      setTranscripts((prev) => ({ ...prev, [data.run_id]: mergeLines(prev[data.run_id] || [], [data]) }));
    },
    // A resolution conversation opened/closed anywhere → recompute the indicators.
    'agent:open-tabs': () => reloadResolving(),
  });
  // Re-sync after a WS reconnect (the broadcast channel doesn't replay history).
  // Inclut `reload()` : un scan démarré pendant que l'onglet était gelé (mobile / mise en
  // veille) n'a pas déclenché de `surveillance:event` reçu → l'overview serait périmé sans ça.
  useEffect(() => {
    if (epoch > 0) { reload(); hydrateSweep(); reloadResolving(); }
  }, [epoch, reload, hydrateSweep, reloadResolving]);

  const handleStartSweep = async () => {
    try {
      const r = await startSurveillanceSweep();
      setSweep(r.data?.sweep || null);
    } catch (e) {
      if (e.response?.status !== 409) showToast('Démarrage du sweep a échoué : ' + apiErr(e), 'error');
    }
  };
  const handleCancelSweep = async () => {
    try { await cancelSurveillanceSweep(); }
    catch (e) { showToast('Annulation a échoué : ' + apiErr(e), 'error'); }
  };
  const saveSchedule = async (patch) => {
    const next = { enabled: schedule?.enabled ?? false, hour: schedule?.hour ?? 3, cadence: schedule?.cadence ?? 'daily', ...patch };
    setSchedule(next);
    try {
      const r = await putSweepSchedule({ enabled: next.enabled, hour: next.hour, cadence: next.cadence });
      setSchedule(r.data?.schedule || next);
    } catch (e) {
      showToast('Sauvegarde planification a échoué : ' + apiErr(e), 'error');
    }
  };

  const totals = overview?.totals;
  const apps = [...(overview?.apps || [])].sort(
    (a, b) => b.open_total - a.open_total || a.name.localeCompare(b.name)
  );

  // Per-app aggregation of findings being resolved → card badge + per-kind wrench.
  const resolvingByApp = useMemo(() => {
    const m = new Map();
    for (const r of resolving) {
      const e = m.get(r.slug) || { count: 0, kinds: new Set() };
      e.count += 1;
      if (r.kind) e.kinds.add(r.kind);
      m.set(r.slug, e);
    }
    return m;
  }, [resolving]);
  const anyResolving = resolving.length > 0;
  // Un scan unitaire (lancé depuis le Studio) est en cours → bloquer le sweep global + la
  // planification. Le bouton « Tout scanner » n'est rendu que si `!sweepActive`, donc ici
  // `totals.running` ne compte que des scans unitaires (jamais des runs de sweep). Réactif :
  // l'overview est rechargé sur chaque `surveillance:event` (run started/finished).
  const anyRunning = (totals?.running ?? 0) > 0;

  return (
    <div className="h-full flex flex-col">
      <Toast toast={toast} />
      <PageHeader title="Surveillance IA" icon={ShieldAlert}>
        <SchedulePopover schedule={schedule} onSave={saveSchedule} disabled={anyRunning} />
        {!sweepActive && (
          <button
            onClick={handleStartSweep}
            disabled={anyResolving || anyRunning}
            title={anyRunning
              ? 'Indisponible : un scan est en cours (lancé depuis le Studio). Attends sa fin avant de tout scanner.'
              : anyResolving
                ? 'Indisponible : une résolution de finding est en cours (conversation agent ouverte). Ferme-la avant de tout scanner.'
                : 'Scanner toutes les apps (3 scans chacune, app par app)'}
            className="px-2 py-1 text-xs border rounded-sm flex items-center gap-1 disabled:opacity-50 disabled:cursor-not-allowed bg-emerald-500/20 text-emerald-700 dark:text-emerald-200 hover:bg-emerald-500/30 border-emerald-500/30"
          >
            <Radar className="w-3 h-3" /> Tout scanner
          </button>
        )}
      </PageHeader>

      <div className="flex-1 overflow-y-auto p-4 space-y-4">
        {sweepActive ? (
          <SweepLiveView sweep={sweep} transcripts={transcripts} onCancel={handleCancelSweep} />
        ) : (
          <>
            {err && (
              <div className="p-3 bg-red-900/30 border border-red-700/50 text-red-700 dark:text-red-300 rounded-sm text-sm">
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
                    color={totals?.running > 0 ? 'text-blue-700 dark:text-blue-300' : 'text-gray-500'}
                  />
                  <StatCard
                    icon={XCircle}
                    label="Échecs"
                    value={totals?.failed ?? 0}
                    color={totals?.failed > 0 ? 'text-red-700 dark:text-red-300' : 'text-gray-500'}
                  />
                </div>

                {apps.length === 0 ? (
                  <div className="text-center py-12 text-gray-500 text-sm">Aucune app.</div>
                ) : (
                  <div className="grid md:grid-cols-2 xl:grid-cols-3 gap-4">
                    {apps.map((app) => <AppCard key={app.slug} app={app} resolving={resolvingByApp.get(app.slug)} />)}
                  </div>
                )}
              </>
            )}
          </>
        )}
      </div>
    </div>
  );
}
