// Atelier surveillance scan runner — shim Node headless, LECTURE SEULE, single-turn.
// Pendant du runner.js interactif (agent Studio), mais radicalement plus simple :
//   - un SEUL tour (le prompt d'init), pas de canal stdin multi-tour, pas de dialogues
//     (AskUserQuestion/ExitPlanMode), pas d'introspection de sessions ;
//   - LECTURE SEULE stricte : canUseTool n'autorise que Read/Glob/Grep + les tools MCP
//     (le serveur les borne déjà à la whitelist `?scope=surveillance`), et REFUSE tout le
//     reste (Write/Edit/Bash/AskUserQuestion/ExitPlanMode/…). Le scan ne modifie jamais le
//     code ni les données de l'app ; il signale ses findings via le tool MCP findings_upsert ;
//   - persistSession:false → aucune session écrite sur disque, donc le scan ne POLLUE PAS la
//     liste de conversations du Studio (listSessions ne le voit pas).
// Le flux SDK est réémis en NDJSON sur stdout (1 objet {t:…}/ligne) que le driver Rust
// (atelier_watcher::claude) relaie tel quel à la console live + lit `usage` pour les tokens.
// Aucune auth en dur, aucun secret en argv : tourne en hr-studio, OAuth abonnement via
// HOME/CLAUDE_CONFIG_DIR ; le token MCP arrive par l'init (stdin), jamais par l'env.
import { query, deleteSession } from '@anthropic-ai/claude-agent-sdk';
import { createInterface } from 'node:readline';
import {
  makeIo,
  assertOAuthOnly,
  buildMcpServers,
  toolResultText,
  makeSdkAuthReporter,
  SDK_AUTH_ERRORS,
  SDK_AUTH_RE,
} from './common.js';

const { emit, diag, fail } = makeIo('scan');
// Signale UNE fois un authentication_failed du SDK (token OAuth mort) → run FAILED
// côté Rust (claude.rs::exec) + notification plateforme dédupliquée.
const reportSdkAuth = makeSdkAuthReporter(emit);

// Fin propre : émet `done`, sort dans le callback du write (flush garanti). Le driver Rust
// voit l'EOF de stdout = fin du scan (sans attendre le timeout → run `success`, pas `failed`).
function emitDoneAndExit() {
  process.stdout.write(JSON.stringify({ t: 'done', exit_ok: true }) + '\n', () => process.exit(0));
}

// Échec MCP fatal : les findings passent par les tools MCP (findings_upsert), pas par
// stdout — si le serveur MCP est injoignable/refusé (token invalide → 401), le scan
// sortirait quand même en exit 0 et le run serait marqué `success_empty` (faux « tout
// est clean »). L'event `error` porte un `code` machine (`mcp_auth_failed`) que le
// driver Rust (claude.rs) mappe en run FAILED. Sortie flush-safe, comme fail().
function failMcp(message) {
  diag(message);
  process.stdout.write(JSON.stringify({ t: 'error', code: 'mcp_auth_failed', message }) + '\n', () => process.exit(2));
  return new Promise(() => {});
}

// La garde OAuth est appelée APRÈS le parse de l'init (le token longue durée
// éventuel arrive par stdin et doit être posé sur process.env AVANT la garde).

// Arrêt sur signal : aucune session à flush (persistSession:false), on sort directement.
// Le driver Rust SIGKILL le groupe de process sur cancel/timeout ; ce handler couvre le
// SIGTERM amont (ex. drain du service).
for (const sig of ['SIGTERM', 'SIGINT']) {
  process.on(sig, () => { diag(`signal ${sig} reçu → exit`); process.exit(0); });
}

// Init : une SEULE ligne JSON sur stdin (le driver écrit l'init + "\n" puis ferme stdin).
const rl = createInterface({ input: process.stdin });
const initLine = await new Promise((resolve) => { rl.once('line', resolve); });
rl.close();
process.stdin.destroy();

