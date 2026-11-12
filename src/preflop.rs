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
    /// One raise in front of you and nobody has called it yet.
    Raised,
    /// A raise and this many callers (1 to 3) in front of you.
    RaisedCalled(u8),
    /// You opened and were 3-bet. In position means you did not open the small
    /// blind and the 3-bettor is in a blind.
    ThreeBet { in_position: bool },
    /// You 3-bet and were 4-bet.
    FourBet,
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
        (Action::Raise, Situation::Raised | Situation::RaisedCalled(_)) => "3-bet",
        (Action::Raise, Situation::ThreeBet { .. }) => "4-bet",
        (Action::Raise, Situation::FourBet) => "all in",
        (Action::Raise, _) => "raise",
        (Action::Fold, _) => "fold",
        (Action::Call, Situation::Raised | Situation::RaisedCalled(_) | Situation::ThreeBet { .. } | Situation::FourBet) => "call",
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
    /// From the cutoff or button, a limp-along hand that raises instead.
    IsoLateRaise,
    IsoLimp,
    IsoLimpNeedsLateOrTwoLimpers,
    NotInIso,
    BigBlindIsoRaise,
    BigBlindCheck,
    /// The answer came from one of the facing-action charts.
    Chart { chart: Chart, kind: HandKind },
}

/// The chart that answers a situation with action in front of you, or `None`
/// where the opening and limper lines answer instead.
pub fn chart_for(seat: Seat, situation: Situation) -> Option<Chart> {
    match situation {
        Situation::FoldedTo | Situation::Limpers(_) => None,
        Situation::Raised if seat == Seat::BigBlind => Some(Chart::BlindsBb),
        Situation::Raised if seat == Seat::SmallBlind => Some(Chart::Blinds),
        Situation::Raised if matches!(seat, Seat::Early | Seat::Middle) => Some(Chart::PositionEarly),
        Situation::Raised => Some(Chart::Position),
        Situation::RaisedCalled(_) if seat == Seat::BigBlind => Some(Chart::CallersBb),
        Situation::RaisedCalled(_) => Some(Chart::Callers),
        Situation::ThreeBet { in_position: true } => Some(Chart::ThreeBetIp),
        Situation::ThreeBet { in_position: false } => Some(Chart::ThreeBetOop),
        Situation::FourBet => Some(Chart::FourBet),
    }
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
            let late = matches!(seat, Seat::Cutoff | Seat::Button);
            if card.iso_raise.range.contains_holding(hand) {
                (Action::Raise, PreflopReason::IsoRaise)
            } else if late && card.iso_late_raise.range.contains_holding(hand) {
                (Action::Raise, PreflopReason::IsoLateRaise)
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
        Situation::Raised | Situation::RaisedCalled(_) | Situation::ThreeBet { .. } | Situation::FourBet => {
            let chart = chart_for(seat, situation).expect("this situation has a chart");
            (chart.action(hand), PreflopReason::Chart { chart, kind: HandKind::of(hand) })
        }
    }
}

/// A facing-action chart on the card: a raise list and a call list, fold the rest.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Chart {
    /// One raise, from the cutoff or button.
    Position,
    /// One raise, from an early or middle seat: a tighter raise list and a much tighter call list.
    PositionEarly,
    Blinds,
    /// One raise, closing the action from the big blind: an extra call line.
    BlindsBb,
    Callers,
    /// Callers, closing the action from the big blind: an extra call line.
    CallersBb,
    ThreeBetIp,
    ThreeBetOop,
    FourBet,
}

impl Chart {
    pub const ALL: [Chart; 9] = [
        Chart::Position,
        Chart::PositionEarly,
        Chart::Blinds,
        Chart::BlindsBb,
        Chart::Callers,
        Chart::CallersBb,
        Chart::ThreeBetIp,
        Chart::ThreeBetOop,
        Chart::FourBet,
    ];

