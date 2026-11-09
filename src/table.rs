//! Table spots: a frozen library of realistic decisions graded offline by the
//! engine (see `engine/README.md`). Its verdicts are about simulated opponents
//! built on invented assumptions, not real players. Everything here is pure.
//!
//! The library is parsed once, lazily, so players who never reach the skill pay
//! nothing for it. Every number is held as a scaled integer, as printed in the
//! file, so grading has no floating-point rounding at its thresholds.

use std::collections::{HashSet, VecDeque};
use std::sync::OnceLock;

use crate::answer::AnswerOption;
use crate::cards::{parse_cards, parse_holding, Card, Holding};
use crate::grade::EvBand;
use crate::item::{is_held_out, HELD_OUT_BIT};
use crate::rng::Rng;

/// The graded library, produced by the engine. `README.md` says how.
pub const SPOTS_TEXT: &str = include_str!("../data/spots_v2.txt");

pub const SPOT_COUNT: usize = 3000;
pub const COUNT_ROBUST: usize = 562;
pub const COUNT_CLOSE: usize = 284;
pub const COUNT_DEPENDS: usize = 2154;
const FORMAT_HEADER: &str = "#SPOTS v2";
const SPOT_FIELDS: usize = 17;
const STACK_EPS: f64 = 0.02;

/// A `depends` option that is best on at least this share of simulated tables,
/// in thousandths, is a fair read and so is correct.
pub const DEPENDS_MIN_SHARE_MILLI: u16 = 150;

/// Share of `table` drill spots, in percent, by verdict. Starting guesses.
pub const DEAL_ROBUST_PCT: u64 = 45;
pub const DEAL_DEPENDS_PCT: u64 = 40;
pub const DEAL_CLOSE_PCT: u64 = 15;
/// A spot seen within this many `table` attempts is not dealt again while unseen spots remain.
pub const NO_REPEAT_WINDOW: usize = 200;

/// Robust and close spots with `id % HELD_OUT_MODULUS == 0` are reserved for checkups.
pub const HELD_OUT_MODULUS: u16 = 3;
/// Robust and close spots in one checkup form. Fixed, so forms stay comparable.
pub const CHECKUP_ROBUST: usize = 26;
pub const CHECKUP_CLOSE: usize = 14;

/// What every table spot's verdict is built on. Shown on every reveal.
pub const ASSUMPTION_NOTE: &str = "These results use simulated loose-table opponents built on assumptions, not real players.";
/// Said wherever a checkup form reuses reserved spots.
pub const REUSE_NOTE: &str = "The reserved spots have run out, so this form reuses some spots from earlier checkups.";

/// Low bits of a `table` seed: an index into the drill or checkup pool.
const POOL_INDEX_MASK: u64 = 0xFFF;
const PERM_SALT: u64 = 0x5375_6974_5065_726D;
const CHECKUP_ORDER_SALT: u64 = 0x5461_626C_6543_6B31;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Verdict {
    Robust,
    Close,
    Depends,
}

