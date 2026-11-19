//! Limpers preflop drill: ranges, raise sizing, grading and weighted generator.
//!
//! Encodes section "Q7. Practical ranges and rules for Drew" from
//! `~/Software/postflop-lab/research/limper-preflop-2026-11-17.md`:
//! the "Vs one limper" table, the "Vs two or more limpers" rules, and the
//! raise-size rule. Hands where that section gives no answer are left
//! `Unspecified` and are never dealt.

use crate::cards::{Card, Holding};
use crate::preflop::{Action, Range};
use crate::rng::Rng;

const FOUR: u8 = 2;
const SIX: u8 = 4;
const NINE: u8 = 7;
const TEN: u8 = 8;
const JACK: u8 = 9;
const QUEEN: u8 = 10;
const KING: u8 = 11;
const ACE: u8 = 12;

/// Seats at a 5- or 6-seat table, in preflop acting order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum LimperSeat {
    Utg,
    Utg1,
    Co,
    Btn,
    Sb,
    Bb,
}

impl LimperSeat {
    pub const FIVE_HANDED: [LimperSeat; 5] = [
        LimperSeat::Utg,
        LimperSeat::Co,
        LimperSeat::Btn,
        LimperSeat::Sb,
        LimperSeat::Bb,
    ];

    pub const SIX_HANDED: [LimperSeat; 6] = [
        LimperSeat::Utg,
        LimperSeat::Utg1,
        LimperSeat::Co,
        LimperSeat::Btn,
        LimperSeat::Sb,
        LimperSeat::Bb,
    ];

    pub fn seats_for_table(players: u8) -> &'static [LimperSeat] {
        match players {
            5 => &Self::FIVE_HANDED,
            6 => &Self::SIX_HANDED,
            _ => panic!("limpers drill uses 5- or 6-seat tables, got {players}"),
        }
    }

    /// Hero's seat from table size (`5` or `6`) and players still to act behind hero.
    pub fn from_behind(players: u8, behind: u8) -> Option<LimperSeat> {
        let seats = match players {
            5 => &Self::FIVE_HANDED[..],
            6 => &Self::SIX_HANDED[..],
            _ => return None,
        };
        let index = seats.len().checked_sub(1 + usize::from(behind))?;
        seats.get(index).copied()
    }

    pub fn parse(text: &str) -> Option<LimperSeat> {
        match text.trim().to_ascii_uppercase().as_str() {
            "UTG" => Some(LimperSeat::Utg),
            "UTG+1" | "UTG1" => Some(LimperSeat::Utg1),
            "CO" => Some(LimperSeat::Co),
            "BTN" | "BU" => Some(LimperSeat::Btn),
            "SB" => Some(LimperSeat::Sb),
            "BB" => Some(LimperSeat::Bb),
            _ => None,
        }
    }

    pub const fn short(self) -> &'static str {
        match self {
            LimperSeat::Utg => "UTG",
            LimperSeat::Utg1 => "UTG+1",
            LimperSeat::Co => "CO",
            LimperSeat::Btn => "BTN",
            LimperSeat::Sb => "SB",
            LimperSeat::Bb => "BB",
        }
    }

    pub const fn title(self) -> &'static str {
        match self {
            LimperSeat::Utg => "UTG",
            LimperSeat::Utg1 => "UTG+1",
            LimperSeat::Co => "Cutoff",
            LimperSeat::Btn => "Button",
            LimperSeat::Sb => "Small blind",
            LimperSeat::Bb => "Big blind",
        }
    }

    pub const fn sentence_phrase(self) -> &'static str {
        match self {
            LimperSeat::Utg => "under the gun",
            LimperSeat::Utg1 => "UTG+1",
            LimperSeat::Co => "in the cutoff",
            LimperSeat::Btn => "on the button",
            LimperSeat::Sb => "in the small blind",
            LimperSeat::Bb => "in the big blind",
        }
    }

    pub const fn is_blind(self) -> bool {
        matches!(self, LimperSeat::Sb | LimperSeat::Bb)
    }

    pub const fn row_seat(self) -> RowSeat {
        match self {
            LimperSeat::Utg | LimperSeat::Utg1 => RowSeat::Early,
            LimperSeat::Co => RowSeat::Co,
            LimperSeat::Btn => RowSeat::Btn,
            LimperSeat::Sb => RowSeat::Sb,
            LimperSeat::Bb => RowSeat::Bb,
        }
    }

    pub const fn family_code(self) -> &'static str {
        match self {
            LimperSeat::Utg | LimperSeat::Utg1 => "early",
            LimperSeat::Co => "cutoff",
            LimperSeat::Btn => "button",
            LimperSeat::Sb => "small_blind",
            LimperSeat::Bb => "big_blind",
        }
    }
}

