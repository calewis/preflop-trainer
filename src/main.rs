//! Command-line front end. All terminal and file I/O lives here.

mod render;

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, IsTerminal, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use poker_trainer::anchors;
use poker_trainer::baseline;
use poker_trainer::checkup::{checkup_plan, checkup_runs, next_checkup_form, next_checkup_run};
use poker_trainer::equity_skill::equity_bias;
use poker_trainer::grade::{grade_timed, Outcome};
use poker_trainer::item::{Item, Skill};
use poker_trainer::log::{format_attempt, parse_log, Attempt, CheckRef, LOG_HEADER};
use poker_trainer::progress::{
    all_stats, baseline_runs, current_step, drill_skills, has_baseline, next_baseline_run,
};
use poker_trainer::rng::Rng;
use poker_trainer::schedule::{Origin, Session, SessionLimits};
use poker_trainer::table;
use render::Style;

const USAGE: &str = "poker-trainer: fundamentals drills for no-limit hold'em

Usage:
  poker-trainer baseline    take the frozen 36-item test
  poker-trainer card        print the preflop card
  poker-trainer drill [--skill NAME]... [--minutes N] [--items N] [--seed N]
                            run a drill (10 minutes by default)
  poker-trainer check --skill NAME
                            take a held-out checkup for one skill
  poker-trainer progress    show results against the pass marks

Skills: preflop, nuts, possible, made, beats, board_type, outs, price, hit, equity,
anchor, spot, spot_mw, table
Type q at any question to quit and save.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("baseline") if args.len() == 1 => run_baseline(),
        Some("card") if args.len() == 1 => {
            print!("{}", render::card_text());
            Ok(())
        }
        Some("drill") => parse_drill_args(&args[1..]).and_then(run_drill),
        Some("check") => parse_check_args(&args[1..]).and_then(run_check),
        Some("progress") if args.len() == 1 => run_progress(),
        Some("gen-baseline") if args.len() == 1 => {
            print!("{}", baseline::to_text(&baseline::generate_baseline()));
            Ok(())
        }
        Some("gen-anchors") if args.len() == 1 => {
            print!("{}", anchors::to_text(&anchors::generate_deck()));
            Ok(())
        }
        Some("help" | "--help" | "-h") => {
            println!("{USAGE}");
            Ok(())
        }
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

struct DrillArgs {
    skills: Vec<Skill>,
    minutes: Option<u64>,
    items: Option<u64>,
    seed: Option<u64>,
}

fn parse_skill(name: &str) -> Result<Skill, String> {
    Skill::from_name(name).ok_or_else(|| {
        let names: Vec<&str> = Skill::ALL.iter().map(|s| s.name()).collect();
        format!("unknown skill {name}. Skills: {}", names.join(", "))
    })
}

fn parse_check_args(args: &[String]) -> Result<Skill, String> {
    match args {
        [flag, name] if flag == "--skill" => parse_skill(name),
        _ => Err(format!("check needs --skill NAME\n\n{USAGE}")),
    }
}

fn parse_drill_args(args: &[String]) -> Result<DrillArgs, String> {
    let mut parsed = DrillArgs { skills: Vec::new(), minutes: None, items: None, seed: None };
    let mut iter = args.iter();
    while let Some(flag) = iter.next() {
        let mut value = || iter.next().ok_or_else(|| format!("{flag} needs a value"));
        let number = |text: &String| text.parse::<u64>().map_err(|_| format!("{flag} needs a whole number, got {text}"));
        match flag.as_str() {
            "--skill" => {
                parsed.skills.push(parse_skill(value()?)?);
            }
            "--minutes" => parsed.minutes = Some(positive(flag, number(value()?)?)?),
            "--items" => parsed.items = Some(positive(flag, number(value()?)?)?),
            "--seed" => parsed.seed = Some(number(value()?)?),
            other => return Err(format!("unknown drill option {other}\n\n{USAGE}")),
        }
    }
    Ok(parsed)
}

