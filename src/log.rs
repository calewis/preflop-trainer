//! The attempt log as TSV text. Reading and writing the file is the caller's job.

use crate::baseline::BaselineItem;
use crate::grade::Outcome;
use crate::item::{Skill, GEN_VERSION};
use crate::schedule::{Origin, Planned};

/// Longest response time recorded.
pub const MAX_RESPONSE_MS: u32 = 60_000;

/// First line of a new log file. Parsers skip it.
pub const LOG_HEADER: &str = "timestamp_ms\tmode\tbaseline_run\tskill\titem\tgen_version\tcorrect\tclose\tev_loss_pct\tresponse_ms\tanswer";

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Mode {
    Baseline,
    Drill,
    /// A missed item asked again in the same session; not counted in pass marks.
    DrillRetry,
}

impl Mode {
    pub fn code(self) -> &'static str {
        match self {
            Mode::Baseline => "baseline",
            Mode::Drill => "drill",
            Mode::DrillRetry => "drill-retry",
        }
    }

    pub fn from_code(code: &str) -> Option<Mode> {
        match code {
            "baseline" => Some(Mode::Baseline),
            "drill" => Some(Mode::Drill),
            "drill-retry" => Some(Mode::DrillRetry),
            _ => None,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ItemRef {
    Seed(u64),
    Baseline(String),
}

#[derive(Clone, PartialEq, Debug)]
pub struct Attempt {
    pub timestamp_ms: u64,
    pub mode: Mode,
    pub baseline_run: Option<u32>,
    pub skill: Skill,
    pub item: ItemRef,
    pub gen_version: u32,
    pub correct: bool,
    pub close: bool,
    pub ev_loss_pct: Option<f64>,
    pub response_ms: u32,
    pub answer: String,
}

impl Attempt {
    /// The log record for an answered drill item.
    pub fn from_drill(planned: Planned, outcome: &Outcome, timestamp_ms: u64, response_ms: u32) -> Attempt {
        let mode = if planned.origin == Origin::SessionRetry { Mode::DrillRetry } else { Mode::Drill };
        Attempt::record(mode, None, planned.skill, ItemRef::Seed(planned.seed), outcome, timestamp_ms, response_ms)
    }

    /// The log record for an answered baseline question.
    pub fn from_baseline(
        run: u32,
        entry: &BaselineItem,
        outcome: &Outcome,
        timestamp_ms: u64,
        response_ms: u32,
    ) -> Attempt {
        let item = ItemRef::Baseline(entry.id.clone());
        Attempt::record(Mode::Baseline, Some(run), entry.item.skill, item, outcome, timestamp_ms, response_ms)
    }

    fn record(
        mode: Mode,
        baseline_run: Option<u32>,
        skill: Skill,
        item: ItemRef,
        outcome: &Outcome,
        timestamp_ms: u64,
        response_ms: u32,
    ) -> Attempt {
        Attempt {
            timestamp_ms,
            mode,
            baseline_run,
            skill,
            item,
            gen_version: GEN_VERSION,
            correct: outcome.correct,
            close: outcome.close,
            ev_loss_pct: outcome.ev_loss_pct,
            response_ms: response_ms.min(MAX_RESPONSE_MS),
            answer: outcome.answer.clone(),
        }
    }

    /// Correct, or close enough to count: outs within 1, spots within the close band.
    pub fn ok(&self) -> bool {
        self.correct || self.close
    }
}

fn flag(value: bool) -> &'static str {
    if value {
        "1"
    } else {
        "0"
    }
}

/// One log line, without a trailing newline. Tabs and newlines in the answer are replaced.
pub fn format_attempt(a: &Attempt) -> String {
    let item = match &a.item {
        ItemRef::Seed(seed) => seed.to_string(),
        ItemRef::Baseline(id) => id.clone(),
    };
    let answer: String = a
        .answer
        .chars()
        .map(|c| if c == '\t' || c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        a.timestamp_ms,
        a.mode.code(),
        a.baseline_run.map_or(String::new(), |r| r.to_string()),
        a.skill.name(),
        item,
        a.gen_version,
        flag(a.correct),
        flag(a.close),
        a.ev_loss_pct.map_or(String::new(), |v| format!("{v:.4}")),
        a.response_ms.min(MAX_RESPONSE_MS),
        answer
    )
}

fn parse_flag(text: &str) -> Option<bool> {
    match text {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    }
}

fn parse_line(line: &str) -> Option<Attempt> {
    let fields: Vec<&str> = line.split('\t').collect();
    if fields.len() != 11 {
        return None;
    }
    let mode = Mode::from_code(fields[1])?;
    let baseline_run = if fields[2].is_empty() { None } else { Some(fields[2].parse().ok()?) };
    let item = if mode == Mode::Baseline {
        if fields[4].is_empty() {
            return None;
        }
        ItemRef::Baseline(fields[4].to_string())
    } else {
        ItemRef::Seed(fields[4].parse().ok()?)
    };
    let ev_loss_pct = if fields[8].is_empty() {
        None
    } else {
        Some(fields[8].parse::<f64>().ok().filter(|v| v.is_finite())?)
    };
    Some(Attempt {
        timestamp_ms: fields[0].parse().ok()?,
        mode,
        baseline_run,
        skill: Skill::from_name(fields[3])?,
        item,
        gen_version: fields[5].parse().ok()?,
        correct: parse_flag(fields[6])?,
        close: parse_flag(fields[7])?,
        ev_loss_pct,
        response_ms: fields[9].parse::<u32>().ok()?.min(MAX_RESPONSE_MS),
        answer: fields[10].to_string(),
    })
}

/// Parses log text, oldest attempt first. Malformed lines are skipped and
/// described in the warnings.
pub fn parse_log(text: &str) -> (Vec<Attempt>, Vec<String>) {
    let mut attempts = Vec::new();
    let mut warnings = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.is_empty() || line == LOG_HEADER {
            continue;
        }
        match parse_line(line) {
            Some(attempt) => attempts.push(attempt),
            None => warnings.push(format!("attempt log line {} is malformed and was skipped", index + 1)),
        }
    }
    attempts.sort_by_key(|a| a.timestamp_ms);
    (attempts, warnings)
}
