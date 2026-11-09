//! The `table` skill: the graded spot library, grading, seeds, selection and pass marks.

use std::collections::{HashMap, HashSet, VecDeque};

use poker_trainer::answer::{answer_format, AnswerFormat};
use poker_trainer::checkup::{
    checkup_plan, latest_checkup, CheckupMetric, CHECKUP_LEN, CHECKUP_ORIGINALS, CHECKUP_REPEATS, MIN_REPEAT_GAP,
};
use poker_trainer::generate::generate;
use poker_trainer::grade::{ev_band, grade, EvBand, Outcome, Reveal};
use poker_trainer::item::{is_held_out, Item, Question, Skill, GEN_VERSION, HELD_OUT_BIT};
use poker_trainer::log::{
    format_attempt, parse_log, Attempt, AttemptExtra, CheckRef, ItemRef, Mode, CHECKUP_LOG_HEADER, LOG_HEADER,
};
use poker_trainer::progress::{baseline_runs, pass_mark, skill_stats, step_passed, all_stats, STEPS};
use poker_trainer::rng::Rng;
use poker_trainer::schedule::{due_families, Origin, Planned, Session, DAY_MS};
use poker_trainer::table::{
    self, bb_text, checkup_form_reuses_spots, drill_pool, is_reserved_for_checkups, parse_library, seed_with_index,
    spot_by_id, spot_of_seed, spots, suit_perm_for_seed, TableActionKind, TableGrade, TableQuestion, TableSeat,
    TableStreetName, Verdict, ASSUMPTION_NOTE, CHECKUP_CLOSE, CHECKUP_ROBUST, COUNT_CLOSE, COUNT_DEPENDS, COUNT_ROBUST,
    NO_REPEAT_WINDOW, SPOTS_TEXT, SPOT_COUNT,
};

const DAY: u64 = 86_400_000;

/// A seed that deals exactly the spot with this id.
fn seed_for(id: u16) -> u64 {
    let spot = spot_by_id(id);
    let (pool, bit) = if is_reserved_for_checkups(spot) {
        (table::checkup_pool(), HELD_OUT_BIT)
    } else {
        (drill_pool(), 0)
    };
    let index = pool.iter().position(|s| s.id == id).unwrap();
    seed_with_index(bit | 0xABCD_0000, index)
}

fn item(id: u16) -> Item {
    generate(Skill::Table, seed_for(id))
}

fn question(id: u16) -> TableQuestion {
    match item(id).question {
        Question::Table(q) => q,
        other => panic!("{other:?}"),
    }
}

fn answer(id: u16, code: &str) -> Outcome {
    grade(&item(id), code).unwrap_or_else(|e| panic!("{code}: {}", e.0))
}

fn graded(outcome: &Outcome) -> &TableGrade {
    match &outcome.reveal {
        Reveal::Table(g) => g,
        other => panic!("{other:?}"),
    }
}

// ----- The library file -----

#[test]
fn every_line_parses_and_the_file_matches_its_expected_counts() {
    let all = spots();
    assert_eq!(all.len(), SPOT_COUNT);
    let count = |v: Verdict| all.iter().filter(|s| s.verdict == v).count();
    assert_eq!(count(Verdict::Robust), COUNT_ROBUST);
    assert_eq!(count(Verdict::Close), COUNT_CLOSE);
    assert_eq!(count(Verdict::Depends), COUNT_DEPENDS);
    let by_street = |n: usize| all.iter().filter(|s| s.board.len() == n).count();
    assert_eq!((by_street(3), by_street(4), by_street(5)), (1200, 1050, 750));
    assert!(all.iter().enumerate().all(|(i, s)| usize::from(s.id) == i), "ids are sequential");
    // The file's own header agrees with those counts and the line count.
    let meta = |key: &str| SPOTS_TEXT.lines().find_map(|l| l.strip_prefix(&format!("#meta {key}=")));
    assert_eq!(meta("spots"), Some("3000"));
    assert_eq!(meta("verdicts"), Some("robust=562,close=284,depends=2154"));
    assert_eq!(meta("streets"), Some("flop=1200,turn=1050,river=750"));
    let spot_lines = SPOTS_TEXT.lines().filter(|l| l.starts_with("SPOT|")).count();
    assert_eq!(spot_lines, SPOT_COUNT);
}

#[test]
fn parsing_is_strict_about_malformed_lines() {
    let lines: Vec<&str> = SPOTS_TEXT.lines().collect();
    let first_spot = lines.iter().find(|l| l.starts_with("SPOT|")).unwrap();
    assert!(parse_library(&format!("#SPOTS v2\n{first_spot}\n")).is_ok());
    assert!(parse_library(&format!("#SPOTS v3\n{first_spot}\n")).is_err(), "unknown format version");
    assert!(parse_library(&format!("{first_spot}\n")).is_err(), "no header");
    for broken in [
        first_spot.replace("|depends|", "|maybe|"),
        first_spot.replace("pot=4.00", "pot=4.50"),
        first_spot.replace("call:4.208", "call:4.2"),
        first_spot.replace("7sTc", "7s7s"),
        first_spot.replace("n=4|BB|", "n=4|XX|"),
        format!("{first_spot}|extra"),
        first_spot.replace("SB:c", "SB:c:z"),
        first_spot.replace("best=call", "best=shove"),
        first_spot.replace("meta=", "metaz="),
    ] {
        assert_ne!(&broken, first_spot, "the test edit must change the line");
        assert!(parse_library(&format!("#SPOTS v2\n{broken}\n")).is_err(), "should reject: {broken}");
    }
}