/// The five rows in Q7's range table.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum RowSeat {
    Early,
    Co,
    Btn,
    Sb,
    Bb,
}

impl RowSeat {
    pub const ALL: [RowSeat; 5] = [RowSeat::Early, RowSeat::Co, RowSeat::Btn, RowSeat::Sb, RowSeat::Bb];
}

/// What the Q7 table prescribes for a starting-hand class.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum LimperDecision {
    Raise,
    /// Overlimp (call 1bb), complete in SB, or check in BB.
    Call,
    Fold,
    /// Q7 gives no answer for this hand in this spot; never dealt.
    Unspecified,
}

/// One row of Q7's table: `(seat, 1 limper vs 2+ limpers)` with mutually disjoint
/// `raise`, `call` and `fold` ranges. Any class outside all three is `Unspecified`.
#[derive(Clone, Copy, Debug)]
pub struct TableRow {
    pub seat: RowSeat,
    pub multiway: bool,
    pub raise: Range,
    pub call: Range,
    pub fold: Range,
    pub summary: &'static str,
}

impl TableRow {
    pub const fn decision_class(&self, rank_a: u8, rank_b: u8, suited: bool) -> LimperDecision {
        if self.raise.contains_const(rank_a, rank_b, suited) {
            LimperDecision::Raise
        } else if self.call.contains_const(rank_a, rank_b, suited) {
            LimperDecision::Call
        } else if self.fold.contains_const(rank_a, rank_b, suited) {
            LimperDecision::Fold
        } else {
            LimperDecision::Unspecified
        }
    }

    pub fn decision(&self, hand: Holding) -> LimperDecision {
        let suited = hand[0].suit() == hand[1].suit();
        self.decision_class(hand[0].rank(), hand[1].rank(), suited)
    }
}

const fn is_space(b: u8) -> bool {
    b == b' ' || b == b'\t' || b == b'\r' || b == b'\n'
}

const fn parse_rank(b: u8) -> u8 {
    match b {
        b'2'..=b'9' => b - b'2',
        b'T' => TEN,
        b'J' => JACK,
        b'Q' => QUEEN,
        b'K' => KING,
        b'A' => ACE,
        _ => panic!("bad rank in limper table"),
    }
}

/// Parses one hand token (`88`, `KQs`, `QJo`) into `(high, low, suited)`.
const fn parse_single(bytes: &[u8], start: usize, len: usize) -> (u8, u8, bool) {
    if len < 2 || len > 3 {
        panic!("bad token length in limper table");
    }
    let high = parse_rank(bytes[start]);
    let low = parse_rank(bytes[start + 1]);
    if high == low {
        if len != 2 {
            panic!("pair token takes no suitedness suffix");
        }
        (high, low, false)
    } else {
        if len != 3 || high < low {
            panic!("unpaired token needs high rank first and s/o suffix");
        }
        let suited = match bytes[start + 2] {
            b's' => true,
            b'o' => false,
            _ => panic!("expected s or o in limper table"),
        };
        (high, low, suited)
    }
}

