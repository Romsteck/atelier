import { useState, useEffect, useCallback, useMemo } from 'react';
import { ShieldAlert, RefreshCw, Filter, ChevronDown, ChevronRight, X, Check, AlertOctagon, Lightbulb, ShieldCheck } from 'lucide-react';
import {
  getFindings,
  dismissFinding,
  resolveFinding,
} from '../api/client';
import MarkdownView from '../components/docs/MarkdownView';
import PageHeader from '../components/PageHeader';
import useWebSocket from '../hooks/useWebSocket';

const SEVERITIES = [
  { key: 'critical', label: 'Critical', color: 'text-red-300', bg: 'bg-red-500/20 border-red-500/30' },
  { key: 'high',     label: 'High',     color: 'text-orange-300', bg: 'bg-orange-500/20 border-orange-500/30' },
  { key: 'medium',   label: 'Medium',   color: 'text-yellow-300', bg: 'bg-yellow-500/20 border-yellow-500/30' },
  { key: 'low',      label: 'Low',      color: 'text-blue-300', bg: 'bg-blue-500/20 border-blue-500/30' },
];

const KINDS = [
  { key: 'code_review', label: 'Code Review', icon: AlertOctagon, color: 'text-red-300' },
  { key: 'security',    label: 'Sécurité',    icon: ShieldCheck,  color: 'text-fuchsia-300' },
  { key: 'suggestion',  label: 'Suggestion',  icon: Lightbulb,    color: 'text-blue-300' },
];

// Category labels per kind (mirror RunKind::categories / SurveillanceTab).
const CATEGORIES = {
  code_review: { bug: 'Bug / logique', architecture: 'Architecture', performance: 'Performance', composants: 'Composants', gestion_erreurs: "Gestion d'erreurs", autres: 'Autres' },
  suggestion: { performance: 'Performance', ux: 'UX / ergonomie', autres: 'Autres' },
  security: { auth: 'Auth', injection: 'Injection', secrets: 'Secrets', exposition: 'Exposition', autres: 'Autres' },
};
const catLabel = (kind, cat) => (CATEGORIES[kind] && CATEGORIES[kind][cat]) || cat || 'autres';

const STATUSES = [
  { key: 'open',      label: 'Ouvertes',  color: 'text-amber-300' },
  { key: 'dismissed', label: 'Dismiss',   color: 'text-gray-400' },
  { key: 'resolved',  label: 'Résolues',  color: 'text-emerald-300' },
];

function severityMeta(key) {
  return SEVERITIES.find((s) => s.key === key) || SEVERITIES[3];
}
function kindMeta(key) {
  return KINDS.find((k) => k.key === key) || KINDS[0];
}
function statusMeta(key) {
  return STATUSES.find((s) => s.key === key) || STATUSES[0];
}

function FindingRow({ finding, onDismiss, onResolve }) {
  const [open, setOpen] = useState(false);
  const sev = severityMeta(finding.severity);
  const kind = kindMeta(finding.kind);
  const status = statusMeta(finding.status);
  const KindIcon = kind.icon;
  return (
    <div className="border border-gray-700 bg-gray-800/40 rounded-sm">
      <button
        onClick={() => setOpen((v) => !v)}
        className="w-full flex items-start gap-3 px-3 py-2 text-left hover:bg-gray-800/70"
      >
        {open ? <ChevronDown className="w-4 h-4 text-gray-400 mt-0.5 shrink-0" /> : <ChevronRight className="w-4 h-4 text-gray-400 mt-0.5 shrink-0" />}
        <KindIcon className={`w-4 h-4 ${kind.color} mt-0.5 shrink-0`} />
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2 flex-wrap">
            <span className={`text-xs px-1.5 py-0.5 rounded-sm border ${sev.bg} ${sev.color}`}>
              {sev.label}
            </span>
            <span className="text-xs text-gray-400">{finding.slug}</span>
            <span className="text-xs px-1.5 py-0.5 rounded-sm bg-gray-700/60 text-gray-300">{catLabel(finding.kind, finding.category)}</span>
            <span className={`text-xs ${status.color}`}>{status.label}</span>
            <span className="text-sm text-white truncate">{finding.title}</span>
          </div>
          <div className="text-xs text-gray-500 mt-0.5">
            Vu il y a {timeSince(finding.last_seen)}
          </div>
        </div>
        {finding.status === 'open' && (
          <div className="flex gap-1 shrink-0">
            <button
              onClick={(e) => { e.stopPropagation(); onDismiss(finding); }}
              className="px-2 py-1 text-xs text-gray-300 hover:text-white hover:bg-gray-700 rounded-sm flex items-center gap-1"
              title="Dismiss (faux positif)"
            >
              <X className="w-3 h-3" /> Dismiss
            </button>
            <button
              onClick={(e) => { e.stopPropagation(); onResolve(finding); }}
              className="px-2 py-1 text-xs text-emerald-300 hover:text-emerald-200 hover:bg-emerald-900/30 rounded-sm flex items-center gap-1"
              title="Marquer comme résolu manuellement"
            >
              <Check className="w-3 h-3" /> Résolu
            </button>
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
          {finding.evidence && (
            <details className="text-xs">
              <summary className="cursor-pointer text-gray-400 hover:text-white">Evidence</summary>
              <pre className="mt-2 p-2 bg-gray-900 border border-gray-700 rounded-sm overflow-auto text-xs text-gray-300">
                {JSON.stringify(finding.evidence, null, 2)}
              </pre>
            </details>
          )}
          <div className="text-xs text-gray-500">
            Fingerprint: <code className="text-gray-400">{finding.fingerprint}</code>
            {' · '} ID: {finding.id}
          </div>
        </div>
      )}
    </div>
  );
}

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

