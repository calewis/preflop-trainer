//! The "Read the price" ladder. Truth here is worked out independently of the
//! library: whole-dollar cross-multiplication, not its formulas.

use std::collections::HashMap;

use poker_trainer::answer::{answer_format, AnswerFormat};
use poker_trainer::checkup::checkup_plan;
use poker_trainer::family::{family_of, find_sibling};
use poker_trainer::generate::generate;
use poker_trainer::grade::{grade, Reveal};
use poker_trainer::item::{
    is_held_out, read_price_rung_of_seed, read_price_seed_with_rung, HitStreet, Item, PriceHand, Question, ReadPriceAsk,
    ReadPriceQuestion, Skill, GEN_VERSION, HELD_OUT_BIT, READ_PRICE_RUNGS,
};
use poker_trainer::log::{asks_confidence, Attempt};
use poker_trainer::progress::{skill_stats, skills_up_to_step, STEPS};
use poker_trainer::read_price::{
    context_lines, read_price_rung, read_price_status, DrawKind, PriceBand, PriceCall, ReadPriceReveal, READ_PRICE_PROMOTE_WINDOW,
};
use poker_trainer::reflection::{reflection_eligible, reflection_form};
use poker_trainer::rng::Rng;
use poker_trainer::schedule::{Origin, Planned, Session, DAY_MS, MIN_PREFLOP_PERCENT};

const ITEMS: u64 = 3000;

fn at_rung(i: u64, rung: u8) -> Item {
    let seed = read_price_seed_with_rung(i.wrapping_mul(0x9E37_79B9_7F4A_7C15) & !HELD_OUT_BIT, rung);
    generate(Skill::ReadPrice, seed)
}

fn q(item: &Item) -> &ReadPriceQuestion {
    match &item.question {
        Question::ReadPrice(q) => q,
        other => panic!("not a read_price item: {other:?}"),
    }
}

fn reveal(item: &Item, raw: &str) -> ReadPriceReveal {
    match grade(item, raw).expect("a valid answer").reveal {
        Reveal::ReadPrice(r) => r,
        other => panic!("{other:?}"),
    }
}

/// The price in percent, from nothing but the dollars.
fn price(p: &ReadPriceQuestion) -> f64 {
    let win = p.pot_before + p.bet + p.bet * p.callers;
    100.0 * f64::from(p.bet) / f64::from(win + p.bet)
}

fn share_of(outs: u32, two_cards: bool) -> f64 {
    if two_cards {
        // One minus missing both of the two cards to come, from 47 unseen.
        100.0 * (1.0 - f64::from((47 - outs) * (46 - outs)) / f64::from(47u32 * 46))
    } else {
        100.0 * f64::from(outs) / 46.0
    }
}

fn outs(kind: DrawKind) -> u32 {
    match kind {
        DrawKind::Flush => 9,
        DrawKind::OpenEnded => 8,
        DrawKind::Gutshot => 4,
        DrawKind::Overcards => 6,
        DrawKind::FlushOpenEnded => 15,
        DrawKind::FlushGutshot => 12,
    }
}

fn band_from(pct: f64) -> PriceBand {
    if pct < 20.0 {
        PriceBand::Cheap
    } else if pct < 30.0 {
        PriceBand::Fair
    } else {
        PriceBand::Expensive
    }
}

fn counted<T: std::hash::Hash + Eq>(items: impl Iterator<Item = T>) -> HashMap<T, u64> {
    let mut map = HashMap::new();
    for item in items {
        *map.entry(item).or_insert(0) += 1;
    }
    map
}

fn assert_balanced<T: std::hash::Hash + Eq + std::fmt::Debug>(counts: &HashMap<T, u64>, classes: usize, total: u64) {
    assert_eq!(counts.len(), classes, "{counts:?}");
    let even = total as f64 / classes as f64;
    for (key, count) in counts {
        let ratio = *count as f64 / even;
        assert!((0.8..=1.2).contains(&ratio), "{key:?} {count} of {total}: {counts:?}");
    }
}

#[test]
fn a_seed_carries_its_rung_and_the_extra_values_wrap() {
    for rung in 0..READ_PRICE_RUNGS {
        let seed = read_price_seed_with_rung(0x0123_4567_89AB_CDEF & !HELD_OUT_BIT, rung);
        assert_eq!(read_price_rung_of_seed(seed), rung);
        assert_eq!(q(&generate(Skill::ReadPrice, seed)).rung(), rung);
        assert!(!is_held_out(seed), "the rung bits stop below the held-out bit");
    }
    for raw in 5u64..8 {
        let seed = raw << 60;
        assert_eq!(read_price_rung_of_seed(seed), (raw % 5) as u8);
    }
}

#[test]
fn existing_skills_keep_their_positions_and_the_generator_version() {
    assert_eq!(GEN_VERSION, 3, "read_price adds a skill without changing an existing generator");
    assert_eq!(Skill::BeatsMe as u64, 15, "existing positions seed their generators");
    assert_eq!(Skill::ReadPrice as u64, 16);
    assert_eq!(Skill::ALL.last(), Some(&Skill::ReadPrice));
}

#[test]
fn amounts_stay_in_bounds_and_look_like_a_table() {
    let mut in_core = 0;
    let mut total = 0;
    let (mut tiny, mut big) = (0, 0);
    for rung in 0..=3u8 {
        for i in 0..ITEMS {
            let item = at_rung(i, rung);
            let p = q(&item);
            assert!((3..=300).contains(&p.pot_before), "{p:?}");
            assert!(p.bet >= 1 && p.bet * 2 <= p.pot_before * 5, "{p:?}");
            assert!(p.callers <= 2, "{p:?}");
            in_core += u64::from((6..=60).contains(&p.pot_before));
            total += 1;
            tiny += u64::from(p.bet * 5 <= p.pot_before);
            big += u64::from(p.bet >= p.pot_before);
        }
    }
    assert!(in_core * 100 >= total * 75, "{in_core} of {total} pots in $6-$60");
    for rung in 0..=3u8 {
        let callers = (0..ITEMS).filter(|&i| q(&at_rung(i, rung)).callers > 0).count() as u64;
        assert!((callers * 100).abs_diff(ITEMS * 40) <= ITEMS * 4, "rung {rung}: {callers} of {ITEMS} have callers");
    }
    assert!(tiny * 100 >= total * 5, "tiny stabs: {tiny} of {total}");
    assert!(big * 100 >= total * 8, "pot-sized or larger: {big} of {total}");
}

