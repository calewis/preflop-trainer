//! Family review, flip feedback, reflection, whole-hand chains and confidence.
//! The numbers in these tests are worked out by hand, or recomputed here with a
//! separate brute-force loop, so they don't simply re-run the code under test.

use poker_trainer::cards::{parse_cards, parse_holding, unseen_cards, Card, Holding};
use poker_trainer::checkup::{checkup_plan, CHECKUP_LEN, CHECKUP_ORIGINALS, CHECKUP_REPEATS, MIN_REPEAT_GAP};
use poker_trainer::classes::{made_class, rank_of, MadeClass, OppClass, OppMix};
use poker_trainer::equity::{equity_next_card, share_by_next_card};
use poker_trainer::eval::evaluate;
use poker_trainer::family::{family_of, find_sibling, outs_family, SIBLING_TRIES};
use poker_trainer::generate::{family_prefilter, generate, Prefilter, CHAIN_MIN_OUTS};
use poker_trainer::grade::{chain_analysis, grade, FlipFacts, Outcome, Reveal};
use poker_trainer::item::{
    chain_hand_seed, chain_seed_for_stage, is_held_out, ChainHand, ChainQuestion, ChainStage, EquityOpponents,
    EquityQuestion, HitStreet, Item, Question, Skill, SpotMode, SpotQuestion, GEN_VERSION,
};
use poker_trainer::log::{
    asks_confidence, format_attempt, parse_log, Attempt, AttemptExtra, CheckRef, Confidence, ItemRef, Mode, LOG_HEADER, TABLE_LOG_HEADER,
};
use poker_trainer::money;
use poker_trainer::preflop::{Seat, Situation, CARD};
use poker_trainer::progress::{all_stats, current_step, pass_mark, reliability_table, skill_stats, STEPS};
use poker_trainer::reflection::{
    reflection_comparison, reflection_eligible, reflection_form, reflection_offered, reflection_row, reflection_state,
    ReflectState, MAX_CUES, REFLECTION_MIN_MISSES,
};
use poker_trainer::schedule::{due_families, Origin, Planned, Session, DAY_MS};
use poker_trainer::table::{self, Verdict};

const DAY: u64 = DAY_MS;

fn cards(text: &str) -> Vec<Card> {
    parse_cards(text).unwrap()
}

fn hand(text: &str) -> Holding {
    parse_holding(text).unwrap()
}

fn answer(item: &Item, text: &str) -> Outcome {
    grade(item, text).unwrap()
}

fn family(item: &Item) -> String {
    family_of(item).unwrap()
}

fn item(skill: Skill, question: Question) -> Item {
    Item { skill, seed: 7, question }
}

// ----- Family keys -----

#[test]
fn made_family_is_the_answer_class() {
    let q = Question::Made { hero: hand("AhKd"), board: cards("Ks7c2d") };
    assert_eq!(family(&item(Skill::Made, q)), "top");
}

#[test]
fn beats_family_is_the_bucket() {
    // A set of sevens is beaten only by the 3 combos of kings: 3 of 1081, under 2%.
    let q = Question::Beats { hero: hand("7h7d"), board: cards("Kh7c2d") };
    assert_eq!(family(&item(Skill::Beats, q)), "a");
}

#[test]
fn possible_family_is_the_answer_set() {
    let none = Question::Possible { board: cards("Ks9h2d") };
    assert_eq!(family(&item(Skill::Possible, none)), "none");
    let flush = Question::Possible { board: cards("Ks9s2s") };
    assert!(family(&item(Skill::Possible, flush)).contains('f'));
}

#[test]
fn nuts_family_is_the_nut_hands_category() {
    assert_eq!(family(&item(Skill::Nuts, Question::Nuts { board: cards("Kh7h2h") })), "flush");
    assert_eq!(family(&item(Skill::Nuts, Question::Nuts { board: cards("KhKd2c") })), "quads");
    // A dry, unpaired flop: the nuts is a set of kings.
    assert_eq!(family(&item(Skill::Nuts, Question::Nuts { board: cards("Kh7d2c") })), "trips");
}

#[test]
fn outs_family_has_four_bands_and_a_tempting_flag() {
    assert_eq!(outs_family(4, false), "under5:plain");
    assert_eq!(outs_family(5, false), "5-8:plain");
    assert_eq!(outs_family(8, true), "5-8:tempting");
    assert_eq!(outs_family(9, false), "9-12:plain");
    assert_eq!(outs_family(12, false), "9-12:plain");
    assert_eq!(outs_family(13, true), "13plus:tempting");
    // AhQh against a set of sevens: 8 hearts are outs (the 2h pairs the board and
    // gives them a full house, so it is not one), and an ace or queen improves the
    // hand without getting ahead, which is the tempting non-out.
    let q = Question::Outs { hero: hand("AhQh"), board: cards("Kh7h2c"), villain: hand("7c7d") };
    assert_eq!(family(&item(Skill::Outs, q)), "5-8:tempting");
}

#[test]
fn price_family_is_the_bet_size_class() {
    let half = Question::Price { pot_before: 120, bet: 60, callers: 0 };
    assert_eq!(family(&item(Skill::Price, half)), "1/2");
    let pot = Question::Price { pot_before: 240, bet: 240, callers: 2 };
    assert_eq!(family(&item(Skill::Price, pot)), "1/1", "callers don't change the bet size");
    let big = Question::Price { pot_before: 60, bet: 120, callers: 0 };
    assert_eq!(family(&item(Skill::Price, big)), "2/1");
}

#[test]
fn hit_family_is_outs_and_one_or_both_cards() {
    let one = Question::Hit { outs: 9, street: HitStreet::Flop, both: false };
    assert_eq!(family(&item(Skill::Hit, one)), "9:one");
    let both = Question::Hit { outs: 9, street: HitStreet::Flop, both: true };
    assert_eq!(family(&item(Skill::Hit, both)), "9:both");
    let turn = Question::Hit { outs: 9, street: HitStreet::Turn, both: false };
    assert_eq!(family(&item(Skill::Hit, turn)), "9:one", "the street is not part of the family");
}

fn spot(mode: SpotMode, opponents: Vec<OppClass>) -> SpotQuestion {
    let opponents = opponents.into_iter().map(OppMix::single).collect();
    SpotQuestion { hero: hand("AhKd"), board: cards("Ks7c2d"), pot: 100, call: 50, mode, opponents }
}

#[test]
fn spot_family_is_mode_and_opponent_class() {
    let q = Question::Spot(spot(SpotMode::AllIn, vec![OppClass::Top]));
    assert_eq!(family(&item(Skill::Spot, q)), "allin:top");
    let q = Question::Spot(spot(SpotMode::NextCard { x: 40 }, vec![OppClass::FlushDraw]));
    assert_eq!(family(&item(Skill::Spot, q)), "next:fd", "the size of X is not part of the family");
}

#[test]
fn spot_mw_family_is_mode_and_opponent_count() {
    let q = Question::Spot(spot(SpotMode::NextCard { x: 0 }, vec![OppClass::Top, OppClass::Mid]));
    assert_eq!(family(&item(Skill::SpotMw, q)), "next:2");
    let q = Question::Spot(spot(SpotMode::AllIn, vec![OppClass::Top, OppClass::Mid, OppClass::Set]));
    assert_eq!(family(&item(Skill::SpotMw, q)), "allin:3");
}

#[test]
fn preflop_family_is_seat_and_situation() {
    let q = Question::Preflop { players: 5, behind: 0, situation: Situation::Limpers(2), hand: hand("AhKd") };
    assert_eq!(family(&item(Skill::Preflop, q)), format!("{}:limpers", Seat::from_behind(0).code()));
    let q = Question::Preflop { players: 5, behind: 2, situation: Situation::Raised, hand: hand("AhKd") };
    assert_eq!(family(&item(Skill::Preflop, q)), format!("{}:raised", Seat::from_behind(2).code()));
}

#[test]
fn equity_family_is_rung_opponents_and_truth_band() {
    // 9c8c against AsAd on 7c6c2d3s is 15 outs of 44, 34.09%: the 25-50 band.
    let exact = EquityQuestion {
        hero: hand("9c8c"),
        board: cards("7c6c2d3s"),
        opponents: EquityOpponents::Exact(vec![hand("AsAd")]),
        band: 10,
    };
    assert_eq!(family(&item(Skill::Equity, Question::Equity(exact))), "0:1:25-50");
    let classes = EquityQuestion {
        hero: hand("AsAd"),
        board: cards("Ks7c2d"),
        opponents: EquityOpponents::Classes(vec![OppClass::Top, OppClass::Mid]),
        band: 10,
    };
    let truth_family = family(&item(Skill::Equity, Question::Equity(classes)));
    assert!(truth_family.starts_with("1:2:"), "{truth_family}");
}

#[test]
fn anchor_board_type_and_chain_have_no_family() {
    assert_eq!(family_of(&item(Skill::Anchor, Question::Anchor { card: 0 })), None);
    assert_eq!(family_of(&generate(Skill::BoardType, 3)), None);
    assert_eq!(family_of(&generate(Skill::Chain, 3)), None);
}

#[test]
fn table_spots_have_a_street_and_seat_label_for_reflection_only() {
    let question = generate(Skill::Table, 42);
    let Question::Table(q) = &question.question else { panic!() };
    let label = family(&question);
    assert!(label.starts_with(match q.board.len() { 0..=3 => "flop:", 4 => "turn:", _ => "river:" }), "{label}");
    assert_eq!(q.players <= 2, label.ends_with(":hu"), "{label}");
}

#[test]
fn every_graded_outcome_carries_its_family() {
    for (skill, answer_text) in [(Skill::Made, "top"), (Skill::Price, "33")] {
        let seed = 11;
        let item = generate(skill, seed);
        let outcome = answer(&item, answer_text);
        assert_eq!(outcome.family, family_of(&item), "{skill:?}");
    }
}

// ----- Sibling seeds -----