#[test]
fn the_action_history_replays_to_the_printed_pot() {
    let spot = spot_by_id(0);
    assert_eq!((spot.players, spot.seat, spot.stack_bb), (4, TableSeat::BigBlind, 100));
    assert_eq!((spot.pot_cents, spot.call_cents), (400, 200));
    assert_eq!(spot.streets.len(), 2);
    let preflop = &spot.streets[0];
    assert_eq!(preflop.street, TableStreetName::Preflop);
    assert_eq!(preflop.actions.len(), 4);
    assert_eq!(preflop.actions[2].kind, TableActionKind::Call, "the small blind completes");
    assert_eq!(preflop.actions[2].amount_cents, 50);
    assert_eq!(preflop.pot_start_cents, 150, "blinds");
    let flop = &spot.streets[1];
    assert_eq!(flop.street, TableStreetName::Flop);
    assert_eq!(flop.pot_start_cents, 200, "after preflop");
    assert_eq!(flop.actions[0].kind, TableActionKind::Bet);
    assert_eq!(flop.actions[0].amount_cents, 200);
}

#[test]
fn big_blind_can_be_hero_and_seats_read_plainly() {
    let spot = spot_by_id(0);
    assert_eq!(spot.seat.sentence(), "in the big blind");
    assert_eq!(question(0).behind_cents, 9900, "1bb posted, 99bb still behind");
    let deep = spots().iter().find(|s| s.stack_bb == 60 && s.board.len() == 5).unwrap();
    assert!(deep.behind_cents < 6000, "behind reflects chips already put in, spot {}", deep.id);
    assert_eq!(TableSeat::Button.title(), "Button");
    assert_eq!(TableSeat::Utg.sentence(), "under the gun");
}

#[test]
fn option_labels_use_the_real_amounts() {
    let zero = question(0);
    let label = |q: &TableQuestion, code: &str| q.options.iter().find(|o| o.code == code).unwrap().label;
    assert_eq!(label(&zero, "fold"), "Fold");
    assert_eq!(label(&zero, "call"), "Call 2bb");
    assert_eq!(label(&zero, "raise6"), "Raise to 6bb");
    let two = question(2);
    assert_eq!(label(&two, "check"), "Check");
    // A third of a 2.5bb pot is below the engine's 1bb minimum bet.
    assert_eq!(label(&two, "bet33"), "Bet 1bb (minimum)");
    assert_eq!(label(&two, "bet67"), "Bet 2/3 pot (1.7bb)");
    assert_eq!(label(&two, "bet100"), "Bet pot (2.5bb)");
    assert_eq!(table::spot_by_id(2).options[1].plain, "bet 1bb (minimum)", "archetype rows match");
    // The label uses the exact replayed pot, not the file's two-decimal print.
    assert_eq!(label(&question(1204), "bet67"), "Bet 2/3 pot (5.4bb)");
    for spot in spots() {
        for option in spot.options.iter().filter(|o| o.code.starts_with("bet") && o.label.contains("all-in") && !o.label.contains("pot")) {
            assert!(option.label.ends_with("bb (all-in)"), "spot {}: {}", spot.id, option.label);
            assert_eq!(option.plain, option.label.to_lowercase(), "spot {}", spot.id);
        }
        for option in spot.options.iter().filter(|o| o.code.starts_with("bet") && o.label.contains("pot") && o.label.contains("all-in")) {
            assert!(option.label.contains("pot ("), "spot {}: {}", spot.id, option.label);
        }
    }
    assert!(spots().iter().any(|s| s.options.iter().any(|o| o.label.ends_with("(all-in)") && o.code.starts_with("bet"))));
    assert_eq!(bb_text(835), "8.4");
    assert_eq!(bb_text(200), "2");
    assert_eq!(bb_text(84), "0.8");
}

// ----- Grading by verdict, hand worked from the file -----

#[test]
fn robust_spot_205_is_correct_only_for_the_best_option() {
    // fold 0.0, call -24.2, raise6 -49.2 percent of the pot, best fold.
    let fold = answer(205, "fold");
    assert!(fold.correct);
    assert_eq!(fold.ev_loss_pct, Some(0.0));
    assert!(graded(&fold).band.is_none());
    let call = answer(205, "call");
    assert!(!call.correct);
    assert_eq!(call.ev_loss_pct, Some(24.2));
    assert_eq!(graded(&call).band, Some(EvBand::Blunder));
    assert!(!graded(&call).correct);
    let raise = answer(205, "raise6");
    assert!(!raise.correct);
    assert_eq!(raise.ev_loss_pct, Some(49.2));
}

#[test]
fn robust_spot_93_loses_exactly_five_percent_with_bet67() {
    // check 37.8, bet33 47.3 best, bet67 42.3, bet100 37.7.
    let outcome = answer(93, "bet67");
    assert!(!outcome.correct);
    assert_eq!(outcome.ev_loss_pct, Some(5.0));
    assert_eq!(graded(&outcome).band, Some(EvBand::Mistake), "5.0 is on the Mistake side of the band");
    assert!(answer(93, "bet33").correct);
}

