//! Held-out checkups, the `equity` skill, the anchor deck and `board_type`.

use poker_trainer::anchors::{self, AnchorKind};
use poker_trainer::answer::{answer_format, AnswerFormat};
use poker_trainer::baseline;
use poker_trainer::board_type::{
    board_type_status, category_of_seed, classify_flop, one_attribute_distractors, seed_with_category, BoardCategory,
    CategoryDealer, RankPattern, SuitPattern, BOARD_TYPE_MISS_MS, BOARD_TYPE_TARGET_MS,
};
use poker_trainer::cards::{parse_cards, parse_holding, Card, Holding};
use poker_trainer::checkup::{
    checkup_plan, checkup_runs, latest_checkup, next_checkup_form, next_checkup_run, CheckupMetric, CHECKUP_EQUITY_BAND,
    CHECKUP_LEN, CHECKUP_ORIGINALS, CHECKUP_REPEATS, MIN_REPEAT_GAP,
};
use poker_trainer::classes::{combos_in_class, OppClass};
use poker_trainer::equity::equity_all_in;
use poker_trainer::equity_skill::{
    equity_band, equity_bias, equity_rung, next_band, BIAS_MIN_ATTEMPTS, EQUITY_BAND_MAX, EQUITY_BAND_MIN, EQUITY_BAND_START,
    PROMOTE_MAX_MAE, PROMOTE_WINDOW,
};
use poker_trainer::generate::{generate, EQUITY_MAX_PCT, EQUITY_MIN_PCT};
use poker_trainer::grade::{equity_analysis, grade, grade_timed, Outcome, Reveal};
use poker_trainer::item::{
    equity_seed_with_rung, is_held_out, EquityOpponents, EquityQuestion, Item, Question, Skill, GEN_VERSION, HELD_OUT_BIT,
};
use poker_trainer::log::{format_attempt, parse_log, Attempt, AttemptExtra, CheckRef, ItemRef, Mode, LEGACY_LOG_HEADER, LOG_HEADER};
use poker_trainer::eval::evaluate;
use poker_trainer::preflop::CARD;
use poker_trainer::progress::{pass_mark, skill_stats, STEPS};
use poker_trainer::rng::Rng;
use poker_trainer::schedule::{
    anchor_status, anchors_due, anchors_new, due_items, recent_results, Origin, Session, FIRST_REVIEW_MS,
    LEITNER_INTERVAL_DAYS, MAX_NEW_ANCHORS_PER_SESSION,
};

const DAY: u64 = 86_400_000;

fn cards(text: &str) -> Vec<Card> {
    parse_cards(text).unwrap()
}

fn hand(text: &str) -> Holding {
    parse_holding(text).unwrap()
}

fn answer(item: &Item, text: &str) -> Outcome {
    grade(item, text).unwrap_or_else(|e| panic!("{text}: {}", e.0))
}

fn attempt(skill: Skill, seed: u64, t: u64, correct: bool, ms: u32) -> Attempt {
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
        response_ms: ms,
        answer: "x".to_string(),
        extra: AttemptExtra::default(),
    }
}

fn equity_item(hero: &str, board: &str, opponents: EquityOpponents, band: u8) -> Item {
    Item {
        skill: Skill::Equity,
        seed: 1,
        question: Question::Equity(EquityQuestion { hero: hand(hero), board: cards(board), opponents, band }),
    }
}

// ----- Held-out seeds -----

#[test]
fn drills_and_reviews_never_serve_a_held_out_seed() {
    let allowed = vec![Skill::Preflop, Skill::Nuts, Skill::Equity, Skill::BoardType, Skill::Anchor];
    // A review log containing a held-out seed from before the bit was reserved.
    let old = attempt(Skill::Nuts, HELD_OUT_BIT | 7, 0, false, 1000);
    let plain = attempt(Skill::Nuts, 7, 0, false, 1000);
    for seed in 0..40 {
        let log = vec![old.clone(), plain.clone()];
        let mut session = Session::new(allowed.clone(), &log, FIRST_REVIEW_MS * 3, seed);
        for i in 0..400 {
            let planned = session.try_next_item().unwrap();
            assert!(!is_held_out(planned.seed), "{planned:?}");
            session.record(planned, i % 4 != 0);
        }
    }
    assert!(due_items(&[old], FIRST_REVIEW_MS * 3).is_empty());
}

#[test]
fn checkup_seeds_are_all_held_out_and_baseline_items_never_are_served_from_them() {
    let baseline_seeds: Vec<(Skill, u64)> =
        baseline::load().unwrap().iter().map(|e| (e.item.skill, e.item.seed)).collect();
    for skill in Skill::ALL {
        for form in 0..200 {
            let plan = checkup_plan(skill, form);
            for slot in &plan.slots {
                assert!(is_held_out(slot.seed), "{skill:?} form {form}");
                assert!(!baseline_seeds.contains(&(skill, slot.seed)), "{skill:?} form {form}");
            }
        }
    }
}

// ----- Checkup plans -----

#[test]
fn a_checkup_is_forty_new_items_and_ten_repeats_well_after_their_originals() {
    for skill in Skill::ALL {
        for form in [0, 1, 7, 50] {
            let plan = checkup_plan(skill, form);
            assert_eq!(plan, checkup_plan(skill, form), "deterministic");
            assert_eq!(plan.slots.len(), CHECKUP_LEN);
            let repeats: Vec<(usize, usize)> = plan
                .slots
                .iter()
                .enumerate()
                .filter_map(|(i, s)| s.repeat_of.map(|o| (i, o)))
                .collect();
            assert_eq!(repeats.len(), CHECKUP_REPEATS, "{skill:?}");
            for &(at, original) in &repeats {
                assert!(at - original >= MIN_REPEAT_GAP, "{skill:?} form {form}");
                assert!(plan.slots[original].repeat_of.is_none(), "a repeat copies an original");
                assert_eq!(plan.slots[at].seed, plan.slots[original].seed);
                assert_ne!(plan.slots[at].perm, [0, 1, 2, 3], "repeats relabel suits");
            }
            let mut originals: Vec<u64> =
                plan.slots.iter().filter(|s| s.repeat_of.is_none()).map(|s| s.seed).collect();
            assert_eq!(originals.len(), CHECKUP_ORIGINALS);
            originals.sort_unstable();
            originals.dedup();
            assert_eq!(originals.len(), CHECKUP_ORIGINALS, "{skill:?} originals are distinct");
            let mut used: Vec<usize> = repeats.iter().map(|&(_, o)| o).collect();
            used.sort_unstable();
            used.dedup();
            assert_eq!(used.len(), CHECKUP_REPEATS, "no original is repeated twice");
        }
    }
}

#[test]
fn forms_never_share_items() {
    for skill in [Skill::Nuts, Skill::Outs, Skill::Equity, Skill::BoardType] {
        let mut seen = std::collections::HashSet::new();
        for form in 0..100 {
            for slot in &checkup_plan(skill, form).slots {
                if slot.repeat_of.is_none() {
                    assert!(seen.insert(slot.seed), "{skill:?} form {form}");
                }
            }
        }
    }
}

#[test]
fn suit_permuted_repeats_ask_the_same_question() {
    let plan = checkup_plan(Skill::Made, 3);
    let (at, slot) = plan.slots.iter().enumerate().find(|(_, s)| s.repeat_of.is_some()).unwrap();
    let original = plan.item(slot.repeat_of.unwrap());
    let repeat = plan.item(at);
    assert_eq!(repeat.question, original.question.with_suit_perm(&slot.perm));
    assert_ne!(repeat.question, original.question);
    let (a, b) = (answer(&original, "top"), answer(&repeat, "top"));
    assert_eq!(a.correct, b.correct);
}

