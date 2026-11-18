//! River spots: a library of river decisions graded offline by a solver against
//! assumed opponent ranges (`data/river_drills_v1.txt`, made in the postflop lab and
//! copied here as data), and the "River" drill that deals them.
//!
//! The library and its companion file of opponent replies are the only river data;
//! both are named here, with their version, so a regenerated library replaces them in
//! one place: copy the new files over, bump [`RIVER_LIBRARY_VERSION`] and the counts,
//! and rerun `tools/river_replies.py`. Amounts are held as the scaled integers printed
//! in the file, so grading has no floating-point rounding.
//!
//! River answers are logged in their own mode (`river`), so they count toward no pass
//! mark, checkup, baseline or review of the other drills. Everything here is pure.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use crate::answer::AnswerOption;
use crate::cards::{parse_cards, parse_holding, Card, Holding};
use crate::item::{Item, Question, Skill};
use crate::log::{Attempt, Mode};
use crate::rng::Rng;
use crate::schedule::{Origin, Planned};

/// The graded library, copied verbatim from the generator's export.
pub const RIVER_TEXT: &str = include_str!("../data/river_drills_v1.txt");
/// What the opponent does against each of hero's bets, from the same export
/// (`tools/river_replies.py`). Heads-up spots only.
pub const REPLIES_TEXT: &str = include_str!("../data/river_replies_v1.txt");
/// The library version, as in the file names.
pub const RIVER_LIBRARY_VERSION: u32 = 1;
/// `gen_version` of `river` rows: this base plus the library version. A row with any
/// other value answered another library's spot, so its spot id is not looked up.
pub const RIVER_GEN_BASE: u32 = 3000;
pub const RIVER_GEN_VERSION: u32 = RIVER_GEN_BASE + RIVER_LIBRARY_VERSION;

pub const RIVER_SPOT_COUNT: usize = 1328;
pub const RIVER_HEADS_UP: usize = 1016;
pub const RIVER_THREE_WAY: usize = 312;
pub const RIVER_CLEAR: usize = 848;
pub const RIVER_EITHER: usize = 480;

const FORMAT_HEADER: &str = "#RIVER v1";
const REPLIES_HEADER: &str = "#RIVER-REPLIES v1";
const RIVER_FIELDS: usize = 24;

/// Options within this share of the pot of the best one are graded alike, unless the
/// solver's own noise is larger (the generator's rule, repeated here for the tests).
pub const MARGIN_POT_PCT: u32 = 3;

/// Questions in one River session.
pub const DEFAULT_RIVER_ITEMS: usize = 20;
/// Shares of a session, in percent (starting guesses; the library's own split).
pub const HOME_PCT: u64 = 60;
pub const THREE_WAY_PCT: u64 = 25;
/// A spot answered within this many River attempts is not dealt as new while any
/// other spot of the wanted kind is left.
pub const NO_REPEAT_WINDOW: usize = 300;
/// At most one spot missed in an earlier session comes back in every this many items.
pub const REVIEW_EVERY: usize = 4;
/// A spot missed in this session comes back this many items later, with new suits.
const RETRY_MIN_GAP: u64 = 3;
const RETRY_MAX_GAP: u64 = 5;

/// Low bits of a `river` seed: the spot's index in the library.
const INDEX_MASK: u64 = 0x7FF;
const PERM_SALT: u64 = 0x5269_7665_7250_6572;
const SESSION_SALT: u64 = 0x5269_7665_7253_6573;
/// Seeds never set the checkup bit (63); the drill has no held-out spots.
const SEED_HIGH_MASK: u64 = !INDEX_MASK & !(1 << 63);