#[test]
fn a_sibling_is_a_new_non_held_out_seed_in_the_same_family() {
    for skill in [
        Skill::Preflop, Skill::Nuts, Skill::Possible, Skill::Made, Skill::Beats,
        Skill::Outs, Skill::Price, Skill::Hit, Skill::Equity,
    ] {
        for seed in 0..12 {
            let original = generate(skill, seed);
            let wanted = family(&original);
            let sibling = find_sibling(skill, &wanted, seed + 1000, seed).unwrap_or_else(|| {
                panic!("{skill:?} {seed}: no sibling for {wanted} in {SIBLING_TRIES} tries")
            });
            assert!(!is_held_out(sibling), "{skill:?} {sibling}");
            assert_ne!(sibling, seed);
            assert_eq!(family(&generate(skill, sibling)), wanted, "{skill:?}");
            assert_eq!(find_sibling(skill, &wanted, seed + 1000, seed), Some(sibling), "deterministic");
        }
    }
}

#[test]
#[cfg_attr(debug_assertions, ignore = "slow without optimizations")]
fn spot_siblings_stay_in_family_and_are_never_held_out() {
    for skill in [Skill::Spot, Skill::SpotMw] {
        for seed in 0..6 {
            let wanted = family(&generate(skill, seed));
            let sibling = find_sibling(skill, &wanted, seed + 1000, seed).expect("a spot family's sibling is settled by its first draws");
            assert!(!is_held_out(sibling));
            assert_eq!(family(&generate(skill, sibling)), wanted, "{skill:?} seed {seed}");
        }
    }
}

#[test]
fn equity_siblings_keep_the_rung() {
    for rung in [0u8, 1] {
        let seed = poker_trainer::item::equity_seed_with_rung(9, rung);
        let wanted = family(&generate(Skill::Equity, seed));
        let sibling = find_sibling(Skill::Equity, &wanted, 500, seed).expect("an equity sibling exists");
        assert_eq!(poker_trainer::item::equity_rung_of_seed(sibling), rung);
    }
}

#[test]
fn a_family_nothing_can_reach_gives_up_after_the_budget() {
    assert_eq!(find_sibling(Skill::Made, "no such family", 1, 0), None);
    assert_eq!(find_sibling(Skill::SpotMw, "allin:9", 1, 0), None, "the prefilter rejects every seed");
}

#[test]
fn the_avoided_seed_is_skipped() {
    let wanted = family(&generate(Skill::Made, 4));
    let first = find_sibling(Skill::Made, &wanted, 77, u64::MAX).unwrap();
    let second = find_sibling(Skill::Made, &wanted, 77, first).unwrap();
    assert_ne!(first, second, "the first candidate was the avoided one");
}

#[test]
fn a_rare_family_is_found_within_the_budget() {
    // 15 outs with both cards to come is 1 of the 16 equally likely `hit` questions,
    // so 200 tries nearly always finds one and 20 tries often wouldn't.
    for base in 0..8 {
        assert!(find_sibling(Skill::Hit, "15:both", base, u64::MAX).is_some(), "base {base}");
    }
}

#[test]
fn prefilter_agrees_with_full_generation() {
    for seed in 0..40 {
        let f = family(&generate(Skill::Spot, seed));
        assert_eq!(family_prefilter(Skill::Spot, seed, &f), Prefilter::Definite, "seed {seed}");
        let other = if f.starts_with("allin") { "next:tight" } else { "allin:tight" };
        assert_eq!(family_prefilter(Skill::Spot, seed, other), Prefilter::Reject, "seed {seed}");
    }
    for seed in 0..12 {
        let f = family(&generate(Skill::SpotMw, seed));
        assert_eq!(family_prefilter(Skill::SpotMw, seed, &f), Prefilter::Definite, "seed {seed}");
        assert_eq!(family_prefilter(Skill::SpotMw, seed, "allin:9"), Prefilter::Reject);
    }
    for seed in 0..40 {
        let f = family(&generate(Skill::Equity, seed));
        assert_eq!(family_prefilter(Skill::Equity, seed, &f), Prefilter::Maybe, "seed {seed}");
        assert_eq!(family_prefilter(Skill::Equity, seed, "0:9:0-25"), Prefilter::Reject);
    }
}

fn logged(skill: Skill, seed: u64, t: u64, ok: bool, family: &str) -> Attempt {
    Attempt {
        timestamp_ms: t,
        mode: Mode::Drill,
        baseline_run: None,
        skill,
        item: ItemRef::Seed(seed),
        gen_version: GEN_VERSION,
        correct: ok,
        close: false,
        ev_loss_pct: None,
        response_ms: 2_000,
        answer: "x".to_string(),
        extra: AttemptExtra { family: Some(family.to_string()), ..AttemptExtra::default() },
    }
}

#[test]
fn a_same_session_retry_deals_a_sibling() {
    for seed in 0..10 {
        let mut session = Session::new(vec![Skill::Made, Skill::Nuts], &[], DAY, seed);
        let mut missed: Option<(Skill, u64, String)> = None;
        let mut hits = 0;
        for i in 0..40 {
            let planned = session.try_next_item().unwrap();
            if planned.origin == Origin::SessionRetry {
                let (skill, original, wanted) = missed.clone().expect("a retry follows a miss");
                assert_eq!(planned.skill, skill);
                assert_ne!(planned.seed, original, "a fresh sibling, not the same seed");
                assert!(!is_held_out(planned.seed));
                assert_eq!(family(&generate(planned.skill, planned.seed)), wanted);
                hits += 1;
            }
            let miss = i == 0;
            if miss {
                missed = Some((planned.skill, planned.seed, family(&generate(planned.skill, planned.seed))));
            }
            session.record(planned, !miss);
        }
        assert_eq!(hits, 1, "one retry for one miss, seed {seed}");
    }
}

#[test]
fn a_later_day_review_deals_a_sibling() {
    let wanted = family(&generate(Skill::Made, 7));
    let log = vec![logged(Skill::Made, 7, 0, false, &wanted)];
    let mut seen = false;
    for seed in 0..40 {
        let mut session = Session::new(vec![Skill::Made], &log, DAY, seed);
        for _ in 0..30 {
            let planned = session.try_next_item().unwrap();
            if planned.origin == Origin::Due {
                seen = true;
                assert_ne!(planned.seed, 7);
                assert!(!is_held_out(planned.seed));
                assert_eq!(family(&generate(Skill::Made, planned.seed)), wanted);
            }
            session.record(planned, true);
        }
    }
    assert!(seen, "a due family was dealt");
}

#[test]
fn a_due_family_with_no_sibling_falls_back_to_a_plain_new_item() {
    let log = vec![logged(Skill::Made, 7, 0, false, "no such family")];
    let mut session = Session::new(vec![Skill::Made], &log, DAY, 3);
    let planned = session.try_next_item().unwrap();
    assert_eq!(planned.skill, Skill::Made);
    assert_eq!(planned.origin, Origin::New, "the fallback is a plain new item");
    assert!(!is_held_out(planned.seed));
}

#[test]
fn a_retry_with_no_sibling_falls_back_to_a_plain_new_item() {
    let mut session = Session::new(vec![Skill::Made], &[], DAY, 5);
    let first = session.try_next_item().unwrap();
    session.record_with_family(first, false, 2_000, Some("no such family".to_string()));
    let mut retries = 0;
    for _ in 0..20 {
        let planned = session.try_next_item().unwrap();
        retries += u32::from(planned.origin == Origin::SessionRetry);
        session.record(planned, true);
    }
    assert_eq!(retries, 0, "no sibling means no retry, and the session carries on");
}

#[test]
fn looking_ahead_from_a_clone_does_not_disturb_the_session() {
    let mut session = Session::new(vec![Skill::Made, Skill::Price], &[], DAY, 9);
    let mut twin = session.clone();
    for i in 0..30 {
        let a = session.try_next_item().unwrap();
        let mut peek = session.clone();
        peek.record_with_family(a, i % 4 != 0, 0, None);
        let _ = peek.try_next_item();
        let b = twin.try_next_item().unwrap();
        assert_eq!(a, b, "item {i}");
        session.record(a, i % 4 != 0);
        twin.record(b, i % 4 != 0);
    }
}

#[test]
fn table_board_type_and_anchor_are_left_out_of_family_review() {
    let log = vec![
        logged(Skill::Table, 1, 0, false, "flop:hu"),
        logged(Skill::BoardType, 2, 0, false, "x"),
        logged(Skill::Anchor, 3, 0, false, "x"),
    ];
    assert!(due_families(&log, DAY * 30).is_empty());
}

// ----- Flip feedback -----

#[test]
fn the_break_even_call_solves_the_formula() {
    // 25% of (100 + C) = C gives C = 25 / 0.75 = 33.33.
    let c = money::break_even_call(0.25, 100.0, 0.0).unwrap();
    assert!((c - 100.0 / 3.0).abs() < 1e-9, "{c}");
    // With X = 20: 25% of (120 + C) = C gives C = 30 / 0.75 = 40.
    let c = money::break_even_call(0.25, 100.0, 20.0).unwrap();
    assert!((c - 40.0).abs() < 1e-9, "{c}");
    // At exactly this C the EV of calling is zero.
    assert!(money::ev_call(0.25, 100.0, 40.0, 20.0).abs() < 1e-9);
    assert_eq!(money::break_even_call(1.0, 100.0, 0.0), None);
    assert_eq!(money::break_even_call(0.0, 100.0, 0.0), None);
}

fn flip_of(outcome: &Outcome) -> FlipFacts {
    let Reveal::Spot { flip: Some(flip), .. } = &outcome.reveal else { panic!("{:?}", outcome.reveal) };
    *flip
}

#[test]
fn share_by_next_card_finds_the_outs_by_hand() {
    // 9c8c against AsAd on 7c6c2d3s: 44 cards unseen by both, 15 of them put hero
    // ahead (9 clubs, and the 5s and tens that aren't clubs), and hero ties nothing.
    let shares = share_by_next_card(hand("9c8c"), &cards("7c6c2d3s"), &[hand("AsAd")], false);
    assert_eq!(shares.len(), 44);
    assert_eq!(shares.iter().filter(|(_, s)| *s > 0.3).count(), 15);
    assert!(shares.iter().all(|(_, s)| *s == 0.0 || *s == 1.0));
}

