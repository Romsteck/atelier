import { createContext, useContext, useState, useEffect, useRef, useCallback } from 'react';
import useWebSocket from '../hooks/useWebSocket';
import {
  getNotifications,
  markNotificationRead,
  markAllNotificationsRead,
  deleteNotification,
  unwrapApi,
} from '../api/client';
import { setBadgeSlice, showPlatformNotification } from '../lib/notify';

// Notifications plateforme (canal agent → utilisateur) : notifications
// volontaires `notify_user` (kind=notice) + journal automatique des actions des
// agents (kind=action, né lu). Fetch initial + live via WS `notify:event`
// (created + mutations read/read_all/deleted broadcastées par le store — la
// cohérence multi-onglets vient du serveur, pas d'un état partagé). Re-sync
// autoritaire sur reconnexion (`epoch`) et sur `resync` (buffer WS dépassé) —
// même pattern qu'AgentConversationsContext.
const NotificationsContext = createContext(null);

// L'état {items, unread} est UN SEUL objet : chaque transition (optimiste OU
// écho WS) recalcule le delta d'unread d'après l'item réellement transformé —
// l'écho d'une mutation déjà appliquée en optimiste devient un no-op au lieu
// d'un double décompte. `unread` reste le compte SERVEUR global (il peut
// dépasser les 100 items fetchés) ; les deltas locaux le suivent entre refetch.
function applyRead(state, id, ts) {
  const idx = state.items.findIndex((n) => n.id === id);
  if (idx < 0 || state.items[idx].read_at) return state;
  const items = [...state.items];
  items[idx] = { ...items[idx], read_at: ts || new Date().toISOString() };
  return { items, unread: Math.max(0, state.unread - 1) };
}

function applyReadAll(state) {
  if (state.unread === 0 && state.items.every((n) => n.read_at)) return state;
  const now = new Date().toISOString();
  return { items: state.items.map((n) => (n.read_at ? n : { ...n, read_at: now })), unread: 0 };
}

function applyDelete(state, id) {
  const victim = state.items.find((n) => n.id === id);
  if (!victim) return state;
  return {
    items: state.items.filter((n) => n.id !== id),
    unread: victim.read_at ? state.unread : Math.max(0, state.unread - 1),
  };
}

function applyCreated(state, ev) {
  const exists = state.items.some((n) => n.id === ev.id);
  const items = exists
    ? state.items.map((n) => (n.id === ev.id ? ev : n))
    : [ev, ...state.items].slice(0, 200);
  return { items, unread: exists || ev.read_at ? state.unread : state.unread + 1 };
}

export function NotificationsProvider({ children }) {
  const [state, setState] = useState({ items: [], unread: 0 });
  const [isOpen, setIsOpen] = useState(false);

  const refetch = useCallback(() => {
    getNotifications({ limit: 100 })
      .then((res) => {
        const data = unwrapApi(res);
        if (data && Array.isArray(data.items)) {
          setState({ items: data.items, unread: data.unread ?? 0 });
        }
      })
      .catch(() => {});
  }, []);

  useEffect(() => { refetch(); }, [refetch]);

  // `?notif=1` posé par le clic sur une notification système sans slug
  // (sw.js `notificationclick`) : ouvre le tiroir puis nettoie l'URL.
  useEffect(() => {
    const url = new URL(window.location.href);
    if (url.searchParams.get('notif') === '1') {
      setIsOpen(true);
      url.searchParams.delete('notif');
      window.history.replaceState({}, '', url.pathname + url.search + url.hash);
    }
  }, []);

  const { epoch } = useWebSocket({
    'notify:event': (ev) => {
      if (!ev) return;
      switch (ev.action) {
        case 'created': {
          setState((s) => applyCreated(s, ev));
          // Notification système : seulement si l'onglet est caché, et pour ce
          // qui « signale » — un notice est volontaire (tous levels), une action
          // n'alerte que si warn/error (info = journal silencieux). Le tag par
          // id collapse les doublons multi-onglets côté OS.
          const signals = ev.kind === 'notice' || ev.level === 'warn' || ev.level === 'error';
          if (signals && document.visibilityState === 'hidden') {
            showPlatformNotification(ev);
          }
          break;
        }
        case 'read':
          setState((s) => applyRead(s, ev.id, ev.ts));
          break;
        case 'read_all':
          setState(applyReadAll);
          break;
        case 'deleted':
          setState((s) => applyDelete(s, ev.id));
          break;
        default:
          break;
      }
    },
    'resync': (m) => { if (m?.channel === 'notify:event') refetch(); },
  });

  // Reconnexion WS (coupure réseau, gel mobile) : le broadcast ne rejoue pas
  // l'historique → refetch autoritaire.
  const prevEpoch = useRef(0);
  useEffect(() => {
    if (epoch === 0 || epoch === prevEpoch.current) return;
    prevEpoch.current = epoch;
    refetch();
  }, [epoch, refetch]);

  // Tranche « notify » de la pastille PWA (agrégée avec la tranche « agent »).
  useEffect(() => { setBadgeSlice('notify', state.unread); }, [state.unread]);
  useEffect(() => () => setBadgeSlice('notify', 0), []);

  // Mutations optimistes — l'écho WS re-applique la même transition (no-op) ;
  // refetch autoritaire en cas d'échec HTTP.
  const markRead = useCallback((id) => {
    setState((s) => applyRead(s, id, null));
    markNotificationRead(id).catch(() => refetch());
  }, [refetch]);

  const markAllRead = useCallback(() => {
    setState(applyReadAll);
    markAllNotificationsRead().catch(() => refetch());
  }, [refetch]);

  const remove = useCallback((id) => {
    setState((s) => applyDelete(s, id));
    deleteNotification(id).catch(() => refetch());
  }, [refetch]);

  const value = {
    items: state.items,
    unread: state.unread,
    isOpen,
    setIsOpen,
    markRead,
    markAllRead,
    remove,
  };

  return (
    <NotificationsContext.Provider value={value}>
      {children}
    </NotificationsContext.Provider>
  );
}

export function useNotifications() {
  const ctx = useContext(NotificationsContext);
  if (!ctx) throw new Error('useNotifications must be used within NotificationsProvider');
  return ctx;
}
