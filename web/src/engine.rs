//! The commands the page can run. Every command is a line of fields separated
//! by `\x1f`, and every answer is JSON. The attempts and the open session live
//! here; the page keeps only the raw log text in its own storage.

use std::cell::RefCell;
use std::collections::HashMap;

use poker_trainer::baseline::{self, BaselineItem, BASELINE_SIZE};
use poker_trainer::cards::Card;
use poker_trainer::generate::generate;
use poker_trainer::grade::grade;
use poker_trainer::item::{Item, Skill};
use poker_trainer::log::{format_attempt, parse_log, Attempt, Mode, LOG_HEADER};
use poker_trainer::preflop::{preflop_answer, Action, Seat, Situation, CARD};
use poker_trainer::progress::{
    all_stats, baseline_runs, current_step, drill_skills, has_baseline, next_baseline_run,
    pass_mark, BaselineRun, SkillStats, MIN_ATTEMPTS, PASS_ATTEMPTS, STEPS,
};
use poker_trainer::rng::Rng;
use poker_trainer::schedule::{Origin, Planned, Session, SessionLimits};

use crate::json::{obj, V};
use crate::present;

thread_local! {
    static ENGINE: RefCell<Engine> = RefCell::new(Engine::default());
}

/// Runs one command and returns its JSON answer.
pub fn dispatch(input: &str) -> String {
    // The log text is one field, whatever bytes it happens to contain.
    let fields: Vec<&str> = if input.starts_with("load\x1f") || input.starts_with("merge\x1f") {
        input.splitn(2, '\x1f').collect()
    } else {
        input.split('\x1f').collect()
    };
    let result = ENGINE.with(|engine| engine.borrow_mut().run(&fields));
    match result {
        Ok(value) => value.into_string(),
        Err(message) => obj! { "error": message }.into_string(),
    }
}

type Reply = Result<V, String>;

fn parse_u64(text: &str, name: &str) -> Result<u64, String> {
    text.parse().map_err(|_| format!("{name} is not a whole number"))
}

fn arg<'a>(fields: &[&'a str], index: usize) -> Result<&'a str, String> {
    fields.get(index).copied().ok_or_else(|| "missing field".to_string())
}

fn skill_from(text: &str) -> Result<Skill, String> {
    Skill::from_name(text).ok_or_else(|| "unknown skill".to_string())
}

fn seed_text(seed: u64) -> V {
    V::Str(seed.to_string())
}

#[derive(Default)]
struct Engine {
    attempts: Vec<Attempt>,
    session: Option<LiveSession>,
    baseline: Option<LiveBaseline>,
}

struct LiveSession {
    allowed: Vec<Skill>,
    start_len: usize,
    now_ms: u64,
    seed: u64,
    limits: SessionLimits,
    real: Session,
    current: Option<Planned>,
    /// Finished items, for rebuilding the session when looking ahead.
    done: Vec<(Planned, bool)>,
}

struct LiveBaseline {
    run: u32,
    items: Vec<BaselineItem>,
}

impl LiveSession {
    /// A copy of the session as it stands, by replaying it; [`Session`] can't be cloned.
    fn replayed(&self, attempts: &[Attempt]) -> Session {
        let start = &attempts[..self.start_len.min(attempts.len())];
        let mut copy = Session::new(self.allowed.clone(), start, self.now_ms, self.seed);
        for &(planned, ok) in &self.done {
            copy.next_item();
            copy.record(planned, ok);
        }
        copy
    }
}

fn planned_json(p: Planned) -> V {
    let origin = match p.origin {
        Origin::New => "new",
        Origin::Due => "due",
        Origin::SessionRetry => "retry",
    };
    obj! {
        "skill": p.skill.name(),
        "seed": seed_text(p.seed),
        "origin": origin,
        "title": present::skill_title(p.skill),
    }
}