let init;
try {
  init = JSON.parse(initLine);
  if (!init || typeof init !== 'object') throw new Error('pas un objet');
} catch (e) {
  await fail(`Init JSON invalide sur stdin : ${e?.message || e}`);
}

const { op, prompt, cwd, model, effort, mcpEndpoint, mcpToken, oauthToken } = init || {};

// Token OAuth abonnement longue durée injecté par Atelier (stdin) : posé sur
// process.env AVANT tout query(). Même canal anti-leak que MCP_TOKEN.
if (oauthToken) process.env.CLAUDE_CODE_OAUTH_TOKEN = oauthToken;

// Mode nettoyage one-shot : supprime la session SDK persistée puis sort. WHY : le scan ne
// doit PAS polluer la liste de conversations du Studio (listSessions). `persistSession:false`
// est ignoré par le binaire natif 0.3.167 (vérifié e2e 2026-06-17 : la session est quand même
// écrite) → on supprime explicitement la session après le run. Piloté par le driver Rust pour
// couvrir TOUS les cas (succès / échec / annulation SIGKILL, où scan.js ne peut pas se nettoyer).
if (op === 'delete') {
  if (!init.sessionId) await fail('op delete : champ "sessionId" manquant dans l\'init.');
  try { await deleteSession(init.sessionId, { dir: cwd }); } catch (e) { diag(`deleteSession: ${e?.message || e}`); }
  process.stdout.write(JSON.stringify({ t: 'deleted' }) + '\n', () => process.exit(0));
  // Sortie exclusive via le callback du write ci-dessus — ne JAMAIS retomber dans le flux scan.
  await new Promise(() => {});
}

// Garde OAuth : après le bloc `op:delete` (deleteSession est disque-only, sans
// auth) et après l'injection du token. Refuse ANTHROPIC_API_KEY ; accepte le token
// longue durée OU un .credentials.json présent.
await assertOAuthOnly('scan runner', fail);

if (!prompt) await fail('Champ "prompt" manquant dans l\'init.');

// L'URL MCP porte déjà `?scope=surveillance` → le serveur n'expose et n'accepte que la
// whitelist read-only (findings_upsert/dismiss/resolve + memory + lectures).
const mcpServers = buildMcpServers(
  mcpEndpoint,
  mcpToken,
  diag,
  'mcpEndpoint fourni mais token MCP absent — serveur MCP non câblé (le scan ne pourra rien signaler).',
);

// LECTURE SEULE stricte — garantie par `disallowedTools` (couche autoritaire côté client).
// WHY pas `permissionMode:'plan'` : 'plan' = "no execution of tools" → il empêcherait
// findings_upsert de s'exécuter et pousserait le modèle à planifier au lieu d'agir.
// WHY pas le seul `canUseTool` : en mode 'default', le SDK NE consulte PAS canUseTool pour
// les builtins (vérifié e2e 2026-06-17 : Bash s'est exécuté malgré un canUseTool deny) — un
// allow-rule de settings.local.json le court-circuite aussi. `disallowedTools` RETIRE les
// outils du contexte du modèle ("cannot be used, even if they would otherwise be allowed",
// sdk.d.ts 0.3.167) → garantie dure, indépendante du mode et des settings. On retire tout ce
// qui mute (fichiers/shell/build-deploy/sous-agents) + les dialogues sans humain. Restent :
// Read/Glob/Grep + les tools MCP (déjà bornés par le scope serveur `?scope=surveillance`).
const DISALLOWED = [
  'Bash', 'BashOutput', 'KillShell', 'KillBash',          // shell (mutant + read-only via shell)
  'Write', 'Edit', 'MultiEdit', 'NotebookEdit',           // écriture fichiers
  'Task',                                                  // sous-agents (capacités arbitraires)
  'Skill', 'SlashCommand',                                 // skills (app-build/app-deploy = mutations)
  'WebFetch', 'WebSearch',                                 // réseau externe (hors périmètre d'un scan code)
  'TodoWrite',                                             // bruit
  'ExitPlanMode', 'AskUserQuestion',                       // dialogues : pas d'humain dans la boucle
];

