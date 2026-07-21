import { createContext, useContext, useReducer, useRef, useEffect, useState, useCallback } from 'react';
import useWebSocket from '../hooks/useWebSocket';
import { showAgentNotification, setBadgeSlice } from '../lib/notify';
import { appendEvent } from '../lib/agentEvents';
import { buildSettings, resolveModelId } from '../lib/agentModels';
import { setOpenResolveScans } from '../lib/resolveConvos';
import {
  startAgentQuery,
  resumeAgentQuery,
  sendAgentMessage,
  interruptAgentRun,
  cancelAgentRun,
  answerAgentRun,
  planDecisionAgentRun,
  setAgentMode,
  setAgentModel,
  listConversations,
  getConversation,
  renameConversation,
  setConversationEffort,
  deleteConversation,
  getAgentOpenTabs,
  setAgentOpenTabs,
} from '../api/client';
import { apiErr } from '../utils/apiErr';

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
const activeTabKey = (slug) => `agent:activeTab:${slug}`; // cache local de l'onglet actif

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
// l'ancien format (liste de sids) si la nouvelle clé est absente. = cache de repli
// hors-ligne ; la source de vérité est désormais le serveur (sync cross-PC).
function loadTabs(slug) {
  try {
    const v = JSON.parse(localStorage.getItem(openTabsKey(slug)));
    if (Array.isArray(v)) return v.filter((x) => x && typeof x === 'object');
  } catch {
    /* ignore */
  }
  return loadOpenSids(slug).map((sid) => ({ t: 'c', sid }));
}

function loadActive(slug) {
  try {
    return localStorage.getItem(activeTabKey(slug)) || null;
  } catch {
    return null;
  }
}

// Clé d'onglet dérivée d'un descripteur (miroir EXACT des clés construites par
// RESTORE_TABS). Sert au calcul de l'actif et à la déduplication.
function descriptorKey(t) {
  if (!t) return null;
  if (t.t === 'c' && t.sid) return t.sid;
  if (t.t === 'f' && t.path) return `file:${t.path}`;
  if (t.t === 'g' && t.sha) return `commit:${t.sha}`;
  if (t.t === 'd' && t.path) return `diff:${t.path}`;
  return null;
}

// Sérialisation canonique d'une liste de descripteurs (dédupliquée, ordre + forme
// de champs stables) → DOIT correspondre octet pour octet à celle calculée depuis
// l'état (cf. `tabsArr` du provider) pour que l'anti-écho (lastSyncedRef) marche.
function canonTabs(descriptors) {
  const seen = new Set();
  const out = [];
  for (const t of descriptors || []) {
    const k = descriptorKey(t);
    if (!k || seen.has(k)) continue;
    seen.add(k);
    if (t.t === 'f') out.push({ t: 'f', path: t.path, name: t.name });
    else if (t.t === 'g') out.push({ t: 'g', sha: t.sha, short: t.short, subject: t.subject });
    else if (t.t === 'd') out.push({ t: 'd', path: t.path, status: t.status });
    // `en` = moteur figé de la conversation ('codex'). Absent = 'claude' (rétro-compat
    // des descripteurs persistés avant l'ajout du second moteur).
    else if (t.t === 'c') out.push({ t: 'c', sid: t.sid, ...(t.sk ? { sk: t.sk } : {}), ...(t.eff ? { eff: t.eff } : {}), ...(t.en ? { en: t.en } : {}) });
  }
  return out;
}