impl Verdict {
    pub fn code(self) -> &'static str {
        match self {
            Verdict::Robust => "robust",
            Verdict::Close => "close",
            Verdict::Depends => "depends",
        }
    }

    pub fn from_code(code: &str) -> Option<Verdict> {
        match code {
            "robust" => Some(Verdict::Robust),
            "close" => Some(Verdict::Close),
            "depends" => Some(Verdict::Depends),
            _ => None,
        }
    }

    /// What this verdict means, in plain words.
    pub fn explanation(self) -> &'static str {
        match self {
            Verdict::Robust => "Robust: this option was best at about 90% or more of the simulated tables and led the next option by at least 1% of the pot.",
            Verdict::Close => "Close: the top options were within 1% of the pot of each other, so the choice between them barely mattered.",
            Verdict::Depends => "No option was best at nearly all simulated tables, so the right play can change with the opponent.",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TableSeat {
    Utg,
    Utg1,
    MiddlePosition,
    Cutoff,
    Button,
    SmallBlind,
    BigBlind,
}

impl TableSeat {
    pub fn from_file(code: &str) -> Option<TableSeat> {
        match code {
            "UTG" => Some(TableSeat::Utg),
            "UTG+1" => Some(TableSeat::Utg1),
            "MP" => Some(TableSeat::MiddlePosition),
            "CO" => Some(TableSeat::Cutoff),
            "BTN" => Some(TableSeat::Button),
            "SB" => Some(TableSeat::SmallBlind),
            "BB" => Some(TableSeat::BigBlind),
            _ => None,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            TableSeat::Utg => "Under the gun",
            TableSeat::Utg1 => "Under the gun + 1",
            TableSeat::MiddlePosition => "Middle position",
            TableSeat::Cutoff => "Cutoff",
            TableSeat::Button => "Button",
            TableSeat::SmallBlind => "Small blind",
            TableSeat::BigBlind => "Big blind",
        }
    }

    pub fn sentence(self) -> &'static str {
        match self {
            TableSeat::Utg => "under the gun",
            TableSeat::Utg1 => "next after under the gun",
            TableSeat::MiddlePosition => "in middle position",
            TableSeat::Cutoff => "in the cutoff",
            TableSeat::Button => "on the button",
            TableSeat::SmallBlind => "in the small blind",
            TableSeat::BigBlind => "in the big blind",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TableStreetName {
    Preflop,
    Flop,
    Turn,
    River,
}

impl TableStreetName {
    pub fn title(self) -> &'static str {
        match self {
            TableStreetName::Preflop => "Preflop",
            TableStreetName::Flop => "Flop",
            TableStreetName::Turn => "Turn",
            TableStreetName::River => "River",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TableActionKind {
    Fold,
    Check,
    Call,
    Bet,
    Raise,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TableAction {
    pub seat: TableSeat,
    pub kind: TableActionKind,
    /// Chips put in by a call, or the street total for a bet or raise, in hundredths
    /// of a big blind. Zero for folds and checks.
    pub amount_cents: u32,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct TableStreet {
    pub street: TableStreetName,
    /// The pot before anyone acts on this street, in hundredths of a big blind.
    pub pot_start_cents: u32,
    pub actions: Vec<TableAction>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SpotOption {
    pub code: String,
    /// "Bet 1/3 pot (1bb)", as shown on the answer button.
    pub label: String,
    /// The same choice in lower case without its amount where the label spells a size.
    pub plain: String,
    pub ev_milli_bb: i32,
    pub ev_pct_tenths: i32,
    pub best_share_milli: u16,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Archetype {
    Station,
    LoosePassive,
    Nit,
    Maniac,
    Solid,
}

impl Archetype {
    pub fn from_file(name: &str) -> Option<Archetype> {
        match name {
            "station" => Some(Archetype::Station),
            "loose-passive" => Some(Archetype::LoosePassive),
            "nit" => Some(Archetype::Nit),
            "maniac" => Some(Archetype::Maniac),
            "solid" => Some(Archetype::Solid),
            _ => None,
        }
    }

    pub fn described(self) -> &'static str {
        match self {
            Archetype::Station => "a calling station",
            Archetype::LoosePassive => "a loose-passive player",
            Archetype::Nit => "a nit",
            Archetype::Maniac => "a maniac",
            Archetype::Solid => "a solid player",
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ArchetypeRow {
    pub archetype: Archetype,
    /// The main opponent's simulated weight for this type, in thousandths.
    pub weight_milli: u16,
    /// Index into [`Spot::options`] of the option that is best against this type.
    pub best: usize,
}

impl ArchetypeRow {
    /// `Against a nit: fold (about 16% of the simulated opponents here).`
    pub fn sentence(&self, spot: &Spot) -> String {
        format!(
            "Against {}: {} (about {}% of the simulated opponents here).",
            self.archetype.described(),
            spot.options[self.best].plain,
            u32::from(self.weight_milli).div_ceil(10)
        )
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Spot {
    pub id: u16,
    pub players: u8,
    pub seat: TableSeat,
    pub stack_bb: u32,
    pub hero: Holding,
    pub board: Vec<Card>,
    pub streets: Vec<TableStreet>,
    pub pot_cents: u32,
    pub call_cents: u32,
    /// Hero's chips left behind at the decision.
    pub behind_cents: u32,
    pub options: Vec<SpotOption>,
    pub verdict: Verdict,
    /// Index into `options` of the best option by mean EV.
    pub best: usize,
    pub archetypes: Vec<ArchetypeRow>,
}

/// The structured table question. Everything but `hero` and `board` is the spot as graded.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TableQuestion {
    pub spot_id: u16,
    pub players: u8,
    pub seat: TableSeat,
    pub stack_bb: u32,
    pub hero: Holding,
    pub board: Vec<Card>,
    pub streets: Vec<TableStreet>,
    pub pot_cents: u32,
    pub call_cents: u32,
    /// Hero's chips left behind at the decision.
    pub behind_cents: u32,
    pub options: Vec<AnswerOption>,
}

impl TableQuestion {
    /// The cards dealt on one street.
    pub fn street_cards(&self, street: TableStreetName) -> &[Card] {
        match street {
            TableStreetName::Preflop => &[],
            TableStreetName::Flop => &self.board[..3],
            TableStreetName::Turn => &self.board[3..4.min(self.board.len())],
            TableStreetName::River => &self.board[4.min(self.board.len())..],
        }
    }

    pub fn with_suit_perm(&self, perm: &[u8; 4]) -> TableQuestion {
        TableQuestion {
            hero: [self.hero[0].with_suit_perm(perm), self.hero[1].with_suit_perm(perm)],
            board: self.board.iter().map(|c| c.with_suit_perm(perm)).collect(),
            ..self.clone()
        }
    }
}

// ----- Parsing -----

/// A fixed-point decimal with exactly `decimals` digits, scaled to an integer.
fn parse_fixed(text: &str, decimals: usize) -> Option<i64> {
    let (sign, body) = match text.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, text),
    };
    let (whole, frac) = body.split_once('.')?;
    if whole.is_empty() || frac.len() != decimals || !whole.bytes().all(|b| b.is_ascii_digit()) || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut value: i64 = whole.parse().ok()?;
    for byte in frac.bytes() {
        value = value * 10 + i64::from(byte - b'0');
    }
    Some(sign * value)
}

fn parse_amount(text: &str) -> Option<f64> {
    let value: f64 = text.parse().ok().filter(|v: &f64| v.is_finite() && *v >= 0.0)?;
    Some(value)
}

fn cents(value: f64) -> u32 {
    (value * 100.0).round() as u32
}

fn strip<'a>(field: &'a str, key: &str) -> Option<&'a str> {
    field.strip_prefix(key)
}

fn seat_index(seat: TableSeat) -> usize {
    seat as usize
}

struct Replay {
    put: [f64; 7],
    committed: [f64; 7],
    pot: f64,
    current: f64,
    stack: f64,
}

impl Replay {
    fn new(stack: f64) -> Replay {
        let mut replay = Replay { put: [0.0; 7], committed: [0.0; 7], pot: 1.5, current: 1.0, stack };
        replay.put[seat_index(TableSeat::SmallBlind)] = 0.5;
        replay.committed[seat_index(TableSeat::SmallBlind)] = 0.5;
        replay.put[seat_index(TableSeat::BigBlind)] = 1.0;
        replay.committed[seat_index(TableSeat::BigBlind)] = 1.0;
        replay
    }

    fn next_street(&mut self) {
        self.put = [0.0; 7];
        self.current = 0.0;
    }

    fn apply(&mut self, seat: TableSeat, code: &str) -> Option<TableAction> {
        let i = seat_index(seat);
        let (kind, amount) = match code.as_bytes().first()? {
            b'f' if code.len() == 1 => (TableActionKind::Fold, 0.0),
            b'x' if code.len() == 1 => (TableActionKind::Check, 0.0),
            b'c' if code.len() == 1 => {
                let paid = (self.current - self.put[i]).max(0.0).min(self.stack - self.committed[i]);
                self.put[i] += paid;
                self.committed[i] += paid;
                self.pot += paid;
                (TableActionKind::Call, paid)
            }
            b'b' | b'r' => {
                let to = parse_amount(code.get(1..)?)?;
                let added = to - self.put[i];
                if added <= 0.0 || self.committed[i] + added > self.stack + STACK_EPS {
                    return None;
                }
                self.put[i] = to;
                self.committed[i] += added;
                self.pot += added;
                self.current = to;
                let kind = if code.starts_with('b') { TableActionKind::Bet } else { TableActionKind::Raise };
                (kind, to)
            }
            _ => return None,
        };
        Some(TableAction { seat, kind, amount_cents: cents(amount) })
    }

    fn stack_left(&self, seat: TableSeat) -> f64 {
        self.stack - self.committed[seat_index(seat)]
    }

    fn call_amount(&self, seat: TableSeat) -> f64 {
        (self.current - self.put[seat_index(seat)]).max(0.0).min(self.stack_left(seat))
    }
}

/// `835` as `8.4`, `200` as `2`, rounding halves up.
pub fn bb_text(amount_cents: u32) -> String {
    let tenths = (amount_cents + 5) / 10;
    if tenths.is_multiple_of(10) {
        format!("{}", tenths / 10)
    } else {
        format!("{}.{}", tenths / 10, tenths % 10)
    }
}

/// Builds an option's plain-language label from the replayed state: the amount the
/// engine actually used, so a capped bet is shown at its real size.
fn describe_option(code: &str, pot: f64, call_cents: u32, stack_left: f64, hero_put: f64) -> Option<(String, String)> {
    let all_in_max = stack_left + hero_put;
    let suffix = |amount: f64| if (amount - all_in_max).abs() < STACK_EPS { ", all-in" } else { "" };
    match code {
        "fold" => Some(("Fold".to_string(), "fold".to_string())),
        "check" => Some(("Check".to_string(), "check".to_string())),
        "call" => {
            let amount = bb_text(call_cents);
            let all_in = (f64::from(call_cents) / 100.0 - stack_left).abs() < STACK_EPS;
            let mark = if all_in { " (all-in)" } else { "" };
            Some((format!("Call {amount}bb{mark}"), "call".to_string()))
        }
        "bet33" | "bet67" | "bet100" => {
            let (fraction, words) = match code {
                "bet33" => (1.0 / 3.0, "1/3 pot"),
                "bet67" => (2.0 / 3.0, "2/3 pot"),
                _ => (1.0, "pot"),
            };
            let target = pot * fraction;
            let amount = target.max(1.0).min(all_in_max);
            let shown = bb_text(cents(amount));
            if (amount - target).abs() < 0.005 {
                return Some((format!("Bet {words} ({shown}bb{})", suffix(amount)), format!("bet {words}")));
            }
            // The 1bb minimum or an all-in cap changed the size, so name the amount instead.
            let mark = if (amount - all_in_max).abs() < STACK_EPS { "all-in" } else { "minimum" };
            Some((format!("Bet {shown}bb ({mark})"), format!("bet {shown}bb ({mark})")))
        }
        _ => {
            let to = parse_amount(code.strip_prefix("raise")?)?;
            let mark = if (to - all_in_max).abs() < STACK_EPS { " (all-in)" } else { "" };
            Some((format!("Raise to {}bb{mark}", bb_text(cents(to))), format!("raise to {}bb", bb_text(cents(to)))))
        }
    }
}

fn parse_options(field: &str, pot: f64, call_cents: u32, stack_left: f64, hero_put: f64) -> Option<Vec<SpotOption>> {
    field
        .split(';')
        .map(|entry| {
            let [code, ev_bb, ev_pct, share] = entry.split(':').collect::<Vec<_>>().try_into().ok()?;
            let (label, plain) = describe_option(code, pot, call_cents, stack_left, hero_put)?;
            Some(SpotOption {
                code: code.to_string(),
                label,
                plain,
                ev_milli_bb: i32::try_from(parse_fixed(ev_bb, 3)?).ok()?,
                ev_pct_tenths: i32::try_from(parse_fixed(ev_pct, 1)?).ok()?,
                best_share_milli: u16::try_from(parse_fixed(share, 3)?).ok().filter(|&s| s <= 1000)?,
            })
        })
        .collect()
}

fn parse_archetypes(field: &str, options: &[SpotOption]) -> Option<Vec<ArchetypeRow>> {
    field
        .split(';')
        .map(|entry| {
            let [name, weight, best] = entry.split(':').collect::<Vec<_>>().try_into().ok()?;
            Some(ArchetypeRow {
                archetype: Archetype::from_file(name)?,
                weight_milli: u16::try_from(parse_fixed(weight, 3)?).ok().filter(|&w| w <= 1000)?,
                best: options.iter().position(|o| o.code == best)?,
            })
        })
        .collect()
}

fn street_name(index: usize) -> TableStreetName {
    match index {
        0 => TableStreetName::Preflop,
        1 => TableStreetName::Flop,
        2 => TableStreetName::Turn,
        _ => TableStreetName::River,
    }
}

fn parse_line(line: &str) -> Option<Spot> {
    let fields: Vec<&str> = line.split('|').collect();
    if fields.len() != SPOT_FIELDS || fields[0] != "SPOT" {
        return None;
    }
    let id: u16 = fields[1].parse().ok()?;
    let players: u8 = strip(fields[2], "n=")?.parse().ok()?;
    let seat = TableSeat::from_file(fields[3])?;
    let stack_bb: u32 = strip(fields[4], "stack=")?.parse().ok()?;
    let hero = parse_holding(fields[5])?;
    let board = parse_cards(fields[6])?;
    if !(3..=5).contains(&board.len()) {
        return None;
    }
    let groups: Vec<&str> = fields[7].split('/').collect();
    if groups.len() != board.len() - 1 {
        return None;
    }
    let mut replay = Replay::new(f64::from(stack_bb));
    let mut streets = Vec::with_capacity(groups.len());
    for (index, group) in groups.iter().enumerate() {
        if index > 0 {
            replay.next_street();
        }
        let pot_start_cents = cents(replay.pot);
        let mut actions = Vec::new();
        if !group.is_empty() {
            for token in group.split(',') {
                let (seat_code, code) = token.split_once(':')?;
                actions.push(replay.apply(TableSeat::from_file(seat_code)?, code)?);
            }
        }
        streets.push(TableStreet { street: street_name(index), pot_start_cents, actions });
    }
    let pot = f64::from(u32::try_from(parse_fixed(strip(fields[8], "pot=")?, 2)?).ok()?) / 100.0;
    let call = f64::from(u32::try_from(parse_fixed(strip(fields[9], "call=")?, 2)?).ok()?) / 100.0;
    if (replay.pot - pot).abs() > STACK_EPS || (replay.call_amount(seat) - call).abs() > STACK_EPS {
        return None;
    }
    parse_fixed(strip(fields[10], "price=")?, 1)?;
    let call_cents = cents(call);
    let hero_put = replay.put[seat_index(seat)];
    let options = parse_options(fields[11], replay.pot, call_cents, replay.stack_left(seat), hero_put)?;
    let behind_cents = cents(replay.stack_left(seat));
    let verdict = Verdict::from_code(fields[12])?;
    let best_code = strip(fields[13], "best=")?;
    let best = options.iter().position(|o| o.code == best_code)?;
    let gap = parse_fixed(strip(fields[14], "gap=")?, 2)?;
    if gap < 0 {
        return None;
    }
    if !fields[16].starts_with("meta=") {
        return None;
    }
    let archetypes = parse_archetypes(fields[15], &options)?;
    if archetypes.is_empty() {
        return None;
    }
    Some(Spot { id, players, seat, stack_bb, hero, board, streets, pot_cents: cents(pot), call_cents, behind_cents, options, verdict, best, archetypes })
}

/// Parses the library text strictly: any malformed line is an error naming that line.
pub fn parse_library(text: &str) -> Result<Vec<Spot>, String> {
    let mut lines = text.lines();
    if lines.next() != Some(FORMAT_HEADER) {
        return Err(format!("the library must start with {FORMAT_HEADER}"));
    }
    let mut spots = Vec::with_capacity(SPOT_COUNT);
    for (index, line) in text.lines().enumerate().skip(1) {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let spot = parse_line(line).ok_or_else(|| format!("line {} is malformed: {line}", index + 1))?;
        if usize::from(spot.id) != spots.len() {
            return Err(format!("line {} has id {}, expected {}", index + 1, spot.id, spots.len()));
        }
        spots.push(spot);
    }
    Ok(spots)
}

/// Every spot, parsed once on first use.
pub fn spots() -> &'static [Spot] {
    static SPOTS: OnceLock<Vec<Spot>> = OnceLock::new();
    SPOTS.get_or_init(|| parse_library(SPOTS_TEXT).expect("the compiled-in spot library is valid"))
}

pub fn spot_by_id(id: u16) -> &'static Spot {
    &spots()[usize::from(id)]
}

// ----- Pools and seeds -----

/// True for a robust or close spot reserved for checkups.
pub fn is_reserved_for_checkups(spot: &Spot) -> bool {
    spot.verdict != Verdict::Depends && spot.id.is_multiple_of(HELD_OUT_MODULUS)
}

fn pool(filter: impl Fn(&Spot) -> bool) -> Vec<&'static Spot> {
    spots().iter().filter(|s| filter(s)).collect()
}

/// Spots a drill may deal: every spot not reserved for checkups.
pub fn drill_pool() -> &'static [&'static Spot] {
    static POOL: OnceLock<Vec<&'static Spot>> = OnceLock::new();
    POOL.get_or_init(|| pool(|s| !is_reserved_for_checkups(s)))
}

/// Robust and close spots reserved for checkups. Depends spots are never in it.
pub fn checkup_pool() -> &'static [&'static Spot] {
    static POOL: OnceLock<Vec<&'static Spot>> = OnceLock::new();
    POOL.get_or_init(|| pool(is_reserved_for_checkups))
}

/// The pool a seed draws from: held-out seeds use the checkup pool, others the
/// drill pool, so no plain seed can ever reach a reserved spot.
fn pool_for(seed: u64) -> &'static [&'static Spot] {
    if is_held_out(seed) {
        checkup_pool()
    } else {
        drill_pool()
    }
}

/// The spot a `table` seed refers to.
pub fn spot_of_seed(seed: u64) -> &'static Spot {
    let list = pool_for(seed);
    list[(seed & POOL_INDEX_MASK) as usize % list.len()]
}

/// `seed` with its pool-index bits set to `index`.
pub fn seed_with_index(seed: u64, index: usize) -> u64 {
    (seed & !POOL_INDEX_MASK) | index as u64
}

/// The permutation for a seed, so a repeat of a spot shows different suits.
pub fn suit_perm_for_seed(seed: u64) -> [u8; 4] {
    let mut perm = [0, 1, 2, 3];
    Rng::new(seed ^ PERM_SALT).shuffle(&mut perm);
    perm
}

fn apply_perm(card: Card, perm: &[u8; 4]) -> Card {
    card.with_suit_perm(perm)
}

/// The fully specified question for a seed.
pub fn question_for_seed(seed: u64) -> TableQuestion {
    let spot = spot_of_seed(seed);
    let perm = suit_perm_for_seed(seed);
    TableQuestion {
        spot_id: spot.id,
        players: spot.players,
        seat: spot.seat,
        stack_bb: spot.stack_bb,
        hero: [apply_perm(spot.hero[0], &perm), apply_perm(spot.hero[1], &perm)],
        board: spot.board.iter().map(|&c| apply_perm(c, &perm)).collect(),
        streets: spot.streets.clone(),
        pot_cents: spot.pot_cents,
        call_cents: spot.call_cents,
        behind_cents: spot.behind_cents,
        options: spot.options.iter().map(|o| AnswerOption { code: o.code.as_str(), label: o.label.as_str() }).collect(),
    }
}

// ----- Dealing -----

fn verdict_for_roll(roll: u64) -> Verdict {
    if roll < DEAL_ROBUST_PCT {
        Verdict::Robust
    } else if roll < DEAL_ROBUST_PCT + DEAL_DEPENDS_PCT {
        Verdict::Depends
    } else {
        Verdict::Close
    }
}

/// Picks the next drill spot and returns its seed and id. The verdict mix is
/// 45/40/15 robust/depends/close; a spot seen in the last [`NO_REPEAT_WINDOW`]
/// attempts is skipped while any unseen spot remains anywhere in the pool.
pub fn deal_spot(rng: &mut Rng, recent: &VecDeque<u16>) -> (u64, u16) {
    let want = verdict_for_roll(rng.below(100));
    let seen: HashSet<u16> = recent.iter().copied().collect();
    let drillable = drill_pool();
    let unseen_with = |verdict: Option<Verdict>| -> Vec<usize> {
        (0..drillable.len())
            .filter(|&i| verdict.is_none_or(|v| drillable[i].verdict == v) && !seen.contains(&drillable[i].id))
            .collect()
    };
    // The pool has more spots than the window, so "unseen anywhere" is never empty.
    let mut candidates = unseen_with(Some(want));
    if candidates.is_empty() {
        candidates = unseen_with(None);
    }
    let index = rng.pick(&candidates);
    let seed = seed_with_index(rng.next_u64() & !HELD_OUT_BIT, index);
    (seed, drillable[index].id)
}

// ----- Checkup forms -----

struct CheckupOrder {
    robust: Vec<usize>,
    close: Vec<usize>,
}

fn checkup_order() -> &'static CheckupOrder {
    static ORDER: OnceLock<CheckupOrder> = OnceLock::new();
    ORDER.get_or_init(|| {
        let list = checkup_pool();
        let mut rng = Rng::new(CHECKUP_ORDER_SALT);
        let mut pick = |verdict: Verdict| {
            let mut indices: Vec<usize> = (0..list.len()).filter(|&i| list[i].verdict == verdict).collect();
            rng.shuffle(&mut indices);
            indices
        };
        let robust = pick(Verdict::Robust);
        let close = pick(Verdict::Close);
        CheckupOrder { robust, close }
    })
}

