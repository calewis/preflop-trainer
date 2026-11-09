//! Pure preflop charts derived by Monte Carlo against the sampled population.
//! See `PREFLOP.md`. Everything here is conditional on the invented models in
//! `models.rs`; nothing in this module observes real players.
//!
//! One trial: draw a table of population models and cards, conditioned so the
//! betting before hero matches a chart's situation, then play out every action
//! hero could take with the same cards, models and runout (common random
//! numbers). Hero's later preflop decisions use charts already derived for
//! later situations, and postflop he plays the engine's simple heuristic.
//!
//! Shared work: a batch uses a pool of runouts, each with every combo's class
//! on each street and river rank precomputed, and a pool of models with their
//! postflop tables. A trial picks from the pools, so one trial costs a few
//! microseconds. Batches are independent (separate pools), so batch means give
//! honest standard errors that include the pools' own sampling error.

use crate::board::classify;
use crate::combos::{class_combos, class_name, combos, NCOMBOS};
use crate::game::{Act, Round};
use crate::preflop_cal::Calibration;
use crate::models::{hand_percentiles, PlayerModel, VillainTables, NBUCKET};
use crate::sim::{hero_heuristic, hero_preflop, hero_preflop_raise, villain_postflop, TABLE_SIZES};
use crate::spot::{clamp_preflop_raise, position_of, preflop_can_raise, PreflopTrack};
use poker_trainer::cards::{Card, Holding};
use poker_trainer::eval::evaluate;
use poker_trainer::preflop::Seat;
use poker_trainer::rng::Rng;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

pub const STACK: f64 = 100.0;
pub const NCLASSES: usize = 169;
pub const MAX_BATCHES: usize = 64;
/// Early-stop confidence: a cell stops once its choice is this many standard
/// errors from changing. Larger than the 2-sigma used for reporting, since it
/// is checked repeatedly.
pub const STOP_Z: f64 = 3.0;

/// One pure action. Order is strength: `Fold < Passive < Raise`.
/// `Passive` is call, limp or check depending on the chart.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub enum Pure {
    Fold = 0,
    Passive = 1,
    Raise = 2,
}

pub const PURES: [Pure; 3] = [Pure::Fold, Pure::Passive, Pure::Raise];

const fn bit(s: Seat) -> u8 {
    1 << (s as usize)
}
pub const E: u8 = bit(Seat::Early);
pub const M: u8 = bit(Seat::Middle);
pub const CO: u8 = bit(Seat::Cutoff);
pub const BTN: u8 = bit(Seat::Button);
pub const SB: u8 = bit(Seat::SmallBlind);
pub const BB: u8 = bit(Seat::BigBlind);
pub const IP_SEATS: u8 = E | M | CO | BTN;

/// What the betting before hero must look like.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Sit {
    /// Everyone before hero folded.
    FoldedTo,
    /// Between `lo` and `hi` players limped, the rest folded, no raises.
    Limped { lo: u8, hi: u8 },
    /// No limpers, one raise from a seat in `opener`, then between `lo` and
    /// `hi` cold-calls before hero, everyone else folded.
    FacingOpen { opener: u8, lo: u8, hi: u8 },
    /// Hero opened after folds, one villain three-bet, everyone else folded.
    /// `ip` is whether hero acts after the three-bettor postflop.
    Opened3bet { ip: bool },
}

