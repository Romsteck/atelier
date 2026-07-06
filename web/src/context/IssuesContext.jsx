import { createContext, useContext, useState, useEffect, useRef, useCallback, useMemo } from 'react';
import useWebSocket from '../hooks/useWebSocket';
import { getIssues, patchIssue, deleteIssue, unwrapApi } from '../api/client';

// Remontées plateforme (canal app → dev Atelier) : erreurs / limitations /
// suggestions signalées par les agents des apps via `issue_report`. Fetch
// initial (liste complète — volumes minuscules, filtres client-side) + live via
// WS `issue:event` (created/updated transportent l'entrée complète, deleted
// l'id). Re-sync autoritaire sur reconnexion (`epoch`) et sur `resync` — même
// pattern que NotificationsContext.
const IssuesContext = createContext(null);

function applyCreated(items, ev) {
  if (!ev.item) return items;
  const exists = items.some((it) => it.id === ev.id);
  return exists ? items.map((it) => (it.id === ev.id ? ev.item : it)) : [ev.item, ...items];
}

function applyUpdated(items, ev) {
  if (!ev.item) return items;
  return items.map((it) => (it.id === ev.id ? ev.item : it));
}

function applyDeleted(items, id) {
  return items.filter((it) => it.id !== id);
}

export function IssuesProvider({ children }) {
  const [items, setItems] = useState([]);

  const refetch = useCallback(() => {
    getIssues()
      .then((res) => {
        const data = unwrapApi(res);
        if (Array.isArray(data)) setItems(data);
      })
      .catch(() => {});
  }, []);

  useEffect(() => { refetch(); }, [refetch]);

  const { epoch } = useWebSocket({
    'issue:event': (ev) => {
      if (!ev) return;
      switch (ev.action) {
        case 'created':
          setItems((s) => applyCreated(s, ev));
          break;
        case 'updated':
          setItems((s) => applyUpdated(s, ev));
          break;
        case 'deleted':
          setItems((s) => applyDeleted(s, ev.id));
          break;
        default:
          break;
      }
    },
    'resync': (m) => { if (m?.channel === 'issue:event') refetch(); },
  });

  // Reconnexion WS (coupure réseau, gel mobile) : le broadcast ne rejoue pas
  // l'historique → refetch autoritaire.
  const prevEpoch = useRef(0);
  useEffect(() => {
    if (epoch === 0 || epoch === prevEpoch.current) return;
    prevEpoch.current = epoch;
    refetch();
  }, [epoch, refetch]);

  // Counts pour la pastille sidebar : les suggestions ouvertes ne doivent pas
  // alarmer en rouge — elles ont leur propre compte (pastille bleue).
  const counts = useMemo(() => {
    let openAlerts = 0;
    let openSuggestions = 0;
    for (const it of items) {
      if (it.status !== 'open') continue;
      if (it.kind === 'suggestion') openSuggestions += 1;
      else openAlerts += 1;
    }
    return { openAlerts, openSuggestions };
  }, [items]);

  // Mutations optimistes — l'écho WS `updated`/`deleted` (autoritaire) re-passe
  // derrière ; refetch en cas d'échec HTTP.
  const patch = useCallback((id, body) => {
    setItems((s) => s.map((it) => (it.id === id ? { ...it, ...body } : it)));
    return patchIssue(id, body).catch(() => refetch());
  }, [refetch]);

  const remove = useCallback((id) => {
    setItems((s) => applyDeleted(s, id));
    return deleteIssue(id).catch(() => refetch());
  }, [refetch]);

  const value = { items, counts, patch, remove, refetch };

  return (
    <IssuesContext.Provider value={value}>
      {children}
    </IssuesContext.Provider>
  );
}

export function useIssues() {
  const ctx = useContext(IssuesContext);
  if (!ctx) throw new Error('useIssues must be used within IssuesProvider');
  return ctx;
}
