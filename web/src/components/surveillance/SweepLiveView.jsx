import {
  Square, ShieldCheck, AlertOctagon, Activity,
  CheckCircle2, Clock, XCircle, Loader2,
} from 'lucide-react';
import LiveScanPanel from './LiveScanPanel';
import Button from '../Button';

// The three scan cells of the current app, in launch order.
const KIND_CELLS = [
  { key: 'security', label: 'Sécurité', Icon: ShieldCheck },
  { key: 'code_review', label: 'Qualité', Icon: AlertOctagon },
  { key: 'business', label: 'Business', Icon: Activity },
];

const SETTLED = new Set(['done', 'empty', 'skipped', 'failed', 'cancelled']);

const CELL_LABEL = {
  pending: 'En attente', running: 'En cours', done: 'Terminé',
  empty: 'Aucune finding', skipped: 'Sauté', failed: 'Échec', cancelled: 'Annulé',
};

// Aggregate an app's 3 scan cells into one queue-pill status.
function aggStatus(app) {
  const cells = [app.security.status, app.code_review.status, app.business.status];
  if (cells.some((s) => s === 'running')) return 'running';
  if (cells.every((s) => SETTLED.has(s))) return cells.some((s) => s === 'failed') ? 'failed' : 'done';
  return 'pending';
}

const PILL_CLS = {
  pending: 'bg-gray-700/40 text-gray-500 dark:text-gray-400 border-gray-600/50',
  running: 'bg-blue-500/20 text-blue-700 dark:text-blue-300 border-blue-500/30',
  done: 'bg-emerald-500/20 text-emerald-700 dark:text-emerald-300 border-emerald-500/30',
  failed: 'bg-red-500/20 text-red-700 dark:text-red-300 border-red-500/30',
};

// Live view shown IN PLACE of the overview while a sweep runs: global progress,
// the per-app queue, and the 3 live scan consoles of the current app.
export default function SweepLiveView({ sweep, transcripts, onCancel }) {
  const total = sweep.total || sweep.apps.length;
  const done = sweep.done || 0;
  const pct = total ? Math.round((done / total) * 100) : 0;
  const current = sweep.apps[sweep.current_index];
  const cancelling = sweep.status === 'cancelling';

  return (
    <div className="space-y-4">
      <div className="rounded-lg border border-gray-700/50 bg-gray-800/40 p-4 space-y-3">
        <div className="flex items-center gap-3 flex-wrap">
          <Loader2 className="w-4 h-4 text-blue-700 dark:text-blue-300 animate-spin shrink-0" />
          <span className="text-sm text-gray-50 font-medium">Surveillance automatique en cours</span>
          <span className="text-xs text-gray-400">
            {current ? <>App : <span className="text-gray-200">{current.name}</span> · </> : null}
            {done}/{total} apps
          </span>
          <div className="flex-1" />
          <Button variant="danger" icon={Square} loading={cancelling} onClick={onCancel}>
            {cancelling ? 'Arrêt en cours…' : 'Annuler'}
          </Button>
        </div>

        <div className="h-1.5 overflow-hidden rounded-full bg-gray-950/60">
          <div className="h-full bg-emerald-500/70 transition-all" style={{ width: `${pct}%` }} />
        </div>

        <div className="flex flex-wrap gap-1.5">
          {sweep.apps.map((app, i) => {
            const st = aggStatus(app);
            const isCurrent = i === sweep.current_index && sweep.status === 'running';
            return (
              <span
                key={app.slug}
                className={`text-[11px] px-1.5 py-0.5 rounded-sm border ${PILL_CLS[st] || PILL_CLS.pending} ${isCurrent ? 'ring-1 ring-blue-400/60' : ''}`}
                title={`${app.name} — ${CELL_LABEL[st] || st}`}
              >
                {app.slug}
              </span>
            );
          })}
        </div>
      </div>

      {current && (
        <div className="grid grid-cols-1 lg:grid-cols-3 gap-3">
          {KIND_CELLS.map(({ key, label, Icon }) => {
            const cell = current[key];
            if (cell.status === 'running' && cell.run_id) {
              return (
                <div key={key} className="h-96">
                  <LiveScanPanel
                    lines={transcripts[cell.run_id] || []}
                    kindLabel={label}
                    className="h-full border border-gray-700 bg-gray-950/60 rounded-sm"
                  />
                </div>
              );
            }
            return (
              <div key={key} className="h-96 flex flex-col border border-gray-700 bg-gray-950/60 rounded-sm">
                <div className="px-3 py-2 border-b border-gray-700 flex items-center gap-2">
                  <Icon className="w-3.5 h-3.5 text-gray-400 shrink-0" />
                  <span className="text-xs text-gray-300 flex-1 truncate">{label}</span>
                </div>
                <div className="flex-1 flex items-center justify-center text-xs text-gray-500 gap-1.5">
                  {cell.status === 'done' || cell.status === 'empty' ? (
                    <CheckCircle2 className="w-4 h-4 text-emerald-700 dark:text-emerald-400" />
                  ) : cell.status === 'failed' ? (
                    <XCircle className="w-4 h-4 text-red-700 dark:text-red-400" />
                  ) : cell.status === 'pending' ? (
                    <Clock className="w-4 h-4" />
                  ) : null}
                  {CELL_LABEL[cell.status] || cell.status}
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