/// An item with its question, answer format and every choice already graded.
fn bundle(item: &Item, make_template: &dyn Fn(&poker_trainer::grade::Outcome) -> Attempt) -> V {
    let format = poker_trainer::answer::answer_format(&item.question);
    let mut graded: Vec<(String, V)> = Vec::new();
    if let poker_trainer::answer::AnswerFormat::Choice(options) = &format {
        for option in options {
            if let Ok(outcome) = grade(item, option.code) {
                let template = format_attempt(&make_template(&outcome));
                graded.push((option.code.to_string(), present::outcome(item, &outcome, template)));
            }
        }
    }
    obj! {
        "skill": item.skill.name(),
        "seed": seed_text(item.seed),
        "title": present::skill_title(item.skill),
        "question": present::question(&item.question),
        "format": present::format(&item.question),
        "graded": V::Obj(graded),
    }
}

fn drill_template(planned: Planned, outcome: &poker_trainer::grade::Outcome) -> Attempt {
    Attempt::from_drill(planned, outcome, 0, 0)
}

fn drill_item(skill: Skill, seed: u64) -> Item {
    generate(skill, seed)
}

impl Engine {
    fn run(&mut self, fields: &[&str]) -> Reply {
        match fields.first().copied().unwrap_or("") {
            "prepare" => self.prepare(fields),
            "prepare_baseline" => self.prepare_baseline(fields),
            "load" => self.load(arg(fields, 1)?),
            "export" => Ok(obj! { "text": self.log_text() }),
            "merge" => self.merge(arg(fields, 1)?),
            "home" => Ok(self.home()),
            "progress" => Ok(self.progress()),
            "card_meta" => Ok(card_meta()),
            "card_view" => card_view(arg(fields, 1)?, arg(fields, 2)?),
            "session_open" => self.session_open(fields),
            "session_start" => self.session_start(fields),
            "session_next" => self.session_next(fields),
            "session_peek" => self.session_peek(fields),
            "session_summary" => self.session_summary(),
            "grade" => self.grade(fields),
            "commit" => self.commit(fields),
            "baseline_open" => self.baseline_open(fields),
            "baseline_grade" => self.baseline_grade(fields),
            "baseline_commit" => self.baseline_commit(fields),
            "baseline_report" => self.baseline_report(fields),
            other => Err(format!("unknown command {other}")),
        }
    }

    fn prepare(&mut self, fields: &[&str]) -> Reply {
        let skill = skill_from(arg(fields, 1)?)?;
        let seed = parse_u64(arg(fields, 2)?, "seed")?;
        let item = drill_item(skill, seed);
        let planned = Planned { skill, seed, origin: Origin::New };
        Ok(bundle(&item, &|outcome| drill_template(planned, outcome)))
    }

    fn prepare_baseline(&mut self, fields: &[&str]) -> Reply {
        let seed = parse_u64(arg(fields, 1)?, "seed")?;
        let index = parse_u64(arg(fields, 2)?, "index")? as usize;
        let items = randomized_baseline(seed)?;
        let entry = items.get(index).ok_or("no such question")?;
        Ok(bundle(&entry.item, &|outcome| Attempt::from_baseline(0, entry, outcome, 0, 0)))
    }

    fn load(&mut self, text: &str) -> Reply {
        let (attempts, warnings) = parse_log(text);
        self.attempts = attempts;
        Ok(obj! { "count": self.attempts.len(), "warnings": warnings })
    }

    fn log_text(&self) -> String {
        let mut text = String::from(LOG_HEADER);
        text.push('\n');
        for attempt in &self.attempts {
            text.push_str(&format_attempt(attempt));
            text.push('\n');
        }
        text
    }

    fn merge(&mut self, incoming: &str) -> Reply {
        let (parsed, warnings) = parse_log(incoming);
        let mut seen: HashMap<String, Option<u32>> =
            self.attempts.iter().map(|a| (merge_key(a), a.baseline_run)).collect();
        let renumbered = self.run_ids_for_import(&parsed, &seen);
        let total_in_file = parsed.len();
        let mut added = 0usize;
        for mut attempt in parsed {
            if let Some(&target) = attempt.baseline_run.and_then(|run| renumbered.get(&run)) {
                attempt.baseline_run = Some(target);
            }
            if seen.insert(merge_key(&attempt), attempt.baseline_run).is_none() {
                self.attempts.push(attempt);
                added += 1;
            }
        }
        self.attempts.sort_by_key(|a| a.timestamp_ms);
        Ok(obj! {
            "added": added,
            "duplicates": total_in_file - added,
            "total": self.attempts.len(),
            "warnings": warnings,
            "text": self.log_text(),
        })
    }

