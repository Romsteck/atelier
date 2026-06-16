import { createContext, useContext, useReducer, useRef, useEffect, useState, useCallback } from 'react';
import useWebSocket from '../hooks/useWebSocket';
import { appendEvent } from '../lib/agentEvents';
import { buildSettings } from '../lib/agentModels';
import {
  startAgentQuery,
  resumeAgentQuery,
  sendAgentMessage,
  interruptAgentRun,
  answerAgentRun,
  planDecisionAgentRun,
  setAgentMode,
  setAgentModel,
  listConversations,
  getConversation,
  renameConversation,
  deleteConversation,
} from '../api/client';

// Provider multi-conversations du mode agent. UNE source d'état + UN seul WebSocket
// (routé par session_id, repli run_id) pour tous les panneaux. Une conversation =
// une session SDK (clé stable `sid`), persistée sur disque par le SDK ; le provider
// n'orchestre que l'ouverture/fermeture, l'envoi de tours et le rebranchement live.
const Ctx = createContext(null);
export const useAgentConversations = () => useContext(Ctx);

let _kc = 0;
const newKey = () => `c${Date.now().toString(36)}_${_kc++}`;
const openSidsKey = (slug) => `agent:openSids:${slug}`; // legacy (lecture seule, migration)
const openTabsKey = (slug) => `agent:openTabs:${slug}`;

function loadOpenSids(slug) {
  try {
    const v = JSON.parse(localStorage.getItem(openSidsKey(slug)));
    return Array.isArray(v) ? v.filter((s) => typeof s === 'string') : [];
  } catch {
    return [];
  }
}

// Descripteurs d'onglets persistés (ordre préservé) : conversation `{t:'c',sid}`,
// fichier `{t:'f',path,name}`, commit `{t:'g',sha,short,subject}`. Migration depuis
// l'ancien format (liste de sids) si la nouvelle clé est absente.
function loadTabs(slug) {
  try {
    const v = JSON.parse(localStorage.getItem(openTabsKey(slug)));
    if (Array.isArray(v)) return v.filter((x) => x && typeof x === 'object');
  } catch {
    /* ignore */
  }
  return loadOpenSids(slug).map((sid) => ({ t: 'c', sid }));
}

// Réponse AskUserQuestion d'une conversation FERMÉE → tour en clair (miroir de
// `answerToTurn` côté runner) injecté via resume.
function formatAnswer(payload) {
  if (payload.cancelled) {
    return "J'ai choisi de ne pas répondre à ta question. Continue avec ton meilleur jugement.";
  }
  const lines = Object.entries(payload.answers || {}).map(([q, a]) => `- ${q} → ${a}`);
  let t = lines.length ? `Voici mes réponses à tes questions :\n${lines.join('\n')}` : 'Voici ma réponse.';
  if (payload.response && payload.response.trim()) t += `\n\n${payload.response.trim()}`;
  return t;
}

const emptyConvo = (key, sid) => ({
  key,
  sid: sid || null,
  title: null,
  items: [],
  running: false,
  runId: null,
  answered: new Set(),
  decided: new Set(),
  live: false,
  loading: false,
  error: null,
  activeModel: null,
  activeMode: null, // mode courant ('plan'|'bypass') reflété par le backend (approbation/set_mode)
  autoSend: null, // {prompt, settings} à envoyer une fois le panneau commit (lancement depuis surveillance)
});

