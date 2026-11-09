//! Hand simulator: deals a hand, plays villains from sampled models, hero by
//! the preflop card and a simple solid postflop heuristic, and captures at
//! most one hero postflop decision per hand.

use crate::board::classify;
use crate::combos::combo_index;
use crate::game::{Act, Round, EPS};
use crate::models::{PlayerModel, NBUCKET};
use crate::spot::{clamp_preflop_raise, position_of, preflop_can_raise, HistAct, PreflopTrack, Spot};
use poker_trainer::cards::{Card, Holding};
use poker_trainer::preflop::{preflop_answer, Action, Situation};
use poker_trainer::rng::Rng;

/// Table sizes and weights from the brief.
pub const TABLE_SIZES: [(usize, u64); 4] = [(4, 40), (5, 40), (6, 10), (7, 10)];
pub const STACKS: [f64; 4] = [60.0, 100.0, 150.0, 200.0];

/// Capture probability per hero postflop decision by street, so captures
/// spread across streets and early capture does not end every hand on the flop.
pub const CAPTURE_PROB: [f64; 3] = [0.35, 0.45, 1.0];

pub struct SimResult {
    pub spot: Spot,
    /// Street of the capture: 1 flop, 2 turn, 3 river.
    pub street: u8,
    /// The table was drawn from the recalibrated population (mixture only).
    pub recalibrated: bool,
}

fn pick_weighted(rng: &mut Rng, items: &[(usize, u64)]) -> usize {
    let total: u64 = items.iter().map(|x| x.1).sum();
    let mut u = rng.below(total);
    for &(v, w) in items {
        if u < w {
            return v;
        }
        u -= w;
    }
    items.last().unwrap().0
}

