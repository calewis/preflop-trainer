//! The facing-action charts. Answers are checked against an independent reading
//! of the card text, the charts against each other and the cheaper spots, and
//! every reason line against values computed for the hands it can be shown on.

use std::collections::{BTreeSet, HashMap};

use poker_trainer::cards::{holding_mask, Card, Holding};
use poker_trainer::eval::{evaluate, Category};
use poker_trainer::family::{family_of, find_sibling};
use poker_trainer::generate::{drill_players, generate, generate_preflop};
use poker_trainer::grade::grade;
use poker_trainer::item::{Item, Question, Skill};
use poker_trainer::preflop::{
    action_label, chart_for, preflop_answer, why_line, Action, Chart, HandKind, Seat, Situation, CARD, CARD_TEXT,
};
use poker_trainer::rng::Rng;

const RANKS: &[u8] = b"23456789TJQKA";

/// Every hand class: its label and one holding of that class.
fn classes() -> Vec<(String, Holding)> {
    let mut out = Vec::new();
    for high in (0..13u8).rev() {
        for low in (0..=high).rev() {
            let (h, l) = (RANKS[high as usize] as char, RANKS[low as usize] as char);
            if high == low {
                out.push((format!("{h}{l}"), [Card::new(high, 0), Card::new(low, 1)]));
            } else {
                out.push((format!("{h}{l}s"), [Card::new(high, 0), Card::new(low, 0)]));
                out.push((format!("{h}{l}o"), [Card::new(high, 0), Card::new(low, 1)]));
            }
        }
    }
    assert_eq!(out.len(), 169);
    out
}

fn label_of(hand: Holding) -> String {
    let (a, b) = (hand[0].rank(), hand[1].rank());
    let (high, low) = if a >= b { (a, b) } else { (b, a) };
    let (h, l) = (RANKS[high as usize] as char, RANKS[low as usize] as char);
    if high == low {
        format!("{h}{l}")
    } else if hand[0].suit() == hand[1].suit() {
        format!("{h}{l}s")
    } else {
        format!("{h}{l}o")
    }
}

fn hand(text: &str) -> Holding {
    poker_trainer::cards::parse_holding(text).unwrap()
}

fn rank_index(letter: u8) -> u8 {
    RANKS.iter().position(|r| *r == letter).expect("a rank") as u8
}

/// Expands card notation into class labels, without using the card's own parser.
fn expand(text: &str) -> BTreeSet<String> {
    let mut labels = BTreeSet::new();
    for token in text.split_whitespace() {
        let plus = token.ends_with('+');
        let body = token.trim_end_matches('+').as_bytes();
        let (first, second) = (rank_index(body[0]), rank_index(body[1]));
        let top_second = if !plus { second } else if first == second { 12 } else { first - 1 };
        for rank in second..=top_second {
            let letter = RANKS[rank as usize] as char;
            let label = if first == second {
                format!("{letter}{letter}")
            } else {
                format!("{}{letter}{}", body[0] as char, body[2] as char)
            };
            assert!(labels.insert(label.clone()), "{label} is listed twice in {text}");
        }
    }
    labels
}

/// The right-hand side of a card key, read straight from the card text.
fn card_value(key: &str) -> &'static str {
    let line = CARD_TEXT
        .lines()
        .find(|line| line.split('=').next().map(str::trim) == Some(key))
        .unwrap_or_else(|| panic!("{key} is not on the card"));
    line.split_once('=').unwrap().1.trim()
}

fn card_line(key: &str) -> BTreeSet<String> {
    expand(card_value(key))
}

fn seat_key(seat: Seat) -> &'static str {
    match seat {
        Seat::Early => "early",
        Seat::Middle => "middle",
        Seat::Cutoff => "cutoff",
        Seat::Button => "button",
        Seat::SmallBlind => "small_blind",
        Seat::BigBlind => panic!("the big blind has no open line"),
    }
}

/// The answer as the card text states it, written separately from the library.
fn card_text_answer(seat: Seat, situation: Situation, label: &str) -> Action {
    let raise_or = |raise: &str, call: &[&str]| -> Action {
        if card_line(raise).contains(label) {
            Action::Raise
        } else if call.iter().any(|key| card_line(key).contains(label)) {
            Action::Call
        } else {
            Action::Fold
        }
    };
    match situation {
        Situation::FoldedTo if seat == Seat::BigBlind => Action::Call,
        Situation::FoldedTo => raise_or(&format!("open.{}", seat_key(seat)), &[]),
        Situation::Limpers(n) => {
            let raises = card_line("iso.raise").contains(label);
            let limps = card_line("iso.limp").contains(label);
            let late_raises = card_line("iso.late.raise").contains(label);
            if seat == Seat::BigBlind {
                if raises { Action::Raise } else { Action::Call }
            } else if raises || (late_raises && matches!(seat, Seat::Cutoff | Seat::Button)) {
                Action::Raise
            } else if limps && (n >= 2 || matches!(seat, Seat::Cutoff | Seat::Button | Seat::SmallBlind)) {
                Action::Call
            } else {
                Action::Fold
            }
        }
        Situation::Raised => match seat {
            Seat::Early | Seat::Middle => raise_or("face_raise.early.reraise", &["face_raise.early.call"]),
            Seat::Cutoff | Seat::Button => raise_or("face_raise.reraise", &["face_raise.call"]),
            Seat::SmallBlind => raise_or("face_raise.blinds.reraise", &["face_raise.blinds.call"]),
            Seat::BigBlind => raise_or("face_raise.blinds.reraise", &["face_raise.blinds.call", "face_raise.big_blind.call"]),
        },
        Situation::RaisedCalled(_) if seat == Seat::BigBlind => {
            raise_or("face_callers.reraise", &["face_callers.call", "face_callers.big_blind.call"])
        }
        Situation::RaisedCalled(_) => raise_or("face_callers.reraise", &["face_callers.call"]),
        Situation::ThreeBet { in_position: true } => raise_or("three_bet.ip.fourbet", &["three_bet.ip.call"]),
        Situation::ThreeBet { in_position: false } => raise_or("three_bet.oop.fourbet", &["three_bet.oop.call"]),
        Situation::FourBet => raise_or("four_bet.allin", &["four_bet.call"]),
    }
}

fn every_spot() -> Vec<(Seat, Situation)> {
    let mut out = Vec::new();
    for seat in Seat::ALL {
        for situation in [
            Situation::FoldedTo,
            Situation::Limpers(1),
            Situation::Limpers(2),
            Situation::Limpers(3),
            Situation::Raised,
            Situation::RaisedCalled(1),
            Situation::RaisedCalled(2),
            Situation::RaisedCalled(3),
            Situation::ThreeBet { in_position: true },
            Situation::ThreeBet { in_position: false },
            Situation::FourBet,
        ] {
            out.push((seat, situation));
        }
    }
    out
}

/// Every combo of a class label.
fn combos_of(label: &str) -> Vec<Holding> {
    let bytes = label.as_bytes();
    let (first, second) = (rank_index(bytes[0]), rank_index(bytes[1]));
    let mut out = Vec::new();
    for a in 0..4u8 {
        for b in 0..4u8 {
            if first == second {
                if a < b {
                    out.push([Card::new(first, a), Card::new(second, b)]);
                }
            } else if (bytes[2] == b's') == (a == b) {
                out.push([Card::new(first, a), Card::new(second, b)]);
            }
        }
    }
    out
}

#[test]
fn every_hand_matches_the_card_text_in_every_situation() {
    let mut combos_checked = 0;
    for (seat, situation) in every_spot() {
        for (label, _) in classes() {
            let expected = card_text_answer(seat, situation, &label);
            for combo in combos_of(&label) {
                assert_eq!(preflop_answer(seat, situation, combo).0, expected, "{seat:?} {situation:?} {label}");
                combos_checked += 1;
            }
        }
    }
    assert_eq!(combos_checked, 1326 * 66);
}