/// Compile-time range parser supporting single hands (`KQs`), plus ranges (`88+`,
/// `ATs+`, `J9s+`), pair ranges (`22-77`), kicker ranges (`A2s-ATs`, `K7o-K2o`),
/// and same-gap diagonal ranges (`T9s-65s`, `54s-87s`, `J9s-64s`).
const fn parse_range(text: &str) -> Range {
    let bytes = text.as_bytes();
    let mut range = Range::EMPTY;
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && is_space(bytes[i]) {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let start = i;
        while i < bytes.len() && !is_space(bytes[i]) {
            i += 1;
        }
        let end = i;
        let len = end - start;

        // Check for a dash range `A-B`.
        let mut dash = None;
        let mut j = start;
        while j < end {
            if bytes[j] == b'-' {
                dash = Some(j);
                break;
            }
            j += 1;
        }

        if let Some(d) = dash {
            let (h1, l1, s1) = parse_single(bytes, start, d - start);
            let (h2, l2, s2) = parse_single(bytes, d + 1, end - (d + 1));
            if h1 == l1 && h2 == l2 {
                let (lo, hi) = if h1 <= h2 { (h1, h2) } else { (h2, h1) };
                let mut r = lo;
                while r <= hi {
                    range = range.with_class(r, r, false);
                    r += 1;
                }
            } else if s1 == s2 && h1 == h2 && h1 != l1 && h2 != l2 {
                let (lo, hi) = if l1 <= l2 { (l1, l2) } else { (l2, l1) };
                let mut k = lo;
                while k <= hi {
                    range = range.with_class(h1, k, s1);
                    k += 1;
                }
            } else if s1 == s2 && h1 != l1 && h2 != l2 && h1 - l1 == h2 - l2 {
                let gap = h1 - l1;
                let (lo, hi) = if h1 <= h2 { (h1, h2) } else { (h2, h1) };
                let mut h = lo;
                while h <= hi {
                    range = range.with_class(h, h - gap, s1);
                    h += 1;
                }
            } else {
                panic!("unsupported dash range in limper table");
            }
            continue;
        }

        let plus = bytes[end - 1] == b'+';
        let body_len = if plus { len - 1 } else { len };
        let (high, low, suited) = parse_single(bytes, start, body_len);
        if high == low {
            let top = if plus { ACE } else { high };
            let mut r = high;
            while r <= top {
                range = range.with_class(r, r, false);
                r += 1;
            }
        } else {
            let top = if plus { high - 1 } else { low };
            let mut k = low;
            while k <= top {
                range = range.with_class(high, k, suited);
                k += 1;
            }
        }
    }
    range
}

const fn row(seat: RowSeat, multiway: bool, raise: Range, call: Range, fold: Range, summary: &'static str) -> TableRow {
    assert!(raise.intersect_const(call).is_empty(), "raise and call overlap");
    assert!(raise.intersect_const(fold).is_empty(), "raise and fold overlap");
    assert!(call.intersect_const(fold).is_empty(), "call and fold overlap");
    TableRow { seat, multiway, raise, call, fold, summary }
}

