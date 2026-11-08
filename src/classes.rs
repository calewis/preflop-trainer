//! Hand classes: made-hand codes and the classes opponents are assumed to hold.

use crate::cards::{holding_mask, mask_of, unseen_holdings, Card, Holding, ACE};
use crate::eval::{evaluate, straight_top, Category, HandRank};

/// What hero has right now, as answered in the `made` skill.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum MadeClass {
    HighCard,
    BoardPair,
    LowPair,
    MidPair,
    TopPair,
    OverPair,
    TwoPair,
    Set,
    Trips,
    Straight,
    Flush,
    FullHouse,
    Quads,
    StraightFlush,
}

impl MadeClass {
    pub const ALL: [MadeClass; 14] = [
        MadeClass::HighCard,
        MadeClass::BoardPair,
        MadeClass::LowPair,
        MadeClass::MidPair,
        MadeClass::TopPair,
        MadeClass::OverPair,
        MadeClass::TwoPair,
        MadeClass::Set,
        MadeClass::Trips,
        MadeClass::Straight,
        MadeClass::Flush,
        MadeClass::FullHouse,
        MadeClass::Quads,
        MadeClass::StraightFlush,
    ];

    pub fn code(self) -> &'static str {
        match self {
            MadeClass::HighCard => "hc",
            MadeClass::BoardPair => "board",
            MadeClass::LowPair => "low",
            MadeClass::MidPair => "mid",
            MadeClass::TopPair => "top",
            MadeClass::OverPair => "over",
            MadeClass::TwoPair => "2p",
            MadeClass::Set => "set",
            MadeClass::Trips => "trips",
            MadeClass::Straight => "st",
            MadeClass::Flush => "fl",
            MadeClass::FullHouse => "fh",
            MadeClass::Quads => "4k",
            MadeClass::StraightFlush => "sf",
        }
    }

    /// A short name for the class, for choosing it from a list.
    pub fn label(self) -> &'static str {
        match self {
            MadeClass::HighCard => "high card",
            MadeClass::BoardPair => "pair on the board",
            MadeClass::LowPair => "low pair",
            MadeClass::MidPair => "middle pair",
            MadeClass::TopPair => "top pair",
            MadeClass::OverPair => "overpair",
            MadeClass::TwoPair => "two pair",
            MadeClass::Set => "set",
            MadeClass::Trips => "trips",
            MadeClass::Straight => "straight",
            MadeClass::Flush => "flush",
            MadeClass::FullHouse => "full house",
            MadeClass::Quads => "four of a kind",
            MadeClass::StraightFlush => "straight flush",
        }
    }

    pub fn from_code(code: &str) -> Option<MadeClass> {
        MadeClass::ALL.into_iter().find(|c| c.code() == code)
    }

    /// Why a hand is in this class, as a sentence that doesn't repeat the label.
    pub fn description(self) -> &'static str {
        match self {
            MadeClass::HighCard => "No pair or better, so your highest cards play.",
            MadeClass::BoardPair => "The only pair is on the board, so neither hole card is part of it.",
            MadeClass::LowPair => "Your pair is at or below the lowest board card.",
            MadeClass::MidPair => "Your pair sits between the highest and lowest board cards.",
            MadeClass::TopPair => "Your pair matches the highest board card.",
            MadeClass::OverPair => "Your pocket pair is above every board card.",
            MadeClass::TwoPair => "You have two different pairs.",
            MadeClass::Set => "Your pocket pair matches a board card.",
            MadeClass::Trips => "One hole card matches a pair on the board, or the three of a kind is all on the board.",
            MadeClass::Straight => "Five consecutive ranks.",
            MadeClass::Flush => "Five cards of one suit.",
            MadeClass::FullHouse => "Three of a kind plus a pair.",
            MadeClass::Quads => "All four cards of one rank.",
            MadeClass::StraightFlush => "Five consecutive ranks in one suit.",
        }
    }
}