function FilterPill({ active, onClick, children, color }) {
  return (
    <button
      onClick={onClick}
      className={`px-2 py-0.5 text-xs rounded-sm border transition ${
        active
          ? `${color} bg-gray-700 border-gray-600`
          : 'text-gray-400 border-gray-700 hover:text-white hover:border-gray-600'
      }`}
    >
      {children}
    </button>
  );
}

export default function Surveillance() {
  const [findings, setFindings] = useState([]);
  const [loading, setLoading] = useState(false);
  const [err, setErr] = useState(null);
  const [filterSlug, setFilterSlug] = useState(null);
  const [filterKind, setFilterKind] = useState(null);
  const [filterSeverity, setFilterSeverity] = useState(null);
  const [filterStatus, setFilterStatus] = useState('open');

  const reload = useCallback(() => {
    setLoading(true);
    setErr(null);
    getFindings({
      slug: filterSlug || undefined,
      kind: filterKind || undefined,
      severity: filterSeverity || undefined,
      status: filterStatus || undefined,
      limit: 300,
    })
      .then((res) => {
        setFindings(res.data?.findings || []);
      })
      .catch((e) => {
        if (e.response?.status === 503) {
          setErr('Surveillance désactivée (Postgres injoignable au boot).');
        } else {
          setErr(e.response?.data?.error || e.message);
        }
      })
      .finally(() => setLoading(false));
  }, [filterSlug, filterKind, filterSeverity, filterStatus]);

  useEffect(() => { reload(); }, [reload]);

  // Live updates via WebSocket: reload the global findings list on any
  // finding/run event across apps.
  useWebSocket({
    'surveillance:event': () => reload(),
  });

  const slugs = useMemo(() => {
    const set = new Set(findings.map((f) => f.slug));
    return Array.from(set).sort();
  }, [findings]);

  const handleDismiss = async (f) => {
    const reason = window.prompt('Raison du dismiss (optionnel) :', '');
    if (reason === null) return;
    try {
      await dismissFinding(f.slug, f.id, reason || undefined);
      reload();
    } catch (e) {
      alert('Dismiss a échoué : ' + (e.response?.data?.error || e.message));
    }
  };

  const handleResolve = async (f) => {
    if (!window.confirm(`Marquer la finding "${f.title}" comme résolue ?`)) return;
    try {
      await resolveFinding(f.slug, f.id);
      reload();
    } catch (e) {
      alert('Resolve a échoué : ' + (e.response?.data?.error || e.message));
    }
  };

  return (
    <div className="h-full flex flex-col">
      <PageHeader title="Surveillance IA" icon={ShieldAlert}>
        <button
          onClick={reload}
          disabled={loading}
          className="px-2 py-1 text-xs text-gray-300 hover:text-white border border-gray-700 hover:border-gray-600 rounded-sm flex items-center gap-1 disabled:opacity-50"
        >
          <RefreshCw className={`w-3 h-3 ${loading ? 'animate-spin' : ''}`} />
          Actualiser
        </button>
      </PageHeader>

      <div className="px-4 py-3 border-b border-gray-700 bg-gray-800/30 flex flex-wrap items-center gap-x-4 gap-y-2">
        <div className="flex items-center gap-1">
          <Filter className="w-3 h-3 text-gray-500" />
          <span className="text-xs text-gray-500 mr-1">Statut</span>
          {STATUSES.map((s) => (
            <FilterPill
              key={s.key}
              active={filterStatus === s.key}
              onClick={() => setFilterStatus(filterStatus === s.key ? null : s.key)}
              color={s.color}
            >
              {s.label}
            </FilterPill>
          ))}
        </div>
        <div className="flex items-center gap-1">
          <span className="text-xs text-gray-500 mr-1">Kind</span>
          {KINDS.map((k) => (
            <FilterPill
              key={k.key}
              active={filterKind === k.key}
              onClick={() => setFilterKind(filterKind === k.key ? null : k.key)}
              color={k.color}
            >
              {k.label}
            </FilterPill>
          ))}
        </div>
        <div className="flex items-center gap-1">
          <span className="text-xs text-gray-500 mr-1">Sévérité</span>
          {SEVERITIES.map((s) => (
            <FilterPill
              key={s.key}
              active={filterSeverity === s.key}
              onClick={() => setFilterSeverity(filterSeverity === s.key ? null : s.key)}
              color={s.color}
            >
              {s.label}
            </FilterPill>
          ))}
        </div>
        {slugs.length > 0 && (
          <div className="flex items-center gap-1">
            <span className="text-xs text-gray-500 mr-1">App</span>
            {slugs.map((s) => (
              <FilterPill
                key={s}
                active={filterSlug === s}
                onClick={() => setFilterSlug(filterSlug === s ? null : s)}
                color="text-blue-300"
              >
                {s}
              </FilterPill>
            ))}
          </div>
        )}
      </div>

      <div className="flex-1 overflow-y-auto p-4 space-y-2">
        {err && (
          <div className="p-3 bg-red-900/30 border border-red-700/50 text-red-300 rounded-sm text-sm">
            {err}
          </div>
        )}
        {!err && findings.length === 0 && !loading && (
          <div className="text-center py-12 text-gray-500 text-sm">
            Aucune finding. Lance un run depuis le tab Surveillance d'une app.
          </div>
        )}
        {findings.map((f) => (
          <FindingRow
            key={f.id}
            finding={f}
            onDismiss={handleDismiss}
            onResolve={handleResolve}
          />
        ))}
      </div>
    </div>
  );
}
