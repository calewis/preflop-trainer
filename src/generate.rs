//! Deterministic item generators. A seed always regenerates the same item.

use crate::anchors;
use crate::board_type::{answer_options, category_of_seed, classify_flop};
use crate::beats_me::{gen_beats_me, mix_for, MixBand};
use crate::cards::{holding_mask, mask_of, unseen_holdings, Card, Holding, ACE};
use crate::classes::{
    is_flush_draw, is_oesd, made_class, matches_opp_class, mix_combos, rank_of, worth_a_decision, MadeClass, OppClass, OppMix, PlayerType,
};
use crate::equity::{equity_with_cards_to_come, sampled, Equity};
use crate::equity_skill::EQUITY_BAND_START;
use crate::grade::{beats_analysis, chain_equity, equity_analysis, nuts_analysis, outs_analysis, BeatsBucket};
use crate::item::{
    beats_me_rung_of_seed, chain_hand_seed, chain_stage_of_seed, equity_rung_of_seed, ChainHand, ChainQuestion, EquityOpponents, EquityQuestion,
    HitStreet, Item, Question, Skill, SpotMode, SpotQuestion,
};
use crate::money;
use crate::preflop::{Range, Seat, Situation, CARD};
use crate::rng::Rng;
use crate::table;

/// Pots before the bet, all divisible by 12 so every bet size is a whole dollar.
pub const POTS: [u32; 12] = [60, 72, 84, 96, 120, 144, 180, 240, 300, 360, 480, 600];
/// Bet sizes as fractions of the pot before the bet.
pub const BET_SIZES: [(u32, u32); 8] = [
    (1, 4),
    (1, 3),
    (1, 2),
    (2, 3),
    (3, 4),
    (1, 1),
    (3, 2),
    (2, 1),
];
pub const HIT_OUTS: [u32; 8] = [2, 4, 6, 8, 9, 10, 12, 15];

/// A spot's price stays within this many points of break-even.
pub const SPOT_NEAR_PCT: f64 = 0.20;
/// Prices closer than this to break-even don't count as being on either side.
const SPOT_MIN_MARGIN: f64 = 0.01;
/// Samples for the quick share estimate that screens spot candidates.
const SPOT_QUICK_SAMPLES: u64 = 2_000;
/// Samples for the first look at a hand that has to be foldable.
const SPOT_COARSE_SAMPLES: u64 = 400;
/// Coarse rejections before settling for any near price.
const SPOT_COARSE_TRIES: u32 = 2_000;
/// Room for sampling error when ruling out a fold from the coarse estimate.
const SPOT_COARSE_SLACK: f64 = 0.08;
/// Extra room left for the quick estimate before the exact figure decides.
const SPOT_QUICK_SLACK: f64 = 0.02;
/// Hero's all-in equity stays inside this range, so no spot is one hero can't
/// win or can't lose.
pub const SPOT_MIN_SHARE: f64 = 0.15;
pub const SPOT_MAX_SHARE: f64 = 0.85;
/// Candidate hands tried for the chosen side before settling for any near price.
const SPOT_SIDE_TRIES: u32 = 300;
/// Hero's share in a chain's last stage stays inside this range.
pub const CHAIN_MIN_SHARE: f64 = 0.08;
pub const CHAIN_MAX_SHARE: f64 = 0.92;
/// Extra margin when the share was only estimated from a multiway sample.
const MW_SHARE_MARGIN: f64 = 0.005;
/// Fewest combos an assumed class may have.
pub const MIN_CLASS_COMBOS: usize = 3;
/// Samples for screening multiway candidates during generation only.
const MW_SCREEN_SAMPLES: u64 = 40_000;
/// Screening margin below 50% so a sampled multiway equity is reliably under it.
const MW_NEXT_CARD_LIMIT: f64 = 0.48;

/// True equity of an `equity` item stays inside this range, in percent. Starting guesses.
pub const EQUITY_MIN_PCT: f64 = 5.0;
pub const EQUITY_MAX_PCT: f64 = 95.0;
/// Opponent counts for `equity`, out of 100.
const EQUITY_OPPONENTS: [(usize, u64); 3] = [(1, 50), (2, 35), (3, 15)];
/// Attempts to deal disjoint exact hands from the chosen classes before redealing.
const EQUITY_EXACT_TRIES: u32 = 40;

