import { useState, useRef, useEffect, useCallback, useMemo } from 'react';
import {
  Send, Square, Loader2, Bot, ChevronRight, Wrench, AlertTriangle, X,
  FileText, FilePlus, FilePen, Terminal, FolderSearch, Search, Globe, ListChecks, NotebookPen, Plug,
} from 'lucide-react';
import MarkdownView from './docs/MarkdownView';
import { getSdkVersion, updateSdk } from '../api/client';
import { apiErr } from '../utils/apiErr';
import { useAgentConversations } from '../context/AgentConversationsContext';
import { describeTool, formatToolResult, splitPath, diffLines, editsOf } from '../lib/toolDisplay';
import { MODELS, MODES, buildSettings } from '../lib/agentModels';

// Estimation client-side du nombre de tokens d'un texte. Le flux thinking ne porte
// pas le compte réel → heuristique ≈ caractères / 4 (ordre de grandeur usuel). Sert
// uniquement d'indicateur de progression de la réflexion, pas de facturation.
const estimateTokens = (text) => Math.max(0, Math.round((text?.length || 0) / 4));

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

function ThinkingBlock({ text, active }) {
  const shown = useSmoothCount(estimateTokens(text), active);
  return (
    <details className="text-[12px] text-gray-400 border-l-2 border-gray-700 pl-2 my-1">
      <summary className="cursor-pointer select-none text-gray-500 hover:text-gray-300 flex items-center gap-1.5">
        <span>Réflexion</span>
        <span className="text-gray-600 tabular-nums" title="estimation ≈ caractères / 4">
          · {shown.toLocaleString('fr-FR')} tokens
        </span>
        {active && <Loader2 className="w-3 h-3 animate-spin text-gray-600" />}
      </summary>
      <div className="whitespace-pre-wrap mt-1 italic">{text}</div>
    </details>
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

// Chemin : basename en clair, dossier atténué, chemin complet en title. Tokens gris
// → s'adaptent aux deux thèmes via le mirror de index.css (pas de `dark:`).
function PathLabel({ path, className = '' }) {
  const { dir, base } = splitPath(path);
  return (
    <span className={`font-mono truncate ${className}`} title={path}>
      {dir && <span className="text-gray-500">{dir}/</span>}
      <span className="text-gray-200">{base}</span>
    </span>
  );
}

function ToolHeader({ d }) {
  const Icon = TOOL_ICONS[d.iconKey] || Wrench;
  return (
    <div className="text-[12px] flex items-start gap-1.5 my-1">
      <Icon className="w-3.5 h-3.5 shrink-0 mt-0.5 text-gray-400" />
      <span className="min-w-0 flex-1 flex items-baseline gap-1.5">
        <span className="text-gray-300 shrink-0">{d.verb}</span>
        {d.badge && <span className={TOOL_CHIP}>{d.badge}</span>}
        {d.primaryPath
          ? <PathLabel path={d.primary} className="flex-1 min-w-0" />
          : d.primary
            ? <span className={`flex-1 min-w-0 truncate text-gray-400 ${d.primaryMono ? 'font-mono' : ''}`} title={d.primaryTitle || d.primary}>{d.primary}</span>
            : null}
        {d.secondary && <span className="text-gray-600 shrink-0">{d.secondary}</span>}
      </span>
    </div>
  );
}

// Diff Edit/MultiEdit : toutes les lignes `old` en « - » rouge, `new` en « + » vert.
// Tokens identiques à git/DiffView.jsx (rendent bien dans les deux thèmes).
function EditDiff({ input }) {
  const edits = editsOf(input);
  return (
    <div className="border border-gray-700 bg-gray-900 overflow-x-auto text-[11px] font-mono leading-5 mt-1">
      {edits.map((e, i) => (
        <div key={i}>
          {edits.length > 1 && (
            <div className="px-2 py-0.5 text-gray-500 bg-gray-800/50">edit {i + 1}{e.replace_all ? ' · tout remplacer' : ''}</div>
          )}
          {diffLines(e.old_string, e.new_string).map((d, j) => (
            <div key={j} className={`px-2 whitespace-pre ${d.t === '-' ? 'text-red-400 bg-red-900/15' : 'text-green-400 bg-green-900/15'}`}>
              <span className="select-none text-gray-600 mr-1">{d.t}</span>{d.l || ' '}
            </div>
          ))}
        </div>
      ))}
    </div>
  );
}

const TODO_MARK = { pending: '○', in_progress: '◐', completed: '✓' };
const TODO_MARK_CLS = { pending: 'text-gray-600', in_progress: 'text-blue-400', completed: 'text-green-500' };
const TODO_TEXT_CLS = { pending: 'text-gray-400', in_progress: 'text-gray-200', completed: 'text-gray-500 line-through' };

function TodoList({ todos }) {
  return (
    <div className="text-[12px] my-1">
      <div className="flex items-center gap-1.5 text-gray-400">
        <ListChecks className="w-3.5 h-3.5 shrink-0" /><span className="text-gray-300">Todos</span>
      </div>
      <ul className="mt-1 ml-5 space-y-0.5">
        {todos.map((t, i) => (
          <li key={i} className="flex items-start gap-1.5">
            <span className={`shrink-0 ${TODO_MARK_CLS[t.status] || 'text-gray-600'}`}>{TODO_MARK[t.status] || '○'}</span>
            <span className={TODO_TEXT_CLS[t.status] || 'text-gray-400'}>
              {t.status === 'in_progress' ? (t.activeForm || t.content) : t.content}
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}

// Résultat formaté par outil : corps en diff / markdown / <pre> selon les drapeaux.
// Corps vide → résumé seul (pas de section repliable).
function ResultDisclosure({ name, input, result }) {
  const f = formatToolResult(name, input, result.text, result.isError);
  const tone = result.isError ? 'text-red-400' : 'text-gray-500';
  const hasBody = f.diff || (f.body && f.body.length);
  if (!hasBody) {
    return <div className={`text-[11px] ${tone} pl-5`}>{f.summary}{result.isError ? ' (erreur)' : ''}</div>;
  }
  return (
    <details className={`text-[11px] ${tone} pl-5`}>
      <summary className="cursor-pointer select-none flex items-center gap-1">
        <ChevronRight className="w-3 h-3" /> {f.summary}{result.isError ? ' (erreur)' : ''}
      </summary>
      {f.diff
        ? <EditDiff input={input} />
        : f.markdown
          ? <div className="mt-1 text-gray-300"><MarkdownView>{f.body}</MarkdownView></div>
          : <pre className="whitespace-pre-wrap wrap-break-word mt-1 font-mono text-gray-400">{f.body}</pre>}
    </details>
  );
}

// Un appel d'outil = en-tête formaté + son résultat (corrélé par id en amont).
// Pas encore de résultat + tour en cours sur le dernier item → micro-spinner discret.
function ToolCall({ name, input, result, running, isLast }) {
  const d = describeTool(name, input);
  if (d.todos) return <TodoList todos={d.todos} />; // ack supprimé (résultat consommé)
  return (
    <div>
      <ToolHeader d={d} />
      {result
        ? (!d.suppressResult && <ResultDisclosure name={name} input={input} result={result} />)
        : running && isLast
          ? <div className="pl-5 text-[11px] text-gray-600 inline-flex items-center gap-1"><Loader2 className="w-3 h-3 animate-spin" /> …</div>
          : null}
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
  const setChosen = (qi, label, multi) => {
    setSel((prev) => prev.map((s, i) => {
      if (i !== qi) return s;
      const chosen = new Set(s.chosen);
      if (multi) { chosen.has(label) ? chosen.delete(label) : chosen.add(label); }
      else { chosen.clear(); chosen.add(label); }
      return { ...s, chosen };
    }));
  };
  const setText = (qi, text) => setSel((prev) => prev.map((s, i) => (i === qi ? { ...s, text } : s)));
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
              className="w-full bg-gray-800 border border-gray-700 rounded-sm px-2 py-1 text-[12px] text-gray-100 placeholder-gray-600 focus:outline-none focus:border-blue-500" />
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
// rendre + déléguer (sendMessage/answer/cancel/closeConversation).
export default function AgentPanel({ panelKey }) {
  const { slug, convos, sendMessage, answer, cancel, decidePlan, changeMode, changeModel, closeConversation } = useAgentConversations();
  const convo = convos[panelKey];

  const [input, setInput] = useState('');
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

  // Corrélation résultat ↔ outil par `id` (et non par position : les appels d'outils
  // sont souvent parallèles → le résultat ne suit pas forcément son tool_use). Chaque
  // tool_use rend son résultat ; les tool_result corrélés sont sautés (consumed) dans
  // la boucle ; les orphelins (vieux items sans id) gardent le rendu générique.
  const { resultByUseId, consumedResultIdx } = useMemo(() => {
    const byId = new Map();
    const useIds = new Set();
    items.forEach((it) => { if (it.type === 'tool_use' && it.id != null) useIds.add(it.id); });
    const consumed = new Set();
    items.forEach((it, idx) => {
      if (it.type === 'tool_result' && it.tool_use_id != null) {
        byId.set(it.tool_use_id, it);
        if (useIds.has(it.tool_use_id)) consumed.add(idx);
      }
    });
    return { resultByUseId: byId, consumedResultIdx: consumed };
  }, [items]);
  // Tour suspendu sur une interaction (question/plan) : on remplace le spinner générique
  // par "en attente de ta réponse" pour ne pas laisser croire que le modèle calcule.
  const lastItem = items[items.length - 1];
  const awaitingUser =
    !!lastItem &&
    ((lastItem.type === 'question' && !(convo?.answered?.has(lastItem.request_id) || lastItem.answered)) ||
      (lastItem.type === 'plan_review' && !(convo?.decided?.has(lastItem.request_id) || lastItem.decided)));

  // Auto-scroll bas à chaque nouvel item.
  useEffect(() => {
    const el = bodyRef.current;
    if (el) el.scrollTop = el.scrollHeight;
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

  const send = useCallback(() => {
    const prompt = input.trim();
    if (!prompt || running) return;
    sendMessage(panelKey, prompt, buildSettings({ modelId, effort, mode }));
    setInput('');
  }, [input, running, mode, modelId, effort, panelKey, sendMessage]);

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

  const onKeyDown = (e) => {
    if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); send(); }
  };

  if (!convo) return null;

  const titleLabel = convo.title || convo.activeModel || selModel.label;

  return (
    <div className="flex flex-col h-full min-h-0 bg-gray-900">
      {/* En-tête : titre + modèle + fermer */}
      <div className="flex items-center gap-2 h-[34px] shrink-0 px-3 border-b border-gray-800 text-[12px]">
        <Bot className="w-4 h-4 text-blue-400 shrink-0" />
        <span className="text-gray-300 truncate" title={titleLabel}>{convo.title || 'Conversation'}</span>
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

      {/* Fil de conversation */}
      <div ref={bodyRef} className="flex-1 min-h-0 overflow-y-auto px-3 py-2 space-y-2">
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
        {items.map((it, i) => {
          if (it.type === 'user') {
            return (
              <div key={i} className="flex justify-end">
                <div className="max-w-[85%] bg-blue-500/15 text-gray-100 rounded-lg px-3 py-1.5 text-[13px] whitespace-pre-wrap wrap-break-word">
                  {it.text}
                </div>
              </div>
            );
          }
          if (it.type === 'assistant') {
            return <div key={i} className="text-[13px] text-gray-200"><MarkdownView>{it.text}</MarkdownView></div>;
          }
          if (it.type === 'thinking') return <ThinkingBlock key={i} text={it.text} active={running && i === items.length - 1} />;
          if (it.type === 'tool_use') {
            return (
              <ToolCall key={it.id || i} name={it.name} input={it.input}
                result={it.id != null ? resultByUseId.get(it.id) : undefined}
                running={running} isLast={i === items.length - 1} />
            );
          }
          if (it.type === 'tool_result') {
            if (consumedResultIdx.has(i)) return null; // rendu sous son tool_use
            // Orphelin (pas de tool_use corrélé) : rendu générique de secours.
            return (
              <details key={i} className={`text-[11px] ${it.isError ? 'text-red-400' : 'text-gray-500'} pl-5`}>
                <summary className="cursor-pointer select-none flex items-center gap-1">
                  <ChevronRight className="w-3 h-3" /> résultat{it.isError ? ' (erreur)' : ''}
                </summary>
                <pre className="whitespace-pre-wrap wrap-break-word mt-1 font-mono text-gray-400">{it.text}</pre>
              </details>
            );
          }
          if (it.type === 'result') return <ResultFooter key={i} data={it.data} />;
          if (it.type === 'error') {
            return (
              <div key={i} className="text-[12px] text-red-400 flex items-start gap-1.5">
                <AlertTriangle className="w-3.5 h-3.5 shrink-0 mt-0.5" /> {it.message}
              </div>
            );
          }
          if (it.type === 'question') {
            return (
              <QuestionCard key={i}
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
              <PlanReviewCard key={i}
                plan={it.plan}
                decided={convo.decided.has(it.request_id) || !!it.decided}
                approved={it.approved}
                onApprove={(feedback) => decide(it.request_id, true, feedback)}
                onReject={(feedback) => decide(it.request_id, false, feedback)}
              />
            );
          }
          return null;
        })}
        {running && !awaitingUser && (
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

      {/* Sélecteurs. Modèle + mode sont modifiables EN COURS de session (setModel /
          setPermissionMode à chaud). L'effort, lui, est figé au démarrage (pas d'API live) →
          verrouillé pendant une session ; ouvrir une nouvelle conversation pour le changer. */}
      <div className="flex items-center flex-wrap gap-x-4 gap-y-1 px-3 py-1.5 border-t border-gray-800 shrink-0">
        <div className="flex items-center gap-1">
          <span className="text-[11px] text-gray-600 mr-1">Modèle</span>
          <select value={modelId} onChange={(e) => onChangeModel(e.target.value)}
            className="bg-gray-800 border border-gray-700 rounded-sm text-[11px] text-gray-200 px-1 py-[3px] focus:outline-none focus:border-blue-500">
            {MODELS.map((m) => <option key={m.id} value={m.id}>{m.label}</option>)}
          </select>
        </div>
        <div className="flex items-center gap-1">
          <span className="text-[11px] text-gray-600 mr-1">Mode</span>
          {MODES.map((m) => (
            <button key={m.id} onClick={() => onChangeMode(m.id)} title={m.title}
              className={`px-1.5 py-0.5 rounded-sm text-[11px] ${
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
                className={`px-1.5 py-0.5 rounded-sm text-[11px] disabled:opacity-50 disabled:cursor-not-allowed ${effort === e ? 'bg-blue-500/20 text-blue-300' : 'text-gray-500 hover:text-gray-300 hover:bg-gray-800'}`}>
                {e}
              </button>
            ))}
          </div>
        )}
      </div>

      {/* Saisie */}
      <div className="flex items-end gap-2 p-2 border-t border-gray-800 shrink-0">
        <textarea
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={onKeyDown}
          rows={2}
          placeholder="Message à l'agent… (Entrée pour envoyer, Maj+Entrée = nouvelle ligne)"
          className="flex-1 resize-none bg-gray-800 border border-gray-700 rounded-md px-2.5 py-1.5 text-[13px] text-gray-100 placeholder-gray-600 focus:outline-none focus:border-blue-500"
        />
        {running ? (
          <button onClick={stop} title="Interrompre le tour (la conversation reste ouverte)"
            className="p-2 rounded-md bg-red-500/15 text-red-400 hover:bg-red-500/25">
            <Square className="w-4 h-4" />
          </button>
        ) : (
          <button onClick={send} disabled={!input.trim()} title="Envoyer"
            className="p-2 rounded-md bg-blue-500 text-white hover:bg-blue-600 disabled:opacity-30 disabled:cursor-not-allowed">
            <Send className="w-4 h-4" />
          </button>
        )}
      </div>
    </div>
  );
}