#[test]
fn no_card_line_lists_a_hand_twice_or_on_two_lines_of_a_chart() {
    let charts: [(&str, &[&str]); 9] = [
        ("iso.raise", &["iso.limp"]),
        ("iso.raise", &["iso.late.raise"]),
        ("face_raise.reraise", &["face_raise.call"]),
        ("face_raise.early.reraise", &["face_raise.early.call"]),
        ("face_raise.blinds.reraise", &["face_raise.blinds.call", "face_raise.big_blind.call"]),
        ("face_callers.reraise", &["face_callers.call", "face_callers.big_blind.call"]),
        ("three_bet.ip.fourbet", &["three_bet.ip.call"]),
        ("three_bet.oop.fourbet", &["three_bet.oop.call"]),
        ("four_bet.allin", &["four_bet.call"]),
    ];
    for (raise_key, call_keys) in charts {
        let mut seen = card_line(raise_key);
        for key in call_keys {
            for label in card_line(key) {
                assert!(seen.insert(label.clone()), "{label} is on two lines of the {raise_key} chart");
            }
        }
    }
}

#[test]
fn hand_written_spots_get_the_right_answer() {
    use Action::{Call, Fold, Raise};
    let cases = [
        (Seat::Early, Situation::Raised, "JhJd", Raise),
        (Seat::Early, Situation::Raised, "9h9d", Call),
        (Seat::Early, Situation::Raised, "6h6d", Fold),
        (Seat::Middle, Situation::Raised, "Ah5h", Fold),
        (Seat::Middle, Situation::Raised, "6h5h", Fold),
        (Seat::Middle, Situation::Raised, "KhTh", Fold),
        (Seat::Cutoff, Situation::Raised, "Ah5h", Call),
        (Seat::Button, Situation::Raised, "6h5h", Call),
        (Seat::Middle, Situation::Raised, "KhQd", Fold),
        (Seat::Middle, Situation::Raised, "AhQd", Call),
        (Seat::Button, Situation::Raised, "KhQd", Raise),
        (Seat::Button, Situation::Raised, "AhQd", Raise),
        (Seat::Button, Situation::Raised, "KhQh", Raise),
        (Seat::Button, Situation::Raised, "AhJd", Call),
        (Seat::Button, Situation::Raised, "AhTd", Fold),
        (Seat::Button, Situation::Raised, "5h4h", Fold),
        (Seat::SmallBlind, Situation::Raised, "8h7h", Call),
        (Seat::SmallBlind, Situation::Raised, "7h6h", Fold),
        (Seat::BigBlind, Situation::Raised, "7h6h", Call),
        (Seat::BigBlind, Situation::Raised, "Kh2h", Call),
        (Seat::BigBlind, Situation::Raised, "Qh4h", Fold),
        (Seat::BigBlind, Situation::Raised, "4h3h", Fold),
        (Seat::BigBlind, Situation::Raised, "AhJd", Call),
        (Seat::BigBlind, Situation::Raised, "KhQd", Raise),
        (Seat::BigBlind, Situation::Raised, "JhTd", Call),
        (Seat::BigBlind, Situation::Raised, "Ah9d", Fold),
        (Seat::BigBlind, Situation::Raised, "Kh9d", Fold),
        (Seat::SmallBlind, Situation::Raised, "AhJd", Call),
        (Seat::SmallBlind, Situation::Raised, "KhJd", Fold),
        (Seat::BigBlind, Situation::Raised, "KhJd", Call),
        (Seat::Cutoff, Situation::RaisedCalled(1), "AhQd", Call),
        (Seat::Cutoff, Situation::RaisedCalled(2), "AhJd", Call),
        (Seat::Cutoff, Situation::RaisedCalled(2), "KhJd", Fold),
        (Seat::BigBlind, Situation::RaisedCalled(1), "AhJd", Call),
        (Seat::BigBlind, Situation::RaisedCalled(1), "KhQd", Call),
        (Seat::BigBlind, Situation::RaisedCalled(2), "KhJd", Call),
        (Seat::BigBlind, Situation::RaisedCalled(2), "Ah9d", Fold),
        (Seat::Button, Situation::Limpers(1), "Th9h", Raise),
        (Seat::Cutoff, Situation::Limpers(2), "Ah5h", Raise),
        (Seat::Button, Situation::Limpers(1), "7h6h", Call),
        (Seat::Button, Situation::Limpers(1), "5h5d", Call),
        (Seat::SmallBlind, Situation::Limpers(1), "Th9h", Call),
        (Seat::Middle, Situation::Limpers(1), "Th9h", Fold),
        (Seat::Middle, Situation::Limpers(2), "Th9h", Call),
        (Seat::BigBlind, Situation::RaisedCalled(1), "JhJd", Call),
        (Seat::Cutoff, Situation::ThreeBet { in_position: true }, "AhJh", Fold),
        (Seat::Cutoff, Situation::ThreeBet { in_position: false }, "AhJh", Fold),
        (Seat::SmallBlind, Situation::ThreeBet { in_position: false }, "5h5d", Fold),
        (Seat::Button, Situation::ThreeBet { in_position: true }, "QhQd", Fold),
        (Seat::Button, Situation::ThreeBet { in_position: true }, "AhKh", Fold),
        (Seat::Early, Situation::ThreeBet { in_position: false }, "AhKd", Fold),
        (Seat::Early, Situation::ThreeBet { in_position: false }, "KhKd", Raise),
        (Seat::Early, Situation::FourBet, "AhKh", Fold),
        (Seat::Early, Situation::FourBet, "AhKd", Fold),
        (Seat::Early, Situation::FourBet, "QhQd", Fold),
        (Seat::Early, Situation::FourBet, "KhKd", Raise),
    ];
    for (seat, situation, text, expected) in cases {
        assert_eq!(preflop_answer(seat, situation, hand(text)).0, expected, "{seat:?} {situation:?} {text}");
    }
}

/// Six spots from the player's own games, where the card was too tight. Seats follow
/// the table size: at 4 players the big blind has 0 behind, the button 2 and the small blind 1.
#[test]
fn the_players_disputed_spots_get_the_revised_answers() {
    use Action::{Call, Raise};
    let cases = [
        // 4 players, big blind, one raise in front.
        (4, 0, Situation::Raised, "KhJd", Call),
        // 6 players, big blind: a call, not a 3-bet, so the pot stays small out of position.
        (6, 0, Situation::Raised, "7c6c", Call),
        // 4 players, button: king-queen 3-bets a wide late opener for value.
        (4, 2, Situation::Raised, "KhQd", Raise),
        (4, 0, Situation::Raised, "AhTd", Call),
        // 4 players, small blind: the same value 3-bet as the button.
        (4, 1, Situation::Raised, "KhQd", Raise),
        // 4 players, button, one limper: raise instead of limping along.
        (4, 2, Situation::Limpers(1), "Th9h", Raise),
    ];
    for (players, behind, situation, text, expected) in cases {
        let item = Item { skill: Skill::Preflop, seed: 1, question: Question::Preflop { players, behind, situation, hand: hand(text) } };
        let seat = Seat::from_behind(behind);
        assert_eq!(preflop_answer(seat, situation, hand(text)).0, expected, "{players} players {seat:?} {situation:?} {text}");
        assert!(grade(&item, expected.code()).expect("a valid answer").correct, "{players} players {seat:?} {text}");
    }
    // The reasons the player saw are gone from these spots.
    let old_76 = "A small suited connector usually ends with one pair or less, and that is not enough to call a raise with.";
    let old_kqo = "Out of position, an offsuit hand like this usually ends with one pair or less, and that is hard to play against a raise.";
    for chart in Chart::ALL {
        for (_, holding) in classes() {
            let line = why_line(chart, chart.action(holding), HandKind::of(holding));
            assert!(line != old_76 && line != old_kqo, "{chart:?}: {line}");
        }
    }
}

/// Wherever king-queen 3-bets, every stronger broadway hand 3-bets too.
#[test]
fn three_bets_are_linear_where_king_queen_three_bets() {
    let mut charts_with_kqo = 0;
    for chart in Chart::ALL {
        if chart.action(hand("KhQd")) != Action::Raise {
            continue;
        }
        charts_with_kqo += 1;
        for stronger in ["KhQh", "AhQd", "AhQh", "AhKd", "AhKh", "JhJd", "QhQd", "KhKd", "AhAd"] {
            assert_eq!(chart.action(hand(stronger)), Action::Raise, "{chart:?}: KQo 3-bets but {stronger} does not");
        }
    }
    assert_eq!(charts_with_kqo, 3, "the cutoff and button, the small blind and the big blind");
    assert_eq!(Chart::PositionEarly.action(hand("KhQd")), Action::Fold, "early seats keep the tight 3-bets");
    assert_eq!(Chart::PositionEarly.action(hand("AhQd")), Action::Call);
}

