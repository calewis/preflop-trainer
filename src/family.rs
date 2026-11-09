//! Item families: what kind of problem an item is, so a missed item can come
//! back as a fresh problem of the same kind. `family_of` is pure, and families
//! are logged, so scheduling never has to regenerate an old item.

use crate::beats_me::beats_me_family;
use crate::eval::Category;
use crate::generate::{equity_first_candidate, family_prefilter, generate, Prefilter, EQUITY_MAX_PCT, EQUITY_MIN_PCT};
use crate::grade::{beats_analysis, equity_analysis, nuts_analysis, outs_analysis, possible_analysis};
use crate::item::{beats_me_seed_with_rung, equity_seed_with_rung, EquityQuestion, Item, Question, Skill, SpotMode, SpotQuestion, HELD_OUT_BIT};
use crate::classes::made_class;
use crate::preflop::{Seat, Situation};
use crate::rng::Rng;
use crate::table::TableQuestion;

/// Seeds tried when dealing a fresh sibling, before falling back to a plain new item.
pub const SIBLING_TRIES: u32 = 200;
/// Exact equities one `equity` sibling search may compute. This bounds the worker.
pub const SIBLING_EXACT_CHECKS: u32 = 3;
/// Points of slack when an estimated equity is compared with a family's band.
const SIBLING_ESTIMATE_SLACK: f64 = 3.0;
/// Cuts between the `outs` families, in outs.
pub const OUTS_BANDS: [u32; 3] = [5, 9, 13];
/// Cuts between the `equity` families, in points of true equity. Starting guesses.
pub const EQUITY_FAMILY_BANDS: [f64; 3] = [25.0, 50.0, 75.0];

const SIBLING_SALT: u64 = 0x5369_626C_696E_6731;

/// The item's family, or `None` for skills with no family review (`anchor`,
/// `board_type` and `chain`). `table` has a family only so a reflection can be
/// compared with a later spot of the same kind; it is not reviewed by family.
pub fn family_of(item: &Item) -> Option<String> {
    match &item.question {
        Question::Preflop { behind, situation, .. } => Some(preflop_family(*behind, *situation)),
        Question::Nuts { board } => Some(category_code(nuts_analysis(board).top.category()).to_string()),
        Question::Possible { board } => Some(possible_analysis(board).set.code()),
        Question::Made { hero, board } => Some(made_class(*hero, board).code().to_string()),
        Question::Beats { hero, board } => Some(beats_analysis(*hero, board).bucket.code().to_string()),
        Question::Outs { hero, board, villain } => {
            let analysis = outs_analysis(*hero, board, *villain);
            Some(outs_family(analysis.outs.len() as u32, analysis.tempting.iter().any(|t| t.genuine)))
        }
        Question::Price { pot_before, bet, .. } => Some(bet_size_family(*pot_before, *bet)),
        Question::Hit { outs, both, .. } => Some(format!("{outs}:{}", if *both { "both" } else { "one" })),
        Question::Spot(q) if item.skill == Skill::Spot => Some(spot_family(q)),
        Question::Spot(q) => Some(spot_mw_family(q)),
        Question::Equity(q) => equity_analysis(q, item.seed).map(|a| equity_family(q, a.truth_pct)),
        Question::Table(q) => Some(table_family(q)),
        Question::BeatsMe(q) => Some(beats_me_family(q)),
        Question::Anchor { .. } | Question::BoardType { .. } | Question::Chain(_) => None,
    }
}

fn preflop_family(behind: u8, situation: Situation) -> String {
    let situation = match situation {
        Situation::FoldedTo => "folded",
        Situation::Limpers(_) => "limpers",
        Situation::Raised => "raised",
    };
    format!("{}:{situation}", Seat::from_behind(behind).code())
}

fn category_code(category: Category) -> &'static str {
    match category {
        Category::HighCard => "high",
        Category::OnePair => "pair",
        Category::TwoPair => "twopair",
        Category::Trips => "trips",
        Category::Straight => "straight",
        Category::Flush => "flush",
        Category::FullHouse => "fullhouse",
        Category::Quads => "quads",
        Category::StraightFlush => "straightflush",
    }
}

/// `under5`, `5-8`, `9-12` or `13plus`, then whether a tempting card that isn't an out exists.
pub fn outs_family(outs: u32, tempting: bool) -> String {
    let band = match outs {
        n if n < OUTS_BANDS[0] => "under5",
        n if n < OUTS_BANDS[1] => "5-8",
        n if n < OUTS_BANDS[2] => "9-12",
        _ => "13plus",
    };
    format!("{band}:{}", if tempting { "tempting" } else { "plain" })
}

