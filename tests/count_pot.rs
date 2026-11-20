//! "Count the pot" (preflop): replay accuracy on tricky betting cases,
//! generator invariants across all three rungs, exact-only grading, points
//! ladder, group & mixed scheduling, checkups, and old-log compatibility.

use std::collections::HashMap;

use poker_trainer::answer::{answer_format, AnswerFormat};
use poker_trainer::checkup::{checkup_plan, mixed_checkup_plan, MIXED_FORM_BASE};
use poker_trainer::count_pot::{
    action_text, count_pot_family, count_pot_rung, count_pot_status, pot_summary, replay_pot,
    CountPotQuestion, PotAction, PotActionKind, PotSeat, MAX_DEALT_POT,
};
use poker_trainer::family::{family_of, find_sibling};
use poker_trainer::generate::generate;
use poker_trainer::grade::{grade, Reveal};
use poker_trainer::groups::{group_of, Group};
use poker_trainer::item::{
    count_pot_rung_of_seed, count_pot_seed_with_rung, is_held_out, Item, Question, Skill,
    COUNT_POT_RUNGS, GEN_VERSION, HELD_OUT_BIT,
};
use poker_trainer::ladder::{OLD_RULE_CUTOFF_MS, RIGHT_WRONG_GATE};
use poker_trainer::log::{Attempt, AttemptExtra, ItemRef, Mode};
use poker_trainer::schedule::{Origin, Session};

fn q(item: &Item) -> &CountPotQuestion {
    match &item.question {
        Question::CountPot(q) => q,
        other => panic!("expected CountPot, got {other:?}"),
    }
}

fn at_rung(i: u64, rung: u8) -> Item {
    let seed = count_pot_seed_with_rung((i.wrapping_mul(2_654_435_761) + 17) & !HELD_OUT_BIT, rung);
    generate(Skill::CountPot, seed)
}

fn act(seat: PotSeat, kind: PotActionKind) -> PotAction {
    PotAction { seat, kind }
}

fn drill_attempt(seed: u64, t: u64, correct: bool, mode: Mode) -> Attempt {
    Attempt {
        timestamp_ms: t,
        mode,
        baseline_run: None,
        skill: Skill::CountPot,
        item: ItemRef::Seed(seed),
        gen_version: GEN_VERSION,
        correct,
        close: false,
        ev_loss_pct: None,
        response_ms: 2_500,
        answer: "40".to_string(),
        extra: AttemptExtra::default(),
    }
}

