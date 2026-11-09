// The app's screens. Each `show*` function fills the page and sets the key handler.
import { h } from './dom.js';
import { engine, prepareDrill, prepareBaseline, clearSpeculation, drillKey } from './engine.js';
import { answerControls } from './answers.js';
import { questionView, revealView } from './reveal.js';
import * as store from './store.js';

const app = () => document.getElementById('app');
let onKey = null;

// A second tap of a double-tap lands on the next item; ignore answers this soon.
const MIN_ANSWER_MS = 300;

/** Picks up attempts another tab has added since this one last looked. */
function syncFromStorage() {
  return engine.call('load', store.readLog());
}

document.addEventListener('keydown', event => {
  if (event.metaKey || event.ctrlKey || event.altKey) return;
  if (event.target instanceof HTMLInputElement) return;
  if (onKey) onKey(event);
});

function screen(...children) {
  onKey = null;
  app().replaceChildren(...children.flat().filter(Boolean));
  window.scrollTo(0, 0);
}

function topBar(title, right) {
  return h('header', { class: 'top-bar' },
    h('a', { class: 'back', href: '#home', 'aria-label': 'Back to home' }, 'Back'),
    h('h1', { class: 'top-title' }, title),
    h('div', { class: 'top-right' }, right || null));
}

// --- The clock: time on screen only, so a locked phone doesn't count against you.
let hiddenSince = null;
let hiddenTotal = 0;
document.addEventListener('visibilitychange', () => {
  if (document.hidden) hiddenSince = performance.now();
  else if (hiddenSince !== null) { hiddenTotal += performance.now() - hiddenSince; hiddenSince = null; }
});
export const activeNow = () => (hiddenSince ?? performance.now()) - hiddenTotal;

function randomSeed() {
  const words = new Uint32Array(2);
  crypto.getRandomValues(words);
  return String((BigInt(words[0]) << 32n) | BigInt(words[1]));
}

// --- Home ---------------------------------------------------------------------
let prepared = null;

export async function showHome() {
  await syncFromStorage();
  const home = await engine.call('home');
  const opened = await engine.call('session_open', randomSeed(), Date.now());
  prepared = opened.first;
  prepareDrill(prepared, false);
  let minutes = store.readMinutes();
  if (![5, 10, 15].includes(minutes)) minutes = 10;

  const lengthButtons = [5, 10, 15].map(m => h('button', {
    class: 'seg', 'aria-pressed': String(m === minutes),
    onclick: () => {
      minutes = m;
      store.writeMinutes(m);
      lengthButtons.forEach((b, i) => b.setAttribute('aria-pressed', String([5, 10, 15][i] === m)));
    },
  }, `${m} min`));

  screen(
    h('header', { class: 'home-head' }, h('h1', { class: 'app-title' }, 'Poker Trainer')),
    !home.has_baseline ? h('section', { class: 'callout' },
      h('h2', {}, 'Take the baseline first'),
      h('p', {}, `${home.baseline_size} questions with no feedback until the end. It gives a starting score to compare against later.`),
      h('a', { class: 'btn primary', href: '#baseline' }, 'Start baseline')) : null,
    h('section', { class: 'panel' },
      h('h2', { class: 'panel-title' }, "Today's drill"),
      h('div', { class: 'seg-group', role: 'group', 'aria-label': 'Drill length' }, lengthButtons),
      h('button', { class: 'btn primary', onclick: () => startDrill(minutes) }, 'Start drill'),
      h('p', { class: 'hint' }, 'In rotation: ', home.rotation.join(', '), '.')),
    h('section', { class: 'panel' },
      h('h2', { class: 'panel-title' }, `Step ${home.step} of ${home.step_count}`),
      h('ul', { class: 'steps' }, home.steps.map(stepRow))),
    h('nav', { class: 'home-links' },
      link('#card', 'Preflop card'),
      link('#progress', 'Progress'),
      link('#baseline', home.has_baseline ? 'Take baseline again' : 'Baseline'),
      link('#backup', 'Backup')),
  );
}

function link(href, text) { return h('a', { class: 'home-link', href }, text); }

function stepRow(step) {
  const state = step.passed ? 'Passed' : step.current ? 'Current' : step.unlocked ? 'Open' : 'Locked';
  return h('li', { class: step.unlocked ? 'step' : 'step locked' },
    h('div', { class: 'step-head' }, h('span', {}, `Step ${step.step}`), h('span', { class: `pill ${step.passed ? 'good' : ''}` }, state)),
    h('p', { class: 'step-skills' }, step.skills.map((s, i) =>
      [i > 0 ? ', ' : '', s.passed ? h('strong', {}, `${s.title} (passed)`) : s.title])));
}

