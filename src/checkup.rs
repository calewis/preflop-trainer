//! Held-out checkups: the yardstick. A checkup is 40 new held-out items plus 10
//! suit-permuted repeats, deterministic from (skill, form). No feedback until
//! the end, and nothing here counts as drill practice.

use crate::anchors;
use crate::board_type::{seed_with_category, BoardCategory};
use crate::generate::generate;
use crate::item::{equity_seed_with_rung, Item, Question, Skill, HELD_OUT_BIT};
use crate::log::{Attempt, CheckRef, Mode};
use crate::progress::{meets_mark, pass_mark, window_metrics, PassMark, WindowMetrics};
use crate::rng::Rng;

/// Fresh items scored in a checkup.
pub const CHECKUP_ORIGINALS: usize = 40;
/// Repeats, kept out of the score as a noise measure.
pub const CHECKUP_REPEATS: usize = 10;
pub const CHECKUP_LEN: usize = CHECKUP_ORIGINALS + CHECKUP_REPEATS;
/// A repeat comes at least this many items after its original. Starting guess.
pub const MIN_REPEAT_GAP: usize = 10;
/// `equity` tolerance in a checkup, in points. Starting guess.
pub const CHECKUP_EQUITY_BAND: u8 = 5;

const FORM_SALT: u64 = 0x4368_6563_6B75_7031;
const SKILL_MIX: u64 = 0x9E37_79B9_7F4A_7C15;
const FORM_MIX: u64 = 0xD6E8_FEB8_6659_FD93;

/// One position in a checkup.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CheckupSlot {
    pub seed: u64,
    /// For a repeat, the earlier slot it copies.
    pub repeat_of: Option<usize>,
    /// The suit relabelling applied to a repeat.
    pub perm: [u8; 4],
}

/// A checkup form: every slot's seed and repeat, with no items generated yet.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CheckupPlan {
    pub skill: Skill,
    pub form: u32,
    pub slots: Vec<CheckupSlot>,
}

const IDENTITY: [u8; 4] = [0, 1, 2, 3];

fn original_seeds(skill: Skill, rng: &mut Rng) -> Vec<u64> {
    let deck_len = anchors::deck().len().max(1);
    (0..CHECKUP_ORIGINALS)
        .map(|i| {
            let seed = rng.next_u64() | HELD_OUT_BIT;
            match skill {
                // A fixed composition keeps the score comparable between forms.
                Skill::Equity => equity_seed_with_rung(seed, (i % 2) as u8),
                Skill::BoardType => seed_with_category(seed, BoardCategory::ALL[i % BoardCategory::ALL.len()]),
                Skill::Anchor => anchors::seed_with_card(seed, i % deck_len),
                _ => seed,
            }
        })
        .collect()
}

fn non_identity_perm(rng: &mut Rng) -> [u8; 4] {
    loop {
        let mut perm = IDENTITY;
        rng.shuffle(&mut perm);
        if perm != IDENTITY {
            return perm;
        }
    }
}

/// Builds form `form` of `skill`. The same arguments always give the same plan.
pub fn checkup_plan(skill: Skill, form: u32) -> CheckupPlan {
    let mix = (skill as u64 + 1).wrapping_mul(SKILL_MIX) ^ (u64::from(form) + 1).wrapping_mul(FORM_MIX);
    let mut rng = Rng::new(mix ^ FORM_SALT);
    let mut seeds = original_seeds(skill, &mut rng);
    rng.shuffle(&mut seeds);

    let mut repeat_positions: Vec<usize> = (MIN_REPEAT_GAP..CHECKUP_LEN).collect();
    rng.shuffle(&mut repeat_positions);
    repeat_positions.truncate(CHECKUP_REPEATS);
    repeat_positions.sort_unstable();

    let mut next_seed = seeds.into_iter();
    let mut slots: Vec<CheckupSlot> = (0..CHECKUP_LEN)
        .map(|position| CheckupSlot {
            seed: if repeat_positions.contains(&position) { 0 } else { next_seed.next().expect("40 seeds") },
            repeat_of: None,
            perm: IDENTITY,
        })
        .collect();

    let mut used = [false; CHECKUP_LEN];
    for &position in &repeat_positions {
        // Slots 0..MIN_REPEAT_GAP are never repeats, so an unused original always exists here.
        let candidates: Vec<usize> = (0..=position - MIN_REPEAT_GAP)
            .filter(|&q| !used[q] && !repeat_positions.contains(&q))
            .collect();
        let original = rng.pick(&candidates);
        used[original] = true;
        slots[position].seed = slots[original].seed;
        slots[position].repeat_of = Some(original);
        slots[position].perm = non_identity_perm(&mut rng);
    }
    CheckupPlan { skill, form, slots }
}

