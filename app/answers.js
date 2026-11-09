// Answer controls, built from the library's answer format. They collect a raw
// answer and hand it back; nothing here knows whether an answer is right.
import { h } from './dom.js';
import { suitIcon } from './cards.js';

const CHOICE_KEYS = ['1', '2', '3', '4', '5', '6', '7', '8', '9', '0', 'a', 'b', 'c', 'd'];
const RANK_ORDER = ['A', 'K', 'Q', 'J', 'T', '9', '8', '7', '6', '5', '4', '3', '2'];
const SUITS = ['s', 'h', 'd', 'c'];

export function answerControls(format, submit) {
  switch (format.type) {
    case 'choice': return choiceControls(format, submit);
    case 'multi': return multiControls(format, submit);
    case 'integer': return keypad({ decimals: false, suffix: '' }, submit);
    case 'percent': return keypad({ decimals: true, suffix: '%' }, submit);
    case 'holding': return holdingPicker(submit);
    default: throw new Error('unknown answer format');
  }
}

function keyHint(key) {
  return h('kbd', { class: 'key-hint' }, key);
}

function choiceControls(format, submit) {
  const columns = format.options.length > 4 ? 'choice-grid two' : 'choice-grid';
  const buttons = format.options.map((option, i) =>
    h('button', { class: 'btn choice', onclick: () => submit(option.code) },
      CHOICE_KEYS[i] ? keyHint(CHOICE_KEYS[i]) : null, option.label));
  return {
    el: h('div', { class: columns }, buttons),
    onKey(event) {
      const index = CHOICE_KEYS.indexOf(event.key.toLowerCase());
      if (index >= 0 && format.options[index]) submit(format.options[index].code);
    },
  };
}

function multiControls(format, submit) {
  const picked = new Set();
  const submitBtn = h('button', { class: 'btn primary', onclick: go }, 'None of these');
  const buttons = format.options.map((option, i) => {
    const btn = h('button', {
      class: 'btn toggle',
      'aria-pressed': 'false',
      onclick: () => toggle(i),
    }, CHOICE_KEYS[i] ? keyHint(CHOICE_KEYS[i]) : null, option.label);
    return btn;
  });

  function toggle(i) {
    const code = format.options[i].code;
    if (picked.has(code)) picked.delete(code); else picked.add(code);
    buttons[i].setAttribute('aria-pressed', String(picked.has(code)));
    submitBtn.textContent = picked.size === 0 ? 'None of these' : 'Submit';
  }

  function go() {
    if (picked.size === 0) { submit(format.none_code); return; }
    submit(format.options.filter(o => picked.has(o.code)).map(o => o.code).join(''));
  }

  return {
    el: h('div', { class: 'stack' }, h('div', { class: 'choice-grid' }, buttons), submitBtn),
    onKey(event) {
      const index = CHOICE_KEYS.indexOf(event.key.toLowerCase());
      if (index >= 0 && format.options[index]) toggle(index);
      if (event.key === 'Enter') go();
    },
  };
}

function keypad({ decimals, suffix }, submit) {
  let value = '';
  const display = h('output', { class: 'keypad-display', 'aria-live': 'off' }, ' ');
  const submitBtn = h('button', { class: 'btn primary', disabled: true, onclick: go }, 'Submit');

  function refresh() {
    display.textContent = value ? value + suffix : ' ';
    submitBtn.disabled = value === '';
  }
  function press(key) {
    if (key === 'back') value = value.slice(0, -1);
    else if (key === '.') { if (decimals && !value.includes('.')) value = value ? value + '.' : '0.'; }
    else if (value.length < 5) value += key;
    refresh();
  }
  function go() { if (value !== '') submit(value); }

  const keys = ['1', '2', '3', '4', '5', '6', '7', '8', '9'];
  const lastLeft = decimals
    ? h('button', { class: 'btn key', onclick: () => press('.') }, '.')
    : h('span');
  const pad = h('div', { class: 'keypad' },
    keys.map(k => h('button', { class: 'btn key', onclick: () => press(k) }, k)),
    lastLeft,
    h('button', { class: 'btn key', onclick: () => press('0') }, '0'),
    h('button', { class: 'btn key', 'aria-label': 'Delete', onclick: () => press('back') }, '⌫'));

  return {
    el: h('div', { class: 'stack' }, display, pad, submitBtn),
    onKey(event) {
      if (/^[0-9]$/.test(event.key)) press(event.key);
      else if (event.key === '.' || event.key === ',') press('.');
      else if (event.key === 'Backspace') press('back');
      else if (event.key === 'Enter') go();
    },
  };
}

