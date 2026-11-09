//! `board_type`: naming a flop's suit and rank pattern at a glance, and the
//! pure protocol that decides which categories to deal.
//!
//! Suit pattern crossed with rank pattern gives nine labels, but only eight can
//! occur: two cards sharing a rank have different suits, so a paired flop is
//! never monotone. Offering that label would create a category nobody can ever
//! be dealt or retire, so [`BoardCategory::ALL`] lists the eight real ones.

use std::collections::VecDeque;

use crate::cards::Card;
use crate::generate::board_texture;
use crate::item::Skill;
use crate::log::{Attempt, ItemRef, Mode};
use crate::rng::Rng;

/// Target response time. Starting guess.
pub const BOARD_TYPE_TARGET_MS: u32 = 6_000;
/// A response slower than this counts as a miss. Starting guess.
pub const BOARD_TYPE_MISS_MS: u32 = 12_000;
/// Consecutive correct answers inside the target that retire a category. Starting guess.
pub const RETIRE_STREAK: u32 = 3;
/// Trials of other categories required between two of the same. Starting guess.
pub const MIN_CATEGORY_GAP: usize = 3;
/// Draw weights. Unretired categories come first; retired ones keep turning up
/// for occasional review. Starting guesses.
pub const UNRETIRED_WEIGHT: u64 = 8;
pub const RETIRED_WEIGHT: u64 = 1;

/// Bits of a `board_type` seed that hold the category index.
const CATEGORY_SHIFT: u32 = 56;
const CATEGORY_MASK: u64 = 0xF;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SuitPattern {
    Rainbow,
    TwoTone,
    Monotone,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum RankPattern {
    Paired,
    Connected,
    Disconnected,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct BoardCategory {
    pub suit: SuitPattern,
    pub rank: RankPattern,
}

const fn category(suit: SuitPattern, rank: RankPattern) -> BoardCategory {
    BoardCategory { suit, rank }
}

impl BoardCategory {
    /// Every category that can actually occur on a flop.
    pub const ALL: [BoardCategory; 8] = [
        category(SuitPattern::Rainbow, RankPattern::Paired),
        category(SuitPattern::Rainbow, RankPattern::Connected),
        category(SuitPattern::Rainbow, RankPattern::Disconnected),
        category(SuitPattern::TwoTone, RankPattern::Paired),
        category(SuitPattern::TwoTone, RankPattern::Connected),
        category(SuitPattern::TwoTone, RankPattern::Disconnected),
        category(SuitPattern::Monotone, RankPattern::Connected),
        category(SuitPattern::Monotone, RankPattern::Disconnected),
    ];

    /// Position in [`BoardCategory::ALL`].
    pub fn index(self) -> usize {
        BoardCategory::ALL
            .iter()
            .position(|c| *c == self)
            .expect("a paired monotone flop cannot be built")
    }

    pub fn code(self) -> &'static str {
        match (self.suit, self.rank) {
            (SuitPattern::Rainbow, RankPattern::Paired) => "rp",
            (SuitPattern::Rainbow, RankPattern::Connected) => "rc",
            (SuitPattern::Rainbow, RankPattern::Disconnected) => "rd",
            (SuitPattern::TwoTone, RankPattern::Paired) => "tp",
            (SuitPattern::TwoTone, RankPattern::Connected) => "tc",
            (SuitPattern::TwoTone, RankPattern::Disconnected) => "td",
            (SuitPattern::Monotone, RankPattern::Paired) => "mp",
            (SuitPattern::Monotone, RankPattern::Connected) => "mc",
            (SuitPattern::Monotone, RankPattern::Disconnected) => "md",
        }
    }

    pub fn label(self) -> &'static str {
        match (self.suit, self.rank) {
            (SuitPattern::Rainbow, RankPattern::Paired) => "rainbow, paired",
            (SuitPattern::Rainbow, RankPattern::Connected) => "rainbow, connected",
            (SuitPattern::Rainbow, RankPattern::Disconnected) => "rainbow, disconnected",
            (SuitPattern::TwoTone, RankPattern::Paired) => "two-tone, paired",
            (SuitPattern::TwoTone, RankPattern::Connected) => "two-tone, connected",
            (SuitPattern::TwoTone, RankPattern::Disconnected) => "two-tone, disconnected",
            (SuitPattern::Monotone, RankPattern::Paired) => "monotone, paired",
            (SuitPattern::Monotone, RankPattern::Connected) => "monotone, connected",
            (SuitPattern::Monotone, RankPattern::Disconnected) => "monotone, disconnected",
        }
    }

    pub fn from_code(code: &str) -> Option<BoardCategory> {
        BoardCategory::ALL.into_iter().find(|c| c.code() == code)
    }
}

/// The category of a three-card flop. Paired takes precedence over connected.
pub fn classify_flop(board: &[Card]) -> BoardCategory {
    let texture = board_texture(board);
    let mut suits = [0u8; 4];
    for card in board {
        suits[card.suit() as usize] += 1;
    }
    let suit = match suits.into_iter().max().unwrap_or(0) {
        3 => SuitPattern::Monotone,
        2 => SuitPattern::TwoTone,
        _ => SuitPattern::Rainbow,
    };
    let rank = if texture.paired {
        RankPattern::Paired
    } else if texture.straight_possible {
        RankPattern::Connected
    } else {
        RankPattern::Disconnected
    };
    BoardCategory { suit, rank }
}

