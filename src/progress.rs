//! Pass marks and progress. All marks and EV bands are starting guesses, to be
//! tuned once there is real data.

use crate::baseline;
use crate::beats_me::counts_for_beats_me_mark;
use crate::checkup::latest_checkup;
use crate::item::Skill;
use crate::log::{ItemRef, asks_confidence, Attempt, Confidence, Mode};
use crate::read_price::counts_for_read_price_mark;
use crate::table::Verdict;

/// Attempts considered for a pass mark.
pub const STAT_WINDOW: usize = 40;
/// A skill can only pass with this many recorded attempts.
pub const PASS_ATTEMPTS: usize = STAT_WINDOW;
/// Below this many attempts a skill shows "not enough data".
pub const MIN_ATTEMPTS: usize = 20;

/// Pass mark for one skill, over the last [`STAT_WINDOW`] attempts. Unset fields don't apply.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct PassMark {
    pub min_correct_pct: Option<f64>,
    pub min_within_one_pct: Option<f64>,
    pub max_median_ms: Option<u32>,
    pub max_mean_ev_loss_pct: Option<f64>,
    pub max_blunders: Option<u32>,
    /// Mean absolute error in percentage points (`equity`).
    pub max_mean_abs_error: Option<f64>,
}

const fn accuracy(pct: f64, median_ms: Option<u32>) -> PassMark {
    PassMark {
        min_correct_pct: Some(pct),
        min_within_one_pct: None,
        max_median_ms: median_ms,
        max_mean_ev_loss_pct: None,
        max_blunders: None,
        max_mean_abs_error: None,
    }
}

pub const PASS_PREFLOP: PassMark = accuracy(95.0, None);
pub const PASS_NUTS: PassMark = accuracy(95.0, Some(6_000));
pub const PASS_POSSIBLE: PassMark = accuracy(95.0, Some(6_000));
pub const PASS_MADE: PassMark = accuracy(95.0, Some(6_000));
pub const PASS_BEATS: PassMark = accuracy(80.0, Some(10_000));
pub const PASS_OUTS: PassMark = PassMark {
    min_correct_pct: None,
    min_within_one_pct: Some(90.0),
    max_median_ms: None,
    max_mean_ev_loss_pct: None,
    max_blunders: None,
    max_mean_abs_error: None,
};
pub const PASS_PRICE: PassMark = accuracy(95.0, Some(6_000));
pub const PASS_HIT: PassMark = accuracy(90.0, None);
pub const PASS_SPOT: PassMark = PassMark {
    min_correct_pct: None,
    min_within_one_pct: None,
    max_median_ms: None,
    max_mean_ev_loss_pct: Some(2.0),
    max_blunders: Some(2),
    max_mean_abs_error: None,
};
/// `table`: over the last 40 graded robust and close spots. Starting guesses.
pub const TABLE_MAX_MEAN_EV_LOSS_PCT: f64 = 3.0;
pub const TABLE_MAX_BLUNDERS: u32 = 2;
pub const PASS_TABLE: PassMark = PassMark {
    min_correct_pct: None,
    min_within_one_pct: None,
    max_median_ms: None,
    max_mean_ev_loss_pct: Some(TABLE_MAX_MEAN_EV_LOSS_PCT),
    max_blunders: Some(TABLE_MAX_BLUNDERS),
    max_mean_abs_error: None,
};
/// `beats_me`: over the last 40 attempts at rungs 2-3.
pub const BEATS_ME_MIN_CORRECT_PCT: f64 = 85.0;
pub const PASS_BEATS_ME: PassMark = accuracy(BEATS_ME_MIN_CORRECT_PCT, None);
/// `read_price`: over the last 40 attempts at rungs 2-4. Starting guess.
pub const READ_PRICE_MIN_CORRECT_PCT: f64 = 85.0;
pub const PASS_READ_PRICE: PassMark = accuracy(READ_PRICE_MIN_CORRECT_PCT, None);
/// Stage accuracy over the last 40 stages, outs within 1 counting as right.
/// Starting guess.
pub const PASS_CHAIN: PassMark = PassMark {
    min_correct_pct: None,
    min_within_one_pct: Some(85.0),
    max_median_ms: None,
    max_mean_ev_loss_pct: None,
    max_blunders: None,
    max_mean_abs_error: None,
};
pub const PASS_BOARD_TYPE: PassMark = accuracy(95.0, Some(crate::board_type::BOARD_TYPE_TARGET_MS));
pub const PASS_ANCHOR: PassMark = accuracy(90.0, None);
pub const PASS_EQUITY: PassMark = PassMark {
    min_correct_pct: None,
    min_within_one_pct: None,
    max_median_ms: None,
    max_mean_ev_loss_pct: None,
    max_blunders: None,
    max_mean_abs_error: Some(6.0),
};

