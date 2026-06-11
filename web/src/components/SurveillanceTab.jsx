import { useState, useEffect, useCallback, useMemo, useRef } from 'react';
import {
  RefreshCw, Play, Square, Terminal, X, Check, Clock, FileText,
  ShieldCheck, AlertOctagon, Activity,
} from 'lucide-react';
import {
  getAppFindings,
  dismissFinding,
  resolveFinding,
  runSurveillance,
  cancelSurveillanceRun,
  getSurveillanceTranscript,
  listSurveillanceRuns,
  getScan,
} from '../api/client';
import MarkdownView from './docs/MarkdownView';
import useWebSocket from '../hooks/useWebSocket';

// Each app has THREE scans, discriminated by `kind`:
// - security / code_review: fixed platform scans (label/categories are constant).
// - business: agent-owned, defined as data via the `scan_set` MCP tool — its label
//   and categories come from /surveillance/scan.
const KINDS = [
  {
    id: 'security', label: 'Sécurité', Icon: ShieldCheck,
    color: 'text-fuchsia-300',
    btn: 'bg-fuchsia-500/20 text-fuchsia-200 hover:bg-fuchsia-500/30 border-fuchsia-500/30',
    cats: ['auth', 'injection', 'secrets', 'exposition', 'autres'],
    fixed: true,
  },
  {
    id: 'code_review', label: 'Qualité', Icon: AlertOctagon,
    color: 'text-red-300',
    btn: 'bg-red-500/20 text-red-200 hover:bg-red-500/30 border-red-500/30',
    cats: ['bug', 'architecture', 'performance', 'composants', 'gestion_erreurs', 'autres'],
    fixed: true,
  },
  {
    id: 'business', label: 'Business', Icon: Activity,
    color: 'text-emerald-300',
    btn: 'bg-emerald-500/20 text-emerald-200 hover:bg-emerald-500/30 border-emerald-500/30',
    cats: null, // from the app_scan row
    fixed: false,
  },
];
const kindMeta = (id) => KINDS.find((k) => k.id === id) || KINDS[0];

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

// Per-(app,kind) cap on open findings (mirror MAX_OPEN_FINDINGS in atelier-watcher).
// At/above this, the active kind's scan is skipped server-side and its launch
// button is disabled here.
const MAX_OPEN_FINDINGS = 6;

const sevMeta = (k) => SEVERITIES.find((s) => s.key === k) || SEVERITIES[3];
// Categories are agent-defined (snake_case) — humanize the key for display.
const catLabel = (cat) => (cat || 'autres').replace(/_/g, ' ');

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

// An issue row: title + présentation (summary) only. Clicking opens the side
// drawer with the full resolution-plan document (the annex).
function FindingCard({ finding, active, onSelect, onDismiss, onResolve }) {
  const sev = sevMeta(finding.severity);
  return (
    <div
      onClick={() => onSelect(finding)}
      className={`border rounded-sm px-3 py-2 cursor-pointer transition ${
        active ? 'border-gray-500 bg-gray-800/70' : 'border-gray-700 bg-gray-800/40 hover:bg-gray-800/70'
      }`}
    >
      <div className="flex items-start gap-2">
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2 flex-wrap">
            <span className={`text-xs px-1.5 py-0.5 rounded-sm border ${sev.bg} ${sev.color}`}>{sev.label}</span>
            <span className="text-sm text-gray-50 truncate">{finding.title}</span>
          </div>
          {finding.summary && (
            <div className="text-xs text-gray-400 mt-1 line-clamp-2">{finding.summary}</div>
          )}
          <div className="text-[11px] text-gray-600 mt-1 flex items-center gap-2">
            <span>Vu il y a {timeSince(finding.last_seen)}</span>
            {finding.plan && <span className="flex items-center gap-0.5 text-gray-500"><FileText className="w-3 h-3" /> plan</span>}
          </div>
        </div>
        {finding.status === 'open' && (
          <div className="flex gap-1 shrink-0">
            <button onClick={(e) => { e.stopPropagation(); onDismiss(finding); }} className="px-2 py-1 text-xs text-gray-300 hover:text-gray-50 hover:bg-gray-700 rounded-sm flex items-center gap-1"><X className="w-3 h-3" /> Dismiss</button>
            <button onClick={(e) => { e.stopPropagation(); onResolve(finding); }} className="px-2 py-1 text-xs text-emerald-300 hover:text-emerald-200 hover:bg-emerald-900/30 rounded-sm flex items-center gap-1"><Check className="w-3 h-3" /> Résolu</button>
          </div>
        )}
      </div>
    </div>
  );
}