/// Fewest and most outs a chain's opponent hand may leave, so a chain has real outs.
pub const CHAIN_MIN_OUTS: usize = 4;
pub const CHAIN_MAX_OUTS: usize = 17;
/// Chains targeted near break-even, out of 100. Starting guess.
const CHAIN_TARGETED_PCT: u64 = 70;

const DRILL_TABLE: [(u8, u64); 4] = [(4, 40), (5, 40), (6, 10), (7, 10)];

fn skill_rng(skill: Skill, seed: u64) -> Rng {
    Rng::new(seed ^ (skill as u64 + 1).wrapping_mul(0xA24B_AED4_963E_E407))
}

/// The table size for a drill item: 4 and 5 players at 40% each, 6 and 7 at 10%
/// each. It depends only on the seed, so an item can always be regenerated.
pub fn drill_players(seed: u64) -> u8 {
    let mut rng = Rng::new(seed ^ 0x7461_626C_6573_697A);
    let mut roll = rng.below(DRILL_TABLE.iter().map(|&(_, w)| w).sum());
    for (players, weight) in DRILL_TABLE {
        if roll < weight {
            return players;
        }
        roll -= weight;
    }
    unreachable!("the roll is below the total weight")
}

/// Generates the item for (skill, seed) at the current [`crate::item::GEN_VERSION`].
pub fn generate(skill: Skill, seed: u64) -> Item {
    if skill == Skill::Preflop {
        return generate_preflop(seed, drill_players(seed));
    }
    let mut rng = skill_rng(skill, seed);
    let question = match skill {
        Skill::Preflop => unreachable!("handled above"),
        Skill::Nuts => gen_nuts(&mut rng),
        Skill::Possible => Question::Possible { board: gen_textured_board(&mut rng) },
        Skill::Made => gen_made(&mut rng),
        Skill::Beats => gen_beats(&mut rng),
        Skill::Outs => gen_outs(&mut rng),
        Skill::Price => gen_price(&mut rng),
        Skill::Hit => gen_hit(&mut rng),
        Skill::Spot => Question::Spot(gen_spot(&mut rng, 1, seed)),
        Skill::SpotMw => {
            let opponents = rng.range(2, 3) as usize;
            Question::Spot(gen_spot(&mut rng, opponents, seed))
        }
        Skill::BoardType => gen_board_type(&mut rng, seed),
        Skill::Equity => Question::Equity(gen_equity(&mut rng, seed)),
        Skill::Anchor => Question::Anchor { card: anchors::card_of_seed(seed) as u16 },
        Skill::Table => Question::Table(table::question_for_seed(seed)),
        Skill::Chain => Question::Chain(gen_chain(seed)),
        Skill::BeatsMe => Question::BeatsMe(gen_beats_me(&mut rng, seed)),
    };
    Item { skill, seed, question }
}

/// A preflop item at a table of `players` (at least 2).
pub fn generate_preflop(seed: u64, players: u8) -> Item {
    assert!(players >= 2, "a preflop item needs at least 2 players");
    let mut rng = skill_rng(Skill::Preflop, seed);
    let behind = rng.below(u64::from(players)) as u8;
    let acted = players - 1 - behind;
    let seat = Seat::from_behind(behind);
    let situation = gen_situation(&mut rng, seat, acted);

    let hand = if rng.chance(1, 2) {
        let range = relevant_range(seat, situation);
        let combos: Vec<Holding> = unseen_holdings(0)
            .into_iter()
            .filter(|&h| range.contains_holding(h))
            .collect();
        rng.pick(&combos)
    } else {
        deal_holding(&mut rng, 0)
    };
    Item {
        skill: Skill::Preflop,
        seed,
        question: Question::Preflop { players, behind, situation, hand },
    }
}

/// Only possible situations: first to act is only ever folded to, the big blind
/// is never folded to, and limpers never exceed 3 or the players who acted first.
fn gen_situation(rng: &mut Rng, seat: Seat, acted: u8) -> Situation {
    if acted == 0 {
        return Situation::FoldedTo;
    }
    let limpers = |rng: &mut Rng| Situation::Limpers(rng.range(1, u64::from(acted.min(3))) as u8);
    if seat == Seat::BigBlind {
        return if rng.chance(1, 2) { limpers(rng) } else { Situation::Raised };
    }
    match rng.below(10) {
        0..=3 => Situation::FoldedTo,
        4..=6 => limpers(rng),
        _ => Situation::Raised,
    }
}

