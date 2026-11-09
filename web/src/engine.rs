//! The commands the page can run. Every command is a line of fields separated
//! by `\x1f`, and every answer is JSON. The attempts and the open session live
//! here; the page keeps only the raw log text in its own storage.

use std::cell::RefCell;
use std::collections::HashMap;

use poker_trainer::anchors;
use poker_trainer::baseline::{self, BaselineItem, BASELINE_SIZE};
use poker_trainer::board_type::board_type_status;
use poker_trainer::cards::Card;
use poker_trainer::checkup::{
    checkup_plan, checkup_runs, next_checkup_form, next_checkup_run, CheckupScore, CHECKUP_LEN,
    CHECKUP_ORIGINALS, CHECKUP_REPEATS,
};
use poker_trainer::equity_skill::{equity_band, equity_bias, equity_rung, BIAS_MIN_ATTEMPTS};
use poker_trainer::generate::generate;
use poker_trainer::grade::{grade_choices, grade_timed};
use poker_trainer::item::{Item, Skill};
use poker_trainer::log::{asks_confidence, format_attempt, parse_log, Attempt, CheckRef, Confidence, ItemRef, Mode, LOG_HEADER};
use poker_trainer::preflop::{chart_for, preflop_answer, Action, Seat, Situation, CARD};
use poker_trainer::progress::{
    all_stats, baseline_runs, current_step_for, drill_skills, has_baseline, next_baseline_run,
    pass_mark, reliability_table, BaselineRun, SkillStats, WindowMetrics, MIN_ATTEMPTS, PASS_ATTEMPTS, STEPS,
};
use poker_trainer::reflection::{reflection_comparison, reflection_form, reflection_row, reflection_state, ReflectState};
use poker_trainer::read_price::{read_price_status, READ_PRICE_PROMOTE_PCT, READ_PRICE_PROMOTE_WINDOW};
use poker_trainer::rng::Rng;
use poker_trainer::table;
use poker_trainer::schedule::{
    anchor_status, anchors_due, Origin, Planned, Session, SessionLimits, LEITNER_INTERVAL_DAYS,
};

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
    limits: SessionLimits,
    real: Session,
    current: Option<Planned>,
    /// Finished items, for the session summary.
    done: Vec<(Planned, bool, u32)>,
}

