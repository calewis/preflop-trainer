//! Prints what the bridge answers natively for many (skill, seed) pairs, as one
//! JSON line per request. The browser test runs the same requests through the
//! wasm build and compares the output byte for byte.
//!
//! Usage: diff_native COUNT [--time]

use std::time::Instant;

use trainer_web::dispatch;

const SKILLS: [&str; 10] = [
    "preflop", "nuts", "possible", "made", "beats", "outs", "price", "hit", "spot", "spot_mw",
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
    session(&mut out, 25);
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
    matches!(skill, "nuts" | "possible" | "outs" | "price" | "hit")
}

fn drills(out: &mut Out, count: u64) {
    for &skill in &SKILLS {
        for i in 0..count {
            let seed = (i * 2_654_435_761 + 17).to_string();
            out.call(&["prepare", skill, &seed]);
            if is_free_form(skill) {
                for answer in FREE_ANSWERS {
                    out.call(&["grade", skill, &seed, answer]);
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
        let reply = if is_free_form(&skill) {
            out.call(&["grade", &skill, &seed, scripted_answer(&skill)])
        } else {
            out.call(&["prepare", &skill, &seed])
        };
        let Some(template) = first_template(&reply) else { break };
        out.call(&["commit", &template, &(2_000_000 + step).to_string(), "1500"]);
    }
    out.call(&["session_summary"]);
    out.call(&["progress"]);
    out.call(&["export"]);
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
