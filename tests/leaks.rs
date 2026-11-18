use std::collections::HashMap;
use std::process::Command;

use poker_trainer::checkup::checkup_runs;
use poker_trainer::grade::grade;
use poker_trainer::item::Skill;
use poker_trainer::leaks::{
    decode_leak_seed, family_log_tag, leak_seed, parse_leaks, verify_leaks, LeakSession, DEFAULT_LEAK_ITEMS,
};
use poker_trainer::log::{format_attempt, parse_log, Attempt, Mode};
use poker_trainer::progress::{all_stats, baseline_runs};
use poker_trainer::schedule::due_families;

fn sample_leaks() -> &'static str {
    "#LEAKS v1\n\
#meta generated_at=2026-01-15T12:00:00 hands=20 leaks=4 preflop=3 price=1 half_life_days=14\n\
LEAK|0|preflop|leak:pf:early:limpers:ATo|4000|2|2026-01-15|2026-01-15a|synth-01|4.00|-18.5|You raised to 4bb from UTG+1 (over 1 limper)|Put in 4.0bb here; lost 18.5bb on the hand|players=9|hero_pos=UTG+1|behind=7|sit=limpers:1|hand=AcTd|hero_act=r|expect=f\n\
LEAK|1|preflop|leak:pf:button:raised:87o|2000|1|2026-01-14|2026-01-14a|synth-02|2.00|+12.0|You called 2bb from the button (facing a CO raise)|Put in 2.0bb here (won +12.0bb in the hand, still -EV)|players=9|hero_pos=BTN|behind=2|sit=raised|hand=7c8h|hero_act=c|expect=f\n\
LEAK|2|preflop|leak:pf:big_blind:limpers:QJo|1800|1|2026-01-13|2026-01-13a|synth-03|2.00|-2.0|You raised to 3bb from the big blind (over 1 limper)|Cost 2.0bb on the hand|players=6|hero_pos=BB|behind=0|sit=limpers:1|hand=JcQh|hero_act=r|expect=c\n\
LEAK|3|price|leak:price:call_below:river|3500|1|2026-01-15|2026-01-15a|synth-04|20.00|-20.0|You called 20bb into 120bb on the river (needed 14%, had 2%-4%)|Put in 20.0bb here; lost 20.0bb on the hand|street=river|hero_pos=BB|board=4sQc3s5s3c|hand=As9h|pot_before=100|bet=20|callers=0|share_pct=3|tight_pct=2|loose_pct=4|hero_act=c|expect=f\n"
}

#[test]
fn parses_and_verifies_preflop_and_price_leaks() {
    let lib = parse_leaks(sample_leaks()).expect("valid leak file");
    assert_eq!(lib.counts.total, 4);
    assert_eq!(lib.counts.preflop, 3);
    assert_eq!(lib.counts.price, 1);

    let verified = verify_leaks(&lib).expect("all synthetic rows agree with trainer grader");
    assert_eq!(verified.total, 4);

    let price_entry = lib.get(3).unwrap();
    let (hand, board, street) = price_entry.cards_for_seed(leak_seed(3, 0)).unwrap();
    assert_eq!(street, "river");
    assert_eq!(board.len(), 5);
    assert_eq!(hand[0].to_string(), "As");

    let note = price_entry.reveal_note();
    assert_eq!(note.when, "2026-01-15 (2026-01-15a)");
    assert!(note.did.ends_with('.'));
    assert!(note.cost.ends_with('.'));
}

#[test]
fn rejects_malformed_and_disagreeing_leaks() {
    assert!(parse_leaks("").is_err());
    assert!(parse_leaks("#WRONG\nLEAK|0|preflop|f|1|1|d|s|h|1|1|did|cost|players=6|hero_pos=BTN|behind=2|sit=folded|hand=AcAd|hero_act=r|expect=f\n").is_err());
    // Clash between hero cards and board on price row
    let clash = "#LEAKS v1\nLEAK|0|price|f|1000|1|2026-01-01|s|h0|10|-10|You called|Cost 10bb|street=river|hero_pos=BB|board=4sQc3s5sAs|hand=As9h|pot_before=100|bet=20|callers=0|share_pct=3|tight_pct=2|loose_pct=4|hero_act=c|expect=f\n";
    assert!(parse_leaks(clash).is_err());

    // Hero acted right according to trainer (folded 72o UTG first-in vs wrong expect)
    let bad_expect = "#LEAKS v1\nLEAK|0|preflop|f|1000|1|2026-01-01|s|h1|1|-1|You raised|Cost 1bb|players=6|hero_pos=UTG|behind=5|sit=folded|hand=7c2d|hero_act=r|expect=r\n";
    let lib = parse_leaks(bad_expect).unwrap();
    assert!(verify_leaks(&lib).is_err());

    let hero_was_right = "#LEAKS v1\nLEAK|0|preflop|f|1000|1|2026-01-01|s|h2|3|+2|You raised|Cost 0bb|players=6|hero_pos=BTN|behind=2|sit=folded|hand=AcKd|hero_act=r|expect=r\n";
    let lib2 = parse_leaks(hero_was_right).unwrap();
    assert!(verify_leaks(&lib2).is_err());
}

