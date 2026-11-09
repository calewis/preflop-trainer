//! Prints what the bridge answers natively for many (skill, seed) pairs, as one
//! JSON line per request. The browser test runs the same requests through the
//! wasm build and compares the output byte for byte.
//!
//! Usage: diff_native COUNT [--time]

use std::time::Instant;

use trainer_web::dispatch;

const SKILLS: [&str; 13] = [
    "preflop", "nuts", "possible", "made", "beats", "outs", "price", "hit", "spot", "spot_mw",
    "board_type", "equity", "anchor",
];
/// Answers tried for items that aren't simple choices.
const FREE_ANSWERS: [&str; 16] = [
    "AKs", "QJ", "Ahx", "Ax", "AhKh", "KK", "f", "fs", "fsh", "none", "0", "4", "9", "25", "33.3", "50",
];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let count: u64 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(40);
    if args.iter().any(|a| a == "--time") {
        print_timing(count);
        return;
    }
    let mut out = Out { commands: Vec::new() };
    drills(&mut out, count);
    baselines(&mut out);
    session(&mut out, 40);
    checkup(&mut out);
    anchor_only_session(&mut out);
    for (command, reply) in out.commands {
        println!("{}\t{reply}", command.replace('\x1f', "\\x1f").replace('\t', "\\t"));
    }
}

struct Out {
    commands: Vec<(String, String)>,
}

impl Out {
    fn call(&mut self, parts: &[&str]) -> String {
        let command = parts.join("\x1f");
        let reply = dispatch(&command);
        self.commands.push((command, reply.clone()));
        reply
    }
}

fn is_free_form(skill: &str) -> bool {
    matches!(skill, "nuts" | "possible" | "outs" | "price" | "hit" | "equity" | "anchor" | "board_type")
}

/// Answers for one skill: `board_type` tries its codes at target, slow and too-slow
/// times; `equity` gets only three because each grade replays the whole equity.
fn answers_for(skill: &str) -> Vec<(&'static str, &'static str)> {
    match skill {
        "board_type" => vec![
            ("rc", "1000"), ("rd", "1000"), ("tp", "1000"), ("td", "1000"), ("mc", "1000"),
            ("rc", "7000"), ("rc", "13000"), ("rp", "1000"),
        ],
        "equity" => vec![("10", "0"), ("45", "0"), ("90", "0")],
        _ => FREE_ANSWERS.iter().map(|&a| (a, "0")).collect(),
    }
}

fn drills(out: &mut Out, count: u64) {
    for &skill in &SKILLS {
        let items = if skill == "equity" { count / 3 } else { count };
        for i in 0..items {
            let seed = (i * 2_654_435_761 + 17).to_string();
            out.call(&["prepare", skill, &seed]);
            if is_free_form(skill) {
                for (answer, response) in answers_for(skill) {
                    out.call(&["grade", skill, &seed, answer, response]);
                }
            }
        }
    }
}

fn baselines(out: &mut Out) {
    for seed in ["1", "991"] {
        for index in 0..36 {
            out.call(&["prepare_baseline", seed, &index.to_string()]);
        }
    }
}

/// Pulls the first attempt template out of a reply; the bridge only escapes tabs in it.
fn first_template(reply: &str) -> Option<String> {
    let start = reply.find("\"attempt\":\"")? + "\"attempt\":\"".len();
    let end = start + reply[start..].find('"')?;
    Some(reply[start..end].replace("\\t", "\t"))
}

fn scripted_answer(skill: &str) -> &'static str {
    match skill {
        "nuts" => "AKs",
        "possible" => "f",
        "outs" => "4",
        "price" | "hit" => "25",
        "equity" | "anchor" => "42",
        "board_type" => "td",
        _ => "",
    }
}

fn field<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("\"{key}\":\"");
    let start = json.find(&needle)? + needle.len();
    let end = start + json[start..].find('"')?;
    Some(&json[start..end])
}

