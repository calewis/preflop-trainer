//! Pure scheduling: which skill and item to deal next, and which items are due.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use crate::anchors;
use crate::board_type::{category_of_seed, seed_with_category, CategoryDealer, BOARD_TYPE_TARGET_MS};
use crate::beats_me::beats_me_rung;
use crate::equity_skill::{equity_band, equity_rung, next_band};
use crate::family::{family_of, find_sibling};
use crate::generate::generate;
use crate::item::{
    beats_me_seed_with_rung, chain_seed_for_stage, equity_rung_of_seed, generator_changed, CHAIN_STAGES, equity_seed_with_rung, is_held_out, read_price_seed_with_rung, ChainStage, Item, Question, Skill,
    GEN_VERSION, HELD_OUT_BIT,
};
use crate::log::{Attempt, ItemRef, Mode};
use crate::read_price::read_price_rung;
use crate::rng::Rng;
use crate::table;

pub const MISS_WINDOW: usize = 20;
pub const RETRY_MIN_GAP: u64 = 3;
pub const RETRY_MAX_GAP: u64 = 6;
/// At most one dealt item in this many may be a same-session retry.
pub const MAX_RETRY_ONE_IN: u64 = 3;
/// Days to the next review of a (skill, family) after each correct review. Starting guesses.
pub const FAMILY_LADDER_DAYS: [u64; 5] = [1, 3, 7, 14, 30];
pub const DAY_MS: u64 = 86_400_000;
/// Recent attempts in a family when judging how slow it is, and in the skill for its median.
pub const FAMILY_RESPONSE_WINDOW: usize = 10;
pub const SKILL_RESPONSE_WINDOW: usize = 40;

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

/// A (skill, family) whose review has come due.
#[derive(Clone, PartialEq, Debug)]
pub struct DueFamily {
    pub skill: Skill,
    pub family: String,
    pub due_at_ms: u64,
    /// This family's median recent response time over its skill's. Higher is slower.
    pub slowness: f64,
}

#[derive(Clone, Copy)]
enum LadderState {
    Waiting { stage: usize, due_at: u64 },
    Retired,
}

/// Leitner box intervals, in days, for boxes 1 to 5. Starting guesses.
pub const LEITNER_INTERVAL_DAYS: [u64; 5] = [1, 3, 7, 14, 30];
/// New anchor cards one session may introduce. Starting guess.
pub const MAX_NEW_ANCHORS_PER_SESSION: u32 = 5;

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

/// True for the skills reviewed by family. `anchor`, `board_type` and `table`
/// keep their own scheduling, and a missed `chain` is simply followed by new hands.
pub const fn reviewed_by_family(skill: Skill) -> bool {
    !matches!(skill, Skill::Anchor | Skill::BoardType | Skill::Table | Skill::Chain)
}

fn median_ms(times: &mut [u32]) -> u32 {
    times.sort_unstable();
    match times.len() {
        0 => 0,
        len if len % 2 == 1 => times[len / 2],
        len => (times[len / 2 - 1] + times[len / 2]) / 2,
    }
}

/// Families due for review at `now_ms`, slowest (relative to the skill's median)
/// first, and among equally slow ones the longest overdue first.
///
/// Replays the log: a miss starts or resets a family's ladder at 1 day, a correct
/// first-time answer at or after the due time moves it up, and a correct answer
/// at the 30-day step retires it. A later miss starts the ladder again, since a
/// family is a kind of problem, not one board. Baseline and checkup attempts,
/// held-out seeds, rows dealt before the skill's generator last changed and rows
/// with no family (from before families were logged) don't count. Attempts must be
/// in time order.
pub fn due_families(attempts: &[Attempt], now_ms: u64) -> Vec<DueFamily> {
    let mut states: HashMap<(Skill, String), LadderState> = HashMap::new();
    let mut order: Vec<(Skill, String)> = Vec::new();
    for attempt in attempts {
        let ItemRef::Seed(seed) = attempt.item else { continue };
        if generator_changed(attempt.skill, attempt.gen_version, GEN_VERSION)
            || !matches!(attempt.mode, Mode::Drill | Mode::DrillRetry)
            || is_held_out(seed)
            || !reviewed_by_family(attempt.skill)
        {
            continue;
        }
        let Some(family) = attempt.extra.family.clone() else { continue };
        let key = (attempt.skill, family);
        let at = attempt.timestamp_ms;
        if !attempt.ok() {
            if !states.contains_key(&key) {
                order.push(key.clone());
            }
            states.insert(key, LadderState::Waiting { stage: 0, due_at: at.saturating_add(DAY_MS) });
            continue;
        }
        if attempt.mode != Mode::Drill {
            continue;
        }
        if let Some(LadderState::Waiting { stage, due_at }) = states.get(&key).copied() {
            if at >= due_at {
                states.insert(key, match stage + 1 {
                    next if next < FAMILY_LADDER_DAYS.len() => {
                        LadderState::Waiting { stage: next, due_at: at.saturating_add(FAMILY_LADDER_DAYS[next] * DAY_MS) }
                    }
                    _ => LadderState::Retired,
                });
            }
        }
    }
    let mut due: Vec<DueFamily> = order
        .into_iter()
        .filter_map(|key| match states[&key] {
            LadderState::Waiting { due_at, .. } if now_ms >= due_at => {
                let slowness = slowness(attempts, key.0, &key.1);
                Some(DueFamily { skill: key.0, family: key.1, due_at_ms: due_at, slowness })
            }
            _ => None,
        })
        .collect();
    due.sort_by(|a, b| b.slowness.total_cmp(&a.slowness).then(a.due_at_ms.cmp(&b.due_at_ms)));
    due
}

