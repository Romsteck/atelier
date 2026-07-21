// Notifications côté client (Tier 1) — agent (« réponse prête ») ET plateforme
// (notify_user / journal d'actions, via le WS `notify:event`).
//  - Badging API : pastille (compteur de non-lus) sur l'icône de la PWA installée,
//    agrégée par tranches (`setBadgeSlice`) — plusieurs écrivains (conversations
//    agent, notifications plateforme) sans s'écraser mutuellement.
//  - Notification système via le service worker (registration.showNotification) :
//    s'affiche tant que la PWA est vivante en arrière-plan (Android / desktop).
//    iOS Safari/PWA ne supporte PAS ce chemin → no-op silencieux (cf. Web Push, P5).
//
// La permission DOIT être demandée depuis un geste utilisateur (NotificationsToggle).
// Elle est par-origine : l'accorder depuis la homepage vaut aussi pour le Studio.

export function notificationsSupported() {
  return typeof Notification !== 'undefined';
}

export function notificationPermission() {
  return notificationsSupported() ? Notification.permission : 'unsupported';
}

export async function requestNotificationPermission() {
  if (!notificationsSupported()) return 'unsupported';
  try {
    return await Notification.requestPermission();
  } catch {
    return 'denied';
  }
}

// Pastille PWA brute. n=0 efface. Préférer `setBadgeSlice` (agrégé) — exporté
// pour compat mais plus aucun call-site direct.
// (Limite assumée : la Badging API est par-origine — plusieurs onglets se
// partagent une seule pastille, dernier écrivain gagne. Les tranches étant
// identiques dans tous les onglets pour la part plateforme, ça converge.)
export function updateBadge(n) {
  try {
    const p = n > 0 ? navigator.setAppBadge?.(n) : navigator.clearAppBadge?.();
    p?.catch?.(() => {});
  } catch {
    /* ignore */
  }
}

// Agrégateur de pastille par tranche : chaque écrivain (`'agent'`, `'notify'`)
// pose SON compteur, la pastille affiche la somme. Évite que le compteur de
// conversations non-lues et celui des notifications plateforme s'écrasent.
const badgeSlices = new Map();
export function setBadgeSlice(name, n) {
  badgeSlices.set(name, Math.max(0, n | 0));
  let total = 0;
  for (const v of badgeSlices.values()) total += v;
  updateBadge(total);
}

// Notification système via le SW — silencieuse si permission non accordée / iOS.
export async function showAgentNotification({ slug, sid, title }) {
  if (notificationPermission() !== 'granted') return;
  try {
    const reg = await navigator.serviceWorker?.ready;
    await reg?.showNotification?.('Atelier — réponse prête', {
      body: title || "L'agent a terminé sa réponse.",
      icon: '/icon-192.png',
      badge: '/icon-192.png',
      tag: `agent:${slug}:${sid}`, // collapse les répétitions d'une même conversation
      renotify: true,
      data: { slug, sid }, // consommé par le handler `notificationclick` (sw.js)
    });
  } catch {
    /* ignore */
  }
}

// Notification système pour une notification PLATEFORME (`notify_user` d'un agent,
// event système). `tag` par id → chaque onglet ouvert reçoit le même WS event et
// appelle showNotification : le tag identique collapse nativement les doublons
// multi-onglets (renotify:false = pas de re-alerte).
export async function showPlatformNotification({ id, slug, source, level, title, body }) {
  if (notificationPermission() !== 'granted') return;
  try {
    const reg = await navigator.serviceWorker?.ready;
    const prefix = level === 'error' ? '⛔ ' : level === 'warn' ? '⚠️ ' : '';
    // Routage Pilote par `source` (le backend pose source='pilot' sur toutes les
    // notifs Pilote) ; le préfixe de titre reste en repli pour les entrées émises
    // avant ce contrat.
    const isPilot = source === 'pilot' || String(title || '').startsWith('Pilote —');
    await reg?.showNotification?.(`${prefix}Atelier${slug ? ` — ${slug}` : ''}`, {
      body: body ? `${title}\n${body}` : title,
      icon: '/icon-192.png',
      badge: '/icon-192.png',
      tag: `platform:${id}`,
      renotify: false,
      // `target` consommé par `notificationclick` (sw.js) : Studio de l'app si
      // slug, sinon homepage avec le tiroir notifications ouvert.
      data: { target: isPilot ? '/backlog?notif=1' : (slug ? `/studio/${slug}?tab=code` : '/?notif=1') },
    });
  } catch {
    /* ignore */
  }
}
