//! The River drill: the built-in library, its grading, its deal and its log rows.

use std::collections::{HashMap, HashSet};

use poker_trainer::checkup::{checkup_runs, mixed_checkup_offer};
use poker_trainer::generate::generate;
use poker_trainer::grade::{grade, Reveal};
use poker_trainer::item::{Question, Skill};
use poker_trainer::log::{format_attempt, parse_log, Attempt, Mode};
use poker_trainer::progress::{all_stats, baseline_runs};
use poker_trainer::river::{
    self, item_for_seed, parse_library, seed_for, spot_of_seed, spots, RiverKind, RiverPlay, RiverPool, RiverSession,
    RiverVerdict, DEFAULT_RIVER_ITEMS, RIVER_CLEAR, RIVER_EITHER, RIVER_GEN_VERSION, RIVER_HEADS_UP, RIVER_SPOT_COUNT,
    RIVER_TEXT, RIVER_THREE_WAY,
};
use poker_trainer::schedule::{due_families, Origin};

/// Options' EVs are printed to 0.001bb, so a loss computed from two of them can be
/// off by this much against the generator's unrounded margin.
const ROUNDING_MILLI: i32 = 2;

/// The raw `key=value` fields of every RIVER line, by spot id, read here without the parser.
fn raw_fields() -> HashMap<String, HashMap<String, String>> {
    RIVER_TEXT
        .lines()
        .filter(|l| l.starts_with("RIVER|"))
        .map(|l| {
            let f: Vec<&str> = l.split('|').collect();
            let kv = f[2..].iter().map(|x| x.split_once('=').unwrap()).map(|(k, v)| (k.to_string(), v.to_string())).collect();
            (f[1].to_string(), kv)
        })
        .collect()
}

#[test]
fn every_spot_in_the_library_parses_with_the_recorded_counts() {
    let list = spots();
    assert_eq!(list.len(), RIVER_SPOT_COUNT);
    assert_eq!(list.iter().filter(|s| s.players == 2).count(), RIVER_HEADS_UP);
    assert_eq!(list.iter().filter(|s| s.players == 3).count(), RIVER_THREE_WAY);
    assert_eq!(list.iter().filter(|s| s.verdict == RiverVerdict::Clear).count(), RIVER_CLEAR);
    assert_eq!(list.iter().filter(|s| s.verdict == RiverVerdict::Either).count(), RIVER_EITHER);
    // Every line of the file is a spot: nothing was skipped.
    assert_eq!(RIVER_TEXT.lines().filter(|l| l.starts_with("RIVER|")).count(), RIVER_SPOT_COUNT);
    for (i, s) in list.iter().enumerate() {
        assert_eq!(usize::from(s.index), i);
        assert_eq!(s.board.len(), 5, "{}", s.id);
        assert!(s.options.len() >= 2, "{}", s.id);
        assert!(!s.why.is_empty() && s.why.len() <= 3, "{}", s.id);
    }
    // The library's own split of the deal: both pools, every kind, both table sizes.
    let home = list.iter().filter(|s| s.pool == RiverPool::Home).count();
    assert!((55..=65).contains(&(home * 100 / list.len())), "home share {home}");
    for kind in RiverKind::ALL {
        assert!(list.iter().any(|s| s.players == 2 && s.kind == kind), "no heads-up {kind:?}");
    }
    // The 2,113 replies attach to heads-up bets and raises only.
    let replies: usize = list.iter().map(|s| s.replies.len()).sum();
    assert_eq!(replies, 2113);
    assert!(list.iter().filter(|s| s.players == 3).all(|s| s.replies.is_empty()));
}

