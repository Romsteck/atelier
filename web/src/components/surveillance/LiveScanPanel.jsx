import { useEffect, useMemo, useRef, useState } from 'react';
import { RefreshCw, Square, Terminal, ListTree } from 'lucide-react';
import { formatScanEvent, TONE_CLS } from './scanFormat';
import ScanStepsView from './ScanStepsView';
import Button from '../Button';

// Live console of a scan run in progress. Lines stream in over WebSocket. Two
// views: a derived STEP list (default — driven by the agent's `scan_progress`
// signposts) and the RAW transcript (debug). Shared by the per-app Surveillance
// tab (right-rail drawer, the default `className`) and the global sweep view (a
// grid cell — pass a `className` like `border bg-gray-950/60 rounded-sm`).
export default function LiveScanPanel({
  lines,
  kindLabel,
  onStop,
  stopping,
  className = 'w-96 shrink-0 border-l border-gray-700 bg-gray-950/60',
  title,
}) {
  const bodyRef = useRef(null);
  const [view, setView] = useState('steps');
  const rawEntries = useMemo(
    () => lines.map((l) => formatScanEvent(l.line)).filter((e) => e && (e.text?.trim() || e.tone === 'meta')),
    [lines],
  );
  // Auto-scroll to the live edge, but only when already near the bottom — so a
  // user reading/expanding an earlier step isn't yanked down by new output.
  useEffect(() => {
    const el = bodyRef.current;
    if (!el) return;
    if (el.scrollHeight - el.scrollTop - el.clientHeight < 80) el.scrollTop = el.scrollHeight;
  }, [lines.length, view]);

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
        <button
          onClick={() => setView((v) => (v === 'steps' ? 'raw' : 'steps'))}
          title={view === 'steps' ? 'Voir le détail brut' : 'Voir les étapes'}
          className="text-gray-500 hover:text-gray-200 shrink-0"
        >
          {view === 'steps' ? <Terminal className="w-3.5 h-3.5" /> : <ListTree className="w-3.5 h-3.5" />}
        </button>
        <RefreshCw className={`w-3 h-3 shrink-0 animate-spin ${stopping ? 'text-red-700 dark:text-red-400' : 'text-emerald-700 dark:text-emerald-400'}`} />
        {onStop && (
          <Button variant="danger" icon={Square} loading={stopping} onClick={onStop}>
            Arrêter
          </Button>
        )}
      </div>
      <div ref={bodyRef} className="flex-1 overflow-y-auto min-h-0">
        {view === 'steps' ? (
          <ScanStepsView lines={lines} />
        ) : rawEntries.length === 0 ? (
          <div className="text-xs text-gray-600 italic p-2">En attente de la sortie du scan…</div>
        ) : (
          <div className="p-2 space-y-1.5">
            {rawEntries.map((e, i) => (
              <div key={i} className="flex gap-1.5 text-[11px] leading-relaxed font-mono">
                {e.icon && <span className="shrink-0 select-none">{e.icon}</span>}
                <span className={`whitespace-pre-wrap wrap-break-word min-w-0 ${TONE_CLS[e.tone] || 'text-gray-300'}`}>{e.text}</span>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
