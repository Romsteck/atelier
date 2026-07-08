import { useMemo, useState } from 'react';
import {
  Loader2, Check, FileText, Wrench, Flag, Clock, Brain,
  ChevronRight, Coins, AlertTriangle,
} from 'lucide-react';
import { buildScanSteps } from './scanFormat';
import { charsToTokens, formatTokens, describeScanTool, toolTarget } from '../../lib/toolDisplay';
import { TOOL_ICONS, useSmoothCount } from '../../lib/toolPresentation';

function fmtDur(ms) {
  if (ms == null) return null;
  const s = Math.round(ms / 1000);
  if (s < 60) return `${s}s`;
  return `${Math.floor(s / 60)}m${String(s % 60).padStart(2, '0')}`;
}

function fmtNum(n) {
  if (n == null) return '?';
  if (n >= 1000) return `${(n / 1000).toFixed(n >= 10000 ? 0 : 1)}k`;
  return String(n);
}

// One metric chip (icon + value), shown only when relevant.
function Chip({ Icon, value, title, cls = 'text-gray-400' }) {
  return (
    <span className={`inline-flex items-center gap-0.5 ${cls}`} title={title}>
      <Icon className="w-3 h-3" /> {value}
    </span>
  );
}

// The RUNNING step's live band: the step's aggregated metrics stacked ABOVE the
// current action (thinking → brain, tool → its lucide icon + target), both inside
// ONE blue band swept by a slow sheen (`active-line-sheen`, index.css). Tokens are
// a SINGLE combined counter — the run-wide thinking total (animated) — instead of
// the old duplicated pair (per-step chip + live thinking count).
function ActiveLine({ step, runThinkTokens }) {
  const action = step.activeAction;
  const thinking = action?.kind === 'thinking';
  const tokens = useSmoothCount(runThinkTokens, true);
  const desc = action?.kind === 'tool' ? describeScanTool(action.name, action.input) : null;
  const Icon = desc ? TOOL_ICONS[desc.iconKey] || Wrench : null;
  const target = desc ? toolTarget(desc) : '';
  const dur = fmtDur(step.durationMs);
  const hasChips = step.reads > 0 || step.tools > 0 || step.findings > 0 || runThinkTokens > 0 || !!dur;
  return (
    <div className="active-line-sheen text-[11px] mt-1 px-2 py-1 rounded-md bg-blue-500/10 border border-blue-500/20 text-blue-700 dark:text-blue-200 min-w-0">
      {/* Agrégats de l'étape en cours, AU-DESSUS de l'action — le chip tokens = cumul du run */}
      {hasChips && (
        <div className="flex flex-wrap items-center gap-x-2 gap-y-0.5 mb-0.5 opacity-90">
          {step.reads > 0 && <Chip Icon={FileText} value={step.reads} title="fichiers lus (Read/Grep/Glob)" cls="" />}
          {step.tools > 0 && <Chip Icon={Wrench} value={step.tools} title="actions (autres outils)" cls="" />}
          {step.findings > 0 && (
            <Chip Icon={Flag} value={step.findings} title="findings touchées" cls="text-fuchsia-700 dark:text-fuchsia-300" />
          )}
          {runThinkTokens > 0 && (
            <Chip Icon={Brain} value={formatTokens(tokens)} title="réflexion cumulée du run (≈ caractères / 4)" cls="tabular-nums" />
          )}
          {dur && <Chip Icon={Clock} value={dur} title="durée de l'étape" cls="" />}
        </div>
      )}
      <div className="flex items-center gap-1.5 min-w-0">
        <Loader2 className="w-3 h-3 shrink-0 animate-spin" />
        {thinking ? (
          <span className="inline-flex items-center gap-1 shrink-0">
            <Brain className="w-3 h-3 shrink-0" /> réflexion<span className="opacity-60">…</span>
          </span>
        ) : desc ? (
          <span className="inline-flex items-center gap-1 min-w-0">
            <Icon className="w-3 h-3 shrink-0" />
            <span className="shrink-0">{desc.verb}</span>
            {target && <span className="truncate font-mono opacity-90 min-w-0">{target}</span>}
            <span className="opacity-60 shrink-0">…</span>
          </span>
        ) : (
          <span className="shrink-0">scan travaille…</span>
        )}
      </div>
    </div>
  );
}

// One tool call in a step's expanded detail (icon + verb + target, red on failure).
// Thinking is NEVER shown here — only its aggregated token count lives in the chips.
function ScanToolRow({ tool }) {
  const d = describeScanTool(tool.name, tool.input);
  const Icon = TOOL_ICONS[d.iconKey] || Wrench;
  const target = toolTarget(d);
  return (
    <li className="flex items-center gap-1.5 text-[10px] min-w-0">
      <Icon className={`w-3 h-3 shrink-0 ${tool.isError ? 'text-red-400' : 'text-gray-500'}`} />
      <span className="text-gray-400 shrink-0">{d.verb}</span>
      {d.badge && (
        <span className="shrink-0 text-[9px] uppercase tracking-wider text-gray-400 bg-gray-700/40 px-1 py-0.5 rounded-sm">{d.badge}</span>
      )}
      {target && <span className="truncate text-gray-500 min-w-0 font-mono" title={d.primary}>{target}</span>}
      {tool.isError && <span className="text-red-400 shrink-0 text-[9px]">échec</span>}
    </li>
  );
}