#[test]
fn the_pot_is_stated_before_the_bet_and_callers_go_in_the_pot() {
    for i in 0..500 {
        let item = at_rung(i, (i % 4) as u8);
        let p = q(&item);
        let lines = context_lines(p);
        assert!(lines[0].starts_with(&format!("The pot was ${}. Someone bets ${}.", p.pot_before, p.bet)), "{lines:?}");
        match p.callers {
            0 => assert!(!lines[0].contains("called"), "{lines:?}"),
            1 => assert!(lines[0].contains("1 player has already called"), "{lines:?}"),
            n => assert!(lines[0].contains(&format!("{n} players have already called")), "{lines:?}"),
        }
    }
    let item = Item {
        skill: Skill::ReadPrice,
        seed: 0,
        question: Question::ReadPrice(ReadPriceQuestion { pot_before: 10, bet: 5, callers: 2, ask: ReadPriceAsk::Number }),
    };
    assert_eq!(reveal(&item, "25").lines[0], "Call $5 to win $25 (the $10 pot, the $5 bet and $10 from 2 callers): 5 \u{f7} 30 = 16.7%.");
    assert!(grade(&item, "16.7").unwrap().correct, "the callers' money is in the pot");
    assert!(!grade(&item, "25").unwrap().correct, "ignoring callers overstates the price");
}

#[test]
fn rung_zero_stays_clear_of_the_lines_and_is_balanced() {
    let mut bands = Vec::new();
    for i in 0..ITEMS {
        let item = at_rung(i, 0);
        let p = q(&item);
        let pct = price(p);
        assert!((pct - 20.0).abs() >= 1.5 && (pct - 30.0).abs() >= 1.5, "{p:?} {pct}");
        let truth = band_from(pct);
        for band in PriceBand::ALL {
            assert_eq!(grade(&item, band.code()).unwrap().correct, band == truth, "{p:?} {pct}");
        }
        bands.push(truth);
    }
    assert_balanced(&counted(bands.into_iter()), 3, ITEMS);
}

#[test]
fn rung_zero_shortcut_always_matches_the_answer() {
    for i in 0..ITEMS {
        let item = at_rung(i, 0);
        let p = q(&item);
        // Thirds and three-quarters of the pot, callers added first, by cross-multiplying.
        let bigger_pot = p.pot_before + p.bet * p.callers;
        let shortcut = if p.bet * 3 < bigger_pot {
            PriceBand::Cheap
        } else if p.bet * 4 < bigger_pot * 3 {
            PriceBand::Fair
        } else {
            PriceBand::Expensive
        };
        assert_eq!(shortcut, band_from(price(p)), "{p:?}");
        let r = reveal(&item, shortcut.code());
        assert_eq!(r.answer, shortcut.label());
        assert_eq!(r.lines.len(), 2);
        assert_eq!(
            r.lines[1],
            "Shortcut: compare the bet to the pot before it, with any callers added to the pot first. Under a third of the pot is cheap, a third to three-quarters is fair, more is expensive."
        );
    }
}

#[test]
fn rung_one_uses_the_price_tolerance() {
    let (mut with_callers, mut heads_up) = (0, 0);
    for i in 0..ITEMS {
        let item = at_rung(i, 1);
        let p = q(&item);
        let pct = price(p);
        assert!(grade(&item, &format!("{}", pct + 1.9)).unwrap().correct, "{p:?}");
        assert!(grade(&item, &format!("{}", (pct - 1.9).max(0.0))).unwrap().correct, "{p:?}");
        assert!(!grade(&item, &format!("{}", pct + 2.5)).unwrap().correct, "{p:?}");
        with_callers += u64::from(p.callers > 0);
        heads_up += u64::from(p.callers == 0);
        assert!(matches!(answer_format(&item.question), AnswerFormat::Percent));
    }
    assert!(with_callers > 0 && heads_up > 0);
}

fn truth_call(share: f64, pct: f64) -> Option<PriceCall> {
    if share - pct >= 8.0 {
        Some(PriceCall::Easy)
    } else if pct - share >= 8.0 {
        Some(PriceCall::Fold)
    } else if (share - pct).abs() <= 4.0 {
        Some(PriceCall::Close)
    } else {
        None
    }
}

#[test]
fn rung_two_never_deals_the_gap_and_each_class_comes_from_both_hand_kinds() {
    let mut classes = Vec::new();
    let mut by_kind = HashMap::new();
    for i in 0..ITEMS {
        let item = at_rung(i, 2);
        let p = q(&item);
        let ReadPriceAsk::Stated(hand) = p.ask else { panic!("{p:?}") };
        let (share, read) = match hand {
            PriceHand::Read { share_pct } => {
                assert_eq!(share_pct % 5, 0, "{p:?}");
                (f64::from(share_pct), true)
            }
            PriceHand::Draw { kind, street } => {
                (share_of(outs(kind), street == HitStreet::Flop), false)
            }
        };
        let call = truth_call(share, price(p)).unwrap_or_else(|| panic!("the gap was dealt: {p:?}"));
        let edge = share - price(p);
        assert!(edge.abs() >= 8.5 || edge.abs() <= 3.5, "dealt too near a line: {edge} {p:?}");
        for option in PriceCall::ALL {
            assert_eq!(grade(&item, option.code()).unwrap().correct, option == call, "{p:?}");
        }
        classes.push(call);
        by_kind.entry((read, call)).or_insert(0u64);
        *by_kind.get_mut(&(read, call)).unwrap() += 1;
    }
    assert_balanced(&counted(classes.into_iter()), 3, ITEMS);
    assert_eq!(by_kind.len(), 6, "{by_kind:?}");
}

