//! Hand-worked examples for each skill, plus generator properties.

use poker_trainer::answer::{answer_format, AnswerFormat};
use poker_trainer::baseline;
use poker_trainer::cards::{holding_mask, parse_cards, parse_holding, unseen_holdings, Card, Holding};
use poker_trainer::classes::{
    mix_combos, OppMix,
    combos_in_class, is_flush_draw, is_oesd, made_class, matches_opp_class, MadeClass, OppClass,
};
use poker_trainer::equity::{equity_all_in, equity_next_card};
use poker_trainer::family::family_of;
use poker_trainer::eval::Category;
use poker_trainer::generate::{
    board_texture, drill_players, generate, generate_preflop, MIN_CLASS_COMBOS, SPOT_NEAR_PCT,
};
use poker_trainer::grade::{
    beats_analysis, ev_band, grade, nuts_analysis, outs_analysis, parse_nuts_guess,
    possible_analysis, spot_analysis, BeatsBucket, EvBand, Outcome, Reveal,
};
use poker_trainer::item::{HitStreet, Item, Question, Skill, SpotMode, SpotQuestion, GEN_VERSION};
use poker_trainer::log::{format_attempt, parse_log, Attempt, CheckRef, ItemRef, Mode};
use poker_trainer::money;
use poker_trainer::preflop::{action_label, preflop_answer, Action, Seat, Situation, CARD};
use poker_trainer::groups::{group_of, mixed_skills};
use poker_trainer::progress::{
    all_stats, baseline_runs, drill_skills, has_baseline, next_baseline_run, pass_mark, skill_stats,
};
use poker_trainer::rng::Rng;
use poker_trainer::schedule::{
    due_families, recent_results, skill_weight, Origin, Planned, Session, SessionLimits, DAY_MS,
};

fn cards(text: &str) -> Vec<Card> {
    parse_cards(text).unwrap()
}

fn hand(text: &str) -> Holding {
    parse_holding(text).unwrap()
}

fn item(skill: Skill, question: Question) -> Item {
    Item { skill, seed: 1, question }
}

fn answer(item: &Item, text: &str) -> Outcome {
    grade(item, text).unwrap_or_else(|e| panic!("{text} should parse: {}", e.0))
}

// ----- Card parsing and combos -----

#[test]
fn card_parses_to_expected_combo_counts() {
    assert_eq!(CARD.open_range(Seat::Early).unwrap().range.combo_count(), 142);
    assert_eq!(CARD.open_range(Seat::Middle).unwrap().range.combo_count(), 190);
    assert_eq!(CARD.open_range(Seat::Cutoff).unwrap().range.combo_count(), 254);
    assert_eq!(CARD.open_range(Seat::Button).unwrap().range.combo_count(), 326);
    assert_eq!(CARD.open_range(Seat::SmallBlind).unwrap().range.combo_count(), 258);
    assert!(CARD.open_range(Seat::BigBlind).is_none());
    assert_eq!(CARD.notes().len(), 8);
    assert!(CARD.note("note.big_blind").contains("check"));
}

// ----- Preflop -----

const BEHIND_SEATS: [(u8, Seat); 6] = [
    (0, Seat::BigBlind),
    (1, Seat::SmallBlind),
    (2, Seat::Button),
    (3, Seat::Cutoff),
    (4, Seat::Middle),
    (5, Seat::Early),
];

#[test]
fn seat_comes_from_players_behind() {
    for (behind, seat) in BEHIND_SEATS {
        assert_eq!(Seat::from_behind(behind), seat);
    }
    assert_eq!(Seat::from_behind(9), Seat::Early);
}

fn table_order(players: u8) -> Vec<Seat> {
    (0..players).rev().map(Seat::from_behind).collect()
}

#[test]
fn seat_order_at_each_table_size() {
    use Seat::*;
    assert_eq!(table_order(4), [Cutoff, Button, SmallBlind, BigBlind]);
    assert_eq!(table_order(5), [Middle, Cutoff, Button, SmallBlind, BigBlind]);
}

#[test]
fn six_player_seat_mapping_has_one_early_seat() {
    use Seat::*;
    assert_eq!(table_order(6), [Early, Middle, Cutoff, Button, SmallBlind, BigBlind]);
}

#[test]
fn seven_player_seat_mapping_has_two_early_seats() {
    use Seat::*;
    assert_eq!(table_order(7), [Early, Early, Middle, Cutoff, Button, SmallBlind, BigBlind]);
}

#[test]
fn only_possible_situations_are_dealt() {
    for players in 2..=9u8 {
        for seed in 0..1500 {
            let item = generate_preflop(seed, players);
            let Question::Preflop { players: p, behind, situation, .. } = item.question else {
                panic!("expected a preflop item");
            };
            assert_eq!(p, players);
            assert!(behind < players);
            let acted = players - 1 - behind;
            let seat = Seat::from_behind(behind);
            match situation {
                Situation::FoldedTo => assert_ne!(seat, Seat::BigBlind),
                Situation::Limpers(n) => {
                    assert!(acted >= 1, "first to act can't face limpers");
                    assert!((1..=3).contains(&n) && n <= acted, "{n} limpers with {acted} acted");
                }
                Situation::Raised => assert!(acted >= 1, "first to act can't face a raise"),
                Situation::RaisedCalled(n) => assert!(acted >= 2 && n >= 1, "callers need a raiser and {n}"),
                Situation::ThreeBet { .. } => assert_ne!(seat, Seat::BigBlind, "the big blind never opens"),
                Situation::FourBet => assert!(acted >= 1, "a 4-bet needs a raise in front of you"),
            }
            if acted == 0 {
                assert!(
                    matches!(situation, Situation::FoldedTo | Situation::ThreeBet { .. }),
                    "first to act can only open or be 3-bet after opening"
                );
            }
        }
    }
}

#[test]
fn early_seat_is_dealt_only_at_six_or_more_players() {
    let count_early = |players: u8| -> usize {
        (0..2000)
            .filter(|&s| generate_preflop(s, players).question.seat() == Some(Seat::Early))
            .count()
    };
    assert_eq!(count_early(4), 0);
    assert_eq!(count_early(5), 0);
    assert!(count_early(6) > 200);
    assert!(count_early(7) > 400);
}

#[test]
fn every_seat_shows_up_at_a_five_handed_table() {
    for seat in [Seat::Middle, Seat::Cutoff, Seat::Button, Seat::SmallBlind, Seat::BigBlind] {
        assert!((0..500).any(|s| generate_preflop(s, 5).question.seat() == Some(seat)));
    }
}

#[test]
fn drill_table_sizes_are_weighted_40_40_10_10() {
    let mut counts = [0u32; 4];
    for seed in 0..40_000u64 {
        let players = drill_players(seed);
        assert!((4..=7).contains(&players));
        counts[usize::from(players - 4)] += 1;
    }
    for (count, expected) in counts.iter().zip([16_000i64, 16_000, 4_000, 4_000]) {
        assert!((i64::from(*count) - expected).abs() < 500, "{counts:?}");
    }
    // The dispatcher uses the same table size, so a seed fully identifies the item.
    for seed in 0..50 {
        let Question::Preflop { players, .. } = generate(Skill::Preflop, seed).question else {
            panic!("expected preflop");
        };
        assert_eq!(players, drill_players(seed));
    }
}

fn pf(seat: Seat, situation: Situation, text: &str) -> Action {
    preflop_answer(seat, situation, hand(text)).0
}

#[test]
fn folded_to_uses_the_seat_range() {
    assert_eq!(pf(Seat::Early, Situation::FoldedTo, "AhKd"), Action::Raise);
    assert_eq!(pf(Seat::Early, Situation::FoldedTo, "AhJd"), Action::Fold);
    assert_eq!(pf(Seat::Middle, Situation::FoldedTo, "KhQd"), Action::Raise);
    assert_eq!(pf(Seat::Cutoff, Situation::FoldedTo, "9h7h"), Action::Raise);
    assert_eq!(pf(Seat::Button, Situation::FoldedTo, "7h2d"), Action::Fold);
    assert_eq!(pf(Seat::SmallBlind, Situation::FoldedTo, "Ah2h"), Action::Raise);
    assert_eq!(pf(Seat::SmallBlind, Situation::FoldedTo, "KhTd"), Action::Fold);
}

#[test]
fn limper_rules_for_non_blind_seats() {
    assert_eq!(pf(Seat::Middle, Situation::Limpers(1), "AhJd"), Action::Raise);
    assert_eq!(pf(Seat::Middle, Situation::Limpers(1), "Ah2h"), Action::Fold);
    assert_eq!(pf(Seat::Middle, Situation::Limpers(2), "Ah2h"), Action::Call);
    assert_eq!(pf(Seat::Cutoff, Situation::Limpers(1), "2h2d"), Action::Call);
    // From the cutoff or button the suited aces in the limp-along range raise instead.
    assert_eq!(pf(Seat::Cutoff, Situation::Limpers(1), "Ah2h"), Action::Raise);
    assert_eq!(pf(Seat::SmallBlind, Situation::Limpers(1), "Ah2h"), Action::Call);
    assert_eq!(pf(Seat::Button, Situation::Limpers(1), "6h5h"), Action::Call);
    assert_eq!(pf(Seat::SmallBlind, Situation::Limpers(1), "2h2d"), Action::Call);
    assert_eq!(pf(Seat::Cutoff, Situation::Limpers(2), "7h2d"), Action::Fold);
}

#[test]
fn big_blind_raises_iso_hands_and_checks_the_rest() {
    assert_eq!(pf(Seat::BigBlind, Situation::Limpers(1), "QhJh"), Action::Raise);
    assert_eq!(pf(Seat::BigBlind, Situation::Limpers(3), "AhJd"), Action::Raise);
    // Check, not fold, even with the worst hand.
    assert_eq!(pf(Seat::BigBlind, Situation::Limpers(1), "7h2d"), Action::Call);
    assert_eq!(pf(Seat::BigBlind, Situation::Limpers(2), "Ah2h"), Action::Call);
}

#[test]
fn facing_a_raise_agrees_on_the_clear_hands_in_every_seat() {
    for seat in [Seat::Middle, Seat::Button, Seat::BigBlind] {
        assert_eq!(pf(seat, Situation::Raised, "JhJd"), Action::Raise);
        assert_eq!(pf(seat, Situation::Raised, "AhKd"), Action::Raise);
        assert_eq!(pf(seat, Situation::Raised, "ThTd"), Action::Call);
        // Against the wide late openers the button and blinds face, king-queen 3-bets for value;
        // a middle seat still has players behind it and only calls the suited one.
        let (kqo, kqs) = if seat == Seat::Middle { (Action::Fold, Action::Call) } else { (Action::Raise, Action::Raise) };
        assert_eq!(pf(seat, Situation::Raised, "KhQd"), kqo);
        assert_eq!(pf(seat, Situation::Raised, "KhQh"), kqs);
        assert_eq!(pf(seat, Situation::Raised, "7h2d"), Action::Fold);
    }
}

#[test]
fn preflop_grades_through_the_item() {
    let q = |players, behind, situation, text: &str| {
        item(Skill::Preflop, Question::Preflop { players, behind, situation, hand: hand(text) })
    };
    assert!(answer(&q(5, 0, Situation::Limpers(1), "7h2d"), "c").correct);
    assert!(!answer(&q(5, 0, Situation::Limpers(1), "7h2d"), "f").correct);
    assert!(answer(&q(4, 3, Situation::FoldedTo, "KhQd"), "R").correct);
    assert!(grade(&q(4, 3, Situation::FoldedTo, "KhQd"), "x").is_err());
}