/// Said on every River reveal.
pub const RIVER_NOTE: &str = "Graded by a solver against assumed hand ranges for this kind of opponent, not real players. A different read can change a close spot.";

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum RiverPool {
    /// Opponents leaned to the owner's home game.
    Home,
    /// Ordinary opponents.
    General,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum RiverVerdict {
    /// Every accepted option is the same play.
    Clear,
    /// A near-tie between plays: any accepted option is fine.
    Either,
}

impl RiverVerdict {
    pub fn code(self) -> &'static str {
        match self {
            RiverVerdict::Clear => "clear",
            RiverVerdict::Either => "either",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum RiverDecision {
    /// Hero is first to act on the river.
    First,
    /// Everyone before hero checked.
    CheckedTo,
    /// Hero faces a bet of `pct` of the pot before it.
    Facing { pct: u16 },
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum RiverKind {
    Value,
    ThinValue,
    Bluff,
    Overbet,
    Check,
    BluffCatch,
    StrongVsBet,
    AirVsBet,
    Raise,
}

impl RiverKind {
    pub const ALL: [RiverKind; 9] = [
        RiverKind::Value,
        RiverKind::ThinValue,
        RiverKind::Bluff,
        RiverKind::Overbet,
        RiverKind::Check,
        RiverKind::BluffCatch,
        RiverKind::StrongVsBet,
        RiverKind::AirVsBet,
        RiverKind::Raise,
    ];

    pub fn code(self) -> &'static str {
        match self {
            RiverKind::Value => "value",
            RiverKind::ThinValue => "thin_value",
            RiverKind::Bluff => "bluff",
            RiverKind::Overbet => "overbet",
            RiverKind::Check => "check",
            RiverKind::BluffCatch => "bluff_catch",
            RiverKind::StrongVsBet => "strong_vs_bet",
            RiverKind::AirVsBet => "air_vs_bet",
            RiverKind::Raise => "raise",
        }
    }

    pub fn from_code(code: &str) -> Option<RiverKind> {
        RiverKind::ALL.into_iter().find(|k| k.code() == code)
    }
}

/// What an option does, regardless of size.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum RiverPlay {
    Check,
    Fold,
    Call,
    /// Any bet, raise or all-in.
    Aggressive,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RiverOption {
    /// `check fold call bet33 bet75 bet150 raise allin`.
    pub code: &'static str,
    /// Total put in on the river by this option, in hundredths of a big blind.
    pub amount_cents: Option<u32>,
    /// EV from the decision on, in thousandths of a big blind (folding is 0).
    pub ev_milli_bb: i32,
    /// The same as a share of the pot, in tenths of a percent.
    pub ev_pct_tenths: i32,
    /// "Bet 3/4 pot (4.7bb)", as on the answer button.
    pub label: String,
    /// "bet 3/4 pot", for sentences.
    pub plain: &'static str,
    /// "Betting 3/4 pot", for "... loses 2bb".
    pub gerund: &'static str,
}

impl RiverOption {
    pub fn play(&self) -> RiverPlay {
        match self.code {
            "check" => RiverPlay::Check,
            "fold" => RiverPlay::Fold,
            "call" => RiverPlay::Call,
            _ => RiverPlay::Aggressive,
        }
    }
}

/// A kind of hand and its share, in thousandths.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct HandShare {
    pub what: String,
    pub milli: u16,
}

/// Hero against the range he faces.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RiverSplit {
    pub beat_milli: u16,
    pub tie_milli: u16,
    pub lose_milli: u16,
    /// Shares of the whole range, biggest groups as written.
    pub you_beat: Vec<HandShare>,
    pub beats_you: Vec<HandShare>,
}

/// One part of the opponent's answer to a bet: its share of his range and what is in it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ReplyPart {
    pub milli: u16,
    /// Shares of this part, in hundredths, strongest first.
    pub groups: Vec<(String, u8)>,
}

/// How the opponent answers one of hero's bets or raises.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RiverReply {
    pub code: &'static str,
    pub fold: ReplyPart,
    pub call: ReplyPart,
    /// Hero's showdown share against the hands that call, in thousandths.
    pub call_beat_milli: Option<u16>,
    pub raise: ReplyPart,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RiverSpot {
    /// Position in the library, which is what a seed names.
    pub index: u16,
    /// `H0001` heads-up, `T0001` three-way.
    pub id: String,
    pub players: u8,
    pub pool: RiverPool,
    /// One style per villain, in the order of `villains`.
    pub styles: Vec<String>,
    pub hero_seat: String,
    pub villains: Vec<String>,
    /// River action order of everyone in the pot.
    pub order: Vec<String>,
    pub stack_bb: u32,
    pub hand: Holding,
    pub board: Vec<Card>,
    pub line: String,
    pub pot_cents: u32,
    pub call_cents: u32,
    pub behind_cents: u32,
    pub decision: RiverDecision,
    pub options: Vec<RiverOption>,
    /// Index into `options`.
    pub best: usize,
    /// Indices into `options` graded correct.
    pub accept: Vec<usize>,
    pub verdict: RiverVerdict,
    /// The best option's lead over the best option of another play, hundredths of a percent of the pot.
    pub gap_pct_hundredths: i32,
    /// The solver's noise, thousandths of a big blind.
    pub noise_milli_bb: u32,
    pub split: RiverSplit,
    pub kind: RiverKind,
    pub why: Vec<String>,
    pub replies: Vec<RiverReply>,
}

impl RiverSpot {
    /// The grading margin: the larger of the noise and [`MARGIN_POT_PCT`] of the pot,
    /// in thousandths of a big blind.
    pub fn margin_milli_bb(&self) -> u32 {
        self.noise_milli_bb.max(self.pot_cents * 10 * MARGIN_POT_PCT / 100)
    }

    pub fn best_option(&self) -> &RiverOption {
        &self.options[self.best]
    }

    /// EV given up by `option` against the best, thousandths of a big blind.
    pub fn loss_milli_bb(&self, option: usize) -> i32 {
        self.options[self.best].ev_milli_bb - self.options[option].ev_milli_bb
    }

    pub fn reply_for(&self, code: &str) -> Option<&RiverReply> {
        self.replies.iter().find(|r| r.code == code)
    }

    /// The villain whose range the split is about: the bettor when facing a bet, else
    /// the first villain in the river order.
    pub fn split_villain(&self) -> &str {
        self.order.iter().find(|s| **s != self.hero_seat).map_or("", |s| s.as_str())
    }

    pub fn family_tag(&self) -> String {
        format!("river:{}", self.id)
    }
}

/// The question a seed deals: a library spot with its seed's suit permutation.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RiverQuestion {
    pub spot_index: u16,
    pub hand: Holding,
    pub board: Vec<Card>,
    pub options: Vec<AnswerOption>,
}

impl RiverQuestion {
    pub fn spot(&self) -> &'static RiverSpot {
        &spots()[usize::from(self.spot_index)]
    }

    pub fn with_suit_perm(&self, perm: &[u8; 4]) -> RiverQuestion {
        RiverQuestion {
            hand: [self.hand[0].with_suit_perm(perm), self.hand[1].with_suit_perm(perm)],
            board: self.board.iter().map(|c| c.with_suit_perm(perm)).collect(),
            ..self.clone()
        }
    }
}

// ----- Parsing -----

