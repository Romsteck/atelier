// Shared scan-transcript helpers used by the per-app Surveillance tab AND the
// global sweep live view. The scan engine is the Claude Agent SDK (`scan.js`),
// which emits NDJSON events carrying a `t` field.

// Merge transcript lines deduped by seq (a buffer replay + live WS lines can
// overlap) and kept ordered. Capped to the last 2000.
export function mergeLines(prev, incoming) {
  const bySeq = new Map(prev.map((l) => [l.seq, l]));
  for (const l of incoming) bySeq.set(l.seq, l);
  return [...bySeq.values()].sort((a, b) => a.seq - b.seq).slice(-2000);
}

// Strip the `mcp__server__` prefix off a tool name to get the bare tool id.
export function bareName(name) {
  return name && name.startsWith('mcp__') ? name.split('__').slice(2).join('__') : name;
}

// Readable one-liner for a scan-agent tool call (Claude `tool_use` event).
export function scanToolLabel(ev) {
  const name = ev.name || 'outil';
  const inp = ev.input || {};
  const bare = bareName(name);
  switch (bare) {
    case 'scan_progress': return `→ étape ${inp.step ?? ''}${inp.total ? `/${inp.total}` : ''} : ${inp.label || ''}`;
    case 'findings_upsert': return `finding: [${inp.severity || '?'}] ${inp.title || ''}`;
    case 'findings_dismiss': return `findings_dismiss #${inp.id ?? ''}`;
    case 'findings_resolve': return `findings_resolve #${inp.id ?? ''}`;
    case 'findings_delete': return `findings_delete #${inp.id ?? ''}`;
    case 'findings_list': return `findings_list${inp.kind ? ` (${inp.kind})` : ''}`;
    case 'pm_query': return 'pm_query';
    default: break;
  }
  if (name === 'Read') return `Read ${inp.file_path || ''}`;
  if (name === 'Grep') return `Grep ${inp.pattern || ''}`;
  if (name === 'Glob') return `Glob ${inp.pattern || ''}`;
  return bare;
}

// Render one scan-agent NDJSON event into a readable {icon, text, tone} entry,
// or null to skip pure-noise events. Falls back to the raw line if not JSON.
export function formatScanEvent(raw) {
  let ev;
  try { ev = JSON.parse(raw); } catch { return raw.trim() ? { icon: '', text: raw, tone: 'raw' } : null; }
  if (!ev.t) return null;
  switch (ev.t) {
    case 'system': return { icon: '▸', text: `Session de scan démarrée${ev.model ? ` (${ev.model})` : ''}`, tone: 'meta' };
    case 'assistant': return ev.text?.trim() ? { icon: '🗨', text: ev.text, tone: 'msg' } : null;
    case 'thinking': return ev.text?.trim() ? { icon: '💭', text: ev.text, tone: 'dim' } : null;
    case 'tool_use': return { icon: '🔧', text: scanToolLabel(ev), tone: 'tool' };
    // Successful tool results are implied by the next message; surface errors only.
    case 'tool_result': return ev.is_error ? { icon: '⚠', text: (ev.text || '').slice(0, 200), tone: 'err' } : null;
    case 'result': {
      const u = ev.usage || {};
      return { icon: '✓', text: `Scan terminé — ${u.input_tokens ?? '?'} in / ${u.output_tokens ?? '?'} out tokens`, tone: 'meta' };
    }
    case 'error': return { icon: '⚠', text: ev.message || 'erreur', tone: 'err' };
    default: return null; // done, etc.
  }
}

// Tone → text colour. Non-gray accent colours do NOT auto-reverse with the
// theme (only the gray scale does), so they need explicit dark: pairs to stay
// legible on the light surface in light mode.
export const TONE_CLS = {
  msg: 'text-gray-100',
  meta: 'text-emerald-700 dark:text-emerald-400/80',
  dim: 'text-gray-500',
  cmd: 'text-sky-700 dark:text-sky-300',
  tool: 'text-fuchsia-700 dark:text-fuchsia-300',
  err: 'text-red-700 dark:text-red-300',
  raw: 'text-gray-400',
};

