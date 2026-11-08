//! How each item is answered, as data, so a front end can build its own controls.

use crate::classes::MadeClass;
use crate::item::Question;
use crate::preflop::{action_label, Action, Seat, Situation};

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
        },
        Question::Made { .. } => {
            AnswerFormat::Choice(MadeClass::ALL.iter().map(|c| option(c.code(), c.label())).collect())
        }
        Question::Beats { .. } => AnswerFormat::Choice(vec![
            option("a", "under 2%"),
            option("b", "2-10%"),
            option("c", "10-30%"),
            option("d", "over 30%"),
        ]),
        Question::Outs { .. } => AnswerFormat::Integer { min: 0, max: MAX_OUTS },
        Question::Price { .. } | Question::Hit { .. } => AnswerFormat::Percent,
        Question::Spot(_) => AnswerFormat::Choice(vec![option("c", "call"), option("f", "fold")]),
    }
}