#[test]
fn equity_and_board_checkups_have_a_fixed_composition() {
    for form in 0..30 {
        let plan = checkup_plan(Skill::Equity, form);
        let rung_one = plan
            .slots
            .iter()
            .filter(|s| s.repeat_of.is_none() && s.seed & (1 << 62) != 0)
            .count();
        assert_eq!(rung_one, CHECKUP_ORIGINALS / 2);
        let boards = checkup_plan(Skill::BoardType, form);
        for category in BoardCategory::ALL {
            let count = boards
                .slots
                .iter()
                .filter(|s| s.repeat_of.is_none() && category_of_seed(s.seed) == category)
                .count();
            assert_eq!(count, CHECKUP_ORIGINALS / BoardCategory::ALL.len());
        }
    }
    assert_eq!(BoardCategory::ALL.len() * 5, CHECKUP_ORIGINALS);
}

#[test]
fn anchor_checkup_covers_the_whole_deck() {
    let plan = checkup_plan(Skill::Anchor, 0);
    let mut cards: Vec<usize> = plan
        .slots
        .iter()
        .filter(|s| s.repeat_of.is_none())
        .map(|s| anchors::card_of_seed(s.seed))
        .collect();
    cards.sort_unstable();
    cards.dedup();
    assert_eq!(cards.len(), anchors::deck().len());
}

#[test]
fn a_checkup_equity_item_uses_the_fixed_band() {
    let plan = checkup_plan(Skill::Equity, 0);
    for i in [0, 1, 2] {
        let item = plan.item(i);
        let Question::Equity(q) = &item.question else { panic!() };
        assert_eq!(q.band, CHECKUP_EQUITY_BAND);
    }
}

// ----- Checkup log and scoring -----

/// A complete checkup: `right` of the 40 originals right, every repeat matching its original.
fn checkup_log(skill: Skill, run: u32, form: u32, right: usize) -> Vec<Attempt> {
    let plan = checkup_plan(skill, form);
    let mut originals_seen = 0;
    let mut correct_at = [false; CHECKUP_LEN];
    let mut rows = Vec::new();
    for (slot, spec) in plan.slots.iter().enumerate() {
        let correct = match spec.repeat_of {
            Some(original) => correct_at[original],
            None => {
                originals_seen += 1;
                originals_seen <= right
            }
        };
        correct_at[slot] = correct;
        let mut row = attempt(skill, spec.seed, 5_000_000 + u64::from(run) * 100 + slot as u64, correct, 2_000);
        row.mode = Mode::Check;
        row.extra.check = Some(CheckRef { run, form, slot: slot as u32 });
        rows.push(row);
    }
    rows
}

#[test]
fn checkup_score_uses_the_forty_originals_with_a_standard_error() {
    // 30 of 40 right: 75%, standard error sqrt(.75 * .25 / 40) = 6.847 points.
    let log = checkup_log(Skill::Nuts, 1, 0, 30);
    let score = latest_checkup(Skill::Nuts, &log).unwrap();
    assert!(score.complete);
    assert_eq!(score.answered, 50);
    assert_eq!(score.metric, CheckupMetric::PercentCorrect);
    assert!((score.score - 75.0).abs() < 1e-9);
    assert!((score.standard_error - 6.8465).abs() < 0.001, "{}", score.standard_error);
    assert!(!score.passed);
    assert_eq!(score.agreement.pairs, 10);
    assert_eq!(score.agreement.agree, 10);
    assert!((score.agreement.agree_pct - 100.0).abs() < 1e-9);
}

#[test]
fn repeats_never_change_the_score() {
    let mut log = checkup_log(Skill::Nuts, 1, 0, 40);
    let plan = checkup_plan(Skill::Nuts, 0);
    for row in &mut log {
        let slot = row.extra.check.unwrap().slot as usize;
        if plan.slots[slot].repeat_of.is_some() {
            row.correct = false;
        }
    }
    let score = latest_checkup(Skill::Nuts, &log).unwrap();
    assert!((score.score - 100.0).abs() < 1e-9, "repeats are not scored");
    assert!(score.passed);
    assert_eq!((score.agreement.agree, score.agreement.pairs), (0, 10));
}

#[test]
fn equity_checkup_scores_mean_absolute_error_with_its_standard_error() {
    let plan = checkup_plan(Skill::Equity, 0);
    let mut log = Vec::new();
    let mut n = 0;
    for (slot, spec) in plan.slots.iter().enumerate() {
        // Originals alternate 4 and 8 points of absolute error: mean 6.
        let err = if n % 2 == 0 { 4.0 } else { 8.0 };
        if spec.repeat_of.is_none() {
            n += 1;
        }
        let mut row = attempt(Skill::Equity, spec.seed, 100 + slot as u64, err <= 5.0, 1000);
        row.mode = Mode::Check;
        row.extra.check = Some(CheckRef { run: 1, form: 0, slot: slot as u32 });
        row.extra.signed_error = Some(err);
        row.extra.truth_pct = Some(50.0);
        row.extra.opponents = Some(1);
        row.extra.band = Some(5);
        log.push(row);
    }
    let score = latest_checkup(Skill::Equity, &log).unwrap();
    assert_eq!(score.metric, CheckupMetric::MeanAbsError);
    assert!((score.score - 6.0).abs() < 1e-9, "{}", score.score);
    // Sample sd of a 4/8 alternation over 40 values: sqrt(160/39) = 2.0255.
    assert!((score.standard_error - 2.0255 / 40f64.sqrt()).abs() < 1e-3, "{}", score.standard_error);
    assert!(score.passed, "6 points meets the mark");
    assert_eq!(pass_mark(Skill::Equity).max_mean_abs_error, Some(6.0));
}

#[test]
fn each_completed_checkup_uses_the_next_form() {
    assert_eq!(next_checkup_form(Skill::Nuts, &[]), 0);
    assert_eq!(next_checkup_run(&[]), 1);
    let mut log = checkup_log(Skill::Nuts, 1, 0, 40);
    assert_eq!(next_checkup_form(Skill::Nuts, &log), 1);
    assert_eq!(next_checkup_run(&log), 2);
    assert_eq!(next_checkup_form(Skill::Price, &log), 0, "forms count per skill");
    // An abandoned form is never served again.
    let mut partial = checkup_log(Skill::Nuts, 2, 1, 40);
    partial.truncate(3);
    log.extend(partial);
    assert_eq!(next_checkup_form(Skill::Nuts, &log), 2);
    assert_eq!(next_checkup_run(&log), 3);
}

#[test]
fn checkups_do_not_touch_drill_stats_miss_rates_or_reviews() {
    let log = checkup_log(Skill::Nuts, 1, 0, 5);
    assert_eq!(skill_stats(Skill::Nuts, &log).total_attempts, 0);
    assert!(recent_results(&log).is_empty());
    assert!(due_items(&log, DAY * 90).is_empty());
    assert_eq!(poker_trainer::progress::next_baseline_run(&log), 1);
    assert!(!poker_trainer::progress::has_baseline(&log));
}

#[test]
fn checkup_runs_report_history_oldest_first() {
    let mut log = checkup_log(Skill::Nuts, 1, 0, 38);
    log.extend(checkup_log(Skill::Price, 2, 0, 40));
    log.extend(checkup_log(Skill::Nuts, 3, 1, 20));
    let runs = checkup_runs(&log);
    assert_eq!(runs.iter().map(|r| r.run).collect::<Vec<_>>(), vec![1, 2, 3]);
    assert!(runs[0].passed, "38 of 40 is 95%");
    assert!(!runs[2].passed);
    assert_eq!(latest_checkup(Skill::Nuts, &log).unwrap().run, 3);
}