/// Q7 ranges for all five seats, both vs 1 limper (`multiway = false`) and vs 2+ limpers (`multiway = true`).
/// Source: `~/Software/postflop-lab/research/limper-preflop-2026-11-17.md`, section "Q7. Practical ranges and rules for Drew".
pub static TABLE: [TableRow; 10] = [
    // --- Vs one limper ---
    // Early (UTG+1 at 6-handed, 3+ left to act): KTs, QTs, 54s-32s are omitted by Q7 -> Unspecified.
    {
        let raise = parse_range("88+ AJs+ KQs AQo+");
        let call = parse_range("22-77 A2s-ATs KJs QJs JTs T9s-65s");
        let fold = Range::ALL_OFFSUIT
            .without_const(raise)
            .union_const(parse_range("K9s-K2s Q9s-Q2s J9s-J2s T8s-T2s 97s-92s 86s-82s 75s-72s 64s-62s 53s-52s 42s"));
        row(
            RowSeat::Early,
            false,
            raise,
            call,
            fold,
            "Early vs 1 limper: Raise 88+, AJs+, KQs, AQo+. Call 22-77, A2s-ATs, KJs, QJs, JTs, T9s-65s. Fold offsuit and weak suited gappers.",
        )
    },
    // CO vs 1 limper: explicit cutoffs on pairs, suited hands and offsuit; rest folds.
    {
        let raise = parse_range("77+ ATs+ KJs+ QJs AJo+ KQo");
        let call = parse_range("22-66 A2s-A9s KTs QTs J9s+ T8s+ 97s+ 54s-87s");
        let fold = Range::ALL.without_const(raise).without_const(call);
        row(
            RowSeat::Co,
            false,
            raise,
            call,
            fold,
            "CO vs 1 limper: Raise 77+, ATs+, KJs+, QJs, AJo+, KQo. Call 22-66, A2s-A9s, KTs, QTs, J9s+, T8s+, 97s+, 54s-87s. Fold KJo, QJo, ATo, weaker offsuit, and suited below those cutoffs.",
        )
    },
    // BTN vs 1 limper: explicit cutoffs; rest folds.
    {
        let raise = parse_range("77+ A9s+ KTs+ QTs+ JTs ATo+ KJo+");
        let call = parse_range("22-66 A2s-A8s T9s-54s J9s-75s K9s Q9s QJo JTo");
        let fold = Range::ALL.without_const(raise).without_const(call);
        row(
            RowSeat::Btn,
            false,
            raise,
            call,
            fold,
            "BTN vs 1 limper: Raise 77+, A9s+, KTs+, QTs+, JTs, ATo+, KJo+. Call 22-66, A2s-A8s, suited connectors 54s+, one-gappers 75s+, K9s, Q9s, QJo, JTo. Fold the rest.",
        )
    },
    // SB vs 1 limper: complete pairs, all suited, Broadways and T9o+; fold weak offsuit (K7o, Q5o, J4o and worse).
    // Intermediate offsuit (A9o-A2o, K9o-K8o, Q9o-Q6o, J9o-J5o, T8o-T7o, 98o-32o) is left Unspecified.
    {
        let raise = parse_range("99+ AJs+ KQs AQo+");
        let call = Range::ALL_SUITED
            .without_const(raise)
            .union_const(parse_range("22-88 AJo ATo KQo KJo KTo QJo QTo JTo T9o"));
        let fold = parse_range("K7o-K2o Q5o-Q2o J4o-J2o T6o-T2o 95o-92o 84o-82o 74o-72o 63o-62o 52o 42o");
        row(
            RowSeat::Sb,
            false,
            raise,
            call,
            fold,
            "SB vs 1 limper: Raise 99+, AJs+, KQs, AQo+. Call pairs, suited hands, Broadways, and T9o+. Fold weak offsuit (K7o, Q5o, J4o and worse).",
        )
    },
    // BB vs 1 limper: raise TT+, AQs+, AKo; check everything else.
    {
        let raise = parse_range("TT+ AQs+ AKo");
        let call = Range::ALL.without_const(raise);
        row(
            RowSeat::Bb,
            false,
            raise,
            call,
            Range::EMPTY,
            "BB vs 1 limper: Raise TT+, AQs+, AKo. Check everything else.",
        )
    },
    // --- Vs two or more limpers ---
    // Early vs 2+ (not reached at 5-6 seats; encoded for table completeness; AQo left Unspecified).
    {
        let raise = parse_range("TT+ AQs+ AKo");
        let call = parse_range("22-99 A2s-AJs KTs+ QTs+ JTs T9s-54s J9s-64s");
        let fold = Range::ALL
            .without_const(raise)
            .without_const(call)
            .without_const(parse_range("AQo"));
        row(
            RowSeat::Early,
            true,
            raise,
            call,
            fold,
            "Early vs 2+ limpers: Raise TT+, AQs+, AKo. Call 22-99, suited aces, suited Broadways, suited connectors 54s+, one-gappers 64s+. Fold weaker hands.",
        )
    },
    // CO vs 2+ limpers: add 99, AJs, KQs, AQo to raise; fold AJo, KQo and weaker offsuit.
    {
        let raise = parse_range("99+ AJs+ KQs AQo+");
        let call = parse_range("22-88 A2s-ATs KJs KTs QJs QTs JTs T9s-54s J9s-64s");
        let fold = Range::ALL.without_const(raise).without_const(call);
        row(
            RowSeat::Co,
            true,
            raise,
            call,
            fold,
            "CO vs 2+ limpers: Raise 99+, AJs+, KQs, AQo+. Call 22-88, suited aces, suited Broadways, suited connectors 54s+, one-gappers 64s+. Fold AJo, KQo, weaker offsuit, and weak suited hands.",
        )
    },
    // BTN vs 2+ limpers: also overlimp K9s, Q9s, AJo, KQo; fold KJo, QJo, KTo, ATo and unconnected offsuit.
    // Connected non-QJo offsuit (JTo, T9o-32o) is left Unspecified.
    {
        let raise = parse_range("99+ AJs+ KQs AQo+");
        let call = parse_range("22-88 A2s-ATs KJs KTs QJs QTs JTs T9s-54s J9s-64s K9s Q9s AJo KQo");
        let fold = Range::ALL
            .without_const(raise)
            .without_const(call)
            .without_const(parse_range("JTo T9o-32o"));
        row(
            RowSeat::Btn,
            true,
            raise,
            call,
            fold,
            "BTN vs 2+ limpers: Raise 99+, AJs+, KQs, AQo+. Call 22-88, suited aces, suited Broadways, suited connectors 54s+, one-gappers 64s+, K9s, Q9s, AJo, KQo. Fold KJo, QJo, KTo, ATo, unconnected offsuit, and weak suited hands.",
        )
    },
    // SB vs 2+ limpers: raise TT+, AQs+, AKo; complete pairs, strong/broadway/king/queen/jack suited,
    // connectors 54s+/54o+, one-gappers 64s+, AQo, AJo, KQo, JTo. Fold unconnected offsuit.
    // Bottom suited 2+-gappers (T7s-32s) and offsuit KJo/QJo/KTo/ATo/QTo/43o/32o are left Unspecified.
    {
        let raise = parse_range("TT+ AQs+ AKo");
        let call = parse_range("22-99 A2s-AJs KQs-K2s QJs-Q2s JTs-J2s T9s-54s T8s-64s AQo AJo KQo JTo T9o-54o");
        let fold = parse_range("A9o-A2o K9o-K2o Q9o-Q2o J9o-J2o T8o-T2o 97o-92o 86o-82o 75o-72o 64o-62o 53o-52o 42o");
        row(
            RowSeat::Sb,
            true,
            raise,
            call,
            fold,
            "SB vs 2+ limpers: Raise TT+, AQs+, AKo. Complete pairs, suited hands, connected hands, AQo, AJo, KQo. Fold unconnected offsuit.",
        )
    },
    // BB vs 2+ limpers: raise TT+, AQs+, AKo; check everything else.
    {
        let raise = parse_range("TT+ AQs+ AKo");
        let call = Range::ALL.without_const(raise);
        row(
            RowSeat::Bb,
            true,
            raise,
            call,
            Range::EMPTY,
            "BB vs 2+ limpers: Raise TT+, AQs+, AKo. Check everything else.",
        )
    },
];