#[test]
fn preflop_items_are_not_mostly_folds() {
    let folds = (0..2000u64)
        .filter(|&s| {
            let Question::Preflop { behind, situation, hand, .. } = generate(Skill::Preflop, s).question else {
                panic!("expected preflop");
            };
            preflop_answer(Seat::from_behind(behind), situation, hand).0 == Action::Fold
        })
        .count();
    assert!(folds < 1300, "{folds} folds of 2000");
}

// ----- Hand classes -----

fn class(h: &str, b: &str) -> MadeClass {
    made_class(hand(h), &cards(b))
}

#[test]
fn pair_subdivisions() {
    assert_eq!(class("AhAd", "Kc7d2s"), MadeClass::OverPair);
    assert_eq!(class("KhQd", "Kc7d2s"), MadeClass::TopPair);
    assert_eq!(class("7h6d", "Kc7d2s"), MadeClass::MidPair);
    assert_eq!(class("9h9d", "Kc7d2s"), MadeClass::MidPair);
    assert_eq!(class("2h3d", "Kc7d2s"), MadeClass::LowPair);
    // A pocket pair below the lowest board rank is low, not mid.
    assert_eq!(class("3h3d", "Kc7d5s"), MadeClass::LowPair);
    assert_eq!(class("6h6d", "Kc7d5s"), MadeClass::MidPair);
    // The pair is on the board and neither hole card is part of it.
    assert_eq!(class("AhKd", "7c7d2s"), MadeClass::BoardPair);
    // A river board still works off the five board ranks.
    assert_eq!(class("QhJd", "Ks9d5c3hJc"), MadeClass::MidPair);
}

#[test]
fn trips_and_sets() {
    assert_eq!(class("7h7d", "7c2s9d"), MadeClass::Set);
    assert_eq!(class("7h8s", "7d7cKd"), MadeClass::Trips);
    assert_eq!(class("AhKd", "7c7d7s"), MadeClass::Trips);
}

#[test]
fn other_categories() {
    assert_eq!(class("AhKd", "9c5s2h"), MadeClass::HighCard);
    assert_eq!(class("AhKd", "Ac Kc 2h".replace(' ', "").as_str()), MadeClass::TwoPair);
    assert_eq!(class("9h9d", "9c9s2h"), MadeClass::Quads);
    assert_eq!(class("9h9d", "9c2s2h"), MadeClass::FullHouse);
    assert_eq!(class("AsKs", "9s5s2s"), MadeClass::Flush);
    assert_eq!(class("9h8d", "7c6s5d"), MadeClass::Straight);
    assert_eq!(class("9h8h", "7h6h5h"), MadeClass::StraightFlush);
    // Board plays: the board is the straight.
    assert_eq!(class("2c3d", "AsKdQhJcTs"), MadeClass::Straight);
    assert_eq!(class("2c3d", "AsKsQsJsTs"), MadeClass::StraightFlush);
}

#[test]
fn flush_draw_rules() {
    assert!(is_flush_draw(hand("AhKh"), &cards("2h7h9c")));
    assert!(is_flush_draw(hand("AsKd"), &cards("2s7s9s")));
    // Made flush is not a draw.
    assert!(!is_flush_draw(hand("AhKh"), &cards("2h7h9h")));
    // Four to a flush on the board with no hole card in it.
    assert!(!is_flush_draw(hand("AcKd"), &cards("2h7h9hJh")));
    // Three suited only.
    assert!(!is_flush_draw(hand("AhKd"), &cards("2h7h9c")));
    // Never on the river.
    assert!(!is_flush_draw(hand("AhKh"), &cards("2h7h9c4d5d")));
}

#[test]
fn open_ended_straight_draw_rules() {
    assert!(is_oesd(hand("9h8d"), &cards("7c6sKd")));
    // A-2-3-4 and J-Q-K-A are one-ended.
    assert!(!is_oesd(hand("Ah2d"), &cards("3c4s9h")));
    assert!(!is_oesd(hand("AhKd"), &cards("QcJs2h")));
    // 2-3-4-5 is open at both ends (ace low and six); T-J-Q-K likewise.
    assert!(is_oesd(hand("2h3d"), &cards("4c5s9h")));
    assert!(is_oesd(hand("KhQd"), &cards("JcTs3h")));
    // Gutshot and double-gutter.
    assert!(!is_oesd(hand("9h8d"), &cards("7c5sKd")));
    assert!(!is_oesd(hand("7h9d"), &cards("5c8sJd")));
    // Made straight, or the draw is all on the board.
    assert!(!is_oesd(hand("9h8d"), &cards("7c6s5d")));
    assert!(!is_oesd(hand("AhKd"), &cards("5c6s7d8h")));
    // Never on the river.
    assert!(!is_oesd(hand("9h8d"), &cards("7c6sKd2h3s")));
}

fn class_counts(h: &str, b: &str) -> [usize; 6] {
    [OppClass::Over, OppClass::Top, OppClass::Mid, OppClass::Trips, OppClass::Set, OppClass::TwoPair]
        .map(|class| combos_in_class(class, hand(h), &cards(b)).len())
}

#[test]
fn paired_board_spot_classes_reference_vector() {
    // Hero J-T on J-7-7. Over: QQ/KK/AA, 18. Top: 2 jacks x 43 side cards, 86.
    // Mid: 88 and 99 at 6 each and TT at 3, 15. Trips: 2 sevens x 43, 86.
    // JJ and J7 are full houses, so set and two pair are empty.
    assert_eq!(class_counts("JcTs", "Jd7c7s"), [18, 86, 15, 86, 0, 0]);
}

#[test]
fn paired_top_rank_makes_trips_not_top_pair() {
    // Hero A-Q on K-K-7. Over: only AA, with one ace gone, 3. Top: none, the top
    // rank is the paired one. Mid: 88 to JJ at 6 each and QQ at 3, 27.
    // Trips: 2 kings x 42 side cards, 84. 77 is a full house, so no set.
    assert_eq!(class_counts("AhQd", "KsKd7c"), [3, 0, 27, 84, 0, 0]);
}

#[test]
fn paired_middle_rank_on_the_turn() {
    // Hero A-K on Q-9-9-4. Over: KK and AA with one card gone each, 6.
    // Top: 3 queens x 38, 114. Mid: 55 66 77 88 TT JJ at 6 each, 36. Trips: 2
    // nines x 38, 76. QQ and 44 are full houses, so no set. Two pair: Q-4, 3 x 3.
    assert_eq!(class_counts("AcKd", "Qs9d9h4c"), [6, 114, 36, 76, 0, 9]);
}

#[test]
fn a_pocket_pair_under_a_paired_board_is_not_two_pair() {
    let board = cards("Jd7c7s");
    assert!(matches_opp_class(OppClass::Mid, hand("9h9d"), &board));
    assert!(!matches_opp_class(OppClass::TwoPair, hand("9h9d"), &board));
    // Below both board ranks it is in no pair-type class.
    assert!(OppClass::SINGLE.iter().all(|&c| !matches_opp_class(c, hand("4h4d"), &board)));
    assert!(matches_opp_class(OppClass::Low, hand("4h4d"), &board));
}

#[test]
fn made_hands_are_outside_the_pair_type_classes() {
    let pair_types = [OppClass::Over, OppClass::Top, OppClass::Mid, OppClass::Trips, OppClass::Set, OppClass::TwoPair];
    // A straight, a flush, a full house and a straight that also draws to a flush.
    for (h, b) in [("9h8d", "7c6s5d"), ("AhKh", "Qh7h2h"), ("JhJs", "Jd7c7s"), ("9c8c", "7c6c5d")] {
        assert!(pair_types.iter().all(|&c| !matches_opp_class(c, hand(h), &cards(b))), "{h} on {b}");
    }
    // Trips of a rank that also fill a straight or flush belong to the made hand.
    assert!(!matches_opp_class(OppClass::Trips, hand("7h6h"), &cards("7c7s5h4h8h")));
}

#[test]
fn pair_type_classes_match_made_classes_on_unpaired_boards() {
    let mut rng = Rng::new(404);
    let mut checked = 0;
    while checked < 20_000 {
        let mut deck: Vec<Card> = (0..52u8).map(Card::from_index).collect();
        rng.shuffle(&mut deck);
        let board = &deck[2..5 + (rng.below(2) as usize)];
        if board_texture(board).paired {
            continue;
        }
        checked += 1;
        let holding = [deck[0], deck[1]];
        let made = made_class(holding, board);
        let pairs = [
            (OppClass::Over, MadeClass::OverPair),
            (OppClass::Top, MadeClass::TopPair),
            (OppClass::Mid, MadeClass::MidPair),
            (OppClass::TwoPair, MadeClass::TwoPair),
            (OppClass::Set, MadeClass::Set),
            (OppClass::Trips, MadeClass::Trips),
        ];
        for (opp, expected) in pairs {
            assert_eq!(matches_opp_class(opp, holding, board), made == expected, "{holding:?} on {board:?}");
        }
    }
}

#[test]
fn spots_deal_paired_boards_at_a_natural_rate() {
    let paired = (0..200u64)
        .filter(|&seed| {
            let Question::Spot(q) = generate(Skill::Spot, seed).question else { panic!() };
            board_texture(&q.board).paired
        })
        .count();
    assert!((16..=90).contains(&paired), "{paired} paired of 200");
}

#[test]
fn set_class_combos_on_a_flop() {
    let combos = combos_in_class(OppClass::Set, hand("AhQh"), &cards("Kh7h2c"));
    assert_eq!(combos.len(), 9);
}

// ----- Nuts -----

fn nuts_item(b: &str) -> Item {
    item(Skill::Nuts, Question::Nuts { board: cards(b) })
}

#[test]
fn nuts_guess_parsing() {
    assert!(parse_nuts_guess("QJ").is_some());
    assert!(parse_nuts_guess("qj").is_some());
    assert!(parse_nuts_guess("A K s").is_some());
    assert!(parse_nuts_guess("AhKh").is_some());
    assert!(parse_nuts_guess("ahx").is_some());
    assert!(parse_nuts_guess("Ax").is_some());
    assert!(parse_nuts_guess("AAs").is_none());
    assert!(parse_nuts_guess("AQJ").is_none());
    assert!(parse_nuts_guess("A").is_none());
    assert!(parse_nuts_guess("").is_none());
}

#[test]
fn nuts_x_rule_needs_every_holding() {
    let board = "KhQhJhTh2c";
    assert!(answer(&nuts_item(board), "Ahx").correct);
    assert!(!answer(&nuts_item(board), "Ax").correct);
    assert!(answer(&nuts_item(board), "AK").correct);
    assert!(!answer(&nuts_item(board), "AKs").correct);
}

#[test]
fn nuts_any_ace_makes_broadway_on_four_to_broadway() {
    let board = "KcQdJhTs";
    assert!(answer(&nuts_item(board), "Ax").correct);
    assert!(answer(&nuts_item(board), "As x").correct);
    assert!(answer(&nuts_item(board), "A9").correct);
    assert!(!answer(&nuts_item(board), "Kx").correct);
}

#[test]
fn nuts_on_a_wheel_board_is_five_four() {
    let analysis = nuts_analysis(&cards("As2d3h"));
    assert_eq!(analysis.top.category(), Category::Straight);
    assert_eq!(analysis.top.primary(), 3);
    assert_eq!(analysis.top_holdings.len(), 16);
    assert!(answer(&nuts_item("As2d3h"), "54").correct);
    assert!(!answer(&nuts_item("As2d3h"), "4x").correct);
    assert!(!answer(&nuts_item("As2d3h"), "AA").correct);
}

