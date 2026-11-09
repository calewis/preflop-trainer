// Question and reveal layouts. Every number and description arrives ready to
// show; these functions only place it. Wording follows the command-line app.
import { h } from './dom.js';
import { card, cardRow } from './cards.js';

function plural(n, one, many) { return n === 1 ? `${n} ${one}` : `${n} ${many}`; }

function labelled(title, node) {
  return h('section', { class: 'labelled' }, h('h2', { class: 'label' }, title), node);
}

function chip(title, value) {
  return h('div', { class: 'chip' }, h('span', { class: 'chip-label' }, title), h('span', { class: 'chip-value' }, value));
}

function money(amount) { return `$${amount}`; }

function board(q) {
  return labelled('Board', cardRow(q.board, 'board'));
}

function hand(cards, title = 'Your hand') {
  return labelled(title, cardRow(cards, 'hero'));
}

export function questionView(q) {
  switch (q.kind) {
    case 'preflop': return preflopQuestion(q);
    case 'nuts': return h('div', { class: 'stack' }, board(q), prompt('Name the best possible holding right now.'));
    case 'possible': return h('div', { class: 'stack' }, board(q), prompt('Which can someone hold right now with two hole cards?'));
    case 'made': return h('div', { class: 'stack' }, hand(q.hero), board(q), prompt('What do you have?'));
    case 'beats': return h('div', { class: 'stack' }, hand(q.hero), board(q),
      prompt('What share of all unseen 2-card holdings are strictly ahead of you right now?'));
    case 'outs': return h('div', { class: 'stack' }, hand(q.hero), board(q), hand(q.villain, 'Opponent shows'),
      prompt('You are behind. How many next cards leave you strictly ahead?'));
    case 'price': return priceQuestion(q);
    case 'hit': return h('div', { class: 'stack' }, h('div', { class: 'chips' }, chip('Outs', q.outs), chip('Street', q.street)),
      h('p', { class: 'context' }, q.both ? 'All the money is in, so both remaining cards are coming.' : 'One card is coming.'),
      prompt('What is your chance of hitting?'));
    case 'spot': return spotQuestion(q);
    case 'board_type': return boardTypeQuestion(q);
    case 'equity': return equityQuestion(q);
    case 'anchor': return anchorQuestion(q);
    case 'table': return tableQuestion(q);
    case 'chain': return chainQuestion(q);
    case 'beats_me': return beatsMeQuestion(q);
    default: return h('p', {}, 'Unknown question');
  }
}

function prompt(text) {
  return h('p', { class: 'prompt' }, text);
}

function preflopQuestion(q) {
  const lines = {
    folded: 'It is folded to you.',
    limpers: q.limpers === 1 ? '1 player has limped in front of you.' : `${q.limpers} players have limped in front of you.`,
    raised: 'Someone has raised in front of you.',
  };
  return h('div', { class: 'stack' },
    h('div', { class: 'chips' }, chip('Table of', q.players), chip('Position', q.seat_title),
      chip('Still to act', q.behind)),
    hand(q.hand),
    h('p', { class: 'context' }, `Table of ${q.players}. You are ${q.seat_sentence} with ${plural(q.behind, 'player', 'players')} still to act behind you.`),
    prompt(lines[q.situation]));
}

function priceQuestion(q) {
  const callers = q.callers === 0 ? ''
    : q.callers === 1 ? ` 1 player has already called the ${money(q.bet)}.`
      : ` ${q.callers} players have already called the ${money(q.bet)}.`;
  return h('div', { class: 'stack' },
    h('div', { class: 'chips' }, chip('Pot', money(q.pot_before)), chip('Bet', money(q.bet))),
    h('p', { class: 'context' }, `The pot was ${money(q.pot_before)}. Someone bets ${money(q.bet)}.${callers}`),
    prompt('What equity do you need to call?'));
}