    /// Baseline run numbers in an import that must change. A run with a line we
    /// already hold is the same run, so it takes that line's number. A run with
    /// none of our lines whose number is already used came from another device,
    /// so it gets a new number.
    fn run_ids_for_import(
        &self,
        parsed: &[Attempt],
        seen: &HashMap<String, Option<u32>>,
    ) -> HashMap<u32, u32> {
        let mut runs: Vec<u32> = parsed.iter().filter_map(|a| a.baseline_run).collect();
        runs.sort_unstable();
        runs.dedup();
        let highest = self
            .attempts
            .iter()
            .chain(parsed)
            .filter_map(|a| a.baseline_run)
            .max()
            .unwrap_or(0);
        let mut next = highest + 1;
        let mut map = HashMap::new();
        for run in runs {
            let already_held = parsed
                .iter()
                .filter(|a| a.baseline_run == Some(run))
                .find_map(|a| seen.get(&merge_key(a)).copied().flatten());
            if let Some(local) = already_held {
                map.insert(run, local);
            } else if self.attempts.iter().any(|a| a.baseline_run == Some(run)) {
                map.insert(run, next);
                next += 1;
            }
        }
        map
    }

    fn home(&self) -> V {
        let stats = all_stats(&self.attempts);
        let step = current_step(&stats);
        let passed = |skill: Skill| stats.iter().any(|s| s.skill == skill && s.passed);
        let rotation: Vec<&str> = drill_skills(&[], &self.attempts)
            .into_iter()
            .map(present::skill_title)
            .collect();
        let steps: Vec<V> = STEPS
            .iter()
            .enumerate()
            .map(|(i, skills)| {
                let list: Vec<V> = skills
                    .iter()
                    .map(|&s| obj! { "title": present::skill_title(s), "passed": passed(s) })
                    .collect();
                obj! {
                    "step": i + 1,
                    "current": i + 1 == step,
                    "unlocked": i < step,
                    "passed": skills.iter().all(|&s| passed(s)),
                    "skills": list,
                }
            })
            .collect();
        obj! {
            "has_baseline": has_baseline(&self.attempts),
            "baseline_size": BASELINE_SIZE,
            "step": step,
            "step_count": STEPS.len(),
            "steps": steps,
            "preflop_passed": passed(Skill::Preflop),
            "rotation": rotation,
        }
    }

    fn progress(&self) -> V {
        let stats = all_stats(&self.attempts);
        let skills: Vec<V> = stats.iter().map(stats_json).collect();
        let step = current_step(&stats);
        let runs = baseline_runs(&self.attempts, BASELINE_SIZE);
        let names: Vec<&str> = STEPS[step - 1].iter().map(|&s| present::skill_title(s)).collect();
        obj! {
            "skills": skills,
            "step": step,
            "step_count": STEPS.len(),
            "step_skills": names,
            "runs": runs.iter().map(run_json).collect::<Vec<_>>(),
        }
    }

    fn session_open(&mut self, fields: &[&str]) -> Reply {
        let seed = parse_u64(arg(fields, 1)?, "seed")?;
        let now_ms = parse_u64(arg(fields, 2)?, "now")?;
        let allowed = drill_skills(&[], &self.attempts);
        let start_len = self.attempts.len();
        let real = Session::new(allowed.clone(), &self.attempts, now_ms, seed);
        let limits = SessionLimits::resolve(None, None);
        self.session = Some(LiveSession {
            allowed,
            start_len,
            now_ms,
            seed,
            limits,
            real,
            current: None,
            done: Vec::new(),
        });
        // The first item, so the page can have it ready before Start is pressed.
        let mut preview = self.live()?.replayed(&self.attempts);
        Ok(obj! { "first": planned_json(preview.next_item()) })
    }