function holdingPicker(submit) {
  const slots = [{ rank: null, suit: null }, { rank: null, suit: null, any: false }];
  let active = 0;
  let suitedness = '';

  const preview = h('div', { class: 'picker-preview', 'aria-live': 'polite' });
  const hint = h('p', { class: 'hint' });
  const submitBtn = h('button', { class: 'btn primary', disabled: true, onclick: go }, 'Submit');

  const slotEls = slots.map((_, i) => {
    const rankBtn = h('button', { class: 'slot-rank', onclick: () => { active = i; refresh(); } });
    const suitBtns = SUITS.map(s => h('button', {
      class: `slot-suit suit-${s}`,
      'aria-pressed': 'false',
      'aria-label': { s: 'spades', h: 'hearts', d: 'diamonds', c: 'clubs' }[s],
      onclick: () => toggleSuit(i, s),
    }, suitIcon(s)));
    const anyBtn = i === 1
      ? h('button', { class: 'slot-any', 'aria-pressed': 'false', onclick: () => setAny() }, 'any')
      : null;
    const row = h('div', { class: 'slot' }, h('span', { class: 'slot-label' }, i === 0 ? '1st' : '2nd'),
      rankBtn, suitBtns, anyBtn);
    return { row, rankBtn, suitBtns, anyBtn };
  });

  const suitedRow = h('div', { class: 'suited-row', hidden: true },
    ['', 's', 'o'].map(code => h('button', {
      class: 'chip-toggle',
      'aria-pressed': code === '' ? 'true' : 'false',
      onclick: () => { suitedness = code; refresh(); },
      'data-code': code,
    }, { '': 'Either', s: 'Suited', o: 'Offsuit' }[code])));

  const rankButtons = RANK_ORDER.map(r =>
    h('button', { class: 'btn rank', onclick: () => pickRank(r) }, r === 'T' ? '10' : r));
  const grid = h('div', { class: 'rank-grid' }, rankButtons);

  function pickRank(rank) {
    const slot = slots[active];
    slot.rank = rank;
    if (active === 1) slot.any = false;
    active = active === 0 ? 1 : 1;
    if (!slots[1].rank && !slots[1].any) active = 1;
    refresh();
  }

  function toggleSuit(i, suit) {
    if (!slots[i].rank) return;
    slots[i].suit = slots[i].suit === suit ? null : suit;
    refresh();
  }

  function setAny() {
    slots[1].any = !slots[1].any;
    if (slots[1].any) { slots[1].rank = null; slots[1].suit = null; }
    refresh();
  }

  function compose() {
    const [a, b] = slots;
    if (!a.rank) return { text: null };
    const first = a.rank + (a.suit || '');
    if (b.any) return { text: `${first}x` };
    if (!b.rank) return { text: null };
    const bothSuits = a.suit && b.suit;
    if (bothSuits) return { text: `${a.rank}${a.suit}${b.rank}${b.suit}` };
    if (a.suit || b.suit) return { text: null, need: 'Add a suit to both cards, or neither.' };
    return { text: `${a.rank}${b.rank}${a.rank === b.rank ? '' : suitedness}` };
  }

  function refresh() {
    const [a, b] = slots;
    slotEls.forEach((el, i) => {
      const slot = slots[i];
      el.rankBtn.textContent = slot.any ? 'any' : slot.rank ? (slot.rank === 'T' ? '10' : slot.rank) : '-';
      el.row.classList.toggle('active', active === i);
      el.suitBtns.forEach((btn, j) => {
        btn.setAttribute('aria-pressed', String(slot.suit === SUITS[j]));
        btn.disabled = !slot.rank;
      });
      if (el.anyBtn) el.anyBtn.setAttribute('aria-pressed', String(slot.any));
    });
    const composed = compose();
    submitBtn.disabled = !composed.text;
    hint.textContent = composed.need || '';
    const showSuited = a.rank && b.rank && !b.any && !a.suit && !b.suit && a.rank !== b.rank;
    suitedRow.hidden = !showSuited;
    if (!showSuited) suitedness = '';
    [...suitedRow.children].forEach(el =>
      el.setAttribute('aria-pressed', String(el.dataset.code === suitedness)));
    preview.replaceChildren(previewText(composed.text));
  }

  function previewText(text) {
    if (!text) return document.createTextNode('Pick the cards below.');
    return document.createTextNode(`Your answer: ${describe(text)}`);
  }

  function describe(text) {
    const words = { A: 'A', K: 'K', Q: 'Q', J: 'J', T: '10' };
    const rank = r => words[r] || r;
    const suit = s => ({ s: '♠', h: '♥', d: '♦', c: '♣' }[s]) + '\uFE0E';
    if (text.length === 4) return `${rank(text[0])}${suit(text[1])} ${rank(text[2])}${suit(text[3])}`;
    if (text.endsWith('x')) {
      const head = text.length === 3 ? `${rank(text[0])}${suit(text[1])}` : rank(text[0]);
      return `${head} with any other card`;
    }
    const tail = text[2] === 's' ? ' suited' : text[2] === 'o' ? ' offsuit' : '';
    return `${rank(text[0])}${rank(text[1])}${tail}`;
  }

  function clearAll() {
    slots[0].rank = null; slots[0].suit = null;
    slots[1].rank = null; slots[1].suit = null; slots[1].any = false;
    suitedness = '';
    active = 0;
    refresh();
  }

  function go() {
    const composed = compose();
    if (composed.text) submit(composed.text);
  }

  const clearBtn = h('button', { class: 'btn ghost', onclick: clearAll }, 'Clear');
  refresh();

  return {
    el: h('div', { class: 'stack picker' }, preview,
      h('div', { class: 'slots' }, slotEls.map(s => s.row)),
      suitedRow, grid, hint,
      h('div', { class: 'row' }, clearBtn, submitBtn)),
    onKey(event) {
      const key = event.key.toUpperCase();
      if (RANK_ORDER.includes(key)) pickRank(key);
      else if (key === 'X') setAny();
      else if (event.key === 'Backspace') clearAll();
      else if (event.key === 'Enter') go();
    },
  };
}
