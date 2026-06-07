// Atelier agent runner — shim Node minimal autour du Claude Agent SDK.
// SESSION STREAMING : stdin = canal d'entrée NDJSON (1re ligne = init ; lignes
// suivantes = tours utilisateur / réponses / contrôle). `query()` est pilotée en
// mode streaming-input — le générateur passé en `prompt` EST le canal d'entrée :
// le garder ouvert maintient la session vivante sur plusieurs tours (mémoire
// native) ; en sortir (EOF / {type:'end'}) la termine. Le flux SDK est réémis en
// NDJSON sur stdout (1 objet/ligne) que côté Atelier (Rust) parse et publie sur
// l'EventBus. AskUserQuestion est détecté via le flux `tool_use` (le hook natif
// `onUserDialog` ne se déclenche jamais en headless) : on émet une `question`, et
// la réponse de l'UI revient comme TOUR UTILISATEUR suivant dans la même session.
// Aucune logique métier, aucune auth en dur, aucun secret en argv : le runner tourne
// en hr-studio et lit l'OAuth abonnement via HOME/CLAUDE_CONFIG_DIR.
import {
  query,
  listSessions,
  getSessionMessages,
  renameSession,
  deleteSession,
  tagSession,
} from '@anthropic-ai/claude-agent-sdk';
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { createInterface } from 'node:readline';

// stdout = canal d'events NDJSON ; stderr = diagnostics uniquement (jamais parsé).
function emit(obj) {
  process.stdout.write(JSON.stringify(obj) + '\n');
}
function diag(msg) {
  process.stderr.write(`[runner] ${msg}\n`);
}
function fail(message, code = 2) {
  emit({ t: 'error', message });
  diag(message);
  process.exit(code);
}

// Gardes auth (WHY) : une ANTHROPIC_API_KEY dans l'env bascule SILENCIEUSEMENT le
// SDK en facturation clé API au lieu de l'OAuth abonnement Max20x ; un fichier de
// creds manquant produit un 401 opaque. On échoue fort et tôt plutôt qu'en vol.
if (process.env.ANTHROPIC_API_KEY) {
  fail("ANTHROPIC_API_KEY présent dans l'env : le runner doit utiliser l'OAuth abonnement (hr-studio), pas une clé API. Abandon.");
}
const configDir = process.env.CLAUDE_CONFIG_DIR || join(process.env.HOME || '', '.claude');
if (!existsSync(join(configDir, '.credentials.json'))) {
  fail(`Credentials OAuth introuvables sous ${configDir}/.credentials.json — le runner doit tourner en hr-studio (login claude déjà présent).`);
}

// stdin = canal d'entrée NDJSON : 1re ligne = init JSON ; lignes suivantes = tours
// (`user_message`), réponses (`answer`) ou contrôle (`end`/`interrupt`). Les tours
// sont poussés dans une FILE consommée par le générateur d'entrée de `query()` ;
// tant que la file n'est pas close, la session reste vivante (mémoire multi-tour).
const inputQ = []; // SDKUserMessage[] en attente d'être yield par inputGen()
let qResolve = null; // resolver qui réveille le générateur quand un tour arrive
let inputClosed = false; // EOF / {type:'end'} vu → le générateur sortira (fin de session)
let qHandle = null; // référence à la query() pour interrupt() (cf. AskUserQuestion)
let turnActive = false; // un tour est en cours (≠ session idle) — interrupt n'est SÛR que là
let onInit;
const initPromise = new Promise((r) => { onInit = r; });
let gotInit = false;

function userMsg(text) {
  return { type: 'user', message: { role: 'user', content: String(text) }, parent_tool_use_id: null };
}
// Une réponse AskUserQuestion (ou un "Passer") devient un tour utilisateur en clair :
// le modèle reprend de façon déterministe à partir des choix, pas de l'auto-annulation.
function answerToTurn(msg) {
  if (msg.cancelled) return userMsg("J'ai choisi de ne pas répondre à ta question. Continue avec ton meilleur jugement.");
  const lines = Object.entries(msg.answers || {}).map(([q, a]) => `- ${q} → ${a}`);
  let t = lines.length ? `Voici mes réponses à tes questions :\n${lines.join('\n')}` : 'Voici ma réponse.';
  if (msg.response && msg.response.trim()) t += `\n\n${msg.response.trim()}`;
  return userMsg(t);
}
function pushTurn(m) {
  inputQ.push(m);
  turnActive = true; // un tour est soumis → interrupt devient pertinent
  if (qResolve) { const r = qResolve; qResolve = null; r(); }
}
function closeInput() {
  inputClosed = true;
  if (qResolve) { const r = qResolve; qResolve = null; r(); }
}