function spotQuestion(q) {
  const many = q.opponents.length > 1;
  const betting = q.all_in
    ? 'All-in: there is no more betting and all remaining cards run out.'
    : `More betting is coming. You see one more card. If you then have the best hand you collect about ${money(q.x)} more; otherwise you give up.${q.label ? ` You are up against ${q.label}.` : ''}`;
  return h('div', { class: 'stack' },
    hand(q.hero),
    labelled(`Board (${q.street})`, cardRow(q.board, 'board')),
    h('div', { class: 'chips' },
      chip(`Pot, including the bet${many ? ' and earlier callers' : ''}`, money(q.pot)),
      chip('To call', money(q.call))),
    h('ul', { class: 'facts' },
      many ? h('li', {}, `${q.opponents.length} opponents. Everyone else has acted and nobody can raise.`) : null,
      q.opponents.map(line => h('li', {}, line)),
      h('li', {}, betting)));
}

function boardTypeQuestion(q) {
  const fill = h('span', { class: 'target-fill' });
  fill.style.animationDuration = `${q.target_ms}ms`;
  return h('div', { class: 'stack' },
    board(q),
    h('div', { class: 'target' },
      h('span', { class: 'target-text' }, `Target: ${q.target_seconds} seconds`),
      h('span', { class: 'target-track', 'aria-hidden': 'true' }, fill)),
    prompt('Name the suit pattern and the rank pattern.'));
}

function equityQuestion(q) {
  const many = q.opponents > 1;
  const against = q.rung === 0
    ? h('div', { class: 'opp-row' }, q.exact.map((cards, i) => h('div', { class: 'stack tight' },
      h('p', { class: 'label' }, many ? `Opponent ${i + 1} shows` : 'Opponent shows'), cardRow(cards, 'opp'))))
    : h('ul', { class: 'facts' }, q.classes.map((text, i) => h('li', {},
      many ? `Assume opponent ${i + 1} has ${text}.` : `Assume they have ${text}.`)));
  return h('div', { class: 'stack compact' },
    hand(q.hero),
    labelled(`Board (${q.street})`, cardRow(q.board, 'board')),
    against,
    prompt('Everyone is all-in and all remaining cards run out. What is your equity?'));
}

function anchorQuestion(q) {
  if (q.matchup) {
    return h('div', { class: 'stack' },
      h('p', { class: 'context anchor-label' }, q.matchup.label),
      hand(q.matchup.hero),
      labelled('Opponent shows', cardRow(q.matchup.villain, 'opp')),
      labelled('Flop', cardRow(q.matchup.board, 'board')),
      prompt(q.prompt));
  }
  return h('div', { class: 'stack' },
    q.hand ? hand(q.hand) : null,
    prompt(q.prompt));
}

function tableQuestion(q) {
  return h('div', { class: 'stack compact' },
    h('div', { class: 'chips' }, chip('Table of', q.players), chip('Your seat', q.seat), chip('Behind', q.behind)),
    hand(q.hero),
    h('ol', { class: 'timeline' }, q.streets.map(street => h('li', { class: 'tl-street' },
      h('div', { class: 'tl-head' },
        h('strong', {}, street.name),
        street.cards.length ? h('span', { class: 'mini-cards' }, street.cards.map(c => card(c, 'small'))) : null,
        street.pot ? h('span', { class: 'tl-pot' }, street.pot) : null),
      street.actions.length
        ? h('div', { class: 'tl-actions' }, street.actions.map(a => h('span', { class: `tl-act ${a.kind}${a.hero ? ' you' : ''}` }, a.text)))
        : null))),
    h('div', { class: 'chips' }, chip('Pot', q.pot), chip('To call', q.call || 'Nothing')),
    prompt(`You are ${q.seat_sentence}. What do you do?`));
}

function beatsMeQuestion(q) {
  return h('div', { class: 'stack' }, hand(q.hero),
    labelled(`Board (${q.street})`, cardRow(q.board, 'board')),
    q.hands.length ? h('div', { class: 'stack beats-me-hands' }, q.hands.map(shown =>
      labelled(`Hand ${shown.letter}`, cardRow(shown.cards, 'board')))) : null,
    prompt(q.prompt));
}

