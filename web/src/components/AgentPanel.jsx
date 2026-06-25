import { useState, useRef, useEffect, useCallback, useMemo } from 'react';
import {
  Loader2, Bot, ChevronRight, ChevronDown, Wrench, AlertTriangle, X,
  FileText, FilePlus, FilePen, Terminal, FolderSearch, Search, Globe, ListChecks, NotebookPen, Plug,
} from 'lucide-react';
import MarkdownView from './docs/MarkdownView';
import Composer from './agent/Composer';
import { getSdkVersion, updateSdk, getThinking } from '../api/client';
import { apiErr } from '../utils/apiErr';
import { useAgentConversations } from '../context/AgentConversationsContext';
import { describeTool, splitPath } from '../lib/toolDisplay';
import { MODELS, MODES, buildSettings } from '../lib/agentModels';

// Estimation client-side du nombre de tokens de réflexion à partir d'un nombre de
// caractères. Le flux thinking ne porte pas le compte réel → heuristique ≈ caractères / 4
// (ordre de grandeur usuel). Indicateur de progression, pas de facturation.
const charsToTokens = (chars) => Math.max(0, Math.round((chars || 0) / 4));

// Compteur LISSÉ : `target` saute par paliers (les tokens de réflexion arrivent en
// lots ~50), on l'affiche en montant graduellement (≈12 %/frame + 1 min) via rAF pour
// éviter les à-coups. `active` faux (réflexion finie / bloc d'historique) → snap direct.
// SLOWDOWN : on n'avance qu'une frame sur 4 → animation 4× plus lente (~15 Hz).
const SMOOTH_FRAME_SKIP = 4;
function useSmoothCount(target, active) {
  const [shown, setShown] = useState(target);
  const targetRef = useRef(target);
  const shownRef = useRef(target);
  targetRef.current = target;

  useEffect(() => {
    if (!active) {
      shownRef.current = targetRef.current;
      setShown(targetRef.current);
      return;
    }
    let raf;
    let frame = 0;
    const tick = () => {
      if (++frame % SMOOTH_FRAME_SKIP === 0) {
        const t = targetRef.current;
        const cur = shownRef.current;
        if (cur !== t) {
          const gap = t - cur;
          const step = Math.sign(gap) * Math.max(1, Math.ceil(Math.abs(gap) * 0.12));
          let nextVal = cur + step;
          if ((gap > 0 && nextVal > t) || (gap < 0 && nextVal < t)) nextVal = t;
          shownRef.current = nextVal;
          setShown(nextVal);
        }
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [active]);

  return shown;
}

// Réflexion = compteur léger + contenu PARESSEUX. Le texte (souvent volumineux, rarement
// lu) n'est PAS retenu en front : seul `chars` (→ count live animé) + `tidx` (ordinal) le
// sont. Le texte n'est rapatrié (getThinking) qu'à l'expand. Exception : le bloc ACTIF
// (tail d'un tour en cours) reçoit son `text` en direct → expand instantané, pas de fetch.
function ThinkingBlock({ slug, sid, tidx, chars, text, active }) {
  const [open, setOpen] = useState(false);
  const [loaded, setLoaded] = useState(text ?? null); // texte affichable (déjà en main ou fetché)
  const [loading, setLoading] = useState(false);
  const [err, setErr] = useState(null);
  const shown = useSmoothCount(charsToTokens(chars), active);

  // Le bloc actif accumule son texte en direct (deltas) → on le reflète tant qu'il arrive.
  // (Une fois le bloc dépassé, `text` repasse à undefined : on garde alors `loaded` tel quel.)
  useEffect(() => { if (text != null) setLoaded(text); }, [text]);

  const toggle = async () => {
    const willOpen = !open;
    setOpen(willOpen);
    if (!willOpen || loaded != null || loading) return;
    if (sid == null || tidx == null) { setErr('contenu indisponible'); return; }
    setLoading(true);
    setErr(null);
    try {
      const r = await getThinking(slug, sid, tidx);
      setLoaded(r.data?.text ?? '');
    } catch (e) {
      setErr(apiErr(e, 'chargement de la réflexion échoué'));
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="text-[12px] text-gray-400 border-l-2 border-gray-700 pl-2 my-1">
      <button onClick={toggle}
        className="cursor-pointer select-none text-gray-500 hover:text-gray-300 flex items-center gap-1.5">
        {open ? <ChevronDown className="w-3 h-3 shrink-0" /> : <ChevronRight className="w-3 h-3 shrink-0" />}
        <span>Réflexion</span>
        <span className="text-gray-600 tabular-nums" title="estimation ≈ caractères / 4">
          · {shown.toLocaleString('fr-FR')} tokens
        </span>
        {active && <Loader2 className="w-3 h-3 animate-spin text-gray-600" />}
      </button>
      {open && (
        loading ? (
          <div className="flex items-center gap-1.5 mt-1 text-gray-600"><Loader2 className="w-3 h-3 animate-spin" /> chargement…</div>
        ) : err ? (
          <div className="mt-1 text-red-400">{err}</div>
        ) : (
          <div className="whitespace-pre-wrap mt-1 italic">{loaded}</div>
        )
      )}
    </div>
  );
}

// Mapping iconKey (toolDisplay) → composant lucide. Les imports lucide restent ici
// (côté composant) ; toolDisplay reste pur (sans JSX).
const TOOL_ICONS = {
  read: FileText, write: FilePlus, edit: FilePen, bash: Terminal,
  glob: FolderSearch, search: Search, web: Globe, agent: Bot,
  todo: ListChecks, notebook: NotebookPen, mcp: Plug, tool: Wrench,
};
const TOOL_CHIP = 'shrink-0 text-[10px] uppercase tracking-wider text-gray-400 bg-gray-700/40 px-1.5 py-0.5 rounded-sm';

// Chemin : basename en clair, dossier atténué, chemin complet en title.
function PathLabel({ path, className = '' }) {
  const { dir, base } = splitPath(path);
  return (
    <span className={`font-mono truncate ${className}`} title={path}>
      {dir && <span className="text-gray-500">{dir}/</span>}
      <span className="text-gray-300">{base}</span>
    </span>
  );
}

const TODO_MARK = { pending: '○', in_progress: '◐', completed: '✓' };
const TODO_MARK_CLS = { pending: 'text-gray-600', in_progress: 'text-blue-400', completed: 'text-green-500' };
const TODO_TEXT_CLS = { pending: 'text-gray-400', in_progress: 'text-gray-200', completed: 'text-gray-500 line-through' };

// Checklist ÉPINGLÉE (sticky) en haut du fil (WHY) : Claude Code réécrit sa todolist en
// continu (N appels TodoWrite/tour). L'afficher inline la faisait « sauter » plus bas à
// chaque MAJ (rendue à la position de la dernière occurrence). Épinglée, elle montre
// TOUJOURS le dernier état, mise à jour en place. Repliable pour libérer de la place.
function TodoBanner({ todos }) {
  const [open, setOpen] = useState(true);
  const total = todos.length;
  const done = todos.filter((t) => t.status === 'completed').length;
  return (
    <div className="sticky top-0 z-10 -mx-3 px-3 py-1.5 bg-gray-900/95 backdrop-blur-sm border-b border-gray-800">
      <button onClick={() => setOpen((o) => !o)} className="w-full flex items-center gap-1.5 text-[12px] text-gray-400 hover:text-gray-200">
        {open ? <ChevronDown className="w-3.5 h-3.5 shrink-0" /> : <ChevronRight className="w-3.5 h-3.5 shrink-0" />}
        <ListChecks className="w-3.5 h-3.5 shrink-0" />
        <span className="text-gray-300">Todos</span>
        <span className="text-gray-600">{done}/{total}</span>
      </button>
      {open && (
        <ul className="mt-1 ml-5 space-y-0.5">
          {todos.map((t, i) => (
            <li key={i} className="flex items-start gap-1.5 text-[12px]">
              <span className={`shrink-0 ${TODO_MARK_CLS[t.status] || 'text-gray-600'}`}>{TODO_MARK[t.status] || '○'}</span>
              <span className={TODO_TEXT_CLS[t.status] || 'text-gray-400'}>
                {t.status === 'in_progress' ? (t.activeForm || t.content) : t.content}
              </span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

// Cible compacte d'un outil (basename pour les chemins, primary tronqué sinon).
function toolTarget(d) {
  if (!d.primary) return '';
  if (d.primaryPath) return splitPath(d.primary).base;
  return d.primary.length > 60 ? `${d.primary.slice(0, 59)}…` : d.primary;
}

// Une SUITE d'appels d'outils d'un tour (WHY) : voir l'action EN COURS est utile, revoir
// le détail de chaque action passée ne l'est pas. Tour en cours → on montre l'action live
// (`⚙ verbe cible…`). Sinon → une ligne repliée « N actions » (dépliable à la demande).
function ToolActivityGroup({ tools, isTail }) {
  const [open, setOpen] = useState(false);
  const n = tools.length;
  const last = tools[n - 1];
  const lastDesc = describeTool(last.name, last.input);
  const live = isTail && !last.result; // dernière action sans résultat = en cours
  const anyError = tools.some((t) => t.result?.isError);

  return (
    <div className="text-[12px] my-1">
      <button onClick={() => setOpen((o) => !o)} className="flex items-center gap-1.5 max-w-full text-gray-500 hover:text-gray-300">
        {live ? (
          <Loader2 className="w-3.5 h-3.5 shrink-0 animate-spin text-gray-400" />
        ) : open ? (
          <ChevronDown className="w-3.5 h-3.5 shrink-0" />
        ) : (
          <ChevronRight className="w-3.5 h-3.5 shrink-0" />
        )}
        {live ? (
          <span className="flex items-baseline gap-1.5 min-w-0">
            <span className="text-gray-300 shrink-0">{lastDesc.verb}</span>
            <span className="truncate text-gray-400 font-mono">{toolTarget(lastDesc)}</span>
            <span className="text-gray-600 shrink-0">…</span>
          </span>
        ) : (
          <span className="flex items-center gap-1.5">
            <span>{n} action{n > 1 ? 's' : ''}</span>
            {anyError && <span className="text-red-400" title="une action a échoué">●</span>}
          </span>
        )}
      </button>
      {open && (
        <ul className="mt-1 ml-5 space-y-0.5">
          {tools.map((t, i) => {
            const d = describeTool(t.name, t.input);
            const Icon = TOOL_ICONS[d.iconKey] || Wrench;
            const err = t.result?.isError;
            return (
              <li key={t.id || i} className="flex items-center gap-1.5 min-w-0">
                <Icon className={`w-3 h-3 shrink-0 ${err ? 'text-red-400' : 'text-gray-500'}`} />
                <span className="text-gray-400 shrink-0">{d.verb}</span>
                {d.badge && <span className={TOOL_CHIP}>{d.badge}</span>}
                {d.primary &&
                  (d.primaryPath ? (
                    <PathLabel path={d.primary} className="min-w-0" />
                  ) : (
                    <span className={`truncate text-gray-500 ${d.primaryMono ? 'font-mono' : ''}`} title={d.primaryTitle || d.primary}>
                      {d.primary}
                    </span>
                  ))}
                {err && <span className="text-red-400 shrink-0 text-[10px]">échec</span>}
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}

function ResultFooter({ data }) {
  const cost = typeof data?.total_cost_usd === 'number' ? data.total_cost_usd : null;
  const u = data?.usage || {};
  const inTok = u.input_tokens, outTok = u.output_tokens;
  return (
    <div className="text-[11px] text-gray-500 border-t border-gray-800 mt-2 pt-1.5 flex flex-wrap gap-x-3 gap-y-0.5">
      {cost != null && <span title="estimation client-side, pas la facturation">~${cost.toFixed(4)} (est.)</span>}
      {inTok != null && <span>in {inTok}</span>}
      {outTok != null && <span>out {outTok}</span>}
      {data?.num_turns != null && <span>{data.num_turns} turn(s)</span>}
      {data?.duration_ms != null && <span>{(data.duration_ms / 1000).toFixed(1)}s</span>}
    </div>
  );
}

// Carte de question interactive (AskUserQuestion). Affiche 1-4 questions avec options ;
// collecte les choix + une réponse libre par question, puis renvoie { [texte_question]:
// réponse } à la conversation via /answer (= tour suivant dans la même session).
function QuestionCard({ questions, answered, answerText, onSubmit, onCancel }) {
  const [sel, setSel] = useState(() => questions.map(() => ({ chosen: new Set(), text: '' })));
  // Option et réponse libre sont MUTUELLEMENT EXCLUSIVES (build() priorise déjà le texte) :
  // choisir une option efface la réponse libre, et saisir une réponse libre dé-sélectionne
  // les options (mode « custom »), pour que l'UI reflète ce qui sera réellement envoyé.
  const setChosen = (qi, label, multi) => {
    setSel((prev) => prev.map((s, i) => {
      if (i !== qi) return s;
      const chosen = new Set(s.chosen);
      if (multi) { chosen.has(label) ? chosen.delete(label) : chosen.add(label); }
      else { chosen.clear(); chosen.add(label); }
      return { ...s, chosen, text: '' };
    }));
  };
  const setText = (qi, text) =>
    setSel((prev) => prev.map((s, i) => (i === qi ? { ...s, text, chosen: text.trim() ? new Set() : s.chosen } : s)));
  const build = () => {
    const answers = {};
    questions.forEach((q, i) => {
      const s = sel[i];
      const val = s.text.trim() ? s.text.trim() : Array.from(s.chosen).join(', ');
      if (val) answers[q.question] = val;
    });
    return answers;
  };
  const hasAny = sel.some((s) => s.text.trim() || s.chosen.size);
  return (
    <div className="border border-blue-500/30 bg-blue-500/5 rounded-lg p-3 my-1 space-y-3">
      {questions.map((q, qi) => (
        <div key={qi} className="space-y-1.5">
          {q.header && (
            <span className="inline-block text-[10px] uppercase tracking-wider text-blue-300 bg-blue-500/15 px-1.5 py-0.5 rounded-sm">{q.header}</span>
          )}
          <div className="text-[13px] text-gray-200">{q.question}</div>
          <div className="flex flex-col gap-1.5">
            {(q.options || []).map((o, oi) => {
              const chosen = sel[qi].chosen.has(o.label);
              return (
                <button key={oi} disabled={answered} onClick={() => setChosen(qi, o.label, !!q.multiSelect)}
                  className={`text-left px-2.5 py-1.5 rounded-md text-[12px] border ${chosen ? 'bg-blue-500/25 border-blue-500/50 text-blue-100' : 'border-gray-700 text-gray-300 hover:bg-gray-800'} disabled:opacity-60`}>
                  <div className="font-medium">{o.label}</div>
                  {o.description && <div className="text-[11px] text-gray-400 mt-0.5 font-normal">{o.description}</div>}
                </button>
              );
            })}
          </div>
          {!answered && (
            <input value={sel[qi].text} onChange={(e) => setText(qi, e.target.value)} placeholder="Autre… (réponse libre)"
              className={`w-full bg-gray-800 border rounded-sm px-2 py-1 text-[12px] placeholder-gray-600 focus:outline-none ${
                sel[qi].text.trim()
                  ? 'border-blue-500/50 ring-1 ring-blue-500/30 text-blue-100' // mode custom actif
                  : 'border-gray-700 text-gray-100 focus:border-blue-500'
              }`} />
          )}
        </div>
      ))}
      {answered ? (
        <div className="text-[11px] text-gray-500">{answerText ? `Réponse : ${answerText}` : 'Réponse envoyée.'}</div>
      ) : (
        <div className="flex items-center gap-2">
          <button onClick={() => onSubmit(build())} disabled={!hasAny}
            className="px-3 py-1 rounded-md text-[12px] bg-blue-500 text-white hover:bg-blue-600 disabled:opacity-40 disabled:cursor-not-allowed">
            Répondre
          </button>
          <button onClick={onCancel} className="px-2 py-1 rounded-md text-[12px] text-gray-400 hover:text-gray-200 hover:bg-gray-800">
            Passer
          </button>
        </div>
      )}
    </div>
  );
}

// Carte d'approbation de plan (ExitPlanMode). Affiche le plan proposé et propose à
// l'utilisateur d'implémenter (la session bascule en édition, même mémoire) ou de le
// renvoyer en révision (avec remarques optionnelles). Tant que non décidé, le tour est
// suspendu côté runner (canUseTool).
function PlanReviewCard({ plan, decided, approved, onApprove, onReject }) {
  const [feedback, setFeedback] = useState('');
  return (
    <div className="border border-amber-500/30 bg-amber-500/5 rounded-lg p-3 my-1 space-y-2">
      <span className="inline-block text-[10px] uppercase tracking-wider text-amber-300 bg-amber-500/15 px-1.5 py-0.5 rounded-sm">
        Plan proposé
      </span>
      <div className="text-[13px] text-gray-200"><MarkdownView>{plan || '(plan vide)'}</MarkdownView></div>
      {decided ? (
        <div className="text-[11px] text-gray-500">
          {approved ? '✅ Plan approuvé — implémentation en cours.' : '↩︎ Renvoyé en révision.'}
        </div>
      ) : (
        <>
          <input value={feedback} onChange={(e) => setFeedback(e.target.value)}
            placeholder="Remarques (optionnel, si tu renvoies en révision)"
            className="w-full bg-gray-800 border border-gray-700 rounded-sm px-2 py-1 text-[12px] text-gray-100 placeholder-gray-600 focus:outline-none focus:border-amber-500" />
          <div className="flex items-center gap-2">
            <button onClick={() => onApprove(feedback)}
              className="px-3 py-1 rounded-md text-[12px] bg-amber-500 text-white hover:bg-amber-600">
              Implémenter
            </button>
            <button onClick={() => onReject(feedback)}
              className="px-2 py-1 rounded-md text-[12px] text-gray-400 hover:text-gray-200 hover:bg-gray-800">
              Renvoyer en révision
            </button>
          </div>
        </>
      )}
    </div>
  );
}

// Panneau d'UNE conversation. Contrôlé : tout l'état (items, running, runId, question
// en attente) vit dans le provider, indexé par `panelKey`. Le panneau ne fait que
// rendre + déléguer (sendMessage/answer/cancel/closeConversation). La SAISIE est isolée
// dans <Composer> (état local) pour que taper ne re-render pas la liste des messages.
export default function AgentPanel({ panelKey }) {
  const { slug, convos, convName, sendMessage, answer, cancel, decidePlan, changeMode, changeModel, closeConversation } = useAgentConversations();
  const convo = convos[panelKey];

  const [modelId, setModelId] = useState(() => localStorage.getItem('agent:model') || 'opus-4-8');
  // Effort de CE panneau : l'effort imposé au lancement (ex. 'max' depuis « Résoudre »)
  // prime sur la préférence stockée. Ne persiste PAS un effort synchronisé depuis la
  // conversation (sinon « Résoudre » polluerait la préférence globale) — seul un clic
  // délibéré sur le sélecteur l'enregistre (cf. chooseEffort).
  const [effort, setEffort] = useState(() => convo?.effort || localStorage.getItem('agent:effort') || 'max');
  const [mode, setMode] = useState(() => localStorage.getItem('agent:mode') || 'plan');
  const [sdk, setSdk] = useState(null);
  const [updatingSdk, setUpdatingSdk] = useState(false);
  const [sdkMsg, setSdkMsg] = useState(null); // { ok: bool, text } — retour de la MAJ SDK
  const bodyRef = useRef(null);
  // « Collé en bas » : ref (pas de re-render à chaque pixel scrollé). showNew pilote le
  // bouton flottant « Nouveaux messages » quand du contenu arrive alors qu'on lit plus haut.
  const atBottomRef = useRef(true);
  const [showNew, setShowNew] = useState(false);

  const onScroll = useCallback(() => {
    const el = bodyRef.current;
    if (!el) return;
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 40; // tolérance 40px
    atBottomRef.current = atBottom;
    if (atBottom) setShowNew(false); // no-op si déjà false (React bail-out)
  }, []);

  const jumpToBottom = useCallback(() => {
    const el = bodyRef.current;
    if (!el) return;
    el.scrollTo({ top: el.scrollHeight, behavior: 'smooth' });
    atBottomRef.current = true;
    setShowNew(false);
  }, []);

  // Choix mémorisés → défauts des prochaines conversations. (L'effort n'est PAS persisté
  // ici : seul un clic délibéré l'enregistre, cf. chooseEffort — pour ne pas que l'effort
  // imposé d'une conversation « Résoudre » écrase la préférence globale.)
  useEffect(() => { localStorage.setItem('agent:model', modelId); }, [modelId]);
  useEffect(() => { localStorage.setItem('agent:mode', mode); }, [mode]);

  // Reflète l'effort imposé au lancement (ex. 'max' depuis « Résoudre ») dès qu'il est connu.
  useEffect(() => {
    if (convo?.effort) setEffort(convo.effort);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [convo?.effort]);

  // Changement délibéré d'effort par l'utilisateur → applique + mémorise comme préférence.
  const chooseEffort = useCallback((e) => {
    setEffort(e);
    localStorage.setItem('agent:effort', e);
  }, []);
  useEffect(() => { getSdkVersion().then((r) => setSdk(r.data)).catch(() => {}); }, []);

  const items = useMemo(() => convo?.items || [], [convo?.items]);
  const running = !!convo?.running;
  const live = !!convo?.runId; // session vivante → modèle/mode/effort verrouillés

  // Corrélation résultat ↔ outil par `id` (les appels d'outils sont souvent parallèles →
  // le résultat ne suit pas forcément son tool_use). Sert au regroupement d'activité.
  const resultByUseId = useMemo(() => {
    const byId = new Map();
    items.forEach((it) => {
      if (it.type === 'tool_result' && it.tool_use_id != null) byId.set(it.tool_use_id, it);
    });
    return byId;
  }, [items]);

  // Nœuds de rendu : les suites contiguës de tool_use/tool_result (hors TodoWrite, épinglé)
  // sont fusionnées en UN groupe d'activité (`kind:'tools'`), le reste reste tel quel.
  const renderNodes = useMemo(() => {
    const nodes = [];
    let i = 0;
    while (i < items.length) {
      const it = items[i];
      if (it.type === 'tool_use' || it.type === 'tool_result') {
        const tools = [];
        let j = i;
        while (j < items.length && (items[j].type === 'tool_use' || items[j].type === 'tool_result')) {
          const t = items[j];
          if (t.type === 'tool_use' && t.name !== 'TodoWrite') {
            tools.push({ id: t.id, name: t.name, input: t.input, result: t.id != null ? resultByUseId.get(t.id) : undefined });
          }
          j++;
        }
        if (tools.length) nodes.push({ kind: 'tools', tools, endIdx: j - 1, key: `tools-${i}` });
        i = j;
      } else {
        nodes.push({ kind: 'item', it, idx: i, key: it.id || `i-${i}` });
        i++;
      }
    }
    return nodes;
  }, [items, resultByUseId]);

  // Checklist courante = dernière occurrence TodoWrite du fil (la plus récente).
  const latestTodos = useMemo(() => {
    for (let k = items.length - 1; k >= 0; k--) {
      const it = items[k];
      if (it.type === 'tool_use' && it.name === 'TodoWrite') {
        return Array.isArray(it.input?.todos) ? it.input.todos : null;
      }
    }
    return null;
  }, [items]);

  // Tour suspendu sur une interaction (question/plan) : on remplace le spinner générique
  // par "en attente de ta réponse" pour ne pas laisser croire que le modèle calcule.
  const lastItem = items[items.length - 1];
  const awaitingUser =
    !!lastItem &&
    ((lastItem.type === 'question' && !(convo?.answered?.has(lastItem.request_id) || lastItem.answered)) ||
      (lastItem.type === 'plan_review' && !(convo?.decided?.has(lastItem.request_id) || lastItem.decided)));
  // Action d'outil en cours sur le dernier item → l'activité live l'affiche déjà : on évite
  // le doublon avec le spinner générique « agent travaille… » du bas.
  const liveTool =
    running && lastItem?.type === 'tool_use' && lastItem.name !== 'TodoWrite' &&
    (lastItem.id == null || !resultByUseId.get(lastItem.id));

  // Auto-scroll bas SEULEMENT si l'utilisateur est collé en bas. S'il a scrollé pour lire,
  // on préserve sa position et on signale « Nouveaux messages » à la place. Exception : son
  // propre message vient d'être posté (type 'user') → on re-colle toujours en bas.
  useEffect(() => {
    const el = bodyRef.current;
    if (!el) return;
    const ownSend = items[items.length - 1]?.type === 'user';
    if (atBottomRef.current || ownSend) {
      el.scrollTop = el.scrollHeight;
      atBottomRef.current = true;
      setShowNew(false);
    } else {
      setShowNew(true);
    }
  }, [items, running]);

  // Le backend peut basculer le mode en cours de session (approbation de plan → bypass).
  // On reflète ce changement dans le sélecteur local pour que l'UI ne reste pas sur "Plan".
  useEffect(() => {
    if (convo?.activeMode && convo.activeMode !== mode) setMode(convo.activeMode);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [convo?.activeMode]);

  const selModel = MODELS.find((m) => m.id === modelId) || MODELS[0];
  const onChangeModel = (id) => {
    setModelId(id);
    const m = MODELS.find((x) => x.id === id);
    if (m && m.efforts.length && !m.efforts.includes(effort)) chooseEffort(m.efforts[m.efforts.length - 1]);
    if (live) changeModel(panelKey, m?.model || null); // session vivante → setModel à chaud
  };
  const onChangeMode = (id) => {
    setMode(id);
    if (live) changeMode(panelKey, id); // session vivante → setPermissionMode à chaud
  };

  // Envoi délégué par le Composer (texte + images). Dépend des sélecteurs (rarement) — pas
  // de la frappe : taper ne re-render que le Composer.
  const onSend = useCallback((text, images) => {
    if (running) return;
    sendMessage(panelKey, text, buildSettings({ modelId, effort, mode }), images);
  }, [running, mode, modelId, effort, panelKey, sendMessage]);

  const stop = useCallback(() => cancel(panelKey), [cancel, panelKey]);
  const submitAnswer = useCallback((request_id, payload) => answer(panelKey, request_id, payload), [answer, panelKey]);
  const decide = useCallback((request_id, approved, feedback) => decidePlan(panelKey, request_id, approved, feedback), [decidePlan, panelKey]);

  const onUpdateSdk = useCallback(async () => {
    setUpdatingSdk(true);
    setSdkMsg(null);
    try {
      const r = await updateSdk();
      // Re-lecture de la version live sur disque → update_available repasse à false
      // (le bouton disparaît) et l'UI reflète l'état réel post-install.
      const v = await getSdkVersion();
      setSdk(v.data);
      setSdkMsg({ ok: true, text: `SDK à jour (${r.data?.installed ?? v.data?.installed ?? ''})` });
    } catch (e) {
      setSdkMsg({ ok: false, text: apiErr(e, 'MAJ SDK échouée') });
    } finally {
      setUpdatingSdk(false);
    }
  }, []);

  if (!convo) return null;

  const displayName = convName(convo);

  // Rendu d'un item NON-outil (les outils passent par ToolActivityGroup).
  const renderItem = (it, i) => {
    if (it.type === 'user') {
      return (
        <div key={`i-${i}`} className="flex justify-end">
          <div className="max-w-[85%] bg-blue-500/15 text-gray-100 rounded-lg px-3 py-1.5 text-[13px]">
            {it.images?.length > 0 && (
              <div className="flex flex-wrap gap-1.5 mb-1">
                {it.images.map((src, ii) => (
                  <img key={ii} src={src} alt="" className="max-h-32 max-w-[160px] rounded border border-blue-500/30 object-cover" />
                ))}
              </div>
            )}
            {it.text && <div className="whitespace-pre-wrap wrap-break-word">{it.text}</div>}
          </div>
        </div>
      );
    }
    if (it.type === 'assistant') {
      return <div key={`i-${i}`} className="text-[13px] text-gray-200"><MarkdownView>{it.text}</MarkdownView></div>;
    }
    if (it.type === 'thinking') return <ThinkingBlock key={`i-${i}`} slug={slug} sid={convo.sid} tidx={it.tidx} chars={it.chars} text={it.text} active={running && i === items.length - 1} />;
    if (it.type === 'result') return <ResultFooter key={`i-${i}`} data={it.data} />;
    if (it.type === 'error') {
      return (
        <div key={`i-${i}`} className="text-[12px] text-red-400 flex items-start gap-1.5">
          <AlertTriangle className="w-3.5 h-3.5 shrink-0 mt-0.5" /> {it.message}
        </div>
      );
    }
    if (it.type === 'question') {
      return (
        <QuestionCard key={`i-${i}`}
          questions={it.questions}
          answered={convo.answered.has(it.request_id) || !!it.answered}
          answerText={it.answer}
          onSubmit={(answers) => submitAnswer(it.request_id, { answers })}
          onCancel={() => submitAnswer(it.request_id, { cancelled: true })}
        />
      );
    }
    if (it.type === 'plan_review') {
      return (
        <PlanReviewCard key={`i-${i}`}
          plan={it.plan}
          decided={convo.decided.has(it.request_id) || !!it.decided}
          approved={it.approved}
          onApprove={(feedback) => decide(it.request_id, true, feedback)}
          onReject={(feedback) => decide(it.request_id, false, feedback)}
        />
      );
    }
    return null;
  };

  return (
    <div className="flex flex-col h-full min-h-0 bg-gray-900">
      {/* En-tête : titre + modèle + fermer */}
      <div className="flex items-center gap-2 h-[34px] shrink-0 px-3 border-b border-gray-800 text-[12px]">
        <Bot className="w-4 h-4 text-blue-400 shrink-0" />
        <span className="text-gray-300 truncate" title={displayName}>{displayName}</span>
        <span className="text-gray-600">·</span>
        <span className="text-gray-500 truncate max-w-[120px]" title={`modèle : ${convo.activeModel || selModel.label}`}>
          {convo.activeModel || selModel.label}
        </span>
        {convo.loading && <Loader2 className="w-3.5 h-3.5 animate-spin text-gray-600" />}
        <div className="ml-auto flex items-center gap-2">
          {sdkMsg && (
            <span className={`text-[11px] max-w-[160px] truncate ${sdkMsg.ok ? 'text-emerald-400' : 'text-red-400'}`}
              title={sdkMsg.text}>
              {sdkMsg.text}
            </span>
          )}
          {sdk?.update_available && (
            <button onClick={onUpdateSdk} disabled={updatingSdk}
              className="px-1.5 py-0.5 rounded-sm bg-amber-500/15 text-amber-400 hover:bg-amber-500/25 disabled:opacity-50 disabled:cursor-not-allowed flex items-center gap-1"
              title={`MAJ Agent SDK : ${sdk.installed} → ${sdk.latest}`}>
              {updatingSdk && <Loader2 className="w-3 h-3 animate-spin" />}
              {updatingSdk ? 'MAJ…' : `MAJ ${sdk.latest}`}
            </button>
          )}
          <button onClick={() => closeConversation(panelKey)} title="Fermer (la conversation reste dans l'historique)"
            className="p-1 rounded-sm text-gray-500 hover:text-gray-200 hover:bg-gray-800">
            <X className="w-4 h-4" />
          </button>
        </div>
      </div>

      {/* Fil de conversation. Wrapper `relative` : le bouton « Nouveaux messages » s'ancre
          au bas VISIBLE du panneau (il ne défile pas avec le contenu). */}
      <div className="relative flex-1 min-h-0">
      <div ref={bodyRef} onScroll={onScroll} className="h-full overflow-y-auto px-3 py-2 space-y-2">
        {latestTodos?.length > 0 && <TodoBanner todos={latestTodos} />}
        {items.length === 0 && !convo.loading && (
          <div className="text-[13px] text-gray-600 mt-4 text-center">
            Pose une question à l’agent sur <span className="text-gray-400">{slug}</span>.
            <div className="text-[11px] mt-1 text-gray-700">
              <span className="text-blue-400/80">Plan</span> = lecture seule ·{' '}
              <span className="text-amber-400/80">Bypass</span> = édite &amp; exécute (relu dans Git).
            </div>
          </div>
        )}
        {convo.error && (
          <div className="text-[12px] text-red-400 flex items-start gap-1.5">
            <AlertTriangle className="w-3.5 h-3.5 shrink-0 mt-0.5" /> {convo.error}
          </div>
        )}
        {renderNodes.map((node) =>
          node.kind === 'tools' ? (
            <ToolActivityGroup key={node.key} tools={node.tools} isTail={running && node.endIdx === items.length - 1} />
          ) : (
            renderItem(node.it, node.idx)
          ),
        )}
        {running && !awaitingUser && !liveTool && (
          <div className="flex items-center gap-1.5 text-[12px] text-gray-500">
            <Loader2 className="w-3.5 h-3.5 animate-spin" /> agent travaille…
          </div>
        )}
        {/* Indépendant de `running` : une carte dialogue est intrinsèquement un état
            d'attente, et l'action ne dépend que de runId (restauré au refresh). */}
        {awaitingUser && (
          <div className="text-[12px] text-gray-600 italic">En attente de ta réponse…</div>
        )}
      </div>
        {showNew && (
          <button onClick={jumpToBottom}
            className="absolute left-1/2 -translate-x-1/2 bottom-3 z-10 flex items-center gap-1 px-2.5 py-1 rounded-full text-[11px] font-medium shadow-lg bg-blue-600 text-white hover:bg-blue-500">
            <ChevronDown className="w-3.5 h-3.5" /> Nouveaux messages
          </button>
        )}
      </div>

      {/* Sélecteurs. Modèle + mode sont modifiables EN COURS de session (setModel /
          setPermissionMode à chaud). L'effort, lui, est figé au démarrage (pas d'API live) →
          verrouillé pendant une session ; ouvrir une nouvelle conversation pour le changer. */}
      <div className="flex items-center flex-wrap gap-x-4 gap-y-1 px-3 py-1.5 border-t border-gray-800 shrink-0">
        <div className="flex items-center gap-1 w-full sm:w-auto">
          <span className="text-[11px] text-gray-600 mr-1">Modèle</span>
          <select value={modelId} onChange={(e) => onChangeModel(e.target.value)}
            className="flex-1 sm:flex-none bg-gray-800 border border-gray-700 rounded-sm text-[11px] text-gray-200 px-1 py-[3px] focus:outline-none focus:border-blue-500">
            {MODELS.map((m) => <option key={m.id} value={m.id}>{m.label}</option>)}
          </select>
        </div>
        <div className="flex items-center gap-1">
          <span className="text-[11px] text-gray-600 mr-1">Mode</span>
          {MODES.map((m) => (
            <button key={m.id} onClick={() => onChangeMode(m.id)} title={m.title}
              className={`px-1.5 py-0.5 min-h-[30px] sm:min-h-0 rounded-sm text-[11px] ${
                mode === m.id
                  ? m.id === 'bypass'
                    ? 'bg-amber-500/20 text-amber-300'
                    : 'bg-blue-500/20 text-blue-300'
                  : 'text-gray-500 hover:text-gray-300 hover:bg-gray-800'
              }`}>
              {m.label}
            </button>
          ))}
        </div>
        {selModel.efforts.length > 0 && (
          <div className="flex items-center gap-1"
            title={live ? 'Effort figé au démarrage de la session — ouvre une nouvelle conversation pour le changer' : undefined}>
            <span className="text-[11px] text-gray-600 mr-1">Effort</span>
            {selModel.efforts.map((e) => (
              <button key={e} disabled={live} onClick={() => chooseEffort(e)}
                className={`px-1.5 py-0.5 min-h-[30px] sm:min-h-0 rounded-sm text-[11px] disabled:opacity-50 disabled:cursor-not-allowed ${effort === e ? 'bg-blue-500/20 text-blue-300' : 'text-gray-500 hover:text-gray-300 hover:bg-gray-800'}`}>
                {e}
              </button>
            ))}
          </div>
        )}
      </div>

      {/* Saisie (état local isolé) */}
      <Composer onSend={onSend} running={running} onStop={stop} />
    </div>
  );
}
