import { useState, useEffect, useCallback, useMemo } from 'react';
import {
  ShieldAlert, RefreshCw, Play, Settings as SettingsIcon, ChevronDown, ChevronRight,
  X, Check, AlertOctagon, Lightbulb, Clock, Save, ShieldCheck,
} from 'lucide-react';
import {
  getAppFindings,
  dismissFinding,
  resolveFinding,
  runSurveillance,
  listSurveillanceRuns,
  getSurveillanceConfig,
  updateSurveillanceConfig,
} from '../api/client';
import MarkdownView from './docs/MarkdownView';
import useWebSocket from '../hooks/useWebSocket';

// The three scan kinds. `id` is the finding kind (singular for suggestions);
// `runKind` is what surveillance_run expects (plural for suggestions).
const KINDS = [
  { id: 'code_review', label: 'Bugs', runKind: 'code_review', icon: AlertOctagon, color: 'text-red-300', btn: 'bg-red-500/20 text-red-200 hover:bg-red-500/30 border-red-500/30' },
  { id: 'security', label: 'Sécurité', runKind: 'security', icon: ShieldCheck, color: 'text-fuchsia-300', btn: 'bg-fuchsia-500/20 text-fuchsia-200 hover:bg-fuchsia-500/30 border-fuchsia-500/30' },
  { id: 'suggestion', label: 'Améliorations', runKind: 'suggestions', icon: Lightbulb, color: 'text-blue-300', btn: 'bg-blue-500/20 text-blue-200 hover:bg-blue-500/30 border-blue-500/30' },
];

// Category labels per kind (mirror RunKind::categories in atelier-watcher).
const CATEGORIES = {
  code_review: { bug: 'Bug / logique', architecture: 'Architecture', performance: 'Performance', composants: 'Composants', gestion_erreurs: "Gestion d'erreurs", autres: 'Autres' },
  suggestion: { performance: 'Performance', ux: 'UX / ergonomie', autres: 'Autres' },
  security: { auth: 'Auth / autorisation', injection: 'Injection', secrets: 'Secrets', exposition: 'Exposition données', autres: 'Autres' },
};

const SEVERITIES = [
  { key: 'critical', label: 'Critical', color: 'text-red-300', bg: 'bg-red-500/20 border-red-500/30' },
  { key: 'high', label: 'High', color: 'text-orange-300', bg: 'bg-orange-500/20 border-orange-500/30' },
  { key: 'medium', label: 'Medium', color: 'text-yellow-300', bg: 'bg-yellow-500/20 border-yellow-500/30' },
  { key: 'low', label: 'Low', color: 'text-blue-300', bg: 'bg-blue-500/20 border-blue-500/30' },
];

const STATUSES = [
  { key: 'open', label: 'Ouvertes', color: 'text-amber-300' },
  { key: 'resolved', label: 'Résolues', color: 'text-emerald-300' },
  { key: 'dismissed', label: 'Dismiss', color: 'text-gray-400' },
];

const sevMeta = (k) => SEVERITIES.find((s) => s.key === k) || SEVERITIES[3];
const catLabel = (kind, cat) => (CATEGORIES[kind] && CATEGORIES[kind][cat]) || cat || 'autres';

function timeSince(iso) {
  if (!iso) return '?';
  const s = Math.floor((Date.now() - new Date(iso).getTime()) / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}min`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h`;
  return `${Math.floor(h / 24)}j`;
}

