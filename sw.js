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

const CACHE = 'preflop-v25';

/* Everything either app needs offline. `./` is the directory index, which is
   what a bookmark to the site root asks for. */
const SHELL = [
  /* **One copy of the document, not three.** `./`, `./index.html` and
     `./play.html` are byte-identical — `build.py` writes both outputs from one
     source and now *asserts* they match — so precaching all three spent
     127 KB gzipped, 19% of a first visit, delivering bytes the visitor already
     has in the navigation response.

     Only safe because of that build-time assertion. If the two ever diverged, an
     offline visitor whose first navigation was `play.html` would fall through the
     chain below to `index.html`'s bytes: the shape of the bug in the header
     above, though not the bug, since they cannot be different apps. The guarantee
     lives in `build.py` rather than here, which is the trade. */
  './index.html',           // the product: the merged play + trainer page
  './preflop_wasm.wasm',    // ~900 KB; without it the table cannot deal a hand
  './charts.json',          // preflop ranges: the bots' play and your grading
  './manifest.webmanifest', // without it an installed app has no name or icon
  './icon.png',
  // Apache-2.0 s.4 attribution for `rs_poker`, which is compiled into the wasm
  // above. `build.py` stages these and `APP.md` §1 calls them not optional, so
  // they belong in the offline shell alongside the binary they cover — they were
  // staged and then left out of it.
  './LICENSE',
  './NOTICE',
  './sw.js',
];

/* **`trainer.html` is deliberately not here.** It is large (the Range Lab's
   exact equity table lives in it), and precaching it would put ~30% more on
   the wire for a first visit that may never leave the play page. The play
   page's `%` link reaches it; the fetch handler caches it on demand the first
   time someone follows that link, which is the right trade for a second page.
   `poker-trainer.html` is gone from the list too: `build.py` never stages it,
   so it was a guaranteed 404 at install and a dead arm in the offline
   fallback below. */

self.addEventListener('install', e => e.waitUntil((async () => {
  const c = await caches.open(CACHE);
  // Independently, so a file that is not deployed here cannot fail the install.
  //
  // **`cache: 'no-cache'`, not a plain `c.add(u)`.** `add` fetches with default
  // cache mode, which goes through the browser's own HTTP cache — so bumping
  // `CACHE` would faithfully populate the *new* cache with the *old* bytes and
  // the release still would not ship. That is the same one-layer-down staleness
  // the document handler below already guards against, and it bites hardest on
  // `preflop_wasm.wasm`: the page tests for new exports and degrades when they
  // are missing, so a stale engine costs a feature silently rather than
  // failing. Observed exactly that on 2026-07-28 — a freshly built export was
  // absent from the instantiated module while present in the file on disk.
  //
  // A conditional request costs headers and a 304 when nothing changed, not a
  // megabyte.
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

  /* **The heads-up bot's tables never touch this worker.**
     They are `hubot.bp`, `hubot.flop.abs` and `hubot.turn.abs` — 157 MB of
     blueprint and k-means data that `play.html` loads for local testing only.
     They are not in `SHELL`, but leaving them out of the precache list is not
     enough: the stale-while-revalidate branch below caches *every* same-origin
     non-document GET it sees, so the first load would `cache.put` 157 MB into
     Cache Storage — 45x the entire app, on an origin quota that would then
     evict the shell it exists to hold, and `keep` clones the response so the
     140 MB turn table is briefly held twice.

     Returning without calling `respondWith` hands the request back to the
     browser untouched. That also keeps `play.html`'s streaming reader reading
     from the network directly rather than through a worker that would buffer
     the whole body before the first chunk arrived, which is what makes the
     progress bar move.

     Matched on the filename rather than a substring of the URL: a path that
     merely *contains* "hubot" is somebody else's file. */
  if (/^hubot\.(bp|flop\.abs|turn\.abs)$/.test(url.pathname.split('/').pop())) return;

  /* Writes are keyed by **path without the query**, because reads use
     `ignoreSearch: true`. They disagreed: a request carrying `?v=1` was stored
     under the full URL and could never be read back, so every cache-busted fetch
     added an entry nothing would ever hit. Demonstrated accidentally during
     profiling — two probe URLs left 1.85 MB of unreachable cache. No production
     path adds a query today, so this was latent; a read and a write that use
     different keys is a bug waiting for the first one that does. */
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
        // **An error response does not throw.** Without this, a bad deploy, or a
        // captive portal answering 200 with a login page, was handed straight to
        // the user while a perfectly good copy sat in the cache. Only a real
        // failure should reach the fallback, and a 404 is a real failure.
        if (!res || !res.ok) throw new Error('http ' + (res && res.status));
        // `keep` outside the success path would cache the error page.
        return await keep(res);
      } catch (_) {
        // Offline. Fall back to **the page that was asked for**, then to
        // whichever app is present — never unconditionally to one of them,
        // which is the bug that made the Trainer/Play switch serve the trainer
        // for both links.
        const name = url.pathname.split('/').pop() || 'index.html';
        return (await caches.match(e.request, { ignoreSearch: true }))
            || (await caches.match('./' + name, { ignoreSearch: true }))
            || (await caches.match('./index.html', { ignoreSearch: true }))
            || new Response('offline', { status: 503 });
      }
    })());
    return;
  }

  /* Stale-while-revalidate — with both halves actually working.
     Two bugs lived here, and both are this file's own reasoning not applied to
     its third fetch:

     1. **`cache: 'no-cache'`.** The install handler and the document handler each
        force a conditional request, and each carries a comment explaining that a
        plain `fetch` inside a worker resolves out of the browser's HTTP cache —
        and that this "bites hardest on `preflop_wasm.wasm`". This was that plain
        fetch, so the revalidation could re-`put` the very stale bytes it existed
        to replace. On Pages (`max-age=600`) bumping `CACHE` did not reliably
        deliver a new engine.
     2. **`e.waitUntil(net)`.** On a cache hit the response resolves at once and
        the browser is free to terminate the worker before the background fetch
        and its `cache.put` finish. The refresh meant to arrive "next time" might
        never land, so next time never came. */
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
