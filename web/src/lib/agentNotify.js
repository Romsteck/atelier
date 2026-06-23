// Notifications « réponse de l'agent prête » — couche client (Tier 1).
//  - Badging API : pastille (compteur de non-lus) sur l'icône de la PWA installée.
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

// Pastille de l'icône PWA = nombre de réponses non lues. n=0 efface.
// (Limite assumée : la Badging API est par-origine — plusieurs onglets Studio se
// partagent une seule pastille, dernier écrivain gagne. Le point « non lu » par
// onglet et la notif système restent, eux, par-conversation.)
export function updateBadge(n) {
  try {
    const p = n > 0 ? navigator.setAppBadge?.(n) : navigator.clearAppBadge?.();
    p?.catch?.(() => {});
  } catch {
    /* ignore */
  }
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