function reducer(state, a) {
  switch (a.type) {
    // Restaure TOUS les onglets (conversations + fichiers + commits) dans l'ordre.
    case 'RESTORE_TABS': {
      const convos = {};
      const order = [];
      for (const t of a.tabs) {
        let key = null;
        let c = null;
        if (t.t === 'c' && t.sid) {
          key = t.sid;
          c = emptyConvo(t.sid, t.sid);
          c.loading = true;
        } else if (t.t === 'f' && t.path) {
          key = `file:${t.path}`;
          c = { key, type: 'file', path: t.path, name: t.name };
        } else if (t.t === 'g' && t.sha) {
          key = `commit:${t.sha}`;
          c = { key, type: 'commit', sha: t.sha, short: t.short, subject: t.subject };
        } else if (t.t === 'd' && t.path) {
          key = `diff:${t.path}`;
          c = { key, type: 'diff', path: t.path, status: t.status };
        }
        if (key && !convos[key]) {
          convos[key] = c;
          order.push(key);
        }
      }
      return { order, convos };
    }
    case 'NEW_PANEL': {
      const c = emptyConvo(a.key, null);
      if (a.autoSend) c.autoSend = a.autoSend;
      return { order: [...state.order, a.key], convos: { ...state.convos, [a.key]: c } };
    }
    case 'OPEN_PANEL': {
      if (state.convos[a.key]) return state;
      const c = emptyConvo(a.key, a.sid);
      c.loading = true;
      return { order: [...state.order, a.key], convos: { ...state.convos, [a.key]: c } };
    }
    case 'CLOSE_PANEL': {
      if (!state.convos[a.key]) return state;
      const convos = { ...state.convos };
      delete convos[a.key];
      return { order: state.order.filter((k) => k !== a.key), convos };
    }
    // Onglet « fichier » (visionneuse), à côté des conversations dans le même split.
    // Clé dérivée du chemin → ré-ouvrir le même fichier ne duplique pas l'onglet.
    case 'OPEN_FILE': {
      const key = `file:${a.path}`;
      if (state.convos[key]) return state; // déjà ouvert → le focus est demandé à part
      const c = { key, type: 'file', path: a.path, name: a.name };
      return { order: [...state.order, key], convos: { ...state.convos, [key]: c } };
    }
    // Onglet « commit » (diff plein écran d'un commit), même mécanique que les fichiers.
    case 'OPEN_COMMIT': {
      const key = `commit:${a.sha}`;
      if (state.convos[key]) return state;
      const c = { key, type: 'commit', sha: a.sha, short: a.short, subject: a.subject };
      return { order: [...state.order, key], convos: { ...state.convos, [key]: c } };
    }
    // Onglet « diff » (diff plein écran d'un fichier MODIFIÉ du working tree).
    case 'OPEN_DIFF': {
      const key = `diff:${a.path}`;
      if (state.convos[key]) return state;
      const c = { key, type: 'diff', path: a.path, status: a.status };
      return { order: [...state.order, key], convos: { ...state.convos, [key]: c } };
    }
    case 'SNAPSHOT_OK': {
      const c = state.convos[a.key];
      if (!c) return state;
      const items = a.items || [];
      const answered = new Set(items.filter((it) => it.type === 'question' && it.answered).map((it) => it.request_id));
      const decided = new Set(items.filter((it) => it.type === 'plan_review' && it.decided).map((it) => it.request_id));
      // `running` (tour en vol) doit survivre au refresh : l'event WS `started` ne rejoue pas.
      // Le backend l'expose dans le snapshot d'une session vivante → autoritaire. À défaut
      // (vieux backend / session morte), on retombe sur l'attente d'un dialogue non résolu.
      const lastItem = items[items.length - 1];
      const awaiting =
        !!lastItem &&
        ((lastItem.type === 'question' && !(answered.has(lastItem.request_id) || lastItem.answered)) ||
          (lastItem.type === 'plan_review' && !(decided.has(lastItem.request_id) || lastItem.decided)));
      const running = typeof a.running === 'boolean' ? a.running : a.live ? awaiting : false;
      return {
        ...state,
        convos: {
          ...state.convos,
          [a.key]: { ...c, items, live: a.live, runId: a.runId || null, answered, decided, activeMode: a.mode || c.activeMode, running, loading: false, error: null },
        },
      };
    }
    case 'SNAPSHOT_ERR': {
      const c = state.convos[a.key];
      if (!c) return state;
      return { ...state, convos: { ...state.convos, [a.key]: { ...c, loading: false, error: a.error } } };
    }
    case 'OPTIMISTIC_USER': {
      const c = state.convos[a.key];
      if (!c) return state;
      return {
        ...state,
        convos: { ...state.convos, [a.key]: { ...c, items: [...c.items, { type: 'user', text: a.text }], running: true, error: null } },
      };
    }
    case 'SET_RUN': {
      const c = state.convos[a.key];
      if (!c) return state;
      return { ...state, convos: { ...state.convos, [a.key]: { ...c, runId: a.runId } } };
    }
    case 'SET_STOPPED': {
      const c = state.convos[a.key];
      if (!c) return state;
      return { ...state, convos: { ...state.convos, [a.key]: { ...c, running: false } } };
    }
    case 'SET_ANSWERED': {
      const c = state.convos[a.key];
      if (!c) return state;
      const answered = new Set(c.answered);
      answered.add(a.request_id);
      return { ...state, convos: { ...state.convos, [a.key]: { ...c, answered, running: true } } };
    }
    case 'SET_PLAN_DECIDED': {
      const c = state.convos[a.key];
      if (!c) return state;
      const decided = new Set(c.decided);
      decided.add(a.request_id);
      // approuver/renvoyer relance le tour → running ; on note l'issue sur l'item.
      const items = c.items.map((it) =>
        it.type === 'plan_review' && it.request_id === a.request_id ? { ...it, decided: true, approved: a.approved } : it,
      );
      return { ...state, convos: { ...state.convos, [a.key]: { ...c, decided, items, running: true } } };
    }
    case 'SET_ERROR': {
      const c = state.convos[a.key];
      if (!c) return state;
      return { ...state, convos: { ...state.convos, [a.key]: { ...c, error: a.error, running: false } } };
    }
    case 'SET_TITLE': {
      const c = state.convos[a.key];
      if (!c) return state;
      return { ...state, convos: { ...state.convos, [a.key]: { ...c, title: a.title } } };
    }
    case 'WS': {
      const ev = a.ev;
      const key = state.order.find((k) => {
        const c = state.convos[k];
        return c && ((c.runId && c.runId === ev.run_id) || (c.sid && ev.session_id && c.sid === ev.session_id));
      });
      if (!key) return state;
      const c = state.convos[key];
      let nc; // chaque branche (default inclus) réassigne
      switch (ev.kind) {
        case 'started':
          nc = { ...c, running: true };
          break;
        case 'turn_done':
          nc = { ...c, running: false };
          break;
        case 'done':
          nc = { ...c, running: false, live: false, runId: null };
          break;
        case 'result':
          nc = { ...c, items: appendEvent(c.items, ev), running: false };
          break;
        case 'system':
          nc = { ...c, live: true };
          if (ev.session_id && !c.sid) nc.sid = ev.session_id;
          if (ev.data?.model) nc.activeModel = ev.data.model;
          break;
        case 'permission_mode':
          nc = { ...c, activeMode: ev.data?.mode || c.activeMode };
          break;
        case 'model':
          nc = { ...c, activeModel: ev.data?.model || c.activeModel };
          break;
        case 'question':
          nc = {
            ...c,
            items: [...c.items, { type: 'question', request_id: ev.data?.request_id, questions: ev.data?.questions || [] }],
          };
          break;
        case 'plan_review':
          nc = {
            ...c,
            items: [...c.items, { type: 'plan_review', request_id: ev.data?.request_id, plan: ev.data?.plan || '' }],
          };
          break;
        default:
          nc = { ...c, items: appendEvent(c.items, ev) };
      }
      return { ...state, convos: { ...state.convos, [key]: nc } };
    }
    default:
      return state;
  }
}