/// The cutoff and button raise over limpers only with hands they would open.
#[test]
fn late_raises_over_limpers_are_hands_the_seat_opens() {
    for seat in [Seat::Cutoff, Seat::Button] {
        for (label, holding) in classes() {
            if preflop_answer(seat, Situation::Limpers(1), holding).0 == Action::Raise {
                assert_eq!(preflop_answer(seat, Situation::FoldedTo, holding).0, Action::Raise, "{seat:?} {label}");
            }
        }
    }
    // Every late raise is also a limp-along hand, so the hands dealt with limpers are unchanged.
    for label in card_line("iso.late.raise") {
        assert!(card_line("iso.limp").contains(&label), "{label}");
    }
    // Small pairs and the weakest suited hands still limp along.
    for text in ["2h2d", "6h6d", "7h6h", "6h5h", "5h4h"] {
        assert_eq!(preflop_answer(Seat::Button, Situation::Limpers(1), hand(text)).0, Action::Call, "{text}");
    }
}

/// A fold line that dismisses a whole kind of hand can't sit in a chart that plays some of that kind.
#[test]
fn fold_lines_never_argue_against_hands_the_chart_plays() {
    let dismissals = ["not enough to call a raise", "hard to play against a raise"];
    for chart in Chart::ALL {
        for (label, holding) in classes() {
            if chart.action(holding) != Action::Fold {
                continue;
            }
            let kind = HandKind::of(holding);
            let line = why_line(chart, Action::Fold, kind);
            if !dismissals.iter().any(|d| line.contains(d)) {
                continue;
            }
            let played: Vec<String> = classes()
                .into_iter()
                .filter(|(_, h)| HandKind::of(*h) == kind && chart.action(*h) != Action::Fold)
                .map(|(l, _)| l)
                .collect();
            assert!(played.is_empty(), "{chart:?} folds {label} with {line:?} but plays {played:?}");
        }
    }
}

/// The big blind defends wider than the small blind, which defends no wider than the cutoff and button.
#[test]
fn the_big_blind_defends_wider_than_the_small_blind() {
    let continues = |chart: Chart| classes().into_iter().filter(|(_, h)| chart.action(*h) != Action::Fold).count();
    assert!(continues(Chart::BlindsBb) > continues(Chart::Blinds) + 20);
    for (label, holding) in classes() {
        if Chart::Blinds.action(holding) != Action::Fold {
            assert_ne!(Chart::BlindsBb.action(holding), Action::Fold, "{label}");
        }
    }
}

#[test]
fn an_early_seat_never_calls_a_raise_with_what_it_folds_cheaper() {
    for seat in Seat::ALL {
        for (label, holding) in classes() {
            if preflop_answer(seat, Situation::Raised, holding).0 == Action::Fold {
                continue;
            }
            for cheaper in [Situation::FoldedTo, Situation::Limpers(1)] {
                assert_ne!(
                    preflop_answer(seat, cheaper, holding).0,
                    Action::Fold,
                    "{seat:?} plays {label} against a raise but folds it at {cheaper:?}"
                );
            }
        }
    }
}

#[test]
fn a_call_with_callers_in_is_also_played_with_two_limpers() {
    for seat in Seat::ALL {
        for (label, holding) in classes() {
            if preflop_answer(seat, Situation::RaisedCalled(1), holding).0 != Action::Fold {
                assert_ne!(preflop_answer(seat, Situation::Limpers(2), holding).0, Action::Fold, "{seat:?} {label}");
            }
        }
    }
}

fn level(chart: Chart, holding: Holding) -> u8 {
    match chart.action(holding) {
        Action::Raise => 2,
        Action::Call => 1,
        Action::Fold => 0,
    }
}

#[test]
fn charts_have_no_holes() {
    for chart in Chart::ALL {
        for high in 0..13u8 {
            for low in 0..high {
                let suited = [Card::new(high, 0), Card::new(low, 0)];
                let offsuit = [Card::new(high, 0), Card::new(low, 1)];
                assert!(level(chart, suited) >= level(chart, offsuit), "{chart:?} {high} {low}");
                if low + 1 < high {
                    let better_s = [Card::new(high, 0), Card::new(low + 1, 0)];
                    let better_o = [Card::new(high, 0), Card::new(low + 1, 1)];
                    assert!(level(chart, better_s) >= level(chart, suited), "{chart:?} suited {high} {low}");
                    assert!(level(chart, better_o) >= level(chart, offsuit), "{chart:?} offsuit {high} {low}");
                }
            }
            if high > 0 {
                let pair = [Card::new(high, 0), Card::new(high, 1)];
                let lower = [Card::new(high - 1, 0), Card::new(high - 1, 1)];
                assert!(level(chart, pair) >= level(chart, lower), "{chart:?} pairs {high}");
            }
        }
    }
}

/// Continuing in the first chart of a pair means continuing in the second, which is cheaper or better.
#[test]
fn looser_spots_continue_with_every_hand_the_tighter_spots_continue_with() {
    let pairs = [
        (Chart::PositionEarly, Chart::Position),
        (Chart::Blinds, Chart::Position),
        (Chart::Blinds, Chart::BlindsBb),
        (Chart::Position, Chart::Callers),
        (Chart::Callers, Chart::CallersBb),
        (Chart::ThreeBetOop, Chart::ThreeBetIp),
    ];
    for (tighter, looser) in pairs {
        for (label, holding) in classes() {
            if tighter.action(holding) != Action::Fold {
                assert_ne!(looser.action(holding), Action::Fold, "{label}: {tighter:?} continues, {looser:?} folds");
            }
        }
    }
}

#[test]
fn the_callers_chart_keeps_only_the_big_three_bets() {
    let (raise_after, _) = Chart::Position.lines();
    let (raise_multi, call_multi) = Chart::Callers.lines();
    assert!(raise_multi.range.combo_count() < raise_after.range.combo_count(), "thin 3-bets disappear");
    for hole in ["5h4h", "JcTc", "2d2c", "AdQs"] {
        assert!(call_multi.range.contains_holding(hand(hole)), "{hole}");
    }
    // Ace-jack and king-queen offsuit call anywhere, since the cutoff and button play them
    // against a lone raise; weaker offsuit high cards call only when closing from the big blind.
    for hole in ["AdJs", "KdQs"] {
        assert!(call_multi.range.contains_holding(hand(hole)), "{hole}");
    }
    for hole in ["AdTs", "KdJs", "QdTs", "JdTs"] {
        assert!(!call_multi.range.contains_holding(hand(hole)), "{hole}");
        assert_eq!(Chart::CallersBb.action(hand(hole)), Action::Call, "{hole}");
    }
}

/// The player's rule: we 3-bet, but against a 3-bet or a 4-bet only kings or aces continue.
#[test]
fn only_kings_or_aces_continue_against_a_three_bet_or_four_bet() {
    for chart in [Chart::ThreeBetIp, Chart::ThreeBetOop, Chart::FourBet] {
        let (raise, call) = chart.lines();
        assert_eq!(raise.text, "KK+", "{chart:?}");
        assert_eq!(call.range.combo_count(), 0, "{chart:?} calls nothing");
        assert!(chart.summary().ends_with(". Fold the rest."), "{}", chart.summary());
        assert!(!chart.summary().contains("Call"), "{}", chart.summary());
    }
}

/// The ranges reason lines talk about, read from the card or the model in the notes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum V {
    Raises,
    Calls,
    RaisesCalls,
    ThreeBets,
    FourBets,
    BigPairs,
    /// Aces, kings or ace-king: what a 3-bet against your open usually is in these games.
    Premiums,
    AceKing,
    CutoffOpens,
    ButtonOpens,
}

