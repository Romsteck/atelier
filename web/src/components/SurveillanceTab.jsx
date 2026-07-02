import { useState, useEffect, useCallback, useMemo } from 'react';
import {
  RefreshCw, Play, Square, X, Clock, FileText, Wrench, Trash2,
  ShieldCheck, AlertOctagon, Activity,
} from 'lucide-react';
import {
  getAppFindings,
  runSurveillance,
  cancelSurveillanceRun,
  getSurveillanceTranscript,
  listSurveillanceRuns,
  getScan,
  deleteFinding,
} from '../api/client';
import MarkdownView from './docs/MarkdownView';
import LiveScanPanel from './surveillance/LiveScanPanel';
import { mergeLines } from './surveillance/scanFormat';
import useWebSocket from '../hooks/useWebSocket';
import { useOpenResolveScans } from '../lib/resolveConvos';
import { useToast, Toast } from '../hooks/useToast';
import { apiErr } from '../utils/apiErr';

// Each app has THREE scans, discriminated by `kind`:
// - security / code_review: fixed platform scans (label/categories are constant).
// - business: agent-owned, defined as data via the `scan_set` MCP tool — its label
//   and categories come from /surveillance/scan.
const KINDS = [
  {
    id: 'security', label: 'Sécurité', Icon: ShieldCheck,
    color: 'text-fuchsia-700 dark:text-fuchsia-300',
    btn: 'bg-fuchsia-500/20 text-fuchsia-700 dark:text-fuchsia-200 hover:bg-fuchsia-500/30 border-fuchsia-500/30',
    cats: ['auth', 'injection', 'secrets', 'exposition', 'autres'],
    fixed: true,
  },
  {
    id: 'code_review', label: 'Qualité', Icon: AlertOctagon,
    color: 'text-red-700 dark:text-red-300',
    btn: 'bg-red-500/20 text-red-700 dark:text-red-200 hover:bg-red-500/30 border-red-500/30',
    cats: ['bug', 'architecture', 'performance', 'composants', 'gestion_erreurs', 'autres'],
    fixed: true,
  },
  {
    id: 'business', label: 'Business', Icon: Activity,
    color: 'text-emerald-700 dark:text-emerald-300',
    btn: 'bg-emerald-500/20 text-emerald-700 dark:text-emerald-200 hover:bg-emerald-500/30 border-emerald-500/30',
    cats: null, // from the app_scan row
    fixed: false,
  },
];
const kindMeta = (id) => KINDS.find((k) => k.id === id) || KINDS[0];

const SEVERITIES = [
  { key: 'critical', label: 'Critical', color: 'text-red-700 dark:text-red-300', bg: 'bg-red-500/20 border-red-500/30' },
  { key: 'high', label: 'High', color: 'text-orange-700 dark:text-orange-300', bg: 'bg-orange-500/20 border-orange-500/30' },
  { key: 'medium', label: 'Medium', color: 'text-yellow-700 dark:text-yellow-300', bg: 'bg-yellow-500/20 border-yellow-500/30' },
  { key: 'low', label: 'Low', color: 'text-blue-700 dark:text-blue-300', bg: 'bg-blue-500/20 border-blue-500/30' },
];

const STATUSES = [
  { key: 'open', label: 'Ouvertes', color: 'text-amber-700 dark:text-amber-300' },
  { key: 'resolved', label: 'Résolues', color: 'text-emerald-700 dark:text-emerald-300' },
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
function FindingCard({ finding, active, onSelect }) {
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
      </div>
    </div>
  );
}

// Side drawer: the resolution-plan document (annex) for the selected issue.
function AnnexDrawer({ finding, onClose, onDelete }) {
  const sev = sevMeta(finding.severity);
  return (
    <div className="w-[28rem] shrink-0 border-l border-gray-700 bg-gray-950/60 flex flex-col min-w-0">
      <div className="px-3 py-2 border-b border-gray-700 flex items-center gap-2">
        <FileText className="w-3.5 h-3.5 text-gray-300 shrink-0" />
        <span className="text-xs text-gray-300 flex-1 truncate">Annexe — Plan de résolution</span>
        <button onClick={() => onDelete(finding)} className="text-gray-400 hover:text-red-600 dark:hover:text-red-400" title="Supprimer définitivement cette finding"><Trash2 className="w-4 h-4" /></button>
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
    </div>
  );
}