#[test]
fn close_spot_5_is_correct_when_the_loss_is_under_one_percent() {
    // check 65.8, bet33 70.0 best, bet67 69.8, bet100 68.6.
    let near = answer(5, "bet67");
    assert!(near.correct);
    assert_eq!(near.ev_loss_pct, Some(0.24), "under 1% the loss comes from the EVs in bb, 0.006 over 2.5");
    assert_eq!(graded(&near).loss_text(), "0.24% of the pot (0.01bb)", "two decimals under 1");
    assert_eq!(graded(&near).band, Some(EvBand::Close));
    let bet100 = answer(5, "bet100");
    assert!(!bet100.correct, "1.4 over one percent");
    assert_eq!(bet100.ev_loss_pct, Some(1.4));
    assert_eq!(graded(&bet100).band, Some(EvBand::Small));
    let check = answer(5, "check");
    assert_eq!(check.ev_loss_pct, Some(4.2));
    assert!(!check.correct);
}

#[test]
fn a_depends_read_is_correct_with_enough_share_even_when_it_loses_a_lot() {
    // Spot 0: fold 0.0 share .164, call 105.2 best share .602, raise6 86.5 share .234.
    let fold = answer(0, "fold");
    assert!(fold.correct, "a real option at 16.4% of tables");
    assert_eq!(fold.ev_loss_pct, Some(105.2));
    assert!(graded(&fold).band.is_none(), "a fair read is not called a blunder");
    assert!(answer(0, "raise6").correct);
    assert!(answer(0, "call").correct);
    // Spot 2: check 25.7, bet33 22.0 share .270, bet67 12.7 share .023, bet100 3.3 share .008.
    let bet67 = answer(2, "bet67");
    assert!(!bet67.correct);
    assert_eq!(bet67.ev_loss_pct, Some(13.0));
    assert_eq!(graded(&bet67).band, Some(EvBand::Mistake));
    let bet100 = answer(2, "bet100");
    assert!(!bet100.correct);
    assert_eq!(graded(&bet100).band, Some(EvBand::Blunder));
    assert!(answer(2, "bet33").correct, "bet33 has 27% of tables");
}

#[test]
fn the_correct_rule_reads_its_thresholds_exactly() {
    assert!(table::is_correct(Verdict::Robust, true, false, 0), "the best option is always correct");
    assert!(table::is_correct(Verdict::Robust, false, true, 0));
    assert!(!table::is_correct(Verdict::Robust, false, false, 0), "not inside 1% is wrong");
    assert!(table::is_correct(Verdict::Close, false, true, 0));
    assert!(!table::is_correct(Verdict::Close, false, false, 1000), "share is ignored on a close spot");
    assert!(table::is_correct(Verdict::Depends, false, false, 150), "15.0% of tables is enough");
    assert!(!table::is_correct(Verdict::Depends, false, false, 149));
    assert!(table::is_correct(Verdict::Depends, false, true, 0), "inside 1% is correct on loss alone");
    assert!(!table::is_correct(Verdict::Depends, false, false, 149));
    // 1% of a 20.00bb pot is 200 milli-bb: exactly 1% is not inside.
    assert!(table::inside_one_percent(199, 2000));
    assert!(!table::inside_one_percent(200, 2000));
}

/// Spots where the printed percentages differ by exactly 1.0 but the EVs in bb say otherwise.
const CLOSE_ROUNDED_UP: [(u16, &str); 5] = [(576, "bet100"), (795, "bet33"), (1947, "check"), (2068, "bet67"), (2186, "check")];
const CLOSE_EXACTLY_ONE: [(u16, &str); 3] = [(105, "bet100"), (1220, "bet100"), (2573, "bet100")];

#[test]
fn the_one_percent_rule_uses_the_evs_in_bb_not_the_rounded_percentages() {
    for (id, code) in CLOSE_ROUNDED_UP {
        let outcome = answer(id, code);
        assert!(outcome.correct, "spot {id} {code}: printed 1.0, truly under 1%");
        let g = graded(&outcome);
        assert_eq!(g.band, Some(EvBand::Close), "spot {id}");
        assert!(g.loss_pct < 1.0, "spot {id}: {}", g.loss_pct);
        assert!(!g.loss_text().contains("1.0%"), "a correct answer must not read 1.0%: {}", g.loss_text());
    }
    // 2186 check: the printed 70.0 and 69.0 differ by exactly 1.0, while the EVs say 0.987%.
    assert!((graded(&answer(2186, "check")).loss_pct - 0.9867).abs() < 0.001);
    for (id, code) in CLOSE_EXACTLY_ONE {
        let outcome = answer(id, code);
        assert!(!outcome.correct, "spot {id} {code}: exactly 1.000% is not under 1%");
        assert_eq!(outcome.ev_loss_pct, Some(1.0));
        assert_eq!(graded(&outcome).loss_text().split('%').next(), Some("1.0"));
    }
}

#[test]
fn the_ev_bands_keep_their_boundaries() {
    assert_eq!(ev_band(1.0), EvBand::Small, "exactly 1 is small, not close");
    assert_eq!(ev_band(5.0), EvBand::Mistake);
    assert_eq!(ev_band(15.0), EvBand::Mistake);
    assert_eq!(ev_band(15.1), EvBand::Blunder);
}

#[test]
fn an_unknown_option_is_rejected_as_invalid() {
    let error = grade(&item(0), "shove").unwrap_err();
    assert!(error.0.contains("fold") && error.0.contains("raise6"), "{}", error.0);
    assert!(grade(&item(0), " RAISE6 ").unwrap().correct, "answers are case-insensitive and trimmed");
}