/// One chart. `labels` are the (fold, passive, raise) names, `None` if illegal.
#[derive(Clone, Copy, Debug)]
pub struct ChartDef {
    pub id: &'static str,
    pub desc: &'static str,
    pub seat: &'static str,
    pub situation: &'static str,
    pub sit: Sit,
    pub hero: u8,
    pub labels: [Option<&'static str>; 3],
    /// Derivation stage; later stages may use earlier stages' charts.
    pub stage: u8,
}

const OPEN: [Option<&str>; 3] = [Some("fold"), None, Some("raise")];
const LIMP: [Option<&str>; 3] = [Some("fold"), Some("limp"), Some("raise")];
const BBLIMP: [Option<&str>; 3] = [None, Some("check"), Some("raise")];
const FACE: [Option<&str>; 3] = [Some("fold"), Some("call"), Some("3bet")];
const VS3: [Option<&str>; 3] = [Some("fold"), Some("call"), Some("4bet")];

const fn face(opener: u8, lo: u8, hi: u8) -> Sit {
    Sit::FacingOpen { opener, lo, hi }
}

macro_rules! chart {
    ($id:literal, $desc:literal, $seat:literal, $sit_name:literal, $sit:expr, $hero:expr, $labels:expr, $stage:literal) => {
        ChartDef { id: $id, desc: $desc, seat: $seat, situation: $sit_name, sit: $sit, hero: $hero, labels: $labels, stage: $stage }
    };
}

/// The charts, in stage order. Index constants below depend on this order and
/// are asserted by a unit test.
pub const CHARTS: [ChartDef; 30] = [
    // Stage 0: you opened and got three-bet.
    chart!("vs3bet.ip", "You opened and were 3-bet by a blind; you act after them postflop", "any", "opened_3bet_ip", Sit::Opened3bet { ip: true }, E | M | CO | BTN, VS3, 0),
    chart!("vs3bet.oop", "You opened and were 3-bet by a seat that acts after you postflop", "any", "opened_3bet_oop", Sit::Opened3bet { ip: false }, E | M | CO | BTN | SB, VS3, 0),
    // Stage 1: facing an open.
    chart!("vs_em.ip", "Open from early or middle, no callers, you are in position", "in_position", "facing_open_em", face(E | M, 0, 0), IP_SEATS, FACE, 1),
    chart!("vs_em.sb", "Open from early or middle, no callers, you are in the small blind", "small_blind", "facing_open_em", face(E | M, 0, 0), SB, FACE, 1),
    chart!("vs_em.bb", "Open from early or middle, no callers, you are in the big blind", "big_blind", "facing_open_em", face(E | M, 0, 0), BB, FACE, 1),
    chart!("vs_co.ip", "Open from the cutoff, no callers, you are on the button", "in_position", "facing_open_co", face(CO, 0, 0), IP_SEATS, FACE, 1),
    chart!("vs_co.sb", "Open from the cutoff, no callers, you are in the small blind", "small_blind", "facing_open_co", face(CO, 0, 0), SB, FACE, 1),
    chart!("vs_co.bb", "Open from the cutoff, no callers, you are in the big blind", "big_blind", "facing_open_co", face(CO, 0, 0), BB, FACE, 1),
    chart!("vs_late.sb", "Open from the button, no callers, you are in the small blind", "small_blind", "facing_open_late", face(BTN | SB, 0, 0), SB, FACE, 1),
    chart!("vs_late.bb", "Open from the button or small blind, no callers, you are in the big blind", "big_blind", "facing_open_late", face(BTN | SB, 0, 0), BB, FACE, 1),
    chart!("mw_em.ip", "Open from early or middle with at least one caller, you are in position", "in_position", "facing_open_callers_em", face(E | M, 1, 6), IP_SEATS, FACE, 1),
    chart!("mw_em.sb", "Open from early or middle with at least one caller, you are in the small blind", "small_blind", "facing_open_callers_em", face(E | M, 1, 6), SB, FACE, 1),
    chart!("mw_em.bb", "Open from early or middle with at least one caller, you are in the big blind", "big_blind", "facing_open_callers_em", face(E | M, 1, 6), BB, FACE, 1),
    chart!("mw_late.sb", "Open from the cutoff or button with at least one caller, you are in the small blind", "small_blind", "facing_open_callers_late", face(CO | BTN, 1, 6), SB, FACE, 1),
    chart!("mw_late.bb", "Open from the cutoff or button with at least one caller, you are in the big blind", "big_blind", "facing_open_callers_late", face(CO | BTN | SB, 1, 6), BB, FACE, 1),
    // Stage 2: first in, and limped pots.
    chart!("open.early", "Folded to you, early position", "early", "folded_to", Sit::FoldedTo, E, OPEN, 2),
    chart!("open.middle", "Folded to you, middle position", "middle", "folded_to", Sit::FoldedTo, M, OPEN, 2),
    chart!("open.cutoff", "Folded to you, cutoff", "cutoff", "folded_to", Sit::FoldedTo, CO, OPEN, 2),
    chart!("open.button", "Folded to you, button", "button", "folded_to", Sit::FoldedTo, BTN, OPEN, 2),
    chart!("open.small_blind", "Folded to you, small blind", "small_blind", "folded_to", Sit::FoldedTo, SB, OPEN, 2),
    chart!("limp1.middle", "One limper ahead, middle position", "middle", "limped_1", Sit::Limped { lo: 1, hi: 1 }, M, LIMP, 2),
    chart!("limp1.cutoff", "One limper ahead, cutoff", "cutoff", "limped_1", Sit::Limped { lo: 1, hi: 1 }, CO, LIMP, 2),
    chart!("limp1.button", "One limper ahead, button", "button", "limped_1", Sit::Limped { lo: 1, hi: 1 }, BTN, LIMP, 2),
    chart!("limp1.small_blind", "One limper ahead, small blind", "small_blind", "limped_1", Sit::Limped { lo: 1, hi: 1 }, SB, LIMP, 2),
    chart!("limp1.big_blind", "One limper, you are in the big blind", "big_blind", "limped_1", Sit::Limped { lo: 1, hi: 1 }, BB, BBLIMP, 2),
    chart!("limp2.middle", "Two or more limpers ahead, middle position", "middle", "limped_2plus", Sit::Limped { lo: 2, hi: 6 }, M, LIMP, 2),
    chart!("limp2.cutoff", "Two or more limpers ahead, cutoff", "cutoff", "limped_2plus", Sit::Limped { lo: 2, hi: 6 }, CO, LIMP, 2),
    chart!("limp2.button", "Two or more limpers ahead, button", "button", "limped_2plus", Sit::Limped { lo: 2, hi: 6 }, BTN, LIMP, 2),
    chart!("limp2.small_blind", "Two or more limpers ahead, small blind", "small_blind", "limped_2plus", Sit::Limped { lo: 2, hi: 6 }, SB, LIMP, 2),
    chart!("limp2.big_blind", "Two or more limpers, you are in the big blind", "big_blind", "limped_2plus", Sit::Limped { lo: 2, hi: 6 }, BB, BBLIMP, 2),
];

pub const VS3BET_IP: usize = 0;
pub const VS3BET_OOP: usize = 1;
pub const VS_EM_IP: usize = 2;
pub const VS_EM_SB: usize = 3;
pub const VS_EM_BB: usize = 4;
pub const VS_CO_IP: usize = 5;
pub const VS_CO_SB: usize = 6;
pub const VS_CO_BB: usize = 7;
pub const VS_LATE_SB: usize = 8;
pub const VS_LATE_BB: usize = 9;
pub const MW_EM_IP: usize = 10;
pub const MW_EM_SB: usize = 11;
pub const MW_EM_BB: usize = 12;
pub const MW_LATE_SB: usize = 13;
pub const MW_LATE_BB: usize = 14;
pub const OPEN_E: usize = 15;
pub const OPEN_M: usize = 16;
pub const OPEN_CO: usize = 17;
pub const OPEN_BTN: usize = 18;
pub const OPEN_SB: usize = 19;
pub const LIMP1_M: usize = 20;
pub const LIMP1_BB: usize = 24;
pub const LIMP2_M: usize = 25;
pub const LIMP2_BB: usize = 29;

/// Combos in each class, as a fixed list.
fn combo_lists() -> &'static [Vec<u16>] {
    static L: OnceLock<Vec<Vec<u16>>> = OnceLock::new();
    L.get_or_init(|| {
        let mut v = vec![Vec::new(); NCLASSES];
        for (i, &c) in combos().class169.iter().enumerate() {
            v[c as usize].push(i as u16);
        }
        v
    })
}