/// Runs one whole session through the bridge, so the dealing order is compared too.
fn session(out: &mut Out, items: u64) {
    out.call(&["load", ""]);
    out.call(&["session_open", "424242", "1000000"]);
    out.call(&["session_start", "15"]);
    for step in 0..items {
        let next = out.call(&["session_next", &(step * 1000).to_string()]);
        let (Some(skill), Some(seed)) = (field(&next, "skill"), field(&next, "seed")) else {
            break;
        };
        let (skill, seed) = (skill.to_string(), seed.to_string());
        out.call(&["session_peek", "1"]);
        out.call(&["session_peek", "0"]);
        let response = if skill == "board_type" { "2000" } else { "1500" };
        let reply = if is_free_form(&skill) {
            out.call(&["grade", &skill, &seed, scripted_answer(&skill), response])
        } else {
            out.call(&["prepare", &skill, &seed])
        };
        let Some(template) = first_template(&reply) else { break };
        out.call(&["commit", &template, &(2_000_000 + step).to_string(), response]);
    }
    out.call(&["session_summary"]);
    out.call(&["progress", "2100000"]);
    out.call(&["export"]);
}

/// Answers a whole checkup through the bridge, quitting partway and resuming.
fn checkup(out: &mut Out) {
    out.call(&["load", ""]);
    let begin = out.call(&["check_begin", "price", "0"]);
    let run = number_field(&begin, "run").expect("a run").to_string();
    let form = number_field(&begin, "form").expect("a form").to_string();
    for count in 0..50 {
        if count == 17 {
            out.call(&["check_info", "price"]);
            let resumed = out.call(&["check_begin", "price", "1"]);
            assert_eq!(number_field(&resumed, "run"), Some(run.as_str()), "resume kept the run");
        }
        let next = out.call(&["check_next", "price", &run]);
        let Some(slot) = number_field(&next, "slot") else { break };
        let slot = slot.to_string();
        out.call(&["prepare_check", "price", &form, &slot, &run]);
        let reply = out.call(&["check_grade", "price", &form, &slot, &run, "33", "1500"]);
        let Some(template) = first_template(&reply) else { break };
        out.call(&["check_commit", &template, &(3_000_000 + count).to_string(), "1500"]);
    }
    out.call(&["check_next", "price", &run]);
    out.call(&["check_report", &run]);
    out.call(&["check_info", "price"]);
    out.call(&["home"]);
    out.call(&["progress", "4000000"]);
    out.call(&["export"]);
}

/// A session of only `anchor` runs out of cards: the bridge says so.
fn anchor_only_session(out: &mut Out) {
    out.call(&["load", ""]);
    out.call(&["session_open", "77", "1000000", "anchor"]);
    out.call(&["session_start", "15"]);
    for step in 0..12u64 {
        let next = out.call(&["session_next", &(step * 1000).to_string()]);
        if next.contains("\"finished\"") {
            break;
        }
        let (Some(skill), Some(seed)) = (field(&next, "skill"), field(&next, "seed")) else {
            break;
        };
        let (skill, seed) = (skill.to_string(), seed.to_string());
        out.call(&["session_peek", "1"]);
        let reply = out.call(&["grade", &skill, &seed, "42", "1500"]);
        let Some(template) = first_template(&reply) else { break };
        out.call(&["commit", &template, &(2_000_000 + step).to_string(), "1500"]);
    }
    out.call(&["session_summary"]);
}

/// A number field in a JSON reply, as text.
fn number_field<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("\"{key}\":");
    let start = json.find(&needle)? + needle.len();
    let tail = &json[start..];
    let end = tail.find(|c: char| !c.is_ascii_digit()).unwrap_or(tail.len());
    (end > 0).then(|| &tail[..end])
}

fn print_timing(count: u64) {
    for &skill in &SKILLS {
        let mut worst = (0f64, 0u64);
        let mut total = 0f64;
        for i in 0..count {
            let seed = i * 2_654_435_761 + 17;
            let start = Instant::now();
            let _ = dispatch(&format!("prepare\x1f{skill}\x1f{seed}"));
            let ms = start.elapsed().as_secs_f64() * 1000.0;
            total += ms;
            if ms > worst.0 {
                worst = (ms, seed);
            }
        }
        println!("{skill:9} mean {:9.2} ms  max {:9.2} ms (seed {})", total / count as f64, worst.0, worst.1);
    }
}