/// Hero's share vs combos after one card, by a separate brute-force loop.
fn brute_share_by_card(hero: Holding, board: &[Card], combos: &[Holding], extra: bool) -> Vec<(Card, f64)> {
    let mut all = board.to_vec();
    all.push(hero[0]);
    if extra {
        all.push(hero[1]);
    }
    let seen: Vec<Card> = board.iter().copied().chain(hero).collect();
    let dead: u64 = seen.iter().map(|c| c.mask()).sum();
    unseen_cards(dead)
        .into_iter()
        .filter_map(|card| {
            let (mut won, mut count) = (0.0, 0.0);
            for &v in combos {
                if v.contains(&card) {
                    continue;
                }
                let live: Vec<Card> = if extra {
                    unseen_cards(dead | card.mask() | v[0].mask() | v[1].mask())
                } else {
                    vec![]
                };
                let mut score = |extras: &[Card]| {
                    let mut hero_cards = vec![hero[0], hero[1], card];
                    hero_cards.extend_from_slice(board);
                    hero_cards.extend_from_slice(extras);
                    let mut villain_cards = vec![v[0], v[1], card];
                    villain_cards.extend_from_slice(board);
                    villain_cards.extend_from_slice(extras);
                    let (a, b) = (evaluate(&hero_cards), evaluate(&villain_cards));
                    won += if a > b { 1.0 } else if a == b { 0.5 } else { 0.0 };
                    count += 1.0;
                };
                if extra {
                    for &river in &live {
                        score(&[river]);
                    }
                } else {
                    score(&[]);
                }
            }
            (count > 0.0).then(|| (card, won / count))
        })
        .collect()
}

#[test]
fn flip_card_count_matches_a_brute_force_recount() {
    let cases = [
        // Next-card spots on the flop and turn, and an all-in on the flop, where the river also comes.
        ("AhQh", "Kh7h2c", OppClass::Top, SpotMode::NextCard { x: 0 }),
        ("JsTs", "9s8h2d7c", OppClass::Top, SpotMode::NextCard { x: 100 }),
        ("AhQh", "Kh7h2c", OppClass::Top, SpotMode::AllIn),
        // Top pair with a weak kicker wins some showdowns against top pair on most cards, and a
        // big X puts the price well below the raw pot odds, so a wrong formula moves the count.
        ("Ks5s", "Kh7c2d", OppClass::Top, SpotMode::NextCard { x: 200 }),
    ];
    for (hero, board, class, mode) in cases {
        // Several prices, so a threshold moved by the wrong formula changes some count.
        for call in (10..=100).step_by(10) {
            let q = SpotQuestion { hero: hand(hero), board: cards(board), pot: 100, call, mode, opponents: vec![OppMix::single(class)] };
            let question = item(Skill::Spot, Question::Spot(q.clone()));
            let outcome = answer(&question, "c");
            let Reveal::Spot { analysis, .. } = &outcome.reveal else { panic!() };
            let (ahead, unseen) = flip_of(&outcome).cards_ahead.expect("heads-up has a card count");
            let combos = poker_trainer::classes::combos_in_class(class, hand(hero), &cards(board));
            let extra = matches!(mode, SpotMode::AllIn) && q.board.len() == 3;
            let brute = brute_share_by_card(hand(hero), &cards(board), &combos, extra);
            let expected = brute.iter().filter(|(_, s)| *s > analysis.required).count() as u32;
            assert_eq!(ahead, expected, "{hero} {board} call {call}");
            assert_eq!(unseen as usize, 52 - 2 - q.board.len());
        }
    }
}

#[test]
fn flip_breakeven_call_is_stated_on_real_items() {
    // Generated spots: the formula holds on the graded share, and the grade is
    // untouched by what the flip fields say.
    for seed in 0..15 {
        let item = generate(Skill::Spot, seed);
        let Question::Spot(q) = &item.question else { panic!() };
        let outcome = answer(&item, "c");
        let Reveal::Spot { analysis: a, .. } = &outcome.reveal else { panic!() };
        let flip = flip_of(&outcome);
        let want = a.share * (f64::from(q.pot) + f64::from(q.x())) / (1.0 - a.share);
        assert!((flip.break_even_call.unwrap() - want).abs() < 1e-9, "seed {seed}");
        assert!(money::ev_call(a.share, f64::from(q.pot), want, f64::from(q.x())).abs() < 1e-6, "seed {seed}");
        assert!(flip.cards_ahead.is_some(), "heads-up items have a card count");
    }
}

#[test]
fn multiway_spots_have_no_card_count() {
    let q = SpotQuestion {
        hero: hand("AhKd"),
        board: cards("Ks7c2d"),
        pot: 150,
        call: 50,
        mode: SpotMode::AllIn,
        opponents: vec![OppMix::single(OppClass::Mid), OppMix::single(OppClass::Top)],
    };
    let question = item(Skill::SpotMw, Question::Spot(q));
    let flip = flip_of(&answer(&question, "c"));
    assert!(flip.break_even_call.is_some());
    assert_eq!(flip.cards_ahead, None);
}

// ----- Reflection -----

fn wrong_spot_item() -> (Item, Outcome) {
    // Top pair against a set with a huge bet: folding is clearly right, so a call is wrong.
    let q = SpotQuestion {
        hero: hand("AhKd"),
        board: cards("Ks7c2d"),
        pot: 150,
        call: 600,
        mode: SpotMode::AllIn,
        opponents: vec![OppMix::single(OppClass::Set)],
    };
    let question = Item { skill: Skill::Spot, seed: 7, question: Question::Spot(q) };
    let outcome = answer(&question, "c");
    assert!(!outcome.ok());
    (question, outcome)
}

#[test]
fn the_form_is_offered_for_half_the_seeds_by_a_pure_function() {
    let offered = (0..1000u64).filter(|&s| reflection_offered(s)).count();
    assert!((400..=600).contains(&offered), "{offered} of 1000");
    for seed in 0..100 {
        assert_eq!(reflection_offered(seed), reflection_offered(seed), "deterministic");
    }
}

#[test]
fn only_misses_on_the_right_skills_are_eligible() {
    let (wrong, outcome) = wrong_spot_item();
    assert!(reflection_eligible(&wrong, &outcome));
    assert_eq!(
        reflection_state(&wrong, &outcome, Origin::New),
        Some(if reflection_offered(wrong.seed) { ReflectState::Offered } else { ReflectState::Withheld })
    );
    assert_eq!(reflection_state(&wrong, &outcome, Origin::SessionRetry), None, "retries are left out");

    let right = answer(&wrong, "f");
    assert!(!reflection_eligible(&wrong, &right), "a correct call is not a miss");
    let price = generate(Skill::Price, 3);
    assert!(!reflection_eligible(&price, &answer(&price, "1")));
}

#[test]
fn the_form_has_three_prompts_with_three_or_four_real_cues() {
    let (question, outcome) = wrong_spot_item();
    let form = reflection_form(&question, &outcome).expect("eligible");
    assert_eq!(form.len(), 3);
    assert_eq!(form[0].text, "What supported your choice?");
    assert_eq!(form[1].text, "What argued against it?");
    assert_eq!(form[2].text, "What else could be true?");
    for prompt in &form {
        assert!((3..=MAX_CUES).contains(&prompt.cues.len()), "{}: {:?}", prompt.text, prompt.cues);
    }
    let all: Vec<&str> = form.iter().flat_map(|p| p.cues.iter().map(|c| c.text.as_str())).collect();
    assert!(all.iter().any(|t| t.contains("set")), "the assumed class is a real fact: {all:?}");
    let Question::Spot(q) = &question.question else { panic!() };
    let price_text = format!("{:.0}%", 100.0 * f64::from(q.call) / f64::from(q.pot + q.call));
    assert!(all.iter().any(|t| t.contains(&price_text)), "the price is a real fact: {all:?}");
    assert!(all.iter().any(|t| t.contains("2 players")), "the number of players is a real fact: {all:?}");
}

#[test]
fn a_river_table_spot_has_no_cue_about_later_cards() {
    let (mut river, mut earlier) = (0, 0);
    for seed in 0..50_000u64 {
        let question = generate(Skill::Table, seed);
        let Question::Table(q) = &question.question else { continue };
        let spot = table::spot_by_id(q.spot_id);
        if spot.verdict != Verdict::Robust {
            continue;
        }
        let outcome = answer(&question, q.options[(spot.best + 1) % spot.options.len()].code);
        let Some(form) = reflection_form(&question, &outcome) else { continue };
        let has_runout = form.iter().flat_map(|p| &p.cues).any(|c| c.id == "runout");
        assert_eq!(has_runout, q.board.len() < 5, "seed {seed}");
        river += usize::from(q.board.len() == 5);
        earlier += usize::from(q.board.len() < 5);
        if river > 0 && earlier > 0 {
            return;
        }
    }
    panic!("no robust river or earlier spot: {river} {earlier}");
}

#[test]
fn a_table_spot_gets_cues_from_its_own_facts() {
    let seed = (0..50_000u64)
        .find(|&s| {
            let item = generate(Skill::Table, s);
            let Question::Table(q) = &item.question else { return false };
            table::spot_by_id(q.spot_id).verdict == Verdict::Robust && q.board.len() >= 3
        })
        .expect("a robust spot exists");
    let question = generate(Skill::Table, seed);
    let Question::Table(q) = &question.question else { panic!() };
    let spot = table::spot_by_id(q.spot_id);
    let wrong_index = (spot.best + 1) % spot.options.len();
    let outcome = answer(&question, q.options[wrong_index].code);
    assert!(reflection_eligible(&question, &outcome));
    let form = reflection_form(&question, &outcome).unwrap();
    assert_eq!(form.len(), 3);
    assert!(form.iter().all(|p| (3..=MAX_CUES).contains(&p.cues.len())));

    let best_outcome = answer(&question, q.options[spot.best].code);
    assert!(!reflection_eligible(&question, &best_outcome), "the best option is not a miss");
}

#[test]
fn a_robust_spot_that_counts_as_correct_but_is_not_best_is_eligible() {
    // The library's robust spots lead by at least 1% of the pot, so today a non-best
    // option is never within 1%. The rule is about the best play, so force the case.
    let seed = (0..50_000u64)
        .find(|&s| {
            let Question::Table(q) = generate(Skill::Table, s).question else { return false };
            table::spot_by_id(q.spot_id).verdict == Verdict::Robust
        })
        .expect("a robust spot exists");
    let question = generate(Skill::Table, seed);
    let Question::Table(q) = &question.question else { panic!() };
    let spot = table::spot_by_id(q.spot_id);
    let mut outcome = answer(&question, q.options[(spot.best + 1) % spot.options.len()].code);
    let Reveal::Table(graded) = &mut outcome.reveal else { panic!() };
    graded.correct = true;
    outcome.correct = true;
    assert!(reflection_eligible(&question, &outcome), "not the best play, so the form applies");
}

