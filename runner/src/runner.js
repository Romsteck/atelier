// Atelier agent runner — shim Node minimal autour du Claude Agent SDK.
// SESSION STREAMING : stdin = canal d'entrée NDJSON (1re ligne = init ; lignes
// suivantes = tours utilisateur / réponses / contrôle). `query()` est pilotée en
// mode streaming-input — le générateur passé en `prompt` EST le canal d'entrée :
// le garder ouvert maintient la session vivante sur plusieurs tours (mémoire
// native) ; en sortir (EOF / {type:'end'}) la termine. Le flux SDK est réémis en
// NDJSON sur stdout (1 objet/ligne) que côté Atelier (Rust) parse et publie sur
// l'EventBus. Les dialogues interactifs (AskUserQuestion, ExitPlanMode) sont interceptés
// dans `canUseTool` (le hook `onUserDialog` ne se déclenche pas pour eux en headless,
// vérifié SDK 0.3.167) : on émet `question`/`plan_review`, on SUSPEND le tour sur une
// promesse, et la décision de l'UI (`answer`/`plan_decision` sur stdin) la résout — la
// réponse est livrée AU MODÈLE dans le même tour (pas d'auto-annulation). Stop = `interrupt`
// (abort du tour, session vivante) ≠ `end`/EOF (fin de session).
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

// Toolchain sur PATH (WHY) : le runner est spawné via `sudo -H -u hr-studio` qui
// réinitialise l'env vers son secure_path — `~/.cargo/bin` (cargo) et `~/.local/bin`
// en sont absents. L'outil Bash du SDK hérite de ce process.env, donc les builds
// d'app (cargo) et les appels cargo ad-hoc de l'agent échouaient en "command not found".
// On les rajoute ici, au plus tôt, pour tous les chemins d'exécution (idempotent).
if (process.env.HOME) {
  process.env.PATH = `${process.env.HOME}/.cargo/bin:${process.env.HOME}/.local/bin:${process.env.PATH || ''}`;
}

// stdin = canal d'entrée NDJSON : 1re ligne = init JSON ; lignes suivantes = tours
// (`user_message`), réponses (`answer`) ou contrôle (`end`/`interrupt`). Les tours
// sont poussés dans une FILE consommée par le générateur d'entrée de `query()` ;
// tant que la file n'est pas close, la session reste vivante (mémoire multi-tour).
const inputQ = []; // SDKUserMessage[] en attente d'être yield par inputGen()
let qResolve = null; // resolver qui réveille le générateur quand un tour arrive
let inputClosed = false; // EOF / {type:'end'} vu → le générateur sortira (fin de session)
let qHandle = null; // référence à la query() pour interrupt() / setPermissionMode()
let liveMode = 'plan'; // mode produit COURANT ('plan'|'bypass') — suit setPermissionMode pour le garde-fou MCP
let turnActive = false; // un tour est en cours (≠ session idle) — interrupt n'est SÛR que là
let onInit;
const initPromise = new Promise((r) => { onInit = r; });
let gotInit = false;

// Dialogues BLOQUANTS interceptés dans canUseTool : AskUserQuestion et ExitPlanMode.
// Au lieu de laisser le SDK auto-annuler la question (headless), on suspend le tour sur
// une promesse jusqu'à ce que l'UI réponde (`answer` / `plan_decision` sur stdin). C'est
// la SEULE façon fiable de bloquer : `onUserDialog` ne se déclenche pas pour ces outils
// en headless (vérifié SDK 0.3.167), mais `canUseTool` SI, et son await bloque le tour.
const pendingDialogs = new Map(); // request_id → resolve(payload)
const maskedToolUseIds = new Set(); // tool_use dont le tool_result est livré hors-bande → masqué
let dialogSeq = 0;
// Suspend le tour jusqu'à la réponse UI. Un abort du tour (interrupt) débloque via le signal.
function waitDialog(requestId, signal) {
  return new Promise((resolve) => {
    if (signal?.aborted) { resolve({ aborted: true }); return; }
    pendingDialogs.set(requestId, resolve);
    signal?.addEventListener('abort', () => {
      if (pendingDialogs.delete(requestId)) resolve({ aborted: true });
    }, { once: true });
  });
}
// Réponse AskUserQuestion → texte livré au modèle COMME résultat de l'outil (via deny+message,
// seul canal de canUseTool pour transmettre du texte ; vérifié : le modèle l'exploite tel quel).
function formatAnswerForModel(ans) {
  if (!ans || ans.aborted || ans.cancelled) return "L'utilisateur n'a pas répondu à la question. Continue avec ton meilleur jugement.";
  const lines = Object.entries(ans.answers || {}).map(([q, a]) => `- ${q} → ${a}`);
  let t = lines.length ? `Réponses de l'utilisateur :\n${lines.join('\n')}` : "Réponse de l'utilisateur.";
  if (ans.response && ans.response.trim()) t += `\n\n${ans.response.trim()}`;
  return t;
}
// Vrai si c'est l'écriture du fichier de plan interne (~/.claude/plans/*.md) du mode plan natif.
function isPlanFileWrite(name, input) {
  return (name === 'Write' || name === 'Edit') && typeof input?.file_path === 'string' && input.file_path.includes('/.claude/plans/');
}

