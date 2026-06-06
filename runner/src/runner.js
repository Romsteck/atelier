// Atelier agent runner — shim Node minimal autour du Claude Agent SDK.
// stdin = canal DUPLEX NDJSON (1re ligne = init ; lignes suivantes = réponses aux
// questions). Pilote query() et réémet le flux SDK en NDJSON sur stdout (1 objet par
// ligne) que côté Atelier (Rust) parse et publie sur l'EventBus. AskUserQuestion est
// géré nativement via `onUserDialog` (émet une question, attend la réponse de l'UI).
// Aucune logique métier, aucune auth en dur, aucun secret en argv : le runner tourne
// en hr-studio et lit l'OAuth abonnement via HOME/CLAUDE_CONFIG_DIR.
import { query } from '@anthropic-ai/claude-agent-sdk';
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

// stdin = canal DUPLEX NDJSON : 1re ligne = init JSON ; lignes suivantes = messages
// de contrôle (réponses aux questions AskUserQuestion). stdin reste OUVERT pendant
// tout le run (≠ one-shot) pour que `onUserDialog` puisse attendre la réponse de l'UI.
const pendingAnswers = new Map(); // request_id -> resolve(answerMsg)
let onInit;
const initPromise = new Promise((r) => { onInit = r; });
let gotInit = false;
const rl = createInterface({ input: process.stdin });
rl.on('line', (line) => {
  const s = line.trim();
  if (!s) return;
  let msg;
  try { msg = JSON.parse(s); } catch { diag('ligne stdin non-JSON ignorée'); return; }
  if (!gotInit) { gotInit = true; onInit(msg); return; }
  if (msg.type === 'answer' && msg.request_id) {
    const resolve = pendingAnswers.get(msg.request_id);
    if (resolve) { pendingAnswers.delete(msg.request_id); resolve(msg); }
  }
});

let init;
try {
  init = await initPromise;
  if (!init || typeof init !== 'object') fail('init JSON invalide sur stdin.');
} catch (e) {
  fail(`Init JSON invalide sur stdin : ${e?.message || e}`);
}

const {
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

// AskUserQuestion natif : le SDK route le tool vers ce hook (dialogKind 'ask'). On
// émet une question (NDJSON) puis on BLOQUE jusqu'à la réponse de l'UI (stdin), et on
// renvoie le résultat au format AskUserQuestionOutput attendu par le CLI. Un dialogKind
// inconnu, ou un abort, → 'cancelled' (le CLI applique alors le défaut du dialogue).
let questionSeq = 0;
async function onUserDialog(request, opts = {}) {
  const signal = opts.signal;
  if (request?.dialogKind !== 'ask') return { behavior: 'cancelled' };
  const questions = request.payload?.questions || [];
  const request_id = `q${++questionSeq}`;
  emit({ t: 'question', request_id, questions });
  return await new Promise((resolve) => {
    let settled = false;
    const settle = (v) => { if (!settled) { settled = true; resolve(v); } };
    pendingAnswers.set(request_id, (msg) => {
      if (msg.cancelled) return settle({ behavior: 'cancelled' });
      settle({ behavior: 'completed', result: { questions, answers: msg.answers || {}, response: msg.response } });
    });
    if (signal) {
      signal.addEventListener('abort', () => { pendingAnswers.delete(request_id); settle({ behavior: 'cancelled' }); }, { once: true });
    }
  });
}

const options = {
  // effort : 'low'|'medium'|'high'|'xhigh'|'max' — optionnel (xhigh/max = Opus ; Haiku : aucun).
  ...(effort ? { effort } : {}),
  // display:'summarized' obligatoire : sinon les blocs thinking remontent vides sur Opus 4.8/4.7.
  thinking: { type: 'adaptive', display: 'summarized' },
  includePartialMessages: true,
  permissionMode: bypass ? 'bypassPermissions' : 'plan',
  settingSources: [],
  onUserDialog,
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

try {
  for await (const msg of query({ prompt, options })) {
    switch (msg.type) {
      case 'system':
        // msg.model = modèle réellement résolu (vérité terrain affichée par l'UI).
        emit({ t: 'system', subtype: msg.subtype, session_id: msg.session_id, model: msg.model });
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
          if (block.type === 'tool_use') emit({ t: 'tool_use', id: block.id, name: block.name, input: block.input });
        }
        if (msg.error) emit({ t: 'error', message: `assistant: ${msg.error}` });
        break;
      }
      case 'user': {
        for (const block of msg.message?.content || []) {
          if (block.type === 'tool_result') {
            emit({
              t: 'tool_result',
              tool_use_id: block.tool_use_id,
              is_error: !!block.is_error,
              text: toolResultText(block.content).slice(0, 4000),
            });
          }
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
        break;
      default:
        // status / hooks / etc. ignorés en Phase 1
        break;
    }
  }
} catch (e) {
  fail(`query() a échoué : ${e?.message || e}`, 1);
}

// Fin du run : on libère stdin (readline le maintenait ouvert) pour que le process
// puisse sortir et que le backend voie l'EOF de stdout.
rl.close();
process.stdin.destroy();
