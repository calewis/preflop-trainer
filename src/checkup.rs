//! Held-out checkups: the yardstick. No feedback until the end, and nothing
//! here counts as drill practice or gates any drill.
//!
//! The one offered now is the mixed checkup: 40 held-out items across the seven
//! practice groups, deterministic from its form, offered at most once a month
//! (see [`mixed_checkup_offer`]). The older one-skill checkup (40 new items plus
//! 10 suit-permuted repeats, deterministic from (skill, form)) is kept so its
//! rows in a player's history still score, and for the command line.

use crate::anchors;
use crate::board_type::{seed_with_category, BoardCategory};
use crate::generate::generate;
use crate::groups::{group_of, Group};
use crate::item::{beats_me_seed_with_rung, chain_seed_for_stage, equity_seed_with_rung, generator_changed, read_price_seed_with_rung, ChainStage, BEATS_ME_RUNGS, GEN_VERSION, Item, Question, Skill, HELD_OUT_BIT, READ_PRICE_RUNGS};
use crate::log::{Attempt, CheckRef, Mode};
use crate::schedule::DAY_MS;
use crate::progress::{meets_mark, pass_mark, window_metrics, PassMark, WindowMetrics};
use crate::rng::Rng;
use crate::table;

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
const MIXED_SALT: u64 = 0x4D69_7865_6443_6B31;

/// Mixed checkup forms are stored from this number up, so their rows need no new
/// log column: a checkup row with a lower form belongs to a one-skill checkup.
pub const MIXED_FORM_BASE: u32 = 1_000_000;

/// True for the stored form number of a mixed checkup.
pub const fn is_mixed_form(form: u32) -> bool {
    form >= MIXED_FORM_BASE
}

/// Items per skill in the mixed checkup: 40, near the drill's group shares
/// (preflop 8, read the price 7, equity 7, whole hands 4, board reading 6, outs
/// 4, spots 4). Whole hands is one hand of four stages rather than six items, and
/// the two items that frees go to read the price and equity. `table` is left
/// out: its held-out spots are walked by its own forms.
pub const MIXED_CHECKUP_MIX: [(Skill, usize); 14] = [
    (Skill::Preflop, 8),
    (Skill::ReadPrice, 7),
    (Skill::Equity, 5),
    (Skill::Anchor, 2),
    (Skill::Chain, 4),
    (Skill::Nuts, 1),
    (Skill::Possible, 1),
    (Skill::Made, 1),
    (Skill::BoardType, 1),
    (Skill::BeatsMe, 2),
    (Skill::Outs, 2),
    (Skill::Hit, 2),
    (Skill::Spot, 2),
    (Skill::SpotMw, 2),
];
pub const MIXED_CHECKUP_LEN: usize = 40;

/// Days from the end of one completed mixed checkup until the next is offered.
pub const MIXED_CHECKUP_EVERY_DAYS: u64 = 30;
/// The first mixed checkup waits for this many drill answers, and for this many
/// days since the first one, so a new player isn't asked on day one. Starting guesses.
pub const FIRST_CHECKUP_MIN_DRILLS: usize = 100;
pub const FIRST_CHECKUP_MIN_DAYS: u64 = 7;

/// Which checkup a run is.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum CheckupKind {
    /// The older one-skill checkup.
    Skill(Skill),
    /// The monthly checkup across every group.
    Mixed,
}

impl CheckupKind {
    /// The code a front end passes: a skill's name, or `mixed`.
    pub fn code(self) -> &'static str {
        match self {
            CheckupKind::Skill(skill) => skill.name(),
            CheckupKind::Mixed => "mixed",
        }
    }

    pub fn from_code(code: &str) -> Option<CheckupKind> {
        if code == "mixed" {
            Some(CheckupKind::Mixed)
        } else {
            Skill::from_name(code).map(CheckupKind::Skill)
        }
    }

    /// The kind of checkup a logged row answered, if it is a checkup row.
    pub fn of_row(attempt: &Attempt) -> Option<CheckupKind> {
        let check = attempt.extra.check?;
        Some(if is_mixed_form(check.form) { CheckupKind::Mixed } else { CheckupKind::Skill(attempt.skill) })
    }

    pub fn skill(self) -> Option<Skill> {
        match self {
            CheckupKind::Skill(skill) => Some(skill),
            CheckupKind::Mixed => None,
        }
    }
}
const SKILL_MIX: u64 = 0x9E37_79B9_7F4A_7C15;
const FORM_MIX: u64 = 0xD6E8_FEB8_6659_FD93;