fn villain_labels(v: V) -> BTreeSet<String> {
    match v {
        V::Raises => card_line("open.middle"),
        V::Calls => card_line("face_raise.call"),
        V::RaisesCalls => card_line("open.middle").union(&card_line("face_raise.call")).cloned().collect(),
        V::ThreeBets => card_line("face_raise.reraise"),
        V::FourBets => expand("QQ+ AKs AKo"),
        V::BigPairs => expand("KK+"),
        V::Premiums => expand("KK+ AKs AKo"),
        V::AceKing => expand("AKs AKo"),
        V::CutoffOpens => card_line("open.cutoff"),
        V::ButtonOpens => card_line("open.button"),
    }
}

#[derive(Clone, Copy, Debug)]
enum Check {
    /// Hero is ahead of at least this share of the range's combos.
    Ahead(V, f64),
    /// Hero's average equity against the range is under 50%.
    Behind(V),
    /// Hero's average equity reaches the price the line relies on.
    Equity(V, f64),
    /// Hero dominates at least this share of the range's unpaired combos.
    Dominates(V, f64),
    /// At least this share of the range's unpaired combos dominate hero, and hero is behind on average.
    DominatedBy(V, f64),
    Set,
    OftenOvercard,
    SlightlyAheadOfAceKing,
    ThreeWayPrice,
    /// Equity against early, middle and late opens clears the big blind's price.
    BigBlindPrice,
    /// By the river the hand usually has one pair or less.
    UsuallyOnePairOrLess,
    /// Hero's average equity against each range reaches this price, in percent.
    PriceAgainst(f64, &'static [V]),
    /// At least this share of the range's unpaired combos dominate hero.
    Kicker(V, f64),
    /// The chart plays a hand of the same kind with a higher top card.
    BiggerOfKindPlays,
    /// Calling a 3bb raise with one caller in: 3 to call into 7.5.
    CallersPrice,
}

/// Which computed checks a reason cell needs. A cell the charts reach must be listed,
/// so a new reason line cannot appear without one. An empty list means the line makes no
/// claim about equity or price; its position words are still checked.
fn checks(chart: Chart, action: Action, kind: HandKind) -> &'static [Check] {
    use Action::{Call, Fold, Raise};
    use Chart::{Blinds, BlindsBb, Callers, CallersBb, FourBet, Position, PositionEarly, ThreeBetIp, ThreeBetOop};
    use Check::*;
    use HandKind::*;
    match (chart, action, kind) {
        (Position | PositionEarly | Blinds | BlindsBb, Raise, KingsOrAces) => &[Ahead(V::RaisesCalls, 0.9)],
        (Position | PositionEarly | Blinds | BlindsBb, Raise, Queens) => &[Ahead(V::RaisesCalls, 0.85)],
        (Position | PositionEarly | Blinds | BlindsBb, Raise, Jacks) => &[Ahead(V::RaisesCalls, 0.6), OftenOvercard],
        (Position | PositionEarly | Blinds | BlindsBb, Raise, AceKing) => &[Dominates(V::Calls, 0.4)],
        (Position | PositionEarly | Blinds | BlindsBb, Raise, AceQueen) => &[Dominates(V::Calls, 0.3)],
        (Position | Blinds | BlindsBb, Raise, SuitedBroadway | OffsuitBroadway) => {
            &[Dominates(V::CutoffOpens, 0.1), Dominates(V::ButtonOpens, 0.1)]
        }
        // On the button facing 3bb: 3 to call into 4.5. In the small blind: 2.5 into 4.5.
        (Position, Call, OffsuitBroadway) => &[PriceAgainst(40.0, &[V::Raises, V::CutoffOpens])],
        (Blinds, Call, OffsuitBroadway) => &[PriceAgainst(250.0 / 7.0, &[V::CutoffOpens, V::ButtonOpens])],
        (BlindsBb, Call, OtherSuited) => &[BigBlindPrice],
        (Position, Fold, OffsuitBroadway) => &[Kicker(V::Raises, 0.1), Kicker(V::CutoffOpens, 0.1)],
        (Blinds | BlindsBb, Fold, OffsuitBroadway | OffsuitAce) => &[Kicker(V::CutoffOpens, 0.1), Kicker(V::ButtonOpens, 0.1)],
        (Position | Blinds | BlindsBb | Callers | CallersBb, Fold, SuitedConnector) => &[UsuallyOnePairOrLess, BiggerOfKindPlays],
        (Position | PositionEarly | Blinds | BlindsBb, Call, Tens) => &[OftenOvercard],
        (Position | PositionEarly | Blinds | BlindsBb, Call, MiddlePair | SmallPair) => &[Set],
        (Position | PositionEarly | Blinds | BlindsBb, Call, AceQueen) => &[Equity(V::Raises, 40.0), Behind(V::ThreeBets)],
        (Position | PositionEarly | Blinds | BlindsBb, Call, SuitedAce | SuitedBroadway | SuitedConnector) => &[],
        (PositionEarly, Fold, SmallPair) => &[Set],
        (PositionEarly, Fold, SuitedAce | SuitedBroadway) => &[DominatedBy(V::Raises, 0.1)],
        (PositionEarly, Fold, SuitedConnector) => &[Behind(V::Raises)],

        (Callers | CallersBb, Raise, KingsOrAces) => &[Ahead(V::RaisesCalls, 0.9)],
        (Callers | CallersBb, Raise, Queens) => &[Ahead(V::RaisesCalls, 0.85)],
        (Callers | CallersBb, Raise, AceKing) => &[Ahead(V::RaisesCalls, 0.5)],
        (Callers | CallersBb, Call, Jacks | Tens) => &[OftenOvercard],
        (Callers | CallersBb, Call, MiddlePair | SmallPair) => &[Set],
        (Callers | CallersBb, Call, AceQueen) => &[Behind(V::AceKing)],
        (Callers | CallersBb, Call, SuitedAce | SuitedBroadway | SuitedConnector) => &[],
        (CallersBb, Call, OffsuitBroadway) => &[ThreeWayPrice],
        (Callers, Call, OffsuitBroadway) => &[CallersPrice],
        (Callers | CallersBb, Fold, OffsuitBroadway | OffsuitAce) => &[DominatedBy(V::RaisesCalls, 0.1)],

        (ThreeBetIp | ThreeBetOop, Raise, KingsOrAces) => &[Equity(V::Premiums, 41.0)],
        (ThreeBetIp | ThreeBetOop, Fold, AceQueen) => &[DominatedBy(V::Premiums, 0.9)],
        (ThreeBetIp | ThreeBetOop, Fold, _) => &[Behind(V::Premiums)],

        (FourBet, Raise, KingsOrAces) => &[Equity(V::FourBets, 41.0)],
        (FourBet, Fold, Queens | Jacks) => &[Behind(V::BigPairs), SlightlyAheadOfAceKing],
        (FourBet, Fold, AceKing) => &[Behind(V::BigPairs)],
        (FourBet, Fold, AceQueen) => &[DominatedBy(V::AceKing, 0.9), Behind(V::BigPairs)],
        (FourBet, Fold, _) => &[Behind(V::FourBets)],

        (BlindsBb, Call, OffsuitBroadway) => &[BigBlindPrice],
        (_, Fold, OtherSuited | OtherOffsuit | OffsuitBroadway | OffsuitAce) => &[UsuallyOnePairOrLess],
        cell => panic!("reason cell {cell:?} has no checks: add them when adding a line"),
    }
}

struct Lab {
    rng: Rng,
    average: HashMap<(String, V), f64>,
}

impl Lab {
    fn new() -> Lab {
        Lab { rng: Rng::new(0x5EED_CAFE), average: HashMap::new() }
    }

    fn villains(hero: Holding, v: V) -> Vec<Holding> {
        let mask = holding_mask(&hero);
        villain_labels(v).iter().flat_map(|l| combos_of(l)).filter(|c| holding_mask(c) & mask == 0).collect()
    }