pub fn table_row(seat: RowSeat, multiway: bool) -> &'static TableRow {
    let idx = match seat {
        RowSeat::Early => 0,
        RowSeat::Co => 1,
        RowSeat::Btn => 2,
        RowSeat::Sb => 3,
        RowSeat::Bb => 4,
    } + if multiway { 5 } else { 0 };
    &TABLE[idx]
}

pub fn lookup_class(seat: RowSeat, multiway: bool, rank_a: u8, rank_b: u8, suited: bool) -> LimperDecision {
    table_row(seat, multiway).decision_class(rank_a, rank_b, suited)
}

pub fn lookup(seat: LimperSeat, limpers: u8, hand: Holding) -> LimperDecision {
    table_row(seat.row_seat(), limpers >= 2).decision(hand)
}

/// Raise size in big blinds from Q7's size rule (no ante):
/// 6bb IP / 7bb blinds vs 1 limper; 8bb / 9bb vs 2; 10bb / 11bb vs 3.
pub const fn raise_size_bb(seat: LimperSeat, limpers: u8) -> u8 {
    4 + 2 * limpers + if seat.is_blind() { 1 } else { 0 }
}

/// Fixed set of short one-line reasons shown on the reveal.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum LimperReason {
    ValueRaise,
    BigBlindValueRaise,
    CheapFlopPair,
    CheapFlopSuited,
    PositionOverlimp,
    SmallBlindPrice,
    BigBlindFreeFlop,
    DominatedFold,
    TooWeakFold,
}

impl LimperReason {
    pub const ALL: [LimperReason; 9] = [
        LimperReason::ValueRaise,
        LimperReason::BigBlindValueRaise,
        LimperReason::CheapFlopPair,
        LimperReason::CheapFlopSuited,
        LimperReason::PositionOverlimp,
        LimperReason::SmallBlindPrice,
        LimperReason::BigBlindFreeFlop,
        LimperReason::DominatedFold,
        LimperReason::TooWeakFold,
    ];

