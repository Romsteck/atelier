import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  AlertTriangle, ArrowDown, ArrowUp, Bot, CalendarClock,
  CheckCircle2, ChevronDown, ChevronRight, CirclePlay, ClipboardList, ExternalLink,
  GitBranch, Loader2, Moon, Play, RefreshCw, Square, Trash2, Upload, X,
} from 'lucide-react';
import PageHeader from '../components/PageHeader';
import Button from '../components/Button';
import MarkdownView from '../components/docs/MarkdownView';
import { useApps } from '../context/AppsContext';
import { usePilot } from '../context/PilotContext';
import { getPilotItemRuns, getPilotRepos, getPilotTranscript, pushSource, unwrapApi } from '../api/client';
import useWebSocket from '../hooks/useWebSocket';
import { useToast, Toast } from '../hooks/useToast';
import { apiErr } from '../utils/apiErr';
import { openStudio } from '../lib/openStudio';

// Thème : seuls les gris sont mirrorés par index.css — toute teinte colorée de
// TEXTE porte ses deux variantes (`text-<c>-700 dark:text-<c>-300`, patron
// Issues.jsx / AgentPanel LIVE_BAND). Les voiles bg/border en alpha restent
// simples (translucides, lisibles dans les deux thèmes).
// Colonnes du kanban. `attention` n'en est PAS une : elle a sa propre vue
// (bouton rouge en tête de page) — l'afficher deux fois diluait le signal.
// La lane `inbox` a été supprimée (les items naissent rédigés/scorés par le CP).
const LANES = [
  { id: 'ready', label: 'Prêt', tone: 'border-blue-500/35' },
  { id: 'in_progress', label: 'En cours', tone: 'border-amber-500/40' },
  { id: 'done', label: 'Terminé', tone: 'border-emerald-500/35' },
];
const PRIORITY = {
  critical: 'bg-red-500/20 text-red-700 dark:text-red-300',
  high: 'bg-orange-500/20 text-orange-700 dark:text-orange-300',
  medium: 'bg-blue-500/15 text-blue-700 dark:text-blue-300',
  low: 'bg-gray-700 text-gray-400',
};
const EXEC = { queued: 'En file', running: 'Agent actif', failed: 'Échec', blocked: 'Bloqué', done: 'Livré' };
const OPTIONS = { priority: ['critical', 'high', 'medium', 'low'], severity: ['critical', 'high', 'medium', 'low'], effort: ['xs', 's', 'm', 'l', 'xl'], kind: ['feature', 'bug', 'improvement', 'finding_fix'] };
// Libellé du moteur ayant réellement livré (`item.last_engine`, posé par le backend
// sur le modèle item — moteur du dernier run).
const ENGINE_LABEL = { claude: 'Opus 4.8', codex: 'GPT-5.6 Sol' };

function Chip({ children, className = '', title }) { return <span title={title} className={`text-[10px] px-1.5 py-0.5 rounded-sm whitespace-nowrap ${className}`}>{children}</span>; }

// Position médiane fractionnaire entre les deux voisins de la lane TRIÉE (pas
// d'offset fixe) ; aux extrémités, extrapole d'un pas au-delà du voisin unique.
// dir = -1 (monter) / +1 (descendre). null = déplacement impossible.
function positionBetween(list, index, dir) {
  const target = index + dir;
  if (index < 0 || target < 0 || target >= list.length) return null;
  const neighbor = list[target];
  const beyond = list[target + dir];
  if (!beyond) return neighbor.position + dir * 1024;
  return (neighbor.position + beyond.position) / 2;
}