#[test]
fn rung_two_shows_the_share_and_rung_three_keeps_it_back() {
    let (mut one, mut two) = (0, 0);
    for i in 0..ITEMS {
        let two_q = at_rung(i, 2);
        let two_lines = context_lines(q(&two_q)).join(" ");
        if let ReadPriceAsk::Stated(PriceHand::Draw { .. }) = q(&two_q).ask {
            assert!(two_lines.contains("to come: about ") && two_lines.contains("Assume your outs win and nothing else does."), "{two_lines}");
        }
        let three = at_rung(i, 3);
        let p = q(&three);
        let ReadPriceAsk::Recall { kind, street } = p.ask else { panic!("{p:?}") };
        let lines = context_lines(p).join(" ");
        assert!(!lines.contains('%'), "rung 3 must not show the share: {lines}");
        assert!(lines.contains("Assume your outs win and nothing else does."), "{lines}");
        let flop = street == HitStreet::Flop;
        assert!(lines.contains(if flop { "all-in on the flop, two cards to come" } else { "one card to come" }), "{lines}");
        one += u64::from(!flop);
        two += u64::from(flop);
        let share = share_of(outs(kind), flop);
        let call = truth_call(share, price(p)).unwrap_or_else(|| panic!("the gap was dealt: {p:?}"));
        for option in PriceCall::ALL {
            assert_eq!(grade(&three, option.code()).unwrap().correct, option == call, "{p:?}");
        }
        let r = reveal(&three, "c");
        let cards = if flop { "two cards to come" } else { "one card to come" };
        assert!(r.lines[1].starts_with(&format!("{}: {} outs, about {share:.0}% with {cards},", kind.label(), outs(kind))), "{:?}", r.lines);
    }
    assert!(one > 500 && two > 500, "one card {one}, two cards {two}");
}

#[test]
fn rung_three_is_balanced_across_answers_and_uses_every_draw() {
    let mut classes = Vec::new();
    let mut draws = Vec::new();
    for i in 0..ITEMS {
        let item = at_rung(i, 3);
        let p = q(&item);
        let ReadPriceAsk::Recall { kind, street } = p.ask else { panic!("{p:?}") };
        classes.push(truth_call(share_of(outs(kind), street == HitStreet::Flop), price(p)).unwrap());
        draws.push(kind);
    }
    assert_balanced(&counted(classes.into_iter()), 3, ITEMS);
    assert_eq!(counted(draws.into_iter()).len(), 6);
}

#[test]
fn the_verdict_sentence_is_true_for_every_class() {
    for i in 0..ITEMS {
        for rung in [2, 3] {
            let item = at_rung(i, rung);
            let p = q(&item);
            let call = PriceCall::ALL.into_iter().find(|c| grade(&item, c.code()).unwrap().correct).unwrap();
            let r = reveal(&item, call.code());
            let expected = match call {
                PriceCall::Easy => "points above what you need, so it's an easy call.",
                PriceCall::Close => "points of what you need, so it's close.",
                PriceCall::Fold => "points below what you need, so fold.",
            };
            assert!(r.lines[1].ends_with(expected), "{:?} {p:?}", r.lines);
            assert_eq!(r.lines.len(), 2);
            if let ReadPriceAsk::Stated(hand) = p.ask {
                let opening = match hand {
                    PriceHand::Read { share_pct } => format!("You figure about {share_pct}%,"),
                    PriceHand::Draw { kind, street } => format!("You have about {:.0}%,", share_of(outs(kind), street == HitStreet::Flop)),
                };
                assert!(r.lines[1].starts_with(&opening), "{:?} {p:?}", r.lines);
            }
            // The whole percents on show agree with the sentence.
            let share = match p.ask {
                ReadPriceAsk::Stated(PriceHand::Read { share_pct }) => f64::from(share_pct),
                ReadPriceAsk::Stated(PriceHand::Draw { kind, street }) | ReadPriceAsk::Recall { kind, street } => {
                    share_of(outs(kind), street == HitStreet::Flop)
                }
                _ => panic!("{p:?}"),
            };
            let gap = (share.round() - price(p).round()).abs();
            assert!(if call == PriceCall::Close { gap <= 4.0 } else { gap >= 8.0 }, "shown gap {gap} for {call:?}: {p:?}");
        }
    }
}

#[test]
fn rung_four_has_exactly_one_right_answer_and_clear_options() {
    let mut positions = Vec::new();
    for i in 0..ITEMS {
        let item = at_rung(i, 4);
        let p = q(&item);
        let ReadPriceAsk::SetPrice { kind, options } = p.ask else { panic!("{p:?}") };
        assert_eq!(p.callers, 0, "heads up");
        for &b in &options {
            let need = 100.0 * f64::from(b) / f64::from(p.pot_before + 2 * b);
            assert!((need - share_of(outs(kind), false)).abs() <= 10.0, "too far from break-even: {p:?} option {b}");
        }
        assert!((3..=300).contains(&p.pot_before));
        assert!(options.windows(2).all(|w| w[0] < w[1]), "sorted and distinct: {options:?}");
        let o = outs(kind);
        let share = share_of(o, false);
        let mistake = |b: u32| u64::from(b) * 46 > u64::from(o) * u64::from(p.pot_before + 2 * b);
        for &b in &options {
            assert!(b >= 1 && b * 2 <= p.pot_before * 5, "{p:?}");
            let need = 100.0 * f64::from(b) / f64::from(p.pot_before + 2 * b);
            assert!((need - share).abs() >= 1.5, "{p:?} option {b}: {need} vs {share}");
        }
        let right = options.iter().position(|&b| mistake(b)).unwrap_or_else(|| panic!("no option works: {p:?}"));
        for (index, code) in ["a", "b", "c", "d"].into_iter().enumerate() {
            assert_eq!(grade(&item, code).unwrap().correct, index == right, "{p:?} {code}");
        }
        positions.push(right);
        let right_code = ["a", "b", "c", "d"][right];
        let r = reveal(&item, right_code);
        assert_eq!(r.lines.len(), 2, "a right pick gets no extra line: {:?}", r.lines);
        assert_eq!(r.answer, format!("Bet ${}", options[right]));
        let need = 100.0 * f64::from(options[right]) / f64::from(p.pot_before + 2 * options[right]);
        assert!(
            r.lines[0].starts_with(&format!("They call ${} to win ${}: they need {need:.0}%.", options[right], p.pot_before + options[right])),
            "{:?}",
            r.lines
        );
        assert!(r.lines[1].contains("Real opponents often call anyway"), "{:?}", r.lines);
        let AnswerFormat::Choice(choices) = answer_format(&item.question) else { panic!("choice") };
        assert_eq!(choices.len(), 4);
        for (choice, amount) in choices.iter().zip(options) {
            assert_eq!(choice.label, format!("${amount}"));
        }
    }
    assert_balanced(&counted(positions.into_iter()), 4, ITEMS);
}

