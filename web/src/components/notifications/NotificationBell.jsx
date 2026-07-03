import { Inbox } from 'lucide-react';
import { useNotifications } from '../../context/NotificationsContext';

// Cloche des notifications plateforme (distincte de TaskBell/Bell : icône Inbox).
// Badge = non-lus serveur (les kind=action naissent lus → seuls les notices comptent).
export default function NotificationBell() {
  const { unread, isOpen, setIsOpen } = useNotifications();

  return (
    <button
      onClick={() => setIsOpen(!isOpen)}
      className="relative p-2 text-gray-400 hover:text-gray-50 transition-colors"
      title="Notifications"
    >
      <Inbox className="w-5 h-5" />
      {unread > 0 && (
        <span className="absolute -top-0.5 -right-0.5 bg-amber-500 text-white text-[10px] font-bold rounded-full min-w-[18px] h-[18px] flex items-center justify-center px-1">
          {unread > 99 ? '99+' : unread}
        </span>
      )}
    </button>
  );
}