    /// Hero's equity against opponents drawn from `villains`, each list one opponent.
    fn play(&mut self, hero: Holding, seats: &[&[Holding]], deals: u32) -> f64 {
        let mut total = 0.0;
        let mut counted = 0u32;
        'deals: for _ in 0..deals {
            let mut used = holding_mask(&hero);
            let mut opponents = Vec::with_capacity(seats.len());
            for seat in seats {
                let villain = seat[self.rng.below(seat.len() as u64) as usize];
                if holding_mask(&villain) & used != 0 {
                    continue 'deals;
                }
                used |= holding_mask(&villain);
                opponents.push(villain);
            }
            let mut board = Vec::with_capacity(5);
            while board.len() < 5 {
                let card = Card::from_index(self.rng.below(52) as u8);
                if used & card.mask() == 0 {
                    used |= card.mask();
                    board.push(card);
                }
            }
            let rank = |h: Holding| evaluate(&[h[0], h[1], board[0], board[1], board[2], board[3], board[4]]);
            let mine = rank(hero);
            let others: Vec<_> = opponents.iter().map(|v| rank(*v)).collect();
            counted += 1;
            if others.iter().all(|o| *o <= mine) {
                total += 1.0 / (1.0 + others.iter().filter(|o| **o == mine).count() as f64);
            }
        }
        100.0 * total / f64::from(counted)
    }

    fn equity(&mut self, label: &str, hero: Holding, v: V) -> f64 {
        if let Some(found) = self.average.get(&(label.to_string(), v)) {
            return *found;
        }
        let villains = Lab::villains(hero, v);
        let got = self.play(hero, &[&villains], 40_000);
        self.average.insert((label.to_string(), v), got);
        got
    }

    /// How often hero's final hand by the river is one pair or less.
    fn one_pair_or_less(&mut self, hero: Holding) -> f64 {
        let deals = 20_000u32;
        let mut count = 0u32;
        for _ in 0..deals {
            let mut used = holding_mask(&hero);
            let mut board = Vec::with_capacity(5);
            while board.len() < 5 {
                let card = Card::from_index(self.rng.below(52) as u8);
                if used & card.mask() == 0 {
                    used |= card.mask();
                    board.push(card);
                }
            }
            let rank = evaluate(&[hero[0], hero[1], board[0], board[1], board[2], board[3], board[4]]);
            count += u32::from(matches!(rank.category(), Category::HighCard | Category::OnePair));
        }
        f64::from(count) / f64::from(deals)
    }

    /// The share of the range's combos hero has over 50% against.
    fn ahead_share(&mut self, hero: Holding, v: V) -> f64 {
        let mask = holding_mask(&hero);
        let (mut ahead, mut total) = (0.0, 0.0);
        for label in villain_labels(v) {
            let combos: Vec<Holding> = combos_of(&label).into_iter().filter(|c| holding_mask(c) & mask == 0).collect();
            if combos.is_empty() {
                continue;
            }
            total += combos.len() as f64;
            if self.play(hero, &[&combos], 4_000) > 50.0 {
                ahead += combos.len() as f64;
            }
        }
        ahead / total
    }
}

/// True when `better` shares a rank with `worse` and its other card is higher.
fn dominates(better: Holding, worse: Holding) -> bool {
    let (b, w) = (better.map(|c| c.rank()), worse.map(|c| c.rank()));
    if b[0] == b[1] || w[0] == w[1] {
        return false;
    }
    let shared: Vec<u8> = b.iter().filter(|r| w.contains(r)).copied().collect();
    if shared.len() != 1 {
        return false;
    }
    let other = |h: [u8; 2]| *h.iter().find(|r| **r != shared[0]).expect("a side card");
    other(b) > other(w)
}

fn unpaired(v: V, hero: Holding) -> Vec<Holding> {
    Lab::villains(hero, v).into_iter().filter(|c| c[0].rank() != c[1].rank()).collect()
}

fn flop_chance(remaining: u32, outs: u32) -> f64 {
    // The chance at least one of `outs` cards among `remaining` shows on a three-card flop.
    let choose3 = |n: u32| f64::from(n) * f64::from(n - 1) * f64::from(n - 2);
    1.0 - choose3(remaining - outs) / choose3(remaining)
}

/// Runs one computed check and describes the failure, if there is one.
fn apply(lab: &mut Lab, chart: Chart, label: &str, hero: Holding, check: Check, line: &str) -> Option<String> {
    let fail = |detail: String| Some(format!("{label}: {check:?} on {line:?}: {detail}"));
    match check {
        Check::Ahead(v, share) => {
            let got = lab.ahead_share(hero, v);
            if got < share {
                return fail(format!("ahead of {got}"));
            }
        }
        Check::Behind(v) => {
            let got = lab.equity(label, hero, v);
            if got >= 50.0 {
                return fail(format!("equity {got}"));
            }
        }
        Check::Equity(v, need) => {
            let got = lab.equity(label, hero, v);
            if got < need {
                return fail(format!("equity {got}"));
            }
        }
        Check::Dominates(v, share) => {
            let combos = unpaired(v, hero);
            let got = combos.iter().filter(|c| dominates(hero, **c)).count() as f64 / combos.len() as f64;
            if got < share {
                return fail(format!("dominates {got}"));
            }
        }
        Check::DominatedBy(v, share) => {
            let combos = unpaired(v, hero);
            let got = combos.iter().filter(|c| dominates(**c, hero)).count() as f64 / combos.len() as f64;
            let equity = lab.equity(label, hero, v);
            if got < share || equity >= 50.0 {
                return fail(format!("dominated by {got}, equity {equity}"));
            }
        }
        Check::Set => {
            let chance = flop_chance(50, 2);
            if hero[0].rank() != hero[1].rank() || (chance - 0.125).abs() >= 0.01 {
                return fail(format!("set chance {chance}"));
            }
        }
        Check::OftenOvercard => {
            let chance = flop_chance(50, 4 * u32::from(12 - hero[0].rank()));
            if hero[0].rank() != hero[1].rank() || chance <= 0.5 {
                return fail(format!("overcard chance {chance}"));
            }
        }
        Check::SlightlyAheadOfAceKing => {
            let got = lab.equity(label, hero, V::AceKing);
            if !(50.0..60.0).contains(&got) {
                return fail(format!("equity {got}"));
            }
        }
        Check::BigBlindPrice => {
            let need = 2.0 / 6.5 * 100.0;
            for v in [V::Raises, V::CutoffOpens, V::ButtonOpens] {
                let got = lab.equity(label, hero, v);
                if got < need {
                    return fail(format!("equity {got} against {v:?} needs {need}"));
                }
            }
        }
        Check::UsuallyOnePairOrLess => {
            let share = lab.one_pair_or_less(hero);
            if share <= 0.5 {
                return fail(format!("one pair or less {share}"));
            }
        }
        Check::PriceAgainst(need, ranges) => {
            for v in ranges {
                let got = lab.equity(label, hero, *v);
                if got < need {
                    return fail(format!("equity {got} against {v:?} needs {need}"));
                }
            }
        }
        Check::Kicker(v, share) => {
            let combos = unpaired(v, hero);
            let got = combos.iter().filter(|c| dominates(**c, hero)).count() as f64 / combos.len() as f64;
            if got < share {
                return fail(format!("dominated by {got}"));
            }
        }
        Check::BiggerOfKindPlays => {
            let top = |h: Holding| h[0].rank().max(h[1].rank());
            let kind = HandKind::of(hero);
            let bigger = classes()
                .into_iter()
                .any(|(_, h)| HandKind::of(h) == kind && top(h) > top(hero) && chart.action(h) != Action::Fold);
            if !bigger {
                return fail("no bigger hand of this kind is played".to_string());
            }
        }
        Check::CallersPrice => {
            let need = 3.0 / 10.5 * 100.0;
            let raises = Lab::villains(hero, V::Raises);
            let calls = Lab::villains(hero, V::Calls);
            let got = lab.play(hero, &[&raises, &calls], 40_000);
            if got < need {
                return fail(format!("equity {got} against {need}"));
            }
        }
        Check::ThreeWayPrice => {
            // Closing from the big blind with one caller: 2bb to call into 3 + 3 + 0.5 + 1.
            let need = 2.0 / 9.5 * 100.0;
            let raises = Lab::villains(hero, V::Raises);
            let calls = Lab::villains(hero, V::Calls);
            let got = lab.play(hero, &[&raises, &calls], 40_000);
            if got < need {
                return fail(format!("equity {got} against {need}"));
            }
        }
    }
    None
}

