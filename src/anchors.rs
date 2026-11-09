//! The anchor deck: reference numbers worth memorizing. The values are computed
//! once by the hidden `gen-anchors` command and frozen in `data/anchors_v1.txt`.

use std::sync::OnceLock;

use crate::cards::{cards_to_string, parse_cards, parse_holding, unseen_holdings, Card, Holding};
use crate::equity::{equity_all_in, sampled};
use crate::money;

pub const ANCHORS_TEXT: &str = include_str!("../data/anchors_v1.txt");

/// Tolerance, in points, for price cards. Starting guess.
pub const PRICE_TOLERANCE: u32 = 2;
/// Tolerance, in points, for matchup and preflop cards. Starting guess.
pub const EQUITY_TOLERANCE: u32 = 4;

/// Simulated deals per preflop card. Large enough that the standard error is
/// about 0.02 points, well inside the stored precision of 0.1.
pub const PREFLOP_SAMPLES: u64 = 10_000_000;
const PREFLOP_SEED: u64 = 0x416E_6368_6F72_5631;

/// Low bits of an `anchor` seed that hold the card index.
const CARD_MASK: u64 = 0xFFFF;

/// Bet sizes as fractions of the pot, for the price cards.
pub const PRICE_LATTICE: [(u32, u32); 8] = [
    (1, 4),
    (1, 3),
    (1, 2),
    (2, 3),
    (3, 4),
    (1, 1),
    (3, 2),
    (2, 1),
];

/// (label, hero, villain, flop).
const MATCHUPS: [(&str, &str, &str, &str); 20] = [
    ("Set vs flush draw", "7h7d", "AsQs", "7s4s2c"),
    ("Set vs overpair", "9h9d", "KcKs", "9c5h2d"),
    ("Set vs open-ended draw", "8h8d", "JcTd", "9s8c2h"),
    ("Top set vs middle set", "KhKd", "7c7s", "Kc7d2h"),
    ("Overpair vs flush draw", "KhKd", "Js9s", "Qs7s2c"),
    ("Overpair vs top pair", "AhAd", "KsQc", "Kc7d2h"),
    ("Top pair vs two pair", "AhKd", "Kc7c", "Ks7d2h"),
    ("Top pair, better kicker vs worse kicker", "AhKd", "KsQc", "Kc7d2h"),
    ("Top pair plus flush draw vs overpair", "KhJh", "AdAs", "Jd7h2h"),
    ("Flush draw vs top pair", "9d8d", "KcJs", "Kd7d2s"),
    ("Flush draw plus overcard vs top pair", "Ad9d", "KcJs", "Kd7d2s"),
    ("Flush draw vs overpair", "Qc8c", "AsAd", "Kc7c2h"),
    ("Combo draw vs top pair", "9d8d", "QhTc", "Td7d2s"),
    ("Open-ended draw vs top pair", "JdTh", "Ac9h", "9s8c2h"),
    ("Gutshot vs top pair", "Jh9d", "KsQh", "Kc8s7d"),
    ("Two overcards vs a pair", "AcKd", "9h9s", "8d5c2h"),
    ("Second pair vs top pair", "Qd9h", "AhJc", "Ac9s4d"),
    ("Low pocket pair vs top pair", "6h6d", "KcQs", "Kd7c2h"),
    ("Two pair vs flush draw", "Kh7h", "AsQs", "Ks7s2c"),
    ("Straight vs flush draw", "JcTd", "AdKd", "9d8d7s"),
];

/// (name, example holding, opponents). Each hand appears twice so that one,
/// two and three random opponents are all covered in 12 cards.
const PREFLOP: [(&str, &str, u8); 12] = [
    ("AA", "AhAd", 1),
    ("AA", "AhAd", 3),
    ("AKo", "AhKd", 1),
    ("AKo", "AhKd", 3),
    ("JTs", "JhTh", 2),
    ("JTs", "JhTh", 3),
    ("77", "7h7d", 1),
    ("77", "7h7d", 2),
    ("K9o", "Kh9d", 1),
    ("K9o", "Kh9d", 2),
    ("A5o", "Ah5d", 2),
    ("A5o", "Ah5d", 3),
];