    fn live(&self) -> Result<&LiveSession, String> {
        self.session.as_ref().ok_or_else(|| "no session is open".to_string())
    }

    fn session_start(&mut self, fields: &[&str]) -> Reply {
        let minutes = parse_u64(arg(fields, 1)?, "minutes")?;
        let live = self.session.as_mut().ok_or("no session is open")?;
        live.limits = SessionLimits::resolve(Some(minutes), None);
        Ok(obj! { "minutes": minutes as u32 })
    }

    fn session_next(&mut self, fields: &[&str]) -> Reply {
        let elapsed = parse_u64(arg(fields, 1)?, "elapsed")?;
        let live = self.session.as_mut().ok_or("no session is open")?;
        if live.limits.finished(live.done.len() as u64, elapsed) {
            return Ok(obj! { "finished": true });
        }
        let planned = live.real.next_item();
        live.current = Some(planned);
        Ok(obj! { "planned": planned_json(planned), "answered": live.done.len() })
    }

    /// The item that would follow the current one if it were answered `ok`.
    fn session_peek(&mut self, fields: &[&str]) -> Reply {
        let ok = arg(fields, 1)? == "1";
        let attempts = &self.attempts;
        let live = self.session.as_ref().ok_or("no session is open")?;
        let mut copy = live.replayed(attempts);
        let current = copy.next_item();
        copy.record(current, ok);
        Ok(obj! { "planned": planned_json(copy.next_item()) })
    }

    fn session_summary(&self) -> Reply {
        let live = self.live()?;
        let mut rows: Vec<(Skill, usize, usize)> = Vec::new();
        let mut ok_total = 0;
        for &(planned, ok) in &live.done {
            if !rows.iter().any(|r| r.0 == planned.skill) {
                rows.push((planned.skill, 0, 0));
            }
            let row = rows.iter_mut().find(|r| r.0 == planned.skill).expect("added above");
            row.1 += 1;
            row.2 += usize::from(ok);
            ok_total += usize::from(ok);
        }
        let by_skill: Vec<V> = rows
            .iter()
            .map(|&(skill, answered, ok)| {
                obj! { "title": present::skill_title(skill), "answered": answered, "ok": ok }
            })
            .collect();
        Ok(obj! { "answered": live.done.len(), "ok": ok_total, "by_skill": by_skill })
    }

    fn grade(&mut self, fields: &[&str]) -> Reply {
        let skill = skill_from(arg(fields, 1)?)?;
        let seed = parse_u64(arg(fields, 2)?, "seed")?;
        let raw = arg(fields, 3)?;
        let item = drill_item(skill, seed);
        match grade(&item, raw) {
            Ok(outcome) => {
                let planned = Planned { skill, seed, origin: Origin::New };
                let template = format_attempt(&drill_template(planned, &outcome));
                Ok(obj! { "outcome": present::outcome(&item, &outcome, template) })
            }
            Err(invalid) => Ok(obj! { "invalid": invalid.0 }),
        }
    }

    fn attempt_from_template(template: &str, now: u64, response_ms: u32) -> Result<Attempt, String> {
        let (mut parsed, _) = parse_log(template);
        let mut attempt = parsed.pop().ok_or("the grading result could not be read")?;
        attempt.timestamp_ms = now;
        attempt.response_ms = response_ms;
        Ok(attempt)
    }