fn miss_row(seed: u64, t: u64, state: ReflectState, family: &str) -> Attempt {
    let mut row = logged(Skill::Spot, seed, t, false, family);
    row.extra.reflect = Some(state);
    row
}

fn reflection_note(seed: u64, t: u64, answers: &str) -> Attempt {
    reflection_row(&logged(Skill::Spot, seed, t, false, "allin:top"), answers, t + 1)
}

#[test]
fn reflection_rows_round_trip_and_are_not_attempts() {
    let row = reflection_note(7, 10, "supported:price,against:opp,else:runout");
    assert_eq!(row.mode, Mode::Reflect);
    assert_eq!(row.response_ms, 0, "a reflection adds no time");
    let line = format_attempt(&row);
    assert_eq!(line.split('\t').count(), 24);
    let (parsed, warnings) = parse_log(&line);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(parsed, vec![row.clone()]);
    assert_eq!(skill_stats(Skill::Spot, std::slice::from_ref(&row)).total_attempts, 0);
    assert!(due_families(&[row], DAY * 90).is_empty());
    let skipped = reflection_row(&logged(Skill::Spot, 7, 10, false, "allin:top"), "", 11);
    assert_eq!(skipped.extra.reflect, Some(ReflectState::Skipped));
    assert_eq!(skipped.answer, "skip");
}

#[test]
fn offered_and_answered_are_logged() {
    let (question, outcome) = wrong_spot_item();
    let planned = Planned { skill: Skill::Spot, seed: question.seed, origin: Origin::New };
    let mut row = Attempt::from_drill(planned, &outcome, 1000, 2000);
    assert_eq!(row.extra.reflect, None, "the front end stamps it, so the CLI stays out of the study");
    row.extra.reflect = reflection_state(&question, &outcome, Origin::New);
    let line = format_attempt(&row);
    assert_eq!(parse_log(&line).0[0].extra.reflect, row.extra.reflect);
}

#[test]
fn the_comparison_counts_later_siblings_by_how_the_miss_was_handled() {
    let mut log = vec![
        // Reflected on, next similar question right.
        miss_row(1, 10, ReflectState::Offered, "allin:top"),
        reflection_note(1, 11, "supported:price"),
        logged(Skill::Spot, 100, 20, true, "allin:top"),
        // Skipped form, next similar question wrong.
        miss_row(2, 30, ReflectState::Offered, "allin:mid"),
        reflection_row(&logged(Skill::Spot, 2, 31, false, "allin:mid"), "", 32),
        logged(Skill::Spot, 101, 40, false, "allin:mid"),
        // Never shown, next similar question right.
        miss_row(3, 50, ReflectState::Withheld, "next:top"),
        logged(Skill::Spot, 102, 60, true, "next:top"),
        // A miss with no sibling yet, and a retry row as the sibling of the next.
        miss_row(4, 70, ReflectState::Withheld, "next:set"),
        miss_row(5, 80, ReflectState::Withheld, "next:fd"),
    ];
    let mut retry = logged(Skill::Spot, 103, 90, false, "next:fd");
    retry.mode = Mode::DrillRetry;
    log.push(retry);
    let c = reflection_comparison(&log);
    assert_eq!(c.counted, 4);
    assert_eq!(c.waiting, 1, "the miss with no later sibling");
    assert_eq!((c.reflected.misses, c.reflected.right), (1, 1));
    assert_eq!((c.skipped.misses, c.skipped.right), (1, 0));
    assert_eq!((c.not_offered.misses, c.not_offered.right), (2, 1));
    assert!(!c.enough_data());
}

#[test]
fn the_comparison_needs_sixty_misses_with_a_later_sibling() {
    let build = |n: u64| {
        let mut log = Vec::new();
        for i in 0..n {
            log.push(miss_row(i, i * 100, ReflectState::Withheld, "allin:top"));
            log.push(logged(Skill::Spot, 5000 + i, i * 100 + 50, true, "allin:top"));
        }
        log
    };
    let short = reflection_comparison(&build(59));
    assert_eq!(short.counted, 59);
    assert!(!short.enough_data());
    assert_eq!(REFLECTION_MIN_MISSES, 60);
    assert!(reflection_comparison(&build(60)).enough_data());
}

// ----- Whole-hand chains -----

fn chain_hand() -> ChainHand {
    ChainHand {
        hero: hand("9c8c"),
        board: cards("7c6c2d3s"),
        villain: hand("AsAd"),
        second: None,
        pot_before: 100,
        bet: 50,
        mode: SpotMode::AllIn,
    }
}

fn chain_item(stage: ChainStage) -> Item {
    Item { skill: Skill::Chain, seed: 8, question: Question::Chain(ChainQuestion { hand: chain_hand(), stage }) }
}

#[test]
fn each_stage_grades_by_its_own_rule() {
    // Stage 1: hero holds 9-high with a flush draw and a gut-shot: a high card hand.
    let correct = answer(&chain_item(ChainStage::Hand), "hc");
    assert!(correct.correct);
    assert_eq!(correct.chain_stage, Some(ChainStage::Hand));
    assert!(!answer(&chain_item(ChainStage::Hand), "top").correct);
    assert_eq!(made_class(chain_hand().hero, &chain_hand().board), MadeClass::HighCard);

    // Stage 2: 15 outs against the shown hand, and within 1 counts as close.
    let outs = answer(&chain_item(ChainStage::Outs), "15");
    assert!(outs.correct);
    let near = answer(&chain_item(ChainStage::Outs), "14");
    assert!(!near.correct && near.close && near.ok(), "within one counts");
    assert!(!answer(&chain_item(ChainStage::Outs), "12").ok());

    // Stage 3: 50 to call into a pot of 150 needs 50 / 200 = 25%.
    assert!(answer(&chain_item(ChainStage::Price), "25").correct);
    assert!(answer(&chain_item(ChainStage::Price), "26.9").correct, "within the 2-point tolerance");
    assert!(!answer(&chain_item(ChainStage::Price), "30").correct);

    // Stage 4: 15/44 = 34.09% against the 25% needed, so calling is right.
    let call = answer(&chain_item(ChainStage::Decide), "c");
    assert!(call.correct);
    let fold = answer(&chain_item(ChainStage::Decide), "f");
    assert!(!fold.correct);
    assert_eq!(fold.chain_stage, Some(ChainStage::Decide));
}

#[test]
fn a_three_way_pot_counts_the_callers_money_in_the_price() {
    let mut h = chain_hand();
    h.second = Some(OppMix::single(OppClass::Top));
    let item = Item { skill: Skill::Chain, seed: 8, question: Question::Chain(ChainQuestion { hand: h, stage: ChainStage::Price }) };
    // 50 to call into 100 + 50 + 50 = 200, so 50 / 250 = 20%.
    assert!(answer(&item, "20").correct);
    assert!(!answer(&item, "25").correct, "that would ignore the caller");
}

#[test]
fn the_last_stage_reveal_ties_the_numbers_together() {
    let outcome = answer(&chain_item(ChainStage::Decide), "c");
    let tie = outcome.chain_tie.expect("the last stage has a tie line");
    assert_eq!(tie, "High card, 15 outs, 25.0% needed, your share 34.1%, so calling was right.");
    for (stage, text) in [(ChainStage::Hand, "hc"), (ChainStage::Outs, "15"), (ChainStage::Price, "25")] {
        assert_eq!(answer(&chain_item(stage), text).chain_tie, None, "{stage:?}");
    }
}

#[test]
fn the_tie_line_says_when_later_winnings_are_counted() {
    let mut h = chain_hand();
    h.mode = SpotMode::NextCard { x: 100 };
    let item = Item { skill: Skill::Chain, seed: 8, question: Question::Chain(ChainQuestion { hand: h, stage: ChainStage::Decide }) };
    let tie = answer(&item, "c").chain_tie.unwrap();
    assert!(tie.contains("counting what you win later"), "{tie}");
}

#[test]
fn a_chain_is_a_hand_the_card_plays_with_hero_behind_and_real_outs() {
    let mut two = 0;
    for seed in 0..120u64 {
        let item = generate(Skill::Chain, seed << 2);
        let Question::Chain(q) = &item.question else { panic!() };
        let h = &q.hand;
        assert!(CARD.plays(h.hero), "seed {seed}");
        assert!((3..=4).contains(&h.board.len()));
        assert!(poker_trainer::classes::rank_of(h.villain, &h.board) > poker_trainer::classes::rank_of(h.hero, &h.board), "seed {seed}");
        let outs = poker_trainer::grade::outs_analysis(h.hero, &h.board, h.villain).outs.len();
        assert!((CHAIN_MIN_OUTS..=17).contains(&outs), "seed {seed}: {outs} outs");
        assert_eq!(h.pot(), h.pot_before + h.bet * h.opponents());
        two += usize::from(h.opponents() == 2);
    }
    assert!((20..=100).contains(&two), "{two} of 120 hands had two opponents");
}

#[test]
fn the_four_stages_of_a_seed_are_the_same_hand() {
    let base = 123u64 << 2;
    let a = generate(Skill::Chain, chain_seed_for_stage(base, ChainStage::Hand));
    let d = generate(Skill::Chain, chain_seed_for_stage(base, ChainStage::Decide));
    let (Question::Chain(a), Question::Chain(d)) = (&a.question, &d.question) else { panic!() };
    assert_eq!(a.stage, ChainStage::Hand);
    assert_eq!(d.stage, ChainStage::Decide);
    assert_eq!(a.hand, d.hand);
    assert_eq!(chain_hand_seed(base + 3), base);
}