/// A decimal with exactly `decimals` digits after the point, scaled to an integer.
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
        value = value.checked_mul(10)? + i64::from(byte - b'0');
    }
    Some(sign * value)
}

fn fixed_u32(text: &str, decimals: usize) -> Option<u32> {
    u32::try_from(parse_fixed(text, decimals)?).ok()
}

fn share_milli(text: &str) -> Option<u16> {
    let v = parse_fixed(text, 3)?;
    (0..=1000).contains(&v).then_some(v as u16)
}

/// `52.99` for 5299 cents, `4.7` for 470, `5` for 500.
pub fn river_bb(cents: u32) -> String {
    if cents % 100 == 0 {
        format!("{}", cents / 100)
    } else if cents % 10 == 0 {
        format!("{}.{}", cents / 100, (cents % 100) / 10)
    } else {
        format!("{}.{:02}", cents / 100, cents % 100)
    }
}

/// `2.32` for 2316 thousandths, rounded half away from zero, with the sign kept.
pub fn ev_bb_text(milli: i32) -> String {
    let cents = (milli.unsigned_abs() + 5) / 10;
    let sign = if milli < 0 && cents > 0 { "-" } else { "" };
    format!("{sign}{}", river_bb(cents))
}

fn option_words(code: &str) -> Option<(&'static str, &'static str, &'static str)> {
    // (button prefix, plain, gerund)
    Some(match code {
        "check" => ("Check", "check", "Checking"),
        "fold" => ("Fold", "fold", "Folding"),
        "call" => ("Call", "call", "Calling"),
        "bet33" => ("Bet 1/3 pot", "bet 1/3 pot", "Betting 1/3 pot"),
        "bet75" => ("Bet 3/4 pot", "bet 3/4 pot", "Betting 3/4 pot"),
        "bet150" => ("Bet 1.5x pot", "bet 1.5x pot", "Betting 1.5x pot"),
        "raise" => ("Raise to", "raise", "Raising"),
        "allin" => ("All-in", "go all-in", "Going all-in"),
        _ => return None,
    })
}

fn static_code(code: &str) -> Option<&'static str> {
    ["check", "fold", "call", "bet33", "bet75", "bet150", "raise", "allin"].into_iter().find(|c| *c == code)
}

fn parse_option(text: &str) -> Option<RiverOption> {
    let mut parts = text.split(':');
    let code = static_code(parts.next()?)?;
    let amount = parts.next()?;
    let ev = parse_fixed(parts.next()?, 3)?;
    let pct = parse_fixed(parts.next()?, 1)?;
    if parts.next().is_some() {
        return None;
    }
    let amount_cents = if amount == "-" { None } else { Some(fixed_u32(amount, 2)?) };
    // Check and fold put nothing in; everything else names its amount.
    if matches!(code, "check" | "fold") != amount_cents.is_none() {
        return None;
    }
    let (prefix, plain, gerund) = option_words(code)?;
    let label = match (code, amount_cents) {
        ("raise", Some(a)) => format!("{prefix} {}bb", river_bb(a)),
        ("call", Some(a)) => format!("{prefix} {}bb", river_bb(a)),
        (_, Some(a)) => format!("{prefix} ({}bb)", river_bb(a)),
        (_, None) => prefix.to_string(),
    };
    Some(RiverOption {
        code,
        amount_cents,
        ev_milli_bb: i32::try_from(ev).ok()?,
        ev_pct_tenths: i32::try_from(pct).ok()?,
        label,
        plain,
        gerund,
    })
}

fn parse_shares(text: &str) -> Option<Vec<HandShare>> {
    if text.is_empty() {
        return Some(Vec::new());
    }
    text.split(',')
        .map(|item| {
            let (what, share) = item.rsplit_once(':')?;
            (!what.is_empty()).then_some(())?;
            Some(HandShare { what: what.to_string(), milli: share_milli(share)? })
        })
        .collect()
}

fn parse_split(text: &str) -> Option<RiverSplit> {
    let mut parts = text.split('/');
    let head: Vec<&str> = parts.next()?.split(':').collect();
    let you_beat = parse_shares(parts.next()?)?;
    let beats_you = parse_shares(parts.next()?)?;
    if parts.next().is_some() || head.len() != 3 {
        return None;
    }
    Some(RiverSplit {
        beat_milli: share_milli(head[0])?,
        tie_milli: share_milli(head[1])?,
        lose_milli: share_milli(head[2])?,
        you_beat,
        beats_you,
    })
}

fn field<'a>(fields: &[&'a str], index: usize, key: &str) -> Result<&'a str, String> {
    fields[index].strip_prefix(key).and_then(|rest| rest.strip_prefix('=')).ok_or_else(|| format!("field {index} is not {key}="))
}

