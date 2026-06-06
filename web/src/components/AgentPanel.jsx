import { useState, useRef, useEffect, useCallback } from 'react';
import { Send, Square, Loader2, Bot, ChevronRight, Wrench, AlertTriangle } from 'lucide-react';
import useWebSocket from '../hooks/useWebSocket';
import { startAgentQuery, cancelAgentRun, answerAgentRun, getSdkVersion, updateSdk } from '../api/client';
import MarkdownView from './docs/MarkdownView';

// Modèles sélectionnables. Opus 4.8 par défaut = on N'ENVOIE PAS de model → le CLI
// résout le défaut de l'abonnement, soit `claude-opus-4-8[1m]` (contexte 1M).
// `efforts` = niveaux supportés (xhigh/max = Opus ; Haiku n'a aucun param effort).
const MODELS = [
  { id: 'opus-4-8', label: 'Opus 4.8', model: null, efforts: ['low', 'medium', 'high', 'xhigh', 'max'] },
  { id: 'opus-4-7', label: 'Opus 4.7', model: 'claude-opus-4-7', efforts: ['low', 'medium', 'high', 'xhigh', 'max'] },
  { id: 'sonnet-4-6', label: 'Sonnet 4.6', model: 'claude-sonnet-4-6', efforts: ['low', 'medium', 'high'] },
  { id: 'haiku-4-5', label: 'Haiku 4.5', model: 'claude-haiku-4-5', efforts: [] },
];
// Deux modes seulement (cf. runner.js) : Plan = lecture seule (explore + planifie),
// Bypass = pleine capacité (édite/exécute, relu via l'onglet Git).
const MODES = [
  { id: 'plan', label: 'Plan', pm: 'plan', title: 'Lecture seule : explore et planifie, n’écrit rien.' },
  { id: 'bypass', label: 'Bypass', pm: 'bypassPermissions', title: 'Pleine capacité : édite les fichiers, exécute, MCP. À relire dans l’onglet Git.' },
];

// Reconstruit un fil de conversation à partir du flux NDJSON normalisé (deltas
// déjà coalescés côté backend). On accumule les deltas consécutifs d'un même
// type dans le dernier item ouvert ; les autres kinds créent des items discrets.
function appendEvent(items, ev) {
  const next = items.slice();
  const last = next[next.length - 1];
  switch (ev.kind) {
    case 'assistant_delta': {
      const text = ev.data?.text || '';
      if (last && last.type === 'assistant') last.text += text;
      else next.push({ type: 'assistant', text });
      break;
    }
    case 'thinking_delta': {
      const text = ev.data?.text || '';
      if (last && last.type === 'thinking') last.text += text;
      else next.push({ type: 'thinking', text });
      break;
    }
    case 'tool_use':
      next.push({ type: 'tool_use', name: ev.data?.name, input: ev.data?.input });
      break;
    case 'tool_result':
      next.push({ type: 'tool_result', text: ev.data?.text || '', isError: !!ev.data?.is_error });
      break;
    case 'result':
      next.push({ type: 'result', data: ev.data });
      break;
    case 'error':
      next.push({ type: 'error', message: ev.data?.message || 'erreur' });
      break;
    default:
      break; // system / started / done : non rendus dans le fil
  }
  return next;
}

function ThinkingBlock({ text }) {
  return (
    <details className="text-[12px] text-gray-400 border-l-2 border-gray-700 pl-2 my-1">
      <summary className="cursor-pointer select-none text-gray-500 hover:text-gray-300">Réflexion…</summary>
      <div className="whitespace-pre-wrap mt-1 italic">{text}</div>
    </details>
  );
}

