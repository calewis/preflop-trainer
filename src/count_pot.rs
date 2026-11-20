//! "Count the pot" (preflop): keep track of a 5/10 chip pot in a loose, passive
//! 4-to-6-handed home game. Plain chip numbers only, always multiples of 5.

use crate::item::{count_pot_rung_of_seed, Skill, COUNT_POT_RUNGS};
use crate::ladder::{right_wrong_points, right_wrong_rung, RIGHT_WRONG_GATE};
use crate::log::{Attempt, ItemRef, Mode};
use crate::rng::Rng;

pub const SMALL_BLIND: u32 = 5;
pub const BIG_BLIND: u32 = 10;
/// Largest pot dealt by the generator.
pub const MAX_DEALT_POT: u32 = 1000;
/// Largest chip count the answer input accepts.
pub const MAX_COUNT_POT_ANSWER: u32 = 2000;
/// Recent attempts at the current rung reported on Progress.
pub const COUNT_POT_RECENT_WINDOW: usize = 20;

const OPEN_SIZES: [u32; 4] = [30, 40, 50, 60];
const BIG_OPEN_SIZE: u32 = 100;
const THREE_BET_SIZES: [u32; 9] = [90, 100, 120, 140, 150, 160, 180, 200, 250];

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PotSeat {
    Utg,
    Mp,
    Co,
    Btn,
    Sb,
    Bb,
}

impl PotSeat {
    pub const fn short(self) -> &'static str {
        match self {
            PotSeat::Utg => "UTG",
            PotSeat::Mp => "MP",
            PotSeat::Co => "CO",
            PotSeat::Btn => "BTN",
            PotSeat::Sb => "SB",
            PotSeat::Bb => "BB",
        }
    }

    const fn index(self) -> usize {
        match self {
            PotSeat::Utg => 0,
            PotSeat::Mp => 1,
            PotSeat::Co => 2,
            PotSeat::Btn => 3,
            PotSeat::Sb => 4,
            PotSeat::Bb => 5,
        }
    }
}

/// Preflop acting order for a 4-, 5- or 6-handed table.
pub fn acting_seats(players: u8) -> &'static [PotSeat] {
    match players {
        4 => &[PotSeat::Co, PotSeat::Btn, PotSeat::Sb, PotSeat::Bb],
        5 => &[PotSeat::Utg, PotSeat::Co, PotSeat::Btn, PotSeat::Sb, PotSeat::Bb],
        _ => &[PotSeat::Utg, PotSeat::Mp, PotSeat::Co, PotSeat::Btn, PotSeat::Sb, PotSeat::Bb],
    }
}

