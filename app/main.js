import { engine, onFatalError, prepareDrill, syncEquityMode, syncLeaks } from './engine.js';
import * as store from './store.js';
import { showHome, showDrill, showBaseline, showCheckup, showCard, showProgress, showBackup, showMistakes,
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
  mistakes: showMistakes,
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
  const leaks = store.readLeaks();
  if (leaks) {
    try { await syncLeaks(leaks); } catch (_) { store.clearLeaks(); await syncLeaks(''); }
  }
  if (store.readEquityExact()) await syncEquityMode(true);
  await route();
  if ('serviceWorker' in navigator && location.protocol.startsWith('http')) {
    navigator.serviceWorker.register('sw.js').catch(() => {});
  }
})();