#[test]
fn replay_handles_every_tricky_preflop_pot_case() {
    // 1. SB completing adds only 5 (total 10), not another 10.
    let sb_complete = replay_pot(
        5,
        &[
            act(PotSeat::Sb, PotActionKind::PostSb),
            act(PotSeat::Bb, PotActionKind::PostBb),
            act(PotSeat::Utg, PotActionKind::Limp),
            act(PotSeat::Co, PotActionKind::Fold),
            act(PotSeat::Btn, PotActionKind::Limp),
            act(PotSeat::Sb, PotActionKind::Complete),
            act(PotSeat::Bb, PotActionKind::Check),
        ],
    )
    .unwrap();
    assert_eq!(sb_complete.pot, 40);
    assert_eq!(sb_complete.breakdown_line(PotSeat::Btn), "SB 10 \u{00b7} BB 10 \u{00b7} UTG 10 \u{00b7} You 10");

    // 2. BB calling a raise adds only raise - 10.
    let bb_call = replay_pot(
        4,
        &[
            act(PotSeat::Sb, PotActionKind::PostSb),
            act(PotSeat::Bb, PotActionKind::PostBb),
            act(PotSeat::Co, PotActionKind::Raise(40)),
            act(PotSeat::Btn, PotActionKind::Call),
            act(PotSeat::Sb, PotActionKind::Fold),
            act(PotSeat::Bb, PotActionKind::Call),
        ],
    )
    .unwrap();
    assert_eq!(bb_call.pot, 125);
    assert_eq!(bb_call.breakdown_line(PotSeat::Bb), "SB 5 \u{00b7} You 40 \u{00b7} CO 40 \u{00b7} BTN 40");

    // 3. Limper calling a raise adds only the difference (raise - 10).
    let limp_call = replay_pot(
        5,
        &[
            act(PotSeat::Sb, PotActionKind::PostSb),
            act(PotSeat::Bb, PotActionKind::PostBb),
            act(PotSeat::Utg, PotActionKind::Limp),
            act(PotSeat::Co, PotActionKind::Raise(50)),
            act(PotSeat::Btn, PotActionKind::Call),
            act(PotSeat::Sb, PotActionKind::Fold),
            act(PotSeat::Bb, PotActionKind::Fold),
            act(PotSeat::Utg, PotActionKind::Call),
        ],
    )
    .unwrap();
    assert_eq!(limp_call.pot, 165);
    assert_eq!(limp_call.breakdown_line(PotSeat::Utg), "SB 5 \u{00b7} BB 10 \u{00b7} You 50 \u{00b7} CO 50 \u{00b7} BTN 50");

    // 4. Limp-fold leaves the limper's 10 in the pot.
    let limp_fold = replay_pot(
        5,
        &[
            act(PotSeat::Sb, PotActionKind::PostSb),
            act(PotSeat::Bb, PotActionKind::PostBb),
            act(PotSeat::Utg, PotActionKind::Limp),
            act(PotSeat::Co, PotActionKind::Raise(40)),
            act(PotSeat::Btn, PotActionKind::Call),
            act(PotSeat::Sb, PotActionKind::Fold),
            act(PotSeat::Bb, PotActionKind::Fold),
            act(PotSeat::Utg, PotActionKind::Fold),
        ],
    )
    .unwrap();
    assert_eq!(limp_fold.pot, 105);
    assert_eq!(limp_fold.breakdown_line(PotSeat::Co), "SB 5 \u{00b7} BB 10 \u{00b7} UTG 10 \u{00b7} You 40 \u{00b7} BTN 40");

    // 5. SB calling a raise adds only raise - 5.
    let sb_call = replay_pot(
        4,
        &[
            act(PotSeat::Sb, PotActionKind::PostSb),
            act(PotSeat::Bb, PotActionKind::PostBb),
            act(PotSeat::Co, PotActionKind::Raise(60)),
            act(PotSeat::Btn, PotActionKind::Fold),
            act(PotSeat::Sb, PotActionKind::Call),
            act(PotSeat::Bb, PotActionKind::Fold),
        ],
    )
    .unwrap();
    assert_eq!(sb_call.pot, 130);
    assert_eq!(sb_call.breakdown_line(PotSeat::Sb), "You 60 \u{00b7} BB 10 \u{00b7} CO 60");

    // 6. Open + 3-bet + limp-fold + open-call (multiway re-raise).
    let reraise = replay_pot(
        6,
        &[
            act(PotSeat::Sb, PotActionKind::PostSb),
            act(PotSeat::Bb, PotActionKind::PostBb),
            act(PotSeat::Utg, PotActionKind::Limp),
            act(PotSeat::Mp, PotActionKind::Fold),
            act(PotSeat::Co, PotActionKind::Raise(40)),
            act(PotSeat::Btn, PotActionKind::Call),
            act(PotSeat::Sb, PotActionKind::Fold),
            act(PotSeat::Bb, PotActionKind::Raise(120)),
            act(PotSeat::Utg, PotActionKind::Fold),
            act(PotSeat::Co, PotActionKind::Call),
            act(PotSeat::Btn, PotActionKind::Fold),
        ],
    )
    .unwrap();
    assert_eq!(reraise.pot, 295);
    assert_eq!(reraise.breakdown_line(PotSeat::Co), "SB 5 \u{00b7} BB 120 \u{00b7} UTG 10 \u{00b7} You 120 \u{00b7} BTN 40");

    // Illegal sequences are rejected.
    assert!(replay_pot(4, &[act(PotSeat::Sb, PotActionKind::Fold), act(PotSeat::Sb, PotActionKind::Call)]).is_err());
    assert!(replay_pot(4, &[act(PotSeat::Sb, PotActionKind::PostSb), act(PotSeat::Bb, PotActionKind::PostBb), act(PotSeat::Co, PotActionKind::Raise(33))]).is_err());
    assert!(replay_pot(4, &[act(PotSeat::Sb, PotActionKind::PostSb), act(PotSeat::Bb, PotActionKind::PostBb), act(PotSeat::Co, PotActionKind::Check)]).is_err());
}

