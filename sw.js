/* Service worker for the preflop trainer. Makes the whole thing work with no
   network at all, which is the point of a trainer you take to a game.

   The site is ONE page under two names: `index.html` (the root, what a
   visitor gets) and `trainer.html` (what existing links and bookmarks point
   at). `build.py --deploy` writes both from a single render, so they cannot
   differ — which is what makes it safe to precache only one and let the
   offline fallback answer either name with those bytes. (A play-vs-bot page
   shared this worker once; it was retired before release, and most of this
   file's historical complexity — the wasm, the 157 MB bot tables, the
   two-app navigation fallback — went with it.)

   ## Cache names

   `CACHE` must be bumped on every app update or clients keep the old build
   for ever. The number here is the only thing that invalidates it.

   ## Adding files

   `cache.addAll` is atomic: one 404 and the whole install rejects, leaving no
   cache at all. A partial set must not take the worker down (the art backs,
   for instance, may be absent from a bare copy), so each file is added
   independently and a miss is tolerated. */

const CACHE = 'preflop-v28';

/* Everything the page needs offline. `./` is the directory index, which is
   what a bookmark to the site root asks for; it and `trainer.html` are the
   same bytes as `index.html`, so one copy in the cache serves all three
   names through the fallback below. The art card backs are not precached:
   the one the user picked is cached by the fetch handler the first time it
   renders, and paying ~0.75 MB at install for eight alternatives nobody may
   ever select is the wrong trade. */
const SHELL = [
  './index.html',
  './manifest.webmanifest', // without it an installed app has no name or icon
  './icon.png',
  // Attribution for the MIT `phe` evaluator inlined in the page, and the
  // art-back credits. Staged by `build.py`; they belong in the offline shell
  // alongside what they cover.
  './LICENSE',
  './NOTICE',
  './sw.js',
];

self.addEventListener('install', e => e.waitUntil((async () => {
  const c = await caches.open(CACHE);
  // Independently, so a file that is not deployed here cannot fail the install.
  //
  // **`cache: 'no-cache'`, not a plain `c.add(u)`.** `add` fetches with default
  // cache mode, which goes through the browser's own HTTP cache — so bumping
  // `CACHE` would faithfully populate the *new* cache with the *old* bytes and
  // the release still would not ship. A conditional request costs headers and
  // a 304 when nothing changed, not the whole page again.
  await Promise.all(SHELL.map(async u => {
    try {
      const res = await fetch(u, { cache: 'no-cache', credentials: 'same-origin' });
      if (res && res.ok) await c.put(u, res);
    } catch (_) { /* not deployed here; the others still install */ }
  }));
  self.skipWaiting();
})()));

self.addEventListener('activate', e => e.waitUntil((async () => {
  const keys = await caches.keys();
  await Promise.all(keys.filter(k => k !== CACHE).map(k => caches.delete(k)));
  await self.clients.claim();
})()));

/* Two strategies, chosen by what the file is for.

   ## Documents: network-first

   A cache-first document is a build that never ships. `CACHE` would have to
   be bumped by hand on every single edit, and the failure when somebody
   forgets is invisible: the server has the new page, the address bar shows
   the right URL, and the browser quietly serves last week's HTML out of the
   cache. That happened here — an updated page was staged and the browser
   kept rendering the previous one, which looks exactly like a build that did
   not take. So a document goes to the network first and falls back to the
   cache only when the network fails.

   ## Everything else: stale-while-revalidate

   The art backs are the only sizeable assets; they are served from cache
   immediately and refreshed in the background for next time. */
const isDoc = (req, url) =>
  req.mode === 'navigate' || req.destination === 'document' ||
  /\.html?$/.test(url.pathname);

self.addEventListener('fetch', e => {
  const url = new URL(e.request.url);
  // Someone else's origin is none of our business.
  if (url.origin !== location.origin) return;
  // Only GET is cacheable, and nothing here does anything else.
  if (e.request.method !== 'GET') return;

  /* Writes are keyed by **path without the query**, because reads use
     `ignoreSearch: true`. They disagreed once: a request carrying `?v=1` was
     stored under the full URL and could never be read back, so every
     cache-busted fetch added an entry nothing would ever hit. No production
     path adds a query today; a read and a write that use different keys is a
     bug waiting for the first one that does. */
  const keep = async res => {
    if (res && res.ok && res.type === 'basic') {
      const c = await caches.open(CACHE);
      const key = new URL(e.request.url);
      key.search = '';
      await c.put(key.href, res.clone());
    }
    return res;
  };

  if (isDoc(e.request, url)) {
    e.respondWith((async () => {
      try {
        // `cache: 'no-cache'` is doing real work here, not being cautious.
        // A plain `fetch(request)` inside a worker still goes through the
        // browser's own HTTP cache, so "network-first" quietly resolved out
        // of that cache and served a stale page anyway — the same invisible
        // failure this whole file exists to avoid, one layer down. `no-cache`
        // forces a conditional request: the server answers 304 when nothing
        // changed, so this costs a round trip and not a megabyte.
        //
        // The URL is refetched rather than the Request being reused because a
        // navigation Request has `mode: 'navigate'`, which cannot be
        // reconstructed with different options.
        const res = await fetch(url.href, {
          cache: 'no-cache',
          credentials: 'same-origin',
        });
        // **An error response does not throw.** Without this, a bad deploy,
        // or a captive portal answering 200 with a login page, was handed
        // straight to the user while a perfectly good copy sat in the cache.
        // Only a real failure should reach the fallback, and a 404 is one.
        if (!res || !res.ok) throw new Error('http ' + (res && res.status));
        // `keep` outside the success path would cache the error page.
        return await keep(res);
      } catch (_) {
        // Offline. The page that was asked for first; then `index.html`,
        // which is byte-identical to every name this site serves — see the
        // header. Never a silent wrong page: if nothing is cached, say so.
        const name = url.pathname.split('/').pop() || 'index.html';
        return (await caches.match(e.request, { ignoreSearch: true }))
            || (await caches.match('./' + name, { ignoreSearch: true }))
            || (await caches.match('./index.html', { ignoreSearch: true }))
            || new Response('offline', { status: 503 });
      }
    })());
    return;
  }

  /* Stale-while-revalidate — with both halves actually working:
     `cache: 'no-cache'` so the background refresh cannot re-`put` the very
     stale bytes it exists to replace (a plain fetch resolves out of the HTTP
     cache), and `e.waitUntil(net)` so the browser cannot terminate the worker
     before the refresh's `cache.put` lands — without it, "next time" never
     came. */
  e.respondWith((async () => {
    const hit = await caches.match(e.request, { ignoreSearch: true });
    const net = fetch(e.request, { cache: 'no-cache', credentials: 'same-origin' })
      .then(res => (res && res.ok ? keep(res) : null))
      .catch(() => null);
    if (hit) {
      e.waitUntil(net);
      return hit;
    }
    return (await net) || new Response('offline', { status: 503 });
  })());
});