#[test]
fn nuts_on_a_paired_board_is_quads_then_kings_full() {
    let analysis = nuts_analysis(&cards("KhKd7c"));
    assert_eq!(analysis.top.category(), Category::Quads);
    let (next, holdings) = analysis.next.unwrap();
    assert_eq!(next.category(), Category::FullHouse);
    // K-7 can't be beaten by anything disjoint from it, so next is sevens full of kings.
    assert_eq!((next.field(0), next.field(1)), (5, 11));
    assert_eq!(holdings.len(), 3, "the three pairs of the unseen sevens");
    assert!(answer(&nuts_item("KhKd7c"), "KK").correct);
    // Card removal: with one king left nobody can have KK against K-7, so it is the nuts.
    assert!(answer(&nuts_item("KhKd7c"), "K7").correct);
}

#[test]
fn nuts_on_a_three_flush_board_needs_both_hole_cards() {
    assert!(answer(&nuts_item("9h5h2h"), "AKs").correct);
    assert!(answer(&nuts_item("9h5h2h"), "AhKh").correct);
    assert!(!answer(&nuts_item("9h5h2h"), "Ahx").correct);
    assert!(!answer(&nuts_item("9h5h2h"), "AsKs").correct);
}

#[test]
fn nuts_on_a_five_flush_board() {
    let board = "AhKhQhJh9h";
    assert!(answer(&nuts_item(board), "Thx").correct);
    assert!(answer(&nuts_item(board), "Th2c").correct);
    assert!(!answer(&nuts_item(board), "Tx").correct);
}

#[test]
fn nuts_board_straight_flush_can_be_beaten_from_the_hole() {
    let analysis = nuts_analysis(&cards("9h8h7h6h5h"));
    assert_eq!(analysis.top.category(), Category::StraightFlush);
    assert_eq!(analysis.top.primary(), 9);
    assert!(!analysis.all_tie);
    assert!(answer(&nuts_item("9h8h7h6h5h"), "JhTh").correct);
}

#[test]
fn nuts_generator_skips_boards_that_all_tie() {
    assert!(nuts_analysis(&cards("AsKsQsJsTs")).all_tie);
    for seed in 0..200 {
        let Question::Nuts { board } = generate(Skill::Nuts, seed).question else { panic!() };
        assert!(!nuts_analysis(&board).all_tie);
    }
}

#[test]
fn nuts_answer_naming_board_cards_is_invalid_not_wrong() {
    assert!(grade(&nuts_item("AhKh2c"), "AhKh").is_err());
    assert!(grade(&nuts_item("KhKdKc"), "KK").is_err());
    assert!(grade(&nuts_item("AhKh2c"), "Ahx").is_err());
}

// ----- Possible -----

fn possible(b: &str) -> String {
    possible_analysis(&cards(b)).set.code()
}

#[test]
fn possible_hand_worked_boards() {
    assert_eq!(possible("Ks9h2d"), "none");
    assert_eq!(possible("Ah7h2h"), "f");
    assert_eq!(possible("As2d3h"), "s");
    assert_eq!(possible("8h8d3c"), "h");
    assert_eq!(possible("7h8h9h"), "fs");
    assert_eq!(possible("AhKhQh2h3h"), "fs");
    assert_eq!(possible("KhKd7c2s"), "h");
    assert_eq!(possible("8h8d8c"), "h");
    assert_eq!(possible("2c3d4h5s6c"), "s");
}

#[test]
fn possible_grading_is_an_exact_set_match() {
    let q = item(Skill::Possible, Question::Possible { board: cards("7h8h9h") });
    assert!(answer(&q, "fs").correct);
    assert!(answer(&q, "s f").correct);
    assert!(!answer(&q, "f").correct);
    assert!(!answer(&q, "none").correct);
    assert!(grade(&q, "fx").is_err());
    assert!(grade(&q, "ff").is_err());
}

#[test]
fn board_texture_agrees_with_enumeration() {
    let mut rng = Rng::new(99);
    for _ in 0..2000 {
        let mut deck: Vec<Card> = (0..52u8).map(Card::from_index).collect();
        rng.shuffle(&mut deck);
        let board = &deck[..3 + (rng.below(3) as usize)];
        let texture = board_texture(board);
        let set = possible_analysis(board).set;
        assert_eq!(texture.flush_possible, set.flush, "{board:?}");
        assert_eq!(texture.straight_possible, set.straight, "{board:?}");
        assert_eq!(texture.paired, set.full, "{board:?}");
    }
}

#[test]
fn about_sixty_percent_of_boards_are_textured() {
    for skill in [Skill::Nuts, Skill::Possible] {
        let textured = (0..1000u64)
            .filter(|&seed| {
                let (Question::Nuts { board } | Question::Possible { board }) = generate(skill, seed).question else {
                    panic!();
                };
                !board_texture(&board).is_dry()
            })
            .count();
        assert!((540..=660).contains(&textured), "{skill:?}: {textured} of 1000");
    }
}

// ----- Made -----

#[test]
fn made_grades_by_code_case_insensitively() {
    let q = item(Skill::Made, Question::Made { hero: hand("7h7d"), board: cards("7c2s9d") });
    assert!(answer(&q, "SET").correct);
    assert!(!answer(&q, "trips").correct);
    assert!(grade(&q, "pair").is_err());
}

#[test]
fn made_answers_are_roughly_balanced() {
    let mut counts = std::collections::HashMap::new();
    for seed in 0..1400u64 {
        let Question::Made { hero, board } = generate(Skill::Made, seed).question else { panic!() };
        *counts.entry(made_class(hero, &board)).or_insert(0u32) += 1;
    }
    for class in MadeClass::ALL {
        let n = counts.get(&class).copied().unwrap_or(0);
        assert!(n > 0, "{class:?} never dealt");
        assert!(n < 260, "{class:?} dealt {n} of 1400");
    }
}

// ----- Beats -----

#[test]
fn beats_hand_worked() {
    // Top set on a dry flop: nothing is ahead.
    let a = beats_analysis(hand("AsAd"), &cards("Ah7c2d"));
    assert_eq!((a.ahead, a.total, a.bucket), (0, 1081, BeatsBucket::A));
    // Middle set: only the three combos of pocket kings are ahead.
    let a = beats_analysis(hand("7h7d"), &cards("Kh7c2d"));
    assert_eq!((a.ahead, a.total), (3, 1081));
    assert_eq!(a.bucket, BeatsBucket::A);
    assert_eq!(a.kinds.len(), 1);
    assert_eq!(a.kinds[0].category, Category::Trips);
    assert_eq!(a.hero_rank.describe(), "three sevens with K-2 kickers");
    // 3-2 high on an A-K-9 flop: only the nine other 3-2 combos fail to beat it.
    let a = beats_analysis(hand("3c2d"), &cards("AhKd9s"));
    assert_eq!((a.ahead, a.total), (1072, 1081));
    assert_eq!(a.bucket, BeatsBucket::D);
    // A royal flush on the board ties everyone, so nobody is strictly ahead.
    let a = beats_analysis(hand("2c3d"), &cards("AsKsQsJsTs"));
    assert_eq!((a.ahead, a.total, a.bucket), (0, 990, BeatsBucket::A));
}

#[test]
fn beats_bucket_boundaries() {
    assert_eq!(BeatsBucket::from_pct(1.99), BeatsBucket::A);
    assert_eq!(BeatsBucket::from_pct(2.0), BeatsBucket::B);
    assert_eq!(BeatsBucket::from_pct(9.99), BeatsBucket::B);
    assert_eq!(BeatsBucket::from_pct(10.0), BeatsBucket::C);
    assert_eq!(BeatsBucket::from_pct(30.0), BeatsBucket::C);
    assert_eq!(BeatsBucket::from_pct(30.01), BeatsBucket::D);
}

#[test]
fn beats_grades_a_to_d_and_buckets_are_balanced() {
    let q = item(Skill::Beats, Question::Beats { hero: hand("3c2d"), board: cards("AhKd9s") });
    assert!(answer(&q, "d").correct);
    assert!(!answer(&q, "a").correct);
    assert!(grade(&q, "e").is_err());
    let mut counts = [0u32; 4];
    for seed in 0..400u64 {
        let Question::Beats { hero, board } = generate(Skill::Beats, seed).question else { panic!() };
        counts[beats_analysis(hero, &board).bucket as usize] += 1;
    }
    assert!(counts.iter().all(|&n| n >= 60), "{counts:?}");
}

// ----- Outs -----

fn outs_item(h: &str, b: &str, v: &str) -> Item {
    item(Skill::Outs, Question::Outs { hero: hand(h), board: cards(b), villain: hand(v) })
}

#[test]
fn outs_exact_and_close_are_separate() {
    let q = outs_item("AhQh", "Kh7h2c", "KsTd");
    let exact = answer(&q, "12");
    assert!(exact.correct && !exact.close);
    let near = answer(&q, "11");
    assert!(!near.correct && near.close);
    let off = answer(&q, "9");
    assert!(!off.correct && !off.close);
    assert!(grade(&q, "twelve").is_err());
}

#[test]
fn outs_on_the_turn_use_one_card() {
    let Reveal::Outs { analysis, .. } = answer(&outs_item("AhQh", "Kh7h2c3d", "KsTd"), "12").reveal else {
        panic!();
    };
    assert_eq!(analysis.outs.len(), 12);
    assert_eq!(analysis.unseen, 44);
}

#[test]
fn outs_a_pair_below_the_opponents_pair_is_not_an_out() {
    // Hero AhQs has ace high against KsKd on 9c5d2h.
    let Reveal::Outs { analysis, .. } = answer(&outs_item("AhQs", "9c5d2h", "KsKd"), "3").reveal else {
        panic!();
    };
    // Only the three remaining aces win: queens pair below kings.
    assert_eq!(analysis.outs.len(), 3);
    assert!(analysis.tempting.iter().any(|t| t.card.rank() == 10));
}

#[test]
fn outs_generator_meets_the_brief() {
    let mut with_tempting = 0;
    let total = 300;
    for seed in 0..total {
        let Question::Outs { hero, board, villain } = generate(Skill::Outs, seed).question else { panic!() };
        assert!((3..=4).contains(&board.len()));
        let a = outs_analysis(hero, &board, villain);
        assert!((2..=17).contains(&a.outs.len()));
        assert!(poker_trainer::classes::rank_of(villain, &board) > poker_trainer::classes::rank_of(hero, &board));
        if !a.tempting.is_empty() {
            with_tempting += 1;
        }
    }
    assert!(with_tempting * 100 >= total * 30, "{with_tempting} of {total}");
}

// ----- Price -----

fn price_item(pot_before: u32, bet: u32, callers: u32) -> Item {
    item(Skill::Price, Question::Price { pot_before, bet, callers })
}

#[test]
fn price_within_two_points_is_correct() {
    let q = price_item(100, 50, 0);
    assert!(answer(&q, "25").correct);
    assert!(answer(&q, "25%").correct);
    assert!(answer(&q, "23").correct);
    assert!(answer(&q, "27").correct);
    assert!(!answer(&q, "28").correct);
    assert!(!answer(&q, "22").correct);
    assert!(grade(&q, "cheap").is_err());
    assert!(grade(&q, "150").is_err());
}

#[test]
fn price_counts_earlier_callers_in_the_pot() {
    // P = 100 + 50 + 50 = 200, C = 50, so 50 / 250 = 20%.
    let Reveal::Price { pot, call, required } = answer(&price_item(100, 50, 1), "20").reveal else { panic!() };
    assert_eq!((pot, call), (200, 50));
    assert!((required - 0.2).abs() < 1e-12);
}

