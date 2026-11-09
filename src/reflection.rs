//! Structured reflection after a missed spot: three one-tap prompts whose cues
//! come from the item's real facts. Half the eligible misses are offered the
//! form, by a pure function of the seed, so whether it helps can be judged later.

use crate::cards::Card;
use crate::classes::{is_flush_draw, is_oesd, made_class};
use crate::grade::{Outcome, Reveal, SpotAnalysis};
use crate::item::{Item, Question, Skill, SpotMode, SpotQuestion};
use crate::log::{Attempt, ItemRef, Mode};
use crate::preflop::Action;
use crate::rng::Rng;
use crate::schedule::Origin;
use crate::table::{bb_text, TableGrade, TableQuestion, Verdict};

/// Misses that have to have a later sibling before the comparison is shown.
/// Starting guess.
pub const REFLECTION_MIN_MISSES: usize = 60;

const OFFER_SALT: u64 = 0x5265_666C_6563_7431;

/// One attempt row's status in the log's reflection column.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ReflectState {
    /// A missed item that the form was offered for.
    Offered,
    /// A missed item the seed held back from the form.
    Withheld,
    /// A reflection row with at least one prompt answered.
    Answered,
    /// A reflection row where the player skipped the form.
    Skipped,
}

impl ReflectState {
    pub fn code(self) -> &'static str {
        match self {
            ReflectState::Offered => "offered",
            ReflectState::Withheld => "withheld",
            ReflectState::Answered => "answered",
            ReflectState::Skipped => "skipped",
        }
    }

    pub fn from_code(code: &str) -> Option<ReflectState> {
        [ReflectState::Offered, ReflectState::Withheld, ReflectState::Answered, ReflectState::Skipped]
            .into_iter()
            .find(|s| s.code() == code)
    }
}

/// The form is shown on half of eligible misses, decided only by the seed.
pub fn reflection_offered(seed: u64) -> bool {
    Rng::new(seed ^ OFFER_SALT).chance(1, 2)
}

/// A wrong `spot` or `spot_mw` answer, or a `table` robust spot that wasn't the best play.
pub fn reflection_eligible(item: &Item, outcome: &Outcome) -> bool {
    match (&item.question, &outcome.reveal) {
        (Question::Spot(_), _) => !outcome.ok(),
        (Question::Table(_), Reveal::Table(g)) => g.spot.verdict == Verdict::Robust && g.chosen != g.spot.best,
        _ => false,
    }
}

/// What the attempt row's reflection column should hold. Retries and items the
/// form doesn't apply to are empty, so they can't enter the comparison.
pub fn reflection_state(item: &Item, outcome: &Outcome, origin: Origin) -> Option<ReflectState> {
    if origin == Origin::SessionRetry || !reflection_eligible(item, outcome) {
        return None;
    }
    Some(if reflection_offered(item.seed) { ReflectState::Offered } else { ReflectState::Withheld })
}

/// One tappable cue: `id` is logged and `text` is shown.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Cue {
    pub id: &'static str,
    pub text: String,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Prompt {
    pub id: &'static str,
    pub text: &'static str,
    pub cues: Vec<Cue>,
}

/// Prompts on the form, in the order they are asked.
pub const SUPPORTED_PROMPT: &str = "What supported your choice?";
pub const AGAINST_PROMPT: &str = "What argued against it?";
pub const ELSE_PROMPT: &str = "What else could be true?";

/// Cues on one prompt, at most.
pub const MAX_CUES: usize = 4;

fn cue(id: &'static str, text: String) -> Cue {
    Cue { id, text }
}

fn prompts(supported: Vec<Cue>, against: Vec<Cue>, otherwise: Vec<Cue>) -> Vec<Prompt> {
    let trimmed = |cues: Vec<Cue>| cues.into_iter().take(MAX_CUES).collect();
    vec![
        Prompt { id: "supported", text: SUPPORTED_PROMPT, cues: trimmed(supported) },
        Prompt { id: "against", text: AGAINST_PROMPT, cues: trimmed(against) },
        Prompt { id: "else", text: ELSE_PROMPT, cues: trimmed(otherwise) },
    ]
}

