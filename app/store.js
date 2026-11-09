// The attempt log lives in localStorage as the library's TSV text.
const KEY = 'poker-trainer.attempts.v1';
const MINUTES_KEY = 'poker-trainer.minutes';

let memory = '';
let available = true;
// True when the newest log text only reached `memory`: stored text is then older.
let unsaved = false;

function safely(action, fallback) {
  try {
    return action();
  } catch (_) {
    available = false;
    return fallback;
  }
}

export function storageAvailable() { return available; }

export function probeStorage() {
  safely(() => {
    const probe = KEY + '.probe';
    localStorage.setItem(probe, '1');
    localStorage.removeItem(probe);
  });
  return available;
}

export function readLog() {
  if (unsaved) return memory;
  const text = safely(() => localStorage.getItem(KEY), null);
  if (text !== null) memory = text;
  else if (available) memory = '';
  return memory;
}

export function writeLog(text) {
  memory = text;
  unsaved = !safely(() => { localStorage.setItem(KEY, text); return true; }, false);
}

export function appendLogLine(line) {
  // Read first: another tab may have added lines since this one last looked.
  const current = readLog();
  const joiner = current === '' || current.endsWith('\n') ? '' : '\n';
  writeLog(`${current}${joiner}${line}\n`);
}

export function readMinutes() {
  const value = safely(() => localStorage.getItem(MINUTES_KEY), null);
  return value === null ? null : Number(value);
}

export function writeMinutes(minutes) {
  safely(() => localStorage.setItem(MINUTES_KEY, String(minutes)));
}