/// The bet as a fraction of the pot before it, in lowest terms.
fn bet_size_family(pot_before: u32, bet: u32) -> String {
    let divisor = gcd(bet, pot_before);
    format!("{}/{}", bet / divisor, pot_before / divisor)
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 {
        a.max(1)
    } else {
        gcd(b, a % b)
    }
}

fn mode_code(mode: SpotMode) -> &'static str {
    match mode {
        SpotMode::AllIn => "allin",
        SpotMode::NextCard { .. } => "next",
    }
}

fn spot_family(q: &SpotQuestion) -> String {
    let opponent = q.opponents[0].player.map_or_else(|| q.opponents[0].classes[0].code(), |p| p.code());
    format!("{}:{opponent}", mode_code(q.mode))
}

fn spot_mw_family(q: &SpotQuestion) -> String {
    format!("{}:{}", mode_code(q.mode), q.opponents.len())
}

/// Rung, opponent count and the band the true equity falls in.
pub fn equity_family(q: &EquityQuestion, truth_pct: f64) -> String {
    let band = match truth_pct {
        t if t < EQUITY_FAMILY_BANDS[0] => "0-25",
        t if t < EQUITY_FAMILY_BANDS[1] => "25-50",
        t if t < EQUITY_FAMILY_BANDS[2] => "50-75",
        _ => "75-100",
    };
    format!("{}:{}:{band}", q.rung(), q.opponents.count())
}

/// Street and whether the pot is heads-up or multiway.
pub fn table_family(q: &TableQuestion) -> String {
    let street = match q.board.len() {
        0..=3 => "flop",
        4 => "turn",
        _ => "river",
    };
    format!("{street}:{}", if q.players <= 2 { "hu" } else { "mw" })
}

/// A fresh non-held-out seed in `family`, by bounded rejection sampling from
/// `base`. `avoid` is never returned, so a miss doesn't come back as itself.
/// `None` once [`SIBLING_TRIES`] candidates are used up, or an `equity` search
/// has spent [`SIBLING_EXACT_CHECKS`] exact equities. The caller then deals a
/// plain new item and the family waits for another session.
pub fn find_sibling(skill: Skill, family: &str, base: u64, avoid: u64) -> Option<u64> {
    let mut rng = Rng::new(base ^ SIBLING_SALT);
    let rung = (skill == Skill::Equity || skill == Skill::BeatsMe).then(|| equity_rung_in_family(family)).flatten();
    let mut exact_left = SIBLING_EXACT_CHECKS;
    for _ in 0..SIBLING_TRIES {
        let mut seed = rng.next_u64() & !HELD_OUT_BIT;
        if let Some(rung) = rung {
            seed = if skill == Skill::BeatsMe { beats_me_seed_with_rung(seed, rung) } else { equity_seed_with_rung(seed, rung) };
        }
        if seed == avoid {
            continue;
        }
        match family_prefilter(skill, seed, family) {
            Prefilter::Reject => continue,
            Prefilter::Definite => return Some(seed),
            Prefilter::Maybe => {}
        }
        if skill == Skill::Equity {
            let (question, estimate) = equity_first_candidate(seed);
            if !estimate_fits_band(family, estimate) {
                continue;
            }
            if exact_left == 0 {
                return None;
            }
            exact_left -= 1;
            let truth = equity_analysis(&question, seed).map(|a| a.truth_pct);
            if truth.is_some_and(|t| (EQUITY_MIN_PCT..=EQUITY_MAX_PCT).contains(&t) && equity_family(&question, t) == family) {
                return Some(seed);
            }
            continue;
        }
        if family_of(&generate(skill, seed)).as_deref() == Some(family) {
            return Some(seed);
        }
    }
    None
}

/// Whether an estimated equity could be in the family's band, given sampling error.
fn estimate_fits_band(family: &str, estimate: f64) -> bool {
    let Some(band) = family.rsplit(':').next() else { return false };
    let bounds = match band {
        "0-25" => (0.0, EQUITY_FAMILY_BANDS[0]),
        "25-50" => (EQUITY_FAMILY_BANDS[0], EQUITY_FAMILY_BANDS[1]),
        "50-75" => (EQUITY_FAMILY_BANDS[1], EQUITY_FAMILY_BANDS[2]),
        "75-100" => (EQUITY_FAMILY_BANDS[2], 100.0),
        _ => return false,
    };
    estimate >= bounds.0 - SIBLING_ESTIMATE_SLACK && estimate <= bounds.1 + SIBLING_ESTIMATE_SLACK
}

fn equity_rung_in_family(family: &str) -> Option<u8> {
    family.split(':').next()?.parse().ok()
}
