import { useEffect, useMemo, useRef } from 'react';
import { Bot, Brain, PenLine, Wrench, Square } from 'lucide-react';
import Button from '../Button';
import { charsToTokens, formatTokens } from '../../lib/toolDisplay';
import { TONE_CLS } from '../surveillance/scanFormat';

// Libellé lisible du moteur, dérivé du modèle réel (event `system`) puis, à
// défaut (avant son arrivée), de l'engine du run.
export function engineLabel(model, hint) {
  const m = (model || '').toLowerCase();
  if (m.includes('opus') || m.includes('claude')) return 'Claude Opus 4.8';
  if (m.includes('gpt-5.6') || m.includes('sol') || m.includes('codex')) return 'Codex · GPT-5.6 Sol';
  if (model) return model;
  if (hint === 'codex') return 'Codex · GPT-5.6 Sol';
  if (hint === 'claude') return 'Claude Opus 4.8';
  return 'Agent';
}

// Agrège les métriques live d'un run à partir de ses lignes de transcript
// (NDJSON). Réutilisé par la carte (compact) et le drawer (détaillé).
export function pilotRunMetrics(lines) {
  let thinkingChars = 0, inputTokens = 0, outputTokens = 0, actions = 0, model = null;
  for (const l of lines || []) {
    let ev;
    try { ev = JSON.parse(l.line); } catch { continue; }
    switch (ev.t) {
      case 'system': if (ev.model) model = ev.model; break;
      case 'thinking': thinkingChars += ev.chars || 0; break;
      case 'tool_use': actions += 1; break;
      case 'result': {
        const u = ev.usage || {};
        inputTokens += u.input_tokens || 0;
        outputTokens += u.output_tokens || 0;
        break;
      }
      default: break;
    }
  }
  return { thinkingTokens: charsToTokens(thinkingChars), inputTokens, outputTokens, actions, model };
}

// Une ligne de conversation lisible (icône + texte + tonalité), ou null pour
// les events de pur bruit. Parse la ligne brute NDJSON (`l.line`).
function formatPilotEvent(raw) {
  let ev;
  try { ev = JSON.parse(raw); } catch { return raw.trim() ? { icon: '', text: raw, tone: 'raw' } : null; }
  switch (ev.t) {
    case 'system': return { icon: '▸', text: `Agent démarré${ev.model ? ` · ${engineLabel(ev.model)}` : ''}`, tone: 'meta' };
    case 'assistant': return ev.text?.trim() ? { icon: '🗨', text: ev.text, tone: 'msg' } : null;
    case 'thinking': { const c = ev.chars || 0; return c ? { icon: '🧠', text: `réflexion · ${formatTokens(charsToTokens(c))} tokens`, tone: 'dim' } : null; }
    case 'tool_use': return { icon: '🔧', text: pilotToolLabel(ev), tone: 'tool' };
    case 'tool_result': return ev.is_error ? { icon: '⚠', text: (ev.text || '').slice(0, 200), tone: 'err' } : null;
    case 'result': { const u = ev.usage || {}; return { icon: '✓', text: `Tour terminé — ${u.input_tokens ?? '?'} in / ${u.output_tokens ?? '?'} out`, tone: 'meta' }; }
    case 'final_report': return { icon: '📝', text: 'Rapport final rédigé', tone: 'meta' };
    case 'error': return { icon: '⚠', text: ev.message || ev.code || 'erreur', tone: 'err' };
    default: return null;
  }
}

function pilotToolLabel(ev) {
  const name = ev.name || 'outil';
  const inp = ev.input || {};
  const bare = name.startsWith('mcp__') ? name.split('__').slice(2).join('__') : name;
  const short = (p) => (p ? String(p).split('/').slice(-1)[0] : '');
  switch (name) {
    case 'Read': return `Read ${short(inp.file_path)}`;
    case 'Write': return `Write ${short(inp.file_path)}`;
    case 'Edit': case 'MultiEdit': return `Edit ${short(inp.file_path)}`;
    case 'Grep': return `Grep ${inp.pattern || ''}`;
    case 'Glob': return `Glob ${inp.pattern || ''}`;
    case 'Bash': return `$ ${(inp.command || '').slice(0, 70)}`;
    default: break;
  }
  if (bare === 'ship') return 'ship (livraison)';
  if (bare === 'app.build' || bare === 'app_build') return 'app.build';
  if (bare.startsWith('backlog_')) return `${bare} #${inp.id ?? ''}`;
  if (bare.startsWith('findings_')) return `${bare} #${inp.id ?? ''}`;
  return bare;
}

function Metric({ icon: Icon, value, label }) {
  return (
    <span className="inline-flex items-center gap-1 text-xs" title={label}>
      <Icon className="w-3.5 h-3.5 text-blue-600 dark:text-blue-400" />
      <span className="tabular-nums font-medium text-gray-200">{value}</span>
      <span className="text-[10px] text-gray-500">{label}</span>
    </span>
  );
}

// Panneau live d'un run Pilote : moteur + compteurs (réflexion / génération /
// actions, incrémentés en direct) + conversation qui défile. Présentation
// seule : le parent fournit `lines` (ring hydraté + flux WS mergés).
export default function PilotRunLive({ lines, engineHint, onStop, stopping }) {
  const bodyRef = useRef(null);
  const m = useMemo(() => pilotRunMetrics(lines), [lines]);
  const entries = useMemo(
    () => (lines || []).map((l) => formatPilotEvent(l.line)).filter((e) => e && (e.text?.trim() || e.tone === 'meta')),
    [lines],
  );
  useEffect(() => {
    const el = bodyRef.current;
    if (el && el.scrollHeight - el.scrollTop - el.clientHeight < 80) el.scrollTop = el.scrollHeight;
  }, [entries.length]);

  return (
    <section className="rounded-lg border border-blue-500/35 bg-blue-500/5 overflow-hidden">
      <div className="px-3 py-2 border-b border-blue-500/25 flex items-center gap-3 flex-wrap">
        <span className="inline-flex items-center gap-1.5 text-xs font-medium text-blue-800 dark:text-blue-200">
          <Bot className="w-4 h-4" />{engineLabel(m.model, engineHint)}
        </span>
        <span className="flex items-center gap-3 ml-auto">
          <Metric icon={Brain} value={formatTokens(m.thinkingTokens)} label="réflexion" />
          <Metric icon={PenLine} value={formatTokens(m.outputTokens)} label="générés" />
          <Metric icon={Wrench} value={m.actions} label="actions" />
        </span>
        {onStop && <Button size="xs" variant="danger" icon={Square} loading={stopping} onClick={onStop}>Stopper</Button>}
      </div>
      <div ref={bodyRef} className="max-h-72 overflow-y-auto p-2 space-y-1 text-[11px] font-mono leading-relaxed">
        {entries.length === 0
          ? <div className="text-gray-500 px-1 py-2">En attente des premières lignes de l’agent…</div>
          : entries.map((e, i) => (
            <div key={i} className={`flex gap-1.5 ${TONE_CLS[e.tone] || 'text-gray-400'}`}>
              <span className="shrink-0 select-none">{e.icon}</span>
              <span className="whitespace-pre-wrap break-words min-w-0">{e.text}</span>
            </div>
          ))}
      </div>
    </section>
  );
}
