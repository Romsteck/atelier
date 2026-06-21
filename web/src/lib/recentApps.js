// Liste MRU des apps récemment ouvertes (slugs), partagée homepage ↔ Studio via
// localStorage. WHY un event custom EN PLUS de `storage` : l'event `storage` du
// navigateur ne se déclenche QUE dans les AUTRES onglets, jamais celui qui écrit.
// `openStudio` (homepage) écrit puis dispatch `atelier:recent-apps` → la galerie
// du même onglet se rafraîchit aussi.
const KEY = 'studio:recentApps';
const EVENT = 'atelier:recent-apps';
const MAX = 8;

export function getRecentSlugs() {
  try {
    const v = JSON.parse(localStorage.getItem(KEY));
    return Array.isArray(v) ? v : [];
  } catch {
    return [];
  }
}

export function pushRecentSlug(slug) {
  if (!slug) return;
  const next = [slug, ...getRecentSlugs().filter((s) => s !== slug)].slice(0, MAX);
  try {
    localStorage.setItem(KEY, JSON.stringify(next));
    window.dispatchEvent(new Event(EVENT)); // listeners du même onglet
  } catch {
    /* ignore (quota / private mode) */
  }
}

export const RECENT_APPS_KEY = KEY;
export const RECENT_APPS_EVENT = EVENT;
