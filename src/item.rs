//! Skills and the fully specified questions they ask.

use crate::board_type::BoardCategory;
use crate::cards::{Card, Holding};
use crate::classes::{OppClass, OppMix, PlayerType};
use crate::preflop::{Seat, Situation};
use crate::read_price::DrawKind;
use crate::table::TableQuestion;

/// Bump whenever a generator's output for a given seed changes, and add the
/// bump to [`GENERATOR_CHANGES`].
pub const GEN_VERSION: u32 = 3;

/// Which skills' generators changed at each [`GEN_VERSION`], so a bump for one skill
/// doesn't void another skill's unfinished checkups or queued reviews. `chain` and
/// `beats_me` first appeared in version 2 and `read_price` in version 3, so they have no
/// earlier rows to compare.
pub const GENERATOR_CHANGES: [(u32, &[Skill]); 2] = [
    (2, &[Skill::Spot, Skill::SpotMw, Skill::Equity]),
    (3, &[Skill::Preflop]),
];

/// Which of `skill`'s generators wrote a row marked `version`: the number of times
/// it has changed up to that version. `None` for a version outside this app's history.
/// Not meaningful for `table`, whose rows name a spot library instead.
pub fn generator_epoch(skill: Skill, version: u32) -> Option<u32> {
    if version == 0 || version > GEN_VERSION {
        return None;
    }
    Some(GENERATOR_CHANGES.iter().filter(|(at, skills)| *at <= version && skills.contains(&skill)).count() as u32)
}

/// True if `skill`'s items differ between versions `from` and `to` of the
/// generators. An unknown version counts as a change, since nothing is known about it.
pub fn generator_changed(skill: Skill, from: u32, to: u32) -> bool {
    from != to && (generator_epoch(skill, from).is_none() || generator_epoch(skill, from) != generator_epoch(skill, to))
}

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

/// How many rungs the [`Skill::BeatsMe`] ladder has.
pub const BEATS_ME_RUNGS: u8 = 4;
const BEATS_ME_RUNG_SHIFT: u32 = 60;
const BEATS_ME_RUNG_MASK: u64 = 0b11 << BEATS_ME_RUNG_SHIFT;

/// The rung a `beats_me` seed asks for, carried in its bits like `equity`.
pub const fn beats_me_rung_of_seed(seed: u64) -> u8 {
    ((seed & BEATS_ME_RUNG_MASK) >> BEATS_ME_RUNG_SHIFT) as u8
}

/// `seed` with its `beats_me` rung set to `rung`.
pub const fn beats_me_seed_with_rung(seed: u64, rung: u8) -> u64 {
    (seed & !BEATS_ME_RUNG_MASK) | ((rung as u64) << BEATS_ME_RUNG_SHIFT)
}

/// How many rungs the [`Skill::ReadPrice`] ladder has.
pub const READ_PRICE_RUNGS: u8 = 5;
const READ_PRICE_RUNG_SHIFT: u32 = 60;
const READ_PRICE_RUNG_MASK: u64 = 0b111 << READ_PRICE_RUNG_SHIFT;

/// The rung a `read_price` seed asks for, carried in its bits like `beats_me`.
/// Three bits hold eight values and there are five rungs, so the extra values wrap.
pub const fn read_price_rung_of_seed(seed: u64) -> u8 {
    ((seed & READ_PRICE_RUNG_MASK) >> READ_PRICE_RUNG_SHIFT) as u8 % READ_PRICE_RUNGS
}

/// `seed` with its `read_price` rung set to `rung`.
pub const fn read_price_seed_with_rung(seed: u64, rung: u8) -> u64 {
    (seed & !READ_PRICE_RUNG_MASK) | ((rung as u64) << READ_PRICE_RUNG_SHIFT)
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
    Table,
    Chain,
    BeatsMe,
    ReadPrice,
}