/// Classes an opponent can be assumed to hold in `spot` and `spot_mw`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum OppClass {
    Over,
    Top,
    Mid,
    TwoPair,
    Set,
    Trips,
    FlushDraw,
    Oesd,
}

impl OppClass {
    pub const ALL: [OppClass; 8] = [
        OppClass::Over,
        OppClass::Top,
        OppClass::Mid,
        OppClass::TwoPair,
        OppClass::Set,
        OppClass::Trips,
        OppClass::FlushDraw,
        OppClass::Oesd,
    ];

    pub fn code(self) -> &'static str {
        match self {
            OppClass::Over => "over",
            OppClass::Top => "top",
            OppClass::Mid => "mid",
            OppClass::TwoPair => "2p",
            OppClass::Set => "set",
            OppClass::Trips => "trips",
            OppClass::FlushDraw => "fd",
            OppClass::Oesd => "oesd",
        }
    }

    pub fn from_code(code: &str) -> Option<OppClass> {
        OppClass::ALL.into_iter().find(|c| c.code() == code)
    }

    /// The class in plain words, for "Assume they have ...".
    pub fn description(self) -> &'static str {
        match self {
            OppClass::Over => "an overpair",
            OppClass::Top => "top pair",
            OppClass::Mid => "a middle pair",
            OppClass::TwoPair => "two pair",
            OppClass::Set => "a set",
            OppClass::Trips => "trips",
            OppClass::FlushDraw => "a flush draw",
            OppClass::Oesd => "an open-ended straight draw",
        }
    }
}

/// Evaluates a holding on a 3 to 5 card board.
pub fn rank_of(holding: Holding, board: &[Card]) -> HandRank {
    let mut seven = [holding[0]; 7];
    seven[1] = holding[1];
    seven[2..2 + board.len()].copy_from_slice(board);
    evaluate(&seven[..2 + board.len()])
}

pub fn made_class(holding: Holding, board: &[Card]) -> MadeClass {
    let rank = rank_of(holding, board);
    match rank.category() {
        Category::HighCard => MadeClass::HighCard,
        Category::OnePair => pair_class(rank.primary(), holding, board),
        Category::TwoPair => MadeClass::TwoPair,
        Category::Trips => {
            if holding[0].rank() == holding[1].rank()
                && board.iter().any(|c| c.rank() == holding[0].rank())
            {
                MadeClass::Set
            } else {
                MadeClass::Trips
            }
        }
        Category::Straight => MadeClass::Straight,
        Category::Flush => MadeClass::Flush,
        Category::FullHouse => MadeClass::FullHouse,
        Category::Quads => MadeClass::Quads,
        Category::StraightFlush => MadeClass::StraightFlush,
    }
}

fn pair_class(pair_rank: u8, holding: Holding, board: &[Card]) -> MadeClass {
    if holding[0].rank() != pair_rank && holding[1].rank() != pair_rank {
        return MadeClass::BoardPair;
    }
    let top = board.iter().map(|c| c.rank()).max().expect("board is not empty");
    let lowest = board.iter().map(|c| c.rank()).min().expect("board is not empty");
    if pair_rank > top {
        MadeClass::OverPair
    } else if pair_rank == top {
        MadeClass::TopPair
    } else if pair_rank <= lowest {
        MadeClass::LowPair
    } else {
        MadeClass::MidPair
    }
}

/// Exactly four cards of one suit among hole and board, at least one a hole
/// card, on the flop or turn, with no made flush.
pub fn is_flush_draw(holding: Holding, board: &[Card]) -> bool {
    if !(3..=4).contains(&board.len()) {
        return false;
    }
    let mut counts = [0u8; 4];
    for card in holding.iter().chain(board) {
        counts[card.suit() as usize] += 1;
    }
    if counts.iter().any(|&n| n >= 5) {
        return false;
    }
    (0..4u8).any(|suit| {
        counts[suit as usize] == 4 && holding.iter().any(|c| c.suit() == suit)
    })
}