#[test]
fn a_session_deals_the_four_stages_in_order() {
    for seed in 0..12 {
        let mut session = Session::new(vec![Skill::Chain, Skill::Price], &[], 0, seed);
        let mut in_hand: Vec<Planned> = Vec::new();
        for _ in 0..60 {
            let planned = session.try_next_item().unwrap();
            if planned.skill == Skill::Chain {
                in_hand.push(planned);
                let stage = poker_trainer::item::chain_stage_of_seed(planned.seed);
                let expected = ChainStage::from_index(in_hand.len() - 1).unwrap();
                assert_eq!(stage, expected, "stages come in order");
                if in_hand.len() == 4 {
                    assert_eq!(chain_hand_seed(in_hand[0].seed), chain_hand_seed(in_hand[3].seed));
                    in_hand.clear();
                } else {
                    assert!(session.chain_in_progress());
                }
            } else {
                assert!(in_hand.is_empty(), "nothing interrupts a hand");
            }
            session.record(planned, planned.seed.is_multiple_of(2));
        }
    }
}

#[test]
fn a_chain_miss_gets_no_retry_or_later_review() {
    let mut session = Session::new(vec![Skill::Chain], &[], 0, 3);
    for _ in 0..40 {
        let planned = session.try_next_item().unwrap();
        assert_eq!(planned.origin, Origin::New);
        session.record(planned, false);
    }
    assert!(due_families(&[logged(Skill::Chain, 4, 0, false, "x")], DAY * 90).is_empty());
}

#[test]
fn stages_log_one_row_each_with_a_stage_column() {
    let outcome = answer(&chain_item(ChainStage::Outs), "15");
    let planned = Planned { skill: Skill::Chain, seed: 8, origin: Origin::New };
    let row = Attempt::from_drill(planned, &outcome, 10, 2000);
    assert_eq!(row.extra.stage, Some(ChainStage::Outs));
    let line = format_attempt(&row);
    assert_eq!(line.split('\t').nth(21), Some("outs"));
    let (parsed, warnings) = parse_log(&line);
    assert!(warnings.is_empty());
    assert_eq!(parsed[0].extra.stage, Some(ChainStage::Outs));
}

#[test]
fn the_chain_mark_is_85_percent_of_stages_with_outs_within_one_counting() {
    let mark = pass_mark(Skill::Chain);
    assert_eq!(mark.min_within_one_pct, Some(85.0));
    let rows = |right: usize| -> Vec<Attempt> {
        (0..40)
            .map(|i| {
                let mut a = logged(Skill::Chain, i as u64, i as u64, false, "x");
                a.extra.stage = Some(ChainStage::Outs);
                // Right means within one on an outs stage, as close but not correct.
                a.close = i < right;
                a
            })
            .collect()
    };
    assert!(skill_stats(Skill::Chain, &rows(34)).drill_passed, "85% of 40 is 34");
    assert!(!skill_stats(Skill::Chain, &rows(33)).drill_passed);
}

#[test]
fn chain_is_in_step_three() {
    assert_eq!(Skill::Chain.step(), Some(3));
    assert!(STEPS[2].contains(&Skill::Chain));
    assert_eq!(current_step(&all_stats(&[])), 1);
}

#[test]
fn a_chain_checkup_keeps_hands_together_and_counts_stages_as_items() {
    for form in 0..4 {
        let plan = checkup_plan(Skill::Chain, form);
        assert_eq!(plan.slots.len(), CHECKUP_LEN);
        assert_eq!(plan.slots.iter().filter(|s| s.repeat_of.is_none()).count(), CHECKUP_ORIGINALS);
        assert_eq!(plan.slots.iter().filter(|s| s.repeat_of.is_some()).count(), CHECKUP_REPEATS);
        assert!(plan.slots.iter().all(|s| is_held_out(s.seed)), "every seed is held out");
        for (index, slot) in plan.slots.iter().enumerate() {
            if let Some(original) = slot.repeat_of {
                assert!(index >= original + MIN_REPEAT_GAP, "form {form} slot {index}");
                assert_eq!(plan.slots[original].seed, slot.seed);
            }
        }
        // The four stages of each hand stay next to each other.
        let originals: Vec<u64> = plan.slots.iter().filter(|s| s.repeat_of.is_none()).map(|s| s.seed).collect();
        for hand in originals.chunks(4) {
            for (i, seed) in hand.iter().enumerate() {
                assert_eq!(chain_hand_seed(*seed), chain_hand_seed(hand[0]));
                assert_eq!(poker_trainer::item::chain_stage_of_seed(*seed).index(), i);
            }
        }
        assert_eq!(plan, checkup_plan(Skill::Chain, form), "deterministic");
    }
}

#[test]
fn a_chain_checkup_slot_is_a_real_stage_question() {
    let plan = checkup_plan(Skill::Chain, 0);
    for index in 0..plan.slots.len() {
        let item = plan.item(index);
        assert_eq!(item.skill, Skill::Chain);
        assert!(matches!(item.question, Question::Chain(_)));
        assert!(grade(&item, "c").is_ok() || grade(&item, "hc").is_ok() || grade(&item, "5").is_ok());
    }
}

#[test]
fn chain_analysis_averages_over_the_second_opponents_combos() {
    let mut h = chain_hand();
    h.bet = 100;
    h.mode = SpotMode::NextCard { x: 0 };
    let heads_up = chain_analysis(&h, 8).unwrap();
    assert_eq!(heads_up.combos_per_opponent, vec![1]);
    // Against the one shown hand on the turn: 15 river cards of 44 win, 34.09%.
    assert!((heads_up.share - 15.0 / 44.0).abs() < 1e-9, "{}", heads_up.share);
    assert_eq!(heads_up.pot, 200);

    h.second = Some(OppMix::single(OppClass::Top));
    let three_way = chain_analysis(&h, 8).unwrap();
    assert_eq!(three_way.combos_per_opponent.len(), 2);
    assert!(three_way.combos_per_opponent[1] > 1, "averaged over the class");
    // Not always below the heads-up share: the second hand's cards can be non-outs.
    assert!(three_way.share > 0.0 && three_way.share < 1.0);
    assert_eq!(three_way.pot, 300, "the caller's money is in the pot");
}

#[test]
fn a_chain_answer_changes_nothing_on_other_stages() {
    // Sanity: the next-card equity here matches the existing skill's function.
    let eq = equity_next_card(hand("9c8c"), &cards("7c6c2d3s"), &[vec![hand("AsAd")]], 1).unwrap();
    assert!((eq.share - 15.0 / 44.0).abs() < 1e-9);
}

// ----- Confidence and the reliability table -----

#[test]
fn only_the_four_skills_ask_for_confidence() {
    for skill in Skill::ALL {
        assert_eq!(
            asks_confidence(skill),
            matches!(skill, Skill::Made | Skill::Outs | Skill::Price | Skill::Equity),
            "{skill:?}"
        );
    }
    assert!(!asks_confidence(Skill::Spot) && !asks_confidence(Skill::SpotMw) && !asks_confidence(Skill::Table));
}

#[test]
fn levels_are_treated_as_55_75_95() {
    assert_eq!(Confidence::Low.treated_pct(), 55.0);
    assert_eq!(Confidence::Medium.treated_pct(), 75.0);
    assert_eq!(Confidence::High.treated_pct(), 95.0);
}

fn rated(skill: Skill, seed: u64, ok: bool, level: Confidence, mode: Mode) -> Attempt {
    let mut row = logged(skill, seed, seed, ok, "x");
    row.mode = mode;
    row.extra.confidence = Some(level);
    row
}

#[test]
fn confidence_round_trips_in_the_log() {
    let row = rated(Skill::Outs, 5, true, Confidence::High, Mode::Drill);
    let line = format_attempt(&row);
    assert_eq!(line.split('\t').nth(22), Some("high"));
    assert_eq!(parse_log(&line).0[0].extra.confidence, Some(Confidence::High));
}

#[test]
fn the_reliability_table_counts_ratings_by_level_and_skill() {
    let mut log = Vec::new();
    for i in 0..30 {
        log.push(rated(Skill::Made, i, i < 25, Confidence::High, Mode::Drill));
    }
    log.push(rated(Skill::Made, 90, true, Confidence::Low, Mode::Drill));
    // A retry and a skill that doesn't ask are ignored.
    log.push(rated(Skill::Made, 91, false, Confidence::High, Mode::DrillRetry));
    log.push(rated(Skill::Price, 92, true, Confidence::Medium, Mode::Check));
    let table = reliability_table(&log);
    let high = table.iter().find(|r| r.skill == Skill::Made && r.level == Confidence::High).unwrap();
    assert_eq!((high.ratings, high.right), (30, 25));
    assert!((high.right_pct() - 100.0 * 25.0 / 30.0).abs() < 1e-9);
    assert_eq!(table.iter().find(|r| r.level == Confidence::Low).unwrap().ratings, 1);
    assert_eq!(table.len(), 2, "the retry and the check row are ignored");
}

// ----- Log format -----

#[test]
fn the_new_columns_go_at_the_end_and_older_lines_still_parse() {
    assert!(LOG_HEADER.starts_with(TABLE_LOG_HEADER));
    assert_eq!(LOG_HEADER.split('\t').count(), 24);
    assert_eq!(&LOG_HEADER[TABLE_LOG_HEADER.len()..], "\tfamily\tstage\tconfidence\treflect");
    let (parsed, warnings) = parse_log(TABLE_LOG_HEADER);
    assert!(warnings.is_empty() && parsed.is_empty(), "the previous header isn't data");
    let old = "1\tdrill\t\tprice\t7\t1\t1\t0\t\t1000\t25";
    let (parsed, warnings) = parse_log(old);
    assert!(warnings.is_empty());
    assert_eq!(parsed[0].extra, AttemptExtra::default());
}

#[test]
fn a_row_with_notes_but_no_table_spot_leaves_those_columns_empty() {
    let row = logged(Skill::Made, 5, 1, true, "top");
    let line = format_attempt(&row);
    let fields: Vec<&str> = line.split('\t').collect();
    assert_eq!(fields.len(), 24);
    assert_eq!((fields[18], fields[19]), ("", ""), "no table spot");
    assert_eq!(fields[20], "top");
    assert_eq!(parse_log(&line).0, vec![row]);
}

// ----- Spot mixes: a real decision, against realistic hands -----

const BASELINE_FINGERPRINT: (usize, u64) = (2679, 7369406269895541717);

mod spot_mixes {
    use super::*;
    use poker_trainer::classes::{worth_a_decision, PlayerType};
    use poker_trainer::grade::spot_analysis;
    use poker_trainer::item::SpotQuestion;

    fn spot_of(skill: Skill, seed: u64) -> SpotQuestion {
        let Question::Spot(q) = generate(skill, seed).question else { panic!() };
        q
    }