impl Skill {
    pub const ALL: [Skill; 17] = [
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
        Skill::Table,
        Skill::Chain,
        Skill::BeatsMe,
        Skill::ReadPrice,
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
            Skill::Table => "table",
            Skill::Chain => "chain",
            Skill::BeatsMe => "beats_me",
            Skill::ReadPrice => "read_price",
        }
    }

    pub fn from_name(name: &str) -> Option<Skill> {
        Skill::ALL.into_iter().find(|s| s.name() == name)
    }

    /// The training step a skill belongs to. `preflop` is outside the steps.
    pub fn step(self) -> Option<u8> {
        match self {
            Skill::Preflop | Skill::ReadPrice => None,
            Skill::Nuts | Skill::Possible | Skill::Made | Skill::Beats | Skill::BoardType | Skill::BeatsMe => Some(1),
            Skill::Outs | Skill::Price | Skill::Hit | Skill::Equity | Skill::Anchor => Some(2),
            Skill::Spot | Skill::SpotMw | Skill::Chain => Some(3),
            Skill::Table => Some(4),
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
    /// One assumed mix of hands per opponent.
    pub opponents: Vec<OppMix>,
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
        if let Some(player) = self.opponents.first().and_then(|m| m.player) {
            return match player {
                PlayerType::Tight => OpponentLabel::Tight,
                PlayerType::Average => OpponentLabel::Average,
                PlayerType::Loose => OpponentLabel::CallingStation,
            };
        }
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

/// The stages of one whole-hand [`Skill::Chain`] item, in the order they are asked.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ChainStage {
    Hand,
    Outs,
    Price,
    Decide,
}

impl ChainStage {
    pub const ALL: [ChainStage; 4] = [ChainStage::Hand, ChainStage::Outs, ChainStage::Price, ChainStage::Decide];

    pub const fn index(self) -> usize {
        match self {
            ChainStage::Hand => 0,
            ChainStage::Outs => 1,
            ChainStage::Price => 2,
            ChainStage::Decide => 3,
        }
    }

    pub fn from_index(index: usize) -> Option<ChainStage> {
        ChainStage::ALL.get(index).copied()
    }

    /// The code logged in the attempt log's stage column.
    pub fn code(self) -> &'static str {
        match self {
            ChainStage::Hand => "hand",
            ChainStage::Outs => "outs",
            ChainStage::Price => "price",
            ChainStage::Decide => "decide",
        }
    }

    pub fn from_code(code: &str) -> Option<ChainStage> {
        ChainStage::ALL.into_iter().find(|s| s.code() == code)
    }
}

/// How many stages one chain has.
pub const CHAIN_STAGES: u64 = 4;

/// The low bits of a [`Skill::Chain`] seed name the stage, so one hand is four
/// seeds that differ only there.
pub const CHAIN_STAGE_MASK: u64 = CHAIN_STAGES - 1;

/// The stage a chain seed asks for.
pub const fn chain_stage_of_seed(seed: u64) -> ChainStage {
    match seed & CHAIN_STAGE_MASK {
        0 => ChainStage::Hand,
        1 => ChainStage::Outs,
        2 => ChainStage::Price,
        _ => ChainStage::Decide,
    }
}

/// The seed shared by all four stages of a chain.
pub const fn chain_hand_seed(seed: u64) -> u64 {
    seed & !CHAIN_STAGE_MASK
}

/// The seed of `stage` of the chain `hand_seed` belongs to.
pub const fn chain_seed_for_stage(hand_seed: u64, stage: ChainStage) -> u64 {
    chain_hand_seed(hand_seed) | stage.index() as u64
}

/// One hand followed through four stages. Facts common to every stage live here;
/// each stage only chooses what to ask about them.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ChainHand {
    pub hero: Holding,
    pub board: Vec<Card>,
    /// The opponent whose exact hand the outs stage is counted against.
    pub villain: Holding,
    /// A second opponent, by assumed mix of hands, when the pot is three-way.
    pub second: Option<OppMix>,
    /// Pot before the bet.
    pub pot_before: u32,
    pub bet: u32,
    pub mode: SpotMode,
}

impl ChainHand {
    /// Players other than hero and the opponent in front of them.
    pub fn callers(&self) -> u32 {
        u32::from(self.second.is_some())
    }

    pub fn opponents(&self) -> u32 {
        self.callers() + 1
    }

    /// The pot hero faces: the pot before the bet, the bet and any callers.
    pub fn pot(&self) -> u32 {
        self.pot_before + self.bet * self.opponents()
    }

