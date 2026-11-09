//! Skills and the fully specified questions they ask.

use crate::board_type::BoardCategory;
use crate::cards::{Card, Holding};
use crate::classes::OppClass;
use crate::preflop::{Seat, Situation};

/// Bump whenever a generator's output for a given seed changes.
pub const GEN_VERSION: u32 = 1;

/// Seeds with this bit set are held out for checkups: drills and reviews never
/// serve one. See [`is_held_out`].
pub const HELD_OUT_BIT: u64 = 1 << 63;

/// True for a seed reserved for checkups.
pub const fn is_held_out(seed: u64) -> bool {
    seed & HELD_OUT_BIT != 0
}

/// Equity items carry their rung in this seed bit, so a seed always regenerates
/// the same question even after the player has moved up the ladder.
pub const EQUITY_RUNG_BIT: u64 = 1 << 62;

/// The ladder rung (0 exact hands, 1 classes) an `equity` seed asks for.
pub const fn equity_rung_of_seed(seed: u64) -> u8 {
    if seed & EQUITY_RUNG_BIT != 0 {
        1
    } else {
        0
    }
}

/// `seed` with its rung bit set to `rung`.
pub const fn equity_seed_with_rung(seed: u64, rung: u8) -> u64 {
    if rung == 0 {
        seed & !EQUITY_RUNG_BIT
    } else {
        seed | EQUITY_RUNG_BIT
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Skill {
    Preflop,
    Nuts,
    Possible,
    Made,
    Beats,
    Outs,
    Price,
    Hit,
    Spot,
    SpotMw,
    // New variants go last: a skill's position seeds its generator.
    BoardType,
    Equity,
    Anchor,
}

impl Skill {
    pub const ALL: [Skill; 13] = [
        Skill::Preflop,
        Skill::Nuts,
        Skill::Possible,
        Skill::Made,
        Skill::Beats,
        Skill::Outs,
        Skill::Price,
        Skill::Hit,
        Skill::Spot,
        Skill::SpotMw,
        Skill::BoardType,
        Skill::Equity,
        Skill::Anchor,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Skill::Preflop => "preflop",
            Skill::Nuts => "nuts",
            Skill::Possible => "possible",
            Skill::Made => "made",
            Skill::Beats => "beats",
            Skill::Outs => "outs",
            Skill::Price => "price",
            Skill::Hit => "hit",
            Skill::Spot => "spot",
            Skill::SpotMw => "spot_mw",
            Skill::BoardType => "board_type",
            Skill::Equity => "equity",
            Skill::Anchor => "anchor",
        }
    }

    pub fn from_name(name: &str) -> Option<Skill> {
        Skill::ALL.into_iter().find(|s| s.name() == name)
    }

    /// The training step a skill belongs to. `preflop` is outside the steps.
    pub fn step(self) -> Option<u8> {
        match self {
            Skill::Preflop => None,
            Skill::Nuts | Skill::Possible | Skill::Made | Skill::Beats | Skill::BoardType => Some(1),
            Skill::Outs | Skill::Price | Skill::Hit | Skill::Equity | Skill::Anchor => Some(2),
            Skill::Spot | Skill::SpotMw => Some(3),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum HitStreet {
    Flop,
    Turn,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SpotMode {
    /// No more betting: all remaining cards run out and X is 0.
    AllIn,
    /// One more card; if hero then has the best hand, about `x` more is collected.
    NextCard { x: u32 },
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SpotQuestion {
    pub hero: Holding,
    pub board: Vec<Card>,
    /// P: the pot before hero acts, including the bet and any earlier callers.
    pub pot: u32,
    /// C: the amount to call.
    pub call: u32,
    pub mode: SpotMode,
    /// One assumed class per opponent.
    pub opponents: Vec<OppClass>,
}

/// How the opponent is labelled in a next-card spot, which sets X.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum OpponentLabel {
    Tight,
    Average,
    CallingStation,
}

impl SpotQuestion {
    /// X = 0 is a tight player; up to the size of the pot after calling is
    /// average; anything larger is a calling station.
    pub fn opponent_label(&self) -> OpponentLabel {
        match self.mode {
            SpotMode::NextCard { x } if x > self.pot + self.call => OpponentLabel::CallingStation,
            SpotMode::NextCard { x } if x > 0 => OpponentLabel::Average,
            _ => OpponentLabel::Tight,
        }
    }

    pub fn x(&self) -> u32 {
        match self.mode {
            SpotMode::AllIn => 0,
            SpotMode::NextCard { x } => x,
        }
    }
}

/// Who hero is up against in an `equity` question.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum EquityOpponents {
    /// Rung 0: each opponent's exact hand is shown.
    Exact(Vec<Holding>),
    /// Rung 1: each opponent is assumed to hold a class, averaged over its combos.
    Classes(Vec<OppClass>),
}

impl EquityOpponents {
    pub fn count(&self) -> usize {
        match self {
            EquityOpponents::Exact(list) => list.len(),
            EquityOpponents::Classes(list) => list.len(),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EquityQuestion {
    pub hero: Holding,
    pub board: Vec<Card>,
    pub opponents: EquityOpponents,
    /// Points either side of the true equity that count as a hit. Fixed when
    /// the item is served (see [`crate::equity_skill`]), and logged with it.
    pub band: u8,
}

impl EquityQuestion {
    pub fn rung(&self) -> u8 {
        match self.opponents {
            EquityOpponents::Exact(_) => 0,
            EquityOpponents::Classes(_) => 1,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Question {
    Preflop {
        players: u8,
        /// Players still to act behind hero; this sets the seat.
        behind: u8,
        situation: Situation,
        hand: Holding,
    },
    Nuts {
        board: Vec<Card>,
    },
    Possible {
        board: Vec<Card>,
    },
    Made {
        hero: Holding,
        board: Vec<Card>,
    },
    Beats {
        hero: Holding,
        board: Vec<Card>,
    },
    Outs {
        hero: Holding,
        board: Vec<Card>,
        villain: Holding,
    },
    Price {
        /// Pot before the bet.
        pot_before: u32,
        bet: u32,
        callers: u32,
    },
    Hit {
        outs: u32,
        street: HitStreet,
        /// True for both remaining cards coming (flop only, all the money in).
        both: bool,
    },
    Spot(SpotQuestion),
    Equity(EquityQuestion),
    /// One card of the anchor deck, by its index.
    Anchor {
        card: u16,
    },
    /// Name a flop's suit and rank pattern from four choices.
    BoardType {
        board: Vec<Card>,
        options: [BoardCategory; 4],
    },
}

impl Question {
    pub fn seat(&self) -> Option<Seat> {
        match self {
            Question::Preflop { behind, .. } => Some(Seat::from_behind(*behind)),
            _ => None,
        }
    }

    /// The same question with every suit relabelled through `perm`.
    pub fn with_suit_perm(&self, perm: &[u8; 4]) -> Question {
        let card = |c: &Card| c.with_suit_perm(perm);
        let holding = |h: &Holding| [card(&h[0]), card(&h[1])];
        let board = |b: &[Card]| b.iter().map(card).collect::<Vec<_>>();
        match self {
            Question::Preflop { players, behind, situation, hand } => Question::Preflop {
                players: *players,
                behind: *behind,
                situation: *situation,
                hand: holding(hand),
            },
            Question::Nuts { board: b } => Question::Nuts { board: board(b) },
            Question::Possible { board: b } => Question::Possible { board: board(b) },
            Question::Made { hero, board: b } => Question::Made { hero: holding(hero), board: board(b) },
            Question::Beats { hero, board: b } => Question::Beats { hero: holding(hero), board: board(b) },
            Question::Outs { hero, board: b, villain } => Question::Outs {
                hero: holding(hero),
                board: board(b),
                villain: holding(villain),
            },
            Question::Price { .. } | Question::Hit { .. } | Question::Anchor { .. } => self.clone(),
            Question::Equity(q) => Question::Equity(EquityQuestion {
                hero: holding(&q.hero),
                board: board(&q.board),
                opponents: match &q.opponents {
                    EquityOpponents::Exact(list) => {
                        EquityOpponents::Exact(list.iter().map(holding).collect())
                    }
                    EquityOpponents::Classes(list) => EquityOpponents::Classes(list.clone()),
                },
                band: q.band,
            }),
            Question::BoardType { board: b, options } => Question::BoardType {
                board: board(b),
                options: *options,
            },
            Question::Spot(q) => Question::Spot(SpotQuestion {
                hero: holding(&q.hero),
                board: board(&q.board),
                ..q.clone()
            }),
        }
    }
}

/// A generated item, identified by (skill, seed, [`GEN_VERSION`]).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Item {
    pub skill: Skill,
    pub seed: u64,
    pub question: Question,
}