fn parse_line(line: &str, index: u16) -> Result<RiverSpot, String> {
    let fields: Vec<&str> = line.split('|').collect();
    if fields.len() != RIVER_FIELDS || fields[0] != "RIVER" {
        return Err(format!("{} fields", fields.len()));
    }
    let id = fields[1].to_string();
    let players: u8 = field(&fields, 2, "n")?.parse().map_err(|_| "bad n")?;
    let pool = match field(&fields, 3, "pool")? {
        "home" => RiverPool::Home,
        "general" => RiverPool::General,
        other => return Err(format!("pool {other}")),
    };
    let styles: Vec<String> = field(&fields, 4, "style")?.split(',').map(str::to_string).collect();
    let hero_seat = field(&fields, 5, "hero")?.to_string();
    let villains: Vec<String> = field(&fields, 6, "villains")?.split(',').map(str::to_string).collect();
    let order: Vec<String> = field(&fields, 7, "order")?.split(',').map(str::to_string).collect();
    let stack_bb: u32 = field(&fields, 8, "stack")?.parse().map_err(|_| "bad stack")?;
    let hand = parse_holding(field(&fields, 9, "hand")?).ok_or("bad hand")?;
    let board = parse_cards(field(&fields, 10, "board")?).ok_or("bad board")?;
    let line_text = field(&fields, 11, "line")?.to_string();
    let pot_cents = fixed_u32(field(&fields, 12, "pot")?, 2).ok_or("bad pot")?;
    let call_cents = fixed_u32(field(&fields, 13, "call")?, 2).ok_or("bad call")?;
    let behind_cents = fixed_u32(field(&fields, 14, "behind")?, 2).ok_or("bad behind")?;
    let decision = match field(&fields, 15, "decision")? {
        "first" => RiverDecision::First,
        "checked_to" => RiverDecision::CheckedTo,
        other => {
            let pct = other.strip_prefix("facing:").and_then(|p| fixed_u32(p, 2)).ok_or("bad decision")?;
            RiverDecision::Facing { pct: u16::try_from(pct).map_err(|_| "bad decision")? }
        }
    };
    let options: Vec<RiverOption> =
        field(&fields, 16, "opts")?.split(';').map(parse_option).collect::<Option<_>>().ok_or("bad opts")?;
    let position = |code: &str| options.iter().position(|o| o.code == code);
    let best = position(field(&fields, 17, "best")?).ok_or("best is not an option")?;
    let accept: Vec<usize> = field(&fields, 18, "accept")?
        .split(',')
        .map(position)
        .collect::<Option<_>>()
        .ok_or("accept names a missing option")?;
    let verdict = match field(&fields, 19, "verdict")? {
        "clear" => RiverVerdict::Clear,
        "either" => RiverVerdict::Either,
        other => return Err(format!("verdict {other}")),
    };
    let (gap, noise) = field(&fields, 20, "gap")?.split_once(':').ok_or("bad gap")?;
    let gap_pct_hundredths = i32::try_from(parse_fixed(gap, 2).ok_or("bad gap")?).map_err(|_| "bad gap")?;
    let noise_milli_bb = fixed_u32(noise, 3).ok_or("bad noise")?;
    let split = parse_split(field(&fields, 21, "split")?).ok_or("bad split")?;
    let kind = RiverKind::from_code(field(&fields, 22, "kind")?).ok_or("bad kind")?;
    let why: Vec<String> = field(&fields, 23, "why")?.split(" ~ ").map(|s| s.trim().to_string()).collect();

    // The facts the drill relies on, checked once here.
    let mut seen = 0u64;
    for card in hand.iter().chain(board.iter()) {
        if seen & card.mask() != 0 {
            return Err("a card appears twice".into());
        }
        seen |= card.mask();
    }
    if board.len() != 5 || !(2..=3).contains(&players) {
        return Err("not a river spot".into());
    }
    if styles.len() != villains.len() || villains.len() + 1 != usize::from(players) || order.len() != usize::from(players) {
        return Err("players do not add up".into());
    }
    if !order.contains(&hero_seat) || villains.iter().any(|v| !order.contains(v)) {
        return Err("order does not hold everyone".into());
    }
    let facing = matches!(decision, RiverDecision::Facing { .. });
    if facing != (call_cents > 0) || facing != options.iter().any(|o| o.code == "call") {
        return Err("call does not match the decision".into());
    }
    if options.iter().any(|o| o.code == "call" && o.amount_cents != Some(call_cents)) {
        return Err("call amount".into());
    }
    let mut codes = HashSet::new();
    if !options.iter().all(|o| codes.insert(o.code)) {
        return Err("an option twice".into());
    }
    if accept.is_empty() || !accept.contains(&best) {
        return Err("best is not accepted".into());
    }
    if options.iter().any(|o| o.ev_milli_bb > options[best].ev_milli_bb) {
        return Err("best is not the highest EV".into());
    }
    let plays: HashSet<RiverPlay> = accept.iter().map(|&i| options[i].play()).collect();
    if (verdict == RiverVerdict::Clear) != (plays.len() == 1) {
        return Err("verdict does not match the accepted plays".into());
    }
    if why.iter().any(|s| s.is_empty()) || line_text.is_empty() {
        return Err("empty text".into());
    }
    Ok(RiverSpot {
        index,
        id,
        players,
        pool,
        styles,
        hero_seat,
        villains,
        order,
        stack_bb,
        hand,
        board,
        line: line_text,
        pot_cents,
        call_cents,
        behind_cents,
        decision,
        options,
        best,
        accept,
        verdict,
        gap_pct_hundredths,
        noise_milli_bb,
        split,
        kind,
        why,
        replies: Vec::new(),
    })
}

fn parse_part(text: &str, key: &str) -> Option<(ReplyPart, Option<u16>)> {
    let body = text.strip_prefix(key)?.strip_prefix('=')?;
    let (head, groups) = body.split_once('/')?;
    let (share, beat) = match head.split_once(':') {
        Some((s, b)) => (share_milli(s)?, Some(share_milli(b)?)),
        None => (share_milli(head)?, None),
    };
    let groups: Vec<(String, u8)> = if groups.is_empty() {
        Vec::new()
    } else {
        groups
            .split(',')
            .map(|g| {
                let (what, v) = g.rsplit_once(':')?;
                let v = parse_fixed(v, 2)?;
                (0..=100).contains(&v).then(|| (what.to_string(), v as u8))
            })
            .collect::<Option<_>>()?
    };
    Some((ReplyPart { milli: share, groups }, beat))
}