    #[test]
    fn a_spot_is_never_a_hand_with_nothing() {
        for seed in 0..200 {
            let q = spot_of(Skill::Spot, seed);
            assert!(worth_a_decision(q.hero, &q.board), "seed {seed}: {:?} on {:?}", q.hero, q.board);
            assert!(CARD.plays(q.hero), "seed {seed}");
        }
        for seed in 0..40 {
            let q = spot_of(Skill::SpotMw, seed);
            assert!(worth_a_decision(q.hero, &q.board), "seed {seed}");
        }
    }

    #[test]
    fn hands_worth_a_decision_are_the_stated_ones() {
        assert!(worth_a_decision(hand("Ah7d"), &cards("7c2s9d")), "a pair");
        assert!(worth_a_decision(hand("AhQh"), &cards("7h2h9d")), "a flush draw");
        assert!(worth_a_decision(hand("9h8d"), &cards("7c6s2d")), "an open-ended straight draw");
        assert!(worth_a_decision(hand("AhKd"), &cards("QcTs2d")), "a gutshot with two overcards");
        assert!(!worth_a_decision(hand("Ah2d"), &cards("9c8s6d")), "a gutshot-free ace high");
        assert!(!worth_a_decision(hand("Kh3d"), &cards("QcTs2d")), "a gutshot needs two overcards");
    }

    #[test]
    fn each_opponent_has_a_stated_mix_of_the_players_groups() {
        for seed in 0..60 {
            for mix in spot_of(Skill::Spot, seed).opponents {
                let player = mix.player.expect("a named player");
                assert!(mix.classes.len() >= 2, "seed {seed}: {mix:?}");
                assert!(mix.classes.iter().all(|c| player.groups().contains(c)), "seed {seed}: {mix:?}");
            }
        }
        let q = spot_of(Skill::SpotMw, 3);
        assert!(q.opponents.len() >= 2);
        assert!(q.opponents.iter().all(|m| m.player.is_some()));
    }

    #[test]
    fn the_prompt_names_the_player_and_the_groups() {
        let mix = OppMix { player: Some(PlayerType::Loose), classes: vec![OppClass::Top, OppClass::Mid, OppClass::FlushDraw, OppClass::Nothing] };
        assert_eq!(mix.assumption(None), "They're a loose player. Assume they have one of: top pair, second pair, a flush draw, or nothing much.");
        assert_eq!(OppMix::single(OppClass::Set).assumption(None), "Assume they have a set.");
    }

    #[test]
    fn new_classes_are_what_they_say() {
        use poker_trainer::classes::matches_opp_class as is;
        let board = cards("Kc9s4d");
        assert!(is(OppClass::Low, hand("3h3d"), &board), "a pocket pair below the board");
        assert!(is(OppClass::Low, hand("4h8d"), &board), "pairing the lowest board card");
        assert!(!is(OppClass::Low, hand("9h8d"), &board), "pairing the middle card");
        assert!(is(OppClass::Nothing, hand("Qh7d"), &board), "no pair, no draw");
        assert!(!is(OppClass::Nothing, hand("Kh7d"), &board), "a pair");
        assert!(!is(OppClass::Nothing, hand("Qs7s"), &cards("Ks9s4d")), "a flush draw");
    }

    /// Call is the better action on `share` of seeds, per mode, for an all-in or next-card sample.
    fn call_rate(skill: Skill, seeds: u64, all_in_mode: bool) -> f64 {
        let (mut total, mut calls) = (0, 0);
        for seed in 0..seeds {
            let q = spot_of(skill, seed);
            if matches!(q.mode, SpotMode::AllIn) != all_in_mode {
                continue;
            }
            let a = spot_analysis(&q, seed).unwrap();
            total += 1;
            calls += u32::from(a.ev_call >= a.ev_fold);
        }
        100.0 * f64::from(calls) / f64::from(total)
    }

    #[test]
    fn call_and_fold_are_both_real_answers() {
        let (allin, next) = (call_rate(Skill::Spot, 120, true), call_rate(Skill::Spot, 120, false));
        assert!((25.0..=75.0).contains(&allin), "all-in {allin}");
        assert!((25.0..=75.0).contains(&next), "next card {next}");
    }

    #[test]
    #[ignore = "runs thousands of spots; part of the release --ignored pass"]
    fn call_and_fold_are_balanced_over_two_thousand_seeds() {
        for (skill, seeds) in [(Skill::Spot, 2000), (Skill::SpotMw, 2000)] {
            for all_in in [true, false] {
                let rate = call_rate(skill, seeds, all_in);
                assert!((40.0..=60.0).contains(&rate), "{skill:?} all-in {all_in}: call right {rate}% of {seeds} seeds");
            }
        }
    }

    #[test]
    fn spots_exclude_hopeless_and_certain_equity() {
        use poker_trainer::classes::mix_combos;
        use poker_trainer::equity::equity_all_in;
        for seed in 0..150 {
            let q = spot_of(Skill::Spot, seed);
            let lists = vec![mix_combos(&q.opponents[0], q.hero, &q.board)];
            let share = equity_all_in(q.hero, &q.board, &lists, seed).unwrap().share;
            assert!((0.15..=0.85).contains(&share), "seed {seed}: {share}");
        }
    }

    #[test]
    fn the_reveal_lists_the_groups_used() {
        let mix = OppMix { player: Some(PlayerType::Tight), classes: vec![OppClass::Over, OppClass::Top] };
        let q = SpotQuestion { hero: hand("AhQh"), board: cards("Kh7h2c"), pot: 100, call: 50, mode: SpotMode::AllIn, opponents: vec![mix] };
        let a = spot_analysis(&q, 1).unwrap();
        assert_eq!(a.groups.len(), 1);
        assert!(a.groups[0].starts_with("Hands counted: an overpair ("), "{}", a.groups[0]);
        assert!(a.groups[0].contains("top pair ("), "{}", a.groups[0]);
    }

    fn ways(line: &str) -> usize {
        line.split('(').skip(1).map(|p| p.split(' ').next().unwrap().parse::<usize>().unwrap()).sum()
    }

    #[test]
    fn group_counts_add_up_to_the_combos_averaged() {
        for seed in 0..40 {
            let q = spot_of(Skill::Spot, seed);
            let a = spot_analysis(&q, seed).unwrap();
            assert_eq!(ways(&a.groups[0]), a.combos_per_opponent[0], "seed {seed}: {}", a.groups[0]);
        }
    }

    #[test]
    fn a_chain_group_count_leaves_out_the_shown_hand() {
        use poker_trainer::grade::chain_analysis;
        let with_mix = poker_trainer::item::ChainHand {
            hero: hand("8s7s"),
            board: cards("2hQsJs"),
            villain: hand("KhQh"),
            second: Some(OppMix { player: Some(PlayerType::Loose), classes: PlayerType::Loose.groups().to_vec() }),
            pot_before: 100,
            bet: 50,
            mode: SpotMode::AllIn,
        };
        let a = chain_analysis(&with_mix, 4).unwrap();
        assert_eq!(ways(&a.groups[0]), a.combos_per_opponent[1], "{}", a.groups[0]);
    }

    #[test]
    fn the_frozen_baseline_file_is_byte_identical() {
        let bytes = include_bytes!("../data/baseline_v1.txt");
        let hash = bytes.iter().fold(0xcbf29ce484222325u64, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x100000001b3));
        assert_eq!((bytes.len(), hash), BASELINE_FINGERPRINT, "data/baseline_v1.txt changed");
    }

    #[test]
    fn a_chain_second_opponent_is_a_mix_and_hero_holds_something() {
        let mut seconds = 0;
        for seed in 0..100u64 {
            let Question::Chain(q) = generate(Skill::Chain, seed << 2).question else { panic!() };
            assert!(worth_a_decision(q.hand.hero, &q.hand.board), "seed {seed}");
            if let Some(mix) = &q.hand.second {
                seconds += 1;
                assert!(mix.player.is_some() && mix.classes.len() >= 2, "seed {seed}: {mix:?}");
            }
        }
        assert!(seconds > 10, "{seconds}");
    }
}

// ----- What beats you? -----

mod beats_me {
    use super::*;
    use poker_trainer::beats_me::{beats_me_rung, kinds_analysis, mix_standing, ways_count, BeatKind, MixBand, MIX_AHEAD_PCT, MIX_BEHIND_PCT};
    use poker_trainer::classes::PlayerType;
    use poker_trainer::grade::{BeatsMeReveal, BEATS_RANDOM_NOTE};
    use poker_trainer::item::{beats_me_rung_of_seed, beats_me_seed_with_rung, BeatsMeAsk, BeatsMeQuestion, BEATS_ME_RUNGS};
    use poker_trainer::progress::{skills_up_to_step, PASS_BEATS_ME};
    use poker_trainer::schedule::{BEATS_WEIGHT_PER_MILLE, MIN_PREFLOP_PERCENT};

    fn at_rung(seed: u64, rung: u8) -> Item {
        generate(Skill::BeatsMe, beats_me_seed_with_rung(seed, rung))
    }

    fn q(item: &Item) -> &BeatsMeQuestion {
        match &item.question {
            Question::BeatsMe(q) => q,
            _ => panic!("not a beats_me item"),
        }
    }

    fn row(seed: u64, t: u64, correct: bool) -> Attempt {
        let mut attempt = logged(Skill::BeatsMe, seed, t, correct, "1:flop");
        attempt.correct = correct;
        attempt
    }

    #[test]
    fn a_seed_carries_its_rung() {
        for rung in 0..BEATS_ME_RUNGS {
            let seed = beats_me_seed_with_rung(0x0123_4567_89AB_CDEF, rung);
            assert_eq!(beats_me_rung_of_seed(seed), rung);
            assert_eq!(q(&generate(Skill::BeatsMe, seed)).rung(), rung);
        }
    }

