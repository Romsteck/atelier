import { useState, useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { CheckCircle, XCircle, Loader, Clock, ChevronRight, ListTodo } from 'lucide-react';

const STATUS_CONFIG = {
  pending: { icon: Clock, color: 'text-gray-400', bg: 'bg-gray-400/10', label: 'En attente' },
  running: { icon: Loader, color: 'text-blue-400', bg: 'bg-blue-400/10', spin: true, label: 'En cours' },
  done: { icon: CheckCircle, color: 'text-green-400', bg: 'bg-green-400/10', label: 'Terminé' },
  failed: { icon: XCircle, color: 'text-red-400', bg: 'bg-red-400/10', label: 'Échoué' },
  cancelled: { icon: XCircle, color: 'text-gray-500', bg: 'bg-gray-500/10', label: 'Annulé' },
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

function timeAgo(dateStr) {
  if (!dateStr) return '';
  const diff = Date.now() - new Date(dateStr).getTime();
  const s = Math.floor(diff / 1000);
  if (s < 60) return 'à l\'instant';
  const m = Math.floor(s / 60);
  if (m < 60) return `il y a ${m}min`;
  const h = Math.floor(m / 60);
  if (h < 24) return `il y a ${h}h`;
  const d = Math.floor(h / 24);
  return `il y a ${d}j`;
}

function duration(start, end) {
  if (!start) return '';
  const s = ((end ? new Date(end) : new Date()) - new Date(start)) / 1000;
  if (s < 60) return `${Math.round(s)}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m${Math.round(s % 60)}s`;
  return `${Math.floor(s / 3600)}h${Math.floor((s % 3600) / 60)}m`;
}

export default function Tasks() {
  const [tasks, setTasks] = useState([]);
  const [total, setTotal] = useState(0);
  const [filter, setFilter] = useState('');
  const [offset, setOffset] = useState(0);
  const [loading, setLoading] = useState(true);
  const navigate = useNavigate();
  const limit = 30;

  useEffect(() => {
    setLoading(true);
    const params = new URLSearchParams({ limit, offset });
    if (filter) params.set('status', filter);
    fetch(`/api/tasks?${params}`)
      .then(r => r.json())
      .then(data => {
        if (data?.tasks) setTasks(data.tasks);
        if (data?.total != null) setTotal(data.total);
      })
      .catch(() => {})
      .finally(() => setLoading(false));
  }, [filter, offset]);

  return (
    <div className="p-6 max-w-5xl mx-auto">
      <div className="flex items-center gap-3 mb-6">
        <ListTodo className="w-6 h-6 text-blue-400" />
        <h1 className="text-2xl font-bold text-white">Activité</h1>
        <span className="text-sm text-gray-500">{total} tâches</span>
      </div>

      {/* Filters */}
      <div className="flex gap-2 mb-4">
        {['', 'running', 'done', 'failed'].map(f => (
          <button
            key={f}
            onClick={() => { setFilter(f); setOffset(0); }}
            className={`px-3 py-1.5 text-xs rounded-lg transition-colors ${
              filter === f
                ? 'bg-blue-500/20 text-blue-400 border border-blue-500/30'
                : 'bg-gray-800 text-gray-400 border border-gray-700 hover:bg-gray-700'
            }`}
          >
            {f === '' ? 'Toutes' : STATUS_CONFIG[f]?.label || f}
          </button>
        ))}
      </div>

      {/* Task list */}
      <div className="bg-gray-800 border border-gray-700 rounded-lg overflow-hidden">
        {loading ? (
          <div className="p-8 text-center">
            <Loader className="w-6 h-6 text-blue-400 animate-spin mx-auto" />
          </div>
        ) : tasks.length === 0 ? (
          <div className="p-8 text-center text-gray-500 text-sm">
            Aucune tâche trouvée
          </div>
        ) : (
          tasks.map(task => {
            const cfg = STATUS_CONFIG[task.status] || STATUS_CONFIG.pending;
            const Icon = cfg.icon;
            return (
              <button
                key={task.id}
                onClick={() => navigate(`/tasks/${task.id}`)}
                className="w-full text-left p-4 hover:bg-gray-700/50 transition-colors border-b border-gray-700/50 flex items-center gap-4"
              >
                <div className={`p-1.5 rounded ${cfg.bg}`}>
                  <Icon className={`w-5 h-5 ${cfg.color} ${cfg.spin ? 'animate-spin' : ''}`} />
                </div>
                <div className="flex-1 min-w-0">
                  <p className="text-sm font-medium text-gray-200">{task.title}</p>
                  <div className="flex items-center gap-2 mt-1">
                    <span className="text-xs text-gray-500">{TYPE_LABELS[task.task_type] || task.task_type}</span>
                    {task.target && (
                      <>
                        <span className="text-xs text-gray-600">·</span>
                        <span className="text-xs text-gray-500 truncate max-w-[200px]">{task.target}</span>
                      </>
                    )}
                  </div>
                </div>
                <div className="text-right flex-shrink-0">
                  <p className="text-xs text-gray-500">{timeAgo(task.created_at)}</p>
                  {task.started_at && (
                    <p className="text-xs text-gray-600 mt-0.5">
                      {duration(task.started_at, task.finished_at)}
                    </p>
                  )}
                </div>
                <ChevronRight className="w-4 h-4 text-gray-600 flex-shrink-0" />
              </button>
            );
          })
        )}
      </div>

      {/* Pagination */}
      {total > limit && (
        <div className="flex justify-center gap-2 mt-4">
          <button
            disabled={offset === 0}
            onClick={() => setOffset(Math.max(0, offset - limit))}
            className="px-3 py-1.5 text-xs bg-gray-800 text-gray-400 border border-gray-700 rounded-lg disabled:opacity-40 hover:bg-gray-700"
          >
            Précédent
          </button>
          <span className="px-3 py-1.5 text-xs text-gray-500">
            {offset + 1}-{Math.min(offset + limit, total)} / {total}
          </span>
          <button
            disabled={offset + limit >= total}
            onClick={() => setOffset(offset + limit)}
            className="px-3 py-1.5 text-xs bg-gray-800 text-gray-400 border border-gray-700 rounded-lg disabled:opacity-40 hover:bg-gray-700"
          >
            Suivant
          </button>
        </div>
      )}
    </div>
  );
}
