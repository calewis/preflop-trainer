import { engine, onFatalError, prepareDrill } from './engine.js';
import * as store from './store.js';
import { showHome, showDrill, showBaseline, showCard, showProgress, showBackup,
  drillActive, baselineActive, endEverything, noticeIfStorageLost, showFatalNotice, activeNow } from './screens.js';

const fatal = showFatalNotice;
onFatalError(fatal);
window.__trainer = { engine, store, current: null, activeNow, prepare: prepareDrill }; // for the test harness

const routes = {
  home: showHome,
  drill: showDrill,
  baseline: showBaseline,
  card: showCard,
  progress: showProgress,
  backup: showBackup,
};

async function route() {
  const name = location.hash.replace('#', '') || 'home';
  // Leaving a drill or baseline by the back gesture ends it; answers are kept.
  if ((drillActive() && name !== 'drill') || (baselineActive() && name !== 'baseline')) endEverything();
  const show = routes[name] || showHome;
  try {
    await show();
  } catch (err) {
    fatal(err.message || String(err));
    throw err;
  }
}

window.addEventListener('hashchange', route);

(async () => {
  store.probeStorage();
  noticeIfStorageLost();
  const log = store.readLog();
  await engine.call('load', log);
  await route();
  if ('serviceWorker' in navigator && location.protocol.startsWith('http')) {
    navigator.serviceWorker.register('sw.js').catch(() => {});
  }
})();