    fn commit(&mut self, fields: &[&str]) -> Reply {
        let now = parse_u64(arg(fields, 2)?, "now")?;
        let response = parse_u64(arg(fields, 3)?, "response")?.min(u64::from(u32::MAX)) as u32;
        let mut attempt = Self::attempt_from_template(arg(fields, 1)?, now, response)?;
        let live = self.session.as_mut().ok_or("no session is open")?;
        let planned = live.current.take().ok_or("no item is waiting for an answer")?;
        attempt.mode = if planned.origin == Origin::SessionRetry {
            Mode::DrillRetry
        } else {
            Mode::Drill
        };
        let ok = attempt.ok();
        let line = format_attempt(&attempt);
        self.attempts.push(attempt);
        live.real.record(planned, ok);
        live.done.push((planned, ok));
        Ok(obj! { "line": line, "ok": ok, "answered": live.done.len() })
    }

    fn baseline_open(&mut self, fields: &[&str]) -> Reply {
        let seed = parse_u64(arg(fields, 1)?, "seed")?;
        let items = randomized_baseline(seed)?;
        let run = next_baseline_run(&self.attempts);
        let count = items.len();
        self.baseline = Some(LiveBaseline { run, items });
        Ok(obj! { "run": run, "count": count })
    }

    fn baseline_entry(&self, index: usize) -> Result<&BaselineItem, String> {
        let live = self.baseline.as_ref().ok_or("no baseline is open")?;
        live.items.get(index).ok_or_else(|| "no such question".to_string())
    }

    fn baseline_grade(&mut self, fields: &[&str]) -> Reply {
        let index = parse_u64(arg(fields, 1)?, "index")? as usize;
        let raw = arg(fields, 2)?;
        let entry = self.baseline_entry(index)?;
        match grade(&entry.item, raw) {
            Ok(outcome) => {
                let template = format_attempt(&Attempt::from_baseline(0, entry, &outcome, 0, 0));
                Ok(obj! { "outcome": present::outcome(&entry.item, &outcome, template) })
            }
            Err(invalid) => Ok(obj! { "invalid": invalid.0 }),
        }
    }

    fn baseline_commit(&mut self, fields: &[&str]) -> Reply {
        let now = parse_u64(arg(fields, 2)?, "now")?;
        let response = parse_u64(arg(fields, 3)?, "response")?.min(u64::from(u32::MAX)) as u32;
        let mut attempt = Self::attempt_from_template(arg(fields, 1)?, now, response)?;
        let run = self.baseline.as_ref().ok_or("no baseline is open")?.run;
        attempt.baseline_run = Some(run);
        attempt.mode = Mode::Baseline;
        let line = format_attempt(&attempt);
        self.attempts.push(attempt);
        Ok(obj! { "line": line })
    }

    fn baseline_report(&self, fields: &[&str]) -> Reply {
        let run_id = parse_u64(arg(fields, 1)?, "run")? as u32;
        let runs = baseline_runs(&self.attempts, BASELINE_SIZE);
        match runs.iter().position(|r| r.run == run_id) {
            Some(index) => Ok(obj! {
                "report": run_json(&runs[index]),
                "earlier": runs[..index].iter().map(run_json).collect::<Vec<_>>(),
            }),
            None => Ok(obj! { "report": V::Null, "earlier": V::Arr(Vec::new()) }),
        }
    }
}

/// Identifies an attempt across devices. The run number is left out because the
/// same attempt can carry a different number after an earlier import.
fn merge_key(attempt: &Attempt) -> String {
    let mut plain = attempt.clone();
    plain.baseline_run = None;
    format_attempt(&plain)
}

fn randomized_baseline(seed: u64) -> Result<Vec<BaselineItem>, String> {
    let items = baseline::load()?;
    let mut rng = Rng::new(seed);
    Ok(baseline::randomize(&items, &mut rng))
}

fn mark_lines(stats: &SkillStats) -> Vec<String> {
    let mark = pass_mark(stats.skill);
    let mut parts = Vec::new();
    if let Some(m) = mark.min_correct_pct {
        parts.push(format!("at least {m:.0}% correct"));
    }
    if let Some(m) = mark.min_within_one_pct {
        parts.push(format!("at least {m:.0}% within 1"));
    }
    if let Some(m) = mark.max_median_ms {
        parts.push(format!("median at most {} s", m / 1000));
    }
    if let Some(m) = mark.max_mean_ev_loss_pct {
        parts.push(format!("mean EV loss at most {m:.0}% of pot"));
    }
    if let Some(m) = mark.max_blunders {
        parts.push(format!("at most {m} blunders"));
    }
    parts
}

