//! Pure rules for the `equity` skill: the tolerance staircase, the ladder rung
//! and the bias report. Everything is computed from the attempt log. All
//! thresholds are starting guesses, to be tuned with real data.

use crate::item::{equity_rung_of_seed, Skill};
use crate::ladder::{before_old_rule_cutoff, GateStatus, PointsGate};
use crate::log::{Attempt, ItemRef, Mode};

/// Starting tolerance, in points either side of the true equity.
pub const EQUITY_BAND_START: u8 = 10;
pub const EQUITY_BAND_MIN: u8 = 2;
pub const EQUITY_BAND_MAX: u8 = 20;
/// Points the band tightens after a hit.
pub const EQUITY_BAND_TIGHTEN: u8 = 1;
/// Points the band loosens after a miss. With a tighten of 1 this settles near
/// 6 hits for every miss, about 86% hits.
pub const EQUITY_BAND_LOOSEN: u8 = 6;

/// Each rung-0 estimate adds this many points less its error, in whole points...
pub const PROMOTE_PAR_POINTS: i32 = 8;
/// ...counting an error over this as this, so one wild guess costs at most 12.
pub const PROMOTE_ERROR_CAP: i32 = 20;
/// The points gate from rung 0 to rung 1, shared in shape with the other
/// ladders (see [`crate::ladder`]). Points rise on average while the mean error
/// is under [`PROMOTE_PAR_POINTS`], the old mark. Simulated over 20,000 players
/// each, with errors spread like a half-normal or an exponential: a mean error of
/// 6 moves up within 60 attempts about 90% of the time, a mean error of 10 within
/// 100 attempts under 5%. The old rule, any 30 straight attempts averaging 8 or
/// less, moved a mean error of 10 up within 100 attempts about half the time.
pub const EQUITY_GATE: PointsGate = PointsGate { start: 30, goal: 110 };
/// The old rule, for attempts before [`crate::ladder::OLD_RULE_CUTOFF_MS`]: any this many
/// straight rung-0 attempts...
pub const OLD_PROMOTE_WINDOW: usize = 30;
/// ...with a mean absolute error at or under this.
pub const OLD_PROMOTE_MAX_MAE: f64 = 8.0;

/// Attempts needed before [`equity_bias`] is worth showing.
pub const BIAS_MIN_ATTEMPTS: usize = 30;
/// True equities are clamped to this range before taking logits.
pub const LOGIT_CLAMP_MIN_PCT: f64 = 1.0;
pub const LOGIT_CLAMP_MAX_PCT: f64 = 99.0;

/// The attempts that move the staircase and the rung: first-time drill answers.
fn drill_equity(attempts: &[Attempt]) -> impl Iterator<Item = &Attempt> {
    attempts.iter().filter(|a| a.skill == Skill::Equity && a.mode == Mode::Drill)
}

fn rung_of(attempt: &Attempt) -> Option<u8> {
    match attempt.item {
        ItemRef::Seed(seed) => Some(equity_rung_of_seed(seed)),
        ItemRef::Baseline(_) => None,
    }
}

/// One staircase step: tighten by 1 after a hit, loosen by 6 after a miss.
pub fn next_band(band: u8, hit: bool) -> u8 {
    if hit {
        band.saturating_sub(EQUITY_BAND_TIGHTEN).max(EQUITY_BAND_MIN)
    } else {
        band.saturating_add(EQUITY_BAND_LOOSEN).min(EQUITY_BAND_MAX)
    }
}

/// The band to serve next at `rung`, replayed from the log.
pub fn equity_band(attempts: &[Attempt], rung: u8) -> u8 {
    drill_equity(attempts)
        .filter(|a| rung_of(a) == Some(rung))
        .fold(EQUITY_BAND_START, |band, a| next_band(band, a.correct))
}

/// One rung-0 estimate's points: [`PROMOTE_PAR_POINTS`] less the error rounded
/// to whole points and capped at [`PROMOTE_ERROR_CAP`].
pub fn estimate_points(error: f64) -> i32 {
    // The cap comes first, so the cast never sees a huge value.
    let off = error.abs().min(f64::from(PROMOTE_ERROR_CAP)).round() as i32;
    PROMOTE_PAR_POINTS - off
}

fn rung_zero(attempts: &[Attempt]) -> impl Iterator<Item = &Attempt> {
    drill_equity(attempts).filter(|a| rung_of(a) == Some(0)).filter(|a| a.extra.signed_error.is_some())
}

/// Points toward rung 1 from the log, and whether they have reached the goal.
pub fn equity_points(attempts: &[Attempt]) -> GateStatus {
    EQUITY_GATE.status(rung_zero(attempts).filter_map(|a| a.extra.signed_error).map(estimate_points))
}