impl CheckupPlan {
    /// The item for one slot. Repeats relabel suits; `equity` uses the fixed band.
    pub fn item(&self, index: usize) -> Item {
        let slot = self.slots[index];
        let mut item = generate(self.skill, slot.seed);
        if let Question::Equity(q) = &mut item.question {
            q.band = CHECKUP_EQUITY_BAND;
        }
        if slot.repeat_of.is_some() {
            item.question = item.question.with_suit_perm(&slot.perm);
        }
        item
    }
}

/// The primary measure for a skill's checkup score.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum CheckupMetric {
    /// Percent answered correctly, higher is better.
    PercentCorrect,
    /// Percent correct or close, higher is better (`outs`).
    PercentWithinOne,
    /// Mean EV loss as a percentage of the pot, lower is better (`spot`).
    MeanEvLoss,
    /// Mean absolute error in points, lower is better (`equity`).
    MeanAbsError,
}

/// The measure a pass mark is judged on.
pub fn checkup_metric(mark: &PassMark) -> CheckupMetric {
    if mark.max_mean_abs_error.is_some() {
        CheckupMetric::MeanAbsError
    } else if mark.max_mean_ev_loss_pct.is_some() {
        CheckupMetric::MeanEvLoss
    } else if mark.min_within_one_pct.is_some() {
        CheckupMetric::PercentWithinOne
    } else {
        CheckupMetric::PercentCorrect
    }
}

/// Agreement between the repeats and their originals, as a noise measure.
#[derive(Clone, PartialEq, Debug)]
pub struct RepeatAgreement {
    pub pairs: usize,
    /// Pairs where both answers were right or both were wrong.
    pub agree: usize,
    pub agree_pct: f64,
    /// Mean absolute gap between the two numeric answers, when both were numbers.
    pub mean_abs_diff: Option<f64>,
}

/// One checkup run's result, scored on the 40 originals only.
#[derive(Clone, PartialEq, Debug)]
pub struct CheckupScore {
    pub skill: Skill,
    pub run: u32,
    pub form: u32,
    pub started_ms: u64,
    pub answered: usize,
    pub complete: bool,
    pub metric: CheckupMetric,
    /// The primary score, in the units of `metric`.
    pub score: f64,
    pub standard_error: f64,
    pub metrics: WindowMetrics,
    pub agreement: RepeatAgreement,
    /// Complete and meets the skill's pass mark on the originals.
    pub passed: bool,
}

/// The next form index for `skill`, one past every form ever started, so no
/// item is ever seen in two forms.
pub fn next_checkup_form(skill: Skill, attempts: &[Attempt]) -> u32 {
    attempts
        .iter()
        .filter(|a| a.skill == skill)
        .filter_map(|a| a.extra.check.map(|c| c.form))
        .max()
        .map_or(0, |form| form.saturating_add(1))
}

/// The run id for a new checkup.
pub fn next_checkup_run(attempts: &[Attempt]) -> u32 {
    attempts.iter().filter_map(|a| a.extra.check.map(|c| c.run)).max().unwrap_or(0).saturating_add(1)
}

/// Assumes the items are independent. `hit` has only 24 distinct questions, so
/// its 40 originals always contain duplicates and this figure is optimistic.
fn proportion_se(pct: f64, n: usize) -> f64 {
    if n == 0 {
        return 0.0;
    }
    let p = pct / 100.0;
    100.0 * (p * (1.0 - p) / n as f64).sqrt()
}

fn mean_se(values: &[f64]) -> f64 {
    if values.len() < 2 {
        return 0.0;
    }
    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;
    let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1.0);
    (variance / n).sqrt()
}