function chainQuestion(q) {
  const part = h('p', { class: 'context chain-part' }, `Part ${q.part} of ${q.parts}`);
  const street = labelled(`Board (${q.street})`, cardRow(q.board, 'board'));
  if (q.stage === 'hand') return h('div', { class: 'stack' }, part, hand(q.hero), street, prompt('What do you have?'));
  if (q.stage === 'outs') {
    return h('div', { class: 'stack' }, part, hand(q.hero), street, hand(q.villain, 'Opponent shows'),
      prompt('You are behind. How many next cards leave you strictly ahead?'));
  }
  if (q.stage === 'price') return h('div', { class: 'stack' }, part, hand(q.hero), street, priceQuestion(q));
  return h('div', { class: 'stack' }, part, hand(q.hero), street, hand(q.villain, 'Opponent shows'),
    h('div', { class: 'chips' }, chip(`Pot, including the bet${q.second ? ' and the caller' : ''}`, money(q.pot)), chip('To call', money(q.call))),
    h('ul', { class: 'facts' },
      q.second ? h('li', {}, q.second) : null,
      h('li', {}, q.next_card_line)),
    prompt('Call or fold?'));
}

function holdingCards(cards) {
  return h('span', { class: 'mini-cards' }, cards.map(c => card(c, 'small')));
}

function holdingsLine(list) {
  return h('span', { class: 'holdings' }, list.shown.map(holdingCards),
    list.more > 0 ? h('span', {}, `and ${list.more} more`) : null);
}

function answerLine(text) {
  return h('p', { class: 'reveal-answer' }, 'Answer: ', h('strong', {}, text));
}

function p(...children) { return h('p', { class: 'reveal-line' }, ...children); }

function revealBody(r) {
  switch (r.kind) {
    case 'preflop': return [answerLine(r.answer), p(r.why)];
    case 'nuts': return nutsReveal(r);
    case 'possible': return possibleReveal(r);
    case 'made': return [answerLine(r.answer), p(r.description), p(`Your best five cards make ${r.rank}.`)];
    case 'beats': return beatsReveal(r);
    case 'beats_me': return beatsMeReveal(r);
    case 'outs': return outsReveal(r);
    case 'price': return [
      answerLine(r.required),
      p(`Needed equity = call / (pot + call) = ${r.call} / (${r.pot} + ${r.call}) = ${r.required}.`),
      p('The pot is everything in the middle before you act, including the bet.'),
    ];
    case 'hit': return hitReveal(r);
    case 'spot': return spotReveal(r);
    case 'board_type': return boardTypeReveal(r);
    case 'equity': return equityReveal(r);
    case 'anchor': return anchorReveal(r);
    case 'table': return tableReveal(r);
    default: return [];
  }
}

function nutsReveal(r) {
  const rule = r.used_x
    ? 'When you say "any", every holding you describe has to make the nuts'
    : 'Without "any", one holding you describe making the nuts is enough';
  return [
    h('p', { class: 'reveal-answer' }, 'The nuts: ', h('strong', {}, r.top)),
    r.grouped ? null : p('Held by ', holdingsLine(r.holdings)),
    r.by_card_removal ? p('Also can\'t be beaten, because they hold a card the better hand needs: ', holdingsLine(r.by_card_removal)) : null,
    r.next ? p(`Next best: ${r.next.rank}, held by `, holdingsLine(r.next.holdings)) : null,
    p(`${rule}. Your answer covers ${plural(r.consistent, 'unseen holding', 'unseen holdings')}, of which ${r.consistent_nuts} ${r.consistent_nuts === 1 ? 'makes' : 'make'} the nuts.`),
  ];
}