fn stat_lines(stats: &SkillStats) -> Vec<String> {
    let mark = pass_mark(stats.skill);
    let mut parts = vec![format!("Last {}", stats.window)];
    if mark.min_correct_pct.is_some() {
        parts.push(format!("{:.0}% correct", stats.correct_pct));
    }
    if mark.min_within_one_pct.is_some() {
        parts.push(format!("{:.0}% within 1 ({:.0}% exact)", stats.within_one_pct, stats.correct_pct));
    }
    if mark.max_median_ms.is_some() {
        parts.push(format!("median {:.1} s", f64::from(stats.median_ms) / 1000.0));
    }
    if mark.max_mean_ev_loss_pct.is_some() {
        parts.push(format!("mean EV loss {:.1}% of pot", stats.mean_ev_loss_pct));
    }
    if mark.max_blunders.is_some() {
        parts.push(format!("{} blunders", stats.blunders));
    }
    parts
}

fn stats_json(s: &SkillStats) -> V {
    let status = if s.passed {
        "Passed.".to_string()
    } else if s.total_attempts < PASS_ATTEMPTS {
        format!("Not passed yet: needs {PASS_ATTEMPTS} attempts, has {}.", s.total_attempts)
    } else {
        "Not passed yet.".to_string()
    };
    obj! {
        "title": present::skill_title(s.skill),
        "step": s.skill.step().map(u32::from),
        "enough_data": s.enough_data,
        "total": s.total_attempts,
        "needed": MIN_ATTEMPTS,
        "passed": s.passed,
        "stats": stat_lines(s),
        "mark": mark_lines(s),
        "status": status,
    }
}

fn run_json(run: &BaselineRun) -> V {
    let sections: Vec<V> = run
        .sections
        .iter()
        .map(|s| {
            obj! {
                "title": present::skill_title(s.skill),
                "ok": s.ok,
                "answered": s.answered,
                "exact": s.exact,
                "show_exact": s.ok != s.exact,
            }
        })
        .collect();
    let steps: Vec<V> = run
        .steps
        .iter()
        .map(|s| obj! { "step": s.step, "ok": s.ok, "answered": s.answered })
        .collect();
    obj! {
        "run": run.run,
        "started_ms": V::Str(run.started_ms.to_string()),
        "ok": run.ok,
        "answered": run.answered,
        "complete": run.complete,
        "sections": sections,
        "steps": steps,
    }
}

fn seat_from(code: &str) -> Result<Seat, String> {
    Seat::ALL.into_iter().find(|s| s.code() == code).ok_or_else(|| "unknown seat".to_string())
}

fn card_meta() -> V {
    let seats: Vec<V> = Seat::ALL
        .iter()
        .map(|&s| obj! { "code": s.code(), "title": present::seat_title(s) })
        .collect();
    let situations: Vec<V> = [("folded", "Folded to you"), ("limpers", "Limpers"), ("raised", "Someone raised")]
        .into_iter()
        .map(|(code, title)| obj! { "code": code, "title": title })
        .collect();
    let notes: Vec<String> = CARD.notes().iter().map(|n| present::plain_note(n.text)).collect();
    let header: Vec<&str> = CARD
        .header
        .lines()
        .map(|line| line.trim_start_matches('#').trim_start())
        .filter(|line| !line.is_empty())
        .collect();
    obj! { "seats": seats, "situations": situations, "notes": notes, "header": header }
}

/// One rank in a cell label; `Card` itself writes the rank in upper case.
fn rank_letter(rank: u8) -> char {
    poker_trainer::cards::rank_char(rank)
}

