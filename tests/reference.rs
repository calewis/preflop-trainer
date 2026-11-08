//! Independent reference values from the brief.

use poker_trainer::cards::{parse_cards, parse_holding, Card, Holding};
use poker_trainer::equity::equity_all_in;
use poker_trainer::eval::{evaluate, Category};
use poker_trainer::grade::{hit_chance, outs_analysis};
use poker_trainer::item::HitStreet;
use poker_trainer::money;

fn board(text: &str) -> Vec<Card> {
    parse_cards(text).unwrap()
}

fn hand(text: &str) -> Holding {
    parse_holding(text).unwrap()
}

fn deck() -> Vec<Card> {
    (0..52u8).map(Card::from_index).collect()
}

fn counts_index(category: Category) -> usize {
    category as usize
}

#[test]
#[allow(clippy::needless_range_loop)]
fn five_card_category_counts() {
    let d = deck();
    let mut counts = [0u64; 9];
    for a in 0..52 {
        for b in a + 1..52 {
            for c in b + 1..52 {
                for e in c + 1..52 {
                    for f in e + 1..52 {
                        let rank = evaluate(&[d[a], d[b], d[c], d[e], d[f]]);
                        counts[counts_index(rank.category())] += 1;
                    }
                }
            }
        }
    }
    assert_eq!(counts, [1_302_540, 1_098_240, 123_552, 54_912, 10_200, 5_108, 3_744, 624, 40]);
}