fn unit(rng: &mut Rng) -> f64 {
    (rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64
}

fn sample_col(rng: &mut Rng, p: [f32; 3]) -> usize {
    let u = unit(rng) * (p[0] + p[1] + p[2]) as f64;
    if u < p[0] as f64 {
        0
    } else if u < (p[0] + p[1]) as f64 {
        1
    } else {
        2
    }
}

/// Hero's preflop raise-to size: open 3bb plus one per limper, 3x a raise.
pub fn hero_preflop_raise(r: &Round) -> f64 {
    if r.raises == 0 {
        let limpers = (r.pot - 1.5).max(0.0);
        3.0 + limpers
    } else {
        r.current * 3.0
    }
}

pub fn hero_preflop(r: &Round, track: &PreflopTrack, n: usize, seat: usize, hand: Holding) -> Act {
    let situation = if r.raises >= 1 {
        Situation::Raised
    } else if track.limpers > 0 {
        Situation::Limpers(track.limpers)
    } else {
        Situation::FoldedTo
    };
    let position = position_of(n, seat);
    let (action, _) = preflop_answer(position, situation, hand);
    let facing = r.is_facing(seat);
    match action {
        Action::Fold if !facing => Act::Check,
        Action::Fold => Act::Fold,
        Action::Call => {
            if facing {
                Act::Call
            } else {
                Act::Check
            }
        }
        Action::Raise => {
            if preflop_can_raise(r, seat) && r.raises < 3 {
                let to = clamp_preflop_raise(r, seat, hero_preflop_raise(r));
                Act::Raise(to.min(r.stacks[seat] + r.put[seat]))
            } else if facing {
                Act::Call
            } else {
                Act::Check
            }
        }
    }
}

/// A simple solid postflop rule for hero on streets before the graded one.
pub fn hero_heuristic(r: &Round, seat: usize, bucket: usize, draw: bool, street: usize, rng: &mut Rng) -> Act {
    if !r.is_facing(seat) {
        let bet = bucket <= 2 || (draw && street < 2 && rng.below(2) == 0);
        if bet {
            let to = r.bet_to(seat, 2.0 / 3.0);
            if to > EPS {
                return Act::Bet(to);
            }
        }
        Act::Check
    } else {
        if bucket == 0 && r.raises < 2 {
            if let Some(to) = r.raise_to_3x(seat) {
                return Act::Raise(to);
            }
        }
        let sit = r.situation(seat);
        let call = bucket <= 1
            || (bucket == 2 && sit <= 2)
            || (bucket == 3 && sit == 1)
            || (draw && street < 2 && sit <= 2);
        if call {
            Act::Call
        } else {
            Act::Fold
        }
    }
}

pub fn villain_postflop(
    r: &Round,
    seat: usize,
    class: usize,
    tables_street: usize,
    tables: &crate::models::VillainTables,
    rng: &mut Rng,
) -> Act {
    let facing = r.is_facing(seat);
    let rl = usize::from(r.raises).min(2);
    let mw = (r.other_active(seat) - 1).min(2);
    let sit = r.situation(seat);
    let row = tables.row(tables_street, mw, rl, sit);
    let p = row[class];
    if !facing {
        if unit(rng) < p[1] as f64 {
            Act::Bet(r.bet_to(seat, f64::from(tables.bet_frac[tables_street])))
        } else {
            Act::Check
        }
    } else {
        let raise_to = if r.raises < 2 { r.raise_to_3x(seat) } else { None };
        match raise_to {
            Some(to) => match sample_col(rng, p) {
                0 => Act::Fold,
                1 => Act::Call,
                _ => Act::Raise(to),
            },
            None => match sample_col(rng, p) {
                0 => Act::Fold,
                _ => Act::Call,
            },
        }
    }
}

pub fn villain_preflop(r: &Round, track: &PreflopTrack, n: usize, seat: usize, hand: Holding, model: &PlayerModel, rng: &mut Rng) -> Act {
    let ctx = track.ctx(r, seat, n);
    let row = model.preflop_row(&ctx);
    let class = crate::combos::class_of(hand[0], hand[1]) as usize;
    let p = row[class];
    let col = sample_col(rng, p);
    let facing = r.is_facing(seat);
    match col {
        0 if facing => Act::Fold,
        0 => Act::Check,
        1 => {
            if facing {
                Act::Call
            } else {
                Act::Check
            }
        }
        _ => {
            if preflop_can_raise(r, seat) {
                let to = clamp_preflop_raise(r, seat, model.preflop_raise_to(r));
                Act::Raise(to.min(r.stacks[seat] + r.put[seat]))
            } else if facing {
                Act::Call
            } else {
                Act::Check
            }
        }
    }
}

/// Plays one preflop with every seat a sampled model: (archetype, vpip, pfr) by seat.
pub fn preflop_population(seed: u64) -> Vec<(usize, bool, bool)> {
    let mut rng = Rng::new(seed);
    let n = pick_weighted(&mut rng, &TABLE_SIZES);
    let stack = STACKS[rng.below(STACKS.len() as u64) as usize];
    let mut deck: Vec<Card> = (0..52u8).map(Card::from_index).collect();
    rng.shuffle(&mut deck);
    let holdings: Vec<Holding> = (0..n).map(|i| [deck[2 * i], deck[2 * i + 1]]).collect();
    let models: Vec<PlayerModel> = (0..n).map(|_| PlayerModel::sample(&mut rng)).collect();
    let mut r = Round::preflop(n, stack);
    let mut track = PreflopTrack::default();
    let mut vpip = vec![false; n];
    let mut pfr = vec![false; n];
    while r.to_act != 0 {
        let seat = r.next as usize;
        let act = villain_preflop(&r, &track, n, seat, holdings[seat], &models[seat], &mut rng);
        match act {
            Act::Call => vpip[seat] = true,
            Act::Raise(_) => {
                vpip[seat] = true;
                pfr[seat] = true;
            }
            _ => {}
        }
        track.note(&r, seat, act);
        r.apply(seat, act);
    }
    (0..n).map(|s| (models[s].archetype, vpip[s], pfr[s])).collect()
}

/// Simulates one hand from a seed, from the population this process uses.
pub fn simulate(seed: u64) -> Option<SimResult> {
    simulate_with(seed, crate::preflop_cal::active())
}

/// Simulates one hand. With `mixture`, the whole table is drawn 50/50 from the
/// base population and that recalibrated one; otherwise from the base population.
/// Returns a spot if hero reaches a captured postflop decision.
pub fn simulate_with(seed: u64, mixture: Option<&'static crate::preflop_cal::Calibration>) -> Option<SimResult> {
    let mut rng = Rng::new(seed);
    let n = pick_weighted(&mut rng, &TABLE_SIZES);
    let stack = STACKS[rng.below(STACKS.len() as u64) as usize];
    let hero_seat = rng.below(n as u64) as usize;

    let mut deck: Vec<Card> = (0..52u8).map(Card::from_index).collect();
    rng.shuffle(&mut deck);
    let holdings: Vec<Holding> = (0..n).map(|i| [deck[2 * i], deck[2 * i + 1]]).collect();
    let board_all: Vec<Card> = deck[2 * n..2 * n + 5].to_vec();

    let table_cal = mixture.filter(|_| rng.below(2) == 1);
    let models: Vec<PlayerModel> = (0..n)
        .map(|_| match table_cal {
            Some(cal) => PlayerModel::sample_cal(&mut rng, cal),
            None => PlayerModel::sample(&mut rng),
        })
        .collect();

    let mut r = Round::preflop(n, stack);
    let mut track = PreflopTrack::default();
    let mut history: Vec<HistAct> = Vec::new();

    // Preflop.
    while r.to_act != 0 {
        let seat = r.next as usize;
        let act = if seat == hero_seat {
            hero_preflop(&r, &track, n, seat, holdings[seat])
        } else {
            villain_preflop(&r, &track, n, seat, holdings[seat], &models[seat], &mut rng)
        };
        track.note(&r, seat, act);
        history.push(HistAct { street: 0, seat: seat as u8, act });
        r.apply(seat, act);
        if r.in_hand >> hero_seat & 1 == 0 {
            return None;
        }
    }
    if r.in_hand.count_ones() == 1 {
        return None;
    }

    let tables: Vec<_> = (0..n).map(|s| models[s].tables()).collect();

    // Postflop streets.
    for street_idx in 1..=3 {
        r = r.next_street();
        let board_len = 2 + street_idx;
        let board = &board_all[..board_len];
        let classes = classify(board, street_idx < 3);
        let class_of = |seat: usize| classes[combo_index(holdings[seat][0], holdings[seat][1])] as usize;
        while r.to_act != 0 {
            let seat = r.next as usize;
            let act = if seat == hero_seat {
                let (bucket, draw) = {
                    let c = class_of(seat);
                    (c % NBUCKET, c >= NBUCKET)
                };
                if unit(&mut rng) < CAPTURE_PROB[street_idx - 1] && r.other_active(seat) >= 1 {
                    return Some(SimResult {
                        spot: Spot {
                            n,
                            hero_seat,
                            stack,
                            hero: holdings[seat],
                            board: board.to_vec(),
                            history,
                        },
                        street: street_idx as u8,
                        recalibrated: table_cal.is_some(),
                    });
                }
                hero_heuristic(&r, seat, bucket, draw, street_idx - 1, &mut rng)
            } else {
                villain_postflop(&r, seat, class_of(seat), street_idx - 1, &tables[seat], &mut rng)
            };
            history.push(HistAct { street: street_idx as u8, seat: seat as u8, act });
            r.apply(seat, act);
            if r.in_hand >> hero_seat & 1 == 0 {
                return None;
            }
            if r.in_hand.count_ones() == 1 {
                return None;
            }
        }
    }
    None
}