function possibleReveal(r) {
  const text = r.answer.length ? r.answer.join(', ') : 'none of these';
  const yes = (name, example) => p(`${name}: yes, for example `, holdingCards(example), '.');
  return [
    answerLine(text),
    r.flush ? yes('Flush', r.flush) : p(`Flush: no. The most of one suit on the board is ${r.max_suit}, and two hole cards need at least 3 to build on.`),
    r.straight ? yes('Straight', r.straight) : p('Straight: no. No run of five consecutive ranks (ace counts high or low) contains three board ranks.'),
    r.full ? yes('Full house or quads', r.full) : p('Full house or quads: no. Without a pair on the board, two hole cards can only reach a set or two pair.'),
  ];
}

function beatsReveal(r) {
  return [
    answerLine(r.answer),
    p(`Strictly ahead right now: ${r.ahead} of ${r.total} unseen holdings, ${r.pct}. You have ${r.hero}.`),
    p(r.random_note),
    r.kinds.length === 0 ? p('Nothing beats you right now.') : [
      p('Strongest holdings that beat you:'),
      h('ul', { class: 'facts' }, r.kinds.map(k => h('li', {},
        `${k.category}: ${plural(k.count, 'holding', 'holdings')}, the best being `,
        holdingCards(k.example), ` (${k.best}).`))),
    ],
  ];
}

function beatsMeReveal(r) {
  return [
    answerLine(r.answer),
    p(`You have ${r.hero}.`),
    r.hands.length ? h('ul', { class: 'facts' }, r.hands.map(hand => h('li', {},
      `Hand ${hand.letter} `, holdingCards(hand.cards), ` ${hand.text}`))) : null,
    r.examples.length ? h('ul', { class: 'facts' }, r.examples.map(ex => h('li', {},
      `${ex.label}: ${ex.ways} ${ex.ways === 1 ? 'way' : 'ways'}, such as `, holdingCards(ex.cards), '.'))) : null,
    r.lines.map(line => p(line)),
  ];
}

function outsReveal(r) {
  return [
    answerLine(r.answer),
    p(`You have ${r.hero} and they have ${r.villain}. There are ${r.unseen} unseen cards.`),
    labelled('Outs', h('div', { class: 'mini-cards wrap' }, r.outs.map(c => card(c, 'small')))),
    r.tempting.length ? [
      p('Tempting non-outs (they improve you but don\'t put you ahead):'),
      h('ul', { class: 'facts' }, r.tempting.map(t => h('li', {},
        holdingCards([t.card]), ` gives you ${t.rank}${t.ties ? ' (you would tie)' : ''}.`))),
    ] : null,
  ];
}

function hitReveal(r) {
  const answer = answerLine(r.exact);
  if (r.both) {
    return [answer,
      p(`Exact: ${r.exact}. That is 1 minus the chance of missing twice, (${r.unseen}-${r.outs})/${r.unseen} x (${r.unseen_next}-${r.outs})/${r.unseen_next}.`),
      p(`Times-4 shortcut: ${r.outs} x 4 = ${r.shortcut}. Use it only when both cards are sure to come, as when all the money is in on the flop. It overshoots with many outs.`),
    ];
  }
  return [answer,
    p(`Exact: ${r.outs}/${r.unseen} = ${r.exact}.`),
    p(`Times-2 shortcut: ${r.outs} x 2 = ${r.shortcut}. Use it when one card is coming, or whenever more betting could stop you seeing the second card.`),
  ];
}

function spotReveal(r) {
  const shareLabel = r.next_card ? 'Your share of the pot after the next card' : 'Your equity';
  return [
    h('p', { class: 'reveal-answer' }, 'Best play: ', h('strong', {}, r.answer)),
    h('dl', { class: 'stat-list' },
      row('Needed equity = call / (pot + call)', r.price),
      r.x > 0 ? row(`With about $${r.x} more to come, you need`, r.required) : null,
      row(shareLabel, r.share),
      row('EV of calling', r.ev_call),
      row('EV of folding', r.ev_fold),
      r.matched ? row('EV lost', '$0.00') : row('EV lost', `${r.loss_money} (${r.loss_pct} of the pot)`)),
    p(r.matched
      ? 'You chose the better action.'
      : `That is ${r.band}.`),
    p(`Averaged over ${r.combos} possible combos${r.sampled ? ` (estimated from a ${r.sample_size}-outcome sample)` : ''}.`),
    r.groups.map(line => p(line)),
    r.break_even === null || r.break_even === undefined ? null
      : r.break_even === '' ? p('Calling already breaks even without winning anything more.')
        : p(`You would need to win about ${r.break_even} more when you hit for calling to break even.`),
    p('This decision was averaged over all possible remaining cards and combos, so the actual next card doesn\'t matter.'),
    r.flip_call ? p(r.flip_call) : null,
    r.flip_cards ? p(r.flip_cards) : null,
  ];
}