/// Pool index of the spot in slot `slot` (0 to 39) of checkup form `form`. Slots
/// 0 to 25 are robust and 26 to 39 close, walking each list in a fixed order so a
/// form's spots don't overlap an earlier form's until the lists wrap.
pub fn checkup_pool_index(form: u32, slot: usize) -> usize {
    let order = checkup_order();
    if slot < CHECKUP_ROBUST {
        order.robust[(form as usize * CHECKUP_ROBUST + slot) % order.robust.len()]
    } else {
        order.close[(form as usize * CHECKUP_CLOSE + (slot - CHECKUP_ROBUST)) % order.close.len()]
    }
}

/// True when form `form` reuses a spot already used in an earlier form.
pub fn checkup_form_reuses_spots(form: u32) -> bool {
    let order = checkup_order();
    (form as usize + 1) * CHECKUP_ROBUST > order.robust.len() || (form as usize + 1) * CHECKUP_CLOSE > order.close.len()
}

// ----- Grading -----

/// True when a loss of `loss_milli_bb` is strictly under 1% of the pot, in integers
/// (`loss_bb / pot_bb * 100 < 1` is `loss_milli_bb * 10 < pot_cents`). Uses the file's
/// EV in bb rather than its percentages, which are rounded to a tenth.
pub fn inside_one_percent(loss_milli_bb: i32, pot_cents: u32) -> bool {
    i64::from(loss_milli_bb) * 10 < i64::from(pot_cents)
}

