//! The preflop card, parsed at compile time from `data/preflop_card.txt`.

use crate::cards::Holding;

pub const CARD_TEXT: &str = include_str!("../data/preflop_card.txt");

/// A malformed card is a compile error, because this is evaluated in a const.
pub static CARD: PreflopCard = parse_card(CARD_TEXT);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Seat {
    Early,
    Middle,
    Cutoff,
    Button,
    SmallBlind,
    BigBlind,
}

impl Seat {
    pub const ALL: [Seat; 6] = [
        Seat::Early,
        Seat::Middle,
        Seat::Cutoff,
        Seat::Button,
        Seat::SmallBlind,
        Seat::BigBlind,
    ];

    /// The seat follows from how many players are still to act behind you.
    pub fn from_behind(behind: u8) -> Seat {
        match behind {
            0 => Seat::BigBlind,
            1 => Seat::SmallBlind,
            2 => Seat::Button,
            3 => Seat::Cutoff,
            4 => Seat::Middle,
            _ => Seat::Early,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Seat::Early => "early",
            Seat::Middle => "middle",
            Seat::Cutoff => "cutoff",
            Seat::Button => "button",
            Seat::SmallBlind => "small_blind",
            Seat::BigBlind => "big_blind",
        }
    }

    /// The seat whose opening range is the next one looser than this seat's.
    /// On the card the small blind (258 combos) is looser than the cutoff (254)
    /// and tighter only than the button. The button is already the loosest, and
    /// the big blind has no opening range, so both stay where they are.
    pub fn next_looser(self) -> Seat {
        match self {
            Seat::Early => Seat::Middle,
            Seat::Middle => Seat::Cutoff,
            Seat::Cutoff | Seat::SmallBlind | Seat::Button => Seat::Button,
            Seat::BigBlind => Seat::BigBlind,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Situation {
    FoldedTo,
    Limpers(u8),
    Raised,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Action {
    Raise,
    Call,
    Fold,
}

impl Action {
    pub fn code(self) -> &'static str {
        match self {
            Action::Raise => "r",
            Action::Call => "c",
            Action::Fold => "f",
        }
    }
}

/// What an action is called in a situation: calling with limpers in front is
/// limping along, and calling in the big blind without a raise is a check.
pub fn action_label(action: Action, seat: Seat, situation: Situation) -> &'static str {
    match (action, situation) {
        (Action::Raise, _) => "raise",
        (Action::Fold, _) => "fold",
        (Action::Call, Situation::Raised) => "call",
        (Action::Call, _) if seat == Seat::BigBlind => "check",
        (Action::Call, Situation::Limpers(_)) => "limp along",
        (Action::Call, Situation::FoldedTo) => "call",
    }
}

/// Why the card gives its answer; a front end turns this into text.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PreflopReason {
    InOpen,
    NotInOpen,
    /// The big blind is never folded to in a dealt item; with no raise it checks.
    BigBlindNoRaise,
    IsoRaise,
    IsoLimp,
    IsoLimpNeedsLateOrTwoLimpers,
    NotInIso,
    BigBlindIsoRaise,
    BigBlindCheck,
    Reraise,
    FaceRaiseCall,
    NotInFaceRaise,
}

pub fn preflop_answer(seat: Seat, situation: Situation, hand: Holding) -> (Action, PreflopReason) {
    let card = &CARD;
    match situation {
        Situation::FoldedTo => match card.open_range(seat) {
            None => (Action::Call, PreflopReason::BigBlindNoRaise),
            Some(line) if line.range.contains_holding(hand) => (Action::Raise, PreflopReason::InOpen),
            Some(_) => (Action::Fold, PreflopReason::NotInOpen),
        },
        Situation::Limpers(_) if seat == Seat::BigBlind => {
            if card.iso_raise.range.contains_holding(hand) {
                (Action::Raise, PreflopReason::BigBlindIsoRaise)
            } else {
                (Action::Call, PreflopReason::BigBlindCheck)
            }
        }
        Situation::Limpers(limpers) => {
            if card.iso_raise.range.contains_holding(hand) {
                (Action::Raise, PreflopReason::IsoRaise)
            } else if card.iso_limp.range.contains_holding(hand) {
                if limpers >= 2 || matches!(seat, Seat::Cutoff | Seat::Button | Seat::SmallBlind) {
                    (Action::Call, PreflopReason::IsoLimp)
                } else {
                    (Action::Fold, PreflopReason::IsoLimpNeedsLateOrTwoLimpers)
                }
            } else {
                (Action::Fold, PreflopReason::NotInIso)
            }
        }
        Situation::Raised => {
            if card.reraise.range.contains_holding(hand) {
                (Action::Raise, PreflopReason::Reraise)
            } else if card.call.range.contains_holding(hand) {
                (Action::Call, PreflopReason::FaceRaiseCall)
            } else {
                (Action::Fold, PreflopReason::NotInFaceRaise)
            }
        }
    }
}

/// A set of starting-hand classes held as a 13x13 grid. For two ranks
/// `high > low`, suited hands use bit `low` of row `high`, offsuit hands use bit
/// `high` of row `low`, and pairs use the diagonal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Range {
    grid: [u16; 13],
}

