import { pushRecentSlug } from './recentApps';

// Vrai quand on tourne dans une PWA installée (fenêtre standalone). Là, un
// `window.open` n'ouvre PAS un onglet mais une Custom Tab du navigateur (avec sa
// barre d'URL disgracieuse) — car une PWA est mono-fenêtre. On détecte ce cas
// pour naviguer la MÊME fenêtre à la place et rester dans la PWA.
function isStandalone() {
  return (
    window.matchMedia?.('(display-mode: standalone)').matches ||
    window.navigator.standalone === true // iOS Safari (non standard)
  );
}

// Ouvre le Studio d'une app.
//  - PWA installée (standalone) → navigation MÊME fenêtre : reste dans la PWA, pas
//    de Custom Tab. Le bouton retour (et « ← Atelier ») ramène à la homepage.
//  - Navigateur classique → onglet dédié nommé `atelier-studio-<slug>` (recliquer la
//    même app refocus l'onglet existant au lieu d'ouvrir un doublon).
// Le Studio est une app Vite séparée (servie sous `/studio/<slug>`) : on passe l'onglet
// + le kind par l'URL (un window.open / une navigation ne transporte pas le state du router).
export function openStudio(slug, { tab, kind } = {}) {
  if (!slug) return;
  const params = new URLSearchParams();
  if (tab) params.set('tab', tab);
  if (kind) params.set('kind', kind);
  const qs = params.toString();
  const url = `/studio/${slug}${qs ? `?${qs}` : ''}`;
  pushRecentSlug(slug); // alimente « apps récentes » + galerie (avant toute navigation)
  if (isStandalone()) {
    window.location.assign(url); // même fenêtre → on ne sort pas de la PWA
    return;
  }
  const win = window.open(url, `atelier-studio-${slug}`);
  win?.focus();
}