fn rank_mask(cards: &[Card]) -> u16 {
    cards.iter().fold(0u16, |m, c| m | (1 << c.rank()))
}

/// Ranks not present in `ranks` that would complete a straight.
fn completing_ranks(ranks: u16) -> u16 {
    let mut out = 0u16;
    for r in 0..13u8 {
        let bit = 1u16 << r;
        if ranks & bit == 0 && straight_top(ranks | bit).is_some() {
            out |= bit;
        }
    }
    out
}

/// Open-ended straight draw: on the flop or turn, no made straight, at least one
/// hole card needed for the draw, and exactly two ranks complete a straight as
/// the two ends of four consecutive ranks. A-2-3-4 and J-Q-K-A are one-ended.
pub fn is_oesd(holding: Holding, board: &[Card]) -> bool {
    if !(3..=4).contains(&board.len()) {
        return false;
    }
    let board_ranks = rank_mask(board);
    let ranks = board_ranks | rank_mask(&holding);
    if straight_top(ranks).is_some() {
        return false;
    }
    let completes = completing_ranks(ranks);
    if completes.count_ones() != 2 {
        return false;
    }
    // Windows of four consecutive ranks whose two ends both exist: 2-3-4-5
    // (ends A and 6) up to T-J-Q-K (ends 9 and A).
    for low_rank in 0..=8u8 {
        let window = 0b1111u16 << low_rank;
        if ranks & window != window || board_ranks & window == window {
            continue;
        }
        let below = if low_rank == 0 { 1u16 << ACE } else { 1u16 << (low_rank - 1) };
        let above = 1u16 << (low_rank + 4);
        if completes == below | above {
            return true;
        }
    }
    false
}

/// The pair-type spot class of a holding, from what its hole cards do. The
/// evaluator's category is not used for this, because on a paired board it calls
/// nearly any pair two pair. A straight, flush, full house or better is in none
/// of these classes.
fn spot_pair_class(holding: Holding, board: &[Card]) -> Option<OppClass> {
    if rank_of(holding, board).category() >= Category::Straight {
        return None;
    }
    let count = |rank: u8| board.iter().filter(|c| c.rank() == rank).count();
    let max = board.iter().map(|c| c.rank()).max()?;
    let min = board.iter().map(|c| c.rank()).min()?;
    let between = |rank: u8| min < rank && rank < max;
    let (a, b) = (holding[0].rank(), holding[1].rank());
    if a == b {
        return match count(a) {
            0 if a > max => Some(OppClass::Over),
            0 if between(a) => Some(OppClass::Mid),
            1 => Some(OppClass::Set),
            _ => None,
        };
    }
    let paired = match (count(a), count(b)) {
        (1, 1) => return Some(OppClass::TwoPair),
        (0, 0) => return None,
        (_, 0) => a,
        (0, _) => b,
        _ => return None,
    };
    match count(paired) {
        2 => Some(OppClass::Trips),
        1 if paired == max => Some(OppClass::Top),
        1 if between(paired) => Some(OppClass::Mid),
        _ => None,
    }
}

/// Whether a holding is in a spot class. This is separate from [`made_class`],
/// which answers the `made` skill and follows the evaluator's category.
pub fn matches_opp_class(class: OppClass, holding: Holding, board: &[Card]) -> bool {
    match class {
        OppClass::FlushDraw => is_flush_draw(holding, board),
        OppClass::Oesd => is_oesd(holding, board),
        pair_type => spot_pair_class(holding, board) == Some(pair_type),
    }
}

/// Every unseen holding in `class`, given hero's cards and the board.
pub fn combos_in_class(class: OppClass, hero: Holding, board: &[Card]) -> Vec<Holding> {
    unseen_holdings(holding_mask(&hero) | mask_of(board))
        .into_iter()
        .filter(|&h| matches_opp_class(class, h, board))
        .collect()
}