#[test]
fn every_option_grades_consistently_with_the_file() {
    for spot in spots() {
        let best = &spot.options[spot.best];
        for (i, option) in spot.options.iter().enumerate() {
            let g = table::grade_option(spot, i);
            let printed = f64::from(best.ev_pct_tenths - option.ev_pct_tenths) / 10.0;
            let exact = f64::from(best.ev_milli_bb - option.ev_milli_bb) * 10.0 / f64::from(spot.pot_cents);
            if table::inside_one_percent(best.ev_milli_bb - option.ev_milli_bb, spot.pot_cents) {
                assert!((g.loss_pct - exact).abs() < 1e-9, "spot {}", spot.id);
            } else {
                assert_eq!(g.loss_pct, printed, "spot {}", spot.id);
            }
            assert!(g.loss_pct >= 0.0);
            if i == spot.best {
                assert!(g.correct, "the best option is always correct, spot {}", spot.id);
            }
        }
    }
}

// ----- Reveal facts -----

#[test]
fn the_reveal_carries_every_fact_the_front_ends_show() {
    let depends = answer(0, "fold");
    let g = graded(&depends);
    assert_eq!(g.spot.verdict, Verdict::Depends);
    assert_eq!(g.spot.verdict.explanation(), "No option was best at nearly all simulated tables, so the right play can change with the opponent.");
    assert_eq!(g.read_note().as_deref(), Some("With no read on the opponent, this was a fair choice."));
    let wrong_read = table::grade_option(spot_by_id(2), 3);
    assert_eq!(wrong_read.read_note().as_deref(), Some("It was best at only 0.8% of simulated tables, so it is rarely a read worth making."));
    assert_eq!(table::ev_bb_text(-3), "+0.00", "never -0.00");
    assert_eq!(table::ev_bb_text(-5), "-0.01");
    assert_eq!(table::ev_bb_text(4208), "+4.21");
    assert_eq!(g.best().code, "call");
    assert_eq!(g.chosen_option().code, "fold");
    assert_eq!(g.spot.options.len(), 3);
    let share = g.spot.options[0].best_share_milli;
    assert_eq!(share, 164);
    assert_eq!(g.spot.archetypes.len(), 5);
    let line = g.spot.archetypes[0].sentence(g.spot);
    assert_eq!(line, "Against a maniac: call (about 60% of the simulated opponents here).", "main opponent breakdown, in plain words");
    assert!(g.spot.archetypes[4].sentence(g.spot).starts_with("Against a nit:"));
    assert!(ASSUMPTION_NOTE.contains("simulated") && ASSUMPTION_NOTE.contains("not real players"));
    assert!(Verdict::Close.explanation().contains("1%"));
    assert!(Verdict::Robust.explanation().contains("90%") && !Verdict::Robust.explanation().contains("weight"));
}

// ----- Seeds and the suit permutation -----

#[test]
fn a_seed_always_regenerates_the_same_question_and_varies_with_the_seed() {
    let mut rng = Rng::new(99);
    let mut distinct = HashSet::new();
    for _ in 0..500 {
        let seed = rng.next_u64() & !HELD_OUT_BIT;
        assert_eq!(generate(Skill::Table, seed), generate(Skill::Table, seed));
        let q = match generate(Skill::Table, seed).question {
            Question::Table(q) => q,
            other => panic!("{other:?}"),
        };
        distinct.insert(q.spot_id);
    }
    assert!(distinct.len() > 400, "seeds should spread over the pool: {}", distinct.len());
}

fn permutations() -> Vec<[u8; 4]> {
    let mut all = Vec::new();
    for a in 0..4u8 {
        for b in 0..4u8 {
            for c in 0..4u8 {
                for d in 0..4u8 {
                    let perm = [a, b, c, d];
                    if a != b && a != c && a != d && b != c && b != d && c != d {
                        all.push(perm);
                    }
                }
            }
        }
    }
    all
}

#[test]
fn a_suit_permutation_never_changes_any_answer() {
    assert_eq!(permutations().len(), 24);
    let mut checked = 0;
    for spot in spots() {
        let base = Item {
            skill: Skill::Table,
            seed: 0,
            question: Question::Table(TableQuestion {
                spot_id: spot.id,
                players: spot.players,
                seat: spot.seat,
                stack_bb: spot.stack_bb,
                hero: spot.hero,
                board: spot.board.clone(),
                streets: spot.streets.clone(),
                pot_cents: spot.pot_cents,
                call_cents: spot.call_cents,
                behind_cents: spot.behind_cents,
                options: match &generate(Skill::Table, seed_for(spot.id)).question {
                    Question::Table(q) => q.options.clone(),
                    _ => unreachable!(),
                },
            }),
        };
        let codes: Vec<&str> = spot.options.iter().map(|o| o.code.as_str()).collect();
        let reference: Vec<(bool, f64, Option<EvBand>)> = codes
            .iter()
            .map(|code| {
                let o = grade(&base, code).unwrap();
                (o.correct, o.ev_loss_pct.unwrap(), graded(&o).band)
            })
            .collect();
        for perm in permutations().into_iter().step_by(7) {
            let permuted = Item { question: base.question.with_suit_perm(&perm), ..base.clone() };
            assert_eq!(
                answer_format(&permuted.question),
                answer_format(&base.question),
                "the options are the same choices at spot {}", spot.id
            );
            let got: Vec<(bool, f64, Option<EvBand>)> = codes
                .iter()
                .map(|code| {
                    let o = grade(&permuted, code).unwrap();
                    (o.correct, o.ev_loss_pct.unwrap(), graded(&o).band)
                })
                .collect();
            assert_eq!(got, reference, "spot {} under {perm:?}", spot.id);
            checked += 1;
        }
    }
    assert!(checked >= 3000, "{checked}");
}