const rl = createInterface({ input: process.stdin });
rl.on('line', (line) => {
  const s = line.trim();
  if (!s) return;
  let msg;
  try { msg = JSON.parse(s); } catch { diag('ligne stdin non-JSON ignorée'); return; }
  if (!gotInit) { gotInit = true; onInit(msg); return; }
  switch (msg.type) {
    case 'user_message': if (typeof msg.text === 'string') pushTurn(userMsg(msg.text)); break;
    case 'answer': pushTurn(answerToTurn(msg)); break;
    // interrupt UNIQUEMENT si un tour tourne : sur une session idle, interrupt() casse
    // le flush de fin propre. Idle → on ignore (l'arrêt se fait par EOF stdin côté Atelier).
    case 'interrupt': if (turnActive && qHandle) qHandle.interrupt().catch(() => {}); break;
    case 'end': closeInput(); break;
    default: diag(`type de message stdin inconnu: ${msg.type}`);
  }
});
rl.on('close', () => { diag('stdin EOF (rl close) → closeInput'); closeInput(); }); // EOF stdin = fin de session

let init;
try {
  init = await initPromise;
  if (!init || typeof init !== 'object') fail('init JSON invalide sur stdin.');
} catch (e) {
  fail(`Init JSON invalide sur stdin : ${e?.message || e}`);
}

const {
  op, // si présent : mode introspection one-shot (list/messages/rename/delete/tag), pas de session chat
  prompt,
  effort, // optionnel : Haiku 4.5 ne supporte PAS le param effort → on l'omet alors
  cwd,
  allowedTools,
  permissionMode = 'default',
  resume,
  model,
  mcpEndpoint,
  mcpToken,
} = init || {};

// Mode introspection : opère sur les sessions SDK persistées (CLAUDE_CONFIG_DIR)
// puis sort, SANS démarrer de session chat streaming. C'est le pont d'Atelier vers
// listSessions / getSessionMessages / renameSession / deleteSession.
if (op) {
  await runIntrospection(op, init);
  // runIntrospection sort le process ; ne revient jamais.
}

if (!prompt) fail('Champ "prompt" manquant dans l\'init.');

// MCP (WHY) : le token arrive par l'init (stdin), pas par l'env — pour ne pas que
// sudo le journalise. Fallback env pour le smoke-test standalone.
const mcpServers = {};
const token = mcpToken || process.env.MCP_TOKEN;
if (mcpEndpoint && token) {
  mcpServers.studio = {
    type: 'http',
    url: mcpEndpoint,
    headers: { Authorization: `Bearer ${token}` },
  };
} else if (mcpEndpoint) {
  diag('mcpEndpoint fourni mais token MCP absent — serveur MCP non câblé.');
}

// Deux modes de permission seulement (décision produit) :
//   - 'plan' (défaut, SÛR) : le modèle explore + planifie en LECTURE SEULE. On
//     refuse toute écriture/exécution via `canUseTool` (deny-by-default sur une
//     allowlist de lecture) + `disallowedTools` qui retire édition/Bash du contexte.
//   - 'bypassPermissions' : pleine capacité (édition fichiers, MCP, Bash). L'agent
//     tourne en hr-studio et écrit réellement dans le `src/` de l'app — les
//     mutations se relisent via l'onglet Git du Studio.
// WHY settingSources:[] : en headless le SDK n'auto-refuse PAS hors allowedTools,
// et le `.claude/settings.json` généré par Atelier auto-approuve `mcp__studio__*`
// (dont `exec` EN ROOT). On ignore donc tout settings disque et on tranche nous-mêmes.
// AskUserQuestion est un PRÉREQUIS du mode plan (clarifier avant de proposer) → autorisé.
const PLAN_READ = ['Read', 'Glob', 'Grep', 'NotebookRead', 'TodoWrite', 'ExitPlanMode', 'AskUserQuestion', 'WebSearch', 'WebFetch'];
const WRITE_TOOLS = ['Edit', 'Write', 'NotebookEdit', 'MultiEdit'];
const BASH_TOOLS = ['Bash', 'BashOutput', 'KillShell'];

const allowedSet = new Set([...(allowedTools || []), ...PLAN_READ]);
function isAllowed(toolName) {
  if (allowedSet.has(toolName)) return true;
  for (const a of allowedSet) {
    if (a.endsWith('*') && toolName.startsWith(a.slice(0, -1))) return true;
  }
  return false;
}

const bypass = permissionMode === 'bypassPermissions';

// AskUserQuestion n'est PAS géré via `onUserDialog` : le CLI headless ne délègue
// jamais le dialogue 'ask' au host (auto-annulé). On le détecte via le flux
// `tool_use` (cf. boucle de sortie) et la réponse revient comme tour suivant.

