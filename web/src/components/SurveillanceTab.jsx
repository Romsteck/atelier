import { useState, useEffect, useCallback, useMemo, useRef } from 'react';
import {
  ShieldAlert, RefreshCw, Play, Square, Terminal, ChevronDown, ChevronRight,
  X, Check, AlertOctagon, Lightbulb, Clock, ShieldCheck,
} from 'lucide-react';
import {
  getAppFindings,
  dismissFinding,
  resolveFinding,
  runSurveillance,
  cancelSurveillanceRun,
  getSurveillanceTranscript,
  listSurveillanceRuns,
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

// Per-kind cap on open findings (mirror MAX_OPEN_FINDINGS in atelier-watcher).
// At/above this, a kind's scan is skipped server-side and its launch button is
// disabled here.
const MAX_OPEN_FINDINGS = 6;

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
    <div className="border border-gray-700 bg-gray-800/40 rounded-sm">
      <button onClick={() => setOpen((v) => !v)} className="w-full flex items-start gap-3 px-3 py-2 text-left hover:bg-gray-800/70">
        {open ? <ChevronDown className="w-4 h-4 text-gray-400 mt-0.5 shrink-0" /> : <ChevronRight className="w-4 h-4 text-gray-400 mt-0.5 shrink-0" />}
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2 flex-wrap">
            <span className={`text-xs px-1.5 py-0.5 rounded-sm border ${sev.bg} ${sev.color}`}>{sev.label}</span>
            <span className="text-sm text-white truncate">{finding.title}</span>
          </div>
          <div className="text-xs text-gray-500 mt-0.5">Vu il y a {timeSince(finding.last_seen)}</div>
        </div>
        {finding.status === 'open' && (
          <div className="flex gap-1 shrink-0">
            <button onClick={(e) => { e.stopPropagation(); onDismiss(finding); }} className="px-2 py-1 text-xs text-gray-300 hover:text-white hover:bg-gray-700 rounded-sm flex items-center gap-1"><X className="w-3 h-3" /> Dismiss</button>
            <button onClick={(e) => { e.stopPropagation(); onResolve(finding); }} className="px-2 py-1 text-xs text-emerald-300 hover:text-emerald-200 hover:bg-emerald-900/30 rounded-sm flex items-center gap-1"><Check className="w-3 h-3" /> Résolu</button>
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
    cancelled: 'text-orange-300',
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


export default function SurveillanceTab({ slug }) {
  const [activeKind, setActiveKind] = useState('code_review');
  const [findings, setFindings] = useState([]);
  const [runs, setRuns] = useState([]);
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false); // launch/stop request in flight
  const [transcript, setTranscript] = useState([]); // live Codex output (ephemeral)
  const [err, setErr] = useState(null);
  const [statusFilter, setStatusFilter] = useState('open');
  // Open findings count for the active kind, independent of statusFilter — drives
  // the launch-button cap. Refreshed on every reload (incl. WS-triggered).
  const [openCount, setOpenCount] = useState(0);

  const kindMeta = KINDS.find((k) => k.id === activeKind) || KINDS[0];

  // The in-progress run for the active kind (drives the launch/stop button).
  // `runs.kind` stores the run_kind (plural for suggestions), matching runKind.
  const activeRun = useMemo(
    () => runs.find((r) => r.kind === kindMeta.runKind && r.status === 'running'),
    [runs, kindMeta.runKind],
  );
  const activeRunId = activeRun?.id;

  const reload = useCallback(() => {
    setLoading(true);
    setErr(null);
    Promise.all([
      getAppFindings(slug, { kind: activeKind, status: statusFilter || undefined, limit: 300 }),
      listSurveillanceRuns(slug, { limit: 12 }),
      // Always fetch the open list for the active kind to drive the cap — the
      // main list above is filtered by statusFilter, so it can't be trusted.
      getAppFindings(slug, { kind: activeKind, status: 'open', limit: 50 }),
    ])
      .then(([f, r, o]) => {
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

  // Live updates via WebSocket (no polling).
  useWebSocket({
    'surveillance:event': (data) => {
      if (!data || !data.slug || data.slug === slug) reload();
    },
    'surveillance:transcript': (data) => {
      // Only the active run's lines (lines before activeRunId is known are
      // recovered via the buffer replay below).
      if (!data || data.slug !== slug || data.run_id !== activeRunId) return;
      setTranscript((prev) => mergeLines(prev, [data]));
    },
  });

  // The live console is tied to one running run. On (re)mount or whenever the
  // active run changes, replay the server-buffered transcript so far, then keep
  // appending live WS lines. Cleared when no run is active (panel disappears).
  useEffect(() => {
    setTranscript([]);
    if (!activeRunId) return;
    let cancelled = false;
    getSurveillanceTranscript(slug, activeRunId)
      .then((r) => { if (!cancelled) setTranscript((prev) => mergeLines(prev, r.data?.lines || [])); })
      .catch(() => {});
    return () => { cancelled = true; };
  }, [activeRunId, slug]);

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
    setBusy(true);
    setTranscript([]);
    try {
      await runSurveillance(slug, kindMeta.runKind);
      // The run is fire-and-forget server-side; the running row + WS events
      // drive the button state from here on.
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
    try { await dismissFinding(slug, f.id, reason || undefined); reload(); }
    catch (e) { alert('Dismiss a échoué : ' + (e.response?.data?.error || e.message)); }
  };

  const handleResolve = async (f) => {
    if (!window.confirm(`Marquer "${f.title}" comme résolue ?`)) return;
    try { await resolveFinding(slug, f.id); reload(); }
    catch (e) { alert('Resolve a échoué : ' + (e.response?.data?.error || e.message)); }
  };

  const atCap = openCount >= MAX_OPEN_FINDINGS;

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
        {activeRun ? (
          <button onClick={handleStop} disabled={busy} className="px-2.5 py-1 text-xs border rounded-sm flex items-center gap-1 disabled:opacity-50 bg-red-500/20 text-red-200 hover:bg-red-500/30 border-red-500/30">
            {busy ? <RefreshCw className="w-3 h-3 animate-spin" /> : <Square className="w-3 h-3" />}
            Arrêter {kindMeta.label.toLowerCase()}
          </button>
        ) : (
          <button onClick={handleRun} disabled={busy || atCap}
            title={atCap ? `${openCount} findings ouvertes (max ${MAX_OPEN_FINDINGS}) — résous-en avant de relancer` : `Lancer ${kindMeta.label.toLowerCase()}`}
            className={`px-2.5 py-1 text-xs border rounded-sm flex items-center gap-1 disabled:opacity-50 disabled:cursor-not-allowed ${kindMeta.btn}`}>
            {busy ? <RefreshCw className="w-3 h-3 animate-spin" /> : <Play className="w-3 h-3" />}
            Lancer {kindMeta.label.toLowerCase()}
          </button>
        )}
        <div className="flex-1" />
        <div className="flex items-center gap-1 text-xs">
          {STATUSES.map((s) => (
            <button key={s.key} onClick={() => setStatusFilter(statusFilter === s.key ? null : s.key)} className={`px-2 py-0.5 rounded-sm border transition ${statusFilter === s.key ? `${s.color} bg-gray-700 border-gray-600` : 'text-gray-400 border-gray-700 hover:text-white hover:border-gray-600'}`}>
              {s.label}
            </button>
          ))}
        </div>
        <button onClick={reload} disabled={loading} className="px-2 py-1 text-xs text-gray-300 hover:text-white border border-gray-700 hover:border-gray-600 rounded-sm flex items-center gap-1 disabled:opacity-50">
          <RefreshCw className={`w-3 h-3 ${loading ? 'animate-spin' : ''}`} />
        </button>
      </div>

      <div className="flex-1 overflow-y-auto flex">
        <div className="flex-1 p-4 space-y-4 min-w-0">
          {err && <div className="p-3 bg-red-900/30 border border-red-700/50 text-red-300 rounded-sm text-sm">{err}</div>}
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

        {(activeRun || transcript.length > 0) && (
          <LiveScanPanel
            lines={transcript}
            kindLabel={kindMeta.label.toLowerCase()}
            onStop={activeRun ? handleStop : undefined}
            stopping={busy}
          />
        )}

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