/// Attaches the companion replies to their spots.
pub fn parse_replies(text: &str, spots: &mut [RiverSpot]) -> Result<usize, String> {
    let mut lines = text.lines();
    if lines.next() != Some(REPLIES_HEADER) {
        return Err(format!("the replies file must start with {REPLIES_HEADER}"));
    }
    let by_id: HashMap<String, usize> = spots.iter().enumerate().map(|(i, s)| (s.id.clone(), i)).collect();
    let mut count = 0;
    let mut expected = None;
    for (n, line) in lines.enumerate() {
        if let Some(meta) = line.strip_prefix("#meta replies=") {
            expected = meta.parse::<usize>().ok();
            continue;
        }
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let bad = || format!("replies line {}: {line}", n + 2);
        let fields: Vec<&str> = line.split('|').collect();
        if fields.len() != 6 || fields[0] != "REPLY" {
            return Err(bad());
        }
        let &spot = by_id.get(fields[1]).ok_or_else(bad)?;
        let code = static_code(fields[2]).ok_or_else(bad)?;
        let spot = &mut spots[spot];
        if spot.players != 2 || !spot.options.iter().any(|o| o.code == code && o.play() == RiverPlay::Aggressive) {
            return Err(bad());
        }
        let (fold, _) = parse_part(fields[3], "fold").ok_or_else(bad)?;
        let (call, call_beat_milli) = parse_part(fields[4], "call").ok_or_else(bad)?;
        let (raise, _) = parse_part(fields[5], "raise").ok_or_else(bad)?;
        let total = u32::from(fold.milli) + u32::from(call.milli) + u32::from(raise.milli);
        if !(997..=1003).contains(&total) || spot.reply_for(code).is_some() {
            return Err(bad());
        }
        spot.replies.push(RiverReply { code, fold, call, call_beat_milli, raise });
        count += 1;
    }
    if expected != Some(count) {
        return Err(format!("the replies file says {expected:?} replies but has {count}"));
    }
    Ok(count)
}

/// Parses a whole library strictly: every line must parse and the meta counts must match.
pub fn parse_library(text: &str) -> Result<Vec<RiverSpot>, String> {
    let mut lines = text.lines();
    if lines.next() != Some(FORMAT_HEADER) {
        return Err(format!("the river library must start with {FORMAT_HEADER}"));
    }
    let mut spots = Vec::new();
    let mut meta = None;
    for line in lines {
        if let Some(rest) = line.strip_prefix("#meta ") {
            meta = Some(rest.to_string());
            continue;
        }
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let index = u16::try_from(spots.len()).map_err(|_| "too many spots")?;
        let spot = parse_line(line, index).map_err(|err| format!("{}: {err}", line.split('|').nth(1).unwrap_or("?")))?;
        spots.push(spot);
    }
    if spots.len() as u64 > INDEX_MASK + 1 {
        return Err("more spots than a seed can name".into());
    }
    let mut ids = HashSet::new();
    if !spots.iter().all(|s| ids.insert(s.id.clone())) {
        return Err("a spot id appears twice".into());
    }
    let count = |f: &dyn Fn(&RiverSpot) -> bool| spots.iter().filter(|s| f(s)).count();
    let want = format!(
        "spots={} heads_up={} three_way={} clear={} either={}",
        spots.len(),
        count(&|s| s.players == 2),
        count(&|s| s.players == 3),
        count(&|s| s.verdict == RiverVerdict::Clear),
        count(&|s| s.verdict == RiverVerdict::Either),
    );
    if meta.as_deref() != Some(want.as_str()) {
        return Err(format!("meta line {meta:?} does not match the spots ({want})"));
    }
    Ok(spots)
}

/// The built-in library, with its replies, parsed on first use.
pub fn spots() -> &'static [RiverSpot] {
    static SPOTS: OnceLock<Vec<RiverSpot>> = OnceLock::new();
    SPOTS.get_or_init(|| {
        let mut spots = parse_library(RIVER_TEXT).expect("the built-in river library parses");
        parse_replies(REPLIES_TEXT, &mut spots).expect("the built-in river replies parse");
        spots
    })
}

pub fn spot_by_id(id: &str) -> Option<&'static RiverSpot> {
    static BY_ID: OnceLock<HashMap<&'static str, usize>> = OnceLock::new();
    let map = BY_ID.get_or_init(|| spots().iter().enumerate().map(|(i, s)| (s.id.as_str(), i)).collect());
    map.get(id).map(|&i| &spots()[i])
}

// ----- Seeds and questions -----

pub fn spot_of_seed(seed: u64) -> &'static RiverSpot {
    let list = spots();
    &list[(seed & INDEX_MASK) as usize % list.len()]
}

/// A seed for spot `index`, its suits picked by `high`'s other bits.
pub fn seed_for(index: u16, high: u64) -> u64 {
    (high & SEED_HIGH_MASK) | u64::from(index)
}

pub fn suit_perm_for_seed(seed: u64) -> [u8; 4] {
    let mut perm = [0, 1, 2, 3];
    Rng::new(seed ^ PERM_SALT).shuffle(&mut perm);
    perm
}