// Un tour utilisateur. Sans image → `content` reste une chaîne (chemin historique).
// Avec image(s) → `content` devient un tableau de blocs Anthropic ({text} optionnel +
// {image, source:base64}) — forme acceptée par MessageParam du SDK (vérifié sdk.d.ts).
function userMsg(text, images) {
  const t = text == null ? '' : String(text);
  if (Array.isArray(images) && images.length) {
    const content = [];
    if (t.trim()) content.push({ type: 'text', text: t });
    for (const img of images) {
      if (img && img.media_type && img.data) {
        content.push({ type: 'image', source: { type: 'base64', media_type: img.media_type, data: img.data } });
      }
    }
    return { type: 'user', message: { role: 'user', content }, parent_tool_use_id: null };
  }
  return { type: 'user', message: { role: 'user', content: t }, parent_tool_use_id: null };
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
  // Débloque tout dialogue en attente (sinon le canUseTool resterait suspendu jusqu'au
  // SIGKILL du reaper) : on les résout en "non répondu" pour que le tour s'achève et que
  // la session se termine proprement (flush du transcript sur disque).
  for (const resolve of pendingDialogs.values()) resolve({ aborted: true });
  pendingDialogs.clear();
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
    case 'user_message':
      if (typeof msg.text === 'string' || Array.isArray(msg.images)) pushTurn(userMsg(msg.text, msg.images));
      break;
    // Réponse AskUserQuestion : débloque le tour suspendu dans canUseTool. Fallback (pas de
    // dialogue en attente) = session reprise depuis l'historique → on l'injecte en tour clair.
    case 'answer': {
      const resolve = pendingDialogs.get(msg.request_id);
      if (resolve) { pendingDialogs.delete(msg.request_id); resolve({ answers: msg.answers, response: msg.response, cancelled: msg.cancelled }); }
      else pushTurn(answerToTurn(msg));
      break;
    }
    // Décision sur un plan (ExitPlanMode) : approuver = implémenter, sinon renvoyer en révision.
    case 'plan_decision': {
      const resolve = pendingDialogs.get(msg.request_id);
      if (resolve) { pendingDialogs.delete(msg.request_id); resolve({ approved: !!msg.approved, feedback: msg.feedback }); }
      break;
    }
    // interrupt UNIQUEMENT si un tour tourne : sur une session idle, interrupt() casse
    // le flush de fin propre. Idle → on ignore (l'arrêt se fait par EOF stdin côté Atelier).
    case 'interrupt': if (turnActive && qHandle) qHandle.interrupt().catch(() => {}); break;
    // Changement de mode/modèle EN COURS de session (setPermissionMode/setModel — possibles
    // en streaming-input ; l'effort, lui, est figé au démarrage, pas d'API live). On émet en
    // retour pour que l'UI reflète l'état réel.
    case 'set_mode':
      if (qHandle && (msg.mode === 'plan' || msg.mode === 'bypass')) {
        qHandle.setPermissionMode(msg.mode === 'bypass' ? 'acceptEdits' : 'plan').catch(() => {});
        liveMode = msg.mode;
        emit({ t: 'permission_mode', mode: msg.mode });
      }
      break;
    case 'set_model':
      if (qHandle) {
        qHandle.setModel(msg.model || undefined).catch(() => {});
        emit({ t: 'model', model: msg.model || null });
      }
      break;
    case 'end': closeInput(); break;
    default: diag(`type de message stdin inconnu: ${msg.type}`);
  }
});
rl.on('close', () => { diag('stdin EOF (rl close) → closeInput'); closeInput(); }); // EOF stdin = fin de session

