import { engine, onFatalError, prepareDrill } from './engine.js';
import * as store from './store.js';
import { showHome, showDrill, showBaseline, showCheckup, showCard, showProgress, showBackup,
  drillActive, baselineActive, checkupActive, endEverything, noticeIfStorageLost, showFatalNotice, activeNow } from './screens.js';

const fatal = showFatalNotice;
onFatalError(fatal);
window.__trainer = { engine, store, current: null, activeNow, prepare: prepareDrill }; // for the test harness

const routes = {
  home: showHome,
  drill: showDrill,
  baseline: showBaseline,
  checkup: showCheckup,
  card: showCard,
  progress: showProgress,
  backup: showBackup,
};

async function route() {
  const [name, arg] = (location.hash.replace('#', '') || 'home').split(':');
  // Leaving a drill, baseline or checkup by the back gesture ends it; answers are kept.
  if ((drillActive() && name !== 'drill') || (baselineActive() && name !== 'baseline')
      || (checkupActive() && name !== 'checkup')) endEverything();
  const show = routes[name] || showHome;
  try {
    await show(arg);
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