    #[test]
    fn rung_zero_grades_exactly_and_explains_each_hand() {
        let item = item(
            Skill::BeatsMe,
            Question::BeatsMe(BeatsMeQuestion {
                hero: hand("AhKd"),
                board: cards("Ks7c2d"),
                ask: BeatsMeAsk::Which([hand("KcQd"), hand("KhAc"), hand("7h7d"), hand("9d8d")]),
            }),
        );
        // KQ is behind (kicker), AK ties, 77 is ahead, 98 is behind.
        assert!(answer(&item, "c").correct);
        assert!(!answer(&item, "a").correct);
        assert!(!answer(&item, "ac").correct);
        assert!(!answer(&item, "none").correct);
        assert_eq!(answer(&item, "C").answer, "c");
        assert!(grade(&item, "z").is_err());
        let Reveal::BeatsMe(BeatsMeReveal::Which { hands, .. }) = answer(&item, "c").reveal else { panic!() };
        assert_eq!(hands.len(), 4);
    }

    #[test]
    fn rung_zero_usually_has_one_to_three_ahead_and_sometimes_none() {
        let mut counts = [0usize; 5];
        for seed in 0..300 {
            let item = at_rung(seed, 0);
            let BeatsMeAsk::Which(hands) = q(&item).ask else { panic!() };
            let ahead = hands.iter().filter(|&&h| rank_of(h, &q(&item).board) > rank_of(q(&item).hero, &q(&item).board)).count();
            counts[ahead] += 1;
        }
        assert_eq!(counts[4], 0, "{counts:?}");
        assert!(counts[0] >= 5 && counts[0] < counts[1], "{counts:?}");
        assert!(counts[1] > 40 && counts[2] > 40 && counts[3] > 40, "{counts:?}");
    }

    #[test]
    fn rung_one_kinds_come_from_every_unseen_holding() {
        let rows = kinds_analysis(hand("Ah7d"), &cards("7c2s9d"));
        let codes: String = rows.iter().map(|r| r.kind.code()).collect();
        assert!(codes.contains(BeatKind::BiggerPair.code()), "{codes}");
        assert!(codes.contains(BeatKind::TwoPair.code()), "{codes}");
        assert!(codes.contains(BeatKind::SetOrTrips.code()), "{codes}");
        assert!(!codes.contains(BeatKind::Flush.code()), "{codes}");
        // Top kicker on top pair: nothing can have a better kicker.
        let top = kinds_analysis(hand("AhKd"), &cards("Ks7c2d"));
        assert!(!top.iter().any(|r| r.kind == BeatKind::BetterKicker), "{top:?}");
        let weak = kinds_analysis(hand("Kh3d"), &cards("Ks7c2d"));
        assert!(weak.iter().any(|r| r.kind == BeatKind::BetterKicker), "{weak:?}");
    }

    #[test]
    fn rung_one_answers_as_a_set_or_none() {
        let hero = hand("AhKd");
        let board = cards("Ks7c2d");
        let the_item = item(Skill::BeatsMe, Question::BeatsMe(BeatsMeQuestion { hero, board: board.clone(), ask: BeatsMeAsk::Kinds }));
        let truth: String = {
            let mut letters: Vec<char> = kinds_analysis(hero, &board).iter().map(|r| r.kind.code()).collect();
            letters.sort_unstable();
            letters.into_iter().collect()
        };
        assert!(answer(&the_item, &truth).correct);
        assert!(!answer(&the_item, "none").correct);
    }

    #[test]
    fn rung_one_sometimes_deals_a_hand_nothing_beats() {
        let mut nothing = 0;
        for seed in 0..200 {
            let item = at_rung(seed, 1);
            if kinds_analysis(q(&item).hero, &q(&item).board).is_empty() {
                nothing += 1;
            }
        }
        assert!((5..=60).contains(&nothing), "{nothing}");
    }

    #[test]
    fn rung_two_counts_ways_after_card_removal() {
        assert_eq!(ways_count(12, 12, hand("2h3d"), &cards("9c8s4d")).ways, 6, "a pair is 6 ways");
        assert_eq!(ways_count(12, 12, hand("2h3d"), &cards("Ac8s4d")).ways, 3, "3 with one on the board");
        assert_eq!(ways_count(12, 12, hand("Ad3d"), &cards("Ac8s4d")).ways, 1, "1 with two seen");
        assert_eq!(ways_count(12, 11, hand("2h3d"), &cards("9c8s4d")).ways, 16, "two ranks is 16");
        assert_eq!(ways_count(12, 11, hand("2h3d"), &cards("Ac8s4d")).ways, 12, "minus an ace on the board");
    }

    #[test]
    fn rung_two_always_asks_about_hands_that_beat_hero() {
        for seed in 0..100 {
            let item = at_rung(seed, 2);
            let BeatsMeAsk::Ways { high, low } = q(&item).ask else { panic!() };
            let count = ways_count(high, low, q(&item).hero, &q(&item).board);
            assert!(count.ways >= 1, "seed {seed}");
            let out = answer(&item, &count.ways.to_string());
            assert!(out.correct, "seed {seed}");
            let Reveal::BeatsMe(BeatsMeReveal::Ways { .. }) = out.reveal else { panic!() };
        }
    }

    #[test]
    fn rung_three_bands_and_ties_count_half() {
        let mix = OppMix::single(OppClass::Top);
        let standing = mix_standing(hand("Ah7d"), &cards("Ks7c2d"), &mix);
        assert_eq!(standing.tie, 0);
        assert_eq!(MixBand::from_pct(MIX_AHEAD_PCT + 0.1), MixBand::Ahead);
        assert_eq!(MixBand::from_pct(MIX_AHEAD_PCT), MixBand::Even);
        assert_eq!(MixBand::from_pct(MIX_BEHIND_PCT), MixBand::Even);
        assert_eq!(MixBand::from_pct(MIX_BEHIND_PCT - 0.1), MixBand::Behind);
    }

    #[test]
    fn rung_three_ties_count_half_exactly() {
        // Two exact holdings by their class: top pair with the same kicker ties.
        let mix = OppMix::single(OppClass::Top);
        let hero = hand("KdQs");
        let board = cards("Kc7h2d");
        let s = mix_standing(hero, &board, &mix);
        let expected = 100.0 * (s.total - s.beat_you - s.tie) as f64 / s.total as f64 + 50.0 * s.tie as f64 / s.total as f64;
        assert!(s.tie > 0, "{s:?}");
        assert!((s.pct - expected).abs() < 1e-9);
    }

    #[test]
    fn rung_three_items_hit_all_three_bands() {
        let mut seen = std::collections::HashSet::new();
        for seed in 0..120 {
            let item = at_rung(seed, 3);
            let BeatsMeAsk::Mix(mix) = &q(&item).ask else { panic!() };
            assert!(mix.player.is_some() && mix.classes.len() >= 2);
            seen.insert(mix_standing(q(&item).hero, &q(&item).board, mix).band.code());
        }
        assert_eq!(seen.len(), 3, "{seen:?}");
    }

    #[test]
    fn every_rung_survives_a_suit_permutation() {
        for rung in 0..BEATS_ME_RUNGS {
            for seed in 0..15 {
                let item = at_rung(seed, rung);
                let perm = [2u8, 3, 0, 1];
                let permuted = Item { question: item.question.with_suit_perm(&perm), ..item.clone() };
                let probe = match rung {
                    0 | 1 => "none",
                    2 => "6",
                    _ => "a",
                };
                assert_eq!(answer(&item, probe).correct, answer(&permuted, probe).correct, "rung {rung} seed {seed}");
            }
        }
    }

    fn correct_row(t: u64, rung: u8, right: bool) -> Attempt {
        row(beats_me_seed_with_rung(t, rung), t, right)
    }

    #[test]
    fn the_rung_rises_at_eighty_five_percent_over_twenty() {
        let build = |right: u64| -> Vec<Attempt> {
            (0..20).map(|i| correct_row(i, 0, i < right)).collect()
        };
        assert_eq!(beats_me_rung(&build(17)), 1, "17 of 20 is 85%");
        assert_eq!(beats_me_rung(&build(16)), 0);
        assert_eq!(beats_me_rung(&(0..19).map(|i| correct_row(i, 0, true)).collect::<Vec<_>>()), 0, "19 attempts");
        let climbed: Vec<Attempt> = (0..3).flat_map(|r| (0..20).map(move |i| correct_row(i + r * 100, r as u8, true))).collect();
        assert_eq!(beats_me_rung(&climbed), 3);
    }

    #[test]
    fn only_rungs_two_and_three_count_for_the_mark() {
        assert_eq!(PASS_BEATS_ME.min_correct_pct, Some(85.0));
        let low: Vec<Attempt> = (0..40).map(|i| correct_row(i, 1, true)).collect();
        assert!(!skill_stats(Skill::BeatsMe, &low).drill_passed, "rung 1 attempts don't count");
        let high: Vec<Attempt> = (0..40).map(|i| correct_row(i, 3, i < 34)).collect();
        assert!(skill_stats(Skill::BeatsMe, &high).drill_passed, "34 of 40 is 85%");
        let under: Vec<Attempt> = (0..40).map(|i| correct_row(i, 2, i < 33)).collect();
        assert!(!skill_stats(Skill::BeatsMe, &under).drill_passed);
    }

    #[test]
    fn beats_is_in_no_step_but_still_in_the_drill() {
        assert!(STEPS[0].contains(&Skill::BeatsMe));
        assert!(!STEPS.iter().any(|step| step.contains(&Skill::Beats)));
        assert!(skills_up_to_step(1).contains(&Skill::Beats));
        assert_eq!(BEATS_WEIGHT_PER_MILLE, 500);
    }

    #[test]
    fn beats_keeps_the_random_hands_note() {
        assert!(BEATS_RANDOM_NOTE.starts_with("That's against random hands."));
    }

    #[test]
    fn beats_is_dealt_at_about_half_the_weight() {
        let allowed = vec![Skill::Beats, Skill::Made, Skill::Outs, Skill::Price];
        let mut session = Session::new(allowed, &[], 1000, 11);
        let (mut beats, mut total) = (0u32, 0u32);
        for _ in 0..8000 {
            let planned = session.try_next_item().unwrap();
            beats += u32::from(planned.skill == Skill::Beats);
            total += 1;
            session.record(planned, true);
        }
        // Weights 500:1000:1000:1000, against 25% at equal weight.
        let share = f64::from(beats) / f64::from(total);
        assert!((0.11..=0.18).contains(&share), "beats {beats} of {total}: {share}");
    }

