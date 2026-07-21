// Atelier Pilote worker — single-turn autonomous editing shim.
// The Rust orchestrator owns checkpoint/build/ship/health/commit/rollback. This
// process may edit only `writeRoot`, never commits, and emits normalized NDJSON.
import { query, deleteSession } from '@anthropic-ai/claude-agent-sdk';
import { createInterface } from 'node:readline';
import { realpathSync, existsSync, lstatSync, readlinkSync } from 'node:fs';
import { dirname, resolve, relative, isAbsolute } from 'node:path';
import {
  makeIo, assertOAuthOnly, buildMcpServers, toolResultText,
  makeSdkAuthReporter, SDK_AUTH_ERRORS, SDK_AUTH_RE,
} from './common.js';

const { emit, diag, fail } = makeIo('pilot-worker');
const reportSdkAuth = makeSdkAuthReporter(emit);

// Toolchain sur PATH (WHY) : identique à runner.js — le worker est spawné via
// `sudo -H -u …` qui réinitialise l'env sur son secure_path, où `~/.cargo/bin` (cargo)
// et `~/.local/bin` sont absents. L'outil Bash du SDK hérite de ce process.env : sans
// ce prepend, tout build/validation Rust de l'agent meurt en « cargo: command not found ».
if (process.env.HOME) {
  process.env.PATH = `${process.env.HOME}/.cargo/bin:${process.env.HOME}/.local/bin:${process.env.PATH || ''}`;
}

// Arrêt sur signal (WHY) : un SIGTERM amont (drain d'atelier.service, `systemctl stop`
// de l'unité détachée, `timeout` du script atelier) tuerait le worker EN SILENCE —
// l'appelant (engine.rs / pilot-atelier-worker.sh) classerait alors l'interruption en
// `agent_error` (retry inutile) au lieu de `cancelled`. On émet un verdict typé puis
// `done`, flush-safe (exit dans le callback du write, comme fail()).
for (const sig of ['SIGTERM', 'SIGINT']) {
  process.on(sig, () => {
    diag(`signal ${sig} reçu → arrêt`);
    emit({ t: 'error', code: 'cancelled', message: `signal ${sig}` });
    process.stdout.write(JSON.stringify({ t: 'done', exit_ok: false }) + '\n', () => process.exit(0));
  });
}

const rl = createInterface({ input: process.stdin });
const initLine = await new Promise((done) => rl.once('line', done));
rl.close();
process.stdin.destroy();

let init;
try { init = JSON.parse(initLine); } catch (e) { await fail(`Init JSON invalide : ${e?.message || e}`); }
const { prompt, cwd, writeRoot, model, effort, mcpEndpoint, mcpToken, oauthToken } = init || {};
if (oauthToken) process.env.CLAUDE_CODE_OAUTH_TOKEN = oauthToken;

// Mode nettoyage one-shot (CONTRAT avec le driver Rust, patron scan.js) : après un
// SIGKILL du groupe (cancel/timeout), le run n'a pas pu exécuter sa suppression inline —
// le driver relance ce worker en `{"op":"delete","sessionId":…,"cwd":…}` pour purger la
// session SDK persistée (persistSession:false est ignoré par le binaire natif). Aucun
// tour d'inférence, et AVANT la garde OAuth : deleteSession est disque-only, sans auth.
if (init?.op === 'delete') {
  if (!init.sessionId) await fail('op delete : champ "sessionId" manquant dans l\'init.');
  try { await deleteSession(init.sessionId, { dir: cwd }); } catch (e) { diag(`deleteSession: ${e?.message || e}`); }
  process.stdout.write(JSON.stringify({ t: 'done' }) + '\n', () => process.exit(0));
  // Sortie exclusive via le callback du write ci-dessus — ne JAMAIS retomber dans le run.
  await new Promise(() => {});
}

await assertOAuthOnly('pilot worker', fail);
if (!prompt || !cwd || !writeRoot) await fail('prompt/cwd/writeRoot requis.');

const root = canonicalExisting(writeRoot);
if (!root) await fail(`writeRoot introuvable : ${writeRoot}`);
const mcpServers = buildMcpServers(mcpEndpoint, mcpToken, diag, 'MCP Pilote non câblé.');