/// "The first option at least 30% of the pot" looks only at the pot. For a
/// flush or open-ended draw that really is about the right size, since their
/// break-even bet is about a third of the pot and no wrong option can sit between,
/// so the guess is only checked on the other draws; the right answer's place
/// doesn't depend on how big the pot is.
#[test]
fn rung_four_cannot_be_read_from_the_pot_alone() {
    let (mut heuristic_right, mut heuristic_total) = (0u64, 0u64);
    let (mut small_total, mut small_first, mut small_last) = (0u64, 0u64, 0u64);
    for i in 0..ITEMS {
        let item = at_rung(i, 4);
        let p = q(&item);
        let ReadPriceAsk::SetPrice { kind, options } = p.ask else { panic!("{p:?}") };
        let right = options.iter().position(|&b| u64::from(b) * 46 > u64::from(outs(kind)) * u64::from(p.pot_before + 2 * b)).unwrap();
        let guess = options.iter().position(|&b| b * 10 >= p.pot_before * 3).unwrap_or(3);
        if !matches!(kind, DrawKind::Flush | DrawKind::OpenEnded) {
            heuristic_total += 1;
            heuristic_right += u64::from(guess == right);
        }
        if p.pot_before <= 20 {
            small_total += 1;
            small_first += u64::from(right == 0);
            small_last += u64::from(right == 3);
        }
    }
    let score = 100.0 * heuristic_right as f64 / heuristic_total as f64;
    println!("the 30%-of-the-pot guess scores {score:.1}% on the other draws");
    assert!(score < 40.0, "{score:.1}%");
    assert!(small_total > 50, "{small_total} small pots dealt");
    let (first, last) = (100.0 * small_first as f64 / small_total as f64, 100.0 * small_last as f64 / small_total as f64);
    assert!(first < 35.0 && last > 15.0, "first {first:.1}%, last {last:.1}% of {small_total}");
}

#[test]
fn rung_four_explains_a_wrong_pick_truthfully() {
    let (mut low, mut high) = (0, 0);
    for i in 0..ITEMS {
        let item = at_rung(i, 4);
        let p = q(&item);
        let ReadPriceAsk::SetPrice { kind, options } = p.ask else { panic!("{p:?}") };
        let o = outs(kind);
        let share = share_of(o, false);
        let right = options.iter().position(|&b| u64::from(b) * 46 > u64::from(o) * u64::from(p.pot_before + 2 * b)).unwrap();
        for (index, code) in ["a", "b", "c", "d"].into_iter().enumerate() {
            let r = reveal(&item, code);
            if index == right {
                continue;
            }
            assert_eq!(r.lines.len(), 3, "{:?}", r.lines);
            let line = &r.lines[1];
            let chosen = options[index];
            if index < right {
                let asked = 100.0 * f64::from(chosen) / f64::from(p.pot_before + 2 * chosen);
                let shown = if asked < 0.5 { "under 1%".to_string() } else { format!("{asked:.0}%") };
                assert_eq!(*line, format!("${chosen} only asks them for {shown}, and their hand gets there about {share:.0}%, so they can still call."));
                assert!(asked < share, "{p:?}");
                low += 1;
            } else {
                assert_eq!(*line, format!("${chosen} also makes their call a mistake, but ${} is the smallest of these that does.", options[right]));
                assert!(u64::from(chosen) * 46 > u64::from(o) * u64::from(p.pot_before + 2 * chosen), "{p:?}");
                high += 1;
            }
        }
    }
    assert!(low > 500 && high > 500, "{low} low, {high} high");
}

#[test]
fn every_choice_rung_has_exactly_one_right_option() {
    for rung in [0, 2, 3, 4] {
        for i in 0..500 {
            let item = at_rung(i, rung);
            let AnswerFormat::Choice(options) = answer_format(&item.question) else { panic!("rung {rung} is a choice") };
            let right = options.iter().filter(|o| grade(&item, o.code).unwrap().correct).count();
            assert_eq!(right, 1, "{:?}", q(&item));
        }
    }
}

#[test]
fn bad_answers_are_rejected_not_counted() {
    for rung in [0, 2, 3, 4] {
        assert!(grade(&at_rung(1, rung), "zzz").is_err(), "rung {rung}");
    }
    assert!(grade(&at_rung(1, 1), "abc").is_err());
    assert!(grade(&at_rung(1, 4), "").is_err());
}

fn fam(item: &Item) -> String {
    family_of(item).expect("a family")
}

#[test]
fn families_are_rung_plus_answer_class_or_draw_and_siblings_match() {
    let wanted = [
        ("0:cheap", 0u8), ("0:fair", 0), ("0:expensive", 0), ("1:cheap", 1), ("1:fair", 1), ("1:expensive", 1),
        ("2:easy", 2), ("2:close", 2), ("2:fold", 2), ("3:flush", 3), ("3:open", 3), ("3:gutshot", 3), ("3:overs", 3),
        ("3:flushopen", 3), ("3:flushgut", 3), ("4:flush", 4), ("4:gutshot", 4), ("4:flushopen", 4),
    ];
    let mut seen = HashMap::new();
    for rung in 0..READ_PRICE_RUNGS {
        for i in 0..800 {
            let item = at_rung(i, rung);
            let family = fam(&item);
            assert!(family.starts_with(&format!("{rung}:")), "{family}");
            *seen.entry(family).or_insert(0u32) += 1;
        }
    }
    for (family, rung) in wanted {
        assert!(seen.contains_key(family), "family {family} never dealt: {seen:?}");
        let sibling = find_sibling(Skill::ReadPrice, family, 7, u64::MAX).unwrap_or_else(|| panic!("no sibling for {family}"));
        assert!(!is_held_out(sibling), "{family}");
        assert_eq!(read_price_rung_of_seed(sibling), rung, "{family}");
        assert_eq!(fam(&generate(Skill::ReadPrice, sibling)), family);
    }
}

fn logged(item: &Item, t: u64, correct: bool) -> Attempt {
    let planned = Planned { skill: Skill::ReadPrice, seed: item.seed, origin: Origin::New };
    let AnswerFormat::Choice(options) = answer_format(&item.question) else { panic!("use a choice rung") };
    let code = options.iter().find(|o| grade(item, o.code).unwrap().correct == correct).expect("an answer").code;
    Attempt::from_drill(planned, &grade(item, code).unwrap(), t, 1500)
}

