//! How each item is answered, as data, so a front end can build its own controls.

use crate::beats_me::{BeatKind, MixBand};
use crate::classes::MadeClass;
use crate::item::{BeatsMeAsk, BeatsMeQuestion, ChainQuestion, ChainStage, Question, ReadPriceAsk, ReadPriceQuestion};
use crate::preflop::{action_label, Action, Seat, Situation};
use crate::read_price::{dollar_label, PriceBand, PriceCall};

/// Largest answer an `outs` item accepts.
pub const MAX_OUTS: u32 = 47;

/// One selectable answer. `code` is exactly what [`crate::grade::grade`] accepts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AnswerOption {
    pub code: &'static str,
    pub label: &'static str,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum AnswerFormat {
    /// Pick exactly one option.
    Choice(Vec<AnswerOption>),
    /// Pick any subset: submit the codes joined together, or `none_code` for no options.
    MultiSelect {
        options: Vec<AnswerOption>,
        none_code: &'static str,
        none_label: &'static str,
    },
    /// A whole number in `min..=max`.
    Integer { min: u32, max: u32 },
    /// A percentage from 0 to 100, as `25` or `25%`.
    Percent,
    /// Holding notation: two ranks (`QJ`), ranks with `s` or `o` (`AKs`), two
    /// exact cards (`AhKh`), or a card or rank plus `x` (`Ahx`, `Ax`).
    Holding,
}

fn option(code: &'static str, label: &'static str) -> AnswerOption {
    AnswerOption { code, label }
}

fn preflop_options(behind: u8, situation: Situation) -> Vec<AnswerOption> {
    let seat = Seat::from_behind(behind);
    let label = |action| action_label(action, seat, situation);
    let mut options = vec![
        option(Action::Raise.code(), label(Action::Raise)),
        option(Action::Call.code(), label(Action::Call)),
    ];
    // Checking in the big blind is free, so folding is not offered there.
    if !(seat == Seat::BigBlind && matches!(situation, Situation::Limpers(_))) {
        options.push(option(Action::Fold.code(), label(Action::Fold)));
    }
    options
}

/// Largest answer a `beats_me` count accepts: two different ranks with all cards unseen.
pub const MAX_WAYS: u32 = 16;

fn beats_me_format(q: &BeatsMeQuestion) -> AnswerFormat {
    match q.ask {
        BeatsMeAsk::Which(_) => AnswerFormat::MultiSelect {
            options: vec![option("a", "Hand A"), option("b", "Hand B"), option("c", "Hand C"), option("d", "Hand D")],
            none_code: "none",
            none_label: "None of them",
        },
        BeatsMeAsk::Kinds => AnswerFormat::MultiSelect {
            options: BeatKind::ALL.iter().map(|k| option(KIND_CODES[k_index(*k)], k.label())).collect(),
            none_code: "none",
            none_label: "Nothing beats you",
        },
        BeatsMeAsk::Ways { .. } => AnswerFormat::Integer { min: 0, max: MAX_WAYS },
        BeatsMeAsk::Mix(_) => AnswerFormat::Choice(MixBand::ALL.iter().map(|b| option(b.code(), b.label())).collect()),
    }
}

fn read_price_format(q: &ReadPriceQuestion) -> AnswerFormat {
    match q.ask {
        ReadPriceAsk::Band => AnswerFormat::Choice(PriceBand::ALL.iter().map(|b| option(b.code(), b.label())).collect()),
        ReadPriceAsk::Number => AnswerFormat::Percent,
        ReadPriceAsk::Stated(_) | ReadPriceAsk::Recall { .. } => {
            AnswerFormat::Choice(PriceCall::ALL.iter().map(|c| option(c.code(), c.label())).collect())
        }
        ReadPriceAsk::SetPrice { options, .. } => AnswerFormat::Choice(
            options.iter().zip(["a", "b", "c", "d"]).map(|(&amount, code)| option(code, dollar_label(amount))).collect(),
        ),
    }
}

/// The kinds' codes as static strings, in [`BeatKind::ALL`] order.
const KIND_CODES: [&str; 7] = ["p", "k", "t", "s", "r", "f", "h"];

fn k_index(kind: BeatKind) -> usize {
    BeatKind::ALL.iter().position(|k| *k == kind).expect("in ALL")
}

/// How the given question is answered.
pub fn answer_format(question: &Question) -> AnswerFormat {
    match question {
        Question::Preflop { behind, situation, .. } => {
            AnswerFormat::Choice(preflop_options(*behind, *situation))
        }
        Question::Nuts { .. } => AnswerFormat::Holding,
        Question::Possible { .. } => AnswerFormat::MultiSelect {
            options: vec![
                option("f", "flush"),
                option("s", "straight"),
                option("h", "full house or quads"),
            ],
            none_code: "none",
            none_label: "None of these",
        },
        Question::Made { .. } | Question::Chain(ChainQuestion { stage: ChainStage::Hand, .. }) => {
            AnswerFormat::Choice(MadeClass::ALL.iter().map(|c| option(c.code(), c.label())).collect())
        }
        Question::Beats { .. } => AnswerFormat::Choice(vec![
            option("a", "under 2%"),
            option("b", "2-10%"),
            option("c", "10-30%"),
            option("d", "over 30%"),
        ]),
        Question::Outs { .. } | Question::Chain(ChainQuestion { stage: ChainStage::Outs, .. }) => {
            AnswerFormat::Integer { min: 0, max: MAX_OUTS }
        }
        Question::Price { .. }
        | Question::Hit { .. }
        | Question::Equity(_)
        | Question::Anchor { .. }
        | Question::Chain(ChainQuestion { stage: ChainStage::Price, .. }) => AnswerFormat::Percent,
        Question::Spot(_) | Question::Chain(ChainQuestion { stage: ChainStage::Decide, .. }) => {
            AnswerFormat::Choice(vec![option("c", "call"), option("f", "fold")])
        }
        Question::BoardType { options, .. } => {
            AnswerFormat::Choice(options.iter().map(|c| option(c.code(), c.label())).collect())
        }
        Question::Table(q) => AnswerFormat::Choice(q.options.clone()),
        Question::BeatsMe(q) => beats_me_format(q),
        Question::ReadPrice(q) => read_price_format(q),
    }
}
