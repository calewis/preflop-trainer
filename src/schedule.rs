//! Pure scheduling: which skill and item to deal next, and which items are due.

use std::collections::{HashMap, VecDeque};

use crate::anchors;
use crate::board_type::{category_of_seed, seed_with_category, CategoryDealer, BOARD_TYPE_TARGET_MS};
use crate::equity_skill::{equity_band, equity_rung, next_band};
use crate::generate::generate;
use crate::item::{
    equity_rung_of_seed, equity_seed_with_rung, is_held_out, Item, Question, Skill, GEN_VERSION, HELD_OUT_BIT,
};
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

/// Leitner box intervals, in days, for boxes 1 to 5. Starting guesses.
pub const LEITNER_INTERVAL_DAYS: [u64; 5] = [1, 3, 7, 14, 30];
/// New anchor cards one session may introduce. Starting guess.
pub const MAX_NEW_ANCHORS_PER_SESSION: u32 = 5;

const DAY_MS: u64 = 86_400_000;

/// Where one anchor card stands, for a GUI to show.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AnchorStatus {
    pub card: usize,
    /// 1 to 5; a card not yet seen sits in box 1.
    pub leitner_box: u8,
    /// `None` until the card has been answered in a drill.
    pub due_at_ms: Option<u64>,
}

/// Replays `drill` attempts through the boxes. A correct answer moves a card up
/// one box (at most box 5) and a miss returns it to box 1. Attempts must be in
/// time order.
pub fn anchor_status(attempts: &[Attempt]) -> Vec<AnchorStatus> {
    let mut status: Vec<AnchorStatus> = (0..anchors::deck().len())
        .map(|card| AnchorStatus { card, leitner_box: 1, due_at_ms: None })
        .collect();
    for attempt in attempts.iter().filter(|a| a.skill == Skill::Anchor && a.mode == Mode::Drill) {
        let ItemRef::Seed(seed) = attempt.item else { continue };
        let entry = &mut status[anchors::card_of_seed(seed)];
        entry.leitner_box = if attempt.ok() { (entry.leitner_box + 1).min(5) } else { 1 };
        let days = LEITNER_INTERVAL_DAYS[usize::from(entry.leitner_box) - 1];
        entry.due_at_ms = Some(attempt.timestamp_ms.saturating_add(days * DAY_MS));
    }
    status
}

/// Cards whose review has come due at `now_ms`, longest overdue first.
pub fn anchors_due(attempts: &[Attempt], now_ms: u64) -> Vec<usize> {
    let mut due: Vec<(u64, usize)> = anchor_status(attempts)
        .iter()
        .filter_map(|s| s.due_at_ms.filter(|&at| now_ms >= at).map(|at| (at, s.card)))
        .collect();
    due.sort_unstable();
    due.into_iter().map(|(_, card)| card).collect()
}

/// Cards never answered in a drill, in deck order.
pub fn anchors_new(attempts: &[Attempt]) -> Vec<usize> {
    anchor_status(attempts).iter().filter(|s| s.due_at_ms.is_none()).map(|s| s.card).collect()
}

