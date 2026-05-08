import { useEffect, useMemo, useState } from 'react';
import { Archive, CheckCircle2, Clock3, Loader2, Play, Square, XCircle } from 'lucide-react';
import PageHeader from '../components/PageHeader';
import Button from '../components/Button';
import useWebSocket from '../hooks/useWebSocket';

const api = (path, opts = {}) =>
  fetch(`/api${path}`, { credentials: 'include', ...opts }).then((r) => r.json());

const getBackupStatus = () => api('/backup/status');
const getBackupRepos = () => api('/backup/repos');
const getBackupJobs = () => api('/backup/jobs');
const getBackupProgress = () => api('/backup/progress');
const triggerBackup = () => api('/backup/trigger', { method: 'POST' });
const cancelBackup = () => api('/backup/cancel', { method: 'POST' });

const REPO_LABELS = {
  homeroute: 'HomeRoute',
  containers: 'Containers',
  git: 'Git',
  pixel: 'Pixel',
  homecloud: 'Home Cloud',
};

const timeAgo = (dateStr) => {
  if (!dateStr) return 'Jamais';
  const diff = Math.floor((Date.now() - new Date(dateStr).getTime()) / 1000);
  if (diff < 60) return 'à l’instant';
  if (diff < 3600) return `il y a ${Math.floor(diff / 60)} min`;
  if (diff < 86400) return `il y a ${Math.floor(diff / 3600)} h`;
  if (diff < 604800) return `il y a ${Math.floor(diff / 86400)} j`;
  return new Date(dateStr).toLocaleDateString('fr-FR');
};

const formatDate = (dateStr) =>
  dateStr
    ? new Date(dateStr).toLocaleString('fr-FR', {
        day: '2-digit',
        month: '2-digit',
        year: 'numeric',
        hour: '2-digit',
        minute: '2-digit',
      })
    : '—';

const formatDuration = (secs) => {
  if (secs === null || secs === undefined) return '—';
  if (secs < 60) return `${secs}s`;
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = secs % 60;
  if (h > 0) return `${h}h ${m}m`;
  return `${m}m${s ? ` ${s}s` : ''}`;
};

const freshnessClasses = (dateStr) => {
  if (!dateStr) return 'border-red-500/30 bg-red-500/10 text-red-200';
  const ageHours = (Date.now() - new Date(dateStr).getTime()) / 3600000;
  if (ageHours < 24) return 'border-emerald-500/30 bg-emerald-500/10 text-emerald-200';
  if (ageHours < 24 * 7) return 'border-amber-500/30 bg-amber-500/10 text-amber-200';
  return 'border-red-500/30 bg-red-500/10 text-red-200';
};

const statusBadge = (job) => {
  if (!job) return { label: '—', className: 'border-gray-700 bg-gray-800 text-gray-300', icon: Clock3 };
  if (job.success) return { label: 'Succès', className: 'border-emerald-500/30 bg-emerald-500/10 text-emerald-200', icon: CheckCircle2 };
  return { label: 'Échec', className: 'border-red-500/30 bg-red-500/10 text-red-200', icon: XCircle };
};

const PIPELINE_STEPS = [
  { id: 'homeroute', label: 'Backup HomeRoute', repo: 'homeroute' },
  { id: 'containers', label: 'Backup Containers', repo: 'containers' },
  { id: 'git', label: 'Backup Git', repo: 'git' },
  { id: 'pixel', label: 'Backup Pixel', repo: 'pixel' },
  { id: 'homecloud', label: 'Backup Home Cloud', repo: 'homecloud' },
];

const formatBytes = (bytes) => {
  if (bytes === null || bytes === undefined) return null;
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
};

function getStepStatuses(status, progress) {
  const stage = status?.stage || 'idle';
  const currentRepo = progress?.current_repo;
  const isFailed = stage === 'failed';

  let activeIndex = -1;
  PIPELINE_STEPS.forEach((step, i) => {
    if (step.stages?.includes(stage)) activeIndex = i;
    if (step.repo && stage === 'running_backup' && step.repo === currentRepo) activeIndex = i;
  });

  if (stage === 'done') return PIPELINE_STEPS.map(() => 'complete');
  if (stage === 'idle') return PIPELINE_STEPS.map(() => 'pending');

  return PIPELINE_STEPS.map((_, i) => {
    if (i < activeIndex) return 'complete';
    if (i === activeIndex) return isFailed ? 'failed' : 'active';
    return 'pending';
  });
}