fn relevant_range(seat: Seat, situation: Situation) -> Range {
    match situation {
        Situation::FoldedTo => match (CARD.open_range(seat), CARD.open_range(seat.next_looser())) {
            (Some(own), Some(looser)) => own.range.union(&looser.range),
            _ => CARD.iso_raise.range.union(&CARD.iso_limp.range),
        },
        Situation::Limpers(_) => CARD.iso_raise.range.union(&CARD.iso_limp.range),
        Situation::Raised => CARD.reraise.range.union(&CARD.call.range),
    }
}

fn deal_card(rng: &mut Rng, dead: &mut u64) -> Card {
    loop {
        let card = Card::from_index(rng.below(52) as u8);
        if *dead & card.mask() == 0 {
            *dead |= card.mask();
            return card;
        }
    }
}

pub(crate) fn deal_holding(rng: &mut Rng, dead: u64) -> Holding {
    let mut used = dead;
    let first = deal_card(rng, &mut used);
    let second = deal_card(rng, &mut used);
    if first.index() > second.index() {
        [first, second]
    } else {
        [second, first]
    }
}

pub(crate) fn deal_board(rng: &mut Rng, len: usize, dead: &mut u64) -> Vec<Card> {
    (0..len).map(|_| deal_card(rng, dead)).collect()
}

/// A board's texture, in the terms the `nuts` and `possible` skills oversample.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BoardTexture {
    pub paired: bool,
    pub flush_possible: bool,
    pub straight_possible: bool,
}

impl BoardTexture {
    pub fn is_dry(self) -> bool {
        !self.paired && !self.flush_possible && !self.straight_possible
    }
}

pub fn board_texture(board: &[Card]) -> BoardTexture {
    let mut suits = [0u8; 4];
    let mut ranks = 0u16;
    let mut paired = false;
    for card in board {
        suits[card.suit() as usize] += 1;
        let bit = 1u16 << card.rank();
        paired |= ranks & bit != 0;
        ranks |= bit;
    }
    // Positions 0..=13 count the ace at both ends of the ladder.
    let ladder = (u32::from(ranks) << 1) | u32::from(ranks >> ACE & 1);
    let straight_possible = (0..=9).any(|low| (ladder >> low & 0b1_1111).count_ones() >= 3);
    BoardTexture {
        paired,
        flush_possible: suits.iter().any(|&n| n >= 3),
        straight_possible,
    }
}

/// 60% of boards are paired, flush-possible or straight-possible (a third each);
/// the other 40% have none of those, which also makes `none` a live answer.
fn gen_textured_board(rng: &mut Rng) -> Vec<Card> {
    let len = rng.range(3, 5) as usize;
    let kind = rng.below(10);
    loop {
        let mut dead = 0;
        let board = deal_board(rng, len, &mut dead);
        let texture = board_texture(&board);
        let wanted = match kind {
            0 | 1 => texture.paired,
            2 | 3 => texture.flush_possible,
            4 | 5 => texture.straight_possible,
            _ => texture.is_dry(),
        };
        if wanted {
            return board;
        }
    }
}

fn gen_nuts(rng: &mut Rng) -> Question {
    loop {
        let board = gen_textured_board(rng);
        if !nuts_analysis(&board).all_tie {
            return Question::Nuts { board };
        }
    }
}

fn deal_street(rng: &mut Rng) -> (Vec<Card>, u64) {
    let mut dead = 0;
    let len = rng.range(3, 5) as usize;
    let board = deal_board(rng, len, &mut dead);
    (board, dead)
}

/// Answer weights for `made`, so results are not mostly high card or one pair.
const MADE_TARGETS: [(MadeClass, u64); 14] = [
    (MadeClass::HighCard, 4),
    (MadeClass::BoardPair, 4),
    (MadeClass::LowPair, 4),
    (MadeClass::MidPair, 4),
    (MadeClass::TopPair, 4),
    (MadeClass::OverPair, 4),
    (MadeClass::TwoPair, 4),
    (MadeClass::Set, 4),
    (MadeClass::Trips, 4),
    (MadeClass::Straight, 4),
    (MadeClass::Flush, 4),
    (MadeClass::FullHouse, 4),
    (MadeClass::Quads, 2),
    (MadeClass::StraightFlush, 1),
];
const MADE_ATTEMPTS: u32 = 1_000_000;