function FindingCard({ finding, onDismiss, onResolve }) {
  const [open, setOpen] = useState(false);
  const sev = sevMeta(finding.severity);
  return (
    <div className="border border-gray-700 bg-gray-800/40 rounded">
      <button onClick={() => setOpen((v) => !v)} className="w-full flex items-start gap-3 px-3 py-2 text-left hover:bg-gray-800/70">
        {open ? <ChevronDown className="w-4 h-4 text-gray-400 mt-0.5 shrink-0" /> : <ChevronRight className="w-4 h-4 text-gray-400 mt-0.5 shrink-0" />}
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2 flex-wrap">
            <span className={`text-xs px-1.5 py-0.5 rounded border ${sev.bg} ${sev.color}`}>{sev.label}</span>
            <span className="text-sm text-white truncate">{finding.title}</span>
          </div>
          <div className="text-xs text-gray-500 mt-0.5">Vu il y a {timeSince(finding.last_seen)}</div>
        </div>
        {finding.status === 'open' && (
          <div className="flex gap-1 shrink-0">
            <button onClick={(e) => { e.stopPropagation(); onDismiss(finding); }} className="px-2 py-1 text-xs text-gray-300 hover:text-white hover:bg-gray-700 rounded flex items-center gap-1"><X className="w-3 h-3" /> Dismiss</button>
            <button onClick={(e) => { e.stopPropagation(); onResolve(finding); }} className="px-2 py-1 text-xs text-emerald-300 hover:text-emerald-200 hover:bg-emerald-900/30 rounded flex items-center gap-1"><Check className="w-3 h-3" /> Résolu</button>
          </div>
        )}
      </button>
      {open && (
        <div className="px-3 pb-3 pt-1 border-t border-gray-700/50 space-y-3">
          <div>
            <div className="text-xs text-gray-500 mb-1">Summary</div>
            <MarkdownView>{finding.summary}</MarkdownView>
          </div>
          {finding.plan && (
            <div>
              <div className="text-xs text-gray-500 mb-1">Plan</div>
              <MarkdownView>{finding.plan}</MarkdownView>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function RunRow({ run }) {
  const colorByStatus = {
    success: 'text-emerald-300', success_empty: 'text-gray-400',
    skipped: 'text-yellow-400', failed: 'text-red-400', running: 'text-blue-400',
  };
  const kindShort = { code_review: 'review', suggestions: 'sugg.', security: 'sécu' };
  return (
    <div className="flex items-center gap-2 text-xs px-2 py-1 border-b border-gray-700/30 last:border-b-0">
      <Clock className="w-3 h-3 text-gray-500 shrink-0" />
      <span className="text-gray-400 w-12 shrink-0">{kindShort[run.kind] || run.kind}</span>
      <span className={`${colorByStatus[run.status] || 'text-gray-300'} w-24 shrink-0`}>{run.status}</span>
      <span className="text-gray-400 flex-1 truncate">{run.skip_reason || run.error || `${run.findings_count} finding${run.findings_count > 1 ? 's' : ''}`}</span>
      <span className="text-gray-600 shrink-0">{timeSince(run.started_at)}</span>
    </div>
  );
}

function ConfigPanel({ slug, onClose }) {
  const [cfg, setCfg] = useState(null);
  const [saving, setSaving] = useState(false);
  const [err, setErr] = useState(null);

  useEffect(() => {
    getSurveillanceConfig(slug).then((r) => setCfg(r.data)).catch((e) => setErr(e.response?.data?.error || e.message));
  }, [slug]);

  if (!cfg && !err) return <div className="p-3 text-xs text-gray-500">Chargement…</div>;
  if (err) return <div className="p-3 text-xs text-red-400">{err}</div>;

  const save = async () => {
    setSaving(true);
    try {
      const r = await updateSurveillanceConfig(slug, {
        throttle_threshold: cfg.throttle_threshold,
        max_tokens_per_day: cfg.max_tokens_per_day,
      });
      setCfg(r.data);
      onClose?.();
    } catch (e) {
      setErr(e.response?.data?.error || e.message);
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="p-3 space-y-2 text-xs">
      <div className="text-gray-500">Runs manuels uniquement — gates appliqués à chaque lancement.</div>
      <label className="flex items-center gap-2">
        <span className="w-40">Throttle (findings open max)</span>
        <input type="number" min={1} max={100} value={cfg.throttle_threshold} onChange={(e) => setCfg({ ...cfg, throttle_threshold: parseInt(e.target.value, 10) || 5 })} className="ml-auto bg-gray-900 border border-gray-700 px-1 py-0.5 rounded text-gray-200 w-20" />
      </label>
      <label className="flex items-center gap-2">
        <span className="w-40">Budget tokens / jour</span>
        <input type="number" min={1000} step={1000} value={cfg.max_tokens_per_day} onChange={(e) => setCfg({ ...cfg, max_tokens_per_day: parseInt(e.target.value, 10) || 100000 })} className="ml-auto bg-gray-900 border border-gray-700 px-1 py-0.5 rounded text-gray-200 w-24" />
      </label>
      <button onClick={save} disabled={saving} className="mt-2 px-3 py-1.5 text-xs bg-blue-500 hover:bg-blue-600 text-white rounded flex items-center gap-1 disabled:opacity-50">
        <Save className="w-3 h-3" /> {saving ? 'Sauvegarde…' : 'Sauvegarder'}
      </button>
    </div>
  );
}

export default function SurveillanceTab({ slug }) {
  const [activeKind, setActiveKind] = useState('code_review');
  const [findings, setFindings] = useState([]);
  const [runs, setRuns] = useState([]);
  const [loading, setLoading] = useState(false);
  const [running, setRunning] = useState(null); // runKind currently launching
  const [err, setErr] = useState(null);
  const [statusFilter, setStatusFilter] = useState('open');
  const [showConfig, setShowConfig] = useState(false);

  const kindMeta = KINDS.find((k) => k.id === activeKind) || KINDS[0];

  const reload = useCallback(() => {
    setLoading(true);
    setErr(null);
    Promise.all([
      getAppFindings(slug, { kind: activeKind, status: statusFilter || undefined, limit: 300 }),
      listSurveillanceRuns(slug, { limit: 12 }),
    ])
      .then(([f, r]) => {
        setFindings(f.data?.findings || []);
        setRuns(r.data?.runs || []);
      })
      .catch((e) => {
        if (e.response?.status === 503) setErr('Surveillance désactivée (Postgres injoignable).');
        else setErr(e.response?.data?.error || e.message);
      })
      .finally(() => setLoading(false));
  }, [slug, activeKind, statusFilter]);

  useEffect(() => { reload(); }, [reload]);

  // Live updates via WebSocket (no polling).
  useWebSocket({
    'surveillance:event': (data) => {
      if (!data || !data.slug || data.slug === slug) reload();
    },
  });

  // Group findings by category for the active kind.
  const grouped = useMemo(() => {
    const order = Object.keys(CATEGORIES[activeKind] || { autres: 1 });
    const byCat = {};
    for (const f of findings) {
      const c = f.category || 'autres';
      (byCat[c] ||= []).push(f);
    }
    return order
      .filter((c) => byCat[c]?.length)
      .map((c) => ({ cat: c, items: byCat[c] }))
      // include any unexpected categories at the end
      .concat(
        Object.keys(byCat)
          .filter((c) => !order.includes(c))
          .map((c) => ({ cat: c, items: byCat[c] })),
      );
  }, [findings, activeKind]);

  const handleRun = async () => {
    setRunning(kindMeta.runKind);
    try {
      await runSurveillance(slug, kindMeta.runKind);
      await reload();
    } catch (e) {
      alert(e.response?.status === 501 ? 'Runner Codex non implémenté.' : (e.response?.data?.error || e.message));
    } finally {
      setRunning(null);
    }
  };

  const handleDismiss = async (f) => {
    const reason = window.prompt('Raison du dismiss (optionnel) :', '');
    if (reason === null) return;
    try { await dismissFinding(slug, f.id, reason || undefined); reload(); }
    catch (e) { alert('Dismiss a échoué : ' + (e.response?.data?.error || e.message)); }
  };

  const handleResolve = async (f) => {
    if (!window.confirm(`Marquer "${f.title}" comme résolue ?`)) return;
    try { await resolveFinding(slug, f.id); reload(); }
    catch (e) { alert('Resolve a échoué : ' + (e.response?.data?.error || e.message)); }
  };

  return (
    <div className="h-full flex flex-col">
      {/* Kind segments */}
      <div className="px-4 pt-3 flex items-center gap-1 border-b border-gray-700/50">
        {KINDS.map((k) => {
          const Icon = k.icon;
          const active = k.id === activeKind;
          return (
            <button key={k.id} onClick={() => setActiveKind(k.id)}
              className={`px-3 py-1.5 text-[13px] rounded-t flex items-center gap-1.5 border-b-2 -mb-px ${active ? `${k.color} border-current font-medium` : 'text-gray-400 border-transparent hover:text-gray-200'}`}>
              <Icon className="w-3.5 h-3.5" /> {k.label}
            </button>
          );
        })}
      </div>

      {/* Action bar */}
      <div className="px-4 py-2 border-b border-gray-700 bg-gray-800/30 flex items-center gap-2 flex-wrap">
        <button onClick={handleRun} disabled={!!running} className={`px-2.5 py-1 text-xs border rounded flex items-center gap-1 disabled:opacity-50 ${kindMeta.btn}`}>
          {running === kindMeta.runKind ? <RefreshCw className="w-3 h-3 animate-spin" /> : <Play className="w-3 h-3" />}
          Lancer {kindMeta.label.toLowerCase()}
        </button>
        <div className="flex-1" />
        <div className="flex items-center gap-1 text-xs">
          {STATUSES.map((s) => (
            <button key={s.key} onClick={() => setStatusFilter(statusFilter === s.key ? null : s.key)} className={`px-2 py-0.5 rounded border transition ${statusFilter === s.key ? `${s.color} bg-gray-700 border-gray-600` : 'text-gray-400 border-gray-700 hover:text-white hover:border-gray-600'}`}>
              {s.label}
            </button>
          ))}
        </div>
        <button onClick={reload} disabled={loading} className="px-2 py-1 text-xs text-gray-300 hover:text-white border border-gray-700 hover:border-gray-600 rounded flex items-center gap-1 disabled:opacity-50">
          <RefreshCw className={`w-3 h-3 ${loading ? 'animate-spin' : ''}`} />
        </button>
        <button onClick={() => setShowConfig((v) => !v)} className={`px-2 py-1 text-xs border rounded flex items-center gap-1 ${showConfig ? 'text-amber-300 border-amber-500/40' : 'text-gray-300 border-gray-700 hover:text-white hover:border-gray-600'}`}>
          <SettingsIcon className="w-3 h-3" />
        </button>
      </div>

      {showConfig && (
        <div className="border-b border-gray-700 bg-gray-900/30">
          <ConfigPanel slug={slug} onClose={() => setShowConfig(false)} />
        </div>
      )}

      <div className="flex-1 overflow-y-auto flex">
        <div className="flex-1 p-4 space-y-4 min-w-0">
          {err && <div className="p-3 bg-red-900/30 border border-red-700/50 text-red-300 rounded text-sm">{err}</div>}
          {!err && findings.length === 0 && !loading && (
            <div className="text-center py-12 text-gray-500 text-sm">
              Aucune finding « {kindMeta.label} » pour ce statut. Lance un scan ci-dessus.
            </div>
          )}
          {grouped.map(({ cat, items }) => (
            <div key={cat} className="space-y-2">
              <div className="flex items-center gap-2">
                <span className={`text-xs font-semibold uppercase tracking-wider ${kindMeta.color}`}>{catLabel(activeKind, cat)}</span>
                <span className="text-xs text-gray-600">({items.length})</span>
                <div className="flex-1 h-px bg-gray-700/50" />
              </div>
              {items.map((f) => (
                <FindingCard key={f.id} finding={f} onDismiss={handleDismiss} onResolve={handleResolve} />
              ))}
            </div>
          ))}
        </div>

        <aside className="w-72 shrink-0 border-l border-gray-700 bg-gray-900/30 p-3 hidden lg:block">
          <div className="text-xs uppercase tracking-wider text-gray-500 mb-2">Runs récents</div>
          {runs.length === 0 ? (
            <div className="text-xs text-gray-600">Aucun run.</div>
          ) : (
            <div className="rounded border border-gray-700 bg-gray-800/30">
              {runs.map((r) => <RunRow key={r.id} run={r} />)}
            </div>
          )}
        </aside>
      </div>
    </div>
  );
}
