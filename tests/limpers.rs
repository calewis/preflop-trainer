//! Tests for the Limpers preflop drill: Q7 table cells, raise sizing, grading,
//! generator invariants & weighting, families, checkup backward compatibility,
//! and Preflop mistakes integration.

use std::collections::HashMap;

use poker_trainer::answer::{answer_format, AnswerFormat};
use poker_trainer::cards::parse_holding;
use poker_trainer::checkup::{
    checkup_runs, legacy_mixed_checkup_plan, mixed_checkup_plan, unfinished_run, CheckupKind, MIXED_FORM_BASE,
};
use poker_trainer::family::{family_of, find_sibling};
use poker_trainer::generate::generate;
use poker_trainer::grade::{grade, Reveal};
use poker_trainer::groups::{group_of, Group};
use poker_trainer::item::{Item, Question, Skill, GEN_VERSION};
use poker_trainer::leaks::{parse_leaks, verify_leaks, LeakSession};
use poker_trainer::limpers::{
    answer, is_boundary, is_priority_hand, lookup, lookup_class, raise_size_bb, table_row, LimperDecision,
    LimperReason, LimperSeat, LimpersQuestion, RowSeat, TABLE,
};
use poker_trainer::log::{Attempt, AttemptExtra, CheckRef, ItemRef, Mode};

fn h(text: &str) -> poker_trainer::cards::Holding {
    parse_holding(text).unwrap_or_else(|| panic!("bad holding {text}"))
}

fn limpers_q(item: &Item) -> &LimpersQuestion {
    match &item.question {
        Question::Limpers(q) => q,
        other => panic!("expected Limpers question, got {other:?}"),
    }
}

