//! Pure scheduling: which skill and item to deal next, and which items are due.

use std::collections::{HashMap, VecDeque};

use crate::item::{Skill, GEN_VERSION};
use crate::log::{Attempt, ItemRef, Mode};
use crate::rng::Rng;

pub const MISS_WINDOW: usize = 20;
pub const RETRY_MIN_GAP: u64 = 3;
pub const RETRY_MAX_GAP: u64 = 6;
/// At most one dealt item in this many may be a same-session retry.
pub const MAX_RETRY_ONE_IN: u64 = 3;
pub const FIRST_REVIEW_MS: u64 = 86_400_000;
pub const SECOND_REVIEW_MS: u64 = 3 * FIRST_REVIEW_MS;

const BASE_WEIGHT_PER_MILLE: u64 = 100;

/// `0.1 + miss rate`, expressed in thousandths so the choice is exact integers.
pub fn skill_weight(recent_ok: &[bool]) -> u64 {
    let window = &recent_ok[recent_ok.len().saturating_sub(MISS_WINDOW)..];
    if window.is_empty() {
        return BASE_WEIGHT_PER_MILLE;
    }
    let misses = window.iter().filter(|ok| !**ok).count() as u64;
    BASE_WEIGHT_PER_MILLE + misses * 1000 / window.len() as u64
}

/// Session length when neither a time nor an item limit is given.
pub const DEFAULT_SESSION_MINUTES: u64 = 10;

/// When a drill session ends.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SessionLimits {
    pub minutes: Option<u64>,
    pub items: Option<u64>,
}

impl SessionLimits {
    /// An item limit alone runs until the items are done. With no limit at all
    /// the session lasts [`DEFAULT_SESSION_MINUTES`].
    pub fn resolve(minutes: Option<u64>, items: Option<u64>) -> SessionLimits {
        let minutes = if minutes.is_none() && items.is_none() {
            Some(DEFAULT_SESSION_MINUTES)
        } else {
            minutes
        };
        SessionLimits { minutes, items }
    }

