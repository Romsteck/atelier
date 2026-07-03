import { useState } from 'react';
import { AlertTriangle, Bot, ExternalLink, Info, Server, ShieldAlert, Trash2, User, XCircle } from 'lucide-react';
import { timeAgo } from '../../utils/formatters';
import { openStudio } from '../../lib/openStudio';
import { useNotifications } from '../../context/NotificationsContext';

const LEVEL_ICON = {
  info: { icon: Info, color: 'text-blue-600 dark:text-blue-400' },
  warn: { icon: AlertTriangle, color: 'text-yellow-600 dark:text-yellow-400' },
  error: { icon: XCircle, color: 'text-red-600 dark:text-red-400' },
};

const SOURCE_ICON = { agent: Bot, scan: ShieldAlert, system: Server, user: User };

// Une ligne du tiroir. Deux rendus :
//  - kind=notice : titre plein + point non-lu + body dépliable au clic (mark-read).
//  - kind=action : ligne journal compacte et grise (jamais de badge/notif système).
export default function NotificationItem({ n }) {
  const { markRead, remove, setIsOpen } = useNotifications();
  const [expanded, setExpanded] = useState(false);
  const isAction = n.kind === 'action';
  const lvl = LEVEL_ICON[n.level] || LEVEL_ICON.info;
  const LevelIcon = lvl.icon;
  const SourceIcon = SOURCE_ICON[n.source] || Server;
  const unread = !n.read_at;

  const onRowClick = () => {
    if (unread) markRead(n.id);
    if (n.body) setExpanded((e) => !e);
  };

  if (isAction) {
    return (
      <div className="group flex items-center gap-2 px-3 py-1.5 border-b border-gray-700/40 text-[12px] text-gray-500">
        <SourceIcon className="w-3.5 h-3.5 shrink-0 text-gray-600" />
        <span className="flex-1 truncate">{n.title}</span>
        <span className="shrink-0 text-[10px] text-gray-600">{timeAgo(n.ts)}</span>
        <button
          onClick={() => remove(n.id)}
          className="opacity-0 group-hover:opacity-100 p-0.5 text-gray-600 hover:text-red-400 shrink-0"
          title="Supprimer"
        >
          <Trash2 className="w-3.5 h-3.5" />
        </button>
      </div>
    );
  }

  return (
    <div
      className={`group border-b border-gray-700/50 px-3 py-2.5 cursor-pointer hover:bg-gray-700/40 transition-colors ${unread ? 'bg-gray-700/20' : ''}`}
      onClick={onRowClick}
    >
      <div className="flex items-start gap-2.5">
        <LevelIcon className={`w-4 h-4 mt-0.5 shrink-0 ${lvl.color}`} />
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2">
            {unread && <span className="w-2 h-2 rounded-full bg-amber-500 shrink-0" />}
            <p className={`text-sm truncate ${unread ? 'text-gray-100 font-medium' : 'text-gray-300'}`}>{n.title}</p>
          </div>
          <div className="flex items-center gap-2 mt-1 text-[10px] text-gray-500">
            <span className="flex items-center gap-1 px-1.5 py-0.5 rounded-sm bg-gray-700 text-gray-400">
              <SourceIcon className="w-3 h-3" />
              {n.source}
            </span>
            {n.slug && <span className="px-1.5 py-0.5 rounded-sm bg-gray-700 text-gray-400">{n.slug}</span>}
            <span>{timeAgo(n.ts)}</span>
          </div>
          {expanded && n.body && (
            <p className="mt-2 text-xs text-gray-400 whitespace-pre-wrap break-words">{n.body}</p>
          )}
        </div>
        <div className="flex items-center gap-1 shrink-0">
          {n.slug && (
            <button
              onClick={(e) => { e.stopPropagation(); if (unread) markRead(n.id); setIsOpen(false); openStudio(n.slug); }}
              className="opacity-0 group-hover:opacity-100 p-1 text-gray-500 hover:text-blue-400"
              title={`Ouvrir le Studio de ${n.slug}`}
            >
              <ExternalLink className="w-3.5 h-3.5" />
            </button>
          )}
          <button
            onClick={(e) => { e.stopPropagation(); remove(n.id); }}
            className="opacity-0 group-hover:opacity-100 p-1 text-gray-500 hover:text-red-400"
            title="Supprimer"
          >
            <Trash2 className="w-3.5 h-3.5" />
          </button>
        </div>
      </div>
    </div>
  );
}
