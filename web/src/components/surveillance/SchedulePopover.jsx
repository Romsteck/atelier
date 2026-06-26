import { useState, useEffect } from 'react';
import { createPortal } from 'react-dom';
import { Settings, X, CalendarClock } from 'lucide-react';

// Config for the scheduled automatic sweep. A header trigger button opens a real
// centered modal (portal → document.body, so no overflow ancestor can clip it).
// `schedule` = { enabled, hour, cadence } | null ; `onSave(patch)` merges + persists.
export default function SchedulePopover({ schedule, onSave }) {
  const [open, setOpen] = useState(false);

  useEffect(() => {
    if (!open) return;
    const onKey = (e) => { if (e.key === 'Escape') setOpen(false); };
    document.addEventListener('keydown', onKey);
    const prev = document.body.style.overflow;
    document.body.style.overflow = 'hidden';
    return () => {
      document.removeEventListener('keydown', onKey);
      document.body.style.overflow = prev;
    };
  }, [open]);

  const enabled = schedule?.enabled ?? false;
  const hour = schedule?.hour ?? 3;
  const cadence = schedule?.cadence ?? 'daily';

  return (
    <>
      <button
        type="button"
        onClick={() => setOpen(true)}
        title="Planification de la surveillance automatique"
        className={`px-2 py-1 text-xs border rounded-sm flex items-center gap-1 transition ${
          enabled
            ? 'border-emerald-500/40 text-emerald-700 dark:text-emerald-300 bg-emerald-500/10'
            : 'border-gray-700 text-gray-400 hover:text-gray-200 hover:border-gray-600'
        }`}
      >
        <Settings className="w-3 h-3" />
        {enabled ? `Planifié ${String(hour).padStart(2, '0')}h` : 'Planification'}
      </button>

      {open && createPortal(
        <div className="fixed inset-0 z-50 flex items-center justify-center p-4">
          <div className="absolute inset-0 bg-black/60 backdrop-blur-xs" onClick={() => setOpen(false)} />
          <div
            role="dialog"
            aria-modal="true"
            className="relative w-full max-w-sm rounded-lg border border-gray-700 bg-gray-800 shadow-2xl"
          >
            <div className="flex items-center gap-2 px-4 py-3 border-b border-gray-700">
              <CalendarClock className="w-4 h-4 text-emerald-600 dark:text-emerald-400 shrink-0" />
              <h3 className="text-sm font-semibold text-gray-50 flex-1">Surveillance automatique planifiée</h3>
              <button onClick={() => setOpen(false)} className="text-gray-400 hover:text-gray-50" title="Fermer">
                <X className="w-4 h-4" />
              </button>
            </div>

            <div className="p-4 space-y-4 text-sm">
              <label className="flex items-center gap-2 cursor-pointer select-none">
                <input
                  type="checkbox"
                  checked={enabled}
                  onChange={(e) => onSave({ enabled: e.target.checked })}
                  className="accent-emerald-500 w-4 h-4"
                />
                <span className="text-gray-300">Activer le scan planifié</span>
              </label>

              <div className={`space-y-3 ${enabled ? '' : 'opacity-50 pointer-events-none'}`}>
                <div className="flex items-center justify-between gap-2">
                  <span className="text-gray-400">Heure (locale)</span>
                  <select
                    value={hour}
                    onChange={(e) => onSave({ hour: Number(e.target.value) })}
                    className="bg-gray-900 border border-gray-700 rounded-sm px-2 py-1 text-gray-200"
                  >
                    {Array.from({ length: 24 }, (_, h) => (
                      <option key={h} value={h}>{String(h).padStart(2, '0')}:00</option>
                    ))}
                  </select>
                </div>
                <div className="flex items-center justify-between gap-2">
                  <span className="text-gray-400">Cadence</span>
                  <select
                    value={cadence}
                    onChange={(e) => onSave({ cadence: e.target.value })}
                    className="bg-gray-900 border border-gray-700 rounded-sm px-2 py-1 text-gray-200"
                  >
                    <option value="daily">Quotidienne</option>
                    <option value="weekly">Hebdomadaire</option>
                  </select>
                </div>
              </div>

              <p className="text-[11px] text-gray-500 leading-relaxed border-t border-gray-700 pt-3">
                Lance un scan complet (toutes les apps, 3 scans chacune) à l'heure choisie.
                Forcé : re-scanne et nettoie les findings obsolètes partout.
              </p>
            </div>
          </div>
        </div>,
        document.body,
      )}
    </>
  );
}