// ----- Log format -----

#[test]
fn old_lines_still_parse_and_new_columns_go_at_the_end() {
    let old = "1700000000000\tdrill\t\tprice\t42\t1\t1\t0\t\t1200\t25";
    let new_header = format!("{LOG_HEADER}\n");
    let legacy_header = format!("{LEGACY_LOG_HEADER}\n{old}\n");
    assert!(LOG_HEADER.starts_with(LEGACY_LOG_HEADER));
    let (parsed, warnings) = parse_log(&legacy_header);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].extra, AttemptExtra::default());
    let (_, warnings) = parse_log(&new_header);
    assert!(warnings.is_empty());

    // Attempts without extras are written exactly as before.
    let plain = attempt(Skill::Price, 42, 1, true, 1000);
    assert_eq!(format_attempt(&plain).split('\t').count(), 11);

    let mut row = attempt(Skill::Equity, 42, 1, true, 1000);
    row.extra = AttemptExtra {
        check: Some(CheckRef { run: 3, form: 7, slot: 12 }),
        band: Some(9),
        signed_error: Some(-4.5),
        truth_pct: Some(37.25),
        opponents: Some(2),
    };
    row.mode = Mode::Check;
    let line = format_attempt(&row);
    let fields: Vec<&str> = line.split('\t').collect();
    assert_eq!(fields.len(), 18);
    assert_eq!(fields[10], "x", "the answer keeps its old column");
    let (round, warnings) = parse_log(&line);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(round[0], row);
}

#[test]
fn a_line_with_the_wrong_number_of_columns_is_malformed() {
    let (parsed, warnings) = parse_log("1\tdrill\t\tprice\t42\t1\t1\t0\t\t1200\t25\t1\t2");
    assert!(parsed.is_empty());
    assert_eq!(warnings.len(), 1);
}

// ----- Equity skill: truth -----

#[test]
fn exact_opponents_turn_equity_is_fifteen_outs_of_forty_four() {
    // Hero's open-ended flush draw on the turn against aces: nine clubs plus
    // the six off-suit fives and tens, out of 44 unseen rivers.
    let item = equity_item("9c8c", "7c6c2d3s", EquityOpponents::Exact(vec![hand("AsAd")]), 10);
    let Reveal::Equity { analysis, .. } = answer(&item, "34").reveal else { panic!() };
    assert!((analysis.truth_pct - 100.0 * 15.0 / 44.0).abs() < 1e-9, "{}", analysis.truth_pct);
    assert_eq!(analysis.tuples, Some(1));
    assert!(!analysis.sampled);
    assert_eq!(analysis.combos_per_opponent, vec![1]);
}

#[test]
fn a_class_is_averaged_over_every_combo() {
    let board = cards("Kh7h2c");
    let hero = hand("AhQh");
    let combos = combos_in_class(OppClass::Set, hero, &board);
    assert_eq!(combos.len(), 9);
    let per_combo: f64 = combos
        .iter()
        .map(|&c| equity_all_in(hero, &board, &[vec![c]], 0).unwrap().share)
        .sum::<f64>()
        / combos.len() as f64;
    let item = equity_item("AhQh", "Kh7h2c", EquityOpponents::Classes(vec![OppClass::Set]), 10);
    let Reveal::Equity { analysis, .. } = answer(&item, "20").reveal else { panic!() };
    assert!((analysis.truth_pct - 100.0 * per_combo).abs() < 1e-9);
    assert_eq!(analysis.combos_per_opponent, vec![9]);
}

#[test]
fn hit_or_miss_compares_the_estimate_with_the_band() {
    // Truth 15/44 = 34.0909...
    let item = |band| equity_item("9c8c", "7c6c2d3s", EquityOpponents::Exact(vec![hand("AsAd")]), band);
    let result = answer(&item(5), "39");
    assert!(result.correct, "off by 4.9 points");
    let log = result.equity_log.unwrap();
    assert!((log.signed_error - (39.0 - 100.0 * 15.0 / 44.0)).abs() < 1e-9);
    assert_eq!(log.band, 5);
    assert_eq!(log.opponents, 1);
    assert!((log.truth_pct - 100.0 * 15.0 / 44.0).abs() < 1e-9);

    assert!(!answer(&item(5), "40").correct, "off by 5.9 points");
    let Reveal::Equity { estimate_pct, signed_error, band, hit, .. } = answer(&item(2), "32").reveal else { panic!() };
    assert_eq!((estimate_pct, band, hit), (32.0, 2, false));
    assert!(signed_error < 0.0);
    assert!(grade(&item(2), "not a number").is_err());
}

#[test]
fn suit_permutations_change_no_equity_number() {
    let perms: [[u8; 4]; 3] = [[1, 2, 3, 0], [3, 0, 1, 2], [2, 3, 0, 1]];
    for seed in 0..40u64 {
        let item = generate(Skill::Equity, equity_seed_with_rung(seed, (seed % 2) as u8));
        let Question::Equity(q) = &item.question else { panic!() };
        let base = equity_analysis(q, item.seed).unwrap();
        for perm in &perms {
            let Question::Equity(shifted) = item.question.with_suit_perm(perm) else { panic!() };
            let got = equity_analysis(&shifted, item.seed).unwrap();
            assert_eq!(base, got, "{perm:?}");
        }
    }
}

// ----- Equity generator -----

#[test]
fn equity_items_meet_the_brief() {
    let mut counts = [0u32; 3];
    let mut streets = [0u32; 2];
    let n = 300;
    for seed in 0..n {
        let seed = equity_seed_with_rung(seed * 7919, (seed % 2) as u8);
        let item = generate(Skill::Equity, seed);
        let Question::Equity(q) = &item.question else { panic!() };
        assert_eq!(q.band, EQUITY_BAND_START);
        assert!(CARD.plays(q.hero), "{:?}", q.hero);
        assert!(q.board.len() == 3 || q.board.len() == 4);
        streets[q.board.len() - 3] += 1;
        let opponents = q.opponents.count();
        assert!((1..=3).contains(&opponents));
        counts[opponents - 1] += 1;
        assert_eq!(q.rung(), (seed >> 62 & 1) as u8);
        let analysis = equity_analysis(q, item.seed).unwrap();
        assert!(
            (EQUITY_MIN_PCT..=EQUITY_MAX_PCT).contains(&analysis.truth_pct),
            "seed {seed}: {}",
            analysis.truth_pct
        );
        if let EquityOpponents::Exact(hands) = &q.opponents {
            let mut used = poker_trainer::cards::holding_mask(&q.hero)
                | poker_trainer::cards::mask_of(&q.board);
            for h in hands {
                let mask = poker_trainer::cards::holding_mask(h);
                assert_eq!(used & mask, 0, "exact hands are card-disjoint");
                used |= mask;
            }
        }
    }
    // Roughly 50/35/15.
    assert!(counts[0] > 110 && counts[0] < 190, "{counts:?}");
    assert!(counts[1] > 70 && counts[1] < 140, "{counts:?}");
    assert!(counts[2] > 20 && counts[2] < 75, "{counts:?}");
    assert!(streets[0] > streets[1], "{streets:?}");
}

