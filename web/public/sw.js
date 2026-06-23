const CACHE_NAME = 'atelier-v5';
const STATIC_ASSETS = [
  '/manifest.json',
  '/favicon.svg',
  '/icon-192x192.svg',
  '/icon-512x512.svg',
  '/icon-192.png',
  '/icon-512.png',
  '/icon-maskable-512.png',
  '/apple-touch-icon.png'
];

self.addEventListener('install', (event) => {
  event.waitUntil(
    caches.open(CACHE_NAME).then((cache) => cache.addAll(STATIC_ASSETS))
  );
  self.skipWaiting();
});

self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches.keys().then((keys) =>
      Promise.all(keys.filter((k) => k !== CACHE_NAME).map((k) => caches.delete(k)))
    )
  );
  self.clients.claim();
});

self.addEventListener('fetch', (event) => {
  const { request } = event;

  // On NE TOUCHE PAS aux navigations (documents) : elles vont droit au réseau,
  // comme un onglet navigateur classique. Les intercepter pouvait produire une
  // PAGE BLANCHE — `fetch(request).catch(() => caches.match(request))` renvoyait
  // `undefined` quand le réseau hoquetait (PWA standalone, auth edge, changement
  // de build /studio/*) car la navigation n'est jamais en cache → respondWith(undefined).
  // Idem API/socket.io : aucun bénéfice à les passer par le SW.
  if (request.mode === 'navigate' || request.method !== 'GET') return;

  const url = new URL(request.url);
  if (url.pathname.startsWith('/api') || url.pathname.startsWith('/socket.io')) return;

  // JS/CSS hashés : network-first (le hash gère le versionnage), repli cache offline.
  // Couvre la homepage (/assets/) ET l'app Studio séparée (/studio/assets/).
  if (url.pathname.startsWith('/assets/') || url.pathname.startsWith('/studio/assets/')) {
    event.respondWith(
      fetch(request)
        .then((response) => {
          if (response && response.status === 200) {
            const clone = response.clone();
            caches.open(CACHE_NAME).then((cache) => cache.put(request, clone));
          }
          return response;
        })
        .catch(() => caches.match(request))
    );
    return;
  }

  // Cache-first pour les icônes/manifest statiques uniquement.
  event.respondWith(
    caches.match(request).then((cached) =>
      cached || fetch(request)
    )
  );
});

// Clic sur une notification « réponse de l'agent prête » → focus l'onglet Studio de
// l'app concernée s'il est ouvert, sinon en ouvre un (onglet Code = AgentWorkspace).
self.addEventListener('notificationclick', (event) => {
  event.notification.close();
  const data = event.notification.data || {};
  const slug = data.slug;
  const target = slug ? `/studio/${slug}?tab=code` : '/';
  event.waitUntil(
    (async () => {
      const all = await self.clients.matchAll({ type: 'window', includeUncontrolled: true });
      const hit = slug ? all.find((c) => c.url.includes(`/studio/${slug}`)) : all[0];
      if (hit) {
        await hit.focus();
        return;
      }
      await self.clients.openWindow(target);
    })()
  );
});