const options = {
  // effort : 'low'|'medium'|'high'|'xhigh'|'max' — optionnel (xhigh/max = Opus ; Haiku : aucun).
  ...(effort ? { effort } : {}),
  // display:'summarized' obligatoire : sinon les blocs thinking remontent vides sur Opus 4.8/4.7.
  thinking: { type: 'adaptive', display: 'summarized' },
  includePartialMessages: true,
  permissionMode: bypass ? 'bypassPermissions' : 'plan',
  settingSources: [],
  ...(bypass
    ? {} // pleine puissance : ni canUseTool ni disallowedTools
    : {
        disallowedTools: [...WRITE_TOOLS, ...BASH_TOOLS],
        canUseTool: async (toolName, input) =>
          isAllowed(toolName)
            ? { behavior: 'allow', updatedInput: input }
            : {
                behavior: 'deny',
                message: `Mode Plan (lecture seule) : outil '${toolName}' non autorisé. Bascule en Bypass pour exécuter.`,
              },
      }),
  // Omettre model → le CLI résout le défaut de l'abonnement = claude-opus-4-8[1m] (contexte 1M).
  ...(model ? { model } : {}),
  ...(allowedTools ? { allowedTools } : {}),
  ...(cwd ? { cwd } : {}),
  ...(resume ? { resume } : {}),
  ...(Object.keys(mcpServers).length ? { mcpServers } : {}),
};

function toolResultText(content) {
  if (typeof content === 'string') return content;
  if (Array.isArray(content)) {
    return content.map((x) => (x && x.type === 'text' ? x.text : `[${x?.type || 'block'}]`)).join('');
  }
  return '';
}

// Convertit le transcript persisté (SessionMessage[] = messages Anthropic bruts) en
// items normalisés IDENTIQUES au flux live (user/assistant/thinking/tool_use/tool_result/
// question) — l'UI les rend avec le même code. `getSessionMessages` rend `message`
// opaque : on déballe les blocs comme la boucle live. Une AskUserQuestion devient un
// item `question` ; son tool_result d'auto-annulation est masqué. `answered` = il
// existe un item postérieur (sinon = question finale en attente, à re-proposer).
function messagesToItems(msgs) {
  const items = [];
  const askIds = new Set(); // ids des tool_use AskUserQuestion → masquer leur tool_result
  for (const m of msgs || []) {
    const message = m?.message;
    if (m?.type === 'assistant') {
      for (const b of message?.content || []) {
        if (b.type === 'text' && b.text) items.push({ type: 'assistant', text: b.text });
        else if (b.type === 'thinking' && b.thinking) items.push({ type: 'thinking', text: b.thinking });
        else if (b.type === 'tool_use') {
          if (b.name === 'AskUserQuestion') {
            askIds.add(b.id);
            items.push({ type: 'question', request_id: b.id, questions: b.input?.questions || [] });
          } else {
            items.push({ type: 'tool_use', name: b.name, input: b.input });
          }
        }
      }
    } else if (m?.type === 'user') {
      const content = message?.content;
      if (typeof content === 'string') {
        if (content.trim()) items.push({ type: 'user', text: content });
      } else if (Array.isArray(content)) {
        for (const b of content) {
          if (b.type === 'text' && b.text) items.push({ type: 'user', text: b.text });
          else if (b.type === 'tool_result') {
            if (askIds.has(b.tool_use_id)) continue; // auto-annulation AskUserQuestion masquée
            items.push({ type: 'tool_result', text: toolResultText(b.content).slice(0, 4000), isError: !!b.is_error });
          }
        }
      }
    }
    // type 'system' : ignoré (non rendu dans le fil)
  }
  // Une question est "répondue" dès qu'un item la suit (le tour de réponse en clair).
  items.forEach((it, i) => { if (it.type === 'question') it.answered = i < items.length - 1; });
  return items;
}

// Mode introspection one-shot : exécute l'op SDK, émet UN objet NDJSON, puis sort.
// WHY le write+callback : un transcript volumineux dépasse le buffer du pipe ; un
// `process.exit(0)` immédiat TRONQUE l'écriture async (stdout pas vidé). On sort donc
// dans le callback de `write`, garanti après que le noyau a accepté toute la ligne.
async function runIntrospection(op, init) {
  const dir = init?.cwd; // = src/ de l'app → scope projet pour listSessions/getSessionMessages
  let result;
  try {
    if (op === 'list') {
      result = { t: 'sessions', sessions: await listSessions({ dir }) };
    } else if (op === 'messages') {
      if (!init?.sessionId) throw new Error('sessionId manquant');
      result = { t: 'transcript', items: messagesToItems(await getSessionMessages(init.sessionId, { dir })) };
    } else if (op === 'rename') {
      if (!init?.sessionId) throw new Error('sessionId manquant');
      await renameSession(init.sessionId, String(init.title || ''), { dir });
      result = { t: 'ok' };
    } else if (op === 'delete') {
      if (!init?.sessionId) throw new Error('sessionId manquant');
      await deleteSession(init.sessionId, { dir });
      result = { t: 'ok' };
    } else if (op === 'tag') {
      if (!init?.sessionId) throw new Error('sessionId manquant');
      await tagSession(init.sessionId, init.tag ?? null, { dir });
      result = { t: 'ok' };
    } else {
      result = { t: 'error', message: `op inconnue: ${op}` };
    }
  } catch (e) {
    result = { t: 'error', message: `op ${op} échouée: ${e?.message || e}` };
  }
  rl.close();
  process.stdin.destroy();
  process.stdout.write(JSON.stringify(result) + '\n', () => process.exit(0));
  // On NE retombe JAMAIS dans le chemin chat (sinon son `fail("prompt manquant")`
  // ferait un process.exit(2) qui tronquerait l'écriture ci-dessus encore en vol).
  // On sort exclusivement via le callback de write ci-dessus.
  await new Promise(() => {});
}