#[test]
fn equity_seed_rung_bit_sets_the_rung() {
    for seed in [0u64, 1, 2, 999] {
        let exact = generate(Skill::Equity, equity_seed_with_rung(seed, 0));
        let classes = generate(Skill::Equity, equity_seed_with_rung(seed, 1));
        let (Question::Equity(a), Question::Equity(b)) = (&exact.question, &classes.question) else { panic!() };
        assert_eq!(a.rung(), 0);
        assert_eq!(b.rung(), 1);
    }
}

// ----- Equity skill: staircase and ladder -----

fn equity_attempt(seed: u64, t: u64, signed_error: f64, truth: f64, opponents: u8, band: u8) -> Attempt {
    let hit = signed_error.abs() <= f64::from(band);
    let mut row = attempt(Skill::Equity, seed, t, hit, 1000);
    row.extra.signed_error = Some(signed_error);
    row.extra.truth_pct = Some(truth);
    row.extra.opponents = Some(opponents);
    row.extra.band = Some(band);
    row.answer = format!("{}", truth + signed_error);
    row
}

#[test]
fn staircase_starts_at_ten_and_moves_by_one_and_six() {
    assert_eq!(EQUITY_BAND_START, 10);
    assert_eq!(next_band(10, true), 9);
    assert_eq!(next_band(10, false), 16);
    assert_eq!(next_band(3, true), 2);
    assert_eq!(next_band(2, true), EQUITY_BAND_MIN, "floor of 2");
    assert_eq!(next_band(18, false), EQUITY_BAND_MAX, "ceiling of 20");
    assert_eq!(next_band(20, false), EQUITY_BAND_MAX);
}

#[test]
fn staircase_reads_only_drill_attempts_of_its_own_rung() {
    let rung0 = equity_seed_with_rung(10, 0);
    let rung1 = equity_seed_with_rung(10, 1);
    // Two hits then a miss at rung 0: 10 -> 9 -> 8 -> 14.
    let mut log = vec![
        equity_attempt(rung0, 0, 1.0, 40.0, 1, 10),
        equity_attempt(rung0, 1, 1.0, 40.0, 1, 9),
        equity_attempt(rung0, 2, 15.0, 40.0, 1, 8),
    ];
    assert_eq!(equity_band(&log, 0), 14);
    assert_eq!(equity_band(&log, 1), 10, "rungs keep separate staircases");

    let mut retry = equity_attempt(rung0, 3, 15.0, 40.0, 1, 14);
    retry.mode = Mode::DrillRetry;
    log.push(retry);
    let mut check = equity_attempt(rung1 | HELD_OUT_BIT, 4, 15.0, 40.0, 1, 5);
    check.mode = Mode::Check;
    log.push(check);
    assert_eq!(equity_band(&log, 0), 14, "retries don't move the staircase");
    assert_eq!(equity_band(&log, 1), 10, "checkups don't move it either");
}

#[test]
fn staircase_settles_near_eighty_six_percent_hits() {
    // A player whose absolute error is uniform on 0..spread. The wider spread
    // stays clear of the ceiling, because a miss only happens below the spread.
    for spread in [12.0, 15.0] {
        let mut rng = Rng::new(spread as u64);
        let mut band = EQUITY_BAND_START;
        let steps = 200_000;
        let mut hits = 0u32;
        for _ in 0..steps {
            let error = rng.below(1_000_000) as f64 / 1_000_000.0 * spread;
            let hit = error <= f64::from(band);
            hits += u32::from(hit);
            band = next_band(band, hit);
            assert!((EQUITY_BAND_MIN..=EQUITY_BAND_MAX).contains(&band), "{band}");
        }
        let rate = f64::from(hits) / f64::from(steps);
        assert!((rate - 6.0 / 7.0).abs() < 0.01, "spread {spread}: {rate}");
    }
}

#[test]
fn staircase_stops_at_both_clamps() {
    // A player who always hits settles on the floor, and one who always misses
    // on the ceiling, however long the streak runs.
    let mut band = EQUITY_BAND_START;
    for _ in 0..500 {
        band = next_band(band, true);
        assert!(band >= EQUITY_BAND_MIN);
    }
    assert_eq!(band, EQUITY_BAND_MIN);
    for _ in 0..500 {
        band = next_band(band, false);
        assert!(band <= EQUITY_BAND_MAX);
    }
    assert_eq!(band, EQUITY_BAND_MAX);
}

#[test]
fn a_session_serves_the_band_and_tracks_it_like_the_log() {
    let rung0 = equity_seed_with_rung(10, 0);
    let log = vec![equity_attempt(rung0, 0, 1.0, 40.0, 1, 10)];
    let mut session = Session::new(vec![Skill::Equity], &log, 100, 7);
    let mut mirror = log.clone();
    for i in 0..12 {
        let planned = session.try_next_item().unwrap();
        assert_eq!(planned.skill, Skill::Equity);
        let item = session.item_for(planned);
        let Question::Equity(q) = &item.question else { panic!() };
        assert_eq!(q.band, equity_band(&mirror, 0), "turn {i}");
        let hit = i % 4 != 0;
        session.record(planned, hit);
        if planned.origin != Origin::SessionRetry {
            let err = if hit { 0.0 } else { 25.0 };
            mirror.push(equity_attempt(planned.seed, i + 1, err, 40.0, 1, q.band));
        } else {
            let mut retry = equity_attempt(planned.seed, i + 1, 25.0, 40.0, 1, q.band);
            retry.mode = Mode::DrillRetry;
            mirror.push(retry);
        }
    }
}

#[test]
fn drill_attempts_log_the_band_and_signed_error() {
    let item = equity_item("9c8c", "7c6c2d3s", EquityOpponents::Exact(vec![hand("AsAd")]), 7);
    let result = answer(&item, "30");
    let planned = poker_trainer::schedule::Planned { skill: Skill::Equity, seed: 1, origin: Origin::New };
    let row = Attempt::from_drill(planned, &result, 1, 500);
    assert_eq!(row.extra.band, Some(7));
    assert_eq!(row.extra.opponents, Some(1));
    let signed = row.extra.signed_error.unwrap();
    assert!((signed - (30.0 - 100.0 * 15.0 / 44.0)).abs() < 1e-9);
    let line = format_attempt(&row);
    assert_eq!(line.split('\t').count(), 18);
    let (parsed, _) = parse_log(&line);
    assert_eq!(parsed[0].extra.band, Some(7));
}

#[test]
fn promotion_needs_thirty_rung_zero_attempts_at_or_under_eight_points() {
    let seed = equity_seed_with_rung(10, 0);
    let build = |errors: &[f64]| -> Vec<Attempt> {
        errors.iter().enumerate().map(|(i, &e)| equity_attempt(seed, i as u64, e, 50.0, 1, 10)).collect()
    };
    assert_eq!(PROMOTE_WINDOW, 30);
    assert_eq!(PROMOTE_MAX_MAE, 8.0);
    assert_eq!(equity_rung(&build(&[1.0; 29])), 0, "29 attempts are not enough");
    assert_eq!(equity_rung(&build(&[8.0; 30])), 1, "exactly 8 points promotes");
    assert_eq!(equity_rung(&build(&[8.5; 30])), 0);
    // A long bad stretch then 30 good attempts.
    let mut mixed = vec![20.0; 50];
    mixed.extend(vec![6.0; 30]);
    assert_eq!(equity_rung(&build(&mixed)), 1);
    // Signs cancel in signed error, but promotion uses the absolute error.
    let mut signs = vec![20.0; 15];
    signs.extend(vec![-20.0; 15]);
    assert_eq!(equity_rung(&build(&signs)), 0);
}