// --- Drill ---------------------------------------------------------------------
let drill = null;

let starting = false;

async function startDrill(minutes) {
  if (starting || drill) return;
  starting = true;
  try {
    await engine.call('session_start', minutes);
    store.writeMinutes(minutes);
    drill = { minutes, startedAt: activeNow(), answered: 0, ok: 0 };
    location.hash = '#drill';
  } finally {
    starting = false;
  }
}

export function drillActive() { return drill !== null; }

export async function showDrill() {
  if (!drill) { location.hash = '#home'; return; }
  if (!drill.bar) {
    drill.bar = true;
    await nextDrillItem();
  }
}

function timeLeft() {
  const left = drill.minutes * 60000 - (activeNow() - drill.startedAt);
  const seconds = Math.max(0, Math.ceil(left / 1000));
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`;
}

function drillHeader(title, tag) {
  const clock = h('span', { class: 'timer', 'aria-hidden': 'true' }, timeLeft());
  const count = h('span', { class: 'count' }, `${drill.answered} done`);
  if (drill.clock) clearInterval(drill.clock);
  drill.clock = setInterval(() => {
    if (!drill) return;
    clock.textContent = timeLeft();
  }, 1000);
  return h('header', { class: 'top-bar' },
    h('button', { class: 'back', onclick: endDrill }, 'End'),
    h('h1', { class: 'top-title' }, title, tag ? h('span', { class: 'tag' }, tag) : null),
    h('div', { class: 'top-right' }, count, clock));
}

function leaveHeader(leave, title) {
  return h('header', { class: 'top-bar' },
    h('button', { class: 'back', onclick: leave.action }, leave.label),
    h('h1', { class: 'top-title' }, title),
    h('span', {}));
}

function renderLoading(leave) {
  screen(leaveHeader(leave, 'Dealing'), h('p', { class: 'loading' }, 'Dealing...'));
}

function renderFailure(err, leave) {
  showFatalNotice(err.message || String(err));
  screen(
    leaveHeader(leave, 'Something went wrong'),
    h('section', { class: 'panel' },
      h('p', {}, 'This question could not be prepared. Everything you have answered is saved.'),
      h('button', { class: 'btn primary', onclick: () => location.reload() }, 'Reload'),
      h('button', { class: 'btn', onclick: leave.action }, leave.label)));
}

async function nextDrillItem() {
  const state = drill;
  if (!state) return;
  const leave = { label: 'End', action: endDrill };
  let timer;
  try {
    const elapsed = Math.round(activeNow() - state.startedAt);
    const next = await engine.call('session_next', elapsed);
    if (drill !== state) return;
    if (next.finished) { await finishDrill(); return; }
    const planned = next.planned;
    // Only show the waiting message if the item isn't already prepared.
    timer = setTimeout(() => { if (drill === state) renderLoading(leave); }, 150);
    const bundle = await prepareDrill(planned, true);
    if (drill !== state) return;
    showItem(planned, bundle);
    speculate(planned).catch(() => {});
  } catch (err) {
    if (drill === state) renderFailure(err, leave);
  } finally {
    clearTimeout(timer);
  }
}

async function speculate(planned) {
  const [right, wrong] = await Promise.all([
    engine.call('session_peek', 1),
    engine.call('session_peek', 0),
  ]);
  clearSpeculation([drillKey(right.planned), drillKey(wrong.planned)]);
  prepareDrill(right.planned, false);
  prepareDrill(wrong.planned, false);
}

const ORIGIN_TAG = { new: '', due: 'review', retry: 'again' };

function showItem(planned, bundle) {
  const tag = ORIGIN_TAG[planned.origin];
  const status = h('div', { role: 'status', 'aria-live': 'polite', class: 'live-slot' });
  const errorSlot = h('p', { class: 'hint error', role: 'alert' });
  const shownAt = activeNow();
  // Real time, so a missed visibility event can never make answers ignored.
  const appearedAt = performance.now();
  window.__trainer.current = { planned, bundle, shownAt, appearedAt };
  let locked = false;

  async function submit(raw) {
    if (locked || performance.now() - appearedAt < MIN_ANSWER_MS) return;
    locked = true;
    const response = Math.round(activeNow() - shownAt);
    let outcome = bundle.graded[raw];
    if (!outcome) {
      let graded;
      try {
        graded = await engine.call('grade', planned.skill, planned.seed, raw);
      } catch (err) {
        renderFailure(err, { label: 'End', action: endDrill });
        return;
      }
      if (graded.invalid) {
        errorSlot.textContent = graded.invalid;
        locked = false;
        return;
      }
      outcome = graded.outcome;
    }
    showReveal(outcome, response);
  }

  const controls = answerControls(bundle.format, submit);
  screen(
    drillHeader(bundle.title, tag),
    h('section', { class: 'question' }, questionView(bundle.question)),
    h('section', { class: 'answers' }, controls.el, errorSlot),
    status);
  onKey = controls.onKey;
  const lockControls = () => controls.el.querySelectorAll('button').forEach(b => { b.disabled = true; });

  function showReveal(outcome, response) {
    lockControls();
    onKey = null;
    const commitPromise = commitAnswer(outcome, response);
    const nextBtn = h('button', { class: 'btn primary', onclick: goNext }, 'Next');
    async function goNext() {
      if (nextBtn.disabled) return;
      nextBtn.disabled = true;
      await commitPromise;
      await nextDrillItem();
    }
    const revealNode = revealView(outcome);
    screen(
      drillHeader(bundle.title, tag),
      h('section', { class: 'question revealed' }, questionView(bundle.question)),
      status,
      h('section', { class: 'answers' }, revealNode),
      h('div', { class: 'action-bar' }, nextBtn));
    status.textContent = revealNode.querySelector('.verdict').textContent;
    scrollRevealIntoView(revealNode);
    onKey = event => { if (event.key === 'Enter') goNext(); };
    nextBtn.focus({ preventScroll: true });
  }
}

// A reveal below the fold scrolls up so the answer is the first thing read.
function scrollRevealIntoView(node) {
  const rect = node.getBoundingClientRect();
  const bar = document.querySelector('.action-bar');
  const limit = window.innerHeight - (bar ? bar.offsetHeight + 8 : 80);
  if (rect.bottom <= limit) return;
  const header = document.querySelector('.top-bar');
  window.scrollBy(0, rect.top - (header ? header.offsetHeight + 8 : 8));
}

async function commitAnswer(outcome, response) {
  const state = drill;
  let result;
  try {
    result = await engine.call('commit', outcome.attempt, Date.now(), response);
  } catch (err) {
    showFatalNotice(err.message || String(err));
    return;
  }
  store.appendLogLine(result.line);
  noticeIfStorageLost();
  if (state) {
    state.answered = result.answered;
    if (result.ok) state.ok += 1;
    const count = document.querySelector('.top-right .count');
    if (count) count.textContent = `${result.answered} done`;
  }
}

function endDrill() {
  finishDrill();
}

async function finishDrill() {
  const state = drill;
  if (!state) return;
  if (state.clock) clearInterval(state.clock);
  drill = null;
  let summary;
  try {
    summary = await engine.call('session_summary');
  } catch (err) {
    showFatalNotice(err.message || String(err));
    location.hash = '#home';
    return;
  }
  if (summary.answered === 0) { location.hash = '#home'; return; }
  screen(
    h('header', { class: 'top-bar' },
      h('span', {}, ''), h('h1', { class: 'top-title' }, 'Session done'), h('span', {}, '')),
    h('section', { class: 'panel' },
      h('p', { class: 'big-score' }, `${summary.ok} of ${summary.answered} right`),
      h('dl', { class: 'stat-list' }, summary.by_skill.map(s => [h('dt', {}, s.title), h('dd', {}, `${s.ok} of ${s.answered}`)])),
      h('p', { class: 'hint' }, store.storageAvailable()
        ? 'Saved to your attempt log.'
        : 'Storage is unavailable, so this session will not be saved.')),
    h('div', { class: 'action-bar' }, h('button', {
      class: 'btn primary',
      onclick: () => { location.hash = '#home'; },
    }, 'Home')));
  onKey = event => { if (event.key === 'Enter') location.hash = '#home'; };
}

// --- Baseline ------------------------------------------------------------------
let baseline = null;

export function baselineActive() { return baseline !== null; }

export async function showBaseline() {
  if (baseline && !baseline.done) { renderBaselineItem(); return; }
  const { baseline_size: size } = await engine.call('home');
  screen(
    topBar('Baseline'),
    h('section', { class: 'panel' },
      h('p', {}, `${size} fixed questions in a shuffled order, with no feedback until the end. Take it once to get a starting score, then again every few weeks.`),
      h('button', { class: 'btn primary', onclick: startBaseline }, 'Begin')),
    await baselineHistory());
}

async function baselineHistory() {
  const progress = await engine.call('progress');
  if (!progress.runs.length) return null;
  return h('section', { class: 'panel' },
    h('h2', { class: 'panel-title' }, 'Earlier runs'),
    h('ul', { class: 'facts' }, progress.runs.slice().reverse().map(runLine)));
}

let startingBaseline = false;

async function startBaseline() {
  if (startingBaseline || baseline) return;
  startingBaseline = true;
  try {
    await syncFromStorage();
    const seed = randomSeed();
    const opened = await engine.call('baseline_open', seed);
    baseline = { seed, run: opened.run, count: opened.count, index: 0, done: false };
    for (let i = 0; i < Math.min(2, opened.count); i += 1) prepareBaseline(seed, i, false);
    await renderBaselineItem();
  } finally {
    startingBaseline = false;
  }
}

async function renderBaselineItem() {
  const state = baseline;
  const index = state.index;
  if (index >= state.count) { finishBaseline(); return; }
  const leave = { label: 'Stop', action: finishBaseline };
  let bundle;
  let timer;
  try {
    timer = setTimeout(() => { if (baseline === state && state.index === index) renderLoading(leave); }, 150);
    bundle = await prepareBaseline(state.seed, index, true);
  } catch (err) {
    if (baseline === state) renderFailure(err, leave);
    return;
  } finally {
    clearTimeout(timer);
  }
  if (baseline !== state || state.index !== index) return;
  if (index + 1 < state.count) prepareBaseline(state.seed, index + 1, false);
  let locked = false;
  const shownAt = activeNow();
  const appearedAt = performance.now();
  window.__trainer.current = { baselineIndex: index, bundle, shownAt, appearedAt };
  const errorSlot = h('p', { class: 'hint error', role: 'alert' });

  async function submit(raw) {
    if (locked || performance.now() - appearedAt < MIN_ANSWER_MS) return;
    locked = true;
    const response = Math.round(activeNow() - shownAt);
    const buttons = [...controls.el.querySelectorAll('button')];
    buttons.forEach(b => { b.disabled = true; });
    try {
      let outcome = bundle.graded[raw];
      if (!outcome) {
        const graded = await engine.call('baseline_grade', index, raw);
        if (graded.invalid) {
          errorSlot.textContent = graded.invalid;
          locked = false;
          buttons.forEach(b => { b.disabled = false; });
          return;
        }
        outcome = graded.outcome;
      }
      const result = await engine.call('baseline_commit', outcome.attempt, Date.now(), response);
      store.appendLogLine(result.line);
      noticeIfStorageLost();
    } catch (err) {
      renderFailure(err, leave);
      return;
    }
    state.index += 1;
    renderBaselineItem();
  }

  const controls = answerControls(bundle.format, submit);
  screen(
    h('header', { class: 'top-bar' },
      h('button', { class: 'back', onclick: finishBaseline }, 'Stop'),
      h('h1', { class: 'top-title' }, bundle.title),
      h('div', { class: 'top-right' }, h('span', { class: 'count' }, `${index + 1} of ${state.count}`))),
    h('section', { class: 'question' }, questionView(bundle.question)),
    h('section', { class: 'answers' }, controls.el, errorSlot));
  onKey = controls.onKey;
}

async function finishBaseline() {
  const state = baseline;
  if (!state) return;
  baseline = null;
  const result = await engine.call('baseline_report', state.run);
  if (!result.report) {
    screen(topBar('Baseline'), h('section', { class: 'panel' }, h('p', {}, 'No answers were recorded, so there is nothing to score.')));
    return;
  }
  const r = result.report;
  screen(
    topBar('Baseline result'),
    h('section', { class: 'panel' },
      h('p', { class: 'big-score' }, `${r.ok} of ${r.answered}${r.complete ? '' : ' (unfinished)'}`),
      h('h2', { class: 'panel-title' }, 'By section'),
      h('dl', { class: 'stat-list' }, r.sections.map(s => [
        h('dt', {}, s.title),
        h('dd', {}, `${s.ok} of ${s.answered}${s.show_exact ? ` (${s.exact} exact)` : ''}`),
      ])),
      h('h2', { class: 'panel-title' }, 'By step'),
      h('dl', { class: 'stat-list' }, r.steps.map(s => [
        h('dt', {}, `Step ${s.step}`), h('dd', {}, `${s.ok} of ${s.answered}`),
      ]))),
    h('section', { class: 'panel' },
      h('h2', { class: 'panel-title' }, 'Earlier runs'),
      result.earlier.length
        ? h('ul', { class: 'facts' }, result.earlier.map(runLine))
        : h('p', {}, 'This is your first baseline, so there is nothing to compare yet.')),
    h('div', { class: 'action-bar' }, h('a', { class: 'btn primary', href: '#home' }, 'Home')));
}

function runDate(run) {
  const ms = Number(run.started_ms);
  return ms ? new Date(ms).toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: 'numeric' }) : '';
}

function runLine(run) {
  const steps = run.steps.map(s => `step ${s.step} ${s.ok}/${s.answered}`).join(', ');
  return h('li', {}, `Run ${run.run}${runDate(run) ? ` (${runDate(run)})` : ''}: ${run.ok} of ${run.answered}, ${steps}${run.complete ? '' : ', unfinished'}`);
}

// --- Preflop card ------------------------------------------------------------
let cardState = { seat: 'cutoff', situation: 'folded' };

export async function showCard() {
  const meta = await engine.call('card_meta');
  const body = h('div', {});
  const seatTabs = meta.seats.map(s => h('button', {
    class: 'tab', 'aria-pressed': String(s.code === cardState.seat),
    onclick: () => { cardState.seat = s.code; refresh(); },
  }, s.title));
  const situationTabs = meta.situations.map(s => h('button', {
    class: 'tab', 'aria-pressed': String(s.code === cardState.situation),
    onclick: () => { cardState.situation = s.code; refresh(); },
  }, s.title));

  async function refresh() {
    const view = await engine.call('card_view', cardState.seat, cardState.situation);
    seatTabs.forEach((t, i) => t.setAttribute('aria-pressed', String(meta.seats[i].code === cardState.seat)));
    situationTabs.forEach((t, i) => t.setAttribute('aria-pressed', String(meta.situations[i].code === cardState.situation)));
    body.replaceChildren(
      h('div', { class: 'grid' }, view.grid.flat().map(cell =>
        h('div', { class: `cell act-${cell.a}`, title: cell.t }, cell.t))),
      h('ul', { class: 'legend' }, view.legend.map(l =>
        h('li', {}, h('span', { class: `swatch act-${l.code}` }), l.label))),
      view.lines.length
        ? h('div', { class: 'range-lines' }, view.lines.map(line => h('div', {},
          h('h3', {}, line.name),
          h('p', {}, `${line.pct}, ${line.combos}`),
          h('p', { class: 'hint' }, line.text))))
        : h('p', { class: 'hint' }, 'The big blind has no opening range: with no raise, check.'),
    );
  }

  await refresh();
  screen(
    topBar('Preflop card'),
    h('div', { class: 'tabs', role: 'group', 'aria-label': 'Seat' }, seatTabs),
    h('div', { class: 'tabs', role: 'group', 'aria-label': 'Situation' }, situationTabs),
    h('section', { class: 'panel' }, body),
    h('section', { class: 'panel' },
      h('h2', { class: 'panel-title' }, 'Notes'),
      h('ul', { class: 'facts' }, meta.notes.map(n => h('li', {}, n))),
      h('details', {}, h('summary', {}, 'About this card'),
        meta.header.map(line => h('p', {}, line)))));
}

// --- Progress --------------------------------------------------------------
export async function showProgress() {
  await syncFromStorage();
  const data = await engine.call('progress');
  screen(
    topBar('Progress'),
    h('section', { class: 'panel' },
      h('p', { class: 'big-score small' }, `Step ${data.step} of ${data.step_count}`),
      h('p', { class: 'hint' }, data.step_skills.join(', '), '. Pass marks are starting guesses.')),
    h('section', { class: 'panel' },
      h('h2', { class: 'panel-title' }, 'Skills'),
      data.skills.map(s => h('article', { class: 'skill-row' },
        h('div', { class: 'step-head' }, h('h3', {}, s.title),
          s.passed ? h('span', { class: 'pill good' }, 'Passed') : null),
        s.enough_data
          ? h('p', {}, s.stats.join(', '), '.')
          : h('p', { class: 'hint' }, `Not enough data (${s.total} of ${s.needed} attempts).`),
        h('p', { class: 'hint' }, 'Pass mark: ', s.mark.join(', '), '.'),
        s.enough_data && !s.passed ? h('p', { class: 'hint' }, s.status) : null))),
    h('section', { class: 'panel' },
      h('h2', { class: 'panel-title' }, 'Baseline history'),
      data.runs.length
        ? h('ul', { class: 'facts' }, data.runs.map(runLine))
        : h('p', { class: 'hint' }, 'No baseline on record yet.')));
}

// --- Backup ----------------------------------------------------------------
export async function showBackup() {
  const status = h('p', { class: 'hint', role: 'status' });
  const file = h('input', { type: 'file', accept: '.tsv,text/tab-separated-values,text/plain', hidden: true,
    onchange: async event => {
      const picked = event.target.files[0];
      if (!picked) return;
      const contents = await picked.text();
      // Another tab may have added attempts; merge into the log as it is now.
      await syncFromStorage();
      const merged = await engine.call('merge', contents);
      store.writeLog(merged.text);
      noticeIfStorageLost();
      const parts = [`Imported ${merged.added} new attempt${merged.added === 1 ? '' : 's'}.`];
      if (merged.duplicates) parts.push(`${merged.duplicates} ${merged.duplicates === 1 ? 'was' : 'were'} already here.`);
      if (merged.warnings.length) parts.push(`${merged.warnings.length} malformed line${merged.warnings.length === 1 ? ' was' : 's were'} skipped.`);
      parts.push(`You now have ${merged.total} attempts.`);
      status.textContent = parts.join(' ');
      event.target.value = '';
    } });

  // Prepared ahead, because iOS only allows sharing right after the tap.
  await syncFromStorage();
  let exported = (await engine.call('export')).text;
  let exportedFrom = store.readLog();
  const FILE_NAME = 'poker-trainer-attempts.tsv';
  const FILE_TYPE = 'text/tab-separated-values';

  function downloadFile(text) {
    const url = URL.createObjectURL(new Blob([text], { type: FILE_TYPE }));
    const a = h('a', { href: url, download: FILE_NAME });
    document.body.append(a);
    a.click();
    a.remove();
    setTimeout(() => URL.revokeObjectURL(url), 10000);
    status.textContent = 'Download started.';
  }

  async function exportLog() {
    if (store.readLog() !== exportedFrom) {
      await syncFromStorage();
      exported = (await engine.call('export')).text;
      exportedFrom = store.readLog();
    }
    const file = new File([exported], FILE_NAME, { type: FILE_TYPE });
    if (navigator.canShare && navigator.canShare({ files: [file] })) {
      try {
        await navigator.share({ files: [file] });
        status.textContent = 'Exported.';
        return;
      } catch (err) {
        if (err.name === 'AbortError') {
          status.textContent = 'Export cancelled.';
          return;
        }
      }
    }
    downloadFile(exported);
  }

  screen(
    topBar('Backup'),
    h('section', { class: 'panel' },
      h('h2', { class: 'panel-title' }, 'Your data'),
      h('p', {}, store.storageAvailable()
        ? 'Your attempts are kept as text in this browser, on this device, and nowhere else. Nothing is sent to a server. Clearing this site\'s data removes them, so export a backup now and then.'
        : 'Storage is unavailable in this browser, so nothing here will be saved after you close the app. Export before leaving.')),
    h('section', { class: 'panel' },
      h('h2', { class: 'panel-title' }, 'Export'),
      h('p', {}, 'Saves the attempt log as a file.'),
      h('button', { class: 'btn primary', onclick: exportLog }, 'Export attempt log')),
    h('section', { class: 'panel' },
      h('h2', { class: 'panel-title' }, 'Import'),
      h('p', {}, 'Adds attempts from a file exported earlier. Lines already here are skipped.'),
      h('label', { class: 'btn file-btn' }, 'Choose a file', file),
      status));
}

// --- Notices ------------------------------------------------------------------
export function showFatalNotice(message) {
  const notice = document.getElementById('notice');
  notice.hidden = false;
  notice.classList.add('fatal');
  notice.replaceChildren(`Something went wrong: ${message}. Your saved attempts are safe.`,
    h('button', { class: 'btn notice-reload', onclick: () => location.reload() }, 'Reload'));
}

export function noticeIfStorageLost() {
  const notice = document.getElementById('notice');
  if (notice.classList.contains('fatal')) return;
  if (store.storageAvailable()) {
    notice.hidden = true;
    return;
  }
  notice.hidden = false;
  notice.textContent = 'Storage is unavailable, so progress won\'t be saved on this device.';
}

export function endEverything() {
  if (drill && drill.clock) clearInterval(drill.clock);
  drill = null;
  baseline = null;
}
