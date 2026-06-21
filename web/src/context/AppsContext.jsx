import { createContext, useContext, useState, useCallback, useEffect, useMemo } from 'react';
import { listApps, controlApp } from '../api/client';
import useWebSocket from '../hooks/useWebSocket';
import { getRecentSlugs, RECENT_APPS_KEY, RECENT_APPS_EVENT } from '../lib/recentApps';

// État partagé de la homepage : liste des apps (live via WS `app:state`), apps
// récemment ouvertes (pour le sous-menu de la Sidebar) et action start/stop/restart.
// Remplace l'ancien `StudioContext` (qui portait l'app courante per-app) : depuis
// que le Studio est une app séparée en onglet, la homepage ne connaît plus d'« app
// courante », seulement la liste + les récentes.
const AppsContext = createContext(null);

const FALLBACK = { apps: [], loading: false, recentApps: [], control: () => {}, reload: () => {} };

export function AppsProvider({ children }) {
  const [apps, setApps] = useState([]);
  const [loading, setLoading] = useState(true);
  const [recentSlugs, setRecentSlugs] = useState(() => getRecentSlugs());

  const reload = useCallback(async () => {
    try {
      const res = await listApps();
      const d = res.data?.data || res.data;
      const list = d?.apps || (Array.isArray(d) ? d : []);
      setApps(Array.isArray(list) ? list : []);
    } catch {
      /* garde la dernière liste connue */
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => { reload(); }, [reload]);

  // Statuts en direct (start/stop/crash) sans polling.
  useWebSocket({
    'app:state': (data) => {
      setApps((prev) =>
        prev.map((a) => (a.slug === data.slug ? { ...a, state: data.state, port: data.port || a.port } : a)),
      );
    },
  });

  // Récentes fraîches que la mise à jour vienne de cet onglet (event custom, cf.
  // recentApps.js) ou d'un autre onglet (event `storage` natif).
  useEffect(() => {
    const refresh = () => setRecentSlugs(getRecentSlugs());
    const onStorage = (e) => { if (!e.key || e.key === RECENT_APPS_KEY) refresh(); };
    window.addEventListener(RECENT_APPS_EVENT, refresh);
    window.addEventListener('storage', onStorage);
    return () => {
      window.removeEventListener(RECENT_APPS_EVENT, refresh);
      window.removeEventListener('storage', onStorage);
    };
  }, []);

  const control = useCallback(async (slug, action) => {
    try { await controlApp(slug, action); } catch { /* l'erreur remonte via WS / UI */ }
  }, []);

  const recentApps = useMemo(
    () => recentSlugs.map((s) => apps.find((a) => a.slug === s)).filter(Boolean).slice(0, 4),
    [recentSlugs, apps],
  );

  const value = useMemo(
    () => ({ apps, loading, recentApps, control, reload }),
    [apps, loading, recentApps, control, reload],
  );

  return <AppsContext.Provider value={value}>{children}</AppsContext.Provider>;
}

export function useApps() {
  return useContext(AppsContext) || FALLBACK;
}