function PipelineStep({ step, stepStatus, progress, status, isLast }) {
  const icons = {
    pending: <Clock3 className="h-5 w-5 text-gray-500" />,
    active: <Loader2 className="h-5 w-5 text-blue-400 animate-spin" />,
    complete: <CheckCircle2 className="h-5 w-5 text-emerald-400" />,
    failed: <XCircle className="h-5 w-5 text-red-400" />,
  };

  const lineColors = {
    pending: 'bg-gray-700',
    active: 'bg-blue-500',
    complete: 'bg-emerald-500',
    failed: 'bg-red-500',
  };

  const borderColors = {
    pending: 'border-gray-700/70 bg-gray-800/70',
    active: 'border-blue-500/40 bg-blue-500/10',
    complete: 'border-emerald-500/30 bg-emerald-500/10',
    failed: 'border-red-500/30 bg-red-500/10',
  };

  const isBackupStep = Boolean(step.repo);
  const phase = progress?.phase;
  const isScanning = stepStatus === 'active' && isBackupStep && phase === 'scanning';
  const isTransferring = stepStatus === 'active' && isBackupStep && phase === 'transferring' && progress?.running;
  const isVerifying = stepStatus === 'active' && isBackupStep && phase === 'verifying';
  const progressPct = Math.max(0, Math.min(100, Number(progress?.progress || 0)));

  return (
    <div className="flex gap-4">
      <div className="flex flex-col items-center">
        <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full border border-gray-700 bg-gray-900">
          {icons[stepStatus]}
        </div>
        {!isLast && (
          <div className={`w-0.5 flex-1 ${lineColors[stepStatus]} transition-colors duration-300`} style={{ minHeight: '24px' }} />
        )}
      </div>

      <div className={`mb-3 flex-1 rounded-xl border px-4 py-3 ${borderColors[stepStatus]} transition-colors duration-300`}>
        <div className="text-sm font-medium text-white">{step.label}</div>

        {stepStatus === 'active' && status?.current_message && (
          <div className="mt-1 text-xs text-gray-400">{status.current_message}</div>
        )}

        {isScanning && (
          <div className="mt-2 flex items-center gap-2">
            <div className="h-1.5 w-24 overflow-hidden rounded-full bg-gray-950/60">
              <div className="h-full w-full animate-pulse rounded-full bg-gradient-to-r from-blue-500/60 to-cyan-400/60" />
            </div>
            <span className="text-xs text-blue-100/80">Scan des fichiers…</span>
            {progress?.files_total != null && progress.files_total > 0 && (
              <span className="text-xs text-gray-400">{progress.files_total} fichiers</span>
            )}
          </div>
        )}

        {isVerifying && (
          <div className="mt-2 flex items-center gap-2">
            <div className="h-1.5 w-24 overflow-hidden rounded-full bg-gray-950/60">
              <div className="h-full w-full animate-pulse rounded-full bg-gradient-to-r from-amber-500/60 to-yellow-400/60" />
            </div>
            <span className="text-xs text-amber-100/80">Vérification…</span>
          </div>
        )}

        {isTransferring && (
          <div className="mt-2">
            <div className="h-2 overflow-hidden rounded-full bg-gray-950/60">
              <div
                className="h-full rounded-full bg-gradient-to-r from-blue-500 to-cyan-400 transition-all duration-150"
                style={{ width: `${progressPct}%` }}
              />
            </div>
            <div className="mt-1.5 flex flex-wrap gap-x-4 text-xs text-blue-100/80">
              <span>{Math.round(progressPct)}%</span>
              {progress?.files_changed != null && (
                <span>{progress.files_changed} fichiers modifiés</span>
              )}
              {progress?.bytes_transferred != null && progress?.total_bytes != null && (
                <span>{formatBytes(progress.bytes_transferred)} / {formatBytes(progress.total_bytes)}</span>
              )}
              {progress?.speed && <span>{progress.speed}</span>}
              {progress?.remaining_secs != null && <span>Reste {formatDuration(progress.remaining_secs)}</span>}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

function RepoRow({ repo, active }) {
  const hasLastStats = repo.last_files_changed != null || repo.last_transferred_bytes != null;
  return (
    <div className={`flex items-center justify-between gap-4 rounded-2xl border px-4 py-4 ${active ? 'border-blue-500/40 bg-blue-500/10' : 'border-gray-700/70 bg-gray-800/70'}`}>
      <div className="min-w-0">
        <div className="truncate text-sm font-semibold text-white">{REPO_LABELS[repo.name] || repo.name}</div>
        <div className="text-sm text-gray-400">Dernière sauvegarde {timeAgo(repo.last_backup_at)}</div>
        {hasLastStats && (
          <div className="mt-0.5 flex gap-3 text-xs text-gray-500">
            {repo.last_files_changed != null && (
              <span>{repo.last_files_changed} fichier{repo.last_files_changed !== 1 ? 's' : ''} modifié{repo.last_files_changed !== 1 ? 's' : ''}</span>
            )}
            {repo.last_transferred_bytes != null && (
              <span>{formatBytes(repo.last_transferred_bytes)} transférés</span>
            )}
          </div>
        )}
      </div>
      <span className={`inline-flex shrink-0 items-center rounded-full border px-2.5 py-1 text-xs font-medium ${freshnessClasses(repo.last_backup_at)}`}>
        {repo.last_backup_at ? timeAgo(repo.last_backup_at) : 'Jamais'}
      </span>
    </div>
  );
}

export default function Backup() {
  const [status, setStatus] = useState(null);
  const [repos, setRepos] = useState([]);
  const [latestJob, setLatestJob] = useState(null);
  const [progress, setProgress] = useState({ running: false });
  const [loading, setLoading] = useState(true);
  const [triggering, setTriggering] = useState(false);
  const [message, setMessage] = useState(null);

  const fetchInitial = async () => {
    try {
      const [statusRes, reposRes, jobsRes, progressRes] = await Promise.all([
        getBackupStatus(),
        getBackupRepos(),
        getBackupJobs(),
        getBackupProgress(),
      ]);
      setStatus(statusRes && !statusRes.error ? statusRes : null);
      setRepos(Array.isArray(reposRes) ? reposRes : reposRes?.data || []);
      const jobs = Array.isArray(jobsRes) ? jobsRes : jobsRes?.data || [];
      setLatestJob(jobs[0] || null);
      setProgress(progressRes && !progressRes.error ? progressRes : { running: false });
    } catch (error) {
      console.error('Backup fetch error:', error);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    fetchInitial();
  }, []);

  useWebSocket({
    'backup:live': (data) => {
      if (data?.status) setStatus(data.status);
      if (data?.progress) setProgress(data.progress);
      if (Array.isArray(data?.repos)) setRepos(data.repos);
      setLatestJob(data?.latestJob || null);
      setLoading(false);
    },
  });

  const handleTrigger = async () => {
    if (triggering || isRunning) return;
    setTriggering(true);
    setStatus(prev => ({ ...prev, running: true, stage: 'running_backup', current_message: 'Demarrage...' }));
    setMessage(null);
    try {
      const res = await triggerBackup();
      if (res?.error) {
        setStatus(prev => ({ ...prev, running: false, stage: 'idle' }));
        setMessage({ type: 'error', text: res.error });
      }
    } catch {
      setStatus(prev => ({ ...prev, running: false, stage: 'idle' }));
      setMessage({ type: 'error', text: 'Erreur reseau' });
    } finally {
      setTriggering(false);
    }
  };

  const isRunning = Boolean(status?.running || progress?.running);
  const lastRun = useMemo(() => latestJob || null, [latestJob]);
  const stepStatuses = useMemo(() => getStepStatuses(status, progress), [status, progress]);
  const lastRunBadge = statusBadge(lastRun);
  const StatusIcon = lastRunBadge.icon;

  return (
    <div className="mx-auto flex max-w-4xl flex-col gap-6 p-6">
      <PageHeader
        icon={Archive}
        title="Sauvegarde"
        subtitle="Une vue simple, en temps réel."
      />

      {message && (
        <div className={`rounded-2xl border px-4 py-3 text-sm ${message.type === 'error' ? 'border-red-500/30 bg-red-500/10 text-red-200' : 'border-emerald-500/30 bg-emerald-500/10 text-emerald-200'}`}>
          {message.text}
        </div>
      )}

      <section className="rounded-3xl border border-gray-700/70 bg-gray-800/70 p-6">
        <div className="flex flex-col gap-5">
          <div>
            <div className="text-sm font-medium text-gray-400">Action</div>
            <div className="mt-1 text-2xl font-semibold text-white">
              {isRunning ? (status?.current_message || 'Sauvegarde en cours') : 'Prêt à lancer une sauvegarde'}
            </div>
          </div>

          <div className="flex gap-3">
            <Button
              onClick={handleTrigger}
              disabled={isRunning || triggering || loading}
              className="flex flex-1 h-14 items-center justify-center gap-3 rounded-2xl text-base font-semibold"
            >
              {isRunning || triggering ? <Loader2 className="h-5 w-5 animate-spin" /> : <Play className="h-5 w-5" />}
              {isRunning ? 'Sauvegarde en cours…' : 'Lancer une sauvegarde'}
            </Button>
            {isRunning && (
              <Button
                onClick={cancelBackup}
                variant="danger"
                className="flex h-14 items-center justify-center gap-3 rounded-2xl px-6 text-base font-semibold"
              >
                <Square className="h-5 w-5" />
                Arrêter
              </Button>
            )}
          </div>

          {isRunning && (
            <div className="rounded-2xl border border-gray-700/70 bg-gray-800/70 p-5">
              <div className="mb-4 text-sm font-medium text-gray-400">Pipeline</div>
              <div>
                {PIPELINE_STEPS.map((step, i) => (
                  <PipelineStep
                    key={step.id}
                    step={step}
                    stepStatus={stepStatuses[i]}
                    progress={step.repo === progress?.current_repo ? progress : null}
                    status={status}
                    isLast={i === PIPELINE_STEPS.length - 1}
                  />
                ))}
              </div>
            </div>
          )}
        </div>
      </section>

      {!isRunning && (
        <section className="space-y-3">
          <div>
            <h2 className="text-lg font-semibold text-white">Repos</h2>
            <p className="text-sm text-gray-400">Un coup d’œil suffit pour voir si tout est frais.</p>
          </div>
          <div className="grid gap-3">
            {repos.map((repo) => (
              <RepoRow key={repo.name} repo={repo} active={false} />
            ))}
            {!loading && repos.length === 0 && (
              <div className="rounded-2xl border border-dashed border-gray-700 bg-gray-800/50 px-4 py-6 text-sm text-gray-400">
                Aucun repo remonté par l’API.
              </div>
            )}
          </div>
        </section>
      )}

      <section className="rounded-3xl border border-gray-700/70 bg-gray-800/70 p-5">
        <div className="mb-3 text-sm font-medium text-gray-400">Dernière sauvegarde</div>
        {lastRun ? (
          <div className="flex flex-col gap-3 md:flex-row md:items-center md:justify-between">
            <div className="min-w-0">
              <div className="truncate text-base font-semibold text-white">
                {REPO_LABELS[lastRun.repo_name] || lastRun.repo_name}
              </div>
              <div className="text-sm text-gray-400">
                {formatDate(lastRun.started_at)} · {formatDuration(lastRun.duration_secs)}
              </div>
            </div>
            <div className="flex items-center gap-3">
              <span className={`inline-flex items-center gap-2 rounded-full border px-3 py-1.5 text-sm ${lastRunBadge.className}`}>
                <StatusIcon className="h-4 w-4" />
                {lastRunBadge.label}
              </span>
            </div>
          </div>
        ) : (
          <div className="text-sm text-gray-400">Aucune sauvegarde enregistrée pour le moment.</div>
        )}
      </section>
    </div>
  );
}
