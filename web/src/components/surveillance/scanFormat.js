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

// Readable one-liner for a scan-agent tool call (Claude `tool_use` event).
export function scanToolLabel(ev) {
  const name = ev.name || 'outil';
  const inp = ev.input || {};
  const bare = name.startsWith('mcp__') ? name.split('__').slice(2).join('__') : name;
  switch (bare) {
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