/// The player's ladder rung: 1 once the rung-0 points reach [`EQUITY_GATE`]'s
/// goal, or once the old rule passed on attempts before [`crate::ladder::OLD_RULE_CUTOFF_MS`].
/// Promotion is permanent, so an old rung-0 item coming back for review cannot
/// demote the player.
pub fn equity_rung(attempts: &[Attempt]) -> u8 {
    let old: Vec<f64> = rung_zero(attempts)
        .filter(|a| before_old_rule_cutoff(a))
        .filter_map(|a| a.extra.signed_error.map(f64::abs))
        .collect();
    let old_passed = old.windows(OLD_PROMOTE_WINDOW).any(|window| {
        window.iter().sum::<f64>() / window.len() as f64 <= OLD_PROMOTE_MAX_MAE
    });
    u8::from(old_passed || equity_points(attempts).passed)
}

/// Mean signed error (estimate minus truth, in points) for one group of attempts.
#[derive(Clone, PartialEq, Debug)]
pub struct BiasBucket {
    pub label: String,
    pub count: usize,
    /// Positive means overestimating. 0 when the bucket is empty.
    pub mean_signed_error: f64,
}

/// `logit(estimate) = intercept + slope * logit(truth)`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct LogitFit {
    pub intercept: f64,
    pub slope: f64,
    pub count: usize,
}

#[derive(Clone, PartialEq, Debug)]
pub struct EquityBias {
    pub count: usize,
    pub mean_signed_error: f64,
    /// One bucket each for 1, 2 and 3 opponents.
    pub by_opponents: Vec<BiasBucket>,
    /// True equity bands 0-20, 20-40, 40-60, 60-80 and 80-100.
    pub by_truth_band: Vec<BiasBucket>,
    /// `None` without at least two different true equities.
    pub fit: Option<LogitFit>,
}

const TRUTH_BANDS: [(f64, f64, &str); 5] = [
    (0.0, 20.0, "0-20"),
    (20.0, 40.0, "20-40"),
    (40.0, 60.0, "40-60"),
    (60.0, 80.0, "60-80"),
    (80.0, 100.0, "80-100"),
];

struct Sample {
    signed: f64,
    truth: f64,
    estimate: f64,
    opponents: u8,
}

fn samples(attempts: &[Attempt]) -> Vec<Sample> {
    attempts
        .iter()
        .filter(|a| a.skill == Skill::Equity && a.mode != Mode::DrillRetry)
        .filter_map(|a| {
            let signed = a.extra.signed_error?;
            let truth = a.extra.truth_pct?;
            Some(Sample { signed, truth, estimate: truth + signed, opponents: a.extra.opponents? })
        })
        .collect()
}

fn bucket<'a>(label: String, members: impl Iterator<Item = &'a Sample>) -> BiasBucket {
    let values: Vec<f64> = members.map(|s| s.signed).collect();
    let mean_signed_error = values.iter().sum::<f64>() / values.len().max(1) as f64;
    BiasBucket { label, count: values.len(), mean_signed_error }
}

fn logit(pct: f64) -> f64 {
    let p = pct.clamp(LOGIT_CLAMP_MIN_PCT, LOGIT_CLAMP_MAX_PCT) / 100.0;
    (p / (1.0 - p)).ln()
}

fn logit_fit(all: &[Sample]) -> Option<LogitFit> {
    let points: Vec<(f64, f64)> = all.iter().map(|s| (logit(s.truth), logit(s.estimate))).collect();
    let n = points.len() as f64;
    let mean_x = points.iter().map(|p| p.0).sum::<f64>() / n;
    let mean_y = points.iter().map(|p| p.1).sum::<f64>() / n;
    let sxx: f64 = points.iter().map(|p| (p.0 - mean_x).powi(2)).sum();
    if sxx <= f64::EPSILON {
        return None;
    }
    let sxy: f64 = points.iter().map(|p| (p.0 - mean_x) * (p.1 - mean_y)).sum();
    let slope = sxy / sxx;
    Some(LogitFit { intercept: mean_y - slope * mean_x, slope, count: points.len() })
}

/// Calibration of the player's equity estimates. Same-session retries follow a
/// reveal, so they are left out; drills and checkups both count.
pub fn equity_bias(attempts: &[Attempt]) -> EquityBias {
    let all = samples(attempts);
    let by_opponents = (1..=3u8)
        .map(|n| {
            let label = format!("{n} opponent{}", if n == 1 { "" } else { "s" });
            bucket(label, all.iter().filter(move |s| s.opponents == n))
        })
        .collect();
    let by_truth_band = TRUTH_BANDS
        .iter()
        .map(|&(low, high, label)| {
            // The top band includes 100, every other band excludes its upper edge.
            let inside = move |s: &&Sample| s.truth >= low && (s.truth < high || high == 100.0);
            bucket(label.to_string(), all.iter().filter(inside))
        })
        .collect();
    EquityBias {
        count: all.len(),
        mean_signed_error: all.iter().map(|s| s.signed).sum::<f64>() / all.len().max(1) as f64,
        by_opponents,
        by_truth_band,
        fit: logit_fit(&all),
    }
}
