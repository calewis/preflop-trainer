//! The frozen 36-item baseline test.

use crate::cards::{cards_to_string, parse_cards, parse_holding, Card, Holding};
use crate::classes::{OppClass, OppMix};
use crate::item::{HitStreet, Item, Question, Skill, SpotMode, SpotQuestion};
use crate::rng::Rng;

pub const BASELINE_TEXT: &str = include_str!("../data/baseline_v1.txt");
pub const BASELINE_SEED: u64 = 0x0B45_E11E_2026_0001;
pub const BASELINE_MIX: [(Skill, usize); 9] = [
    (Skill::Nuts, 5),
    (Skill::Possible, 3),
    (Skill::Made, 4),
    (Skill::Beats, 4),
    (Skill::Outs, 6),
    (Skill::Price, 4),
    (Skill::Hit, 2),
    (Skill::Spot, 6),
    (Skill::SpotMw, 2),
];
pub const BASELINE_SIZE: usize = 36;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BaselineItem {
    pub id: String,
    pub item: Item,
}

fn holding_text(h: &Holding) -> String {
    format!("{}{}", h[0], h[1])
}

fn item_line(entry: &BaselineItem) -> String {
    let item = &entry.item;
    let body = match &item.question {
        Question::Preflop { .. }
        | Question::Equity(_)
        | Question::Anchor { .. }
        | Question::Table(_)
        | Question::Chain(_)
        | Question::BeatsMe(_)
        | Question::ReadPrice(_)
        | Question::BoardType { .. } => unreachable!("this skill is not part of the baseline"),
        Question::Nuts { board } | Question::Possible { board } => {
            format!("board={}", cards_to_string(board))
        }
        Question::Made { hero, board } | Question::Beats { hero, board } => {
            format!("hero={} board={}", holding_text(hero), cards_to_string(board))
        }
        Question::Outs { hero, board, villain } => format!(
            "hero={} board={} villain={}",
            holding_text(hero),
            cards_to_string(board),
            holding_text(villain)
        ),
        Question::Price { pot_before, bet, callers } => {
            format!("pot_before={pot_before} bet={bet} callers={callers}")
        }
        Question::Hit { outs, street, both } => format!(
            "outs={outs} street={} both={}",
            match street {
                HitStreet::Flop => "flop",
                HitStreet::Turn => "turn",
            },
            u8::from(*both)
        ),
        Question::Spot(q) => {
            let classes: Vec<&str> = q.opponents.iter().map(|m| m.classes[0].code()).collect();
            let mode = match q.mode {
                SpotMode::AllIn => "allin".to_string(),
                SpotMode::NextCard { x } => format!("next:{x}"),
            };
            format!(
                "hero={} board={} pot={} call={} mode={} opponents={}",
                holding_text(&q.hero),
                cards_to_string(&q.board),
                q.pot,
                q.call,
                mode,
                classes.join(",")
            )
        }
    };
    format!("{} {} seed={} {}", entry.id, item.skill.name(), item.seed, body)
}

/// The file contents for a list of baseline items.
pub fn to_text(items: &[BaselineItem]) -> String {
    let mut out = String::from(
        "# Frozen baseline, written once by the hidden gen-baseline command. Do not regenerate:\n# changing this file makes old runs incomparable. One question per line.\n",
    );
    for entry in items {
        out.push_str(&item_line(entry));
        out.push('\n');
    }
    out
}

struct Fields<'a>(Vec<(&'a str, &'a str)>);

impl Fields<'_> {
    fn get(&self, key: &str) -> Result<&str, String> {
        self.0
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| *v)
            .ok_or_else(|| format!("missing field {key}"))
    }

    fn number(&self, key: &str) -> Result<u32, String> {
        self.get(key)?.parse().map_err(|_| format!("bad number in {key}"))
    }

    fn board(&self) -> Result<Vec<Card>, String> {
        let board = parse_cards(self.get("board")?).ok_or("bad board")?;
        if (3..=5).contains(&board.len()) {
            Ok(board)
        } else {
            Err("board must have 3 to 5 cards".to_string())
        }
    }

    fn holding(&self, key: &str) -> Result<Holding, String> {
        parse_holding(self.get(key)?).ok_or_else(|| format!("bad cards in {key}"))
    }
}