#[test]
fn price_generator_uses_whole_dollar_bets() {
    for seed in 0..200 {
        let Question::Price { pot_before, bet, .. } = generate(Skill::Price, seed).question else { panic!() };
        assert!(bet > 0 && bet <= pot_before * 2);
    }
}

// ----- Hit -----

#[test]
fn hit_within_two_points_and_rule_of_two_and_four() {
    let q = item(Skill::Hit, Question::Hit { outs: 9, street: HitStreet::Flop, both: false });
    assert!(answer(&q, "19").correct);
    assert!(answer(&q, "21").correct);
    assert!(!answer(&q, "22").correct);
    let Reveal::Hit { unseen, shortcut, .. } = answer(&q, "19").reveal else { panic!() };
    assert_eq!((unseen, shortcut), (47, 18.0));

    let q = item(Skill::Hit, Question::Hit { outs: 9, street: HitStreet::Flop, both: true });
    let Reveal::Hit { exact, shortcut, .. } = answer(&q, "35").reveal else { panic!() };
    assert!((exact - 0.3497).abs() < 0.001);
    assert_eq!(shortcut, 36.0);

    // Turn uses 46 unseen: 9/46 = 19.6%.
    let q = item(Skill::Hit, Question::Hit { outs: 9, street: HitStreet::Turn, both: false });
    let Reveal::Hit { unseen, exact, .. } = answer(&q, "20").reveal else { panic!() };
    assert_eq!(unseen, 46);
    assert!((exact - 9.0 / 46.0).abs() < 1e-12);
}

#[test]
fn hit_generator_only_asks_both_cards_on_the_flop() {
    for seed in 0..300 {
        let Question::Hit { outs, street, both } = generate(Skill::Hit, seed).question else { panic!() };
        assert!([2, 4, 6, 8, 9, 10, 12, 15].contains(&outs));
        assert!(!both || street == HitStreet::Flop);
    }
}

// ----- Spot -----

fn spot(mode: SpotMode, opponents: Vec<OppClass>, pot: u32, call: u32) -> SpotQuestion {
    let opponents = opponents.into_iter().map(OppMix::single).collect();
    SpotQuestion { hero: hand("AhQh"), board: cards("Kh7h2c"), pot, call, mode, opponents }
}

fn spot_item(q: SpotQuestion) -> Item {
    item(Skill::Spot, Question::Spot(q))
}

#[test]
fn next_card_share_against_a_set_is_eight_of_forty_five() {
    // Against any set the eight clean hearts win and 2h pairs the board or
    // makes quads, so hero has 8 of 45 cards no matter which combo it is.
    let q = spot(SpotMode::NextCard { x: 0 }, vec![OppClass::Set], 150, 50);
    let a = spot_analysis(&q, 1).unwrap();
    assert!((a.share - 8.0 / 45.0).abs() < 1e-12, "{}", a.share);
    assert!((a.ev_call - (8.0 / 45.0 * 200.0 - 50.0)).abs() < 1e-9);
    assert!((a.break_even_x.unwrap() - 81.25).abs() < 1e-9);
    assert_eq!(a.combos_per_opponent, vec![9]);
    assert!(a.next_card);
}

#[test]
fn spot_grades_by_ev_loss_bands() {
    // EV(call) with X = 0 is 8/45 * 200 - 50 = -14.44, which is 9.63% of the pot.
    let q = spot_item(spot(SpotMode::NextCard { x: 0 }, vec![OppClass::Set], 150, 50));
    let called = answer(&q, "c");
    assert!(!called.correct && !called.close);
    assert!((called.ev_loss_pct.unwrap() - 9.6296).abs() < 0.01);
    let Reveal::Spot { band, .. } = called.reveal else { panic!() };
    assert_eq!(band, Some(EvBand::Mistake));
    let folded = answer(&q, "f");
    assert!(folded.correct);
    assert_eq!(folded.ev_loss_pct, Some(0.0));
    assert!(grade(&q, "r").is_err());
}

#[test]
fn spot_large_x_turns_it_into_a_call() {
    // X = 200: EV(call) = 8/45 * 400 - 50 = 21.11, so folding is the error.
    let q = spot_item(spot(SpotMode::NextCard { x: 200 }, vec![OppClass::Set], 150, 50));
    assert!(answer(&q, "c").correct);
    let folded = answer(&q, "f");
    assert!(!folded.correct);
    assert!((folded.ev_loss_pct.unwrap() - 14.074).abs() < 0.01);
}

#[test]
fn spot_a_close_decision_counts_as_correct_either_way() {
    // X = 80: EV(call) = 8/45 * 280 - 50 = -0.22, under 1% of the pot.
    let q = spot_item(spot(SpotMode::NextCard { x: 80 }, vec![OppClass::Set], 150, 50));
    let called = answer(&q, "c");
    assert!(called.correct && called.close);
    let Reveal::Spot { band, .. } = called.reveal else { panic!() };
    assert_eq!(band, Some(EvBand::Close));
    let folded = answer(&q, "f");
    assert!(folded.correct && folded.close);
}

#[test]
fn ev_bands_at_their_boundaries() {
    assert_eq!(ev_band(0.99), EvBand::Close);
    assert_eq!(ev_band(1.0), EvBand::Small);
    assert_eq!(ev_band(4.99), EvBand::Small);
    assert_eq!(ev_band(5.0), EvBand::Mistake);
    assert_eq!(ev_band(15.0), EvBand::Mistake);
    assert_eq!(ev_band(15.01), EvBand::Blunder);
}

#[test]
fn all_in_spot_averages_every_combo_equally() {
    let q = spot(SpotMode::AllIn, vec![OppClass::Set], 150, 50);
    let a = spot_analysis(&q, 1).unwrap();
    let combos = combos_in_class(OppClass::Set, q.hero, &q.board);
    let mean: f64 = combos
        .iter()
        .map(|c| equity_all_in(q.hero, &q.board, &[vec![*c]], 1).unwrap().share)
        .sum::<f64>()
        / combos.len() as f64;
    assert!((a.share - mean).abs() < 1e-12);
    assert!((a.ev_call - money::ev_call(mean, 150.0, 50.0, 0.0)).abs() < 1e-9);
    assert!((a.price - 0.25).abs() < 1e-12);
}

#[test]
fn spot_generator_meets_the_brief() {
    let total = 200u64;
    let mut near = 0;
    let mut next_card = 0;
    for seed in 0..total {
        let generated = generate(Skill::Spot, seed);
        let Question::Spot(q) = &generated.question else { panic!() };
        assert_eq!(q.opponents.len(), 1);
        assert!((3..=4).contains(&q.board.len()), "never the river");
        assert_spot_quality(q, seed);
        for mix in &q.opponents {
            for &class in &mix.classes {
                assert!(combos_in_class(class, q.hero, &q.board).len() >= MIN_CLASS_COMBOS);
            }
        }
        let a = spot_analysis(q, seed).unwrap();
        if (a.required - a.share).abs() <= SPOT_NEAR_PCT {
            near += 1;
        }
        if let SpotMode::NextCard { .. } = q.mode {
            next_card += 1;
            let lists = vec![mix_combos(&q.opponents[0], q.hero, &q.board)];
            let all_in = equity_all_in(q.hero, &q.board, &lists, seed).unwrap();
            assert!(all_in.share < 0.5, "seed {seed}: next-card spot at {}", all_in.share);
        }
        // Tight players pay nothing more, loose players pay the most; rounded to $5.
        let after_calling = q.pot + q.call;
        if matches!(q.mode, SpotMode::NextCard { .. }) {
            match q.opponent_label() {
                poker_trainer::item::OpponentLabel::Tight => assert_eq!(q.x(), 0),
                poker_trainer::item::OpponentLabel::Average => assert!(q.x().abs_diff(after_calling / 4) <= 3, "{q:?}"),
                poker_trainer::item::OpponentLabel::CallingStation => assert!(q.x().abs_diff(after_calling / 3) <= 3, "{q:?}"),
            }
        } else {
            assert_eq!(q.x(), 0);
        }
    }
    assert!(near * 100 >= total * 70, "{near} of {total} near break-even");
    assert!(next_card > 40 && next_card < 160, "{next_card} next-card spots");
}

/// A hand the card plays, and a share hero can both win and lose.
fn assert_spot_quality(q: &SpotQuestion, seed: u64) {
    assert!(CARD.plays(q.hero), "seed {seed}: the card never plays {:?}", q.hero);
    let share = spot_analysis(q, seed).unwrap().share;
    assert!((0.08..=0.92).contains(&share), "seed {seed}: share {share}");
}


#[test]
fn the_card_plays_real_hands_only() {
    assert!(CARD.plays(hand("AhKd")));
    assert!(CARD.plays(hand("6h5h")));
    assert!(!CARD.plays(hand("6h3d")));
    assert!(!CARD.plays(hand("7h2d")));
}

#[test]
fn baseline_spots_meet_the_quality_rules() {
    for entry in baseline::load().unwrap() {
        if let Question::Spot(q) = &entry.item.question {
            assert_spot_quality(q, entry.item.seed);
        }
    }
}

#[test]
fn made_descriptions_do_not_repeat_the_label() {
    assert!(!MadeClass::Trips.description().to_lowercase().contains("trips"));
    assert!(!MadeClass::Set.description().to_lowercase().contains(" set"));
    assert!(!MadeClass::TopPair.description().to_lowercase().contains("top pair"));
}

// ----- Answer formats and front-end facts -----

#[test]
fn every_answer_format_offers_codes_the_grader_accepts() {
    for skill in Skill::ALL {
        for seed in 0..30 {
            let item = generate(skill, seed);
            match answer_format(&item.question) {
                AnswerFormat::Choice(options) => {
                    assert!(!options.is_empty());
                    for o in options {
                        assert!(grade(&item, o.code).is_ok(), "{skill:?} {}", o.code);
                    }
                }
                AnswerFormat::MultiSelect { options, none_code, .. } => {
                    assert!(grade(&item, none_code).is_ok());
                    let all: String = options.iter().map(|o| o.code).collect();
                    assert!(grade(&item, &all).is_ok());
                }
                AnswerFormat::Integer { min, max } => {
                    assert!(grade(&item, &min.to_string()).is_ok());
                    assert!(grade(&item, &max.to_string()).is_ok());
                    assert!(grade(&item, &(max + 1).to_string()).is_err());
                }
                AnswerFormat::Percent => assert!(grade(&item, "25").is_ok()),
                AnswerFormat::Holding => assert_eq!(skill, Skill::Nuts),
            }
        }
    }
}

fn preflop_format(behind: u8, situation: Situation) -> Vec<(&'static str, &'static str)> {
    let question = Question::Preflop { players: 5, behind, situation, hand: hand("AhKd") };
    let AnswerFormat::Choice(options) = answer_format(&question) else { panic!() };
    options.iter().map(|o| (o.code, o.label)).collect()
}

#[test]
fn preflop_answer_format_labels_c_for_the_situation() {
    assert_eq!(preflop_format(3, Situation::Limpers(1)), [("r", "raise"), ("c", "limp along"), ("f", "fold")]);
    assert_eq!(preflop_format(3, Situation::Raised), [("r", "3-bet"), ("c", "call"), ("f", "fold")]);
    assert_eq!(preflop_format(0, Situation::Limpers(2)), [("r", "raise"), ("c", "check")]);
    assert_eq!(preflop_format(0, Situation::Raised), [("r", "3-bet"), ("c", "call"), ("f", "fold")]);
    assert_eq!(preflop_format(3, Situation::ThreeBet { in_position: true }), [("r", "4-bet"), ("c", "call"), ("f", "fold")]);
    assert_eq!(preflop_format(3, Situation::FourBet), [("r", "all in"), ("c", "call"), ("f", "fold")]);
    assert_eq!(action_label(Action::Call, Seat::Cutoff, Situation::Limpers(1)), "limp along");
    assert_eq!(action_label(Action::Call, Seat::BigBlind, Situation::Limpers(1)), "check");
}