/// Clockwise order starting from the small blind, used for the reveal breakdown.
pub fn reveal_seats(players: u8) -> &'static [PotSeat] {
    match players {
        4 => &[PotSeat::Sb, PotSeat::Bb, PotSeat::Co, PotSeat::Btn],
        5 => &[PotSeat::Sb, PotSeat::Bb, PotSeat::Utg, PotSeat::Co, PotSeat::Btn],
        _ => &[PotSeat::Sb, PotSeat::Bb, PotSeat::Utg, PotSeat::Mp, PotSeat::Co, PotSeat::Btn],
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PotActionKind {
    PostSb,
    PostBb,
    Fold,
    Check,
    Limp,
    Complete,
    Call,
    Raise(u32),
}

impl PotActionKind {
    pub const fn code(self) -> &'static str {
        match self {
            PotActionKind::PostSb | PotActionKind::PostBb => "post",
            PotActionKind::Fold => "fold",
            PotActionKind::Check => "check",
            PotActionKind::Limp | PotActionKind::Complete | PotActionKind::Call => "call",
            PotActionKind::Raise(_) => "raise",
        }
    }

    pub const fn is_voluntary(self) -> bool {
        !matches!(self, PotActionKind::PostSb | PotActionKind::PostBb)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct PotAction {
    pub seat: PotSeat,
    pub kind: PotActionKind,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CountPotQuestion {
    pub rung: u8,
    pub players: u8,
    pub hero: PotSeat,
    pub actions: Vec<PotAction>,
}

impl CountPotQuestion {
    pub fn voluntary_actions(&self) -> usize {
        self.actions.iter().filter(|a| a.kind.is_voluntary()).count()
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PotSummary {
    pub pot: u32,
    pub contributions: Vec<(PotSeat, u32)>,
    pub voluntary_actions: usize,
    pub raises: u8,
    pub remaining: usize,
}

impl PotSummary {
    /// One line listing every player who put chips in, e.g. `SB 5 · BB 40 · CO 40 · BTN 40 · You 60`.
    pub fn breakdown_line(&self, hero: PotSeat) -> String {
        self.contributions
            .iter()
            .map(|&(seat, amount)| format!("{} {amount}", seat_label(seat, hero)))
            .collect::<Vec<_>>()
            .join(" \u{00b7} ")
    }
}

pub fn seat_label(seat: PotSeat, hero: PotSeat) -> &'static str {
    if seat == hero {
        "You"
    } else {
        seat.short()
    }
}

pub fn action_text(action: PotAction, hero: PotSeat) -> String {
    let you = action.seat == hero;
    let who = seat_label(action.seat, hero);
    let verb = |plain: &'static str, third: &'static str| if you { plain } else { third };
    match action.kind {
        PotActionKind::PostSb => format!("{who} {} {SMALL_BLIND}", verb("post", "posts")),
        PotActionKind::PostBb => format!("{who} {} {BIG_BLIND}", verb("post", "posts")),
        PotActionKind::Fold => format!("{who} {}", verb("fold", "folds")),
        PotActionKind::Check => format!("{who} {}", verb("check", "checks")),
        PotActionKind::Limp => format!("{who} {}", verb("limp", "limps")),
        PotActionKind::Complete => format!("{who} {}", verb("complete", "completes")),
        PotActionKind::Call => format!("{who} {}", verb("call", "calls")),
        PotActionKind::Raise(to) => format!("{who} {} to {to}", verb("raise", "raises")),
    }
}

/// Replays a preflop action sequence and computes the exact pot and per-player
/// chip contributions. Validates legal betting rules and multiple-of-5 amounts.
pub fn replay_pot(players: u8, actions: &[PotAction]) -> Result<PotSummary, String> {
    if !(4..=6).contains(&players) {
        return Err(format!("players must be 4..=6, got {players}"));
    }
    let seats = acting_seats(players);
    let mut put = [0u32; 6];
    let mut folded = [false; 6];
    let mut current_bet = 0u32;
    let mut raises = 0u8;
    let mut voluntary_actions = 0usize;

    for (i, action) in actions.iter().enumerate() {
        if !seats.contains(&action.seat) {
            return Err(format!("seat {:?} not at {players}-handed table", action.seat));
        }
        let idx = action.seat.index();
        if folded[idx] {
            return Err(format!("seat {:?} acted after folding at index {i}", action.seat));
        }
        match action.kind {
            PotActionKind::PostSb => {
                if action.seat != PotSeat::Sb || put[idx] != 0 {
                    return Err("invalid small blind post".to_string());
                }
                put[idx] = SMALL_BLIND;
                current_bet = current_bet.max(SMALL_BLIND);
            }
            PotActionKind::PostBb => {
                if action.seat != PotSeat::Bb || put[idx] != 0 {
                    return Err("invalid big blind post".to_string());
                }
                put[idx] = BIG_BLIND;
                current_bet = current_bet.max(BIG_BLIND);
            }
            PotActionKind::Fold => {
                folded[idx] = true;
                voluntary_actions += 1;
            }
            PotActionKind::Check => {
                if put[idx] != current_bet {
                    return Err(format!("{:?} cannot check facing {current_bet} with {} in", action.seat, put[idx]));
                }
                voluntary_actions += 1;
            }
            PotActionKind::Limp => {
                if current_bet != BIG_BLIND || put[idx] != 0 {
                    return Err(format!("{:?} cannot limp with bet {current_bet} and {} in", action.seat, put[idx]));
                }
                put[idx] = BIG_BLIND;
                voluntary_actions += 1;
            }
            PotActionKind::Complete => {
                if action.seat != PotSeat::Sb || current_bet != BIG_BLIND || put[idx] != SMALL_BLIND {
                    return Err(format!("{:?} cannot complete with bet {current_bet} and {} in", action.seat, put[idx]));
                }
                put[idx] = BIG_BLIND;
                voluntary_actions += 1;
            }
            PotActionKind::Call => {
                if current_bet <= put[idx] {
                    return Err(format!("{:?} cannot call {current_bet} with {} already in", action.seat, put[idx]));
                }
                put[idx] = current_bet;
                voluntary_actions += 1;
            }
            PotActionKind::Raise(to) => {
                if to <= current_bet || !to.is_multiple_of(5) {
                    return Err(format!("invalid raise to {to} over {current_bet}"));
                }
                put[idx] = to;
                current_bet = to;
                raises = raises.saturating_add(1);
                voluntary_actions += 1;
            }
        }
    }

    let contributions: Vec<(PotSeat, u32)> = reveal_seats(players)
        .iter()
        .copied()
        .filter_map(|seat| {
            let amount = put[seat.index()];
            (amount > 0).then_some((seat, amount))
        })
        .collect();
    let pot: u32 = contributions.iter().map(|&(_, a)| a).sum();
    let remaining = seats.iter().filter(|&&s| !folded[s.index()]).count();
    Ok(PotSummary { pot, contributions, voluntary_actions, raises, remaining })
}

pub fn pot_summary(q: &CountPotQuestion) -> PotSummary {
    replay_pot(q.players, &q.actions).expect("generated count_pot questions always replay cleanly")
}

/// Family key: rung and betting shape (`limp`, `raise`, or `reraise`).
pub fn count_pot_family(q: &CountPotQuestion) -> String {
    let summary = pot_summary(q);
    let shape = match summary.raises {
        0 => "limp",
        1 => "raise",
        _ => "reraise",
    };
    format!("{}:{shape}", q.rung)
}

/// One attempt's rung, from its seed.
fn rung_of(attempt: &Attempt) -> Option<u8> {
    match attempt.item {
        ItemRef::Seed(seed) => Some(count_pot_rung_of_seed(seed)),
        ItemRef::Baseline(_) => None,
    }
}

fn rung_attempts(attempts: &[Attempt], rung: u8) -> impl Iterator<Item = &Attempt> {
    attempts.iter().filter(move |a| a.skill == Skill::CountPot && a.mode == Mode::Drill && rung_of(a) == Some(rung))
}

/// The player's current rung, using the shared points ladder.
pub fn count_pot_rung(attempts: &[Attempt]) -> u8 {
    right_wrong_rung(COUNT_POT_RUNGS, |rung| rung_attempts(attempts, rung))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CountPotStatus {
    pub rung: u8,
    pub points: i32,
    pub recent_right: usize,
    pub recent_total: usize,
}

pub fn count_pot_status(attempts: &[Attempt]) -> CountPotStatus {
    let rung = count_pot_rung(attempts);
    let results: Vec<bool> = rung_attempts(attempts, rung).map(|a| a.correct).collect();
    let points = RIGHT_WRONG_GATE.status(results.iter().map(|&right| right_wrong_points(right))).points;
    let recent = &results[results.len().saturating_sub(COUNT_POT_RECENT_WINDOW)..];
    CountPotStatus {
        rung,
        points,
        recent_right: recent.iter().filter(|&&right| right).count(),
        recent_total: recent.len(),
    }
}

pub fn rung_label(rung: u8) -> &'static str {
    match rung {
        0 => "Short hands (3-5 actions)",
        1 => "Raises and limpers (5-7 actions)",
        _ => "Raises and re-raises (up to 10 actions)",
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TargetShape {
    Limped,
    SingleRaise,
    ReRaise,
}

fn pick_open_size(rng: &mut Rng, allow_big: bool) -> u32 {
    if allow_big && rng.chance(1, 10) {
        BIG_OPEN_SIZE
    } else {
        rng.pick(&OPEN_SIZES)
    }
}

fn pick_three_bet_size(rng: &mut Rng, open: u32) -> Option<u32> {
    let min_to = (open * 5 / 2).max(open + 30);
    let max_to = (open * 4).min(250);
    let choices: Vec<u32> = THREE_BET_SIZES.iter().copied().filter(|&s| s >= min_to && s <= max_to).collect();
    if choices.is_empty() {
        None
    } else {
        Some(rng.pick(&choices))
    }
}

/// Generates a preflop pot-counting question at the seed's rung.
pub fn gen_count_pot(rng: &mut Rng, seed: u64) -> CountPotQuestion {
    let rung = count_pot_rung_of_seed(seed);
    let shape = match rung {
        0 => {
            if rng.chance(55, 100) {
                TargetShape::Limped
            } else {
                TargetShape::SingleRaise
            }
        }
        1 => {
            if rng.chance(15, 100) {
                TargetShape::Limped
            } else {
                TargetShape::SingleRaise
            }
        }
        _ => {
            if rng.chance(65, 100) {
                TargetShape::ReRaise
            } else {
                TargetShape::SingleRaise
            }
        }
    };
    let (min_acts, max_acts) = match rung {
        0 => (3usize, 5usize),
        1 => (5, 7),
        _ => (6, 10),
    };
    // On rung 0, some hands stop right before hero acts in the big blind (giving 3-action pots too).
    let pause_before_hero = rung == 0 && rng.chance(1, 3);

    loop {
        let players: u8 = match rung {
            0 => rng.range(4, 5) as u8,
            1 => rng.range(4, 6) as u8,
            _ => if rng.chance(85, 100) { rng.range(5, 6) as u8 } else { 4 },
        };
        let seats = acting_seats(players);
        let hero = if pause_before_hero { PotSeat::Bb } else { rng.pick(seats) };
        let Some(actions) = simulate_preflop(rng, players, hero, rung, shape, pause_before_hero, max_acts) else {
            continue;
        };
        let Ok(summary) = replay_pot(players, &actions) else {
            continue;
        };
        if !(min_acts..=max_acts).contains(&summary.voluntary_actions) {
            continue;
        }
        if summary.pot > MAX_DEALT_POT || summary.pot < 20 || !summary.pot.is_multiple_of(5) {
            continue;
        }
        if summary.remaining < 2 {
            continue;
        }
        // Hero must appear at least once so "You" is visibly placed in the strip.
        if !actions.iter().any(|a| a.seat == hero) {
            continue;
        }
        let shape_ok = match shape {
            TargetShape::Limped => summary.raises == 0 && summary.pot >= 25,
            TargetShape::SingleRaise => summary.raises == 1,
            TargetShape::ReRaise => summary.raises == 2,
        };
        if !shape_ok {
            continue;
        }
        return CountPotQuestion { rung, players, hero, actions };
    }
}

fn simulate_preflop(
    rng: &mut Rng,
    players: u8,
    hero: PotSeat,
    rung: u8,
    shape: TargetShape,
    pause_before_hero: bool,
    max_acts: usize,
) -> Option<Vec<PotAction>> {
    let seats = acting_seats(players);
    let mut actions = vec![
        PotAction { seat: PotSeat::Sb, kind: PotActionKind::PostSb },
        PotAction { seat: PotSeat::Bb, kind: PotActionKind::PostBb },
    ];
    let mut put = [0u32; 6];
    let mut folded = [false; 6];
    let mut acted = [false; 6];
    put[PotSeat::Sb.index()] = SMALL_BLIND;
    put[PotSeat::Bb.index()] = BIG_BLIND;
    let mut current_bet = BIG_BLIND;
    let mut raises = 0u8;
    let mut limpers = 0u8;
    let mut voluntary = 0usize;
    let mut pos = 0usize;
    let mut open_size = 0u32;
    let mut callers_of_raise = 0u8;
    let mut callers_of_3bet = 0u8;

    let want_limp_before_raise = shape != TargetShape::Limped && rung >= 1 && rng.chance(60, 100);
    let allow_big_open = rung >= 1 && shape != TargetShape::ReRaise;

    for _ in 0..24 {
        let active_count = seats.iter().filter(|&&s| !folded[s.index()]).count();
        if active_count < 2 {
            return None;
        }
        let closed = seats.iter().all(|&s| {
            let idx = s.index();
            folded[idx] || (acted[idx] && put[idx] == current_bet)
        });
        if closed {
            break;
        }
        if voluntary >= max_acts {
            return None;
        }

        let seat = seats[pos % seats.len()];
        pos += 1;
        let idx = seat.index();
        if folded[idx] || (acted[idx] && put[idx] == current_bet) {
            continue;
        }

        // Optional mid-round pause right before hero acts in BB on rung 0 (3+ actions already).
        if pause_before_hero && seat == hero && voluntary >= 3 {
            break;
        }

        let kind = if raises == 0 {
            if seat == PotSeat::Bb && put[idx] == current_bet {
                if shape != TargetShape::Limped && limpers > 0 && rng.chance(35, 100) {
                    let to = pick_open_size(rng, allow_big_open);
                    open_size = to;
                    PotActionKind::Raise(to)
                } else {
                    PotActionKind::Check
                }
            } else if seat == PotSeat::Sb {
                if shape != TargetShape::Limped && raises == 0 && rng.chance(35, 100) {
                    let to = pick_open_size(rng, allow_big_open);
                    open_size = to;
                    PotActionKind::Raise(to)
                } else if rng.chance(72, 100) {
                    PotActionKind::Complete
                } else {
                    PotActionKind::Fold
                }
            } else {
                // Non-blind seat facing no raise yet.
                let can_raise = shape != TargetShape::Limped && (!want_limp_before_raise || limpers >= 1);
                let raise_pct = if !can_raise {
                    0
                } else if seat == PotSeat::Btn {
                    70
                } else if limpers >= 1 {
                    50
                } else {
                    35
                };
                if raise_pct > 0 && rng.chance(raise_pct, 100) {
                    let to = pick_open_size(rng, allow_big_open);
                    open_size = to;
                    PotActionKind::Raise(to)
                } else if limpers < 3 && rng.chance(62, 100) {
                    limpers += 1;
                    PotActionKind::Limp
                } else {
                    PotActionKind::Fold
                }
            }
        } else if raises == 1 {
            // Facing an open raise.
            let can_3bet = shape == TargetShape::ReRaise && (acted[idx] || callers_of_raise >= 1 || seat == PotSeat::Btn || seat == PotSeat::Sb || seat == PotSeat::Bb);
            let want_3bet = can_3bet && rng.chance(if seat == PotSeat::Bb || seat == PotSeat::Btn { 55 } else { 35 }, 100);
            if want_3bet {
                if let Some(to) = pick_three_bet_size(rng, open_size) {
                    PotActionKind::Raise(to)
                } else {
                    PotActionKind::Call
                }
            } else {
                // Loose-passive home game: limpers and blinds call often, with folds mixed in.
                let call_pct = if put[idx] == BIG_BLIND && seat != PotSeat::Bb {
                    // Limper facing a raise: mix of calling the difference and limp-folding!
                    58
                } else if seat == PotSeat::Bb {
                    62
                } else if seat == PotSeat::Sb {
                    42
                } else {
                    52
                };
                if rng.chance(call_pct, 100) {
                    callers_of_raise += 1;
                    PotActionKind::Call
                } else {
                    PotActionKind::Fold
                }
            }
        } else {
            // Facing a re-raise (3-bet): 1-2 callers, no 4-bet so pots stay under 1000.
            let call_pct = if callers_of_3bet >= 2 {
                10
            } else if put[idx] >= open_size {
                65
            } else {
                35
            };
            if rng.chance(call_pct, 100) {
                callers_of_3bet += 1;
                PotActionKind::Call
            } else {
                PotActionKind::Fold
            }
        };

        match kind {
            PotActionKind::Fold => folded[idx] = true,
            PotActionKind::Check => {}
            PotActionKind::Limp | PotActionKind::Complete => put[idx] = BIG_BLIND,
            PotActionKind::Call => put[idx] = current_bet,
            PotActionKind::Raise(to) => {
                put[idx] = to;
                current_bet = to;
                raises += 1;
            }
            PotActionKind::PostSb | PotActionKind::PostBb => unreachable!(),
        }
        acted[idx] = true;
        voluntary += 1;
        actions.push(PotAction { seat, kind });
    }

    Some(actions)
}
