//! Pass marks and progress. All marks and EV bands are starting guesses, to be
//! tuned once there is real data.

use crate::item::Skill;
use crate::log::{Attempt, Mode};

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
}

const fn accuracy(pct: f64, median_ms: Option<u32>) -> PassMark {
    PassMark {
        min_correct_pct: Some(pct),
        min_within_one_pct: None,
        max_median_ms: median_ms,
        max_mean_ev_loss_pct: None,
        max_blunders: None,
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
};
pub const PASS_PRICE: PassMark = accuracy(95.0, Some(6_000));
pub const PASS_HIT: PassMark = accuracy(90.0, None);
pub const PASS_SPOT: PassMark = PassMark {
    min_correct_pct: None,
    min_within_one_pct: None,
    max_median_ms: None,
    max_mean_ev_loss_pct: Some(2.0),
    max_blunders: Some(2),
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
    }
}

/// EV loss above this percentage of P counts as a blunder.
const BLUNDER_PCT: f64 = crate::grade::EV_MISTAKE_PCT;

pub const STEPS: [&[Skill]; 3] = [
    &[Skill::Nuts, Skill::Possible, Skill::Made, Skill::Beats],
    &[Skill::Outs, Skill::Price, Skill::Hit],
    &[Skill::Spot, Skill::SpotMw],
];

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
    pub enough_data: bool,
    pub passed: bool,
}

/// Only `drill` attempts count for pass marks. The baseline is measured
/// separately and same-session retries follow a reveal.
fn counts_for_stats(attempt: &Attempt) -> bool {
    attempt.mode == Mode::Drill
}

pub fn skill_stats(skill: Skill, attempts: &[Attempt]) -> SkillStats {
    let all: Vec<&Attempt> = attempts
        .iter()
        .filter(|a| a.skill == skill && counts_for_stats(a))
        .collect();
    let recent = &all[all.len().saturating_sub(STAT_WINDOW)..];
    let n = recent.len();
    let denom = n.max(1) as f64;
    let correct_pct = 100.0 * recent.iter().filter(|a| a.correct).count() as f64 / denom;
    let within_one_pct = 100.0 * recent.iter().filter(|a| a.ok()).count() as f64 / denom;
    let mut times: Vec<u32> = recent.iter().map(|a| a.response_ms).collect();
    times.sort_unstable();
    let median_ms = match times.len() {
        0 => 0,
        len if len % 2 == 1 => times[len / 2],
        len => (times[len / 2 - 1] + times[len / 2]) / 2,
    };
    let losses: Vec<f64> = recent.iter().filter_map(|a| a.ev_loss_pct).collect();
    let mean_ev_loss_pct = losses.iter().sum::<f64>() / losses.len().max(1) as f64;
    let blunders = losses.iter().filter(|&&l| l > BLUNDER_PCT).count() as u32;
    let enough_data = all.len() >= MIN_ATTEMPTS;

    let mark = pass_mark(skill);
    let passed = all.len() >= PASS_ATTEMPTS
        && mark.min_correct_pct.is_none_or(|m| correct_pct >= m)
        && mark.min_within_one_pct.is_none_or(|m| within_one_pct >= m)
        && mark.max_median_ms.is_none_or(|m| median_ms <= m)
        && mark.max_mean_ev_loss_pct.is_none_or(|m| mean_ev_loss_pct <= m)
        && mark.max_blunders.is_none_or(|m| blunders <= m);
    SkillStats {
        skill,
        total_attempts: all.len(),
        window: n,
        correct_pct,
        within_one_pct,
        median_ms,
        mean_ev_loss_pct,
        blunders,
        enough_data,
        passed,
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

/// The first step not yet passed (1 to 3); 3 once every step is passed.
pub fn current_step(stats: &[SkillStats]) -> usize {
    (1..=STEPS.len())
        .find(|&step| !step_passed(step, stats))
        .unwrap_or(STEPS.len())
}

/// `preflop` plus every skill up to and including `step`.
pub fn skills_up_to_step(step: usize) -> Vec<Skill> {
    let mut out = vec![Skill::Preflop];
    for skills in &STEPS[..step] {
        out.extend_from_slice(skills);
    }
    out
}

/// Which skills a drill deals: exactly the `requested` ones, or if none are
/// requested, `preflop` plus every skill up to the player's current step.
pub fn drill_skills(requested: &[Skill], attempts: &[Attempt]) -> Vec<Skill> {
    if requested.is_empty() {
        return skills_up_to_step(current_step(&all_stats(attempts)));
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

/// Scores of every baseline run, oldest first.
pub fn baseline_runs(attempts: &[Attempt], baseline_size: usize) -> Vec<BaselineRun> {
    let mut ids: Vec<u32> = attempts
        .iter()
        .filter(|a| a.mode == Mode::Baseline)
        .filter_map(|a| a.baseline_run)
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids.into_iter()
        .map(|run| {
            let rows: Vec<&Attempt> = attempts
                .iter()
                .filter(|a| a.mode == Mode::Baseline && a.baseline_run == Some(run))
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
            let steps = (1..=STEPS.len())
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