#[test]
fn every_option_has_an_ev_and_the_best_is_the_recorded_one() {
    let raw = raw_fields();
    for s in spots() {
        let fields = &raw[&s.id];
        let recorded: Vec<&str> = fields["opts"].split(';').collect();
        assert_eq!(recorded.len(), s.options.len(), "{}", s.id);
        for (option, text) in s.options.iter().zip(&recorded) {
            let parts: Vec<&str> = text.split(':').collect();
            assert_eq!(parts[0], option.code, "{}", s.id);
            let ev: f64 = parts[2].parse().unwrap();
            assert_eq!((ev * 1000.0).round() as i32, option.ev_milli_bb, "{} {}", s.id, option.code);
            assert!(!option.label.is_empty());
        }
        assert_eq!(s.best_option().code, fields["best"], "{}", s.id);
        let top = s.options.iter().map(|o| o.ev_milli_bb).max().unwrap();
        assert_eq!(s.best_option().ev_milli_bb, top, "{}: the recorded best is not the highest EV", s.id);
        let accepted: Vec<&str> = s.accept.iter().map(|&i| s.options[i].code).collect();
        assert_eq!(accepted.join(","), fields["accept"], "{}", s.id);
    }
}

#[test]
fn an_either_spot_accepts_every_option_within_its_margin() {
    let mut either = 0;
    for s in spots() {
        let margin = s.margin_milli_bb() as i32;
        for (i, _) in s.options.iter().enumerate() {
            let loss = s.loss_milli_bb(i);
            let accepted = s.accept.contains(&i);
            if accepted {
                assert!(loss <= margin + ROUNDING_MILLI, "{}: an accepted option loses {loss} > margin {margin}", s.id);
            }
            if s.verdict == RiverVerdict::Either && loss <= margin - ROUNDING_MILLI {
                assert!(accepted, "{}: option {} is within the margin but not accepted", s.id, s.options[i].code);
            }
        }
        if s.verdict == RiverVerdict::Either {
            either += 1;
            let plays: HashSet<RiverPlay> = s.accept.iter().map(|&i| s.options[i].play()).collect();
            assert!(plays.len() >= 2, "{}: an either spot with one play", s.id);
            // Graded as the drill grades it: every accepted option is right.
            let item = item_for_seed(seed_for(s.index, 99));
            for &i in &s.accept {
                let out = grade(&item, s.options[i].code).unwrap();
                assert!(out.correct, "{} {}", s.id, s.options[i].code);
                let Reveal::River(g) = &out.reveal else { panic!("not a river reveal") };
                assert_eq!(g.verdict_text(), "Either is fine.");
                assert!(g.loss_text().is_none());
                assert!(g.close_note().unwrap().contains("close"));
            }
        } else {
            let plays: HashSet<RiverPlay> = s.accept.iter().map(|&i| s.options[i].play()).collect();
            assert_eq!(plays.len(), 1, "{}", s.id);
        }
    }
    assert_eq!(either, RIVER_EITHER);
}

#[test]
fn grading_matches_the_accepted_options_and_reports_the_loss() {
    for s in spots() {
        for high in [0u64, 0xDEAD_BEEF_0000] {
            let seed = seed_for(s.index, high);
            assert!(seed >> 63 == 0, "river seeds are never held out");
            let item = generate(Skill::River, seed);
            let Question::River(q) = &item.question else { panic!() };
            assert_eq!(q.spot().id, s.id);
            // The suits are relabelled, the ranks and the rest of the spot are not.
            let ranks = |cards: &[poker_trainer::cards::Card]| cards.iter().map(|c| c.rank()).collect::<Vec<_>>();
            assert_eq!(ranks(&q.board), ranks(&s.board));
            for (i, option) in s.options.iter().enumerate() {
                let out = grade(&item, option.code).unwrap();
                assert_eq!(out.correct, s.accept.contains(&i), "{} {}", s.id, option.code);
                assert_eq!(out.family.as_deref(), Some(format!("river:{}", s.id).as_str()));
                let Reveal::River(g) = &out.reveal else { panic!() };
                if out.correct {
                    assert!(g.loss_text().is_none());
                } else {
                    let text = g.loss_text().unwrap();
                    assert!(text.contains("bb") && text.contains("% of the pot"), "{text}");
                    assert!(out.ev_loss_pct.unwrap() > 0.0, "{} {}", s.id, option.code);
                }
            }
            assert!(grade(&item, "nonsense").is_err());
        }
        // Every spot has words for its question and its feedback.
        assert!(!s.context_line().is_empty() && !s.beat_line().is_empty() && !s.beaten_line().is_empty());
        for o in &s.options {
            if o.play() == RiverPlay::Aggressive && s.players == 2 {
                if let Some(line) = s.reply_line(o.code) {
                    assert!(line.starts_with("If you "), "{line}");
                }
            }
        }
    }
}