function RunRow({ run }) {
  const colorByStatus = {
    success: 'text-emerald-700 dark:text-emerald-300', success_empty: 'text-gray-400',
    skipped: 'text-yellow-700 dark:text-yellow-400', failed: 'text-red-700 dark:text-red-400',
    running: 'text-blue-700 dark:text-blue-400', cancelled: 'text-orange-700 dark:text-orange-300',
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

const VALID_KINDS = ['security', 'code_review', 'business'];

// Prompt préparé envoyé à l'agent quand on clique « Résoudre tout » sur un scan : TOUS les
// findings ouverts du kind dans une conversation unique, pour que l'agent concilie les
// diagnostics (causes racines communes, fixes qui se recouvrent) au lieu de les traiter en
// silo. Démarre en lecture seule (l'auto-envoi force le mode Plan) ; le déroulé en 4 phases
// impose l'orchestration : investigation → plan consolidé → exécution suivie → vérification
// qu'aucun finding ne reste ouvert.
function buildResolveAllPrompt(openFindings, slug, kind, kindLabel) {
  const n = openFindings.length;
  const blocks = openFindings.map((f) => `### Finding #${f.id} — ${f.title}
**Sévérité :** ${f.severity} · **Catégorie :** ${f.category}
#### Présentation
${f.summary || '(aucune)'}
#### Plan proposé par le scan
${f.plan || '(aucun)'}`).join('\n\n---\n\n');

  return `Tu travailles sur l'app **${slug}**. Le scan de surveillance **${kindLabel}** a ${n} finding${n > 1 ? 's' : ''} ouvert${n > 1 ? 's' : ''}.
Ta mission : ${n > 1 ? 'les résoudre TOUS en une seule traite orchestrée' : 'le résoudre'}, proprement et définitivement (pas de contournement).

## Findings ouverts (${n})

${blocks}

---

## Déroulé imposé

**Phase 1 — Investigation & conciliation (lecture seule).**
Vérifie chaque finding dans le code actuel (toujours d'actualité ? diagnostic correct ?). Croise-les :
causes racines communes, correctifs qui se recouvrent ou se contredisent, ordre de dépendance.
Un finding obsolète ou faux positif se traite par \`findings_delete\` / \`findings_dismiss\` (justifié), pas par un correctif.

**Phase 2 — Plan consolidé.**
Propose UN plan unique et ordonné couvrant les ${n} findings (pas ${n} plans indépendants) ;
pour chaque étape, indique les findings couverts (#id).

**Phase 3 — Exécution (après approbation).**
Tiens une todo list avec une entrée par finding et marque-les au fil de l'eau. Corrige à la racine,
groupe les modifications par cause racine. Commits : \`fix(surveillance:<id>): …\` (un id principal par
commit) ; les autres findings couverts par le même commit → \`findings_resolve\` explicite avec le sha.
Termine par build + livraison (skills \`0-build\` puis \`0-deploy\`).

**Phase 4 — Vérification finale.**
Vérifie de bout en bout que les correctifs fonctionnent, puis reliste les findings (\`findings_list\`,
kind \`${kind}\`) : AUCUN des ${n} findings ci-dessus ne doit rester \`open\`. Ne conclus pas tant
qu'il en reste un non traité (résolu, dismissé ou supprimé avec justification).`;
}

export default function SurveillanceTab({ slug, initialKind, onResolve }) {
  const [activeKind, setActiveKind] = useState(
    VALID_KINDS.includes(initialKind) ? initialKind : 'security'
  );

  // Deep-link hint (kind from the global dashboard, propagé par le backend
  // `studio_tab` + broadcast WS `studio:tab` → StudioShell `pendingKind`) —
  // re-applies live when it changes (onglet Studio déjà ouvert qui bascule).
  // Manual kind clicks afterwards take precedence.
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
  const [transcript, setTranscript] = useState([]); // live scan-agent output (ephemeral)
  const [err, setErr] = useState(null);
  const { toast, showToast } = useToast();
  const [statusFilter, setStatusFilter] = useState('open');
  // Open findings count for the ACTIVE kind, independent of statusFilter — drives
  // that kind's launch-button cap.
  const [openCount, setOpenCount] = useState(0);
  // Open findings count per kind — drives the count badges on the scan tabs.
  const [openByKind, setOpenByKind] = useState({});

  // Kinds dont une conversation de résolution groupée est ouverte → « Résoudre tout » gaté.
  const resolvingKinds = useOpenResolveScans();

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
      // Open findings across ALL kinds (cap is ~6/kind) → per-kind tab badges + active cap.
      getAppFindings(slug, { status: 'open', limit: 100 }),
    ])
      .then(([s, f, r, o]) => {
        setScan(s.data?.scan || null);
        setBlank(s.data?.blank ?? true);
        setFindings(f.data?.findings || []);
        setRuns(r.data?.runs || []);
        const byKind = {};
        for (const x of o.data?.findings || []) byKind[x.kind] = (byKind[x.kind] || 0) + 1;
        setOpenByKind(byKind);
        setOpenCount(byKind[activeKind] || 0);
      })
      .catch((e) => {
        if (e.response?.status === 503) setErr('Surveillance désactivée (Postgres injoignable).');
        else setErr(apiErr(e));
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
      showToast(e.response?.status === 501 ? 'Runner de scan non disponible.' : apiErr(e), 'error');
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
      showToast('Arrêt a échoué : ' + apiErr(e), 'error');
    } finally {
      setBusy(false);
    }
  };

  // « Résoudre tout » → ouvre une conversation agent unique (Studio.openAgentWithPrompt) chargée
  // de TOUS les findings ouverts du kind actif. Refetch au clic : la liste `findings` affichée
  // dépend du filtre de statut courant, le prompt doit lui embarquer les findings OUVERTS frais.
  const handleResolveAll = async () => {
    setBusy(true);
    try {
      const r = await getAppFindings(slug, { kind: activeKind, status: 'open', limit: 50 });
      const open = r.data?.findings || [];
      if (!open.length) return;
      // scanKind voyage avec le prompt → la conversation créée le porte → le bouton se
      // désactive tant qu'elle est ouverte (cf. useOpenResolveScans). effort:'max' force
      // le thinking maximal (résoudre un scan entier est une tâche profonde) plutôt que
      // d'hériter de la préférence agent stockée (souvent medium).
      onResolve?.({ prompt: buildResolveAllPrompt(open, slug, activeKind, headerLabel), scanKind: activeKind, effort: 'max' });
      setSelected(null);
    } catch (e) {
      showToast('Lancement de la résolution a échoué : ' + apiErr(e), 'error');
    } finally {
      setBusy(false);
    }
  };

  // HARD-delete a finding (irreversible) — for obsolete/stale findings a human
  // wants gone rather than dismissed/resolved.
  const handleDelete = async (f) => {
    if (!window.confirm(`Supprimer définitivement cette finding ?\n\n${f.title}\n\n(Irréversible — préfère « Résoudre » ou « Dismiss » si applicable.)`)) return;
    try {
      await deleteFinding(slug, f.id);
      setSelected(null);
      await reload();
    } catch (e) {
      showToast('Suppression a échoué : ' + apiErr(e), 'error');
    }
  };

  const atCap = openCount >= MAX_OPEN_FINDINGS;
  const blankBusiness = isBusiness && blank;

  return (
    <div className="h-full flex flex-col">
      <Toast toast={toast} />
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
              {openByKind[k.id] > 0 && (
                <span className={`text-[10px] leading-none px-1.5 py-0.5 rounded-full tabular-nums ${
                  on ? 'bg-gray-900/60 text-current' : 'bg-gray-700/70 text-gray-200'
                }`}>{openByKind[k.id]}</span>
              )}
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
            <div className="text-gray-500">Aucun scan Business défini. L&apos;agent du projet le crée/maintient via le tool MCP <code className="text-gray-300">scan_set</code> (cf. <code className="text-gray-300">.claude/rules/surveillance.md</code>).</div>
          ) : (
            <>
              <div><span className="text-gray-500">catégories :</span> {(scan.categories || []).join(', ') || '—'}</div>
              {scan.gate === 'data' && scan.gate_sql && (
                <div className="truncate"><span className="text-gray-500">gate_sql :</span> <code>{scan.gate_sql}</code></div>
              )}
              {scan.updated_by && <div className="text-gray-500">maintenu par {scan.updated_by}</div>}
              <pre className="mt-1 max-h-48 overflow-y-auto whitespace-pre-wrap bg-gray-950/50 p-2 rounded-sm border border-gray-800 text-gray-400">{scan.prompt}</pre>
            </>
          )}
        </div>
      )}

      {/* Fixed scans (Sécurité / Qualité) : axes d'analyse listés sous le nom du scan */}
      {!isBusiness && meta.cats?.length > 0 && (
        <div className="px-4 pt-2 pb-2 flex items-center gap-1.5 flex-wrap border-b border-gray-700/40 text-xs">
          <span className="text-gray-500">Axes d&apos;analyse :</span>
          {meta.cats.map((c) => (
            <span key={c} className="px-1.5 py-0.5 rounded-sm bg-gray-700/60 text-gray-300">{catLabel(c)}</span>
          ))}
        </div>
      )}

      {/* Action bar */}
      <div className="px-4 py-2 border-b border-gray-700 bg-gray-800/30 flex items-center gap-2 flex-wrap">
        {activeRun ? (
          <button onClick={handleStop} disabled={busy} className="px-2.5 py-1 text-xs border rounded-sm flex items-center gap-1 disabled:opacity-50 bg-red-500/20 text-red-700 dark:text-red-200 hover:bg-red-500/30 border-red-500/30">
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
        {(() => {
          const resolving = resolvingKinds.has(activeKind);
          const disabled = busy || resolving || openCount === 0 || !!activeRun;
          const title = resolving
            ? 'Conversation de résolution déjà ouverte pour ce scan — ferme-la pour relancer'
            : activeRun
              ? 'Scan en cours — attends sa fin avant de lancer la résolution'
              : openCount === 0
                ? 'Aucun finding ouvert pour ce scan'
                : `Confier les ${openCount} findings ouverts à l'agent dans une conversation unique (conciliation + résolution orchestrée)`;
          return (
            <button
              onClick={handleResolveAll}
              disabled={disabled}
              title={title}
              className={`px-2.5 py-1 text-xs border rounded-sm flex items-center gap-1 ${
                disabled
                  ? 'text-gray-500 border-gray-700 opacity-60 cursor-not-allowed'
                  : 'bg-blue-500/20 text-blue-700 dark:text-blue-200 hover:bg-blue-500/30 border-blue-500/30'
              }`}
            >
              <Wrench className="w-3 h-3" />
              {resolving ? 'Conversation ouverte' : `Résoudre tout${openCount > 0 ? ` (${openCount})` : ''}`}
            </button>
          );
        })()}
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
          {err && <div className="p-3 bg-red-900/30 border border-red-700/50 text-red-700 dark:text-red-300 rounded-sm text-sm">{err}</div>}
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
                <FindingCard key={f.id} finding={f} active={selected?.id === f.id} onSelect={setSelected} />
              ))}
            </div>
          ))}
        </div>

        {/* Right side: the annex drawer (selected issue) takes priority; otherwise
            the live scan console while a run is in progress. */}
        {selected ? (
          <AnnexDrawer finding={selected} onClose={() => setSelected(null)} onDelete={handleDelete} />
        ) : (activeRun || transcript.length > 0) ? (
          <LiveScanPanel
            lines={transcript}
            kindLabel={headerLabel.toLowerCase()}
            onStop={activeRun ? handleStop : undefined}
            stopping={busy}
          />
        ) : null}

        {/* Historique des runs — masqué pendant qu'un scan tourne (console live) ou
            quand l'annexe d'un finding est ouverte (le drawer prend la place). */}
        {!activeRun && !selected && (
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
        )}
      </div>
    </div>
  );
}
