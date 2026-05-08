import { useState, useEffect } from 'react';
import { Loader2, Circle, Clock } from 'lucide-react';
import useWebSocket from '../hooks/useWebSocket';
import api from '../api/client';

const STATUS_ORDER = ['in_progress', 'pending'];

const STATUS_META = {
  pending:     { label: 'Pending',     badge: 'bg-gray-600 text-gray-200',     Icon: Circle },
  in_progress: { label: 'In progress', badge: 'bg-blue-500/20 text-blue-300 border border-blue-500/30', Icon: Clock },
};

function TodoItem({ todo }) {
  const [expanded, setExpanded] = useState(false);
  const meta = STATUS_META[todo.status] || STATUS_META.pending;
  const Icon = meta.Icon;
  const desc = todo.description || '';
  const isLong = desc.length > 120;

  return (
    <div className="px-3 py-2 border-b border-gray-700/50 hover:bg-gray-700/20 transition-colors">
      <div className="flex items-start gap-2">
        <Icon className="w-3.5 h-3.5 mt-0.5 shrink-0 text-gray-400" />
        <div className="flex-1 min-w-0">
          <div className="text-[13px] font-medium text-gray-100 leading-snug break-words">{todo.name}</div>
          {desc && (
            <div
              className={`text-xs text-gray-400 mt-0.5 break-words ${isLong && !expanded ? 'line-clamp-2' : ''} ${isLong ? 'cursor-pointer' : ''}`}
              onClick={() => isLong && setExpanded(e => !e)}
              title={isLong ? (expanded ? 'Cliquer pour réduire' : 'Cliquer pour étendre') : undefined}
            >
              {desc}
            </div>
          )}
          <div className="flex items-center gap-2 mt-1.5 flex-wrap">
            <span className={`inline-block px-1.5 py-0.5 text-[10px] font-medium rounded ${meta.badge}`}>
              {meta.label}
            </span>
          </div>
        </div>
      </div>
    </div>
  );
}

export default function TodosPanel({ slug }) {
  const [todos, setTodos] = useState([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    if (!slug) return;
    let cancelled = false;
    setLoading(true);
    api.get(`/apps/${slug}/todos`)
      .then(res => {
        if (cancelled) return;
        const d = res.data?.data || res.data;
        const list = d?.todos || (Array.isArray(d) ? d : []);
        setTodos(Array.isArray(list) ? list : []);
      })
      .catch(() => { if (!cancelled) setTodos([]); })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [slug]);

  useWebSocket({
    'app:todos': (data) => {
      if (!data || data.slug !== slug) return;
      setTodos(Array.isArray(data.todos) ? data.todos : []);
    },
  });

  const grouped = STATUS_ORDER.map(s => ({
    status: s,
    items: todos.filter(t => t.status === s),
  })).filter(g => g.items.length > 0);

  return (
    <aside className="w-[300px] min-w-[300px] h-full bg-gray-800/30 border-l border-gray-700 flex flex-col">
      <div className="flex items-center justify-between px-3 py-2 border-b border-gray-700 shrink-0">
        <span className="text-[10px] font-semibold uppercase tracking-wider text-gray-500">Todos</span>
        <span className="text-[11px] text-gray-400 bg-gray-700/50 px-1.5 py-0.5 rounded">{todos.length}</span>
      </div>
      <div className="flex-1 overflow-y-auto">
        {loading ? (
          <div className="flex items-center justify-center py-8 text-gray-500">
            <Loader2 className="w-4 h-4 animate-spin" />
          </div>
        ) : grouped.length === 0 ? (
          <div className="text-center py-8 px-4 text-gray-500 text-xs">
            Aucun todo en cours.
            <div className="mt-1 text-gray-600 text-[11px]">Utilisez <code className="text-blue-400">todos.create</code> via MCP.</div>
          </div>
        ) : (
          grouped.map(group => (
            <div key={group.status}>
              <div className="px-3 pt-3 pb-1 text-[10px] font-semibold uppercase tracking-wider text-gray-500">
                {STATUS_META[group.status].label} <span className="text-gray-600">({group.items.length})</span>
              </div>
              {group.items.map(t => <TodoItem key={t.id} todo={t} />)}
            </div>
          ))
        )}
      </div>
    </aside>
  );
}