// canUseTool = backstop pour les tools MCP (le scope serveur reste l'autorité). Blanket-allow
// mcp__ (déjà borné serveur) + Read/Glob/Grep ; refuse le reste (n'est consulté que pour MCP
// en pratique, les builtins dangereux étant déjà retirés par disallowedTools).
const ALLOWED_BUILTIN = new Set(['Read', 'Glob', 'Grep']);
async function canUseTool(toolName, input) {
  if (typeof toolName === 'string' && toolName.startsWith('mcp__')) {
    return { behavior: 'allow', updatedInput: input };
  }
  if (ALLOWED_BUILTIN.has(toolName)) {
    return { behavior: 'allow', updatedInput: input };
  }
  return {
    behavior: 'deny',
    message: `Scan en lecture seule : \`${toolName}\` est interdit. Lis le code avec Read/Glob/Grep et signale via les outils MCP de surveillance (findings_upsert).`,
  };
}

const options = {
  // effort 'low'|'medium'|'high'|'xhigh'|'max' (xhigh/max = Opus ; Haiku ne le supporte pas).
  // Omis si absent. Défaut côté Rust = 'max' (analyse la plus profonde).
  ...(effort ? { effort } : {}),
  permissionMode: 'default',
  disallowedTools: DISALLOWED,
  // persistSession:false demandé mais IGNORÉ par le binaire natif 0.3.167 (la session est
  // quand même écrite) → la non-pollution du Studio est assurée par le `op:delete` post-run
  // piloté côté Rust (claude.rs::cleanup_session). On le garde par acquit de conscience.
  persistSession: false,
  // display:'summarized' obligatoire : sinon les blocs thinking remontent vides sur Opus.
  thinking: { type: 'adaptive', display: 'summarized' },
  // 'project' charge CLAUDE.md + .claude/rules/ du workspace de l'app (conventions du projet).
  // Le preset claude_code est requis pour que CLAUDE.md soit injecté. On exclut user/local.
  settingSources: ['project'],
  systemPrompt: { type: 'preset', preset: 'claude_code' },
  canUseTool,
  // Omettre model → le SDK résout le défaut de l'abonnement (Opus).
  ...(model ? { model } : {}),
  ...(cwd ? { cwd } : {}),
  ...(Object.keys(mcpServers).length ? { mcpServers } : {}),
};

// Single-turn : prompt = string simple → la boucle se termine après le message `result`,
// et le process sort. On émet UN événement par bloc sémantique (pas de deltas token-à-token :
// includePartialMessages reste à false) → transcript propre, une ligne lisible par item.

// Corrèle tool_use (nom) → tool_result (tool_use_id) pour détecter les échecs d'AUTH des
// tools MCP en vol (token tourné/invalide après le handshake : le serveur re-vérifie le
// Bearer à chaque requête HTTP). Signalé une seule fois — le premier échec condamne les
// appels suivants.
const toolNames = new Map();
const MCP_AUTH_RE = /\b401\b|unauthorized|invalid[ _-]?token|authentication/i;
let mcpAuthReported = false;