impl Range {
    pub const EMPTY: Range = Range { grid: [0; 13] };

    pub fn contains(&self, rank_a: u8, rank_b: u8, suited: bool) -> bool {
        let (high, low) = if rank_a >= rank_b { (rank_a, rank_b) } else { (rank_b, rank_a) };
        let (row, column) = if high == low || suited { (high, low) } else { (low, high) };
        self.grid[row as usize] >> column & 1 == 1
    }

    pub fn contains_holding(&self, hand: Holding) -> bool {
        let suited = hand[0].suit() == hand[1].suit();
        self.contains(hand[0].rank(), hand[1].rank(), suited)
    }

    /// Number of 2-card combinations in the range.
    pub fn combo_count(&self) -> u32 {
        let mut total = 0;
        for high in 0..13u8 {
            for low in 0..=high {
                if high == low {
                    if self.contains(high, low, false) {
                        total += 6;
                    }
                } else {
                    if self.contains(high, low, true) {
                        total += 4;
                    }
                    if self.contains(high, low, false) {
                        total += 12;
                    }
                }
            }
        }
        total
    }

    /// Share of all 1,326 starting combinations, 0 to 100.
    pub fn percent_of_hands(&self) -> f64 {
        f64::from(self.combo_count()) * 100.0 / 1326.0
    }

    pub fn union(&self, other: &Range) -> Range {
        let mut grid = self.grid;
        let mut row = 0;
        while row < 13 {
            grid[row] |= other.grid[row];
            row += 1;
        }
        Range { grid }
    }

    const fn with(mut self, row: u8, column: u8) -> Range {
        self.grid[row as usize] |= 1 << column;
        self
    }
}

/// A range together with its notation as written on the card.
#[derive(Clone, Copy, Debug)]
pub struct CardLine {
    pub text: &'static str,
    pub range: Range,
}

impl CardLine {
    const EMPTY: CardLine = CardLine { text: "", range: Range::EMPTY };
}

#[derive(Clone, Copy, Debug)]
pub struct CardNote {
    pub key: &'static str,
    pub text: &'static str,
}

pub const MAX_NOTES: usize = 8;

#[derive(Clone, Copy, Debug)]
pub struct PreflopCard {
    /// The comment block at the top of the file, `#` markers included.
    pub header: &'static str,
    pub open: [CardLine; 5],
    pub iso_raise: CardLine,
    pub iso_limp: CardLine,
    pub reraise: CardLine,
    pub call: CardLine,
    pub notes: [CardNote; MAX_NOTES],
    pub note_count: usize,
}

impl PreflopCard {
    /// The opening range for a seat; the big blind has none.
    pub fn open_range(&self, seat: Seat) -> Option<&CardLine> {
        self.open.get(seat as usize)
    }

    /// True if any line of the card would ever play this hand.
    pub fn plays(&self, hand: Holding) -> bool {
        self.open.iter().any(|line| line.range.contains_holding(hand))
            || [&self.iso_raise, &self.iso_limp, &self.reraise, &self.call]
                .iter()
                .any(|line| line.range.contains_holding(hand))
    }

    pub fn notes(&self) -> &[CardNote] {
        &self.notes[..self.note_count]
    }

    pub fn note(&self, key: &str) -> &'static str {
        self.notes().iter().find(|n| n.key == key).map_or("", |n| n.text)
    }
}