/// Correct by the spot's verdict. `inside_loss` is [`inside_one_percent`] for the chosen
/// option; `share_milli` is its best-share.
pub fn is_correct(verdict: Verdict, is_best: bool, inside_loss: bool, share_milli: u16) -> bool {
    match verdict {
        Verdict::Robust => is_best || inside_loss,
        Verdict::Close => inside_loss,
        Verdict::Depends => share_milli >= DEPENDS_MIN_SHARE_MILLI || inside_loss,
    }
}

/// How a chosen option fared. Under 1% of the pot the loss comes from the EVs in bb;
/// from 1% up it is the file's printed percentages subtracted, so the bands apply to
/// the numbers the reveal shows.
#[derive(Clone, Debug)]
pub struct TableGrade {
    pub spot: &'static Spot,
    pub chosen: usize,
    pub correct: bool,
    pub loss_pct: f64,
    pub loss_bb: f64,
    /// `None` for the best option and for a correct depends read.
    pub band: Option<EvBand>,
}

impl TableGrade {
    pub fn best(&self) -> &'static SpotOption {
        &self.spot.options[self.spot.best]
    }

    pub fn chosen_option(&self) -> &'static SpotOption {
        &self.spot.options[self.chosen]
    }

    /// The loss in words: two decimals under 1%, so a correct answer never reads as 1.0%.
    pub fn loss_text(&self) -> String {
        let pct = if self.loss_pct < 1.0 { format!("{:.2}", self.loss_pct) } else { format!("{:.1}", self.loss_pct) };
        format!("{pct}% of the pot ({:.2}bb)", self.loss_bb)
    }

    /// The line about the read on a depends spot: a fair choice, or how rarely it was best.
    pub fn read_note(&self) -> Option<String> {
        if self.spot.verdict != Verdict::Depends {
            return None;
        }
        Some(if self.correct {
            "With no read on the opponent, this was a fair choice.".to_string()
        } else {
            format!(
                "It was best at only {:.1}% of simulated tables, so it is rarely a read worth making.",
                f64::from(self.chosen_option().best_share_milli) / 10.0
            )
        })
    }
}

