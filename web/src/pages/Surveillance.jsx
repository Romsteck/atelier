import { useState, useEffect, useCallback, useMemo } from 'react';
import { ShieldAlert, RefreshCw, Filter, X, Check, FileText, ShieldCheck, AlertOctagon, Activity } from 'lucide-react';
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

// The three scans every app has. Findings carry agent-defined, snake_case
// categories — humanize the key for display.
const KINDS = [
  { key: 'security',    label: 'Sécurité', icon: ShieldCheck,   color: 'text-fuchsia-300' },
  { key: 'code_review', label: 'Qualité',  icon: AlertOctagon,  color: 'text-red-300' },
  { key: 'business',    label: 'Business', icon: Activity,      color: 'text-emerald-300' },
];
const catLabel = (cat) => (cat || 'autres').replace(/_/g, ' ');

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

// An issue row: title + présentation (summary) only. Clicking opens the side
// drawer with the full resolution-plan document.
function FindingRow({ finding, active, onSelect }) {
  const sev = severityMeta(finding.severity);
  const kind = kindMeta(finding.kind);
  const status = statusMeta(finding.status);
  const KindIcon = kind.icon;
  return (
    <div
      onClick={() => onSelect(finding)}
      className={`border rounded-sm px-3 py-2 cursor-pointer transition flex items-start gap-3 ${
        active ? 'border-gray-500 bg-gray-800/70' : 'border-gray-700 bg-gray-800/40 hover:bg-gray-800/70'
      }`}
    >
      <KindIcon className={`w-4 h-4 ${kind.color} mt-0.5 shrink-0`} />
      <div className="flex-1 min-w-0">
        <div className="flex items-center gap-2 flex-wrap">
          <span className={`text-xs px-1.5 py-0.5 rounded-sm border ${sev.bg} ${sev.color}`}>{sev.label}</span>
          <span className="text-xs text-gray-400">{finding.slug}</span>
          <span className="text-xs px-1.5 py-0.5 rounded-sm bg-gray-700/60 text-gray-300">{catLabel(finding.category)}</span>
          <span className={`text-xs ${status.color}`}>{status.label}</span>
          <span className="text-sm text-gray-50 truncate">{finding.title}</span>
        </div>
        {finding.summary && <div className="text-xs text-gray-400 mt-1 line-clamp-2">{finding.summary}</div>}
        <div className="text-[11px] text-gray-600 mt-1 flex items-center gap-2">
          <span>Vu il y a {timeSince(finding.last_seen)}</span>
          {finding.plan && <span className="flex items-center gap-0.5 text-gray-500"><FileText className="w-3 h-3" /> plan</span>}
        </div>
      </div>
    </div>
  );
}