#[test]
fn a_bad_library_is_refused() {
    assert!(parse_library("#SPOTS v2\n").is_err());
    let first_spot = RIVER_TEXT.lines().find(|l| l.starts_with("RIVER|")).unwrap();
    let header: String = RIVER_TEXT.lines().filter(|l| l.starts_with('#') && !l.starts_with("#meta")).map(|l| format!("{l}\n")).collect();
    let one = |line: &str, meta: &str| format!("{header}{meta}\n{line}\n");
    let either = first_spot.contains("verdict=either");
    let meta = format!(
        "#meta spots=1 heads_up=1 three_way=0 clear={} either={}",
        usize::from(!either),
        usize::from(either)
    );
    assert!(parse_library(&one(first_spot, &meta)).is_ok());
    // A best that is not the top EV, a missing option in accept, wrong counts.
    assert!(parse_library(&one(&first_spot.replace("best=allin", "best=call"), &meta)).is_err());
    assert!(parse_library(&one(&first_spot.replace("accept=allin", "accept=bet33"), &meta)).is_err());
    assert!(parse_library(&one(first_spot, "#meta spots=2 heads_up=2 three_way=0 clear=2 either=0")).is_err());
}

fn deal_sessions(count: u64, attempts: &[Attempt]) -> Vec<Vec<poker_trainer::schedule::Planned>> {
    (0..count)
        .map(|s| {
            let mut session = RiverSession::new(attempts, s * 7919 + 1, None);
            let mut dealt = Vec::new();
            while let Some(p) = session.try_next_item() {
                dealt.push(p);
                session.record(p, true);
            }
            dealt
        })
        .collect()
}

#[test]
fn the_deal_mixes_pools_table_sizes_and_kinds() {
    let sessions = deal_sessions(100, &[]);
    let all: Vec<_> = sessions.iter().flatten().collect();
    assert!(sessions.iter().all(|s| s.len() == DEFAULT_RIVER_ITEMS));
    let n = all.len();
    let home = all.iter().filter(|p| spot_of_seed(p.seed).pool == RiverPool::Home).count();
    let three = all.iter().filter(|p| spot_of_seed(p.seed).players == 3).count();
    assert!((55..=65).contains(&(home * 100 / n)), "home {home} of {n}");
    assert!((21..=29).contains(&(three * 100 / n)), "three-way {three} of {n}");
    for kind in RiverKind::ALL {
        let k = all.iter().filter(|p| spot_of_seed(p.seed).kind == kind).count();
        assert!(k * 100 >= n * 2, "{kind:?} dealt only {k} of {n}");
    }
    // A session never repeats a spot unless it was missed.
    for s in &sessions {
        let ids: HashSet<u16> = s.iter().map(|p| spot_of_seed(p.seed).index).collect();
        assert_eq!(ids.len(), s.len());
        assert!(s.iter().all(|p| p.skill == Skill::River && p.origin == Origin::New));
    }
    // Most of a session's kinds differ: at least six of the nine in twenty questions.
    let kinds_per_session: Vec<usize> =
        sessions.iter().map(|s| s.iter().map(|p| spot_of_seed(p.seed).kind).collect::<HashSet<_>>().len()).collect();
    let mean = kinds_per_session.iter().sum::<usize>() as f64 / kinds_per_session.len() as f64;
    assert!(mean >= 6.0, "kinds per session {mean}");
}