    pub fn x(&self) -> u32 {
        match self.mode {
            SpotMode::AllIn => 0,
            SpotMode::NextCard { x } => x,
        }
    }
}

/// What a [`Skill::BeatsMe`] item asks about hero's hand.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum BeatsMeAsk {
    /// Rung 0: which of these four shown hands are ahead?
    Which([Holding; 4]),
    /// Rung 1: which kinds of hand beat you?
    Kinds,
    /// Rung 2: how many ways can this one holding type be held? `high` and
    /// `low` are ranks and are equal for a pair.
    Ways { high: u8, low: u8 },
    /// Rung 3: are you ahead of this realistic mix?
    Mix(OppMix),
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BeatsMeQuestion {
    pub hero: Holding,
    pub board: Vec<Card>,
    pub ask: BeatsMeAsk,
}

impl BeatsMeQuestion {
    pub fn rung(&self) -> u8 {
        match self.ask {
            BeatsMeAsk::Which(_) => 0,
            BeatsMeAsk::Kinds => 1,
            BeatsMeAsk::Ways { .. } => 2,
            BeatsMeAsk::Mix(_) => 3,
        }
    }
}

/// Hero's hand at the "Read the price" rungs that have one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PriceHand {
    /// A named draw, with the one or two cards it may still see.
    Draw { kind: DrawKind, street: HitStreet },
    /// The player's own read of how often they are ahead, in percent.
    Read { share_pct: u32 },
}

/// What a [`Skill::ReadPrice`] item asks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReadPriceAsk {
    /// Rung 0: cheap, fair or expensive?
    Band,
    /// Rung 1: the exact percent needed.
    Number,
    /// Rung 2: easy call, close or fold, with the hand's share stated.
    Stated(PriceHand),
    /// Rung 3: the same for a draw, with its share left for the player to recall.
    Recall { kind: DrawKind, street: HitStreet },
    /// Rung 4: the smallest of four bets that makes an opponent's call a mistake.
    SetPrice { kind: DrawKind, options: [u32; 4] },
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ReadPriceQuestion {
    /// Pot before the bet; at rung 4, the pot hero is betting into.
    pub pot_before: u32,
    /// The bet faced. Unused at rung 4, where hero picks the bet.
    pub bet: u32,
    pub callers: u32,
    pub ask: ReadPriceAsk,
}

impl ReadPriceQuestion {
    pub fn rung(&self) -> u8 {
        match self.ask {
            ReadPriceAsk::Band => 0,
            ReadPriceAsk::Number => 1,
            ReadPriceAsk::Stated(_) => 2,
            ReadPriceAsk::Recall { .. } => 3,
            ReadPriceAsk::SetPrice { .. } => 4,
        }
    }
}

/// One stage of a chain, as a question.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ChainQuestion {
    pub hand: ChainHand,
    pub stage: ChainStage,
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
    /// A spot from the graded library, with its seed's suit permutation applied.
    Table(TableQuestion),
    /// One stage of a whole hand.
    Chain(ChainQuestion),
    /// A rung of the "What beats you?" ladder.
    BeatsMe(BeatsMeQuestion),
    /// A rung of the "Read the price" ladder.
    ReadPrice(ReadPriceQuestion),
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
            Question::Price { .. } | Question::Hit { .. } | Question::Anchor { .. } | Question::ReadPrice(_) => self.clone(),
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
            Question::Table(q) => Question::Table(q.with_suit_perm(perm)),
            Question::BeatsMe(q) => Question::BeatsMe(BeatsMeQuestion {
                hero: holding(&q.hero),
                board: board(&q.board),
                ask: match &q.ask {
                    BeatsMeAsk::Which(list) => BeatsMeAsk::Which([
                        holding(&list[0]),
                        holding(&list[1]),
                        holding(&list[2]),
                        holding(&list[3]),
                    ]),
                    other => other.clone(),
                },
            }),
            Question::Chain(q) => Question::Chain(ChainQuestion {
                hand: ChainHand {
                    hero: holding(&q.hand.hero),
                    board: board(&q.hand.board),
                    villain: holding(&q.hand.villain),
                    ..q.hand.clone()
                },
                stage: q.stage,
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