fn positive(flag: &str, value: u64) -> Result<u64, String> {
    if value > 0 {
        Ok(value)
    } else {
        Err(format!("{flag} must be at least 1"))
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn clock_seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

fn style() -> Style {
    Style { color: io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none() }
}

/// Where the log lives: `$POKER_TRAINER_HOME`, or `~/.poker-trainer`.
fn home_dir() -> Result<PathBuf, String> {
    if let Some(dir) = std::env::var_os("POKER_TRAINER_HOME").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    let home = std::env::var_os("HOME")
        .filter(|d| !d.is_empty())
        .ok_or("Set POKER_TRAINER_HOME or HOME so the attempt log has somewhere to live.")?;
    Ok(PathBuf::from(home).join(".poker-trainer"))
}

struct Store {
    path: PathBuf,
}

impl Store {
    fn open() -> Result<Store, String> {
        Ok(Store { path: home_dir()?.join("attempts.tsv") })
    }

    /// Reads every readable attempt, warning about the rest.
    fn load(&self) -> Vec<Attempt> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Vec::new(),
            Err(error) => {
                eprintln!("Warning: could not read {}: {error}", self.path.display());
                return Vec::new();
            }
        };
        let (attempts, warnings) = parse_log(&String::from_utf8_lossy(&bytes));
        for warning in warnings {
            eprintln!("Warning: {warning}.");
        }
        attempts
    }

    fn append(&self, attempt: &Attempt) {
        if let Err(error) = self.try_append(attempt) {
            eprintln!("Warning: could not save this attempt to {}: {error}", self.path.display());
        }
    }

    fn try_append(&self, attempt: &Attempt) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let is_new = !self.path.exists();
        let cut_off = !is_new && ends_mid_line(&self.path)?;
        let mut file = OpenOptions::new().create(true).append(true).open(&self.path)?;
        if is_new {
            writeln!(file, "{LOG_HEADER}")?;
        }
        if cut_off {
            writeln!(file)?;
        }
        writeln!(file, "{}", format_attempt(attempt))
    }
}

/// True if a non-empty file doesn't end in a newline, as after an interrupted write.
fn ends_mid_line(path: &Path) -> io::Result<bool> {
    let mut file = File::open(path)?;
    if file.metadata()?.len() == 0 {
        return Ok(false);
    }
    file.seek(SeekFrom::End(-1))?;
    let mut last = [0u8; 1];
    file.read_exact(&mut last)?;
    Ok(last[0] != b'\n')
}

enum Asked {
    Quit,
    Answered { outcome: Box<Outcome>, response_ms: u32 },
}

fn ask(item: &Item, input: &mut impl BufRead, stdout: &mut impl Write) -> Asked {
    let started = Instant::now();
    let hint = render::answer_hint(item);
    loop {
        let _ = write!(stdout, "[{hint}; q quits] > ");
        let _ = stdout.flush();
        let mut line = String::new();
        match input.read_line(&mut line) {
            Ok(0) | Err(_) => return Asked::Quit,
            Ok(_) => {}
        }
        let answer = line.trim();
        if answer.eq_ignore_ascii_case("q") {
            return Asked::Quit;
        }
        let response_ms = started.elapsed().as_millis().min(u128::from(u32::MAX)) as u32;
        match grade_timed(item, answer, response_ms) {
            Ok(outcome) => {
                return Asked::Answered { outcome: Box::new(outcome), response_ms };
            }
            Err(invalid) => {
                let _ = writeln!(stdout, "{}", invalid.0);
            }
        }
    }
}