function NightLivePanel({ night, showToast }) {
  const { items, cancelRun } = usePilot();
  const stats = night?.stats || {};
  const queue = Array.isArray(stats.queue) ? stats.queue : [];
  const total = Number(stats.total || queue.length || 0);
  const done = Number(stats.done || 0);
  // L'item courant du snapshot porte son id → on retrouve son last_run_id côté
  // items pour brancher l'annulation du run live.
  const currentItem = stats.current ? items.find((x) => x.id === stats.current.id) : null;
  async function stopCurrent() {
    if (!currentItem?.last_run_id || !window.confirm('Stopper le run en cours ?')) return;
    try { await cancelRun(currentItem.last_run_id); showToast?.('Annulation demandée'); }
    catch (e) { showToast?.(apiErr(e), 'error'); }
  }
  return (
    <section className="rounded-lg border border-blue-500/35 bg-blue-500/10 p-3 space-y-2 text-xs text-blue-900 dark:text-blue-100">
      <div className="flex items-center gap-2"><Loader2 className="w-4 h-4 animate-spin" /><span className="font-medium">Nuit Pilote en cours</span><span className="ml-auto text-[11px] text-blue-700 dark:text-blue-300">{done}/{total} livré(s)</span></div>
      {total > 0 && <div className="h-1.5 rounded-full bg-gray-800 overflow-hidden"><div className="h-full bg-blue-400 transition-all" style={{ width: `${Math.min(100, Math.round((done / total) * 100))}%` }} /></div>}
      {stats.current && <div className="flex items-center gap-2 text-[11px] text-gray-300">
        <span className="min-w-0 truncate">En cours · <span className="text-blue-800 dark:text-blue-200">{stats.current.scope}</span> · {stats.current.title}{stats.current.attempt ? ` · tentative ${stats.current.attempt}/3` : ''}</span>
        {currentItem?.last_run_id && <Button size="xs" variant="danger" icon={Square} onClick={stopCurrent}>Stopper</Button>}
      </div>}
      {queue.length > 0 && <div className="flex gap-1.5 overflow-x-auto pb-1">{queue.map((item) => <span key={item.id} title={item.title} className={`max-w-48 truncate rounded-sm border px-2 py-1 text-[10px] ${item.status === 'done' ? 'border-emerald-500/30 text-emerald-700 dark:text-emerald-300' : ['queued', 'running'].includes(item.status) ? 'border-blue-400/40 text-blue-800 dark:text-blue-200' : item.status === 'blocked' || item.status === 'failed' ? 'border-red-500/35 text-red-700 dark:text-red-300' : 'border-gray-700 text-gray-500'}`}>{item.scope} · #{item.id}</span>)}</div>}
      <div className="text-[10px] text-gray-500">Les apps et leurs findings passent avant Atelier.</div>
    </section>
  );
}

// Bande « État des dépôts » : agrège le git status des 8 dépôts (apps + Atelier)
// — fichiers en attente de commit, commits en attente de push. Repliable, live
// via `source:changed` (debounce) + refetch après chaque event backlog (les
// commits du Pilote changent l'état). Un dépôt d'app se répare dans son Studio
// (panneau Git) ; le push direct est proposé quand c'est le seul geste manquant.
function RepoStatusBand({ showToast }) {
  const [repos, setRepos] = useState(null);
  const [open, setOpen] = useState(() => localStorage.getItem('pilot:repoBand') !== '0');
  const [pushing, setPushing] = useState(null);
  const debounce = useRef(null);
  const refresh = useCallback(() => {
    getPilotRepos().then((r) => setRepos(unwrapApi(r))).catch(() => {});
  }, []);
  const refreshSoon = useCallback(() => {
    clearTimeout(debounce.current);
    debounce.current = setTimeout(refresh, 800);
  }, [refresh]);
  useEffect(() => { refresh(); return () => clearTimeout(debounce.current); }, [refresh]);
  const { epoch } = useWebSocket({ 'source:changed': refreshSoon, 'pilot:backlog': refreshSoon });
  const prevEpoch = useRef(0);
  useEffect(() => {
    if (epoch === 0 || epoch === prevEpoch.current) return;
    prevEpoch.current = epoch;
    refresh();
  }, [epoch, refresh]);
  const toggle = () => setOpen((v) => { localStorage.setItem('pilot:repoBand', v ? '0' : '1'); return !v; });
  async function push(scope) {
    setPushing(scope);
    try { await pushSource(scope); showToast(`${scope} : poussé`); refreshSoon(); }
    catch (err) { showToast(apiErr(err), 'error'); }
    finally { setPushing(null); }
  }
  const pending = (repos || []).filter((r) => r.error || r.dirty > 0 || (r.ahead ?? 0) > 0 || !r.has_upstream).length;
  return (
    <div className="rounded-lg border border-gray-700/60 bg-gray-800/40">
      <button onClick={toggle} className="w-full flex items-center gap-2 px-3 py-2 text-left">
        {open ? <ChevronDown className="w-3.5 h-3.5 text-gray-500" /> : <ChevronRight className="w-3.5 h-3.5 text-gray-500" />}
        <GitBranch className="w-4 h-4 text-gray-500" />
        <span className="text-xs font-medium text-gray-300">État des dépôts</span>
        {repos && (pending > 0
          ? <span className="text-[10px] px-1.5 py-0.5 rounded-sm bg-amber-500/15 text-amber-700 dark:text-amber-300">{pending} en attente</span>
          : <span className="text-[10px] px-1.5 py-0.5 rounded-sm bg-emerald-500/15 text-emerald-700 dark:text-emerald-300">tout est committé et poussé</span>)}
      </button>
      {open && repos && (
        <div className="px-3 pb-2.5 flex flex-wrap gap-1.5">
          {repos.map((r) => {
            const clean = !r.error && r.dirty === 0 && (r.ahead ?? 0) === 0 && r.has_upstream;
            const isApp = r.scope !== 'atelier';
            return (
              <div key={r.scope} title={r.last_commit ? `Dernier commit : ${r.last_commit.subject}` : undefined}
                className={`flex items-center gap-1.5 rounded-sm border px-2 py-1 text-[11px] ${clean ? 'border-gray-700/60 text-gray-400' : 'border-amber-500/30 bg-amber-500/5 text-gray-300'}`}>
                <span className={`w-1.5 h-1.5 rounded-full ${r.error ? 'bg-red-500' : clean ? 'bg-emerald-500' : 'bg-amber-500'}`} />
                {isApp
                  ? <button onClick={() => openStudio(r.scope, { tab: 'code' })} className="font-medium text-gray-200 hover:underline">{r.scope}</button>
                  : <span className="font-medium text-gray-200">atelier</span>}
                {r.error && <span className="text-red-700 dark:text-red-300">illisible</span>}
                {r.dirty > 0 && <span className="text-amber-700 dark:text-amber-300">{r.dirty} à committer</span>}
                {(r.ahead ?? 0) > 0 && <span className="text-blue-700 dark:text-blue-300">{r.ahead} à pousser</span>}
                {!r.has_upstream && !r.error && <span className="text-red-700 dark:text-red-300">sans upstream</span>}
                {isApp && (r.ahead ?? 0) > 0 && r.dirty === 0 && (
                  <button onClick={() => push(r.scope)} disabled={pushing === r.scope}
                    className="ml-0.5 inline-flex items-center gap-0.5 text-blue-700 dark:text-blue-300 hover:underline disabled:opacity-50">
                    {pushing === r.scope ? <Loader2 className="w-3 h-3 animate-spin" /> : <Upload className="w-3 h-3" />}Pousser
                  </button>
                )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

// La saisie directe (quick-add) a été retirée : le chef de projet est LA porte
// d'entrée du backlog — il reformule, scope, score et planifie chaque besoin
// (items « bien écrits » plutôt que notes brutes). L'endpoint POST /pilot/backlog
// reste côté serveur (future dictée vocale / intégrations).
function CaptureHint() {
  return (
    <div className="flex items-center gap-3 rounded-lg border border-gray-700/60 bg-gray-800/40 px-3 py-2.5">
      <Bot className="w-4 h-4 shrink-0 text-blue-600 dark:text-blue-400" />
      <span className="text-xs text-gray-400 flex-1">Dis ce dont tu as besoin au <span className="text-gray-200 font-medium">chef de projet</span> — il rédige, score et planifie l&apos;item pour toi.</span>
      <Button size="xs" icon={ClipboardList} onClick={() => window.dispatchEvent(new CustomEvent('pilot:open-assistant'))}>Ouvrir l&apos;assistant</Button>
    </div>
  );
}

function BacklogCard({ item, laneItems = [], onOpen, showToast }) {
  const { move, run, cancelRun } = usePilot();
  const active = ['queued', 'running'].includes(item.exec_status);
  const index = laneItems.findIndex((x) => x.id === item.id);
  async function execute(e) {
    e.stopPropagation();
    try {
      const confirmAtelier = item.scope === 'atelier' ? window.confirm('Exécuter un changement Atelier peut redémarrer la plateforme. Continuer ?') : false;
      if (item.scope === 'atelier' && !confirmAtelier) return;
      await run(item.id, confirmAtelier); showToast('Run Pilote lancé');
    } catch (err) { showToast(apiErr(err), 'error'); }
  }
  async function stop(e) {
    e.stopPropagation();
    if (!window.confirm('Stopper ce run ?')) return;
    try { await cancelRun(item.last_run_id); showToast('Annulation demandée'); }
    catch (err) { showToast(apiErr(err), 'error'); }
  }
  async function moveTo(lane, position) {
    try { await move(item.id, lane, position); } catch (err) { showToast(apiErr(err), 'error'); }
  }
  function reorder(dir) {
    const position = positionBetween(laneItems, index, dir);
    if (position != null) moveTo(item.lane, position);
  }
  // Badges Attention distincts au niveau carte : Questions (ambre) vs Bloqué (rouge).
  const questionsBadge = item.lane === 'attention' && (item.needs_user || (item.questions || []).length > 0);
  const blockedBadge = item.lane === 'attention' && !questionsBadge && item.exec_status === 'blocked';
  // Moves sémantiques par lane — « En cours » n'est JAMAIS une destination UI ;
  // attention se traite dans le drawer (Relancer / Marquer traité).
  const canMove = !active && item.lane === 'ready';
  const canRun = !active && item.lane === 'ready' && !item.needs_user;
  const canStop = item.exec_status === 'running' && Boolean(item.last_run_id);
  return (
    <article onClick={() => onOpen(item)} className={`rounded-lg border bg-gray-900/55 p-3 cursor-pointer hover:bg-gray-800/65 transition ${item.lane === 'attention' ? 'border-red-500/45' : 'border-gray-700/60'}`}>
      <div className="flex items-start gap-2">
        <div className="flex-1 min-w-0">
          <div className="text-[13px] font-medium text-gray-100 leading-snug">{item.title}</div>
          <div className="mt-1.5 flex items-center gap-1 flex-wrap">
            <Chip className="bg-gray-700/70 text-gray-300">{item.scope}</Chip>
            <Chip className={PRIORITY[item.priority] || PRIORITY.medium}>{item.priority}</Chip>
            <Chip className="bg-gray-800 text-gray-400">{item.kind}</Chip>
            <Chip className="bg-gray-800 text-gray-400">{item.effort}</Chip>
            {item.created_by === 'user' && item.lane === 'attention' && <Chip className="bg-amber-500/15 text-amber-700 dark:text-amber-300">à trier</Chip>}
            {item.lane === 'done' && ENGINE_LABEL[item.last_engine] && <Chip className="bg-blue-500/15 text-blue-700 dark:text-blue-300">{ENGINE_LABEL[item.last_engine]}</Chip>}
            {questionsBadge && <Chip className="bg-amber-500/15 text-amber-700 dark:text-amber-300">Questions</Chip>}
            {blockedBadge && <Chip className="bg-red-500/15 text-red-700 dark:text-red-300">Bloqué — {item.attempts || 0} échec{(item.attempts || 0) > 1 ? 's' : ''}</Chip>}
          </div>
        </div>
        {active && <Loader2 className="w-4 h-4 text-blue-600 dark:text-blue-400 animate-spin shrink-0" />}
      </div>
      {item.needs_user_reason && <p className={`mt-2 text-[11px] line-clamp-3 ${item.needs_user ? 'text-amber-700 dark:text-amber-300' : 'text-red-700 dark:text-red-300'}`}>{item.needs_user_reason}</p>}
      {item.exec_status !== 'idle' && <div className="mt-2 text-[10px] text-gray-500">{EXEC[item.exec_status] || item.exec_status}{item.attempts ? ` · tentative ${item.attempts}/3` : ''}</div>}
      {(canMove || canRun || canStop) && <div className="mt-2 pt-2 border-t border-gray-800 flex items-center gap-1" onClick={(e) => e.stopPropagation()}>
        {!active && item.lane === 'ready' && <>
          {index > 0 && <button title="Monter" onClick={() => reorder(-1)} className="p-1 text-gray-500 hover:text-gray-200"><ArrowUp className="w-3.5 h-3.5" /></button>}
          {index >= 0 && index < laneItems.length - 1 && <button title="Descendre" onClick={() => reorder(1)} className="p-1 text-gray-500 hover:text-gray-200"><ArrowDown className="w-3.5 h-3.5" /></button>}
        </>}
        <span className="flex-1" />
        {canStop && <Button size="xs" variant="danger" icon={Square} onClick={stop}>Stopper</Button>}
        {canRun && <Button size="xs" variant="primary" icon={Play} onClick={execute}>Exécuter</Button>}
      </div>}
    </article>
  );
}

// Lecture = markdown rendu (MarkdownView, le même que le fil agent) ; édition =
// textarea à la demande via « Éditer » / double-clic (patron note d'IssueCard).
function MarkdownField({ label, value, rows, onChange }) {
  const [editing, setEditing] = useState(false);
  const empty = !value?.trim();
  return (
    <div>
      <div className="flex items-center gap-2">
        <label className="text-[10px] uppercase text-gray-500">{label}</label>
        <button type="button" onClick={() => setEditing((v) => !v)} className="text-[10px] text-gray-500 hover:text-gray-200">{editing ? 'Aperçu' : 'Éditer'}</button>
      </div>
      {editing
        ? <textarea autoFocus value={value || ''} onChange={onChange} rows={rows} className="w-full mt-1 bg-gray-950 border border-gray-700 rounded-sm px-3 py-2 text-xs text-gray-300" />
        : empty
          ? <button type="button" onClick={() => setEditing(true)} className="mt-1 w-full text-left text-xs text-gray-600 border border-dashed border-gray-800 rounded-sm px-3 py-2 hover:text-gray-400">Vide — cliquer pour éditer</button>
          : <div onDoubleClick={() => setEditing(true)} className="mt-1 rounded-sm border border-gray-800 bg-gray-950/60 px-3 py-2 max-h-72 overflow-y-auto"><MarkdownView>{value}</MarkdownView></div>}
    </div>
  );
}

function ItemDrawer({ item, onClose, showToast }) {
  const { patch, remove, run, move, cancelRun, state, transcripts } = usePilot();
  const [draft, setDraft] = useState(item);
  const [runs, setRuns] = useState([]);
  const [selectedRun, setSelectedRun] = useState(null);
  const [persistedTranscript, setPersistedTranscript] = useState([]);
  useEffect(() => { setDraft(item); getPilotItemRuns(item.id).then((r) => setRuns(unwrapApi(r) || [])).catch(() => setRuns([])); }, [item]);
  const active = ['queued', 'running'].includes(item.exec_status);
  async function save() {
    try {
      const body = {}; for (const k of ['title', 'request', 'description', 'plan', 'kind', 'priority', 'severity', 'effort', 'engine']) if (draft[k] !== item[k] && draft[k] != null) body[k] = draft[k];
      await patch(item.id, body); showToast('Item mis à jour');
    } catch (e) { showToast(apiErr(e), 'error'); }
  }
  async function retry() { try { await patch(item.id, { lane: 'ready', exec_status: 'idle', needs_user: false, reset_attempts: true }); showToast('Item remis en file Prêt'); } catch (e) { showToast(apiErr(e), 'error'); } }
  async function markHandled() { try { await move(item.id, 'done'); showToast('Item marqué traité'); onClose(); } catch (e) { showToast(apiErr(e), 'error'); } }
  async function answerAndReplan() {
    if ((draft.questions || []).some((q) => !q.answer?.trim())) { showToast('Réponds à chaque question', 'error'); return; }
    try { await patch(item.id, { questions: draft.questions, needs_user: false, lane: 'ready', exec_status: 'idle', reset_attempts: true }); showToast('Réponses enregistrées · item prêt'); }
    catch (e) { showToast(apiErr(e), 'error'); }
  }
  async function execute() {
    try { const confirmAtelier = item.scope === 'atelier' && window.confirm('Confirmer l’exécution sur Atelier ?'); if (item.scope === 'atelier' && !confirmAtelier) return; await run(item.id, !!confirmAtelier); showToast('Run lancé'); }
    catch (e) { showToast(apiErr(e), 'error'); }
  }
  async function stop() {
    if (!item.last_run_id || !window.confirm('Stopper ce run ?')) return;
    try { await cancelRun(item.last_run_id); showToast('Annulation demandée'); }
    catch (e) { showToast(apiErr(e), 'error'); }
  }
  async function showRun(r) { setSelectedRun(r.id); try { setPersistedTranscript(unwrapApi(await getPilotTranscript(r.id)) || []); } catch { setPersistedTranscript([]); } }
  const transcript = selectedRun ? (transcripts[selectedRun] || persistedTranscript) : [];
  // Repli : le ring mémoire ne survit pas au restart — `transcript_tail` (runs API)
  // reste alors la seule trace du run.
  const selectedRunTail = selectedRun ? runs.find((r) => r.id === selectedRun)?.transcript_tail : null;
  return (
    <div className="fixed inset-0 z-50 flex justify-end bg-black/55" onMouseDown={(e) => { if (e.target === e.currentTarget) onClose(); }}>
      <aside className="w-full max-w-2xl h-full bg-gray-900 border-l border-gray-700 flex flex-col shadow-2xl">
        <div className="h-12 px-4 border-b border-gray-700 flex items-center gap-2 shrink-0">
          <ClipboardList className="w-4 h-4 text-blue-600 dark:text-blue-400" /><span className="font-medium text-sm flex-1 truncate">Item #{item.id}</span>
          {ENGINE_LABEL[item.last_engine] && <Chip className="bg-blue-500/15 text-blue-700 dark:text-blue-300" title="Moteur du dernier run">{ENGINE_LABEL[item.last_engine]}</Chip>}
          <button onClick={onClose} className="p-1.5 text-gray-400 hover:text-white"><X className="w-4 h-4" /></button>
        </div>
        <div className="flex-1 overflow-y-auto p-4 space-y-5">
          <div>
            <label className="text-[10px] uppercase text-gray-500">Titre</label>
            <input value={draft.title || ''} onChange={(e) => setDraft({ ...draft, title: e.target.value })} className="w-full mt-1 bg-gray-950 border border-gray-700 rounded-sm px-3 py-2 text-sm" />
          </div>
          <div className="grid grid-cols-2 sm:grid-cols-5 gap-2">
            {Object.entries(OPTIONS).map(([key, vals]) => <label key={key} className="text-[10px] uppercase text-gray-500">{key}<select value={draft[key]} onChange={(e) => setDraft({ ...draft, [key]: e.target.value })} className="mt-1 w-full bg-gray-950 border border-gray-700 rounded-sm px-2 py-1.5 text-xs text-gray-300 normal-case">{vals.map((v) => <option key={v}>{v}</option>)}</select></label>)}
            {/* « Auto » reste une option VALIDE (un item peut déjà porter engine='auto') :
                jamais disabled+sélectionnée — le libellé dit la vérité tant que le
                routeur de complexité n'est pas branché. */}
            <label className="text-[10px] uppercase text-gray-500">Moteur<select value={draft.engine} onChange={(e) => setDraft({ ...draft, engine: e.target.value })} className="mt-1 w-full bg-gray-950 border border-gray-700 rounded-sm px-2 py-1.5 text-xs text-gray-300 normal-case"><option value="claude">Opus 4.8</option><option value="auto">{state?.engines?.auto_router ? 'Auto' : 'Auto (bientôt — Claude pour l’instant)'}</option><option value="codex" disabled={!state?.engines?.codex_worker}>GPT-5.6 Sol{state?.engines?.codex_worker ? '' : ' (en attente)'}</option></select></label>
          </div>
          {['request', 'description', 'plan'].map((key) => <MarkdownField key={key} label={key} value={draft[key]} rows={key === 'plan' ? 8 : 4} onChange={(e) => setDraft({ ...draft, [key]: e.target.value })} />)}
          {item.needs_user && <section className="rounded-lg border border-amber-500/35 bg-amber-500/5 p-3 space-y-3"><div className="text-sm font-medium text-amber-700 dark:text-amber-300 flex items-center gap-2"><AlertTriangle className="w-4 h-4" />Questions — décision requise</div><p className="text-xs text-gray-400">{item.needs_user_reason}</p>{(draft.questions || []).map((q, i) => <label key={i} className="block text-xs text-gray-300">{q.q}<textarea value={q.answer || ''} onChange={(e) => { const questions = [...draft.questions]; questions[i] = { ...q, answer: e.target.value }; setDraft({ ...draft, questions }); }} rows={2} className="mt-1 w-full bg-gray-950 border border-gray-700 rounded-sm px-2 py-1.5" /></label>)}<Button variant="warning" size="sm" onClick={answerAndReplan}>Répondre &amp; replanifier</Button></section>}
          {item.exec_status === 'blocked' && !item.needs_user && <section className="rounded-lg border border-red-500/35 bg-red-500/5 p-3"><div className="text-sm text-red-700 dark:text-red-300 font-medium">Bloqué — {item.attempts || 3} échec{(item.attempts || 3) > 1 ? 's' : ''}</div><p className="text-xs text-gray-400 mt-1">{item.needs_user_reason}</p></section>}
          <section><h3 className="text-xs uppercase text-gray-500 mb-2">Historique des runs</h3>{runs.length === 0 ? <p className="text-xs text-gray-600">Aucun run.</p> : <div className="space-y-1">{runs.map((r) => <button key={r.id} onClick={() => showRun(r)} className={`w-full text-left rounded-sm border px-2 py-1.5 text-xs ${selectedRun === r.id ? 'border-blue-500/50 bg-blue-500/10' : 'border-gray-800 hover:bg-gray-800/50'}`}><span className={r.status === 'success' ? 'text-emerald-600 dark:text-emerald-400' : r.status === 'running' ? 'text-blue-600 dark:text-blue-400' : 'text-red-600 dark:text-red-400'}>{r.status}</span> · tentative {r.attempt}/3 · {r.phase}{r.failure_reason ? ` · ${r.failure_reason}` : ''}{ENGINE_LABEL[r.engine] ? ` · ${ENGINE_LABEL[r.engine]}` : ''}</button>)}</div>}{selectedRun && (transcript.length > 0
            ? <pre className="mt-2 max-h-64 overflow-auto bg-gray-950 border border-gray-800 rounded-sm p-2 text-[10px] whitespace-pre-wrap text-gray-400">{transcript.map((l) => l.line).join('\n')}</pre>
            : selectedRunTail
              ? <div className="mt-2"><div className="text-[10px] text-gray-500 mb-1">Fin de transcript (repli — ring live indisponible)</div><pre className="max-h-64 overflow-auto bg-gray-950 border border-gray-800 rounded-sm p-2 text-[10px] whitespace-pre-wrap text-gray-400">{selectedRunTail}</pre></div>
              : <pre className="mt-2 max-h-64 overflow-auto bg-gray-950 border border-gray-800 rounded-sm p-2 text-[10px] whitespace-pre-wrap text-gray-400">Transcript indisponible.</pre>)}</section>
        </div>
        <div className="p-3 border-t border-gray-700 flex items-center gap-2 flex-wrap shrink-0">
          <Button size="sm" onClick={save}>Enregistrer</Button>
          {!['queued', 'running', 'done'].includes(item.exec_status) && !item.needs_user && <Button size="sm" variant="success" icon={CirclePlay} onClick={execute}>Exécuter</Button>}
          {item.exec_status === 'running' && item.last_run_id && <Button size="sm" variant="danger" icon={Square} onClick={stop}>Stopper</Button>}
          {item.lane === 'attention' && !active && <>
            <Button size="sm" variant="warning" icon={RefreshCw} onClick={retry}>Relancer</Button>
            <Button size="sm" variant="success" icon={CheckCircle2} onClick={markHandled}>Marquer traité</Button>
          </>}
          {item.scope !== 'atelier' && <Button as="a" href={`/studio/${item.scope}?tab=backlog`} target="_blank" size="sm" variant="neutral" icon={ExternalLink}>Studio</Button>}
          <span className="flex-1" /><Button variant="danger" icon={Trash2} onClick={async () => { if (window.confirm('Supprimer cet item ?')) { await remove(item.id); onClose(); } }}>Supprimer</Button>
        </div>
      </aside>
    </div>
  );
}

function SchedulePanel() {
  const { schedule, saveSchedule, launchNight, stopNight, night, state } = usePilot();
  const [draft, setDraft] = useState(schedule);
  useEffect(() => setDraft(schedule), [schedule]);
  if (!draft) return null;
  const active = ['running', 'waiting_atelier'].includes(night?.status);
  return (
    <details className="relative">
      <summary className="list-none cursor-pointer"><Button as="span" variant="neutral" icon={CalendarClock} active={schedule.enabled}>Planification</Button></summary>
      <div className="absolute z-30 right-0 top-9 w-[320px] rounded-lg border border-gray-700 bg-gray-900 shadow-xl p-3 space-y-3">
        <label className="flex items-center justify-between text-xs text-gray-300">Activer les nuits<input type="checkbox" checked={draft.enabled} onChange={(e) => setDraft({ ...draft, enabled: e.target.checked })} /></label>
        <div className="grid grid-cols-3 gap-2">{[['Début', 'start_hour', 0, 23], ['Fin', 'end_hour', 0, 23], ['Agents', 'max_concurrent', 1, 4]].map(([label, key, min, max]) => <label key={key} className="text-[10px] text-gray-500 uppercase">{label}<input type="number" min={min} max={max} value={draft[key]} onChange={(e) => setDraft({ ...draft, [key]: Number(e.target.value) })} className="mt-1 w-full bg-gray-950 border border-gray-700 rounded-sm px-2 py-1 text-xs text-gray-300" /></label>)}</div>
        <label className="flex items-center justify-between text-xs text-gray-300">Inclure Atelier<input type="checkbox" checked={draft.include_atelier} onChange={(e) => setDraft({ ...draft, include_atelier: e.target.checked })} /></label>
        <label className="flex items-center justify-between text-xs text-gray-300">Résoudre les findings<input type="checkbox" checked={draft.resolve_findings} onChange={(e) => setDraft({ ...draft, resolve_findings: e.target.checked })} /></label>
        <label className="flex items-center justify-between gap-3 text-xs text-gray-300">Politique moteur
          <select value={draft.engine_policy || 'claude'} onChange={(e) => setDraft({ ...draft, engine_policy: e.target.value })}
            className="bg-gray-950 border border-gray-700 rounded-sm px-2 py-1 text-xs">
            <option value="claude">Opus 4.8</option>
            <option value="auto" disabled={!state?.engines?.auto_router}>Auto 4.8 / 5.6{state?.engines?.auto_router ? '' : ' (en attente)'}</option>
          </select>
        </label>
        <p className="text-[10px] text-gray-500">Routeur automatique Opus 4.8 / GPT-5.6 Sol : <span className={state?.engines?.auto_router ? 'text-emerald-700 dark:text-emerald-300' : 'text-amber-700 dark:text-amber-300'}>{state?.engines?.auto_router ? 'actif' : 'en attente'}</span></p>
        <div className="flex gap-2"><Button onClick={() => saveSchedule(draft)}>Enregistrer</Button>{active ? <Button variant="danger" onClick={stopNight}>Annuler la nuit</Button> : <Button variant="success" icon={Moon} onClick={launchNight}>Lancer maintenant</Button>}</div>
      </div>
    </details>
  );
}

export default function Backlog({ lockedScope = null, embedded = false }) {
  const { items, counts, loading, night } = usePilot();
  const { apps } = useApps();
  const { toast, showToast } = useToast();
  const [scope, setScope] = useState(lockedScope || 'all');
  const [kind, setKind] = useState('all');
  const [view, setView] = useState('kanban');
  const [selected, setSelected] = useState(null);
  const scopes = useMemo(() => lockedScope ? [lockedScope] : ['atelier', ...apps.map((a) => a.slug).sort()], [apps, lockedScope]);
  const filtered = useMemo(() => items.filter((x) => (scope === 'all' || x.scope === scope) && (kind === 'all' || x.kind === kind)), [items, scope, kind]);
  useEffect(() => { if (selected) { const fresh = items.find((x) => x.id === selected.id); if (fresh) setSelected(fresh); else setSelected(null); } }, [items, selected]);
  const nightActive = ['running', 'waiting_atelier'].includes(night?.status);
  return (
    <div className="h-full flex flex-col overflow-hidden">
      <Toast toast={toast} />
      {!embedded && <PageHeader title="Pilote · Backlog autonome" icon={Bot}><SchedulePanel /></PageHeader>}
      <div className="flex-1 min-h-0 overflow-y-auto p-4 space-y-3">
        {nightActive && <NightLivePanel night={night} showToast={showToast} />}
        <CaptureHint />
        {!embedded && <RepoStatusBand showToast={showToast} />}
        <div className="flex items-center gap-2 flex-wrap">
          <div className="flex border border-gray-700 rounded-sm overflow-hidden">
            <button onClick={() => setView('kanban')} className={`px-2.5 py-1.5 text-xs ${view === 'kanban' ? 'bg-gray-700 text-white' : 'text-gray-400'}`}>Kanban</button>
            {/* Attention n'est plus une colonne du board : ce bouton est le SEUL
                signal — teinté rouge dès qu'un item attend Romain. */}
            <button onClick={() => setView('attention')} className={`px-2.5 py-1.5 text-xs font-medium ${
              view === 'attention'
                ? 'bg-red-500/25 text-red-800 dark:text-red-200'
                : counts.attention > 0
                  ? 'bg-red-500/10 text-red-700 dark:text-red-300'
                  : 'text-gray-400'
            }`}>Attention ({counts.attention})</button>
          </div>
          {!lockedScope && <select value={scope} onChange={(e) => setScope(e.target.value)} className="bg-gray-900 border border-gray-700 rounded-sm px-2 py-1.5 text-xs text-gray-300"><option value="all">Tous les scopes</option>{scopes.map((s) => <option key={s}>{s}</option>)}</select>}
          <select value={kind} onChange={(e) => setKind(e.target.value)} className="bg-gray-900 border border-gray-700 rounded-sm px-2 py-1.5 text-xs text-gray-300"><option value="all">Tous les types</option>{OPTIONS.kind.map((v) => <option key={v}>{v}</option>)}</select>
          <span className="ml-auto text-[11px] text-gray-500">{counts.ready} prêt(s) · {counts.running} actif(s) · {counts.blocked} bloqué(s)</span>
        </div>
        {loading ? <div className="h-40 flex items-center justify-center"><Loader2 className="animate-spin text-blue-600 dark:text-blue-400" /></div> : view === 'attention' ? (
          <div className="max-w-4xl space-y-2">{filtered.filter((x) => x.lane === 'attention').map((item) => <BacklogCard key={item.id} item={item} onOpen={setSelected} showToast={showToast} />)}{filtered.filter((x) => x.lane === 'attention').length === 0 && <div className="text-center py-14 text-gray-500"><CheckCircle2 className="w-8 h-8 mx-auto mb-2 text-emerald-600 dark:text-emerald-400" />Aucun item ne demande ton attention.</div>}</div>
        ) : (
          <div className="flex gap-3 overflow-x-auto pb-3 min-h-[420px]">{LANES.map((lane) => { const list = filtered.filter((x) => x.lane === lane.id).sort((a, b) => a.position - b.position); // flex-1 + min-w : les colonnes remplissent la largeur disponible quand
          // l'écran le permet, et retombent en scroll horizontal en dessous.
          return <section key={lane.id} className={`flex-1 min-w-[290px] rounded-lg border ${lane.tone} bg-gray-800/20 flex flex-col`}><header className="px-3 py-2 border-b border-gray-700/50 flex items-center"><span className="text-xs font-medium text-gray-300">{lane.label}</span><span className="ml-auto text-[10px] text-gray-500">{list.length}</span></header><div className="p-2 space-y-2">{list.map((item) => <BacklogCard key={item.id} item={item} laneItems={list} onOpen={setSelected} showToast={showToast} />)}</div></section>; })}</div>
        )}
      </div>
      {selected && <ItemDrawer item={selected} onClose={() => setSelected(null)} showToast={showToast} />}
    </div>
  );
}
