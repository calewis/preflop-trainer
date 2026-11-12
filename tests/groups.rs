//! Every drill open from the first day: the seven practice groups, their fixed
//! shares in the default drill, one-group drills, the monthly mixed checkup and
//! old histories loading under the new rules.

use std::collections::HashMap;

use poker_trainer::checkup::{
    checkup_runs, mixed_checkup_offer, mixed_checkup_plan, next_checkup_form, next_form, next_mixed_form, unfinished_run,
    CheckupKind, CheckupMetric, CheckupOffer, FIRST_CHECKUP_MIN_DAYS, FIRST_CHECKUP_MIN_DRILLS, MIXED_CHECKUP_EVERY_DAYS,
    MIXED_CHECKUP_LEN, MIXED_CHECKUP_MIX, MIXED_FORM_BASE,
};
use poker_trainer::generate::generate;
use poker_trainer::groups::{group_of, mixed_skills, weight_in_group, Group};
use poker_trainer::item::{chain_hand_seed, chain_stage_of_seed, is_held_out, ChainStage, Skill, GEN_VERSION};
use poker_trainer::log::{parse_log, Attempt, AttemptExtra, CheckRef, ItemRef, Mode};
use poker_trainer::progress::{all_stats, drill_skills};
use poker_trainer::rng::Rng;
use poker_trainer::schedule::{Origin, Session, DAY_MS, MIN_PREFLOP_PERCENT};

fn drill_row(skill: Skill, seed: u64, t: u64, ok: bool, family: Option<&str>) -> Attempt {
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
        extra: AttemptExtra { family: family.map(str::to_string), ..AttemptExtra::default() },
    }
}

/// Rows for the first `count` slots of a mixed checkup, the first `wrong` of them missed.
fn mixed_rows(run: u32, form: u32, count: usize, wrong: usize, start: u64) -> Vec<Attempt> {
    mixed_checkup_plan(form)
        .slots
        .iter()
        .enumerate()
        .take(count)
        .map(|(slot, s)| {
            let mut row = drill_row(s.skill, s.seed, start + slot as u64, slot >= wrong, None);
            row.mode = Mode::Check;
            row.extra.check = Some(CheckRef { run, form, slot: slot as u32 });
            row
        })
        .collect()
}

// ----- the groups --------------------------------------------------------------

#[test]
fn the_seven_groups_have_the_agreed_shares_and_cover_every_dealt_skill() {
    let shares: Vec<(Group, u64)> = Group::ALL.iter().map(|&g| (g, g.share_pct())).collect();
    assert_eq!(
        shares,
        vec![
            (Group::Preflop, 20),
            (Group::ReadPrice, 15),
            (Group::Equity, 15),
            (Group::WholeHands, 15),
            (Group::Board, 15),
            (Group::Outs, 10),
            (Group::Spots, 10),
        ]
    );
    assert_eq!(MIN_PREFLOP_PERCENT, 20, "the preflop floor is its group's share");
    for skill in Skill::ALL {
        let retired = matches!(skill, Skill::Price | Skill::Beats);
        assert_eq!(group_of(skill).is_none(), retired, "{skill:?}");
        assert_eq!(Group::ALL.iter().filter(|g| g.skills().contains(&skill)).count(), usize::from(!retired), "{skill:?} in one group");
    }
    assert_eq!(mixed_skills().len(), Skill::ALL.len() - 2);
    for g in Group::ALL {
        assert_eq!(Group::from_code(g.code()), Some(g));
    }
    assert!(weight_in_group(Skill::Table) < weight_in_group(Skill::Spot), "table spots come least");
}

/// Group counts over `sessions` default drills of `items` each, for a brand-new
/// player who misses about one in `miss_one_in`.
fn mixed_counts(sessions: u64, items: u64, miss_one_in: u64) -> (HashMap<Group, u64>, u64) {
    let mut counts: HashMap<Group, u64> = HashMap::new();
    let mut total = 0;
    for seed in 0..sessions {
        let mut session = Session::mixed(&[], 1_000, seed * 6_151 + 7);
        let mut misses = Rng::new(seed ^ 0xA5A5);
        for _ in 0..items {
            let planned = session.try_next_item().expect("the default drill never runs out");
            let group = group_of(planned.skill).unwrap_or_else(|| panic!("{:?} has no group", planned.skill));
            *counts.entry(group).or_default() += 1;
            total += 1;
            session.record(planned, !misses.chance(1, miss_one_in));
        }
    }
    (counts, total)
}

