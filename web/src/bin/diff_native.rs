//! Prints what the bridge answers natively for many (skill, seed) pairs, as one
//! JSON line per request. The browser test runs the same requests through the
//! wasm build and compares the output byte for byte.
//!
//! Usage: diff_native COUNT [--time]

use std::time::Instant;

use trainer_web::dispatch;

const SKILLS: [&str; 16] = [
    "preflop", "nuts", "possible", "made", "beats", "outs", "price", "hit", "spot", "spot_mw",
    "board_type", "equity", "anchor", "table", "chain", "beats_me",
];
/// Answers tried for items that aren't simple choices.
const FREE_ANSWERS: [&str; 18] = [
    "AKs", "QJ", "Ahx", "Ax", "AhKh", "KK", "f", "fs", "fsh", "none", "0", "4", "9", "25", "33.3", "50", "Asxs", "Kdxh",
];
/// Answers tried on `beats_me`: subsets of the letters, ways counts and bands.
const BEATS_ME_ANSWERS: [&str; 11] = ["none", "a", "ab", "abcd", "pk", "ptfh", "6", "3", "16", "e", "b"];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let count: u64 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(40);
    if args.iter().any(|a| a == "--time") {
        print_timing(count);
        return;
    }
    let mut out = Out { commands: Vec::new() };
    drills(&mut out, count);
    origins(&mut out);
    chain_session(&mut out);
    retried_session(&mut out);
    start_over_notes(&mut out);
    baselines(&mut out);
    session(&mut out, 40);
    checkup(&mut out);
    anchor_only_session(&mut out);
    table_session(&mut out);
    table_checkup(&mut out);
    for (command, reply) in out.commands {
        // One request per output line: a JSON reply never holds a raw newline.
        assert!(!reply.contains('\n'), "a reply holds a raw newline: {reply}");
        println!("{}\t{reply}", escape_command(&command));
    }
}

/// Makes a command one tab-free line. `load` commands carry whole log texts, so
/// a command can hold tabs and newlines as well as the field separator.
fn escape_command(command: &str) -> String {
    command
        .replace('\\', "\\\\")
        .replace('\x1f', "\\x1f")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
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
    matches!(skill, "nuts" | "possible" | "outs" | "price" | "hit" | "equity" | "anchor" | "board_type" | "chain" | "beats_me")
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
        "chain" => ["hc", "top", "2p", "4", "9", "25", "33", "c", "f", "zzz"].into_iter().map(|a| (a, "0")).collect(),
        "beats_me" => BEATS_ME_ANSWERS.iter().map(|&a| (a, "0")).collect(),
        _ => FREE_ANSWERS.iter().map(|&a| (a, "0")).collect(),
    }
}

fn drills(out: &mut Out, count: u64) {
    for &skill in &SKILLS {
        let items = if skill == "equity" { count / 3 } else { count };
        for i in 0..items {
            // beats_me carries its rung in the top bits of the seed.
            let rung_bits = if skill == "beats_me" { (i % 4) << 60 } else { 0 };
            let seed = (rung_bits + i * 2_654_435_761 + 17).to_string();
            let prepared = out.call(&["prepare", skill, &seed]);
            if skill == "table" {
                for code in option_codes(&prepared) {
                    out.call(&["grade", skill, &seed, &code, "0"]);
                }
                out.call(&["grade", skill, &seed, "zzz", "0"]);
                continue;
            }
            if is_free_form(skill) {
                for (answer, response) in answers_for(skill) {
                    out.call(&["grade", skill, &seed, answer, response]);
                }
            }
        }
    }
}