#[test]
fn promotion_is_permanent_and_ignores_other_attempts() {
    let seed = equity_seed_with_rung(10, 0);
    let mut log: Vec<Attempt> =
        (0..30).map(|i| equity_attempt(seed, i, 2.0, 50.0, 1, 10)).collect();
    log.extend((30..60).map(|i| equity_attempt(seed, i, 30.0, 50.0, 1, 10)));
    assert_eq!(equity_rung(&log), 1, "later rung-0 reviews cannot demote");

    let rung1 = equity_seed_with_rung(10, 1);
    let mut other: Vec<Attempt> = (0..30).map(|i| equity_attempt(rung1, i, 1.0, 50.0, 1, 10)).collect();
    let mut retries: Vec<Attempt> = (0..30).map(|i| equity_attempt(seed, i, 1.0, 50.0, 1, 10)).collect();
    for row in &mut retries {
        row.mode = Mode::DrillRetry;
    }
    other.extend(retries);
    assert_eq!(equity_rung(&other), 0, "only rung-0 first answers count");
}

#[test]
fn a_session_deals_at_the_players_rung() {
    let promoted_seed = equity_seed_with_rung(10, 0);
    let log: Vec<Attempt> = (0..30).map(|i| equity_attempt(promoted_seed, i, 1.0, 50.0, 1, 10)).collect();
    let mut session = Session::new(vec![Skill::Equity], &log, 100, 3);
    let planned = session.try_next_item().unwrap();
    let Question::Equity(q) = session.item_for(planned).question else { panic!() };
    assert_eq!(q.rung(), 1, "a promoted player gets classes");

    let mut fresh = Session::new(vec![Skill::Equity], &[], 100, 3);
    let planned = fresh.try_next_item().unwrap();
    let Question::Equity(q) = fresh.item_for(planned).question else { panic!() };
    assert_eq!(q.rung(), 0);
}

#[test]
fn equity_pass_mark_is_mean_absolute_error_over_forty() {
    let seed = equity_seed_with_rung(10, 0);
    let build = |error: f64| -> Vec<Attempt> {
        (0..40).map(|i| equity_attempt(seed, i, error, 50.0, 1, 10)).collect()
    };
    assert!(skill_stats(Skill::Equity, &build(6.0)).drill_passed);
    assert!(!skill_stats(Skill::Equity, &build(6.5)).drill_passed);
    assert!((skill_stats(Skill::Equity, &build(-4.0)).mean_abs_error.unwrap() - 4.0).abs() < 1e-9);
}

// ----- Equity bias -----

#[test]
fn bias_groups_by_opponents_and_true_equity_band() {
    let seed = 10;
    let rows = vec![
        equity_attempt(seed, 0, 4.0, 10.0, 1, 10), // 0-20
        equity_attempt(seed, 1, -2.0, 25.0, 2, 10), // 20-40
        equity_attempt(seed, 2, 6.0, 20.0, 1, 10), // 20-40, the lower edge is included
        equity_attempt(seed, 3, 2.0, 100.0, 3, 10), // 80-100, the top edge is included
    ];
    let bias = equity_bias(&rows);
    assert_eq!(bias.count, 4);
    assert!((bias.mean_signed_error - 2.5).abs() < 1e-9);
    let opp = &bias.by_opponents;
    assert_eq!(opp[0].count, 2);
    assert!((opp[0].mean_signed_error - 5.0).abs() < 1e-9);
    assert_eq!((opp[1].count, opp[2].count), (1, 1));
    let bands = &bias.by_truth_band;
    assert_eq!(bands.iter().map(|b| b.count).collect::<Vec<_>>(), vec![1, 2, 0, 0, 1]);
    assert!((bands[1].mean_signed_error - 2.0).abs() < 1e-9, "(-2 + 6) / 2");
    assert_eq!(bands[2].mean_signed_error, 0.0, "an empty bucket reads as 0");
}

#[test]
fn bias_logit_fit_recovers_a_known_line() {
    // Estimates generated exactly from logit(est) = 0.25 + 0.5 * logit(truth).
    let rows: Vec<Attempt> = [10.0f64, 25.0, 50.0, 75.0, 90.0]
        .iter()
        .enumerate()
        .map(|(i, &truth)| {
            let lt = (truth / (100.0 - truth)).ln();
            let le = 0.25 + 0.5 * lt;
            let estimate = 100.0 * le.exp() / (1.0 + le.exp());
            equity_attempt(10, i as u64, estimate - truth, truth, 1, 10)
        })
        .collect();
    let fit = equity_bias(&rows).fit.unwrap();
    assert!((fit.intercept - 0.25).abs() < 1e-9, "{}", fit.intercept);
    assert!((fit.slope - 0.5).abs() < 1e-9, "{}", fit.slope);
    assert_eq!(fit.count, 5);
}

#[test]
fn bias_excludes_retries_and_clamps_logits() {
    let mut retry = equity_attempt(10, 0, 40.0, 50.0, 1, 10);
    retry.mode = Mode::DrillRetry;
    let rows = vec![
        retry,
        equity_attempt(10, 1, -5.0, 3.0, 1, 10),
        equity_attempt(10, 2, 2.0, 50.0, 1, 10),
    ];
    let bias = equity_bias(&rows);
    assert_eq!(bias.count, 2, "retries are excluded");
    // Estimate of -2 and truth of 3 still give finite logits after clamping.
    let fit = bias.fit.unwrap();
    assert!(fit.slope.is_finite() && fit.intercept.is_finite());
    assert!(equity_bias(&[]).fit.is_none());
    assert_eq!(BIAS_MIN_ATTEMPTS, 30);
}

// ----- Anchor deck -----

#[test]
fn the_deck_has_about_forty_cards_of_each_kind() {
    let deck = anchors::deck();
    assert_eq!(deck.len(), 40);
    let count = |pick: fn(&AnchorKind) -> bool| deck.iter().filter(|c| pick(&c.kind)).count();
    assert_eq!(count(|k| matches!(k, AnchorKind::Price { .. })), 8);
    assert_eq!(count(|k| matches!(k, AnchorKind::Matchup { .. })), 20);
    assert_eq!(count(|k| matches!(k, AnchorKind::Preflop { .. })), 12);
    for opponents in 1..=3u8 {
        assert_eq!(deck.iter().filter(|c| matches!(c.kind, AnchorKind::Preflop { opponents: n, .. } if n == opponents)).count(), 4);
    }
    let mut ids: Vec<&str> = deck.iter().map(|c| c.id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 40);
    assert_eq!(
        anchors::parse_deck(&anchors::to_text(deck)).unwrap(),
        deck.to_vec(),
        "the file round trips"
    );
    for card in deck {
        assert_eq!(card.tolerance, if matches!(card.kind, AnchorKind::Price { .. }) { 2 } else { 4 }, "{}", card.id);
    }
}