#[derive(Clone, PartialEq, Debug)]
pub enum AnchorKind {
    /// Equity needed to call a bet of `num / den` of the pot.
    Price { num: u32, den: u32 },
    /// Heads-up equity on a flop, all in.
    Matchup { hero: Holding, villain: Holding, board: Vec<Card> },
    /// Preflop equity against `opponents` random hands.
    Preflop { hand: Holding, opponents: u8 },
}

#[derive(Clone, PartialEq, Debug)]
pub struct AnchorCard {
    pub id: String,
    pub kind: AnchorKind,
    pub prompt: String,
    /// The answer in percent, to one decimal place.
    pub answer_pct: f64,
    /// Points either side of the answer that count as correct.
    pub tolerance: u32,
}

/// The exact price: `bet / (pot + 2 bet)`, as a percentage.
pub fn price_answer(num: u32, den: u32) -> f64 {
    let frac = f64::from(num) / f64::from(den);
    100.0 * money::break_even(1.0 + frac, frac)
}

/// Exact heads-up flop equity for a matchup, as a percentage.
pub fn matchup_answer(hero: Holding, villain: Holding, board: &[Card]) -> f64 {
    let equity = equity_all_in(hero, board, &[vec![villain]], 0).expect("a matchup has a runout");
    100.0 * equity.share
}

/// Preflop equity against `opponents` random hands, from a deterministic sample
/// of `samples` deals.
pub fn preflop_answer(hand: Holding, opponents: u8, samples: u64) -> f64 {
    let list = unseen_holdings(hand[0].mask() | hand[1].mask());
    let lists = vec![list; usize::from(opponents)];
    let equity = sampled(hand, &[], &lists, 5, samples, PREFLOP_SEED ^ u64::from(opponents))
        .expect("a random hand always exists");
    100.0 * equity.share
}