/// The optional origin on `prepare`: it decides whether a miss offers a reflection.
fn origins(out: &mut Out) {
    for skill in ["made", "beats", "spot", "spot_mw", "preflop", "chain"] {
        for i in 0..6u64 {
            let seed = (i * 2_654_435_761 + 71).to_string();
            for origin in ["new", "due", "retry", "unknown"] {
                out.call(&["prepare", skill, &seed, origin]);
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
        "chain" => "c",
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

/// The option codes in a prepared bundle's answer format.
fn option_codes(prepared: &str) -> Vec<String> {
    let format = &prepared[prepared.find("\"format\":").unwrap_or(0)..];
    format.match_indices("\"code\":\"").map(|(i, needle)| {
        let tail = &format[i + needle.len()..];
        tail[..tail.find('"').unwrap_or(0)].to_string()
    }).collect()
}

/// A whole chain session: stages in order, each logged with its stage.
fn chain_session(out: &mut Out) {
    out.call(&["load", ""]);
    out.call(&["session_open", "31337", "1000000", "chain"]);
    out.call(&["session_start", "15"]);
    for step in 0..16u64 {
        let next = out.call(&["session_next", &(step * 1000).to_string()]);
        let (Some(skill), Some(seed)) = (field(&next, "skill"), field(&next, "seed")) else { break };
        let (skill, seed) = (skill.to_string(), seed.to_string());
        out.call(&["session_peek", "1"]);
        out.call(&["prepare", &skill, &seed]);
        let reply = out.call(&["grade", &skill, &seed, ["hc", "6", "25", "c"][(step % 4) as usize], "1500"]);
        let Some(template) = first_template(&reply) else { break };
        out.call(&["commit", &template, &(2_000_000 + step).to_string(), "1500"]);
    }
    out.call(&["session_summary"]);
    out.call(&["progress", "2100000"]);
    out.call(&["export"]);
}

/// Misses come back as siblings, confidence and reflections are logged, and
/// every sibling search and later-day review goes through the bridge.
fn retried_session(out: &mut Out) {
    out.call(&["load", ""]);
    out.call(&["session_open", "9009", "1000000", "made,spot,price"]);
    out.call(&["session_start", "15"]);
    for step in 0..30u64 {
        let next = out.call(&["session_next", &(step * 1000).to_string()]);
        let (Some(skill), Some(seed)) = (field(&next, "skill"), field(&next, "seed")) else { break };
        let (skill, seed) = (skill.to_string(), seed.to_string());
        out.call(&["session_peek", "0"]);
        let answer = if skill == "price" { "99" } else if skill == "made" { "sf" } else { "c" };
        let reply = out.call(&["grade", &skill, &seed, answer, "1500"]);
        let Some(template) = first_template(&reply) else { break };
        // The confidence field is optional, and a code that means nothing is dropped.
        let time = (2_000_000 + step).to_string();
        match step % 5 {
            0 => out.call(&["commit", &template, &time, "1500"]),
            1 => out.call(&["commit", &template, &time, "1500", ""]),
            2 => out.call(&["commit", &template, &time, "1500", "bogus"]),
            3 => out.call(&["commit", &template, &time, "1500", "medium"]),
            _ => out.call(&["commit", &template, &time, "1500", "high"]),
        };
        if step % 3 == 0 {
            out.call(&["reflect_commit", &template, &(2_000_500 + step).to_string(), "supported:price,against:opp"]);
        }
    }
    out.call(&["session_summary"]);
    out.call(&["progress", "2100000"]);
    out.call(&["export"]);
    // A later-day session reviews the families it just missed.
    out.call(&["session_open", "9010", "400000000", "made,spot,price"]);
    out.call(&["session_start", "15"]);
    for step in 0..12u64 {
        let next = out.call(&["session_next", &(step * 1000).to_string()]);
        let (Some(skill), Some(seed)) = (field(&next, "skill"), field(&next, "seed")) else { break };
        let (skill, seed) = (skill.to_string(), seed.to_string());
        let reply = out.call(&["grade", &skill, &seed, "1", "1500"]);
        let Some(template) = first_template(&reply) else { break };
        out.call(&["commit", &template, &(500_000_000 + step).to_string(), "1500"]);
    }
    out.call(&["progress", "600000000"]);
}

/// The table checkup start screen: no form in progress, form 4 in progress, form 5 in progress.
fn start_over_notes(out: &mut Out) {
    out.call(&["load", ""]);
    out.call(&["check_info", "table"]);
    for form in [4u32, 5] {
        out.call(&["load", &format!("1\tcheck\t\ttable\t7\t2\t1\t0\t\t1000\tfold\t1\t{form}\t0\t\t\t\t\n")]);
        out.call(&["check_info", "table"]);
    }
}

/// A `table`-only session through the bridge: dealing order, recency and logging.
fn table_session(out: &mut Out) {
    out.call(&["load", ""]);
    out.call(&["session_open", "2024", "1000000", "table"]);
    out.call(&["session_start", "15"]);
    for step in 0..30u64 {
        let next = out.call(&["session_next", &(step * 1000).to_string()]);
        let (Some(skill), Some(seed)) = (field(&next, "skill"), field(&next, "seed")) else { break };
        let (skill, seed) = (skill.to_string(), seed.to_string());
        out.call(&["session_peek", "1"]);
        let prepared = out.call(&["prepare", &skill, &seed]);
        let codes = option_codes(&prepared);
        let code = codes.get((step as usize) % codes.len().max(1)).cloned().unwrap_or_default();
        let reply = out.call(&["grade", &skill, &seed, &code, "1500"]);
        let Some(template) = first_template(&reply) else { break };
        out.call(&["commit", &template, &(2_000_000 + step).to_string(), "1500"]);
    }
    out.call(&["session_summary"]);
    out.call(&["progress", "2100000"]);
    out.call(&["export"]);
}

/// A whole `table` checkup through the bridge, grading each slot's first option.
fn table_checkup(out: &mut Out) {
    out.call(&["load", ""]);
    let begin = out.call(&["check_begin", "table", "0"]);
    let run = number_field(&begin, "run").expect("a run").to_string();
    let form = number_field(&begin, "form").expect("a form").to_string();
    for count in 0..50 {
        let next = out.call(&["check_next", "table", &run]);
        let Some(slot) = number_field(&next, "slot").map(str::to_string) else { break };
        let prepared = out.call(&["prepare_check", "table", &form, &slot, &run]);
        let code = option_codes(&prepared).into_iter().next().unwrap_or_default();
        let reply = out.call(&["check_grade", "table", &form, &slot, &run, &code, "1500"]);
        let Some(template) = first_template(&reply) else { break };
        out.call(&["check_commit", &template, &(3_000_000 + count).to_string(), "1500"]);
    }
    out.call(&["check_next", "table", &run]);
    out.call(&["check_report", &run]);
    out.call(&["home"]);
    out.call(&["progress", "4000000"]);
    out.call(&["export"]);
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