pub fn question_for_seed(seed: u64) -> RiverQuestion {
    let spot = spot_of_seed(seed);
    let perm = suit_perm_for_seed(seed);
    RiverQuestion {
        spot_index: spot.index,
        hand: [spot.hand[0].with_suit_perm(&perm), spot.hand[1].with_suit_perm(&perm)],
        board: spot.board.iter().map(|c| c.with_suit_perm(&perm)).collect(),
        options: spot.options.iter().map(|o| AnswerOption { code: o.code, label: o.label.as_str() }).collect(),
    }
}

// ----- Grading -----

#[derive(Clone, Debug)]
pub struct RiverGrade {
    pub spot: &'static RiverSpot,
    /// Index into the spot's options.
    pub chosen: usize,
    pub accepted: bool,
    pub loss_milli_bb: i32,
}

impl RiverGrade {
    pub fn chosen_option(&self) -> &'static RiverOption {
        &self.spot.options[self.chosen]
    }

    /// EV lost as a percent of the pot.
    pub fn loss_pct(&self) -> f64 {
        f64::from(self.loss_milli_bb) / (f64::from(self.spot.pot_cents) * 10.0) * 100.0
    }
}

/// Grades option `code` of `q`'s spot; `None` when the spot has no such option.
pub fn grade_code(q: &RiverQuestion, code: &str) -> Option<RiverGrade> {
    let spot = q.spot();
    let chosen = spot.options.iter().position(|o| o.code == code)?;
    Some(RiverGrade {
        spot,
        chosen,
        accepted: spot.accept.contains(&chosen),
        loss_milli_bb: spot.loss_milli_bb(chosen).max(0),
    })
}

// ----- Words, shared by every front end -----

/// An opponent style in a few words.
pub fn style_words(style: &str) -> &str {
    match style {
        "passive" => "passive, calls a lot",
        "fit-or-fold" => "fit-or-fold",
        "straightforward" => "straightforward",
        "regular" => "solid regular",
        "aggressive" => "aggressive",
        other => other,
    }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |c| c.to_uppercase().collect::<String>() + chars.as_str())
}

fn percent(milli: u16) -> String {
    format!("{}%", (u32::from(milli) + 5) / 10)
}

/// Joins `a`, `a and b`, `a, b and c`.
fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

impl RiverSpot {
    /// "Heads-up", "Three-way".
    pub fn players_text(&self) -> &'static str {
        if self.players == 2 {
            "Heads-up"
        } else {
            "Three-way"
        }
    }

    /// "BTN (passive, calls a lot)", or both villains three-way.
    pub fn opponents_text(&self) -> String {
        let each: Vec<String> = self.villains.iter().zip(&self.styles).map(|(seat, style)| format!("{seat} ({})", style_words(style))).collect();
        and_list(&each)
    }

    /// "Home game" or "Ordinary game".
    pub fn pool_text(&self) -> &'static str {
        match self.pool {
            RiverPool::Home => "home game",
            RiverPool::General => "ordinary game",
        }
    }

    /// "You (BB), 200bb deep, against BTN (passive, calls a lot) · home game".
    pub fn context_line(&self) -> String {
        format!("You are {}, {}bb deep. {} against {} · {}.", self.hero_seat, self.stack_bb, self.players_text(), self.opponents_text(), self.pool_text())
    }

    /// "Bet 21.07bb (66% of the pot)" when facing a bet.
    pub fn bet_text(&self) -> Option<String> {
        match self.decision {
            RiverDecision::Facing { pct } => Some(format!("{}bb ({}% of the pot)", river_bb(self.call_cents), pct)),
            _ => None,
        }
    }

    pub fn prompt(&self) -> &'static str {
        match self.decision {
            RiverDecision::Facing { .. } => "You face a bet on the river. What do you do?",
            RiverDecision::CheckedTo => "It is checked to you on the river. What do you do?",
            RiverDecision::First => "You are first to act on the river. What do you do?",
        }
    }

    /// The accepted options in words: "Fold", "Bet 3/4 pot or bet 1.5x pot", "Fold or raise".
    pub fn answer_text(&self) -> String {
        let mut accepted: Vec<usize> = self.accept.clone();
        accepted.sort_unstable();
        let words: Vec<String> = accepted.iter().map(|&i| self.options[i].plain.to_string()).collect();
        capitalized(&match words.as_slice() {
            [] => String::new(),
            [one] => one.clone(),
            [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
        })
    }

    /// "the hands that bet", "BTN's range", for the split line.
    pub fn split_against(&self) -> String {
        match (self.decision, self.players) {
            (RiverDecision::Facing { .. }, _) => "the hands that bet".to_string(),
            (_, 2) => "their range".to_string(),
            _ => format!("{}'s range", self.split_villain()),
        }
    }

    /// "You beat 41%: weaker pairs 30%, no pair 11%." Groups under 1% are left out.
    pub fn beat_line(&self) -> String {
        let s = &self.split;
        let parts = share_list(&s.you_beat);
        let ties = if s.tie_milli >= 5 { format!(" Ties: {}.", percent(s.tie_milli)) } else { String::new() };
        if parts.is_empty() {
            format!("You beat {} of {}.{ties}", percent(s.beat_milli), self.split_against())
        } else {
            format!("You beat {} of {}: {parts}.{ties}", percent(s.beat_milli), self.split_against())
        }
    }

    /// "Beats you 59%: two pair 30%, three of a kind 29%."
    pub fn beaten_line(&self) -> String {
        let s = &self.split;
        let parts = share_list(&s.beats_you);
        if parts.is_empty() {
            format!("Beats you: {}.", percent(s.lose_milli))
        } else {
            format!("Beats you {}: {parts}.", percent(s.lose_milli))
        }
    }

    /// What the opponent does against option `code`, when the data has it:
    /// "If you bet 3/4 pot: 46% call (two pair, top pair or an overpair), and you beat 52% of those; 43% fold (...); 11% raise."
    pub fn reply_line(&self, code: &str) -> Option<String> {
        let reply = self.reply_for(code)?;
        let option = self.options.iter().find(|o| o.code == code)?;
        let part = |p: &ReplyPart, verb: &str| -> Option<String> {
            if p.milli < 5 {
                return None;
            }
            let top: Vec<&str> = {
                let mut g: Vec<&(String, u8)> = p.groups.iter().collect();
                g.sort_by(|a, b| b.1.cmp(&a.1));
                g.iter().take(2).map(|(w, _)| w.as_str()).collect()
            };
            Some(if top.is_empty() { format!("{} {verb}", percent(p.milli)) } else { format!("{} {verb} ({})", percent(p.milli), top.join(", ")) })
        };
        let mut parts = Vec::new();
        if let Some(call) = part(&reply.call, "call") {
            parts.push(match reply.call_beat_milli {
                Some(beat) => format!("{call}, and you beat {} of those", percent(beat)),
                None => call,
            });
        }
        parts.extend(part(&reply.fold, "fold"));
        parts.extend(part(&reply.raise, "raise"));
        Some(format!("If you {}: {}.", option.plain, parts.join("; ")))
    }
}