#[test]
fn a_new_players_default_drill_deals_every_group_at_its_share() {
    for miss_one_in in [20, 4, 2] {
        let (counts, total) = mixed_counts(4, 2_000, miss_one_in);
        for g in Group::ALL {
            let pct = 100.0 * counts.get(&g).copied().unwrap_or(0) as f64 / total as f64;
            println!("miss 1 in {miss_one_in}: {:<15} {pct:.1}%", g.title());
            // Retries, reviews and chain stages all count toward a group's share.
            assert!((pct - g.share_pct() as f64).abs() <= 1.5, "miss 1 in {miss_one_in}: {} at {pct:.1}%", g.title());
        }
    }
}

#[test]
fn every_group_comes_up_in_a_ten_minute_session() {
    for seed in 0..300u64 {
        let mut session = Session::mixed(&[], 1_000, seed);
        let mut misses = Rng::new(seed);
        let mut seen: Vec<Group> = Vec::new();
        for _ in 0..45 {
            let planned = session.try_next_item().unwrap();
            let group = group_of(planned.skill).unwrap();
            if !seen.contains(&group) {
                seen.push(group);
            }
            session.record(planned, !misses.chance(1, 4));
        }
        assert_eq!(seen.len(), 7, "seed {seed}: only {seen:?}");
    }
}

#[test]
fn the_shares_hold_at_every_point_not_just_on_average() {
    for seed in 0..200u64 {
        let mut session = Session::mixed(&[], 1_000, seed * 31 + 1);
        let mut dealt: HashMap<Group, i64> = HashMap::new();
        for n in 1..=300i64 {
            let planned = session.try_next_item().unwrap();
            *dealt.entry(group_of(planned.skill).unwrap()).or_default() += 1;
            for g in Group::ALL {
                // Ahead by at most the slack, a whole hand and a retry; never far behind.
                let ahead = 100 * dealt.get(&g).copied().unwrap_or(0) - g.share_pct() as i64 * n;
                assert!(ahead <= 600, "seed {seed} item {n}: {} ahead by {ahead}", g.title());
                assert!(ahead >= -600, "seed {seed} item {n}: {} behind by {ahead}", g.title());
            }
            session.record(planned, n % 5 != 0);
        }
    }
}

#[test]
fn price_and_beats_are_never_dealt_even_with_reviews_due() {
    // A player of the live release who missed both: their families are due today.
    let mut log = Vec::new();
    for i in 0..30u64 {
        log.push(drill_row(Skill::Price, i, i, false, Some("p:small")));
        log.push(drill_row(Skill::Beats, 100 + i, i, false, Some("b:pair")));
        let made = poker_trainer::family::family_of(&generate(Skill::Made, 200 + i));
        log.push(drill_row(Skill::Made, 200 + i, i, false, made.as_deref()));
    }
    let now = 5 * DAY_MS;
    for seed in 0..60u64 {
        let mut session = Session::mixed(&log, now, seed);
        let mut reviews = 0;
        for _ in 0..200 {
            let planned = session.try_next_item().unwrap();
            assert!(!matches!(planned.skill, Skill::Price | Skill::Beats), "seed {seed}: dealt {:?}", planned.skill);
            reviews += u32::from(planned.origin == Origin::Due);
            session.record(planned, !planned.seed.is_multiple_of(3));
        }
        assert!(reviews > 0, "seed {seed}: the made review still comes");
    }
    for group in Group::ALL {
        let mut session = Session::for_group(group, &log, now, 3);
        for _ in 0..100 {
            let Some(planned) = session.try_next_item() else { break };
            assert!(!matches!(planned.skill, Skill::Price | Skill::Beats), "{group:?} dealt {:?}", planned.skill);
            session.record(planned, false);
        }
    }
}