fn card_view(seat_code: &str, situation_code: &str) -> Reply {
    let seat = seat_from(seat_code)?;
    let situation = match situation_code {
        "folded" => Situation::FoldedTo,
        "limpers" => Situation::Limpers(1),
        "raised" => Situation::Raised,
        _ => return Err("unknown situation".to_string()),
    };
    let mut codes_seen: Vec<char> = Vec::new();
    let mut grid: Vec<V> = Vec::with_capacity(13);
    for row in (0..13u8).rev() {
        let mut cells: Vec<V> = Vec::with_capacity(13);
        for col in (0..13u8).rev() {
            let (a, b) = if col > row { (col, row) } else { (row, col) };
            let hand = if col == row {
                [Card::new(a, 0), Card::new(b, 1)]
            } else if col < row {
                [Card::new(a, 0), Card::new(b, 0)]
            } else {
                [Card::new(a, 0), Card::new(b, 1)]
            };
            let mut label = String::new();
            label.push(rank_letter(a));
            label.push(rank_letter(b));
            if col < row {
                label.push('s');
            } else if col > row {
                label.push('o');
            }
            let (action, _) = preflop_answer(seat, situation, hand);
            let code = match action {
                Action::Raise => 'r',
                Action::Call => 'c',
                Action::Fold => {
                    let looser = matches!(situation, Situation::Limpers(_))
                        && preflop_answer(seat, Situation::Limpers(2), hand).0 == Action::Call;
                    if looser {
                        'l'
                    } else {
                        'f'
                    }
                }
            };
            if !codes_seen.contains(&code) {
                codes_seen.push(code);
            }
            cells.push(obj! { "t": label, "a": code.to_string() });
        }
        grid.push(V::Arr(cells));
    }
    let lines = card_lines(seat, situation);
    let legend: Vec<V> = ['r', 'c', 'l', 'f']
        .into_iter()
        .filter(|c| codes_seen.contains(c))
        .map(|code| obj! { "code": code.to_string(), "label": legend_label(code, seat, situation) })
        .collect();
    Ok(obj! { "grid": grid, "lines": lines, "legend": legend })
}

fn legend_label(code: char, seat: Seat, situation: Situation) -> String {
    let word = |action| poker_trainer::preflop::action_label(action, seat, situation);
    match code {
        'r' => match situation {
            Situation::Raised => "Reraise".to_string(),
            _ => "Raise".to_string(),
        },
        'c' => {
            let mut text = word(Action::Call).to_string();
            text[..1].make_ascii_uppercase();
            text
        }
        'l' => "Limp along if two or more have limped".to_string(),
        _ => "Fold".to_string(),
    }
}

