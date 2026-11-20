// Keeps installed home-screen PWAs on the newest build without losing in-progress
// drill answers or looping if a server serves stale assets.
import { VERSION } from './version.js';

export const UPDATE_THROTTLE_MS = 3 * 60 * 1000;
const RELOAD_GUARD_KEY = 'poker-trainer.reloaded-for';
const CACHE_PREFIX = 'poker-trainer-';

let registration = null;
let lastCheckAt = 0;
let pendingVersion = null;
let reloading = false;
let initialized = false;
let hooks = {
  isBusy: () => false,
  onFlushBeforeReload: async () => {},
  onPendingChange: () => {},
  reload: () => location.reload(),
};

function canUseSessionStorage() {
  try {
    const probe = 'poker-trainer.__probe';
    sessionStorage.setItem(probe, '1');
    sessionStorage.removeItem(probe);
    return true;
  } catch (_) {
    return false;
  }
}

function readReloadGuard() {
  try { return sessionStorage.getItem(RELOAD_GUARD_KEY); } catch (_) { return null; }
}

function writeReloadGuard(target) {
  try { sessionStorage.setItem(RELOAD_GUARD_KEY, target); } catch (_) {}
}

function clearReloadGuard() {
  try { sessionStorage.removeItem(RELOAD_GUARD_KEY); } catch (_) {}
}

function isReloadBlocked(target) {
  return readReloadGuard() === target;
}

function queryWorkerVersion(worker) {
  if (!worker) return Promise.resolve(null);
  return new Promise(resolve => {
    const channel = new MessageChannel();
    const timer = setTimeout(() => resolve(null), 1500);
    channel.port1.onmessage = event => {
      clearTimeout(timer);
      const v = event.data && typeof event.data.version === 'string' ? event.data.version : null;
      resolve(v);
    };
    try {
      worker.postMessage({ type: 'version' }, [channel.port2]);
    } catch (_) {
      clearTimeout(timer);
      resolve(null);
    }
  });
}

async function findNewerCacheVersion() {
  if (typeof caches === 'undefined') return null;
  try {
    const names = await caches.keys();
    const ours = names.filter(name => name.startsWith(CACHE_PREFIX));
    if (!ours.length || ours.includes(CACHE_PREFIX + VERSION)) return null;
    const other = ours.find(name => name !== CACHE_PREFIX + VERSION);
    return other ? other.slice(CACHE_PREFIX.length) : null;
  } catch (_) {
    return null;
  }
}

async function detectNewerVersion() {
  if ('serviceWorker' in navigator && navigator.serviceWorker.controller) {
    const workerVersion = await queryWorkerVersion(navigator.serviceWorker.controller);
    if (workerVersion) return workerVersion !== VERSION ? workerVersion : null;
  }
  return findNewerCacheVersion();
}

function markPending(target) {
  pendingVersion = target;
  hooks.onPendingChange(true, target);
}

function triggerReload(target, { manual = false } = {}) {
  if (!target || reloading) return false;
  if (!manual) {
    if (!canUseSessionStorage()) {
      markPending(target);
      return false;
    }
    if (isReloadBlocked(target)) {
      // Already reloaded once for this target version and still booted on VERSION:
      // stop to avoid a reload loop.
      return false;
    }
  }
  reloading = true;
  writeReloadGuard(target);
  Promise.resolve()
    .then(() => hooks.onFlushBeforeReload())
    .catch(() => {})
    .finally(() => hooks.reload());
  return true;
}

function handleNewerVersion(target) {
  if (!target || target === VERSION || reloading || pendingVersion === target) return;
  if (isReloadBlocked(target)) return;
  if (hooks.isBusy()) {
    markPending(target);
    return;
  }
  triggerReload(target, { manual: false });
}

function throttleMs() {
  const override = window.__trainer && typeof window.__trainer.updateThrottleMs === 'number'
    ? window.__trainer.updateThrottleMs
    : null;
  return override !== null ? override : UPDATE_THROTTLE_MS;
}

export async function checkForUpdate(force = false) {
  if (!('serviceWorker' in navigator) || !location.protocol.startsWith('http')) return false;
  const now = Date.now();
  if (!force && lastCheckAt > 0 && now - lastCheckAt < throttleMs()) return false;
  lastCheckAt = now;
  try {
    if (registration) await registration.update();
  } catch (_) {}
  const newer = await detectNewerVersion();
  if (newer) handleNewerVersion(newer);
  return true;
}

export function consumePendingUpdate() {
  if (!pendingVersion || pendingVersion === VERSION) return false;
  const target = pendingVersion;
  return triggerReload(target, { manual: false });
}

export function applyPendingUpdate() {
  const target = pendingVersion || 'manual';
  return triggerReload(target, { manual: true });
}

export function pendingUpdateVersion() {
  return pendingVersion;
}

function watchRegistration(reg) {
  registration = reg;
  const watchWorker = worker => {
    if (!worker) return;
    worker.addEventListener('statechange', () => {
      if (worker.state === 'activated') {
        detectNewerVersion().then(v => { if (v) handleNewerVersion(v); }).catch(() => {});
      }
    });
  };
  if (reg.installing) watchWorker(reg.installing);
  if (reg.waiting) watchWorker(reg.waiting);
  reg.addEventListener('updatefound', () => watchWorker(reg.installing));
}

export function initUpdates(options = {}) {
  if (initialized) return;
  initialized = true;
  hooks = { ...hooks, ...options };
  if (readReloadGuard() === VERSION) clearReloadGuard();
  if (!('serviceWorker' in navigator) || !location.protocol.startsWith('http')) return;

  navigator.serviceWorker.addEventListener('controllerchange', () => {
    detectNewerVersion().then(v => { if (v) handleNewerVersion(v); }).catch(() => {});
  });

  navigator.serviceWorker.register('sw.js', { updateViaCache: 'none' })
    .then(reg => {
      watchRegistration(reg);
      return checkForUpdate(true);
    })
    .catch(() => {});

  document.addEventListener('visibilitychange', () => {
    if (!document.hidden) checkForUpdate(false);
  });
  window.addEventListener('pageshow', () => {
    checkForUpdate(false);
  });
}
