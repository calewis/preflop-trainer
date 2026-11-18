//! Personal leak drills exported as `#LEAKS v1` text.
//! Parsing, round-trip agreement verification and weighted session dealing.

use std::collections::{HashMap, HashSet};

use crate::cards::{holding_mask, mask_of, parse_cards, parse_holding, Card, Holding};
use crate::grade::grade;
use crate::item::{Item, PriceHand, Question, ReadPriceAsk, ReadPriceQuestion, Skill};
use crate::log::{Attempt, Mode};
use crate::preflop::Situation;
use crate::rng::Rng;
use crate::schedule::{Origin, Planned};

pub const LEAKS_HEADER: &str = "#LEAKS v1";
pub const LEAK_GEN_VERSION: u32 = 2001;
pub const DEFAULT_LEAK_ITEMS: usize = 15;
pub const STREAK_DECAY: f64 = 0.45;
pub const MISS_BOOST: f64 = 1.75;
const RETRY_MIN_GAP: usize = 3;
const RETRY_MAX_GAP: usize = 5;
const MAX_POT_OR_BET: u32 = 1_000_000;

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum LeakSpec {
    Preflop {
        players: u8,
        behind: u8,
        situation: Situation,
        hand: Holding,
        hero_act: String,
        expect: String,
    },
    Price {
        street: String,
        hero_pos: String,
        board: Vec<Card>,
        hand: Holding,
        pot_before: u32,
        bet: u32,
        callers: u32,
        share_pct: u32,
        tight_pct: u8,
        loose_pct: u8,
        hero_act: String,
        expect: String,
    },
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LeakEntry {
    pub id: u16,
    pub family: String,
    pub weight_milli: u32,
    pub occ: u16,
    pub date: String,
    pub session: String,
    pub hand_id: String,
    pub hero_did: String,
    pub cost_summary: String,
    pub spec: LeakSpec,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LeakCounts {
    pub total: usize,
    pub preflop: usize,
    pub price: usize,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LeakLibrary {
    pub entries: Vec<LeakEntry>,
    pub counts: LeakCounts,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LeakRevealNote {
    pub when: String,
    pub did: String,
    pub cost: String,
}

/// Deterministic seed encoding `(entry_id, perm_slot)` with bit 63 clear.
pub const fn leak_seed(id: u16, perm_slot: u8) -> u64 {
    (id as u64) | ((perm_slot as u64) << 16)
}

pub const fn decode_leak_seed(seed: u64) -> (u16, u8) {
    ((seed & 0xFFFF) as u16, ((seed >> 16) & 0xFF) as u8)
}

/// Anonymous family tag stored in `attempts.tsv` so real hand details stay local.
pub fn family_log_tag(family: &str) -> String {
    let mut hash = 0x9E37_79B9_7F4A_7C15u64;
    for byte in family.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        hash ^= hash >> 27;
    }
    format!("leak:{hash:016x}")
}

fn ensure_period(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.ends_with('.') || trimmed.ends_with('!') || trimmed.ends_with('?') {
        trimmed.to_string()
    } else {
        format!("{trimmed}.")
    }
}

impl LeakEntry {
    pub fn skill(&self) -> Skill {
        match &self.spec {
            LeakSpec::Preflop { .. } => Skill::Preflop,
            LeakSpec::Price { .. } => Skill::ReadPrice,
        }
    }

    pub fn hero_act_code(&self) -> &str {
        match &self.spec {
            LeakSpec::Preflop { hero_act, .. } | LeakSpec::Price { hero_act, .. } => hero_act,
        }
    }

    pub fn expect_code(&self) -> &str {
        match &self.spec {
            LeakSpec::Preflop { expect, .. } | LeakSpec::Price { expect, .. } => expect,
        }
    }

    pub fn item_for_seed(&self, seed: u64) -> Item {
        let (_, perm_slot) = decode_leak_seed(seed);
        let base = match &self.spec {
            LeakSpec::Preflop { players, behind, situation, hand, .. } => Item {
                skill: Skill::Preflop,
                seed,
                question: Question::Preflop {
                    players: *players,
                    behind: *behind,
                    situation: *situation,
                    hand: *hand,
                },
            },
            LeakSpec::Price { pot_before, bet, callers, share_pct, .. } => Item {
                skill: Skill::ReadPrice,
                seed,
                question: Question::ReadPrice(ReadPriceQuestion {
                    pot_before: *pot_before,
                    bet: *bet,
                    callers: *callers,
                    ask: ReadPriceAsk::Stated(PriceHand::Read { share_pct: *share_pct }),
                }),
            },
        };
        if perm_slot == 0 {
            base
        } else {
            let perm = suit_perm_for(self.id, perm_slot);
            Item {
                skill: base.skill,
                seed: base.seed,
                question: base.question.with_suit_perm(&perm),
            }
        }
    }

    pub fn item(&self) -> Item {
        self.item_for_seed(leak_seed(self.id, 0))
    }

    /// Hero's holding and board (only present on `price` leaks) rotated identically when `perm_slot > 0`.
    pub fn cards_for_seed(&self, seed: u64) -> Option<(Holding, Vec<Card>, &str)> {
        let LeakSpec::Price { hand, board, street, .. } = &self.spec else {
            return None;
        };
        let (_, perm_slot) = decode_leak_seed(seed);
        if perm_slot == 0 {
            return Some((*hand, board.clone(), street.as_str()));
        }
        let perm = suit_perm_for(self.id, perm_slot);
        Some((
            [hand[0].with_suit_perm(&perm), hand[1].with_suit_perm(&perm)],
            board.iter().map(|c| c.with_suit_perm(&perm)).collect(),
            street.as_str(),
        ))
    }

    pub fn reveal_note(&self) -> LeakRevealNote {
        let when = if self.session.is_empty() || self.session == "unknown" {
            self.date.clone()
        } else {
            format!("{} ({})", self.date, self.session)
        };
        LeakRevealNote {
            when,
            did: ensure_period(&self.hero_did),
            cost: ensure_period(&self.cost_summary),
        }
    }
}

fn suit_perm_for(id: u16, perm_slot: u8) -> [u8; 4] {
    let mut perm = [0u8, 1, 2, 3];
    let mut rng = Rng::new((u64::from(id) << 32) ^ u64::from(perm_slot).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    rng.shuffle(&mut perm);
    perm
}

impl LeakLibrary {
    pub fn get(&self, id: u16) -> Option<&LeakEntry> {
        self.entries.get(usize::from(id)).filter(|e| e.id == id)
    }
}

fn parse_kv(fields: &[&str]) -> Result<HashMap<String, String>, String> {
    let mut map = HashMap::new();
    for &field in fields {
        let (k, v) = field.split_once('=').ok_or_else(|| format!("malformed key=value field: {field}"))?;
        if map.insert(k.to_string(), v.to_string()).is_some() {
            return Err(format!("duplicate field {k}"));
        }
    }
    Ok(map)
}

fn need<'a>(kv: &'a HashMap<String, String>, key: &str) -> Result<&'a str, String> {
    kv.get(key).map(String::as_str).ok_or_else(|| format!("missing field {key}"))
}

fn parse_situation(text: &str) -> Result<Situation, String> {
    if text == "folded" {
        return Ok(Situation::FoldedTo);
    }
    if text == "raised" {
        return Ok(Situation::Raised);
    }
    if text == "4bet" {
        return Ok(Situation::FourBet);
    }
    if text == "3bet:ip" {
        return Ok(Situation::ThreeBet { in_position: true });
    }
    if text == "3bet:oop" {
        return Ok(Situation::ThreeBet { in_position: false });
    }
    if let Some(rest) = text.strip_prefix("limpers:") {
        let n: u8 = rest.parse().map_err(|_| format!("bad limpers count in {text}"))?;
        if n == 0 {
            return Err(format!("limpers count must be >= 1 in {text}"));
        }
        return Ok(Situation::Limpers(n));
    }
    if let Some(rest) = text.strip_prefix("callers:") {
        let n: u8 = rest.parse().map_err(|_| format!("bad callers count in {text}"))?;
        if n == 0 {
            return Err(format!("callers count must be >= 1 in {text}"));
        }
        return Ok(Situation::RaisedCalled(n));
    }
    Err(format!("unknown preflop situation {text}"))
}

fn parse_pf_action(code: &str) -> Result<String, String> {
    match code {
        "r" | "c" | "f" => Ok(code.to_string()),
        _ => Err(format!("invalid preflop action code {code}")),
    }
}

fn parse_price_action(code: &str) -> Result<String, String> {
    match code {
        "c" | "f" => Ok(code.to_string()),
        _ => Err(format!("invalid price action code {code}")),
    }
}

pub fn parse_leaks(text: &str) -> Result<LeakLibrary, String> {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let header = lines.next().ok_or_else(|| "leak file is empty".to_string())?;
    if header != LEAKS_HEADER {
        return Err(format!("expected {LEAKS_HEADER} header, got {header}"));
    }

    let mut entries = Vec::new();
    let (mut preflop, mut price) = (0usize, 0usize);

    for (line_no, line) in lines.enumerate() {
        if line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() < 14 || parts[0] != "LEAK" {
            return Err(format!("malformed leak row at line {}", line_no + 2));
        }
        let id: u16 = parts[1].parse().map_err(|_| format!("bad leak id {}", parts[1]))?;
        if usize::from(id) != entries.len() {
            return Err(format!("expected sequential leak id {}, got {id}", entries.len()));
        }
        let kind = parts[2];
        let family = parts[3].trim().to_string();
        if family.is_empty() {
            return Err(format!("empty family on leak {id}"));
        }
        let weight_milli: u32 = parts[4].parse().map_err(|_| format!("bad weight_milli on leak {id}"))?;
        let occ: u16 = parts[5].parse().map_err(|_| format!("bad occ on leak {id}"))?;
        let date = parts[6].trim().to_string();
        let session = parts[7].trim().to_string();
        let hand_id = parts[8].trim().to_string();
        let hero_did = parts[11].trim().to_string();
        let cost_summary = parts[12].trim().to_string();
        if hero_did.is_empty() || cost_summary.is_empty() {
            return Err(format!("missing description on leak {id}"));
        }
        let kv = parse_kv(&parts[13..])?;

        let spec = match kind {
            "preflop" => {
                preflop += 1;
                let players: u8 = need(&kv, "players")?.parse().map_err(|_| format!("bad players on leak {id}"))?;
                let behind: u8 = need(&kv, "behind")?.parse().map_err(|_| format!("bad behind on leak {id}"))?;
                if players < 3 || behind >= players {
                    return Err(format!("invalid table geometry players={players} behind={behind} on leak {id}"));
                }
                let situation = parse_situation(need(&kv, "sit")?)?;
                let hand = parse_holding(need(&kv, "hand")?).ok_or_else(|| format!("bad hand cards on leak {id}"))?;
                let hero_act = parse_pf_action(need(&kv, "hero_act")?)?;
                let expect = parse_pf_action(need(&kv, "expect")?)?;
                LeakSpec::Preflop { players, behind, situation, hand, hero_act, expect }
            }
            "price" => {
                price += 1;
                let street = need(&kv, "street")?.to_string();
                if !matches!(street.as_str(), "flop" | "turn" | "river") {
                    return Err(format!("bad street {street} on leak {id}"));
                }
                let hero_pos = need(&kv, "hero_pos")?.to_string();
                let board = parse_cards(need(&kv, "board")?).ok_or_else(|| format!("bad board cards on leak {id}"))?;
                if !(3..=5).contains(&board.len()) {
                    return Err(format!("board length {} invalid on leak {id}", board.len()));
                }
                let hand = parse_holding(need(&kv, "hand")?).ok_or_else(|| format!("bad hand cards on leak {id}"))?;
                if holding_mask(&hand) & mask_of(&board) != 0 {
                    return Err(format!("duplicate card between hand and board on leak {id}"));
                }
                let pot_before: u32 = need(&kv, "pot_before")?.parse().map_err(|_| format!("bad pot_before on leak {id}"))?;
                let bet: u32 = need(&kv, "bet")?.parse().map_err(|_| format!("bad bet on leak {id}"))?;
                let callers: u32 = need(&kv, "callers")?.parse().map_err(|_| format!("bad callers on leak {id}"))?;
                let share_pct: u32 = need(&kv, "share_pct")?.parse().map_err(|_| format!("bad share_pct on leak {id}"))?;
                let tight_pct: u8 = need(&kv, "tight_pct")?.parse().map_err(|_| format!("bad tight_pct on leak {id}"))?;
                let loose_pct: u8 = need(&kv, "loose_pct")?.parse().map_err(|_| format!("bad loose_pct on leak {id}"))?;
                if !(1..=MAX_POT_OR_BET).contains(&pot_before) || !(1..=MAX_POT_OR_BET).contains(&bet) || callers > 8 || share_pct > 100 {
                    return Err(format!("invalid price values on leak {id}"));
                }
                let hero_act = parse_price_action(need(&kv, "hero_act")?)?;
                let expect = parse_price_action(need(&kv, "expect")?)?;
                LeakSpec::Price {
                    street,
                    hero_pos,
                    board,
                    hand,
                    pot_before,
                    bet,
                    callers,
                    share_pct,
                    tight_pct,
                    loose_pct,
                    hero_act,
                    expect,
                }
            }
            other => return Err(format!("unknown leak kind {other} on leak {id}")),
        };

        entries.push(LeakEntry {
            id,
            family,
            weight_milli: weight_milli.max(1),
            occ,
            date,
            session,
            hand_id,
            hero_did,
            cost_summary,
            spec,
        });
    }

    if entries.is_empty() {
        return Err("leak file contains zero leaks".to_string());
    }
    let counts = LeakCounts { total: entries.len(), preflop, price };
    Ok(LeakLibrary { entries, counts })
}

/// Checks every exported leak against the trainer's own grader: `expect` must
/// grade correct and `hero_act` must grade incorrect (100% agreement gate).
pub fn verify_leaks(lib: &LeakLibrary) -> Result<LeakCounts, Vec<String>> {
    let mut errors = Vec::new();
    for entry in &lib.entries {
        let item = entry.item();
        let exp = entry.expect_code();
        match grade(&item, exp) {
            Ok(out) if out.correct => {}
            Ok(out) => errors.push(format!(
                "leak {} ({}): expected answer {exp:?} graded wrong (correct={}, picked={})",
                entry.id, entry.hand_id, out.correct, out.answer
            )),
            Err(err) => errors.push(format!("leak {} ({}): invalid expected answer {exp:?}: {}", entry.id, entry.hand_id, err.0)),
        }
        let hero_act = entry.hero_act_code();
        match grade(&item, hero_act) {
            Ok(out) if !out.correct => {}
            Ok(_) => errors.push(format!(
                "leak {} ({}): trainer graded Hero's flagged action {hero_act:?} as right",
                entry.id, entry.hand_id
            )),
            Err(err) => errors.push(format!("leak {} ({}): invalid hero_act {hero_act:?}: {}", entry.id, entry.hand_id, err.0)),
        }
    }
    if errors.is_empty() {
        Ok(lib.counts)
    } else {
        Err(errors)
    }
}

#[derive(Clone, Copy, Default)]
struct FamilyMastery {
    streak: u32,
    last_missed: bool,
}

fn family_mastery(attempts: &[Attempt]) -> HashMap<String, FamilyMastery> {
    let mut map: HashMap<String, FamilyMastery> = HashMap::new();
    for a in attempts {
        if a.mode != Mode::Leak {
            continue;
        }
        let Some(tag) = a.extra.family.as_deref() else { continue };
        let st = map.entry(tag.to_string()).or_default();
        if a.ok() {
            st.streak = st.streak.saturating_add(1);
            st.last_missed = false;
        } else {
            st.streak = 0;
            st.last_missed = true;
        }
    }
    map
}

#[derive(Clone)]
struct LeakRequeue {
    id: u16,
    perm_slot: u8,
    remaining: usize,
}

#[derive(Clone)]
pub struct LeakSession {
    lib: LeakLibrary,
    rng: Rng,
    weights: Vec<u64>,
    used_in_cycle: HashSet<u16>,
    cycle_perm: u8,
    requeue: Vec<LeakRequeue>,
    dealt: usize,
    retries_dealt: usize,
    max_items: usize,
    last_id: Option<u16>,
    last_hand_id: Option<String>,
    /// Indices into the library's entries this session may deal.
    eligible: Vec<usize>,
}

impl LeakSession {
    pub fn new(lib: LeakLibrary, attempts: &[Attempt], seed: u64, max_items: Option<usize>) -> LeakSession {
        let mastery = family_mastery(attempts);
        let lib_len = lib.entries.len();
        let weights: Vec<u64> = lib
            .entries
            .iter()
            .map(|e| {
                let tag = family_log_tag(&e.family);
                let m = mastery.get(&tag).copied().unwrap_or_default();
                let mult = STREAK_DECAY.powi(m.streak.min(10) as i32) * if m.last_missed { MISS_BOOST } else { 1.0 };
                ((f64::from(e.weight_milli) * mult).round() as u64).max(1)
            })
            .collect();
        LeakSession {
            lib,
            rng: Rng::new(seed ^ 0x4C45_414B_5331_3031),
            weights,
            used_in_cycle: HashSet::new(),
            cycle_perm: 0,
            requeue: Vec::new(),
            dealt: 0,
            retries_dealt: 0,
            max_items: max_items.unwrap_or(DEFAULT_LEAK_ITEMS).max(1),
            last_id: None,
            last_hand_id: None,
            eligible: (0..lib_len).collect(),
        }
    }

    /// The same session dealing only the preflop leaks ("Preflop mistakes").
    pub fn preflop_only(mut self) -> LeakSession {
        self.eligible = (0..self.lib.entries.len())
            .filter(|&idx| matches!(self.lib.entries[idx].spec, LeakSpec::Preflop { .. }))
            .collect();
        self
    }

    /// How many leaks this session can deal from.
    pub fn eligible_count(&self) -> usize {
        self.eligible.len()
    }

    pub fn total_items(&self) -> usize {
        self.max_items
    }

    pub fn try_next_item(&mut self) -> Option<Planned> {
        if self.dealt >= self.max_items || self.eligible.is_empty() {
            return None;
        }
        let max_retries = (self.dealt + 1) / 3;
        if self.retries_dealt < max_retries {
            if let Some(pos) = self.requeue.iter().position(|r| r.remaining == 0) {
                let req = self.requeue.remove(pos);
                if let Some(entry) = self.lib.get(req.id) {
                    self.dealt += 1;
                    self.retries_dealt += 1;
                    self.last_id = Some(req.id);
                    self.last_hand_id = Some(entry.hand_id.clone());
                    return Some(Planned {
                        skill: entry.skill(),
                        seed: leak_seed(entry.id, req.perm_slot),
                        origin: Origin::SessionRetry,
                    });
                }
            }
        }

        if self.eligible.iter().all(|&idx| self.used_in_cycle.contains(&self.lib.entries[idx].id)) {
            self.used_in_cycle.clear();
            self.cycle_perm = self.cycle_perm.wrapping_add(1);
            if self.cycle_perm == 0 {
                self.cycle_perm = 1;
            }
        }

        let mut candidates: Vec<usize> = self
            .eligible
            .iter()
            .copied()
            .filter(|&idx| {
                let e = &self.lib.entries[idx];
                !self.used_in_cycle.contains(&e.id)
                    && self.last_id != Some(e.id)
                    && self.last_hand_id.as_deref() != Some(e.hand_id.as_str())
            })
            .collect();
        if candidates.is_empty() {
            candidates = self
                .eligible
                .iter()
                .copied()
                .filter(|&idx| !self.used_in_cycle.contains(&self.lib.entries[idx].id))
                .collect();
        }
        if candidates.is_empty() {
            candidates = self.eligible.clone();
        }

        let total_w: u64 = candidates.iter().map(|&idx| self.weights[idx]).sum();
        let mut roll = self.rng.below(total_w.max(1));
        let mut picked_idx = candidates[0];
        for &idx in &candidates {
            let w = self.weights[idx];
            if roll < w {
                picked_idx = idx;
                break;
            }
            roll -= w;
        }

        let entry = &self.lib.entries[picked_idx];
        self.used_in_cycle.insert(entry.id);
        self.dealt += 1;
        self.last_id = Some(entry.id);
        self.last_hand_id = Some(entry.hand_id.clone());
        Some(Planned {
            skill: entry.skill(),
            seed: leak_seed(entry.id, self.cycle_perm),
            origin: Origin::New,
        })
    }

    pub fn item_for(&self, planned: Planned) -> Item {
        let (id, _) = decode_leak_seed(planned.seed);
        self.lib
            .get(id)
            .expect("valid leak entry id")
            .item_for_seed(planned.seed)
    }

    pub fn entry_for_seed(&self, seed: u64) -> Option<&LeakEntry> {
        let (id, _) = decode_leak_seed(seed);
        self.lib.get(id)
    }

    pub fn record(&mut self, planned: Planned, ok: bool) {
        for r in &mut self.requeue {
            r.remaining = r.remaining.saturating_sub(1);
        }
        if !ok && planned.origin != Origin::SessionRetry {
            let (id, perm_slot) = decode_leak_seed(planned.seed);
            let gap = self.rng.range(RETRY_MIN_GAP as u64, RETRY_MAX_GAP as u64) as usize;
            let next_perm = perm_slot.wrapping_add(1);
            self.requeue.push(LeakRequeue {
                id,
                perm_slot: if next_perm == 0 { 1 } else { next_perm },
                remaining: gap,
            });
        }
    }
}
