import { useState } from 'react';
import useWebSocket from './useWebSocket';

// Suit les apps en cours de build à partir du canal `app:build` du WebSocket.
// Renvoie un Set de slugs actuellement en build (status `started`/`step`) ; un
// slug en sort sur `finished`/`error`. Permet d'afficher un indicateur de build
// AILLEURS que dans le header Studio (ex. le point d'état de la sidebar), pour
// qu'on voie qu'une app build même sans être focalisé dessus.
export default function useBuildingApps() {
  const [building, setBuilding] = useState(() => new Set());

  useWebSocket({
    'app:build': (data) => {
      const slug = data?.slug;
      if (!slug) return;
      setBuilding((prev) => {
        const inProgress = data.status === 'started' || data.status === 'step';
        if (inProgress === prev.has(slug)) return prev; // pas de changement → pas de re-render
        const next = new Set(prev);
        if (inProgress) next.add(slug);
        else next.delete(slug); // finished | error → fin de build
        return next;
      });
    },
  });

  return building;
}
