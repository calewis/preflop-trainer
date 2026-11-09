//! The attempt log as TSV text. Reading and writing the file is the caller's job.

use crate::baseline::BaselineItem;
use crate::grade::{Outcome, Reveal};
use crate::item::{ChainStage, Skill, GEN_VERSION};
use crate::reflection::ReflectState;
use crate::schedule::{Origin, Planned};
use crate::table::{self, Verdict};

/// Longest response time recorded.
pub const MAX_RESPONSE_MS: u32 = 60_000;

/// First line of a new log file. Parsers skip it.
pub const LOG_HEADER: &str = "timestamp_ms\tmode\tbaseline_run\tskill\titem\tgen_version\tcorrect\tclose\tev_loss_pct\tresponse_ms\tanswer\tcheck_run\tcheck_form\tcheck_slot\tband\tsigned_error\ttruth_pct\topponents\ttable_spot\ttable_verdict\tfamily\tstage\tconfidence\treflect";
/// The header of logs written before the family, stage and reflection columns existed.
pub const TABLE_LOG_HEADER: &str = "timestamp_ms\tmode\tbaseline_run\tskill\titem\tgen_version\tcorrect\tclose\tev_loss_pct\tresponse_ms\tanswer\tcheck_run\tcheck_form\tcheck_slot\tband\tsigned_error\ttruth_pct\topponents\ttable_spot\ttable_verdict";
/// The header of logs written before the table columns existed.
pub const CHECKUP_LOG_HEADER: &str = "timestamp_ms\tmode\tbaseline_run\tskill\titem\tgen_version\tcorrect\tclose\tev_loss_pct\tresponse_ms\tanswer\tcheck_run\tcheck_form\tcheck_slot\tband\tsigned_error\ttruth_pct\topponents";
/// The header of logs written before the extra columns existed.
pub const LEGACY_LOG_HEADER: &str = "timestamp_ms\tmode\tbaseline_run\tskill\titem\tgen_version\tcorrect\tclose\tev_loss_pct\tresponse_ms\tanswer";

/// Columns in a line with no extras, in one with the original extras, in a table
/// row, and in a row with the family, stage, confidence and reflection columns.
const BASE_COLUMNS: usize = 11;
const FULL_COLUMNS: usize = 18;
const TABLE_COLUMNS: usize = 20;
const NOTES_COLUMNS: usize = 24;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Mode {
    Baseline,
    Drill,
    /// A missed item asked again in the same session; not counted in pass marks.
    DrillRetry,
    /// A held-out checkup item. Never counted as drill practice.
    Check,
    /// Not an attempt: the player's answers to a reflection form. Everything that
    /// counts attempts filters by the modes above, so these rows are invisible to it.
    Reflect,
}

impl Mode {
    pub fn code(self) -> &'static str {
        match self {
            Mode::Baseline => "baseline",
            Mode::Drill => "drill",
            Mode::DrillRetry => "drill-retry",
            Mode::Check => "check",
            Mode::Reflect => "reflect",
        }
    }

    pub fn from_code(code: &str) -> Option<Mode> {
        match code {
            "baseline" => Some(Mode::Baseline),
            "drill" => Some(Mode::Drill),
            "drill-retry" => Some(Mode::DrillRetry),
            "check" => Some(Mode::Check),
            "reflect" => Some(Mode::Reflect),
            _ => None,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ItemRef {
    Seed(u64),
    Baseline(String),
}

/// Which checkup slot an attempt answered.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CheckRef {
    pub run: u32,
    pub form: u32,
    pub slot: u32,
}

/// How sure the player said they were, on the skills that ask.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Confidence {
    Low,
    Medium,
    High,
}

impl Confidence {
    pub const ALL: [Confidence; 3] = [Confidence::Low, Confidence::Medium, Confidence::High];

    pub fn code(self) -> &'static str {
        match self {
            Confidence::Low => "low",
            Confidence::Medium => "medium",
            Confidence::High => "high",
        }
    }

    pub fn from_code(code: &str) -> Option<Confidence> {
        Confidence::ALL.into_iter().find(|c| c.code() == code)
    }

    /// The chance of being right that this level is treated as. Starting guesses.
    pub fn treated_pct(self) -> f64 {
        match self {
            Confidence::Low => 55.0,
            Confidence::Medium => 75.0,
            Confidence::High => 95.0,
        }
    }
}