/// Position and setting words must fit the chart the line is shown on.
fn check_position_words(chart: Chart, line: &str) {
    use Chart::{Blinds, BlindsBb, FourBet, Position, PositionEarly, ThreeBetIp, ThreeBetOop};
    let in_chart = |list: &[Chart]| list.contains(&chart);
    let says = |text: &str| line.contains(text);
    if says("in position") || says("with position") || says("position lets") || says("position makes") || says("use position") {
        assert!(in_chart(&[Position, PositionEarly, ThreeBetIp]), "{chart:?}: {line}");
    }
    if says("out of position") {
        assert!(in_chart(&[Blinds, BlindsBb, ThreeBetOop]), "{chart:?}: {line}");
    }
    if says("from the blinds") || says("money in the pot") {
        assert!(in_chart(&[Blinds, BlindsBb]), "{chart:?}: {line}");
    }
    if says("players still to act behind you") {
        assert!(in_chart(&[PositionEarly]), "{chart:?}: {line}");
    }
    if says("3-bet here") {
        assert!(in_chart(&[ThreeBetIp, ThreeBetOop]), "{chart:?}: {line}");
    }
    if says("4-bet here") || says("a 4-bet") && !says("then") {
        assert!(in_chart(&[FourBet]), "{chart:?}: {line}");
    }
    if says("callers") || says("this many hands") {
        assert!(in_chart(&[Chart::Callers, Chart::CallersBb]), "{chart:?}: {line}");
    }
}

/// A line that places a hand behind "the hands that raise" must hold against every
/// opening seat a raiser can have, late seats included.
fn check_raise_words(lab: &mut Lab, label: &str, hero: Holding, line: &str, failures: &mut Vec<String>) {
    if line.contains("the hands that raise") {
        for v in [V::Raises, V::CutoffOpens, V::ButtonOpens] {
            let got = lab.equity(label, hero, v);
            if got >= 50.0 {
                failures.push(format!("{label} on {line:?}: {got} against {v:?}"));
            }
        }
    }
}

/// A line that says a hand is ahead of or behind ace-king has to be true for it.
fn check_ace_king_words(lab: &mut Lab, label: &str, hero: Holding, line: &str) {
    if label.starts_with("AK") {
        return;
    }
    if let Some(at) = line.find("ace-king") {
        let before = &line[..at];
        if before.contains("behind") && !before.contains("ahead") {
            let got = lab.equity(label, hero, V::AceKing);
            assert!(got < 50.0, "{label} on {line:?}: {got} against ace-king");
        }
        if before.contains("ahead of") {
            let got = lab.equity(label, hero, V::AceKing);
            assert!(got > 50.0, "{label} on {line:?}: {got} against ace-king");
        }
    }
}

#[test]
#[cfg_attr(debug_assertions, ignore = "slow without optimizations")]
fn every_reason_line_passes_its_computed_check() {
    let mut lab = Lab::new();
    let mut cells = 0;
    let mut failures = Vec::new();
    for chart in Chart::ALL {
        for (label, holding) in classes() {
            let action = chart.action(holding);
            let kind = HandKind::of(holding);
            let line = why_line(chart, action, kind);
            for check in checks(chart, action, kind) {
                failures.extend(apply(&mut lab, chart, &label, holding, *check, line));
            }
            check_position_words(chart, line);
            check_ace_king_words(&mut lab, &label, holding, line);
            check_raise_words(&mut lab, &label, holding, line, &mut failures);
            cells += 1;
        }
    }
    assert!(failures.is_empty(), "{} failed:\n{}", failures.len(), failures.join("\n"));
    assert_eq!(cells, 9 * 169);
}

#[test]
fn reason_lines_are_plain_single_sentences() {
    let fallbacks = [
        "This hand isn't strong enough to continue here.",
        "This hand is strong enough to continue here.",
        "This hand is strong enough to raise here.",
    ];
    let banned = ["book", "author", "chapter", "according to", "solver", "%"];
    for chart in Chart::ALL {
        for (label, holding) in classes() {
            let line = why_line(chart, chart.action(holding), HandKind::of(holding));
            assert!(!fallbacks.contains(&line), "{chart:?} {label}: {line}");
            assert_eq!(line.matches(". ").count(), 0, "one sentence: {line}");
            let lower = line.to_lowercase();
            for word in banned {
                assert!(!lower.contains(word), "{word} in {line}");
            }
        }
    }
}

/// Lines that name one specific hand must only ever be reached by that hand.
#[test]
fn lines_that_name_a_hand_describe_only_that_hand() {
    let cases = [
        (Chart::Position, Action::Raise, HandKind::AceQueen, vec!["AQo", "AQs"]),
        (Chart::PositionEarly, Action::Call, HandKind::AceQueen, vec!["AQo"]),
        (Chart::Blinds, Action::Raise, HandKind::AceQueen, vec!["AQo", "AQs"]),
        (Chart::Position, Action::Raise, HandKind::SuitedBroadway, vec!["KQs"]),
        (Chart::Position, Action::Raise, HandKind::OffsuitBroadway, vec!["KQo"]),
        (Chart::Blinds, Action::Raise, HandKind::SuitedBroadway, vec!["KQs"]),
        (Chart::Blinds, Action::Raise, HandKind::OffsuitBroadway, vec!["KQo"]),
        (Chart::BlindsBb, Action::Raise, HandKind::SuitedBroadway, vec!["KQs"]),
        (Chart::BlindsBb, Action::Raise, HandKind::OffsuitBroadway, vec!["KQo"]),
        (Chart::Position, Action::Call, HandKind::OffsuitBroadway, vec!["AJo"]),
        (Chart::Blinds, Action::Call, HandKind::OffsuitBroadway, vec!["AJo"]),
        (Chart::Callers, Action::Call, HandKind::OffsuitBroadway, vec!["AJo", "KQo"]),
        (Chart::Position, Action::Raise, HandKind::Jacks, vec!["JJ"]),
        (Chart::Callers, Action::Call, HandKind::AceQueen, vec!["AQo", "AQs"]),

        (Chart::ThreeBetIp, Action::Fold, HandKind::AceQueen, vec!["AQo", "AQs"]),
        (Chart::ThreeBetOop, Action::Fold, HandKind::AceQueen, vec!["AQo", "AQs"]),
        (Chart::ThreeBetIp, Action::Fold, HandKind::AceKing, vec!["AKo", "AKs"]),
        (Chart::FourBet, Action::Raise, HandKind::KingsOrAces, vec!["AA", "KK"]),
        (Chart::FourBet, Action::Fold, HandKind::AceKing, vec!["AKo", "AKs"]),
        (Chart::FourBet, Action::Fold, HandKind::Queens, vec!["QQ"]),
        (Chart::FourBet, Action::Fold, HandKind::AceQueen, vec!["AQo", "AQs"]),
    ];
    for (chart, action, kind, expected) in cases {
        let mut got: Vec<String> = classes()
            .into_iter()
            .filter(|(_, h)| HandKind::of(*h) == kind && chart.action(*h) == action)
            // A 4-bet is only ever faced with a hand you 3-bet.
            .filter(|(_, h)| chart != Chart::FourBet || Chart::Position.lines().0.range.contains_holding(*h))
            .map(|(label, _)| label)
            .collect();
        got.sort();
        let mut expected: Vec<String> = expected.into_iter().map(str::to_string).collect();
        expected.sort();
        assert_eq!(got, expected, "{chart:?} {action:?} {kind:?}");
    }
}

#[test]
fn hand_kinds_are_what_they_say() {
    assert_eq!(HandKind::of(hand("AsKh")), HandKind::AceKing);
    assert_eq!(HandKind::of(hand("AsQs")), HandKind::AceQueen);
    assert_eq!(HandKind::of(hand("KsKh")), HandKind::KingsOrAces);
    assert_eq!(HandKind::of(hand("7s7h")), HandKind::MiddlePair);
    assert_eq!(HandKind::of(hand("6s6h")), HandKind::SmallPair);
    assert_eq!(HandKind::of(hand("As5s")), HandKind::SuitedAce);
    assert_eq!(HandKind::of(hand("Ks8s")), HandKind::OtherSuited);
    assert_eq!(HandKind::of(hand("QsTs")), HandKind::SuitedBroadway);
    assert_eq!(HandKind::of(hand("Ts9s")), HandKind::SuitedConnector);
    assert_eq!(HandKind::of(hand("AsJh")), HandKind::OffsuitBroadway);
    assert_eq!(HandKind::of(hand("As8h")), HandKind::OffsuitAce);
    assert_eq!(HandKind::of(hand("7h2d")), HandKind::OtherOffsuit);
}

