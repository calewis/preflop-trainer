//! Cards, parsing and small deck helpers.

use std::fmt;

pub const RANK_CHARS: &[u8; 13] = b"23456789TJQKA";
pub const SUIT_CHARS: &[u8; 4] = b"shdc";

pub const ACE: u8 = 12;

/// A card, stored as `rank * 4 + suit`. Rank 0 is a deuce, rank 12 an ace.
/// Suits: 0 spades, 1 hearts, 2 diamonds, 3 clubs.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Card(u8);

impl Card {
    pub const fn new(rank: u8, suit: u8) -> Card {
        Card(rank * 4 + suit)
    }

    pub const fn from_index(index: u8) -> Card {
        Card(index)
    }

    pub const fn rank(self) -> u8 {
        self.0 >> 2
    }

    pub const fn suit(self) -> u8 {
        self.0 & 3
    }

    pub const fn index(self) -> usize {
        self.0 as usize
    }

    pub const fn mask(self) -> u64 {
        1u64 << self.0
    }

    /// Parses a two-character card such as `Ah` (case-insensitive).
    pub fn parse(text: &str) -> Option<Card> {
        let bytes = text.as_bytes();
        if bytes.len() != 2 {
            return None;
        }
        Some(Card::new(rank_from_byte(bytes[0])?, suit_from_byte(bytes[1])?))
    }

    /// The same card with its suit relabelled through `perm`.
    pub fn with_suit_perm(self, perm: &[u8; 4]) -> Card {
        Card::new(self.rank(), perm[self.suit() as usize])
    }
}

impl fmt::Display for Card {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}{}",
            RANK_CHARS[self.rank() as usize] as char,
            SUIT_CHARS[self.suit() as usize] as char
        )
    }
}

pub fn rank_from_byte(b: u8) -> Option<u8> {
    let up = b.to_ascii_uppercase();
    RANK_CHARS.iter().position(|&c| c == up).map(|i| i as u8)
}

pub fn suit_from_byte(b: u8) -> Option<u8> {
    let low = b.to_ascii_lowercase();
    SUIT_CHARS.iter().position(|&c| c == low).map(|i| i as u8)
}

pub fn rank_char(rank: u8) -> char {
    RANK_CHARS[rank as usize] as char
}

/// Parses concatenated cards such as `Kh7h2c`. `None` on any bad or repeated card.
pub fn parse_cards(text: &str) -> Option<Vec<Card>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let mut cards = Vec::with_capacity(text.len() / 2);
    let mut seen = 0u64;
    for i in (0..text.len()).step_by(2) {
        let card = Card::parse(text.get(i..i + 2)?)?;
        if seen & card.mask() != 0 {
            return None;
        }
        seen |= card.mask();
        cards.push(card);
    }
    Some(cards)
}

/// Parses exactly two cards.
pub fn parse_holding(text: &str) -> Option<Holding> {
    let cards = parse_cards(text)?;
    if cards.len() == 2 {
        Some([cards[0], cards[1]])
    } else {
        None
    }
}

pub fn cards_to_string(cards: &[Card]) -> String {
    cards.iter().map(|c| c.to_string()).collect()
}

/// Two hole cards.
pub type Holding = [Card; 2];

pub fn mask_of(cards: &[Card]) -> u64 {
    cards.iter().fold(0, |m, c| m | c.mask())
}

pub fn holding_mask(holding: &Holding) -> u64 {
    holding[0].mask() | holding[1].mask()
}

/// All cards not present in the `dead` mask, in index order.
pub fn unseen_cards(dead: u64) -> Vec<Card> {
    (0..52u8)
        .map(Card::from_index)
        .filter(|c| dead & c.mask() == 0)
        .collect()
}

/// All 2-card holdings avoiding `dead`, each written high card first.
pub fn unseen_holdings(dead: u64) -> Vec<Holding> {
    let cards = unseen_cards(dead);
    let mut out = Vec::with_capacity(cards.len() * cards.len() / 2);
    for (i, &low) in cards.iter().enumerate() {
        for &high in &cards[i + 1..] {
            out.push([high, low]);
        }
    }
    out
}
