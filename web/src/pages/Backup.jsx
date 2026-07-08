import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  Archive, Play, Square, Loader2, CheckCircle2, XCircle, Clock3, ChevronRight,
  Database, GitBranch, FileCog, ShieldCheck, AlertTriangle, RefreshCw, Eye, Server, FolderSearch,
  Settings2, X,
} from 'lucide-react';
import PageHeader from '../components/PageHeader';
import Button from '../components/Button';
import useWebSocket from '../hooks/useWebSocket';
import {
  getBackupStatus, getBackupTarget, setBackupTarget, discoverShares,
  revealResticPassword, runBackup, cancelBackup, getBackupRuns,
} from '../api/client';
import { timeAgo, formatDate, formatDuration, durationSecs, formatBytes, freshnessClasses } from '../utils/formatters';
import { apiErr } from '../utils/apiErr';

const STEPS = [
  { tag: 'git', label: 'Dépôts Git', icon: GitBranch },
  { tag: 'postgres', label: 'PostgreSQL', icon: Database },
  { tag: 'config', label: 'Config Atelier', icon: FileCog },
];

const STATUS_META = {
  success: { label: 'Réussi', cls: 'border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-200', icon: CheckCircle2 },
  failed: { label: 'Échec', cls: 'border-red-500/30 bg-red-500/10 text-red-700 dark:text-red-200', icon: XCircle },
  cancelled: { label: 'Annulé', cls: 'border-orange-500/30 bg-orange-500/10 text-orange-700 dark:text-orange-200', icon: Square },
  running: { label: 'En cours', cls: 'border-blue-500/30 bg-blue-500/10 text-blue-700 dark:text-blue-200', icon: Loader2 },
};

// État par étape, accumulé depuis les events `backup:live` (cf. WS handler).
const blankProgress = () => ({
  git: { status: 'pending' },
  postgres: { status: 'pending' },
  config: { status: 'pending' },
});