#[test]
fn raise_words_name_the_situation() {
    assert_eq!(action_label(Action::Raise, Seat::Button, Situation::Raised), "3-bet");
    assert_eq!(action_label(Action::Raise, Seat::Button, Situation::RaisedCalled(2)), "3-bet");
    assert_eq!(action_label(Action::Raise, Seat::Button, Situation::ThreeBet { in_position: true }), "4-bet");
    assert_eq!(action_label(Action::Raise, Seat::Button, Situation::FourBet), "all in");
    assert_eq!(action_label(Action::Raise, Seat::Button, Situation::FoldedTo), "raise");
    assert_eq!(action_label(Action::Call, Seat::Cutoff, Situation::FourBet), "call");
}

#[test]
fn the_right_chart_answers_each_spot() {
    assert_eq!(chart_for(Seat::Early, Situation::Raised), Some(Chart::PositionEarly));
    assert_eq!(chart_for(Seat::Middle, Situation::Raised), Some(Chart::PositionEarly));
    assert_eq!(chart_for(Seat::Cutoff, Situation::Raised), Some(Chart::Position));
    assert_eq!(chart_for(Seat::Button, Situation::Raised), Some(Chart::Position));
    assert_eq!(chart_for(Seat::SmallBlind, Situation::Raised), Some(Chart::Blinds));
    assert_eq!(chart_for(Seat::BigBlind, Situation::Raised), Some(Chart::BlindsBb));
    assert_eq!(chart_for(Seat::BigBlind, Situation::RaisedCalled(2)), Some(Chart::CallersBb));
    assert_eq!(chart_for(Seat::Button, Situation::RaisedCalled(2)), Some(Chart::Callers));
    assert_eq!(chart_for(Seat::Cutoff, Situation::ThreeBet { in_position: true }), Some(Chart::ThreeBetIp));
    assert_eq!(chart_for(Seat::Cutoff, Situation::ThreeBet { in_position: false }), Some(Chart::ThreeBetOop));
    assert_eq!(chart_for(Seat::Cutoff, Situation::FourBet), Some(Chart::FourBet));
    assert_eq!(chart_for(Seat::Cutoff, Situation::FoldedTo), None);
    assert_eq!(chart_for(Seat::Cutoff, Situation::Limpers(1)), None);
}

/// The note states the rule for in position; the deal has to follow that rule.
#[test]
fn the_three_bet_note_states_the_rule_the_deal_uses() {
    let note = CARD.note("note.three_bet");
    assert!(note.contains("did not open the small blind and the 3-bettor is in a blind"), "{note}");
    assert!(card_value("note.three_bet").contains("otherwise the out-of-position chart"));
    let (mut sb_seen, mut button_seen, mut cutoff_ip, mut cutoff_oop) = (0, 0, 0, 0);
    for players in 2..=9u8 {
        for seed in 0..4000 {
            let Question::Preflop { behind, situation, .. } = generate_preflop(seed, players).question else {
                panic!("expected preflop");
            };
            let Situation::ThreeBet { in_position } = situation else { continue };
            let seat = Seat::from_behind(behind);
            match seat {
                Seat::SmallBlind => {
                    assert!(!in_position, "the small blind opener is out of position against the big blind");
                    sb_seen += 1;
                }
                Seat::Button => {
                    assert!(in_position, "only the blinds are behind the button");
                    button_seen += 1;
                }
                Seat::Cutoff if in_position => cutoff_ip += 1,
                Seat::Cutoff => cutoff_oop += 1,
                _ => {}
            }
        }
    }
    assert!(sb_seen > 0 && button_seen > 0 && cutoff_ip > 0 && cutoff_oop > 0);
}

fn situation_of(item: &Item) -> (Seat, Situation, Holding) {
    let Question::Preflop { behind, situation, hand, .. } = item.question else {
        panic!("expected preflop");
    };
    (Seat::from_behind(behind), situation, hand)
}

fn all_preflop(count: u64) -> Vec<Item> {
    (0..count).map(|seed| generate(Skill::Preflop, seed)).collect()
}

#[test]
fn every_chart_is_reached_at_the_drill_table_sizes() {
    let items = all_preflop(120_000);
    let mut seen: HashMap<Chart, usize> = HashMap::new();
    let mut seats_three_bet: Vec<Seat> = Vec::new();
    let mut four_bet_seats: Vec<Seat> = Vec::new();
    let mut callers_by_count: [usize; 4] = [0; 4];
    for item in &items {
        let (seat, situation, _) = situation_of(item);
        if let Some(chart) = chart_for(seat, situation) {
            *seen.entry(chart).or_insert(0) += 1;
        }
        match situation {
            Situation::ThreeBet { .. } if !seats_three_bet.contains(&seat) => seats_three_bet.push(seat),
            Situation::FourBet if !four_bet_seats.contains(&seat) => four_bet_seats.push(seat),
            Situation::RaisedCalled(n) => callers_by_count[usize::from(n)] += 1,
            _ => {}
        }
    }
    for chart in Chart::ALL {
        assert!(seen.get(&chart).copied().unwrap_or(0) > 500, "{chart:?}: {seen:?}");
    }
    for seat in [Seat::Early, Seat::Middle, Seat::Cutoff, Seat::Button, Seat::SmallBlind] {
        assert!(seats_three_bet.contains(&seat), "{seat:?}");
    }
    assert!(!seats_three_bet.contains(&Seat::BigBlind), "the big blind has no open");
    assert!(four_bet_seats.contains(&Seat::BigBlind));
    assert_eq!(callers_by_count[0], 0, "callers come in ones, twos and threes");
    assert!(callers_by_count[1] > 0 && callers_by_count[2] > 0 && callers_by_count[3] > 0);
    for seat in Seat::ALL {
        assert!(
            items.iter().any(|i| {
                let (s, situation, _) = situation_of(i);
                s == seat && situation == Situation::Raised
            }),
            "{seat:?}"
        );
    }
}

#[test]
fn drills_have_realistic_situation_frequencies() {
    let items = all_preflop(200_000);
    let total = items.len() as f64;
    let share = |test: &dyn Fn(Situation) -> bool| -> f64 {
        items.iter().filter(|i| test(situation_of(i).1)).count() as f64 / total
    };
    let bands: [(&str, f64, f64, f64); 6] = [
        ("folded to", share(&|s| s == Situation::FoldedTo), 0.27, 0.40),
        ("limpers", share(&|s| matches!(s, Situation::Limpers(_))), 0.16, 0.27),
        ("one raise", share(&|s| s == Situation::Raised), 0.20, 0.33),
        ("callers", share(&|s| matches!(s, Situation::RaisedCalled(_))), 0.08, 0.17),
        // Rare on purpose: only kings or aces continue against a 3-bet or a 4-bet.
        ("three-bet", share(&|s| matches!(s, Situation::ThreeBet { .. })), 0.03, 0.07),
        ("four-bet", share(&|s| s == Situation::FourBet), 0.01, 0.03),
    ];
    for (name, got, min, max) in bands {
        assert!((min..=max).contains(&got), "{name} is {got}");
    }
    let small = (0..20_000u64).filter(|&s| drill_players(s) <= 5).count();
    assert!((small as i64 - 16_000).abs() < 500, "{small}");
}

#[test]
fn only_possible_new_situations_are_dealt() {
    for players in 2..=9u8 {
        for seed in 0..3000 {
            let item = generate_preflop(seed, players);
            let Question::Preflop { behind, situation, .. } = item.question else {
                panic!("expected preflop");
            };
            let acted = players - 1 - behind;
            let seat = Seat::from_behind(behind);
            match situation {
                Situation::RaisedCalled(n) => {
                    assert!(acted >= 2, "a caller needs a raiser ahead of it");
                    assert!((1..=3).contains(&n) && n < acted, "{n} callers and a raiser need {acted} acted");
                }
                Situation::ThreeBet { in_position } => {
                    assert_ne!(seat, Seat::BigBlind);
                    assert!(behind >= 1);
                    if in_position {
                        assert!(behind >= 2, "in position needs a blind behind and a seat other than the small blind");
                    }
                }
                Situation::FourBet => assert!(acted >= 1, "you need a raise to have 3-bet"),
                _ => {}
            }
        }
    }
}