#[test]
fn every_cell_of_the_source_table_has_hands_asserted() {
    use LimperDecision::{Call, Fold, Raise, Unspecified};
    use LimperSeat::*;

    // Pairwise disjointness across all 10 table rows and all 169 starting classes.
    for row in &TABLE {
        for high in 0..13u8 {
            for low in 0..=high {
                let suiteds: &[bool] = if high == low { &[false] } else { &[true, false] };
                for &suited in suiteds {
                    let hits = u8::from(row.raise.contains(high, low, suited))
                        + u8::from(row.call.contains(high, low, suited))
                        + u8::from(row.fold.contains(high, low, suited));
                    assert!(hits <= 1, "overlap in {:?} mw={} ({high},{low},{suited})", row.seat, row.multiway);
                }
            }
        }
    }

    // --- Vs one limper table (every seat x Raise / Overlimp / Fold) ---
    // Early: Raise 88+, AJs+, KQs, AQo+ | Call 22-77, A2s-ATs, KJs, QJs, JTs, T9s-65s | Fold offsuit except above, K9s & worse suited gappers.
    for hand in ["AsAd", "8s8d", "AsJs", "KsQs", "AhQd", "AcKd"] {
        assert_eq!(lookup(Utg1, 1, h(hand)), Raise, "Early vs 1 Raise {hand}");
    }
    for hand in ["7s7d", "2c2h", "AsTs", "As2s", "KsJs", "QsJs", "JsTs", "Ts9s", "6s5s"] {
        assert_eq!(lookup(Utg1, 1, h(hand)), Call, "Early vs 1 Call {hand}");
    }
    for hand in ["AhJd", "KhQd", "Ks9s", "Ks2s", "Qs9s", "Js9s", "Ts8s", "7s5s", "7c2d"] {
        assert_eq!(lookup(Utg1, 1, h(hand)), Fold, "Early vs 1 Fold {hand}");
    }
    for hand in ["KsTs", "QsTs", "5s4s", "4s3s", "3s2s"] {
        assert_eq!(lookup(Utg1, 1, h(hand)), Unspecified, "Early vs 1 Unspecified {hand}");
    }

    // CO: Raise 77+, ATs+, KJs+, QJs, AJo+, KQo | Call 22-66, A2s-A9s, KTs, QTs, J9s+, T8s+, 97s+, 54s-87s | Fold KJo, QJo, ATo, weaker offsuit & weaker suited.
    for hand in ["7s7h", "AsTs", "KsJs", "QsJs", "AhJd", "KhQd"] {
        assert_eq!(lookup(Co, 1, h(hand)), Raise, "CO vs 1 Raise {hand}");
    }
    for hand in ["6s6h", "2s2h", "As9s", "As2s", "KsTs", "QsTs", "JsTs", "Js9s", "Ts8s", "9s7s", "8s7s", "5s4s"] {
        assert_eq!(lookup(Co, 1, h(hand)), Call, "CO vs 1 Call {hand}");
    }
    for hand in ["KhJd", "QhJd", "AhTd", "JhTd", "Ks9s", "Qs9s", "Js8s", "8s6s", "4s3s"] {
        assert_eq!(lookup(Co, 1, h(hand)), Fold, "CO vs 1 Fold {hand}");
    }

    // BTN: Raise 77+, A9s+, KTs+, QTs+, JTs, ATo+, KJo+ | Call 22-66, A2s-A8s, 54s+, 75s+, K9s, Q9s, QJo, JTo | Fold other offsuit & weaker suited.
    for hand in ["7s7h", "As9s", "KsTs", "QsTs", "JsTs", "AhTd", "KhJd"] {
        assert_eq!(lookup(Btn, 1, h(hand)), Raise, "BTN vs 1 Raise {hand}");
    }
    for hand in ["6s6h", "2s2h", "As8s", "As2s", "Ts9s", "5s4s", "Js9s", "7s5s", "Ks9s", "Qs9s", "QhJd", "JhTd"] {
        assert_eq!(lookup(Btn, 1, h(hand)), Call, "BTN vs 1 Call {hand}");
    }
    for hand in ["Ah9d", "KhTd", "QhTd", "Th9d", "Ks8s", "Qs8s", "Js8s", "6s4s", "4s3s"] {
        assert_eq!(lookup(Btn, 1, h(hand)), Fold, "BTN vs 1 Fold {hand}");
    }

    // SB: Raise 99+, AJs+, KQs, AQo+ | Call pairs, suited, Broadways, T9o+ | Fold weak offsuit (K7o, Q5o, J4o and worse).
    for hand in ["9s9h", "AsJs", "KsQs", "AhQd"] {
        assert_eq!(lookup(Sb, 1, h(hand)), Raise, "SB vs 1 Raise {hand}");
    }
    for hand in ["8s8h", "2s2h", "AsTs", "Ks6s", "Qs4s", "7s2s", "AhJd", "KhTd", "QhJd", "JhTd", "Th9d"] {
        assert_eq!(lookup(Sb, 1, h(hand)), Call, "SB vs 1 Call {hand}");
    }
    for hand in ["Kh7d", "Kh2d", "Qh5d", "Jh4d", "Th6d", "7h2d"] {
        assert_eq!(lookup(Sb, 1, h(hand)), Fold, "SB vs 1 Fold {hand}");
    }
    for hand in ["Ah9d", "Kh8d", "Qh6d", "Jh5d", "9h8d"] {
        assert_eq!(lookup(Sb, 1, h(hand)), Unspecified, "SB vs 1 Unspecified {hand}");
    }

    // BB: Raise TT+, AQs+, AKo | Check everything else.
    for hand in ["TsTh", "AsQs", "AhKd"] {
        assert_eq!(lookup(Bb, 1, h(hand)), Raise, "BB vs 1 Raise {hand}");
    }
    for hand in ["9s9h", "AsJs", "AhQd", "KsQs", "7c2d"] {
        assert_eq!(lookup(Bb, 1, h(hand)), Call, "BB vs 1 Check {hand}");
    }

    // --- Vs two or more limpers ---
    // Early vs 2+: Raise TT+, AQs+, AKo | Call 22-99, A2s-AJs, suited Broadways, 54s+, 64s+ | Fold rest (AQo Unspecified).
    assert_eq!(lookup_class(RowSeat::Early, true, 8, 8, false), Raise); // TT
    assert_eq!(lookup_class(RowSeat::Early, true, 7, 7, false), Call); // 99
    assert_eq!(lookup_class(RowSeat::Early, true, 12, 10, false), Unspecified); // AQo
    assert_eq!(lookup_class(RowSeat::Early, true, 12, 9, false), Fold); // AJo

    // CO vs 2+: Raise 99+, AJs+, KQs, AQo+ | Call 22-88, A2s-ATs, suited Broadways, 54s+, 64s+ | Fold AJo, KQo, weaker.
    for hand in ["9s9h", "AsJs", "KsQs", "AhQd"] {
        assert_eq!(lookup(Co, 2, h(hand)), Raise, "CO vs 2+ Raise {hand}");
    }
    for hand in ["8s8h", "2s2h", "AsTs", "KsJs", "QsTs", "JsTs", "5s4s", "6s4s"] {
        assert_eq!(lookup(Co, 2, h(hand)), Call, "CO vs 2+ Call {hand}");
    }
    for hand in ["AhJd", "KhQd", "KhJd", "QhJd", "KhTd", "AhTd", "Ks9s", "Qs9s", "5s3s"] {
        assert_eq!(lookup(Co, 2, h(hand)), Fold, "CO vs 2+ Fold {hand}");
    }

    // BTN vs 2+: Raise 99+, AJs+, KQs, AQo+ | Call adds K9s, Q9s, AJo, KQo | Fold KJo, QJo, KTo, ATo, unconnected offsuit.
    for hand in ["9s9h", "AsJs", "KsQs", "AhQd"] {
        assert_eq!(lookup(Btn, 3, h(hand)), Raise, "BTN vs 2+ Raise {hand}");
    }
    for hand in ["8s8h", "AsTs", "Ks9s", "Qs9s", "6s4s", "AhJd", "KhQd"] {
        assert_eq!(lookup(Btn, 3, h(hand)), Call, "BTN vs 2+ Call {hand}");
    }
    for hand in ["KhJd", "QhJd", "KhTd", "AhTd", "Ah9d", "Ks8s", "5s3s"] {
        assert_eq!(lookup(Btn, 3, h(hand)), Fold, "BTN vs 2+ Fold {hand}");
    }
    for hand in ["JhTd", "Th9d", "5h4d"] {
        assert_eq!(lookup(Btn, 2, h(hand)), Unspecified, "BTN vs 2+ Unspecified {hand}");
    }

    // SB vs 2+: Raise TT+, AQs+, AKo | Call 22-99, suited A/K/Q/J/connectors/1-gappers, AQo/AJo/KQo, JTo/T9o-54o | Fold unconnected offsuit.
    for hand in ["TsTh", "AsQs", "AhKd"] {
        assert_eq!(lookup(Sb, 2, h(hand)), Raise, "SB vs 2+ Raise {hand}");
    }
    for hand in ["9s9h", "AsJs", "KsQs", "Ks3s", "Qs4s", "Js5s", "6s4s", "AhQd", "AhJd", "KhQd", "JhTd", "5h4d"] {
        assert_eq!(lookup(Sb, 2, h(hand)), Call, "SB vs 2+ Call {hand}");
    }
    for hand in ["Ah8d", "Kh7d", "Qh5d", "Jh3d", "8h4d"] {
        assert_eq!(lookup(Sb, 2, h(hand)), Fold, "SB vs 2+ Fold {hand}");
    }
    for hand in ["KhJd", "QhJd", "AhTd", "Ts6s", "4s3s"] {
        assert_eq!(lookup(Sb, 2, h(hand)), Unspecified, "SB vs 2+ Unspecified {hand}");
    }

    // BB vs 2+: Raise TT+, AQs+, AKo | Check everything else.
    assert_eq!(lookup(Bb, 3, h("TsTh")), Raise);
    assert_eq!(lookup(Bb, 3, h("9s9h")), Call);
    assert_eq!(lookup(Bb, 3, h("AhQd")), Call);
}

