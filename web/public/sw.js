const CACHE_NAME = 'atelier-v7';
const STATIC_ASSETS = [
  '/manifest.json',
  '/favicon.svg',
  '/icon-192x192.svg',
  '/icon-512x512.svg',
  '/icon-192.png',
  '/icon-512.png',
  '/icon-maskable-512.png',
  '/apple-touch-icon.png',
  '/maintenance.html'
];

// Filet ultime si /maintenance.html manque au cache (installation partielle) :
// une réponse synthétique minimale — le respondWith d'une navigation ne doit
// JAMAIS résoudre en `undefined` (page blanche, bug historique v≤5).
const FALLBACK_HTML = '<!doctype html><html lang="fr"><meta charset="utf-8">'
  + '<meta name="viewport" content="width=device-width, initial-scale=1">'
  + '<title>Atelier — indisponible</title>'
  + '<body style="font-family:system-ui;display:flex;min-height:100vh;align-items:center;justify-content:center;background:#030712;color:#f9fafb">'
  + '<div style="text-align:center"><h1 style="font-size:18px">Atelier est momentanément indisponible</h1>'
  + '<p style="color:#9ca3af;font-size:14px">Redémarrage en cours — cette page se rechargera automatiquement.</p></div>'
  + '<script>setInterval(function(){fetch("/api/health",{cache:"no-store"}).then(function(r){if(r.ok)location.reload()}).catch(function(){})},3000)</scr' + 'ipt></body></html>';

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

  // Navigations : réseau d'abord, TOUJOURS — le SW n'intervient QUE sur un rejet
  // réseau (Atelier down : restart, mise à jour autonome) pour servir la page de
  // secours. Un statut HTTP quelconque (302 auth edge, 404, 500) passe intact.
  // WHY le triple filet : l'ancienne interception (v≤5) produisait une PAGE
  // BLANCHE quand `caches.match(request)` renvoyait `undefined` (navigation
  // jamais en cache) — ici on matche une page PRÉ-CACHÉE à l'install, avec une
  // réponse synthétique en dernier recours : jamais d'`undefined`.
  if (request.mode === 'navigate') {
    event.respondWith(
      fetch(request).catch(() =>
        caches.match('/maintenance.html').then((cached) =>
          cached || new Response(FALLBACK_HTML, {
            status: 503,
            headers: { 'Content-Type': 'text/html; charset=utf-8', 'Cache-Control': 'no-store' }
          })
        )
      )
    );
    return;
  }
  // API/socket.io : aucun bénéfice à les passer par le SW.
  if (request.method !== 'GET') return;

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

// Clic sur une notification → focus l'onglet correspondant s'il est ouvert, sinon
// en ouvre un. Deux familles de payload :
//  - agent (« réponse prête ») : `data.slug` → Studio de l'app (chemin historique) ;
//  - plateforme (notify_user / journal) : `data.target` explicite — `/studio/<slug>`
//    ou `/?notif=1` (homepage, tiroir notifications ouvert par NotificationsContext).
self.addEventListener('notificationclick', (event) => {
  event.notification.close();
  const data = event.notification.data || {};
  const target = data.target || (data.slug ? `/studio/${data.slug}?tab=code` : '/');
  // Path de matching : un onglet déjà ouvert sur ce préfixe est focusé plutôt
  // que d'ouvrir un doublon (les query params du target ne matchent pas l'URL).
  const targetPath = target.split('?')[0];
  const matchPath = targetPath === '/' ? null : targetPath;
  event.waitUntil(
    (async () => {
      const all = await self.clients.matchAll({ type: 'window', includeUncontrolled: true });
      const hit = matchPath ? all.find((c) => c.url.includes(matchPath)) : all[0];
      if (hit) {
        await hit.focus();
        return;
      }
      await self.clients.openWindow(target);
    })()
  );
});