#[test]
fn picking_a_group_deals_only_that_group_and_every_skill_in_it() {
    for group in Group::ALL {
        let mut counts: HashMap<Skill, u32> = HashMap::new();
        for seed in 0..20u64 {
            let mut session = Session::for_group(group, &[], 1_000, seed);
            for i in 0..100u64 {
                let Some(planned) = session.try_next_item() else { break };
                assert_eq!(group_of(planned.skill), Some(group), "{group:?} dealt {:?}", planned.skill);
                *counts.entry(planned.skill).or_default() += 1;
                session.record(planned, i % 4 != 0);
            }
        }
        for skill in group.skills() {
            assert!(counts.get(&skill).copied().unwrap_or(0) > 0, "{group:?} never dealt {skill:?}: {counts:?}");
        }
    }
}

#[test]
fn a_one_group_drill_weights_its_skills_like_the_default_drill() {
    let mut counts: HashMap<Skill, u32> = HashMap::new();
    for seed in 0..40u64 {
        let mut session = Session::for_group(Group::Spots, &[], 1_000, seed);
        for _ in 0..100 {
            let planned = session.try_next_item().unwrap();
            *counts.entry(planned.skill).or_default() += 1;
            session.record(planned, true);
        }
    }
    let table = f64::from(counts[&Skill::Table]) / 4000.0;
    // Weights 2:2:1, so table is about a fifth.
    assert!((0.15..=0.25).contains(&table), "table at {table:.3}: {counts:?}");
}

#[test]
fn a_group_drill_has_no_preflop_floor_or_shares() {
    let mut session = Session::for_group(Group::Board, &[], 1_000, 4);
    for _ in 0..50 {
        let planned = session.try_next_item().unwrap();
        assert_ne!(planned.skill, Skill::Preflop);
        session.record(planned, true);
    }
}

#[test]
fn the_ladders_inside_skills_still_set_new_items() {
    // A read_price player promoted to rung 1 gets rung 1 items in the default drill.
    let t = poker_trainer::ladder::OLD_RULE_CUTOFF_MS + 1;
    let log: Vec<Attempt> = (0..22u64)
        .map(|i| {
            let seed = poker_trainer::item::read_price_seed_with_rung(i * 977, 0);
            drill_row(Skill::ReadPrice, seed, t + i, true, None)
        })
        .collect();
    assert_eq!(poker_trainer::read_price::read_price_rung(&log), 1);
    let mut session = Session::mixed(&log, t + 100, 8);
    let mut seen = 0;
    for _ in 0..200 {
        let planned = session.try_next_item().unwrap();
        if planned.skill == Skill::ReadPrice && planned.origin == Origin::New {
            assert_eq!(poker_trainer::item::read_price_rung_of_seed(planned.seed), 1);
            seen += 1;
        }
        session.record(planned, true);
    }
    assert!(seen > 10);
}

// ----- the monthly mixed checkup -----------------------------------------------

