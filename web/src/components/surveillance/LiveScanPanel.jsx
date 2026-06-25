import { useEffect, useMemo, useRef } from 'react';
import { RefreshCw, Square, Terminal } from 'lucide-react';
import { formatScanEvent, TONE_CLS } from './scanFormat';

// Live console of a scan run in progress. Lines stream in over WebSocket; the
// panel auto-scrolls. Shared by the per-app Surveillance tab (right-rail drawer,
// the default `className`) and the global sweep view (a grid cell — pass a
// `className` like `border bg-gray-950/60 rounded-sm`).
export default function LiveScanPanel({
  lines,
  kindLabel,
  onStop,
  stopping,
  className = 'w-96 shrink-0 border-l border-gray-700 bg-gray-950/60',
  title,
}) {
  const bodyRef = useRef(null);
  const entries = useMemo(
    () => lines.map((l) => formatScanEvent(l.line)).filter((e) => e && (e.text?.trim() || e.tone === 'meta')),
    [lines],
  );
  useEffect(() => {
    const el = bodyRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [entries.length]);
  return (
    <div className={`flex flex-col min-w-0 min-h-0 ${className}`}>
      <div className="px-3 py-2 border-b border-gray-700 flex items-center gap-2">
        <Terminal className="w-3.5 h-3.5 text-emerald-700 dark:text-emerald-300 shrink-0" />
        <span className="text-xs text-gray-300 flex-1 truncate">
          {title
            ? title
            : stopping
              ? 'Arrêt en cours…'
              : <>Scan en cours — <span className="text-emerald-700 dark:text-emerald-300">{kindLabel}</span></>}
        </span>
        <RefreshCw className={`w-3 h-3 shrink-0 animate-spin ${stopping ? 'text-red-700 dark:text-red-400' : 'text-emerald-700 dark:text-emerald-400'}`} />
        {onStop && (
          <button
            onClick={onStop}
            disabled={stopping}
            className="px-2 py-0.5 text-xs border border-red-500/40 text-red-700 dark:text-red-200 hover:bg-red-500/20 rounded-sm flex items-center gap-1 disabled:opacity-50"
          >
            <Square className="w-3 h-3" /> Arrêter
          </button>
        )}
      </div>
      <div ref={bodyRef} className="flex-1 overflow-y-auto p-2 space-y-1.5">
        {entries.length === 0 ? (
          <div className="text-xs text-gray-600 italic">En attente de la sortie du scan…</div>
        ) : (
          entries.map((e, i) => (
            <div key={i} className="flex gap-1.5 text-[11px] leading-relaxed font-mono">
              {e.icon && <span className="shrink-0 select-none">{e.icon}</span>}
              <span className={`whitespace-pre-wrap wrap-break-word min-w-0 ${TONE_CLS[e.tone] || 'text-gray-300'}`}>{e.text}</span>
            </div>
          ))
        )}
      </div>
    </div>
  );
}
