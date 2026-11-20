/* Poker Trainer service worker: precaches the app so it runs offline.
   web/build.sh rewrites the VERSION line. */
const VERSION = '5ddccca07118';
const CACHE_PREFIX = 'poker-trainer-';
const CACHE = CACHE_PREFIX + VERSION;
// The origin is shared with other project pages, so only these caches are ours.
const OLD_APP_CACHE = /^preflop-v\d+$/;

const FILES = [
  './',
  'index.html',
  'trainer.html',
  'manifest.webmanifest',
  'app/style.css',
  'app/version.js',
  'app/update.js',
  'app/main.js',
  'app/engine.js',
  'app/worker.js',
  'app/store.js',
  'app/cards.js',
  'app/answers.js',
  'app/reveal.js',
  'app/screens.js',
  'app/dom.js',
  'app/trainer.wasm',
  'app/icon.svg',
  'app/icon-180.png',
  'app/icon-192.png',
  'app/icon-512.png',
  'app/icon-512-maskable.png',
];

self.addEventListener('message', event => {
  if (event.data && event.data.type === 'version' && event.ports[0]) {
    event.ports[0].postMessage({ version: VERSION });
  }
});

self.addEventListener('install', event => {
  // `reload` skips the browser's HTTP cache, so a new version gets new bytes.
  event.waitUntil((async () => {
    const cache = await caches.open(CACHE);
    await cache.addAll(FILES.map(f => new Request(f, { cache: 'reload' })));
    await self.skipWaiting();
  })());
});

self.addEventListener('activate', event => {
  event.waitUntil((async () => {
    const names = await caches.keys();
    const stale = names.filter(name => name !== CACHE
      && (name.startsWith(CACHE_PREFIX) || OLD_APP_CACHE.test(name)));
    const replacedOtherApp = stale.some(name => OLD_APP_CACHE.test(name));
    await Promise.all(stale.map(name => caches.delete(name)));
    await self.clients.claim();
    if (replacedOtherApp) {
      // Move pages of the old app onto this one. These navigations must not be
      // awaited: their requests are held until activation finishes, so waiting
      // for them here would stall both for minutes.
      const windows = await self.clients.matchAll({ type: 'window', includeUncontrolled: true });
      windows.forEach(win => win.navigate(win.url).catch(() => null));
    }
  })());
});

self.addEventListener('fetch', event => {
  const request = event.request;
  if (request.method !== 'GET' || new URL(request.url).origin !== location.origin) return;
  if (request.mode === 'navigate') {
    event.respondWith((async () => {
      const cache = await caches.open(CACHE);
      try {
        // Fetch navigations fresh so a stale shell cannot pin old script URLs.
        const fresh = await fetch(request.url, { cache: 'reload' });
        if (fresh && fresh.ok) {
          let resp = fresh;
          if (fresh.redirected) {
            const body = await fresh.arrayBuffer();
            resp = new Response(body, { status: fresh.status, statusText: fresh.statusText, headers: fresh.headers });
          }
          const path = new URL(request.url).pathname;
          if (path.endsWith('/trainer.html')) {
            cache.put('trainer.html', resp.clone()).catch(() => {});
          } else if (path.endsWith('/') || path.endsWith('/index.html')) {
            cache.put('index.html', resp.clone()).catch(() => {});
            cache.put('./', resp.clone()).catch(() => {});
          }
          return resp;
        }
      } catch (_) {}
      // Offline or unknown legacy address: fall back to the precached shell.
      const exact = await cache.match(request, { ignoreSearch: true });
      if (exact) return exact;
      return (await cache.match('index.html')) || Response.error();
    })());
    return;
  }
  event.respondWith((async () => {
    const cache = await caches.open(CACHE);
    const hit = await cache.match(request, { ignoreSearch: true });
    if (hit) return hit;
    return fetch(request);
  })());
});