export function AgentConversationsProvider({ slug, launch, onLaunchConsumed, children }) {
  const [state, dispatch] = useReducer(reducer, { order: [], convos: {} });
  const [allConvos, setAllConvos] = useState([]);
  // Demande de mise au premier plan d'un onglet (ouverture de fichier) : {key, n}.
  // ConversationsSplit l'observe pour activer l'onglet (utile en mode replié/onglets).
  const [focusReq, setFocusReq] = useState(null);
  const focusNonce = useRef(0);
  const stateRef = useRef(state);
  stateRef.current = state;

  // UN seul WebSocket pour tout le workspace ; le reducer route par session_id/run_id.
  useWebSocket({ 'agent:event': (d) => { if (d) dispatch({ type: 'WS', ev: d }); } });

  // Restauration au montage (par slug) : recharge TOUS les onglets (conversations,
  // fichiers, commits) dans leur ordre. Les conversations re-fetchent leur snapshot ;
  // fichiers/commits re-fetchent leur contenu à leur propre montage.
  useEffect(() => {
    const tabs = loadTabs(slug);
    if (!tabs.length) return;
    dispatch({ type: 'RESTORE_TABS', tabs });
    for (const t of tabs) {
      if (t.t !== 'c' || !t.sid) continue;
      getConversation(slug, t.sid)
        .then((r) =>
          dispatch({ type: 'SNAPSHOT_OK', key: t.sid, items: r.data?.items || [], live: !!r.data?.live, runId: r.data?.run_id || null, mode: r.data?.mode, running: r.data?.running }),
        )
        .catch((e) => {
          if (e.response?.status === 404) dispatch({ type: 'CLOSE_PANEL', key: t.sid });
          else dispatch({ type: 'SNAPSHOT_ERR', key: t.sid, error: e.message });
        });
    }
  }, [slug]);

  // Persiste l'ensemble des onglets ouverts (ordre + type) — recalculé seulement
  // quand la description change (pas à chaque delta de conversation). Une conversation
  // neuve sans sid n'est pas encore persistable (rien à restaurer côté serveur).
  const tabsStr = JSON.stringify(
    state.order
      .map((k) => {
        const c = state.convos[k];
        if (!c) return null;
        if (c.type === 'file') return { t: 'f', path: c.path, name: c.name };
        if (c.type === 'commit') return { t: 'g', sha: c.sha, short: c.short, subject: c.subject };
        if (c.type === 'diff') return { t: 'd', path: c.path, status: c.status };
        return c.sid ? { t: 'c', sid: c.sid } : null;
      })
      .filter(Boolean),
  );
  useEffect(() => {
    localStorage.setItem(openTabsKey(slug), tabsStr);
  }, [tabsStr, slug]);

  const refreshAll = useCallback(() => {
    listConversations(slug)
      .then((r) => setAllConvos(r.data?.conversations || []))
      .catch(() => {});
  }, [slug]);

  const newConversation = useCallback(() => {
    dispatch({ type: 'NEW_PANEL', key: newKey() });
  }, []);

  const openConversation = useCallback(
    (sid) => {
      const st = stateRef.current;
      if (st.order.some((k) => st.convos[k]?.sid === sid)) return; // déjà ouverte
      const key = sid;
      dispatch({ type: 'OPEN_PANEL', key, sid });
      getConversation(slug, sid)
        .then((r) =>
          dispatch({ type: 'SNAPSHOT_OK', key, items: r.data?.items || [], live: !!r.data?.live, runId: r.data?.run_id || null, mode: r.data?.mode, running: r.data?.running }),
        )
        .catch((e) => dispatch({ type: 'SNAPSHOT_ERR', key, error: e.message }));
    },
    [slug],
  );

  const closeConversation = useCallback((key) => {
    // Ferme le panneau SANS couper le run : la conversation continue côté serveur si
    // elle est vivante, et reste sur disque sinon. Ré-ouvrable depuis l'historique.
    // (Sert aussi à fermer un onglet « fichier ».)
    dispatch({ type: 'CLOSE_PANEL', key });
  }, []);

  // Ouvre un fichier comme onglet dans le split central (façon éditeur VS Code) et
  // demande sa mise au premier plan (nouvel onglet OU onglet déjà ouvert).
  const openFile = useCallback((entry) => {
    if (!entry?.path) return;
    dispatch({ type: 'OPEN_FILE', path: entry.path, name: entry.name });
    focusNonce.current += 1;
    setFocusReq({ key: `file:${entry.path}`, n: focusNonce.current });
  }, []);

  // Idem pour un commit : onglet central plein écran (diff du commit) au lieu d'un
  // aperçu condensé. `commit` = { sha, short, subject }.
  const openCommit = useCallback((commit) => {
    if (!commit?.sha) return;
    dispatch({ type: 'OPEN_COMMIT', sha: commit.sha, short: commit.short, subject: commit.subject });
    focusNonce.current += 1;
    setFocusReq({ key: `commit:${commit.sha}`, n: focusNonce.current });
  }, []);

  // Idem pour un fichier modifié du working tree : onglet central (diff vs HEAD) au
  // lieu de l'aperçu condensé. `file` = { path, status }.
  const openDiff = useCallback((file) => {
    if (!file?.path) return;
    dispatch({ type: 'OPEN_DIFF', path: file.path, status: file.status });
    focusNonce.current += 1;
    setFocusReq({ key: `diff:${file.path}`, n: focusNonce.current });
  }, []);

  const sendMessage = useCallback(
    async (key, text, settings = {}) => {
      const c = stateRef.current.convos[key];
      if (!c) return;
      const t = (text || '').trim();
      if (!t || c.running) return;
      dispatch({ type: 'OPTIMISTIC_USER', key, text: t });
      try {
        let runId = c.runId;
        if (c.runId) {
          try {
            await sendAgentMessage(slug, c.runId, { text: t }); // tour suivant, session vivante
          } catch (e) {
            // runId périmé : le run est mort sans que `done` n'ait atteint ce client (ex.
            // après un deploy qui a coupé la session). On retombe sur la reprise de la session
            // sur disque → la conversation se relance au lieu de renvoyer une erreur 404.
            if (e.response?.status === 404 && c.sid) {
              const r = await resumeAgentQuery(slug, c.sid, { prompt: t, ...settings });
              runId = r.data?.run_id;
            } else {
              throw e;
            }
          }
        } else if (c.sid) {
          const r = await resumeAgentQuery(slug, c.sid, { prompt: t, ...settings }); // reprise
          runId = r.data?.run_id;
        } else {
          const r = await startAgentQuery(slug, { prompt: t, ...settings }); // session neuve
          runId = r.data?.run_id;
        }
        if (runId && runId !== c.runId) dispatch({ type: 'SET_RUN', key, runId });
      } catch (e) {
        dispatch({ type: 'SET_ERROR', key, error: e.response?.data?.error || e.message });
      }
    },
    [slug],
  );

  // Lancement externe (bouton « Résoudre » de la surveillance) : on crée une conversation
  // pré-chargée d'un `autoSend`. Garde par `nonce` → un même `launch` n'est traité qu'une fois,
  // y compris quand le provider reste monté (re-clic sans remonter AgentWorkspace).
  const launchNonce = useRef(null);
  useEffect(() => {
    if (!launch || launch.nonce === launchNonce.current) return;
    launchNonce.current = launch.nonce;
    const settings = buildSettings({
      modelId: localStorage.getItem('agent:model') || 'opus-4-8',
      effort: localStorage.getItem('agent:effort') || 'max',
      mode: launch.mode || 'plan',
    });
    dispatch({ type: 'NEW_PANEL', key: newKey(), autoSend: { prompt: launch.prompt, settings } });
    onLaunchConsumed?.();
  }, [launch, onLaunchConsumed]);

  // Envoi différé du tour `autoSend` : `sendMessage` lit `stateRef.current.convos[key]`, pas
  // encore commit juste après le dispatch NEW_PANEL → on attend que le panneau soit dans l'état.
  // `autoSent` (ref) empêche tout double-envoi sur les re-rendus suivants.
  const autoSent = useRef(new Set());
  useEffect(() => {
    for (const key of state.order) {
      const c = state.convos[key];
      if (c?.autoSend && !autoSent.current.has(key)) {
        autoSent.current.add(key);
        sendMessage(key, c.autoSend.prompt, c.autoSend.settings);
      }
    }
  }, [state.order, state.convos, sendMessage]);

  const answer = useCallback(
    async (key, request_id, payload) => {
      const c = stateRef.current.convos[key];
      if (!c) return;
      dispatch({ type: 'SET_ANSWERED', key, request_id });
      try {
        if (c.runId) {
          await answerAgentRun(slug, c.runId, { request_id, ...payload });
        } else if (c.sid) {
          // Conversation fermée : la réponse relance la session via resume.
          const r = await resumeAgentQuery(slug, c.sid, { prompt: formatAnswer(payload) });
          if (r.data?.run_id) dispatch({ type: 'SET_RUN', key, runId: r.data.run_id });
        }
      } catch (e) {
        dispatch({ type: 'SET_ERROR', key, error: e.response?.data?.error || e.message });
      }
    },
    [slug],
  );

  // Stop = interrompt le TOUR courant (abort SDK), la session reste vivante pour la suite.
  // La fermeture/suppression de session passe par deleteConversation (EOF côté serveur).
  const cancel = useCallback(
    async (key) => {
      const c = stateRef.current.convos[key];
      if (!c?.runId) return;
      dispatch({ type: 'SET_STOPPED', key });
      try {
        await interruptAgentRun(slug, c.runId);
      } catch {
        /* déjà terminé */
      }
    },
    [slug],
  );

  // Décision sur un plan (ExitPlanMode) : approuver = implémenter, sinon renvoyer en révision.
  const decidePlan = useCallback(
    async (key, request_id, approved, feedback) => {
      const c = stateRef.current.convos[key];
      if (!c?.runId) return;
      dispatch({ type: 'SET_PLAN_DECIDED', key, request_id, approved });
      try {
        await planDecisionAgentRun(slug, c.runId, { request_id, approved, feedback });
      } catch (e) {
        dispatch({ type: 'SET_ERROR', key, error: e.response?.data?.error || e.message });
      }
    },
    [slug],
  );

  // Changement de mode/modèle EN COURS de session (sinon c'est un choix local pour la
  // prochaine session, géré côté panneau). `mode` = 'plan'|'bypass' ; `model` = id SDK|null.
  const changeMode = useCallback(
    async (key, mode) => {
      const c = stateRef.current.convos[key];
      if (!c?.runId) return;
      dispatch({ type: 'WS', ev: { run_id: c.runId, kind: 'permission_mode', data: { mode } } }); // optimiste
      try {
        await setAgentMode(slug, c.runId, mode);
      } catch (e) {
        dispatch({ type: 'SET_ERROR', key, error: e.response?.data?.error || e.message });
      }
    },
    [slug],
  );

  const changeModel = useCallback(
    async (key, model) => {
      const c = stateRef.current.convos[key];
      if (!c?.runId) return;
      try {
        await setAgentModel(slug, c.runId, model);
      } catch (e) {
        dispatch({ type: 'SET_ERROR', key, error: e.response?.data?.error || e.message });
      }
    },
    [slug],
  );

  const renameBySid = useCallback(
    async (sid, title) => {
      const st = stateRef.current;
      const key = st.order.find((k) => st.convos[k]?.sid === sid);
      if (key) dispatch({ type: 'SET_TITLE', key, title });
      setAllConvos((prev) => prev.map((x) => (x.sessionId === sid ? { ...x, customTitle: title, summary: title } : x)));
      try {
        await renameConversation(slug, sid, title);
      } catch {
        /* ignore */
      }
    },
    [slug],
  );

  const removeBySid = useCallback(
    async (sid) => {
      const st = stateRef.current;
      const key = st.order.find((k) => st.convos[k]?.sid === sid);
      if (key) dispatch({ type: 'CLOSE_PANEL', key });
      setAllConvos((prev) => prev.filter((x) => x.sessionId !== sid));
      try {
        await deleteConversation(slug, sid);
      } catch {
        /* ignore */
      }
    },
    [slug],
  );

  const value = {
    slug,
    order: state.order,
    convos: state.convos,
    allConvos,
    refreshAll,
    newConversation,
    openConversation,
    closeConversation,
    openFile,
    openCommit,
    openDiff,
    focusReq,
    sendMessage,
    answer,
    cancel,
    decidePlan,
    changeMode,
    changeModel,
    renameBySid,
    removeBySid,
  };

  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}
