//! Practice groups: every drill is open from the first day, and the default
//! mixed session deals the groups at fixed shares. Shares and weights are
//! starting guesses, to be tuned with real data.

use crate::item::Skill;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Group {
    Preflop,
    ReadPrice,
    Equity,
    WholeHands,
    Board,
    Outs,
    Spots,
}

impl Group {
    pub const ALL: [Group; 7] =
        [Group::Preflop, Group::ReadPrice, Group::Equity, Group::WholeHands, Group::Board, Group::Outs, Group::Spots];

    /// Share of a mixed session's items, in percent. The seven add up to 100.
    pub const fn share_pct(self) -> u64 {
        match self {
            Group::Preflop => 20,
            Group::ReadPrice | Group::Equity | Group::WholeHands | Group::Board => 15,
            Group::Outs | Group::Spots => 10,
        }
    }

    /// The skills a group deals, with their relative weights inside it.
    pub const fn members(self) -> &'static [(Skill, u64)] {
        match self {
            Group::Preflop => &[(Skill::Preflop, 1)],
            Group::ReadPrice => &[(Skill::ReadPrice, 1)],
            Group::Equity => &[(Skill::Equity, 3), (Skill::Anchor, 1)],
            Group::WholeHands => &[(Skill::Chain, 1)],
            Group::Board => &[(Skill::Nuts, 1), (Skill::Possible, 1), (Skill::Made, 1), (Skill::BoardType, 1), (Skill::BeatsMe, 1)],
            Group::Outs => &[(Skill::Outs, 3), (Skill::Hit, 2)],
            // `table` is the slowest drill and graded against simulated tables, so it comes least.
            Group::Spots => &[(Skill::Spot, 2), (Skill::SpotMw, 2), (Skill::Table, 1)],
        }
    }

    pub fn skills(self) -> Vec<Skill> {
        self.members().iter().map(|&(skill, _)| skill).collect()
    }

    /// The code a front end passes to pick this group.
    pub const fn code(self) -> &'static str {
        match self {
            Group::Preflop => "preflop",
            Group::ReadPrice => "read_price",
            Group::Equity => "equity",
            Group::WholeHands => "hands",
            Group::Board => "board",
            Group::Outs => "outs",
            Group::Spots => "spots",
        }
    }

    pub fn from_code(code: &str) -> Option<Group> {
        Group::ALL.into_iter().find(|g| g.code() == code)
    }

    pub const fn title(self) -> &'static str {
        match self {
            Group::Preflop => "Preflop",
            Group::ReadPrice => "Read the price",
            Group::Equity => "Equity and anchors",
            Group::WholeHands => "Whole hands",
            Group::Board => "Board reading",
            Group::Outs => "Outs",
            Group::Spots => "Spots",
        }
    }
}

/// The group a skill is dealt in. `price` and `beats` are no longer dealt by
/// default; their rows stay readable and they can still be asked for by name.
pub fn group_of(skill: Skill) -> Option<Group> {
    Group::ALL.into_iter().find(|g| g.members().iter().any(|&(s, _)| s == skill))
}

/// Every skill the default mixed session deals, group by group.
pub fn mixed_skills() -> Vec<Skill> {
    Group::ALL.iter().flat_map(|g| g.skills()).collect()
}

/// A skill's weight inside its group, 1 for a skill outside every group.
pub fn weight_in_group(skill: Skill) -> u64 {
    group_of(skill).and_then(|g| g.members().iter().find(|&&(s, _)| s == skill).map(|&(_, w)| w)).unwrap_or(1)
}