#[test]
fn answer_formats_for_the_other_skills() {
    let board = cards("Kh7h2c");
    assert_eq!(answer_format(&Question::Nuts { board: board.clone() }), AnswerFormat::Holding);
    let possible = answer_format(&Question::Possible { board: board.clone() });
    assert!(matches!(possible, AnswerFormat::MultiSelect { none_code: "none", .. }));
    let outs = Question::Outs { hero: hand("AhQh"), board: board.clone(), villain: hand("KsTd") };
    assert_eq!(answer_format(&outs), AnswerFormat::Integer { min: 0, max: 47 });
    assert_eq!(answer_format(&Question::Price { pot_before: 100, bet: 50, callers: 0 }), AnswerFormat::Percent);
    let AnswerFormat::Choice(options) = answer_format(&Question::Made { hero: hand("AhQh"), board }) else {
        panic!();
    };
    assert_eq!(options.len(), MadeClass::ALL.len());
}

#[test]
fn a_tie_is_a_fact_in_the_outs_analysis() {
    // K-4 v K-5 on Q-T-9: a jack gives both the same king-high straight.
    let tied = outs_analysis(hand("Kh4d"), &cards("QsTd9c"), hand("Ks5c"));
    let jack = tied.tempting.iter().find(|t| t.card == Card::parse("Js").unwrap()).unwrap();
    assert!(jack.ties);
    assert!(!tied.outs.contains(&jack.card));
    // 2h leaves hero's flush behind the full house, which is not a tie.
    let behind = outs_analysis(hand("AhQh"), &cards("Kh7h2c"), hand("7c7d"));
    let deuce = behind.tempting.iter().find(|t| t.card == Card::parse("2h").unwrap()).unwrap();
    assert!(!deuce.ties);
}

#[test]
fn session_limits_default_and_finish() {
    assert_eq!(SessionLimits::resolve(None, None).minutes, Some(10));
    assert_eq!(SessionLimits::resolve(None, Some(5)).minutes, None);
    assert_eq!(SessionLimits::resolve(Some(3), Some(5)).minutes, Some(3));
    let limits = SessionLimits::resolve(Some(1), Some(5));
    assert!(!limits.finished(4, 59_999));
    assert!(limits.finished(5, 0));
    assert!(limits.finished(0, 60_000));
    let huge = SessionLimits::resolve(Some(u64::MAX), None);
    assert!(!huge.finished(0, u64::MAX - 1), "a huge --minutes must not wrap");
    let items_only = SessionLimits::resolve(None, Some(2));
    assert!(!items_only.finished(1, u64::MAX / 2));
}

#[test]
fn drill_skills_are_only_those_requested_or_every_group() {
    assert_eq!(drill_skills(&[Skill::Nuts, Skill::Nuts, Skill::Hit]), vec![Skill::Nuts, Skill::Hit]);
    let rotation = drill_skills(&[]);
    assert_eq!(rotation, mixed_skills());
    assert!(rotation.contains(&Skill::Preflop) && rotation.contains(&Skill::Table) && rotation.contains(&Skill::Chain));
    assert!(!rotation.contains(&Skill::Beats) && !rotation.contains(&Skill::Price));
}

#[test]
fn baseline_on_record_is_detected() {
    assert!(!has_baseline(&[attempt(Skill::Nuts, 1, 0, true)]));
    let mut a = attempt(Skill::Nuts, 1, 0, true);
    a.mode = Mode::Baseline;
    assert!(has_baseline(&[a]));
}

#[test]
fn attempts_are_built_from_outcomes() {
    let q = price_item(100, 50, 0);
    let outcome = answer(&q, "25");
    assert!(outcome.ok());
    let new = Planned { skill: Skill::Price, seed: 9, origin: Origin::New };
    let a = Attempt::from_drill(new, &outcome, 7, 90_000);
    assert_eq!((a.mode, a.response_ms, a.item.clone()), (Mode::Drill, 60_000, ItemRef::Seed(9)));
    assert_eq!(a.gen_version, GEN_VERSION);
    let retry = Planned { origin: Origin::SessionRetry, ..new };
    assert_eq!(Attempt::from_drill(retry, &outcome, 7, 10).mode, Mode::DrillRetry);
    let entry = baseline::load().unwrap().remove(0);
    let b = Attempt::from_baseline(3, &entry, &outcome, 7, 10);
    assert_eq!((b.mode, b.baseline_run), (Mode::Baseline, Some(3)));
    assert_eq!(b.item, ItemRef::Baseline(entry.id));
}

#[test]
fn parsed_logs_are_oldest_first() {
    let late = format_attempt(&attempt(Skill::Nuts, 1, 9, true));
    let early = format_attempt(&attempt(Skill::Nuts, 2, 3, true));
    let (parsed, _) = parse_log(&format!("{late}\n{early}\n"));
    assert_eq!(parsed.iter().map(|a| a.timestamp_ms).collect::<Vec<_>>(), vec![3, 9]);
}

// ----- Multiway -----

#[test]
fn multiway_ties_split_the_pot() {
    let board = cards("AsKsQsJsTs");
    let hero = hand("2c3c");
    let one = vec![vec![hand("4c5c")]];
    let two = vec![vec![hand("4c5c")], vec![hand("6c7c")]];
    let three = vec![vec![hand("4c5c")], vec![hand("6c7c")], vec![hand("8c9c")]];
    assert_eq!(equity_all_in(hero, &board, &one, 1).unwrap().share, 1.0 / 2.0);
    assert_eq!(equity_all_in(hero, &board, &two, 1).unwrap().share, 1.0 / 3.0);
    assert_eq!(equity_all_in(hero, &board, &three, 1).unwrap().share, 1.0 / 4.0);
}

#[test]
fn multiway_weights_every_disjoint_tuple_equally() {
    let q = SpotQuestion {
        hero: hand("AhQh"),
        board: cards("Kh7h2c"),
        pot: 200,
        call: 50,
        mode: SpotMode::AllIn,
        opponents: vec![OppMix::single(OppClass::Set), OppMix::single(OppClass::Top)],
    };
    let a = spot_analysis(&q, 1).unwrap();
    assert!(!a.sampled);
    let first = combos_in_class(OppClass::Set, q.hero, &q.board);
    let second = combos_in_class(OppClass::Top, q.hero, &q.board);
    let (mut sum, mut n) = (0.0, 0.0);
    for f in &first {
        for s in &second {
            if holding_mask(f) & holding_mask(s) != 0 {
                continue;
            }
            sum += equity_all_in(q.hero, &q.board, &[vec![*f], vec![*s]], 1).unwrap().share;
            n += 1.0;
        }
    }
    assert!((a.share - sum / n).abs() < 1e-12);
}

#[test]
fn large_multiway_case_is_sampled_deterministically() {
    let board = cards("Kh7h2c");
    let hero = hand("AhQh");
    let dead = holding_mask(&hero) | poker_trainer::cards::mask_of(&board);
    let wide: Vec<Holding> = unseen_holdings(dead).into_iter().take(150).collect();
    let lists = vec![wide.clone(), wide.clone(), wide];
    let first = equity_all_in(hero, &board, &lists, 77).unwrap();
    let again = equity_all_in(hero, &board, &lists, 77).unwrap();
    let other = equity_all_in(hero, &board, &lists, 78).unwrap();
    assert!(first.sampled);
    assert_eq!(first.share, again.share);
    assert_ne!(first.share, other.share);
    assert!((first.share - other.share).abs() < 0.02);
}

#[test]
fn the_sample_lands_close_to_exact_enumeration() {
    let board = cards("Kh7h2c");
    let hero = hand("AhQh");
    let lists = vec![
        combos_in_class(OppClass::Top, hero, &board),
        combos_in_class(OppClass::Set, hero, &board),
    ];
    let exact = equity_all_in(hero, &board, &lists, 1).unwrap();
    assert!(!exact.sampled);
    let estimate = poker_trainer::equity::sampled(hero, &board, &lists, 2, 300_000, 5).unwrap();
    assert!(estimate.sampled);
    assert!((exact.share - estimate.share).abs() < 0.005, "{} v {}", exact.share, estimate.share);
}

#[test]
fn tuple_count_is_only_reported_for_exact_results() {
    let board = cards("Kh7h2c");
    let hero = hand("AhQh");
    let exact = equity_all_in(hero, &board, &[vec![hand("KsTd"), hand("7c7d")]], 1).unwrap();
    assert_eq!(exact.tuples, Some(2));
    let list = vec![hand("KsTd"), hand("7c7d")];
    let estimate = poker_trainer::equity::sampled(hero, &board, &[list], 2, 100, 5).unwrap();
    assert_eq!(estimate.tuples, None);
}

#[test]
fn heads_up_is_never_sampled() {
    let board = cards("Kh7h2c");
    let hero = hand("AhQh");
    let dead = holding_mask(&hero) | poker_trainer::cards::mask_of(&board);
    let all = unseen_holdings(dead);
    assert!(!equity_all_in(hero, &board, &[all], 1).unwrap().sampled);
}

#[test]
fn no_disjoint_tuple_is_reported() {
    let board = cards("Kh7h2c");
    let hero = hand("AhQh");
    let only = vec![hand("KsKd")];
    assert!(equity_all_in(hero, &board, &[only.clone(), only], 1).is_none());
}

#[test]
fn next_card_equity_counts_one_card_only() {
    let eq = equity_next_card(hand("AhQh"), &cards("Kh7h2c"), &[vec![hand("KsTd")]], 1).unwrap();
    assert!((eq.share - 12.0 / 45.0).abs() < 1e-12, "{}", eq.share);
}

#[test]
fn spot_mw_generator_meets_the_brief() {
    let total = 60u64;
    let mut near = 0;
    let mut sizes = [0u32; 4];
    for seed in 0..total {
        let generated = generate(Skill::SpotMw, seed);
        let Question::Spot(q) = &generated.question else { panic!() };
        sizes[q.opponents.len()] += 1;
        assert!((3..=4).contains(&q.board.len()));
        assert_spot_quality(q, seed);
        for mix in &q.opponents {
            for &class in &mix.classes {
                assert!(combos_in_class(class, q.hero, &q.board).len() >= MIN_CLASS_COMBOS);
            }
        }
        let a = spot_analysis(q, seed).unwrap();
        if (a.required - a.share).abs() <= SPOT_NEAR_PCT {
            near += 1;
        }
        if let SpotMode::NextCard { .. } = q.mode {
            let lists: Vec<Vec<Holding>> =
                q.opponents.iter().map(|m| mix_combos(m, q.hero, &q.board)).collect();
            let all_in = equity_all_in(q.hero, &q.board, &lists, seed).unwrap();
            assert!(all_in.share < 0.5, "seed {seed}: next-card spot at {}", all_in.share);
        }
    }
    assert!(sizes[2] > 0 && sizes[3] > 0, "{sizes:?}");
    assert!(near * 100 >= total * 70, "{near} of {total} near break-even");
}

// ----- Determinism -----

#[test]
fn a_seed_always_regenerates_the_same_item() {
    for skill in Skill::ALL {
        for seed in [0u64, 1, 42, u64::MAX, 987_654_321] {
            assert_eq!(generate(skill, seed), generate(skill, seed), "{skill:?} seed {seed}");
        }
    }
    assert_ne!(generate(Skill::Nuts, 1), generate(Skill::Nuts, 2));
    assert_eq!(generate_preflop(9, 6), generate_preflop(9, 6));
}