fn gen_made(rng: &mut Rng) -> Question {
    let total: u64 = MADE_TARGETS.iter().map(|&(_, w)| w).sum();
    let mut roll = rng.below(total);
    let mut start = 0;
    for (i, &(_, weight)) in MADE_TARGETS.iter().enumerate() {
        if roll < weight {
            start = i;
            break;
        }
        roll -= weight;
    }
    // Rare targets can run out of attempts; then fall through to the next one.
    for offset in 0..MADE_TARGETS.len() {
        let target = MADE_TARGETS[(start + offset) % MADE_TARGETS.len()].0;
        for _ in 0..MADE_ATTEMPTS {
            let (board, dead) = deal_street(rng);
            let hero = deal_holding(rng, dead);
            if made_class(hero, &board) == target {
                return Question::Made { hero, board };
            }
        }
    }
    unreachable!("high card and pairs are always found")
}

fn gen_beats(rng: &mut Rng) -> Question {
    let target = match rng.below(4) {
        0 => BeatsBucket::A,
        1 => BeatsBucket::B,
        2 => BeatsBucket::C,
        _ => BeatsBucket::D,
    };
    loop {
        let (board, dead) = deal_street(rng);
        let hero = deal_holding(rng, dead);
        if beats_analysis(hero, &board).bucket == target {
            return Question::Beats { hero, board };
        }
    }
}

fn gen_outs(rng: &mut Rng) -> Question {
    let want_draw = rng.chance(7, 10);
    let want_tempting = rng.chance(1, 2);
    loop {
        let mut dead = 0;
        let board_len = rng.range(3, 4) as usize;
        let board = deal_board(rng, board_len, &mut dead);
        let hero = deal_holding(rng, dead);
        if want_draw && !is_flush_draw(hero, &board) && !is_oesd(hero, &board) {
            continue;
        }
        let villain = deal_holding(rng, dead | holding_mask(&hero));
        if rank_of(villain, &board) <= rank_of(hero, &board) {
            continue;
        }
        let analysis = outs_analysis(hero, &board, villain);
        if !(2..=17).contains(&analysis.outs.len()) {
            continue;
        }
        if want_tempting && !analysis.tempting.iter().any(|t| t.genuine) {
            continue;
        }
        return Question::Outs { hero, board, villain };
    }
}

fn gen_price(rng: &mut Rng) -> Question {
    let pot_before = rng.pick(&POTS);
    let (num, den) = rng.pick(&BET_SIZES);
    let callers = if rng.chance(3, 10) { rng.range(1, 2) as u32 } else { 0 };
    Question::Price { pot_before, bet: pot_before * num / den, callers }
}

fn gen_hit(rng: &mut Rng) -> Question {
    let outs = rng.pick(&HIT_OUTS);
    let street = if rng.chance(1, 2) { HitStreet::Flop } else { HitStreet::Turn };
    let both = street == HitStreet::Flop && rng.chance(1, 2);
    Question::Hit { outs, street, both }
}

/// Picks one class per opponent as indexes into `available`: different classes
/// whenever possible, and at most one opponent holding a set, two pair or trips. `None`
/// if the board offers too few classes to do that.
fn pick_classes(
    rng: &mut Rng,
    available: &[(OppClass, Vec<Holding>)],
    opponents: usize,
) -> Option<Vec<usize>> {
    let is_strong = |index: usize| matches!(available[index].0, OppClass::Set | OppClass::TwoPair | OppClass::Trips);
    let mut order: Vec<usize> = (0..available.len()).collect();
    rng.shuffle(&mut order);
    let mut chosen = Vec::with_capacity(opponents);
    let mut has_strong = false;
    for &index in &order {
        if chosen.len() == opponents {
            break;
        }
        if is_strong(index) {
            if has_strong {
                continue;
            }
            has_strong = true;
        }
        chosen.push(index);
    }
    let repeatable: Vec<usize> = order.iter().copied().filter(|&i| !is_strong(i)).collect();
    while chosen.len() < opponents {
        if repeatable.is_empty() {
            return None;
        }
        chosen.push(rng.pick(&repeatable));
    }
    Some(chosen)
}