/// "weaker pairs 30%, no pair 11%", biggest first, groups under 1% left out.
fn share_list(list: &[HandShare]) -> String {
    let mut sorted: Vec<&HandShare> = list.iter().filter(|s| s.milli >= 10).collect();
    sorted.sort_by(|a, b| b.milli.cmp(&a.milli));
    sorted.iter().take(3).map(|s| format!("{} {}", s.what, percent(s.milli))).collect::<Vec<_>>().join(", ")
}

impl RiverGrade {
    /// "Calling loses 21.07bb against the best play, 39.8% of the pot." `None` when accepted.
    pub fn loss_text(&self) -> Option<String> {
        if self.accepted {
            return None;
        }
        Some(format!(
            "{} loses {}bb against the best play, {:.1}% of the pot.",
            self.chosen_option().gerund,
            ev_bb_text(self.loss_milli_bb),
            self.loss_pct()
        ))
    }

    /// The headline: right, either is fine, or not best.
    pub fn verdict_text(&self) -> &'static str {
        match (self.accepted, self.spot.verdict) {
            (true, RiverVerdict::Either) => "Either is fine.",
            (true, RiverVerdict::Clear) => "Right.",
            (false, _) => "Not best.",
        }
    }

    /// Said on an "either is fine" spot.
    pub fn close_note(&self) -> Option<String> {
        (self.spot.verdict == RiverVerdict::Either).then(|| {
            let mut accepted = self.spot.accept.clone();
            accepted.sort_unstable();
            let names: Vec<String> = accepted.iter().map(|&i| self.spot.options[i].plain.to_string()).collect();
            format!("A close spot: {} earn about the same, so any of them is right.", capitalized(&and_list(&names)))
        })
    }
}

// ----- The drill -----

/// What the log says about one spot: right answers in a row since the last miss,
/// whether the last answer missed, and how many River attempts ago it was answered.
#[derive(Clone, Copy, Default, Debug)]
struct SpotMemory {
    seen: bool,
    streak: u32,
    last_missed: bool,
    last_at: usize,
}

#[derive(Clone, Debug)]
struct Requeue {
    index: u16,
    remaining: u64,
}

/// A River session: [`DEFAULT_RIVER_ITEMS`] questions, about [`HOME_PCT`]% home-game
/// spots and [`THREE_WAY_PCT`]% three-way, every decision kind dealt (kinds are picked
/// by the square root of their size, so small kinds turn up), spots missed in earlier
/// sessions coming back, and a miss in this session coming back a few questions later.
#[derive(Clone, Debug)]
pub struct RiverSession {
    rng: Rng,
    memory: Vec<SpotMemory>,
    attempts_seen: usize,
    reviews: Vec<u16>,
    used: HashSet<u16>,
    requeue: Vec<Requeue>,
    dealt: usize,
    retries_dealt: usize,
    reviews_dealt: usize,
    max_items: usize,
    last: Option<u16>,
}

impl RiverSession {
    pub fn new(attempts: &[Attempt], seed: u64, max_items: Option<usize>) -> RiverSession {
        let list = spots();
        let mut memory = vec![SpotMemory::default(); list.len()];
        let mut order = 0usize;
        for a in attempts {
            let Some(spot) = spot_of_row(a) else {
                continue;
            };
            order += 1;
            let m = &mut memory[usize::from(spot.index)];
            m.seen = true;
            m.last_at = order;
            if a.ok() {
                m.streak = m.streak.saturating_add(1);
                m.last_missed = false;
            } else {
                m.streak = 0;
                m.last_missed = true;
            }
        }
        // Spots whose last answer missed, the longest-waiting first.
        let mut reviews: Vec<u16> = (0..list.len() as u16).filter(|&i| memory[usize::from(i)].last_missed).collect();
        reviews.sort_by_key(|&i| memory[usize::from(i)].last_at);
        RiverSession {
            rng: Rng::new(seed ^ SESSION_SALT),
            memory,
            attempts_seen: order,
            reviews,
            used: HashSet::new(),
            requeue: Vec::new(),
            dealt: 0,
            retries_dealt: 0,
            reviews_dealt: 0,
            max_items: max_items.unwrap_or(DEFAULT_RIVER_ITEMS).max(1),
            last: None,
        }
    }