// Arrêt PROPRE sur signal — backstop du drain piloté par Atelier (interrupt+EOF via stdin).
// Si le cgroup est tué malgré KillMode=mixed (ex. crash d'Atelier, `systemctl kill`), on
// avorte le tour en vol (frontière propre → pas de tool_use pendouillant) puis on ferme
// l'entrée → le SDK termine la session et flush un transcript RESUMABLE. On NE fait PAS
// process.exit() : on laisse la boucle `for await` finir pour que le flush s'achève (un
// exit prématuré tronquerait le transcript, le défaut même qu'on corrige).
let shuttingDown = false;
function onShutdownSignal(sig) {
  if (shuttingDown) return;
  shuttingDown = true;
  diag(`signal ${sig} reçu → interrupt du tour + fin de session`);
  if (turnActive && qHandle) qHandle.interrupt().catch(() => {});
  closeInput();
}
process.on('SIGTERM', () => onShutdownSignal('SIGTERM'));
process.on('SIGINT', () => onShutdownSignal('SIGINT'));

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
  permissionMode = 'plan', // défaut SÛR : lecture seule si non précisé
  // (allowedTools n'est plus consommé : canUseTool + permissionMode gouvernent les permissions)
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

// prompt vide TOLÉRÉ si des images sont jointes (tour image-only).
if (!prompt && !(Array.isArray(init.images) && init.images.length)) fail('Champ "prompt" manquant dans l\'init.');

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

// Deux modes produit, mappés sur le SDK + un canUseTool UNIQUE qui est le host complet :
//   - 'plan' (défaut, SÛR) : permissionMode SDK 'plan'. Le SDK applique nativement la
//     LECTURE SEULE (vérifié : il refuse Write/Bash sur de vrais fichiers, n'autorise que
//     le fichier de plan). Plus de `disallowedTools` — c'était lui qui retirait Write et
//     cassait l'écriture du plan + faisait dérailler le modèle.
//   - 'bypass' : permissionMode SDK 'acceptEdits' (PAS 'bypassPermissions') — les éditions
//     s'exécutent ET canUseTool reste consulté, donc les questions bloquent aussi dans ce mode.
// canUseTool intercepte les 2 dialogues que le headless n'achemine pas au host autrement :
//   - AskUserQuestion : on émet une `question`, on SUSPEND le tour, et on livre la réponse
//     au modèle via deny+message (le SDK reprend le tour avec — pas d'auto-annulation).
//   - ExitPlanMode : on émet un `plan_review`, on SUSPEND ; approuver = on bascule la session
//     en 'acceptEdits' (setPermissionMode) et on `allow` (le SDK enchaîne sur l'implémentation).
// WHY settingSources:['project'] (et SEULEMENT 'project') : c'est la source qui charge
// CLAUDE.md, .claude/rules/ et les skills projet — sans elle l'agent travaille sans les
// règles du workspace (bug constaté). On EXCLUT 'user' et 'local' : les settings user
// hr-studio portent l'auto-approve `mcp__studio` des sessions interactives, et les
// settings.local.json accumulent des allow larges (`Bash(*)`, `Write(*)`…) issus du
// terminal Studio. INVARIANT : aucune source chargée ne doit contenir de permissions.allow
// — une allow rule court-circuite canUseTool (vérifié SDK 0.3.167 : `mcp__studio` en allow
// exécute `exec` EN ROOT même en Plan, sans consulter canUseTool). Le settings.json généré
// par Atelier (context.rs) n'émet donc PLUS de bloc permissions.
const isPlan = permissionMode !== 'bypassPermissions';
liveMode = isPlan ? 'plan' : 'bypass';