const FINDING_TOOLS = new Set(['findings_upsert', 'findings_delete', 'findings_resolve', 'findings_dismiss']);
const READ_TOOLS = new Set(['Read', 'Grep', 'Glob']);

// Parse a scan transcript into an ordered list of STEPS for the live step view.
// A `scan_progress` tool call (emitted by the agent via the MCP signpost tool)
// opens a new step; every event until the next marker is attributed to it.
// Activity before the first marker becomes a synthetic "Initialisation" step.
// Per-step metrics (reads/tools/findings/duration) are derived from the events;
// total tokens/cost/duration come from the final `result` event. Degrades
// gracefully: with NO markers, everything lands in one "Initialisation" step.
// Each step keeps its raw line strings in `entries` so the detail view can
// re-render them through `formatScanEvent` on demand.
export function buildScanSteps(lines) {
  const steps = [];
  let cur = null;
  let result = null;
  let model = null;

  const open = (n, total, label, ts) => {
    cur = {
      n, total, label, status: 'running',
      reads: 0, tools: 0, findings: 0,
      startTs: ts || null, endTs: ts || null, lastText: '', error: null, entries: [],
    };
    steps.push(cur);
  };
  const ensure = (ts) => { if (!cur) open(0, null, 'Initialisation', ts); };

  for (const l of lines || []) {
    let ev;
    try { ev = JSON.parse(l.line); } catch { continue; }
    if (!ev || !ev.t) continue;
    const ts = l.ts || null;

    if (ev.t === 'system') { model = ev.model || model; ensure(ts); continue; }
    if (ev.t === 'result') { result = ev; if (cur && ts) cur.endTs = ts; continue; }
    if (ev.t === 'tool_use' && bareName(ev.name) === 'scan_progress') {
      const inp = ev.input || {};
      if (cur && ts) cur.endTs = ts;
      open(
        Number(inp.step) || steps.length + 1,
        inp.total != null ? Number(inp.total) : null,
        String(inp.label || `Étape ${inp.step ?? ''}`).slice(0, 80),
        ts,
      );
      continue;
    }

    ensure(ts);
    if (ts) { if (!cur.startTs) cur.startTs = ts; cur.endTs = ts; }
    switch (ev.t) {
      case 'tool_use': {
        const b = bareName(ev.name);
        if (FINDING_TOOLS.has(b)) cur.findings++;
        else if (READ_TOOLS.has(ev.name)) cur.reads++;
        else cur.tools++;
        cur.entries.push(l.line);
        break;
      }
      case 'assistant':
      case 'thinking':
        if (ev.text?.trim()) { cur.lastText = ev.text.trim(); cur.entries.push(l.line); }
        break;
      case 'tool_result':
        if (ev.is_error) cur.entries.push(l.line);
        break;
      case 'error':
        cur.error = ev.message || 'erreur';
        cur.entries.push(l.line);
        break;
      default:
        break;
    }
  }

  // Drop a leading empty "Initialisation" (agent called scan_progress first thing).
  if (steps.length > 1 && steps[0].n === 0 && steps[0].entries.length === 0) steps.shift();

  // With a final result the run finished → every step done; otherwise the last
  // step is the one in progress.
  const finished = !!result;
  steps.forEach((s, i) => {
    s.status = finished || i < steps.length - 1 ? 'done' : 'running';
    if (s.startTs && s.endTs) s.durationMs = s.endTs - s.startTs;
  });

  const usage = result?.usage || null;
  const footer = result
    ? {
        tokensIn: usage?.input_tokens ?? null,
        tokensOut: usage?.output_tokens ?? null,
        durationMs: result.duration_ms ?? null,
        costUsd: result.total_cost_usd ?? null,
        isError: !!result.is_error,
      }
    : null;

  return { steps, footer, model };
}