    pub const fn text(self) -> &'static str {
        match self {
            LimperReason::ValueRaise => "Beats what they limp-call with.",
            LimperReason::BigBlindValueRaise => "Too strong to check: raise for value.",
            LimperReason::CheapFlopPair => "Wants a cheap flop to hit a set: call.",
            LimperReason::CheapFlopSuited => "Wants a cheap flop: call.",
            LimperReason::PositionOverlimp => "Plays well in position for 1bb: call.",
            LimperReason::SmallBlindPrice => "Great price in the small blind: call.",
            LimperReason::BigBlindFreeFlop => "Checking is free; not strong enough to raise.",
            LimperReason::DominatedFold => "Dominated when called: fold.",
            LimperReason::TooWeakFold => "Too weak to play here: fold.",
        }
    }
}

pub const SIZE_RULE_NOTE: &str =
    "Size rule: 6bb IP or 7bb from the blinds vs 1 limper, 8bb / 9bb vs 2, 10bb / 11bb vs 3 (+1bb with an ante; bigger is fine with AA/KK).";

pub fn reason_for(seat: LimperSeat, action: Action, hand: Holding) -> LimperReason {
    let (a, b) = (hand[0].rank(), hand[1].rank());
    let (high, low) = if a >= b { (a, b) } else { (b, a) };
    let suited = hand[0].suit() == hand[1].suit();
    match action {
        Action::Raise => {
            if seat == LimperSeat::Bb {
                LimperReason::BigBlindValueRaise
            } else {
                LimperReason::ValueRaise
            }
        }
        Action::Call => {
            if seat == LimperSeat::Bb {
                LimperReason::BigBlindFreeFlop
            } else if high == low {
                LimperReason::CheapFlopPair
            } else if seat == LimperSeat::Sb {
                LimperReason::SmallBlindPrice
            } else if suited {
                LimperReason::CheapFlopSuited
            } else {
                LimperReason::PositionOverlimp
            }
        }
        Action::Fold => {
            if high >= JACK {
                LimperReason::DominatedFold
            } else {
                LimperReason::TooWeakFold
            }
        }
    }
}

/// One limpers question.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LimpersQuestion {
    pub players: u8,
    pub seat: LimperSeat,
    /// Seats that limped before hero, in acting order (1, 2 or 3 seats).
    pub limpers: Vec<LimperSeat>,
    pub hand: Holding,
}

impl LimpersQuestion {
    pub fn limper_count(&self) -> u8 {
        self.limpers.len() as u8
    }

    pub fn multiway(&self) -> bool {
        self.limpers.len() >= 2
    }

    pub fn with_suit_perm(&self, perm: &[u8; 4]) -> LimpersQuestion {
        LimpersQuestion {
            players: self.players,
            seat: self.seat,
            limpers: self.limpers.clone(),
            hand: [self.hand[0].with_suit_perm(perm), self.hand[1].with_suit_perm(perm)],
        }
    }

    pub fn limpers_short_list(&self) -> String {
        self.limpers.iter().map(|s| s.short()).collect::<Vec<_>>().join(", ")
    }

    /// One short line of context, e.g. `"CO limps. You are on the button."`.
    pub fn context_line(&self) -> String {
        let who = match self.limpers.as_slice() {
            [a] => format!("{} limps.", a.short()),
            [a, b] => format!("{} and {} limp.", a.short(), b.short()),
            [a, b, c] => format!("{}, {}, and {} limp.", a.short(), b.short(), c.short()),
            many => format!("{} limp.", many.iter().map(|s| s.short()).collect::<Vec<_>>().join(", ")),
        };
        format!("{who} You are {}.", self.seat.sentence_phrase())
    }
}

/// Graded answer and reveal fields for a limpers question.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LimperGrade {
    pub seat: LimperSeat,
    pub limpers: u8,
    pub action: Action,
    pub raise_bb: Option<u8>,
    pub reason: LimperReason,
    pub row_summary: &'static str,
}

impl LimperGrade {
    pub fn best_text(&self) -> String {
        match self.action {
            Action::Raise => format!("Raise to {}bb", self.raise_bb.expect("raise has size")),
            Action::Call if self.seat == LimperSeat::Bb => "Check".to_string(),
            Action::Call => "Call".to_string(),
            Action::Fold => "Fold".to_string(),
        }
    }
}

