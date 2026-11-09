//! Exact equity by enumeration, with a deterministic sample for huge multiway cases.

use crate::cards::{holding_mask, mask_of, unseen_cards, Card, Holding};
use crate::eval::{evaluate, HandRank};
use crate::rng::Rng;

/// Above this many hand evaluations a multiway case is sampled instead.
pub const EVAL_LIMIT: u64 = 20_000_000;
/// Outcomes drawn when a case is sampled.
pub const SAMPLE_SIZE: u64 = 300_000;
const SAMPLE_SEED_SALT: u64 = 0x4571_7569_7479_0001;

/// Share of the pot, in twelfths: 1, 1/2, 1/3 and 1/4 are all whole numbers.
const SHARE_UNITS: u64 = 12;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Equity {
    /// Hero's expected share of the pot, 0 to 1.
    pub share: f64,
    /// True when the result came from the deterministic sample.
    pub sampled: bool,
    /// How many card-disjoint opponent tuples were weighted equally, or `None`
    /// for a sampled result, where the full count is never computed.
    pub tuples: Option<u64>,
}

/// Hero's equity with every remaining board card dealt.
///
/// `opponents` holds one candidate list per opponent. Every card-disjoint tuple
/// is weighted equally, and every runout is enumerated. `None` if no disjoint
/// tuple exists, or if the board is longer than 5 cards.
pub fn equity_all_in(
    hero: Holding,
    board: &[Card],
    opponents: &[Vec<Holding>],
    seed: u64,
) -> Option<Equity> {
    if board.len() > 5 {
        return None;
    }
    equity_with_cards_to_come(hero, board, opponents, 5 - board.len(), seed)
}

/// Hero's share after exactly one more card.
pub fn equity_next_card(
    hero: Holding,
    board: &[Card],
    opponents: &[Vec<Holding>],
    seed: u64,
) -> Option<Equity> {
    equity_with_cards_to_come(hero, board, opponents, 1, seed)
}

/// Equity with `to_come` more board cards.
pub fn equity_with_cards_to_come(
    hero: Holding,
    board: &[Card],
    opponents: &[Vec<Holding>],
    to_come: usize,
    seed: u64,
) -> Option<Equity> {
    if opponents.is_empty() || board.len() + to_come > 5 {
        return None;
    }
    let dead = holding_mask(&hero) | mask_of(board);
    let unseen = 52 - 2 - board.len() - 2 * opponents.len();
    let evals_per_tuple = combinations(unseen as u64, to_come as u64) * (opponents.len() as u64 + 1);
    let cap = EVAL_LIMIT / evals_per_tuple.max(1);
    let tuples = count_tuples(opponents, dead, cap + 1);
    if tuples == 0 {
        return None;
    }
    if opponents.len() >= 2 && tuples > cap {
        sampled(hero, board, opponents, to_come, SAMPLE_SIZE, seed)
    } else {
        Some(enumerated(hero, board, opponents, to_come, tuples))
    }
}

/// Hero's share against `combos`, averaged per possible next card. With
/// `extra_card` the card after that is enumerated too, as an all-in flop still
/// has the river to come; both orders of a runout count once for each card, so
/// this costs one pass over the runouts. Cards every combo holds, or that are
/// dead, stay out of the result; the caller compares each card with its price.
pub fn share_by_next_card(hero: Holding, board: &[Card], combos: &[Holding], extra_card: bool) -> Vec<(Card, f64)> {
    let dead = holding_mask(&hero) | mask_of(board);
    let cards = unseen_cards(dead);
    let mut wins = vec![0.0f64; 52];
    let mut counts = vec![0u32; 52];
    let share = |hero_rank: HandRank, villain_rank: HandRank| -> f64 {
        match hero_rank.cmp(&villain_rank) {
            std::cmp::Ordering::Greater => 1.0,
            std::cmp::Ordering::Equal => 0.5,
            std::cmp::Ordering::Less => 0.0,
        }
    };
    let mut full: Vec<Card> = board.to_vec();
    let last = full.len();
    full.push(hero[0]);
    if extra_card {
        full.push(hero[0]);
    }
    let rank = |h: Holding, b: &[Card]| {
        let mut seven = [hero[0]; 7];
        seven[0] = h[0];
        seven[1] = h[1];
        seven[2..2 + b.len()].copy_from_slice(b);
        evaluate(&seven[..2 + b.len()])
    };
    for &combo in combos {
        let mask = holding_mask(&combo);
        for (i, &first) in cards.iter().enumerate() {
            if mask & first.mask() != 0 {
                continue;
            }
            full[last] = first;
            if extra_card {
                for &second in &cards[i + 1..] {
                    if mask & second.mask() != 0 {
                        continue;
                    }
                    full[last + 1] = second;
                    let s = share(rank(hero, &full), rank(combo, &full));
                    wins[first.index()] += s;
                    counts[first.index()] += 1;
                    wins[second.index()] += s;
                    counts[second.index()] += 1;
                }
            } else {
                let s = share(rank(hero, &full), rank(combo, &full));
                wins[first.index()] += s;
                counts[first.index()] += 1;
            }
        }
    }
    cards
        .iter()
        .filter(|c| counts[c.index()] > 0)
        .map(|c| (*c, wins[c.index()] / f64::from(counts[c.index()])))
        .collect()
}

