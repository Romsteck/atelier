import { useEffect, useRef, useState } from 'react';

/**
 * Connexion au WebSocket Atelier (`/api/ws`), résiliente au backgrounding mobile.
 *
 * API rétro-compatible : `useWebSocket(handlers)` où `handlers` mappe un type
 * d'event (`msg.type`) à un callback. Le retour est enrichi (`{ status, epoch }`)
 * — `epoch` s'incrémente à CHAQUE reconnexion (au-delà de la 1re connexion), ce
 * qui permet à un consommateur (ex. AgentConversationsContext) de re-synchroniser
 * son état après une coupure (le canal broadcast serveur ne rejoue pas l'historique).
 *
 * Résilience :
 *  - backoff exponentiel plafonné (1s·2ⁿ, max 30s) + jitter ;
 *  - `visibilitychange` : au retour au premier plan, reconnecte immédiatement si le
 *    socket n'est pas OPEN (les mobiles gèlent le socket SANS émettre `onclose`) ;
 *  - `online`/`offline` : reconnecte au retour réseau ;
 *  - anti-tempête : aucune tentative de reconnexion tant que l'onglet est caché ou
 *    hors-ligne — la reprise est pilotée par les events `visible`/`online`.
 */
export default function useWebSocket(handlers, opts = {}) {
  const handlersRef = useRef(handlers);
  handlersRef.current = handlers;
  const onReconnectRef = useRef(opts.onReconnect);
  onReconnectRef.current = opts.onReconnect;

  const wsRef = useRef(null);
  const [status, setStatus] = useState('connecting'); // 'connecting' | 'open' | 'closed'
  const [epoch, setEpoch] = useState(0); // ++ à chaque (re)connexion après la 1re

  useEffect(() => {
    const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const url = `${proto}//${window.location.host}/api/ws`;

    let alive = true;
    let attempt = 0;          // exposant du backoff
    let openedOnce = false;   // distingue 1re ouverture des reconnexions
    let timer = null;

    const clearTimer = () => { if (timer) { clearTimeout(timer); timer = null; } };

    const scheduleReconnect = () => {
      if (!alive) return;
      // Anti-tempête : on ne boucle pas pendant que l'onglet est caché / hors-ligne.
      // La reprise sera déclenchée par `visibilitychange` (visible) ou `online`.
      if (document.visibilityState === 'hidden') return;
      if (navigator.onLine === false) return;
      const delay = Math.min(30000, 1000 * 2 ** attempt) + Math.random() * 250;
      attempt += 1;
      clearTimer();
      timer = setTimeout(connect, delay);
    };

    function connect() {
      clearTimer();
      if (!alive) return;
      // Évite les sockets en double (visibilitychange + online peuvent coïncider).
      const cur = wsRef.current;
      if (cur && (cur.readyState === WebSocket.OPEN || cur.readyState === WebSocket.CONNECTING)) return;

      setStatus('connecting');
      const ws = new WebSocket(url);
      wsRef.current = ws;

      ws.onopen = () => {
        attempt = 0;
        setStatus('open');
        if (openedOnce) {
          setEpoch((e) => e + 1);      // signal de reconnexion pour les consommateurs
          onReconnectRef.current?.();
        }
        openedOnce = true;
      };
      ws.onmessage = (e) => {
        try {
          const msg = JSON.parse(e.data);
          // `resync` (subscriber broadcast laggé côté serveur : events perdus) porte
          // son payload À PLAT ({channel, dropped}), pas sous `data` → on délivre le
          // message entier pour que le consommateur puisse router par `channel`.
          if (msg.type === 'resync') handlersRef.current.resync?.(msg);
          else handlersRef.current[msg.type]?.(msg.data);
        } catch {
          // ignore parse errors
        }
      };
      ws.onclose = () => { setStatus('closed'); scheduleReconnect(); };
      ws.onerror = () => { try { ws.close(); } catch { /* ignore */ } };
    }

    // Mobile : le socket peut être gelé sans `onclose` → on force la reprise au retour.
    const onVisible = () => {
      if (document.visibilityState !== 'visible') return;
      const ws = wsRef.current;
      if (!ws || ws.readyState === WebSocket.CLOSED || ws.readyState === WebSocket.CLOSING) {
        attempt = 0; connect();
      }
    };
    const onOnline = () => { attempt = 0; connect(); };
    const onOffline = () => { setStatus('closed'); };

    document.addEventListener('visibilitychange', onVisible);
    window.addEventListener('online', onOnline);
    window.addEventListener('offline', onOffline);

    connect();

    return () => {
      alive = false;
      clearTimer();
      document.removeEventListener('visibilitychange', onVisible);
      window.removeEventListener('online', onOnline);
      window.removeEventListener('offline', onOffline);
      try { wsRef.current?.close(); } catch { /* ignore */ }
    };
  }, []);

  return { wsRef, status, epoch };
}
