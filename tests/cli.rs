//! Drives the real binary through stdin with a temporary home directory.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn temp_home(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("poker-trainer-test-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn run(home: &Path, args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_poker-trainer"))
        .args(args)
        .env("POKER_TRAINER_HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("binary runs");
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    child.wait_with_output().unwrap()
}

fn log_rows(home: &Path) -> Vec<Vec<String>> {
    fs::read_to_string(home.join("attempts.tsv"))
        .unwrap()
        .lines()
        .skip(1)
        .map(|l| l.split('\t').map(str::to_string).collect())
        .collect()
}

/// Answers every kind of drill question; a wrong kind is re-prompted and not counted.
fn answers(repeats: usize) -> String {
    "not-an-answer\nr\n25\nnone\nhc\na\n5\nc\nAK\nrp\nrc\nrd\ntp\ntc\ntd\nmc\nmd\n".repeat(repeats)
}

#[test]
fn drill_logs_each_answer_and_ignores_invalid_input() {
    let home = temp_home("drill");
    let out = run(&home, &["drill", "--items", "5", "--seed", "7", "--skill", "price"], &answers(60));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("No baseline on record"));
    assert!(stdout.contains("Session done"));
    assert!(!stdout.contains('\x1b'), "no colour when stdout is not a terminal");

    let rows = log_rows(&home);
    assert_eq!(rows.len(), 5, "invalid input must not count");
    for row in &rows {
        assert_eq!(row.len(), 24);
        assert!(row[1] == "drill" || row[1] == "drill-retry");
        assert_eq!(row[2], "");
        assert_eq!(row[3], "price", "--skill deals only the named skill");
        assert!(row[4].parse::<u64>().is_ok());
        assert_eq!(row[5], "2");
        assert!(row[6] == "0" || row[6] == "1");
        assert!(row[9].parse::<u32>().unwrap() <= 60_000);
    }
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn several_skill_flags_deal_only_those_skills() {
    let home = temp_home("only");
    let out = run(&home, &["drill", "--items", "12", "--seed", "4", "--skill", "price", "--skill", "hit"], &answers(80));
    assert!(out.status.success());
    let rows = log_rows(&home);
    assert_eq!(rows.len(), 12);
    assert!(rows.iter().all(|r| r[3] == "price" || r[3] == "hit"), "{rows:?}");
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn without_skill_flags_preflop_is_in_rotation() {
    let home = temp_home("rotation");
    let out = run(&home, &["drill", "--items", "40", "--seed", "4"], &answers(200));
    assert!(out.status.success());
    assert!(log_rows(&home).iter().any(|r| r[3] == "preflop"));
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn q_quits_and_saves_what_was_answered() {
    let home = temp_home("quit");
    let out = run(&home, &["drill", "--items", "9", "--seed", "3", "--skill", "price"], "r\n25\nr\n25\nq\n");
    assert!(out.status.success());
    let answered = log_rows(&home).len();
    assert!((1..=4).contains(&answered), "{answered}");
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn preflop_prompt_states_table_seat_and_players_behind() {
    let home = temp_home("preflop");
    let out = run(&home, &["drill", "--items", "6", "--seed", "21", "--skill", "preflop"], &"r\nc\n".repeat(20));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Table of "), "{stdout}");
    assert!(stdout.contains("still to act behind you"), "{stdout}");
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn spot_mw_prompt_states_nobody_can_raise() {
    let home = temp_home("mw");
    let out = run(&home, &["drill", "--items", "8", "--seed", "5", "--skill", "spot_mw"], &"c\nr\n".repeat(30));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Everyone else has acted and nobody can raise."), "{stdout}");
    assert!(stdout.contains("Opponent 1 is a"), "{stdout}");
    assert!(stdout.contains("averaged over all possible remaining cards and combos"), "{stdout}");
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn baseline_runs_and_compares_with_earlier_runs() {
    let home = temp_home("baseline");
    let script = "5\n25\nnone\nhc\na\nc\nAK\nx\n".repeat(60);
    let first = run(&home, &["baseline"], &script);
    let stdout = String::from_utf8_lossy(&first.stdout);
    assert!(first.status.success(), "{stdout}");
    assert!(stdout.contains("BASELINE RESULT (run 1)"), "{stdout}");
    assert!(stdout.contains("first baseline"));
    assert!(!stdout.contains("Correct."), "no feedback during the test");
    let rows = log_rows(&home);
    assert_eq!(rows.len(), 36);
    assert!(rows.iter().all(|r| r[1] == "baseline" && r[2] == "1" && r[4].starts_with('b')));

    let second = run(&home, &["baseline"], &script);
    let stdout = String::from_utf8_lossy(&second.stdout);
    assert!(stdout.contains("BASELINE RESULT (run 2)"));
    assert!(stdout.contains("Earlier runs"));
    assert!(stdout.contains("Run 1:"));

    let progress = run(&home, &["progress"], "");
    let stdout = String::from_utf8_lossy(&progress.stdout);
    assert!(stdout.contains("Not enough data"), "{stdout}");
    assert!(stdout.contains("Current step: 1 of 4"), "{stdout}");
    assert!(stdout.contains("Run 2:"), "{stdout}");
    assert!(!stdout.to_lowercase().contains("win rate"));
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn a_checkup_gives_no_feedback_until_the_end_and_logs_its_form() {
    let home = temp_home("check");
    let script = "25\n".repeat(50);
    let out = run(&home, &["check", "--skill", "price"], &script);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("form 0"), "{stdout}");
    assert!(stdout.contains("CHECKUP RESULT"), "{stdout}");
    assert!(!stdout.contains("Correct.") && !stdout.contains("Not quite."), "no feedback during the test");
    let rows = log_rows(&home);
    assert_eq!(rows.len(), 50);
    assert!(rows.iter().all(|r| r[1] == "check" && r.len() == 24));
    let again = run(&home, &["check", "--skill", "price"], "q\n");
    assert!(String::from_utf8_lossy(&again.stdout).contains("form 1"));
    let progress = run(&home, &["progress"], "");
    assert!(String::from_utf8_lossy(&progress.stdout).contains("Checkup history"));
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn anchor_only_drills_stop_when_nothing_is_due() {
    let home = temp_home("anchor");
    let out = run(&home, &["drill", "--items", "30", "--seed", "2", "--skill", "anchor"], &"50\n".repeat(80));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("No anchor cards are due"), "{stdout}");
    assert!(log_rows(&home).len() <= 10, "at most five new cards plus their retries");
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn board_type_and_equity_drills_run_and_log() {
    let home = temp_home("newskills");
    let script = "rp\nrc\nrd\ntp\ntc\ntd\nmc\nmd\n50\n".repeat(60);
    let out = run(&home, &["drill", "--items", "8", "--seed", "11", "--skill", "board_type"], &script);
    assert!(out.status.success());
    let out = run(&home, &["drill", "--items", "3", "--seed", "11", "--skill", "equity"], &"50\n".repeat(10));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("Exact equity:"), "{stdout}");
    let rows = log_rows(&home);
    assert_eq!(rows.iter().filter(|r| r[3] == "board_type").count(), 8);
    let equity: Vec<_> = rows.iter().filter(|r| r[3] == "equity").collect();
    assert_eq!(equity.len(), 3);
    assert!(equity.iter().all(|r| r.len() == 24 && !r[15].is_empty()));
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn malformed_log_lines_warn_but_do_not_crash() {
    let home = temp_home("malformed");
    fs::create_dir_all(&home).unwrap();
    fs::write(home.join("attempts.tsv"), "garbage\n\u{0}\n").unwrap();
    let out = run(&home, &["progress"], "");
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("malformed"));
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn a_cut_off_last_line_cannot_swallow_the_next_record() {
    let home = temp_home("cutoff");
    fs::create_dir_all(&home).unwrap();
    fs::write(home.join("attempts.tsv"), "1700000000000\tdrill\t\tpri").unwrap();
    let out = run(&home, &["drill", "--items", "1", "--seed", "2", "--skill", "price"], "25\n");
    assert!(out.status.success());
    let text = fs::read_to_string(home.join("attempts.tsv")).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "{text}");
    assert_eq!(lines[1].split('\t').count(), 24, "{text}");
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn card_prints_lines_and_notes() {
    let home = temp_home("card");
    let out = run(&home, &["card"], "");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("early"));
    assert!(stdout.contains("Small blind"));
    assert!(stdout.contains("258 combos"));
    assert!(stdout.contains("In the big blind"));
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn unknown_skill_is_a_clear_error() {
    let home = temp_home("unknown");
    let out = run(&home, &["drill", "--skill", "wat"], "");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown skill"));
}