/// A family's median recent response time divided by its skill's median.
fn slowness(attempts: &[Attempt], skill: Skill, family: &str) -> f64 {
    let rows = || attempts.iter().rev().filter(|a| a.skill == skill && matches!(a.mode, Mode::Drill | Mode::DrillRetry));
    let mut family_times: Vec<u32> = rows()
        .filter(|a| a.extra.family.as_deref() == Some(family))
        .take(FAMILY_RESPONSE_WINDOW)
        .map(|a| a.response_ms)
        .collect();
    let mut skill_times: Vec<u32> = rows().take(SKILL_RESPONSE_WINDOW).map(|a| a.response_ms).collect();
    let skill_median = median_ms(&mut skill_times);
    if skill_median == 0 {
        return 1.0;
    }
    f64::from(median_ms(&mut family_times)) / f64::from(skill_median)
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

#[derive(Clone)]
struct Requeued {
    skill: Skill,
    /// The missed item's seed, and its family when one should be found again.
    seed: u64,
    family: Option<String>,
    remaining: u64,
}

/// A finished sibling search, shared between a session and copies of it, so
/// looking ahead doesn't redo the search.
#[derive(Clone, Copy)]
struct SiblingSearch {
    result: Option<u64>,
}

type SiblingMemo = Rc<RefCell<HashMap<(Skill, String, u64), SiblingSearch>>>;

/// `beats` stays in drills at this share of a skill's normal weight, per mille.
pub const BEATS_WEIGHT_PER_MILLE: u64 = 500;

/// In the default drill, `preflop` is at least this share of the items dealt.
pub const MIN_PREFLOP_PERCENT: u64 = 25;

/// In the default drill, while `read_price` is under its share and `preflop` isn't
/// owed, it is dealt at this rate per mille. It takes no part in the weighted
/// choice, so its share doesn't depend on how many skills are in rotation.
pub const READ_PRICE_DEAL_PER_MILLE: u64 = 300;
/// Later-day reviews take at most one `read_price` slot in this many, so new
/// items at the player's rung keep coming in a session. Starting guess.
pub const READ_PRICE_REVIEW_SHARE_DIVISOR: u64 = 2;
/// No new `read_price` item once it has reached this share of the items dealt;
/// retries and reviews of it count toward the share. Starting guesses, chosen
/// to hold it at about 12-15% of the deal.
pub const READ_PRICE_MAX_PERCENT: u64 = 14;

/// One drill session's dealing state. Cloning is cheap: a copy can look ahead
/// without changing the real session.
#[derive(Clone)]
pub struct Session {
    rng: Rng,
    allowed: Vec<Skill>,
    recent: HashMap<Skill, VecDeque<bool>>,
    dealt: Vec<Skill>,
    requeue: Vec<Requeued>,
    retries_dealt: u64,
    due: Vec<DueFamily>,
    sibling_memo: SiblingMemo,
    /// `chain`: the hand being played and the stage to deal next, if any.
    chain: Option<(u64, ChainStage)>,
    /// True for the default drill: `preflop` keeps its minimum share.
    preflop_floor: bool,
    /// A `chain` was picked but wouldn't have left `preflop` its share, so it
    /// starts right after the next `preflop` item instead of being dropped.
    chain_waiting: bool,
    /// False on a look-ahead copy, so a guess at the next item can't start a slow search.
    searches_siblings: bool,
    /// Set on a look-ahead copy when a deal needed a sibling search it couldn't do:
    /// its guess at the next item is then not the item the real deal will find.
    skipped_search: bool,
    /// `beats_me`: the rung new items are dealt at.
    beats_me_rung: u8,
    /// `read_price`: the rung new items are dealt at, and the reviews dealt so far.
    read_price_rung: u8,
    read_price_reviews: u64,
    /// `equity`: the rung new items are dealt at, and each rung's current band.
    equity_rung: u8,
    equity_bands: [u8; 2],
    /// `anchor`: cards due now, cards never seen, and how many new ones may still come.
    anchors_due: VecDeque<usize>,
    anchors_new: VecDeque<usize>,
    anchors_new_left: u32,
    board_dealer: CategoryDealer,
    /// `table`: spots dealt or logged in the last [`table::NO_REPEAT_WINDOW`] attempts.
    table_recent: VecDeque<u16>,
}

/// The spots of the last [`table::NO_REPEAT_WINDOW`] `table` attempts on the current
/// library, oldest first.
fn table_recent_from(attempts: &[Attempt]) -> VecDeque<u16> {
    let mut recent: VecDeque<u16> = attempts
        .iter()
        .rev()
        .filter(|a| a.skill == Skill::Table && a.in_current_library())
        .filter_map(|a| a.extra.table_spot)
        .take(table::NO_REPEAT_WINDOW)
        .collect();
    recent.make_contiguous().reverse();
    recent
}

impl Session {
    pub fn new(allowed: Vec<Skill>, attempts: &[Attempt], now_ms: u64, seed: u64) -> Session {
        assert!(!allowed.is_empty(), "a session needs at least one skill");
        let recent = recent_results(attempts);
        let due = due_families(attempts, now_ms)
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
            sibling_memo: Rc::new(RefCell::new(HashMap::new())),
            chain: None,
            preflop_floor: false,
            chain_waiting: false,
            searches_siblings: true,
            skipped_search: false,
            beats_me_rung: beats_me_rung(attempts),
            read_price_rung: read_price_rung(attempts),
            read_price_reviews: 0,
            equity_rung: equity_rung(attempts),
            equity_bands: [equity_band(attempts, 0), equity_band(attempts, 1)],
            anchors_due: anchors_due(attempts, now_ms).into(),
            anchors_new: anchors_new(attempts).into(),
            anchors_new_left: MAX_NEW_ANCHORS_PER_SESSION,
            board_dealer: CategoryDealer::new(attempts),
            table_recent: table_recent_from(attempts),
        }
    }

    /// A look-ahead copy: it uses sibling searches already done and starts none.
    pub fn without_sibling_search(mut self) -> Session {
        self.searches_siblings = false;
        self
    }

    /// The default drill: `preflop` keeps at least [`MIN_PREFLOP_PERCENT`] of the deal.
    pub fn with_preflop_floor(mut self) -> Session {
        self.preflop_floor = true;
        self
    }

    /// True when dealing anything but `preflop` next would drop it below its share.
    fn preflop_owed(&self, blocked: Option<Skill>) -> bool {
        let dealt = self.dealt.iter().filter(|s| **s == Skill::Preflop).count() as u64;
        self.preflop_floor
            && self.allowed.contains(&Skill::Preflop)
            && blocked != Some(Skill::Preflop)
            && dealt * 100 < (self.dealt.len() as u64 + 1) * MIN_PREFLOP_PERCENT
    }

    /// False if starting a chain now would leave `preflop` under its share after
    /// the chain's last stage.
    fn chain_fits_preflop_floor(&self) -> bool {
        if !self.preflop_floor || !self.allowed.contains(&Skill::Preflop) {
            return true;
        }
        let preflop = self.dealt.iter().filter(|s| **s == Skill::Preflop).count() as u64;
        preflop * 100 >= (self.dealt.len() as u64 + CHAIN_STAGES) * MIN_PREFLOP_PERCENT
    }

    /// True when `read_price` is dealt by its own share in this session: the
    /// default drill, where the weighted choice would let its share follow how
    /// many other skills are in rotation.
    fn read_price_by_share(&self, skill: Skill) -> bool {
        skill == Skill::ReadPrice && self.preflop_floor && self.allowed.contains(&Skill::ReadPrice)
    }

    /// False when `read_price` has given reviews enough of its slots already, so
    /// the next slot goes to a new item at the player's rung.
    fn review_slot_open(&self, skill: Skill) -> bool {
        if skill != Skill::ReadPrice {
            return true;
        }
        let slots = self.dealt.iter().filter(|s| **s == Skill::ReadPrice).count() as u64 + 1;
        (self.read_price_reviews + 1) * READ_PRICE_REVIEW_SHARE_DIVISOR <= slots
    }

    /// True when this item should be `read_price`, by its roll and share limit.
    fn read_price_turn(&mut self, blocked: Option<Skill>) -> bool {
        if !self.read_price_by_share(Skill::ReadPrice) || blocked == Some(Skill::ReadPrice) {
            return false;
        }
        let dealt = self.dealt.iter().filter(|s| **s == Skill::ReadPrice).count() as u64;
        if dealt * 100 >= (self.dealt.len() as u64 + 1) * READ_PRICE_MAX_PERCENT {
            return false;
        }
        self.rng.below(1000) < READ_PRICE_DEAL_PER_MILLE
    }

    /// True while a picked `chain` is waiting for the next `preflop` item.
    pub fn chain_is_waiting(&self) -> bool {
        self.chain_waiting
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

    /// True while the current `chain` hand still has stages to deal.
    pub fn chain_in_progress(&self) -> bool {
        self.chain.is_some()
    }

    /// The next item, or `None` when no allowed skill can be dealt and no
    /// same-session retry is waiting. Only `anchor` can run out.
    pub fn try_next_item(&mut self) -> Option<Planned> {
        // A chain's stages come straight after each other, ahead of anything else.
        if let Some((hand, stage)) = self.chain {
            self.chain = ChainStage::from_index(stage.index() + 1).map(|next| (hand, next));
            return Some(self.deal(Skill::Chain, chain_seed_for_stage(hand, stage), Origin::New));
        }
        let blocked = self.blocked();
        // The chain that had to wait starts as soon as it fits, which takes a
        // `preflop` item: one is enough unless the floor is exactly met, then two.
        if self.chain_waiting && self.chain_fits_preflop_floor() {
            self.chain_waiting = false;
            return Some(self.deal_fresh(Skill::Chain));
        }
        let preflop_owed = self.preflop_owed(blocked) || (self.chain_waiting && blocked != Some(Skill::Preflop));
        // Dealing a retry now must keep retries at or under 1 in MAX_RETRY_ONE_IN.
        let retry_allowed =
            (self.retries_dealt + 1) * MAX_RETRY_ONE_IN <= self.dealt.len() as u64 + 1;
        let ready = self
            .requeue
            .iter()
            .position(|r| !preflop_owed && retry_allowed && r.remaining == 0 && Some(r.skill) != blocked);
        if let Some(index) = ready {
            let retry = self.requeue.remove(index);
            return Some(self.deal_retry(retry));
        }
        let eligible: Vec<Skill> = self.allowed.iter().copied().filter(|&s| self.can_deal(s)).collect();
        if eligible.is_empty() {
            // A retry's countdown only moves as other items are served, so with
            // nothing else to deal the waiting retries are served now.
            let index = (0..self.requeue.len()).min_by_key(|&i| self.requeue[i].remaining)?;
            let retry = self.requeue.remove(index);
            return Some(self.deal_retry(retry));
        }
        // The no-three-in-a-row rule can't apply with one dealable skill.
        let blocked = blocked.filter(|_| eligible.len() >= 2);
        let mut skill = if preflop_owed {
            Skill::Preflop
        } else if self.read_price_turn(blocked) {
            Skill::ReadPrice
        } else {
            self.choose_skill(&eligible, blocked)
        };
        // A chain's stages come in a row, so it only starts if the floor survives them
        // all. Otherwise it waits for the next `preflop` item instead of being lost.
        if skill == Skill::Chain && !self.chain_fits_preflop_floor() {
            self.chain_waiting = true;
            if blocked == Some(Skill::Preflop) {
                let others: Vec<Skill> = eligible.iter().copied().filter(|&s| s != Skill::Chain).collect();
                skill = self.choose_skill(&others, blocked);
            } else {
                skill = Skill::Preflop;
            }
        }
        if let Some(index) = self.due.iter().position(|d| d.skill == skill).filter(|_| self.review_slot_open(skill)) {
            let due = self.due.remove(index);
            let base = self.rng.next_u64();
            return Some(match self.sibling(skill, &due.family, base, HELD_OUT_BIT) {
                Some(seed) => self.deal(skill, seed, Origin::Due),
                // None found: this family waits for another session, and a plain item goes out now.
                None => self.deal_fresh(skill),
            });
        }
        Some(self.deal_fresh(skill))
    }

    /// True if the last deal on this look-ahead copy was a guess because it needed
    /// a sibling search, so the player would get a different item from the real deal.
    pub fn guessed_without_search(&self) -> bool {
        self.skipped_search
    }

    /// A previously searched sibling, or a new search. `avoid` is the missed
    /// item's own seed, or an impossible seed for a later-day review.
    fn sibling(&mut self, skill: Skill, family: &str, base: u64, avoid: u64) -> Option<u64> {
        let key = (skill, family.to_string(), base);
        let found = self.sibling_memo.borrow().get(&key).copied();
        if let Some(search) = found {
            return search.result;
        }
        if !self.searches_siblings {
            self.skipped_search = true;
            return None;
        }
        let result = find_sibling(skill, family, base, avoid);
        self.sibling_memo.borrow_mut().insert(key, SiblingSearch { result });
        result
    }

    /// A same-session retry: a fresh sibling, the same item when it has no
    /// family (`anchor`), or a plain new item if no sibling turned up.
    fn deal_retry(&mut self, retry: Requeued) -> Planned {
        let seed = match retry.family.as_deref() {
            Some(family) => self.sibling(retry.skill, family, retry.seed, retry.seed),
            None => Some(retry.seed),
        };
        match seed {
            Some(seed) => self.deal(retry.skill, seed, Origin::SessionRetry),
            None => self.deal_fresh(retry.skill),
        }
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
        if skill == Skill::Table {
            let (seed, id) = table::deal_spot(&mut self.rng, &self.table_recent);
            self.table_recent.push_back(id);
            if self.table_recent.len() > table::NO_REPEAT_WINDOW {
                self.table_recent.pop_front();
            }
            return self.deal(skill, seed, Origin::New);
        }
        let mut seed = self.rng.next_u64() & !HELD_OUT_BIT;
        if skill == Skill::Chain {
            self.chain = ChainStage::from_index(1).map(|next| (seed, next));
            seed = chain_seed_for_stage(seed, ChainStage::Hand);
        }
        match skill {
            Skill::Equity => seed = equity_seed_with_rung(seed, self.equity_rung),
            Skill::BeatsMe => seed = beats_me_seed_with_rung(seed, self.beats_me_rung),
            Skill::ReadPrice => seed = read_price_seed_with_rung(seed, self.read_price_rung),
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
        if origin == Origin::Due && skill == Skill::ReadPrice {
            self.read_price_reviews += 1;
        }
        Planned { skill, seed, origin }
    }

    fn choose_skill(&mut self, eligible: &[Skill], blocked: Option<Skill>) -> Skill {
        let choices: Vec<(Skill, u64)> = eligible
            .iter()
            .filter(|&&s| Some(s) != blocked && !self.read_price_by_share(s))
            .map(|&s| {
                let recent: Vec<bool> = self
                    .recent
                    .get(&s)
                    .map_or_else(Vec::new, |w| w.iter().copied().collect());
                let weight = skill_weight(&recent);
                (s, if s == Skill::Beats { weight * BEATS_WEIGHT_PER_MILLE / 1000 } else { weight })
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

    /// [`Session::record_with_family`] for callers that haven't kept the family;
    /// a miss regenerates the item to find it.
    pub fn record_timed(&mut self, planned: Planned, ok: bool, response_ms: u32) {
        let family = if !ok && retries_with_sibling(planned.skill) && planned.origin != Origin::SessionRetry {
            family_of(&generate(planned.skill, planned.seed))
        } else {
            None
        };
        self.record_with_family(planned, ok, response_ms, family);
    }

    /// [`Session::record`] with the response time, which `board_type` needs to
    /// tell a retiring answer from a merely correct one, and the item's family
    /// from the attempt log. A miss with no family is retried as the same item.
    pub fn record_with_family(&mut self, planned: Planned, ok: bool, response_ms: u32, family: Option<String>) {
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
        // Boards are not reused on purpose, so a missed one does not come back;
        // a table spot would repeat inside its no-repeat window, and a missed
        // chain stage is followed by the rest of its own hand.
        if !ok && !matches!(planned.skill, Skill::BoardType | Skill::Table | Skill::Chain) {
            let remaining = self.rng.range(RETRY_MIN_GAP, RETRY_MAX_GAP);
            self.requeue.push(Requeued { skill: planned.skill, seed: planned.seed, family, remaining });
        }
    }
}

/// True for the skills whose retries deal a fresh sibling rather than the same item.
fn retries_with_sibling(skill: Skill) -> bool {
    reviewed_by_family(skill)
}
