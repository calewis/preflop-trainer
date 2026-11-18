// Talks to the two wasm workers: `engine` holds the attempts and the session and
// stays responsive; `prep` does the slow item generation in the background.
const workerUrl = new URL('./worker.js', import.meta.url);

// Every worker this page starts, including replacements started much later, runs
// the same build of the engine: the page fetches it once and hands each worker a
// copy. A replacement that fetched it itself could get a newer build from an
// updated service worker, and then deal questions the engine grades differently.
// If the fetch fails, each worker fetches the file itself, as before.
const wasmBytes = fetch(new URL('./trainer.wasm', import.meta.url))
  .then(response => (response.ok ? response.arrayBuffer() : null))
  .catch(() => null);

function spawn() {
  const worker = new Worker(workerUrl);
  wasmBytes.then(bytes => {
    const copy = bytes ? bytes.slice(0) : null;
    worker.postMessage({ wasm: copy }, copy ? [copy] : []);
  });
  return worker;
}

function client(onFatal) {
  let worker = spawn();
  let nextId = 1;
  let dead = null;
  const pending = new Map();

  function attach() {
    worker.onmessage = event => {
      const { id, text, fatal, ms } = event.data;
      const waiting = pending.get(id);
      if (!waiting) return;
      pending.delete(id);
      if (fatal) {
        waiting.reject(new Error(fatal));
        onFatal(fatal);
        return;
      }
      const reply = JSON.parse(text);
      if (reply && reply.error) waiting.reject(new Error(reply.error));
      else waiting.resolve({ reply, ms });
    };
    worker.onerror = event => onFatal(event.message || 'worker failed');
  }
  attach();

  function rejectPending(reason) {
    for (const waiting of pending.values()) waiting.reject(new Error(reason));
    pending.clear();
  }

  // Commands wait until the worker has been handed the engine, so they arrive after it.
  function post(message) {
    const target = worker;
    wasmBytes.then(() => target.postMessage(message));
  }

  function send(args) {
    if (dead) return Promise.reject(new Error(dead));
    const id = nextId++;
    return new Promise((resolve, reject) => {
      const message = { id, args: args.map(String) };
      pending.set(id, { resolve, reject, message });
      post(message);
    });
  }

  return {
    call: (...args) => send(args).then(result => result.reply),
    timed: (...args) => send(args),
    restart() {
      worker.terminate();
      rejectPending('the worker restarted');
      worker = spawn();
      attach();
    },
    /** Swaps in a fresh worker and resends everything still waiting, after `setup` (which may send first). */
    revive(setup) {
      const waiting = [...pending.values()].map(entry => entry.message);
      worker.terminate();
      worker = spawn();
      attach();
      setup();
      waiting.forEach(post);
    },
    /** Gives up on this worker: everything waiting or to come fails with `reason`. */
    stop(reason) {
      worker.terminate();
      dead = reason;
      rejectPending(reason);
    },
  };
}

let fatalHandler = () => {};
export function onFatalError(handler) { fatalHandler = handler; }

export const engine = client(message => fatalHandler(message));

// A crashed prep worker is replaced, but not forever if it can't even start.
const MAX_PREP_RESTARTS = 5;
let prepRestarts = 0;
let leaksText = '';
const prepWorker = client(message => {
  prepRestarts += 1;
  if (prepRestarts > MAX_PREP_RESTARTS) prepWorker.stop(message);
  else {
    prepWorker.restart();
    if (leaksText) prepWorker.call('load_leaks', leaksText).catch(() => {});
  }
});

// iOS can bring a home-screen app back from the background with its workers
// frozen: a reply never comes and the page sits on "Dealing..." for good. The
// prep worker holds no state, so on every return replace it and resend
// whatever it was working on.
document.addEventListener('visibilitychange', () => {
  if (document.hidden) return;
  prepWorker.revive(() => {});
});

// One prepare runs at a time. Requests the player is waiting for jump the queue.
const bundles = new Map();
const CACHE_LIMIT = 64;
let queue = [];
let busy = false;

// Set once a grade came back about a different question from the one dealt: from
// then on this page deals from the engine that grades, so the two always agree.
let prepOnEngine = false;

/** Deals every later question from the grading engine and forgets what the prep worker made. */
export function distrustPrep() {
  prepOnEngine = true;
  bundles.clear();
}

function pump() {
  if (busy || queue.length === 0) return;
  busy = true;
  const job = queue.shift();
  (prepOnEngine ? engine : prepWorker).timed(...job.args).then(
    ({ reply }) => { prepRestarts = 0; job.resolve(reply); },
    err => job.reject(err),
  ).finally(() => { busy = false; pump(); });
}

function request(key, args, urgent) {
  let entry = bundles.get(key);
  if (!entry) {
    let resolve, reject;
    const promise = new Promise((res, rej) => { resolve = res; reject = rej; });
    const created = { promise, job: { args, resolve, reject, key } };
    entry = created;
    bundles.set(key, entry);
    // A failed item must be retried, not served from here. Speculative requests
    // have nobody waiting on them, so this also keeps their failures quiet.
    promise.catch(() => { if (bundles.get(key) === created) bundles.delete(key); });
    if (bundles.size > CACHE_LIMIT) bundles.delete(bundles.keys().next().value);
    if (urgent) queue.unshift(entry.job); else queue.push(entry.job);
    pump();
  } else if (urgent) {
    const index = queue.indexOf(entry.job);
    if (index > 0) { queue.splice(index, 1); queue.unshift(entry.job); }
  }
  return entry.promise;
}

export function prepareDrill(planned, urgent) {
  return request(`d:${planned.skill}:${planned.seed}`, ['prepare', planned.skill, planned.seed, planned.origin], urgent);
}

export function prepareLeak(planned, urgent) {
  return request(`l:${planned.seed}:${planned.origin}`, ['prepare_leak', planned.seed, planned.origin], urgent);
}

export async function syncLeaks(text) {
  bundles.clear();
  queue.splice(0);
  leaksText = text || '';
  const [reply] = await Promise.all([
    engine.call('load_leaks', leaksText),
    prepWorker.call('load_leaks', leaksText),
  ]);
  return reply;
}

export function prepareBaseline(seed, index, urgent) {
  return request(`b:${seed}:${index}`, ['prepare_baseline', seed, index], urgent);
}

export function prepareCheck(skill, form, slot, run, urgent) {
  return request(`c:${skill}:${form}:${slot}:${run}`, ['prepare_check', skill, form, slot, run], urgent);
}

/** Drops speculative work that hasn't started, keeping anything being waited on. */
export function clearSpeculation(keepKeys) {
  for (const job of queue.splice(0)) {
    if (keepKeys.includes(job.key)) queue.push(job);
    else bundles.delete(job.key);
  }
}

export function drillKey(planned) { return `d:${planned.skill}:${planned.seed}`; }

export function leakKey(planned) { return `l:${planned.seed}:${planned.origin}`; }

/** For tests and timing: the milliseconds a prepare call took inside the worker. */
export function timePrepare(args) { return prepWorker.timed(...args); }