// Réglages connus de la conversation → payload d'un resume implicite (answer/
// decidePlan sur conversation fermée). WHY : un resume « nu » repartirait sur le
// modèle/effort par défaut (Opus) ET écraserait le meta serveur au re-binding.
function convoSettings(c) {
  const effort = c?.settings?.effort || c?.effort;
  const engine = c?.engine || c?.settings?.engine;
  return {
    // Le moteur est figé au binding : un resume DOIT repartir sur le même (les stores
    // de sessions Claude et Codex sont distincts).
    ...(engine ? { engine } : {}),
    ...(c?.settings?.model ? { model: c.settings.model } : {}),
    ...(effort ? { effort } : {}),
  };
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
  scanKind: null, // si lancée par « Résoudre tout » : kind du scan (gate le bouton tant que l'onglet est ouvert)
  effort: null, // effort imposé au lancement (ex. 'max' depuis « Résoudre tout ») — reflété par le sélecteur du panneau
  settings: null, // {engine, model, effort, mode} persistés côté serveur (agent_conversation_meta) — null = legacy/brouillon
  engine: null, // moteur FIGÉ au binding de session ('claude'|'codex') — null = brouillon pas encore lié
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
          if (t.sk) c.scanKind = t.sk; // restaure le lien scan↔conversation
          if (t.eff) c.effort = t.eff; // restaure l'effort imposé (ex. 'max' depuis « Résoudre tout »)
          c.engine = t.en || 'claude'; // descripteur sans `en` = conversation Claude (legacy)
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
      const active = a.active && order.includes(a.active) ? a.active : order[order.length - 1] || null;
      return { order, convos, active };
    }
    // Réconciliation d'un état serveur reçu par WS (autre PC). Reconstruit l'ordre
    // depuis `data.tabs`, CONSERVE les objets convos existants (état live préservé),
    // PRÉSERVE les brouillons locaux sans sid (non représentés côté serveur), retire
    // les onglets absents et applique l'actif. Idempotent : renvoie le même état si
    // rien ne change (évite render + re-PUT en boucle).
    case 'SYNC_TABS': {
      const tabs = Array.isArray(a.data?.tabs) ? a.data.tabs : [];
      const convos = {};
      const order = [];
      const addKey = (key, makeConvo) => {
        if (key && !convos[key]) {
          convos[key] = state.convos[key] || makeConvo();
          order.push(key);
        }
      };
      // Une conversation neuve vit d'abord sous une clé LOCALE (`c…`) puis acquiert son
      // `sid` (event WS `system`) SANS changer de clé. Le descripteur d'onglet ne porte
      // que le `sid` → on retrouve la convo vivante par son sid pour la PRÉSERVER (items,
      // runId, running, saisie en cours) sous SA clé existante, au lieu d'en recréer une
      // vide (perdrait le 1er message + le spinner) ou de la re-keyer (remonterait le
      // panneau via la clé React → saisie perdue).
      const bySid = {};
      for (const k of state.order) {
        const c = state.convos[k];
        if (c?.sid) bySid[c.sid] = { key: k, convo: c };
      }
      const addConvo = (t) => {
        const existing = state.convos[t.sid] ? { key: t.sid, convo: state.convos[t.sid] } : bySid[t.sid];
        if (existing) {
          if (!convos[existing.key]) { convos[existing.key] = existing.convo; order.push(existing.key); }
          return;
        }
        addKey(t.sid, () => {
          const c = emptyConvo(t.sid, t.sid);
          c.loading = true;
          if (t.sk) c.scanKind = t.sk;
          if (t.eff) c.effort = t.eff;
          c.engine = t.en || 'claude';
          return c;
        });
      };
      for (const t of tabs) {
        if (t.t === 'c' && t.sid) {
          addConvo(t);
        } else if (t.t === 'f' && t.path) {
          addKey(`file:${t.path}`, () => ({ key: `file:${t.path}`, type: 'file', path: t.path, name: t.name }));
        } else if (t.t === 'g' && t.sha) {
          addKey(`commit:${t.sha}`, () => ({ key: `commit:${t.sha}`, type: 'commit', sha: t.sha, short: t.short, subject: t.subject }));
        } else if (t.t === 'd' && t.path) {
          addKey(`diff:${t.path}`, () => ({ key: `diff:${t.path}`, type: 'diff', path: t.path, status: t.status }));
        }
      }
      // Brouillons locaux (conversation neuve sans sid) : on les garde en fin.
      for (const k of state.order) {
        const c = state.convos[k];
        if (c && !c.type && !c.sid && !convos[k]) {
          convos[k] = c;
          order.push(k);
        }
      }
      let active = a.data?.active;
      if (!active || !order.includes(active)) {
        active = order.includes(state.active) ? state.active : order[order.length - 1] || null;
      }
      const sameOrder = order.length === state.order.length && order.every((k, i) => k === state.order[i]);
      if (sameOrder && active === state.active) return state; // no-op
      return { order, convos, active };
    }
    case 'SET_ACTIVE': {
      if (a.key === state.active) return state;
      if (a.key != null && !state.order.includes(a.key)) return state;
      return { ...state, active: a.key };
    }
    case 'NEW_PANEL': {
      const c = emptyConvo(a.key, null);
      if (a.autoSend) c.autoSend = a.autoSend;
      if (a.scanKind) c.scanKind = a.scanKind;
      if (a.effort) c.effort = a.effort;
      // Modèle (donc moteur) choisi à la création via le menu du « + » : graine du
      // sélecteur du panneau, qui retomberait sinon sur la préférence localStorage.
      if (a.modelId) c.seedModelId = a.modelId;
      // Mode imposé au lancement (ex. plan depuis « Résoudre tout ») : seedé ici pour que
      // le sélecteur du panneau l'affiche immédiatement (sans lui, il retombait sur la
      // préférence localStorage — potentiellement « bypass » — alors que le run part en plan).
      if (a.mode) c.activeMode = a.mode;
      // Une conversation neuve prend le focus (sinon, en mode onglets, elle s'ouvrirait
      // en arrière-plan). C'est aussi l'actif synchronisé vers les autres PCs.
      return { ...state, order: [...state.order, a.key], convos: { ...state.convos, [a.key]: c }, active: a.key };
    }
    case 'OPEN_PANEL': {
      if (state.convos[a.key]) return { ...state, active: a.key };
      const c = emptyConvo(a.key, a.sid);
      c.loading = true;
      c.engine = a.engine || 'claude'; // moteur connu dès l'ouverture (liste des conversations)
      return { ...state, order: [...state.order, a.key], convos: { ...state.convos, [a.key]: c }, active: a.key };
    }
    case 'CLOSE_PANEL': {
      if (!state.convos[a.key]) return state;
      const convos = { ...state.convos };
      delete convos[a.key];
      const order = state.order.filter((k) => k !== a.key);
      // Si l'onglet fermé était l'actif, basculer sur le dernier restant.
      const active = state.active === a.key ? order[order.length - 1] || null : state.active;
      return { ...state, order, convos, active };
    }
    // Onglet « fichier » (visionneuse), à côté des conversations dans le même split.
    // Clé dérivée du chemin → ré-ouvrir le même fichier ne duplique pas l'onglet.
    case 'OPEN_FILE': {
      const key = `file:${a.path}`;
      if (state.convos[key]) return state; // déjà ouvert → le focus est demandé via focusReq
      const c = { key, type: 'file', path: a.path, name: a.name };
      return { ...state, order: [...state.order, key], convos: { ...state.convos, [key]: c }, active: key };
    }
    // Onglet « commit » (diff plein écran d'un commit), même mécanique que les fichiers.
    case 'OPEN_COMMIT': {
      const key = `commit:${a.sha}`;
      if (state.convos[key]) return state;
      const c = { key, type: 'commit', sha: a.sha, short: a.short, subject: a.subject };
      return { ...state, order: [...state.order, key], convos: { ...state.convos, [key]: c }, active: key };
    }
    // Onglet « diff » (diff plein écran d'un fichier MODIFIÉ du working tree).
    case 'OPEN_DIFF': {
      const key = `diff:${a.path}`;
      if (state.convos[key]) return state;
      const c = { key, type: 'diff', path: a.path, status: a.status };
      return { ...state, order: [...state.order, key], convos: { ...state.convos, [key]: c }, active: key };
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
          [a.key]: {
            ...c,
            items,
            live: a.live,
            runId: a.runId || null,
            answered,
            decided,
            activeMode: a.mode || c.activeMode,
            running,
            loading: false,
            error: null,
            // Réglages serveur de la conversation (vérité session, cross-PC). L'effort
            // du snapshot prime sur le `eff` du descripteur d'onglet ; le panneau se
            // resynchronise via ses effets convo.effort / convo.settings.
            settings: a.settings || c.settings,
            effort: a.settings?.effort || c.effort,
            // Moteur figé de la session (autoritaire côté serveur) ; à défaut on garde
            // celui déjà connu (descripteur d'onglet / liste), sinon 'claude' (legacy).
            engine: a.settings?.engine || c.engine || 'claude',
          },
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
      const userItem = { type: 'user', text: a.text, ...(a.images?.length ? { images: a.images } : {}) };
      return {
        ...state,
        convos: { ...state.convos, [a.key]: { ...c, items: [...c.items, userItem], running: true, error: null } },
      };
    }
    case 'SET_RUN': {
      const c = state.convos[a.key];
      if (!c) return state;
      return { ...state, convos: { ...state.convos, [a.key]: { ...c, runId: a.runId } } };
    }
    // Moteur du dernier envoi (pastille). Le VERROU, lui, tient au binding réel (`sid`) :
    // tant qu'aucune session n'existe, un nouvel envoi peut changer ce moteur.
    case 'SET_ENGINE': {
      const c = state.convos[a.key];
      if (!c || c.engine === a.engine) return state;
      return { ...state, convos: { ...state.convos, [a.key]: { ...c, engine: a.engine } } };
    }
    case 'SET_STOPPED': {
      const c = state.convos[a.key];
      if (!c) return state;
      return { ...state, convos: { ...state.convos, [a.key]: { ...c, running: false } } };
    }
    // Choix délibéré d'effort (changeEffort) : reflété localement pour que ni un
    // snapshot resync ni la sync d'onglets (`eff`) ne revertent le sélecteur.
    case 'SET_CONVO_EFFORT': {
      const c = state.convos[a.key];
      if (!c) return state;
      const settings = c.settings ? { ...c.settings, effort: a.effort } : c.settings;
      return { ...state, convos: { ...state.convos, [a.key]: { ...c, effort: a.effort, settings } } };
    }
    case 'SET_ANSWERED': {
      const c = state.convos[a.key];
      if (!c) return state;
      const answered = new Set(c.answered);
      answered.add(a.request_id);
      // Écrit le texte de réponse sur l'item question. WHY : sur le chemin live le
      // tool_result (qui porte la réponse) est masqué côté runner → sans ça l'item n'a
      // pas d'`answer` et la carte affiche « Réponse envoyée. » au lieu de la réponse
      // fournie (notamment la réponse libre). Même format que le backend (agent.rs) →
      // affichage identique avant/après reload.
      const items = c.items.map((it) =>
        it.type === 'question' && it.request_id === a.request_id ? { ...it, answered: true, answer: a.answerText } : it,
      );
      return { ...state, convos: { ...state.convos, [a.key]: { ...c, answered, items, running: true } } };
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
          // `done` tardif d'un ANCIEN run (ex. session fermée par changeEffort puis
          // déjà relancée en resume) : ne pas clobberer le runId du nouveau run.
          if (ev.run_id && c.runId && ev.run_id !== c.runId) { nc = { ...c }; break; }
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
          // `model: null` = retour explicite au défaut abonnement (Opus) — distinct
          // d'un event sans champ. Un `||` avalait ce null et l'UI restait figée.
          nc = { ...c, activeModel: ev.data && 'model' in ev.data ? (ev.data.model ?? null) : c.activeModel };
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
        // Dialogue expiré côté CLI (timeout headless) : l'agent s'est mis en pause. La carte
        // reste actionnable — on la marque `idle` pour afficher le hint « réponds pour reprendre ».
        case 'question_idle':
        case 'plan_idle': {
          const ty = ev.kind === 'question_idle' ? 'question' : 'plan_review';
          nc = {
            ...c,
            items: c.items.map((it) =>
              it.type === ty && it.request_id === ev.data?.request_id ? { ...it, idle: true } : it),
          };
          break;
        }
        default:
          nc = { ...c, items: appendEvent(c.items, ev) };
      }
      return { ...state, convos: { ...state.convos, [key]: nc } };
    }
    default:
      return state;
  }
}

