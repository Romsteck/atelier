import { useMemo, useState } from 'react';
import {
  Loader2, Check, FileText, Wrench, Flag, Clock,
  ChevronRight, Coins, AlertTriangle,
} from 'lucide-react';
import { buildScanSteps, formatScanEvent, TONE_CLS } from './scanFormat';

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

// Step list derived from the live transcript (driven by the agent's
// `scan_progress` MCP signposts). Shown in place of the raw conversation: one
// row per phase with its metrics; click a row to reveal its raw sub-events.
export default function ScanStepsView({ lines }) {
  const { steps, footer, model } = useMemo(() => buildScanSteps(lines), [lines]);
  const [open, setOpen] = useState({}); // explicit per-index override of the default

  if (!steps.length) {
    return <div className="text-xs text-gray-600 italic p-2">En attente de la sortie du scan…</div>;
  }

  return (
    <div className="p-2 space-y-1">
      {steps.map((s, i) => {
        const running = s.status === 'running';
        const isOpen = open[i] ?? running; // running step expanded by default
        const dur = fmtDur(s.durationMs);
        const detail = isOpen
          ? s.entries.map((raw) => formatScanEvent(raw)).filter((e) => e && (e.text?.trim() || e.tone === 'meta'))
          : [];
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

              {/* Metric chips */}
              <div className="flex flex-wrap items-center gap-x-2.5 gap-y-0.5 text-[10px] mt-0.5 pl-4">
                {s.reads > 0 && <Chip Icon={FileText} value={s.reads} title="fichiers lus (Read/Grep/Glob)" />}
                {s.tools > 0 && <Chip Icon={Wrench} value={s.tools} title="autres outils" />}
                {s.findings > 0 && (
                  <Chip Icon={Flag} value={s.findings} title="findings touchées" cls="text-fuchsia-700 dark:text-fuchsia-300" />
                )}
                {dur && <Chip Icon={Clock} value={dur} title="durée de l'étape" />}
                {s.reads + s.tools + s.findings === 0 && !dur && running && (
                  <span className="text-gray-600 italic">en cours…</span>
                )}
              </div>

              {/* Running subtitle: the latest thinking/message line */}
              {running && s.lastText && !isOpen && (
                <div className="text-[10px] text-gray-500 truncate mt-0.5 pl-4 italic">{s.lastText}</div>
              )}

              {/* Expanded raw sub-events */}
              {isOpen && detail.length > 0 && (
                <div className="mt-1 ml-4 pl-2 border-l border-gray-700/60 space-y-1 max-h-48 overflow-y-auto">
                  {detail.map((e, j) => (
                    <div key={j} className="flex gap-1.5 text-[10px] leading-relaxed font-mono">
                      {e.icon && <span className="shrink-0 select-none">{e.icon}</span>}
                      <span className={`whitespace-pre-wrap wrap-break-word min-w-0 ${TONE_CLS[e.tone] || 'text-gray-300'}`}>{e.text}</span>
                    </div>
                  ))}
                </div>
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
