/* Preflop Trainer service worker — full offline. Bump CACHE on each app update. */
const CACHE = 'preflop-v4';
const SHELL = ['./', './index.html', './sw.js'];
self.addEventListener('install', e => e.waitUntil(
  caches.open(CACHE).then(c => c.addAll(SHELL)).then(() => self.skipWaiting())));
self.addEventListener('activate', e => e.waitUntil(
  caches.keys().then(ks => Promise.all(ks.filter(k => k !== CACHE).map(k => caches.delete(k))))
    .then(() => self.clients.claim())));
self.addEventListener('fetch', e => e.respondWith((async () => {
  const hit = await caches.match(e.request, { ignoreSearch: true });
  if (hit) return hit;
  if (e.request.mode === 'navigate')
    return (await caches.match('./index.html', { ignoreSearch: true })) || fetch(e.request);
  try { return await fetch(e.request); }
  catch (_) { return (await caches.match('./index.html', { ignoreSearch: true })) || new Response('offline', {status:503}); }
})()));