fn primary_score(metric: CheckupMetric, rows: &[&Attempt], metrics: &WindowMetrics) -> (f64, f64) {
    match metric {
        CheckupMetric::PercentCorrect => (metrics.correct_pct, proportion_se(metrics.correct_pct, rows.len())),
        CheckupMetric::PercentWithinOne => (metrics.within_one_pct, proportion_se(metrics.within_one_pct, rows.len())),
        CheckupMetric::MeanEvLoss => {
            let values: Vec<f64> = rows.iter().filter_map(|a| a.ev_loss_pct).collect();
            (metrics.mean_ev_loss_pct, mean_se(&values))
        }
        CheckupMetric::MeanAbsError => {
            let values: Vec<f64> = rows.iter().filter_map(|a| a.extra.signed_error.map(f64::abs)).collect();
            (metrics.mean_abs_error.unwrap_or(0.0), mean_se(&values))
        }
    }
}

fn agreement(plan: &CheckupPlan, by_slot: &[Option<&Attempt>]) -> RepeatAgreement {
    let mut pairs = 0;
    let mut agree = 0;
    let mut gaps: Vec<f64> = Vec::new();
    for (index, slot) in plan.slots.iter().enumerate() {
        let Some(original) = slot.repeat_of else { continue };
        let (Some(first), Some(second)) = (by_slot[original], by_slot[index]) else { continue };
        pairs += 1;
        agree += usize::from(first.ok() == second.ok());
        if let (Ok(a), Ok(b)) = (first.answer.parse::<f64>(), second.answer.parse::<f64>()) {
            gaps.push((a - b).abs());
        }
    }
    RepeatAgreement {
        pairs,
        agree,
        agree_pct: 100.0 * agree as f64 / pairs.max(1) as f64,
        mean_abs_diff: (!gaps.is_empty()).then(|| gaps.iter().sum::<f64>() / gaps.len() as f64),
    }
}

fn score_run(skill: Skill, check: CheckRef, rows: &[&Attempt]) -> CheckupScore {
    let plan = checkup_plan(skill, check.form);
    let mut by_slot: Vec<Option<&Attempt>> = vec![None; CHECKUP_LEN];
    for attempt in rows {
        if let Some(check) = attempt.extra.check {
            if let Some(entry) = by_slot.get_mut(check.slot as usize) {
                entry.get_or_insert(attempt);
            }
        }
    }
    let originals: Vec<&Attempt> = plan
        .slots
        .iter()
        .zip(&by_slot)
        .filter(|(slot, _)| slot.repeat_of.is_none())
        .filter_map(|(_, attempt)| *attempt)
        .collect();
    let metrics = window_metrics(&originals);
    let mark = pass_mark(skill);
    let metric = checkup_metric(&mark);
    let (score, standard_error) = primary_score(metric, &originals, &metrics);
    let complete = by_slot.iter().all(Option::is_some);
    CheckupScore {
        skill,
        run: check.run,
        form: check.form,
        started_ms: rows.iter().map(|a| a.timestamp_ms).min().unwrap_or(0),
        answered: by_slot.iter().flatten().count(),
        complete,
        metric,
        score,
        standard_error,
        metrics,
        agreement: agreement(&plan, &by_slot),
        passed: complete && meets_mark(&mark, &metrics),
    }
}

/// Scores of every checkup run, oldest first.
pub fn checkup_runs(attempts: &[Attempt]) -> Vec<CheckupScore> {
    let mut starts: Vec<(u32, u32, Skill)> = Vec::new();
    for attempt in attempts.iter().filter(|a| a.mode == Mode::Check) {
        if let Some(check) = attempt.extra.check {
            if !starts.iter().any(|&(run, _, _)| run == check.run) {
                starts.push((check.run, check.form, attempt.skill));
            }
        }
    }
    starts
        .into_iter()
        .map(|(run, form, skill)| {
            let rows: Vec<&Attempt> = attempts
                .iter()
                .filter(|a| a.mode == Mode::Check && a.skill == skill && a.extra.check.is_some_and(|c| c.run == run))
                .collect();
            score_run(skill, CheckRef { run, form, slot: 0 }, &rows)
        })
        .collect()
}

/// The most recent completed checkup for a skill.
pub fn latest_checkup(skill: Skill, attempts: &[Attempt]) -> Option<CheckupScore> {
    checkup_runs(attempts).into_iter().rfind(|s| s.skill == skill && s.complete)
}