function PipelineStep({ step, state, isLast }) {
  const Icon = step.icon;
  const status = state?.status || 'pending';
  const icons = {
    pending: <Clock3 className="h-5 w-5 text-gray-500" />,
    active: <Loader2 className="h-5 w-5 animate-spin text-blue-400" />,
    complete: <CheckCircle2 className="h-5 w-5 text-emerald-400" />,
    failed: <XCircle className="h-5 w-5 text-red-400" />,
  };
  const lineColor = { pending: 'bg-gray-700', active: 'bg-blue-500', complete: 'bg-emerald-500', failed: 'bg-red-500' }[status];
  const border = {
    pending: 'border-gray-700/70 bg-gray-800/70',
    active: 'border-blue-500/40 bg-blue-500/10',
    complete: 'border-emerald-500/30 bg-emerald-500/10',
    failed: 'border-red-500/30 bg-red-500/10',
  }[status];
  const isActive = status === 'active';
  const bd = state?.bytes_done;
  const bt = state?.bytes_total;
  // Déterminé (git/config : taille connue) vs indéterminé (postgres streamé :
  // restic ignore la taille du flux → barre animée, pas de %).
  const determinate = isActive && bt > 0;
  const pct = determinate ? Math.max(3, Math.min(100, Math.round((bd / bt) * 100))) : 0;
  return (
    <div className="flex gap-4">
      <div className="flex flex-col items-center">
        <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full border border-gray-700 bg-gray-900">
          {icons[status]}
        </div>
        {!isLast && <div className={`w-0.5 flex-1 ${lineColor} transition-colors duration-300`} style={{ minHeight: '24px' }} />}
      </div>
      <div className={`mb-3 flex-1 rounded-xl border px-4 py-3 ${border} transition-colors duration-300`}>
        <div className="flex items-center justify-between gap-2 text-sm font-medium text-gray-50">
          <span className="flex items-center gap-2"><Icon className="h-4 w-4 text-gray-400" /> {step.label}</span>
          {status === 'complete' && bd != null && <span className="text-xs font-normal text-emerald-700 dark:text-emerald-300/90">+{formatBytes(bd)}</span>}
        </div>
        {isActive && (
          <div className="mt-2">
            <div className="h-1.5 overflow-hidden rounded-full bg-gray-950/60">
              {determinate ? (
                <div className="h-full rounded-full bg-linear-to-r from-blue-500 to-cyan-400 transition-[width] duration-300 ease-out" style={{ width: `${pct}%` }} />
              ) : (
                <div className="h-full w-full animate-pulse rounded-full bg-linear-to-r from-blue-500/50 to-cyan-400/60" />
              )}
            </div>
            <div className="mt-1 text-xs text-blue-700 dark:text-blue-100/80">
              {determinate
                ? `${formatBytes(bd)} / ${formatBytes(bt)} · ${pct}%`
                : bd != null
                  ? `${formatBytes(bd)} traités…`
                  : 'En cours…'}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

function StatusPill({ status }) {
  const m = STATUS_META[status] || STATUS_META.running;
  const Icon = m.icon;
  return (
    <span className={`inline-flex items-center gap-1.5 rounded-full border px-2.5 py-0.5 text-xs font-medium ${m.cls}`}>
      <Icon className={`h-3.5 w-3.5 ${status === 'running' ? 'animate-spin' : ''}`} /> {m.label}
    </span>
  );
}

// Détail d'un snapshot (git/postgres/config) dans une mini-timeline.
function SnapshotRow({ snap, isLast }) {
  const ok = snap.status === 'success';
  return (
    <div className="flex gap-3">
      <div className="flex flex-col items-center">
        <div className="mt-1 flex h-3 w-3 shrink-0 items-center justify-center rounded-full border border-gray-700 bg-gray-900">
          <div className={`h-1.5 w-1.5 rounded-full ${ok ? 'bg-emerald-400' : 'bg-red-400'}`} />
        </div>
        {!isLast && <div className="w-px flex-1 bg-gray-700" style={{ minHeight: '18px' }} />}
      </div>
      <div className="flex-1 pb-3">
        <div className="flex items-center gap-2 text-sm">
          <span className="font-medium text-gray-200">{snap.tag}</span>
          {!ok && <span className="text-xs text-red-700 dark:text-red-300">échec</span>}
        </div>
        <div className="mt-0.5 flex flex-wrap gap-x-4 gap-y-0.5 text-xs text-gray-400">
          <span>{snap.files} fichiers</span>
          <span>traité {formatBytes(snap.bytes_processed)}</span>
          <span className="text-emerald-700 dark:text-emerald-300/90">ajouté {formatBytes(snap.bytes_added)}</span>
          {snap.snapshot_id && <span className="font-mono text-gray-500">{snap.snapshot_id}</span>}
        </div>
        {snap.error && <div className="mt-1 text-xs text-red-700 dark:text-red-300">{snap.error}</div>}
      </div>
    </div>
  );
}

function HistoryRow({ run, expanded, onToggle }) {
  const dur = durationSecs(run.started_at, run.finished_at);
  return (
    <div className="rounded-xl border border-gray-700/60 bg-gray-800/50">
      <button onClick={onToggle} className="flex w-full items-center gap-3 px-4 py-3 text-left hover:bg-gray-700/30">
        <ChevronRight className={`h-4 w-4 shrink-0 text-gray-500 transition-transform ${expanded ? 'rotate-90' : ''}`} />
        <StatusPill status={run.status} />
        <span className="text-sm text-gray-300" title={formatDate(run.started_at)}>{timeAgo(run.started_at)}</span>
        <span className="rounded border border-gray-700 bg-gray-900/60 px-1.5 py-0.5 text-[11px] text-gray-400">
          {run.trigger === 'cron' ? 'planifié' : 'manuel'}
        </span>
        <span className="ml-auto flex items-center gap-4 text-xs text-gray-400">
          {run.total_added != null && <span className="text-emerald-700 dark:text-emerald-300/90">+{formatBytes(run.total_added)}</span>}
          <span>{formatDuration(dur)}</span>
        </span>
      </button>
      {expanded && (
        <div className="border-t border-gray-700/60 bg-gray-900/40 px-4 py-3">
          {run.error && (
            <div className="mb-3 rounded-lg border border-red-500/30 bg-red-500/10 px-3 py-2 text-xs text-red-700 dark:text-red-200">
              {run.error}
            </div>
          )}
          <div className="mb-2 flex flex-wrap gap-x-6 gap-y-1 text-xs text-gray-400">
            <span>Début {formatDate(run.started_at)}</span>
            {run.finished_at && <span>Fin {formatDate(run.finished_at)}</span>}
            <span>Durée {formatDuration(dur)}</span>
          </div>
          {run.snapshots?.length > 0 ? (
            <div>
              {run.snapshots.map((s, i) => (
                <SnapshotRow key={s.tag} snap={s} isLast={i === run.snapshots.length - 1} />
              ))}
            </div>
          ) : (
            <div className="text-xs text-gray-500">Aucun snapshot enregistré.</div>
          )}
        </div>
      )}
    </div>
  );
}

function Sparkline({ runs }) {
  const recent = useMemo(() => runs.slice(0, 30).reverse(), [runs]);
  const max = Math.max(1, ...recent.map((r) => r.total_added || 0));
  if (recent.length === 0) return null;
  return (
    <div className="flex items-end gap-0.5" style={{ height: '32px' }}>
      {recent.map((r) => {
        const h = Math.max(3, Math.round(((r.total_added || 0) / max) * 30));
        const ok = r.status === 'success';
        return (
          <div
            key={r.id}
            className={`w-1.5 rounded-sm ${ok ? 'bg-emerald-500/70' : 'bg-red-500/70'}`}
            style={{ height: `${h}px` }}
            title={`${formatDate(r.started_at)} · +${formatBytes(r.total_added || 0)}`}
          />
        );
      })}
    </div>
  );
}

function ToolDot({ ok, label }) {
  return (
    <span className="inline-flex items-center gap-1.5 text-xs text-gray-400">
      <span className={`h-2 w-2 rounded-full ${ok ? 'bg-emerald-400' : 'bg-red-400'}`} /> {label}
    </span>
  );
}

const emptyForm = {
  host: '', share: '', username: '',
  password: '', repo_subpath: 'atelier-backup', retention_keep: 7,
  schedule_enabled: false, schedule_cadence: 'daily', schedule_hour: 3,
};

const FIELD = 'w-full rounded-lg border border-gray-700 bg-gray-900/60 px-3 py-2 text-sm text-gray-100 focus:border-blue-500 focus:outline-none';
const LBL = 'mb-1 block text-xs font-medium text-gray-400';
const STEP_LABELS = ['Serveur', 'Partage', 'Planification'];

// Carte récap sur la page principale ; la config se fait dans la popup.
function ServerCard({ target, onConfigure, onReveal }) {
  const configured = target && target.host;
  return (
    <section className="rounded-2xl border border-gray-700/70 bg-gray-800/50 p-5">
      <div className="flex flex-wrap items-center justify-between gap-4">
        <div className="flex items-center gap-3">
          <Server className="h-5 w-5 text-gray-400" />
          <div>
            <div className="text-sm font-semibold text-gray-50">Serveur de sauvegarde</div>
            {configured ? (
              <div className="text-sm text-gray-400">
                <span className="font-mono text-gray-300">{target.host}</span>
                {target.share && <> · partage <span className="font-mono text-gray-300">{target.share}</span></>}
              </div>
            ) : (
              <div className="text-sm text-gray-500">Aucun serveur configuré</div>
            )}
          </div>
        </div>
        <div className="flex items-center gap-2">
          {target?.has_restic_password && (
            <Button onClick={onReveal} variant="neutral" size="md" icon={Eye}>Mot de passe</Button>
          )}
          <Button onClick={onConfigure} variant="primary" size="md" icon={Settings2}>Configurer</Button>
        </div>
      </div>
    </section>
  );
}

// Assistant pas-à-pas (popup) : 1) serveur+identifiants → liste les partages,
// 2) choix du partage + emplacement, 3) planification.
function ConfigWizard({ target, onClose, onSaved, setToast }) {
  const [step, setStep] = useState(1);
  const [form, setForm] = useState(() =>
    target
      ? {
          host: target.host || '', share: target.share || '', username: target.username || '',
          password: '', repo_subpath: target.repo_subpath || 'atelier-backup',
          retention_keep: target.retention_keep ?? 7,
          schedule_enabled: !!target.schedule_enabled,
          schedule_cadence: target.schedule_cadence || 'daily',
          schedule_hour: target.schedule_hour ?? 3,
        }
      : { ...emptyForm },
  );
  const [shares, setShares] = useState([]);
  const [discovering, setDiscovering] = useState(false);
  const [saving, setSaving] = useState(false);

  const set = (k) => (e) => {
    const v = e.target.type === 'checkbox' ? e.target.checked : e.target.value;
    if (k === 'host') setShares([]);
    setForm((f) => ({ ...f, [k]: v }));
  };

  const discover = async () => {
    if (!form.host.trim() || !form.username.trim()) {
      setToast({ type: 'error', text: 'Serveur et utilisateur requis' });
      return;
    }
    setDiscovering(true);
    try {
      const { data } = await discoverShares({ host: form.host, username: form.username, password: form.password });
      const list = data.shares || [];
      setShares(list);
      if (list.length === 0) {
        setToast({ type: 'error', text: 'Connecté, mais aucun partage exposé' });
      } else {
        if (!list.includes(form.share)) setForm((f) => ({ ...f, share: list[0] }));
        setStep(2);
      }
    } catch (e) {
      setToast({ type: 'error', text: apiErr(e) });
    }
    setDiscovering(false);
  };

  const save = async () => {
    setSaving(true);
    try {
      await setBackupTarget({
        ...form,
        retention_keep: Number(form.retention_keep),
        schedule_hour: Number(form.schedule_hour),
      });
      setToast({ type: 'ok', text: 'Configuration enregistrée' });
      onSaved();
      onClose();
    } catch (e) {
      setToast({ type: 'error', text: apiErr(e) });
    }
    setSaving(false);
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4 backdrop-blur-sm" onClick={onClose}>
      <div className="w-full max-w-lg rounded-2xl border border-gray-700 bg-gray-900 shadow-2xl" onClick={(e) => e.stopPropagation()}>
        {/* En-tête + stepper */}
        <div className="flex items-center justify-between border-b border-gray-700 px-5 py-3">
          <div className="flex items-center gap-2">
            {STEP_LABELS.map((s, i) => (
              <div key={s} className="flex items-center gap-2">
                <span className={`flex h-6 w-6 items-center justify-center rounded-full text-xs ${step === i + 1 ? 'bg-blue-500 text-white' : step > i + 1 ? 'bg-emerald-500/80 text-white' : 'bg-gray-700 text-gray-400'}`}>{i + 1}</span>
                <span className={`text-xs ${step === i + 1 ? 'text-gray-50' : 'text-gray-500'}`}>{s}</span>
                {i < STEP_LABELS.length - 1 && <span className="mx-1 text-gray-600">›</span>}
              </div>
            ))}
          </div>
          <button onClick={onClose} className="text-gray-400 hover:text-gray-200"><X className="h-5 w-5" /></button>
        </div>

        {/* Corps */}
        <div className="px-5 py-5">
          {step === 1 && (
            <div className="space-y-4">
              <div><label className={LBL}>Serveur (nom ou IP) *</label><input className={FIELD} value={form.host} onChange={set('host')} placeholder="192.168.1.10 ou nas.local" autoFocus /></div>
              <div><label className={LBL}>Utilisateur *</label><input className={FIELD} value={form.username} onChange={set('username')} placeholder="atelier" /></div>
              <div>
                <label className={LBL}>Mot de passe {target?.has_password && <span className="text-emerald-400">• défini</span>}</label>
                <input className={FIELD} type="password" value={form.password} onChange={set('password')} placeholder={target?.has_password ? '•••••• (inchangé)' : ''} />
              </div>
              <p className="text-xs text-gray-500">On se connecte au serveur pour lister ses partages — rien n&apos;est enregistré à cette étape.</p>
            </div>
          )}

          {step === 2 && (
            <div className="space-y-4">
              <div>
                <label className={LBL}>Partage *</label>
                {shares.length > 0 ? (
                  <select className={FIELD} value={form.share} onChange={set('share')}>
                    {shares.map((s) => <option key={s} value={s}>{s}</option>)}
                  </select>
                ) : form.share ? (
                  <div className="flex items-center gap-2 rounded-lg border border-gray-700 bg-gray-900/60 px-3 py-2 text-sm text-gray-200">
                    {form.share} <span className="text-xs text-gray-500">— revenez à l&apos;étape 1 pour relister</span>
                  </div>
                ) : (
                  <div className="rounded-lg border border-dashed border-gray-700 px-3 py-2 text-xs text-gray-500">Aucun partage — revenez à l&apos;étape 1.</div>
                )}
              </div>
              <div><label className={LBL}>Dossier du dépôt</label><input className={FIELD} value={form.repo_subpath} onChange={set('repo_subpath')} /></div>
              <div><label className={LBL}>Rétention (snapshots gardés)</label><input className={FIELD} type="number" min="1" value={form.retention_keep} onChange={set('retention_keep')} /></div>
            </div>
          )}

          {step === 3 && (
            <div className="space-y-4">
              <label className="flex items-center gap-2 text-sm text-gray-200">
                <input type="checkbox" checked={form.schedule_enabled} onChange={set('schedule_enabled')} className="h-4 w-4" />
                Sauvegarde planifiée
              </label>
              {form.schedule_enabled ? (
                <div className="grid gap-4 sm:grid-cols-2">
                  <div>
                    <label className={LBL}>Cadence</label>
                    <select className={FIELD} value={form.schedule_cadence} onChange={set('schedule_cadence')}>
                      <option value="daily">Quotidienne</option>
                      <option value="weekly">Hebdomadaire</option>
                    </select>
                  </div>
                  <div><label className={LBL}>Heure (0–23, locale)</label><input className={FIELD} type="number" min="0" max="23" value={form.schedule_hour} onChange={set('schedule_hour')} /></div>
                </div>
              ) : (
                <p className="text-xs text-gray-500">Désactivée : les sauvegardes se lancent à la demande.</p>
              )}
            </div>
          )}
        </div>

        {/* Pied — navigation */}
        <div className="flex items-center justify-between border-t border-gray-700 px-5 py-3">
          <Button variant="neutral" size="md" onClick={step === 1 ? onClose : () => setStep(step - 1)}>
            {step === 1 ? 'Annuler' : 'Retour'}
          </Button>
          {step === 1 && (
            <div className="flex gap-2">
              {form.share && <Button variant="neutral" size="md" onClick={() => setStep(2)}>Continuer →</Button>}
              <Button onClick={discover} variant="primary" size="md" icon={FolderSearch} loading={discovering}>
                {discovering ? 'Connexion…' : 'Lister les partages'}
              </Button>
            </div>
          )}
          {step === 2 && <Button onClick={() => setStep(3)} variant="neutral" size="md" disabled={!form.share.trim()}>Suivant →</Button>}
          {step === 3 && <Button onClick={save} variant="primary" size="md" loading={saving}>Enregistrer</Button>}
        </div>
      </div>
    </div>
  );
}

export default function Backup() {
  const [status, setStatus] = useState(null);
  const [target, setTarget] = useState(null);
  const [runs, setRuns] = useState([]);
  const [total, setTotal] = useState(0);
  const [limit, setLimit] = useState(50);
  const [live, setLive] = useState(null);
  const [running, setRunning] = useState(false);
  const [expanded, setExpanded] = useState(null);
  const [filter, setFilter] = useState('all');
  const [toast, setToast] = useState(null);
  const [loading, setLoading] = useState(true);
  const [triggering, setTriggering] = useState(false);
  const [wizardOpen, setWizardOpen] = useState(false);
  const [runProgress, setRunProgress] = useState(blankProgress);
  const liveRunIdRef = useRef(null);

  const reveal = async () => {
    try {
      const { data } = await revealResticPassword();
      window.prompt('Mot de passe du dépôt restic — à CONSERVER HORS-LIGNE (sans lui, les sauvegardes sont définitivement illisibles) :', data.password);
    } catch (e) {
      setToast({ type: 'error', text: apiErr(e) });
    }
  };

  const reload = useCallback(async () => {
    try {
      const [s, t, r] = await Promise.all([
        getBackupStatus().catch(() => null),
        getBackupTarget().catch(() => null),
        getBackupRuns(limit, 0).catch(() => null),
      ]);
      if (s?.data) { setStatus(s.data); setRunning(!!s.data.running); }
      if (t?.data) setTarget(t.data.target || null);
      if (r?.data) { setRuns(r.data.runs || []); setTotal(r.data.total || 0); }
    } finally {
      setLoading(false);
    }
  }, [limit]);

  useEffect(() => { reload(); }, [reload]);

  const { epoch } = useWebSocket({
    'backup:live': (data) => {
      setLive(data);
      // Nouveau run → réinitialise la map d'étapes (ref pour comparer sans re-render).
      const isNewRun = data.run_id && data.run_id !== liveRunIdRef.current;
      if (data.run_id) liveRunIdRef.current = data.run_id;
      const tag = data.detail?.tag;
      setRunProgress((prev) => {
        let next = isNewRun ? blankProgress() : { ...prev };
        if (tag) {
          const cur = next[tag] || {};
          if (data.status === 'success') {
            next[tag] = { status: 'complete', bytes_done: data.detail.bytes_done ?? cur.bytes_done, bytes_total: cur.bytes_total };
          } else if (data.status === 'failed' || data.status === 'cancelled') {
            next[tag] = { ...cur, status: 'failed' };
          } else {
            next[tag] = { status: 'active', bytes_done: data.detail.bytes_done, bytes_total: data.detail.bytes_total };
          }
        }
        // Run terminé OK → toutes les étapes non échouées passent complete.
        if (data.phase === 'done') {
          for (const t of Object.keys(next)) {
            if (next[t].status !== 'failed') next[t] = { ...next[t], status: 'complete' };
          }
        }
        return next;
      });

      if (['done', 'failed', 'cancelled'].includes(data.phase)) {
        setRunning(false);
        setToast({
          type: data.phase === 'done' ? 'ok' : 'error',
          text: data.phase === 'done' ? 'Sauvegarde terminée' : data.message,
        });
        reload();
      } else {
        setRunning(true);
      }
    },
    // Subscriber laggé côté serveur (events `backup:live` perdus) → l'état local peut
    // être périmé (event terminal raté = bouton bloqué sur « Arrêter »), on re-fetch.
    'resync': (m) => { if (m?.channel === 'backup:live') reload(); },
  });

  // Réconciliation au reconnect WS : `running` est posé de façon optimiste et ne
  // retombe que sur l'event `backup:live` terminal — si le WS a coupé pendant un run,
  // cet event est perdu. `reload()` re-lit le statut serveur (autoritaire) et
  // repose `running` depuis `status.running`.
  useEffect(() => {
    if (epoch > 0) reload();
  }, [epoch, reload]);

  const trigger = async () => {
    setTriggering(true);
    try {
      await runBackup();
      liveRunIdRef.current = null; // force le reset de la map au 1ᵉʳ event du run
      setRunProgress(blankProgress());
      setRunning(true);
      setLive({ phase: 'repo', status: 'running', message: 'Démarrage…' });
    } catch (e) {
      setToast({ type: 'error', text: apiErr(e) });
    }
    setTriggering(false);
  };

  const stop = async () => {
    const id = status?.current_run_id || live?.run_id;
    if (!id) return;
    try { await cancelBackup(id); } catch (e) { setToast({ type: 'error', text: apiErr(e) }); }
  };

  const tools = status?.tools || { restic: false, rclone: false };
  const toolsOk = tools.restic && tools.rclone;
  const configured = status?.target_configured;
  const repo = status?.repo_stats;

  const filteredRuns = useMemo(() => {
    if (filter === 'all') return runs;
    return runs.filter((r) => r.status === filter);
  }, [runs, filter]);

  // Mémoïsé : la liste d'historique ne dépend pas de la progression live, donc
  // ne re-rend pas à chaque tick (évite le jank pendant un run).
  const historyRows = useMemo(
    () => filteredRuns.map((run) => (
      <HistoryRow
        key={run.id}
        run={run}
        expanded={expanded === run.id}
        onToggle={() => setExpanded((cur) => (cur === run.id ? null : run.id))}
      />
    )),
    [filteredRuns, expanded],
  );

  return (
    <div className="mx-auto flex max-w-4xl flex-col gap-6 p-6">
      <PageHeader icon={Archive} title="Sauvegarde" subtitle="Incrémentale, chiffrée — vers votre serveur Samba.">
        {running ? (
          <Button onClick={stop} variant="danger" size="md" icon={Square}>Arrêter</Button>
        ) : (
          <Button onClick={trigger} variant="success" size="md" icon={Play} loading={triggering} disabled={triggering || loading || !configured || !toolsOk}>
            Lancer une sauvegarde
          </Button>
        )}
      </PageHeader>

      {toast && (
        <div className={`rounded-xl border px-4 py-3 text-sm ${toast.type === 'error' ? 'border-red-500/30 bg-red-500/10 text-red-700 dark:text-red-200' : 'border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-200'}`}>
          {toast.text}
          <button onClick={() => setToast(null)} className="ml-3 text-xs text-gray-400 hover:text-gray-200">fermer</button>
        </div>
      )}

      {/* Aperçu du dépôt */}
      <section className="rounded-2xl border border-gray-700/70 bg-gray-800/50 p-5">
        <div className="flex flex-wrap items-center justify-between gap-4">
          <div className="flex items-center gap-3">
            <span className={`inline-flex items-center gap-2 rounded-full border px-3 py-1 text-sm ${freshnessClasses(status?.last_success_at)}`}>
              <ShieldCheck className="h-4 w-4" />
              {status?.last_success_at ? `Dernière : ${timeAgo(status.last_success_at)}` : 'Jamais sauvegardé'}
            </span>
            {repo && (
              <span className="text-sm text-gray-400">
                {formatBytes(repo.total_size_bytes)} · {repo.snapshot_count} snapshot{repo.snapshot_count !== 1 ? 's' : ''}
              </span>
            )}
          </div>
          <div className="flex items-center gap-4">
            <ToolDot ok={tools.restic} label="restic" />
            <ToolDot ok={tools.rclone} label="rclone" />
            <span className="text-xs text-gray-500">
              {status?.schedule_enabled ? 'planifié' : 'manuel'}
            </span>
          </div>
        </div>
        {runs.length > 0 && (
          <div className="mt-4">
            <div className="mb-1 text-xs text-gray-500">Activité récente (octets ajoutés / run)</div>
            <Sparkline runs={runs} />
          </div>
        )}
        {!toolsOk && (
          <div className="mt-4 flex items-center gap-2 rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-xs text-amber-700 dark:text-amber-200">
            <AlertTriangle className="h-4 w-4" /> Binaire manquant sur le serveur : installez {!tools.restic && 'restic'} {!tools.restic && !tools.rclone && '+'} {!tools.rclone && 'rclone'}.
          </div>
        )}
      </section>

      {/* Run en cours */}
      {running && (
        <section className="rounded-2xl border border-blue-500/20 bg-blue-500/5 p-5">
          <div className="mb-4 text-sm font-medium text-gray-300">{live?.message || 'Sauvegarde en cours…'}</div>
          <div>
            {STEPS.map((step, i) => (
              <PipelineStep key={step.tag} step={step} state={runProgress[step.tag]} isLast={i === STEPS.length - 1} />
            ))}
          </div>
        </section>
      )}

      {/* Historique */}
      <section>
        <div className="mb-3 flex items-center justify-between">
          <h2 className="text-lg font-semibold text-gray-50">Historique</h2>
          <div className="flex items-center gap-2">
            {['all', 'success', 'failed'].map((f) => (
              <button
                key={f}
                onClick={() => setFilter(f)}
                className={`rounded-full px-3 py-1 text-xs ${filter === f ? 'bg-gray-700 text-gray-100' : 'text-gray-400 hover:bg-gray-800'}`}
              >
                {f === 'all' ? 'Tous' : f === 'success' ? 'Réussis' : 'Échoués'}
              </button>
            ))}
            <button onClick={reload} className="rounded-full p-1.5 text-gray-400 hover:bg-gray-800" title="Rafraîchir">
              <RefreshCw className="h-4 w-4" />
            </button>
          </div>
        </div>
        <div className="flex flex-col gap-2">
          {historyRows}
          {!loading && filteredRuns.length === 0 && (
            <div className="rounded-xl border border-dashed border-gray-700 bg-gray-800/40 px-4 py-8 text-center text-sm text-gray-400">
              Aucune sauvegarde enregistrée.
            </div>
          )}
        </div>
        {runs.length < total && (
          <div className="mt-3 text-center">
            <Button onClick={() => setLimit((l) => l + 50)} variant="ghost" size="sm">
              Voir plus ({runs.length}/{total})
            </Button>
          </div>
        )}
      </section>

      {/* Configuration */}
      <ServerCard target={target} onConfigure={() => setWizardOpen(true)} onReveal={reveal} />
      {wizardOpen && (
        <ConfigWizard target={target} onClose={() => setWizardOpen(false)} onSaved={reload} setToast={setToast} />
      )}
    </div>
  );
}
