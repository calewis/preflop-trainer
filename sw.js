/* Poker Trainer service worker: precaches the app so it runs offline.
   web/build.sh rewrites the VERSION line. */
const VERSION = '2b0c404fea13';
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
  event.respondWith((async () => {
    const cache = await caches.open(CACHE);
    const hit = await cache.match(request, { ignoreSearch: true });
    if (hit) return hit;
    if (request.mode === 'navigate') {
      // Any page under this scope, including the old app's addresses, is this app.
      return (await cache.match('index.html')) || fetch(request);
    }
    return fetch(request);
  })());
});
