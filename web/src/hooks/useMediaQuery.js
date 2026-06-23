import { useEffect, useState } from 'react';

// Hook matchMedia générique. SPA client-only : on lit l'état initial de façon
// synchrone (pas de flash) et on s'abonne à `change` — qui ne tire qu'au
// franchissement du seuil, contrairement à un listener `resize` (plus économe,
// et exact en devtools responsive / rotation).
export function useMediaQuery(query) {
  const [matches, setMatches] = useState(
    () => typeof window !== 'undefined' && window.matchMedia(query).matches,
  );
  useEffect(() => {
    const mql = window.matchMedia(query);
    const onChange = (e) => setMatches(e.matches);
    setMatches(mql.matches); // resync si `query` change
    mql.addEventListener('change', onChange);
    return () => mql.removeEventListener('change', onChange);
  }, [query]);
  return matches;
}

// Seuils alignés sur Tailwind v4 (lg=1024, md=768). `useIsNarrow` remplace le
// `window.innerWidth < 900` ad-hoc du StudioShell : sous `lg`, on bascule les
// layouts multi-volets (Studio split, sidebars) en mono-volet / drawer.
export const useIsNarrow = () => useMediaQuery('(max-width: 1023px)'); // < lg
export const useIsPhone = () => useMediaQuery('(max-width: 767px)'); // < md