pub fn pass_mark(skill: Skill) -> PassMark {
    match skill {
        Skill::Preflop => PASS_PREFLOP,
        Skill::Nuts => PASS_NUTS,
        Skill::Possible => PASS_POSSIBLE,
        Skill::Made => PASS_MADE,
        Skill::Beats => PASS_BEATS,
        Skill::Outs => PASS_OUTS,
        Skill::Price => PASS_PRICE,
        Skill::Hit => PASS_HIT,
        Skill::Spot | Skill::SpotMw => PASS_SPOT,
        Skill::BoardType => PASS_BOARD_TYPE,
        Skill::Equity => PASS_EQUITY,
        Skill::Anchor => PASS_ANCHOR,
        Skill::Table => PASS_TABLE,
        Skill::Chain => PASS_CHAIN,
        Skill::BeatsMe => PASS_BEATS_ME,
        Skill::ReadPrice => PASS_READ_PRICE,
    }
}

/// EV loss above this percentage of P counts as a blunder.
const BLUNDER_PCT: f64 = crate::grade::EV_MISTAKE_PCT;

pub const STEPS: [&[Skill]; 4] = [
    &[Skill::Nuts, Skill::Possible, Skill::Made, Skill::BeatsMe, Skill::BoardType],
    &[Skill::Outs, Skill::Price, Skill::Hit, Skill::Equity, Skill::Anchor],
    &[Skill::Spot, Skill::SpotMw, Skill::Chain],
    &[Skill::Table],
];

/// The generator version whose rows earned steps under [`OLD_STEPS`].
const OLD_GEN_VERSION: u32 = 1;
/// The step gates before `beats_me` replaced `beats` and `chain` joined step 3.
const OLD_STEPS: [&[Skill]; 4] = [
    &[Skill::Nuts, Skill::Possible, Skill::Made, Skill::Beats, Skill::BoardType],
    &[Skill::Outs, Skill::Price, Skill::Hit, Skill::Equity, Skill::Anchor],
    &[Skill::Spot, Skill::SpotMw],
    &[Skill::Table],
];

/// Steps the frozen baseline covers: it has no questions for later steps.
pub const BASELINE_STEPS: usize = 3;

#[derive(Clone, PartialEq, Debug)]
pub struct SkillStats {
    pub skill: Skill,
    pub total_attempts: usize,
    /// Attempts in the last-40 window.
    pub window: usize,
    pub correct_pct: f64,
    pub within_one_pct: f64,
    pub median_ms: u32,
    pub mean_ev_loss_pct: f64,
    pub blunders: u32,
    /// `equity` only: mean absolute error, in points, over the window.
    pub mean_abs_error: Option<f64>,
    pub enough_data: bool,
    /// The drill pass mark alone.
    pub drill_passed: bool,
    /// Whether the most recent completed checkup met the same standard;
    /// `None` until the skill has a completed checkup.
    pub checkup_passed: Option<bool>,
    /// The drill pass mark is met and so is the most recent checkup.
    pub passed: bool,
}

/// What a set of attempts scored on the pass-mark measures.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct WindowMetrics {
    pub correct_pct: f64,
    pub within_one_pct: f64,
    pub median_ms: u32,
    pub mean_ev_loss_pct: f64,
    pub blunders: u32,
    pub mean_abs_error: Option<f64>,
}

/// The pass-mark measures for a set of attempts.
pub fn window_metrics(rows: &[&Attempt]) -> WindowMetrics {
    let denom = rows.len().max(1) as f64;
    let correct_pct = 100.0 * rows.iter().filter(|a| a.correct).count() as f64 / denom;
    let within_one_pct = 100.0 * rows.iter().filter(|a| a.ok()).count() as f64 / denom;
    let mut times: Vec<u32> = rows.iter().map(|a| a.response_ms).collect();
    times.sort_unstable();
    let median_ms = match times.len() {
        0 => 0,
        len if len % 2 == 1 => times[len / 2],
        len => (times[len / 2 - 1] + times[len / 2]) / 2,
    };
    let losses: Vec<f64> = rows.iter().filter_map(|a| a.ev_loss_pct).collect();
    let mean_ev_loss_pct = losses.iter().sum::<f64>() / losses.len().max(1) as f64;
    let blunders = losses.iter().filter(|&&l| l > BLUNDER_PCT).count() as u32;
    let errors: Vec<f64> = rows.iter().filter_map(|a| a.extra.signed_error.map(f64::abs)).collect();
    let mean_abs_error = (!errors.is_empty()).then(|| errors.iter().sum::<f64>() / errors.len() as f64);
    WindowMetrics { correct_pct, within_one_pct, median_ms, mean_ev_loss_pct, blunders, mean_abs_error }
}