/// Grades a `(seat, limpers, hand)` spot from Q7. Returns `None` for `Unspecified` hands.
pub fn answer(seat: LimperSeat, limpers: u8, hand: Holding) -> Option<LimperGrade> {
    let row = table_row(seat.row_seat(), limpers >= 2);
    let action = match row.decision(hand) {
        LimperDecision::Raise => Action::Raise,
        LimperDecision::Call => Action::Call,
        LimperDecision::Fold => Action::Fold,
        LimperDecision::Unspecified => return None,
    };
    let raise_bb = (action == Action::Raise).then(|| raise_size_bb(seat, limpers));
    let reason = reason_for(seat, action, hand);
    Some(LimperGrade {
        seat,
        limpers,
        action,
        raise_bb,
        reason,
        row_summary: row.summary,
    })
}

/// True if `(rank_a, rank_b, suited)` sits within one step of a different decision
/// (including `Unspecified`) along its natural hand progression in `row`.
pub fn is_boundary(row: &TableRow, rank_a: u8, rank_b: u8, suited: bool) -> bool {
    let (high, low) = if rank_a >= rank_b { (rank_a, rank_b) } else { (rank_b, rank_a) };
    let here = row.decision_class(high, low, suited);
    if here == LimperDecision::Unspecified {
        return false;
    }
    let differs = |h: u8, l: u8, s: bool| row.decision_class(h, l, s) != here;

    if high == low {
        (high > 0 && differs(high - 1, low - 1, false))
            || (high < ACE && differs(high + 1, low + 1, false))
    } else if high >= JACK {
        (low > 0 && differs(high, low - 1, suited))
            || (low + 1 < high && differs(high, low + 1, suited))
    } else if high - low <= 2 {
        // Connectors and 1-gappers with high <= Ten step diagonally.
        (low > 0 && differs(high - 1, low - 1, suited))
            || (high < ACE && differs(high + 1, low + 1, suited))
    } else {
        (low > 0 && differs(high, low - 1, suited))
            || (low + 1 < high && differs(high, low + 1, suited))
    }
}

/// Priority leak categories weighted heavily by the generator:
/// weak suited K/Q/J, offsuit Broadways, small pairs (22-66), and suited connectors (T9s-54s).
pub const fn is_priority_hand(rank_a: u8, rank_b: u8, suited: bool) -> bool {
    let (high, low) = if rank_a >= rank_b { (rank_a, rank_b) } else { (rank_b, rank_a) };
    if high == low {
        return high <= SIX;
    }
    if suited {
        if (high == KING || high == QUEEN || high == JACK) && low <= NINE {
            return true;
        }
        if high - low == 1 && high <= TEN && low >= FOUR {
            return true;
        }
        false
    } else {
        low >= TEN
    }
}

/// Other playable/tempting hands that deserve more reps than plain unplayable trash.
const fn is_relevant_hand(high: u8, low: u8, suited: bool) -> bool {
    if high == low {
        return true;
    }
    if suited {
        high == ACE || low >= TEN || (high - low <= 2 && low >= FOUR)
    } else {
        high == ACE || (high - low == 1 && high <= TEN && low >= FOUR)
    }
}

/// Relative weight multiplier for boundary vs interior hands (3:10, so boundary hands
/// are asked about 0.3x as often per class as interior hands).
pub const BOUNDARY_WEIGHT: u64 = 3;
pub const INTERIOR_WEIGHT: u64 = 10;

fn class_weight(row: &TableRow, high: u8, low: u8, suited: bool) -> u64 {
    let tier = if is_priority_hand(high, low, suited) {
        12
    } else if is_relevant_hand(high, low, suited) {
        4
    } else {
        1
    };
    let edge = if is_boundary(row, high, low, suited) {
        BOUNDARY_WEIGHT
    } else {
        INTERIOR_WEIGHT
    };
    tier * edge
}

/// Family tag logged for review scheduling: hero's seat group and 1 vs 2+ limpers.
pub fn limpers_family(q: &LimpersQuestion) -> String {
    format!("{}:{}", q.seat.family_code(), if q.multiway() { "2plus" } else { "1" })
}