    /// The raise line and the call line of this chart.
    pub fn lines(self) -> (&'static CardLine, &'static CardLine) {
        let card = &CARD;
        match self {
            Chart::Position => (&card.reraise, &card.call),
            Chart::PositionEarly => (&card.early_reraise, &card.early_call),
            Chart::Blinds | Chart::BlindsBb => (&card.blinds_reraise, &card.blinds_call),
            Chart::Callers | Chart::CallersBb => (&card.callers_reraise, &card.callers_call),
            Chart::ThreeBetIp => (&card.three_bet_ip_raise, &card.three_bet_ip_call),
            Chart::ThreeBetOop => (&card.three_bet_oop_raise, &card.three_bet_oop_call),
            Chart::FourBet => (&card.four_bet_raise, &card.four_bet_call),
        }
    }

    /// The extra call line some charts add to their call list.
    pub fn extra_call(self) -> Option<&'static CardLine> {
        match self {
            Chart::BlindsBb => Some(&CARD.blinds_bb_call),
            Chart::CallersBb => Some(&CARD.callers_bb_call),
            _ => None,
        }
    }

    /// The one action this chart gives a hand.
    pub fn action(self, hand: Holding) -> Action {
        let (raise, call) = self.lines();
        if raise.range.contains_holding(hand) {
            Action::Raise
        } else if call.range.contains_holding(hand) || self.extra_call().is_some_and(|l| l.range.contains_holding(hand)) {
            Action::Call
        } else {
            Action::Fold
        }
    }

    /// Title for the card, in plain words.
    pub fn title(self) -> &'static str {
        match self {
            Chart::Position => "One raise, cutoff or button",
            Chart::PositionEarly => "One raise, early or middle seat",
            Chart::Blinds => "One raise, from the blinds",
            Chart::BlindsBb => "One raise, from the big blind",
            Chart::Callers => "A raise and callers",
            Chart::CallersBb => "A raise and callers, big blind",
            Chart::ThreeBetIp => "You opened and got 3-bet, in position",
            Chart::ThreeBetOop => "You opened and got 3-bet, out of position",
            Chart::FourBet => "You 3-bet and got 4-bet",
        }
    }

    /// The word for this chart's raise line.
    pub fn raise_word(self) -> &'static str {
        match self {
            Chart::ThreeBetIp | Chart::ThreeBetOop => "4-bet",
            Chart::FourBet => "All in",
            _ => "3-bet",
        }
    }

    /// `3-bet JJ+ AQs+ AKo. Call 22 33. Fold the rest.`, the chart as a line. A chart
    /// with an empty call line, such as facing a 4-bet, has no call sentence.
    pub fn summary(self) -> String {
        let (raise, call) = self.lines();
        if call.text.is_empty() && self.extra_call().is_none() {
            return format!("{}. {} {}. Fold the rest.", self.title(), self.raise_word(), raise.text);
        }
        match self.extra_call() {
            Some(extra) => format!(
                "{}. {} {}. Call {}. Also call {} here. Fold the rest.",
                self.title(),
                self.raise_word(),
                raise.text,
                call.text,
                extra.text
            ),
            None => format!("{}. {} {}. Call {}. Fold the rest.", self.title(), self.raise_word(), raise.text, call.text),
        }
    }
}

/// What a hand is, for choosing the line that explains its action.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum HandKind {
    KingsOrAces,
    Queens,
    Jacks,
    Tens,
    MiddlePair,
    SmallPair,
    AceKing,
    AceQueen,
    SuitedAce,
    SuitedBroadway,
    SuitedConnector,
    OtherSuited,
    OffsuitBroadway,
    OffsuitAce,
    OtherOffsuit,
}

const TEN: u8 = 8;
const JACK: u8 = 9;
const QUEEN: u8 = 10;
const KING: u8 = 11;
const ACE: u8 = 12;
/// 7 is the lowest middle pair.
const SEVEN: u8 = 5;