#[test]
fn the_mixed_checkup_is_forty_held_out_items_across_the_groups() {
    assert_eq!(MIXED_CHECKUP_MIX.iter().map(|&(_, n)| n).sum::<usize>(), MIXED_CHECKUP_LEN);
    assert_eq!(MIXED_CHECKUP_LEN, 40);
    for form in MIXED_FORM_BASE..MIXED_FORM_BASE + 6 {
        let plan = mixed_checkup_plan(form);
        assert_eq!(plan, mixed_checkup_plan(form), "deterministic");
        assert_eq!(plan.kind, CheckupKind::Mixed);
        assert_eq!(plan.slots.len(), 40);
        let mut by_group: HashMap<Group, usize> = HashMap::new();
        let mut by_skill: HashMap<Skill, usize> = HashMap::new();
        for (i, slot) in plan.slots.iter().enumerate() {
            assert!(is_held_out(slot.seed), "form {form} slot {i}");
            assert!(slot.repeat_of.is_none(), "no repeats");
            *by_group.entry(group_of(slot.skill).expect("every item has a group")).or_default() += 1;
            *by_skill.entry(slot.skill).or_default() += 1;
            let item = plan.item(i);
            assert_eq!(item.skill, slot.skill);
        }
        for &(skill, count) in &MIXED_CHECKUP_MIX {
            assert_eq!(by_skill.get(&skill).copied().unwrap_or(0), count, "{skill:?}");
        }
        let want = [
            (Group::Preflop, 8),
            (Group::ReadPrice, 7),
            (Group::Equity, 7),
            (Group::WholeHands, 4),
            (Group::Board, 6),
            (Group::Outs, 4),
            (Group::Spots, 4),
        ];
        for (g, n) in want {
            assert_eq!(by_group.get(&g).copied().unwrap_or(0), n, "form {form}: {g:?}");
        }
        assert!(!by_skill.contains_key(&Skill::Table) && !by_skill.contains_key(&Skill::Price) && !by_skill.contains_key(&Skill::Beats));
        // The whole hand's stages come together and in order.
        let first = plan.slots.iter().position(|s| s.skill == Skill::Chain).unwrap();
        for (k, stage) in ChainStage::ALL.into_iter().enumerate() {
            let slot = plan.slots[first + k];
            assert_eq!(slot.skill, Skill::Chain);
            assert_eq!(chain_stage_of_seed(slot.seed), stage);
            assert_eq!(chain_hand_seed(slot.seed), chain_hand_seed(plan.slots[first].seed));
        }
    }
    assert_ne!(mixed_checkup_plan(MIXED_FORM_BASE).slots, mixed_checkup_plan(MIXED_FORM_BASE + 1).slots, "forms differ");
    // Its items never turn up in a drill: drills only deal seeds without the held-out bit.
    let item = generate(Skill::Preflop, mixed_checkup_plan(MIXED_FORM_BASE).slots[0].seed);
    assert!(is_held_out(item.seed));
}

#[test]
fn a_mixed_run_is_scored_by_group_and_can_be_resumed() {
    let form = next_mixed_form(&[]);
    assert_eq!(form, MIXED_FORM_BASE);
    let partial = mixed_rows(4, form, 15, 0, 10_000);
    let runs = checkup_runs(&partial);
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].kind, CheckupKind::Mixed);
    assert!(!runs[0].complete && !runs[0].stale && runs[0].answered == 15);
    assert_eq!(unfinished_run(&runs, CheckupKind::Mixed).map(|r| r.run), Some(4));
    assert_eq!(next_mixed_form(&partial), form + 1);

    let full = mixed_rows(4, form, 40, 10, 10_000);
    let score = &checkup_runs(&full)[0];
    assert!(score.complete && !score.passed, "a mixed checkup has no mark to pass");
    assert_eq!(score.metric, CheckupMetric::PercentOk);
    assert!((score.score - 75.0).abs() < 1e-9, "{}", score.score);
    assert_eq!(score.groups.iter().map(|g| g.answered).sum::<usize>(), 40);
    assert_eq!(score.groups.iter().map(|g| g.ok).sum::<usize>(), 30);
    assert_eq!(score.generator_key, Some(GEN_VERSION));
    assert!(unfinished_run(&checkup_runs(&full), CheckupKind::Mixed).is_none());
}

#[test]
fn a_mixed_run_from_an_older_generator_is_stale() {
    // Preflop changed at version 4, so a run begun under 3 can't be finished on the same items.
    let rows: Vec<Attempt> = mixed_rows(1, MIXED_FORM_BASE, 5, 0, 1).into_iter().map(|a| Attempt { gen_version: 3, ..a }).collect();
    let runs = checkup_runs(&rows);
    assert!(runs[0].stale);
    assert!(unfinished_run(&runs, CheckupKind::Mixed).is_none());
}

#[test]
fn mixed_and_one_skill_forms_never_collide() {
    let mut log = mixed_rows(1, MIXED_FORM_BASE, 40, 0, 1);
    // A preflop row in the mixed run must not lift preflop's own next form.
    assert_eq!(next_checkup_form(Skill::Preflop, &log), 0);
    assert_eq!(next_form(CheckupKind::Skill(Skill::Preflop), &log), 0);
    // And a one-skill run doesn't lift the mixed forms.
    log.push({
        let mut row = drill_row(Skill::Nuts, 1 << 63, 99, true, None);
        row.mode = Mode::Check;
        row.extra.check = Some(CheckRef { run: 2, form: 3, slot: 0 });
        row
    });
    assert_eq!(next_mixed_form(&log), MIXED_FORM_BASE + 1);
    let runs = checkup_runs(&log);
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[1].kind, CheckupKind::Skill(Skill::Nuts));
}