// Garde-fou Plan pour les tools MCP : le mode plan natif ne bloque QUE les tools builtin
// (Write/Edit/Bash…) ; les tools MCP arrivent dans canUseTool même en Plan (vérifié), donc
// sans cette liste le blanket-allow exécuterait `mcp__studio__exec` (root) en lecture seule.
// Allowlist par suffixe (nom sans préfixe mcp__<server>__) : lectures uniquement.
const MCP_READONLY = new Set([
  'findings_list', 'memory_get', 'runs_list', 'pm_query', 'status', 'logs',
  'db_tables', 'db_schema', 'db_query', 'db_overview', 'db_count_rows', 'db_get_schema',
  'docs_overview', 'docs_list_entries', 'docs_get', 'docs_search', 'docs_completeness',
  'docs_diagram_get', 'git_log', 'git_branches', 'scan_get',
]);

async function canUseTool(toolName, input, opts) {
  if (toolName === 'AskUserQuestion') {
    const requestId = opts?.toolUseID || `ask-${++dialogSeq}`;
    maskedToolUseIds.add(requestId);
    emit({ t: 'question', request_id: requestId, questions: input?.questions || [] });
    const ans = await waitDialog(requestId, opts?.signal);
    return { behavior: 'deny', message: formatAnswerForModel(ans) };
  }
  if (toolName === 'ExitPlanMode') {
    const requestId = opts?.toolUseID || `plan-${++dialogSeq}`;
    maskedToolUseIds.add(requestId);
    emit({ t: 'plan_review', request_id: requestId, plan: input?.plan || '' });
    const dec = await waitDialog(requestId, opts?.signal);
    if (dec?.approved) {
      // Approbation = on quitte la lecture seule pour que l'implémentation écrive vraiment,
      // DANS LA MÊME SESSION (mémoire du plan conservée). Vérifié : le switch persiste sur
      // les tours suivants. On notifie l'UI pour qu'elle reflète le passage Plan → Bypass.
      await qHandle?.setPermissionMode('acceptEdits').catch(() => {});
      liveMode = 'bypass';
      emit({ t: 'permission_mode', mode: 'bypass' });
      return { behavior: 'allow', updatedInput: input };
    }
    const why = dec?.feedback?.trim();
    return {
      behavior: 'deny',
      message: why
        ? `L'utilisateur n'approuve pas encore le plan. Retour : ${why}\nAffine le plan (toujours en lecture seule) puis re-propose.`
        : "L'utilisateur n'a pas approuvé le plan. Continue de l'affiner en lecture seule, puis re-propose.",
    };
  }
  if (liveMode === 'plan' && toolName.startsWith('mcp__')) {
    const bare = toolName.split('__').slice(2).join('__');
    if (!MCP_READONLY.has(bare)) {
      return {
        behavior: 'deny',
        message: `Mode plan (lecture seule) : \`${toolName}\` modifie l'état. Ne l'exécute pas maintenant — intègre cette action à ton plan.`,
      };
    }
  }
  return { behavior: 'allow', updatedInput: input };
}

// Suivi de la todolist COURANTE de la session (WHY) : sert au hook UserPromptSubmit
// ci-dessous, qui doit savoir si la liste est 100% terminée à l'arrivée d'un nouveau
// tour. On réplique la dérivation du front (AgentPanel `reduceTasks`/`latestTodos`) :
// `TodoWrite` (ancien système) OU réduction du flux `Task*` (nouveau SDK ≥0.3.x) — une
// session n'emploie que l'un des deux. Alimenté depuis la boucle `case 'assistant'`.
let todoWriteList = []; // dernier TodoWrite vu (liste complète)
const taskById = new Map(); // Task* : id ordinal (« 1,2,3… ») → {status,content,activeForm}
const taskOrder = [];
let taskCreated = 0;
function trackTodo(name, input) {
  if (name === 'TodoWrite') {
    todoWriteList = Array.isArray(input?.todos) ? input.todos : [];
  } else if (name === 'TaskCreate') {
    const id = String(++taskCreated);
    taskById.set(id, { id, content: input?.subject || '', activeForm: input?.activeForm || '', status: 'pending' });
    taskOrder.push(id);
  } else if (name === 'TaskUpdate') {
    const id = String(input?.taskId ?? '');
    if (!id) return;
    let t = taskById.get(id);
    if (!t) { t = { id, content: '', activeForm: '', status: 'pending' }; taskById.set(id, t); taskOrder.push(id); }
    if (input?.subject) t.content = input.subject;
    if (input?.activeForm) t.activeForm = input.activeForm;
    if (input?.status) t.status = input.status;
  }
}
function effectiveTodos() {
  // Task* prime s'il est employé (comme `reducedTasks.length ? reducedTasks : latestTodos`).
  if (taskOrder.length) return taskOrder.map((id) => taskById.get(id)).filter((t) => t && t.status !== 'deleted');
  return todoWriteList;
}