/// The skills that may ask for a confidence tap. `spot`, `spot_mw` and `table`
/// are left out on purpose: those decks are near break-even, so a rating would
/// measure the deck more than the player.
pub const fn asks_confidence(skill: Skill) -> bool {
    matches!(skill, Skill::Made | Skill::Outs | Skill::Price | Skill::Equity)
}

/// Columns added after the original eleven. An empty value means not applicable.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct AttemptExtra {
    pub check: Option<CheckRef>,
    /// The `equity` tolerance band served, in points.
    pub band: Option<u8>,
    /// `equity`: estimate minus truth, in percentage points.
    pub signed_error: Option<f64>,
    pub truth_pct: Option<f64>,
    pub opponents: Option<u8>,
    /// `table`: which spot was answered and its verdict, so pass marks and the
    /// no-repeat window don't need the spot library. The id only means anything
    /// in the library that row's `gen_version` names.
    pub table_spot: Option<u16>,
    pub table_verdict: Option<Verdict>,
    /// The item's family, so scheduling and comparisons needn't regenerate it.
    pub family: Option<String>,
    /// `chain`: which of the four stages this row answers.
    pub stage: Option<ChainStage>,
    /// `made`, `outs`, `price`, `equity`: the player's rating, if they gave one.
    pub confidence: Option<Confidence>,
    /// Whether the reflection form applied, or the outcome of one.
    pub reflect: Option<ReflectState>,
}

impl AttemptExtra {
    fn is_empty(&self) -> bool {
        *self == AttemptExtra::default()
    }

    fn has_notes(&self) -> bool {
        self.family.is_some() || self.stage.is_some() || self.confidence.is_some() || self.reflect.is_some()
    }
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
    pub extra: AttemptExtra,
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

    /// The log record for an answered checkup slot.
    pub fn from_check(
        check: CheckRef,
        skill: Skill,
        seed: u64,
        outcome: &Outcome,
        timestamp_ms: u64,
        response_ms: u32,
    ) -> Attempt {
        let mut attempt =
            Attempt::record(Mode::Check, None, skill, ItemRef::Seed(seed), outcome, timestamp_ms, response_ms);
        attempt.extra.check = Some(check);
        attempt
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
        let mut extra = outcome.equity_log.map_or_else(AttemptExtra::default, |e| AttemptExtra {
            check: None,
            band: Some(e.band),
            signed_error: Some(e.signed_error),
            truth_pct: Some(e.truth_pct),
            opponents: Some(e.opponents),
            ..AttemptExtra::default()
        });
        if let Reveal::Table(graded) = &outcome.reveal {
            extra.table_spot = Some(graded.spot.id);
            extra.table_verdict = Some(graded.spot.verdict);
        }
        extra.family = outcome.family.clone();
        extra.stage = outcome.chain_stage;
        Attempt {
            timestamp_ms,
            mode,
            baseline_run,
            skill,
            item,
            gen_version: if skill == Skill::Table { table::LIBRARY_GEN_VERSION } else { GEN_VERSION },
            correct: outcome.correct,
            close: outcome.close,
            ev_loss_pct: outcome.ev_loss_pct,
            response_ms: response_ms.min(MAX_RESPONSE_MS),
            answer: outcome.answer.clone(),
            extra,
        }
    }