fn rung_log(rung: u8, count: u64, right_every: u64, offset: u64) -> Vec<Attempt> {
    (0..count).map(|i| logged(&at_rung(i + offset, rung), i + offset, i % right_every != 0)).collect()
}

#[test]
fn retries_do_not_count_toward_promotion() {
    let retry = |i: u64| {
        let item = at_rung(i, 0);
        let planned = Planned { skill: Skill::ReadPrice, seed: item.seed, origin: Origin::SessionRetry };
        let AnswerFormat::Choice(options) = answer_format(&item.question) else { panic!("rung 0 is a choice") };
        let code = options.iter().find(|o| grade(&item, o.code).unwrap().correct).unwrap().code;
        Attempt::from_drill(planned, &grade(&item, code).unwrap(), i, 1500)
    };
    let log: Vec<Attempt> = (0..40).map(retry).collect();
    assert_eq!(read_price_rung(&log), 0, "only first-time answers promote");
}

#[test]
fn the_rung_rises_at_85_percent_of_twenty_and_stays_up() {
    assert_eq!(read_price_rung(&[]), 0);
    // 17 of 20 is exactly 85%.
    let passing: Vec<Attempt> = (0..20u64).map(|i| logged(&at_rung(i, 0), i, i >= 3)).collect();
    assert_eq!(read_price_rung(&passing), 1);
    let failing: Vec<Attempt> = (0..20u64).map(|i| logged(&at_rung(i, 0), i, i >= 4)).collect();
    assert_eq!(read_price_rung(&failing), 0, "16 of 20 is not enough");
    let mut log = rung_log(0, 19, 1000, 0);
    assert_eq!(read_price_rung(&log), 0, "needs a full window");
    log.push(logged(&at_rung(19, 0), 19, true));
    assert_eq!(read_price_rung(&log), 1);
    // A later bad run at a passed rung doesn't demote.
    log.extend(rung_log(0, 20, 1, 100));
    assert_eq!(read_price_rung(&log), 1);
    let status = read_price_status(&log);
    assert_eq!(status.rung, 1);
    assert_eq!((status.recent_right, status.recent_total), (0, 0));
}

#[test]
fn the_ladder_stops_at_the_top_rung_and_reports_recent_accuracy() {
    let mut log = Vec::new();
    for rung in [0u8, 2, 3] {
        log.extend(rung_log(rung, 20, 1000, u64::from(rung) * 1000));
    }
    // Rung 1 isn't a choice item, so log it by hand: 20 right answers.
    for i in 0..20u64 {
        let item = at_rung(i + 5000, 1);
        let answer = format!("{}", price(q(&item)));
        let planned = Planned { skill: Skill::ReadPrice, seed: item.seed, origin: Origin::New };
        log.push(Attempt::from_drill(planned, &grade(&item, &answer).unwrap(), 5000 + i, 1500));
    }
    log.sort_by_key(|a| a.timestamp_ms);
    assert_eq!(read_price_rung(&log), 4);
    assert_eq!(read_price_status(&log).recent_total, 0);
    let more: Vec<Attempt> = (0..READ_PRICE_PROMOTE_WINDOW as u64 + 3).map(|i| logged(&at_rung(i, 4), 9000 + i, i % 4 != 0)).collect();
    log.extend(more);
    assert_eq!(read_price_rung(&log), 4, "there is no rung past the top");
    let status = read_price_status(&log);
    assert_eq!(status.recent_total, READ_PRICE_PROMOTE_WINDOW, "only the last window");
    assert_eq!(status.recent_right, 15);
}

#[test]
fn only_the_decision_rungs_count_for_the_mark() {
    let low = rung_log(0, 40, 1000, 0);
    assert_eq!(skill_stats(Skill::ReadPrice, &low).total_attempts, 0, "rungs 0-1 are practice");
    let high = rung_log(2, 40, 1000, 0);
    let stats = skill_stats(Skill::ReadPrice, &high);
    assert_eq!(stats.total_attempts, 40);
    assert!(stats.drill_passed, "{stats:?}");
    let four = rung_log(4, 40, 1000, 0);
    assert!(skill_stats(Skill::ReadPrice, &four).drill_passed, "rung 4 counts");
    let mut poor = rung_log(2, 40, 1000, 0);
    for row in poor.iter_mut().take(10) {
        row.correct = false;
    }
    assert!(!skill_stats(Skill::ReadPrice, &poor).drill_passed);
}

#[test]
fn checkups_take_eight_of_each_rung_from_held_out_seeds() {
    for form in 0..30 {
        let plan = checkup_plan(Skill::ReadPrice, form);
        let originals: Vec<_> = plan.slots.iter().filter(|s| s.repeat_of.is_none()).collect();
        assert_eq!(originals.len(), 40);
        assert!(plan.slots.iter().all(|s| is_held_out(s.seed)));
        let by_rung = counted(originals.iter().map(|s| read_price_rung_of_seed(s.seed)));
        assert_eq!(by_rung.len(), 5, "{by_rung:?}");
        assert!(by_rung.values().all(|&n| n == 8), "{by_rung:?}");
        for (i, slot) in plan.slots.iter().enumerate() {
            if let Some(original) = slot.repeat_of {
                assert_eq!(plan.item(i).question, plan.item(original).question, "no cards to relabel");
            }
        }
    }
}

#[test]
fn it_is_dealt_from_day_one_outside_the_steps_and_asks_confidence() {
    assert_eq!(Skill::ReadPrice.step(), None);
    assert!(STEPS.iter().all(|step| !step.contains(&Skill::ReadPrice)));
    for step in 1..=STEPS.len() {
        assert!(skills_up_to_step(step).contains(&Skill::ReadPrice), "step {step}");
    }
    assert!(asks_confidence(Skill::ReadPrice));
    assert_eq!(Skill::from_name("read_price"), Some(Skill::ReadPrice));
}