/// True when the measures meet a pass mark. A mark needing a measure the rows
/// don't have (mean absolute error without any logged errors) is not met.
pub fn meets_mark(mark: &PassMark, m: &WindowMetrics) -> bool {
    mark.min_correct_pct.is_none_or(|t| m.correct_pct >= t)
        && mark.min_within_one_pct.is_none_or(|t| m.within_one_pct >= t)
        && mark.max_median_ms.is_none_or(|t| m.median_ms <= t)
        && mark.max_mean_ev_loss_pct.is_none_or(|t| m.mean_ev_loss_pct <= t)
        && mark.max_blunders.is_none_or(|t| m.blunders <= t)
        && mark.max_mean_abs_error.is_none_or(|t| m.mean_abs_error.is_some_and(|e| e <= t))
}

/// Only `drill` attempts count for pass marks. The baseline is measured
/// separately and same-session retries follow a reveal.
fn counts_for_stats(attempt: &Attempt) -> bool {
    attempt.mode == Mode::Drill
        && attempt.extra.table_verdict != Some(Verdict::Depends)
        && (attempt.skill != Skill::BeatsMe || counts_for_beats_me_mark(attempt))
        && (attempt.skill != Skill::ReadPrice || counts_for_read_price_mark(attempt))
}

pub fn skill_stats(skill: Skill, attempts: &[Attempt]) -> SkillStats {
    let all: Vec<&Attempt> = attempts
        .iter()
        .filter(|a| a.skill == skill && counts_for_stats(a))
        .collect();
    let recent = &all[all.len().saturating_sub(STAT_WINDOW)..];
    let m = window_metrics(recent);
    let drill_passed = all.len() >= PASS_ATTEMPTS && meets_mark(&pass_mark(skill), &m);
    let checkup_passed = latest_checkup(skill, attempts).map(|score| score.passed);
    SkillStats {
        skill,
        total_attempts: all.len(),
        window: recent.len(),
        correct_pct: m.correct_pct,
        within_one_pct: m.within_one_pct,
        median_ms: m.median_ms,
        mean_ev_loss_pct: m.mean_ev_loss_pct,
        blunders: m.blunders,
        mean_abs_error: m.mean_abs_error,
        enough_data: all.len() >= MIN_ATTEMPTS,
        drill_passed,
        checkup_passed,
        passed: drill_passed && checkup_passed == Some(true),
    }
}

pub fn all_stats(attempts: &[Attempt]) -> Vec<SkillStats> {
    Skill::ALL.into_iter().map(|s| skill_stats(s, attempts)).collect()
}

pub fn step_passed(step: usize, stats: &[SkillStats]) -> bool {
    STEPS[step - 1]
        .iter()
        .all(|skill| stats.iter().any(|s| s.skill == *skill && s.passed))
}

/// The first step not yet passed; the last step once every step is passed.
pub fn current_step(stats: &[SkillStats]) -> usize {
    (1..=STEPS.len())
        .find(|&step| !step_passed(step, stats))
        .unwrap_or(STEPS.len())
}

/// The player's step: the higher of the step under today's gates and the step
/// the old gates give from rows logged before they changed. A player never drops
/// a step, and a new player can't skip `beats_me` by passing `beats`.
pub fn current_step_for(attempts: &[Attempt]) -> usize {
    let today = current_step(&all_stats(attempts));
    let old_rows: Vec<Attempt> = attempts.iter().filter(|a| a.gen_version == OLD_GEN_VERSION).cloned().collect();
    let old_stats = all_stats(&old_rows);
    let passed_old = |step: &&[Skill]| step.iter().all(|skill| old_stats.iter().any(|s| s.skill == *skill && s.passed));
    let earned = OLD_STEPS.iter().position(|step| !passed_old(step)).map_or(OLD_STEPS.len(), |i| i + 1);
    today.max(earned)
}

/// `preflop`, `beats` (kept in drills but in no step's pass requirement),
/// `read_price` (dealt from the first day, also outside the steps) and every
/// skill up to and including `step`.
pub fn skills_up_to_step(step: usize) -> Vec<Skill> {
    let mut out = vec![Skill::Preflop, Skill::Beats, Skill::ReadPrice];
    for skills in &STEPS[..step] {
        out.extend_from_slice(skills);
    }
    out
}

/// Which skills a drill deals: exactly the `requested` ones, or if none are
/// requested, `preflop` plus every skill up to the player's current step.
pub fn drill_skills(requested: &[Skill], attempts: &[Attempt]) -> Vec<Skill> {
    if requested.is_empty() {
        return skills_up_to_step(current_step_for(attempts));
    }
    let mut skills: Vec<Skill> = Vec::with_capacity(requested.len());
    for &skill in requested {
        if !skills.contains(&skill) {
            skills.push(skill);
        }
    }
    skills
}