struct LiveBaseline {
    run: u32,
    items: Vec<BaselineItem>,
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
/// `board_type` is left ungraded: its answer depends on the response time, so
/// it is graded when the answer arrives. `table` is left ungraded so the page
/// holds no EV, verdict or best option before the player commits.
fn bundle(item: &Item, origin: Option<Origin>, make_template: &dyn Fn(&poker_trainer::grade::Outcome) -> Attempt) -> V {
    let format = poker_trainer::answer::answer_format(&item.question);
    let mut graded: Vec<(String, V)> = Vec::new();
    if !matches!(item.skill, Skill::BoardType | Skill::Table) {
        if let poker_trainer::answer::AnswerFormat::Choice(options) = &format {
            let codes: Vec<&str> = options.iter().map(|o| o.code).collect();
            for (option, result) in options.iter().zip(grade_choices(item, &codes)) {
                if let Ok(outcome) = result {
                    graded.push((option.code.to_string(), graded_outcome(item, &outcome, origin, make_template(&outcome), 0)));
                }
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
        "confidence": (asks_confidence(item.skill) && origin.is_some()).then(present::confidence_levels),
    }
}

/// An answer's reply. `origin` is set for drills, the only mode with a reveal to
/// reflect on; the template is stamped with what the reflection column should
/// say, and the form is sent only when the seed offers it.
fn graded_outcome(item: &Item, outcome: &poker_trainer::grade::Outcome, origin: Option<Origin>, mut template: Attempt, response: u32) -> V {
    let state = origin.and_then(|origin| reflection_state(item, outcome, origin));
    template.extra.reflect = state;
    let form = (state == Some(ReflectState::Offered))
        .then(|| reflection_form(item, outcome))
        .flatten()
        .map_or(V::Null, |prompts| present::reflection_form(&prompts));
    present::outcome(item, outcome, format_attempt(&template), response, form)
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
            "prepare_check" => self.prepare_check(fields),
            "load" => self.load(arg(fields, 1)?),
            "export" => Ok(obj! { "text": self.log_text() }),
            "merge" => self.merge(arg(fields, 1)?),
            "home" => Ok(self.home()),
            "progress" => Ok(self.progress(fields)),
            "card_meta" => Ok(card_meta()),
            "card_view" => card_view(arg(fields, 1)?, arg(fields, 2)?),
            "session_open" => self.session_open(fields),
            "session_start" => self.session_start(fields),
            "session_next" => self.session_next(fields),
            "session_peek" => self.session_peek(fields),
            "session_summary" => self.session_summary(),
            "grade" => self.grade(fields),
            "commit" => self.commit(fields),
            "reflect_commit" => self.reflect_commit(fields),
            "baseline_open" => self.baseline_open(fields),
            "baseline_grade" => self.baseline_grade(fields),
            "baseline_commit" => self.baseline_commit(fields),
            "baseline_report" => self.baseline_report(fields),
            "check_info" => self.check_info(fields),
            "check_begin" => self.check_begin(fields),
            "check_next" => self.check_next(fields),
            "check_grade" => self.check_grade(fields),
            "check_commit" => self.check_commit(fields),
            "check_report" => self.check_report(fields),
            other => Err(format!("unknown command {other}")),
        }
    }

    fn prepare(&mut self, fields: &[&str]) -> Reply {
        let skill = skill_from(arg(fields, 1)?)?;
        let seed = parse_u64(arg(fields, 2)?, "seed")?;
        let item = drill_item(skill, seed);
        let origin = fields.get(3).map_or(Origin::New, |text| origin_from(text));
        let planned = Planned { skill, seed, origin };
        Ok(bundle(&item, Some(origin), &|outcome| drill_template(planned, outcome)))
    }

    fn prepare_baseline(&mut self, fields: &[&str]) -> Reply {
        let seed = parse_u64(arg(fields, 1)?, "seed")?;
        let index = parse_u64(arg(fields, 2)?, "index")? as usize;
        let items = randomized_baseline(seed)?;
        let entry = items.get(index).ok_or("no such question")?;
        Ok(bundle(&entry.item, None, &|outcome| Attempt::from_baseline(0, entry, outcome, 0, 0)))
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
        let mut seen: HashMap<String, SeenRuns> =
            self.attempts.iter().map(|a| (merge_key(a), seen_runs(a))).collect();
        let baseline_map = self.run_ids_for_import(&parsed, &seen, |a| a.baseline_run, |s| s.baseline);
        let check_map = self.run_ids_for_import(&parsed, &seen, check_run, |s| s.check);
        let total_in_file = parsed.len();
        let mut added = 0usize;
        for mut attempt in parsed {
            if let Some(&target) = attempt.baseline_run.and_then(|run| baseline_map.get(&run)) {
                attempt.baseline_run = Some(target);
            }
            if let Some(check) = attempt.extra.check.as_mut() {
                if let Some(&target) = check_map.get(&check.run) {
                    check.run = target;
                }
            }
            if seen.insert(merge_key(&attempt), seen_runs(&attempt)).is_none() {
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

    /// Baseline or checkup run numbers in an import that must change. A run with
    /// a line we already hold is the same run, so it takes that line's number. A
    /// run with none of our lines whose number is already used came from another
    /// device, so it gets a new number.
    fn run_ids_for_import(
        &self,
        parsed: &[Attempt],
        seen: &HashMap<String, SeenRuns>,
        get: impl Fn(&Attempt) -> Option<u32>,
        held: impl Fn(&SeenRuns) -> Option<u32>,
    ) -> HashMap<u32, u32> {
        let mut runs: Vec<u32> = parsed.iter().filter_map(&get).collect();
        runs.sort_unstable();
        runs.dedup();
        let highest = self.attempts.iter().chain(parsed).filter_map(&get).max().unwrap_or(0);
        let mut next = highest + 1;
        let mut map = HashMap::new();
        for run in runs {
            let already_held = parsed
                .iter()
                .filter(|a| get(a) == Some(run))
                .find_map(|a| seen.get(&merge_key(a)).and_then(&held));
            if let Some(local) = already_held {
                map.insert(run, local);
            } else if self.attempts.iter().any(|a| get(a) == Some(run)) {
                map.insert(run, next);
                next += 1;
            }
        }
        map
    }

    fn home(&self) -> V {
        let stats = all_stats(&self.attempts);
        let checkups = checkup_runs(&self.attempts);
        let step = current_step_for(&self.attempts);
        let rotation: Vec<&str> = drill_skills(&[], &self.attempts)
            .into_iter()
            .map(present::skill_title)
            .collect();
        obj! {
            "has_baseline": has_baseline(&self.attempts),
            "baseline_size": BASELINE_SIZE,
            "step": step,
            "step_count": STEPS.len(),
            "steps": steps_json(&stats, step),
            "rotation": rotation,
            "checkups": checkup_actions(&stats, &checkups),
        }
    }

    fn progress(&self, fields: &[&str]) -> V {
        let now_ms = fields.get(1).and_then(|t| t.parse().ok()).unwrap_or(0);
        let stats = all_stats(&self.attempts);
        let checkups = checkup_runs(&self.attempts);
        let skills: Vec<V> = stats.iter().map(|s| stats_json(s, &checkups)).collect();
        let step = current_step_for(&self.attempts);
        let runs = baseline_runs(&self.attempts, BASELINE_SIZE);
        let names: Vec<&str> = STEPS[step - 1].iter().map(|&s| present::skill_title(s)).collect();
        obj! {
            "skills": skills,
            "step": step,
            "step_count": STEPS.len(),
            "step_skills": names,
            "steps": steps_json(&stats, step),
            "runs": runs.iter().map(run_json).collect::<Vec<_>>(),
            "checkups": checkups.iter().map(checkup_json).collect::<Vec<_>>(),
            "board_types": self.board_types_json(),
            "equity": self.equity_json(),
            "read_price": self.read_price_json(),
            "anchors": self.anchors_json(now_ms),
            "reflection": present::reflection_comparison(&reflection_comparison(&self.attempts)),
            "reliability": present::reliability(&reliability_table(&self.attempts)),
        }
    }

    fn board_types_json(&self) -> V {
        let status = board_type_status(&self.attempts);
        let total: usize = status.iter().map(|s| s.attempts).sum();
        let rows: Vec<V> = status
            .iter()
            .map(|s| {
                obj! {
                    "label": s.category.label(),
                    "attempts": s.attempts,
                    "correct": s.correct,
                    "streak": s.streak,
                    "status": if s.retired { "Solid" } else if s.attempts == 0 { "Not yet" } else { "Learning" },
                }
            })
            .collect();
        obj! { "enough_data": total >= MIN_ATTEMPTS, "attempts": total, "needed": MIN_ATTEMPTS, "rows": rows }
    }

    /// The player's rung and recent accuracy at it, for the Progress panel.
    fn read_price_json(&self) -> V {
        let status = read_price_status(&self.attempts);
        let recent = (status.recent_total > 0).then(|| {
            format!(
                "Last {} at this rung: {} right ({:.0}%).",
                status.recent_total,
                status.recent_right,
                100.0 * status.recent_right as f64 / status.recent_total as f64
            )
        });
        obj! {
            "rung": u32::from(status.rung) + 1,
            "rungs": u32::from(poker_trainer::item::READ_PRICE_RUNGS),
            "label": poker_trainer::read_price::rung_label(status.rung),
            "recent": recent,
            "rule": if status.rung + 1 == poker_trainer::item::READ_PRICE_RUNGS {
                "This is the top rung.".to_string()
            } else {
                format!("Moves up when {READ_PRICE_PROMOTE_WINDOW} straight attempts at this rung reach {READ_PRICE_PROMOTE_PCT:.0}%.")
            },
        }
    }

    fn equity_json(&self) -> V {
        let rung = equity_rung(&self.attempts);
        let rung_label = |r: u8| if r == 0 { "exact hands" } else { "hand types" };
        let bias = equity_bias(&self.attempts);
        let enough = bias.count >= BIAS_MIN_ATTEMPTS;
        obj! {
            "rung": u32::from(rung) + 1,
            "rung_label": rung_label(rung),
            "bands": vec![
                format!("Exact hands: within {} points counts.", equity_band(&self.attempts, 0)),
                format!("Hand types: within {} points counts.", equity_band(&self.attempts, 1)),
            ],
            "bias": obj! {
                "enough_data": enough,
                "count": bias.count,
                "needed": BIAS_MIN_ATTEMPTS,
                "overall": enough.then(|| present::overall_bias(bias.mean_signed_error)),
                "by_opponents": bias.by_opponents.iter().filter(|b| b.count > 0)
                    .map(present::bias_row).collect::<Vec<_>>(),
                "by_truth": bias.by_truth_band.iter().filter(|b| b.count > 0)
                    .map(present::bias_row).collect::<Vec<_>>(),
            },
        }
    }

    fn anchors_json(&self, now_ms: u64) -> V {
        let status = anchor_status(&self.attempts);
        let seen = status.iter().filter(|s| s.due_at_ms.is_some()).count();
        let boxes: Vec<V> = LEITNER_INTERVAL_DAYS
            .iter()
            .enumerate()
            .map(|(i, days)| {
                let count = status
                    .iter()
                    .filter(|s| s.due_at_ms.is_some() && usize::from(s.leitner_box) == i + 1)
                    .count();
                let every = if *days == 1 { "day".to_string() } else { format!("{days} days") };
                obj! { "label": format!("Box {} (every {every})", i + 1), "count": count }
            })
            .collect();
        obj! {
            "deck": anchors::deck().len(),
            "seen": seen,
            "new": status.len() - seen,
            "due": anchors_due(&self.attempts, now_ms).len(),
            "boxes": boxes,
        }
    }

    fn session_open(&mut self, fields: &[&str]) -> Reply {
        let seed = parse_u64(arg(fields, 1)?, "seed")?;
        let now_ms = parse_u64(arg(fields, 2)?, "now")?;
        // An optional comma-separated list narrows the deal to those skills.
        let requested: Vec<Skill> = match fields.get(3).copied().unwrap_or("") {
            "" => Vec::new(),
            list => list.split(',').map(skill_from).collect::<Result<_, _>>()?,
        };
        let allowed = drill_skills(&requested, &self.attempts);
        let mut real = Session::new(allowed, &self.attempts, now_ms, seed);
        if requested.is_empty() {
            real = real.with_preflop_floor();
        }
        let limits = SessionLimits::resolve(None, None);
        self.session = Some(LiveSession {
            limits,
            real,
            current: None,
            done: Vec::new(),
        });
        // The first item, so the page can have it ready before Start is pressed.
        let mut preview = self.live()?.real.clone();
        Ok(obj! { "first": preview.try_next_item().map(planned_json) })
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
        // A hand in progress finishes before the session's time runs out.
        if !live.real.chain_in_progress() && live.limits.finished(live.done.len() as u64, elapsed) {
            return Ok(obj! { "finished": true });
        }
        let Some(planned) = live.real.try_next_item() else {
            return Ok(obj! { "finished": true, "out_of_items": true });
        };
        live.current = Some(planned);
        Ok(obj! { "planned": planned_json(planned), "answered": live.done.len() })
    }

    /// The item that would follow the current one if it were answered `ok`.
    fn session_peek(&mut self, fields: &[&str]) -> Reply {
        let ok = arg(fields, 1)? == "1";
        let live = self.session.as_ref().ok_or("no session is open")?;
        let Some(current) = live.current else {
            return Ok(obj! { "planned": V::Null });
        };
        let mut copy = live.real.clone().without_sibling_search();
        // The family isn't known ahead of grading, but this item's miss can't come
        // back as the very next item: retries wait at least RETRY_MIN_GAP items.
        copy.record_with_family(current, ok, 0, None);
        let planned = copy.try_next_item();
        // A retry or review needs a sibling search only the real deal can do. Guessing
        // would have the prep worker build an item the player won't get, and the job
        // can't be cancelled, so the real item would wait behind it: guess nothing.
        Ok(obj! { "planned": planned.filter(|_| !copy.guessed_without_search()).map(planned_json) })
    }

    fn session_summary(&self) -> Reply {
        let live = self.live()?;
        let mut rows: Vec<(Skill, usize, usize)> = Vec::new();
        let mut ok_total = 0;
        for &(planned, ok, _) in &live.done {
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
        let response = response_field(fields, 4)?;
        let item = self.session_item(skill, seed);
        match grade_timed(&item, raw, response) {
            Ok(outcome) => {
                // A retry stays a retry even if this answer's commit arrives late.
                let origin = self
                    .session
                    .as_ref()
                    .and_then(|live| live.current)
                    .filter(|p| p.skill == skill && p.seed == seed)
                    .map_or(Origin::New, |p| p.origin);
                let planned = Planned { skill, seed, origin };
                let template = drill_template(planned, &outcome);
                Ok(obj! { "outcome": graded_outcome(&item, &outcome, Some(origin), template, response) })
            }
            Err(invalid) => Ok(obj! { "invalid": invalid.0 }),
        }
    }

    /// The item the open session served, with its current equity band. Outside a
    /// session (the differential tests) the plain generated item stands in.
    fn session_item(&self, skill: Skill, seed: u64) -> Item {
        match &self.session {
            Some(live) => live.real.item_for(Planned { skill, seed, origin: Origin::New }),
            None => drill_item(skill, seed),
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
        // A fourth field carries the optional confidence tap.
        if asks_confidence(attempt.skill) {
            attempt.extra.confidence = fields.get(4).and_then(|code| Confidence::from_code(code));
        }
        let live = self.session.as_mut().ok_or("no session is open")?;
        let expected = live.current.is_some_and(|p| p.skill == attempt.skill && attempt.item == ItemRef::Seed(p.seed));
        if !expected {
            // The player left this drill while its grade was still on its way, so a
            // new session may have opened. The answer is real: log it, but leave
            // that session's bookkeeping alone.
            let ok = attempt.ok();
            let answered = live.done.len();
            let line = format_attempt(&attempt);
            self.attempts.push(attempt);
            return Ok(obj! { "line": line, "ok": ok, "answered": answered });
        }
        let planned = live.current.take().ok_or("no item is waiting for an answer")?;
        attempt.mode = if planned.origin == Origin::SessionRetry {
            Mode::DrillRetry
        } else {
            Mode::Drill
        };
        if planned.origin == Origin::SessionRetry {
            attempt.extra.reflect = None;
        }
        let ok = attempt.ok();
        let family = attempt.extra.family.clone();
        let line = format_attempt(&attempt);
        self.attempts.push(attempt);
        live.real.record_with_family(planned, ok, response, family);
        live.done.push((planned, ok, response));
        Ok(obj! { "line": line, "ok": ok, "answered": live.done.len() })
    }

    /// Logs a finished reflection form. The time spent on it is never recorded.
    fn reflect_commit(&mut self, fields: &[&str]) -> Reply {
        let now = parse_u64(arg(fields, 2)?, "now")?;
        let answers = arg(fields, 3)?;
        let attempt = Self::attempt_from_template(arg(fields, 1)?, now, 0)?;
        let row = reflection_row(&attempt, answers, now);
        let line = format_attempt(&row);
        self.attempts.push(row);
        Ok(obj! { "line": line })
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
        match grade_timed(&entry.item, raw, 0) {
            Ok(outcome) => {
                let template = format_attempt(&Attempt::from_baseline(0, entry, &outcome, 0, 0));
                Ok(obj! { "outcome": present::outcome(&entry.item, &outcome, template, 0, V::Null) })
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

    fn prepare_check(&mut self, fields: &[&str]) -> Reply {
        let (skill, form, slot, run) = check_fields(fields)?;
        let plan = checkup_plan(skill, form);
        let seed = plan.slots.get(slot).ok_or("no such slot")?.seed;
        let item = plan.item(slot);
        let check = CheckRef { run, form, slot: slot as u32 };
        Ok(bundle(&item, None, &|outcome| Attempt::from_check(check, skill, seed, outcome, 0, 0)))
    }

    /// What a player can do about a skill's checkups, and how they went.
    fn check_info(&self, fields: &[&str]) -> Reply {
        let skill = skill_from(arg(fields, 1)?)?;
        let runs = checkup_runs(&self.attempts);
        // A run from an older generator or an earlier spot library can't be picked up.
        let unfinished = unfinished_run(&runs, skill);
        let upcoming_form = unfinished.map_or_else(|| next_checkup_form(skill, &self.attempts), |r| r.form);
        let in_progress = unfinished.map(|r| {
            obj! { "answered": r.answered, "total": CHECKUP_LEN }
        });
        let history: Vec<V> = runs.iter().filter(|r| r.skill == skill).map(checkup_json).collect();
        // Starting over deals a new form, which may be past the reserved spots.
        let start_over_form = next_checkup_form(skill, &self.attempts);
        Ok(obj! {
            "skill": skill.name(),
            "title": present::skill_title(skill),
            "total": CHECKUP_LEN,
            "originals": CHECKUP_ORIGINALS,
            "repeats": CHECKUP_REPEATS,
            "in_progress": in_progress,
            "history": history,
            "reuse_note": (skill == Skill::Table && table::checkup_form_reuses_spots(upcoming_form)).then_some(table::REUSE_NOTE),
            "start_over_note": (skill == Skill::Table && unfinished.is_some() && table::checkup_form_reuses_spots(start_over_form)).then_some(table::REUSE_NOTE),
        })
    }

    /// Starts a checkup, or picks up the skill's unfinished one when `resume` is 1.
    fn check_begin(&mut self, fields: &[&str]) -> Reply {
        let skill = skill_from(arg(fields, 1)?)?;
        let resume = arg(fields, 2)? == "1";
        let runs = checkup_runs(&self.attempts);
        let unfinished = unfinished_run(&runs, skill);
        let (run, form) = match unfinished.filter(|_| resume) {
            Some(score) => (score.run, score.form),
            None => {
                // After a run-number clash the page keeps its form, so its place holds.
                let form = match fields.get(3) {
                    Some(text) => parse_u64(text, "form")? as u32,
                    None => next_checkup_form(skill, &self.attempts),
                };
                (next_checkup_run(&self.attempts), form)
            }
        };
        Ok(obj! {
            "skill": skill.name(),
            "title": present::skill_title(skill),
            "run": run,
            "form": form,
            "total": CHECKUP_LEN,
        })
    }

    fn answered_slots(&self, skill: Skill, run: u32) -> Vec<bool> {
        let mut answered = vec![false; CHECKUP_LEN];
        for attempt in self.attempts.iter().filter(|a| a.skill == skill && a.mode == Mode::Check) {
            if let Some(check) = attempt.extra.check.filter(|c| c.run == run) {
                if let Some(slot) = answered.get_mut(check.slot as usize) {
                    *slot = true;
                }
            }
        }
        answered
    }

    fn check_next(&self, fields: &[&str]) -> Reply {
        let skill = skill_from(arg(fields, 1)?)?;
        let run = parse_u64(arg(fields, 2)?, "run")? as u32;
        let answered = self.answered_slots(skill, run);
        let done = answered.iter().filter(|a| **a).count();
        Ok(match answered.iter().position(|a| !a) {
            Some(slot) => obj! {
                "done": false,
                "slot": slot,
                "after": answered[slot + 1..].iter().position(|a| !a).map(|gap| slot + 1 + gap),
                "number": done + 1,
                "total": CHECKUP_LEN,
            },
            None => obj! { "done": true, "total": CHECKUP_LEN },
        })
    }

    fn check_grade(&self, fields: &[&str]) -> Reply {
        let (skill, form, slot, run) = check_fields(fields)?;
        let raw = arg(fields, 5)?;
        let response = response_field(fields, 6)?;
        let plan = checkup_plan(skill, form);
        let seed = plan.slots.get(slot).ok_or("no such slot")?.seed;
        let item = plan.item(slot);
        match grade_timed(&item, raw, response) {
            Ok(outcome) => {
                let check = CheckRef { run, form, slot: slot as u32 };
                let template = format_attempt(&Attempt::from_check(check, skill, seed, &outcome, 0, 0));
                Ok(obj! { "outcome": present::outcome(&item, &outcome, template, response, V::Null) })
            }
            Err(invalid) => Ok(obj! { "invalid": invalid.0 }),
        }
    }

    fn check_commit(&mut self, fields: &[&str]) -> Reply {
        let now = parse_u64(arg(fields, 2)?, "now")?;
        let response = response_field(fields, 3)?;
        let mut attempt = Self::attempt_from_template(arg(fields, 1)?, now, response)?;
        let mut check = attempt.extra.check.ok_or("not a checkup answer")?;
        // A fifth field moves the answer onto a new run after a clash.
        if let Some(text) = fields.get(4) {
            check.run = parse_u64(text, "run")? as u32;
            attempt.extra.check = Some(check);
        }
        let taken = self.attempts.iter().any(|a| {
            a.extra.check.is_some_and(|c| c.run == check.run && (a.skill != attempt.skill || c.form != check.form))
        });
        if taken {
            // Another tab began a checkup under this number first, so nothing is logged.
            return Ok(obj! { "conflict": true });
        }
        if self.answered_slots(attempt.skill, check.run).get(check.slot as usize).copied().unwrap_or(false) {
            // Another tab already answered this slot; keep only the first.
            return Ok(obj! { "duplicate": true });
        }
        let line = format_attempt(&attempt);
        self.attempts.push(attempt);
        Ok(obj! { "line": line })
    }

    fn check_report(&self, fields: &[&str]) -> Reply {
        let run = parse_u64(arg(fields, 1)?, "run")? as u32;
        let runs = checkup_runs(&self.attempts);
        let Some(index) = runs.iter().position(|r| r.run == run) else {
            return Ok(obj! { "report": V::Null });
        };
        let score = &runs[index];
        let earlier: Vec<&CheckupScore> =
            runs[..index].iter().filter(|r| r.skill == score.skill && r.complete).collect();
        Ok(obj! {
            "report": checkup_report_json(score, &earlier),
            "earlier": earlier.iter().map(|r| checkup_json(r)).collect::<Vec<_>>(),
        })
    }
}

type SeenRuns = RunIds;

#[derive(Clone, Copy)]
struct RunIds {
    baseline: Option<u32>,
    check: Option<u32>,
}

fn check_run(attempt: &Attempt) -> Option<u32> {
    attempt.extra.check.map(|c| c.run)
}

fn seen_runs(attempt: &Attempt) -> SeenRuns {
    RunIds { baseline: attempt.baseline_run, check: check_run(attempt) }
}

fn origin_from(text: &str) -> Origin {
    match text {
        "retry" => Origin::SessionRetry,
        "due" => Origin::Due,
        _ => Origin::New,
    }
}

fn check_fields(fields: &[&str]) -> Result<(Skill, u32, usize, u32), String> {
    let skill = skill_from(arg(fields, 1)?)?;
    let form = parse_u64(arg(fields, 2)?, "form")? as u32;
    let slot = parse_u64(arg(fields, 3)?, "slot")? as usize;
    let run = parse_u64(arg(fields, 4)?, "run")? as u32;
    Ok((skill, form, slot, run))
}

fn response_field(fields: &[&str], index: usize) -> Result<u32, String> {
    Ok(parse_u64(arg(fields, index)?, "response")?.min(u64::from(u32::MAX)) as u32)
}

/// Identifies an attempt across devices. Run numbers are left out because the
/// same attempt can carry a different number after an earlier import.
fn merge_key(attempt: &Attempt) -> String {
    let mut plain = attempt.clone();
    plain.baseline_run = None;
    if let Some(check) = &mut plain.extra.check {
        check.run = 0;
    }
    format_attempt(&plain)
}

fn randomized_baseline(seed: u64) -> Result<Vec<BaselineItem>, String> {
    let items = baseline::load()?;
    let mut rng = Rng::new(seed);
    Ok(baseline::randomize(&items, &mut rng))
}

fn mark_lines(skill: Skill) -> Vec<String> {
    let mark = pass_mark(skill);
    let mut parts = Vec::new();
    if let Some(m) = mark.min_correct_pct {
        parts.push(format!("at least {m:.0}% correct"));
    }
    if let (Some(m), Skill::Chain) = (mark.min_within_one_pct, skill) {
        parts.push(format!("at least {m:.0}% of stages right (outs within 1 count)"));
    } else if let Some(m) = mark.min_within_one_pct {
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
    if let Some(m) = mark.max_mean_abs_error {
        parts.push(format!("average error at most {m:.0} points"));
    }
    if skill == Skill::Table {
        parts.push("robust and close spots only".to_string());
    }
    parts
}

/// The measures that count for `skill`'s pass mark, as plain phrases.
fn measure_lines(skill: Skill, m: &WindowMetrics) -> Vec<String> {
    let mark = pass_mark(skill);
    let mut parts = Vec::new();
    if mark.min_correct_pct.is_some() {
        parts.push(format!("{:.0}% correct", m.correct_pct));
    }
    if mark.min_within_one_pct.is_some() && skill == Skill::Chain {
        parts.push(format!("{:.0}% of stages right", m.within_one_pct));
    } else if mark.min_within_one_pct.is_some() {
        parts.push(format!("{:.0}% within 1 ({:.0}% exact)", m.within_one_pct, m.correct_pct));
    }
    if mark.max_median_ms.is_some() {
        parts.push(format!("median {:.1} s", f64::from(m.median_ms) / 1000.0));
    }
    if mark.max_mean_ev_loss_pct.is_some() {
        parts.push(format!("mean EV loss {:.1}% of pot", m.mean_ev_loss_pct));
    }
    if mark.max_blunders.is_some() {
        parts.push(format!("{} blunders", m.blunders));
    }
    if let (true, Some(e)) = (mark.max_mean_abs_error.is_some(), m.mean_abs_error) {
        parts.push(format!("average error {e:.1} points"));
    }
    parts
}

fn stat_lines(stats: &SkillStats) -> Vec<String> {
    let metrics = WindowMetrics {
        correct_pct: stats.correct_pct,
        within_one_pct: stats.within_one_pct,
        median_ms: stats.median_ms,
        mean_ev_loss_pct: stats.mean_ev_loss_pct,
        blunders: stats.blunders,
        mean_abs_error: stats.mean_abs_error,
    };
    let mut parts = vec![format!("Last {}", stats.window)];
    parts.extend(measure_lines(stats.skill, &metrics));
    parts
}

fn attempts_needed(s: &SkillStats) -> String {
    if s.skill == Skill::BeatsMe {
        format!("Drill mark counts levels 3 and 4 only: needs {PASS_ATTEMPTS} attempts there, has {}.", s.total_attempts)
    } else {
        format!("Drill mark needs {PASS_ATTEMPTS} attempts, has {}.", s.total_attempts)
    }
}

/// Where a skill stands on its drill mark and its checkup, in words.
fn skill_status(s: &SkillStats) -> String {
    if s.passed {
        "Passed: drill mark and checkup both met.".to_string()
    } else if !s.drill_passed && s.total_attempts < PASS_ATTEMPTS {
        attempts_needed(s)
    } else if !s.drill_passed {
        "Drill mark not met yet.".to_string()
    } else if s.checkup_passed.is_none() {
        "Drill mark met. Checkup still to take.".to_string()
    } else {
        "Drill mark met, but the latest checkup missed it.".to_string()
    }
}

/// The player's latest run of `skill` this build can still deal, if it's unfinished.
/// A run from an older generator or an earlier spot library can't be picked up, and
/// it mustn't hide a newer run that can, as it would if an old tab wrote it later.
fn unfinished_run(runs: &[CheckupScore], skill: Skill) -> Option<&CheckupScore> {
    runs.iter().rfind(|r| r.skill == skill && r.in_current_library && !r.stale).filter(|r| !r.complete)
}

/// `None` when no checkup is offered for this skill.
fn checkup_action(s: &SkillStats, checkups: &[CheckupScore]) -> V {
    let unfinished = unfinished_run(checkups, s.skill);
    if let Some(run) = unfinished {
        return obj! {
            "skill": s.skill.name(),
            "title": present::skill_title(s.skill),
            "resume": true,
            "answered": run.answered,
            "total": CHECKUP_LEN,
        };
    }
    if s.drill_passed && s.checkup_passed != Some(true) {
        return obj! { "skill": s.skill.name(), "title": present::skill_title(s.skill), "resume": false };
    }
    V::Null
}

fn steps_json(stats: &[SkillStats], step: usize) -> Vec<V> {
    let of = |skill: Skill| stats.iter().find(|s| s.skill == skill).expect("every skill has stats");
    STEPS
        .iter()
        .enumerate()
        .map(|(i, skills)| {
            let list: Vec<V> = skills
                .iter()
                .map(|&s| {
                    let st = of(s);
                    obj! {
                        "title": present::skill_title(s),
                        "passed": st.passed,
                        "status": skill_status(st),
                    }
                })
                .collect();
            obj! {
                "step": i + 1,
                "current": i + 1 == step,
                "unlocked": i < step,
                "passed": skills.iter().all(|&s| of(s).passed),
                "drill_passed": skills.iter().all(|&s| of(s).drill_passed),
                "checkup_passed": skills.iter().all(|&s| of(s).checkup_passed == Some(true)),
                "skills": list,
            }
        })
        .collect()
}

fn checkup_actions(stats: &[SkillStats], checkups: &[CheckupScore]) -> V {
    V::Arr(
        stats
            .iter()
            .map(|s| checkup_action(s, checkups))
            .filter(|action| !matches!(action, V::Null))
            .collect(),
    )
}

fn stats_json(s: &SkillStats, checkups: &[CheckupScore]) -> V {
    obj! {
        "title": present::skill_title(s.skill),
        "step": s.skill.step().map(u32::from),
        "enough_data": s.enough_data,
        "total": s.total_attempts,
        "needed": MIN_ATTEMPTS,
        "passed": s.passed,
        "drill_passed": s.drill_passed,
        "checkup_passed": s.checkup_passed,
        "stats": stat_lines(s),
        "mark": mark_lines(s.skill),
        "status": skill_status(s),
        "checkup": checkup_action(s, checkups),
    }
}

/// Beside a run on a spot library this build no longer deals.
const OLDER_LIBRARY_NOTE: &str = "older spot library";

fn checkup_json(s: &CheckupScore) -> V {
    let status = if !s.complete {
        format!("Unfinished: {} of {CHECKUP_LEN} answered", s.answered)
    } else if s.passed {
        "Mark met".to_string()
    } else {
        "Mark not met".to_string()
    };
    obj! {
        "run": s.run,
        "title": present::skill_title(s.skill),
        "started_ms": V::Str(s.started_ms.to_string()),
        "complete": s.complete,
        "passed": s.passed,
        "answered": s.answered,
        "score": s.complete.then(|| present::checkup_score(s)),
        "margin": s.complete.then(|| present::checkup_margin(s)),
        "status": status,
        "library_note": (s.skill == Skill::Table && !s.in_current_library).then_some(OLDER_LIBRARY_NOTE),
    }
}

fn checkup_report_json(s: &CheckupScore, earlier: &[&CheckupScore]) -> V {
    let mut json = vec![
        ("summary".to_string(), checkup_json(s)),
        ("repeats".to_string(), present::repeats_line(&s.agreement).into()),
        ("mark".to_string(), mark_lines(s.skill).into()),
        ("measures".to_string(), measure_lines(s.skill, &s.metrics).into()),
    ];
    // Only a run on the same generator or spot library asked comparable questions.
    let prior = s.generator_key.and_then(|key| earlier.iter().rev().find(|r| r.generator_key == Some(key)));
    let compared = prior.map(|prior| present::comparison(prior, s));
    json.push(("comparison".to_string(), compared.into()));
    let reused = (s.skill == Skill::Table && s.in_current_library && table::checkup_form_reuses_spots(s.form))
        .then_some(table::REUSE_NOTE);
    json.push(("reused_note".to_string(), reused.into()));
    json.push(("originals".to_string(), CHECKUP_ORIGINALS.into()));
    V::Obj(json)
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
    let situations: Vec<V> = [
        ("folded", "Folded to you", false),
        ("limpers", "Limpers", false),
        ("raised", "One raise", false),
        ("callers", "Raise + callers", false),
        ("three_bet_ip", "3-bet, in position", true),
        ("three_bet_oop", "3-bet, out of position", true),
        ("four_bet", "4-bet", true),
    ]
    .into_iter()
    .map(|(code, title, one_chart)| obj! { "code": code, "title": title, "one_chart": one_chart })
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
        "callers" => Situation::RaisedCalled(1),
        "three_bet_ip" => Situation::ThreeBet { in_position: true },
        "three_bet_oop" => Situation::ThreeBet { in_position: false },
        "four_bet" => Situation::FourBet,
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
        'r' => match chart_for(seat, situation) {
            Some(chart) => chart.raise_word().to_string(),
            None => "Raise".to_string(),
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
        Situation::Raised | Situation::RaisedCalled(_) | Situation::ThreeBet { .. } | Situation::FourBet => {
            let chart = chart_for(seat, situation).expect("these situations have a chart");
            let (raise, call) = chart.lines();
            let mut lines = vec![
                line(format!("{}: {}", chart.title(), chart.raise_word()), raise),
                line(format!("{}: call", chart.title()), call),
            ];
            if let Some(extra) = chart.extra_call() {
                lines.push(line(format!("{}: also call", chart.title()), extra));
            }
            lines
        }
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

    fn check_line(ts: u32, run: u32, slot: u32) -> String {
        format!("{ts}\tcheck\t\tprice\t7\t1\t1\t0\t\t1000\t25\t{run}\t0\t{slot}\t\t\t\t\n")
    }

    /// (timestamp, check run) for every checkup line in the log.
    fn check_runs_in_log() -> Vec<(u32, String)> {
        let reply = dispatch("export");
        let text = reply.replace("\\t", "\t").replace("\\n", "\n");
        text.lines()
            .filter(|l| l.contains("\tcheck\t"))
            .map(|l| {
                let f: Vec<&str> = l.split('\t').collect();
                let ts = f[0].trim_start_matches("{\"text\":\"").parse().unwrap();
                (ts, f[11].to_string())
            })
            .collect()
    }

    fn number(reply: &str, key: &str) -> u32 {
        let needle = format!("\"{key}\":");
        let tail = &reply[reply.find(&needle).unwrap_or_else(|| panic!("no {key} in {reply}")) + needle.len()..];
        tail[..tail.find(|c: char| !c.is_ascii_digit()).unwrap()].parse().unwrap()
    }

    /// The attempt template in a grade reply, with its tabs restored.
    fn template(reply: &str) -> String {
        let start = reply.find("\"attempt\":\"").unwrap_or_else(|| panic!("no template in {reply}")) + 11;
        reply[start..start + reply[start..].find('"').unwrap()].replace("\\t", "\t")
    }

    fn grade_check(skill: &str, form: u32, slot: u32, run: u32, answer: &str, ms: u32) -> String {
        dispatch(&format!("check_grade\x1f{skill}\x1f{form}\x1f{slot}\x1f{run}\x1f{answer}\x1f{ms}"))
    }

    #[test]
    fn a_checkup_run_taken_by_another_skill_is_rejected() {
        load("");
        let first = dispatch("check_begin\x1fprice\x1f0");
        let second = dispatch("check_begin\x1fouts\x1f0");
        // Two tabs starting together both get run 1: the commit has to sort it out.
        assert_eq!((number(&first, "run"), number(&second, "run")), (1, 1));
        let price = template(&grade_check("price", 0, 0, 1, "25", 1500));
        assert!(dispatch(&format!("check_commit\x1f{price}\x1f1000\x1f1500")).contains("\"line\""));

        let outs = template(&grade_check("outs", 0, 0, 1, "4", 1500));
        let clash = dispatch(&format!("check_commit\x1f{outs}\x1f2000\x1f1500"));
        assert!(clash.contains("\"conflict\":true"), "{clash}");
        assert_eq!(dispatch("export").matches("\\tcheck\\t").count(), 1, "a conflict must log nothing");

        // The losing tab takes a new run and keeps its form, so its answer still counts.
        let fresh = dispatch("check_begin\x1fouts\x1f0\x1f0");
        assert_eq!((number(&fresh, "run"), number(&fresh, "form")), (2, 0));
        let moved = dispatch(&format!("check_commit\x1f{outs}\x1f3000\x1f1500\x1f2"));
        assert!(moved.contains("\"line\""), "{moved}");
        assert!(dispatch("check_report\x1f1").contains("Pot odds"));
        let report = dispatch("check_report\x1f2");
        assert!(report.contains("Counting outs") && report.contains("\"answered\":1"), "{report}");
        assert_eq!(dispatch("export").matches("\\tcheck\\t").count(), 2, "one row per answer");

        // The tab that took the number first carries on undisturbed.
        let next = template(&grade_check("price", 0, 1, 1, "25", 1500));
        assert!(dispatch(&format!("check_commit\x1f{next}\x1f4000\x1f1500")).contains("\"line\""));
    }

    #[test]
    fn a_slow_board_checkup_answer_is_a_miss() {
        load("");
        let bundle = dispatch("prepare_check\x1fboard_type\x1f0\x1f0\x1f1");
        assert!(bundle.contains("\"graded\":{}"), "board checkups are graded when answered: {bundle}");
        let code = ["rp", "rc", "rd", "tp", "tc", "td", "mc", "md"]
            .into_iter()
            .find(|c| grade_check("board_type", 0, 0, 1, c, 1000).contains("\"correct\":true"))
            .expect("one option is right");
        let slow = grade_check("board_type", 0, 0, 1, code, 13_000);
        assert!(slow.contains("\"correct\":false") && slow.contains("\"too_slow\":true"), "{slow}");
        assert_eq!(template(&slow).split('\t').nth(6), Some("0"), "logged as a miss");
    }

    #[test]
    fn exact_hands_have_no_combo_count_to_show() {
        load("");
        let exact = dispatch("grade\x1fequity\x1f17\x1f45\x1f0");
        assert!(exact.contains("\"combos\":null"), "{exact}");
        // The same seed at the second rung averages over hand types.
        let classes = dispatch("grade\x1fequity\x1f4611686018427387921\x1f45\x1f0");
        assert!(!classes.contains("\"combos\":null"), "{classes}");
    }

    #[test]
    fn a_late_commit_of_a_retry_stays_a_retry() {
        load("");
        assert!(dispatch("session_open\x1f5\x1f1000000\x1fprice").contains("first"));
        let field = |json: &str, key: &str| {
            let needle = format!("\"{key}\":\"");
            let start = json.find(&needle)? + needle.len();
            Some(json[start..start + json[start..].find('"')?].to_string())
        };
        for step in 0..60u64 {
            let next = dispatch(&format!("session_next\x1f{}", step * 100));
            let (skill, seed, origin) = (field(&next, "skill").unwrap(), field(&next, "seed").unwrap(), field(&next, "origin").unwrap());
            let graded = dispatch(&format!("grade\x1f{skill}\x1f{seed}\x1f99\x1f1500"));
            let line = template(&graded);
            if origin == "retry" {
                // The player left: a new session replaces this one before the commit lands.
                assert!(line.contains("\tdrill-retry\t"), "the template keeps the retry mode: {line}");
                dispatch("session_open\x1f6\x1f1000000\x1fprice");
                let committed = dispatch(&format!("commit\x1f{line}\x1f9000000\x1f1500"));
                assert!(committed.contains("drill-retry"), "{committed}");
                let export = dispatch("export").replace("\\t", "\t");
                assert!(export.contains("9000000\tdrill-retry"), "logged as a retry, not a counted drill: {export}");
                return;
            }
            assert!(dispatch(&format!("commit\x1f{line}\x1f{}\x1f1500", 2_000_000 + step)).contains("line"));
        }
        panic!("no retry was dealt");
    }

    #[test]
    fn the_default_drill_keeps_preflop_at_a_quarter() {
        load("");
        dispatch("session_open\x1f7\x1f1000000");
        dispatch("session_start\x1f15");
        let mut preflop = 0;
        let total = 120;
        for step in 0..total {
            let next = dispatch(&format!("session_next\x1f{}", step * 1000));
            preflop += u32::from(next.contains("\"skill\":\"preflop\""));
        }
        assert!(preflop * 4 >= total, "{preflop} of {total}");
    }

    #[test]
    fn an_old_generators_unfinished_checkup_cannot_resume() {
        // `spot` changed at version 2, so its old items can't be dealt again.
        load("1\tcheck\t\tspot\t7\t1\t1\t0\t\t1000\tcall\t1\t5\t0\t\t\t\t\n");
        assert!(dispatch("check_info\x1fspot").contains("\"in_progress\":null"), "offered a resume");
        let begun = dispatch("check_begin\x1fspot\x1f1");
        assert!(begun.contains("\"run\":2"), "{begun}");
        // `hit` hasn't changed since version 1, so the same questions are still there.
        load("1\tcheck\t\thit\t7\t1\t1\t0\t\t1000\t20\t1\t5\t0\t\t\t\t\n");
        assert!(dispatch("check_info\x1fhit").contains("\"in_progress\":{\"answered\":1"), "a run nothing has changed can resume");
    }

    #[test]
    fn the_beats_me_status_says_only_the_top_levels_count() {
        load("");
        let progress = dispatch("progress\x1f0");
        assert!(progress.contains("counts levels 3 and 4 only"), "{progress}");
    }

    #[test]
    fn a_card_with_any_of_a_suit_is_echoed_in_words() {
        for seed in 0..40u32 {
            let reply = dispatch(&format!("grade\x1fnuts\x1f{seed}\x1fAsxs\x1f0"));
            if reply.contains("\"outcome\"") {
                assert!(reply.contains("with any other spade"), "{reply}");
                return;
            }
        }
        panic!("the ace of spades was on every board");
    }

    #[test]
    fn a_checkup_from_another_device_gets_a_new_run_number() {
        load(&(check_line(1, 1, 0) + &check_line(2, 1, 1)));
        // The other device also numbered its first checkup 1.
        assert_eq!(added(&merge(&(check_line(3, 1, 0) + &check_line(4, 1, 1)))), 2);
        let expected = [(1, "1"), (2, "1"), (3, "2"), (4, "2")];
        assert_eq!(check_runs_in_log(), expected.map(|(t, r)| (t, r.to_string())).to_vec());
    }

    #[test]
    fn reimporting_a_checkup_adds_nothing_and_keeps_its_number() {
        load(&check_line(1, 1, 0));
        assert_eq!(added(&merge(&(check_line(3, 1, 0)))), 1);
        assert_eq!(added(&merge(&(check_line(3, 1, 0) + &check_line(4, 1, 1)))), 1);
        assert_eq!(added(&merge(&check_line(3, 1, 0))), 0);
        assert_eq!(
            check_runs_in_log(),
            vec![(1, "1".to_string()), (3, "2".to_string()), (4, "2".to_string())]
        );
    }

    #[test]
    fn starting_over_a_late_table_checkup_warns_about_the_next_form() {
        let line = |form: u32, slot: u32| format!("{}\tcheck\t\ttable\t7\t1003\t1\t0\t\t1000\tfold\t1\t{form}\t{slot}\t\t\t\t\n", 100 + slot);
        let note = "\"start_over_note\":";
        // Form 3 unfinished: starting over deals form 4, which still has fresh spots.
        load(&line(3, 0));
        let info = dispatch("check_info\x1ftable");
        assert!(info.contains(&format!("{note}null")), "{info}");
        // Form 4 unfinished: starting over deals form 5, which reuses spots, even
        // though the unfinished form itself does not.
        load(&line(4, 0));
        let info = dispatch("check_info\x1ftable");
        assert!(!info.contains(&format!("{note}null")), "{info}");
        assert!(info.contains("\"reuse_note\":null"), "{info}");
        // Other skills never warn.
        load("1\tcheck\t\thit\t7\t2\t1\t0\t\t1000\t20\t1\t5\t0\t\t\t\t\n");
        assert!(dispatch("check_info\x1fhit").contains(&format!("{note}null")));
    }

    #[test]
    fn an_unfinished_table_checkup_from_the_old_library_is_not_resumed() {
        // Two old forms were unfinished, the later one with spots this library assigns elsewhere.
        let line = |form: u32, slot: u32| format!("{}\tcheck\t\ttable\t7\t2\t1\t0\t\t1000\tfold\t1\t{form}\t{slot}\t\t\t\t\n", 100 + slot);
        load(&(line(5, 0) + &line(5, 1)));
        let info = dispatch("check_info\x1ftable");
        assert!(info.contains("\"in_progress\":null"), "{info}");
        assert!(info.contains("\"start_over_note\":null"), "{info}");
        assert!(info.contains("\"reuse_note\":null"), "{info}");
        // Home doesn't offer it either.
        let home = dispatch("home");
        assert!(!home.contains("\"resume\":true"), "{home}");
        // Beginning starts form 0 of the new library, with a new run number.
        let begun = dispatch("check_begin\x1ftable\x1f1");
        assert!(begun.contains("\"form\":0"), "{begun}");
        assert!(!begun.contains("\"run\":1"), "{begun}");
    }

    #[test]
    fn a_report_on_an_old_library_run_does_not_claim_reuse_from_the_current_pool() {
        // Form 5 of the old library reused nothing, and this library's pool can't say otherwise.
        let rows: String = (0..50).map(|slot| format!("{}\tcheck\t\ttable\t7\t2\t1\t0\t0.5000\t1000\tfold\t1\t5\t{slot}\t\t\t\t\t205\trobust\n", 100 + slot)).collect();
        load(&rows);
        let report = dispatch("check_report\x1f1");
        assert!(report.contains("\"reused_note\":null"), "{report}");
    }

    fn table_run(run: u32, form: u32, gen_version: u32, loss: f64, start: u64) -> String {
        (0..50u64)
            .map(|slot| {
                format!("{}\tcheck\t\ttable\t7\t{gen_version}\t1\t0\t{loss:.4}\t1000\tfold\t{run}\t{form}\t{slot}\t\t\t\t\t205\trobust\n", start + slot)
            })
            .collect()
    }

    #[test]
    fn a_checkup_is_compared_only_with_runs_on_the_same_library() {
        // An old-library run, then a current one, then another current one, then a mixed one.
        let old = table_run(1, 0, 2, 0.5, 100);
        let first_new = table_run(2, 0, 1003, 1.0, 1000);
        let second_new = table_run(3, 1, 1003, 0.8, 2000);
        let mixed: String = (0..50u64)
            .map(|slot| {
                let gen_version = if slot < 25 { 2 } else { 1003 };
                format!("{}\tcheck\t\ttable\t7\t{gen_version}\t1\t0\t0.7000\t1000\tfold\t4\t2\t{slot}\t\t\t\t\t205\trobust\n", 3000 + slot)
            })
            .collect();
        load(&format!("{old}{first_new}{second_new}{mixed}"));
        let first = dispatch("check_report\x1f2");
        assert!(first.contains("\"comparison\":null"), "the first new run has only an old-library run behind it: {first}");
        assert!(first.contains("older spot library"), "the old run in the list is labelled: {first}");
        let second = dispatch("check_report\x1f3");
        assert!(second.contains("Better than your last checkup"), "compared with the first new run: {second}");
        assert!(second.contains("older spot library"), "{second}");
        let unmixed = dispatch("check_report\x1f4");
        assert!(unmixed.contains("\"comparison\":null"), "a run across libraries compares with nothing: {unmixed}");
    }

    #[test]
    fn an_old_tabs_unfinished_run_does_not_hide_a_resumable_one() {
        // Run 1 is on this library and unfinished; run 2, on the old one, was written after it.
        let line = |t: u64, gen_version: u32, run: u32| format!("{t}\tcheck\t\ttable\t7\t{gen_version}\t1\t0\t\t1000\tfold\t{run}\t0\t0\t\t\t\t\n");
        load(&(line(100, 1003, 1) + &line(200, 2, 2)));
        let info = dispatch("check_info\x1ftable");
        assert!(info.contains("\"in_progress\":{\"answered\":1"), "the current run stays resumable: {info}");
        let home = dispatch("home");
        assert!(home.contains("\"resume\":true"), "{home}");
        let begun = dispatch("check_begin\x1ftable\x1f1");
        assert!(begun.contains("\"run\":1,"), "resumes run 1, not a new one: {begun}");
    }

    #[test]
    fn a_retry_or_review_coming_next_is_not_speculated() {
        // Two families are due, so whatever is dealt first, the next one needs a sibling search.
        let row = |t: u64, seed: u64, family: &str| {
            format!("{t}\tdrill\t\tmade\t{seed}\t3\t0\t0\t\t2000\tx\t\t\t\t\t\t\t\t\t\t{family}\t\t\t\n")
        };
        load(&(row(100, 1, "pair") + &row(200, 2, "twopair")));
        dispatch("session_open\x1f7\x1f1000000000\x1fmade");
        assert!(dispatch("session_next\x1f0").contains("\"planned\""));
        let peek = dispatch("session_peek\x1f1");
        assert!(peek.contains("\"planned\":null"), "a review coming up needs a search: {peek}");
        // Ordinary items are still looked ahead.
        load("");
        dispatch("session_open\x1f5\x1f1000000\x1fprice");
        assert!(dispatch("session_next\x1f0").contains("\"planned\""));
        let peek = dispatch("session_peek\x1f1");
        assert!(!peek.contains("\"planned\":null"), "{peek}");
    }

    #[test]
    fn a_session_retry_bundle_offers_no_reflection() {
        load("");
        assert!(dispatch("session_open\x1f5\x1f1000000\x1fspot").contains("first"));
        let seed = "11";
        let retry = dispatch(&format!("prepare\x1fspot\x1f{seed}\x1fretry"));
        assert!(!retry.contains("\"reflection\":["), "a retried spot gets no form: {retry}");
        assert!(retry.contains("drill-retry"), "the template keeps the retry mode: {retry}");
    }

    #[test]
    fn confidence_is_kept_only_on_skills_that_ask_for_it() {
        load("");
        dispatch("session_open\x1f6\x1f1000000\x1fprice");
        let next = dispatch("session_next\x1f0");
        let seed = {
            let needle = "\"seed\":\"";
            let start = next.find(needle).unwrap() + needle.len();
            next[start..start + next[start..].find('"').unwrap()].to_string()
        };
        let line = template(&dispatch(&format!("grade\x1fprice\x1f{seed}\x1f25\x1f1500")));
        let committed = dispatch(&format!("commit\x1f{line}\x1f2000000\x1f1500\x1fhigh"));
        assert!(committed.contains("high"), "{committed}");
    }

    #[test]
    fn a_reflection_row_is_logged_with_no_time() {
        load("");
        let line = "1\tdrill\t\tspot\t7\t1\t0\t0\t\t1000\tc\t\t\t\t\t\t\t\t\t\tallin:top\t\t\toffered";
        let reply = dispatch(&format!("reflect_commit\x1f{line}\x1f5000\x1fsupported:price"));
        assert!(reply.contains("\"line\""), "{reply}");
        let export = dispatch("export").replace("\\t", "\t");
        assert!(export.contains("5000\treflect\t\tspot\t7"), "{export}");
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