#[test]
fn reflection_is_offered_on_a_missed_decision_with_true_cues() {
    let mut seen = 0;
    for i in 0..200 {
        for rung in [0u8, 1, 2, 3, 4] {
            let item = at_rung(i, rung);
            let AnswerFormat::Choice(options) = answer_format(&item.question) else {
                let outcome = grade(&item, "99").unwrap();
                assert!(!reflection_eligible(&item, &outcome), "rung 1 has no form");
                continue;
            };
            let wrong = options.iter().find(|o| !grade(&item, o.code).unwrap().correct).unwrap();
            let outcome = grade(&item, wrong.code).unwrap();
            let eligible = reflection_eligible(&item, &outcome);
            assert_eq!(eligible, rung == 2 || rung == 3, "rung {rung}");
            if eligible {
                let form = reflection_form(&item, &outcome).expect("a form");
                assert_eq!(form.len(), 3);
                let cues: Vec<&str> = form.iter().flat_map(|p| p.cues.iter().map(|c| c.text.as_str())).collect();
                let p = q(&item);
                let needed = if price(p) < 0.5 { "The price: under 1% needed".to_string() } else { format!("The price: about {:.0}% needed", price(p)) };
                assert!(cues.contains(&needed.as_str()), "{cues:?}");
                for prompt in &form {
                    assert!((3..=4).contains(&prompt.cues.len()), "{}: {:?}", prompt.text, prompt.cues);
                }
                let joined = cues.join(" | ");
                assert!(!joined.contains("outs may not"), "{joined}");
                let (read, all_in) = match p.ask {
                    ReadPriceAsk::Stated(PriceHand::Read { .. }) => (true, false),
                    ReadPriceAsk::Stated(PriceHand::Draw { street, .. }) | ReadPriceAsk::Recall { street, .. } => (false, street == HitStreet::Flop),
                    _ => panic!("{p:?}"),
                };
                assert_eq!(joined.contains("Later betting"), !all_in, "{joined}");
                assert_eq!(joined.contains("no more betting"), all_in, "{joined}");
                assert_eq!(joined.contains("than you figured"), read, "a draw's share is stated exactly: {joined}");
                assert_eq!(joined.contains("Two cards are still to come"), !read && all_in, "{joined}");
                assert_eq!(joined.contains("One card is still to come"), !read && !all_in, "{joined}");
                assert!(!joined.contains("1 callers"), "{joined}");
                seen += 1;
            }
        }
    }
    assert!(seen > 300);
}

/// (read_price share, preflop share) over many default drills, in percent.
fn default_shares(step: usize, sessions: u64, items: u64, miss_one_in: u64) -> (f64, f64) {
    let (mut read_price, mut preflop, mut total) = (0u64, 0u64, 0u64);
    for seed in 0..sessions {
        let mut session = Session::new(skills_up_to_step(step), &[], 0, seed * 7919 + 1).with_preflop_floor();
        let mut misses = Rng::new(seed);
        for _ in 0..items {
            let planned = session.try_next_item().expect("always an item");
            read_price += u64::from(planned.skill == Skill::ReadPrice);
            preflop += u64::from(planned.skill == Skill::Preflop);
            total += 1;
            let ok = miss_one_in == 0 || !misses.chance(1, miss_one_in);
            session.record(planned, ok);
        }
    }
    (100.0 * read_price as f64 / total as f64, 100.0 * preflop as f64 / total as f64)
}

#[test]
fn the_default_drill_deals_it_at_12_to_15_percent_without_hurting_preflop() {
    for (label, step, miss) in [("step 1 perfect", 1, 0), ("step 1 misses", 1, 4), ("step 3", 3, 0), ("step 4", 4, 0)] {
        let (share, preflop) = default_shares(step, 200, 150, miss);
        println!("{label}: read_price {share:.1}%, preflop {preflop:.1}%");
        assert!((12.0..=15.5).contains(&share), "{label}: read_price at {share:.1}%");
        assert!(preflop >= MIN_PREFLOP_PERCENT as f64 - 0.5, "{label}: preflop at {preflop:.1}%");
    }
}

/// Dealing the next item when preflop is owed must deal preflop, so its share
/// holds at every point in a session, not just on average. Step 1 has no chain,
/// whose stages are dealt straight after each other.
#[test]
fn preflop_never_falls_below_its_floor_at_any_point() {
    for seed in 0..300u64 {
        let mut session = Session::new(skills_up_to_step(1), &[], 0, seed * 104_729 + 3).with_preflop_floor();
        let mut misses = Rng::new(seed);
        let (mut preflop, mut dealt) = (0u64, 0u64);
        for _ in 0..80 {
            let planned = session.try_next_item().unwrap();
            preflop += u64::from(planned.skill == Skill::Preflop);
            dealt += 1;
            assert!(preflop * 100 >= dealt * MIN_PREFLOP_PERCENT, "seed {seed}: {preflop} of {dealt}");
            session.record(planned, !misses.chance(1, 4));
        }
    }
}

/// Without aiming at the family's rung, most tries land on the wrong rung and
/// a rare family would sometimes not turn up inside the search budget.
#[test]
fn the_sibling_search_targets_the_rung() {
    for family in ["3:gutshot", "4:overs"] {
        for base in 0..3000u64 {
            let found = find_sibling(Skill::ReadPrice, family, base, u64::MAX);
            assert!(found.is_some(), "{family} from base {base}");
        }
    }
}

/// A player who always prices the bet as if nobody had called.
fn ignore_callers_score(rung: u8) -> f64 {
    let right = (0..ITEMS)
        .filter(|&i| {
            let item = at_rung(i, rung);
            let p = q(&item);
            let ignored = 100.0 * f64::from(p.bet) / f64::from(p.pot_before + 2 * p.bet);
            let call_for = |share: f64| {
                let edge = share - ignored;
                if edge >= 6.0 {
                    PriceCall::Easy
                } else if edge <= -6.0 {
                    PriceCall::Fold
                } else {
                    PriceCall::Close
                }
            };
            match p.ask {
                ReadPriceAsk::Band => band_from(ignored) == band_from(price(p)),
                ReadPriceAsk::Number => (ignored - price(p)).abs() <= 2.0,
                ReadPriceAsk::Stated(PriceHand::Read { share_pct }) => {
                    Some(call_for(f64::from(share_pct))) == truth_call(f64::from(share_pct), price(p))
                }
                ReadPriceAsk::Stated(PriceHand::Draw { kind, street }) | ReadPriceAsk::Recall { kind, street } => {
                    let share = share_of(outs(kind), street == HitStreet::Flop);
                    Some(call_for(share)) == truth_call(share, price(p))
                }
                ReadPriceAsk::SetPrice { .. } => panic!("rung 4 has no callers"),
            }
        })
        .count();
    100.0 * right as f64 / ITEMS as f64
}