#[test]
fn raise_size_rule_matches_limpers_and_position() {
    for seat in [LimperSeat::Utg1, LimperSeat::Co, LimperSeat::Btn] {
        assert_eq!(raise_size_bb(seat, 1), 6, "{seat:?} vs 1");
        assert_eq!(raise_size_bb(seat, 2), 8, "{seat:?} vs 2");
        assert_eq!(raise_size_bb(seat, 3), 10, "{seat:?} vs 3");
    }
    for seat in [LimperSeat::Sb, LimperSeat::Bb] {
        assert_eq!(raise_size_bb(seat, 1), 7, "{seat:?} vs 1");
        assert_eq!(raise_size_bb(seat, 2), 9, "{seat:?} vs 2");
        assert_eq!(raise_size_bb(seat, 3), 11, "{seat:?} vs 3");
    }
}

#[test]
fn grading_accepts_only_valid_actions_and_rejects_fold_in_bb_and_unspecified_hands() {
    // Non-BB raise includes size from the size rule.
    let btn_raise = Item {
        skill: Skill::Limpers,
        seed: 1,
        question: Question::Limpers(LimpersQuestion {
            players: 5,
            seat: LimperSeat::Btn,
            limpers: vec![LimperSeat::Co],
            hand: h("AsQs"),
        }),
    };
    let AnswerFormat::Choice(opts) = answer_format(&btn_raise.question) else { panic!("choice") };
    assert_eq!(opts.iter().map(|o| o.label).collect::<Vec<_>>(), vec!["Raise", "Call", "Fold"]);
    let ok = grade(&btn_raise, "r").unwrap();
    assert!(ok.correct);
    let Reveal::Limpers(g) = ok.reveal else { panic!("limpers reveal") };
    assert_eq!(g.best_text(), "Raise to 6bb");
    assert_eq!(g.reason, LimperReason::ValueRaise);
    assert!(LimperReason::ALL.contains(&g.reason));
    assert!(!grade(&btn_raise, "c").unwrap().correct);
    assert!(grade(&btn_raise, "x").is_err());

    // BB offers Raise / Check and rejects Fold.
    let bb_check = Item {
        skill: Skill::Limpers,
        seed: 2,
        question: Question::Limpers(LimpersQuestion {
            players: 6,
            seat: LimperSeat::Bb,
            limpers: vec![LimperSeat::Utg, LimperSeat::Co],
            hand: h("Ks7s"),
        }),
    };
    let AnswerFormat::Choice(bb_opts) = answer_format(&bb_check.question) else { panic!("choice") };
    assert_eq!(bb_opts.iter().map(|o| o.label).collect::<Vec<_>>(), vec!["Raise", "Check"]);
    let bb_ok = grade(&bb_check, "c").unwrap();
    assert!(bb_ok.correct);
    let Reveal::Limpers(bg) = bb_ok.reveal else { panic!("limpers reveal") };
    assert_eq!(bg.best_text(), "Check");
    assert_eq!(bg.reason, LimperReason::BigBlindFreeFlop);
    assert!(grade(&bb_check, "f").is_err(), "fold must be rejected in BB");

    // Unspecified hand returns None from answer() and Err from grade().
    let unspec = Item {
        skill: Skill::Limpers,
        seed: 3,
        question: Question::Limpers(LimpersQuestion {
            players: 6,
            seat: LimperSeat::Utg1,
            limpers: vec![LimperSeat::Utg],
            hand: h("KsTs"),
        }),
    };
    assert!(answer(LimperSeat::Utg1, 1, h("KsTs")).is_none());
    assert!(grade(&unspec, "c").is_err());
}