// Hook UserPromptSubmit = rappel de reset DÉTERMINISTE, piloté par l'HÔTE (donc indépendant
// de la mémoire de l'agent, qui oublie souvent de nettoyer sa todolist terminée après un
// long effort). À CHAQUE nouveau prompt utilisateur : si la todolist courante est non vide
// ET 100% complétée, on injecte un additionalContext ordonnant sa réinitialisation. Dédup
// par signature → un seul rappel tant que la même liste complète subsiste (pas de spam si
// l'agent l'ignore et que l'utilisateur ré-écrit). Reset de la signature dès que la liste
// redevient incomplète/vide, pour re-nudger sur une future complétion.
let lastNudgedSig = null;
async function todoResetHook() {
  const todos = effectiveTodos();
  const total = todos.length;
  const done = todos.filter((t) => t && t.status === 'completed').length;
  if (!total || done !== total) { lastNudgedSig = null; return {}; }
  const sig = JSON.stringify(todos.map((t) => `${t.status}:${t.content}`));
  if (sig === lastNudgedSig) return {};
  lastNudgedSig = sig;
  diag(`todo-reset: todolist terminée (${done}/${total}) au nouveau tour → rappel de reset injecté`);
  return {
    hookSpecificOutput: {
      hookEventName: 'UserPromptSubmit',
      additionalContext:
        `La todolist précédente est terminée à 100 % (${done}/${total}). Nouvelle requête reçue : ` +
        `commence par réinitialiser ta todolist AVANT toute autre chose — un TodoWrite avec une liste vide ` +
        `si la nouvelle demande est triviale, sinon une nouvelle liste pour cette tâche. ` +
        `Ne laisse pas l'ancienne checklist complétée épinglée.`,
    },
  };
}