    pub fn total_items(&self) -> usize {
        self.max_items
    }

    /// Spots missed in earlier sessions and not yet answered right again.
    pub fn reviews_waiting(&self) -> usize {
        self.reviews.len()
    }

    fn planned(&mut self, index: u16, origin: Origin) -> Planned {
        self.dealt += 1;
        self.used.insert(index);
        self.last = Some(index);
        Planned { skill: Skill::River, seed: seed_for(index, self.rng.next_u64()), origin }
    }

    pub fn try_next_item(&mut self) -> Option<Planned> {
        if self.dealt >= self.max_items {
            return None;
        }
        // A miss from this session, once its gap has passed.
        if self.retries_dealt < self.dealt.div_ceil(3) {
            if let Some(pos) = self.requeue.iter().position(|r| r.remaining == 0 && Some(r.index) != self.last) {
                let index = self.requeue.remove(pos).index;
                self.retries_dealt += 1;
                return Some(self.planned(index, Origin::SessionRetry));
            }
        }
        // A miss from an earlier session, every REVIEW_EVERY items.
        if self.dealt % REVIEW_EVERY == REVIEW_EVERY - 1 && self.reviews_dealt < self.max_items / REVIEW_EVERY {
            if let Some(pos) = self.reviews.iter().position(|i| !self.used.contains(i)) {
                let index = self.reviews.remove(pos);
                self.reviews_dealt += 1;
                return Some(self.planned(index, Origin::Due));
            }
        }
        let index = self.pick_new();
        Some(self.planned(index, Origin::New))
    }

    fn pick_new(&mut self) -> u16 {
        let list = spots();
        let players = if self.rng.below(100) < THREE_WAY_PCT { 3 } else { 2 };
        let pool = if self.rng.below(100) < HOME_PCT { RiverPool::Home } else { RiverPool::General };
        let in_stratum: Vec<&RiverSpot> = list.iter().filter(|s| s.players == players && s.pool == pool).collect();
        // Kinds by the square root of their size.
        let mut kinds: Vec<(RiverKind, u64)> = Vec::new();
        for kind in RiverKind::ALL {
            let n = in_stratum.iter().filter(|s| s.kind == kind).count();
            if n > 0 {
                kinds.push((kind, ((n as f64).sqrt() * 1000.0) as u64));
            }
        }
        let total: u64 = kinds.iter().map(|k| k.1).sum();
        let mut roll = self.rng.below(total.max(1));
        let mut kind = kinds.first().map_or(RiverKind::Check, |k| k.0);
        for &(k, w) in &kinds {
            if roll < w {
                kind = k;
                break;
            }
            roll -= w;
        }
        let of_kind: Vec<&RiverSpot> = in_stratum.iter().copied().filter(|s| s.kind == kind).collect();
        let fresh = |s: &&RiverSpot, window: bool| {
            let m = self.memory[usize::from(s.index)];
            !self.used.contains(&s.index)
                && !m.last_missed
                && (!window || !m.seen || self.attempts_seen - m.last_at >= NO_REPEAT_WINDOW)
        };
        let mut candidates: Vec<&RiverSpot> = of_kind.iter().copied().filter(|s| fresh(s, true)).collect();
        if candidates.is_empty() {
            candidates = of_kind.iter().copied().filter(|s| fresh(s, false)).collect();
        }
        if candidates.is_empty() {
            candidates = list.iter().filter(|s| !self.used.contains(&s.index)).collect();
        }
        if candidates.is_empty() {
            candidates = list.iter().collect();
        }
        // Unseen spots first; among seen ones, the fewer right answers in a row the likelier.
        let weight = |s: &RiverSpot| {
            let m = self.memory[usize::from(s.index)];
            if m.seen {
                (1000 >> m.streak.min(8)).max(4)
            } else {
                4000
            }
        };
        let total: u64 = candidates.iter().map(|s| weight(s)).sum();
        let mut roll = self.rng.below(total.max(1));
        for s in &candidates {
            let w = weight(s);
            if roll < w {
                return s.index;
            }
            roll -= w;
        }
        candidates[0].index
    }

    /// Records an answer to `planned`: a miss comes back a few questions later.
    pub fn record(&mut self, planned: Planned, ok: bool) {
        for r in &mut self.requeue {
            r.remaining = r.remaining.saturating_sub(1);
        }
        let index = spot_of_seed(planned.seed).index;
        if ok {
            self.reviews.retain(|&i| i != index);
        } else if planned.origin != Origin::SessionRetry {
            let gap = self.rng.range(RETRY_MIN_GAP, RETRY_MAX_GAP) - 1;
            self.requeue.push(Requeue { index, remaining: gap });
        }
    }
}

/// The item a `river` seed deals.
pub fn item_for_seed(seed: u64) -> Item {
    Item { skill: Skill::River, seed, question: Question::River(question_for_seed(seed)) }
}

/// River answers of the current library whose spot is known: the rows the drill learns from.
pub fn answered_spots(attempts: &[Attempt]) -> usize {
    attempts.iter().filter(|a| spot_of_row(a).is_some()).count()
}

/// The spot a River row of the current library answered.
fn spot_of_row(a: &Attempt) -> Option<&'static RiverSpot> {
    if a.mode != Mode::River || a.gen_version != RIVER_GEN_VERSION {
        return None;
    }
    a.extra.family.as_deref()?.strip_prefix("river:").and_then(spot_by_id)
}
