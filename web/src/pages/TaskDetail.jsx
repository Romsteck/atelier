import { useState, useEffect } from 'react';
import { useParams, useNavigate } from 'react-router-dom';
import { CheckCircle, XCircle, Loader, Clock, ArrowLeft, Ban } from 'lucide-react';
import useWebSocket from '../hooks/useWebSocket';

const STATUS_CONFIG = {
  pending: { icon: Clock, color: 'text-gray-400', border: 'border-gray-500', label: 'En attente' },
  running: { icon: Loader, color: 'text-blue-400', border: 'border-blue-500', spin: true, label: 'En cours' },
  done: { icon: CheckCircle, color: 'text-green-400', border: 'border-green-500', label: 'Terminé' },
  failed: { icon: XCircle, color: 'text-red-400', border: 'border-red-500', label: 'Échoué' },
  cancelled: { icon: XCircle, color: 'text-gray-500', border: 'border-gray-600', label: 'Annulé' },
};

const TYPE_LABELS = {
  container_create: 'Création conteneur',
  container_delete: 'Suppression conteneur',
  container_migrate: 'Migration conteneur',
  container_rename: 'Renommage conteneur',
  app_deploy: 'Déploiement app',
  agent_update: 'Mise à jour agent',
  backup_trigger: 'Sauvegarde',
  git_sync: 'Git sync',
  acme_renew: 'Renouvellement certificat',
  updates_check: 'Scan mises à jour',
  updates_upgrade: 'Mise à jour système',
  dns_reload: 'Rechargement DNS',
  proxy_reload: 'Rechargement proxy',
  host_power: 'Action hôte',
};

function duration(start, end) {
  if (!start) return '';
  const s = ((end ? new Date(end) : new Date()) - new Date(start)) / 1000;
  if (s < 1) return '<1s';
  if (s < 60) return `${Math.round(s)}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m${Math.round(s % 60)}s`;
  return `${Math.floor(s / 3600)}h${Math.floor((s % 3600) / 60)}m`;
}

function formatDate(dateStr) {
  if (!dateStr) return '—';
  return new Date(dateStr).toLocaleString('fr-FR', {
    day: '2-digit', month: '2-digit', year: 'numeric',
    hour: '2-digit', minute: '2-digit', second: '2-digit',
  });
}

function StepTimeline({ steps }) {
  if (!steps?.length) {
    return <p className="text-sm text-gray-500 p-4">Aucune étape enregistrée</p>;
  }

  return (
    <div className="relative pl-8">
      {steps.map((step, i) => {
        const cfg = STATUS_CONFIG[step.status] || STATUS_CONFIG.pending;
        const Icon = cfg.icon;
        const isLast = i === steps.length - 1;

        return (
          <div key={step.id} className={`relative flex ${isLast ? '' : 'pb-4'}`}>
            {/* Vertical line (hidden on last step) */}
            {!isLast && (
              <div className="absolute left-[-22px] top-[22px] bottom-0 w-0.5 bg-gray-700" />
            )}

            {/* Dot */}
            <div className={`absolute -left-8 top-0.5 w-[22px] h-[22px] rounded-full border-2 ${cfg.border} bg-gray-800 flex items-center justify-center z-10`}>
              <Icon className={`w-3 h-3 ${cfg.color} ${cfg.spin ? 'animate-spin' : ''}`} />
            </div>

            {/* Content */}
            <div className="flex-1 min-w-0">
              <div className="flex items-center gap-2">
                <span className="text-sm font-medium text-gray-200">{step.step_name}</span>
                <span className={`text-xs ${cfg.color}`}>{cfg.label}</span>
                {step.started_at && (
                  <span className="text-xs text-gray-600 ml-auto">
                    {duration(step.started_at, step.finished_at)}
                  </span>
                )}
              </div>
              {step.message && (
                <p className="text-xs text-gray-400 mt-0.5">{step.message}</p>
              )}
              {step.details && (
                <pre className="text-xs text-gray-500 mt-1 bg-gray-900 rounded p-2 overflow-x-auto">
                  {typeof step.details === 'string' ? step.details : JSON.stringify(step.details, null, 2)}
                </pre>
              )}
            </div>
          </div>
        );
      })}
    </div>
  );
}

export default function TaskDetail() {
  const { id } = useParams();
  const navigate = useNavigate();
  const [task, setTask] = useState(null);
  const [steps, setSteps] = useState([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    setLoading(true);
    fetch(`/api/tasks/${id}`)
      .then(r => r.json())
      .then(data => {
        if (data?.task) setTask(data.task);
        if (data?.steps) setSteps(data.steps);
      })
      .catch(() => {})
      .finally(() => setLoading(false));
  }, [id]);

  // Live updates via WebSocket
  useWebSocket({
    'task:update': (data) => {
      if (data?.task?.id === id) {
        setTask(data.task);
        if (data.steps) setSteps(data.steps);
      }
    }
  });

  const handleCancel = async () => {
    try {
      await fetch(`/api/tasks/${id}/cancel`, { method: 'POST' });
    } catch { /* ignore */ }
  };

  if (loading) {
    return (
      <div className="p-6 flex justify-center">
        <Loader className="w-6 h-6 text-blue-400 animate-spin" />
      </div>
    );
  }

  if (!task) {
    return (
      <div className="p-6">
        <p className="text-gray-500">Tâche non trouvée</p>
      </div>
    );
  }

  const cfg = STATUS_CONFIG[task.status] || STATUS_CONFIG.pending;
  const TaskIcon = cfg.icon;

  return (
    <div className="p-6 max-w-3xl mx-auto">
      {/* Header */}
      <button
        onClick={() => navigate('/tasks')}
        className="flex items-center gap-1 text-sm text-gray-400 hover:text-white mb-4 transition-colors"
      >
        <ArrowLeft className="w-4 h-4" />
        Retour
      </button>

      <div className="bg-gray-800 border border-gray-700 rounded-lg p-6 mb-6">
        <div className="flex items-start gap-4">
          <div className={`p-2 rounded-lg ${cfg.border} border`}>
            <TaskIcon className={`w-6 h-6 ${cfg.color} ${cfg.spin ? 'animate-spin' : ''}`} />
          </div>
          <div className="flex-1">
            <h1 className="text-lg font-bold text-white">{task.title}</h1>
            <div className="flex flex-wrap items-center gap-3 mt-2 text-xs text-gray-400">
              <span className="bg-gray-700 px-2 py-0.5 rounded">
                {TYPE_LABELS[task.task_type] || task.task_type}
              </span>
              {task.target && (
                <span className="bg-gray-700 px-2 py-0.5 rounded">{task.target}</span>
              )}
              <span>{formatDate(task.created_at)}</span>
              {task.started_at && (
                <span>Durée: {duration(task.started_at, task.finished_at)}</span>
              )}
            </div>
            {task.error && (
              <div className="mt-3 p-3 bg-red-500/10 border border-red-500/20 rounded text-sm text-red-400">
                {task.error}
              </div>
            )}
          </div>
          {(task.status === 'pending' || task.status === 'running') && (
            <button
              onClick={handleCancel}
              className="p-2 text-gray-400 hover:text-red-400 transition-colors"
              title="Annuler"
            >
              <Ban className="w-5 h-5" />
            </button>
          )}
        </div>
      </div>

      {/* Steps timeline */}
      <div className="bg-gray-800 border border-gray-700 rounded-lg p-6">
        <h2 className="text-sm font-medium text-gray-300 mb-4">Étapes</h2>
        <StepTimeline steps={steps} />
      </div>
    </div>
  );
}