#[test]
fn generator_obeys_invariants_and_covers_all_tricky_cases() {
    let mut sb_completes = 0usize;
    let mut bb_calls_raise = 0usize;
    let mut limper_calls_raise = 0usize;
    let mut limp_folds = 0usize;
    let mut reraises = 0usize;
    let mut big_opens = 0usize;
    let mut three_action_hands = 0usize;
    let mut ten_action_hands = 0usize;

    let ranges = [(3usize, 5usize), (5, 7), (6, 10)];
    for rung in 0..COUNT_POT_RUNGS {
        let (min_acts, max_acts) = ranges[usize::from(rung)];
        for i in 0..1_500u64 {
            let item = at_rung(i, rung);
            let cp = q(&item);
            assert_eq!(cp.rung, rung);
            assert!((4..=6).contains(&cp.players), "{cp:?}");
            let summary = replay_pot(cp.players, &cp.actions).expect("generated hand replays");
            assert!((min_acts..=max_acts).contains(&summary.voluntary_actions), "rung {rung}: {} acts", summary.voluntary_actions);
            assert!(summary.pot >= 20 && summary.pot <= MAX_DEALT_POT && summary.pot.is_multiple_of(5), "pot={}", summary.pot);
            assert!(summary.remaining >= 2);
            assert!(cp.actions.iter().any(|a| a.seat == cp.hero), "hero absent");

            let breakdown = summary.breakdown_line(cp.hero);
            assert!(!breakdown.contains('$') && !breakdown.contains("bb") && !breakdown.contains('.'), "{breakdown}");
            let mut limped = [false; 6];
            let mut raised = false;
            for &a in &cp.actions {
                let text = action_text(a, cp.hero);
                assert!(!text.contains('$') && !text.contains("bb") && !text.contains('.'), "{text}");
                match a.kind {
                    PotActionKind::Complete => sb_completes += 1,
                    PotActionKind::Limp => limped[a.seat as usize] = true,
                    PotActionKind::Raise(to) => {
                        raised = true;
                        if to == 100 && summary.raises == 1 {
                            big_opens += 1;
                        }
                    }
                    PotActionKind::Call if raised => {
                        if a.seat == PotSeat::Bb {
                            bb_calls_raise += 1;
                        }
                        if limped[a.seat as usize] {
                            limper_calls_raise += 1;
                        }
                    }
                    PotActionKind::Fold if raised && limped[a.seat as usize] => {
                        limp_folds += 1;
                    }
                    _ => {}
                }
            }
            if summary.raises >= 2 {
                reraises += 1;
            }
            if summary.voluntary_actions == 3 {
                three_action_hands += 1;
            }
            if summary.voluntary_actions == 10 {
                ten_action_hands += 1;
            }
            assert_eq!(item.question.with_suit_perm(&[1, 2, 3, 0]), item.question);
        }
    }

    assert!(sb_completes >= 200, "sb_completes={sb_completes}");
    assert!(bb_calls_raise >= 200, "bb_calls_raise={bb_calls_raise}");
    assert!(limper_calls_raise >= 100, "limper_calls_raise={limper_calls_raise}");
    assert!(limp_folds >= 100, "limp_folds={limp_folds}");
    assert!(reraises >= 400, "reraises={reraises}");
    assert!(big_opens >= 40, "big_opens={big_opens}");
    assert!(three_action_hands >= 50, "three_action_hands={three_action_hands}");
    assert!(ten_action_hands >= 20, "ten_action_hands={ten_action_hands}");
}

#[test]
fn grading_requires_exact_match_and_reveals_tight_breakdown() {
    for rung in 0..COUNT_POT_RUNGS {
        let item = at_rung(42, rung);
        assert!(matches!(answer_format(&item.question), AnswerFormat::Integer { min: 0, max: 2000 }));
        let cp = q(&item);
        let summary = pot_summary(cp);
        let exact = grade(&item, &summary.pot.to_string()).unwrap();
        assert!(exact.correct && !exact.close);
        match &exact.reveal {
            Reveal::CountPot { pot, breakdown } => {
                assert_eq!(*pot, summary.pot);
                assert_eq!(breakdown, &summary.breakdown_line(cp.hero));
                assert!(!breakdown.contains('$') && !breakdown.contains("bb"));
            }
            other => panic!("unexpected reveal: {other:?}"),
        }

        // Off by 5 or 1 is always wrong, never close.
        for wrong in [summary.pot + 5, summary.pot.saturating_sub(5), summary.pot + 1] {
            let outcome = grade(&item, &wrong.to_string()).unwrap();
            assert!(!outcome.correct && !outcome.close, "wrong={wrong} pot={}", summary.pot);
        }

        // Invalid answers are rejected.
        for bad in ["", "abc", "-5", "2001", "40.5", "$40"] {
            assert!(grade(&item, bad).is_err(), "accepted bad input {bad:?}");
        }
    }
}