#[test]
fn the_permutation_relabels_hero_and_board_and_is_really_applied() {
    let mut changed = 0;
    let mut rng = Rng::new(5);
    for _ in 0..500 {
        let seed = rng.next_u64() & !HELD_OUT_BIT;
        let q = match generate(Skill::Table, seed).question {
            Question::Table(q) => q,
            other => panic!("{other:?}"),
        };
        let spot = spot_of_seed(seed);
        assert_eq!(q.spot_id, spot.id);
        let perm = suit_perm_for_seed(seed);
        let expected_hero = [spot.hero[0].with_suit_perm(&perm), spot.hero[1].with_suit_perm(&perm)];
        assert_eq!(q.hero, expected_hero, "the seed's permutation is applied to the hand");
        let expected_board: Vec<_> = spot.board.iter().map(|c| c.with_suit_perm(&perm)).collect();
        assert_eq!(q.board, expected_board, "and to the board");
        // Ranks never change, and no cards collide.
        let ranks = |cards: &[poker_trainer::cards::Card]| cards.iter().map(|c| c.rank()).collect::<Vec<_>>();
        assert_eq!(ranks(&q.board), ranks(&spot.board));
        assert_eq!(ranks(&q.hero), ranks(&spot.hero));
        if q.hero != spot.hero || q.board != spot.board {
            changed += 1;
        }
    }
    assert!(changed > 400, "most deals should show different suits: {changed}");
}

#[test]
fn board_cards_split_by_street() {
    let q = question(0);
    assert_eq!(q.street_cards(TableStreetName::Preflop).len(), 0);
    assert_eq!(q.street_cards(TableStreetName::Flop).len(), 3);
    assert_eq!(q.street_cards(TableStreetName::Turn).len(), 0);
    assert_eq!(q.street_cards(TableStreetName::River).len(), 0);
    let river = spots().iter().find(|s| s.board.len() == 5 && !is_reserved_for_checkups(s)).unwrap();
    let q = question(river.id);
    assert_eq!(q.street_cards(TableStreetName::Turn).len(), 1);
    assert_eq!(q.street_cards(TableStreetName::River).len(), 1);
}

// ----- Held-out separation -----

#[test]
fn reserved_spots_are_robust_or_close_with_id_divisible_by_three() {
    let reserved = table::checkup_pool();
    assert_eq!(reserved.len(), 172 + 96);
    for spot in reserved {
        assert!(spot.id % 3 == 0 && spot.verdict != Verdict::Depends, "{}", spot.id);
    }
    for spot in drill_pool() {
        assert!(!is_reserved_for_checkups(spot), "{} leaked into drills", spot.id);
        assert!(spot.id % 3 != 0 || spot.verdict == Verdict::Depends, "{}", spot.id);
    }
    assert_eq!(reserved.len() + drill_pool().len(), SPOT_COUNT, "together they cover the library");
    // Depends spots with id divisible by three are still drillable.
    assert!(drill_pool().iter().any(|s| s.verdict == Verdict::Depends && s.id % 3 == 0));
}

#[test]
fn no_plain_seed_can_reach_a_reserved_spot_and_held_out_seeds_only_reach_them() {
    let mut rng = Rng::new(7);
    for _ in 0..20_000 {
        let seed = rng.next_u64();
        let spot = spot_of_seed(seed);
        if is_held_out(seed) {
            assert!(is_reserved_for_checkups(spot), "seed {seed}");
        } else {
            assert!(!is_reserved_for_checkups(spot), "seed {seed}");
        }
    }
}

#[test]
fn drills_never_deal_a_reserved_spot() {
    for seed in 0..12 {
        let mut session = Session::new(vec![Skill::Table], &[], DAY, seed);
        for _ in 0..1500 {
            let planned = session.try_next_item().unwrap();
            assert_eq!(planned.origin, Origin::New);
            assert!(!is_held_out(planned.seed));
            assert!(!is_reserved_for_checkups(spot_of_seed(planned.seed)), "{planned:?}");
        }
    }
}

#[test]
fn checkups_use_only_reserved_robust_or_close_spots_with_a_fixed_mix() {
    for form in 0..12 {
        let plan = checkup_plan(Skill::Table, form);
        assert_eq!(plan, checkup_plan(Skill::Table, form), "deterministic");
        assert_eq!(plan.slots.len(), CHECKUP_LEN);
        let mut ids = HashSet::new();
        let (mut robust, mut close) = (0, 0);
        for (index, slot) in plan.slots.iter().enumerate() {
            assert!(is_held_out(slot.seed));
            let spot = spot_of_seed(slot.seed);
            assert!(is_reserved_for_checkups(spot), "form {form} slot {index}");
            assert_ne!(spot.verdict, Verdict::Depends, "depends spots are never in checkups");
            if slot.repeat_of.is_none() {
                ids.insert(spot.id);
                match spot.verdict {
                    Verdict::Robust => robust += 1,
                    _ => close += 1,
                }
            }
        }
        assert_eq!(ids.len(), CHECKUP_ORIGINALS, "originals are distinct spots, form {form}");
        assert_eq!((robust, close), (CHECKUP_ROBUST, CHECKUP_CLOSE), "form {form}");
    }
}