fn plain_pct(fraction: f64) -> String {
    format!("{:.0}%", fraction * 100.0)
}

fn hand_label(hero: [Card; 2], board: &[Card]) -> String {
    if board.is_empty() {
        return "two hole cards".to_string();
    }
    made_class(hero, board).label().to_string()
}

fn draw_label(hero: [Card; 2], board: &[Card]) -> Option<&'static str> {
    if is_flush_draw(hero, board) {
        Some("a flush draw")
    } else if is_oesd(hero, board) {
        Some("an open-ended straight draw")
    } else {
        None
    }
}

/// The three prompts for an eligible miss, or `None` for anything else.
pub fn reflection_form(item: &Item, outcome: &Outcome) -> Option<Vec<Prompt>> {
    if !reflection_eligible(item, outcome) {
        return None;
    }
    match (&item.question, &outcome.reveal) {
        (Question::Spot(q), Reveal::Spot { analysis, chosen, .. }) => Some(spot_form(q, analysis, *chosen)),
        (Question::Table(q), Reveal::Table(graded)) => Some(table_form(q, graded)),
        _ => None,
    }
}

fn spot_form(q: &SpotQuestion, a: &SpotAnalysis, chosen: Action) -> Vec<Prompt> {
    let hand = cue("hand", format!("Your hand: {}", hand_label(q.hero, &q.board)));
    let draw = draw_label(q.hero, &q.board).map(|d| cue("draw", format!("You had {d}")));
    let price = cue("price", format!("The price: about {} needed", plain_pct(a.required)));
    let share = cue("share", format!("Your share: about {}", plain_pct(a.share)));
    let opp = cue("opp", format!("They likely have {}", q.opponents[0].listed()));
    let players = cue("players", format!("{} players in the pot", q.opponents.len() + 1));
    let more = match q.mode {
        SpotMode::AllIn => cue("more", "All-in, no more betting".to_string()),
        SpotMode::NextCard { x: 0 } => cue("more", "One more card, nothing more to win".to_string()),
        SpotMode::NextCard { x } => cue("more", format!("About ${x} more if you hit")),
    };
    // The share goes on whichever side it really argues for: above the price or below it.
    let (mut supports_call, mut supports_fold) = (vec![hand.clone()], vec![opp.clone(), players.clone()]);
    supports_call.extend(draw.clone());
    supports_call.push(more.clone());
    supports_fold.push(price.clone());
    if a.share > a.required {
        supports_call.push(share.clone());
    } else {
        supports_fold.push(share.clone());
    }
    supports_call.push(price.clone());
    let (supported, against) = if chosen == Action::Call {
        (supports_call, supports_fold)
    } else {
        (supports_fold, supports_call)
    };
    let otherwise = vec![
        cue("otherhand", format!("They might have something other than {}", q.opponents[0].listed())),
        cue("runout", "A later card could change who is ahead".to_string()),
        players,
        more,
    ];
    prompts(supported, against, otherwise)
}

fn table_form(q: &TableQuestion, graded: &TableGrade) -> Vec<Prompt> {
    let hand = cue("hand", format!("Your hand: {}", hand_label(q.hero, &q.board)));
    let draw = draw_label(q.hero, &q.board).map(|d| cue("draw", format!("You had {d}")));
    let price = if q.call_cents > 0 {
        let needed = f64::from(q.call_cents) / f64::from(q.pot_cents + q.call_cents);
        cue("price", format!("The price: about {} needed", plain_pct(needed)))
    } else {
        cue("price", format!("Nothing to call into a {}bb pot", bb_text(q.pot_cents)))
    };
    let players = cue("players", format!("{} players at the table", q.players));
    let stack = cue("stack", format!("About {}bb left behind", bb_text(q.behind_cents)));
    let seat = cue("seat", format!("Your seat: {}", q.seat.title().to_lowercase()));
    let chosen_passive = {
        let label = graded.chosen_option().label.to_lowercase();
        label.starts_with("fold") || label.starts_with("check")
    };
    let mut aggressive = vec![hand];
    aggressive.extend(draw);
    aggressive.extend([price.clone(), seat.clone()]);
    let passive = vec![players.clone(), stack.clone(), price.clone(), seat.clone()];
    let (supported, against) = if chosen_passive { (passive, aggressive) } else { (aggressive, passive) };
    let mut otherwise = vec![cue("otherhand", "They could hold more than you think".to_string())];
    // On the river there are no cards left to come.
    if q.board.len() < 5 {
        otherwise.push(cue("runout", "A later card could change who is ahead".to_string()));
    }
    otherwise.extend([players, stack]);
    prompts(supported, against, otherwise)
}