#[test]
fn generator_obeys_invariants_and_never_deals_unspecified_hands() {
    let mut five = 0u32;
    let mut six = 0u32;
    let mut limper_counts = [0u32; 4];

    for seed in 0..4_000u64 {
        let item = generate(Skill::Limpers, seed);
        assert_eq!(item, generate(Skill::Limpers, seed), "deterministic");
        let q = limpers_q(&item);
        assert!(matches!(q.players, 5 | 6), "{q:?}");
        if q.players == 5 {
            five += 1;
        } else {
            six += 1;
        }
        let seats = LimperSeat::seats_for_table(q.players);
        let hero_idx = seats.iter().position(|&s| s == q.seat).expect("seat on table");
        assert!(hero_idx > 0, "UTG cannot face limpers: {q:?}");
        assert!((1..=3).contains(&q.limpers.len()), "{q:?}");
        limper_counts[q.limpers.len()] += 1;
        let earlier = &seats[..hero_idx];
        for (i, &limper) in q.limpers.iter().enumerate() {
            assert!(earlier.contains(&limper), "limper {limper:?} not before hero in {q:?}");
            if i > 0 {
                assert!(q.limpers[i - 1] < limper, "limpers must be distinct and ordered: {q:?}");
            }
        }
        assert!(
            !(q.seat == LimperSeat::Bb && q.limpers.as_slice() == [LimperSeat::Sb]),
            "BB vs lone SB complete must not be dealt: {q:?}"
        );
        assert_ne!(
            lookup(q.seat, q.limper_count(), q.hand),
            LimperDecision::Unspecified,
            "unspecified hand dealt on seed {seed}: {q:?}"
        );
        // Context line is one short sentence pair with no newlines.
        let ctx = q.context_line();
        assert!(!ctx.contains('\n') && ctx.contains("You are "), "{ctx}");

        // Suit permutation preserves decision and grade.
        if seed < 200 {
            let perm = [2, 3, 0, 1];
            let permuted = Item {
                skill: Skill::Limpers,
                seed,
                question: item.question.with_suit_perm(&perm),
            };
            let g1 = grade(&item, "r").unwrap();
            let g2 = grade(&permuted, "r").unwrap();
            assert_eq!(g1.correct, g2.correct);
        }
    }
    assert!((1_800..=2_200).contains(&five), "five={five} six={six}");
    assert!((1_800..=2_200).contains(&six), "five={five} six={six}");
    assert!(limper_counts[1] > 1_000 && limper_counts[2] > 600 && limper_counts[3] > 200, "{limper_counts:?}");
}