#[test]
fn a_miss_comes_back_in_the_session_and_in_the_next_one() {
    // In the session: three to five questions later, with new suits.
    let mut session = RiverSession::new(&[], 5, None);
    let first = session.try_next_item().unwrap();
    session.record(first, false);
    let mut later = Vec::new();
    while let Some(p) = session.try_next_item() {
        later.push(p);
        session.record(p, true);
    }
    let back = later.iter().position(|p| p.origin == Origin::SessionRetry).expect("the miss came back");
    assert!((2..=4).contains(&back), "came back {} questions later", back + 1);
    assert_eq!(spot_of_seed(later[back].seed).index, spot_of_seed(first.seed).index);
    assert_ne!(later[back].seed, first.seed, "the same suits again");

    // In the next session, as a review, and not again once answered right.
    let spot = spots().iter().find(|s| s.kind == RiverKind::BluffCatch && s.accept.len() < s.options.len()).unwrap();
    let first = poker_trainer::schedule::Planned { skill: Skill::River, seed: seed_for(spot.index, 77), origin: Origin::New };
    let item = item_for_seed(first.seed);
    let wrong = spot.options.iter().enumerate().find(|(i, _)| !spot.accept.contains(i)).unwrap().1.code;
    let missed = grade(&item, wrong).unwrap();
    assert!(!missed.correct);
    let log = vec![Attempt::from_river(first.seed, &missed, 1_000, 4_000)];
    let mut found = 0;
    for s in deal_sessions(20, &log) {
        let review = s.iter().position(|p| p.origin == Origin::Due);
        if let Some(at) = review {
            assert_eq!(spot_of_seed(s[at].seed).index, spot.index);
            assert!(at < river::REVIEW_EVERY, "the review waited until question {}", at + 1);
            found += 1;
        }
        assert_eq!(s.iter().filter(|p| spot_of_seed(p.seed).index == spot.index).count(), usize::from(review.is_some()));
    }
    assert_eq!(found, 20);
    let right = grade(&item, spot.best_option().code).unwrap();
    let mut log2 = log.clone();
    log2.push(Attempt::from_river(first.seed, &right, 2_000, 4_000));
    assert!(deal_sessions(20, &log2).iter().flatten().all(|p| p.origin != Origin::Due));
    // A spot just answered is not dealt again while others of its kind remain.
    assert!(deal_sessions(50, &log2).iter().flatten().all(|p| spot_of_seed(p.seed).index != spot.index));
}

#[test]
fn river_rows_count_toward_no_mark_checkup_or_baseline() {
    let mut rows = Vec::new();
    for (i, s) in spots().iter().take(200).enumerate() {
        let item = item_for_seed(seed_for(s.index, i as u64));
        let out = grade(&item, s.options[i % s.options.len()].code).unwrap();
        rows.push(Attempt::from_river(item.seed, &out, 1_000_000 + i as u64 * 60_000, 3_000));
    }
    let text = rows.iter().map(format_attempt).collect::<Vec<_>>().join("\n");
    assert!(text.lines().all(|l| l.contains("\triver\t")));
    let (parsed, warnings) = parse_log(&text);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(parsed.len(), rows.len());
    for (a, b) in parsed.iter().zip(&rows) {
        assert_eq!(a.mode, Mode::River);
        assert_eq!(a.skill, Skill::River);
        assert_eq!(a.gen_version, RIVER_GEN_VERSION);
        assert_eq!(a.extra.family, b.extra.family);
        assert!(a.extra.family.as_deref().unwrap().starts_with("river:"));
    }
    assert!(all_stats(&parsed).iter().all(|s| s.total_attempts == 0));
    assert!(baseline_runs(&parsed, 32).is_empty());
    assert!(checkup_runs(&parsed).is_empty());
    assert!(due_families(&parsed, 10_000_000_000_000).is_empty());
    assert!(mixed_checkup_offer(&parsed, 10_000_000_000_000).is_none());
    assert_eq!(river::answered_spots(&parsed), 200);
    // The default drill never deals a river spot.
    assert!(!poker_trainer::groups::mixed_skills().contains(&Skill::River));
    assert!(poker_trainer::groups::group_of(Skill::River).is_none());
}