#[test]
fn a_checkup_is_forty_originals_and_ten_suit_permuted_repeats() {
    let plan = checkup_plan(Skill::Table, 2);
    let repeats: Vec<(usize, usize)> =
        plan.slots.iter().enumerate().filter_map(|(i, s)| s.repeat_of.map(|o| (i, o))).collect();
    assert_eq!(repeats.len(), CHECKUP_REPEATS);
    let mut visible = 0;
    for (at, original) in repeats {
        assert!(at - original >= MIN_REPEAT_GAP);
        let (first, second) = (plan.item(original), plan.item(at));
        let (a, b) = match (&first.question, &second.question) {
            (Question::Table(a), Question::Table(b)) => (a, b),
            other => panic!("{other:?}"),
        };
        assert_eq!(a.spot_id, b.spot_id, "a repeat is the same spot");
        assert_eq!(a.options, b.options);
        assert_eq!(a.hero[0].rank(), b.hero[0].rank());
        if a.hero != b.hero || a.board != b.board {
            visible += 1;
        }
    }
    assert!(visible >= 5, "repeats are suit-permuted, not identical: {visible} of {CHECKUP_REPEATS}");
}

#[test]
fn checkup_forms_are_disjoint_until_the_reserved_spots_run_out_then_say_so() {
    let mut used: HashMap<u16, u32> = HashMap::new();
    for form in 0..6u32 {
        assert!(!checkup_form_reuses_spots(form), "form {form}");
        for slot in checkup_plan(Skill::Table, form).slots.iter().filter(|s| s.repeat_of.is_none()) {
            let id = spot_of_seed(slot.seed).id;
            assert_eq!(used.insert(id, form), None, "spot {id} reused in form {form}");
        }
    }
    assert!(checkup_form_reuses_spots(6));
    let shared = checkup_plan(Skill::Table, 6)
        .slots
        .iter()
        .filter(|s| s.repeat_of.is_none() && used.contains_key(&spot_of_seed(s.seed).id))
        .count();
    assert!(shared > 0, "form 6 reuses spots");
}

// ----- Drill selection -----

#[test]
fn the_drill_mix_is_about_45_robust_40_depends_15_close() {
    let mut session = Session::new(vec![Skill::Table], &[], DAY, 11);
    let mut counts: HashMap<Verdict, usize> = HashMap::new();
    let total = 20_000;
    for _ in 0..total {
        let planned = session.try_next_item().unwrap();
        *counts.entry(spot_of_seed(planned.seed).verdict).or_insert(0) += 1;
    }
    let share = |v: Verdict| *counts.get(&v).unwrap_or(&0) as f64 / total as f64 * 100.0;
    assert!((share(Verdict::Robust) - 45.0).abs() < 2.0, "{}", share(Verdict::Robust));
    assert!((share(Verdict::Depends) - 40.0).abs() < 2.0, "{}", share(Verdict::Depends));
    assert!((share(Verdict::Close) - 15.0).abs() < 2.0, "{}", share(Verdict::Close));
}

#[test]
fn a_spot_is_not_repeated_inside_the_window_within_a_session() {
    let mut session = Session::new(vec![Skill::Table], &[], DAY, 3);
    let mut order = Vec::new();
    for _ in 0..3000 {
        order.push(spot_of_seed(session.try_next_item().unwrap().seed).id);
    }
    for window in order.windows(NO_REPEAT_WINDOW) {
        let distinct: HashSet<u16> = window.iter().copied().collect();
        assert_eq!(distinct.len(), window.len(), "a spot repeated within {NO_REPEAT_WINDOW} attempts");
    }
}

fn table_attempt(id: u16, t: u64, loss: f64, verdict: Verdict) -> Attempt {
    Attempt {
        timestamp_ms: t,
        mode: Mode::Drill,
        baseline_run: None,
        skill: Skill::Table,
        item: ItemRef::Seed(seed_for(id)),
        gen_version: 1,
        correct: loss < 1.0,
        close: false,
        ev_loss_pct: Some(loss),
        response_ms: 2000,
        answer: "check".to_string(),
        extra: AttemptExtra { table_spot: Some(id), table_verdict: Some(verdict), ..AttemptExtra::default() },
    }
}

fn log_of(ids: impl Iterator<Item = u16>) -> Vec<Attempt> {
    ids.enumerate().map(|(i, id)| table_attempt(id, i as u64, 0.0, spot_by_id(id).verdict)).collect()
}

#[test]
fn recent_attempts_in_the_log_are_not_dealt_again_while_unseen_spots_remain() {
    let close_ids: Vec<u16> = drill_pool().iter().filter(|s| s.verdict == Verdict::Close).map(|s| s.id).collect();
    assert!(close_ids.len() < NO_REPEAT_WINDOW, "the test needs the close pool smaller than the window");
    let log = log_of(close_ids.iter().copied());
    let seen: HashSet<u16> = close_ids.iter().copied().collect();
    // Every close spot was seen recently, so the 15% close rolls must fall through to unseen spots.
    // Only 12 more deals fit before the oldest log rows leave the window.
    for seed in 0..80 {
        let mut session = Session::new(vec![Skill::Table], &log, DAY, seed);
        for _ in 0..NO_REPEAT_WINDOW - close_ids.len() {
            let dealt = spot_of_seed(session.try_next_item().unwrap().seed).id;
            assert!(!seen.contains(&dealt), "spot {dealt} repeats inside the window");
        }
    }
    // Attempts older than the window no longer block a spot.
    let old_id = close_ids[0];
    let mut padded = vec![table_attempt(old_id, 0, 0.0, Verdict::Close)];
    let filler: Vec<u16> = drill_pool().iter().map(|s| s.id).filter(|&id| id != old_id).take(NO_REPEAT_WINDOW).collect();
    padded.extend(filler.iter().enumerate().map(|(i, &id)| table_attempt(id, i as u64 + 1, 0.0, spot_by_id(id).verdict)));
    let mut any_old = false;
    for seed in 0..80 {
        let mut session = Session::new(vec![Skill::Table], &padded, DAY, seed);
        for _ in 0..400 {
            any_old |= spot_of_seed(session.try_next_item().unwrap().seed).id == old_id;
        }
    }
    assert!(any_old, "a spot older than the window can be dealt again");
}

