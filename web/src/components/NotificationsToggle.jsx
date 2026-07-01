import { useState } from 'react';
import { Bell, BellRing, BellOff } from 'lucide-react';
import {
  notificationsSupported,
  notificationPermission,
  requestNotificationPermission,
} from '../lib/agentNotify';

// Opt-in (geste utilisateur requis) aux notifications « réponse de l'agent prête ».
// La permission est par-origine : l'accorder ici vaut pour la homepage ET le Studio.
//  - `compact` : bouton-icône (barre supérieure du Studio).
//  - sinon : ligne pleine largeur (footer de la Sidebar), avec repli rail (`collapsed`).
export default function NotificationsToggle({ collapsed, compact }) {
  const [perm, setPerm] = useState(() => notificationPermission());
  if (!notificationsSupported()) return null;

  const ask = async () => {
    if (perm !== 'default') return; // 'granted'/'denied' sont définitifs côté navigateur
    setPerm(await requestNotificationPermission());
  };

  const granted = perm === 'granted';
  const denied = perm === 'denied';
  const Icon = granted ? BellRing : denied ? BellOff : Bell;
  const title = granted
    ? "Notifications activées (réponses de l'agent)"
    : denied
    ? 'Notifications bloquées — autorise-les dans les réglages du navigateur'
    : "Activer les notifications de réponse de l'agent";

  if (compact) {
    return (
      <button
        onClick={ask}
        disabled={perm !== 'default'}
        title={title}
        aria-label={title}
        className={`p-2 sm:p-1.5 rounded-sm transition-colors ${
          granted ? 'text-emerald-400' : denied ? 'text-gray-600 cursor-not-allowed' : 'text-gray-400 hover:text-gray-100 hover:bg-gray-700'
        }`}
      >
        <Icon className="w-4 h-4" />
      </button>
    );
  }

  const railLabel = collapsed ? 'lg:hidden lg:group-hover/aside:block' : '';
  const railRow = collapsed ? 'lg:justify-center lg:group-hover/aside:justify-start' : '';
  const label = granted ? 'Notifications activées' : denied ? 'Notifications bloquées' : 'Activer les notifications';

  return (
    <button
      onClick={ask}
      disabled={perm !== 'default'}
      title={title}
      className={`flex items-center gap-2 w-full px-2 py-1.5 rounded-sm text-sm transition-[background-color,color] duration-300 ease-out hover:duration-0 ${
        granted ? 'text-emerald-700 dark:text-emerald-300' : denied ? 'text-gray-500 cursor-not-allowed' : 'text-gray-300 hover:text-gray-100 hover:bg-gray-700/50'
      } ${railRow}`}
    >
      <Icon className="w-4 h-4 shrink-0" />
      <span className={`whitespace-nowrap ${railLabel}`}>{label}</span>
    </button>
  );
}