// Side drawer: the resolution-plan document (annex) for the selected issue.
function AnnexDrawer({ finding, onClose, onDismiss, onResolve }) {
  const sev = sevMeta(finding.severity);
  return (
    <div className="w-[28rem] shrink-0 border-l border-gray-700 bg-gray-950/60 flex flex-col min-w-0">
      <div className="px-3 py-2 border-b border-gray-700 flex items-center gap-2">
        <FileText className="w-3.5 h-3.5 text-gray-300 shrink-0" />
        <span className="text-xs text-gray-300 flex-1 truncate">Annexe — Plan de résolution</span>
        <button onClick={onClose} className="text-gray-400 hover:text-gray-50" title="Fermer"><X className="w-4 h-4" /></button>
      </div>
      <div className="flex-1 overflow-y-auto p-3 space-y-3">
        <div className="flex items-center gap-2 flex-wrap">
          <span className={`text-xs px-1.5 py-0.5 rounded-sm border ${sev.bg} ${sev.color}`}>{sev.label}</span>
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

function RunRow({ run }) {
  const colorByStatus = {
    success: 'text-emerald-300', success_empty: 'text-gray-400',
    skipped: 'text-yellow-400', failed: 'text-red-400', running: 'text-blue-400',
    cancelled: 'text-orange-300',
  };
  const kindShort = { security: 'sécu', code_review: 'qual', business: 'biz' };
  return (
    <div className="flex items-center gap-2 text-xs px-2 py-1 border-b border-gray-700/30 last:border-b-0">
      <Clock className="w-3 h-3 text-gray-500 shrink-0" />
      <span className="text-gray-400 w-10 shrink-0">{kindShort[run.kind] || run.kind}</span>
      <span className={`${colorByStatus[run.status] || 'text-gray-300'} w-24 shrink-0`}>{run.status}</span>
      <span className="text-gray-400 flex-1 truncate">{run.skip_reason || run.error || `${run.findings_count} finding${run.findings_count > 1 ? 's' : ''}`}</span>
      <span className="text-gray-600 shrink-0">{timeSince(run.started_at)}</span>
    </div>
  );
}

// Merge transcript lines deduped by seq (a buffer replay + live WS lines can
// overlap) and kept ordered. Capped to the last 2000.
function mergeLines(prev, incoming) {
  const bySeq = new Map(prev.map((l) => [l.seq, l]));
  for (const l of incoming) bySeq.set(l.seq, l);
  return [...bySeq.values()].sort((a, b) => a.seq - b.seq).slice(-2000);
}

// Render one Codex JSONL event into a readable {icon, text, tone} entry, or
// null to skip pure-noise events. Falls back to the raw line if not JSON.
function formatCodexEvent(raw) {
  let ev;
  try { ev = JSON.parse(raw); } catch { return raw.trim() ? { icon: '', text: raw, tone: 'raw' } : null; }
  const t = ev.type;
  if (t === 'thread.started') return { icon: '▸', text: 'Session Codex démarrée', tone: 'meta' };
  if (t === 'turn.started' || t === 'item.started') return null;
  if (t === 'turn.completed') {
    const u = ev.usage || {};
    return { icon: '✓', text: `Tour terminé — ${u.input_tokens ?? '?'} in / ${u.output_tokens ?? '?'} out tokens`, tone: 'meta' };
  }
  if (t === 'turn.failed' || t === 'error') {
    return { icon: '⚠', text: ev.error?.message || ev.message || JSON.stringify(ev), tone: 'err' };
  }
  if (t === 'item.completed') {
    const it = ev.item || {};
    switch (it.type) {
      case 'agent_message': return { icon: '🗨', text: it.text || '', tone: 'msg' };
      case 'reasoning': return { icon: '💭', text: it.text || it.summary || '', tone: 'dim' };
      case 'command_execution': return { icon: '$', text: it.command || it.cmd || '', tone: 'cmd' };
      case 'mcp_tool_call':
      case 'tool_call': return { icon: '🔧', text: it.tool || it.name || it.server || 'appel outil', tone: 'tool' };
      case 'file_change': return { icon: '✏', text: it.path || '', tone: 'tool' };
      default: return { icon: '·', text: it.text || it.type || '', tone: 'dim' };
    }
  }
  return null;
}

const TONE_CLS = {
  msg: 'text-gray-100', meta: 'text-emerald-400/80', dim: 'text-gray-500',
  cmd: 'text-sky-300', tool: 'text-fuchsia-300', err: 'text-red-300', raw: 'text-gray-400',
};

// Live console of the Codex run in progress. Lines stream in over WebSocket;
// the panel auto-scrolls and disappears once the run settles.
function LiveScanPanel({ lines, kindLabel, onStop, stopping }) {
  const bodyRef = useRef(null);
  const entries = useMemo(
    () => lines.map((l) => formatCodexEvent(l.line)).filter((e) => e && (e.text?.trim() || e.tone === 'meta')),
    [lines],
  );
  useEffect(() => {
    const el = bodyRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [entries.length]);
  return (
    <div className="w-96 shrink-0 border-l border-gray-700 bg-gray-950/60 flex flex-col min-w-0">
      <div className="px-3 py-2 border-b border-gray-700 flex items-center gap-2">
        <Terminal className="w-3.5 h-3.5 text-emerald-300 shrink-0" />
        <span className="text-xs text-gray-300 flex-1 truncate">
          {stopping ? 'Arrêt en cours…' : <>Scan en cours — <span className="text-emerald-300">{kindLabel}</span></>}
        </span>
        <RefreshCw className={`w-3 h-3 shrink-0 animate-spin ${stopping ? 'text-red-400' : 'text-emerald-400'}`} />
        {onStop && (
          <button
            onClick={onStop}
            disabled={stopping}
            className="px-2 py-0.5 text-xs border border-red-500/40 text-red-200 hover:bg-red-500/20 rounded-sm flex items-center gap-1 disabled:opacity-50"
          >
            <Square className="w-3 h-3" /> Arrêter
          </button>
        )}
      </div>
      <div ref={bodyRef} className="flex-1 overflow-y-auto p-2 space-y-1.5">
        {entries.length === 0 ? (
          <div className="text-xs text-gray-600 italic">En attente de la sortie de Codex…</div>
        ) : (
          entries.map((e, i) => (
            <div key={i} className="flex gap-1.5 text-[11px] leading-relaxed font-mono">
              {e.icon && <span className="shrink-0 select-none">{e.icon}</span>}
              <span className={`whitespace-pre-wrap wrap-break-word min-w-0 ${TONE_CLS[e.tone] || 'text-gray-300'}`}>{e.text}</span>
            </div>
          ))
        )}
      </div>
    </div>
  );
}

const VALID_KINDS = ['security', 'code_review', 'business'];

export default function SurveillanceTab({ slug, initialKind }) {
  const [activeKind, setActiveKind] = useState(
    VALID_KINDS.includes(initialKind) ? initialKind : 'security'
  );

  // Deep-link hint (?kind= from the global dashboard) — also applies when the
  // Studio is already mounted. Manual tab clicks afterwards take precedence.
  useEffect(() => {
    if (VALID_KINDS.includes(initialKind)) setActiveKind(initialKind);
  }, [initialKind]);
  const [scan, setScan] = useState(null); // the BUSINESS scan definition
  const [blank, setBlank] = useState(true);
  const [showDef, setShowDef] = useState(false); // business definition panel toggle
  const [findings, setFindings] = useState([]);
  const [runs, setRuns] = useState([]);
  const [selected, setSelected] = useState(null); // finding shown in the annex drawer
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false); // launch/stop request in flight
  const [transcript, setTranscript] = useState([]); // live Codex output (ephemeral)
  const [err, setErr] = useState(null);
  const [statusFilter, setStatusFilter] = useState('open');
  // Open findings count for the ACTIVE kind, independent of statusFilter — drives
  // that kind's launch-button cap.
  const [openCount, setOpenCount] = useState(0);

  const meta = kindMeta(activeKind);
  const isBusiness = activeKind === 'business';
  // Business shows the agent-given label; the two platform scans use fixed labels.
  const headerLabel = isBusiness
    ? ((scan?.label && scan.label.trim()) || (blank ? 'Business (en veille)' : 'Business'))
    : meta.label;

  // The in-progress run of the ACTIVE kind drives the launch/stop button.
  const activeRun = useMemo(
    () => runs.find((r) => r.kind === activeKind && r.status === 'running'),
    [runs, activeKind],
  );
  const activeRunId = activeRun?.id;

  const reload = useCallback(() => {
    setLoading(true);
    setErr(null);
    Promise.all([
      getScan(slug),
      getAppFindings(slug, { kind: activeKind, status: statusFilter || undefined, limit: 300 }),
      listSurveillanceRuns(slug, { limit: 15 }),
      getAppFindings(slug, { kind: activeKind, status: 'open', limit: 50 }),
    ])
      .then(([s, f, r, o]) => {
        setScan(s.data?.scan || null);
        setBlank(s.data?.blank ?? true);
        setFindings(f.data?.findings || []);
        setRuns(r.data?.runs || []);
        setOpenCount((o.data?.findings || []).length);
      })
      .catch((e) => {
        if (e.response?.status === 503) setErr('Surveillance désactivée (Postgres injoignable).');
        else setErr(e.response?.data?.error || e.message);
      })
      .finally(() => setLoading(false));
  }, [slug, activeKind, statusFilter]);

  useEffect(() => { reload(); }, [reload]);

  // Switching kind (or app) closes the annex drawer and clears the live console.
  useEffect(() => { setSelected(null); setTranscript([]); }, [activeKind, slug]);

  // Live updates via WebSocket (no polling).
  useWebSocket({
    'surveillance:event': (data) => {
      if (!data || !data.slug || data.slug === slug) reload();
    },
    'surveillance:transcript': (data) => {
      if (!data || data.slug !== slug || data.run_id !== activeRunId) return;
      setTranscript((prev) => mergeLines(prev, [data]));
    },
  });

  // The live console is tied to the active kind's running run. On change, replay
  // the server-buffered transcript so far, then keep appending live WS lines.
  useEffect(() => {
    setTranscript([]);
    if (!activeRunId) return;
    let cancelled = false;
    getSurveillanceTranscript(slug, activeRunId)
      .then((r) => { if (!cancelled) setTranscript((prev) => mergeLines(prev, r.data?.lines || [])); })
      .catch(() => {});
    return () => { cancelled = true; };
  }, [activeRunId, slug]);

  // Keep the drawer in sync with reloaded findings (close if the issue is gone).
  useEffect(() => {
    if (!selected) return;
    const fresh = findings.find((f) => f.id === selected.id);
    if (fresh) setSelected(fresh);
  }, [findings]); // eslint-disable-line react-hooks/exhaustive-deps

  // Group findings by category, ordered by the kind's declared categories.
  const grouped = useMemo(() => {
    const order = (isBusiness ? scan?.categories : meta.cats) || ['autres'];
    const byCat = {};
    for (const f of findings) {
      const c = f.category || 'autres';
      (byCat[c] ||= []).push(f);
    }
    return order
      .filter((c) => byCat[c]?.length)
      .map((c) => ({ cat: c, items: byCat[c] }))
      .concat(
        Object.keys(byCat)
          .filter((c) => !order.includes(c))
          .map((c) => ({ cat: c, items: byCat[c] })),
      );
  }, [findings, scan, isBusiness, meta]);

  const handleRun = async () => {
    setBusy(true);
    setTranscript([]);
    try {
      await runSurveillance(slug, activeKind);
      await reload();
    } catch (e) {
      alert(e.response?.status === 501 ? 'Runner Codex non implémenté.' : (e.response?.data?.error || e.message));
    } finally {
      setBusy(false);
    }
  };

  const handleStop = async () => {
    if (!activeRun) return;
    setBusy(true);
    try {
      await cancelSurveillanceRun(slug, activeRun.id);
      await reload();
    } catch (e) {
      alert('Arrêt a échoué : ' + (e.response?.data?.error || e.message));
    } finally {
      setBusy(false);
    }
  };

  const handleDismiss = async (f) => {
    const reason = window.prompt('Raison du dismiss (optionnel) :', '');
    if (reason === null) return;
    try { await dismissFinding(slug, f.id, reason || undefined); setSelected(null); reload(); }
    catch (e) { alert('Dismiss a échoué : ' + (e.response?.data?.error || e.message)); }
  };

  const handleResolve = async (f) => {
    if (!window.confirm(`Marquer "${f.title}" comme résolue ?`)) return;
    try { await resolveFinding(slug, f.id); setSelected(null); reload(); }
    catch (e) { alert('Resolve a échoué : ' + (e.response?.data?.error || e.message)); }
  };

  const atCap = openCount >= MAX_OPEN_FINDINGS;
  const blankBusiness = isBusiness && blank;

  return (
    <div className="h-full flex flex-col">
      {/* Kind selector — the app's three scans */}
      <div className="px-4 pt-3 pb-0 flex items-end gap-1 border-b border-gray-700/50">
        {KINDS.map((k) => {
          const on = k.id === activeKind;
          const label = k.id === 'business'
            ? ((scan?.label && scan.label.trim()) || 'Business')
            : k.label;
          return (
            <button
              key={k.id}
              onClick={() => setActiveKind(k.id)}
              className={`px-3 py-1.5 text-sm rounded-t-sm border-b-2 flex items-center gap-1.5 transition ${
                on ? `${k.color} border-current bg-gray-800/50` : 'text-gray-400 border-transparent hover:text-gray-200'
              }`}
            >
              <k.Icon className="w-4 h-4" />
              {label}
              {k.id === 'business' && blank && <span className="text-[10px] text-gray-500">(veille)</span>}
            </button>
          );
        })}
      </div>

      {/* Business: read-only definition panel (the agent edits it via scan_set) */}
      {isBusiness && (
        <div className="px-4 pt-2 pb-1 flex items-center gap-2 border-b border-gray-700/40 text-xs">
          {scan && !blank && <span className="text-gray-500">{scan.cadence} · gate {scan.gate}</span>}
          <button onClick={() => setShowDef((v) => !v)} className="text-gray-400 hover:text-gray-200 underline decoration-dotted">
            {showDef ? 'masquer la définition' : 'voir la définition'}
          </button>
        </div>
      )}
      {isBusiness && showDef && (
        <div className="px-4 py-2 border-b border-gray-700 bg-gray-900/40 text-xs text-gray-300 space-y-1">
          {blank ? (
            <div className="text-gray-500">Aucun scan Business défini. L'agent du projet le crée/maintient via le tool MCP <code className="text-gray-300">scan_set</code> (cf. <code className="text-gray-300">.claude/rules/surveillance.md</code>).</div>
          ) : (
            <>
              <div><span className="text-gray-500">catégories :</span> {(scan.categories || []).join(', ') || '—'}</div>
              {scan.gate === 'data' && scan.gate_sql && (
                <div className="truncate"><span className="text-gray-500">gate_sql :</span> <code>{scan.gate_sql}</code></div>
              )}
              {scan.updated_by && <div className="text-gray-500">maintenu par {scan.updated_by}</div>}
              <pre className="mt-1 max-h-48 overflow-y-auto whitespace-pre-wrap bg-black/30 p-2 rounded-sm border border-gray-800 text-gray-400">{scan.prompt}</pre>
            </>
          )}
        </div>
      )}

      {/* Action bar */}
      <div className="px-4 py-2 border-b border-gray-700 bg-gray-800/30 flex items-center gap-2 flex-wrap">
        {activeRun ? (
          <button onClick={handleStop} disabled={busy} className="px-2.5 py-1 text-xs border rounded-sm flex items-center gap-1 disabled:opacity-50 bg-red-500/20 text-red-200 hover:bg-red-500/30 border-red-500/30">
            {busy ? <RefreshCw className="w-3 h-3 animate-spin" /> : <Square className="w-3 h-3" />}
            Arrêter le scan
          </button>
        ) : (
          <button onClick={handleRun} disabled={busy || atCap || blankBusiness}
            title={blankBusiness ? 'Scan Business en veille — défini par l\'agent du projet' : atCap ? `${openCount} findings ouvertes (max ${MAX_OPEN_FINDINGS}) — résous-en avant de relancer` : `Lancer le scan ${meta.label}`}
            className={`px-2.5 py-1 text-xs border rounded-sm flex items-center gap-1 disabled:opacity-50 disabled:cursor-not-allowed ${meta.btn}`}>
            {busy ? <RefreshCw className="w-3 h-3 animate-spin" /> : <Play className="w-3 h-3" />}
            Lancer {meta.label}
          </button>
        )}
        <div className="flex-1" />
        <div className="flex items-center gap-1 text-xs">
          {STATUSES.map((s) => (
            <button key={s.key} onClick={() => setStatusFilter(statusFilter === s.key ? null : s.key)} className={`px-2 py-0.5 rounded-sm border transition ${statusFilter === s.key ? `${s.color} bg-gray-700 border-gray-600` : 'text-gray-400 border-gray-700 hover:text-gray-50 hover:border-gray-600'}`}>
              {s.label}
            </button>
          ))}
        </div>
        <button onClick={reload} disabled={loading} className="px-2 py-1 text-xs text-gray-300 hover:text-gray-50 border border-gray-700 hover:border-gray-600 rounded-sm flex items-center gap-1 disabled:opacity-50">
          <RefreshCw className={`w-3 h-3 ${loading ? 'animate-spin' : ''}`} />
        </button>
      </div>

      <div className="flex-1 overflow-y-auto flex">
        <div className="flex-1 p-4 space-y-4 min-w-0">
          {err && <div className="p-3 bg-red-900/30 border border-red-700/50 text-red-300 rounded-sm text-sm">{err}</div>}
          {!err && findings.length === 0 && !loading && (
            <div className="text-center py-12 text-gray-500 text-sm">
              {blankBusiness ? 'Scan Business en veille — il sera défini par l\'agent du projet.' : `Aucune finding ${meta.label} pour ce statut. Lance le scan ci-dessus.`}
            </div>
          )}
          {grouped.map(({ cat, items }) => (
            <div key={cat} className="space-y-2">
              <div className="flex items-center gap-2">
                <span className={`text-xs font-semibold uppercase tracking-wider ${meta.color}`}>{catLabel(cat)}</span>
                <span className="text-xs text-gray-600">({items.length})</span>
                <div className="flex-1 h-px bg-gray-700/50" />
              </div>
              {items.map((f) => (
                <FindingCard key={f.id} finding={f} active={selected?.id === f.id} onSelect={setSelected} onDismiss={handleDismiss} onResolve={handleResolve} />
              ))}
            </div>
          ))}
        </div>

        {/* Right side: the annex drawer (selected issue) takes priority; otherwise
            the live Codex console while a run is in progress. */}
        {selected ? (
          <AnnexDrawer finding={selected} onClose={() => setSelected(null)} onDismiss={handleDismiss} onResolve={handleResolve} />
        ) : (activeRun || transcript.length > 0) ? (
          <LiveScanPanel
            lines={transcript}
            kindLabel={headerLabel.toLowerCase()}
            onStop={activeRun ? handleStop : undefined}
            stopping={busy}
          />
        ) : null}

        <aside className="w-72 shrink-0 border-l border-gray-700 bg-gray-900/30 p-3 hidden lg:block">
          <div className="text-xs uppercase tracking-wider text-gray-500 mb-2">Runs récents</div>
          {runs.length === 0 ? (
            <div className="text-xs text-gray-600">Aucun run.</div>
          ) : (
            <div className="rounded-sm border border-gray-700 bg-gray-800/30">
              {runs.map((r) => <RunRow key={r.id} run={r} />)}
            </div>
          )}
        </aside>
      </div>
    </div>
  );
}