#[test]
fn table_spots_get_no_same_session_retry_and_no_later_review() {
    let mut session = Session::new(vec![Skill::Table], &[], DAY, 19);
    for _ in 0..300 {
        let planned = session.try_next_item().unwrap();
        assert_eq!(planned.origin, Origin::New, "a missed spot must not repeat inside the window");
        session.record(planned, false);
    }
    let mut miss = Attempt { correct: false, ..table_attempt(0, 1000, 20.0, Verdict::Robust) };
    miss.gen_version = GEN_VERSION;
    miss.extra.family = Some("flop:hu".to_string());
    assert!(due_families(&[miss], DAY_MS * 5).is_empty());
}

#[test]
fn dealing_is_deterministic_from_the_session_seed() {
    let mut first = Session::new(vec![Skill::Table], &[], DAY, 42);
    let mut second = Session::new(vec![Skill::Table], &[], DAY, 42);
    for _ in 0..300 {
        let a: Planned = first.try_next_item().unwrap();
        let b = second.try_next_item().unwrap();
        assert_eq!(a, b);
        first.record(a, true);
        second.record(b, true);
    }
}

#[test]
fn a_seed_encodes_the_spot_and_keeps_its_other_bits() {
    let drill = seed_with_index(0x1234_5000, 77);
    assert_eq!(drill & 0xFFF, 77);
    assert!(!is_held_out(drill));
    let mut recent = VecDeque::new();
    let (seed, id) = table::deal_spot(&mut Rng::new(1), &recent);
    assert_eq!(spot_of_seed(seed).id, id);
    recent.push_back(id);
    let (second, id2) = table::deal_spot(&mut Rng::new(1), &recent);
    assert_ne!((seed, id), (second, id2), "the second deal skips the recent spot");
}

// ----- Pass marks -----

fn mark_row(id: u16, t: u64, loss: f64, verdict: Verdict) -> Attempt {
    table_attempt(id, t, loss, verdict)
}

/// Forty graded attempts on robust or close spots, each with this loss.
fn forty(loss: f64) -> Vec<Attempt> {
    let robust: Vec<u16> = drill_pool().iter().filter(|s| s.verdict == Verdict::Robust).map(|s| s.id).collect();
    (0..40u64).map(|i| mark_row(robust[i as usize], i, loss, Verdict::Robust)).collect()
}

#[test]
fn the_pass_mark_constants_are_the_starting_guesses_from_the_brief() {
    let mark = pass_mark(Skill::Table);
    assert_eq!(mark.max_mean_ev_loss_pct, Some(3.0));
    assert_eq!(mark.max_blunders, Some(2));
}

#[test]
fn table_drill_mark_needs_forty_graded_robust_or_close_attempts_at_three_percent() {
    assert!(skill_stats(Skill::Table, &forty(3.0)).drill_passed, "3.0% passes");
    assert!(!skill_stats(Skill::Table, &forty(3.1)).drill_passed, "3.1% fails");
    let short = &forty(0.5)[..39];
    let stats = skill_stats(Skill::Table, short);
    assert!(!stats.drill_passed && stats.total_attempts == 39);
}

#[test]
fn more_than_two_blunders_fail_the_drill_mark() {
    let mut log = forty(0.5);
    for row in log.iter_mut().take(3) {
        row.ev_loss_pct = Some(15.5);
    }
    assert!(!skill_stats(Skill::Table, &log).drill_passed);
    let mut log = forty(0.5);
    for row in log.iter_mut().take(2) {
        row.ev_loss_pct = Some(15.5);
    }
    assert!(skill_stats(Skill::Table, &log).drill_passed, "two blunders are allowed");
}

#[test]
fn depends_attempts_do_not_count_toward_the_table_mark() {
    let mut log = forty(0.5);
    // Ten terrible depends attempts in between change nothing.
    for i in 0..10 {
        log.insert(10 + i * 2, mark_row(1, 1000 + i as u64, 90.0, Verdict::Depends));
    }
    let stats = skill_stats(Skill::Table, &log);
    assert!(stats.drill_passed);
    assert_eq!(stats.total_attempts, 40, "only graded robust or close spots count");
    assert_eq!(stats.blunders, 0);
    assert!((stats.mean_ev_loss_pct - 0.5).abs() < 1e-9);
    // And a depends-only log has nothing to judge.
    let depends_only: Vec<Attempt> = (0..60).map(|i| mark_row(1, i, 0.0, Verdict::Depends)).collect();
    assert_eq!(skill_stats(Skill::Table, &depends_only).total_attempts, 0);
    assert!(!skill_stats(Skill::Table, &depends_only).drill_passed);
}