// Step list derived from the live transcript (driven by the agent's
// `scan_progress` MCP signposts). Shown in place of the raw conversation: one
// row per phase with its aggregated metrics + (for the running step) the single
// active action; click a row to reveal its tool calls.
export default function ScanStepsView({ lines }) {
  const { steps, footer, model } = useMemo(() => buildScanSteps(lines), [lines]);
  const [open, setOpen] = useState({}); // explicit per-index override of the default
  // Cumul de réflexion du run entier — LE compteur de tokens unique de la ligne active.
  const runThinkTokens = useMemo(
    () => charsToTokens(steps.reduce((acc, s) => acc + s.thinkingChars, 0)),
    [steps],
  );

  if (!steps.length) {
    return <div className="text-xs text-gray-600 italic p-2">En attente de la sortie du scan…</div>;
  }

  return (
    <div className="p-2 space-y-1">
      {steps.map((s, i) => {
        const running = s.status === 'running';
        const isOpen = open[i] ?? running; // running step expanded by default
        const dur = fmtDur(s.durationMs);
        return (
          <div key={i} className="flex gap-2">
            {/* Left rail: number badge + connector line */}
            <div className="flex flex-col items-center shrink-0">
              <div
                className={`w-5 h-5 rounded-full flex items-center justify-center text-[10px] font-semibold border ${
                  running
                    ? 'border-blue-500/50 text-blue-700 dark:text-blue-300 bg-blue-500/15'
                    : s.error
                      ? 'border-red-500/50 text-red-700 dark:text-red-300 bg-red-500/15'
                      : 'border-emerald-500/50 text-emerald-700 dark:text-emerald-300 bg-emerald-500/15'
                }`}
              >
                {running ? <Loader2 className="w-3 h-3 animate-spin" /> : s.error ? '!' : (s.n || <Check className="w-3 h-3" />)}
              </div>
              {i < steps.length - 1 && <div className="w-px flex-1 bg-gray-700/60 my-0.5" />}
            </div>

            {/* Body */}
            <div className="min-w-0 flex-1 pb-1.5">
              <button
                onClick={() => setOpen((p) => ({ ...p, [i]: !isOpen }))}
                className="w-full flex items-center gap-1.5 text-left group"
              >
                <ChevronRight className={`w-3 h-3 shrink-0 text-gray-500 transition-transform ${isOpen ? 'rotate-90' : ''}`} />
                <span className={`text-xs font-medium truncate ${running ? 'text-blue-700 dark:text-blue-200' : 'text-gray-200'}`}>
                  {s.label}
                </span>
                {s.total ? <span className="text-[10px] text-gray-500 shrink-0">{s.n}/{s.total}</span> : null}
              </button>

              {/* Done steps: aggregated metric chips on their own muted row.
                  The RUNNING step carries them inline in its active band instead. */}
              {!running && (
                <div className="flex flex-wrap items-center gap-x-2.5 gap-y-0.5 text-[10px] mt-0.5 pl-4">
                  {s.reads > 0 && <Chip Icon={FileText} value={s.reads} title="fichiers lus (Read/Grep/Glob)" />}
                  {s.tools > 0 && <Chip Icon={Wrench} value={s.tools} title="actions (autres outils)" />}
                  {s.findings > 0 && (
                    <Chip Icon={Flag} value={s.findings} title="findings touchées" cls="text-fuchsia-700 dark:text-fuchsia-300" />
                  )}
                  {s.thinkingChars > 0 && (
                    <Chip Icon={Brain} value={formatTokens(charsToTokens(s.thinkingChars))} title="réflexion (estimation ≈ caractères / 4)" />
                  )}
                  {dur && <Chip Icon={Clock} value={dur} title="durée de l'étape" />}
                </div>
              )}

              {/* Running step: active action + live aggregates, one glowing line */}
              {running && <ActiveLine step={s} runThinkTokens={runThinkTokens} />}

              {/* Expanded tool calls (never thinking text) */}
              {isOpen && s.toolEvents.length > 0 && (
                <ul className="mt-1 ml-4 pl-2 border-l border-gray-700/60 space-y-0.5 max-h-48 overflow-y-auto">
                  {s.toolEvents.map((t, j) => <ScanToolRow key={t.id || j} tool={t} />)}
                </ul>
              )}
              {isOpen && s.error && (
                <div className="mt-1 ml-4 pl-2 text-[10px] text-red-700 dark:text-red-300 wrap-break-word">⚠ {s.error}</div>
              )}
            </div>
          </div>
        );
      })}

      {/* Run footer: totals from the final result event */}
      {footer && (
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[10px] pt-2 mt-1 border-t border-gray-700/60 pl-2">
          {footer.isError ? (
            <span className="inline-flex items-center gap-1 text-red-700 dark:text-red-300">
              <AlertTriangle className="w-3 h-3" /> Scan terminé en erreur
            </span>
          ) : (
            <span className="inline-flex items-center gap-1 text-emerald-700 dark:text-emerald-300">
              <Check className="w-3 h-3" /> Scan terminé
            </span>
          )}
          <Chip Icon={Coins} value={`${fmtNum(footer.tokensIn)} / ${fmtNum(footer.tokensOut)} tok`} title="tokens entrée / sortie" />
          {footer.durationMs != null && <Chip Icon={Clock} value={fmtDur(footer.durationMs)} title="durée totale" />}
          {model && <span className="text-gray-600 truncate">{model}</span>}
        </div>
      )}
    </div>
  );
}