#[test]
#[allow(clippy::needless_range_loop)]
#[ignore = "walks all 133,784,560 seven-card hands"]
fn seven_card_category_counts() {
    let d = deck();
    let mut counts = [0u64; 9];
    let mut cards = [d[0]; 7];
    for a in 0..52 {
        cards[0] = d[a];
        for b in a + 1..52 {
            cards[1] = d[b];
            for c in b + 1..52 {
                cards[2] = d[c];
                for e in c + 1..52 {
                    cards[3] = d[e];
                    for f in e + 1..52 {
                        cards[4] = d[f];
                        for g in f + 1..52 {
                            cards[5] = d[g];
                            for h in g + 1..52 {
                                cards[6] = d[h];
                                counts[counts_index(evaluate(&cards).category())] += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    assert_eq!(
        counts,
        [23_294_460, 58_627_800, 31_433_400, 6_461_620, 6_180_020, 4_047_644, 3_473_184, 224_848, 41_584]
    );
}

fn rank(cards: &str) -> poker_trainer::eval::HandRank {
    evaluate(&board(cards))
}

#[test]
fn wheel_is_a_five_high_straight_below_six_high() {
    let wheel = rank("As2d3h4c5s");
    assert_eq!(wheel.category(), Category::Straight);
    assert_eq!(wheel.primary(), 3);
    assert!(wheel < rank("2d3h4c5s6s"));
    assert!(wheel > rank("AsAd3h4c5s"));
}

#[test]
fn steel_wheel_is_the_lowest_straight_flush() {
    let steel = rank("As2s3s4s5s");
    assert_eq!(steel.category(), Category::StraightFlush);
    assert_eq!(steel.primary(), 3);
    assert!(steel < rank("2s3s4s5s6s"));
    assert!(steel > rank("KhKdKcKs2d"));
}

#[test]
fn kicker_order_breaks_ties_in_order() {
    assert!(rank("AsAcKh2d3d") > rank("AhAd9c7s3d"));
    assert!(rank("AhAdKc7s3d") > rank("AsAcQh9d8d"));
    assert!(rank("AhAdKc7s3d") < rank("AsAcKhJd2d"));
    assert_eq!(rank("AhAdKc7s3d"), rank("AsAcKh7d3c"));
    // High cards compare all five.
    assert!(rank("AhKd9c5s3d") > rank("AsKc9h5d2d"));
    // A sixth and seventh card can't change a tie that the best five decide.
    assert_eq!(rank("AhKdQcJs9d3c2h"), rank("AsKcQhJd9c4c5h"));
}

#[test]
fn two_pair_uses_the_best_kicker_even_from_a_third_pair() {
    let r = rank("KhKd8c8s4d4c2h");
    assert_eq!(r.category(), Category::TwoPair);
    assert_eq!((r.field(0), r.field(1), r.field(2)), (11, 6, 2));
    assert!(rank("KhKd8c8s9d4c2h") > r);
}

#[test]
fn full_house_tie_breaks() {
    // Trips rank first.
    assert!(rank("KhKdKc2s2d") > rank("QhQdQcAsAd"));
    // Then the pair.
    assert!(rank("KhKdKcAsAd") > rank("KsKcKh2s2d"));
    // Two sets of trips: the lower one is the pair.
    let two_trips = rank("9h9d9c5s5d5c2h");
    assert_eq!(two_trips.category(), Category::FullHouse);
    assert_eq!((two_trips.field(0), two_trips.field(1)), (7, 3));
    // Trips plus two pairs: the higher pair fills.
    let two_pairs = rank("9h9d9cAsAd3c3h");
    assert_eq!((two_pairs.field(0), two_pairs.field(1)), (7, 12));
}

#[test]
fn quads_take_the_best_kicker_including_trips() {
    let r = rank("7h7d7c7sKdKcKh");
    assert_eq!(r.category(), Category::Quads);
    assert_eq!((r.field(0), r.field(1)), (5, 11));
}

#[test]
fn flush_needs_five_of_one_suit_and_compares_top_five() {
    let r = rank("Ah9h7h4h2hKdKc");
    assert_eq!(r.category(), Category::Flush);
    assert!(r > rank("Ks9s7s4s3sAdAc"));
}

#[test]
fn hand_names_use_the_right_articles() {
    let cases = [
        ("8h8d8c8sAd", "four eights with an ace kicker"),
        ("8h8d8c8s5d", "four eights with a five kicker"),
        ("KhKdJc7s3d", "a pair of kings with J-7-3 kickers"),
        ("AhKh9h5h2h", "an ace-high flush (A-K-9-5-2)"),
        ("KhJh9h5h2h", "a king-high flush (K-J-9-5-2)"),
        ("9hKdQcJsTd", "a king-high straight"),
        ("8h7d6c5s4d", "an eight-high straight"),
        ("KhKdKc7s7d", "a full house, kings full of sevens"),
        ("KhKd8c8s4d", "two pair, kings and eights, with a four kicker"),
        ("KhKd9c9sAd", "two pair, kings and nines, with an ace kicker"),
        ("KhKdKc7s3d", "three kings with 7-3 kickers"),
        ("8h7h6h5h4h", "an eight-high straight flush"),
        ("9h8h7h6h5h", "a nine-high straight flush"),
        ("AhKhQhJhTh", "a royal flush"),
    ];
    for (hand, expected) in cases {
        assert_eq!(rank(hand).describe(), expected);
    }
}

#[test]
fn exact_flop_equity_matches_reference() {
    let cases = [
        ("AhQh", "KsTd", "Kh7h2c", 45.6),
        ("9h8h", "KsTd", "Kh7h2c", 39.2),
        ("9h8d", "KsTd", "7c6sKd", 28.7),
        ("9h8h", "KsTd", "7h6hKd", 52.4),
        ("AdKc", "9s9c", "Qh7s2d", 25.6),
        ("KsTd", "Kd7c", "Kh7h2c", 15.7),
        ("AsKd", "Kc5s", "Kh8d3c", 88.2),
        ("7c7d", "KsTd", "Kh7h2c", 98.4),
        ("7c7d", "AhQh", "Kh7h2c", 74.4),
        ("AsAd", "QhJh", "9h4h2c", 61.6),
        ("8s8d", "AcKh", "Qd5c2s", 72.8),
        ("9h8h", "AhTd", "Kh7h2h", 71.3),
    ];
    for (hero, villain, flop, expected) in cases {
        let eq = equity_all_in(hand(hero), &board(flop), &[vec![hand(villain)]], 1).unwrap();
        assert!(!eq.sampled);
        let got = eq.share * 100.0;
        assert!(
            (got - expected).abs() <= 0.05,
            "{hero} v {villain} on {flop}: got {got:.3}, expected {expected}"
        );
    }
}

#[test]
fn outs_reference_values() {
    let flush_and_aces = outs_analysis(hand("AhQh"), &board("Kh7h2c"), hand("KsTd"));
    assert_eq!(flush_and_aces.outs.len(), 12);

    let vs_set = outs_analysis(hand("AhQh"), &board("Kh7h2c"), hand("7c7d"));
    assert_eq!(vs_set.outs.len(), 8);
    let two_hearts = Card::parse("2h").unwrap();
    assert!(!vs_set.outs.contains(&two_hearts));
    let tempting = vs_set.tempting.iter().find(|t| t.card == two_hearts);
    let tempting = tempting.expect("2h makes hero a flush but gives the set a full house");
    assert_eq!(tempting.new_rank.category(), Category::Flush);
}

#[test]
fn break_even_reference_values() {
    let cases = [
        (0.25, 16.7),
        (1.0 / 3.0, 20.0),
        (0.5, 25.0),
        (2.0 / 3.0, 28.6),
        (0.75, 30.0),
        (1.0, 33.3),
        (2.0, 40.0),
    ];
    for (fraction, expected) in cases {
        let got = money::break_even(100.0 + 100.0 * fraction, 100.0 * fraction) * 100.0;
        assert!((got - expected).abs() < 0.05, "{fraction}: got {got}, expected {expected}");
    }
}

#[test]
fn hit_chance_reference_values() {
    let cases = [(4, 8.5, 16.5), (8, 17.0, 31.5), (9, 19.1, 35.0), (15, 31.9, 54.1)];
    for (outs, one, both) in cases {
        let got_one = hit_chance(outs, HitStreet::Flop, false) * 100.0;
        let got_both = hit_chance(outs, HitStreet::Flop, true) * 100.0;
        assert!((got_one - one).abs() < 0.05, "{outs} outs one card: {got_one}");
        assert!((got_both - both).abs() < 0.05, "{outs} outs both cards: {got_both}");
    }
}

#[test]
fn next_card_break_even_x_reference() {
    // Exactly 50 * 47 / 9 - 200 = 61.1. The brief's "about 62" comes from
    // rounding the share to 19.1% first (50 / 0.191 - 200 = 61.8).
    let x = money::break_even_x(9.0 / 47.0, 150.0, 50.0).unwrap();
    assert!((x - 61.111).abs() < 0.01, "got {x}");
    assert!((x - 62.0).abs() <= 1.0, "got {x}");
    assert!(money::ev_call(9.0 / 47.0, 150.0, 50.0, x).abs() < 1e-9);
}