#[test]
fn boundary_hands_are_asked_less_often_than_interior_hands() {
    // Direct boundary spot checks.
    let co1 = table_row(RowSeat::Co, false);
    assert!(is_boundary(co1, 5, 5, false), "77 is on CO vs 1 pair boundary");
    assert!(is_boundary(co1, 4, 4, false), "66 is on CO vs 1 pair boundary");
    assert!(!is_boundary(co1, 3, 3, false), "55 is interior Call in CO vs 1");
    assert!(is_boundary(co1, 11, 7, true), "K9s is on CO vs 1 Kxs boundary");
    assert!(!is_boundary(co1, 11, 5, true), "K7s is interior Fold in CO vs 1");
    assert!(is_boundary(co1, 3, 2, true), "54s is on CO vs 1 connector boundary");
    assert!(!is_boundary(co1, 6, 5, true), "87s is interior Call in CO vs 1");

    // Empirical check over 8,000 deals: within the same priority tier and decision,
    // boundary hands are asked markedly less often per class than interior hands.
    let mut interior_hits = 0u64;
    let mut boundary_hits = 0u64;
    let mut interior_classes = 0u64;
    let mut boundary_classes = 0u64;

    // Focus on CO vs 1 Fold priority hands (weak suited K/Q/J + offsuit Broadways):
    // clean mix of boundary (K9s, Q9s, J8s, ATo, KJo) and interior (K8s-K2s, Q8s-Q2s, J7s-J2s, QJo, KTo, QTo, JTo).
    for high in 0..13u8 {
        for low in 0..=high {
            let suiteds: &[bool] = if high == low { &[false] } else { &[true, false] };
            for &suited in suiteds {
                if co1.decision_class(high, low, suited) == LimperDecision::Fold && is_priority_hand(high, low, suited) {
                    if is_boundary(co1, high, low, suited) {
                        boundary_classes += 1;
                    } else {
                        interior_classes += 1;
                    }
                }
            }
        }
    }
    assert!(boundary_classes > 0 && interior_classes > 0);

    for seed in 0..12_000u64 {
        let item = generate(Skill::Limpers, seed);
        let q = limpers_q(&item);
        if q.seat != LimperSeat::Co || q.multiway() {
            continue;
        }
        let suited = q.hand[0].suit() == q.hand[1].suit();
        let (a, b) = (q.hand[0].rank(), q.hand[1].rank());
        if co1.decision_class(a, b, suited) == LimperDecision::Fold && is_priority_hand(a, b, suited) {
            if is_boundary(co1, a, b, suited) {
                boundary_hits += 1;
            } else {
                interior_hits += 1;
            }
        }
    }
    let avg_boundary = boundary_hits as f64 / boundary_classes as f64;
    let avg_interior = interior_hits as f64 / interior_classes as f64;
    assert!(
        avg_boundary < avg_interior * 0.55,
        "boundary avg {avg_boundary:.2} should be much lower than interior avg {avg_interior:.2}"
    );
}