    pub fn finished(&self, answered: u64, elapsed_ms: u64) -> bool {
        self.items.is_some_and(|limit| answered >= limit)
            || self.minutes.is_some_and(|m| elapsed_ms >= m.saturating_mul(60_000))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct DueItem {
    pub skill: Skill,
    pub seed: u64,
}

#[derive(Clone, Copy)]
enum ReviewState {
    NeverMissed,
    Waiting { stage: u8, due_at: u64 },
    Retired,
}

/// Items due for review at `now_ms`. Baseline items and items from a different
/// [`GEN_VERSION`] are skipped. Attempts must be in time order.
pub fn due_items(attempts: &[Attempt], now_ms: u64) -> Vec<DueItem> {
    let mut states: HashMap<DueItem, ReviewState> = HashMap::new();
    let mut order: Vec<DueItem> = Vec::new();
    for attempt in attempts {
        let ItemRef::Seed(seed) = attempt.item else { continue };
        if attempt.gen_version != GEN_VERSION || attempt.mode == Mode::Baseline {
            continue;
        }
        let key = DueItem { skill: attempt.skill, seed };
        let state = states.entry(key).or_insert_with(|| {
            order.push(key);
            ReviewState::NeverMissed
        });
        *state = match (*state, attempt.ok()) {
            (ReviewState::Retired, _) => ReviewState::Retired,
            (_, false) => ReviewState::Waiting { stage: 1, due_at: attempt.timestamp_ms.saturating_add(FIRST_REVIEW_MS) },
            (ReviewState::Waiting { stage: 1, due_at }, true) if attempt.timestamp_ms >= due_at => {
                ReviewState::Waiting { stage: 2, due_at: attempt.timestamp_ms.saturating_add(SECOND_REVIEW_MS) }
            }
            (ReviewState::Waiting { stage: 2, due_at }, true) if attempt.timestamp_ms >= due_at => {
                ReviewState::Retired
            }
            (state, true) => state,
        };
    }
    order
        .into_iter()
        .filter(|key| matches!(states[key], ReviewState::Waiting { due_at, .. } if now_ms >= due_at))
        .collect()
}

/// Each skill's last [`MISS_WINDOW`] results. Only `drill` attempts count: the
/// baseline is measured separately and same-session retries follow a reveal.
pub fn recent_results(attempts: &[Attempt]) -> HashMap<Skill, VecDeque<bool>> {
    let mut recent: HashMap<Skill, VecDeque<bool>> = HashMap::new();
    for attempt in attempts.iter().filter(|a| a.mode == Mode::Drill) {
        let window = recent.entry(attempt.skill).or_default();
        window.push_back(attempt.ok());
        if window.len() > MISS_WINDOW {
            window.pop_front();
        }
    }
    recent
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Origin {
    New,
    Due,
    SessionRetry,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Planned {
    pub skill: Skill,
    pub seed: u64,
    pub origin: Origin,
}

struct Requeued {
    skill: Skill,
    seed: u64,
    remaining: u64,
}

/// One drill session's dealing state.
pub struct Session {
    rng: Rng,
    allowed: Vec<Skill>,
    recent: HashMap<Skill, VecDeque<bool>>,
    dealt: Vec<Skill>,
    requeue: Vec<Requeued>,
    retries_dealt: u64,
    due: Vec<DueItem>,
}

impl Session {
    pub fn new(allowed: Vec<Skill>, attempts: &[Attempt], now_ms: u64, seed: u64) -> Session {
        assert!(!allowed.is_empty(), "a session needs at least one skill");
        let recent = recent_results(attempts);
        let due = due_items(attempts, now_ms)
            .into_iter()
            .filter(|d| allowed.contains(&d.skill))
            .collect();
        Session { rng: Rng::new(seed), allowed, recent, dealt: Vec::new(), requeue: Vec::new(), retries_dealt: 0, due }
    }

    /// The skill that may not be dealt next, if the last two were the same.
    /// With a single allowed skill the rule cannot apply.
    fn blocked(&self) -> Option<Skill> {
        if self.allowed.len() < 2 {
            return None;
        }
        match self.dealt.as_slice() {
            [.., a, b] if a == b => Some(*a),
            _ => None,
        }
    }

    pub fn next_item(&mut self) -> Planned {
        let blocked = self.blocked();
        // Dealing a retry now must keep retries at or under 1 in MAX_RETRY_ONE_IN.
        let retry_allowed =
            (self.retries_dealt + 1) * MAX_RETRY_ONE_IN <= self.dealt.len() as u64 + 1;
        let ready = self
            .requeue
            .iter()
            .position(|r| retry_allowed && r.remaining == 0 && Some(r.skill) != blocked);
        if let Some(index) = ready {
            let retry = self.requeue.remove(index);
            return self.deal(retry.skill, retry.seed, Origin::SessionRetry);
        }
        let skill = self.choose_skill(blocked);
        if let Some(index) = self.due.iter().position(|d| d.skill == skill) {
            let item = self.due.remove(index);
            return self.deal(item.skill, item.seed, Origin::Due);
        }
        let seed = self.rng.next_u64();
        self.deal(skill, seed, Origin::New)
    }

    fn deal(&mut self, skill: Skill, seed: u64, origin: Origin) -> Planned {
        self.dealt.push(skill);
        if origin == Origin::SessionRetry {
            self.retries_dealt += 1;
        }
        Planned { skill, seed, origin }
    }

    fn choose_skill(&mut self, blocked: Option<Skill>) -> Skill {
        let choices: Vec<(Skill, u64)> = self
            .allowed
            .iter()
            .filter(|&&s| Some(s) != blocked)
            .map(|&s| {
                let recent: Vec<bool> = self
                    .recent
                    .get(&s)
                    .map_or_else(Vec::new, |w| w.iter().copied().collect());
                (s, skill_weight(&recent))
            })
            .collect();
        let total: u64 = choices.iter().map(|&(_, w)| w).sum();
        let mut roll = self.rng.below(total);
        for (skill, weight) in &choices {
            if roll < *weight {
                return *skill;
            }
            roll -= weight;
        }
        unreachable!("the roll is below the total weight")
    }

    /// Records the result of the item from the latest [`Session::next_item`].
    /// A miss returns once in this session; a retry that is missed again waits
    /// for the later-day review queue, which is built from the attempt log.
    pub fn record(&mut self, planned: Planned, ok: bool) {
        for retry in &mut self.requeue {
            retry.remaining = retry.remaining.saturating_sub(1);
        }
        if planned.origin == Origin::SessionRetry {
            return;
        }
        let window = self.recent.entry(planned.skill).or_default();
        window.push_back(ok);
        if window.len() > MISS_WINDOW {
            window.pop_front();
        }
        if !ok {
            let remaining = self.rng.range(RETRY_MIN_GAP, RETRY_MAX_GAP);
            self.requeue.push(Requeued { skill: planned.skill, seed: planned.seed, remaining });
        }
    }
}
