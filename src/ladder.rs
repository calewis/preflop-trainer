//! How a player moves up a skill's ladder (`read_price`, `beats_me` and the two
//! rungs of `equity`): one points rule shared by all three.
//!
//! Each first-time drill answer at the player's rung adds or takes off points.
//! The count starts at [`PointsGate::start`] on every rung, never goes below 0,
//! and the player moves up the first time it reaches [`PointsGate::goal`].
//! Promotion is permanent: answers after that point don't matter, so a review
//! of an old item can't demote the player.
//!
//! Why points and not "any 20 straight attempts at 85%": that window is checked
//! after every attempt and promotion only goes up, so luck accumulates. A player
//! right 76% of the time passed it within 100 attempts about 96% of the time. A
//! miss here costs [`MISS_POINTS`] right answers, so a lucky run is undone by the
//! misses around it, while a player who is truly accurate climbs steadily (a
//! one-sided sequential test, CUSUM style). Simulated with 20,000 players each,
//! the chance of moving up within 60 and within 100 attempts at a rung, by true
//! accuracy (tests/read_price.rs reruns this and asserts the targets):
//!
//! | accuracy | within 60 | within 100 |
//! |----------|-----------|------------|
//! | 70%      | 0.4%      | 0.7%       |
//! | 76%      | 3.9%      | 8.0%       |
//! | 80%      | 13.2%     | 27.3%      |
//! | 85%      | 42.4%     | 72.7%      |
//! | 90%      | 82.7%     | 98.3%      |
//! | 95%      | 99.4%     | 100.0%     |
//!
//! All numbers are starting guesses, to be tuned with real data.

use crate::log::Attempt;

/// A points count that starts at `start`, never drops below 0 and passes the
/// first time it reaches `goal`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PointsGate {
    pub start: i32,
    pub goal: i32,
}

/// Where a player stands at one rung.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GateStatus {
    /// The count now, or when it first reached the goal.
    pub points: i32,
    pub passed: bool,
}

impl PointsGate {
    /// Replays one rung's answers, given as point changes in the order answered.
    pub fn status(self, changes: impl IntoIterator<Item = i32>) -> GateStatus {
        let mut points = self.start;
        if points >= self.goal {
            return GateStatus { points, passed: true };
        }
        for change in changes {
            points = points.saturating_add(change).max(0);
            if points >= self.goal {
                return GateStatus { points, passed: true };
            }
        }
        GateStatus { points, passed: false }
    }

    pub fn passed(self, changes: impl IntoIterator<Item = i32>) -> bool {
        self.status(changes).passed
    }
}

/// Points a right answer adds on a right-or-wrong ladder.
pub const RIGHT_POINTS: i32 = 1;
/// Points a miss takes off on a right-or-wrong ladder.
pub const MISS_POINTS: i32 = 4;
/// The gate for `read_price` and `beats_me`: from 10 to 32, so 22 right in a row
/// move a new player up.
pub const RIGHT_WRONG_GATE: PointsGate = PointsGate { start: 10, goal: 32 };

/// The point change for one right-or-wrong answer.
pub fn right_wrong_points(correct: bool) -> i32 {
    if correct {
        RIGHT_POINTS
    } else {
        -MISS_POINTS
    }
}

/// Attempts logged before this moment (2026-11-13 00:00 UTC) also promote by
/// the rule used until then, so nobody drops a rung they had already reached.
/// It must not be earlier than the release that brought in [`PointsGate`].
pub const OLD_RULE_CUTOFF_MS: u64 = 1_794_528_000_000;
/// The last [`crate::item::GEN_VERSION`] of the builds that promoted by the old
/// rule. A phone can keep running such a build from its cache well after the
/// cutoff, and the rungs it shows there must not be taken back either.
pub const LAST_OLD_RULE_GEN_VERSION: u32 = 3;
/// The old rule: any this many straight attempts at a rung...
pub const OLD_RULE_WINDOW: usize = 20;
/// ...that are at least this share right, in percent.
pub const OLD_RULE_PCT: f64 = 85.0;

/// True for an attempt that the old rule still applies to.
pub fn before_old_rule_cutoff(attempt: &Attempt) -> bool {
    attempt.gen_version <= LAST_OLD_RULE_GEN_VERSION || attempt.timestamp_ms < OLD_RULE_CUTOFF_MS
}

/// The old right-or-wrong rule: some [`OLD_RULE_WINDOW`] straight results at
/// least [`OLD_RULE_PCT`] right.
pub fn old_rule_passed(results: &[bool]) -> bool {
    results.windows(OLD_RULE_WINDOW).any(|window| {
        100.0 * window.iter().filter(|&&right| right).count() as f64 / window.len() as f64 >= OLD_RULE_PCT
    })
}

/// A right-or-wrong ladder's rung. `results(rung)` lists a rung's first-time
/// drill answers as (attempt, correct) in log order. The old rule, over attempts
/// before [`OLD_RULE_CUTOFF_MS`], keeps any rung it had already given.
pub fn right_wrong_rung<'a, I>(rungs: u8, results: impl Fn(u8) -> I) -> u8
where
    I: Iterator<Item = &'a Attempt>,
{
    let mut old = 0;
    while old + 1 < rungs {
        let before: Vec<bool> = results(old).filter(|a| before_old_rule_cutoff(a)).map(|a| a.correct).collect();
        if !old_rule_passed(&before) {
            break;
        }
        old += 1;
    }
    let mut rung = 0;
    while rung + 1 < rungs && (rung < old || RIGHT_WRONG_GATE.passed(results(rung).map(|a| right_wrong_points(a.correct)))) {
        rung += 1;
    }
    rung
}