export function AgentConversationsProvider({ slug, launch, onLaunchConsumed, profile = 'dev', pmMode = 'normal', children }) {
  // PM tabs deliberately use a separate namespace from developer tabs. This
  // prevents a project-manager dock from opening/closing Studio code chats.
  const tabScope = profile === 'pm' ? `${slug}::pm` : slug;
  const [state, dispatch] = useReducer(reducer, { order: [], convos: {}, active: null });
  const [allConvos, setAllConvos] = useState([]);
  // Moteurs dont le `op:list` a échoué au dernier refresh (champ `unavailable` du serveur).
  // Non vide ⇒ l'historique affiché est PARTIEL : on l'annonce au lieu de le laisser
  // passer pour un historique vide (cf. refreshAll).
  const [unavailableEngines, setUnavailableEngines] = useState([]);
  // Demande de mise au premier plan d'un onglet (ouverture de fichier) : {key, n}.
  // ConversationsSplit l'observe pour activer l'onglet (utile en mode replié/onglets).
  const [focusReq, setFocusReq] = useState(null);
  const focusNonce = useRef(0);
  const stateRef = useRef(state);
  stateRef.current = state;
  // Sync cross-PC : `loadedRef` empêche tout PUT avant le chargement initial ;
  // `lastSyncedRef` = dernier payload connu du serveur (notre PUT OU un event WS reçu)
  // → neutralise la boucle d'écho (on ne re-PUT pas ce qu'on vient de recevoir/d'envoyer).
  const loadedRef = useRef(false);
  const lastSyncedRef = useRef(null);
  // Conversations avec une réponse non lue (clés) → point « non lu » par onglet +
  // pastille PWA. Effacé quand la conversation devient active ET visible.
  const [unread, setUnread] = useState(() => new Set());

  // Re-fetch du snapshot d'une conversation (helper partagé restore/sync). `engine` doit
  // être fourni pour une conversation Codex : les deux moteurs ont des stores distincts,
  // un GET sans moteur chercherait le sid côté Claude (404).
  const fetchSnapshot = useCallback((sid, engine) => {
    getConversation(slug, sid, engine)
      .then((r) => dispatch({ type: 'SNAPSHOT_OK', key: sid, items: r.data?.items || [], live: !!r.data?.live, runId: r.data?.run_id || null, mode: r.data?.mode, running: r.data?.running, settings: r.data?.settings || null }))
      .catch((e) => {
        if (e.response?.status === 404) dispatch({ type: 'CLOSE_PANEL', key: sid });
        else dispatch({ type: 'SNAPSHOT_ERR', key: sid, error: e.message });
      });
  }, [slug]);

  // Re-fetch du snapshot serveur (autoritaire) de chaque conversation ouverte keyée
  // par sid — running d'abord. Partagé entre le reconnect WS (epoch) et l'event
  // `resync` (subscriber laggé) : dans les deux cas des events ont été perdus.
  const resyncAllSnapshots = useCallback(() => {
    const st = stateRef.current;
    const targets = st.order
      .filter((k) => { const c = st.convos[k]; return c && c.sid && k === c.sid; })
      .map((k) => st.convos[k])
      .sort((a, b) => Number(b.running) - Number(a.running));
    for (const c of targets) fetchSnapshot(c.sid, c.engine);
  }, [fetchSnapshot]);

  // UN seul WebSocket pour tout le workspace : events de run (routés par session_id/
  // run_id) + sync de l'ensemble des onglets ouverts (changement venu d'un autre PC).
  const { epoch } = useWebSocket({
    'agent:event': (d) => { if (d) dispatch({ type: 'WS', ev: d }); },
    // Le serveur signale un buffer broadcast dépassé (events agent perdus) : l'état
    // local est troué → même re-sync de snapshots qu'au reconnect.
    'resync': (m) => { if (m?.channel === 'agent:event') resyncAllSnapshots(); },
    'agent:open-tabs': (d) => {
      if (!d || d.slug !== tabScope) return;
      const incoming = canonTabs(d.tabs);
      const keys = incoming.map(descriptorKey);
      const active = d.active && keys.includes(d.active) ? d.active : null;
      // Pose la référence AVANT le dispatch : l'effet de persistance verra un payload
      // identique → pas de re-PUT (anti-écho).
      lastSyncedRef.current = JSON.stringify({ tabs: incoming, active });
      const before = stateRef.current;
      const beforeSids = new Set(before.order.map((k) => before.convos[k]?.sid).filter(Boolean));
      dispatch({ type: 'SYNC_TABS', data: d });
      // Snapshot des conversations nouvellement ouvertes par ce sync.
      for (const t of d.tabs || []) {
        if (t.t === 'c' && t.sid && !beforeSids.has(t.sid)) fetchSnapshot(t.sid, t.en);
      }
    },
  });

  // Re-sync au reconnect WS : le canal broadcast ne rejoue PAS l'historique → après
  // une coupure (bascule d'appli, gel mobile, perte réseau), on re-fetche le snapshot
  // serveur (autoritaire : buffer live en mémoire ou transcript disque) de chaque
  // conversation ouverte keyée par sid — running d'abord — pour récupérer les deltas
  // et le `done` ratés pendant la coupure. SNAPSHOT_OK remplace `items` en bloc et fait
  // confiance au running/live/run_id serveur ; les deltas folded dédoublonnent.
  const prevEpoch = useRef(0);
  useEffect(() => {
    if (epoch === 0 || epoch === prevEpoch.current) return;
    prevEpoch.current = epoch;
    resyncAllSnapshots();
  }, [epoch, resyncAllSnapshots]);

  // Chargement initial (par slug) : l'état des onglets est AUTORITAIRE côté serveur
  // (sync cross-PC). On le charge, re-fetche les snapshots des conversations, et amorce
  // le serveur depuis le cache local si la table est vide (migration douce post-deploy).
  // Repli sur le cache localStorage si le serveur est injoignable (Postgres down).
  useEffect(() => {
    let cancelled = false;
    loadedRef.current = false;
    lastSyncedRef.current = null;
    // Capture le cache local AVANT que l'effet de cache (déclaré après) n'écrive l'état
    // transitoire vide au montage → la migration douce lirait sinon un cache effacé.
    const cachedLocal = loadTabs(tabScope);
    const cachedActive = loadActive(tabScope);

    const applyLoaded = (tabs, rawActive, seed) => {
      if (cancelled) return;
      const keys = [];
      for (const t of tabs) { const k = descriptorKey(t); if (k && !keys.includes(k)) keys.push(k); }
      const active = rawActive && keys.includes(rawActive) ? rawActive : keys[keys.length - 1] || null;
      const canonical = { tabs: canonTabs(tabs), active };
      dispatch({ type: 'RESTORE_TABS', tabs, active });
      lastSyncedRef.current = JSON.stringify(canonical);
      loadedRef.current = true;
      if (seed) setAgentOpenTabs(tabScope, canonical).catch(() => {});
      for (const t of tabs) {
        if (t.t === 'c' && t.sid) fetchSnapshot(t.sid, t.en);
      }
    };

    getAgentOpenTabs(tabScope)
      .then((r) => {
        if (cancelled) return;
        const serverTabs = Array.isArray(r.data?.tabs) ? r.data.tabs : [];
        if (serverTabs.length) {
          applyLoaded(serverTabs, r.data?.active ?? null, false);
        } else {
          // Serveur vide → amorce depuis le cache local (migration douce).
          applyLoaded(cachedLocal, cachedLocal.length ? cachedActive : null, cachedLocal.length > 0);
        }
      })
      .catch(() => {
        // Serveur injoignable → mode dégradé sur le cache local. loadedRef passe quand
        // même à true : un PUT ultérieur réessaiera (et échouera proprement) jusqu'au
        // rétablissement du serveur.
        applyLoaded(cachedLocal, cachedActive, false);
      });

    return () => { cancelled = true; };
  }, [tabScope, fetchSnapshot]);

  // Sérialisation canonique de l'état courant (brouillons sans sid exclus). DOIT
  // matcher `canonTabs(...)` à l'octet près pour que l'anti-écho fonctionne.
  const tabsArr = state.order
    .map((k) => {
      const c = state.convos[k];
      if (!c) return null;
      if (c.type === 'file') return { t: 'f', path: c.path, name: c.name };
      if (c.type === 'commit') return { t: 'g', sha: c.sha, short: c.short, subject: c.subject };
      if (c.type === 'diff') return { t: 'd', path: c.path, status: c.status };
      // `en` n'est émis QUE pour un moteur non-défaut : un descripteur sans `en` vaut
      // 'claude', donc l'omettre garde les payloads legacy identiques (pas de PUT à vide).
      return c.sid
        ? {
            t: 'c',
            sid: c.sid,
            ...(c.scanKind ? { sk: c.scanKind } : {}),
            ...(c.effort ? { eff: c.effort } : {}),
            ...(c.engine && c.engine !== 'claude' ? { en: c.engine } : {}),
          }
        : null;
    })
    .filter(Boolean);
  // Actif persistable = identité cross-PC : le `sid` pour une conversation (sa clé peut
  // rester locale `c…` après acquisition du sid), sinon la clé pour fichier/commit/diff.
  // WHY le sid et pas la clé : émettre la clé locale ferait diverger l'anti-écho sur le
  // champ `active` (la clé locale n'est jamais dans les descripteurs reçus) → PUT en
  // boucle toutes les 400 ms. Un brouillon sans sid n'a pas d'identité reproductible → null.
  const ac = state.active ? state.convos[state.active] : null;
  const activeKeyVal = ac ? (ac.type ? state.active : (ac.sid || null)) : null;
  const tabsStr = JSON.stringify(tabsArr);
  const payloadStr = JSON.stringify({ tabs: tabsArr, active: activeKeyVal });

  // Cache local (repli hors-ligne). Gated par `loadedRef` : ne PAS écrire l'état
  // transitoire vide du montage (sinon on efface le cache avant la migration douce).
  useEffect(() => {
    if (!loadedRef.current) return;
    try {
      localStorage.setItem(openTabsKey(tabScope), tabsStr);
      if (activeKeyVal) localStorage.setItem(activeTabKey(tabScope), activeKeyVal);
      else localStorage.removeItem(activeTabKey(tabScope));
    } catch { /* ignore */ }
  }, [tabsStr, activeKeyVal, tabScope]);

  // Source de vérité = serveur : PUT (debouncé 400 ms) à chaque changement réel →
  // broadcast WS aux autres PCs. Anti-écho via lastSyncedRef ; pas de PUT tant que
  // le chargement initial n'a pas eu lieu.
  useEffect(() => {
    if (!loadedRef.current) return;
    if (payloadStr === lastSyncedRef.current) return;
    const id = setTimeout(() => {
      setAgentOpenTabs(tabScope, { tabs: tabsArr, active: activeKeyVal })
        .then(() => { lastSyncedRef.current = payloadStr; })
        .catch(() => { /* serveur/PG down → cache local seul, retry au prochain changement */ });
    }, 400);
    return () => clearTimeout(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [payloadStr, tabScope]);

  // Filet de sécurité : si l'actif n'est plus dans l'ordre (chemin ne passant pas par
  // CLOSE_PANEL), basculer sur le dernier onglet (miroir de l'ancienne logique de
  // ConversationsSplit, désormais propriétaire dans le provider).
  useEffect(() => {
    if (!state.order.length) return;
    if (!state.active || !state.order.includes(state.active)) {
      dispatch({ type: 'SET_ACTIVE', key: state.order[state.order.length - 1] });
    }
  }, [state.order, state.active]);

  // Publie l'ensemble des kinds de scan ayant une conversation de résolution OUVERTE, pour
  // que la surveillance (hors de cet arbre) désactive leur bouton « Résoudre tout ».
  // Recalculé à chaque changement d'onglets ; pas de reset au démontage → en mode onglets,
  // l'état survit pendant qu'on regarde la surveillance (l'AgentWorkspace est démonté).
  useEffect(() => {
    const kinds = state.order.map((k) => state.convos[k]?.scanKind).filter(Boolean);
    setOpenResolveScans(kinds);
  }, [state.order, state.convos]);

  // Onglet actif : propriété du provider (pour être synchronisé cross-PC). Le
  // reducer valide la clé (ignore si absente de l'ordre).
  const setActive = useCallback((key) => { dispatch({ type: 'SET_ACTIVE', key }); }, []);

  const refreshAll = useCallback(() => {
    listConversations(slug, profile)
      .then((r) => {
        const list = Array.isArray(r.data?.conversations) ? r.data.conversations : [];
        const un = Array.isArray(r.data?.unavailable) ? r.data.unavailable : [];
        setUnavailableEngines(un);
        if (un.length === 0) { setAllConvos(list); return; }
        // WHY : quand un moteur ne répond pas (op:list en échec/timeout), le serveur
        // renvoie 200 + liste PARTIELLE + `unavailable`. Écraser l'état avec cette liste
        // EFFACERAIT à l'écran l'historique du moteur en panne (une panne du moteur par
        // défaut ne laissait que les conversations Codex, vides en pratique). Pas d'info
        // fraîche ⇒ on ne jette rien : on conserve les entrées connues de ces moteurs.
        setAllConvos((prev) => {
          const fresh = new Set(list.map((c) => c.sessionId));
          const kept = prev.filter(
            (c) => un.includes(c.engine || 'claude') && !fresh.has(c.sessionId),
          );
          // Re-tri global : les entrées conservées doivent se replacer chronologiquement
          // parmi les fraîches (le serveur trie la liste fusionnée, pas nous par moitiés).
          return [...list, ...kept].sort(
            (a, b) => (b.lastModified || b.createdAt || 0) - (a.lastModified || a.createdAt || 0),
          );
        });
      })
      // Liste best-effort (les en-têtes retombent sur les ids) — mais on trace l'échec,
      // sinon une API en erreur est indistinguable d'un historique vide.
      .catch((e) => console.warn('[agent] listConversations a échoué :', apiErr(e)));
  }, [slug, profile]);

  // Charge la liste des sessions au montage du workspace → les noms (résumés générés
  // par le SDK) sont dispo dans les en-têtes de chat même sans ouvrir l'historique.
  useEffect(() => { refreshAll(); }, [refreshAll]);

  // Nom affiché d'une conversation : titre manuel > résumé/nom généré par le SDK (depuis
  // la liste des sessions, comme l'historique) > 1er message utilisateur > « Conversation ».
  const convName = useCallback(
    (convo) => {
      if (!convo) return 'Conversation';
      if (convo.title) return convo.title;
      const sess = allConvos.find((s) => s.sessionId === convo.sid);
      const sdk = sess?.customTitle || sess?.summary || sess?.firstPrompt;
      if (sdk) return sdk;
      const fu = (convo.items || []).find((it) => it.type === 'user')?.text;
      if (fu) return fu.trim().replace(/\s+/g, ' ').slice(0, 60);
      return 'Conversation';
    },
    [allConvos],
  );

  // ── Notification « réponse prête » : front montant running → fini d'une
  // conversation qui n'est PAS active+visible. Effet (le reducer reste pur) qui
  // détecte la transition par clé via un ref de l'état running précédent. Marque
  // l'onglet « non lu » + déclenche une notif système (SW). Récupère aussi un
  // `done` survenu pendant une coupure WS (via le re-sync du snapshot).
  const prevRunning = useRef(new Map());
  useEffect(() => {
    const st = state;
    for (const key of st.order) {
      const c = st.convos[key];
      if (!c || !c.sid) continue;
      const was = prevRunning.current.get(key) || false;
      const now = !!c.running;
      if (was && !now) {
        const activeVisible = st.active === key && document.visibilityState === 'visible';
        if (!activeVisible) {
          setUnread((s) => { if (s.has(key)) return s; const n = new Set(s); n.add(key); return n; });
          showAgentNotification({ slug, sid: c.sid, title: convName(c) });
        }
      }
      prevRunning.current.set(key, now);
    }
    for (const k of [...prevRunning.current.keys()]) if (!st.convos[k]) prevRunning.current.delete(k);
  }, [state, slug, convName]);

  // Pastille PWA = tranche « agent » du badge agrégé (les notifications
  // plateforme posent leur propre tranche) ; effacée au démontage du workspace.
  useEffect(() => { setBadgeSlice('agent', unread.size); }, [unread]);
  useEffect(() => () => setBadgeSlice('agent', 0), []);

  // Efface le « non lu » de la conversation active dès qu'elle est visible (ouverture
  // d'onglet OU retour au premier plan sur l'onglet déjà actif).
  useEffect(() => {
    const active = state.active;
    if (!active) return;
    const clear = () => {
      if (document.visibilityState !== 'visible') return;
      setUnread((s) => { if (!s.has(active)) return s; const n = new Set(s); n.delete(active); return n; });
    };
    clear();
    document.addEventListener('visibilitychange', clear);
    return () => document.removeEventListener('visibilitychange', clear);
  }, [state.active]);

  // `modelId` (optionnel) = modèle pré-sélectionné, donc MOTEUR pré-sélectionné : posé
  // par le menu du bouton « + » (NewConversationButton). Simple graine du sélecteur du
  // panneau — rien n'est figé tant qu'aucun tour n'est parti.
  const newConversation = useCallback((opts) => {
    dispatch({ type: 'NEW_PANEL', key: newKey(), modelId: opts?.modelId });
  }, []);

  // `engine` vient de l'entrée d'historique (chaque conversation listée porte son moteur).
  const openConversation = useCallback(
    (sid, engine) => {
      const st = stateRef.current;
      if (st.order.some((k) => st.convos[k]?.sid === sid)) return; // déjà ouverte
      const key = sid;
      dispatch({ type: 'OPEN_PANEL', key, sid, engine });
      getConversation(slug, sid, engine)
        .then((r) =>
          dispatch({ type: 'SNAPSHOT_OK', key, items: r.data?.items || [], live: !!r.data?.live, runId: r.data?.run_id || null, mode: r.data?.mode, running: r.data?.running, settings: r.data?.settings || null }),
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

  // `images` = [{ media_type, data(base64), url(dataURL aperçu) }]. On n'envoie au backend
  // que { media_type, data } ; `url` ne sert qu'à la bulle optimiste (aperçu immédiat).
  const sendMessage = useCallback(
    async (key, text, settings = {}, images = []) => {
      const c = stateRef.current.convos[key];
      if (!c) return;
      const t = (text || '').trim();
      const imgs = Array.isArray(images) ? images : [];
      if ((!t && !imgs.length) || c.running) return;
      const apiImages = imgs.length ? imgs.map(({ media_type, data }) => ({ media_type, data })) : undefined;
      dispatch({ type: 'OPTIMISTIC_USER', key, text: t, images: imgs.map((i) => i.url).filter(Boolean) });
      // Le moteur d'une conversation LIÉE (sid acquis) prime sur le sélecteur (verrou
      // d'engine). WHY tant qu'il n'y a PAS de `sid` : le sélecteur prime, même si un 1er
      // message a déjà posé un engine optimiste — sinon un 1er tour Codex échoué (auth
      // morte) figerait le brouillon sur Codex sans retour possible vers Claude.
      const engine = (c.sid ? c.engine : null) || settings.engine || c.engine || 'claude';
      const withEngine = { ...settings, engine };
      // Pastille optimiste : reflète le moteur RÉELLEMENT envoyé (donc réévaluée tant que
      // la conversation n'est pas liée) ; le reducer no-op si la valeur est inchangée.
      if (c.engine !== engine) dispatch({ type: 'SET_ENGINE', key, engine });
      try {
        let runId = c.runId;
        if (c.runId) {
          try {
            await sendAgentMessage(slug, c.runId, { text: t, images: apiImages, pm_mode: pmMode }); // tour suivant, session vivante
          } catch (e) {
            // runId périmé : le run est mort sans que `done` n'ait atteint ce client (ex.
            // après un deploy qui a coupé la session). On retombe sur la reprise de la session
            // sur disque → la conversation se relance au lieu de renvoyer une erreur 404.
            if (e.response?.status === 404 && c.sid) {
              const r = await resumeAgentQuery(slug, c.sid, { prompt: t, images: apiImages, ...withEngine, profile, pm_mode: pmMode });
              runId = r.data?.run_id;
            } else {
              throw e;
            }
          }
        } else if (c.sid) {
          const r = await resumeAgentQuery(slug, c.sid, { prompt: t, images: apiImages, ...withEngine, profile, pm_mode: pmMode }); // reprise
          runId = r.data?.run_id;
        } else {
          const r = await startAgentQuery(slug, { prompt: t, images: apiImages, ...withEngine, profile, pm_mode: pmMode }); // session neuve
          runId = r.data?.run_id;
        }
        if (runId && runId !== c.runId) dispatch({ type: 'SET_RUN', key, runId });
      } catch (e) {
        dispatch({ type: 'SET_ERROR', key, error: apiErr(e) });
      }
    },
    [slug, profile, pmMode],
  );

  // Lancement externe (bouton « Résoudre tout » de la surveillance) : on crée une conversation
  // pré-chargée d'un `autoSend`. Garde par `nonce` → un même `launch` n'est traité qu'une fois,
  // y compris quand le provider reste monté (re-clic sans remonter AgentWorkspace).
  const launchNonce = useRef(null);
  useEffect(() => {
    if (!launch || launch.nonce === launchNonce.current) return;
    launchNonce.current = launch.nonce;
    const settings = buildSettings({
      // WHY moteur épinglé à Claude : ce flux (résolution de findings de surveillance) a
      // besoin des tools MCP `studio` (findings_*, docs_*) que le moteur Codex n'a pas. La
      // préférence globale `agent:model` peut pointer un modèle Codex → on la contraint au
      // moteur Claude (2e argument de resolveModelId) au lieu de la suivre aveuglément.
      modelId: resolveModelId(localStorage.getItem('agent:model'), launch.engine || 'claude'),
      // launch.effort (ex. 'max' depuis « Résoudre tout ») prime sur la préférence agent stockée.
      effort: launch.effort || localStorage.getItem('agent:effort') || 'max',
      mode: launch.mode || 'plan',
    });
    dispatch({
      type: 'NEW_PANEL',
      key: newKey(),
      autoSend: { prompt: launch.prompt, settings },
      scanKind: launch.scanKind,
      effort: settings.effort,
      // Seed du mode réel du run (cf. NEW_PANEL) — le sélecteur affiche « Plan » dès l'ouverture.
      mode: settings.permission_mode === 'bypassPermissions' ? 'bypass' : 'plan',
    });
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
      // Texte affiché sur l'item question, en miroir EXACT du backend (agent.rs::answer)
      // → l'affichage live est identique à celui d'après un reload (qui lit it.answer du buffer).
      const answerText = payload.cancelled
        ? '(non répondu)'
        : [
            ...Object.entries(payload.answers || {}).map(([q, v]) => `- ${q} → ${v}`),
            ...(payload.response && payload.response.trim() ? [payload.response.trim()] : []),
          ].join('\n');
      dispatch({ type: 'SET_ANSWERED', key, request_id, answerText });
      try {
        let sent = false;
        if (c.runId) {
          try {
            await answerAgentRun(slug, c.runId, { request_id, ...payload });
            sent = true;
          } catch (e) {
            // runId périmé ou session en cours d'arrêt (drain) → même reprise que la
            // conversation fermée ci-dessous (miroir du fallback de sendMessage).
            if (!(e.response?.status === 404 && c.sid)) throw e;
          }
        }
        if (!sent && c.sid) {
          // Conversation fermée : la réponse relance la session via resume, dans le MODE
          // où la conversation se trouvait (sans ça, un resume repartait toujours en plan)
          // et avec ses réglages connus (sans ça, il repartait en Opus/effort défaut).
          const r = await resumeAgentQuery(slug, c.sid, {
            prompt: formatAnswer(payload),
            permission_mode: c.activeMode === 'bypass' ? 'bypassPermissions' : 'plan',
            ...convoSettings(c),
          });
          if (r.data?.run_id) dispatch({ type: 'SET_RUN', key, runId: r.data.run_id });
        }
      } catch (e) {
        dispatch({ type: 'SET_ERROR', key, error: apiErr(e) });
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
  // Session morte (reapée à l'idle avec un plan en attente) : la décision RELANCE la session
  // (resume) — approuver reprend directement en bypass pour implémenter, refuser reste en plan.
  const decidePlan = useCallback(
    async (key, request_id, approved, feedback) => {
      const c = stateRef.current.convos[key];
      if (!c) return;
      dispatch({ type: 'SET_PLAN_DECIDED', key, request_id, approved });
      try {
        let sent = false;
        if (c.runId) {
          try {
            await planDecisionAgentRun(slug, c.runId, { request_id, approved, feedback });
            sent = true;
          } catch (e) {
            // runId périmé / session en cours d'arrêt → reprise (miroir de sendMessage).
            if (!(e.response?.status === 404 && c.sid)) throw e;
          }
        }
        if (!sent && c.sid) {
          const why = feedback?.trim();
          const prompt = approved
            ? "J'approuve ton plan. Passe à l'implémentation maintenant."
            : `Je n'approuve pas encore le plan.${why ? ` Retour : ${why}` : ''} Affine-le puis re-propose.`;
          const r = await resumeAgentQuery(slug, c.sid, {
            prompt,
            permission_mode: approved ? 'bypassPermissions' : 'plan',
            ...convoSettings(c),
          });
          if (r.data?.run_id) dispatch({ type: 'SET_RUN', key, runId: r.data.run_id });
        }
      } catch (e) {
        dispatch({ type: 'SET_ERROR', key, error: apiErr(e) });
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
        dispatch({ type: 'SET_ERROR', key, error: apiErr(e) });
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
        dispatch({ type: 'SET_ERROR', key, error: apiErr(e) });
      }
    },
    [slug],
  );

  // L'effort SDK est figé au démarrage de la session (pas de set_effort live) →
  // changer l'effort d'une conversation : (1) persiste l'INTENTION dans le meta
  // serveur (sinon un resync de snapshot / un autre PC reverterait le sélecteur à
  // l'ancien effort avant le prochain message), (2) si la session est vivante mais
  // idle, la ferme proprement (cancel = interrupt + EOF → transcript resumable) ;
  // le `done` remet runId à null et le prochain send repart en resume avec le
  // nouvel effort — mémoire complète conservée. Disabled pendant un tour en vol.
  const changeEffort = useCallback(
    async (key, effort) => {
      const c = stateRef.current.convos[key];
      if ((!c?.sid && !c?.runId) || c?.running) return; // brouillon (localStorage suffit) / tour en vol
      dispatch({ type: 'SET_CONVO_EFFORT', key, effort });
      try {
        if (c.sid) await setConversationEffort(slug, c.sid, effort, c.engine);
        if (c.runId) await cancelAgentRun(slug, c.runId);
      } catch {
        /* best-effort : le resume du prochain send porte l'effort de toute façon */
      }
    },
    [slug],
  );

  // Moteur d'une conversation depuis le sid : onglet ouvert d'abord, sinon l'entrée de
  // l'historique (les mutations rename/delete sont scopées par moteur côté serveur).
  const allConvosRef = useRef(allConvos);
  allConvosRef.current = allConvos;
  const engineOfSid = useCallback((sid, hint) => {
    if (hint) return hint;
    const st = stateRef.current;
    const key = st.order.find((k) => st.convos[k]?.sid === sid);
    return st.convos[key]?.engine || allConvosRef.current.find((c) => c.sessionId === sid)?.engine || undefined;
  }, []);

  const renameBySid = useCallback(
    async (sid, title, engine) => {
      const st = stateRef.current;
      const key = st.order.find((k) => st.convos[k]?.sid === sid);
      const eng = engineOfSid(sid, engine);
      if (key) dispatch({ type: 'SET_TITLE', key, title });
      setAllConvos((prev) => prev.map((x) => (x.sessionId === sid ? { ...x, customTitle: title, summary: title } : x)));
      try {
        await renameConversation(slug, sid, title, eng);
      } catch {
        /* ignore */
      }
    },
    [slug, engineOfSid],
  );

  const removeBySid = useCallback(
    async (sid, engine) => {
      const st = stateRef.current;
      const key = st.order.find((k) => st.convos[k]?.sid === sid);
      const eng = engineOfSid(sid, engine);
      if (key) dispatch({ type: 'CLOSE_PANEL', key });
      setAllConvos((prev) => prev.filter((x) => x.sessionId !== sid));
      try {
        await deleteConversation(slug, sid, eng);
      } catch {
        /* ignore */
      }
    },
    [slug, engineOfSid],
  );

  const value = {
    slug,
    profile,
    pmMode,
    order: state.order,
    convos: state.convos,
    active: state.active,
    unread,
    setActive,
    allConvos,
    unavailableEngines,
    convName,
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
    changeEffort,
    renameBySid,
    removeBySid,
  };

  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}