function tableReveal(r) {
  return [
    h('p', { class: 'reveal-answer' }, 'Best: ', h('strong', {}, r.best)),
    p(r.explanation),
    r.loss
      ? p(`You gave up ${r.loss} against the best option${r.band ? `: ${r.band}` : ''}.`)
      : p('You picked the best option.'),
    r.read_note ? p(r.read_note) : null,
    h('ul', { class: 'table-options' }, r.options.map(o => h('li', { class: o.is_chosen ? 'chosen' : '' },
      h('div', { class: 'tl-head' }, h('strong', {}, o.label), o.is_best ? h('span', { class: 'pill good' }, 'Best') : null),
      h('div', { class: 'hint' }, `${o.ev_bb}, ${o.ev_pct}, ${o.share}`)))),
    r.archetypes.length ? [
      h('h3', { class: 'sub-title' }, 'By opponent type'),
      h('ul', { class: 'facts' }, r.archetypes.map(line => h('li', {}, line))),
    ] : null,
    h('p', { class: 'hint' }, r.note),
  ];
}

function boardTypeReveal(r) {
  return [
    answerLine(r.answer),
    p(r.why_suit, ' ', r.why_rank),
    r.matched ? null : p(`You said ${r.chosen}.`),
    r.slow_note ? p(r.slow_note) : null,
  ];
}

function equityReveal(r) {
  return [
    h('p', { class: 'reveal-answer' }, 'Exact equity: ', h('strong', {}, r.answer)),
    p(`${r.error}.`),
    h('dl', { class: 'stat-list' },
      row('You said', r.estimate),
      row(`Within ${plural(r.band, 'point', 'points')} counts as a hit`, r.hit ? 'Hit' : 'Miss')),
    p(!r.combos
      ? 'These are exact hands, so there is just one matchup to work out.'
      : r.tuples
      ? `Worked out over ${r.combos} opponent combos (${r.tuples} possible once clashing cards are removed).`
      : r.sampled
        ? `Estimated over ${r.combos} opponent combos from a ${r.sample_size}-outcome sample.`
        : `Worked out over ${r.combos} opponent combos.`),
  ];
}

function anchorReveal(r) {
  return [
    h('p', { class: 'reveal-answer' }, 'Answer: ', h('strong', {}, r.answer)),
    p(`${r.error}.`),
    h('dl', { class: 'stat-list' },
      row('You said', r.estimate),
      row(`Within ${plural(r.tolerance, 'point', 'points')} counts as correct`, r.hit ? 'Yes' : 'No')),
  ];
}

function row(label, value) {
  return [h('dt', {}, label), h('dd', {}, value)];
}

export function revealView(outcome) {
  const verdict = outcome.correct ? 'Correct.' : outcome.close ? 'Close.'
    : outcome.reveal.too_slow ? 'Too slow.' : 'Not quite.';
  const tone = outcome.correct ? 'good' : outcome.close ? 'close' : 'bad';
  return h('div', { class: `reveal ${tone}` },
    h('p', { class: 'verdict' }, verdict, ' ', h('span', { class: 'you' }, `You answered ${outcome.picked}.`)),
    revealBody(outcome.reveal),
    outcome.tie_line ? h('p', { class: 'reveal-tie' }, outcome.tie_line) : null);
}