fn class_lists(hero: Holding, board: &[Card]) -> Vec<(OppClass, Vec<Holding>)> {
    let holdings = unseen_holdings(holding_mask(&hero) | mask_of(board));
    OppClass::SINGLE
        .into_iter()
        .map(|class| {
            let combos = holdings
                .iter()
                .copied()
                .filter(|&h| matches_opp_class(class, h, board))
                .collect();
            (class, combos)
        })
        .collect()
}

fn share_for_screening(
    hero: Holding,
    board: &[Card],
    lists: &[Vec<Holding>],
    to_come: usize,
    seed: u64,
) -> Option<Equity> {
    if lists.len() == 1 {
        equity_with_cards_to_come(hero, board, lists, to_come, seed)
    } else {
        sampled(hero, board, lists, to_come, MW_SCREEN_SAMPLES, seed)
    }
}

fn round_to_five(value: u32) -> u32 {
    (value + 2) / 5 * 5
}

/// The next-card money a player of `kind` pays off, as a fraction of the pot after calling.
fn spot_next_card_x(kind: usize, pot: u32, bet: u32) -> u32 {
    let after_calling = pot + bet;
    match kind {
        0 => 0,
        1 => round_to_five(after_calling / 4),
        _ => round_to_five(after_calling / 3),
    }
}

fn gen_spot(rng: &mut Rng, opponents: usize, seed: u64) -> SpotQuestion {
    let next_card = rng.chance(1, 2);
    let want_call = rng.chance(1, 2);
    let kind = rng.below(PlayerType::ALL.len() as u64) as usize;
    let next_card_limit = if opponents == 1 { 0.5 } else { MW_NEXT_CARD_LIMIT };
    let share_margin = if opponents == 1 { 0.0 } else { MW_SHARE_MARGIN };
    let (mut tries, mut coarse_rejects) = (0, 0);
    loop {
        let mut dead = 0;
        let board_len = if rng.chance(2, 3) { 3 } else { 4 };
        let board = deal_board(rng, board_len, &mut dead);
        let hero = deal_holding(rng, dead);
        if !CARD.plays(hero) || !worth_a_decision(hero, &board) {
            continue;
        }
        // The first opponent follows the seed's player type; others vary.
        let players: Vec<PlayerType> = (0..opponents)
            .map(|i| if i == 0 { PlayerType::ALL[kind] } else { PlayerType::ALL[rng.below(3) as usize] })
            .collect();
        let Some(mixes) = players.iter().map(|&p| mix_for(p, hero, &board)).collect::<Option<Vec<OppMix>>>() else {
            continue;
        };
        let lists: Vec<Vec<Holding>> = mixes.iter().map(|m| mix_combos(m, hero, &board)).collect();
        // A coarse estimate clears most hands that can't be folded at any price.
        let pot_before = rng.pick(&POTS);
        if !want_call {
            let to_come = if next_card { 1 } else { 5 - board.len() };
            let Some(coarse) = sampled(hero, &board, &lists, to_come, SPOT_COARSE_SAMPLES, seed) else { continue };
            if coarse.share > spot_top_price(pot_before, opponents, next_card, kind) + SPOT_COARSE_SLACK {
                coarse_rejects += 1;
                if coarse_rejects < SPOT_COARSE_TRIES {
                    continue;
                }
            }
        }
        // A quick estimate settles most candidates; only a likely one pays for the exact figure.
        let estimate = |to_come| sampled(hero, &board, &lists, to_come, SPOT_QUICK_SAMPLES, seed);
        let Some(quick_all_in) = estimate(5 - board.len()) else { continue };
        if out_of_spot_bounds(quick_all_in.share, share_margin + SPOT_QUICK_SLACK)
            || (next_card && quick_all_in.share >= next_card_limit + SPOT_QUICK_SLACK)
        {
            continue;
        }
        let Some(quick) = (if next_card { estimate(1) } else { Some(quick_all_in) }) else { continue };
        tries += 1;
        let side_free = tries >= SPOT_SIDE_TRIES || coarse_rejects >= SPOT_COARSE_TRIES;
        if spot_prices(pot_before, opponents, next_card, kind, quick.share, want_call, side_free, share_margin).is_empty() {
            continue;
        }
        let Some(all_in) = share_for_screening(hero, &board, &lists, 5 - board.len(), seed) else {
            continue;
        };
        if out_of_spot_bounds(all_in.share, share_margin) || (next_card && all_in.share >= next_card_limit) {
            continue;
        }
        let share = if next_card {
            match share_for_screening(hero, &board, &lists, 1, seed) {
                Some(eq) => eq.share,
                None => continue,
            }
        } else {
            all_in.share
        };
        let eligible = spot_prices(pot_before, opponents, next_card, kind, share, want_call, side_free, share_margin);
        if eligible.is_empty() {
            continue;
        }
        let (pot, call, x) = rng.pick(&eligible);
        return SpotQuestion {
            hero,
            board,
            pot,
            call,
            mode: if next_card { SpotMode::NextCard { x } } else { SpotMode::AllIn },
            opponents: mixes,
        };
    }
}