/// `count` drill rows spread from `first` to `last`.
fn practice(count: usize, first: u64, last: u64) -> Vec<Attempt> {
    (0..count)
        .map(|i| {
            let t = first + (last - first) * i as u64 / (count.max(2) - 1) as u64;
            drill_row(Skill::Nuts, i as u64, t, true, None)
        })
        .collect()
}

#[test]
fn the_first_checkup_waits_for_some_practice() {
    let day = DAY_MS;
    let start = 1_000 * day;
    assert_eq!(mixed_checkup_offer(&[], start), None, "nothing on day one with no history");
    let enough_days = start + FIRST_CHECKUP_MIN_DAYS * day;
    // Plenty of practice but all this week: not yet.
    let busy_week = practice(500, start, start + 2 * day);
    assert_eq!(mixed_checkup_offer(&busy_week, enough_days - 1), None);
    assert_eq!(mixed_checkup_offer(&busy_week, enough_days), Some(CheckupOffer::Take));
    // A week or more but too few answers: not yet.
    let light = practice(FIRST_CHECKUP_MIN_DRILLS - 1, start, start + day);
    assert_eq!(mixed_checkup_offer(&light, enough_days + 30 * day), None);
    let just = practice(FIRST_CHECKUP_MIN_DRILLS, start, start + day);
    assert_eq!(mixed_checkup_offer(&just, enough_days), Some(CheckupOffer::Take));
    // Retries and checkup answers aren't practice answers.
    let retries: Vec<Attempt> = just.iter().cloned().map(|a| Attempt { mode: Mode::DrillRetry, ..a }).collect();
    assert_eq!(mixed_checkup_offer(&retries, enough_days), None);
}

#[test]
fn a_mixed_checkup_is_offered_at_most_once_a_month() {
    let day = DAY_MS;
    let mut log = practice(200, day, 10 * day);
    let done_at = 20 * day;
    log.extend(mixed_rows(1, MIXED_FORM_BASE, 40, 3, done_at - 39));
    let every = MIXED_CHECKUP_EVERY_DAYS * day;
    assert_eq!(mixed_checkup_offer(&log, done_at), None, "just taken");
    assert_eq!(mixed_checkup_offer(&log, done_at + every - 1), None, "29 days later");
    assert_eq!(mixed_checkup_offer(&log, done_at + every), Some(CheckupOffer::Take), "30 days later");
    // An unfinished one is offered to resume whenever it is.
    log.extend(mixed_rows(2, MIXED_FORM_BASE + 1, 7, 0, done_at + every + 5));
    assert_eq!(mixed_checkup_offer(&log, done_at + every + 6), Some(CheckupOffer::Resume { run: 2, answered: 7 }));
    assert_eq!(mixed_checkup_offer(&log, done_at + 200 * day), Some(CheckupOffer::Resume { run: 2, answered: 7 }));
    // Finishing it starts the month again from its last answer.
    log.extend(mixed_rows(2, MIXED_FORM_BASE + 1, 40, 0, done_at + every + 5).into_iter().skip(7));
    let finished = done_at + every + 5 + 39;
    assert_eq!(mixed_checkup_offer(&log, finished + every - 1), None);
    assert_eq!(mixed_checkup_offer(&log, finished + every), Some(CheckupOffer::Take));
}

#[test]
fn old_one_skill_checkups_neither_count_nor_block_the_monthly_offer() {
    let day = DAY_MS;
    let mut log = practice(200, day, 10 * day);
    // A complete and an unfinished one-skill checkup from the live release.
    for slot in 0..50u32 {
        let mut row = drill_row(Skill::Nuts, (1 << 63) | u64::from(slot), 11 * day + u64::from(slot), true, None);
        row.mode = Mode::Check;
        row.gen_version = 3;
        row.extra.check = Some(CheckRef { run: 1, form: 0, slot });
        log.push(row);
    }
    let mut partial = drill_row(Skill::Hit, 1 << 63, 12 * day, true, None);
    partial.mode = Mode::Check;
    partial.extra.check = Some(CheckRef { run: 2, form: 0, slot: 0 });
    log.push(partial);
    assert_eq!(mixed_checkup_offer(&log, 13 * day), Some(CheckupOffer::Take));
}