const DISALLOWED = [
  'AskUserQuestion', 'ExitPlanMode', 'Task', 'TaskCreate', 'TaskUpdate', 'TaskList', 'TaskGet',
  'WebFetch', 'WebSearch', 'Skill', 'SlashCommand', 'KillShell', 'KillBash', 'TodoWrite',
];
const FILE_TOOLS = new Set(['Write', 'Edit', 'MultiEdit', 'NotebookEdit']);
const BASH_DENY = [
  /(^|[;&|\s])sudo([;&|\s]|$)/i,
  /\bsystemctl\b/i,
  /\b(service|shutdown|reboot|poweroff|mount|umount)\b/i,
  /\bgit\s+(commit|push|reset|rebase|checkout|switch|clean)\b/i,
  /\bmake\s+(deploy|deploy-local)\b/i,
  /\brm\s+(-[^\n]*r|--recursive)\b/i,
  /(^|[;&|\s])(cd|pushd|popd)([;&|\s]|$)/i,
  /\$(?:\{|HOME\b|OLDPWD\b|TMPDIR\b)/,
  /(?:^|\s)(?:>|>>|2>|&>)\s*\/(?!dev\/null\b)/,
];

function canonicalExisting(p) {
  try { return realpathSync(p); } catch { return null; }
}

// Canonicalise the nearest existing parent too, so a new file below a symlink
// cannot escape the workspace merely because the leaf does not exist yet.
function canonicalTarget(raw, depth = 0) {
  if (typeof raw !== 'string' || !raw.trim()) return null;
  if (depth > 8) return null; // chaîne/boucle de symlinks : irrésoluble → refus
  const abs = resolve(cwd, raw);
  let probe = abs;
  const suffix = [];
  while (!existsSync(probe)) {
    const parent = dirname(probe);
    if (parent === probe) return null;
    suffix.unshift(relative(parent, probe));
    probe = parent;
  }
  let base;
  try { base = realpathSync(probe); } catch { return null; }
  const target = resolve(base, ...suffix);
  // Symlink pendouillant (WHY) : existsSync() SUIT les liens — un leaf symlink vers une
  // cible inexistante passe pour « absent », le parent se canonicalise proprement… mais
  // l'écriture réelle suivrait le lien HORS racine. lstat() voit le lien lui-même : on
  // résout SA cible (relative au dossier du lien) et on re-valide récursivement contre
  // la racine ; irrésoluble (readlink en échec, boucle) → null → deny côté appelant.
  let leaf = null;
  try { leaf = lstatSync(target); } catch { /* rien sur disque : vraie création, ok */ }
  if (leaf?.isSymbolicLink()) {
    let dest;
    try { dest = readlinkSync(target); } catch { return null; }
    return canonicalTarget(resolve(dirname(target), dest), depth + 1);
  }
  return target;
}

function insideRoot(target) {
  const rel = relative(root, target);
  return rel === '' || (!rel.startsWith('..') && !isAbsolute(rel));
}

async function canUseTool(toolName, input) {
  if (FILE_TOOLS.has(toolName)) {
    const raw = input?.file_path || input?.notebook_path;
    const target = canonicalTarget(raw);
    const rel = target ? relative(root, target) : '';
    if (!target || !insideRoot(target) || rel === '.git' || rel.startsWith(`.git${process.platform === 'win32' ? '\\' : '/'}`)) {
      return { behavior: 'deny', message: `Pilote : ${toolName} refusé hors workspace (${raw || '?'})` };
    }
    return { behavior: 'allow', updatedInput: input };
  }
  if (toolName === 'Bash') {
    const command = String(input?.command || '');
    const denied = BASH_DENY.find((re) => re.test(command));
    const traversesParent = /(^|[\s'"=])\.\.(?:\/|[\s'";&|]|$)/.test(command) || /(^|[\s'"=])~(?:\/|[\s'";&|]|$)/.test(command);
    // Les URLs ne sont PAS des chemins filesystem (WHY) : `curl http://127.0.0.1:4100/…`
    // — le chemin de test canonique de testing.md (path-proxy) — matcherait le scan de
    // chemins absolus via `//127.0.0.1:4100/...` et serait refusé à tort. On retire les
    // tokens URL de la chaîne ANALYSÉE seulement ; la commande exécutée reste intacte et
    // le reste de la policy (denylist, redirections, traversées ..) s'applique toujours
    // à la commande complète.
    const scannable = command.replace(/https?:\/\/\S+/gi, ' ');
    // Absolute filesystem arguments are allowed only when their canonical target
    // remains in the workspace (plus /dev/null). This catches sed/cp/find paths,
    // not merely shell redirections.
    const absolutePaths = scannable.match(/\/(?:[^\s'";&|<>])+/g) || [];
    const escapesRoot = absolutePaths.some((raw) => raw !== '/dev/null' && !insideRoot(canonicalTarget(raw) || resolve(raw)));
    if (denied || traversesParent || escapesRoot) {
      return { behavior: 'deny', message: 'Pilote : commande refusée. Les commits, deploys, services, privilèges et suppressions récursives appartiennent à l’orchestrateur.' };
    }
    return { behavior: 'allow', updatedInput: input };
  }
  if (typeof toolName === 'string' && toolName.startsWith('mcp__')) {
    return { behavior: 'allow', updatedInput: input };
  }
  return { behavior: 'allow', updatedInput: input };
}

const options = {
  ...(effort ? { effort } : {}),
  ...(model ? { model } : {}),
  cwd,
  permissionMode: 'acceptEdits',
  disallowedTools: DISALLOWED,
  settingSources: ['project'],
  systemPrompt: { type: 'preset', preset: 'claude_code' },
  thinking: { type: 'adaptive', display: 'summarized' },
  persistSession: false,
  canUseTool,
  ...(Object.keys(mcpServers).length ? { mcpServers } : {}),
};

let sessionId = null;
let lastAssistant = '';
let fatal = false;
const toolNames = new Map();
try {
  for await (const msg of query({ prompt, options })) {
    switch (msg.type) {
      case 'system':
        if (msg.subtype === 'api_retry' && SDK_AUTH_ERRORS.has(msg.error)) reportSdkAuth(`api_retry=${msg.error}`);
        if (!sessionId && msg.session_id) {
          sessionId = msg.session_id;
          emit({ t: 'system', subtype: msg.subtype, session_id: msg.session_id, model: msg.model });
        }
        if (msg.subtype === 'init' && Object.keys(mcpServers).length) {
          const dead = (msg.mcp_servers || []).find((s) => s.status === 'failed' || s.status === 'needs-auth');
          if (dead) { fatal = true; emit({ t: 'error', code: 'mcp_auth_failed', message: `MCP ${dead.name}: ${dead.status}` }); }
        }
        break;
      case 'assistant': {
        const texts = [];
        for (const b of msg.message?.content || []) {
          if (b.type === 'text' && b.text) { texts.push(b.text); emit({ t: 'assistant', text: b.text }); }
          else if (b.type === 'thinking' && b.thinking) emit({ t: 'thinking', chars: b.thinking.length });
          else if (b.type === 'tool_use') { toolNames.set(b.id, b.name); emit({ t: 'tool_use', id: b.id, name: b.name, input: b.input }); }
        }
        if (texts.length) lastAssistant = texts.join('\n');
        if (msg.error) {
          // Auth → reporter once-only (patron makeSdkAuthReporter) : un token mort se
          // manifeste souvent sur PLUSIEURS canaux du même run (api_retry + assistant +
          // result) — une seule émission `sdk_auth_failed` suffit au driver Rust.
          if (SDK_AUTH_ERRORS.has(msg.error)) reportSdkAuth(`assistant.error=${msg.error}`);
          else emit({ t: 'error', code: 'agent_error', message: String(msg.error) });
          fatal = true;
        }
        break;
      }
      case 'user':
        for (const b of msg.message?.content || []) {
          if (b.type !== 'tool_result') continue;
          emit({ t: 'tool_result', tool_use_id: b.tool_use_id, name: toolNames.get(b.tool_use_id), is_error: !!b.is_error, text: toolResultText(b.content).slice(0, 4000) });
        }
        break;
      case 'result':
        if (Array.isArray(msg.errors) && msg.errors.some((e) => SDK_AUTH_RE.test(String(e)))) {
          reportSdkAuth(`result.errors=${msg.errors.join('; ').slice(0, 160)}`);
          fatal = true;
        }
        emit({ t: 'result', subtype: msg.subtype, is_error: !!msg.is_error, usage: msg.usage, num_turns: msg.num_turns });
        if (msg.is_error) fatal = true;
        break;
      default:
        break;
    }
  }
} catch (e) {
  const message = String(e?.message || e);
  if (SDK_AUTH_RE.test(message)) reportSdkAuth(`exception: ${message.slice(0, 160)}`);
  else emit({ t: 'error', code: 'agent_error', message: message.slice(0, 1000) });
  fatal = true;
}

if (lastAssistant) emit({ t: 'final_report', text: lastAssistant });
if (sessionId) {
  try { await deleteSession(sessionId, { dir: cwd }); } catch (e) { diag(`deleteSession: ${e?.message || e}`); }
}
process.stdout.write(JSON.stringify({ t: 'done', exit_ok: !fatal && !!lastAssistant }) + '\n', () => process.exit(fatal || !lastAssistant ? 2 : 0));