/// The category a `board_type` seed asks for.
pub fn category_of_seed(seed: u64) -> BoardCategory {
    BoardCategory::ALL[((seed >> CATEGORY_SHIFT) & CATEGORY_MASK) as usize % BoardCategory::ALL.len()]
}

/// `seed` with its category bits set to `category`.
pub fn seed_with_category(seed: u64, category: BoardCategory) -> u64 {
    (seed & !(CATEGORY_MASK << CATEGORY_SHIFT)) | ((category.index() as u64) << CATEGORY_SHIFT)
}

/// Every real category that differs from `right` in exactly one attribute.
pub fn one_attribute_distractors(right: BoardCategory) -> Vec<BoardCategory> {
    BoardCategory::ALL
        .into_iter()
        .filter(|c| (c.suit == right.suit) != (c.rank == right.rank))
        .collect()
}

/// The four choices for a trial, in a dealt order: `right` plus three distractors
/// that each differ from it in one attribute. Every real category has at least three.
pub fn answer_options(right: BoardCategory, rng: &mut Rng) -> [BoardCategory; 4] {
    let mut distractors = one_attribute_distractors(right);
    rng.shuffle(&mut distractors);
    let mut options = [right, distractors[0], distractors[1], distractors[2]];
    rng.shuffle(&mut options);
    options
}

/// How one category stands, for a GUI to show.
#[derive(Clone, PartialEq, Debug)]
pub struct BoardCategoryStatus {
    pub category: BoardCategory,
    pub attempts: usize,
    pub correct: usize,
    /// Consecutive correct answers inside the target, up to the latest attempt.
    pub streak: u32,
    /// True while the streak is at least [`RETIRE_STREAK`]. A later miss or slow
    /// answer on review brings the category back.
    pub retired: bool,
}

fn inside_target(attempt: &Attempt) -> bool {
    attempt.correct && attempt.response_ms <= BOARD_TYPE_TARGET_MS
}

/// Per-category status from `drill` attempts, in [`BoardCategory::ALL`] order.
pub fn board_type_status(attempts: &[Attempt]) -> Vec<BoardCategoryStatus> {
    let mut status: Vec<BoardCategoryStatus> = BoardCategory::ALL
        .into_iter()
        .map(|category| BoardCategoryStatus { category, attempts: 0, correct: 0, streak: 0, retired: false })
        .collect();
    for attempt in attempts.iter().filter(|a| a.skill == Skill::BoardType && a.mode == Mode::Drill) {
        let ItemRef::Seed(seed) = attempt.item else { continue };
        let entry = &mut status[category_of_seed(seed).index()];
        entry.attempts += 1;
        entry.correct += usize::from(attempt.correct);
        entry.streak = if inside_target(attempt) { entry.streak + 1 } else { 0 };
        entry.retired = entry.streak >= RETIRE_STREAK;
    }
    status
}

/// Which category to deal next, from the log so far plus this session's trials.
#[derive(Clone, Debug)]
pub struct CategoryDealer {
    retired: [bool; 8],
    streaks: [u32; 8],
    recent: VecDeque<usize>,
}

impl CategoryDealer {
    pub fn new(attempts: &[Attempt]) -> CategoryDealer {
        let mut dealer = CategoryDealer { retired: [false; 8], streaks: [0; 8], recent: VecDeque::new() };
        for status in board_type_status(attempts) {
            let index = status.category.index();
            dealer.retired[index] = status.retired;
            dealer.streaks[index] = status.streak;
        }
        for attempt in attempts.iter().filter(|a| a.skill == Skill::BoardType && a.mode == Mode::Drill) {
            if let ItemRef::Seed(seed) = attempt.item {
                dealer.note_dealt(category_of_seed(seed));
            }
        }
        dealer
    }

    fn note_dealt(&mut self, category: BoardCategory) {
        self.recent.push_back(category.index());
        if self.recent.len() > MIN_CATEGORY_GAP {
            self.recent.pop_front();
        }
    }

    /// Picks a category not dealt in the last [`MIN_CATEGORY_GAP`] trials, with
    /// unretired ones far more likely than retired ones, and records it.
    pub fn deal(&mut self, rng: &mut Rng) -> BoardCategory {
        let choices: Vec<(usize, u64)> = (0..BoardCategory::ALL.len())
            .filter(|index| !self.recent.contains(index))
            .map(|index| (index, if self.retired[index] { RETIRED_WEIGHT } else { UNRETIRED_WEIGHT }))
            .collect();
        let mut roll = rng.below(choices.iter().map(|&(_, w)| w).sum());
        for (index, weight) in choices {
            if roll < weight {
                let picked = BoardCategory::ALL[index];
                self.note_dealt(picked);
                return picked;
            }
            roll -= weight;
        }
        unreachable!("the roll is below the total weight")
    }

    /// Updates the category's streak after a trial.
    pub fn record(&mut self, category: BoardCategory, correct_inside_target: bool) {
        let index = category.index();
        self.streaks[index] = if correct_inside_target { self.streaks[index] + 1 } else { 0 };
        self.retired[index] = self.streaks[index] >= RETIRE_STREAK;
    }

    pub fn is_retired(&self, category: BoardCategory) -> bool {
        self.retired[category.index()]
    }
}