/// Items due for review at `now_ms`. Baseline and checkup items, held-out seeds
/// and items from a different [`GEN_VERSION`] are skipped, as are `anchor` (it
/// has its own boxes) and `board_type` (boards are not reused on purpose).
/// Attempts must be in time order.
pub fn due_items(attempts: &[Attempt], now_ms: u64) -> Vec<DueItem> {
    let mut states: HashMap<DueItem, ReviewState> = HashMap::new();
    let mut order: Vec<DueItem> = Vec::new();
    for attempt in attempts {
        let ItemRef::Seed(seed) = attempt.item else { continue };
        if attempt.gen_version != GEN_VERSION
            || matches!(attempt.mode, Mode::Baseline | Mode::Check)
            || is_held_out(seed)
            || matches!(attempt.skill, Skill::Anchor | Skill::BoardType)
        {
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
    /// `equity`: the rung new items are dealt at, and each rung's current band.
    equity_rung: u8,
    equity_bands: [u8; 2],
    /// `anchor`: cards due now, cards never seen, and how many new ones may still come.
    anchors_due: VecDeque<usize>,
    anchors_new: VecDeque<usize>,
    anchors_new_left: u32,
    board_dealer: CategoryDealer,
}

impl Session {
    pub fn new(allowed: Vec<Skill>, attempts: &[Attempt], now_ms: u64, seed: u64) -> Session {
        assert!(!allowed.is_empty(), "a session needs at least one skill");
        let recent = recent_results(attempts);
        let due = due_items(attempts, now_ms)
            .into_iter()
            .filter(|d| allowed.contains(&d.skill))
            .collect();
        Session {
            rng: Rng::new(seed),
            allowed,
            recent,
            dealt: Vec::new(),
            requeue: Vec::new(),
            retries_dealt: 0,
            due,
            equity_rung: equity_rung(attempts),
            equity_bands: [equity_band(attempts, 0), equity_band(attempts, 1)],
            anchors_due: anchors_due(attempts, now_ms).into(),
            anchors_new: anchors_new(attempts).into(),
            anchors_new_left: MAX_NEW_ANCHORS_PER_SESSION,
            board_dealer: CategoryDealer::new(attempts),
        }
    }

    /// The fully specified item for a plan from this session: an `equity` item
    /// is served with its rung's current tolerance band.
    pub fn item_for(&self, planned: Planned) -> Item {
        let mut item = generate(planned.skill, planned.seed);
        if let Question::Equity(q) = &mut item.question {
            q.band = self.equity_bands[usize::from(equity_rung_of_seed(planned.seed))];
        }
        item
    }

    /// False for `anchor` when no card is due and no new card may be introduced.
    fn can_deal(&self, skill: Skill) -> bool {
        skill != Skill::Anchor
            || !self.anchors_due.is_empty()
            || (self.anchors_new_left > 0 && !self.anchors_new.is_empty())
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

    /// The next item, or `None` when no allowed skill can be dealt and no
    /// same-session retry is waiting. Only `anchor` can run out.
    pub fn try_next_item(&mut self) -> Option<Planned> {
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
            return Some(self.deal(retry.skill, retry.seed, Origin::SessionRetry));
        }
        let eligible: Vec<Skill> = self.allowed.iter().copied().filter(|&s| self.can_deal(s)).collect();
        if eligible.is_empty() {
            // A retry's countdown only moves as other items are served, so with
            // nothing else to deal the waiting retries are served now.
            let index = (0..self.requeue.len()).min_by_key(|&i| self.requeue[i].remaining)?;
            let retry = self.requeue.remove(index);
            return Some(self.deal(retry.skill, retry.seed, Origin::SessionRetry));
        }
        // The no-three-in-a-row rule can't apply with one dealable skill.
        let blocked = blocked.filter(|_| eligible.len() >= 2);
        let skill = self.choose_skill(&eligible, blocked);
        if let Some(index) = self.due.iter().position(|d| d.skill == skill) {
            let item = self.due.remove(index);
            return Some(self.deal(item.skill, item.seed, Origin::Due));
        }
        Some(self.deal_fresh(skill))
    }

    /// A new item for `skill`, or an `anchor` card from its boxes. Seeds keep
    /// their held-out bit clear: checkup items are never dealt as practice.
    fn deal_fresh(&mut self, skill: Skill) -> Planned {
        if skill == Skill::Anchor {
            if let Some(card) = self.anchors_due.pop_front() {
                return self.deal(skill, anchors::seed_with_card(0, card), Origin::Due);
            }
            let card = self.anchors_new.pop_front().expect("can_deal checked");
            self.anchors_new_left -= 1;
            return self.deal(skill, anchors::seed_with_card(0, card), Origin::New);
        }
        let mut seed = self.rng.next_u64() & !HELD_OUT_BIT;
        match skill {
            Skill::Equity => seed = equity_seed_with_rung(seed, self.equity_rung),
            Skill::BoardType => seed = seed_with_category(seed, self.board_dealer.deal(&mut self.rng)),
            _ => {}
        }
        self.deal(skill, seed, Origin::New)
    }

    fn deal(&mut self, skill: Skill, seed: u64, origin: Origin) -> Planned {
        self.dealt.push(skill);
        if origin == Origin::SessionRetry {
            self.retries_dealt += 1;
        }
        Planned { skill, seed, origin }
    }

    fn choose_skill(&mut self, eligible: &[Skill], blocked: Option<Skill>) -> Skill {
        let choices: Vec<(Skill, u64)> = eligible
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

    /// Records the result of the item from the latest [`Session::try_next_item`].
    /// A miss returns once in this session; a retry that is missed again waits
    /// for the later-day review queue, which is built from the attempt log.
    pub fn record(&mut self, planned: Planned, ok: bool) {
        self.record_timed(planned, ok, u32::MAX);
    }

    /// [`Session::record`] with the response time, which `board_type` needs to
    /// tell a retiring answer from a merely correct one.
    pub fn record_timed(&mut self, planned: Planned, ok: bool, response_ms: u32) {
        for retry in &mut self.requeue {
            retry.remaining = retry.remaining.saturating_sub(1);
        }
        if planned.origin == Origin::SessionRetry {
            return;
        }
        match planned.skill {
            Skill::Equity => {
                let rung = usize::from(equity_rung_of_seed(planned.seed));
                self.equity_bands[rung] = next_band(self.equity_bands[rung], ok);
            }
            Skill::BoardType => {
                let category = category_of_seed(planned.seed);
                self.board_dealer.record(category, ok && response_ms <= BOARD_TYPE_TARGET_MS);
            }
            _ => {}
        }
        let window = self.recent.entry(planned.skill).or_default();
        window.push_back(ok);
        if window.len() > MISS_WINDOW {
            window.pop_front();
        }
        // Boards are not reused on purpose, so a missed one does not come back.
        if !ok && planned.skill != Skill::BoardType {
            let remaining = self.rng.range(RETRY_MIN_GAP, RETRY_MAX_GAP);
            self.requeue.push(Requeued { skill: planned.skill, seed: planned.seed, remaining });
        }
    }
}