try {
  let sessionEmitted = false;
  for await (const msg of query({ prompt, options })) {
    switch (msg.type) {
      case 'system': {
        // Retry API en boucle sur une auth morte (token OAuth qui ne se refresh plus).
        if (msg.subtype === 'api_retry' && SDK_AUTH_ERRORS.has(msg.error)) reportSdkAuth(`api_retry=${msg.error}`);
        if (!sessionEmitted) {
          emit({ t: 'system', subtype: msg.subtype, session_id: msg.session_id, model: msg.model });
          sessionEmitted = true;
        }
        // Serveur MCP non connecté à l'init (token invalide → 401 dès le handshake, le
        // SDK marque le serveur `failed`) : inutile de dérouler un scan complet qui ne
        // pourra rien signaler — on avorte immédiatement. La session vient d'être émise
        // ci-dessus, donc le cleanup post-run côté Rust reste possible.
        if (msg.subtype === 'init' && Object.keys(mcpServers).length) {
          const dead = (msg.mcp_servers || []).find((s) => s.status === 'failed' || s.status === 'needs-auth');
          if (dead) {
            await failMcp(`MCP auth failed (server '${dead.name}' ${dead.status} at init) — findings not recorded`);
          }
        }
        break;
      }
      case 'assistant':
        for (const b of msg.message?.content || []) {
          if (b.type === 'text' && b.text) emit({ t: 'assistant', text: b.text });
          // Réflexion = compteur SEUL (jamais le texte), calqué sur le chat (runner.js) dont le
          // serveur ne transmet aucun détail de réflexion au navigateur. On n'émet que la longueur
          // (→ tokens ≈ chars/4 côté front) : le texte ne quitte pas le runner = impossible à fuiter.
          else if (b.type === 'thinking' && b.thinking) emit({ t: 'thinking', chars: b.thinking.length });
          else if (b.type === 'tool_use') {
            toolNames.set(b.id, b.name);
            // Le scan n'est pas censé appeler ces 2 dialogues (refusés par canUseTool) ;
            // on ne les remonte pas comme outils pour ne pas polluer la console.
            if (b.name === 'AskUserQuestion' || b.name === 'ExitPlanMode') continue;
            emit({ t: 'tool_use', id: b.id, name: b.name, input: b.input });
          }
        }
        if (msg.error) {
          if (SDK_AUTH_ERRORS.has(msg.error)) reportSdkAuth(`assistant.error=${msg.error}`);
          else emit({ t: 'error', message: `assistant: ${msg.error}` });
        }
        break;
      case 'user':
        for (const b of msg.message?.content || []) {
          if (b.type !== 'tool_result') continue;
          const text = toolResultText(b.content);
          // Tool MCP en échec d'auth (401/unauthorized/invalid token) : l'agent ne peut
          // plus écrire de findings mais le process sortirait quand même en exit 0. On
          // signale l'échec au driver Rust (run FAILED) sans interrompre le tour : le
          // `result` final arrive et le comptage de tokens reste intact.
          const toolName = toolNames.get(b.tool_use_id) || '';
          if (!mcpAuthReported && b.is_error && toolName.startsWith('mcp__') && MCP_AUTH_RE.test(text)) {
            mcpAuthReported = true;
            emit({
              t: 'error',
              code: 'mcp_auth_failed',
              message: `MCP auth failed (${toolName}: ${text.slice(0, 160)}) — findings not recorded`,
            });
          }
          emit({
            t: 'tool_result',
            tool_use_id: b.tool_use_id,
            is_error: !!b.is_error,
            text: text.slice(0, 4000),
          });
        }
        break;
      case 'result':
        // Cause d'auth éventuellement portée par errors[] (texte libre) du result.
        if (Array.isArray(msg.errors) && msg.errors.some((e) => SDK_AUTH_RE.test(String(e)))) {
          reportSdkAuth('result.errors[]');
        }
        // usage porte input_tokens/output_tokens (lus par le driver Rust pour surveillance_runs).
        emit({
          t: 'result',
          subtype: msg.subtype,
          is_error: !!msg.is_error,
          total_cost_usd: msg.total_cost_usd,
          usage: msg.usage,
          num_turns: msg.num_turns,
          duration_ms: msg.duration_ms,
        });
        break;
      default:
        break;
    }
  }
} catch (e) {
  const m = String(e?.message || e);
  if (SDK_AUTH_RE.test(m)) reportSdkAuth(`exception: ${m.slice(0, 160)}`);
  await fail(`query() a échoué : ${m}`, 1);
}

emitDoneAndExit();