#[test]
fn a_player_who_ignores_callers_does_not_get_promoted() {
    for rung in 0..=3u8 {
        let score = ignore_callers_score(rung);
        println!("rung {rung}: ignoring callers scores {score:.1}%");
        assert!(score < 85.0, "rung {rung}: ignoring callers scores {score:.1}%");
    }
}

#[test]
fn leaving_callers_out_changes_the_answer_on_most_caller_items() {
    for rung in 0..=2u8 {
        let (mut with_callers, mut changed) = (0, 0);
        for i in 0..ITEMS {
            let item = at_rung(i, rung);
            let p = q(&item);
            if p.callers == 0 {
                continue;
            }
            with_callers += 1;
            let ignored = 100.0 * f64::from(p.bet) / f64::from(p.pot_before + 2 * p.bet);
            let different = match p.ask {
                ReadPriceAsk::Band => band_from(ignored) != band_from(price(p)),
                ReadPriceAsk::Number => (ignored - price(p)).abs() > 2.0,
                ReadPriceAsk::Stated(hand) => {
                    let share = match hand {
                        PriceHand::Read { share_pct } => f64::from(share_pct),
                        PriceHand::Draw { kind, street } => share_of(outs(kind), street == HitStreet::Flop),
                    };
                    truth_call(share, ignored) != truth_call(share, price(p))
                }
                _ => false,
            };
            changed += u64::from(different);
        }
        assert!(with_callers > 500, "rung {rung}");
        assert!(changed * 100 >= with_callers * 45, "rung {rung}: {changed} of {with_callers} change");
    }
    let item = Item {
        skill: Skill::ReadPrice,
        seed: 0,
        question: Question::ReadPrice(ReadPriceQuestion { pot_before: 10, bet: 5, callers: 2, ask: ReadPriceAsk::Band }),
    };
    let p = q(&item);
    assert_eq!(band_from(price(p)), PriceBand::Cheap, "16.7%");
    assert_eq!(band_from(25.0), PriceBand::Fair);
    assert!(grade(&item, "c").unwrap().correct);
}

#[test]
fn no_skill_is_dealt_three_times_running() {
    for seed in 0..300u64 {
        let mut session = Session::new(skills_up_to_step(1), &[], 0, seed * 15_485_863 + 11).with_preflop_floor();
        let mut misses = Rng::new(seed ^ 77);
        let mut last = Vec::new();
        for _ in 0..80 {
            let planned = session.try_next_item().unwrap();
            last.push(planned.skill);
            if last.len() >= 3 {
                let tail = &last[last.len() - 3..];
                assert!(!(tail[0] == tail[1] && tail[1] == tail[2]), "seed {seed}: three of {:?} in a row", tail[0]);
            }
            session.record(planned, !misses.chance(1, 5));
        }
    }
}

fn missed(item: &Item, t: u64) -> Attempt {
    let planned = Planned { skill: Skill::ReadPrice, seed: item.seed, origin: Origin::New };
    let raw = match answer_format(&item.question) {
        AnswerFormat::Choice(options) => options.iter().find(|o| !grade(item, o.code).unwrap().correct).unwrap().code.to_string(),
        _ => "99".to_string(),
    };
    Attempt::from_drill(planned, &grade(item, &raw).unwrap(), t, 1500)
}

#[test]
fn reviews_take_at_most_half_the_slots_and_new_items_keep_coming() {
    let mut log = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for rung in 0..READ_PRICE_RUNGS {
        for i in 0..400 {
            let item = at_rung(i, rung);
            if seen.insert(fam(&item)) {
                log.push(missed(&item, log.len() as u64));
            }
        }
    }
    assert!(log.len() > 12, "{} families", log.len());
    let now = log.len() as u64 + 2 * DAY_MS;
    let mut session = Session::new(vec![Skill::ReadPrice], &log, now, 5);
    let (mut slots, mut reviews, mut new_items) = (0u64, 0u64, 0u64);
    for _ in 0..24 {
        let planned = session.try_next_item().unwrap();
        slots += 1;
        reviews += u64::from(planned.origin == Origin::Due);
        new_items += u64::from(planned.origin == Origin::New);
        assert!(reviews * 2 <= slots + 1, "{reviews} reviews in {slots} slots");
        if planned.origin == Origin::New {
            assert_eq!(read_price_rung_of_seed(planned.seed), 0, "new items stay at the player's rung");
        }
        session.record(planned, true);
    }
    assert!(reviews >= 8, "reviews still happen: {reviews}");
    assert!(new_items >= 8, "new items still come: {new_items}");
}

#[test]
fn tiny_prices_read_under_one_percent() {
    let band = Item {
        skill: Skill::ReadPrice,
        seed: 0,
        question: Question::ReadPrice(ReadPriceQuestion { pot_before: 300, bet: 1, callers: 0, ask: ReadPriceAsk::Band }),
    };
    assert!(reveal(&band, "c").lines[0].ends_with("= under 1%."), "{:?}", reveal(&band, "c").lines);
    let stated = Item {
        skill: Skill::ReadPrice,
        seed: 0,
        question: Question::ReadPrice(ReadPriceQuestion {
            pot_before: 300,
            bet: 1,
            callers: 0,
            ask: ReadPriceAsk::Stated(PriceHand::Read { share_pct: 40 }),
        }),
    };
    assert!(reveal(&stated, "c").lines[0].ends_with("= under 1%."), "{:?}", reveal(&stated, "c").lines);
    let number = Item {
        skill: Skill::ReadPrice,
        seed: 0,
        question: Question::ReadPrice(ReadPriceQuestion { pot_before: 300, bet: 1, callers: 0, ask: ReadPriceAsk::Number }),
    };
    assert_eq!(reveal(&number, "1").answer, "0.3%", "the number rung keeps its decimal");
}

fn form_for(ask: ReadPriceAsk, pot_before: u32, bet: u32, callers: u32, picked: &str) -> Vec<(String, Vec<String>)> {
    let item = Item {
        skill: Skill::ReadPrice,
        seed: 1,
        question: Question::ReadPrice(ReadPriceQuestion { pot_before, bet, callers, ask }),
    };
    let outcome = grade(&item, picked).unwrap();
    assert!(!outcome.correct, "the form only follows a miss: {picked}");
    reflection_form(&item, &outcome)
        .expect("an eligible miss")
        .into_iter()
        .map(|p| (p.id.to_string(), p.cues.into_iter().map(|c| c.text).collect()))
        .collect()
}