impl HandKind {
    pub fn of(hand: Holding) -> HandKind {
        let (a, b) = (hand[0].rank(), hand[1].rank());
        let (high, low) = if a >= b { (a, b) } else { (b, a) };
        if high == low {
            return match high {
                ACE | KING => HandKind::KingsOrAces,
                QUEEN => HandKind::Queens,
                JACK => HandKind::Jacks,
                TEN => HandKind::Tens,
                r if r >= SEVEN => HandKind::MiddlePair,
                _ => HandKind::SmallPair,
            };
        }
        if high == ACE && low == KING {
            return HandKind::AceKing;
        }
        if high == ACE && low == QUEEN {
            return HandKind::AceQueen;
        }
        let suited = hand[0].suit() == hand[1].suit();
        match (high, suited) {
            (ACE, true) => HandKind::SuitedAce,
            (ACE, false) if low >= TEN => HandKind::OffsuitBroadway,
            (ACE, false) => HandKind::OffsuitAce,
            _ if low >= TEN && suited => HandKind::SuitedBroadway,
            _ if low >= TEN => HandKind::OffsuitBroadway,
            _ if suited && high - low == 1 => HandKind::SuitedConnector,
            _ if suited => HandKind::OtherSuited,
            _ => HandKind::OtherOffsuit,
        }
    }
}