#[test]
fn generator_weights_toward_target_leaks_and_small_blind() {
    let mut by_seat: HashMap<LimperSeat, u32> = HashMap::new();
    let mut by_action: HashMap<LimperDecision, u32> = HashMap::new();
    let mut priority = 0u32;
    let mut weak_suited_kqj = 0u32;
    let mut offsuit_broadway = 0u32;
    let mut small_pair = 0u32;
    let mut suited_connector = 0u32;
    let total = 8_000u32;

    for seed in 0..u64::from(total) {
        let item = generate(Skill::Limpers, seed);
        let q = limpers_q(&item);
        *by_seat.entry(q.seat).or_default() += 1;
        *by_action.entry(lookup(q.seat, q.limper_count(), q.hand)).or_default() += 1;
        let suited = q.hand[0].suit() == q.hand[1].suit();
        let (a, b) = (q.hand[0].rank(), q.hand[1].rank());
        let (high, low) = if a >= b { (a, b) } else { (b, a) };
        if is_priority_hand(high, low, suited) {
            priority += 1;
        }
        if suited && (9..=11).contains(&high) && low <= 7 {
            weak_suited_kqj += 1;
        } else if !suited && high != low && low >= 8 {
            offsuit_broadway += 1;
        } else if high == low && high <= 4 {
            small_pair += 1;
        } else if suited && high - low == 1 && high <= 8 && low >= 2 {
            suited_connector += 1;
        }
    }

    let sb = by_seat[&LimperSeat::Sb];
    for (&seat, &count) in &by_seat {
        if seat != LimperSeat::Sb {
            assert!(sb > count, "SB ({sb}) should be weighted above {seat:?} ({count})");
        }
    }
    assert!(sb * 100 / total >= 28, "SB share too low: {sb}/{total}");
    assert!(priority * 100 / total >= 45, "priority leak share too low: {priority}/{total}");
    assert!(weak_suited_kqj >= 800, "weak suited K/Q/J: {weak_suited_kqj}");
    assert!(offsuit_broadway >= 800, "offsuit broadways: {offsuit_broadway}");
    assert!(small_pair >= 400, "small pairs: {small_pair}");
    assert!(suited_connector >= 400, "suited connectors: {suited_connector}");
    assert!(by_action[&LimperDecision::Raise] >= 1_500, "{by_action:?}");
    assert!(by_action[&LimperDecision::Call] >= 3_500, "{by_action:?}");
    assert!(by_action[&LimperDecision::Fold] >= 1_500, "{by_action:?}");
}

#[test]
fn families_and_sibling_search_work_for_every_limpers_family() {
    let reachable = [
        "early:1",
        "cutoff:1",
        "cutoff:2plus",
        "button:1",
        "button:2plus",
        "small_blind:1",
        "small_blind:2plus",
        "big_blind:1",
        "big_blind:2plus",
    ];
    for fam in reachable {
        let mut found = 0;
        for base in 0..20u64 {
            if let Some(seed) = find_sibling(Skill::Limpers, fam, base, 999) {
                let item = generate(Skill::Limpers, seed);
                assert_eq!(family_of(&item).as_deref(), Some(fam));
                found += 1;
            }
        }
        assert!(found >= 18, "family {fam} found only {found}/20 siblings");
    }
}

fn check_row(skill: Skill, seed: u64, run: u32, form: u32, slot: usize, ok: bool) -> Attempt {
    Attempt {
        timestamp_ms: 10_000 + slot as u64,
        mode: Mode::Check,
        baseline_run: None,
        skill,
        item: ItemRef::Seed(seed),
        gen_version: GEN_VERSION,
        correct: ok,
        close: false,
        ev_loss_pct: None,
        response_ms: 1_500,
        answer: "c".to_string(),
        extra: AttemptExtra {
            check: Some(CheckRef { run, form, slot: slot as u32 }),
            ..AttemptExtra::default()
        },
    }
}

