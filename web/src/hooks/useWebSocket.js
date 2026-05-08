import { useEffect, useRef, useCallback } from 'react';

/**
 * Hook to connect to the HomeRoute WebSocket and listen for events.
 * @param {Object<string, function>} handlers - Map of event type to handler function
 *   e.g. { 'servers:status': (data) => { ... }, 'updates:started': () => { ... } }
 */
export default function useWebSocket(handlers) {
  const wsRef = useRef(null);
  const handlersRef = useRef(handlers);
  handlersRef.current = handlers;

  useEffect(() => {
    const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const url = `${proto}//${window.location.host}/api/ws`;

    let ws;
    let reconnectTimer;

    function connect() {
      ws = new WebSocket(url);
      wsRef.current = ws;

      ws.onmessage = (e) => {
        try {
          const msg = JSON.parse(e.data);
          const handler = handlersRef.current[msg.type];
          if (handler) {
            handler(msg.data);
          }
        } catch (err) {
          // ignore parse errors
        }
      };

      ws.onclose = () => {
        // Reconnect after 3 seconds
        reconnectTimer = setTimeout(connect, 3000);
      };

      ws.onerror = () => {
        ws.close();
      };
    }

    connect();

    return () => {
      clearTimeout(reconnectTimer);
      if (wsRef.current) {
        wsRef.current.close();
      }
    };
  }, []);

  return wsRef;
}