// ----- old histories -----------------------------------------------------------

/// Rows as the live release and earlier ones wrote them: the original eleven
/// columns, checkup columns, table columns and the notes columns, with `price`
/// and `beats` practice, per-skill checkups and a baseline.
fn old_log() -> String {
    let mut text = String::from(poker_trainer::log::LEGACY_LOG_HEADER);
    text.push('\n');
    text.push_str("1000\tbaseline\t1\tnuts\tb01\t1\t1\t0\t\t3000\tqj\n");
    text.push_str("2000\tdrill\t\tprice\t7\t1\t1\t0\t\t2500\t25\n");
    text.push_str("3000\tdrill\t\tbeats\t8\t1\t0\t0\t\t4000\t30\n");
    for i in 0..45u64 {
        text.push_str(&format!("{}\tdrill\t\tprice\t{}\t3\t{}\t0\t\t2500\t25\t\t\t\t\t\t\t\t\t\tp:small\t\tmedium\t\n", 10_000 + i, 100 + i, u8::from(i % 9 != 0)));
        text.push_str(&format!("{}\tdrill\t\tbeats\t{}\t2\t1\t0\t\t3000\t30\t\t\t\t\t\t\t\t\t\tb:pair\t\t\t\n", 20_000 + i, 300 + i));
        text.push_str(&format!("{}\tdrill\t\tnuts\t{}\t3\t1\t0\t\t3000\tAsKs\t\t\t\t\t\t\t\t\t\tflush\t\t\t\n", 30_000 + i, 500 + i));
    }
    for slot in 0..50u32 {
        text.push_str(&format!("{}\tcheck\t\tprice\t9223372036854775815\t3\t1\t0\t\t1000\t25\t1\t0\t{slot}\t\t\t\t\n", 40_000 + u64::from(slot)));
    }
    text.push_str("50000\tcheck\t\tnuts\t9223372036854775816\t3\t1\t0\t\t1000\tAsKs\t2\t0\t0\t\t\t\t\n");
    text.push_str("60000\tdrill\t\ttable\t11\t1003\t1\t0\t0.5000\t9000\tfold\t\t\t\t\t\t\t\t205\trobust\n");
    text.push_str("61000\tdrill\t\tequity\t12\t3\t1\t0\t\t5000\t40\t\t\t\t8\t3.0000\t37.0000\t1\n");
    text
}

#[test]
fn a_live_release_history_loads_and_works_under_the_new_rules() {
    let (log, warnings) = parse_log(&old_log());
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(log.len(), 3 + 45 * 3 + 51 + 2);
    let stats = all_stats(&log);
    let price = stats.iter().find(|s| s.skill == Skill::Price).unwrap();
    assert_eq!(price.total_attempts, 46, "retired skills keep their history");
    assert_eq!(price.checkup_passed, Some(true), "an old checkup still scores");
    assert!(stats.iter().find(|s| s.skill == Skill::Nuts).unwrap().passed);
    let runs = checkup_runs(&log);
    assert_eq!(runs.iter().map(|r| r.kind).collect::<Vec<_>>(), vec![CheckupKind::Skill(Skill::Price), CheckupKind::Skill(Skill::Nuts)]);
    assert!(runs[0].complete && !runs[1].complete);
    assert_eq!(next_mixed_form(&log), MIXED_FORM_BASE);
    assert_eq!(drill_skills(&[]), mixed_skills(), "everything is open, whatever the history");
    // The default drill deals every group and nothing retired, reviews included.
    let mut session = Session::mixed(&log, 10 * DAY_MS, 1);
    let mut groups: Vec<Group> = Vec::new();
    for _ in 0..200 {
        let planned = session.try_next_item().unwrap();
        let group = group_of(planned.skill).expect("only grouped skills are dealt");
        if !groups.contains(&group) {
            groups.push(group);
        }
        session.record(planned, true);
    }
    assert_eq!(groups.len(), 7);
    // The offer works on it too: a resumable old run is not the monthly one.
    assert_eq!(mixed_checkup_offer(&log, 10 * DAY_MS), Some(CheckupOffer::Take));
}