fn out_of_spot_bounds(share: f64, margin: f64) -> bool {
    share < SPOT_MIN_SHARE + margin || share > SPOT_MAX_SHARE - margin
}

/// The highest price any bet size puts on this spot: above it, folding can't be right.
fn spot_top_price(pot_before: u32, opponents: usize, next_card: bool, kind: usize) -> f64 {
    BET_SIZES
        .iter()
        .map(|&(num, den)| {
            let bet = pot_before * num / den;
            let pot = pot_before + bet * opponents as u32;
            let x = if next_card { spot_next_card_x(kind, pot, bet) } else { 0 };
            money::break_even_with_x(f64::from(pot), f64::from(bet), f64::from(x))
        })
        .fold(0.0, f64::max)
}

/// The (pot, call, x) choices near break-even on the wanted side of it.
#[allow(clippy::too_many_arguments)]
fn spot_prices(
    pot_before: u32,
    opponents: usize,
    next_card: bool,
    kind: usize,
    share: f64,
    want_call: bool,
    side_free: bool,
    share_margin: f64,
) -> Vec<(u32, u32, u32)> {
    let mut prices = Vec::with_capacity(BET_SIZES.len());
    for (num, den) in BET_SIZES {
        let bet = pot_before * num / den;
        let pot = pot_before + bet * opponents as u32;
        let x = if next_card { spot_next_card_x(kind, pot, bet) } else { 0 };
        let required = money::break_even_with_x(f64::from(pot), f64::from(bet), f64::from(x));
        let margin = share - required;
        let near = margin.abs() <= SPOT_NEAR_PCT && margin.abs() >= SPOT_MIN_MARGIN + share_margin;
        let on_side = if want_call { margin > 0.0 } else { margin < 0.0 };
        if near && (on_side || side_free) {
            prices.push((pot, bet, x));
        }
    }
    prices
}

/// A random flop of the seed's category. The card space is huge, so a given
/// board comes up again only by chance.
fn gen_board_type(rng: &mut Rng, seed: u64) -> Question {
    let category = category_of_seed(seed);
    loop {
        let mut dead = 0;
        let board = deal_board(rng, 3, &mut dead);
        if classify_flop(&board) == category {
            return Question::BoardType { board, options: answer_options(category, rng) };
        }
    }
}

fn roll_equity_opponents(rng: &mut Rng) -> usize {
    let mut roll = rng.below(EQUITY_OPPONENTS.iter().map(|&(_, w)| w).sum());
    for (count, weight) in EQUITY_OPPONENTS {
        if roll < weight {
            return count;
        }
        roll -= weight;
    }
    unreachable!("the roll is below the total weight")
}

/// One card-disjoint exact hand from each candidate list, if the deal finds one.
fn deal_exact_hands(rng: &mut Rng, lists: &[Vec<Holding>], dead: u64) -> Option<Vec<Holding>> {
    'attempt: for _ in 0..EQUITY_EXACT_TRIES {
        let mut used = dead;
        let mut hands = Vec::with_capacity(lists.len());
        for list in lists {
            let pick = rng.pick(list);
            let mask = holding_mask(&pick);
            if used & mask != 0 {
                continue 'attempt;
            }
            used |= mask;
            hands.push(pick);
        }
        return Some(hands);
    }
    None
}