const fn sub_str(bytes: &'static [u8], start: usize, end: usize) -> &'static str {
    let (_, tail) = bytes.split_at(start);
    let (middle, _) = tail.split_at(end - start);
    match core::str::from_utf8(middle) {
        Ok(text) => text,
        Err(_) => panic!("preflop card is not valid UTF-8"),
    }
}

const fn key_is(bytes: &[u8], start: usize, end: usize, key: &[u8]) -> bool {
    if end - start != key.len() {
        return false;
    }
    let mut i = 0;
    while i < key.len() {
        if bytes[start + i] != key[i] {
            return false;
        }
        i += 1;
    }
    true
}

const fn is_space(b: u8) -> bool {
    b == b' ' || b == b'\t' || b == b'\r'
}

const fn parse_rank(b: u8) -> u8 {
    match b {
        b'2'..=b'9' => b - b'2',
        b'T' => 8,
        b'J' => 9,
        b'Q' => 10,
        b'K' => 11,
        b'A' => 12,
        _ => panic!("bad rank in preflop card"),
    }
}

/// Parses tokens such as `22+`, `ATs+`, `AQo+`, `QJs` and `KQo`.
const fn parse_range(bytes: &[u8], mut i: usize, end: usize) -> Range {
    let mut range = Range::EMPTY;
    while i < end {
        while i < end && is_space(bytes[i]) {
            i += 1;
        }
        if i >= end {
            break;
        }
        let start = i;
        while i < end && !is_space(bytes[i]) {
            i += 1;
        }
        let mut len = i - start;
        let plus = bytes[start + len - 1] == b'+';
        if plus {
            len -= 1;
        }
        if len < 2 {
            panic!("bad token in preflop card");
        }
        let high = parse_rank(bytes[start]);
        let low = parse_rank(bytes[start + 1]);
        if high == low {
            if len != 2 {
                panic!("a pair takes no suitedness marker");
            }
            let top = if plus { 12 } else { high };
            let mut r = high;
            while r <= top {
                range = range.with(r, r);
                r += 1;
            }
        } else {
            if len != 3 || high < low {
                panic!("unpaired token needs high rank first and s or o");
            }
            let suited = match bytes[start + 2] {
                b's' => true,
                b'o' => false,
                _ => panic!("expected s or o in preflop card"),
            };
            let top = if plus { high - 1 } else { low };
            let mut r = low;
            while r <= top {
                range = if suited { range.with(high, r) } else { range.with(r, high) };
                r += 1;
            }
        }
    }
    range
}

/// Parses the card file. Panics (a compile error in const context) on anything malformed.
pub const fn parse_card(text: &'static str) -> PreflopCard {
    let bytes = text.as_bytes();
    let mut card = PreflopCard {
        header: "",
        open: [CardLine::EMPTY; 5],
        iso_raise: CardLine::EMPTY,
        iso_limp: CardLine::EMPTY,
        reraise: CardLine::EMPTY,
        call: CardLine::EMPTY,
        notes: [CardNote { key: "", text: "" }; MAX_NOTES],
        note_count: 0,
    };
    let mut pos = 0;
    let mut seen = 0u16;
    let mut header_end = 0;
    let mut in_header = true;
    while pos < bytes.len() {
        let mut line_end = pos;
        while line_end < bytes.len() && bytes[line_end] != b'\n' {
            line_end += 1;
        }
        let mut start = pos;
        let mut end = line_end;
        pos = line_end + 1;
        while start < end && is_space(bytes[start]) {
            start += 1;
        }
        while end > start && is_space(bytes[end - 1]) {
            end -= 1;
        }
        if start == end {
            continue;
        }
        if bytes[start] == b'#' {
            if in_header {
                header_end = line_end;
            }
            continue;
        }
        in_header = false;
        let mut equals = start;
        while equals < end && bytes[equals] != b'=' {
            equals += 1;
        }
        if equals == end {
            panic!("preflop card line has no =");
        }
        let mut key_end = equals;
        while key_end > start && is_space(bytes[key_end - 1]) {
            key_end -= 1;
        }
        let mut value = equals + 1;
        while value < end && is_space(bytes[value]) {
            value += 1;
        }
        if key_is(bytes, start, key_end, b"note") || (key_end - start > 5 && key_is(bytes, start, start + 5, b"note.")) {
            if card.note_count == MAX_NOTES {
                panic!("too many notes in preflop card");
            }
            card.notes[card.note_count] = CardNote {
                key: sub_str(bytes, start, key_end),
                text: sub_str(bytes, value, end),
            };
            card.note_count += 1;
            continue;
        }
        let line = CardLine {
            text: sub_str(bytes, value, end),
            range: parse_range(bytes, value, end),
        };
        let mut slot = 0;
        if key_is(bytes, start, key_end, b"open.early") {
            card.open[Seat::Early as usize] = line;
        } else if key_is(bytes, start, key_end, b"open.middle") {
            slot = 1;
            card.open[Seat::Middle as usize] = line;
        } else if key_is(bytes, start, key_end, b"open.cutoff") {
            slot = 2;
            card.open[Seat::Cutoff as usize] = line;
        } else if key_is(bytes, start, key_end, b"open.button") {
            slot = 3;
            card.open[Seat::Button as usize] = line;
        } else if key_is(bytes, start, key_end, b"open.small_blind") {
            slot = 4;
            card.open[Seat::SmallBlind as usize] = line;
        } else if key_is(bytes, start, key_end, b"iso.raise") {
            slot = 5;
            card.iso_raise = line;
        } else if key_is(bytes, start, key_end, b"iso.limp") {
            slot = 6;
            card.iso_limp = line;
        } else if key_is(bytes, start, key_end, b"face_raise.reraise") {
            slot = 7;
            card.reraise = line;
        } else if key_is(bytes, start, key_end, b"face_raise.call") {
            slot = 8;
            card.call = line;
        } else {
            panic!("unknown key in preflop card");
        }
        seen |= 1 << slot;
    }
    if seen != 0b1_1111_1111 {
        panic!("preflop card is missing a line");
    }
    card.header = sub_str(bytes, 0, header_end);
    card
}
