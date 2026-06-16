import { useState, useEffect, useCallback, useRef } from 'react';
import { getSourceGitStatus } from '../api/client';
import useWebSocket from './useWebSocket';

// Statut git du working tree (`…/{slug}/src`) PARTAGÉ par le badge de la barre
// d'activité et l'onglet Git. Rafraîchi automatiquement SANS polling (convention
// projet « live = WebSocket ») :
//   - au montage / changement d'app ;
//   - à la fin d'un tour de l'agent (qui édite les fichiers) via le canal WS
//     `agent:event` (debounce pour coalescer les rafales) ;
//   - au retour de focus / visibilité de la fenêtre (édition via code-server).
// Le spinner (`loading`) ne s'affiche qu'au PREMIER chargement par app : les
// rafraîchissements de fond échangent les données en place, sans clignotement.
export default function useSourceGit(slug) {
  const [status, setStatus] = useState(null);
  const [loading, setLoading] = useState(true);
  const loadedOnce = useRef(false);
  const debounce = useRef(null);

  const refresh = useCallback(() => {
    if (!slug) return;
    if (!loadedOnce.current) setLoading(true);
    getSourceGitStatus(slug)
      .then((r) => setStatus(r.data))
      .catch((e) => setStatus({ error: e.response?.data?.error || 'Erreur git status' }))
      .finally(() => {
        loadedOnce.current = true;
        setLoading(false);
      });
  }, [slug]);

  const refreshSoon = useCallback(() => {
    clearTimeout(debounce.current);
    debounce.current = setTimeout(refresh, 600);
  }, [refresh]);

  // Reset du flag « déjà chargé » à chaque app → spinner sur la nouvelle app.
  useEffect(() => {
    loadedOnce.current = false;
    refresh();
    return () => clearTimeout(debounce.current);
  }, [refresh]);

  // L'agent modifie les fichiers → on resynchronise quand un tour se termine.
  useWebSocket({
    'agent:event': (d) => {
      if (d && (d.kind === 'turn_done' || d.kind === 'done' || d.kind === 'result')) refreshSoon();
    },
  });

  // Édition hors-agent (code-server) : on resynchronise au retour sur l'onglet.
  useEffect(() => {
    const onFocus = () => refresh();
    const onVis = () => { if (document.visibilityState === 'visible') refresh(); };
    window.addEventListener('focus', onFocus);
    document.addEventListener('visibilitychange', onVis);
    return () => {
      window.removeEventListener('focus', onFocus);
      document.removeEventListener('visibilitychange', onVis);
    };
  }, [refresh]);

  const count = Array.isArray(status?.files) ? status.files.length : 0;
  return { status, loading, refresh, count };
}