#[test]
fn old_mixed_checkup_logs_still_load_and_score() {
    let form = MIXED_FORM_BASE;
    // Current mixed plan has 4 preflop + 4 limpers = 8 Group::Preflop items.
    let current = mixed_checkup_plan(form);
    assert_eq!(current.slots.iter().filter(|s| s.skill == Skill::Preflop).count(), 4);
    assert_eq!(current.slots.iter().filter(|s| s.skill == Skill::Limpers).count(), 4);
    assert_eq!(current.slots.iter().filter(|s| group_of(s.skill) == Some(Group::Preflop)).count(), 8);

    // Completed pre-Limpers 36-slot and 40-slot mixed checkups still score completely.
    for (run_id, dealt_only) in [(1u32, true), (2u32, false)] {
        let old_plan = legacy_mixed_checkup_plan(form, dealt_only);
        assert!(old_plan.slots.iter().all(|s| s.skill != Skill::Limpers));
        let full_rows: Vec<Attempt> = old_plan
            .slots
            .iter()
            .enumerate()
            .map(|(i, s)| check_row(s.skill, s.seed, run_id, form, i, i % 4 != 0))
            .collect();
        let runs = checkup_runs(&full_rows);
        assert_eq!(runs.len(), 1);
        assert!(runs[0].complete, "dealt_only={dealt_only} must stay complete");
        assert!(!runs[0].stale, "completed legacy run must not be stale");
        assert_eq!(runs[0].answered, old_plan.slots.len());
        let pf_group = runs[0].groups.iter().find(|g| g.group == Group::Preflop).unwrap();
        assert_eq!(pf_group.answered, 8);

        // Unfinished legacy run that has already answered a slot differing from the current plan
        // is marked stale and cannot resume onto mismatched slots.
        let first_diff = old_plan
            .slots
            .iter()
            .enumerate()
            .position(|(i, s)| current.slots.get(i).map(|c| (c.skill, c.seed)) != Some((s.skill, s.seed)))
            .expect("legacy plan differs from current plan");
        let partial = &full_rows[..=(first_diff.min(old_plan.slots.len() - 2))];
        let partial_runs = checkup_runs(partial);
        assert!(!partial_runs[0].complete);
        assert!(partial_runs[0].stale);
        assert!(unfinished_run(&partial_runs, CheckupKind::Mixed).is_none());
    }
}

#[test]
fn preflop_mistakes_accepts_limpers_leaks_and_rejects_bad_ones() {
    let text = "#LEAKS v1\n\
LEAK|0|preflop|preflop:button:folded|1000|1|2026-11-10|s1|h1|100|0|Folded KJo on button|cost 1bb|players=5|behind=2|sit=folded|hand=KhJd|hero_act=f|expect=r\n\
LEAK|1|limpers|limpers:btn:1|1500|2|2026-11-18|s2|h2|100|0|Raised K7s over CO limp|cost 4bb|players=5|seat=BTN|limpers=CO|hand=Ks7s|hero_act=r|expect=f\n\
LEAK|2|limpers|limpers:sb:2plus|1400|1|2026-11-18|s2|h3|100|0|Raised 55 in SB over two limpers|cost 6bb|players=6|behind=1|sit=limpers:2|hand=5s5d|hero_act=r|expect=c\n\
LEAK|3|price|price:flop|1000|1|2026-11-18|s2|h4|100|0|Called flop bet|cost 3bb|street=flop|hero_pos=BTN|board=Kh7h2c|hand=AhQh|pot_before=20|bet=10|callers=0|share_pct=35|tight_pct=15|loose_pct=30|hero_act=f|expect=c\n";
    let lib = parse_leaks(text).expect("parses limpers leaks");
    let counts = verify_leaks(&lib).expect("verifies limpers leaks");
    assert_eq!(counts.total, 4);
    assert_eq!(counts.preflop, 3);
    assert_eq!(counts.price, 1);

    let mut session = LeakSession::new(lib, &[], 42, Some(15)).preflop_only();
    assert_eq!(session.eligible_count(), 3);
    let mut saw_limpers = false;
    let mut saw_preflop = false;
    while let Some(planned) = session.try_next_item() {
        let item = session.item_for(planned);
        assert_eq!(item.skill, planned.skill);
        if planned.skill == Skill::Limpers {
            saw_limpers = true;
        }
        if planned.skill == Skill::Preflop {
            saw_preflop = true;
        }
        session.record(planned, true);
    }
    assert!(saw_limpers && saw_preflop);

    // Unspecified hand (KTs in Early vs 1) fails verification.
    let unspec = "#LEAKS v1\n\
LEAK|0|limpers|limpers:early:1|1000|1|2026-11-18|s1|h1|100|0|Raised KTs|cost 2bb|players=6|seat=UTG+1|limpers=UTG|hand=KsTs|hero_act=r|expect=c\n";
    let lib_unspec = parse_leaks(unspec).unwrap();
    assert!(verify_leaks(&lib_unspec).is_err());

    // Lone SB complete vs BB is rejected by parser.
    let lone_sb = "#LEAKS v1\n\
LEAK|0|limpers|limpers:bb:1|1000|1|2026-11-18|s1|h1|100|0|Raised K9s|cost 2bb|players=5|seat=BB|limpers=SB|hand=Ks9s|hero_act=r|expect=c\n";
    assert!(parse_leaks(lone_sb).is_err());
}