function ToolUse({ name, input }) {
  const inputStr = (() => {
    try { return JSON.stringify(input); } catch { return ''; }
  })();
  return (
    <div className="text-[12px] text-blue-300/90 flex items-start gap-1.5 my-1 font-mono">
      <Wrench className="w-3.5 h-3.5 shrink-0 mt-0.5" />
      <span className="min-w-0 wrap-break-word">
        <span className="text-blue-200">{name}</span>
        {inputStr && inputStr !== '{}' && <span className="text-gray-500"> {inputStr.slice(0, 200)}</span>}
      </span>
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

// Carte de question interactive (AskUserQuestion natif via onUserDialog côté runner).
// Affiche 1-4 questions avec options ; collecte les choix + une réponse libre par
// question, puis renvoie { [texte_question]: réponse } au run via /answer.
function QuestionCard({ questions, answered, onSubmit, onCancel }) {
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
          <div className="flex flex-wrap gap-1.5">
            {(q.options || []).map((o, oi) => {
              const chosen = sel[qi].chosen.has(o.label);
              return (
                <button key={oi} disabled={answered} onClick={() => setChosen(qi, o.label, !!q.multiSelect)} title={o.description}
                  className={`px-2 py-1 rounded-md text-[12px] border ${chosen ? 'bg-blue-500/25 border-blue-500/50 text-blue-100' : 'border-gray-700 text-gray-300 hover:bg-gray-800'} disabled:opacity-60`}>
                  {o.label}
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
        <div className="text-[11px] text-gray-500">Réponse envoyée.</div>
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

export default function AgentPanel({ slug }) {
  const [items, setItems] = useState([]);
  const [input, setInput] = useState('');
  const [modelId, setModelId] = useState(() => localStorage.getItem('agent:model') || 'opus-4-8');
  const [effort, setEffort] = useState(() => localStorage.getItem('agent:effort') || 'max');
  const [mode, setMode] = useState(() => localStorage.getItem('agent:mode') || 'plan');
  const [activeModel, setActiveModel] = useState(null); // modèle réel (depuis l'event system)
  const [answered, setAnswered] = useState(() => new Set()); // request_id des questions répondues
  const [runId, setRunId] = useState(null);
  const [running, setRunning] = useState(false);
  const [err, setErr] = useState(null);
  const [sdk, setSdk] = useState(null);
  const bodyRef = useRef(null);
  const runIdRef = useRef(null);
  runIdRef.current = runId;

  // Live stream : on ne garde que les events du run actif (le WS est global).
  useWebSocket({
    'agent:event': (d) => {
      if (!d || d.run_id !== runIdRef.current) return;
      if (d.kind === 'done') { setRunning(false); return; }
      if (d.kind === 'started') { setRunning(true); return; }
      if (d.kind === 'system') { if (d.data?.model) setActiveModel(d.data.model); return; }
      if (d.kind === 'question') {
        setItems((prev) => [...prev, { type: 'question', request_id: d.data?.request_id, questions: d.data?.questions || [] }]);
        return;
      }
      setItems((prev) => appendEvent(prev, d));
    },
  });

  // Auto-scroll bas à chaque nouvel item.
  useEffect(() => {
    const el = bodyRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [items, running]);

  useEffect(() => {
    getSdkVersion().then((r) => setSdk(r.data)).catch(() => {});
  }, []);

  // Mémorise les choix → ils deviennent les défauts à la prochaine ouverture.
  useEffect(() => { localStorage.setItem('agent:model', modelId); }, [modelId]);
  useEffect(() => { localStorage.setItem('agent:effort', effort); }, [effort]);
  useEffect(() => { localStorage.setItem('agent:mode', mode); }, [mode]);

  const selModel = MODELS.find((m) => m.id === modelId) || MODELS[0];

  // Changer de modèle : si l'effort courant n'est pas supporté, retombe sur le plus
  // haut dispo du nouveau modèle (ex. Opus 'max' → Sonnet 'high').
  const onChangeModel = (id) => {
    setModelId(id);
    const m = MODELS.find((x) => x.id === id);
    if (m && m.efforts.length && !m.efforts.includes(effort)) {
      setEffort(m.efforts[m.efforts.length - 1]);
    }
  };

  const send = useCallback(async () => {
    const prompt = input.trim();
    if (!prompt || running) return;
    setErr(null);
    setItems((prev) => [...prev, { type: 'user', text: prompt }]);
    setInput('');
    setRunning(true);
    try {
      const permission_mode = MODES.find((m) => m.id === mode)?.pm || 'plan';
      const sm = MODELS.find((m) => m.id === modelId) || MODELS[0];
      const body = { prompt, permission_mode };
      if (sm.model) body.model = sm.model;      // Opus 4.8 (model:null) → défaut [1m] conservé
      if (sm.efforts.length) body.effort = effort; // Haiku → pas de param effort
      const r = await startAgentQuery(slug, body);
      setRunId(r.data?.run_id || null);
    } catch (e) {
      setRunning(false);
      setErr(e.response?.data?.error || e.message);
    }
  }, [input, running, slug, effort, mode, modelId]);

  const stop = useCallback(async () => {
    if (!runId) return;
    try { await cancelAgentRun(slug, runId); } catch { /* déjà terminé */ }
    setRunning(false);
  }, [runId, slug]);

  const submitAnswer = useCallback((request_id, payload) => {
    const rid = runIdRef.current;
    if (rid) answerAgentRun(slug, rid, { request_id, ...payload }).catch(() => {});
    setAnswered((s) => new Set(s).add(request_id));
  }, [slug]);

  const onUpdateSdk = useCallback(async () => {
    try {
      const r = await updateSdk();
      setErr(r.data?.message || 'MAJ lancée');
    } catch (e) {
      setErr(e.response?.data?.error || 'MAJ non disponible');
    }
  }, []);

  const onKeyDown = (e) => {
    if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); send(); }
  };

  return (
    <div className="flex flex-col h-full min-h-0 bg-gray-900">
      {/* En-tête : app + badge SDK */}
      <div className="flex items-center gap-2 h-[34px] shrink-0 px-3 border-b border-gray-800 text-[12px]">
        <Bot className="w-4 h-4 text-blue-400" />
        <span className="text-gray-300">Agent</span>
        <span className="text-gray-600">·</span>
        <span className="text-gray-500">{slug}</span>
        <span className="text-gray-600">·</span>
        <span className="text-gray-400 truncate max-w-[160px]" title={`modèle ${activeModel ? 'actif' : 'sélectionné'} : ${activeModel || selModel.label}`}>
          {activeModel || selModel.label}
        </span>
        <div className="ml-auto flex items-center gap-2">
          {sdk?.installed && (
            <span className="text-gray-600" title="version Agent SDK installée">SDK {sdk.installed}</span>
          )}
          {sdk?.update_available && (
            <button onClick={onUpdateSdk}
              className="px-1.5 py-0.5 rounded-sm bg-amber-500/15 text-amber-400 hover:bg-amber-500/25"
              title={`MAJ disponible : ${sdk.installed} → ${sdk.latest}`}>
              MAJ {sdk.latest}
            </button>
          )}
        </div>
      </div>

      {/* Fil de conversation */}
      <div ref={bodyRef} className="flex-1 min-h-0 overflow-y-auto px-3 py-2 space-y-2">
        {items.length === 0 && (
          <div className="text-[13px] text-gray-600 mt-4 text-center">
            Pose une question à l’agent sur <span className="text-gray-400">{slug}</span>.
            <div className="text-[11px] mt-1 text-gray-700">
              <span className="text-blue-400/80">Plan</span> = lecture seule ·{' '}
              <span className="text-amber-400/80">Bypass</span> = édite &amp; exécute (relu dans Git).
            </div>
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
          if (it.type === 'thinking') return <ThinkingBlock key={i} text={it.text} />;
          if (it.type === 'tool_use') return <ToolUse key={i} name={it.name} input={it.input} />;
          if (it.type === 'tool_result') {
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
                answered={answered.has(it.request_id)}
                onSubmit={(answers) => submitAnswer(it.request_id, { answers })}
                onCancel={() => submitAnswer(it.request_id, { cancelled: true })}
              />
            );
          }
          return null;
        })}
        {running && (
          <div className="flex items-center gap-1.5 text-[12px] text-gray-500">
            <Loader2 className="w-3.5 h-3.5 animate-spin" /> …
          </div>
        )}
      </div>

      {err && <div className="px-3 py-1 text-[11px] text-red-400 border-t border-gray-800 shrink-0">{err}</div>}

      {/* Sélecteurs : modèle + mode de permission + effort */}
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
            <button key={m.id} onClick={() => setMode(m.id)} title={m.title}
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
          <div className="flex items-center gap-1">
            <span className="text-[11px] text-gray-600 mr-1">Effort</span>
            {selModel.efforts.map((e) => (
              <button key={e} onClick={() => setEffort(e)}
                className={`px-1.5 py-0.5 rounded-sm text-[11px] ${effort === e ? 'bg-blue-500/20 text-blue-300' : 'text-gray-500 hover:text-gray-300 hover:bg-gray-800'}`}>
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
          <button onClick={stop} title="Arrêter"
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