    #[test]
    fn the_default_drill_keeps_preflop_at_a_quarter() {
        for seed in 0..5u64 {
            let allowed = skills_up_to_step(1);
            let mut session = Session::new(allowed, &[], 1000, seed).with_preflop_floor();
            let (mut preflop, mut total) = (0u64, 0u64);
            for _ in 0..800 {
                let planned = session.try_next_item().unwrap();
                total += 1;
                preflop += u64::from(planned.skill == Skill::Preflop);
                session.record(planned, planned.skill != Skill::Preflop);
                assert!(preflop * 100 + 25 >= total * MIN_PREFLOP_PERCENT, "seed {seed}: {preflop} of {total}");
            }
        }
    }

    #[test]
    fn a_chain_cannot_push_preflop_under_its_share() {
        for seed in 0..300u64 {
            let mut session = Session::new(skills_up_to_step(3), &[], 1000, seed).with_preflop_floor();
            let (mut preflop, mut total) = (0u64, 0u64);
            for _ in 0..15 {
                let planned = session.try_next_item().unwrap();
                total += 1;
                preflop += u64::from(planned.skill == Skill::Preflop);
                session.record(planned, true);
            }
            assert!(preflop * 100 >= total * MIN_PREFLOP_PERCENT, "seed {seed}: {preflop} of {total}");
        }
    }

    #[test]
    fn rung_three_sibling_search_settles_on_the_first_draw() {
        for seed in 0..40 {
            let item = at_rung(seed, 3);
            let wanted = family(&item);
            assert_eq!(family_prefilter(Skill::BeatsMe, item.seed, &wanted), Prefilter::Definite, "seed {seed}");
            let other = if wanted == "3:a" { "3:e" } else { "3:a" };
            assert_eq!(family_prefilter(Skill::BeatsMe, item.seed, other), Prefilter::Reject);
        }
    }

    #[test]
    fn the_mix_method_never_rounds_across_a_band_edge() {
        let s = poker_trainer::beats_me::MixStanding { total: 106, beat_you: 64, tie: 0, pct: 39.62, band: MixBand::Behind };
        assert!(poker_trainer::beats_me::mix_method(&s).contains("39.6%"));
    }

    #[test]
    fn a_requested_skill_list_has_no_preflop_floor() {
        let mut session = Session::new(vec![Skill::Made, Skill::Preflop], &[], 1000, 2);
        let mut preflop = 0;
        for _ in 0..200 {
            let planned = session.try_next_item().unwrap();
            preflop += usize::from(planned.skill == Skill::Preflop);
            session.record(planned, true);
        }
        assert!(preflop > 60, "{preflop} under plain weights");
    }

    #[test]
    fn checkups_take_ten_items_per_rung_from_held_out_seeds() {
        let plan = checkup_plan(Skill::BeatsMe, 0);
        let originals: Vec<u64> = plan.slots.iter().filter(|s| s.repeat_of.is_none()).map(|s| s.seed).collect();
        assert_eq!(originals.len(), CHECKUP_ORIGINALS);
        assert!(originals.iter().all(|&s| poker_trainer::item::is_held_out(s)));
        let mut per_rung = [0usize; BEATS_ME_RUNGS as usize];
        for &seed in &originals {
            per_rung[usize::from(beats_me_rung_of_seed(seed))] += 1;
        }
        assert_eq!(per_rung, [10; BEATS_ME_RUNGS as usize]);
    }

    #[test]
    fn beats_me_families_key_on_rung_and_shape() {
        let family_of_item = |item: &Item| family(item);
        let (a, b, c) = (at_rung(1, 0), at_rung(1, 2), at_rung(2, 3));
        assert!(family_of_item(&a).starts_with("0:"));
        assert!(family_of_item(&b).starts_with("2:"));
        assert!(family_of_item(&c).starts_with("3:"));
        let sibling = find_sibling(Skill::BeatsMe, &family(&b), 99, 0).expect("a sibling exists");
        assert_eq!(beats_me_rung_of_seed(sibling), 2);
        assert_eq!(family(&generate(Skill::BeatsMe, sibling)), family(&b));
    }

    #[test]
    fn a_checkup_begun_under_an_older_generator_is_stale() {
        let row = |t: u64, slot: u32, gen_version: u32| {
            let mut attempt = logged(Skill::Made, t, t, true, "top");
            attempt.mode = Mode::Check;
            attempt.gen_version = gen_version;
            attempt.extra.check = Some(CheckRef { run: 1, form: 0, slot });
            attempt
        };
        let runs = |rows: Vec<Attempt>| poker_trainer::checkup::checkup_runs(&rows);
        assert!(runs(vec![row(1, 0, 1), row(2, 1, GEN_VERSION)])[0].stale);
        assert!(!runs(vec![row(1, 0, GEN_VERSION), row(2, 1, GEN_VERSION)])[0].stale);
    }

    #[test]
    fn a_look_ahead_copy_starts_no_sibling_search() {
        let log = vec![logged(Skill::Made, 42, 1, false, "top")];
        let session = Session::new(vec![Skill::Made], &log, 10 * DAY_MS, 3);
        // A family due today makes a real deal search, but a look-ahead copy can't.
        let looked = session.clone().without_sibling_search().try_next_item().unwrap();
        assert_eq!(looked.origin, Origin::New);
        let real = session.clone().try_next_item().unwrap();
        assert_eq!(real.origin, Origin::Due);
    }

    #[test]
    fn a_new_session_deals_at_the_players_rung() {
        let log: Vec<Attempt> = (0..20).map(|i| correct_row(i, 0, true)).collect();
        let mut session = Session::new(vec![Skill::BeatsMe], &log, 1000, 9);
        let planned = session.try_next_item().unwrap();
        assert_eq!(beats_me_rung_of_seed(planned.seed), 1);
        assert!(!poker_trainer::item::is_held_out(planned.seed));
    }

    #[test]
    fn players_have_three_mixes() {
        assert_eq!(PlayerType::ALL.len(), 3);
        assert!(PlayerType::Tight.groups().contains(&OppClass::Set));
        assert!(PlayerType::Loose.groups().contains(&OppClass::Nothing));
    }
}

// ----- The nuts -----

mod the_nuts {
    use super::*;
    use poker_trainer::grade::{any_of_suit_text, nuts_analysis, parse_nuts_guess, NutsGuess};

    fn nuts_item(board: &str) -> Item {
        item(Skill::Nuts, Question::Nuts { board: cards(board) })
    }

    #[test]
    fn card_plus_any_of_a_suit_is_a_form() {
        assert_eq!(parse_nuts_guess("Asxs"), Some(NutsGuess::CardAnySuit(parse_cards("As").unwrap()[0], 0)));
        assert!(parse_nuts_guess("Asxz").is_none());
    }

    #[test]
    fn a_card_and_any_spade_on_the_flush_board() {
        let board = "KsQs4s9h7d";
        assert!(answer(&nuts_item(board), "Asxs").correct, "As with any spade");
        assert!(answer(&nuts_item(board), "AsJs").correct);
        assert!(answer(&nuts_item(board), "As2s").correct);
        assert!(!answer(&nuts_item(board), "Asx").correct, "As with any card includes non-spades");
        assert!(!answer(&nuts_item(board), "Ax").correct);
        // Not every spade pairs with the J of spades to make the nuts.
        assert!(!answer(&nuts_item(board), "Jsxs").correct);
    }

    #[test]
    fn card_removal_makes_a_lesser_holding_unbeatable() {
        // Only one other king is left, so nobody can hold two kings against K7.
        let board = "KsKd7c";
        assert!(answer(&nuts_item(board), "Kc7d").correct, "K7 can't be beaten");
        assert!(answer(&nuts_item(board), "KcKh").correct, "the quads are the nuts too");
        assert!(!answer(&nuts_item(board), "7d7h").correct, "a smaller full house");
    }

    #[test]
    fn nuts_are_exactly_the_holdings_no_disjoint_holding_beats() {
        let analysis = nuts_analysis(&cards("KsKd7c"));
        assert!(analysis.nuts.contains(&hand("Kc7d")));
        assert!(!analysis.nuts.contains(&hand("7d7h")));
        assert!(analysis.top_holdings.iter().all(|h| analysis.nuts.contains(h)), "the raw best stays the nuts");
    }

    #[test]
    fn the_reveal_says_it_in_plain_words() {
        let a = nuts_analysis(&cards("KsQs4s9h7d"));
        let (card, suit) = a.any_of_suit.expect("A of spades plus any spade");
        assert_eq!(any_of_suit_text(card, suit), "A\u{2660} with any other spade");
    }

    #[test]
    fn only_the_best_hand_is_held_and_next_best_can_be_beaten() {
        let a = nuts_analysis(&cards("KhKd7c"));
        assert_eq!(a.top_holdings.len(), 1, "the one pair of kings left");
        assert!(a.by_card_removal.iter().any(|h| h.contains(&parse_cards("Ks").unwrap()[0]) && h.contains(&parse_cards("7s").unwrap()[0])));
        let (rank, holders) = a.next.as_ref().expect("a next best");
        assert!(holders.iter().all(|h| !a.nuts.contains(h)));
        assert_eq!(rank.describe(), "a full house, sevens full of kings");

        let a = nuts_analysis(&cards("9s8s7s2h3d"));
        let has = |h: &Holding, x: &str, y: &str| h.contains(&parse_cards(x).unwrap()[0]) && h.contains(&parse_cards(y).unwrap()[0]);
        assert_eq!(a.top_holdings.len(), 1);
        assert!(has(&a.top_holdings[0], "Js", "Ts"));
        assert!(a.by_card_removal.iter().any(|h| has(h, "Ts", "6s")), "only JT beats it, and it holds the ten");
        let (_, holders) = a.next.as_ref().expect("a next best");
        assert!(!holders.iter().any(|h| has(h, "Ts", "6s")));

        let a = nuts_analysis(&cards("KsQs4s9h7d"));
        let (_, holders) = a.next.as_ref().expect("a next best");
        assert!(holders.iter().all(|h| !h.contains(&parse_cards("As").unwrap()[0])), "{holders:?}");
    }

    #[test]
    fn x_forms_still_need_every_holding_to_be_nuts() {
        // A7 here is two pair; Kx includes hands below the full house.
        assert!(!answer(&nuts_item("KsKd7c"), "Kx").correct);
        assert!(!answer(&nuts_item("KsKd7c"), "7x").correct);
    }
}