#[test]
fn leak_session_weights_retries_and_isolates_stats() {
    let lib = parse_leaks(sample_leaks()).unwrap();
    let tag_zero = family_log_tag(&lib.entries[0].family);
    assert!(!tag_zero.contains("ATo") && !tag_zero.contains("early"), "{tag_zero}");

    // Three consecutive right answers on entry 0 downweight it strongly compared to missed entry 1.
    let mut attempts = Vec::new();
    let item0 = lib.entries[0].item();
    let out0 = grade(&item0, "f").unwrap();
    for i in 0..5u64 {
        attempts.push(Attempt::from_leak(Skill::Preflop, leak_seed(0, 0), tag_zero.clone(), &out0, 1_000 + i, 500));
    }
    let item1 = lib.entries[1].item();
    let miss1 = grade(&item1, "c").unwrap();
    attempts.push(Attempt::from_leak(Skill::Preflop, leak_seed(1, 0), family_log_tag(&lib.entries[1].family), &miss1, 2_000, 500));

    // Mode::Leak round-trips in TSV log and never pollutes pass marks, checkups, baselines, or due families.
    let tsv = attempts.iter().map(format_attempt).collect::<Vec<_>>().join("\n");
    let (parsed, warnings) = parse_log(&tsv);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(parsed.len(), attempts.len());
    assert!(parsed.iter().all(|a| a.mode == Mode::Leak));
    assert!(all_stats(&parsed).iter().all(|s| s.total_attempts == 0));
    assert!(baseline_runs(&parsed, 32).is_empty());
    assert!(checkup_runs(&parsed).is_empty());
    assert!(due_families(&parsed, 10_000_000_000).is_empty());

    // Over many session starts, entry 1 is dealt first far more often than heavily-mastered entry 0.
    let mut first_counts: HashMap<u16, usize> = HashMap::new();
    for s in 1..=200u64 {
        let mut sess = LeakSession::new(lib.clone(), &parsed, s, Some(DEFAULT_LEAK_ITEMS));
        let first = sess.try_next_item().unwrap();
        let (id, _) = decode_leak_seed(first.seed);
        *first_counts.entry(id).or_default() += 1;
    }
    assert!(first_counts.get(&1).copied().unwrap_or(0) > first_counts.get(&0).copied().unwrap_or(0) * 5, "{first_counts:?}");

    // A full 15-item session finishes cleanly, and a miss queues a suit-permuted retry.
    let mut sess = LeakSession::new(lib, &[], 42, Some(DEFAULT_LEAK_ITEMS));
    let mut seen = Vec::new();
    while let Some(planned) = sess.try_next_item() {
        let ok = seen.len() != 0; // miss item 0
        seen.push(planned);
        sess.record(planned, ok);
    }
    assert_eq!(seen.len(), DEFAULT_LEAK_ITEMS);
    let (missed_id, _) = decode_leak_seed(seen[0].seed);
    assert!(
        seen.iter().skip(1).any(|p| p.origin == poker_trainer::schedule::Origin::SessionRetry && decode_leak_seed(p.seed) == (missed_id, 1)),
        "expected suit-permuted retry for missed item {missed_id}"
    );
}

#[test]
fn cli_check_leaks_accepts_valid_and_rejects_bad_files() {
    let dir = std::env::temp_dir().join("poker-trainer-test-check-leaks");
    let _ = std::fs::create_dir_all(&dir);
    let good_path = dir.join("good.txt");
    let bad_path = dir.join("bad.txt");
    std::fs::write(&good_path, sample_leaks()).unwrap();
    std::fs::write(
        &bad_path,
        "#LEAKS v1\nLEAK|0|preflop|f|1000|1|2026-01-01|s|h1|1|-1|You raised|Cost 1bb|players=6|hero_pos=UTG|behind=5|sit=folded|hand=7c2d|hero_act=r|expect=r\n",
    )
    .unwrap();

    let bin = env!("CARGO_BIN_EXE_poker-trainer");
    let ok = Command::new(bin).arg("check-leaks").arg(&good_path).output().unwrap();
    assert!(ok.status.success(), "{}", String::from_utf8_lossy(&ok.stderr));
    assert!(String::from_utf8_lossy(&ok.stdout).contains("4 leaks verified (3 preflop, 1 price)"));

    let bad = Command::new(bin).arg("check-leaks").arg(&bad_path).output().unwrap();
    assert!(!bad.status.success());
    let _ = std::fs::remove_dir_all(&dir);
}
