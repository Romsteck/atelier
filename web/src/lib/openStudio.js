import { pushRecentSlug } from './recentApps';

// Ouvre le Studio d'une app dans un NOUVEL onglet navigateur, focus dessus.
// Le Studio est une app Vite séparée (servie sous `/studio/<slug>`) — il faut
// donc passer l'app + l'onglet/kind par l'URL (un `window.open` ne transporte
// pas le `state` du router). Le `target` nommé `atelier-studio-<slug>` fait que
// recliquer la MÊME app REUTILISE/refocus l'onglet existant au lieu d'en ouvrir
// un doublon.
export function openStudio(slug, { tab, kind } = {}) {
  if (!slug) return;
  const params = new URLSearchParams();
  if (tab) params.set('tab', tab);
  if (kind) params.set('kind', kind);
  const qs = params.toString();
  const url = `/studio/${slug}${qs ? `?${qs}` : ''}`;
  const win = window.open(url, `atelier-studio-${slug}`);
  win?.focus();
  pushRecentSlug(slug); // alimente le sous-menu « apps récentes » + la galerie
}