    /// False for a `table` row from an earlier spot library: it still counts as
    /// practice, but its spot id names a different spot now.
    pub fn in_current_library(&self) -> bool {
        self.skill != Skill::Table || self.gen_version == table::LIBRARY_GEN_VERSION
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

fn optional<T: ToString>(value: Option<T>) -> String {
    value.map_or(String::new(), |v| v.to_string())
}

fn extra_columns(extra: &AttemptExtra) -> String {
    let table = match (extra.table_spot, extra.table_verdict) {
        (Some(spot), Some(verdict)) => format!("\t{spot}\t{}", verdict.code()),
        // A later column needs the table columns, even when they are empty.
        _ if extra.has_notes() => "\t\t".to_string(),
        _ => String::new(),
    };
    let notes = if extra.has_notes() {
        format!(
            "\t{}\t{}\t{}\t{}",
            extra.family.as_deref().unwrap_or(""),
            extra.stage.map_or("", ChainStage::code),
            extra.confidence.map_or("", Confidence::code),
            extra.reflect.map_or("", ReflectState::code),
        )
    } else {
        String::new()
    };
    format!(
        "\t{}\t{}\t{}\t{}\t{}\t{}\t{}{table}{notes}",
        optional(extra.check.map(|c| c.run)),
        optional(extra.check.map(|c| c.form)),
        optional(extra.check.map(|c| c.slot)),
        optional(extra.band),
        extra.signed_error.map_or(String::new(), |v| format!("{v:.4}")),
        extra.truth_pct.map_or(String::new(), |v| format!("{v:.4}")),
        optional(extra.opponents),
    )
}

/// One log line, without a trailing newline. Tabs and newlines in the answer are
/// replaced. Attempts with no extras keep the original eleven columns.
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
    let extras = if a.extra.is_empty() { String::new() } else { extra_columns(&a.extra) };
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}{extras}",
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
    let extra = match fields.len() {
        BASE_COLUMNS => AttemptExtra::default(),
        FULL_COLUMNS | TABLE_COLUMNS | NOTES_COLUMNS => parse_extra(&fields[BASE_COLUMNS..])?,
        _ => return None,
    };
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
        extra,
    })
}

fn parse_optional<T: std::str::FromStr>(text: &str) -> Option<Option<T>> {
    if text.is_empty() {
        Some(None)
    } else {
        Some(Some(text.parse().ok()?))
    }
}

fn parse_finite(text: &str) -> Option<Option<f64>> {
    let value: Option<f64> = parse_optional(text)?;
    if value.is_some_and(|v| !v.is_finite()) {
        return None;
    }
    Some(value)
}

fn parse_extra(fields: &[&str]) -> Option<AttemptExtra> {
    let run: Option<u32> = parse_optional(fields[0])?;
    let form: Option<u32> = parse_optional(fields[1])?;
    let slot: Option<u32> = parse_optional(fields[2])?;
    let check = match (run, form, slot) {
        (Some(run), Some(form), Some(slot)) => Some(CheckRef { run, form, slot }),
        (None, None, None) => None,
        _ => return None,
    };
    Some(AttemptExtra {
        check,
        band: parse_optional(fields[3])?,
        signed_error: parse_finite(fields[4])?,
        truth_pct: parse_finite(fields[5])?,
        opponents: parse_optional(fields[6])?,
        table_spot: parse_table_spot(fields)?,
        table_verdict: parse_table_verdict(fields)?,
        family: fields.get(9).filter(|f| !f.is_empty()).map(|f| f.to_string()),
        stage: match fields.get(10).copied().unwrap_or("") {
            "" => None,
            code => Some(ChainStage::from_code(code)?),
        },
        confidence: match fields.get(11).copied().unwrap_or("") {
            "" => None,
            code => Some(Confidence::from_code(code)?),
        },
        reflect: match fields.get(12).copied().unwrap_or("") {
            "" => None,
            code => Some(ReflectState::from_code(code)?),
        },
    })
}

fn parse_table_spot(fields: &[&str]) -> Option<Option<u16>> {
    fields.get(7).map_or(Some(None), |text| parse_optional(text))
}

fn parse_table_verdict(fields: &[&str]) -> Option<Option<Verdict>> {
    match fields.get(8).copied().unwrap_or("") {
        "" => Some(None),
        code => Verdict::from_code(code).map(Some),
    }
}

/// Parses log text, oldest attempt first. Malformed lines are skipped and
/// described in the warnings.
pub fn parse_log(text: &str) -> (Vec<Attempt>, Vec<String>) {
    let mut attempts = Vec::new();
    let mut warnings = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.is_empty() || line == LOG_HEADER || line == TABLE_LOG_HEADER || line == CHECKUP_LOG_HEADER || line == LEGACY_LOG_HEADER {
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
