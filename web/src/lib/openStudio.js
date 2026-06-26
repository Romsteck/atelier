import { pushRecentSlug } from './recentApps';
import { setStudioTab } from '../api/client';

// Cache localStorage de l'onglet Studio par app (clé par slug). C'est un cache de
// rendu (graine instantanée, anti-flash) ; la SOURCE DE VÉRITÉ est le backend
// (`agent_open_tabs.studio_tab`, cf. lib/openStudio + StudioShell).
export const studioTabCacheKey = (slug) => `studio:tab:${slug}`;

export function writeStudioTabCache(slug, tab, kind) {
  if (!slug || !tab) return;
  try {
    localStorage.setItem(studioTabCacheKey(slug), JSON.stringify({ tab, kind: kind || null }));
  } catch { /* noop */ }
}

export function readStudioTabCache(slug) {
  try {
    const raw = localStorage.getItem(studioTabCacheKey(slug));
    return raw ? JSON.parse(raw) : null;
  } catch { return null; }
}

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

// Ouvre le Studio d'une app, en ciblant un onglet (tab) + sous-scan (kind) — SANS
// paramètre d'URL. Le ciblage passe par le BACKEND :
//   1. on PUT l'onglet voulu (source de vérité serveur, broadcast WS `studio:tab`) ;
//   2. on écrit le cache localStorage (graine instantanée pour un NOUVEL onglet) ;
//   3. on ouvre l'URL propre `/studio/<slug>`.
// Onglet Studio DÉJÀ ouvert → il reçoit le broadcast WS et bascule en direct (sa
// connexion WS est déjà établie : aucun rechargement, aucune astuce cross-tab).
// Onglet neuf / rechargement → il lit le backend (et le cache) au montage.
// WHY le PUT AVANT le window.open : le write part immédiatement (course gagnée
// contre le chargement de page du nouvel onglet, bien plus lent qu'un upsert DB) ;
// et il N'EST PAS `await` (sinon on perdrait le « user gesture » → popup bloquée).
//
// `tab` défaut = 'preview' : ouvrir une app depuis la homepage (galerie/sidebar,
// sans tab explicite) atterrit sur l'aperçu de l'app. Les deep-links explicites
// (surveillance) passent leur propre tab.
export function openStudio(slug, { tab = 'preview', kind } = {}) {
  if (!slug) return;
  pushRecentSlug(slug); // alimente « apps récentes » + galerie (avant toute navigation)
  if (tab) {
    writeStudioTabCache(slug, tab, kind);
    setStudioTab(slug, { tab, kind: kind || null }).catch(() => {});
  }
  const url = `/studio/${slug}`;
  if (isStandalone()) {
    window.location.assign(url); // même fenêtre → on ne sort pas de la PWA
    return;
  }
  const win = window.open(url, `atelier-studio-${slug}`);
  win?.focus();
}