fn card_lines(seat: Seat, situation: Situation) -> Vec<V> {
    let line = |name: String, card_line: &poker_trainer::preflop::CardLine| {
        obj! {
            "name": name,
            "pct": format!("{:.1}% of hands", card_line.range.percent_of_hands()),
            "combos": format!("{} combos", card_line.range.combo_count()),
            "text": card_line.text,
        }
    };
    match situation {
        Situation::FoldedTo => CARD
            .open_range(seat)
            .map(|l| vec![line(format!("{} opening range", present::seat_title(seat)), l)])
            .unwrap_or_default(),
        Situation::Limpers(_) => vec![
            line("Raise-over-limpers range".to_string(), &CARD.iso_raise),
            line("Limp-along range".to_string(), &CARD.iso_limp),
        ],
        Situation::Raised => vec![
            line("Reraise range".to_string(), &CARD.reraise),
            line("Calling range".to_string(), &CARD.call),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::dispatch;

    fn line(ts: u32, run: u32, item: &str) -> String {
        format!("{ts}\tbaseline\t{run}\tnuts\t{item}\t1\t1\t0\t\t1000\tqj\n")
    }

    fn load(text: &str) {
        assert!(dispatch(&format!("load\x1f{text}")).contains("\"count\""));
    }

    fn merge(text: &str) -> String {
        dispatch(&format!("merge\x1f{text}"))
    }

    fn added(reply: &str) -> usize {
        let start = reply.find("\"added\":").unwrap() + 8;
        reply[start..].split(',').next().unwrap().parse().unwrap()
    }

    /// (timestamp, run) for every line in the log.
    fn runs() -> Vec<(u32, String)> {
        let reply = dispatch("export");
        let text = reply.replace("\\t", "\t").replace("\\n", "\n");
        text.lines()
            .filter(|l| l.contains("baseline"))
            .filter(|l| !l.contains("timestamp_ms"))
            .map(|l| {
                let f: Vec<&str> = l.split('\t').collect();
                let ts = f[0].trim_start_matches("{\"text\":\"").parse().unwrap();
                (ts, f[2].to_string())
            })
            .collect()
    }

    #[test]
    fn a_run_from_another_device_gets_a_new_number() {
        load(&(line(1, 1, "b01") + &line(2, 1, "b02")));
        let file = line(3, 1, "b01") + &line(4, 1, "b02");
        assert_eq!(added(&merge(&file)), 2);
        let expected = [(1, "1"), (2, "1"), (3, "2"), (4, "2")];
        assert_eq!(runs(), expected.map(|(t, r)| (t, r.to_string())).to_vec());
    }

    #[test]
    fn importing_the_same_file_twice_adds_nothing() {
        load(&(line(1, 1, "b01") + &line(2, 1, "b02")));
        let file = line(3, 1, "b01") + &line(4, 1, "b02");
        merge(&file);
        assert_eq!(added(&merge(&file)), 0);
        assert_eq!(runs().len(), 4);
        assert!(runs().iter().all(|(_, r)| r == "1" || r == "2"), "{:?}", runs());
    }

    #[test]
    fn a_partly_duplicate_run_keeps_its_number() {
        load(&line(1, 1, "b01"));
        assert_eq!(added(&merge(&(line(1, 1, "b01") + &line(2, 1, "b02")))), 1);
        let expected = [(1, "1"), (2, "1")];
        assert_eq!(runs(), expected.map(|(t, r)| (t, r.to_string())).to_vec());
    }

    #[test]
    fn a_partly_duplicate_run_joins_the_number_it_was_given() {
        load(&line(1, 1, "b01"));
        merge(&(line(2, 1, "b01")));
        // Run 1 from the other device, now stored as run 2, gains a line.
        assert_eq!(added(&merge(&(line(2, 1, "b01") + &line(3, 1, "b02")))), 1);
        let expected = [(1, "1"), (2, "2"), (3, "2")];
        assert_eq!(runs(), expected.map(|(t, r)| (t, r.to_string())).to_vec());
    }

    #[test]
    fn a_run_new_to_this_device_keeps_its_number() {
        load("");
        merge(&line(1, 5, "b01"));
        assert_eq!(runs(), vec![(1, "5".to_string())]);
    }

    #[test]
    fn fresh_numbers_avoid_other_runs_in_the_same_file() {
        load(&(line(1, 1, "b01") + &line(2, 2, "b01")));
        merge(&(line(3, 1, "b02") + &line(4, 2, "b02") + &line(5, 7, "b03")));
        let expected = [(1, "1"), (2, "2"), (3, "8"), (4, "9"), (5, "7")];
        assert_eq!(runs(), expected.map(|(t, r)| (t, r.to_string())).to_vec());
    }

    #[test]
    fn a_separator_byte_does_not_truncate_an_import() {
        load("");
        let odd = "1\tdrill\t\tprice\t7\t1\t1\t0\t\t1000\ta\x1fb\n";
        let reply = merge(&format!("{odd}2\tdrill\t\tprice\t8\t1\t1\t0\t\t1000\tc\n"));
        assert_eq!(added(&reply), 2, "{reply}");
    }

    #[test]
    fn a_separator_byte_does_not_truncate_a_load() {
        let odd = "1\tdrill\t\tprice\t7\t1\t1\t0\t\t1000\ta\x1fb\n";
        let reply = dispatch(&format!("load\x1f{odd}2\tdrill\t\tprice\t8\t1\t1\t0\t\t1000\tc\n"));
        assert!(reply.contains("\"count\":2"), "{reply}");
    }
}