const options = {
  // effort : 'low'|'medium'|'high'|'xhigh'|'max' — optionnel (xhigh/max = Opus ; Haiku : aucun).
  ...(effort ? { effort } : {}),
  // display:'summarized' obligatoire : sinon les blocs thinking remontent vides sur Opus 4.8/4.7.
  thinking: { type: 'adaptive', display: 'summarized' },
  includePartialMessages: true,
  permissionMode: isPlan ? 'plan' : 'acceptEdits',
  // 'project' charge CLAUDE.md + .claude/rules/ + skills du workspace (cf. WHY ci-dessus).
  // Le preset claude_code est REQUIS pour que CLAUDE.md soit injecté (vérifié SDK 0.3.167).
  settingSources: ['project'],
  systemPrompt: { type: 'preset', preset: 'claude_code' },
  canUseTool, // host unique : AskUserQuestion (2 modes) + ExitPlanMode (plan) + garde-fou MCP en plan
  hooks: { UserPromptSubmit: [{ hooks: [todoResetHook] }] }, // reset déterministe de la todolist terminée au nouveau tour
  // Omettre model → le CLI résout le défaut de l'abonnement = claude-opus-4-8[1m] (contexte 1M).
  ...(model ? { model } : {}),
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

// RÉDUCTION DU PAYLOAD (WHY) : l'UI n'affiche qu'un libellé compact « verbe + cible » par
// appel d'outil (l'action en cours en live, le reste replié). Envoyer l'`input` intégral
// (le `content` complet d'un Write, les old/new_string d'un Edit…) chargeait le front pour
// rien. On ne garde par outil que les champs nécessaires à ce libellé. SEUL TodoWrite garde
// son `input` entier (la checklist épinglée a besoin de tous les todos).
function truncStr(v, n = 200) {
  const s = typeof v === 'string' ? v : '';
  return s.length > n ? s.slice(0, n) : s;
}
function trimToolInput(name, input) {
  const inp = input && typeof input === 'object' && !Array.isArray(input) ? input : {};
  switch (name) {
    case 'TodoWrite':
      return { todos: Array.isArray(inp.todos) ? inp.todos : [] };
    case 'Read':
    case 'Write':
    case 'Edit':
    case 'MultiEdit':
      return { file_path: inp.file_path };
    case 'NotebookEdit':
      return { notebook_path: inp.notebook_path, edit_mode: inp.edit_mode, cell_type: inp.cell_type };
    case 'Bash':
      return { command: truncStr(inp.command, 300), description: truncStr(inp.description, 120), run_in_background: inp.run_in_background };
    case 'Glob':
      return { pattern: inp.pattern, path: inp.path };
    case 'Grep':
      return { pattern: inp.pattern, path: inp.path, glob: inp.glob, type: inp.type };
    case 'WebFetch':
      return { url: inp.url, prompt: truncStr(inp.prompt, 200) };
    case 'WebSearch':
      return { query: inp.query };
    case 'Task':
    case 'Agent':
      return { description: truncStr(inp.description, 200), subagent_type: inp.subagent_type };
    // Système de tâches du SDK (≥0.3.x, remplace TodoWrite) : la checklist épinglée du front
    // se reconstruit en repliant TaskCreate (sujet, id = ordre de création) + TaskUpdate
    // (taskId + status). On ne garde que les champs utiles à la checklist (pas `description`).
    case 'TaskCreate':
      return { subject: truncStr(inp.subject, 200), activeForm: truncStr(inp.activeForm, 120) };
    case 'TaskUpdate':
      return { taskId: inp.taskId, status: inp.status, subject: truncStr(inp.subject, 200), activeForm: truncStr(inp.activeForm, 120) };
    default: {
      // MCP/inconnu : résumé borné (≤4 clés, valeurs tronquées) — assez pour un libellé kv.
      const out = {};
      for (const [k, v] of Object.entries(inp).slice(0, 4)) {
        out[k] = typeof v === 'string' ? truncStr(v, 200) : v;
      }
      return out;
    }
  }
}
// Le corps d'un tool_result n'est utile QUE sur erreur (diagnostic). Succès → pas de corps
// (le front ne l'affiche de toute façon plus ; la revue de fichiers passe par l'onglet Git).
function trimToolResultText(isError, text) {
  if (!isError) return '';
  return (text || '').slice(0, 800);
}

// Convertit le transcript persisté (SessionMessage[] = messages Anthropic bruts) en
// items normalisés IDENTIQUES au flux live (user/assistant/thinking/tool_use/tool_result/
// question) — l'UI les rend avec le même code. `getSessionMessages` rend `message`
// opaque : on déballe les blocs comme la boucle live. Une AskUserQuestion devient un
// item `question` ; son tool_result d'auto-annulation est masqué. `answered` = il
// existe un item postérieur (sinon = question finale en attente, à re-proposer).
function messagesToItems(msgs) {
  const items = [];
  const askIds = new Set(); // tool_use AskUserQuestion → leur tool_result porte la réponse
  const planIds = new Set(); // tool_use ExitPlanMode → leur tool_result porte la décision
  const planFileIds = new Set(); // tool_use Write ~/.claude/plans/*.md → masqué (plomberie)
  for (const m of msgs || []) {
    const message = m?.message;
    if (m?.type === 'assistant') {
      for (const b of message?.content || []) {
        if (b.type === 'text' && b.text) items.push({ type: 'assistant', text: b.text });
        // Réflexion : on n'expose QUE le compteur (chars) — jamais le texte (le front n'affiche
        // qu'un count, on ne charge donc jamais le détail des réflexions).
        else if (b.type === 'thinking' && b.thinking) items.push({ type: 'thinking', chars: b.thinking.length });
        else if (b.type === 'tool_use') {
          if (b.name === 'AskUserQuestion') {
            askIds.add(b.id);
            items.push({ type: 'question', request_id: b.id, questions: b.input?.questions || [] });
          } else if (b.name === 'ExitPlanMode') {
            planIds.add(b.id);
            items.push({ type: 'plan_review', request_id: b.id, plan: b.input?.plan || '' });
          } else if (isPlanFileWrite(b.name, b.input)) {
            planFileIds.add(b.id); // masqué (le plan est surfacé via plan_review)
          } else {
            items.push({ type: 'tool_use', name: b.name, input: trimToolInput(b.name, b.input), id: b.id });
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
          // Image collée par l'utilisateur : on ne réinjecte pas le base64 (lourd, déjà
          // consommé par le modèle) — juste un marqueur pour que le tour ne soit pas vide.
          else if (b.type === 'image') {
            const lastUser = items.length && items[items.length - 1].type === 'user' ? items[items.length - 1] : null;
            if (lastUser) lastUser.text = `${lastUser.text} 🖼`.trim();
            else items.push({ type: 'user', text: '🖼 image' });
          } else if (b.type === 'tool_result') {
            const txt = toolResultText(b.content);
            // Le tool_result d'AskUserQuestion porte la réponse (deny+message) → on l'accroche
            // à la question au lieu de l'afficher comme résultat brut.
            if (askIds.has(b.tool_use_id)) {
              const q = items.findLast?.((it) => it.type === 'question' && it.request_id === b.tool_use_id)
                || [...items].reverse().find((it) => it.type === 'question' && it.request_id === b.tool_use_id);
              if (q) { q.answered = true; q.answer = txt; }
              continue;
            }
            // ExitPlanMode : allow → "User has approved your plan" (is_error=false) ; deny → refusé.
            if (planIds.has(b.tool_use_id)) {
              const p = items.findLast?.((it) => it.type === 'plan_review' && it.request_id === b.tool_use_id)
                || [...items].reverse().find((it) => it.type === 'plan_review' && it.request_id === b.tool_use_id);
              if (p) { p.decided = true; p.approved = !b.is_error; }
              continue;
            }
            if (planFileIds.has(b.tool_use_id)) continue; // résultat du Write de plomberie
            items.push({ type: 'tool_result', text: trimToolResultText(!!b.is_error, txt), isError: !!b.is_error, tool_use_id: b.tool_use_id });
          }
        }
      }
    }
    // type 'system' : ignoré (non rendu dans le fil)
  }
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

// Le prompt d'init = premier tour utilisateur de la session (avec ses images éventuelles).
pushTurn(userMsg(prompt, init.images));

let sessionEmitted = false; // `system` (session_id/model) n'est émis qu'une fois
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
          // Suit l'état de la todolist (input INTÉGRAL, avant tout trim) pour le hook de reset.
          trackTodo(block.name, block.input);
          // AskUserQuestion / ExitPlanMode sont matérialisés par canUseTool (events
          // `question` / `plan_review`) — on ne ré-émet pas leur tool_use brut, et on masque
          // leur tool_result par block.id (source d'id fiable : l'assistant arrive AVANT
          // canUseTool et le tool_result).
          if (block.name === 'AskUserQuestion' || block.name === 'ExitPlanMode') { maskedToolUseIds.add(block.id); continue; }
          // Le mode plan natif fait écrire le plan dans ~/.claude/plans/*.md (plomberie
          // interne) : le contenu est déjà surfacé via plan_review → on masque ce Write.
          if (isPlanFileWrite(block.name, block.input)) { maskedToolUseIds.add(block.id); continue; }
          emit({ t: 'tool_use', id: block.id, name: block.name, input: trimToolInput(block.name, block.input) });
        }
        if (msg.error) emit({ t: 'error', message: `assistant: ${msg.error}` });
        break;
      }
      case 'user': {
        for (const block of msg.message?.content || []) {
          if (block.type !== 'tool_result') continue;
          // tool_result d'AskUserQuestion/ExitPlanMode : la réponse/décision est livrée
          // hors-bande (deny+message) → on masque le résultat brut.
          if (maskedToolUseIds.has(block.tool_use_id)) continue;
          emit({
            t: 'tool_result',
            tool_use_id: block.tool_use_id,
            is_error: !!block.is_error,
            text: trimToolResultText(!!block.is_error, toolResultText(block.content)),
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