#[test]
fn dealt_hands_are_ones_you_could_hold_in_the_spot() {
    for item in all_preflop(80_000) {
        let (seat, situation, holding) = situation_of(&item);
        let label = label_of(holding);
        match situation {
            Situation::ThreeBet { .. } => {
                assert!(card_line(&format!("open.{}", seat_key(seat))).contains(&label), "{seat:?} opened {label}");
            }
            Situation::FourBet => {
                let key = match seat {
                    Seat::SmallBlind | Seat::BigBlind => "face_raise.blinds.reraise",
                    Seat::Early | Seat::Middle => "face_raise.early.reraise",
                    Seat::Cutoff | Seat::Button => "face_raise.reraise",
                };
                assert!(card_line(key).contains(&label), "{seat:?} 3-bet {label}");
            }
            _ => {}
        }
    }
}

/// The big blind's extra calls outside the button's opening range still get dealt, so
/// the drill tests them.
#[test]
fn the_big_blind_extra_calls_are_dealt() {
    let button = card_line("open.button");
    let extra = card_line("face_raise.big_blind.call");
    let outside: BTreeSet<String> = extra.difference(&button).cloned().collect();
    assert!(outside.contains("K2s") && outside.contains("Q5s"), "{outside:?}");
    let mut seen = BTreeSet::new();
    for item in all_preflop(60_000) {
        let (seat, situation, holding) = situation_of(&item);
        if seat == Seat::BigBlind && situation == Situation::Raised {
            seen.insert(label_of(holding));
        }
    }
    let missing: Vec<&String> = outside.iter().filter(|l| !seen.contains(*l)).collect();
    assert!(missing.is_empty(), "never dealt to the big blind against a raise: {missing:?}");
}

#[test]
fn raised_hands_are_never_obvious_folds() {
    let button = card_line("open.button");
    for item in all_preflop(60_000) {
        let (seat, situation, holding) = situation_of(&item);
        if matches!(situation, Situation::Raised | Situation::RaisedCalled(_)) {
            let label = label_of(holding);
            assert!(
                button.contains(&label) || card_text_answer(seat, situation, &label) != Action::Fold,
                "{seat:?} {situation:?} dealt {label}"
            );
        }
    }
}

/// Facing a 3-bet or a 4-bet, the hands dealt are close to the line: kings or aces, which
/// continue, or the strong hands a player is tempted to continue with, never trash.
#[test]
fn three_bet_and_four_bet_spots_deal_hands_close_to_the_line() {
    let close = expand("TT+ AJs+ KQs AQo+");
    let mut continues = 0u32;
    let mut total = 0u32;
    for item in all_preflop(100_000) {
        let (seat, situation, holding) = situation_of(&item);
        if matches!(situation, Situation::ThreeBet { .. } | Situation::FourBet) {
            let label = label_of(holding);
            assert!(close.contains(&label), "{seat:?} {situation:?} dealt {label}");
            total += 1;
            continues += u32::from(card_text_answer(seat, situation, &label) != Action::Fold);
        }
    }
    let share = f64::from(continues) / f64::from(total);
    assert!((0.28..0.39).contains(&share), "{share} of these spots continue");
}

#[test]
fn most_of_the_new_spots_are_real_decisions() {
    let mut folds = HashMap::new();
    let mut totals = HashMap::new();
    let bucket = |s: Situation| match s {
        Situation::Raised => "raised",
        Situation::RaisedCalled(_) => "callers",
        Situation::ThreeBet { .. } => "three_bet",
        Situation::FourBet => "four_bet",
        _ => "other",
    };
    for item in all_preflop(100_000) {
        let (seat, situation, holding) = situation_of(&item);
        let name = bucket(situation);
        *totals.entry(name).or_insert(0u32) += 1;
        if card_text_answer(seat, situation, &label_of(holding)) == Action::Fold {
            *folds.entry(name).or_insert(0u32) += 1;
        }
    }
    for name in ["raised", "callers", "three_bet", "four_bet"] {
        let share = f64::from(folds[name]) / f64::from(totals[name]);
        // Against a 3-bet or a 4-bet only kings or aces continue, so folding is the answer for
        // almost every hand. These spots are kept honest by being rare and by dealing kings or
        // aces a third of the time and tempting hands (queens, ace-king and the like) otherwise,
        // so about two thirds are folds by design; 0.70 leaves room for sampling noise.
        let most = if matches!(name, "three_bet" | "four_bet") { 0.70 } else { 0.66 };
        assert!(share < most, "{name}: {share} folds");
        assert!(share > 0.08, "{name}: {share} folds, always continuing would win");
    }
}

/// Grading through the answer path, against the card text rather than the code that deals it.
#[test]
fn every_dealt_spot_grades_against_the_card_text() {
    for item in all_preflop(40_000) {
        let (seat, situation, holding) = situation_of(&item);
        let right = card_text_answer(seat, situation, &label_of(holding)).code();
        for code in ["r", "c", "f"] {
            let graded = grade(&item, code).expect("a valid answer");
            assert_eq!(graded.correct, code == right, "{seat:?} {situation:?} {holding:?} answered {code}");
        }
        assert!(grade(&item, "x").is_err());
    }
}

#[test]
fn new_spots_have_families_that_keep_a_seat() {
    let families = [
        (Situation::RaisedCalled(1), "cutoff:callers"),
        (Situation::ThreeBet { in_position: true }, "cutoff:3bet_ip"),
        (Situation::ThreeBet { in_position: false }, "cutoff:3bet_oop"),
        (Situation::FourBet, "cutoff:4bet"),
    ];
    for (situation, expected) in families {
        let item = Item { skill: Skill::Preflop, seed: 1, question: Question::Preflop { players: 5, behind: 3, situation, hand: hand("AhKd") } };
        assert_eq!(family_of(&item).as_deref(), Some(expected));
    }
}

#[test]
fn rare_preflop_families_still_find_a_sibling() {
    // Middle position only faces limpers at six or more players, so this is a rare family.
    for base in 0..30u64 {
        let sibling = find_sibling(Skill::Preflop, "middle:limpers", base, 7).expect("found within the budget");
        assert_eq!(family_of(&generate(Skill::Preflop, sibling)).as_deref(), Some("middle:limpers"));
    }
}

#[test]
fn the_new_charts_never_change_which_hands_the_card_plays() {
    for (label, holding) in classes() {
        let opens = CARD.open.iter().any(|line| line.range.contains_holding(holding));
        let iso = CARD.iso_raise.range.contains_holding(holding) || CARD.iso_limp.range.contains_holding(holding);
        assert_eq!(CARD.plays(holding), opens || iso, "{label}");
    }
}

#[test]
fn the_card_notes_never_use_internal_keys() {
    for note in CARD.notes() {
        if ["note.face_raise", "note.callers", "note.three_bet", "note.four_bet"].contains(&note.key) {
            for key in ["face_raise.", "face_callers", "three_bet.", "four_bet."] {
                assert!(!note.text.contains(key), "{}", note.text);
            }
        }
    }
    assert_eq!(CARD.notes().len(), 8, "all notes fit in the table");
    assert!(CARD.note("note.callers").contains("one more raise per caller"));
}

#[test]
fn the_chart_summary_names_the_chart_and_its_lines() {
    let text = Chart::Blinds.summary();
    assert!(text.starts_with("One raise, from the blinds."), "{text}");
    assert!(text.contains("3-bet JJ+"), "{text}");
    assert!(text.contains("Call "), "{text}");
    assert!(text.ends_with("Fold the rest."), "{text}");
    assert!(Chart::FourBet.summary().contains("All in KK+"));
    assert!(Chart::CallersBb.summary().contains("Also call ATo KJo KTo QJo QTo JTo here."));
}