fn rounded(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

#[test]
fn every_price_and_matchup_answer_is_recomputed_exactly() {
    for card in anchors::deck() {
        match &card.kind {
            AnchorKind::Price { num, den } => {
                assert_eq!(rounded(anchors::price_answer(*num, *den)), card.answer_pct, "{}", card.id);
            }
            AnchorKind::Matchup { hero, villain, board } => {
                assert_eq!(rounded(anchors::matchup_answer(*hero, *villain, board)), card.answer_pct, "{}", card.id);
            }
            AnchorKind::Preflop { .. } => {}
        }
    }
}

#[test]
fn price_answers_match_the_lattice_by_hand() {
    // Bet b of a pot of 1: required = b / (1 + 2b).
    assert!((anchors::price_answer(1, 2) - 25.0).abs() < 1e-9);
    assert!((anchors::price_answer(1, 1) - 100.0 / 3.0).abs() < 1e-9);
    assert!((anchors::price_answer(2, 1) - 40.0).abs() < 1e-9);
    assert!((anchors::price_answer(1, 3) - 20.0).abs() < 1e-9);
}

/// Preflop equity by a separate simulator: its own seed, deck shuffle and
/// showdown, sharing only the hand evaluator with the library.
fn independent_preflop_equity(hero: Holding, opponents: usize, deals: u32, seed: u64) -> f64 {
    let mut rng = Rng::new(seed);
    let mut deck: Vec<Card> =
        (0..52u8).map(Card::from_index).filter(|c| *c != hero[0] && *c != hero[1]).collect();
    let needed = 5 + 2 * opponents;
    let mut share = 0.0;
    for _ in 0..deals {
        for i in 0..needed {
            let j = i + rng.below((deck.len() - i) as u64) as usize;
            deck.swap(i, j);
        }
        let board = &deck[..5];
        let rank = |a: Card, b: Card| {
            evaluate(&[a, b, board[0], board[1], board[2], board[3], board[4]])
        };
        let hero_rank = rank(hero[0], hero[1]);
        let others = (0..opponents).map(|n| rank(deck[5 + 2 * n], deck[6 + 2 * n]));
        let (mut beaten, mut tied) = (false, 1u32);
        for other in others {
            beaten |= other > hero_rank;
            tied += u32::from(other == hero_rank);
        }
        if !beaten {
            share += 1.0 / f64::from(tied);
        }
    }
    100.0 * share / f64::from(deals)
}

#[test]
fn preflop_anchors_match_an_independent_simulation() {
    for (index, card) in anchors::deck().iter().enumerate() {
        if let AnchorKind::Preflop { hand, opponents } = &card.kind {
            let seed = 0x5EED_0000 + index as u64;
            let check = independent_preflop_equity(*hand, usize::from(*opponents), 300_000, seed);
            // The standard error is at most 0.09 points; this allows over five.
            assert!((check - card.answer_pct).abs() < 0.5, "{}: {check} vs {}", card.id, card.answer_pct);
        }
    }
}

#[test]
fn the_deck_file_matches_the_generator_for_every_fast_part() {
    let fast = anchors::to_text(&anchors::price_and_matchup_cards());
    assert!(anchors::ANCHORS_TEXT.starts_with(&fast), "the header, price and matchup cards match byte for byte");
    let stored: Vec<_> =
        anchors::deck().iter().filter(|c| matches!(c.kind, AnchorKind::Preflop { .. })).collect();
    // One deal is enough here: only the simulated answer is left out.
    let generated = anchors::preflop_cards(1);
    assert_eq!(stored.len(), generated.len());
    for (a, b) in stored.iter().zip(&generated) {
        assert_eq!((&a.id, &a.kind, &a.prompt, a.tolerance), (&b.id, &b.kind, &b.prompt, b.tolerance));
    }
}

#[test]
#[ignore = "re-runs the full preflop simulation, about half a minute in release"]
fn preflop_anchors_match_the_full_simulation() {
    assert_eq!(anchors::to_text(&anchors::generate_deck()), anchors::ANCHORS_TEXT);
}

#[test]
fn anchor_answers_are_graded_within_tolerance() {
    let deck = anchors::deck();
    let index = deck.iter().position(|c| c.id == "p03").unwrap();
    let item = Item { skill: Skill::Anchor, seed: index as u64, question: Question::Anchor { card: index as u16 } };
    assert!(answer(&item, "25").correct);
    assert!(answer(&item, "27").correct, "2 points on a price card");
    assert!(!answer(&item, "27.5").correct);
    let matchup = deck.iter().position(|c| c.id == "m01").unwrap();
    let item = Item { skill: Skill::Anchor, seed: matchup as u64, question: Question::Anchor { card: matchup as u16 } };
    assert!(answer(&item, &format!("{}", deck[matchup].answer_pct + 4.0)).correct);
    assert!(!answer(&item, &format!("{}", deck[matchup].answer_pct + 4.5)).correct);
    assert_eq!(answer_format(&Question::Anchor { card: 0 }), AnswerFormat::Percent);
    let Reveal::Anchor { card, estimate_pct, signed_error } = answer(&item, "70").reveal else { panic!() };
    assert_eq!(card.id, "m01");
    assert_eq!(estimate_pct, 70.0);
    assert!((signed_error - (70.0 - card.answer_pct)).abs() < 1e-9);
}

#[test]
fn seeds_map_to_cards_and_generate_returns_them() {
    for card in 0..anchors::deck().len() {
        let seed = anchors::seed_with_card(0, card);
        assert_eq!(anchors::card_of_seed(seed), card);
        assert_eq!(generate(Skill::Anchor, seed).question, Question::Anchor { card: card as u16 });
    }
    let held = anchors::seed_with_card(HELD_OUT_BIT | 0xABC0000, 3);
    assert_eq!(anchors::card_of_seed(held), 3);
}

// ----- Leitner scheduling -----

fn card_row(card: usize, t: u64, correct: bool) -> Attempt {
    attempt(Skill::Anchor, anchors::seed_with_card(0, card), t, correct, 2_000)
}

#[test]
fn boxes_move_up_on_a_hit_and_back_to_one_on_a_miss() {
    assert_eq!(LEITNER_INTERVAL_DAYS, [1, 3, 7, 14, 30]);
    // First answer right: box 2, next due 3 days later.
    let log = vec![card_row(0, 0, true)];
    let s = anchor_status(&log)[0];
    assert_eq!(s.leitner_box, 2);
    assert_eq!(s.due_at_ms, Some(3 * DAY));

    // Hit, hit, hit, hit: box 5, 30 days.
    let t = [0, 3 * DAY, 10 * DAY, 24 * DAY, 54 * DAY];
    let log: Vec<Attempt> = t.iter().map(|&at| card_row(1, at, true)).collect();
    let s = anchor_status(&log)[1];
    assert_eq!(s.leitner_box, 5);
    assert_eq!(s.due_at_ms, Some(84 * DAY), "box 5 keeps a 30-day interval");

    let mut missed = log.clone();
    missed.push(card_row(1, 90 * DAY, false));
    let s = anchor_status(&missed)[1];
    assert_eq!(s.leitner_box, 1);
    assert_eq!(s.due_at_ms, Some(91 * DAY));
}

#[test]
fn due_cards_come_longest_overdue_first_and_new_cards_in_deck_order() {
    let log = vec![card_row(2, 0, false), card_row(5, 3 * DAY, false)];
    // Card 2 was due on day 1, card 5 on day 4.
    assert_eq!(anchors_due(&log, 5 * DAY), vec![2, 5]);
    assert_eq!(anchors_due(&log, 2 * DAY), vec![2]);
    assert!(anchors_due(&log, DAY - 1).is_empty());
    let fresh = anchors_new(&log);
    assert_eq!(fresh[..3], [0, 1, 3]);
    assert_eq!(fresh.len(), anchors::deck().len() - 2);
}

#[test]
fn retries_and_checkups_do_not_move_the_boxes() {
    let mut retry = card_row(0, 0, true);
    retry.mode = Mode::DrillRetry;
    let mut check = card_row(0, 1, true);
    check.mode = Mode::Check;
    let status = anchor_status(&[retry, check]);
    assert_eq!(status[0].leitner_box, 1);
    assert_eq!(status[0].due_at_ms, None);
}

#[test]
fn session_deals_due_cards_first_and_at_most_five_new() {
    assert_eq!(MAX_NEW_ANCHORS_PER_SESSION, 5);
    let log = vec![card_row(9, 0, false)];
    let mut session = Session::new(vec![Skill::Anchor], &log, DAY * 2, 1);
    let first = session.try_next_item().unwrap();
    assert_eq!(first.origin, Origin::Due);
    assert_eq!(anchors::card_of_seed(first.seed), 9);
    session.record(first, true);
    for _ in 0..5 {
        let planned = session.try_next_item().unwrap();
        assert_eq!(planned.origin, Origin::New);
        session.record(planned, true);
    }
    assert_eq!(session.try_next_item(), None, "nothing left to deal");
}

#[test]
fn a_session_with_no_anchor_cards_deals_other_skills() {
    let log: Vec<Attempt> = (0..anchors::deck().len()).map(|card| card_row(card, 0, true)).collect();
    let mut session = Session::new(vec![Skill::Anchor, Skill::Price], &log, 1, 2);
    for i in 0..30 {
        let planned = session.try_next_item().unwrap();
        assert_eq!(planned.skill, Skill::Price, "turn {i}");
        session.record(planned, true);
    }
}

#[test]
fn an_anchor_only_session_always_serves_its_pending_retries() {
    for seed in 0..500 {
        let mut session = Session::new(vec![Skill::Anchor], &[], 0, seed);
        let (mut missed, mut retried) = (Vec::new(), Vec::new());
        while let Some(planned) = session.try_next_item() {
            if planned.origin == Origin::SessionRetry {
                retried.push(planned.seed);
                session.record(planned, true);
            } else {
                missed.push(planned.seed);
                session.record(planned, false);
            }
        }
        missed.sort_unstable();
        retried.sort_unstable();
        assert_eq!(missed.len(), MAX_NEW_ANCHORS_PER_SESSION as usize, "seed {seed}");
        assert_eq!(missed, retried, "seed {seed}: every miss comes back exactly once");
    }
}

#[test]
fn anchor_passes_at_ninety_percent_within_tolerance_and_is_step_two() {
    let log: Vec<Attempt> = (0..40).map(|i| card_row(0, i, i < 36)).collect();
    assert!(skill_stats(Skill::Anchor, &log).drill_passed, "36 of 40");
    let log: Vec<Attempt> = (0..40).map(|i| card_row(0, i, i < 35)).collect();
    assert!(!skill_stats(Skill::Anchor, &log).drill_passed, "35 of 40");
    assert!(STEPS[1].contains(&Skill::Anchor) && STEPS[1].contains(&Skill::Equity));
    assert!(STEPS[0].contains(&Skill::BoardType));
}

#[test]
fn anchor_misses_retry_once_but_are_not_scheduled_by_due_items() {
    let log = vec![card_row(0, 0, false)];
    assert!(due_items(&log, DAY * 9).is_empty());
    let mut session = Session::new(vec![Skill::Anchor], &[], 0, 5);
    let first = session.try_next_item().unwrap();
    session.record(first, false);
    let mut retried = false;
    for _ in 0..8 {
        let Some(planned) = session.try_next_item() else { break };
        retried |= planned.seed == first.seed && planned.origin == Origin::SessionRetry;
        session.record(planned, true);
    }
    assert!(retried, "a miss comes back once in the same session");
}

// ----- Board type -----

fn flop(text: &str) -> BoardCategory {
    classify_flop(&cards(text))
}

const fn cat(suit: SuitPattern, rank: RankPattern) -> BoardCategory {
    BoardCategory { suit, rank }
}

#[test]
fn flops_classify_by_suit_pattern_and_rank_pattern() {
    use RankPattern::{Connected, Disconnected, Paired};
    use SuitPattern::{Monotone, Rainbow, TwoTone};
    assert_eq!(flop("Ks8h2d"), cat(Rainbow, Disconnected));
    assert_eq!(flop("Ks8s2d"), cat(TwoTone, Disconnected));
    assert_eq!(flop("Ks8s2s"), cat(Monotone, Disconnected));
    assert_eq!(flop("9s8h7d"), cat(Rainbow, Connected));
    assert_eq!(flop("9s8s7d"), cat(TwoTone, Connected));
    assert_eq!(flop("9s8s7s"), cat(Monotone, Connected));
    assert_eq!(flop("KsKh7d"), cat(Rainbow, Paired));
    assert_eq!(flop("KsKh7s"), cat(TwoTone, Paired));
    assert_eq!(flop("KsKhKd"), cat(Rainbow, Paired), "trips count as paired");
}

#[test]
fn connected_is_three_ranks_within_a_span_of_five_with_the_ace_both_ways() {
    use RankPattern::{Connected, Disconnected};
    let rank = |t: &str| flop(t).rank;
    assert_eq!(rank("Js8h7d"), Connected, "J-8-7 fits 7 through jack");
    assert_eq!(rank("Qs8h7d"), Disconnected, "Q-8-7 needs a span of six");
    assert_eq!(rank("As2h5d"), Connected, "ace low: A-2-3-4-5");
    assert_eq!(rank("As2h3d"), Connected);
    assert_eq!(rank("AsKhTd"), Connected, "ace high: T-J-Q-K-A");
    assert_eq!(rank("AsQh9d"), Disconnected, "A-Q-9 spans too far with the ace high");
    assert_eq!(rank("As3h6d"), Disconnected, "A-3-6 spans too far with the ace low");
    assert_eq!(flop("KsKh9d").rank, RankPattern::Paired, "paired wins over connected");
}

#[test]
fn eight_of_the_nine_labels_exist_and_every_one_has_distractors() {
    assert_eq!(BoardCategory::ALL.len(), 8);
    let impossible = cat(SuitPattern::Monotone, RankPattern::Paired);
    assert!(!BoardCategory::ALL.contains(&impossible));
    for category in BoardCategory::ALL {
        let options = one_attribute_distractors(category);
        assert!(options.len() >= 3, "{category:?}");
        for d in options {
            assert_ne!(d, category);
            assert_ne!(d.suit == category.suit, d.rank == category.rank, "exactly one attribute differs");
        }
    }
    // Enumerate every flop to prove a paired monotone board cannot appear.
    for a in 0..50u8 {
        for b in a + 1..51 {
            for c in b + 1..52 {
                let board = [Card::from_index(a), Card::from_index(b), Card::from_index(c)];
                let found = classify_flop(&board);
                assert!(BoardCategory::ALL.contains(&found), "{found:?}");
            }
        }
    }
}

#[test]
fn board_items_offer_the_right_answer_among_four_distinct_choices() {
    for seed in 0..600u64 {
        let seed = seed_with_category(seed * 31, BoardCategory::ALL[(seed % 8) as usize]);
        let item = generate(Skill::BoardType, seed);
        let Question::BoardType { board, options } = &item.question else { panic!() };
        assert_eq!(board.len(), 3);
        let right = classify_flop(board);
        assert_eq!(right, category_of_seed(seed));
        assert!(options.contains(&right));
        for (i, a) in options.iter().enumerate() {
            for b in &options[i + 1..] {
                assert_ne!(a, b, "seed {seed}");
            }
        }
        assert!(answer(&item, right.code()).correct);
        for option in options.iter().filter(|o| **o != right) {
            assert_ne!(option.suit == right.suit, option.rank == right.rank);
            assert!(!answer(&item, option.code()).correct);
        }
    }
}

#[test]
fn board_type_rejects_codes_that_are_not_on_offer() {
    let item = generate(Skill::BoardType, seed_with_category(9, cat(SuitPattern::Rainbow, RankPattern::Disconnected)));
    let Question::BoardType { options, .. } = &item.question else { panic!() };
    let missing = BoardCategory::ALL.into_iter().find(|c| !options.contains(c)).unwrap();
    assert!(grade(&item, missing.code()).is_err());
    assert!(grade(&item, "nonsense").is_err());
    let AnswerFormat::Choice(choices) = answer_format(&item.question) else { panic!() };
    assert_eq!(choices.len(), 4);
    assert_eq!(choices[0].code, options[0].code());
}

#[test]
fn a_slow_correct_answer_is_a_miss() {
    let category = cat(SuitPattern::Rainbow, RankPattern::Connected);
    let item = generate(Skill::BoardType, seed_with_category(3, category));
    assert_eq!(BOARD_TYPE_MISS_MS, 12_000);
    let right = category.code();
    assert!(grade_timed(&item, right, 12_000).unwrap().correct, "exactly the limit counts");
    let slow = grade_timed(&item, right, 12_001).unwrap();
    assert!(!slow.correct);
    assert!(matches!(slow.reveal, Reveal::BoardType { too_slow: true, .. }));
    // A slow wrong answer is simply wrong, and other skills are unaffected.
    let Question::BoardType { options, .. } = &item.question else { panic!() };
    let wrong = options.iter().find(|c| **c != category).unwrap().code();
    let result = grade_timed(&item, wrong, 40_000).unwrap();
    assert!(!result.correct);
    assert!(matches!(result.reveal, Reveal::BoardType { too_slow: false, .. }));
    let price = generate(Skill::Price, 4);
    assert!(grade_timed(&price, "99", 40_000).unwrap().reveal_is_not_board_type());
}

trait RevealCheck {
    fn reveal_is_not_board_type(&self) -> bool;
}

impl RevealCheck for Outcome {
    fn reveal_is_not_board_type(&self) -> bool {
        !matches!(self.reveal, Reveal::BoardType { .. })
    }
}

fn board_row(category: BoardCategory, t: u64, correct: bool, ms: u32) -> Attempt {
    attempt(Skill::BoardType, seed_with_category(t * 7, category), t, correct, ms)
}

#[test]
fn a_category_retires_after_three_correct_inside_the_target() {
    let target = BOARD_TYPE_TARGET_MS;
    let category = BoardCategory::ALL[2];
    let status = |log: &[Attempt]| board_type_status(log)[category.index()].clone();

    let log: Vec<Attempt> = (0..3).map(|t| board_row(category, t, true, target)).collect();
    let s = status(&log);
    assert_eq!(s.streak, 3);
    assert!(s.retired);

    let mut log: Vec<Attempt> = (0..2).map(|t| board_row(category, t, true, 2_000)).collect();
    log.push(board_row(category, 2, true, target + 1));
    log.push(board_row(category, 3, true, 2_000));
    let s = status(&log);
    assert_eq!(s.streak, 1, "a correct answer over the target breaks the streak");
    assert!(!s.retired);

    let mut log: Vec<Attempt> = (0..3).map(|t| board_row(category, t, true, 2_000)).collect();
    log.push(board_row(category, 3, false, 2_000));
    let s = status(&log);
    assert_eq!(s.streak, 0);
    assert!(!s.retired, "a later miss brings a category back");
    assert_eq!((s.attempts, s.correct), (4, 3));
}

#[test]
fn at_least_three_other_trials_separate_repeats_of_a_category() {
    let mut dealer = CategoryDealer::new(&[]);
    let mut rng = poker_trainer::rng::Rng::new(9);
    let mut last: Vec<BoardCategory> = Vec::new();
    for _ in 0..4000 {
        let category = dealer.deal(&mut rng);
        if let Some(position) = last.iter().rposition(|c| *c == category) {
            assert!(last.len() - position > 3, "{}", last.len() - position - 1);
        }
        last.push(category);
    }
}

#[test]
fn unretired_categories_are_dealt_far_more_than_retired_ones() {
    let retired_set = &BoardCategory::ALL[..4];
    let mut log = Vec::new();
    for (i, category) in retired_set.iter().enumerate() {
        for t in 0..3 {
            log.push(board_row(*category, (i as u64) * 3 + t, true, 2_000));
        }
    }
    let mut dealer = CategoryDealer::new(&log);
    for category in retired_set {
        assert!(dealer.is_retired(*category));
    }
    let mut rng = poker_trainer::rng::Rng::new(1);
    let (mut retired, mut live) = (0, 0);
    for _ in 0..4000 {
        if retired_set.contains(&dealer.deal(&mut rng)) {
            retired += 1;
        } else {
            live += 1;
        }
    }
    assert!(retired > 0, "retired categories still get reviewed");
    assert!(live > retired * 2, "{live} vs {retired}");
}

#[test]
fn a_session_applies_the_board_protocol_and_never_reuses_a_miss() {
    let mut session = Session::new(vec![Skill::BoardType], &[], 0, 8);
    let mut seen: Vec<BoardCategory> = Vec::new();
    for i in 0..600 {
        let planned = session.try_next_item().unwrap();
        assert_eq!(planned.skill, Skill::BoardType);
        assert_ne!(planned.origin, Origin::SessionRetry, "boards are not retried");
        let category = category_of_seed(planned.seed);
        if let Some(position) = seen.iter().rposition(|c| *c == category) {
            assert!(seen.len() - position > 3, "turn {i}");
        }
        seen.push(category);
        session.record_timed(planned, i % 3 != 0, 2_000);
    }
}

#[test]
fn board_type_misses_are_never_scheduled_for_review() {
    let category = BoardCategory::ALL[0];
    let log = vec![board_row(category, 1, false, 1000)];
    assert!(due_items(&log, DAY * 30).is_empty());
}

#[test]
fn board_type_passes_at_ninety_five_percent_and_six_seconds() {
    let category = BoardCategory::ALL[0];
    let log: Vec<Attempt> = (0..40).map(|t| board_row(category, t, t != 0, 6_000)).collect();
    assert!(skill_stats(Skill::BoardType, &log).drill_passed, "39 of 40 inside 6 s");
    let slow: Vec<Attempt> = (0..40).map(|t| board_row(category, t, true, 6_500)).collect();
    assert!(!skill_stats(Skill::BoardType, &slow).drill_passed);
}

#[test]
fn existing_generators_are_unchanged_by_the_new_skills() {
    // Skill::Spot's generator seed must not shift because new variants were added.
    assert_eq!(Skill::SpotMw as usize, 9);
    assert_eq!(Skill::BoardType as usize, 10);
    assert_eq!(baseline::to_text(&baseline::generate_baseline()), baseline::BASELINE_TEXT);
}

#[test]
fn every_new_skill_round_trips_its_name_and_step() {
    assert_eq!(Skill::from_name("board_type"), Some(Skill::BoardType));
    assert_eq!(Skill::from_name("equity"), Some(Skill::Equity));
    assert_eq!(Skill::from_name("anchor"), Some(Skill::Anchor));
    assert_eq!(Skill::BoardType.step(), Some(1));
    assert_eq!(Skill::Equity.step(), Some(2));
    assert_eq!(Skill::Anchor.step(), Some(2));
    let held = generate(Skill::Nuts, HELD_OUT_BIT | 5);
    assert!(is_held_out(held.seed));
    let _: u64 = HELD_OUT_BIT;
    let _ = EQUITY_MIN_PCT + EQUITY_MAX_PCT;
}