// Side drawer: the resolution-plan document (annex) for the selected issue.
function AnnexDrawer({ finding, onClose, onDismiss, onResolve }) {
  const sev = severityMeta(finding.severity);
  const kind = kindMeta(finding.kind);
  return (
    <div className="w-[30rem] shrink-0 border-l border-gray-700 bg-gray-950/60 flex flex-col min-w-0">
      <div className="px-3 py-2 border-b border-gray-700 flex items-center gap-2">
        <FileText className="w-3.5 h-3.5 text-gray-300 shrink-0" />
        <span className="text-xs text-gray-300 flex-1 truncate">Annexe — Plan de résolution</span>
        <button onClick={onClose} className="text-gray-400 hover:text-gray-50" title="Fermer"><X className="w-4 h-4" /></button>
      </div>
      <div className="flex-1 overflow-y-auto p-3 space-y-3">
        <div className="flex items-center gap-2 flex-wrap">
          <span className={`text-xs px-1.5 py-0.5 rounded-sm border ${sev.bg} ${sev.color}`}>{sev.label}</span>
          <span className={`text-xs ${kind.color}`}>{kind.label}</span>
          <span className="text-xs text-gray-400">{finding.slug}</span>
          <span className="text-xs px-1.5 py-0.5 rounded-sm bg-gray-700/60 text-gray-300">{catLabel(finding.category)}</span>
        </div>
        <div className="text-sm text-gray-50 font-medium">{finding.title}</div>
        <div>
          <div className="text-xs text-gray-500 mb-1">Présentation</div>
          <MarkdownView>{finding.summary}</MarkdownView>
        </div>
        <div>
          <div className="text-xs text-gray-500 mb-1">Plan de résolution</div>
          {finding.plan ? <MarkdownView>{finding.plan}</MarkdownView> : <div className="text-xs text-gray-600 italic">Aucun plan.</div>}
        </div>
        {finding.evidence && (
          <details className="text-xs">
            <summary className="cursor-pointer text-gray-400 hover:text-gray-50">Evidence</summary>
            <pre className="mt-2 p-2 bg-gray-900 border border-gray-700 rounded-sm overflow-auto text-xs text-gray-300">{JSON.stringify(finding.evidence, null, 2)}</pre>
          </details>
        )}
        <div className="text-[11px] text-gray-600">ID {finding.id} · <code className="text-gray-500">{finding.fingerprint}</code></div>
      </div>
      {finding.status === 'open' && (
        <div className="px-3 py-2 border-t border-gray-700 flex gap-2">
          <button onClick={() => onDismiss(finding)} className="flex-1 px-2 py-1 text-xs text-gray-300 hover:text-gray-50 border border-gray-700 hover:bg-gray-700 rounded-sm flex items-center justify-center gap-1"><X className="w-3 h-3" /> Dismiss</button>
          <button onClick={() => onResolve(finding)} className="flex-1 px-2 py-1 text-xs text-emerald-300 hover:text-emerald-200 border border-emerald-500/30 hover:bg-emerald-900/30 rounded-sm flex items-center justify-center gap-1"><Check className="w-3 h-3" /> Résolu</button>
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
          : 'text-gray-400 border-gray-700 hover:text-gray-50 hover:border-gray-600'
      }`}
    >
      {children}
    </button>
  );
}

export default function Surveillance() {
  const [findings, setFindings] = useState([]);
  const [selected, setSelected] = useState(null);
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

  // Keep the drawer in sync with reloaded findings (close if the issue is gone).
  useEffect(() => {
    if (!selected) return;
    const fresh = findings.find((f) => f.id === selected.id);
    if (fresh) setSelected(fresh);
  }, [findings]); // eslint-disable-line react-hooks/exhaustive-deps

  const slugs = useMemo(() => {
    const set = new Set(findings.map((f) => f.slug));
    return Array.from(set).sort();
  }, [findings]);

  const handleDismiss = async (f) => {
    const reason = window.prompt('Raison du dismiss (optionnel) :', '');
    if (reason === null) return;
    try {
      await dismissFinding(f.slug, f.id, reason || undefined);
      setSelected(null);
      reload();
    } catch (e) {
      alert('Dismiss a échoué : ' + (e.response?.data?.error || e.message));
    }
  };

  const handleResolve = async (f) => {
    if (!window.confirm(`Marquer la finding "${f.title}" comme résolue ?`)) return;
    try {
      await resolveFinding(f.slug, f.id);
      setSelected(null);
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
          className="px-2 py-1 text-xs text-gray-300 hover:text-gray-50 border border-gray-700 hover:border-gray-600 rounded-sm flex items-center gap-1 disabled:opacity-50"
        >
          <RefreshCw className={`w-3 h-3 ${loading ? 'animate-spin' : ''}`} />
          Actualiser
        </button>
      </PageHeader>

      <div className="px-4 py-3 border-b border-gray-700 bg-gray-800/30 flex flex-wrap items-center gap-x-4 gap-y-2">
        <div className="flex items-center gap-1">
          <Filter className="w-3 h-3 text-gray-500" />
          <span className="text-xs text-gray-500 mr-1">Scan</span>
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

      <div className="flex-1 overflow-hidden flex">
        <div className="flex-1 overflow-y-auto p-4 space-y-2 min-w-0">
          {err && (
            <div className="p-3 bg-red-900/30 border border-red-700/50 text-red-300 rounded-sm text-sm">
              {err}
            </div>
          )}
          {!err && findings.length === 0 && !loading && (
            <div className="text-center py-12 text-gray-500 text-sm">
              Aucune finding. Lance un scan depuis le tab Surveillance d'une app.
            </div>
          )}
          {findings.map((f) => (
            <FindingRow
              key={f.id}
              finding={f}
              active={selected?.id === f.id}
              onSelect={setSelected}
            />
          ))}
        </div>

        {selected && (
          <AnnexDrawer
            finding={selected}
            onClose={() => setSelected(null)}
            onDismiss={handleDismiss}
            onResolve={handleResolve}
          />
        )}
      </div>
    </div>
  );
}