/// A sampled estimate of exactly `samples` outcomes, deterministic in `seed`.
pub fn sampled(
    hero: Holding,
    board: &[Card],
    opponents: &[Vec<Holding>],
    to_come: usize,
    samples: u64,
    seed: u64,
) -> Option<Equity> {
    let dead = holding_mask(&hero) | mask_of(board);
    if count_tuples(opponents, dead, 1) == 0 || board.len() + to_come > 5 || samples == 0 {
        return None;
    }
    let mut rng = Rng::new(seed ^ SAMPLE_SEED_SALT);
    let mut units = 0u64;
    let mut chosen = vec![hero; opponents.len()];
    let mut full_board = [hero[0]; 5];
    full_board[..board.len()].copy_from_slice(board);
    for _ in 0..samples {
        // Independent draws with rejection are uniform over disjoint tuples.
        let tuple_dead = 'tuple: loop {
            let mut used = dead;
            for (slot, list) in chosen.iter_mut().zip(opponents) {
                let pick = list[rng.below(list.len() as u64) as usize];
                let mask = holding_mask(&pick);
                if used & mask != 0 {
                    continue 'tuple;
                }
                used |= mask;
                *slot = pick;
            }
            break used;
        };
        let mut unseen = unseen_cards(tuple_dead);
        for i in 0..to_come {
            let j = i + rng.below((unseen.len() - i) as u64) as usize;
            unseen.swap(i, j);
            full_board[board.len() + i] = unseen[i];
        }
        units += outcome_units(&hero, &chosen, &full_board[..board.len() + to_come]);
    }
    Some(Equity {
        share: units as f64 / (samples as f64 * SHARE_UNITS as f64),
        sampled: true,
        tuples: None,
    })
}

fn combinations(n: u64, k: u64) -> u64 {
    if k > n {
        return 0;
    }
    let mut result = 1;
    for i in 0..k {
        result = result * (n - i) / (i + 1);
    }
    result
}

/// Counts card-disjoint tuples, stopping once `stop_at` is reached.
fn count_tuples(lists: &[Vec<Holding>], dead: u64, stop_at: u64) -> u64 {
    fn walk(lists: &[Vec<Holding>], dead: u64, count: &mut u64, stop_at: u64) {
        let Some((first, rest)) = lists.split_first() else {
            *count += 1;
            return;
        };
        for holding in first {
            let mask = holding_mask(holding);
            if dead & mask == 0 {
                walk(rest, dead | mask, count, stop_at);
                if *count >= stop_at {
                    return;
                }
            }
        }
    }
    let mut count = 0;
    walk(lists, dead, &mut count, stop_at);
    count
}

fn rank_with(holding: &Holding, board: &[Card]) -> HandRank {
    let mut seven = [holding[0]; 7];
    seven[1] = holding[1];
    seven[2..2 + board.len()].copy_from_slice(board);
    evaluate(&seven[..2 + board.len()])
}

/// Hero's share for one fixed outcome, in units of 1/12 of the pot.
fn outcome_units(hero: &Holding, opponents: &[Holding], board: &[Card]) -> u64 {
    let hero_rank = rank_with(hero, board);
    let mut tied = 1;
    for opponent in opponents {
        let rank = rank_with(opponent, board);
        if rank > hero_rank {
            return 0;
        }
        if rank == hero_rank {
            tied += 1;
        }
    }
    SHARE_UNITS / tied
}

fn enumerated(
    hero: Holding,
    board: &[Card],
    opponents: &[Vec<Holding>],
    to_come: usize,
    tuples: u64,
) -> Equity {
    struct State {
        units: u64,
        outcomes: u64,
    }

    fn runouts(
        state: &mut State,
        hero: &Holding,
        chosen: &[Holding],
        unseen: &[Card],
        full_board: &mut [Card; 5],
        filled: usize,
        remaining: usize,
    ) {
        if remaining == 0 {
            state.units += outcome_units(hero, chosen, &full_board[..filled]);
            state.outcomes += 1;
            return;
        }
        for i in 0..=unseen.len() - remaining {
            full_board[filled] = unseen[i];
            runouts(state, hero, chosen, &unseen[i + 1..], full_board, filled + 1, remaining - 1);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn tuples_walk(
        state: &mut State,
        hero: &Holding,
        lists: &[Vec<Holding>],
        chosen: &mut Vec<Holding>,
        dead: u64,
        full_board: &mut [Card; 5],
        filled: usize,
        to_come: usize,
    ) {
        let Some((first, rest)) = lists.split_first() else {
            let unseen = unseen_cards(dead);
            runouts(state, hero, chosen, &unseen, full_board, filled, to_come);
            return;
        };
        for holding in first {
            let mask = holding_mask(holding);
            if dead & mask == 0 {
                chosen.push(*holding);
                tuples_walk(state, hero, rest, chosen, dead | mask, full_board, filled, to_come);
                chosen.pop();
            }
        }
    }

    let dead = holding_mask(&hero) | mask_of(board);
    let mut state = State { units: 0, outcomes: 0 };
    let mut full_board = [hero[0]; 5];
    full_board[..board.len()].copy_from_slice(board);
    let mut chosen = Vec::with_capacity(opponents.len());
    tuples_walk(&mut state, &hero, opponents, &mut chosen, dead, &mut full_board, board.len(), to_come);
    Equity {
        share: state.units as f64 / (state.outcomes as f64 * SHARE_UNITS as f64),
        sampled: false,
        tuples: Some(tuples),
    }
}
