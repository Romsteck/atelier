import { useState, useRef, useEffect, useCallback, useMemo } from 'react';
import {
  Loader2, Bot, ChevronRight, ChevronDown, Wrench, AlertTriangle, X, ListChecks, Brain,
} from 'lucide-react';
import MarkdownView from './docs/MarkdownView';
import Composer from './agent/Composer';
import { getSdkVersion, updateSdk } from '../api/client';
import { apiErr } from '../utils/apiErr';
import { useAgentConversations } from '../context/AgentConversationsContext';
import { describeTool, splitPath, charsToTokens, formatTokens, toolTarget } from '../lib/toolDisplay';
import { TOOL_ICONS, useSmoothCount } from '../lib/toolPresentation';
import { MODELS, MODES, buildSettings, resolveModelId, modelIdFromApi } from '../lib/agentModels';

// Réflexion = compteur SEUL (jamais le texte). Le front ne reçoit que `chars` (le serveur
// n'envoie aucun détail), affiché en tokens (≈ chars/4). Non interactif, rien à déplier.
function ThinkingCount({ chars }) {
  return (
    <span className="flex items-center gap-1.5 text-gray-500" title="réflexion (estimation ≈ caractères / 4)">
      <Brain className="w-3 h-3 shrink-0" />
      Réflexion · {formatTokens(charsToTokens(chars))} tokens
    </span>
  );
}

const TOOL_CHIP = 'shrink-0 text-[10px] uppercase tracking-wider text-gray-400 bg-gray-700/40 px-1.5 py-0.5 rounded-sm';

// Bande « live » : barre PLEINE LARGEUR flush en bas du chat (aucune marge), fond bleu LÉGER,
// bord supérieur de séparation, balayée par un sheen lent (index.css). Couleur de texte
// THEME-AWARE : seul le gris est mirroré par le thème clair (cf. index.css), donc le bleu doit
// l'être à la main via `dark:` — bleu foncé en clair, bleu très clair en sombre — sinon
// illisible en thème clair. Deux rangées empilées : agrégats (si groupe live) puis action.
const LIVE_BAND = 'shrink-0 px-3 py-2 border-t border-blue-500/30 bg-blue-500/10 text-blue-800 dark:text-blue-50 active-line-sheen';

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

// Outils de gestion de checklist : matérialisés dans le bandeau épinglé (TodoBanner), PAS
// dans la liste d'actions (sinon doublon + bruit). `TodoWrite` = ancien système ; `Task*` =
// nouveau système du SDK (≥0.3.x). `TaskGet`/`TaskList` (lectures) sont juste masqués.
const PINNED_TOOLS = new Set(['TodoWrite', 'TaskCreate', 'TaskUpdate', 'TaskGet', 'TaskList']);

// Reconstruit la checklist depuis le flux Task* : `TaskCreate` n'a pas d'id (l'id est
// l'ordinal de création « 1,2,3… »), `TaskUpdate` cible un `taskId` + `status`. On replie
// en liste ordonnée {status, content, activeForm} (même forme que les todos de TodoWrite,
// consommée telle quelle par TodoBanner). `deleted` → retiré.
function reduceTasks(items) {
  const byId = new Map();
  const order = [];
  let created = 0;
  for (const it of items) {
    if (it.type !== 'tool_use') continue;
    if (it.name === 'TaskCreate') {
      const id = String(++created);
      const t = { id, content: it.input?.subject || '', activeForm: it.input?.activeForm || '', status: 'pending' };
      byId.set(id, t);
      order.push(id);
    } else if (it.name === 'TaskUpdate') {
      const id = String(it.input?.taskId ?? '');
      if (!id) continue;
      let t = byId.get(id);
      if (!t) { t = { id, content: '', activeForm: '', status: 'pending' }; byId.set(id, t); order.push(id); }
      if (it.input?.subject) t.content = it.input.subject;
      if (it.input?.activeForm) t.activeForm = it.input.activeForm;
      if (it.input?.status) t.status = it.input.status;
    }
  }
  return order.map((id) => byId.get(id)).filter((t) => t.status !== 'deleted');
}

