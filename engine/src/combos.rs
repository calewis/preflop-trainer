//! The 1326 two-card holdings: indexing, card masks and 169-class mapping.

use poker_trainer::cards::Card;
use std::sync::OnceLock;

pub const NCOMBOS: usize = 1326;

pub struct ComboTable {
    /// `cards[i]` = the two cards of combo `i`, higher card index first.
    pub cards: Vec<(Card, Card)>,
    pub masks: Vec<u64>,
    pub class169: Vec<u8>,
    /// Lookup by card indices (`index[a][b]`, symmetric).
    pub index: Vec<[u16; 52]>,
}

pub fn combos() -> &'static ComboTable {
    static TABLE: OnceLock<ComboTable> = OnceLock::new();
    TABLE.get_or_init(build)
}

fn build() -> ComboTable {
    let mut cards = Vec::with_capacity(NCOMBOS);
    let mut masks = Vec::with_capacity(NCOMBOS);
    let mut class169 = Vec::with_capacity(NCOMBOS);
    let mut index = vec![[0u16; 52]; 52];
    let mut i = 0u16;
    for high in 1..52u8 {
        for low in 0..high {
            let (h, l) = (Card::from_index(high), Card::from_index(low));
            cards.push((h, l));
            masks.push(h.mask() | l.mask());
            class169.push(class_of(h, l));
            index[high as usize][low as usize] = i;
            index[low as usize][high as usize] = i;
            i += 1;
        }
    }
    assert_eq!(i as usize, NCOMBOS);
    ComboTable { cards, masks, class169, index }
}

/// 169-class index: pairs 0..13 by rank, suited 13.., offsuit 91...
pub fn class_of(a: Card, b: Card) -> u8 {
    let (ra, rb) = (a.rank(), b.rank());
    let (high, low) = if ra >= rb { (ra as usize, rb as usize) } else { (rb as usize, ra as usize) };
    if high == low {
        high as u8
    } else {
        let tri = high * (high - 1) / 2 + low;
        if a.suit() == b.suit() {
            (13 + tri) as u8
        } else {
            (91 + tri) as u8
        }
    }
}

/// Combos per class index: 6 pair, 4 suited, 12 offsuit.
pub fn class_combos(class: usize) -> u32 {
    if class < 13 {
        6
    } else if class < 91 {
        4
    } else {
        12
    }
}

pub fn combo_index(a: Card, b: Card) -> usize {
    combos().index[a.index()][b.index()] as usize
}

/// Mask of the five flop/turn/river cards or the like, from a slice.
pub fn mask(cards: &[Card]) -> u64 {
    cards.iter().fold(0, |m, c| m | c.mask())
}

/// Text name of a 169 class, e.g. `AKs`, `72o`, `TT`.
pub fn class_name(class: usize) -> String {
    const RANKS: &[u8; 13] = b"23456789TJQKA";
    if class < 13 {
        let r = RANKS[class] as char;
        format!("{r}{r}")
    } else {
        let (suffix, tri) = if class < 91 { ('s', class - 13) } else { ('o', class - 91) };
        let mut high = 1usize;
        while high * (high + 1) / 2 <= tri {
            high += 1;
        }
        let low = tri - high * (high - 1) / 2;
        format!("{}{}{}", RANKS[high] as char, RANKS[low] as char, suffix)
    }
}