/// One line on why a chart gives this answer for this kind of hand, written about
/// what the hand does in the spot. The tests check, per chart cell, computed facts about
/// the hands that reach the cell and some position words in the text. They do not parse
/// the sentence, so a reworded line needs its claims reviewed by hand.
pub fn why_line(chart: Chart, action: Action, kind: HandKind) -> &'static str {
    use Action::{Call, Fold, Raise};
    use Chart::{Blinds, BlindsBb, Callers, CallersBb, FourBet, Position, PositionEarly, ThreeBetIp, ThreeBetOop};
    use HandKind::*;
    match (chart, action, kind) {
        // Facing one raise. The same raises at every seat.
        (Position | PositionEarly | Blinds | BlindsBb, Raise, KingsOrAces) => {
            "Aces or kings are ahead of almost every hand that raises or calls, so 3-bet and build the pot now."
        }
        (Position | PositionEarly | Blinds | BlindsBb, Raise, Queens) => {
            "Queens are ahead of nearly every hand that raises or calls, so 3-bet and make weaker hands pay now."
        }
        (Position | PositionEarly | Blinds | BlindsBb, Raise, Jacks) => {
            "Jacks are ahead of most hands that raise or call, so 3-bet before overcards can come."
        }
        (Position | PositionEarly | Blinds | BlindsBb, Raise, AceKing) => {
            "Ace-king dominates the aces and kings that call, so 3-bet and build a pot you flop top pair in."
        }
        (Position | PositionEarly | Blinds | BlindsBb, Raise, AceQueen) => {
            "Ace-queen dominates the weaker aces that call, so 3-bet for value and make them pay."
        }
        // Only the cutoff, button and blinds 3-bet king-queen: a lone raise there usually comes from a wide late opener.
        (Position | Blinds | BlindsBb, Raise, SuitedBroadway | OffsuitBroadway) => {
            "King-queen dominates the weaker kings and queens a wide late opener plays, so 3-bet for value against players who call too much."
        }
        (Position | PositionEarly, Call, Tens) => {
            "Tens flop an overcard too often to build a big pot, so call and see the flop with position."
        }
        (Blinds | BlindsBb, Call, Tens) => "Tens flop an overcard too often to build a big pot, so call from the blinds.",
        (Position | PositionEarly, Call, MiddlePair | SmallPair) => {
            "A pair flops a set about one time in eight, and in position a set wins a big pot from players who call too much."
        }
        (Blinds | BlindsBb, Call, MiddlePair | SmallPair) => {
            "A pair flops a set about one time in eight, and with deep stacks a set wins a big pot even from the blinds."
        }
        (Position | PositionEarly, Call, AceQueen) => {
            "Ace-queen offsuit is strong enough to call in position, but it is behind the hands that 3-bet it."
        }
        (Position | PositionEarly, Call, SuitedAce) => {
            "A suited ace makes the nut flush draw, and position makes it easier to play."
        }
        (Blinds | BlindsBb, Call, SuitedAce) => "A suited ace makes the nut flush draw, even out of position.",
        (Position | PositionEarly, Call, SuitedBroadway) => {
            "Two suited high cards make strong pairs and big draws, and position lets you use them."
        }
        (Blinds | BlindsBb, Call, SuitedBroadway) => {
            "Two suited high cards make strong pairs and big draws, enough to play from the blinds."
        }
        (Position, Call, SuitedConnector) => {
            "Suited connectors make straights and flushes that get paid, and position lets you use them."
        }
        (Blinds | BlindsBb, Call, SuitedConnector) => {
            "Suited connectors make straights and flushes that get paid, and you already have money in the pot."
        }
        (Position, Call, OffsuitBroadway) => {
            "Ace-jack offsuit has the equity to call a raise, and position makes its top pairs easier to play."
        }
        (Blinds, Call, OffsuitBroadway) => "Ace-jack offsuit has the equity to call from the small blind, even out of position.",
        (BlindsBb, Call, OffsuitBroadway) => {
            "Closing the action, two offsuit cards ten or higher have the equity to call at the big blind's price."
        }
        (BlindsBb, Call, OtherSuited) => {
            "Closing the action, this suited hand has the equity to call at the big blind's price, and it can make a flush."
        }

        // Folds facing one raise where the same seat plays stronger hands of the same kind.
        (Position, Fold, OffsuitBroadway) => {
            "Offsuit high cards other than ace-jack and king-queen are often dominated by a raiser's better kicker, so their pairs are hard to play."
        }
        (Blinds, Fold, OffsuitBroadway) => {
            "From the small blind, offsuit high cards other than ace-jack and king-queen often run into a better kicker, and out of position they are hard to play."
        }
        (Blinds | BlindsBb, Fold, OffsuitAce) => {
            "A weak offsuit ace often runs into a better ace, and out of position the pairs it makes are hard to play."
        }
        (Blinds, Fold, SuitedConnector) => {
            "From the small blind the big blind can still raise behind you, so only the bigger suited connectors call out of position."
        }
        (BlindsBb, Fold, OtherSuited) => {
            "Even closing the action, this suited hand is too weak to call out of position: it rarely makes a strong hand, and its draws are easy to beat."
        }

        // An early seat folds hands the cutoff and button call: players are still to act behind you.
        (PositionEarly, Fold, SmallPair) => {
            "With players still to act behind you, a small pair needs a flopped set, and they can raise that chance away."
        }
        (PositionEarly, Fold, SuitedAce) => {
            "With players still to act behind you, a weak suited ace is dominated by the better aces that raise."
        }
        (PositionEarly, Fold, SuitedBroadway) => {
            "With players still to act behind you, these suited cards are dominated by the bigger hands that raise."
        }
        (PositionEarly, Fold, SuitedConnector) => {
            "With players still to act behind you, a suited connector is usually behind the raiser."
        }

        // Facing a raise and callers.

        (Callers | CallersBb, Raise, KingsOrAces) => {
            "Aces or kings are strong enough to 3-bet even with callers, and you want more money in while they are ahead."
        }
        (Callers | CallersBb, Raise, Queens) => {
            "Queens are strong enough to 3-bet with callers in, and a 3-bet thins the field while you are ahead."
        }
        (Callers | CallersBb, Raise, AceKing) => {
            "Ace-king makes top pair with the best kicker, so 3-bet and thin the field while it is ahead."
        }
        (Callers | CallersBb, Call, Jacks) => {
            "Jacks flop an overcard more often than not, so with callers in you call and see the flop."
        }
        (Callers | CallersBb, Call, Tens) => {
            "Tens flop an overcard more often than not, so with callers in you call and see the flop."
        }
        (Callers | CallersBb, Call, MiddlePair | SmallPair) => {
            "With callers in, a flopped set wins a bigger pot, and a pair flops one about one time in eight."
        }
        (Callers | CallersBb, Call, AceQueen) => {
            "Ace-queen is good with callers in, but a 3-bet risks running into ace-king, so call and see the flop."
        }
        (Callers | CallersBb, Call, SuitedAce) => {
            "With callers the pot is bigger, so the nut flush draw from a suited ace pays more."
        }
        (Callers | CallersBb, Call, SuitedBroadway) => {
            "Suited high cards make big hands that win a bigger pot with callers in."
        }
        (Callers | CallersBb, Call, SuitedConnector) => {
            "Suited connectors make straights and flushes that win a bigger pot with callers in."
        }
        (Callers, Call, OffsuitBroadway) => {
            "With callers in, the price is good enough for ace-jack and king-queen offsuit, though weaker offsuit hands are dominated."
        }
        (CallersBb, Call, OffsuitBroadway) => {
            "Two offsuit cards ten or higher have the equity to close the action at the big blind's price."
        }
        (Callers, Fold, OffsuitBroadway | OffsuitAce) => {
            "Below ace-jack and king-queen, offsuit high cards are dominated with this many hands in, so top pair is often second best."
        }
        (CallersBb, Fold, OffsuitAce) => {
            "A weak offsuit ace is dominated with this many hands in, so top pair is often second best."
        }

        // You opened and got 3-bet. Both charts continue only with kings or aces.
        (ThreeBetIp | ThreeBetOop, Raise, KingsOrAces) => {
            "Aces or kings are the only hands with enough equity to get the money in against a 3-bet, so 4-bet."
        }
        (ThreeBetIp | ThreeBetOop, Fold, Queens) => {
            "A 3-bet against your open is usually aces, kings or ace-king, and queens are behind that range, so fold."
        }
        (ThreeBetIp | ThreeBetOop, Fold, AceKing) => {
            "A 3-bet against your open is usually aces, kings or ace-king, and even ace-king is behind that range, so fold."
        }
        (ThreeBetIp | ThreeBetOop, Fold, Jacks | Tens | MiddlePair | SmallPair) => {
            "A 3-bet against your open is usually aces, kings or ace-king, and this pair is behind that range, so fold."
        }
        (ThreeBetIp | ThreeBetOop, Fold, AceQueen) => {
            "A 3-bet against your open is usually aces, kings or ace-king, and ace-queen is dominated by the ace-kings, so fold."
        }
        (ThreeBetIp | ThreeBetOop, Fold, _) => {
            "A 3-bet against your open is usually aces, kings or ace-king, and this hand is behind that range, so fold."
        }

        // You 3-bet and got 4-bet. Only kings or aces continue.
        (FourBet, Raise, KingsOrAces) => "Aces or kings are the hands you 3-bet for, so go all in now.",
        (FourBet, Fold, Queens) => {
            "Queens are behind the aces and kings that 4-bet here and only slightly ahead of ace-king, so fold."
        }
        (FourBet, Fold, AceKing) => "Ace-king is behind the big pairs that 4-bet here, so fold and wait for aces or kings.",
        (FourBet, Fold, Jacks) => "Jacks are behind the big pairs that 4-bet here and only slightly ahead of ace-king.",
        (FourBet, Fold, AceQueen) => "Ace-queen is dominated by the ace-kings that 4-bet here and behind the big pairs.",
        (FourBet, Fold, _) => "A 4-bet here is almost always a big pair or ace-king, and this hand is behind that.",

        // Shared folds for every other cell.
        (_, Fold, SuitedConnector) => {
            "A suited connector this small usually ends with one pair or less, and its straights and flushes are the lowest ones."
        }
        (_, Fold, OffsuitBroadway) => "Offsuit high cards usually end with one pair or less, and that is not enough to call a raise with.",
        (_, Fold, OffsuitAce) => "A weak offsuit ace usually ends with one pair or less, and that is not enough to call a raise with.",
        (_, Fold, OtherSuited) => "This suited hand usually ends with one pair or less, and that is not enough to call a raise with.",
        (_, Fold, OtherOffsuit) => "This offsuit hand usually ends with one pair or less, and that is not enough to call a raise with.",
        // The charts never reach these cells; a test checks that every reached cell has its own line.
        (_, Fold, _) => "This hand isn't strong enough to continue here.",
        (_, Call, _) => "This hand is strong enough to continue here.",
        (_, Raise, _) => "This hand is strong enough to raise here.",
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

    /// A range from card notation such as `TT+ AJs+ KQs`.
    pub fn parse(text: &str) -> Range {
        parse_range(text.as_bytes(), 0, text.len())
    }

    /// The hands in both ranges.
    pub fn intersect(&self, other: &Range) -> Range {
        Range { grid: core::array::from_fn(|row| self.grid[row] & other.grid[row]) }
    }

    /// The hands in this range and not in `other`.
    pub fn without(&self, other: &Range) -> Range {
        Range { grid: core::array::from_fn(|row| self.grid[row] & !other.grid[row]) }
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
    /// From the cutoff or button, limp-along hands that raise over limpers instead.
    pub iso_late_raise: CardLine,
    /// Facing one raise from the cutoff or button.
    pub reraise: CardLine,
    pub call: CardLine,
    /// Facing one raise from an early or middle seat.
    pub early_reraise: CardLine,
    pub early_call: CardLine,
    pub blinds_reraise: CardLine,
    pub blinds_call: CardLine,
    pub blinds_bb_call: CardLine,
    pub callers_reraise: CardLine,
    pub callers_call: CardLine,
    pub callers_bb_call: CardLine,
    pub three_bet_ip_raise: CardLine,
    pub three_bet_ip_call: CardLine,
    pub three_bet_oop_raise: CardLine,
    pub three_bet_oop_call: CardLine,
    pub four_bet_raise: CardLine,
    pub four_bet_call: CardLine,
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
        iso_late_raise: CardLine::EMPTY,
        reraise: CardLine::EMPTY,
        call: CardLine::EMPTY,
        early_reraise: CardLine::EMPTY,
        early_call: CardLine::EMPTY,
        blinds_reraise: CardLine::EMPTY,
        blinds_call: CardLine::EMPTY,
        blinds_bb_call: CardLine::EMPTY,
        callers_reraise: CardLine::EMPTY,
        callers_call: CardLine::EMPTY,
        callers_bb_call: CardLine::EMPTY,
        three_bet_ip_raise: CardLine::EMPTY,
        three_bet_ip_call: CardLine::EMPTY,
        three_bet_oop_raise: CardLine::EMPTY,
        three_bet_oop_call: CardLine::EMPTY,
        four_bet_raise: CardLine::EMPTY,
        four_bet_call: CardLine::EMPTY,
        notes: [CardNote { key: "", text: "" }; MAX_NOTES],
        note_count: 0,
    };
    let mut pos = 0;
    let mut seen = 0u32;
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
        } else if key_is(bytes, start, key_end, b"face_raise.blinds.reraise") {
            slot = 9;
            card.blinds_reraise = line;
        } else if key_is(bytes, start, key_end, b"face_raise.blinds.call") {
            slot = 10;
            card.blinds_call = line;
        } else if key_is(bytes, start, key_end, b"iso.late.raise") {
            slot = 23;
            card.iso_late_raise = line;
        } else if key_is(bytes, start, key_end, b"face_raise.early.reraise") {
            slot = 22;
            card.early_reraise = line;
        } else if key_is(bytes, start, key_end, b"face_raise.early.call") {
            slot = 19;
            card.early_call = line;
        } else if key_is(bytes, start, key_end, b"face_raise.big_blind.call") {
            slot = 21;
            card.blinds_bb_call = line;
        } else if key_is(bytes, start, key_end, b"face_callers.big_blind.call") {
            slot = 20;
            card.callers_bb_call = line;
        } else if key_is(bytes, start, key_end, b"face_callers.reraise") {
            slot = 11;
            card.callers_reraise = line;
        } else if key_is(bytes, start, key_end, b"face_callers.call") {
            slot = 12;
            card.callers_call = line;
        } else if key_is(bytes, start, key_end, b"three_bet.ip.fourbet") {
            slot = 13;
            card.three_bet_ip_raise = line;
        } else if key_is(bytes, start, key_end, b"three_bet.ip.call") {
            slot = 14;
            card.three_bet_ip_call = line;
        } else if key_is(bytes, start, key_end, b"three_bet.oop.fourbet") {
            slot = 15;
            card.three_bet_oop_raise = line;
        } else if key_is(bytes, start, key_end, b"three_bet.oop.call") {
            slot = 16;
            card.three_bet_oop_call = line;
        } else if key_is(bytes, start, key_end, b"four_bet.allin") {
            slot = 17;
            card.four_bet_raise = line;
        } else if key_is(bytes, start, key_end, b"four_bet.call") {
            slot = 18;
            card.four_bet_call = line;
        } else {
            panic!("unknown key in preflop card");
        }
        seen |= 1 << slot;
    }
    if seen != (1 << 24) - 1 {
        panic!("preflop card is missing a line");
    }
    card.header = sub_str(bytes, 0, header_end);
    card
}