fn pick_weighted<T: Copy>(rng: &mut Rng, choices: &[(T, u64)]) -> T {
    let total: u64 = choices.iter().map(|&(_, w)| w).sum();
    let mut roll = rng.below(total);
    for &(item, weight) in choices {
        if roll < weight {
            return item;
        }
        roll -= weight;
    }
    unreachable!("roll is below total weight")
}

/// Generates a deterministic limpers drill question.
pub fn gen_limpers(rng: &mut Rng) -> LimpersQuestion {
    let players: u8 = if rng.chance(1, 2) { 5 } else { 6 };
    let seat = if players == 5 {
        pick_weighted(
            rng,
            &[
                (LimperSeat::Co, 20),
                (LimperSeat::Btn, 25),
                (LimperSeat::Sb, 35),
                (LimperSeat::Bb, 20),
            ],
        )
    } else {
        pick_weighted(
            rng,
            &[
                (LimperSeat::Utg1, 15),
                (LimperSeat::Co, 20),
                (LimperSeat::Btn, 20),
                (LimperSeat::Sb, 30),
                (LimperSeat::Bb, 15),
            ],
        )
    };

    let seats = LimperSeat::seats_for_table(players);
    let hero_idx = seats.iter().position(|&s| s == seat).expect("hero seat on table");
    let earlier = &seats[..hero_idx];
    let max_limpers = earlier.len().min(3);
    let count = match max_limpers {
        1 => 1,
        2 => pick_weighted(rng, &[(1usize, 55), (2, 45)]),
        _ => pick_weighted(rng, &[(1usize, 45), (2, 35), (3, 20)]),
    };

    // For BB facing a single limper, Q7 assumes a 2.5bb pot (an earlier seat limped,
    // not a lone SB complete which is a different 2.0bb blind-vs-blind spot).
    let mut pool: Vec<LimperSeat> = if seat == LimperSeat::Bb && count == 1 {
        earlier.iter().copied().filter(|&s| s != LimperSeat::Sb).collect()
    } else {
        earlier.to_vec()
    };
    rng.shuffle(&mut pool);
    pool.truncate(count);
    pool.sort_unstable();

    let row = table_row(seat.row_seat(), count >= 2);
    let target = match seat {
        LimperSeat::Bb => pick_weighted(rng, &[(LimperDecision::Raise, 25), (LimperDecision::Call, 75)]),
        LimperSeat::Sb => pick_weighted(
            rng,
            &[
                (LimperDecision::Raise, 22),
                (LimperDecision::Call, 58),
                (LimperDecision::Fold, 20),
            ],
        ),
        _ => pick_weighted(
            rng,
            &[
                (LimperDecision::Raise, 25),
                (LimperDecision::Call, 40),
                (LimperDecision::Fold, 35),
            ],
        ),
    };

    let mut weighted_classes: Vec<((u8, u8, bool), u64)> = Vec::with_capacity(169);
    for high in 0..13u8 {
        for low in 0..=high {
            if high == low {
                if row.decision_class(high, low, false) == target {
                    weighted_classes.push(((high, low, false), class_weight(row, high, low, false)));
                }
            } else {
                for suited in [true, false] {
                    if row.decision_class(high, low, suited) == target {
                        weighted_classes.push(((high, low, suited), class_weight(row, high, low, suited)));
                    }
                }
            }
        }
    }
    let (high, low, suited) = pick_weighted(rng, &weighted_classes);
    let hand = deal_class(rng, high, low, suited);

    LimpersQuestion { players, seat, limpers: pool, hand }
}

const PAIR_SUITS: [(u8, u8); 6] = [(0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3)];

fn deal_class(rng: &mut Rng, high: u8, low: u8, suited: bool) -> Holding {
    if high == low {
        let (s1, s2) = PAIR_SUITS[rng.below(6) as usize];
        [Card::new(high, s1), Card::new(low, s2)]
    } else if suited {
        let s = rng.below(4) as u8;
        [Card::new(high, s), Card::new(low, s)]
    } else {
        let s1 = rng.below(4) as u8;
        let s2 = (s1 + 1 + rng.below(3) as u8) % 4;
        [Card::new(high, s1), Card::new(low, s2)]
    }
}