// Checklist ÉPINGLÉE (sticky) en haut du fil (WHY) : Claude Code réécrit sa todolist en
// continu (N appels TodoWrite/tour). L'afficher inline la faisait « sauter » plus bas à
// chaque MAJ (rendue à la position de la dernière occurrence). Épinglée, elle montre
// TOUJOURS le dernier état, mise à jour en place. Repliable pour libérer de la place.
function TodoBanner({ todos, collapsed = false }) {
  const [open, setOpen] = useState(!collapsed);
  // Auto-repli quand la liste devient « terminée + nouveau tour » (todoStale) : n'agit qu'à
  // la transition vers collapsed=true → l'utilisateur peut toujours la rouvrir d'un clic.
  useEffect(() => { if (collapsed) setOpen(false); }, [collapsed]);
  const total = todos.length;
  const done = todos.filter((t) => t.status === 'completed').length;
  const allDone = total > 0 && done === total;
  // Hors du flux scrollé, collé sous l'en-tête (plus de gap). Fond légèrement élevé +
  // ombre portée + `z-10` (l'ombre passe AU-DESSUS du fil qui suit) pour le démarquer.
  return (
    <div className="relative z-10 shrink-0 px-3 py-1.5 bg-gray-800/60 border-b border-gray-700 shadow-[0_2px_6px_rgba(0,0,0,0.35)]">
      <button onClick={() => setOpen((o) => !o)} className="w-full flex items-center gap-1.5 text-[12px] text-gray-400 hover:text-gray-200">
        {open ? <ChevronDown className="w-3.5 h-3.5 shrink-0" /> : <ChevronRight className="w-3.5 h-3.5 shrink-0" />}
        <ListChecks className="w-3.5 h-3.5 shrink-0" />
        <span className="text-gray-300">Todos</span>
        <span className={allDone ? 'text-green-500' : 'text-gray-600'}>{done}/{total}</span>
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

// Une ligne d'outil dans le détail déplié d'un groupe d'activité.
function ToolRow({ tool }) {
  const d = describeTool(tool.name, tool.input);
  const Icon = TOOL_ICONS[d.iconKey] || Wrench;
  const err = tool.result?.isError;
  return (
    <li className="flex items-center gap-1.5 min-w-0">
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
}

// Une SUITE contiguë d'activité INTERNE d'un tour (WHY) : appels d'outils ET réflexions
// entremêlés (« 1 action ▸ thinking ▸ 1 action ▸ thinking… »). Tout est fusionné en UN groupe
// repliable : entête « N actions · 🧠 X tokens » (le compteur de réflexions agrégé, jamais le
// texte), détail déplié = liste des OUTILS seulement (les réflexions ne sont plus lisibles).
function ActivityGroup({ entries }) {
  const [open, setOpen] = useState(false);
  const toolEntries = entries.filter((e) => e.kind === 'tool');
  const nTools = toolEntries.length;
  const anyError = toolEntries.some((e) => e.result?.isError);
  const reflectionTokens = charsToTokens(
    entries.filter((e) => e.kind === 'thinking').reduce((s, e) => s + (e.chars || 0), 0),
  );

  return (
    <div className="text-[12px] my-1">
      <button onClick={() => setOpen((o) => !o)} className="flex items-center gap-1.5 max-w-full text-gray-500 hover:text-gray-300">
        {open ? (
          <ChevronDown className="w-3.5 h-3.5 shrink-0" />
        ) : (
          <ChevronRight className="w-3.5 h-3.5 shrink-0" />
        )}
        <span className="flex items-center gap-1.5 min-w-0">
          {nTools > 0 && <span className="shrink-0">{nTools} action{nTools > 1 ? 's' : ''}</span>}
          {reflectionTokens > 0 && (
            <span className="flex items-center gap-1 shrink-0">
              {nTools > 0 && <span className="text-gray-600">·</span>}
              <Brain className="w-3 h-3 shrink-0" /> {formatTokens(reflectionTokens)} tokens
            </span>
          )}
          {anyError && <span className="text-red-400 shrink-0" title="une action a échoué">●</span>}
        </span>
      </button>
      {open && nTools > 0 && (
        <ul className="mt-1 ml-5 space-y-0.5">
          {toolEntries.map((t, i) => <ToolRow key={t.id || `a-${i}`} tool={t} />)}
        </ul>
      )}
    </div>
  );
}

// Barre LIVE persistante, TOUJOURS en bas du chat (flush, hors flux scrollé) tant que l'agent
// travaille : reflète l'activité courante — réflexion en cours, outil en cours / dernier outil,
// sinon générique « agent travaille… ». Quand la queue du fil est un groupe d'activité EN COURS
// (`group`), ses agrégats (« N actions · 🧠 X tokens ») vivent ICI, à droite dans la bande
// (l'entête ActivityGroup ne réapparaît dans le flux qu'à la clôture de la suite) — avec UN
// SEUL compteur de tokens (le cumul du groupe inclut la réflexion en cours), au lieu de
// l'ancien duo entête agrégée + compteur live. Balayée par un sheen lent (index.css).
function LiveBand({ activity, group }) {
  const thinkingLive = activity.kind === 'thinking';
  const tokTarget = group ? group.reflectionTokens : charsToTokens(thinkingLive ? activity.chars : 0);
  const thinkShown = useSmoothCount(tokTarget, thinkingLive || !!group);
  const desc = activity.kind === 'tool' ? describeTool(activity.name, activity.input) : null;
  const Icon = desc ? TOOL_ICONS[desc.iconKey] || Wrench : null;
  const accent = 'text-blue-600 dark:text-blue-200';
  return (
    <div className={LIVE_BAND}>
      {group && (
        <div className="flex items-center gap-2 mb-1 text-[12px] font-medium text-blue-700/90 dark:text-blue-200/90">
          {group.nTools > 0 && <span className="shrink-0">{group.nTools} action{group.nTools > 1 ? 's' : ''}</span>}
          {group.reflectionTokens > 0 && (
            <span className="flex items-center gap-1 shrink-0" title="réflexion cumulée de la suite en cours (≈ caractères / 4)">
              {group.nTools > 0 && <span className="text-blue-600/50 dark:text-blue-200/50">·</span>}
              <Brain className="w-3.5 h-3.5 shrink-0" />
              <span className="tabular-nums">{formatTokens(thinkShown)} tokens</span>
            </span>
          )}
          {group.anyError && <span className="text-red-400 shrink-0" title="une action a échoué">●</span>}
        </div>
      )}
      <div className="flex items-center gap-2 text-[15px] font-semibold min-w-0">
        <Loader2 className={`w-4 h-4 shrink-0 animate-spin ${accent}`} />
        {thinkingLive ? (
          <>
            <Brain className={`w-4 h-4 shrink-0 ${accent}`} />
            {group ? (
              <span className="shrink-0">réflexion</span>
            ) : (
              <span className="tabular-nums shrink-0" title="réflexion en cours (≈ caractères / 4)">{formatTokens(thinkShown)} tokens</span>
            )}
            <span className="text-blue-600/60 dark:text-blue-200/60 shrink-0">…</span>
          </>
        ) : desc ? (
          <>
            <Icon className={`w-4 h-4 shrink-0 ${accent}`} />
            <span className="shrink-0">{desc.verb}</span>
            <span className="truncate font-mono text-blue-700/90 dark:text-blue-100/90 min-w-0">{toolTarget(desc)}</span>
            <span className="text-blue-600/60 dark:text-blue-200/60 shrink-0">…</span>
          </>
        ) : (
          <span>agent travaille…</span>
        )}
      </div>
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
function QuestionCard({ questions, answered, answerText, idle, onSubmit, onCancel }) {
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
            <span className="inline-block text-[10px] uppercase tracking-wider text-blue-700 dark:text-blue-300 bg-blue-500/15 px-1.5 py-0.5 rounded-sm">{q.header}</span>
          )}
          <div className="text-[13px] text-gray-200">{q.question}</div>
          <div className="flex flex-col gap-1.5">
            {(q.options || []).map((o, oi) => {
              const chosen = sel[qi].chosen.has(o.label);
              return (
                <button key={oi} disabled={answered} onClick={() => setChosen(qi, o.label, !!q.multiSelect)}
                  className={`text-left px-2.5 py-1.5 rounded-md text-[12px] border ${chosen ? 'bg-blue-500/25 border-blue-500/50 text-blue-800 dark:text-blue-100' : 'border-gray-700 text-gray-300 hover:bg-gray-800'} disabled:opacity-60`}>
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
                  ? 'border-blue-500/50 ring-1 ring-blue-500/30 text-blue-800 dark:text-blue-100' // mode custom actif
                  : 'border-gray-700 text-gray-100 focus:border-blue-500'
              }`} />
          )}
        </div>
      ))}
      {answered ? (
        <div className="text-[11px] text-gray-500">{answerText ? `Réponse : ${answerText}` : 'Réponse envoyée.'}</div>
      ) : (
        <>
          {idle && (
            <div className="text-[11px] italic text-amber-700 dark:text-amber-400/90">
              L'agent s'est mis en pause en attendant ta réponse — réponds pour reprendre.
            </div>
          )}
          <div className="flex items-center gap-2">
            <button onClick={() => onSubmit(build())} disabled={!hasAny}
              className="px-3 py-1 rounded-md text-[12px] bg-blue-500 text-white hover:bg-blue-600 disabled:opacity-40 disabled:cursor-not-allowed">
              Répondre
            </button>
            <button onClick={onCancel} className="px-2 py-1 rounded-md text-[12px] text-gray-400 hover:text-gray-200 hover:bg-gray-800">
              Passer
            </button>
          </div>
        </>
      )}
    </div>
  );
}

// Carte d'approbation de plan (ExitPlanMode). Affiche le plan proposé et propose à
// l'utilisateur d'implémenter (la session bascule en édition, même mémoire) ou de le
// renvoyer en révision (avec remarques optionnelles). Tant que non décidé, le tour est
// suspendu côté runner (canUseTool).
function PlanReviewCard({ plan, decided, approved, idle, onApprove, onReject }) {
  const [feedback, setFeedback] = useState('');
  return (
    <div className="border border-amber-500/30 bg-amber-500/5 rounded-lg p-3 my-1 space-y-2">
      <span className="inline-block text-[10px] uppercase tracking-wider text-amber-700 dark:text-amber-300 bg-amber-500/15 px-1.5 py-0.5 rounded-sm">
        Plan proposé
      </span>
      <div className="text-[13px] text-gray-200"><MarkdownView>{plan || '(plan vide)'}</MarkdownView></div>
      {decided ? (
        <div className="text-[11px] text-gray-500">
          {approved ? '✅ Plan approuvé — implémentation en cours.' : '↩︎ Renvoyé en révision.'}
        </div>
      ) : (
        <>
          {idle && (
            <div className="text-[11px] italic text-amber-700 dark:text-amber-400/90">
              L'agent s'est mis en pause en attendant ta décision — décide pour reprendre.
            </div>
          )}
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

// Contrôle segmenté (Mode / Effort) : groupe arrondi, segment actif « en relief ».
// Les gris sont auto-mirrorés par le thème (index.css) — seuls les accents couleur
// portent une variante dark:. `accent(id)` → 'blue' | 'amber' colore par option.
function Segmented({ options, value, onChange, disabled = false, title, accent }) {
  return (
    <div
      title={title}
      className={`inline-flex items-center rounded-md p-0.5 border border-gray-700/80 bg-gray-800/60 ${disabled ? 'opacity-40' : ''}`}
    >
      {options.map((o) => {
        const active = value === o.id;
        const color = (typeof accent === 'function' ? accent(o.id) : accent) === 'amber'
          ? 'text-amber-700 dark:text-amber-300'
          : 'text-blue-700 dark:text-blue-300';
        return (
          <button
            key={o.id}
            disabled={disabled}
            onClick={() => onChange(o.id)}
            title={o.title}
            className={`px-2 py-0.5 min-h-[28px] sm:min-h-0 rounded-[5px] text-[11px] transition-colors ${
              disabled
                ? 'cursor-not-allowed'
                : active
                  ? ''
                  : 'hover:text-gray-200 hover:bg-gray-700/50'
            } ${active ? `bg-white dark:bg-gray-700 shadow-sm font-medium ${color}` : 'text-gray-500'}`}
          >
            {o.label}
          </button>
        );
      })}
    </div>
  );
}

// Panneau d'UNE conversation. Contrôlé : tout l'état (items, running, runId, question
// en attente) vit dans le provider, indexé par `panelKey`. Le panneau ne fait que
// rendre + déléguer (sendMessage/answer/cancel/closeConversation). La SAISIE est isolée
// dans <Composer> (état local) pour que taper ne re-render pas la liste des messages.
export default function AgentPanel({ panelKey }) {
  const { slug, convos, convName, sendMessage, answer, cancel, decidePlan, changeMode, changeModel, changeEffort, closeConversation } = useAgentConversations();
  const convo = convos[panelKey];

  const [modelId, setModelId] = useState(() => resolveModelId(localStorage.getItem('agent:model')));
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

  // Préférences globales (localStorage) : persistées UNIQUEMENT sur choix délibéré
  // (onChangeModel / chooseEffort / onChangeMode) — jamais par les syncs backend→UI
  // ci-dessous (settings serveur, activeMode/activeModel, effort imposé par
  // « Résoudre tout ») qui écraseraient la préférence de l'utilisateur.

  // Vérité session → sélecteur MODÈLE : priorité au modèle RÉSOLU annoncé live
  // (activeModel, events system/model), sinon au modèle demandé persisté serveur
  // (settings.model — null = défaut Opus explicite). `undefined` = aucune info
  // (brouillon neuf / conversation legacy sans meta) → préférence locale conservée.
  const serverModel = convo?.activeModel ?? (convo?.settings ? (convo.settings.model ?? null) : undefined);
  useEffect(() => {
    if (serverModel !== undefined) setModelId(modelIdFromApi(serverModel));
  }, [serverModel]);

  // Reflète l'effort de la conversation (settings serveur au chargement, ou effort
  // imposé au lancement ex. 'max' depuis « Résoudre ») dès qu'il est connu.
  useEffect(() => {
    if (convo?.effort) setEffort(convo.effort);

  }, [convo?.effort]);

  // Changement délibéré d'effort par l'utilisateur → applique + mémorise comme
  // préférence, et délègue au contexte (persistance meta serveur + recycle éventuel
  // de la session vivante : l'effort SDK est figé au démarrage, pas d'API live —
  // le prochain message reprend en resume avec le nouvel effort et toute la mémoire).
  const chooseEffort = useCallback((e) => {
    if (e === effort) return;
    setEffort(e);
    localStorage.setItem('agent:effort', e);
    changeEffort(panelKey, e);
  }, [effort, changeEffort, panelKey]);
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

  // Nœuds de rendu : une suite contiguë d'activité INTERNE (tool_use/tool_result/thinking,
  // hors outils épinglés) est fusionnée en UN groupe (`kind:'activity'`) DÈS qu'elle contient
  // ≥1 outil — la réflexion devient alors un détail du groupe. Une suite de réflexions SEULES
  // (aucun outil) reste rendue en blocs directs (`kind:'thinkitem'`) pour ne pas sur-emballer
  // une réflexion isolée. Le reste (prose/user/result/question) coupe le groupe.
  const isActivity = (it) => it.type === 'tool_use' || it.type === 'tool_result' || it.type === 'thinking';
  const renderNodes = useMemo(() => {
    const nodes = [];
    let i = 0;
    while (i < items.length) {
      const it = items[i];
      if (isActivity(it)) {
        const entries = [];
        let j = i;
        while (j < items.length && isActivity(items[j])) {
          const t = items[j];
          if (t.type === 'tool_use' && !PINNED_TOOLS.has(t.name)) {
            entries.push({ kind: 'tool', id: t.id, name: t.name, input: t.input, result: t.id != null ? resultByUseId.get(t.id) : undefined });
          } else if (t.type === 'thinking') {
            entries.push({ kind: 'thinking', chars: t.chars, idx: j });
          }
          j++;
        }
        if (entries.some((e) => e.kind === 'tool')) {
          nodes.push({ kind: 'activity', entries, endIdx: j - 1, key: `act-${i}` });
        } else {
          for (const e of entries) nodes.push({ kind: 'thinkitem', chars: e.chars, key: `th-${e.idx}` });
        }
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

  // Checklist épinglée : système Task* du SDK s'il est utilisé (reconstruit), sinon repli
  // sur l'ancien TodoWrite. Une session n'emploie que l'un des deux.
  const reducedTasks = useMemo(() => reduceTasks(items), [items]);
  const pinnedTodos = reducedTasks.length ? reducedTasks : latestTodos;

  // Bandeau « terminé + nouveau tour » (WHY) : garantie visuelle déterministe. Si la liste
  // épinglée est 100% complétée ET qu'un message utilisateur est arrivé APRÈS le dernier
  // tool_use qui la définit, on la replie — le bandeau ne reste pas bloqué sur du ✓✓✓ périmé,
  // même quand l'agent oublie de le réinitialiser. Couvre aussi le cas resume (dérivé des items).
  const todoStale = useMemo(() => {
    const todos = pinnedTodos;
    if (!todos?.length || !todos.every((t) => t.status === 'completed')) return false;
    const isTodoTool = (it) =>
      it.type === 'tool_use' && (it.name === 'TodoWrite' || it.name === 'TaskCreate' || it.name === 'TaskUpdate');
    let lastTodoIdx = -1;
    for (let k = items.length - 1; k >= 0; k--) { if (isTodoTool(items[k])) { lastTodoIdx = k; break; } }
    if (lastTodoIdx < 0) return false;
    for (let k = lastTodoIdx + 1; k < items.length; k++) { if (items[k].type === 'user') return true; }
    return false;
  }, [pinnedTodos, items]);

  // Tour suspendu sur une interaction (question/plan) : on remplace le spinner générique
  // par "en attente de ta réponse" pour ne pas laisser croire que le modèle calcule.
  const lastItem = items[items.length - 1];
  const awaitingUser =
    !!lastItem &&
    ((lastItem.type === 'question' && !(convo?.answered?.has(lastItem.request_id) || lastItem.answered)) ||
      (lastItem.type === 'plan_review' && !(convo?.decided?.has(lastItem.request_id) || lastItem.decided)));
  // Activité LIVE courante, dérivée de la queue des items → rendue dans la barre persistante
  // en bas du chat (`LiveBand`). Réflexion en cours (compteur animé) ; outil en cours OU
  // dernier outil (micro-gap tool_result) ; sinon générique. Outils épinglés (Task*/TodoWrite)
  // = bookkeeping → on remonte au dernier vrai outil, sinon générique.
  const liveActivity = useMemo(() => {
    if (!running) return null;
    const last = items[items.length - 1];
    if (!last) return { kind: 'generic' };
    if (last.type === 'thinking') return { kind: 'thinking', chars: last.chars || 0 };
    if (last.type === 'tool_use' && !PINNED_TOOLS.has(last.name)) return { kind: 'tool', name: last.name, input: last.input };
    if (last.type === 'tool_result' || last.type === 'tool_use') {
      for (let k = items.length - 1; k >= 0; k--) {
        const it = items[k];
        if (it.type === 'tool_use' && !PINNED_TOOLS.has(it.name)) return { kind: 'tool', name: it.name, input: it.input };
        if (it.type === 'assistant' || it.type === 'user') break;
      }
    }
    return { kind: 'generic' };
  }, [items, running]);

  // Suite d'activité EN COURS en queue de fil (WHY) : tant qu'elle grandit, son entête
  // agrégée (« N actions · 🧠 X tokens ») est retirée du flux et rendue DANS la LiveBand
  // (un seul compteur de tokens — le cumul du groupe inclut la réflexion live). Idem pour
  // le bloc de réflexion SEUL en cours (sinon double compteur flux + bande). Le nœud
  // réapparaît dans le flux dès que la suite se clôt (prose/fin de tour/dialogue).
  const bandLive = running && !awaitingUser;
  const lastNode = renderNodes[renderNodes.length - 1];
  const liveGroupNode =
    bandLive && lastNode?.kind === 'activity' && lastNode.endIdx === items.length - 1 ? lastNode : null;
  const liveThinkTail =
    bandLive && !liveGroupNode && lastNode?.kind === 'thinkitem' &&
    items[items.length - 1]?.type === 'thinking';
  const flowNodes = liveGroupNode || liveThinkTail ? renderNodes.slice(0, -1) : renderNodes;
  const liveGroupStats = useMemo(() => {
    if (!liveGroupNode) return null;
    const tools = liveGroupNode.entries.filter((e) => e.kind === 'tool');
    return {
      nTools: tools.length,
      anyError: tools.some((e) => e.result?.isError),
      reflectionTokens: charsToTokens(
        liveGroupNode.entries.filter((e) => e.kind === 'thinking').reduce((s, e) => s + (e.chars || 0), 0),
      ),
    };
  }, [liveGroupNode]);

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
    localStorage.setItem('agent:model', id); // choix délibéré → préférence globale
    const m = MODELS.find((x) => x.id === id);
    if (m && m.efforts.length && !m.efforts.includes(effort)) chooseEffort(m.efforts[m.efforts.length - 1]);
    if (live) changeModel(panelKey, m?.model || null); // session vivante → setModel à chaud
  };
  const onChangeMode = (id) => {
    setMode(id);
    localStorage.setItem('agent:mode', id); // choix délibéré → préférence globale
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
      const ver = r.data?.installed ?? v.data?.installed ?? '';
      if (r.data?.source_pinned) {
        // Pin source bumpé → durable : survit aux make deploy (pense à committer le pin).
        setSdkMsg({ ok: true, text: `SDK à jour (${ver}) — pin source mis à jour (à committer)` });
      } else {
        // MAJ appliquée au déployé mais pin source non bumpé → reviendra au prochain deploy.
        const note = r.data?.source_note ? ` (${r.data.source_note})` : '';
        setSdkMsg({ ok: false, text: `MAJ ${ver} appliquée mais pin source non mis à jour${note} — reviendra au prochain deploy` });
      }
    } catch (e) {
      setSdkMsg({ ok: false, text: apiErr(e, 'MAJ SDK échouée') });
    } finally {
      setUpdatingSdk(false);
    }
  }, []);

  if (!convo) return null;

  const displayName = convName(convo);

  // Rendu d'un item NON-activité (outils + réflexions passent par ActivityGroup/thinkitem).
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
      // Carte discrète (fond + bord + padding) : délimite chaque PAQUET de prose. Avant, les
      // blocs de texte successifs (séparés par des groupes d'activité) se fondaient en un
      // seul pavé difficile à lire.
      return (
        <div key={`i-${i}`} className="text-[13px] text-gray-200 bg-gray-800/30 border border-gray-800 rounded-lg px-3 py-2">
          <MarkdownView>{it.text}</MarkdownView>
        </div>
      );
    }
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
          idle={!!it.idle}
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
          idle={!!it.idle}
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

      {/* Checklist épinglée : HORS du flux scrollé, collée sous l'en-tête (plus de gap),
          fond + ombre distincts. Repliée d'elle-même quand terminée + nouveau tour (todoStale). */}
      {pinnedTodos?.length > 0 && <TodoBanner todos={pinnedTodos} collapsed={todoStale} />}

      {/* Fil de conversation. Wrapper `relative` : le bouton « Nouveaux messages » s'ancre
          au bas VISIBLE du panneau (il ne défile pas avec le contenu). */}
      <div className="relative flex-1 min-h-0">
      <div ref={bodyRef} onScroll={onScroll} className="h-full overflow-y-auto px-3 py-2 space-y-2">
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
        {flowNodes.map((node) =>
          node.kind === 'activity' ? (
            <ActivityGroup key={node.key} entries={node.entries} />
          ) : node.kind === 'thinkitem' ? (
            <div key={node.key} className="text-[12px] border-l-2 border-gray-700 pl-2 my-1">
              <ThinkingCount chars={node.chars} />
            </div>
          ) : (
            renderItem(node.it, node.idx)
          ),
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

      {/* Barre LIVE TOUJOURS en bas du chat (hors flux scrollé, flush) tant que l'agent
          travaille — sauf en attente d'une réponse (carte question/plan affichée à la place). */}
      {running && !awaitingUser && liveActivity && <LiveBand activity={liveActivity} group={liveGroupStats} />}

      {/* Sélecteurs. Modèle + mode sont modifiables EN COURS de session (setModel /
          setPermissionMode à chaud). L'effort est figé côté SDK au démarrage : le changer
          sur une session vivante mais idle la RECYCLE en douceur (cancel → resume au
          prochain message, mémoire conservée) ; pendant un tour en vol il est verrouillé.
          Les valeurs affichées se resynchronisent sur les settings serveur de la
          conversation (agent_conversation_meta) — cohérentes entre PCs. */}
      <div className="flex items-center flex-wrap gap-x-3 gap-y-1.5 px-3 py-1.5 border-t border-gray-800 bg-gray-950/40 shrink-0">
        <label className="flex items-center gap-1.5 w-full sm:w-auto">
          <span className="text-[10px] uppercase tracking-wider text-gray-500">Modèle</span>
          <select value={modelId} onChange={(e) => onChangeModel(e.target.value)}
            className="flex-1 sm:flex-none h-[29px] rounded-md border border-gray-700/80 bg-gray-800/60 text-[11px] text-gray-200 px-1.5 focus:outline-none focus:border-blue-500">
            {MODELS.map((m) => <option key={m.id} value={m.id}>{m.label}</option>)}
          </select>
        </label>
        <div className="flex items-center gap-1.5">
          <span className="text-[10px] uppercase tracking-wider text-gray-500">Mode</span>
          <Segmented
            options={MODES.map((m) => ({ id: m.id, label: m.label, title: m.title }))}
            value={mode}
            onChange={onChangeMode}
            accent={(id) => (id === 'bypass' ? 'amber' : 'blue')}
          />
        </div>
        {selModel.efforts.length > 0 && (
          <div className="flex items-center gap-1.5">
            <span className="text-[10px] uppercase tracking-wider text-gray-500">Effort</span>
            <Segmented
              options={selModel.efforts.map((e) => ({ id: e, label: e }))}
              value={effort}
              onChange={chooseEffort}
              disabled={running}
              title={
                running
                  ? 'Effort modifiable dès la fin du tour en cours'
                  : live
                    ? 'Changer l’effort redémarre la session en douceur : le prochain message reprend avec la mémoire complète et le nouvel effort'
                    : undefined
              }
            />
          </div>
        )}
      </div>

      {/* Saisie (état local isolé) */}
      <Composer onSend={onSend} running={running} onStop={stop} />
    </div>
  );
}