#[test]
fn rng_sequence_is_frozen() {
    // Published SplitMix64 outputs for seed 1234567.
    let mut rng = Rng::new(1_234_567);
    assert_eq!(rng.next_u64(), 6_457_827_717_110_365_317);
    assert_eq!(rng.next_u64(), 3_203_168_211_198_807_973);
    assert_eq!(rng.next_u64(), 9_817_491_932_198_370_423);
}

// ----- Scheduling -----

fn attempt(skill: Skill, seed: u64, t: u64, correct: bool) -> Attempt {
    Attempt {
        timestamp_ms: t,
        mode: Mode::Drill,
        baseline_run: None,
        skill,
        item: ItemRef::Seed(seed),
        gen_version: GEN_VERSION,
        correct,
        close: false,
        ev_loss_pct: None,
        response_ms: 1000,
        answer: "x".to_string(),
        extra: Default::default(),
    }
}

#[test]
fn skill_weight_is_a_tenth_plus_miss_rate_over_twenty() {
    assert_eq!(skill_weight(&[]), 100);
    assert_eq!(skill_weight(&[true; 20]), 100);
    assert_eq!(skill_weight(&[false; 20]), 1100);
    let mut mix = vec![false; 30];
    mix.extend(vec![true; 10]);
    mix.extend(vec![false; 5]);
    mix.extend(vec![true; 5]);
    // Only the last 20 count: 5 misses.
    assert_eq!(skill_weight(&mix), 100 + 5 * 1000 / 20);
}

#[test]
fn never_three_of_one_skill_in_a_row() {
    let allowed = vec![Skill::Preflop, Skill::Nuts, Skill::Price];
    let mut session = Session::new(allowed, &[], 0, 5);
    let mut history = Vec::new();
    for i in 0..3000 {
        let planned = session.try_next_item().unwrap();
        history.push(planned.skill);
        session.record(planned, i % 3 != 0);
    }
    for window in history.windows(3) {
        assert!(!(window[0] == window[1] && window[1] == window[2]), "{window:?}");
    }
}

#[test]
fn a_miss_returns_after_three_to_six_others() {
    let allowed = vec![Skill::Preflop, Skill::Nuts, Skill::Price, Skill::Hit];
    let mut session = Session::new(allowed, &[], 0, 11);
    // (skill, family, position) of each miss still waiting for its retry.
    let mut waiting: Vec<(Skill, String, usize)> = Vec::new();
    let mut retries = 0;
    for i in 0..4000 {
        let planned = session.try_next_item().unwrap();
        let family = family_of(&generate(planned.skill, planned.seed));
        if planned.origin == Origin::SessionRetry {
            retries += 1;
            let key = (planned.skill, family.clone().expect("a retried skill has a family"));
            let index = waiting
                .iter()
                .position(|(s, f, _)| (*s, f.clone()) == key)
                .expect("a retry is a sibling of an earlier miss");
            let (_, _, missed_at) = waiting.remove(index);
            let others = i - missed_at - 1;
            assert!((3..=8).contains(&others), "retry after {others} others");
        }
        // Miss every fifth item, and get every retry right.
        let ok = planned.origin == Origin::SessionRetry || i % 5 != 0;
        if !ok {
            waiting.push((planned.skill, family.clone().unwrap(), i));
        }
        session.record(planned, ok);
    }
    assert!(retries > 100);
}

#[test]
fn an_item_returns_at_most_once_per_session() {
    let allowed = vec![Skill::Preflop, Skill::Nuts, Skill::Price];
    let mut session = Session::new(allowed, &[], 0, 21);
    let mut seen: std::collections::HashMap<u64, u32> = std::collections::HashMap::new();
    for _ in 0..600 {
        let planned = session.try_next_item().unwrap();
        *seen.entry(planned.seed).or_insert(0) += 1;
        session.record(planned, false);
    }
    assert!(seen.values().all(|&n| n <= 2), "an item was dealt more than twice");
}

#[test]
fn retries_never_crowd_out_new_items() {
    let allowed = vec![Skill::Preflop, Skill::Nuts, Skill::Price];
    let mut session = Session::new(allowed, &[], 0, 8);
    let (mut retries, mut new_items) = (0u32, 0u32);
    for dealt in 1..=120u32 {
        let planned = session.try_next_item().unwrap();
        if planned.origin == Origin::SessionRetry {
            retries += 1;
        } else {
            new_items += 1;
        }
        assert!(retries * 3 <= dealt, "{retries} retries in {dealt} items");
        session.record(planned, false);
    }
    assert!(new_items >= 80, "{new_items} new items in 120");
    assert!(retries >= 30, "{retries} retries in 120, so the cap should be close to full");
}

/// `attempt`, in a family, so the review ladder sees it.
fn in_family(mut row: Attempt, family: &str) -> Attempt {
    row.extra.family = Some(family.to_string());
    row
}

#[test]
fn a_retry_missed_again_is_due_on_a_later_day() {
    let mut retry = in_family(attempt(Skill::Nuts, 5, 100, false), "flush");
    retry.mode = Mode::DrillRetry;
    let log = vec![in_family(attempt(Skill::Nuts, 5, 0, false), "flush"), retry];
    assert!(due_families(&log, 100 + DAY_MS - 1).is_empty());
    assert_eq!(due_families(&log, 100 + DAY_MS).len(), 1);
}

#[test]
fn timestamps_near_the_limit_saturate() {
    let miss = in_family(attempt(Skill::Nuts, 1, u64::MAX - 5, false), "flush");
    assert!(due_families(std::slice::from_ref(&miss), u64::MAX - 1).is_empty());
    assert_eq!(due_families(&[miss], u64::MAX).len(), 1);
}

#[test]
fn only_drill_attempts_count_for_the_miss_rate() {
    let mut log = filled(Skill::Nuts, 5, false, 1000);
    for a in &mut log {
        a.mode = Mode::Baseline;
    }
    log.push(attempt(Skill::Nuts, 9, 9, true));
    let mut retry = attempt(Skill::Nuts, 10, 10, false);
    retry.mode = Mode::DrillRetry;
    log.push(retry);
    let recent = recent_results(&log);
    assert_eq!(recent[&Skill::Nuts].iter().copied().collect::<Vec<_>>(), vec![true]);
}

#[test]
fn review_ladder_runs_one_three_seven_fourteen_thirty_then_retires() {
    let row = |seed: u64, t: u64, ok: bool| in_family(attempt(Skill::Nuts, seed, t, ok), "straight");
    let due = |attempts: &[Attempt], now: u64| !due_families(attempts, now).is_empty();
    let day = DAY_MS;

    let miss = row(77, 0, false);
    assert!(!due(std::slice::from_ref(&miss), day - 1));
    assert!(due(std::slice::from_ref(&miss), day));

    // A correct answer before the due time doesn't advance it, and doesn't need the same seed.
    let early = row(5, day / 2, true);
    assert!(due(&[miss.clone(), early], day));

    let mut log = vec![miss];
    // Each correct review, on a later seed in the family, moves up the ladder.
    let mut now = day;
    for (index, &gap) in FAMILY_GAPS.iter().enumerate() {
        log.push(row(100 + index as u64, now, true));
        assert!(!due(&log, now + day * gap - 1), "after review {index}, not yet due");
        assert!(due(&log, now + day * gap), "after review {index}, due at {gap} days");
        now += day * gap;
    }
    // Correct at the 30-day step retires it.
    log.push(row(200, now, true));
    assert!(!due(&log, now + day * 365), "retired after the 30-day review");

    // A miss during the ladder starts it again at 1 day.
    let relapse = vec![row(77, 0, false), row(7, day, true), row(8, day + day, false)];
    assert!(!due(&relapse, day * 3 - 1));
    assert!(due(&relapse, day * 3));
}

/// Days between reviews after the first correct one.
const FAMILY_GAPS: [u64; 4] = [3, 7, 14, 30];

#[test]
fn a_miss_resets_the_ladder_to_the_first_rung() {
    let row = |t: u64, ok: bool| in_family(attempt(Skill::Nuts, t, t, ok), "straight");
    let day = DAY_MS;
    // Miss, review right at 1 day (now 3 days away), miss at day 2.
    let log = vec![row(0, false), row(day, true), row(day * 2, false)];
    assert!(due_families(&log, day * 3 - 1).is_empty());
    assert!(!due_families(&log, day * 3).is_empty(), "due again 1 day after the reset");
    // The review after the reset is the first rung, so the next gap is 3 days.
    let mut log = log;
    log.push(row(day * 3, true));
    assert!(due_families(&log, day * 6 - 1).is_empty());
    assert!(!due_families(&log, day * 6).is_empty());
}

#[test]
fn a_retired_family_comes_back_after_a_new_miss() {
    let row = |t: u64, ok: bool| in_family(attempt(Skill::Nuts, t, t, ok), "straight");
    let day = DAY_MS;
    let mut log = vec![row(0, false)];
    let mut now = day;
    for gap in [1, 3, 7, 14, 30] {
        log.push(row(now, true));
        now += day * gap;
    }
    log.push(row(now, true));
    assert!(due_families(&log, now + day * 400).is_empty(), "retired");
    log.push(row(now + day * 400, false));
    assert_eq!(due_families(&log, now + day * 401).len(), 1, "back at the first step");
}

#[test]
fn a_correct_retry_does_not_move_the_ladder() {
    let day = DAY_MS;
    let miss = in_family(attempt(Skill::Nuts, 1, 0, false), "straight");
    let mut retry = in_family(attempt(Skill::Nuts, 2, day, true), "straight");
    retry.mode = Mode::DrillRetry;
    assert_eq!(due_families(&[miss, retry], day).len(), 1, "a retry is not a review");
}

#[test]
fn rows_from_before_families_were_logged_are_left_out() {
    assert!(due_families(&[attempt(Skill::Nuts, 7, 0, false)], DAY_MS * 9).is_empty());
}

#[test]
fn due_families_put_the_slowest_relative_to_the_skill_first() {
    let row = |skill: Skill, seed: u64, t: u64, ok: bool, ms: u32, family: &str| {
        let mut a = in_family(attempt(skill, seed, t, ok), family);
        a.response_ms = ms;
        a
    };
    // Nuts has two due families: one answered slowly, one quickly, around a 2s median.
    let log = vec![
        row(Skill::Nuts, 1, 0, true, 2_000, "flush"),
        row(Skill::Nuts, 2, 1, true, 2_000, "straight"),
        row(Skill::Nuts, 3, 2, false, 1_000, "flush"),
        row(Skill::Nuts, 4, 3, false, 6_000, "straight"),
    ];
    let due = due_families(&log, DAY_MS * 2);
    assert_eq!(due.len(), 2);
    assert_eq!(due[0].family, "straight", "slowest first: {due:?}");
    assert!(due[0].slowness > due[1].slowness);
    // The slowness ratio is the family's median over the skill's median.
    assert!((due[0].slowness - 4_000.0 / 2_000.0).abs() < 1e-9, "{}", due[0].slowness);
}

#[test]
fn never_missed_families_are_never_due() {
    assert!(due_families(&[in_family(attempt(Skill::Nuts, 1, 0, true), "straight")], u64::MAX / 2).is_empty());
}

#[test]
fn items_from_another_gen_version_are_skipped() {
    let mut old = in_family(attempt(Skill::Nuts, 7, 0, false), "straight");
    old.gen_version = GEN_VERSION + 1;
    assert!(due_families(&[old], DAY_MS * 9).is_empty());
}