/// Deals an `equity` question at the seed's rung. Opponents come from the same
/// classes `spot` assumes, so rung 0 teaches the matchups rung 1 averages over.
/// Served with [`EQUITY_BAND_START`]; a session replaces it with the current band.
fn gen_equity(rng: &mut Rng, seed: u64) -> EquityQuestion {
    let opponents = roll_equity_opponents(rng);
    loop {
        let (question, _) = equity_candidate(rng, seed, opponents);
        // The screen is only an estimate, so the range is enforced on the exact
        // figure the player will be graded against.
        if equity_analysis(&question, seed)
            .is_some_and(|a| (EQUITY_MIN_PCT..=EQUITY_MAX_PCT).contains(&a.truth_pct))
        {
            return question;
        }
    }
}

/// The question [`generate`] would try first for an `equity` seed, with the
/// estimated equity in percent, without paying for the exact figure.
pub fn equity_first_candidate(seed: u64) -> (EquityQuestion, f64) {
    let mut rng = skill_rng(Skill::Equity, seed);
    let opponents = roll_equity_opponents(&mut rng);
    equity_candidate(&mut rng, seed, opponents)
}

/// The next dealt hand whose estimated equity is inside the allowed range.
fn equity_candidate(rng: &mut Rng, seed: u64, opponents: usize) -> (EquityQuestion, f64) {
    let rung = equity_rung_of_seed(seed);
    let margin = SPOT_QUICK_SLACK * 100.0;
    loop {
        let mut dead = 0;
        let board_len = if rng.chance(2, 3) { 3 } else { 4 };
        let board = deal_board(rng, board_len, &mut dead);
        let hero = deal_holding(rng, dead);
        if !CARD.plays(hero) {
            continue;
        }
        let available: Vec<(OppClass, Vec<Holding>)> = class_lists(hero, &board)
            .into_iter()
            .filter(|(_, combos)| combos.len() >= MIN_CLASS_COMBOS)
            .collect();
        let Some(picked) = pick_classes(rng, &available, opponents) else {
            continue;
        };
        let lists: Vec<Vec<Holding>> = picked.iter().map(|&i| available[i].1.clone()).collect();
        let (setup, screen_lists) = if rung == 0 {
            let Some(hands) = deal_exact_hands(rng, &lists, dead | holding_mask(&hero)) else {
                continue;
            };
            let exact = hands.iter().map(|&h| vec![h]).collect();
            (EquityOpponents::Exact(hands), exact)
        } else {
            (EquityOpponents::Classes(picked.iter().map(|&i| available[i].0).collect()), lists)
        };
        let Some(screen) = sampled(hero, &board, &screen_lists, 5 - board.len(), SPOT_QUICK_SAMPLES, seed) else {
            continue;
        };
        let share_pct = screen.share * 100.0;
        if share_pct < EQUITY_MIN_PCT - margin || share_pct > EQUITY_MAX_PCT + margin {
            continue;
        }
        return (EquityQuestion { hero, board, opponents: setup, band: EQUITY_BAND_START }, share_pct);
    }
}

/// What a seed's first random draws say about a family, without generating the item.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Prefilter {
    /// The seed can't be in the family.
    Reject,
    /// The seed is in the family for certain.
    Definite,
    /// The draws so far don't settle it.
    Maybe,
}

/// Screens a candidate sibling seed from its first draws, so slow generators
/// only run on seeds that can still be in the family. The draws here must stay
/// in step with [`generate`], and a test checks that they do.
pub fn family_prefilter(skill: Skill, seed: u64, family: &str) -> Prefilter {
    let mut rng = skill_rng(skill, seed);
    match skill {
        Skill::Spot => {
            let (next_card, _want_call) = (rng.chance(1, 2), rng.chance(1, 2));
            let player = PlayerType::ALL[rng.below(PlayerType::ALL.len() as u64) as usize];
            if family == format!("{}:{}", mode_prefix(next_card), player.code()) {
                Prefilter::Definite
            } else {
                Prefilter::Reject
            }
        }
        Skill::SpotMw => {
            let opponents = rng.range(2, 3);
            let (next_card, _want_call) = (rng.chance(1, 2), rng.chance(1, 2));
            if family == format!("{}:{opponents}", mode_prefix(next_card)) {
                Prefilter::Definite
            } else {
                Prefilter::Reject
            }
        }
        Skill::Equity => {
            let opponents = roll_equity_opponents(&mut rng);
            let rung = equity_rung_of_seed(seed);
            if family.starts_with(&format!("{rung}:{opponents}:")) {
                Prefilter::Maybe
            } else {
                Prefilter::Reject
            }
        }
        // The band is drawn first, and the deal keeps trying until it matches.
        Skill::BeatsMe if beats_me_rung_of_seed(seed) == 3 => {
            let band = MixBand::ALL[rng.below(3) as usize];
            if family == format!("3:{}", band.code()) {
                Prefilter::Definite
            } else {
                Prefilter::Reject
            }
        }
        _ => Prefilter::Maybe,
    }
}