#[test]
fn ladder_promotes_via_shared_points_gate_and_siblings_keep_rung() {
    let t = OLD_RULE_CUTOFF_MS + 10_000;
    assert_eq!(count_pot_rung(&[]), 0);
    assert_eq!(count_pot_status(&[]).points, RIGHT_WRONG_GATE.start);

    // Retries never move the rung.
    let retries: Vec<Attempt> = (0..30u64)
        .map(|i| drill_attempt(count_pot_seed_with_rung(i + 1, 0), t + i, true, Mode::DrillRetry))
        .collect();
    assert_eq!(count_pot_rung(&retries), 0);

    // 22 first-try right answers (10 -> 32) promote rung 0 -> 1.
    let mut log: Vec<Attempt> = (0..22u64)
        .map(|i| drill_attempt(count_pot_seed_with_rung(i + 1, 0), t + i, true, Mode::Drill))
        .collect();
    assert_eq!(count_pot_rung(&log), 1);
    assert_eq!(count_pot_status(&log).points, RIGHT_WRONG_GATE.start);

    // A miss at rung 1 drops points by 4 (10 -> 6).
    log.push(drill_attempt(count_pot_seed_with_rung(100, 1), t + 100, false, Mode::Drill));
    assert_eq!(count_pot_status(&log).points, RIGHT_WRONG_GATE.start - 4);
    assert_eq!(count_pot_rung(&log), 1);

    // 26 right answers at rung 1 (6 -> 32) promote rung 1 -> 2.
    for i in 0..26u64 {
        log.push(drill_attempt(count_pot_seed_with_rung(200 + i, 1), t + 200 + i, true, Mode::Drill));
    }
    assert_eq!(count_pot_rung(&log), 2);

    // Sibling search preserves the rung and avoids the original seed.
    for rung in 0..COUNT_POT_RUNGS {
        let item = at_rung(5, rung);
        let fam = family_of(&item).expect("count_pot has a family");
        assert_eq!(fam, count_pot_family(q(&item)));
        let sib = find_sibling(Skill::CountPot, &fam, 77, item.seed).expect("sibling found");
        assert_ne!(sib, item.seed);
        assert!(!is_held_out(sib));
        assert_eq!(count_pot_rung_of_seed(sib), rung);
        assert_eq!(family_of(&generate(Skill::CountPot, sib)).as_deref(), Some(fam.as_str()));
    }
}

#[test]
fn group_mixed_and_checkups_include_count_pot() {
    assert_eq!(group_of(Skill::CountPot), Some(Group::ReadPrice));
    assert_eq!(Group::ReadPrice.skills(), vec![Skill::ReadPrice, Skill::CountPot]);

    // Group::ReadPrice deals both skills roughly evenly.
    let mut counts: HashMap<Skill, u32> = HashMap::new();
    for seed in 0..20u64 {
        let mut s = Session::for_group(Group::ReadPrice, &[], 1_000, seed);
        for _ in 0..100 {
            let p = s.try_next_item().unwrap();
            *counts.entry(p.skill).or_default() += 1;
            s.record(p, true);
        }
    }
    let cp = f64::from(counts[&Skill::CountPot]) / 2000.0;
    assert!((0.40..=0.60).contains(&cp), "count_pot share in ReadPrice group: {cp:.3} ({counts:?})");

    // New items in a session use the player's current rung.
    let t = OLD_RULE_CUTOFF_MS + 10_000;
    let promoted: Vec<Attempt> = (0..22u64)
        .map(|i| drill_attempt(count_pot_seed_with_rung(i + 1, 0), t + i, true, Mode::Drill))
        .collect();
    let mut s = Session::for_group(Group::ReadPrice, &promoted, t + 1_000, 9);
    let mut saw_rung_1 = 0;
    for _ in 0..80 {
        let p = s.try_next_item().unwrap();
        if p.skill == Skill::CountPot && p.origin == Origin::New {
            assert_eq!(count_pot_rung_of_seed(p.seed), 1);
            saw_rung_1 += 1;
        }
        s.record(p, true);
    }
    assert!(saw_rung_1 >= 20, "saw_rung_1={saw_rung_1}");

    // One-skill checkup covers all three rungs with held-out seeds.
    let plan = checkup_plan(Skill::CountPot, 0);
    assert_eq!(plan.slots.len(), 50);
    assert!(plan.slots.iter().all(|s| is_held_out(s.seed)));
    let mut by_rung = [0usize; 3];
    for s in plan.slots.iter().filter(|s| s.repeat_of.is_none()) {
        by_rung[usize::from(count_pot_rung_of_seed(s.seed))] += 1;
    }
    assert!(by_rung.iter().all(|&n| n >= 13), "checkup rung spread: {by_rung:?}");

    // Mixed checkup includes 3 count_pot + 4 read_price = 7 in Group::ReadPrice.
    let mixed = mixed_checkup_plan(MIXED_FORM_BASE);
    let cp_slots: Vec<_> = mixed.slots.iter().filter(|s| s.skill == Skill::CountPot).collect();
    let rp_slots: Vec<_> = mixed.slots.iter().filter(|s| s.skill == Skill::ReadPrice).collect();
    assert_eq!(cp_slots.len(), 3);
    assert_eq!(rp_slots.len(), 4);
    let mut rungs: Vec<u8> = cp_slots.iter().map(|s| count_pot_rung_of_seed(s.seed)).collect();
    rungs.sort_unstable();
    assert_eq!(rungs, vec![0, 1, 2]);
}