#[test]
fn baseline_misses_are_not_scheduled() {
    let mut a = in_family(attempt(Skill::Nuts, 7, 0, false), "straight");
    a.mode = Mode::Baseline;
    a.item = ItemRef::Baseline("b01".to_string());
    a.baseline_run = Some(1);
    assert!(due_families(&[a], DAY_MS * 9).is_empty());
}

#[test]
fn outs_within_one_is_not_a_miss() {
    let mut a = in_family(attempt(Skill::Outs, 7, 0, false), "5-8:plain");
    a.close = true;
    assert!(due_families(&[a], DAY_MS * 9).is_empty());
}

#[test]
fn session_deals_due_items_for_allowed_skills_only() {
    let family = family_of(&generate(Skill::Nuts, 7)).unwrap();
    let log = vec![in_family(attempt(Skill::Nuts, 7, 0, false), &family), in_family(attempt(Skill::Spot, 8, 0, false), "allin:top")];
    let mut session = Session::new(vec![Skill::Preflop, Skill::Nuts], &log, DAY_MS, 3);
    let mut saw_due = false;
    for _ in 0..200 {
        let planned = session.try_next_item().unwrap();
        assert_ne!(planned.skill, Skill::Spot);
        if planned.origin == Origin::Due {
            assert_eq!(planned.skill, Skill::Nuts);
            assert_ne!(planned.seed, 7, "a fresh sibling, not the same seed");
            assert_eq!(family_of(&generate(Skill::Nuts, planned.seed)).unwrap(), family);
            saw_due = true;
        }
        session.record(planned, true);
    }
    assert!(saw_due);
}

// ----- Progress -----

fn filled(skill: Skill, n: usize, correct: bool, ms: u32) -> Vec<Attempt> {
    (0..n)
        .map(|i| {
            // Only rungs 2-3 of beats_me count for its mark.
            let seed = if skill == Skill::BeatsMe { i as u64 | (3 << 60) } else { i as u64 };
            let mut a = attempt(skill, seed, i as u64, correct);
            a.response_ms = ms;
            a
        })
        .collect()
}

#[test]
fn below_twenty_attempts_is_not_enough_data() {
    let s = skill_stats(Skill::Nuts, &filled(Skill::Nuts, 19, true, 1000));
    assert!(!s.enough_data && !s.passed);
    assert!(skill_stats(Skill::Nuts, &filled(Skill::Nuts, 20, true, 1000)).enough_data);
}

#[test]
fn a_skill_needs_a_full_forty_attempts_to_pass() {
    let short = skill_stats(Skill::Nuts, &filled(Skill::Nuts, 39, true, 1000));
    assert!(short.enough_data, "20 or more attempts display their stats");
    assert!(!short.drill_passed, "39 attempts can't pass");
    assert!(skill_stats(Skill::Nuts, &filled(Skill::Nuts, 40, true, 1000)).drill_passed);
}

#[test]
fn pass_marks_check_accuracy_and_speed_over_the_last_forty() {
    assert!(skill_stats(Skill::Nuts, &filled(Skill::Nuts, 40, true, 6000)).drill_passed);
    assert!(!skill_stats(Skill::Nuts, &filled(Skill::Nuts, 40, true, 6500)).drill_passed);
    assert!(skill_stats(Skill::Beats, &filled(Skill::Beats, 40, true, 9000)).drill_passed);
    assert_eq!(pass_mark(Skill::Spot).max_blunders, Some(2));

    // 60 old misses followed by 40 hits: only the last 40 count.
    let mut log = filled(Skill::Made, 60, false, 1000);
    log.extend(filled(Skill::Made, 40, true, 1000));
    let s = skill_stats(Skill::Made, &log);
    assert_eq!(s.window, 40);
    assert!(s.drill_passed);
}

#[test]
fn outs_pass_mark_counts_within_one() {
    let mut log = filled(Skill::Outs, 40, false, 1000);
    for a in &mut log {
        a.close = true;
    }
    assert!(skill_stats(Skill::Outs, &log).drill_passed);
}

#[test]
fn spot_pass_mark_uses_mean_ev_loss_and_blunders() {
    let build = |losses: &[f64]| -> Vec<Attempt> {
        losses
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let mut a = attempt(Skill::Spot, i as u64, i as u64, *l < 1.0);
                a.ev_loss_pct = Some(*l);
                a
            })
            .collect()
    };
    assert!(skill_stats(Skill::Spot, &build(&[1.0; 40])).drill_passed);
    assert!(!skill_stats(Skill::Spot, &build(&[3.0; 40])).drill_passed);
    let mut three_blunders = vec![0.0; 37];
    three_blunders.extend([20.0, 20.0, 20.0]);
    assert!(!skill_stats(Skill::Spot, &build(&three_blunders)).drill_passed);
    let mut two_blunders = vec![0.0; 38];
    two_blunders.extend([20.0, 20.0]);
    assert!(skill_stats(Skill::Spot, &build(&two_blunders)).drill_passed);
}

#[test]
fn baseline_attempts_do_not_count_for_pass_marks() {
    let mut log = filled(Skill::Nuts, 40, true, 1000);
    for a in &mut log {
        a.mode = Mode::Baseline;
    }
    let s = skill_stats(Skill::Nuts, &log);
    assert_eq!(s.total_attempts, 0);
    assert!(!s.passed);
}

#[test]
fn same_session_retries_do_not_count() {
    let mut retries = filled(Skill::Nuts, 40, true, 1000);
    for a in &mut retries {
        a.mode = Mode::DrillRetry;
    }
    assert_eq!(skill_stats(Skill::Nuts, &retries).total_attempts, 0);
}

#[test]
fn a_pass_once_reached_is_never_taken_back() {
    let mut log = filled(Skill::Nuts, 40, true, 1000);
    let reached = skill_stats(Skill::Nuts, &log);
    assert!(reached.drill_passed && reached.passed);
    // A bad run afterwards drops the current window below the mark, not the pass.
    log.extend(filled(Skill::Nuts, 40, false, 1000).into_iter().map(|mut a| {
        a.timestamp_ms += 1_000_000;
        a
    }));
    let after = skill_stats(Skill::Nuts, &log);
    assert!(!after.drill_passed, "the latest 40 are all misses");
    assert!(after.passed, "a reached mark stays reached");
    // Never meeting the mark over any 40 in a row is not a pass.
    let mut mixed = filled(Skill::Nuts, 30, true, 1000);
    mixed.extend(filled(Skill::Nuts, 30, false, 1000));
    assert!(!skill_stats(Skill::Nuts, &mixed).passed);
}

/// Rows that passed one skill, logged under the old generator.
fn passed_under_old_gates(skill: Skill, run: u32) -> Vec<Attempt> {
    let mut rows = filled(skill, 40, true, 1000);
    rows.extend(completed_checkup(skill, run, 0, true));
    for row in &mut rows {
        row.gen_version = 1;
    }
    rows
}

#[test]
fn passes_earned_under_the_old_gates_are_kept_and_every_group_is_dealt() {
    let mut log = Vec::new();
    for (run, skill) in [Skill::Nuts, Skill::Possible, Skill::Made, Skill::Beats, Skill::BoardType].into_iter().enumerate() {
        log.extend(passed_under_old_gates(skill, run as u32 + 1));
    }
    let stats = all_stats(&log);
    for skill in [Skill::Nuts, Skill::Possible, Skill::Made, Skill::Beats, Skill::BoardType] {
        assert!(stats.iter().any(|s| s.skill == skill && s.passed), "{skill:?}");
    }
    let dealt = drill_skills(&[]);
    assert!(dealt.iter().all(|s| group_of(*s).is_some()), "{dealt:?}");
    assert!([Skill::Outs, Skill::Hit, Skill::Equity, Skill::Anchor, Skill::Chain, Skill::Table].iter().all(|s| dealt.contains(s)), "{dealt:?}");
}

#[test]
fn an_old_per_skill_checkup_is_history_and_no_longer_gates_the_pass() {
    let mut log = filled(Skill::Nuts, 40, true, 1000);
    assert_eq!(skill_stats(Skill::Nuts, &log).checkup_passed, None);
    assert!(skill_stats(Skill::Nuts, &log).passed, "the drill mark alone passes");

    log.extend(completed_checkup(Skill::Nuts, 1, 0, false));
    let failed = skill_stats(Skill::Nuts, &log);
    assert!(failed.drill_passed);
    assert_eq!(failed.checkup_passed, Some(false));
    assert!(failed.passed, "a missed checkup takes nothing away");

    log.extend(completed_checkup(Skill::Nuts, 2, 1, true));
    assert_eq!(skill_stats(Skill::Nuts, &log).checkup_passed, Some(true));
    // The latest checkup is still the one reported.
    log.extend(completed_checkup(Skill::Nuts, 3, 2, false));
    assert_eq!(skill_stats(Skill::Nuts, &log).checkup_passed, Some(false));
    assert!(skill_stats(Skill::Nuts, &log).passed);
}

#[test]
fn an_unfinished_checkup_does_not_count_as_the_latest() {
    let mut log = filled(Skill::Nuts, 40, true, 1000);
    log.extend(completed_checkup(Skill::Nuts, 1, 0, true));
    let mut partial = completed_checkup(Skill::Nuts, 2, 1, false);
    partial.truncate(5);
    log.extend(partial);
    let s = skill_stats(Skill::Nuts, &log);
    assert_eq!(s.checkup_passed, Some(true));
    assert!(s.passed);
}

/// A full 50-slot checkup run, all right or all wrong.
fn completed_checkup(skill: Skill, run: u32, form: u32, correct: bool) -> Vec<Attempt> {
    (0..50u32)
        .map(|slot| {
            let mut a = attempt(skill, 1 << 63, 1_000_000 + u64::from(run) * 100 + u64::from(slot), correct);
            a.mode = Mode::Check;
            a.extra.check = Some(CheckRef { run, form, slot });
            a
        })
        .collect()
}

// ----- Attempt log -----

#[test]
fn log_lines_round_trip() {
    let mut a = attempt(Skill::Spot, 42, 1_700_000_000_000, false);
    a.close = true;
    a.ev_loss_pct = Some(3.5);
    a.response_ms = 1234;
    a.answer = "a\tb".to_string();
    let line = format_attempt(&a);
    assert_eq!(line.split('\t').count(), 11);
    let (parsed, warnings) = parse_log(&line);
    assert!(warnings.is_empty());
    assert_eq!(parsed[0].skill, Skill::Spot);
    assert_eq!(parsed[0].ev_loss_pct, Some(3.5));
    assert_eq!(parsed[0].answer, "a b");
    assert_eq!(parsed[0].item, ItemRef::Seed(42));
}

#[test]
fn malformed_log_lines_are_skipped_with_warnings() {
    let good = format_attempt(&attempt(Skill::Nuts, 1, 5, true));
    let text = format!(
        "{}\n{good}\nnot a log line\n\n1\tdrill\t\tbogus\t1\t1\t1\t0\t\t10\tx\n{good}\n",
        poker_trainer::log::LOG_HEADER
    );
    let (parsed, warnings) = parse_log(&text);
    assert_eq!(parsed.len(), 2);
    assert_eq!(warnings.len(), 2);
}

#[test]
fn response_time_is_capped_at_sixty_seconds() {
    let mut a = attempt(Skill::Nuts, 1, 5, true);
    a.response_ms = 90_000;
    assert!(format_attempt(&a).contains("\t60000\t"));
}

// ----- Baseline -----

#[test]
fn baseline_file_loads_with_the_required_mix() {
    let items = baseline::load().unwrap();
    assert_eq!(items.len(), baseline::BASELINE_SIZE);
    for (skill, count) in baseline::BASELINE_MIX {
        assert_eq!(items.iter().filter(|e| e.item.skill == skill).count(), count, "{skill:?}");
    }
    let mut ids: Vec<&str> = items.iter().map(|e| e.id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), baseline::BASELINE_SIZE);
}