fn parse_line(line: &str) -> Result<BaselineItem, String> {
    let mut parts = line.split_whitespace();
    let id = parts.next().ok_or("empty line")?.to_string();
    let skill_name = parts.next().ok_or("missing skill")?;
    let skill = Skill::from_name(skill_name).ok_or("unknown skill")?;
    let mut pairs = Vec::new();
    for part in parts {
        pairs.push(part.split_once('=').ok_or("field without =")?);
    }
    let fields = Fields(pairs);
    let seed: u64 = fields.get("seed")?.parse().map_err(|_| "bad seed")?;
    let question = match skill {
        Skill::Preflop | Skill::BoardType | Skill::Equity | Skill::Anchor | Skill::Table | Skill::Chain | Skill::BeatsMe | Skill::ReadPrice => {
            return Err(format!("{skill_name} is not part of the baseline"));
        }
        Skill::Nuts => Question::Nuts { board: fields.board()? },
        Skill::Possible => Question::Possible { board: fields.board()? },
        Skill::Made => Question::Made { hero: fields.holding("hero")?, board: fields.board()? },
        Skill::Beats => Question::Beats { hero: fields.holding("hero")?, board: fields.board()? },
        Skill::Outs => Question::Outs {
            hero: fields.holding("hero")?,
            board: fields.board()?,
            villain: fields.holding("villain")?,
        },
        Skill::Price => Question::Price {
            pot_before: fields.number("pot_before")?,
            bet: fields.number("bet")?,
            callers: fields.number("callers")?,
        },
        Skill::Hit => Question::Hit {
            outs: fields.number("outs")?,
            street: match fields.get("street")? {
                "flop" => HitStreet::Flop,
                "turn" => HitStreet::Turn,
                _ => return Err("bad street".to_string()),
            },
            both: fields.number("both")? == 1,
        },
        Skill::Spot | Skill::SpotMw => {
            let mode = match fields.get("mode")? {
                "allin" => SpotMode::AllIn,
                other => {
                    let x = other.strip_prefix("next:").ok_or("bad mode")?;
                    SpotMode::NextCard { x: x.parse().map_err(|_| "bad x")? }
                }
            };
            let mut opponents = Vec::new();
            for code in fields.get("opponents")?.split(',') {
                opponents.push(OppMix::single(OppClass::from_code(code).ok_or("unknown class")?));
            }
            Question::Spot(SpotQuestion {
                hero: fields.holding("hero")?,
                board: fields.board()?,
                pot: fields.number("pot")?,
                call: fields.number("call")?,
                mode,
                opponents,
            })
        }
    };
    Ok(BaselineItem { id, item: Item { skill, seed, question } })
}

/// Parses baseline text. Blank lines and `#` comments are ignored.
pub fn parse_baseline(text: &str) -> Result<Vec<BaselineItem>, String> {
    let mut items = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        items.push(parse_line(line).map_err(|e| format!("baseline line {}: {e}", index + 1))?);
    }
    Ok(items)
}

/// The frozen items compiled into the program.
pub fn load() -> Result<Vec<BaselineItem>, String> {
    parse_baseline(BASELINE_TEXT)
}

/// Applies a random suit permutation and shuffles the order, to reduce memorising.
pub fn randomize(items: &[BaselineItem], rng: &mut Rng) -> Vec<BaselineItem> {
    let mut perm = [0u8, 1, 2, 3];
    rng.shuffle(&mut perm);
    let mut out: Vec<BaselineItem> = items
        .iter()
        .map(|entry| BaselineItem {
            id: entry.id.clone(),
            item: Item {
                skill: entry.item.skill,
                seed: entry.item.seed,
                question: entry.item.question.with_suit_perm(&perm),
            },
        })
        .collect();
    rng.shuffle(&mut out);
    out
}