/// The log row for a finished or skipped form. `answers` is empty for a skip.
pub fn reflection_row(from: &Attempt, answers: &str, timestamp_ms: u64) -> Attempt {
    let state = if answers.is_empty() { ReflectState::Skipped } else { ReflectState::Answered };
    Attempt {
        timestamp_ms,
        mode: Mode::Reflect,
        baseline_run: None,
        skill: from.skill,
        item: from.item.clone(),
        gen_version: from.gen_version,
        correct: false,
        close: false,
        ev_loss_pct: None,
        response_ms: 0,
        answer: if answers.is_empty() { "skip".to_string() } else { answers.to_string() },
        extra: crate::log::AttemptExtra { family: from.extra.family.clone(), reflect: Some(state), ..Default::default() },
    }
}

/// How often the next similar question was right after one kind of miss.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ReflectionGroup {
    pub misses: usize,
    pub right: usize,
}

impl ReflectionGroup {
    pub fn pct(&self) -> f64 {
        100.0 * self.right as f64 / self.misses.max(1) as f64
    }
}

/// Misses grouped by what the form did, each with its later sibling's result.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ReflectionComparison {
    /// The player answered at least one prompt.
    pub reflected: ReflectionGroup,
    /// The form was offered and skipped or left alone.
    pub skipped: ReflectionGroup,
    /// The seed held the form back.
    pub not_offered: ReflectionGroup,
    /// Misses with a later sibling attempted, across all three groups.
    pub counted: usize,
    /// Misses still waiting for a later sibling.
    pub waiting: usize,
}

impl ReflectionComparison {
    pub fn enough_data(&self) -> bool {
        self.counted >= REFLECTION_MIN_MISSES
    }
}

fn group_of(state: ReflectState, answered: bool) -> Group {
    match (state, answered) {
        (ReflectState::Withheld, _) => Group::NotOffered,
        (_, true) => Group::Reflected,
        _ => Group::Skipped,
    }
}

#[derive(Clone, Copy)]
enum Group {
    Reflected,
    Skipped,
    NotOffered,
}

/// Compares later siblings across the three groups. A sibling is the next `drill`
/// or `drill-retry` attempt in the same (skill, family) after the miss. `attempts`
/// must be in time order, as `parse_log` returns them.
pub fn reflection_comparison(attempts: &[Attempt]) -> ReflectionComparison {
    let mut out = ReflectionComparison::default();
    for (index, miss) in attempts.iter().enumerate() {
        if miss.mode != Mode::Drill {
            continue;
        }
        let Some(state) = miss.extra.reflect.filter(|s| matches!(s, ReflectState::Offered | ReflectState::Withheld)) else {
            continue;
        };
        let Some(family) = miss.extra.family.as_deref() else { continue };
        let answered = reflected_on(attempts, &miss.item, miss.skill);
        let sibling = attempts[index + 1..].iter().find(|a| {
            matches!(a.mode, Mode::Drill | Mode::DrillRetry) && a.skill == miss.skill && a.extra.family.as_deref() == Some(family)
        });
        let Some(sibling) = sibling else {
            out.waiting += 1;
            continue;
        };
        out.counted += 1;
        let group = match group_of(state, answered) {
            Group::Reflected => &mut out.reflected,
            Group::Skipped => &mut out.skipped,
            Group::NotOffered => &mut out.not_offered,
        };
        group.misses += 1;
        group.right += usize::from(sibling.ok());
    }
    out
}

fn reflected_on(attempts: &[Attempt], item: &ItemRef, skill: Skill) -> bool {
    attempts
        .iter()
        .any(|a| a.mode == Mode::Reflect && a.skill == skill && &a.item == item && a.extra.reflect == Some(ReflectState::Answered))
}