// Générateur d'entrée = canal streaming de `query()`. Tant qu'il ne return pas, la
// session vit. On vide la file (un tour par yield), puis on dort jusqu'au prochain
// `pushTurn`/`closeInput`. `inputClosed` → return → fin de session.
async function* inputGen() {
  for (;;) {
    while (inputQ.length) yield inputQ.shift();
    if (inputClosed) return;
    await new Promise((r) => { qResolve = r; });
  }
}

// Le prompt d'init = premier tour utilisateur de la session.
pushTurn(userMsg(prompt));

let sessionEmitted = false; // `system` (session_id/model) n'est émis qu'une fois
const cancelledToolUseIds = new Set(); // tool_use AskUserQuestion → tool_result d'annulation à masquer
try {
  qHandle = query({ prompt: inputGen(), options });
  for await (const msg of qHandle) {
    switch (msg.type) {
      case 'system':
        // msg.model = modèle réellement résolu (vérité terrain affichée par l'UI).
        // Émis une seule fois : la session garde le même session_id sur tous les tours.
        if (!sessionEmitted) {
          emit({ t: 'system', subtype: msg.subtype, session_id: msg.session_id, model: msg.model });
          sessionEmitted = true;
        }
        break;
      case 'stream_event': {
        const ev = msg.event;
        if (ev && ev.type === 'content_block_delta' && ev.delta) {
          if (ev.delta.type === 'text_delta') emit({ t: 'assistant_delta', text: ev.delta.text });
          else if (ev.delta.type === 'thinking_delta') emit({ t: 'thinking_delta', text: ev.delta.thinking });
        }
        break;
      }
      case 'assistant': {
        // Le texte est déjà streamé via les deltas ; ici on ne remonte que les tool_use.
        for (const block of msg.message?.content || []) {
          if (block.type !== 'tool_use') continue;
          if (block.name === 'AskUserQuestion') {
            // Détection live de la question (le hook natif ne se déclenche pas). On
            // émet une `question` ; la réponse de l'UI reviendra comme tour suivant.
            cancelledToolUseIds.add(block.id);
            emit({ t: 'question', request_id: block.id, questions: block.input?.questions || [] });
          } else {
            emit({ t: 'tool_use', id: block.id, name: block.name, input: block.input });
          }
        }
        if (msg.error) emit({ t: 'error', message: `assistant: ${msg.error}` });
        break;
      }
      case 'user': {
        for (const block of msg.message?.content || []) {
          if (block.type !== 'tool_result') continue;
          // Le CLI auto-annule AskUserQuestion ("Pas de réponse sélectionnée") : on
          // masque ce tool_result bidon (la vraie réponse arrive comme tour suivant).
          if (cancelledToolUseIds.has(block.tool_use_id)) continue;
          emit({
            t: 'tool_result',
            tool_use_id: block.tool_use_id,
            is_error: !!block.is_error,
            text: toolResultText(block.content).slice(0, 4000),
          });
        }
        break;
      }
      case 'result':
        emit({
          t: 'result',
          subtype: msg.subtype,
          is_error: !!msg.is_error,
          session_id: msg.session_id,
          total_cost_usd: msg.total_cost_usd,
          usage: msg.usage,
          num_turns: msg.num_turns,
          duration_ms: msg.duration_ms,
          result: typeof msg.result === 'string' ? msg.result : undefined,
        });
        // Fin de tour : l'UI repasse en "idle" (prête pour le tour suivant) sans
        // que la session soit terminée pour autant.
        turnActive = false;
        emit({ t: 'turn_done' });
        break;
      default:
        // status / hooks / etc. ignorés en Phase 1
        break;
    }
  }
} catch (e) {
  fail(`query() a échoué : ${e?.message || e}`, 1);
}

// Sortie de boucle = générateur d'entrée terminé (EOF/{type:'end'}) = fin de session.
// On libère stdin pour que le process sorte et que le backend voie l'EOF de stdout.
rl.close();
process.stdin.destroy();