fn table_checkup(run: u32, form: u32, loss: f64, count: usize) -> Vec<Attempt> {
    checkup_plan(Skill::Table, form)
        .slots
        .iter()
        .enumerate()
        .take(count)
        .map(|(slot, s)| {
            let id = spot_of_seed(s.seed).id;
            let mut row = table_attempt(id, 10_000 + u64::from(run) * 100 + slot as u64, loss, spot_by_id(id).verdict);
            row.mode = Mode::Check;
            row.item = ItemRef::Seed(s.seed);
            row.extra.check = Some(CheckRef { run, form, slot: slot as u32 });
            row
        })
        .collect()
}

#[test]
fn the_latest_checkup_must_meet_the_same_standard_to_pass() {
    let mut log = forty(0.5);
    log.extend(table_checkup(1, 0, 0.5, CHECKUP_LEN));
    let score = latest_checkup(Skill::Table, &log).unwrap();
    assert_eq!(score.metric, CheckupMetric::MeanEvLoss);
    assert!(score.passed && score.complete);
    assert!(skill_stats(Skill::Table, &log).passed);
    // A failing checkup afterwards blocks the pass even though the drill mark holds.
    log.extend(table_checkup(2, 1, 4.0, CHECKUP_LEN));
    assert!(!skill_stats(Skill::Table, &log).passed);
    assert!(skill_stats(Skill::Table, &log).drill_passed);
    // Passing the checkup but not the drill mark is not a pass.
    let mut drills_bad = forty(5.0);
    drills_bad.extend(table_checkup(3, 2, 0.5, CHECKUP_LEN));
    assert!(!skill_stats(Skill::Table, &drills_bad).passed);
}

#[test]
fn table_counts_as_step_four_and_does_not_disturb_the_baseline_steps() {
    assert_eq!(Skill::Table.step(), Some(4));
    assert_eq!(STEPS.len(), 4);
    assert_eq!(STEPS[3], &[Skill::Table]);
    assert_eq!(Skill::Table as u64, 13, "added last so existing seeds don't shift");
    assert_eq!(Skill::ALL.last(), Some(&Skill::BeatsMe), "new skills go last");
    assert_eq!(Skill::Chain as u64, 14);
    assert!(!step_passed(4, &all_stats(&forty(0.5))));
    let log = vec![baseline_row(1, "b01", Skill::Nuts), baseline_row(1, "b02", Skill::Price), baseline_row(1, "b03", Skill::Spot)];
    let runs = baseline_runs(&log, poker_trainer::baseline::BASELINE_SIZE);
    assert_eq!(runs[0].steps.len(), 3, "the baseline only has three steps");
}

fn baseline_row(run: u32, id: &str, skill: Skill) -> Attempt {
    Attempt {
        timestamp_ms: 1,
        mode: Mode::Baseline,
        baseline_run: Some(run),
        skill,
        item: ItemRef::Baseline(id.to_string()),
        gen_version: 1,
        correct: true,
        close: false,
        ev_loss_pct: None,
        response_ms: 1000,
        answer: "x".to_string(),
        extra: AttemptExtra::default(),
    }
}

// ----- The log -----

#[test]
fn table_rows_round_trip_and_older_rows_still_parse() {
    let row = table_attempt(205, 77, 1.4, Verdict::Robust);
    let line = format_attempt(&row);
    let fields: Vec<&str> = line.split('\t').collect();
    assert_eq!(fields.len(), 20, "{line}");
    assert_eq!(&fields[18..], &["205", "robust"]);
    let (parsed, warnings) = parse_log(&format!("{line}\n"));
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(parsed[0], row);
    // Rows from before these columns existed still read, as does either old header.
    let original = "1000\tdrill\t\tprice\t7\t1\t1\t0\t\t1000\t25";
    let eighteen = "2000\tcheck\t\tprice\t7\t1\t1\t0\t\t1000\t25\t1\t0\t0\t\t\t\t";
    let (parsed, warnings) = parse_log(&format!("{CHECKUP_LOG_HEADER}\n{original}\n{eighteen}\n{LOG_HEADER}\n"));
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[1].extra.table_spot, None);
}

#[test]
fn a_table_verdict_that_is_unrecognised_is_a_malformed_row() {
    let row = table_attempt(205, 77, 1.4, Verdict::Robust);
    let line = format_attempt(&row).replace("\trobust", "\tshaky");
    let (parsed, warnings) = parse_log(&format!("{line}\n"));
    assert!(parsed.is_empty());
    assert_eq!(warnings.len(), 1);
}

#[test]
fn format_attempt_writes_table_columns_only_for_table_rows() {
    let plain = Attempt {
        timestamp_ms: 5,
        mode: Mode::Drill,
        baseline_run: None,
        skill: Skill::Price,
        item: ItemRef::Seed(9),
        gen_version: 1,
        correct: true,
        close: false,
        ev_loss_pct: None,
        response_ms: 900,
        answer: "25".to_string(),
        extra: AttemptExtra::default(),
    };
    assert_eq!(format_attempt(&plain).split('\t').count(), 11);
    let mut check = plain.clone();
    check.extra.check = Some(CheckRef { run: 1, form: 0, slot: 0 });
    assert_eq!(format_attempt(&check).split('\t').count(), 18, "non-table extras keep their old shape");
}

#[test]
fn the_format_is_a_choice_of_the_option_codes() {
    match answer_format(&item(2).question) {
        AnswerFormat::Choice(options) => {
            let codes: Vec<&str> = options.iter().map(|o| o.code).collect();
            assert_eq!(codes, ["check", "bet33", "bet67", "bet100"]);
        }
        other => panic!("{other:?}"),
    }
}