fn cues<'a>(form: &'a [(String, Vec<String>)], id: &str) -> &'a [String] {
    &form.iter().find(|(prompt, _)| prompt == id).expect("a prompt").1
}

fn has(list: &[String], text: &str) -> bool {
    list.iter().any(|c| c.contains(text))
}

#[test]
fn the_reflection_cues_argue_for_the_side_they_are_on() {
    // Share 60%, price 16.7%: the share argues for calling, so against a fold.
    let read = ReadPriceAsk::Stated(PriceHand::Read { share_pct: 60 });
    let fold = form_for(read, 10, 5, 2, "f");
    assert!(has(cues(&fold, "supported"), "You pay $5 to win $25"), "callers' money counts: {fold:?}");
    assert!(!has(cues(&fold, "supported"), "Your share"), "{fold:?}");
    assert!(has(cues(&fold, "against"), "Your share: about 60%"), "{fold:?}");
    assert!(has(cues(&fold, "supported"), "The price: about 17% needed"), "{fold:?}");
    // Share 15%, price 40%: the share argues for folding, so against an easy call.
    let low = ReadPriceAsk::Stated(PriceHand::Read { share_pct: 15 });
    let easy = form_for(low, 5, 10, 0, "c");
    assert!(!has(cues(&easy, "supported"), "Your share"), "{easy:?}");
    assert!(has(cues(&easy, "against"), "Your share: about 15%"), "{easy:?}");
    assert!(has(cues(&easy, "supported"), "The price: about 40% needed"), "{easy:?}");
    assert!(has(cues(&easy, "against"), "You lose the $10 when you're behind"), "{easy:?}");
}

#[test]
fn the_price_cue_survives_the_cue_limit() {
    // Callers, a flop draw and share above the price makes the longest support list.
    let draw = ReadPriceAsk::Stated(PriceHand::Draw { kind: DrawKind::Flush, street: HitStreet::Flop });
    let form = form_for(draw, 2, 10, 1, "c");
    let supported = cues(&form, "supported");
    assert_eq!(supported.len(), 4, "{supported:?}");
    assert!(supported[0].starts_with("The price:"), "price first: {supported:?}");
    assert!(has(supported, "Your share"), "{supported:?}");
}

#[test]
fn a_wrong_close_gets_its_own_cues() {
    // Share 60%, price 16.7%: a gap far wider than close allows.
    let read = ReadPriceAsk::Stated(PriceHand::Read { share_pct: 60 });
    let form = form_for(read, 10, 5, 2, "l");
    let supported = cues(&form, "supported");
    assert!(has(supported, "A gap of about 43 points"), "{supported:?}");
    assert!(!has(supported, "lose"), "{supported:?}");
    let against = cues(&form, "against");
    assert!(has(against, "Close only covers a share within 4 points of the price"), "{against:?}");
    // No fold argument on an all-in close, and no betting talk where none can follow.
    let draw = ReadPriceAsk::Stated(PriceHand::Draw { kind: DrawKind::Flush, street: HitStreet::Flop });
    let form = form_for(draw, 200, 5, 0, "l");
    assert!(!has(cues(&form, "supported"), "All-in"), "{form:?}");
    assert!(!has(cues(&form, "supported"), "Later betting"), "{form:?}");
    assert!(!has(cues(&form, "against"), "Later betting"), "{form:?}");
    let fold = form_for(draw, 200, 5, 0, "f");
    assert!(!has(cues(&fold, "supported"), "All-in"), "{fold:?}");
    assert!(!has(cues(&fold, "supported"), "Later betting"), "{fold:?}");
}

#[test]
fn one_caller_reads_in_the_singular() {
    let item = Item {
        skill: Skill::ReadPrice,
        seed: 0,
        question: Question::ReadPrice(ReadPriceQuestion { pot_before: 10, bet: 5, callers: 1, ask: ReadPriceAsk::Number }),
    };
    assert_eq!(reveal(&item, "20").lines[0], "Call $5 to win $20 (the $10 pot, the $5 bet and $5 from the caller): 5 \u{f7} 25 = 20.0%.");
    let read = ReadPriceAsk::Stated(PriceHand::Read { share_pct: 15 });
    let form = form_for(read, 10, 5, 1, "c");
    assert!(form.iter().any(|(_, list)| has(list, "1 caller's money is in the pot")), "{form:?}");
}

#[test]
fn a_drill_that_asks_for_named_skills_uses_the_normal_weights() {
    let mut counts = HashMap::new();
    for seed in 0..100 {
        let mut session = Session::new(vec![Skill::ReadPrice, Skill::Price], &[], 0, seed);
        for _ in 0..100 {
            let planned = session.try_next_item().unwrap();
            *counts.entry(planned.skill).or_insert(0u32) += 1;
            session.record(planned, true);
        }
    }
    let share = f64::from(counts[&Skill::ReadPrice]) / 100.0;
    assert!((40.0..=60.0).contains(&share), "{share}: {counts:?}");
}

#[test]
fn a_new_item_is_dealt_at_the_players_rung() {
    let log = rung_log(0, 20, 1000, 0);
    let mut session = Session::new(vec![Skill::ReadPrice], &log, 1, 42);
    for _ in 0..40 {
        let planned = session.try_next_item().unwrap();
        assert_eq!(read_price_rung_of_seed(planned.seed), 1);
        session.record(planned, true);
    }
}

#[test]
fn a_miss_comes_back_as_a_fresh_sibling_of_the_same_family() {
    let mut session = Session::new(vec![Skill::ReadPrice, Skill::Price], &[], 1, 9);
    let mut missed: Vec<(u64, String)> = Vec::new();
    let mut retried = 0;
    for _ in 0..200 {
        let planned = session.try_next_item().unwrap();
        if planned.skill == Skill::ReadPrice && planned.origin == Origin::SessionRetry {
            let family = fam(&generate(Skill::ReadPrice, planned.seed));
            assert!(missed.iter().any(|(seed, f)| *f == family && *seed != planned.seed), "{family} {missed:?}");
            retried += 1;
        }
        let miss = planned.skill == Skill::ReadPrice && planned.origin == Origin::New;
        if miss {
            missed.push((planned.seed, fam(&generate(Skill::ReadPrice, planned.seed))));
        }
        session.record(planned, !miss);
    }
    assert!(retried > 3, "{retried}");
}

