import { h, svgNS } from './dom.js';

const RANK_WORD = {
  A: 'Ace', K: 'King', Q: 'Queen', J: 'Jack', T: 'Ten', 9: 'Nine', 8: 'Eight',
  7: 'Seven', 6: 'Six', 5: 'Five', 4: 'Four', 3: 'Three', 2: 'Two',
};
const SUIT_WORD = { s: 'spades', h: 'hearts', d: 'diamonds', c: 'clubs' };
const SUIT_PATHS = {
  s: 'M12 2s7.5 5.4 9.6 9.4c1.6 3.4-.2 6.6-3.5 6.6-1.9 0-3.4-.9-4.6-2.4.2 2.6.9 4.4 2.5 6.4H8c1.6-2 2.3-3.8 2.5-6.4C9.3 17.1 7.8 18 5.9 18c-3.3 0-5.1-3.2-3.5-6.6C4.5 7.4 12 2 12 2z',
  h: 'M12 21.5s-7.8-5.4-10-9.6C.4 8.5 2.2 4.5 6.1 4.5c2.4 0 4.2 1.4 5.9 3.7 1.7-2.3 3.5-3.7 5.9-3.7 3.9 0 5.7 4 4.1 7.4-2.2 4.2-10 9.6-10 9.6z',
  d: 'M12 2 L20.5 12 L12 22 L3.5 12 Z',
  c: 'M12 2.5a4.8 4.8 0 0 0-4.2 7.1 4.9 4.9 0 1 0 2.4 8.9c-.2 1.3-.8 2.4-1.7 3.5h7c-.9-1.1-1.5-2.2-1.7-3.5a4.9 4.9 0 1 0 2.4-8.9 4.8 4.8 0 0 0-4.2-7.1z',
};

export function suitIcon(suit) {
  const svg = document.createElementNS(svgNS, 'svg');
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('aria-hidden', 'true');
  const path = document.createElementNS(svgNS, 'path');
  path.setAttribute('d', SUIT_PATHS[suit]);
  svg.append(path);
  return svg;
}

export function cardName(card) {
  return `${RANK_WORD[card[0]]} of ${SUIT_WORD[card[1]]}`;
}

/** A card such as "Ah". Size: 'hero' | 'board' | 'opp' | 'small'. */
export function card(text, size = 'board') {
  const rank = text[0] === 'T' ? '10' : text[0];
  const suit = text[1];
  return h('div', { class: `card card-${size} suit-${suit}`, role: 'img', 'aria-label': cardName(text) },
    h('span', { class: 'card-rank' }, rank),
    h('span', { class: 'card-corner' }, suitIcon(suit)),
    h('span', { class: 'card-suit' }, suitIcon(suit)));
}

export function cardRow(cards, size) {
  return h('div', { class: 'card-row' }, cards.map(c => card(c, size)));
}