/// True once any baseline question has been answered.
pub fn has_baseline(attempts: &[Attempt]) -> bool {
    attempts.iter().any(|a| a.mode == Mode::Baseline)
}

#[derive(Clone, PartialEq, Debug)]
pub struct SectionScore {
    pub skill: Skill,
    pub answered: usize,
    /// Correct or close (outs within 1, spot within the close band).
    pub ok: usize,
    /// Strictly correct.
    pub exact: usize,
}

#[derive(Clone, PartialEq, Debug)]
pub struct StepScore {
    pub step: usize,
    pub answered: usize,
    pub ok: usize,
}

#[derive(Clone, PartialEq, Debug)]
pub struct BaselineRun {
    pub run: u32,
    pub started_ms: u64,
    pub answered: usize,
    pub ok: usize,
    pub complete: bool,
    pub sections: Vec<SectionScore>,
    pub steps: Vec<StepScore>,
}

/// True for a baseline row that counts: rows for retired questions stay in the log
/// but don't affect a score or whether a run is complete, so runs stay comparable.
fn scored_baseline_row(attempt: &Attempt) -> bool {
    attempt.mode == Mode::Baseline && !matches!(&attempt.item, ItemRef::Baseline(id) if baseline::is_retired(id))
}

/// Scores of every baseline run, oldest first.
pub fn baseline_runs(attempts: &[Attempt], baseline_size: usize) -> Vec<BaselineRun> {
    let mut ids: Vec<u32> = attempts
        .iter()
        .filter(|a| scored_baseline_row(a))
        .filter_map(|a| a.baseline_run)
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids.into_iter()
        .map(|run| {
            let rows: Vec<&Attempt> = attempts
                .iter()
                .filter(|a| scored_baseline_row(a) && a.baseline_run == Some(run))
                .collect();
            let score = |skill: Skill| -> SectionScore {
                let list: Vec<&&Attempt> = rows.iter().filter(|a| a.skill == skill).collect();
                SectionScore {
                    skill,
                    answered: list.len(),
                    ok: list.iter().filter(|a| a.ok()).count(),
                    exact: list.iter().filter(|a| a.correct).count(),
                }
            };
            let sections: Vec<SectionScore> = Skill::ALL
                .into_iter()
                .map(score)
                .filter(|s| s.answered > 0)
                .collect();
            let steps = (1..=BASELINE_STEPS)
                .map(|step| {
                    let in_step: Vec<&SectionScore> =
                        sections.iter().filter(|s| s.skill.step() == Some(step as u8)).collect();
                    StepScore {
                        step,
                        answered: in_step.iter().map(|s| s.answered).sum(),
                        ok: in_step.iter().map(|s| s.ok).sum(),
                    }
                })
                .collect();
            BaselineRun {
                run,
                started_ms: rows.iter().map(|a| a.timestamp_ms).min().unwrap_or(0),
                answered: rows.len(),
                ok: rows.iter().filter(|a| a.ok()).count(),
                complete: rows.len() == baseline_size,
                sections,
                steps,
            }
        })
        .collect()
}

/// The run id for a new baseline attempt.
pub fn next_baseline_run(attempts: &[Attempt]) -> u32 {
    attempts.iter().filter_map(|a| a.baseline_run).max().unwrap_or(0).saturating_add(1)
}

/// Ratings needed per (level, skill) row before it is shown. Starting guess.
pub const RELIABILITY_MIN_RATINGS: usize = 30;

/// One row of the reliability table: how often a confidence level was right.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ReliabilityRow {
    pub skill: Skill,
    pub level: Confidence,
    pub ratings: usize,
    pub right: usize,
}

impl ReliabilityRow {
    pub fn right_pct(&self) -> f64 {
        100.0 * self.right as f64 / self.ratings.max(1) as f64
    }
}

/// Every (level, skill) row with at least one rating, from `drill` attempts only,
/// so a same-session retry after a reveal can't inflate the table.
pub fn reliability_table(attempts: &[Attempt]) -> Vec<ReliabilityRow> {
    let mut rows: Vec<ReliabilityRow> = Vec::new();
    for attempt in attempts.iter().filter(|a| a.mode == Mode::Drill && asks_confidence(a.skill)) {
        let Some(level) = attempt.extra.confidence else { continue };
        if !rows.iter().any(|r| r.skill == attempt.skill && r.level == level) {
            rows.push(ReliabilityRow { skill: attempt.skill, level, ratings: 0, right: 0 });
        }
        let row = rows.iter_mut().find(|r| r.skill == attempt.skill && r.level == level).expect("added above");
        row.ratings += 1;
        row.right += usize::from(attempt.ok());
    }
    rows.sort_by_key(|r| (Skill::ALL.iter().position(|s| *s == r.skill), Confidence::ALL.iter().position(|l| *l == r.level)));
    rows
}