/// The correct answer to a baseline question, worked out from its reveal.
fn correct_answer(item: &Item) -> String {
    let probe = match item.skill {
        Skill::Nuts => "23",
        Skill::Possible => "none",
        Skill::Made => "hc",
        Skill::Beats => "a",
        Skill::Outs => "0",
        Skill::Price | Skill::Hit => "0",
        Skill::Preflop | Skill::Spot | Skill::SpotMw => "f",
        Skill::Equity | Skill::Anchor => "50",
        Skill::BoardType => "rp",
        Skill::Table | Skill::Chain | Skill::BeatsMe | Skill::ReadPrice => "fold",
    };
    let reveal = match &item.question {
        // Two ranks may name no unseen holding, so ask a generic exact holding.
        Question::Nuts { board } => {
            let top = nuts_analysis(board).top_holdings[0];
            return format!("{}{}", top[0], top[1]);
        }
        _ => answer(item, probe).reveal,
    };
    match reveal {
        Reveal::Possible(a) => a.set.code(),
        Reveal::Made { class, .. } => class.code().to_string(),
        Reveal::Beats(a) => a.bucket.code().to_string(),
        Reveal::Outs { analysis, .. } => analysis.outs.len().to_string(),
        Reveal::Price { required, .. } => format!("{}", required * 100.0),
        Reveal::Hit { exact, .. } => format!("{}", exact * 100.0),
        Reveal::Spot { best, .. } => best.code().to_string(),
        other => panic!("unexpected reveal {other:?}"),
    }
}

#[test]
fn baseline_items_all_grade_and_their_correct_answers_pass() {
    for entry in baseline::load().unwrap() {
        let right = correct_answer(&entry.item);
        let outcome = answer(&entry.item, &right);
        assert!(outcome.correct, "{}: answer {right}", entry.id);
    }
}

#[test]
fn frozen_baseline_file_still_loads_and_round_trips() {
    // The generators moved on; the frozen file is what old runs are compared against.
    assert_eq!(baseline::to_text(&baseline::load_file().unwrap()), baseline::BASELINE_TEXT);
}

#[test]
fn baseline_text_round_trips() {
    let items = baseline::load().unwrap();
    assert_eq!(baseline::parse_baseline(&baseline::to_text(&items)).unwrap(), items);
}

#[test]
fn baseline_bad_lines_are_rejected() {
    assert!(baseline::parse_baseline("b01 nuts seed=1").is_err());
    assert!(baseline::parse_baseline("b01 nuts seed=x board=AhKh2c").is_err());
    assert!(baseline::parse_baseline("b01 wat seed=1 board=AhKh2c").is_err());
    assert!(baseline::parse_baseline("b01 nuts seed=1 board=AhAh2c").is_err());
}

fn all_suit_permutations() -> Vec<[u8; 4]> {
    let mut out = Vec::with_capacity(24);
    for a in 0..4u8 {
        for b in 0..4u8 {
            for c in 0..4u8 {
                for d in 0..4u8 {
                    let perm = [a, b, c, d];
                    if (0..4u8).all(|s| perm.contains(&s)) {
                        out.push(perm);
                    }
                }
            }
        }
    }
    out
}

#[test]
fn suit_permutation_preserves_every_answer() {
    let perms = all_suit_permutations();
    assert_eq!(perms.len(), 24);
    for entry in baseline::load().unwrap() {
        // Folding the better action loses exactly 0, so a spot must grade both ways.
        let keys = match entry.item.skill {
            Skill::Spot | Skill::SpotMw => vec!["c", "f"],
            skill => vec![reveal_key(skill)],
        };
        for perm in &perms {
            let shifted = Item {
                skill: entry.item.skill,
                seed: entry.item.seed,
                question: entry.item.question.with_suit_perm(perm),
            };
            for &key in &keys {
                let (base, got) = (answer(&entry.item, key), answer(&shifted, key));
                assert_eq!(base.correct, got.correct, "{} {key} under {perm:?}", entry.id);
                assert_eq!(base.close, got.close, "{} {key} under {perm:?}", entry.id);
                assert_eq!(base.ev_loss_pct, got.ev_loss_pct, "{} {key} under {perm:?}", entry.id);
            }
            if let (Question::Spot(a), Question::Spot(b)) = (&entry.item.question, &shifted.question) {
                let share = |q| spot_analysis(q, entry.item.seed).unwrap().share;
                assert_eq!(share(a), share(b), "{} under {perm:?}", entry.id);
            }
        }
    }
}

#[test]
fn sampled_multiway_numbers_do_not_change_with_suit_labels() {
    // This item's answer used to flip under the permutation [1, 2, 0, 3].
    let item = generate(Skill::SpotMw, 462);
    let Question::Spot(q) = &item.question else { panic!() };
    let base = spot_analysis(q, item.seed).unwrap();
    for perm in all_suit_permutations() {
        let Question::Spot(shifted) = item.question.with_suit_perm(&perm) else { panic!() };
        let got = spot_analysis(&shifted, item.seed).unwrap();
        assert_eq!(base.share, got.share, "{perm:?}");
        assert_eq!(base.sampled, got.sampled, "{perm:?}");
        assert_eq!(base.combos_per_opponent, got.combos_per_opponent, "{perm:?}");
    }
}

fn reveal_key(skill: Skill) -> &'static str {
    match skill {
        Skill::Preflop => "f",
        Skill::Nuts => "AK",
        Skill::Possible => "fs",
        Skill::Made => "top",
        Skill::Beats => "c",
        Skill::Outs => "8",
        Skill::Price | Skill::Hit => "30",
        Skill::Spot | Skill::SpotMw => "f",
        Skill::Equity | Skill::Anchor => "50",
        Skill::BoardType => "rp",
        Skill::Table | Skill::Chain | Skill::BeatsMe | Skill::ReadPrice => "fold",
    }
}

#[test]
fn randomize_shuffles_order_but_keeps_ids() {
    let items = baseline::load().unwrap();
    let mut rng = Rng::new(3);
    let shuffled = baseline::randomize(&items, &mut rng);
    assert_eq!(shuffled.len(), items.len());
    assert_ne!(
        shuffled.iter().map(|e| &e.id).collect::<Vec<_>>(),
        items.iter().map(|e| &e.id).collect::<Vec<_>>()
    );
}

#[test]
fn next_baseline_run_saturates_at_the_largest_id() {
    let mut a = attempt(Skill::Nuts, 1, 0, true);
    a.mode = Mode::Baseline;
    a.baseline_run = Some(4_294_967_295);
    assert_eq!(next_baseline_run(&[a]), 4_294_967_295);
    assert_eq!(next_baseline_run(&[]), 1);
}

#[test]
fn baseline_scores_group_by_run_section_and_step() {
    let row = |run, id: &str, skill, correct| Attempt {
        timestamp_ms: 1,
        mode: Mode::Baseline,
        baseline_run: Some(run),
        skill,
        item: ItemRef::Baseline(id.to_string()),
        gen_version: GEN_VERSION,
        correct,
        close: false,
        ev_loss_pct: None,
        response_ms: 1,
        answer: String::new(),
        extra: Default::default(),
    };
    let log = vec![
        row(1, "b01", Skill::Nuts, true),
        row(1, "b02", Skill::Nuts, false),
        row(1, "b03", Skill::Price, true),
        row(2, "b01", Skill::Nuts, true),
    ];
    let runs = baseline_runs(&log, baseline::BASELINE_SIZE);
    assert_eq!(runs.len(), 2);
    assert_eq!((runs[0].answered, runs[0].ok), (3, 2));
    assert!(!runs[0].complete);
    assert_eq!(runs[0].steps[0].ok, 1);
    assert_eq!(runs[0].steps[1].ok, 1);
}

/// One logged answer per question of the frozen file, all 36 for an old run.
fn old_baseline_rows(run: u32, right: impl Fn(&str) -> bool, keep: impl Fn(&str) -> bool) -> Vec<Attempt> {
    baseline::load_file()
        .unwrap()
        .into_iter()
        .filter(|entry| keep(&entry.id))
        .enumerate()
        .map(|(i, entry)| Attempt {
            timestamp_ms: 1_000 + i as u64,
            mode: Mode::Baseline,
            baseline_run: Some(run),
            skill: entry.item.skill,
            item: ItemRef::Baseline(entry.id.clone()),
            gen_version: 1,
            correct: right(&entry.id),
            close: false,
            ev_loss_pct: None,
            response_ms: 1,
            answer: String::new(),
            extra: Default::default(),
        })
        .collect()
}

#[test]
fn an_old_36_question_run_is_scored_on_the_same_32_questions() {
    // Retired questions marked wrong, everything else right: a true score of 32 of 32.
    let log = old_baseline_rows(1, |id| !baseline::is_retired(id), |_| true);
    assert_eq!(log.len(), 36);
    let runs = baseline_runs(&log, baseline::BASELINE_SIZE);
    assert_eq!((runs[0].answered, runs[0].ok), (32, 32));
    assert!(runs[0].complete, "32 scored rows finish the run");
    assert!(runs[0].sections.iter().all(|s| s.skill != Skill::Beats), "a retired question scored a section");
    assert_eq!(runs[0].steps.iter().map(|s| s.answered).sum::<usize>(), 32);
    // A new run on the 32 has the same total, so the two compare directly.
    let new = old_baseline_rows(2, |_| true, |id| !baseline::is_retired(id));
    let both: Vec<Attempt> = log.iter().chain(&new).cloned().collect();
    let runs = baseline_runs(&both, baseline::BASELINE_SIZE);
    assert_eq!((runs[0].answered, runs[1].answered, runs[1].complete), (32, 32, true));
    // Old rows survive the log.
    let lines: Vec<String> = log.iter().map(format_attempt).collect();
    let text = format!("{}\n{}\n", poker_trainer::log::LOG_HEADER, lines.join("\n"));
    assert_eq!(parse_log(&text).0.len(), 36, "retired rows still load");
}

#[test]
fn a_half_finished_old_run_counts_only_its_32_question_answers() {
    // Half of an old run, including a retired question: it isn't complete yet.
    let half = old_baseline_rows(1, |_| true, |id| matches!(id, "b01" | "b02" | "b13" | "b14"));
    let runs = baseline_runs(&half, baseline::BASELINE_SIZE);
    assert_eq!((runs[0].answered, runs[0].complete), (2, false));
    // Finishing the 32 scored questions completes it, whatever happened to the retired four.
    let rest = old_baseline_rows(1, |_| true, |id| !baseline::is_retired(id) && !matches!(id, "b01" | "b02"));
    let done: Vec<Attempt> = half.into_iter().chain(rest).collect();
    let runs = baseline_runs(&done, baseline::BASELINE_SIZE);
    assert_eq!((runs[0].answered, runs[0].complete), (32, true));
    // A run with only retired answers has nothing to score, and doesn't reuse its number.
    let only_retired = old_baseline_rows(5, |_| true, |id| id == "b13");
    assert!(baseline_runs(&only_retired, baseline::BASELINE_SIZE).is_empty());
    assert_eq!(poker_trainer::progress::next_baseline_run(&only_retired), 6);
    assert_eq!(baseline::load().unwrap().len(), baseline::BASELINE_SIZE);
    assert!(baseline::load().unwrap().iter().all(|e| !baseline::RETIRED_IDS.contains(&e.id.as_str())));
    assert_eq!(baseline::load_file().unwrap().len(), 36, "the frozen file keeps every row");
}