fn run_drill(args: DrillArgs) -> Result<(), String> {
    let store = Store::open()?;
    let attempts = store.load();
    if !has_baseline(&attempts) {
        println!("No baseline on record. Consider taking one first with: poker-trainer baseline");
    }
    let allowed = drill_skills(&args.skills, &attempts);
    let limits = SessionLimits::resolve(args.minutes, args.items);
    let started = Instant::now();
    let seed = args.seed.unwrap_or_else(clock_seed);
    let mut session = Session::new(allowed, &attempts, now_ms(), seed);
    let style = style();
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut stdout = io::stdout();
    let (mut asked_count, mut ok_count) = (0u64, 0u64);

    loop {
        if limits.finished(asked_count, started.elapsed().as_millis() as u64) {
            break;
        }
        let Some(planned) = session.try_next_item() else {
            println!("\nNo anchor cards are due and no new ones can be introduced this session.");
            break;
        };
        let item = session.item_for(planned);
        let tag = match planned.origin {
            Origin::New => "",
            Origin::Due => " (review)",
            Origin::SessionRetry => " (again)",
        };
        println!("\n--- {}{tag} ---", item.skill.name());
        println!("{}", render::question(&item, style));
        let (outcome, response_ms) = match ask(&item, &mut input, &mut stdout) {
            Asked::Quit => break,
            Asked::Answered { outcome, response_ms } => (outcome, response_ms),
        };
        store.append(&Attempt::from_drill(planned, &outcome, now_ms(), response_ms));
        let ok = outcome.ok();
        session.record_timed(planned, ok, response_ms);
        asked_count += 1;
        ok_count += u64::from(ok);
        println!("{}", render::reveal(&outcome, style));
    }
    println!("\nSession done: {ok_count} of {asked_count} right. Saved to {}.", store.path.display());
    Ok(())
}

fn run_baseline() -> Result<(), String> {
    let store = Store::open()?;
    let attempts = store.load();
    let run = next_baseline_run(&attempts);
    let mut rng = Rng::new(clock_seed());
    let items = baseline::randomize(&baseline::load()?, &mut rng);
    let total = items.len();
    println!("Baseline test: {total} questions, no feedback until the end. Type q to stop early.");
    let style = style();
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut stdout = io::stdout();

    for (index, entry) in items.iter().enumerate() {
        println!("\n--- Question {} of {total} ({}) ---", index + 1, entry.item.skill.name());
        println!("{}", render::question(&entry.item, style));
        let (outcome, response_ms) = match ask(&entry.item, &mut input, &mut stdout) {
            Asked::Quit => break,
            Asked::Answered { outcome, response_ms } => (outcome, response_ms),
        };
        store.append(&Attempt::from_baseline(run, entry, &outcome, now_ms(), response_ms));
    }

    let all = store.load();
    let runs = baseline_runs(&all, baseline::BASELINE_SIZE);
    match runs.iter().position(|r| r.run == run) {
        Some(index) => print!("{}", render::baseline_report(&runs[index], &runs[..index])),
        None => println!("\nNo answers were recorded, so there is nothing to score."),
    }
    Ok(())
}

fn run_check(skill: Skill) -> Result<(), String> {
    let store = Store::open()?;
    let attempts = store.load();
    let (form, run) = (next_checkup_form(skill, &attempts), next_checkup_run(&attempts));
    let plan = checkup_plan(skill, form);
    let total = plan.slots.len();
    println!("Checkup for {}: {total} questions, form {form}, no feedback until the end. Type q to stop early.", skill.name());
    if skill == Skill::Table && table::checkup_form_reuses_spots(form) {
        println!("{}", table::REUSE_NOTE);
    }
    let style = style();
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut stdout = io::stdout();

    for index in 0..total {
        let item = plan.item(index);
        println!("\n--- Question {} of {total} ---", index + 1);
        println!("{}", render::question(&item, style));
        let (outcome, response_ms) = match ask(&item, &mut input, &mut stdout) {
            Asked::Quit => break,
            Asked::Answered { outcome, response_ms } => (outcome, response_ms),
        };
        let check = CheckRef { run, form, slot: index as u32 };
        store.append(&Attempt::from_check(check, skill, plan.slots[index].seed, &outcome, now_ms(), response_ms));
    }

    match checkup_runs(&store.load()).into_iter().find(|s| s.run == run) {
        Some(score) => print!("{}", render::checkup_report(&score)),
        None => println!("\nNo answers were recorded, so there is nothing to score."),
    }
    Ok(())
}

fn run_progress() -> Result<(), String> {
    let store = Store::open()?;
    let attempts = store.load();
    let stats = all_stats(&attempts);
    let runs = baseline_runs(&attempts, baseline::BASELINE_SIZE);
    let checkups = checkup_runs(&attempts);
    let bias = equity_bias(&attempts);
    print!("{}", render::progress_text(&stats, current_step(&stats), &runs, &checkups, &bias));
    Ok(())
}