/// An EV in big blinds with its sign, never `-0.00`.
pub fn ev_bb_text(milli_bb: i32) -> String {
    let hundredths = (f64::from(milli_bb) / 10.0).round() / 100.0;
    if hundredths == 0.0 {
        "+0.00".to_string()
    } else {
        format!("{hundredths:+.2}")
    }
}

/// Grades option `chosen` of a spot.
pub fn grade_option(spot: &'static Spot, chosen: usize) -> TableGrade {
    let best = &spot.options[spot.best];
    let picked = &spot.options[chosen];
    let loss_milli = (best.ev_milli_bb - picked.ev_milli_bb).max(0);
    let inside = inside_one_percent(loss_milli, spot.pot_cents);
    let is_best = chosen == spot.best;
    let correct = is_correct(spot.verdict, is_best, inside, picked.best_share_milli);
    let loss_pct = if inside {
        f64::from(loss_milli) * 10.0 / f64::from(spot.pot_cents)
    } else {
        f64::from((best.ev_pct_tenths - picked.ev_pct_tenths).max(0)) / 10.0
    };
    let band = if is_best || (spot.verdict == Verdict::Depends && correct) { None } else { Some(crate::grade::ev_band(loss_pct)) };
    TableGrade {
        spot,
        chosen,
        correct,
        loss_pct,
        loss_bb: f64::from(best.ev_milli_bb - picked.ev_milli_bb).max(0.0) / 1000.0,
        band,
    }
}
