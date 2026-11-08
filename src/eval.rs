//! Hand evaluator for 5 to 7 cards.

use crate::cards::{rank_char, Card};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Category {
    HighCard,
    OnePair,
    TwoPair,
    Trips,
    Straight,
    Flush,
    FullHouse,
    Quads,
    StraightFlush,
}

impl Category {
    pub const ALL: [Category; 9] = [
        Category::HighCard,
        Category::OnePair,
        Category::TwoPair,
        Category::Trips,
        Category::Straight,
        Category::Flush,
        Category::FullHouse,
        Category::Quads,
        Category::StraightFlush,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Category::HighCard => "high card",
            Category::OnePair => "one pair",
            Category::TwoPair => "two pair",
            Category::Trips => "three of a kind",
            Category::Straight => "straight",
            Category::Flush => "flush",
            Category::FullHouse => "full house",
            Category::Quads => "four of a kind",
            Category::StraightFlush => "straight flush",
        }
    }
}

/// Comparable hand strength: a higher value is a better hand, equal values tie.
///
/// The category sits above five 4-bit rank fields, most significant first.
/// For a straight or straight flush the first field is the high card (5 for a wheel).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct HandRank(u32);

impl HandRank {
    fn build(category: Category, fields: [u8; 5]) -> HandRank {
        let mut value = category as u32;
        for field in fields {
            value = (value << 4) | u32::from(field);
        }
        HandRank(value)
    }

    pub fn category(self) -> Category {
        Category::ALL[(self.0 >> 20) as usize]
    }

    /// Rank field `i`, 0 being the most significant.
    pub fn field(self, i: usize) -> u8 {
        ((self.0 >> (16 - 4 * i as u32)) & 0xF) as u8
    }

    /// The rank that defines the hand: the pair, trips, quads or straight top.
    pub fn primary(self) -> u8 {
        self.field(0)
    }

    pub fn describe(self) -> String {
        let name = |i: usize| rank_name(self.field(i));
        let plural = |i: usize| rank_plural(self.field(i));
        let kickers = |from: usize, to: usize| -> String {
            (from..to)
                .map(|i| rank_char(self.field(i)).to_string())
                .collect::<Vec<_>>()
                .join("-")
        };
        let article = |i: usize| rank_article(self.field(i));
        match self.category() {
            Category::StraightFlush if self.field(0) == 12 => "a royal flush".to_string(),
            Category::StraightFlush => format!("{} {}-high straight flush", article(0), name(0)),
            Category::Quads => format!("four {} with {} {} kicker", plural(0), article(1), name(1)),
            Category::FullHouse => format!("a full house, {} full of {}", plural(0), plural(1)),
            Category::Flush => format!("{} {}-high flush ({})", article(0), name(0), kickers(0, 5)),
            Category::Straight => format!("{} {}-high straight", article(0), name(0)),
            Category::Trips => format!("three {} with {} kickers", plural(0), kickers(1, 3)),
            Category::TwoPair => format!(
                "two pair, {} and {}, with {} {} kicker",
                plural(0),
                plural(1),
                article(2),
                name(2)
            ),
            Category::OnePair => format!("a pair of {} with {} kickers", plural(0), kickers(1, 4)),
            Category::HighCard => format!("{} high ({})", name(0), kickers(0, 5)),
        }
    }
}

/// `an` before the two rank names that start with a vowel sound, eight and ace.
fn rank_article(rank: u8) -> &'static str {
    if rank == 6 || rank == 12 {
        "an"
    } else {
        "a"
    }
}

pub fn rank_name(rank: u8) -> &'static str {
    [
        "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "jack", "queen",
        "king", "ace",
    ][rank as usize]
}

pub fn rank_plural(rank: u8) -> &'static str {
    [
        "twos", "threes", "fours", "fives", "sixes", "sevens", "eights", "nines", "tens", "jacks",
        "queens", "kings", "aces",
    ][rank as usize]
}

const WHEEL_MASK: u16 = 0b1_0000_0000_1111;

/// Top rank of the best straight in a 13-bit rank mask, or `None`.
pub fn straight_top(ranks: u16) -> Option<u8> {
    let runs = ranks & (ranks >> 1) & (ranks >> 2) & (ranks >> 3) & (ranks >> 4);
    if runs != 0 {
        Some((15 - runs.leading_zeros()) as u8 + 4)
    } else if ranks & WHEEL_MASK == WHEEL_MASK {
        Some(3)
    } else {
        None
    }
}

fn highest(mask: u16) -> u8 {
    (15 - mask.leading_zeros()) as u8
}

/// The top `N` ranks of `mask`, highest first. Unused slots stay 0.
fn top_ranks<const N: usize>(mut mask: u16) -> [u8; N] {
    let mut out = [0u8; N];
    for slot in out.iter_mut() {
        if mask == 0 {
            break;
        }
        let r = highest(mask);
        *slot = r;
        mask &= !(1 << r);
    }
    out
}

/// Evaluates 5, 6 or 7 distinct cards.
pub fn evaluate(cards: &[Card]) -> HandRank {
    assert!(
        (5..=7).contains(&cards.len()),
        "evaluate expects 5 to 7 cards"
    );
    let mut suits = [0u16; 4];
    let mut counts = [0u8; 13];
    for card in cards {
        suits[card.suit() as usize] |= 1 << card.rank();
        counts[card.rank() as usize] += 1;
    }
    // Five or more of a suit in at most seven cards leaves no room for quads or
    // a full house, so a flush can be scored before anything else.
    for suit in suits {
        if suit.count_ones() >= 5 {
            if let Some(top) = straight_top(suit) {
                return HandRank::build(Category::StraightFlush, [top, 0, 0, 0, 0]);
            }
            return HandRank::build(Category::Flush, top_ranks::<5>(suit));
        }
    }

    let all = suits[0] | suits[1] | suits[2] | suits[3];
    let (mut quad, mut trip, mut trip2, mut pair, mut pair2) = (-1i8, -1i8, -1i8, -1i8, -1i8);
    for rank in (0..13i8).rev() {
        match counts[rank as usize] {
            4 => quad = rank,
            3 if trip < 0 => trip = rank,
            3 => trip2 = rank,
            2 if pair < 0 => pair = rank,
            2 if pair2 < 0 => pair2 = rank,
            _ => {}
        }
    }

    if quad >= 0 {
        let kicker = highest(all & !(1 << quad));
        return HandRank::build(Category::Quads, [quad as u8, kicker, 0, 0, 0]);
    }
    if trip >= 0 && (trip2 >= 0 || pair >= 0) {
        let filler = trip2.max(pair) as u8;
        return HandRank::build(Category::FullHouse, [trip as u8, filler, 0, 0, 0]);
    }
    if let Some(top) = straight_top(all) {
        return HandRank::build(Category::Straight, [top, 0, 0, 0, 0]);
    }
    if trip >= 0 {
        let k = top_ranks::<2>(all & !(1 << trip));
        return HandRank::build(Category::Trips, [trip as u8, k[0], k[1], 0, 0]);
    }
    if pair >= 0 && pair2 >= 0 {
        let rest = all & !(1 << pair) & !(1 << pair2);
        let k = highest(rest);
        return HandRank::build(Category::TwoPair, [pair as u8, pair2 as u8, k, 0, 0]);
    }
    if pair >= 0 {
        let k = top_ranks::<3>(all & !(1 << pair));
        return HandRank::build(Category::OnePair, [pair as u8, k[0], k[1], k[2], 0]);
    }
    HandRank::build(Category::HighCard, top_ranks::<5>(all))
}
