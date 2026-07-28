/* Service worker for both apps: the preflop trainer and the play-vs-bot table.
   Makes the whole thing work with no network at all, which is the point of a
   trainer you take to a game.

   ## The bug this file exists to not have again

   The deployed version answered **every** navigation out of the cache with
   `index.html`:

       if (e.request.mode === 'navigate') return caches.match('./index.html')

   That is the standard single-page-app recipe, and this is not a single-page
   app — it is two pages. So once the worker was installed, following the
   "Play" link served the trainer again, out of the cache, with the right URL
   in the address bar and no error anywhere to notice. A navigation fallback
   must fire only when the network actually failed, and must fall back to the
   page that was asked for.

   ## Cache names

   `CACHE` must be bumped on every app update or clients keep the old build for
   ever. The number here is the only thing that invalidates it.

   ## Adding files

   `cache.addAll` is atomic: one 404 and the whole install rejects, leaving no
   cache at all. The two apps are staged by different commands (`build.py` and
   `build.py --play`) and the trainer has a different filename at the repo root
   than it does when deployed, so a partial set is normal and must not take the
   worker down with it. Each file is added independently and a miss is
   tolerated. */

const CACHE = 'preflop-v16';

/* Everything either app needs offline. `./` is the directory index, which is
   what a bookmark to the site root asks for. */
const SHELL = [
  './',
  './index.html',           // the product: the merged play + trainer page
  './play.html',            // the same page under its old name, for old links
  './trainer.html',         // the standalone trainer: range browser, builder
  './poker-trainer.html',   // the trainer's name when served from the repo root
  './preflop_wasm.wasm',    // ~890 KB; without it the table cannot deal a hand
  './charts.json',          // preflop ranges: the bots' play and your grading
  './manifest.webmanifest', // without it an installed app has no name or icon
  './icon.png',
  './sw.js',
];

self.addEventListener('install', e => e.waitUntil((async () => {
  const c = await caches.open(CACHE);
  // Independently, so a file that is not deployed here cannot fail the install.
  await Promise.all(SHELL.map(u => c.add(u).catch(() => {})));
  self.skipWaiting();
})()));

self.addEventListener('activate', e => e.waitUntil((async () => {
  const keys = await caches.keys();
  await Promise.all(keys.filter(k => k !== CACHE).map(k => caches.delete(k)));
  await self.clients.claim();
})()));

/* Two strategies, chosen by what the file is for.

   ## Documents: network-first

   A cache-first document is a build that never ships. `CACHE` would have to be
   bumped by hand on every single edit, and the failure when somebody forgets is
   invisible: the server has the new page, the address bar shows the right URL,
   and the browser quietly serves last week's HTML out of the cache. That
   already happened here — an updated `play.html` was staged and the browser
   kept rendering the previous one, which looks exactly like a build that did
   not take.

   So a document goes to the network first and falls back to the cache only when
   the network fails. The cost is one round trip on a page that is a few tens of
   kilobytes; the benefit is that "I rebuilt it" and "I can see it" are the same
   event.

   ## Everything else: stale-while-revalidate

   The `.wasm` is ~700 KB and is the thing that makes the first paint slow, so
   it is served from cache immediately and refreshed in the background for next
   time. That means a fresh page can briefly run against the previous engine —
   which is why `play.html` reads optional snapshot fields with a guard and
   degrades to showing less rather than breaking. */
const isDoc = (req, url) =>
  req.mode === 'navigate' || req.destination === 'document' ||
  /\.html?$/.test(url.pathname);

self.addEventListener('fetch', e => {
  const url = new URL(e.request.url);
  // Someone else's origin is none of our business.
  if (url.origin !== location.origin) return;
  // Only GET is cacheable, and nothing here does anything else.
  if (e.request.method !== 'GET') return;

  const keep = async res => {
    if (res && res.ok && res.type === 'basic') {
      const c = await caches.open(CACHE);
      await c.put(e.request, res.clone());
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
        return await keep(await fetch(url.href, {
          cache: 'no-cache',
          credentials: 'same-origin',
        }));
      } catch (_) {
        // Offline. Fall back to **the page that was asked for**, then to
        // whichever app is present — never unconditionally to one of them,
        // which is the bug that made the Trainer/Play switch serve the trainer
        // for both links.
        const name = url.pathname.split('/').pop() || 'index.html';
        return (await caches.match(e.request, { ignoreSearch: true }))
            || (await caches.match('./' + name, { ignoreSearch: true }))
            || (await caches.match('./index.html', { ignoreSearch: true }))
            || (await caches.match('./poker-trainer.html', { ignoreSearch: true }))
            || new Response('offline', { status: 503 });
      }
    })());
    return;
  }

  e.respondWith((async () => {
    const hit = await caches.match(e.request, { ignoreSearch: true });
    const net = fetch(e.request).then(keep).catch(() => null);
    return hit || (await net) || new Response('offline', { status: 503 });
  })());
});