fn rounded(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

fn holding_text(h: &Holding) -> String {
    format!("{}{}", h[0], h[1])
}

/// Builds every card. Only `gen-anchors` uses this, as the preflop cards are slow.
pub fn generate_deck() -> Vec<AnchorCard> {
    let mut deck = price_and_matchup_cards();
    deck.extend(preflop_cards(PREFLOP_SAMPLES));
    deck
}

/// The price and matchup cards, which are exact and fast to compute.
pub fn price_and_matchup_cards() -> Vec<AnchorCard> {
    let mut deck = Vec::with_capacity(PRICE_LATTICE.len() + MATCHUPS.len());
    for (i, &(num, den)) in PRICE_LATTICE.iter().enumerate() {
        let size = if num == den {
            "the size of the pot".to_string()
        } else if den == 2 && num == 3 {
            "1.5 times the pot".to_string()
        } else if den == 1 {
            format!("{num} times the pot")
        } else {
            format!("{num}/{den} of the pot")
        };
        deck.push(AnchorCard {
            id: format!("p{:02}", i + 1),
            kind: AnchorKind::Price { num, den },
            prompt: format!("Someone bets {size}. What equity do you need to call?"),
            answer_pct: rounded(price_answer(num, den)),
            tolerance: PRICE_TOLERANCE,
        });
    }
    for (i, &(label, hero, villain, board)) in MATCHUPS.iter().enumerate() {
        let hero = parse_holding(hero).expect("valid hero");
        let villain = parse_holding(villain).expect("valid villain");
        let board = parse_cards(board).expect("valid flop");
        deck.push(AnchorCard {
            id: format!("m{:02}", i + 1),
            prompt: format!(
                "{label}: your {} against {} on a flop of {}. Your all-in equity?",
                holding_text(&hero),
                holding_text(&villain),
                cards_to_string(&board)
            ),
            answer_pct: rounded(matchup_answer(hero, villain, &board)),
            tolerance: EQUITY_TOLERANCE,
            kind: AnchorKind::Matchup { hero, villain, board },
        });
    }
    deck
}

/// The preflop cards, with answers simulated from `samples` deals each.
pub fn preflop_cards(samples: u64) -> Vec<AnchorCard> {
    PREFLOP
        .iter()
        .enumerate()
        .map(|(i, &(name, hand, opponents))| {
            let hand = parse_holding(hand).expect("valid hand");
            let others = if opponents == 1 {
                "one random hand".to_string()
            } else {
                format!("{opponents} random hands")
            };
            AnchorCard {
                id: format!("f{:02}", i + 1),
                prompt: format!("Preflop, {name} against {others}: your all-in equity?"),
                answer_pct: rounded(preflop_answer(hand, opponents, samples)),
                tolerance: EQUITY_TOLERANCE,
                kind: AnchorKind::Preflop { hand, opponents },
            }
        })
        .collect()
}

fn kind_text(kind: &AnchorKind) -> String {
    match kind {
        AnchorKind::Price { num, den } => format!("price {num}/{den}"),
        AnchorKind::Matchup { hero, villain, board } => format!(
            "matchup {} {} {}",
            holding_text(hero),
            holding_text(villain),
            cards_to_string(board)
        ),
        AnchorKind::Preflop { hand, opponents } => format!("preflop {} {opponents}", holding_text(hand)),
    }
}

/// The file contents for a deck.
pub fn to_text(deck: &[AnchorCard]) -> String {
    let mut out = String::from(
        "# Anchor deck, written once by the hidden gen-anchors command. Do not edit by hand.\n# id, kind and cards, answer in percent, tolerance, prompt, separated by tabs.\n",
    );
    for card in deck {
        out.push_str(&format!(
            "{}\t{}\t{:.1}\t{}\t{}\n",
            card.id,
            kind_text(&card.kind),
            card.answer_pct,
            card.tolerance,
            card.prompt
        ));
    }
    out
}

fn parse_kind(text: &str) -> Result<AnchorKind, String> {
    let parts: Vec<&str> = text.split(' ').collect();
    match parts.as_slice() {
        ["price", frac] => {
            let (num, den) = frac.split_once('/').ok_or("price needs num/den")?;
            Ok(AnchorKind::Price {
                num: num.parse().map_err(|_| "bad numerator")?,
                den: den.parse().map_err(|_| "bad denominator")?,
            })
        }
        ["matchup", hero, villain, board] => Ok(AnchorKind::Matchup {
            hero: parse_holding(hero).ok_or("bad hero")?,
            villain: parse_holding(villain).ok_or("bad villain")?,
            board: parse_cards(board).ok_or("bad board")?,
        }),
        ["preflop", hand, opponents] => Ok(AnchorKind::Preflop {
            hand: parse_holding(hand).ok_or("bad hand")?,
            opponents: opponents.parse().map_err(|_| "bad opponent count")?,
        }),
        _ => Err("unknown card kind".to_string()),
    }
}

fn parse_line(line: &str) -> Result<AnchorCard, String> {
    let fields: Vec<&str> = line.split('\t').collect();
    let [id, kind, answer, tolerance, prompt] = fields.as_slice() else {
        return Err("expected 5 tab-separated fields".to_string());
    };
    Ok(AnchorCard {
        id: (*id).to_string(),
        kind: parse_kind(kind)?,
        prompt: (*prompt).to_string(),
        answer_pct: answer.parse().map_err(|_| "bad answer")?,
        tolerance: tolerance.parse().map_err(|_| "bad tolerance")?,
    })
}

/// Parses deck text. Blank lines and `#` comments are ignored.
pub fn parse_deck(text: &str) -> Result<Vec<AnchorCard>, String> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty() && !line.starts_with('#'))
        .map(|(index, line)| parse_line(line).map_err(|e| format!("anchor line {}: {e}", index + 1)))
        .collect()
}

/// The frozen deck compiled into the program.
pub fn deck() -> &'static [AnchorCard] {
    static DECK: OnceLock<Vec<AnchorCard>> = OnceLock::new();
    DECK.get_or_init(|| parse_deck(ANCHORS_TEXT).expect("the compiled-in anchor deck is valid"))
}

/// The card index an `anchor` seed refers to.
pub fn card_of_seed(seed: u64) -> usize {
    (seed & CARD_MASK) as usize % deck().len()
}

/// `seed` with its card bits set to `card`.
pub fn seed_with_card(seed: u64, card: usize) -> u64 {
    (seed & !CARD_MASK) | card as u64
}