fn mode_prefix(next_card: bool) -> &'static str {
    if next_card {
        "next"
    } else {
        "allin"
    }
}

fn gen_chain(seed: u64) -> ChainQuestion {
    let hand_seed = chain_hand_seed(seed);
    let mut rng = skill_rng(Skill::Chain, hand_seed);
    ChainQuestion { hand: gen_chain_hand(&mut rng, hand_seed), stage: chain_stage_of_seed(seed) }
}

/// Deals a hand the preflop card plays where hero is behind a stated hand with
/// real outs, then a bet that is usually near hero's break-even price.
fn gen_chain_hand(rng: &mut Rng, seed: u64) -> ChainHand {
    let opponents = rng.range(1, 2) as u32;
    let next_card = rng.chance(1, 2);
    let targeted = rng.chance(CHAIN_TARGETED_PCT, 100);
    let x_kind = rng.below(3);
    loop {
        let mut dead = 0;
        let board_len = if rng.chance(2, 3) { 3 } else { 4 };
        let board = deal_board(rng, board_len, &mut dead);
        let hero = deal_holding(rng, dead);
        if !CARD.plays(hero) {
            continue;
        }
        let hero_rank = rank_of(hero, &board);
        if !worth_a_decision(hero, &board) {
            continue;
        }
        let mut available: Vec<(OppClass, Vec<Holding>)> = class_lists(hero, &board)
            .into_iter()
            .filter(|(_, combos)| combos.len() >= MIN_CLASS_COMBOS)
            .collect();
        rng.shuffle(&mut available);
        let Some(villain) = available.iter().find_map(|(_, combos)| {
            let ahead: Vec<Holding> = combos.iter().copied().filter(|&v| rank_of(v, &board) > hero_rank).collect();
            (!ahead.is_empty()).then(|| rng.pick(&ahead))
        }) else {
            continue;
        };
        if !(CHAIN_MIN_OUTS..=CHAIN_MAX_OUTS).contains(&outs_analysis(hero, &board, villain).outs.len()) {
            continue;
        }
        let second = if opponents == 2 {
            let player = PlayerType::ALL[rng.below(3) as usize];
            let Some(mix) = mix_for(player, hero, &board) else { continue };
            Some(mix)
        } else {
            None
        };
        let mode = if next_card { SpotMode::NextCard { x: 0 } } else { SpotMode::AllIn };
        let candidate = ChainHand { hero, board: board.clone(), villain, second: second.clone(), pot_before: 0, bet: 0, mode };
        let Some((equity, _)) = chain_equity(&candidate, seed) else {
            continue;
        };
        if !(CHAIN_MIN_SHARE..=CHAIN_MAX_SHARE).contains(&equity.share) {
            continue;
        }
        let pot_before = rng.pick(&POTS);
        let mut eligible = Vec::with_capacity(BET_SIZES.len());
        for (num, den) in BET_SIZES {
            let bet = pot_before * num / den;
            let pot = pot_before + bet * opponents;
            let x = if !next_card {
                0
            } else {
                match x_kind {
                    0 => 0,
                    1 => round_to_five((pot + bet) / 2),
                    _ => round_to_five((pot + bet) * 3 / 2),
                }
            };
            let required = money::break_even_with_x(f64::from(pot), f64::from(bet), f64::from(x));
            if !targeted || (required - equity.share).abs() <= SPOT_NEAR_PCT {
                eligible.push((bet, x));
            }
        }
        if eligible.is_empty() {
            continue;
        }
        let (bet, x) = rng.pick(&eligible);
        return ChainHand {
            hero,
            board,
            villain,
            second,
            pot_before,
            bet,
            mode: if next_card { SpotMode::NextCard { x } } else { SpotMode::AllIn },
        };
    }
}