/// One position in a checkup.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CheckupSlot {
    pub skill: Skill,
    pub seed: u64,
    /// For a repeat, the earlier slot it copies.
    pub repeat_of: Option<usize>,
    /// The suit relabelling applied to a repeat.
    pub perm: [u8; 4],
}

/// A checkup form: every slot's seed and repeat, with no items generated yet.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CheckupPlan {
    pub kind: CheckupKind,
    /// The form as stored in the log ([`MIXED_FORM_BASE`] and up for a mixed checkup).
    pub form: u32,
    pub slots: Vec<CheckupSlot>,
}

const IDENTITY: [u8; 4] = [0, 1, 2, 3];

fn original_seeds(skill: Skill, form: u32, rng: &mut Rng) -> Vec<u64> {
    let deck_len = anchors::deck().len().max(1);
    (0..CHECKUP_ORIGINALS)
        .map(|i| {
            let seed = rng.next_u64() | HELD_OUT_BIT;
            match skill {
                // A fixed composition keeps the score comparable between forms.
                Skill::Equity => equity_seed_with_rung(seed, (i % 2) as u8),
                // Ten items per rung.
                Skill::BeatsMe => beats_me_seed_with_rung(seed, (i % usize::from(BEATS_ME_RUNGS)) as u8),
                // Eight items per rung.
                Skill::ReadPrice => read_price_seed_with_rung(seed, (i % usize::from(READ_PRICE_RUNGS)) as u8),
                Skill::BoardType => seed_with_category(seed, BoardCategory::ALL[i % BoardCategory::ALL.len()]),
                Skill::Anchor => anchors::seed_with_card(seed, i % deck_len),
                // A fixed robust and close mix, walking the reserved spots form by form.
                Skill::Table => table::seed_with_index(seed, table::checkup_pool_index(form, i)),
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

/// Hands in a `chain` checkup: four stages each make the 40 scored items.
pub const CHAIN_CHECKUP_HANDS: usize = CHECKUP_ORIGINALS / 4;

/// `chain` keeps a hand's four stages together, in order, and places its repeats
/// only between hands. Repeats copy single stages from earlier hands.
fn chain_plan(form: u32, rng: &mut Rng) -> CheckupPlan {
    let mut stages: Vec<CheckupSlot> = Vec::with_capacity(CHECKUP_LEN);
    for _ in 0..CHAIN_CHECKUP_HANDS {
        let hand = rng.next_u64() | HELD_OUT_BIT;
        for stage in ChainStage::ALL {
            stages.push(CheckupSlot { skill: Skill::Chain, seed: chain_seed_for_stage(hand, stage), repeat_of: None, perm: IDENTITY });
        }
    }
    // Copy from the first six hands, so every repeat fits at least ten slots later.
    let mut pool: Vec<usize> = (0..24).collect();
    rng.shuffle(&mut pool);
    pool.truncate(CHECKUP_REPEATS);
    pool.sort_unstable();
    let mut slots = Vec::with_capacity(CHECKUP_LEN);
    let mut pending: Vec<usize> = pool;
    for hand in 0..CHAIN_CHECKUP_HANDS {
        for stage in 0..4 {
            slots.push(stages[hand * 4 + stage]);
        }
        let boundaries_left = CHAIN_CHECKUP_HANDS - hand;
        let quota = pending.len().div_ceil(boundaries_left);
        let mut placed = 0;
        pending.retain(|&stage_index| {
            let original_position = position_of_original(&slots, stage_index);
            match original_position {
                Some(at) if placed < quota && slots.len() >= at + MIN_REPEAT_GAP => {
                    let seed = stages[stage_index].seed;
                    let perm = non_identity_perm(rng);
                    slots.push(CheckupSlot { skill: Skill::Chain, seed, repeat_of: Some(at), perm });
                    placed += 1;
                    false
                }
                _ => true,
            }
        });
    }
    debug_assert!(pending.is_empty(), "every repeat was placed");
    CheckupPlan { kind: CheckupKind::Skill(Skill::Chain), form, slots }
}

/// The position of the `stage_index`th original stage among the slots so far.
fn position_of_original(slots: &[CheckupSlot], stage_index: usize) -> Option<usize> {
    let mut seen = 0;
    for (position, slot) in slots.iter().enumerate() {
        if slot.repeat_of.is_none() {
            if seen == stage_index {
                return Some(position);
            }
            seen += 1;
        }
    }
    None
}

/// Builds form `form` of `skill`. The same arguments always give the same plan.
pub fn checkup_plan(skill: Skill, form: u32) -> CheckupPlan {
    let mix = (skill as u64 + 1).wrapping_mul(SKILL_MIX) ^ (u64::from(form) + 1).wrapping_mul(FORM_MIX);
    let mut rng = Rng::new(mix ^ FORM_SALT);
    if skill == Skill::Chain {
        return chain_plan(form, &mut rng);
    }
    let mut seeds = original_seeds(skill, form, &mut rng);
    rng.shuffle(&mut seeds);

    let mut repeat_positions: Vec<usize> = (MIN_REPEAT_GAP..CHECKUP_LEN).collect();
    rng.shuffle(&mut repeat_positions);
    repeat_positions.truncate(CHECKUP_REPEATS);
    repeat_positions.sort_unstable();

    let mut next_seed = seeds.into_iter();
    let mut slots: Vec<CheckupSlot> = (0..CHECKUP_LEN)
        .map(|position| CheckupSlot {
            skill,
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
    CheckupPlan { kind: CheckupKind::Skill(skill), form, slots }
}

/// Builds the mixed checkup stored as form `form` ([`MIXED_FORM_BASE`] and up).
/// The same form always gives the same plan. Items come in a shuffled order,
/// a whole hand's four stages together and in order; there are no repeats.
pub fn mixed_checkup_plan(form: u32) -> CheckupPlan {
    let mut rng = Rng::new((u64::from(form) + 1).wrapping_mul(FORM_MIX) ^ MIXED_SALT);
    let deck_len = anchors::deck().len().max(1) as u64;
    let top_down = |i: usize, rungs: u8| rungs - 1 - (i % usize::from(rungs)) as u8;
    let mut units: Vec<Vec<CheckupSlot>> = Vec::new();
    for &(skill, count) in &MIXED_CHECKUP_MIX {
        let slot = |seed: u64| CheckupSlot { skill, seed, repeat_of: None, perm: IDENTITY };
        if skill == Skill::Chain {
            for _ in 0..count / ChainStage::ALL.len() {
                let hand = rng.next_u64() | HELD_OUT_BIT;
                units.push(ChainStage::ALL.iter().map(|&stage| slot(chain_seed_for_stage(hand, stage))).collect());
            }
            continue;
        }
        for i in 0..count {
            let seed = rng.next_u64() | HELD_OUT_BIT;
            let seed = match skill {
                Skill::Equity => equity_seed_with_rung(seed, (i % 2) as u8),
                // The upper rungs first: the lowest ones are practice steps.
                Skill::BeatsMe => beats_me_seed_with_rung(seed, top_down(i, BEATS_ME_RUNGS)),
                Skill::ReadPrice => read_price_seed_with_rung(seed, top_down(i, READ_PRICE_RUNGS)),
                Skill::BoardType => seed_with_category(seed, BoardCategory::ALL[rng.below(BoardCategory::ALL.len() as u64) as usize]),
                Skill::Anchor => anchors::seed_with_card(seed, rng.below(deck_len) as usize),
                _ => seed,
            };
            units.push(vec![slot(seed)]);
        }
    }
    rng.shuffle(&mut units);
    CheckupPlan { kind: CheckupKind::Mixed, form, slots: units.concat() }
}

/// The plan for either kind of checkup.
pub fn plan_for(kind: CheckupKind, form: u32) -> CheckupPlan {
    match kind {
        CheckupKind::Skill(skill) => checkup_plan(skill, form),
        CheckupKind::Mixed => mixed_checkup_plan(form),
    }
}

impl CheckupPlan {
    /// The item for one slot. Repeats relabel suits; `equity` uses the fixed band.
    pub fn item(&self, index: usize) -> Item {
        let slot = self.slots[index];
        let mut item = generate(slot.skill, slot.seed);
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
    /// Percent right or close enough to count, higher is better (the mixed checkup).
    PercentOk,
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

/// How a mixed checkup went on one practice group.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GroupScore {
    pub group: Group,
    pub answered: usize,
    pub ok: usize,
}

/// One checkup run's result. A one-skill checkup is scored on its 40 originals
/// only; a mixed one on all 40 items.
#[derive(Clone, PartialEq, Debug)]
pub struct CheckupScore {
    pub kind: CheckupKind,
    pub run: u32,
    pub form: u32,
    pub started_ms: u64,
    /// The time of its latest answer.
    pub finished_ms: u64,
    pub answered: usize,
    pub complete: bool,
    pub metric: CheckupMetric,
    /// The primary score, in the units of `metric`.
    pub score: f64,
    pub standard_error: f64,
    pub metrics: WindowMetrics,
    pub agreement: RepeatAgreement,
    /// Complete and meets the skill's pass mark on the originals. Always false
    /// for a mixed checkup, which has no mark.
    pub passed: bool,
    /// A mixed checkup's result group by group, in [`Group::ALL`] order.
    pub groups: Vec<GroupScore>,
    /// Started under an older generator: its items can't be regenerated, so it
    /// ends where it stopped rather than resuming on different questions.
    pub stale: bool,
    /// False for a `table` run on an earlier spot library. It still counts as a
    /// result, but its form names different spots now, so it can't be resumed.
    pub in_current_library: bool,
    /// The generator or spot library this run's rows came from, or `None` if they
    /// came from more than one (an old tab finishing the run later), which can't
    /// be compared with anything and doesn't count as the latest result.
    pub generator_key: Option<u32>,
}

/// The next form index for `skill`, one past every form started on the current
/// spot library, so no item is ever seen in two forms. A `table` form from an
/// earlier library drew different spots, so it doesn't use up any new ones.
pub fn next_checkup_form(skill: Skill, attempts: &[Attempt]) -> u32 {
    attempts
        .iter()
        .filter(|a| a.skill == skill && a.in_current_library())
        .filter_map(|a| a.extra.check.map(|c| c.form))
        .filter(|&form| !is_mixed_form(form))
        .max()
        .map_or(0, |form| form.saturating_add(1))
}

/// The next stored form for a mixed checkup, one past every mixed form started.
pub fn next_mixed_form(attempts: &[Attempt]) -> u32 {
    attempts
        .iter()
        .filter_map(|a| a.extra.check.map(|c| c.form))
        .filter(|&form| is_mixed_form(form))
        .max()
        .map_or(MIXED_FORM_BASE, |form| form.saturating_add(1))
}

/// The next form for either kind of checkup.
pub fn next_form(kind: CheckupKind, attempts: &[Attempt]) -> u32 {
    match kind {
        CheckupKind::Skill(skill) => next_checkup_form(skill, attempts),
        CheckupKind::Mixed => next_mixed_form(attempts),
    }
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
        CheckupMetric::PercentWithinOne | CheckupMetric::PercentOk => {
            (metrics.within_one_pct, proportion_se(metrics.within_one_pct, rows.len()))
        }
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

fn score_run(kind: CheckupKind, check: CheckRef, rows: &[&Attempt]) -> CheckupScore {
    let plan = plan_for(kind, check.form);
    let mut by_slot: Vec<Option<&Attempt>> = vec![None; plan.slots.len()];
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
    let mark = kind.skill().map(pass_mark);
    let metric = mark.as_ref().map_or(CheckupMetric::PercentOk, checkup_metric);
    let (score, standard_error) = primary_score(metric, &originals, &metrics);
    let complete = by_slot.iter().all(Option::is_some);
    let stale = match kind {
        CheckupKind::Skill(_) => rows.iter().any(|a| if a.skill == Skill::Table { !a.in_current_library() } else { generator_changed(a.skill, a.gen_version, GEN_VERSION) }),
        // Its unanswered items may be any skill's, so a change to any skill in it counts.
        CheckupKind::Mixed => rows.iter().any(|a| plan.slots.iter().any(|slot| generator_changed(slot.skill, a.gen_version, GEN_VERSION))),
    };
    let groups = match kind {
        CheckupKind::Skill(_) => Vec::new(),
        CheckupKind::Mixed => Group::ALL
            .into_iter()
            .map(|group| {
                let in_group: Vec<&&Attempt> = originals.iter().filter(|a| group_of(a.skill) == Some(group)).collect();
                GroupScore { group, answered: in_group.len(), ok: in_group.iter().filter(|a| a.ok()).count() }
            })
            .collect(),
    };
    CheckupScore {
        kind,
        run: check.run,
        form: check.form,
        started_ms: rows.iter().map(|a| a.timestamp_ms).min().unwrap_or(0),
        finished_ms: rows.iter().map(|a| a.timestamp_ms).max().unwrap_or(0),
        answered: by_slot.iter().flatten().count(),
        complete,
        metric,
        score,
        standard_error,
        metrics,
        agreement: agreement(&plan, &by_slot),
        passed: complete && mark.is_some_and(|mark| meets_mark(&mark, &metrics)),
        groups,
        stale,
        in_current_library: rows.iter().all(|a| a.in_current_library()),
        generator_key: match kind {
            CheckupKind::Skill(_) => single_generator(rows),
            // Rows of different skills: comparable when written by one build's generators.
            CheckupKind::Mixed => rows.first().map(|a| a.gen_version).filter(|v| rows.iter().all(|a| a.gen_version == *v)),
        },
    }
}

/// The one generator or library behind `rows`, if they all agree.
fn single_generator(rows: &[&Attempt]) -> Option<u32> {
    let first = rows.first()?.generator_key();
    rows.iter().all(|a| a.generator_key() == first).then_some(first).flatten()
}

/// Scores of every checkup run, oldest first.
pub fn checkup_runs(attempts: &[Attempt]) -> Vec<CheckupScore> {
    let mut starts: Vec<(u32, u32, CheckupKind)> = Vec::new();
    for attempt in attempts.iter().filter(|a| a.mode == Mode::Check) {
        if let (Some(check), Some(kind)) = (attempt.extra.check, CheckupKind::of_row(attempt)) {
            if !starts.iter().any(|&(run, _, _)| run == check.run) {
                starts.push((check.run, check.form, kind));
            }
        }
    }
    starts
        .into_iter()
        .map(|(run, form, kind)| {
            let rows: Vec<&Attempt> = attempts
                .iter()
                .filter(|a| a.mode == Mode::Check && CheckupKind::of_row(a) == Some(kind) && a.extra.check.is_some_and(|c| c.run == run))
                .collect();
            score_run(kind, CheckRef { run, form, slot: 0 }, &rows)
        })
        .collect()
}

/// The most recent completed checkup for a skill. A run whose rows span more than
/// one generator or spot library isn't a result of either, so it doesn't count.
pub fn latest_checkup(skill: Skill, attempts: &[Attempt]) -> Option<CheckupScore> {
    checkup_runs(attempts)
        .into_iter()
        .rfind(|s| s.kind == CheckupKind::Skill(skill) && s.complete && s.generator_key.is_some())
}

/// The player's latest run of `kind` this build can still deal, if it's unfinished.
/// A run from an older generator or an earlier spot library can't be picked up, and
/// it mustn't hide a newer run that can, as it would if an old tab wrote it later.
pub fn unfinished_run(runs: &[CheckupScore], kind: CheckupKind) -> Option<&CheckupScore> {
    runs.iter().rfind(|r| r.kind == kind && r.in_current_library && !r.stale).filter(|r| !r.complete)
}

/// What the home screen offers about the mixed checkup.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CheckupOffer {
    /// A new one is due.
    Take,
    /// One is under way and can be picked up.
    Resume { run: u32, answered: usize },
}

/// The mixed checkup on offer at `now_ms`, if any. It is optional and gates
/// nothing. An unfinished one this build can still deal is always offered to
/// resume. Otherwise a new one is offered [`MIXED_CHECKUP_EVERY_DAYS`] after the
/// last completed one ended, or, before the first, once the player has
/// [`FIRST_CHECKUP_MIN_DRILLS`] drill answers and the first was
/// [`FIRST_CHECKUP_MIN_DAYS`] or more ago. One-skill checkups don't count.
pub fn mixed_checkup_offer(attempts: &[Attempt], now_ms: u64) -> Option<CheckupOffer> {
    let runs = checkup_runs(attempts);
    if let Some(run) = unfinished_run(&runs, CheckupKind::Mixed) {
        return Some(CheckupOffer::Resume { run: run.run, answered: run.answered });
    }
    let last_done = runs.iter().filter(|r| r.kind == CheckupKind::Mixed && r.complete).map(|r| r.finished_ms).max();
    let due = match last_done {
        Some(done) => now_ms >= done.saturating_add(MIXED_CHECKUP_EVERY_DAYS * DAY_MS),
        None => {
            let drills: Vec<u64> = attempts.iter().filter(|a| a.mode == Mode::Drill).map(|a| a.timestamp_ms).collect();
            let first = drills.iter().copied().min();
            drills.len() >= FIRST_CHECKUP_MIN_DRILLS
                && first.is_some_and(|first| now_ms >= first.saturating_add(FIRST_CHECKUP_MIN_DAYS * DAY_MS))
        }
    };
    due.then_some(CheckupOffer::Take)
}