pub fn mix(a: u64, b: u64) -> u64 {
    let mut z = a ^ b.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

// ---------------------------------------------------------------- pools

/// Shared per-batch work: runouts with class and rank tables, and models.
pub struct Pool {
    pub cards: Vec<[Card; 5]>,
    pub masks: Vec<u64>,
    /// Flat: `class[b * 3 * NCOMBOS + street * NCOMBOS + combo]`, street 0..3.
    pub class: Vec<u8>,
    /// Flat: `rank[b * NCOMBOS + combo]`, dense river rank, higher is better.
    pub rank: Vec<u16>,
    pub models: Vec<PlayerModel>,
    pub tables: Vec<VillainTables>,
    /// Recalibrated villain behaviour for the charts, or the plain population.
    pub cal: Option<&'static Calibration>,
}

fn build_board(seed: u64) -> ([Card; 5], u64, Vec<u8>, Vec<u16>) {
    let mut rng = Rng::new(seed);
    let mut mask = 0u64;
    let mut cards = [Card::from_index(0); 5];
    let mut i = 0;
    while i < 5 {
        let c = Card::from_index(rng.below(52) as u8);
        if mask & c.mask() == 0 {
            mask |= c.mask();
            cards[i] = c;
            i += 1;
        }
    }
    let table = combos();
    let mut class = Vec::with_capacity(3 * NCOMBOS);
    class.extend_from_slice(&classify(&cards[..3], true));
    class.extend_from_slice(&classify(&cards[..4], true));
    class.extend_from_slice(&classify(&cards[..5], false));
    let mut scored: Vec<(u32, u16)> = Vec::with_capacity(NCOMBOS);
    let mut order: Vec<(poker_trainer::eval::HandRank, u16)> = Vec::with_capacity(NCOMBOS);
    let mut seven = [cards[0], cards[1], cards[2], cards[3], cards[4], cards[0], cards[0]];
    for (j, &(a, b)) in table.cards.iter().enumerate() {
        if table.masks[j] & mask != 0 {
            continue;
        }
        seven[5] = a;
        seven[6] = b;
        order.push((evaluate(&seven), j as u16));
    }
    order.sort_unstable_by_key(|x| x.0);
    let mut dense = 0u32;
    for k in 0..order.len() {
        if k > 0 && order[k].0 != order[k - 1].0 {
            dense += 1;
        }
        scored.push((dense + 1, order[k].1));
    }
    let mut rank = vec![0u16; NCOMBOS];
    for &(d, c) in &scored {
        rank[c as usize] = d as u16;
    }
    (cards, mask, class, rank)
}

impl Pool {
    /// Builds a pool in parallel; the result depends only on the seed, not the
    /// thread count, because each element has its own seed.
    pub fn build(seed: u64, nboards: usize, nmodels: usize, threads: usize) -> Pool {
        Pool::build_cal(seed, nboards, nmodels, threads, None)
    }

    /// `build` with villains recalibrated by `cal`, or the population if `None`.
    pub fn build_cal(seed: u64, nboards: usize, nmodels: usize, threads: usize, cal: Option<&'static Calibration>) -> Pool {
        let board_next = AtomicUsize::new(0);
        let boards: Mutex<Vec<(usize, ([Card; 5], u64, Vec<u8>, Vec<u16>))>> = Mutex::new(Vec::with_capacity(nboards));
        let model_next = AtomicUsize::new(0);
        let model_out: Mutex<Vec<(usize, PlayerModel, VillainTables)>> = Mutex::new(Vec::with_capacity(nmodels));
        std::thread::scope(|sc| {
            for _ in 0..threads {
                sc.spawn(|| {
                    loop {
                        let i = board_next.fetch_add(1, Ordering::SeqCst);
                        if i >= nboards {
                            break;
                        }
                        let v = build_board(mix(seed ^ 0xB0A7D, i as u64));
                        boards.lock().unwrap().push((i, v));
                    }
                    loop {
                        let i = model_next.fetch_add(1, Ordering::SeqCst);
                        if i >= nmodels {
                            break;
                        }
                        let mut rng = Rng::new(mix(seed ^ 0x30DE1, i as u64));
                        let mut m = PlayerModel::sample(&mut rng);
                        if let Some(c) = cal {
                            c.apply_postflop(&mut m);
                        }
                        let t = m.tables();
                        model_out.lock().unwrap().push((i, m, t));
                    }
                });
            }
        });
        let mut boards = boards.into_inner().unwrap();
        boards.sort_by_key(|x| x.0);
        let mut mo = model_out.into_inner().unwrap();
        mo.sort_by_key(|x| x.0);
        let mut pool = Pool {
            cards: Vec::with_capacity(nboards),
            masks: Vec::with_capacity(nboards),
            class: Vec::with_capacity(nboards * 3 * NCOMBOS),
            rank: Vec::with_capacity(nboards * NCOMBOS),
            models: Vec::with_capacity(nmodels),
            tables: Vec::with_capacity(nmodels),
            cal,
        };
        for (_, (c, m, cl, rk)) in boards {
            pool.cards.push(c);
            pool.masks.push(m);
            pool.class.extend_from_slice(&cl);
            pool.rank.extend_from_slice(&rk);
        }
        for (_, m, t) in mo {
            pool.models.push(m);
            pool.tables.push(t);
        }
        pool
    }

    #[inline]
    pub fn class_at(&self, b: usize, street: usize, combo: usize) -> usize {
        self.class[b * 3 * NCOMBOS + street * NCOMBOS + combo] as usize
    }

    #[inline]
    fn rank_at(&self, b: usize, combo: usize) -> u16 {
        self.rank[b * NCOMBOS + combo]
    }
}

// ---------------------------------------------------------------- deals

/// Weighted list of (table size, hero seat) for a chart's seat mask.
pub struct SeatTable {
    items: Vec<(usize, usize, u64)>,
    total: u64,
}

impl SeatTable {
    pub fn for_mask(hero: u8) -> SeatTable {
        let mut items = Vec::new();
        for &(n, w) in TABLE_SIZES.iter() {
            for seat in 0..n {
                if hero >> (position_of(n, seat) as usize) & 1 == 1 {
                    // P(n) * P(seat | n): uniform over seats, so weight w * (420 / n)
                    // keeps integers (420 is divisible by 4, 5, 6 and 7).
                    items.push((n, seat, w * (420 / n as u64)));
                }
            }
        }
        assert!(!items.is_empty(), "no seats for mask");
        let total = items.iter().map(|x| x.2).sum();
        SeatTable { items, total }
    }

    fn pick(&self, rng: &mut Rng) -> (usize, usize) {
        let mut u = rng.below(self.total);
        for &(n, s, w) in &self.items {
            if u < w {
                return (n, s);
            }
            u -= w;
        }
        let last = self.items.last().unwrap();
        (last.0, last.1)
    }
}

pub const NO_SEAT: i8 = -1;

/// Preflop facts carried along a hand beyond `Round` and `PreflopTrack`.
#[derive(Clone, Copy, Debug)]
pub struct PfState {
    /// Seat of the last raiser, or `NO_SEAT`.
    pub raiser: i8,
    /// Villains who called the current raise before hero acted on it.
    pub callers: u8,
    pub hero_raised: bool,
    pub hero_limped: bool,
}

/// Kind code in `Deal::hist`: 0 fold, 1 check, 2 call, 3 raise, 4 hero open.
#[derive(Clone, Copy, Debug, Default)]
pub struct HistEntry {
    pub seat: u8,
    pub kind: u8,
}

/// One table, conditioned on the betting before hero's decision.
#[derive(Clone, Debug)]
pub struct Deal {
    pub n: usize,
    pub hero: usize,
    pub board: usize,
    /// Combo index by seat (hero's included).
    pub combos: [u16; 7],
    /// Model pool index by seat (hero's unused).
    pub models: [u32; 7],
    pub r: Round,
    pub track: PreflopTrack,
    pub pf: PfState,
    pub hist: [HistEntry; 16],
    pub hist_len: usize,
}

#[inline]
fn deal_cards(rng: &mut Rng, used: &mut u64) -> u16 {
    loop {
        let a = rng.below(52) as u8;
        let ma = 1u64 << a;
        if *used & ma != 0 {
            continue;
        }
        let b = rng.below(52) as u8;
        let mb = 1u64 << b;
        if *used & mb != 0 || a == b {
            continue;
        }
        *used |= ma | mb;
        return combos().index[a as usize][b as usize];
    }
}

/// The action index an existing row entry maps to, as in `sim::sample_col`.
#[inline]
fn pick_col(u: f64, p: [f32; 3]) -> usize {
    let u = u * f64::from(p[0] + p[1] + p[2]);
    if u < f64::from(p[0]) {
        0
    } else if u < f64::from(p[0] + p[1]) {
        1
    } else {
        2
    }
}

/// Whether this seat has already called voluntarily preflop without raising.
#[inline]
fn limped(r: &Round, seat: usize) -> bool {
    r.raises >= 1 && if seat == 1 { false } else if seat == 0 { r.put[0] > 0.5 + 1e-9 && r.put[0] < r.current } else { r.put[seat] > 0.0 && r.put[seat] < r.current }
}

/// Villain `[fold, call, raise]` for one class under `cal`, or the population.
#[inline]
pub fn villain_probs(model: &PlayerModel, ctx: &crate::models::PreflopCtx, class: usize, cal: Option<&'static Calibration>, limped: bool) -> [f32; 3] {
    match cal.and_then(|c| c.thresholds(model, ctx, limped)) {
        Some((raise_th, cont_th)) => model.preflop_probs_at(ctx, class, raise_th, cont_th),
        None => model.preflop_probs(ctx, class),
    }
}

/// Mirrors `sim::villain_preflop` for one class. A unit test checks the
/// distribution against `PlayerModel::preflop_row`.
#[inline]
fn villain_preflop_fast(r: &Round, track: &PreflopTrack, n: usize, seat: usize, class: usize, model: &PlayerModel, cal: Option<&'static Calibration>, u: f64) -> Act {
    let ctx = track.ctx(r, seat, n);
    let p = villain_probs(model, &ctx, class, cal, limped(r, seat));
    let col = pick_col(u, p);
    let facing = r.is_facing(seat);
    match col {
        0 if facing => Act::Fold,
        0 => Act::Check,
        1 => {
            if facing {
                Act::Call
            } else {
                Act::Check
            }
        }
        _ => {
            if preflop_can_raise(r, seat) {
                let to = clamp_preflop_raise(r, seat, model.preflop_raise_to(r));
                Act::Raise(to.min(r.stacks[seat] + r.put[seat]))
            } else if facing {
                Act::Call
            } else {
                Act::Check
            }
        }
    }
}

/// Whether a villain's action before hero fits the situation. `phase` is
/// 0 while hero has not acted, 1 after his open in `Opened3bet`.
#[inline]
fn allowed(sit: Sit, act: Act, r: &Round, pf: &PfState, seat: usize, n: usize, phase: u8) -> bool {
    match (sit, act) {
        (Sit::FoldedTo, Act::Fold) => true,
        (Sit::FoldedTo, _) => false,
        (Sit::Limped { .. }, Act::Fold | Act::Call) => true,
        (Sit::Limped { .. }, _) => false,
        (Sit::FacingOpen { .. }, Act::Fold) => true,
        (Sit::FacingOpen { .. }, Act::Call) => pf.raiser != NO_SEAT,
        (Sit::FacingOpen { opener, .. }, Act::Raise(_)) => r.raises == 0 && opener >> (position_of(n, seat) as usize) & 1 == 1,
        (Sit::FacingOpen { .. }, _) => false,
        (Sit::Opened3bet { .. }, Act::Fold) => true,
        (Sit::Opened3bet { .. }, Act::Raise(_)) => phase == 1 && r.raises == 1,
        (Sit::Opened3bet { .. }, _) => false,
    }
}

/// A table conditioned on `ch`'s situation, hero holding class `class`.
/// Cards and models are drawn lazily so a rejected draw stops at the first
/// seat whose action does not fit. Every seat's model and cards are exchangeable
/// and sampling restarts from scratch on rejection, so this is exact.
pub fn sample_deal(ch: &ChartDef, seats: &SeatTable, class: usize, pool: &Pool, rng: &mut Rng, attempts: &mut u64) -> Deal {
    let list = &combo_lists()[class];
    let nb = pool.masks.len() as u64;
    let nm = pool.models.len() as u64;
    let table = combos();
    loop {
        *attempts += 1;
        let (n, hero) = seats.pick(rng);
        let hc = list[rng.below(list.len() as u64) as usize] as usize;
        let hero_mask = table.masks[hc];
        let b = loop {
            let i = rng.below(nb) as usize;
            if pool.masks[i] & hero_mask == 0 {
                break i;
            }
        };
        let mut used = pool.masks[b] | hero_mask;
        let mut d = Deal {
            n,
            hero,
            board: b,
            combos: [0; 7],
            models: [0; 7],
            r: Round::preflop(n, STACK),
            track: PreflopTrack::default(),
            pf: PfState { raiser: NO_SEAT, callers: 0, hero_raised: false, hero_limped: false },
            hist: [HistEntry::default(); 16],
            hist_len: 0,
        };
        d.combos[hero] = hc as u16;
        let mut dealt = 1u8 << hero;
        let mut phase = 0u8;
        let mut ok = false;
        while d.r.to_act != 0 {
            let seat = d.r.next as usize;
            if seat == hero {
                match ch.sit {
                    Sit::Opened3bet { ip } => {
                        if phase == 0 {
                            if d.r.raises != 0 || d.track.limpers != 0 {
                                break;
                            }
                            phase = 1;
                            let to = clamp_preflop_raise(&d.r, seat, hero_preflop_raise(&d.r));
                            let act = Act::Raise(to.min(d.r.stacks[seat] + d.r.put[seat]));
                            d.track.note(&d.r, seat, act);
                            d.r.apply(seat, act);
                            d.pf.raiser = seat as i8;
                            d.pf.hero_raised = true;
                            d.hist[d.hist_len] = HistEntry { seat: seat as u8, kind: 4 };
                            d.hist_len += 1;
                            continue;
                        }
                        if d.pf.raiser == hero as i8 || d.r.raises != 2 {
                            break;
                        }
                        ok = (hero > d.pf.raiser as usize) == ip;
                        break;
                    }
                    Sit::FoldedTo => {
                        ok = d.r.raises == 0 && d.track.limpers == 0;
                        break;
                    }
                    Sit::Limped { lo, hi } => {
                        ok = d.r.raises == 0 && d.track.limpers >= lo && d.track.limpers <= hi;
                        break;
                    }
                    Sit::FacingOpen { lo, hi, .. } => {
                        ok = d.pf.raiser != NO_SEAT && d.r.raises == 1 && d.pf.callers >= lo && d.pf.callers <= hi;
                        break;
                    }
                }
            }
            if dealt >> seat & 1 == 0 {
                dealt |= 1 << seat;
                d.combos[seat] = deal_cards(rng, &mut used);
                d.models[seat] = rng.below(nm) as u32;
            }
            let c = table.class169[d.combos[seat] as usize] as usize;
            let act = villain_preflop_fast(&d.r, &d.track, n, seat, c, &pool.models[d.models[seat] as usize], pool.cal, unit(rng));
            if !allowed(ch.sit, act, &d.r, &d.pf, seat, n, phase) {
                break;
            }
            d.hist[d.hist_len] = HistEntry { seat: seat as u8, kind: kind_of(act) };
            d.hist_len += 1;
            match act {
                Act::Raise(_) => d.pf.raiser = seat as i8,
                Act::Call if d.pf.raiser != NO_SEAT => d.pf.callers += 1,
                _ => {}
            }
            d.track.note(&d.r, seat, act);
            d.r.apply(seat, act);
        }
        if !ok {
            continue;
        }
        for seat in 0..n {
            if dealt >> seat & 1 == 0 {
                d.combos[seat] = deal_cards(rng, &mut used);
                d.models[seat] = rng.below(nm) as u32;
            }
        }
        return d;
    }
}

fn kind_of(a: Act) -> u8 {
    match a {
        Act::Fold => 0,
        Act::Check => 1,
        Act::Call => 2,
        _ => 3,
    }
}

/// An unconditioned table for the head-to-head: hero's seat and cards uniform.
pub fn sample_full_deal(pool: &Pool, rng: &mut Rng) -> Deal {
    let table = combos();
    let n = pick_table_size(rng);
    let hero = rng.below(n as u64) as usize;
    let nb = pool.masks.len() as u64;
    let nm = pool.models.len() as u64;
    let b = rng.below(nb) as usize;
    let mut used = pool.masks[b];
    let hc = deal_cards(rng, &mut used);
    let mut d = Deal {
        n,
        hero,
        board: b,
        combos: [0; 7],
        models: [0; 7],
        r: Round::preflop(n, STACK),
        track: PreflopTrack::default(),
        pf: PfState { raiser: NO_SEAT, callers: 0, hero_raised: false, hero_limped: false },
        hist: [HistEntry::default(); 16],
        hist_len: 0,
    };
    d.combos[hero] = hc;
    let _ = table;
    for seat in 0..n {
        if seat != hero {
            d.combos[seat] = deal_cards(rng, &mut used);
            d.models[seat] = rng.below(nm) as u32;
        }
    }
    d
}

fn pick_table_size(rng: &mut Rng) -> usize {
    let total: u64 = TABLE_SIZES.iter().map(|x| x.1).sum();
    let mut u = rng.below(total);
    for &(n, w) in TABLE_SIZES.iter() {
        if u < w {
            return n;
        }
        u -= w;
    }
    TABLE_SIZES.last().unwrap().0
}

#[inline]
fn unit(rng: &mut Rng) -> f64 {
    (rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64
}

// ---------------------------------------------------------------- policies

/// Hero's preflop policy at any decision that is not the chart's own root.
pub enum HeroPolicy<'a> {
    /// Charts, indexed like `CHARTS`. Absent charts use the deep rule.
    Charts(&'a [Option<[Pure; NCLASSES]>]),
    /// The hand-tuned card, through the simulator's own card logic.
    Card,
}

/// Fixed rule where no chart applies (facing a four-bet, or cold-called and
/// then re-raised): raise AA and KK if allowed, call QQ and AK, fold the rest.
/// An assumption, deliberately simple and tight; see PREFLOP.md.
pub fn deep_rule(class: usize) -> Pure {
    let ace = 12usize;
    let king = 11usize;
    let tri = ace * (ace - 1) / 2 + king;
    let aks = 13 + tri;
    let ako = 91 + tri;
    match class {
        c if c == ace || c == king => Pure::Raise,
        c if c == 10 || c == aks || c == ako => Pure::Passive,
        _ => Pure::Fold,
    }
}

/// The chart hero's decision falls under, if one exists.
pub fn chart_index(r: &Round, track: &PreflopTrack, pf: &PfState, n: usize, hero: usize) -> Option<usize> {
    let pos = position_of(n, hero);
    let bb_like = pos == Seat::BigBlind;
    let sb_like = pos == Seat::SmallBlind;
    match r.raises {
        0 => {
            if track.limpers == 0 {
                return match pos {
                    Seat::Early => Some(OPEN_E),
                    Seat::Middle => Some(OPEN_M),
                    Seat::Cutoff => Some(OPEN_CO),
                    Seat::Button => Some(OPEN_BTN),
                    Seat::SmallBlind => Some(OPEN_SB),
                    Seat::BigBlind => None,
                };
            }
            let two = track.limpers >= 2;
            let base = if two { LIMP2_M } else { LIMP1_M };
            Some(match pos {
                Seat::Early | Seat::Middle => base,
                Seat::Cutoff => base + 1,
                Seat::Button => base + 2,
                Seat::SmallBlind => base + 3,
                Seat::BigBlind => base + 4,
            })
        }
        1 => {
            if pf.hero_raised {
                return None;
            }
            let opener = position_of(n, pf.raiser.max(0) as usize);
            let others = pf.callers + track.limpers.saturating_sub(u8::from(pf.hero_limped));
            let em = matches!(opener, Seat::Early | Seat::Middle);
            let co = opener == Seat::Cutoff;
            if others == 0 {
                Some(if bb_like {
                    if em { VS_EM_BB } else if co { VS_CO_BB } else { VS_LATE_BB }
                } else if sb_like {
                    if em { VS_EM_SB } else if co { VS_CO_SB } else { VS_LATE_SB }
                } else if em {
                    VS_EM_IP
                } else {
                    VS_CO_IP
                })
            } else {
                Some(if bb_like {
                    if em { MW_EM_BB } else { MW_LATE_BB }
                } else if sb_like {
                    if em { MW_EM_SB } else { MW_LATE_SB }
                } else {
                    MW_EM_IP
                })
            }
        }
        2 if pf.hero_raised => Some(if hero > pf.raiser.max(0) as usize { VS3BET_IP } else { VS3BET_OOP }),
        _ => None,
    }
}

fn pure_to_act(p: Pure, r: &Round, seat: usize) -> Act {
    let facing = r.is_facing(seat);
    let passive = if facing { Act::Call } else { Act::Check };
    match p {
        Pure::Fold => {
            if facing {
                Act::Fold
            } else {
                Act::Check
            }
        }
        Pure::Passive => passive,
        Pure::Raise => {
            if preflop_can_raise(r, seat) && r.raises < 3 {
                let to = clamp_preflop_raise(r, seat, hero_preflop_raise(r));
                Act::Raise(to.min(r.stacks[seat] + r.put[seat]))
            } else {
                passive
            }
        }
    }
}

fn hero_policy_act(pol: &HeroPolicy, r: &Round, track: &PreflopTrack, pf: &PfState, n: usize, seat: usize, class: usize, hand: Holding) -> Act {
    match pol {
        HeroPolicy::Card => hero_preflop(r, track, n, seat, hand),
        HeroPolicy::Charts(charts) => {
            let p = match chart_index(r, track, pf, n, seat) {
                Some(i) => charts[i].map_or_else(|| deep_rule(class), |c| c[class]),
                None => {
                    if !r.is_facing(seat) {
                        Pure::Passive
                    } else {
                        deep_rule(class)
                    }
                }
            };
            pure_to_act(p, r, seat)
        }
    }
}

// ---------------------------------------------------------------- play

#[inline]
fn hole(combo: u16) -> Holding {
    let (a, b) = combos().cards[combo as usize];
    [a, b]
}

/// Chips hero ends the hand with, minus chips he started with: blinds and
/// other money already invested count, so fold = -(money in) at every node.
#[inline]
fn net_of(r: &Round, hero: usize) -> f64 {
    r.stacks[hero] - STACK
}

/// Plays the hand from `d` to the end. `first` is hero's action at the deal's
/// decision node, or `None` to use `pol` there too (the head-to-head).
///
/// Random numbers are keyed by trial seed, seat and that seat's decision count,
/// so the same villain decision uses the same number across branches.
pub fn play(pool: &Pool, d: &Deal, first: Option<Pure>, pol: &HeroPolicy, trial_seed: u64) -> f64 {
    let n = d.n;
    let hero = d.hero;
    let table = combos();
    let hero_combo = d.combos[hero] as usize;
    let hero_class = table.class169[hero_combo] as usize;
    let hero_hole = hole(d.combos[hero]);
    let mut r = d.r;
    let mut track = d.track;
    let mut pf = d.pf;
    let mut k = [0u64; 7];
    let mut first = first;
    let next_u = |seat: usize, k: &mut [u64; 7]| -> u64 {
        let v = mix(trial_seed ^ ((seat as u64 + 1) << 56), k[seat]);
        k[seat] += 1;
        v
    };
    while r.to_act != 0 {
        let seat = r.next as usize;
        let act = if seat == hero {
            match first.take() {
                Some(p) => pure_to_act(p, &r, seat),
                None => hero_policy_act(pol, &r, &track, &pf, n, seat, hero_class, hero_hole),
            }
        } else {
            let c = table.class169[d.combos[seat] as usize] as usize;
            let u = (next_u(seat, &mut k) >> 11) as f64 / (1u64 << 53) as f64;
            villain_preflop_fast(&r, &track, n, seat, c, &pool.models[d.models[seat] as usize], pool.cal, u)
        };
        match act {
            Act::Raise(_) => {
                pf.raiser = seat as i8;
                pf.callers = 0;
                if seat == hero {
                    pf.hero_raised = true;
                }
            }
            Act::Call => {
                if seat == hero && r.raises == 0 {
                    pf.hero_limped = true;
                } else if seat != hero && r.raises == 1 {
                    pf.callers += 1;
                }
            }
            _ => {}
        }
        track.note(&r, seat, act);
        r.apply(seat, act);
        if r.in_hand >> hero & 1 == 0 {
            return net_of(&r, hero);
        }
    }
    if r.in_hand.count_ones() == 1 {
        return net_of(&r, hero) + r.pot;
    }
    for street_idx in 1..=3usize {
        r = r.next_street();
        let st = street_idx - 1;
        while r.to_act != 0 {
            let seat = r.next as usize;
            let mut rng = Rng::new(next_u(seat, &mut k));
            let act = if seat == hero {
                let c = pool.class_at(d.board, st, hero_combo);
                hero_heuristic(&r, seat, c % NBUCKET, c >= NBUCKET, st, &mut rng)
            } else {
                let c = pool.class_at(d.board, st, d.combos[seat] as usize);
                villain_postflop(&r, seat, c, st, &pool.tables[d.models[seat] as usize], &mut rng)
            };
            r.apply(seat, act);
            if r.in_hand >> hero & 1 == 0 {
                return net_of(&r, hero);
            }
            if r.in_hand.count_ones() == 1 {
                return net_of(&r, hero) + r.pot;
            }
        }
    }
    // Showdown. Equal stacks, so one pot and no side pots.
    let mut best = 0u16;
    let mut ties = 0u32;
    let hero_rank = pool.rank_at(d.board, hero_combo);
    for s in 0..n {
        if r.in_hand >> s & 1 == 1 {
            let v = pool.rank_at(d.board, d.combos[s] as usize);
            if v > best {
                best = v;
                ties = 1;
            } else if v == best {
                ties += 1;
            }
        }
    }
    if hero_rank == best {
        net_of(&r, hero) + r.pot / f64::from(ties)
    } else {
        net_of(&r, hero)
    }
}

// ---------------------------------------------------------------- derivation

#[derive(Clone)]
pub struct Cell {
    /// Mean net per action, per completed batch.
    pub per_batch: Vec<[f64; 3]>,
    pub attempts: u64,
    pub trials: u64,
    pub stopped: bool,
}

impl Cell {
    fn new() -> Cell {
        Cell { per_batch: Vec::new(), attempts: 0, trials: 0, stopped: false }
    }

    pub fn means(&self) -> [f64; 3] {
        let mut m = [0.0; 3];
        for b in &self.per_batch {
            for a in 0..3 {
                m[a] += b[a];
            }
        }
        for v in m.iter_mut() {
            *v /= self.per_batch.len().max(1) as f64;
        }
        m
    }

    /// Standard error of mean(a - b) from batch means.
    pub fn se_diff(&self, a: usize, b: usize) -> f64 {
        let n = self.per_batch.len();
        if n < 2 {
            return f64::INFINITY;
        }
        let d: Vec<f64> = self.per_batch.iter().map(|x| x[a] - x[b]).collect();
        let m = d.iter().sum::<f64>() / n as f64;
        let v = d.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / (n - 1) as f64;
        (v / n as f64).sqrt()
    }

    pub fn se_of(&self, a: usize) -> f64 {
        let n = self.per_batch.len();
        if n < 2 {
            return f64::INFINITY;
        }
        let d: Vec<f64> = self.per_batch.iter().map(|x| x[a]).collect();
        let m = d.iter().sum::<f64>() / n as f64;
        let v = d.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / (n - 1) as f64;
        (v / n as f64).sqrt()
    }
}

/// Which actions a chart allows, by `Pure as usize`.
pub fn allowed_actions(ch: &ChartDef) -> [bool; 3] {
    [ch.labels[0].is_some(), ch.labels[1].is_some(), ch.labels[2].is_some()]
}

pub fn best_of(means: &[f64; 3], avail: [bool; 3]) -> usize {
    let mut b = usize::MAX;
    for a in 0..3 {
        if avail[a] && (b == usize::MAX || means[a] > means[b]) {
            b = a;
        }
    }
    b
}

/// The margin rule: the simplest action within `margin` of the best.
/// Simplest is fold, then passive, then raise, which is also `Pure`'s order.
pub fn choose(means: &[f64; 3], avail: [bool; 3], margin: f64) -> Pure {
    let best = best_of(means, avail);
    for a in 0..3 {
        if avail[a] && means[best] - means[a] <= margin {
            return PURES[a];
        }
    }
    PURES[best]
}

/// Early-stop test: whether the margin choice looks settled. Every other
/// action must be clearly inside or clearly outside the margin band around the best.
fn settled(c: &Cell, avail: [bool; 3], margin: f64, min_batches: usize) -> bool {
    if c.per_batch.len() < min_batches {
        return false;
    }
    let m = c.means();
    let best = best_of(&m, avail);
    for a in 0..3 {
        if avail[a] && a != best {
            let gap = m[best] - m[a];
            let se = c.se_diff(best, a);
            if (gap - margin).abs() <= STOP_Z * se {
                return false;
            }
        }
    }
    true
}

#[derive(Clone, Debug)]
pub struct Config {
    pub seed: u64,
    pub threads: usize,
    pub trials_per_batch: u64,
    pub min_batches: usize,
    pub max_batches: usize,
    pub margin: f64,
    pub nboards: usize,
    pub nmodels: usize,
    pub cal: Option<&'static Calibration>,
}

pub struct Derivation {
    pub cells: Vec<Vec<Cell>>,
    /// Choice before monotonicity, per chart and class.
    pub raw: Vec<[Pure; NCLASSES]>,
    /// Final pure chart.
    pub pure: Vec<[Pure; NCLASSES]>,
    pub changed_by_mono: Vec<usize>,
    pub trial_total: u64,
    pub elapsed: f64,
    pub job_seconds: f64,
}

fn job_seed(cfg_seed: u64, stage: u8, batch: usize, chart: usize, class: usize) -> u64 {
    mix(mix(mix(mix(cfg_seed, stage as u64 + 1), batch as u64 + 7), chart as u64), class as u64)
}

/// Runs one (chart, class) for one batch: mean net per action, sums and tallies.
pub fn run_job(ch: &ChartDef, seats: &SeatTable, class: usize, seed: u64, trials: u64, pool: &Pool, pol: &HeroPolicy) -> ([f64; 3], u64) {
    let avail = allowed_actions(ch);
    let mut sums = [0.0f64; 3];
    let mut attempts = 0u64;
    for t in 0..trials {
        let ts = mix(seed, t);
        let mut rng = Rng::new(ts);
        let d = sample_deal(ch, seats, class, pool, &mut rng, &mut attempts);
        for a in 0..3 {
            if avail[a] {
                sums[a] += play(pool, &d, Some(PURES[a]), pol, ts);
            }
        }
    }
    for v in sums.iter_mut() {
        *v /= trials as f64;
    }
    (sums, attempts)
}

/// Derives all charts stage by stage. Later stages use the final charts of
/// earlier stages as hero's continuation policy.
pub fn derive(cfg: &Config) -> Derivation {
    let start = std::time::Instant::now();
    let tseats: Vec<SeatTable> = CHARTS.iter().map(|c| SeatTable::for_mask(c.hero)).collect();
    let mut cells: Vec<Vec<Cell>> = CHARTS.iter().map(|_| (0..NCLASSES).map(|_| Cell::new()).collect()).collect();
    let mut policy: Vec<Option<[Pure; NCLASSES]>> = vec![None; CHARTS.len()];
    let mut raw: Vec<[Pure; NCLASSES]> = vec![[Pure::Fold; NCLASSES]; CHARTS.len()];
    let mut pure: Vec<[Pure; NCLASSES]> = vec![[Pure::Fold; NCLASSES]; CHARTS.len()];
    let mut changed = vec![0usize; CHARTS.len()];
    let mut trial_total = 0u64;
    let mut job_seconds = 0.0f64;
    for stage in 0..3u8 {
        let in_stage: Vec<usize> = (0..CHARTS.len()).filter(|&i| CHARTS[i].stage == stage).collect();
        for batch in 0..cfg.max_batches {
            let active: Vec<(usize, usize)> = in_stage
                .iter()
                .flat_map(|&c| (0..NCLASSES).map(move |h| (c, h)))
                .filter(|&(c, h)| !cells[c][h].stopped)
                .collect();
            if active.is_empty() {
                break;
            }
            let pool_seed = mix(cfg.seed, 0x5000 + (u64::from(stage) << 32) + batch as u64);
            let pool = Pool::build_cal(pool_seed, cfg.nboards, cfg.nmodels, cfg.threads, cfg.cal);
            let next = AtomicUsize::new(0);
            let out: Mutex<Vec<(usize, [f64; 3], u64)>> = Mutex::new(Vec::with_capacity(active.len()));
            let t0 = std::time::Instant::now();
            let policy_ref: &[Option<[Pure; NCLASSES]>] = &policy;
            std::thread::scope(|sc| {
                for _ in 0..cfg.threads {
                    sc.spawn(|| {
                        let pol = HeroPolicy::Charts(policy_ref);
                        loop {
                            let i = next.fetch_add(1, Ordering::Relaxed);
                            if i >= active.len() {
                                break;
                            }
                            let (c, h) = active[i];
                            let seed = job_seed(cfg.seed, stage, batch, c, h);
                            let (m, att) = run_job(&CHARTS[c], &tseats[c], h, seed, cfg.trials_per_batch, &pool, &pol);
                            out.lock().unwrap().push((i, m, att));
                        }
                    });
                }
            });
            job_seconds += t0.elapsed().as_secs_f64() * cfg.threads as f64;
            let mut results = out.into_inner().unwrap();
            results.sort_by_key(|x| x.0);
            for (i, m, att) in results {
                let (c, h) = active[i];
                let cell = &mut cells[c][h];
                cell.per_batch.push(m);
                cell.attempts += att;
                cell.trials += cfg.trials_per_batch;
                trial_total += cfg.trials_per_batch;
            }
            let mut stopped_now = 0usize;
            for &(c, h) in &active {
                let avail = allowed_actions(&CHARTS[c]);
                if settled(&cells[c][h], avail, cfg.margin, cfg.min_batches) {
                    cells[c][h].stopped = true;
                    stopped_now += 1;
                }
            }
            eprintln!("stage {stage} batch {batch}: {} active jobs, {stopped_now} settled, elapsed {:.0}s", active.len(), start.elapsed().as_secs_f64());
        }
        // Finalize this stage's charts so later stages can use them.
        for &c in &in_stage {
            let avail = allowed_actions(&CHARTS[c]);
            for h in 0..NCLASSES {
                raw[c][h] = choose(&cells[c][h].means(), avail, cfg.margin);
            }
            let (p, ch) = monotone(&raw[c], &cells[c], avail);
            pure[c] = p;
            changed[c] = ch;
            policy[c] = Some(p);
        }
    }
    Derivation { cells, raw, pure, changed_by_mono: changed, trial_total, elapsed: start.elapsed().as_secs_f64(), job_seconds }
}

// ---------------------------------------------------------------- monotonicity

/// Dominance chains, weakest hand first: pairs, each suited row, each offsuit row.
pub fn chains() -> Vec<Vec<usize>> {
    let mut out: Vec<Vec<usize>> = Vec::new();
    out.push((0..13).collect());
    for high in 1..13usize {
        let tri = high * (high - 1) / 2;
        out.push((0..high).map(|low| 13 + tri + low).collect());
        out.push((0..high).map(|low| 91 + tri + low).collect());
    }
    out
}

/// Enforces that along each chain a stronger hand never takes a weaker action,
/// choosing the monotone assignment with the least EV lost against `raw`.
/// Returns the chart and how many cells changed.
pub fn monotone(raw: &[Pure; NCLASSES], cells: &[Cell], avail: [bool; 3]) -> ([Pure; NCLASSES], usize) {
    let mut out = *raw;
    for chain in chains() {
        // cost[i][a] = least cost of hands 0..=i with hand i taking action a.
        let n = chain.len();
        let mut cost = vec![[f64::INFINITY; 3]; n];
        let mut from = vec![[0usize; 3]; n];
        let hand_cost = |h: usize, a: usize| -> f64 {
            if !avail[a] {
                return f64::INFINITY;
            }
            let m = cells[h].means();
            let c = raw[h] as usize;
            if a == c {
                0.0
            } else {
                (m[c] - m[a]).max(0.0) + 1e-6
            }
        };
        for a in 0..3 {
            cost[0][a] = hand_cost(chain[0], a);
        }
        for i in 1..n {
            let mut best_prev = f64::INFINITY;
            let mut best_a = 0;
            for a in 0..3 {
                if cost[i - 1][a] < best_prev {
                    best_prev = cost[i - 1][a];
                    best_a = a;
                }
                let hc = hand_cost(chain[i], a);
                cost[i][a] = if best_prev.is_finite() && hc.is_finite() { best_prev + hc } else { f64::INFINITY };
                from[i][a] = best_a;
            }
        }
        let mut a = (0..3).min_by(|&x, &y| cost[n - 1][x].partial_cmp(&cost[n - 1][y]).unwrap()).unwrap();
        for i in (0..n).rev() {
            out[chain[i]] = PURES[a];
            if i > 0 {
                a = from[i][a];
            }
        }
    }
    let changed = (0..NCLASSES).filter(|&h| out[h] != raw[h]).count();
    (out, changed)
}

/// One notch tighter: at each boundary along every chain, the weakest hand of
/// an action level moves down one level. Applied at once to the original chart.
pub fn one_notch_tighter(pure: &[Pure; NCLASSES], avail: [bool; 3]) -> [Pure; NCLASSES] {
    let mut out = *pure;
    let lowest = (0..3).find(|&a| avail[a]).unwrap();
    let lower = |a: usize| -> usize {
        let mut x = a;
        while x > lowest {
            x -= 1;
            if avail[x] {
                return x;
            }
        }
        a
    };
    for chain in chains() {
        for i in 0..chain.len() {
            let a = pure[chain[i]] as usize;
            let prev = if i == 0 { lowest } else { pure[chain[i - 1]] as usize };
            if a > lowest && (i == 0 || a > prev) {
                out[chain[i]] = PURES[lower(a)];
            }
        }
    }
    out
}

// ---------------------------------------------------------------- notation

const RANKS: &[u8; 13] = b"23456789TJQKA";

fn token_of(class: usize) -> String {
    class_name(class)
}

fn compress_row(out: &mut Vec<String>, members: &[bool], high: usize, suffix: char) {
    let r = |x: usize| RANKS[x] as char;
    let mut low = high;
    while low > 0 {
        low -= 1;
        if !members[low] {
            continue;
        }
        let top = low;
        while low > 0 && members[low - 1] {
            low -= 1;
        }
        let bottom = low;
        if top == high - 1 && bottom != top {
            out.push(format!("{}{}{}+", r(high), r(bottom), suffix));
        } else if top == high - 1 {
            out.push(format!("{}{}{}", r(high), r(top), suffix));
        } else if top == bottom {
            out.push(format!("{}{}{}", r(high), r(top), suffix));
        } else {
            out.push(format!("{}{}{}-{}{}{}", r(high), r(bottom), suffix, r(high), r(top), suffix));
        }
    }
}

/// Compact range notation. Tokens: `77`, `77+`, `55-77`, `ATs`, `ATs+`,
/// `A6s-A9s`, `AJo`, `AJo+`, `A6o-A9o`. Strongest-first within groups.
pub fn notation(set: &[bool; NCLASSES]) -> String {
    let mut out: Vec<String> = Vec::new();
    // Pairs.
    let mut r = 13usize;
    while r > 0 {
        r -= 1;
        if !set[r] {
            continue;
        }
        let top = r;
        while r > 0 && set[r - 1] {
            r -= 1;
        }
        let bottom = r;
        let tc = RANKS[top] as char;
        let bc = RANKS[bottom] as char;
        if top == 12 && bottom != 12 {
            out.push(format!("{bc}{bc}+"));
        } else if top == bottom {
            out.push(format!("{tc}{tc}"));
        } else {
            out.push(format!("{bc}{bc}-{tc}{tc}"));
        }
    }
    for suffix in ['s', 'o'] {
        let base = if suffix == 's' { 13 } else { 91 };
        for high in (1..13).rev() {
            let tri = high * (high - 1) / 2;
            let members: Vec<bool> = (0..high).map(|low| set[base + tri + low]).collect();
            compress_row(&mut out, &members, high, suffix);
        }
    }
    out.join(" ")
}

/// Parses notation back into a class set. The inverse of `notation`.
pub fn parse_notation(text: &str) -> Result<[bool; NCLASSES], String> {
    let mut set = [false; NCLASSES];
    let rank = |c: char| RANKS.iter().position(|&x| x as char == c).ok_or_else(|| format!("bad rank {c}"));
    for tok in text.split_whitespace() {
        let (body, plus) = match tok.strip_suffix('+') {
            Some(b) => (b, true),
            None => (tok, false),
        };
        let (left, right) = match body.split_once('-') {
            Some((l, r)) => (l, Some(r)),
            None => (body, None),
        };
        let lc: Vec<char> = left.chars().collect();
        if lc.len() < 2 {
            return Err(format!("bad token {tok}"));
        }
        let hi = rank(lc[0])?;
        let lo = rank(lc[1])?;
        if hi == lo {
            let top = if let Some(rt) = right {
                let rc: Vec<char> = rt.chars().collect();
                rank(rc[0])?
            } else if plus {
                12
            } else {
                hi
            };
            for r in lo..=top {
                set[r] = true;
            }
        } else {
            let suffix = *lc.get(2).ok_or_else(|| format!("bad token {tok}"))?;
            let base = match suffix {
                's' => 13usize,
                'o' => 91usize,
                _ => return Err(format!("bad suffix in {tok}")),
            };
            let tri = hi * (hi - 1) / 2;
            let top = if let Some(rt) = right {
                let rc: Vec<char> = rt.chars().collect();
                if rc.len() < 3 {
                    return Err(format!("bad token {tok}"));
                }
                rank(rc[1])?
            } else if plus {
                hi - 1
            } else {
                lo
            };
            for r in lo..=top {
                set[base + tri + r] = true;
            }
        }
    }
    Ok(set)
}

pub fn pct_of(set: &[bool; NCLASSES]) -> f64 {
    let combos: u32 = (0..NCLASSES).filter(|&c| set[c]).map(class_combos).sum();
    f64::from(combos) * 100.0 / 1326.0
}

/// Indicator set for an action in a chart.
pub fn action_set(ch: &[Pure; NCLASSES], a: Pure) -> [bool; NCLASSES] {
    let mut s = [false; NCLASSES];
    for c in 0..NCLASSES {
        s[c] = ch[c] == a;
    }
    s
}

pub fn class_label(class: usize) -> String {
    token_of(class)
}

pub fn percentiles() -> &'static [f64; 169] {
    hand_percentiles()
}
