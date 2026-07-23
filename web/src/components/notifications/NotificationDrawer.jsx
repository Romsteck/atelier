import { useEffect, useRef, useState } from 'react';
import { CheckCheck, X } from 'lucide-react';
import { useNotifications } from '../../context/NotificationsContext';
import { useIsPhone } from '../../hooks/useMediaQuery';
import NotificationItem from './NotificationItem';
import Button from '../Button';

const FILTERS = [
  { key: 'all', label: 'Tout' },
  { key: 'notice', label: 'Notices' },
  { key: 'action', label: 'Journal' },
];

// Tiroir des notifications plateforme. Desktop : dropdown ancré à la cloche ;
// mobile : drawer plein-hauteur droit (pattern Sidebar).
// `contextSlug` (Studio) ajoute un filtre « cette app » pré-activable.
export default function NotificationDrawer({ contextSlug }) {
  const { items, unread, isOpen, setIsOpen, markAllRead } = useNotifications();
  const isPhone = useIsPhone();
  const ref = useRef(null);
  const [filter, setFilter] = useState('all');
  const [unreadOnly, setUnreadOnly] = useState(false);
  const [thisAppOnly, setThisAppOnly] = useState(false);

  // Close on click outside (desktop dropdown seulement — le mobile a un backdrop).
  useEffect(() => {
    if (!isOpen || isPhone) return;
    function handleClick(e) {
      if (ref.current && !ref.current.contains(e.target)) setIsOpen(false);
    }
    document.addEventListener('mousedown', handleClick);
    return () => document.removeEventListener('mousedown', handleClick);
  }, [isOpen, isPhone, setIsOpen]);

  if (!isOpen) return null;

  const visible = items.filter((n) => {
    if (filter !== 'all' && n.kind !== filter) return false;
    if (unreadOnly && n.read_at) return false;
    if (thisAppOnly && contextSlug && n.slug !== contextSlug) return false;
    return true;
  });

  const chip = (active) =>
    `px-2 py-1 rounded-sm text-[11px] transition-colors ${
      active ? 'bg-amber-600 text-white' : 'bg-gray-800 text-gray-400 hover:text-gray-200'
    }`;

  const body = (
    <>
      <div className="p-3 border-b border-gray-700 flex justify-between items-center shrink-0">
        <span className="text-sm font-medium text-gray-200">
          Notifications{unread > 0 && <span className="ml-2 text-xs text-amber-500">{unread} non lue{unread > 1 ? 's' : ''}</span>}
        </span>
        <div className="flex items-center gap-1">
          <Button
            onClick={markAllRead}
            variant="ghost"
            size="xs"
            icon={CheckCheck}
            title="Tout marquer lu"
          >
            <span className="hidden sm:inline">Tout lu</span>
          </Button>
          {isPhone && (
            <button onClick={() => setIsOpen(false)} className="p-1.5 text-gray-400 hover:text-gray-50" title="Fermer">
              <X className="w-4 h-4" />
            </button>
          )}
        </div>
      </div>
      <div className="px-3 py-2 border-b border-gray-700/60 flex items-center gap-1.5 flex-wrap shrink-0">
        {FILTERS.map((f) => (
          <button key={f.key} onClick={() => setFilter(f.key)} className={chip(filter === f.key)}>
            {f.label}
          </button>
        ))}
        <span className="w-px h-4 bg-gray-700 mx-0.5" />
        <button onClick={() => setUnreadOnly((v) => !v)} className={chip(unreadOnly)}>
          Non-lus
        </button>
        {contextSlug && (
          <button onClick={() => setThisAppOnly((v) => !v)} className={chip(thisAppOnly)}>
            {contextSlug}
          </button>
        )}
      </div>
      <div className="overflow-y-auto flex-1">
        {visible.length === 0 ? (
          <div className="p-8 text-center text-gray-500 text-sm">Aucune notification</div>
        ) : (
          visible.map((n) => <NotificationItem key={n.id} n={n} />)
        )}
      </div>
    </>
  );

  if (isPhone) {
    return (
      <>
        <div className="fixed inset-0 bg-black/60 z-40" onClick={() => setIsOpen(false)} />
        <div className="fixed inset-y-0 right-0 w-full max-w-sm z-50 bg-gray-800 border-l border-gray-700 shadow-xl flex flex-col">
          {body}
        </div>
      </>
    );
  }

  return (
    <div
      ref={ref}
      className="absolute right-0 top-12 w-96 bg-gray-800 border border-gray-700 rounded-lg shadow-xl z-50 max-h-[70vh] overflow-hidden flex flex-col"
    >
      {body}
    </div>
  );
}
