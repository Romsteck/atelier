import { useTaskContext } from '../../context/TaskContext';
import { CheckCircle, XCircle, Loader, Clock, ChevronRight } from 'lucide-react';
import { useNavigate } from 'react-router-dom';
import { useEffect, useRef } from 'react';

const STATUS_CONFIG = {
  pending: { icon: Clock, color: 'text-gray-400', bg: 'bg-gray-400/10' },
  running: { icon: Loader, color: 'text-blue-400', bg: 'bg-blue-400/10', spin: true },
  done: { icon: CheckCircle, color: 'text-green-400', bg: 'bg-green-400/10' },
  failed: { icon: XCircle, color: 'text-red-400', bg: 'bg-red-400/10' },
  cancelled: { icon: XCircle, color: 'text-gray-500', bg: 'bg-gray-500/10' },
};

const TYPE_LABELS = {
  container_create: 'Conteneur',
  container_delete: 'Suppression',
  container_migrate: 'Migration',
  container_rename: 'Renommage',
  app_deploy: 'Déploiement',
  agent_update: 'Mise à jour agent',
  backup_trigger: 'Sauvegarde',
  git_sync: 'Git sync',
  acme_renew: 'Certificat',
  updates_check: 'Scan MAJ',
  updates_upgrade: 'MAJ système',
  dns_reload: 'DNS reload',
  proxy_reload: 'Proxy reload',
  host_power: 'Hôte',
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

function StatusIcon({ status }) {
  const cfg = STATUS_CONFIG[status] || STATUS_CONFIG.pending;
  const Icon = cfg.icon;
  return (
    <div className={`p-1 rounded ${cfg.bg}`}>
      <Icon className={`w-4 h-4 ${cfg.color} ${cfg.spin ? 'animate-spin' : ''}`} />
    </div>
  );
}

function TaskItem({ task, onClick }) {
  return (
    <button
      onClick={onClick}
      className="w-full text-left p-3 hover:bg-gray-700/50 transition-colors border-b border-gray-700/50 flex items-center gap-3"
    >
      <StatusIcon status={task.status} />
      <div className="flex-1 min-w-0">
        <p className="text-sm text-gray-200 truncate">{task.title}</p>
        <div className="flex items-center gap-2 mt-0.5">
          <span className="text-xs text-gray-500">
            {TYPE_LABELS[task.task_type] || task.task_type}
          </span>
          <span className="text-xs text-gray-600">·</span>
          <span className="text-xs text-gray-500">{timeAgo(task.created_at)}</span>
        </div>
      </div>
      <ChevronRight className="w-4 h-4 text-gray-600 flex-shrink-0" />
    </button>
  );
}

export default function TaskDropdown() {
  const { tasks, isOpen, setIsOpen, selectTask } = useTaskContext();
  const navigate = useNavigate();
  const ref = useRef(null);

  // Close on click outside
  useEffect(() => {
    if (!isOpen) return;
    function handleClick(e) {
      if (ref.current && !ref.current.contains(e.target)) {
        setIsOpen(false);
      }
    }
    document.addEventListener('mousedown', handleClick);
    return () => document.removeEventListener('mousedown', handleClick);
  }, [isOpen, setIsOpen]);

  if (!isOpen) return null;

  return (
    <div
      ref={ref}
      className="absolute right-4 top-14 w-96 bg-gray-800 border border-gray-700 rounded-lg shadow-xl z-50 max-h-[70vh] overflow-hidden flex flex-col"
    >
      <div className="p-3 border-b border-gray-700 flex justify-between items-center">
        <span className="text-sm font-medium text-gray-200">Activité</span>
        <button
          onClick={() => { setIsOpen(false); navigate('/tasks'); }}
          className="text-xs text-blue-400 hover:text-blue-300"
        >
          Voir tout
        </button>
      </div>
      <div className="overflow-y-auto flex-1">
        {tasks.length === 0 ? (
          <div className="p-8 text-center text-gray-500 text-sm">
            Aucune activité récente
          </div>
        ) : (
          tasks.map(task => (
            <TaskItem
              key={task.id}
              task={task}
              onClick={() => {
                setIsOpen(false);
                selectTask(task.id);
                navigate(`/tasks/${task.id}`);
              }}
            />
          ))
        )}
      </div>
    </div>
  );
}
