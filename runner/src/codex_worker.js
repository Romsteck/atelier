// Atelier Pilote Codex worker — one autonomous turn, workspace-write sandbox,
// network disabled, normalized NDJSON contract shared with worker.js.
import { createInterface } from 'node:readline';
import { existsSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { makeIo } from './common.js';

const { emit, diag, fail } = makeIo('pilot-codex-worker');

// Toolchain sur PATH (WHY) : identique à codex.js — spawné via `sudo -H -u …` qui
// réinitialise l'env sur son secure_path, où `~/.cargo/bin` et `~/.local/bin` sont
// absents. Le CLI codex hérite de ce process.env pour exécuter les commandes du
// sandbox → sans ce prepend, les builds Rust meurent en « cargo: command not found ».
if (process.env.HOME) {
  process.env.PATH = `${process.env.HOME}/.cargo/bin:${process.env.HOME}/.local/bin:${process.env.PATH || ''}`;
}

// Efforts CONNUS du CLI (patron codex.js) : un effort hors-set serait transmis tel quel
// au binaire, qui retombe silencieusement sur un défaut — on clampe explicitement
// (`max` = alias historique côté Claude → `xhigh`, inconnu → `xhigh`, le défaut worker).
const EFFORTS = new Set(['minimal', 'low', 'medium', 'high', 'xhigh']);
function clampEffort(e) {
  const v = typeof e === 'string' ? e.toLowerCase() : '';
  if (v === 'max') return 'xhigh';
  return EFFORTS.has(v) ? v : 'xhigh';
}

const line = await new Promise((resolve) => createInterface({ input: process.stdin }).once('line', resolve));
let init;
try { init = JSON.parse(line); } catch (e) { await fail(`Init JSON invalide: ${e?.message || e}`); }
const { prompt, cwd, model = 'gpt-5.6-sol', effort = 'xhigh' } = init || {};
if (!prompt || !cwd) await fail('prompt/cwd requis');

const CODEX_HOME = process.env.CODEX_HOME || join(homedir(), '.codex');
process.env.CODEX_HOME = CODEX_HOME;
const authKeys = ['CODEX_API_KEY', 'OPENAI_API_KEY', 'CODEX_ACCESS_TOKEN', 'CODEX_AUTH'];
for (const key of authKeys) if (process.env[key]) await fail(`${key} interdit: abonnement ChatGPT uniquement`);
if (!existsSync(join(CODEX_HOME, 'auth.json'))) await fail('auth.json Codex absent');

const childEnv = {};
for (const [key, value] of Object.entries(process.env)) {
  if (value !== undefined && !authKeys.includes(key)) childEnv[key] = value;
}
childEnv.CODEX_HOME = CODEX_HOME;
const { Codex } = await import('@openai/codex-sdk');
const codex = new Codex({ env: childEnv });
const thread = codex.startThread({
  model,
  modelReasoningEffort: clampEffort(effort),
  workingDirectory: cwd,
  skipGitRepoCheck: true,
  approvalPolicy: 'never',
  sandboxMode: 'workspace-write',
  networkAccessEnabled: false,
});

const authRe = /\b401\b|unauthorized|refresh token|not logged in|login required|invalid_grant|auth\.json/i;
let sessionId = null;
let lastAssistant = '';
let usage = null;
let fatal = false;
const emitted = new Set();
function tool(id, name, input) {
  if (emitted.has(id)) return;
  emitted.add(id); emit({ t: 'tool_use', id, name, input });
}

try {
  const { events } = await thread.runStreamed(prompt);
  for await (const ev of events) {
    if (ev.type === 'thread.started') {
      sessionId = ev.thread_id;
      emit({ t: 'system', subtype: 'init', session_id: sessionId, model });
    } else if (['item.started', 'item.updated', 'item.completed'].includes(ev.type) && ev.item) {
      const item = ev.item;
      if (item.type === 'agent_message' && ev.type === 'item.completed') {
        lastAssistant = item.text || lastAssistant;
        if (item.text) emit({ t: 'assistant', text: item.text });
      } else if (item.type === 'reasoning' && ev.type === 'item.completed' && item.text) {
        emit({ t: 'thinking', chars: item.text.length });
      } else if (item.type === 'command_execution') {
        tool(item.id, 'Bash', { command: String(item.command || '').slice(0, 400) });
        if (ev.type === 'item.completed') emit({ t: 'tool_result', tool_use_id: item.id, is_error: item.status === 'failed' || item.exit_code > 0, text: String(item.aggregated_output || '').slice(0, 4000) });
      } else if (item.type === 'file_change' && ev.type === 'item.completed') {
        for (const [index, change] of (item.changes || []).entries()) {
          const id = `${item.id}#${index}`;
          tool(id, change.kind === 'add' ? 'Write' : 'Edit', { file_path: change.path });
          emit({ t: 'tool_result', tool_use_id: id, is_error: item.status === 'failed', text: change.kind || 'update' });
        }
      }
    } else if (ev.type === 'turn.completed') {
      usage = ev.usage || null;
    } else if (ev.type === 'turn.failed') {
      const message = ev.error?.message || 'turn.failed';
      emit({ t: 'error', code: authRe.test(message) ? 'sdk_auth_failed' : 'agent_error', message: String(message).slice(0, 1000) });
      fatal = true;
    } else if (ev.type === 'error') {
      const message = String(ev.message || '');
      if (authRe.test(message)) {
        emit({ t: 'error', code: 'sdk_auth_failed', message: message.slice(0, 1000) });
        fatal = true;
      } else {
        // Jamais avalé en silence : sans trace, un run qui échoue sur un event `error`
        // non-auth du CLI serait indiagnosticable (stderr = seul canal de diagnostic).
        diag(`codex error event: ${message.slice(0, 300)}`);
      }
    }
  }
} catch (e) {
  const message = String(e?.message || e);
  emit({ t: 'error', code: authRe.test(message) ? 'sdk_auth_failed' : 'agent_error', message: message.slice(0, 1000) });
  fatal = true;
}

if (lastAssistant) emit({ t: 'final_report', text: lastAssistant });
emit({ t: 'result', subtype: fatal ? 'error_during_execution' : 'success', is_error: fatal, usage: {
  input_tokens: usage?.input_tokens || 0,
  output_tokens: usage?.output_tokens || 0,
} });
process.stdout.write(`${JSON.stringify({ t: 'done', exit_ok: !fatal && !!lastAssistant, session_id: sessionId })}\n`, () => process.exit(fatal || !lastAssistant ? 2 : 0));
