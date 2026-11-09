//! Opponent models. EVERY NUMBER IN THIS FILE IS AN ASSUMPTION. None of them
//! is measured from real play; they encode a rough picture of loose home-game
//! opponents. The VPIP/PFR anchors are the only targets taken from the brief.
//!
//! A concrete `PlayerModel` is an archetype plus noise in log-odds, so the
//! ensemble says "we do not know these players precisely".

use crate::combos::class_combos;
use crate::game::{Act, Round};
use crate::preflop_order::ORDER;
use poker_trainer::preflop::Seat;
use poker_trainer::rng::Rng;
use std::sync::OnceLock;

/// Strength buckets, strongest first: assumption, six levels, cut points below.
pub const NBUCKET: usize = 6;
/// A class is a bucket, plus NBUCKET when the draw flag is set.
pub const NCLASS: usize = NBUCKET * 2;

/// Upper edge (share of all holdings stronger than this) of each bucket.
/// Assumption: monsters 5%, strong to 15%, good to 30%, medium to 50%, weak to
/// 75%, air the rest. Percentile is among all holdings on this exact board.
pub const BUCKET_EDGES: [f64; NBUCKET - 1] = [0.05, 0.15, 0.30, 0.50, 0.75];

pub fn bucket_of_share(share: f64) -> u8 {
    for (i, &edge) in BUCKET_EDGES.iter().enumerate() {
        if share < edge {
            return i as u8;
        }
    }
    (NBUCKET - 1) as u8
}

/// Probability floor on every action, so that no observed line is impossible.
pub const PROB_FLOOR: f64 = 0.001;

/// Spread (log-odds) of per-model noise on the main parameters.
pub const NOISE_SD: f64 = 0.5;
/// Extra per-bucket spread on the postflop vectors, assumption.
pub const NOISE_BUCKET_SD: f64 = 0.25;
/// Log-normal spread on price sensitivity and multiway tightening.
pub const NOISE_SCALE_SD: f64 = 0.3;

/// Preflop soft edge (percent of hands), so ranges are blurred, not sharp.
pub const PREFLOP_SOFT: f64 = 1.5;

/// Position range multiplier applied to opening thresholds.
/// Assumption: early seats play about 70% as many hands as the anchors.
pub fn position_factor(seat: Seat) -> f64 {
    match seat {
        Seat::Early => 0.70,
        Seat::Middle => 0.85,
        Seat::Cutoff => 1.00,
        Seat::Button => 1.25,
        Seat::SmallBlind => 1.00,
        Seat::BigBlind => 1.10,
    }
}

/// Numbers for one archetype. Preflop thresholds are percent of all hands
/// in the 169 ordering, postflop vectors are probabilities by bucket, 0 strongest.
#[derive(Clone, Copy, Debug)]
pub struct ArchetypeDef {
    pub name: &'static str,
    /// VPIP and PFR anchors, percent of hands (see tuning note on PlayerModel).
    pub vpip: f64,
    pub pfr: f64,
    /// Cold-call range versus one raise, percent of hands.
    pub cold_call: f64,
    /// Three-bet range versus one raise, percent of hands.
    pub three_bet: f64,
    /// Versus a 3-bet: call range (percent) and 4-bet range (percent).
    pub call_3bet: f64,
    pub four_bet: f64,
    /// Versus a 4-bet or more: call range and shove range (percent).
    pub call_4bet: f64,
    pub five_bet: f64,
    /// Probability of each open-size class (`OPEN_SIZES`), preflop.
    pub open_probs: [f64; 3],
    /// P(bet | checked to) by bucket.
    pub bet: [f64; NBUCKET],
    /// P(continue | facing a medium bet) by bucket.
    pub cont: [f64; NBUCKET],
    /// P(raise | continue) by bucket.
    pub raise: [f64; NBUCKET],
    /// Log-odds additions when the draw flag is set (flop and turn only).
    pub draw_bet: f64,
    pub draw_cont: f64,
    pub draw_raise: f64,
    /// Log-odds shift per later street (applied 0,1,2 for flop,turn,river).
    pub bet_street: f64,
    pub cont_street: f64,
    /// Log-odds of continuing lost per unit of bet size relative to medium.
    pub price_sens: f64,
    /// Log-odds tightening per extra opponent still in hand.
    pub multiway: f64,
    /// Log-odds lost when facing a raise (rather than a first bet).
    pub raised_pen: f64,
    /// Probability of each postflop bet-size class (`SIZE_FRACS`), by street
    /// (flop, turn, river). A sampled model draws its class once per street.
    pub size_probs: [[f64; 3]; 3],
}

/// Realized VPIP/PFR targets from the brief (percent of hands). The threshold
/// fields `vpip`/`pfr` below are tuned so that simulated hands land near these;
/// the validate binary prints the realized numbers.
pub const ANCHORS: [(f64, f64); 5] = [(55.0, 8.0), (45.0, 8.0), (14.0, 11.0), (60.0, 35.0), (25.0, 18.0)];

/// Postflop bet-size classes, fraction of pot. Assumption: three discrete sizes,
/// so a size is a soft signal about the player and does not split every table.
pub const SIZE_FRACS: [f64; 3] = [1.0 / 3.0, 2.0 / 3.0, 1.0];
/// Preflop open-size classes in bb, plus 1bb per limper. Assumption.
pub const OPEN_SIZES: [f64; 3] = [2.5, 3.0, 4.0];

/// Population mix per seat, from the brief.
pub const POPULATION: [f64; 5] = [0.25, 0.35, 0.10, 0.10, 0.20];

pub const STATION: usize = 0;
pub const LOOSE_PASSIVE: usize = 1;
pub const NIT: usize = 2;
pub const MANIAC: usize = 3;
pub const SOLID: usize = 4;

/// Ranges and postflop vectors: assumptions chosen so a simulated table of
/// each archetype lands near the VPIP/PFR anchors (checked by a test) and the
/// archetypes differ in the directions the brief describes.
pub const ARCHETYPES: [ArchetypeDef; 5] = [
    ArchetypeDef {
        name: "station",
        vpip: 62.0,
        pfr: 10.0,
        cold_call: 40.0,
        three_bet: 2.5,
        call_3bet: 16.0,
        four_bet: 1.0,
        call_4bet: 5.0,
        five_bet: 0.8,
        open_probs: [0.25, 0.45, 0.3],
        bet: [0.55, 0.50, 0.35, 0.12, 0.05, 0.03],
        cont: [0.995, 0.985, 0.95, 0.80, 0.55, 0.30],
        raise: [0.25, 0.10, 0.03, 0.01, 0.01, 0.01],
        draw_bet: 0.5,
        draw_cont: 1.5,
        draw_raise: 0.3,
        bet_street: -0.05,
        cont_street: -0.05,
        price_sens: 0.4,
        multiway: 0.10,
        raised_pen: 1.2,
        size_probs: [[0.6, 0.35, 0.05], [0.55, 0.35, 0.1], [0.5, 0.35, 0.15]],
    },
    ArchetypeDef {
        name: "loose-passive",
        vpip: 51.0,
        pfr: 10.5,
        cold_call: 30.0,
        three_bet: 2.0,
        call_3bet: 10.0,
        four_bet: 1.0,
        call_4bet: 3.0,
        five_bet: 0.8,
        open_probs: [0.3, 0.5, 0.2],
        bet: [0.60, 0.50, 0.35, 0.15, 0.08, 0.05],
        cont: [0.98, 0.95, 0.85, 0.60, 0.30, 0.12],
        raise: [0.25, 0.12, 0.04, 0.02, 0.01, 0.01],
        draw_bet: 0.7,
        draw_cont: 1.5,
        draw_raise: 0.4,
        bet_street: -0.1,
        cont_street: -0.2,
        price_sens: 1.0,
        multiway: 0.30,
        raised_pen: 1.5,
        size_probs: [[0.5, 0.4, 0.1], [0.45, 0.45, 0.1], [0.4, 0.45, 0.15]],
    },
    ArchetypeDef {
        name: "nit",
        vpip: 15.5,
        pfr: 15.5,
        cold_call: 4.0,
        three_bet: 2.0,
        call_3bet: 3.0,
        four_bet: 1.5,
        call_4bet: 1.5,
        five_bet: 1.0,
        open_probs: [0.3, 0.6, 0.1],
        bet: [0.85, 0.70, 0.45, 0.10, 0.03, 0.02],
        cont: [0.97, 0.90, 0.60, 0.20, 0.05, 0.02],
        raise: [0.55, 0.30, 0.06, 0.01, 0.01, 0.01],
        draw_bet: 0.8,
        draw_cont: 1.2,
        draw_raise: 0.3,
        bet_street: -0.1,
        cont_street: -0.3,
        price_sens: 3.0,
        multiway: 0.70,
        raised_pen: 2.0,
        size_probs: [[0.15, 0.6, 0.25], [0.1, 0.6, 0.3], [0.1, 0.55, 0.35]],
    },
    ArchetypeDef {
        name: "maniac",
        vpip: 68.0,
        pfr: 46.0,
        cold_call: 22.0,
        three_bet: 12.0,
        call_3bet: 14.0,
        four_bet: 6.0,
        call_4bet: 6.0,
        five_bet: 3.0,
        open_probs: [0.1, 0.3, 0.6],
        bet: [0.90, 0.85, 0.75, 0.60, 0.50, 0.45],
        cont: [0.98, 0.95, 0.85, 0.65, 0.45, 0.35],
        raise: [0.70, 0.55, 0.35, 0.25, 0.20, 0.20],
        draw_bet: 0.9,
        draw_cont: 1.2,
        draw_raise: 0.8,
        bet_street: 0.0,
        cont_street: -0.1,
        price_sens: 0.8,
        multiway: 0.20,
        raised_pen: 0.8,
        size_probs: [[0.05, 0.35, 0.6], [0.05, 0.3, 0.65], [0.05, 0.25, 0.7]],
    },
    ArchetypeDef {
        name: "solid",
        vpip: 27.5,
        pfr: 26.0,
        cold_call: 10.0,
        three_bet: 5.0,
        call_3bet: 6.0,
        four_bet: 2.5,
        call_4bet: 2.0,
        five_bet: 1.5,
        open_probs: [0.3, 0.6, 0.1],
        bet: [0.85, 0.75, 0.55, 0.20, 0.15, 0.15],
        cont: [0.98, 0.95, 0.80, 0.45, 0.18, 0.08],
        raise: [0.55, 0.30, 0.10, 0.04, 0.03, 0.03],
        draw_bet: 1.0,
        draw_cont: 1.4,
        draw_raise: 0.6,
        bet_street: -0.15,
        cont_street: -0.2,
        price_sens: 2.0,
        multiway: 0.50,
        raised_pen: 1.7,
        size_probs: [[0.2, 0.6, 0.2], [0.15, 0.6, 0.25], [0.15, 0.55, 0.3]],
    },
];

fn logit(p: f64) -> f64 {
    let p = p.clamp(1e-4, 1.0 - 1e-4);
    (p / (1.0 - p)).ln()
}

fn logistic(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

/// Standard normal via Box-Muller from the library RNG.
pub fn gauss(rng: &mut Rng) -> f64 {
    let u1 = (rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
    let u2 = (rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
    (-2.0 * u1.max(1e-15).ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
}

/// A concrete table seat's model: archetype plus log-odds noise. The anchor
/// percentages are themselves noised, then used as thresholds.
#[derive(Clone, Debug)]
pub struct PlayerModel {
    pub archetype: usize,
    pub vpip: f64,
    pub pfr: f64,
    pub cold_call: f64,
    pub three_bet: f64,
    pub call_3bet: f64,
    pub four_bet: f64,
    pub call_4bet: f64,
    pub five_bet: f64,
    /// This model's open-size class, and bet-size class per street.
    pub open_class: u8,
    pub size_class: [u8; 3],
    pub bet: [f64; NBUCKET],
    pub cont: [f64; NBUCKET],
    pub raise: [f64; NBUCKET],
    pub draw_bet: f64,
    pub draw_cont: f64,
    pub draw_raise: f64,
    pub bet_street: f64,
    pub cont_street: f64,
    pub price_sens: f64,
    pub multiway: f64,
    pub raised_pen: f64,
    pub bet_frac: [f64; 3],
    /// Recalibrated preflop response rules, or `None` for the base population.
    pub cal: Option<&'static crate::preflop_cal::Calibration>,
}

pub fn sample_archetype(rng: &mut Rng) -> usize {
    let u = (rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
    let mut c = 0.0;
    for (i, &p) in POPULATION.iter().enumerate() {
        c += p;
        if u < c {
            return i;
        }
    }
    POPULATION.len() - 1
}

impl PlayerModel {
    /// Noise-free model for an archetype.
    pub fn exact(archetype: usize) -> PlayerModel {
        let a = &ARCHETYPES[archetype];
        PlayerModel {
            archetype,
            vpip: a.vpip,
            pfr: a.pfr,
            cold_call: a.cold_call,
            three_bet: a.three_bet,
            call_3bet: a.call_3bet,
            four_bet: a.four_bet,
            call_4bet: a.call_4bet,
            five_bet: a.five_bet,
            open_class: modal(&a.open_probs),
            size_class: [modal(&a.size_probs[0]), modal(&a.size_probs[1]), modal(&a.size_probs[2])],
            bet: a.bet,
            cont: a.cont,
            raise: a.raise,
            draw_bet: a.draw_bet,
            draw_cont: a.draw_cont,
            draw_raise: a.draw_raise,
            bet_street: a.bet_street,
            cont_street: a.cont_street,
            price_sens: a.price_sens,
            multiway: a.multiway,
            raised_pen: a.raised_pen,
            bet_frac: [SIZE_FRACS[usize::from(modal(&a.size_probs[0]))], SIZE_FRACS[usize::from(modal(&a.size_probs[1]))], SIZE_FRACS[usize::from(modal(&a.size_probs[2]))]],
            cal: None,
        }
    }

    /// Samples archetype from the population mix, then noise.
    pub fn sample(rng: &mut Rng) -> PlayerModel {
        let archetype = sample_archetype(rng);
        Self::sample_of(rng, archetype)
    }

    /// Samples from the population recalibrated by `cal` (`preflop_cal.rs`).
    pub fn sample_cal(rng: &mut Rng, cal: &'static crate::preflop_cal::Calibration) -> PlayerModel {
        let mut m = Self::sample(rng);
        cal.apply_postflop(&mut m);
        m.cal = Some(cal);
        m
    }

    pub fn sample_of(rng: &mut Rng, archetype: usize) -> PlayerModel {
        let mut m = PlayerModel::exact(archetype);
        let pct = |p: f64, rng: &mut Rng| 100.0 * logistic(logit(p / 100.0) + NOISE_SD * gauss(rng));
        m.vpip = pct(m.vpip, rng);
        m.pfr = pct(m.pfr, rng).min(m.vpip);
        m.cold_call = pct(m.cold_call, rng);
        m.three_bet = pct(m.three_bet, rng);
        m.call_3bet = pct(m.call_3bet, rng);
        m.four_bet = pct(m.four_bet, rng);
        m.call_4bet = pct(m.call_4bet, rng);
        m.five_bet = pct(m.five_bet, rng);
        // Shared shift plus per-bucket jitter on each postflop vector.
        let vec = |v: &mut [f64; NBUCKET], rng: &mut Rng| {
            let common = NOISE_SD * gauss(rng);
            for p in v.iter_mut() {
                *p = logistic(logit(*p) + common + NOISE_BUCKET_SD * gauss(rng));
            }
        };
        vec(&mut m.bet, rng);
        vec(&mut m.cont, rng);
        vec(&mut m.raise, rng);
        m.price_sens *= (NOISE_SCALE_SD * gauss(rng)).exp();
        m.multiway *= (NOISE_SCALE_SD * gauss(rng)).exp();
        let a = &ARCHETYPES[archetype];
        m.open_class = draw_class(&a.open_probs, 0b111, rng).0;
        for street in 0..3 {
            let c = draw_class(&a.size_probs[street], 0b111, rng).0;
            m.size_class[street] = c;
            m.bet_frac[street] = SIZE_FRACS[usize::from(c)];
        }
        m
    }

    /// Fixes size classes to what was observed, returning the log importance
    /// weight. `obs` holds `(street, mask)`, street 0 being preflop (opens) and
    /// 1..=3 flop to river, and the mask marks the classes consistent with the
    /// observed amount. All classes in a mask means no information (capped bets).
    pub fn constrain_sizes(&mut self, obs: &[(u8, u8)], rng: &mut Rng) -> f64 {
        let a = &ARCHETYPES[self.archetype];
        let mut log_w = 0.0;
        for &(street, mask) in obs {
            if street == 0 {
                let (c, sum) = draw_class(&a.open_probs, mask, rng);
                self.open_class = c;
                log_w += sum.ln();
            } else {
                let i = usize::from(street) - 1;
                let (c, sum) = draw_class(&a.size_probs[i], mask, rng);
                self.size_class[i] = c;
                self.bet_frac[i] = SIZE_FRACS[usize::from(c)];
                log_w += sum.ln();
            }
        }
        log_w
    }

    /// Log-odds action table for one postflop situation by class.
    fn post_row_into(&self, street: usize, mw: usize, rl: usize, sit: usize, out: &mut [[f32; 3]; NCLASS]) {
        let st = street as f64;
        let mw = mw as f64;
        let centre = |s: usize| match s {
            1 => 0.33,
            2 => 0.60,
            _ => 1.00,
        };
        for class in 0..NCLASS {
            let bucket = class % NBUCKET;
            let draw = class >= NBUCKET && street < 2;
            if sit == 0 {
                let x = logit(self.bet[bucket])
                    + st * self.bet_street
                    - mw * self.multiway
                    + if draw { self.draw_bet } else { 0.0 };
                let p_bet = floor(logistic(x));
                let p_check = floor(1.0 - p_bet);
                let total = p_check + p_bet;
                out[class] = [(p_check / total) as f32, (p_bet / total) as f32, 0.0];
            } else {
                let x = logit(self.cont[bucket])
                    + st * self.cont_street
                    - mw * self.multiway
                    - self.price_sens * (centre(sit) - 0.60)
                    - rl as f64 * self.raised_pen
                    + if draw { self.draw_cont } else { 0.0 };
                let cont = logistic(x);
                let rx = logit(self.raise[bucket])
                    - mw * self.multiway
                    - rl as f64 * self.raised_pen
                    + if draw { self.draw_raise } else { 0.0 };
                let raise = cont * logistic(rx);
                let call = cont - raise;
                let fold = 1.0 - cont;
                let total = floor(fold) + floor(call) + floor(raise);
                out[class] = [(floor(fold) / total) as f32, (floor(call) / total) as f32, (floor(raise) / total) as f32];
            }
        }
    }

    /// Row of `[check, bet, 0]` or `[fold, call, raise]` for this situation.
    pub fn post_row(&self, street: usize, mw: usize, rl: usize, sit: usize) -> [[f32; 3]; NCLASS] {
        let mut out = [[0.0; 3]; NCLASS];
        self.post_row_into(street, mw, rl, sit, &mut out);
        out
    }

    /// Precomputed action tables for the expectimax.
    pub fn tables(&self) -> VillainTables {
        let mut data = vec![[0f32; 3]; 3 * 3 * 3 * 4 * NCLASS];
        let mut row = [[0f32; 3]; NCLASS];
        for street in 0..3 {
            for mw in 0..3 {
                for rl in 0..3 {
                    for sit in 0..4 {
                        self.post_row_into(street, mw, rl, sit, &mut row);
                        let base = VillainTables::base(street, mw, rl, sit);
                        data[base..base + NCLASS].copy_from_slice(&row);
                    }
                }
            }
        }
        let mut bet_frac = [0f32; 3];
        for (a, b) in bet_frac.iter_mut().zip(self.bet_frac.iter()) {
            *a = *b as f32;
        }
        VillainTables { data, bet_frac }
    }

    /// Preflop `[fold, call_or_check, raise]` by the 169 hand classes.
    pub fn preflop_row(&self, ctx: &PreflopCtx) -> [[f32; 3]; 169] {
        let (raise_th, cont_th) = self.preflop_thresholds(ctx);
        let pct = hand_percentiles();
        let mut out = [[0f32; 3]; 169];
        for class in 0..169 {
            let p = pct[class];
            let raise = soft(raise_th - p);
            let cont = soft(cont_th - p).max(raise);
            let call = cont - raise;
            let fold = 1.0 - cont;
            out[class] = apply_floor_preflop(fold, call, raise, ctx.free_check);
        }
        out
    }

    /// `preflop_row` for a single class, for callers that decide one hand at a time.
    pub fn preflop_probs(&self, ctx: &PreflopCtx, class: usize) -> [f32; 3] {
        let (raise_th, cont_th) = self.preflop_thresholds(ctx);
        let p = hand_percentiles()[class];
        let raise = soft(raise_th - p);
        let cont = soft(cont_th - p).max(raise);
        apply_floor_preflop(1.0 - cont, cont - raise, raise, ctx.free_check)
    }

    /// One class's `[fold, call, raise]` from explicit thresholds (percent of hands).
    pub fn preflop_probs_at(&self, ctx: &PreflopCtx, class: usize, raise_th: f64, cont_th: f64) -> [f32; 3] {
        let p = hand_percentiles()[class];
        let raise = soft(raise_th - p);
        let cont = soft(cont_th - p).max(raise);
        apply_floor_preflop(1.0 - cont, cont - raise, raise, ctx.free_check)
    }

    fn preflop_thresholds(&self, ctx: &PreflopCtx) -> (f64, f64) {
        if let Some(t) = self.cal.and_then(|c| c.thresholds(self, ctx, ctx.limped)) {
            return t;
        }
        let pos = position_factor(ctx.position);
        match ctx.raises {
            0 => {
                if ctx.free_check {
                    (self.pfr * 0.6, 100.0)
                } else if ctx.limpers == 0 {
                    let cont = if ctx.position == Seat::SmallBlind { self.vpip * 1.2 } else { self.vpip * pos };
                    (self.pfr * pos, cont.min(100.0))
                } else {
                    (self.pfr * pos * 0.8, (self.vpip * pos * 1.1).min(100.0))
                }
            }
            1 => {
                // Facing a single raise; bigger raises tighten continues.
                let size_ref = 3.0 / ctx.last_raise_to.max(2.0);
                let size_factor = size_ref.powf(0.5 * self.price_sens.min(3.0));
                let defend = match ctx.position {
                    Seat::BigBlind => 1.3,
                    Seat::SmallBlind => 1.1,
                    _ => 1.0,
                };
                let raise = self.three_bet * pos;
                (raise, (raise + self.cold_call * pos * size_factor * defend).min(100.0))
            }
            2 => {
                // Facing a 3-bet: openers defend more than cold-callers.
                let (four, call) = if ctx.raised_earlier { (self.four_bet, self.call_3bet) } else { (self.four_bet * 0.6, self.call_3bet * 0.6) };
                (four, (four + call).min(100.0))
            }
            _ => (self.five_bet, (self.five_bet + self.call_4bet).min(100.0)),
        }
    }

    /// Preflop raise size in bb (a total raise-to amount): open, 3-bet, 4-bet...
    pub fn preflop_raise_to(&self, r: &Round) -> f64 {
        match r.raises {
            0 => open_target(r, usize::from(self.open_class)),
            1 => r.current * 3.0,
            _ => r.current * 2.5,
        }
    }
}

/// Raise-to for an open of size class `c`: the class size plus 1bb per limper.
pub fn open_target(r: &Round, c: usize) -> f64 {
    OPEN_SIZES[c] + (r.pot - 1.5).max(0.0)
}

fn modal(p: &[f64; 3]) -> u8 {
    let mut b = 0;
    for i in 1..3 {
        if p[i] > p[b] {
            b = i;
        }
    }
    b as u8
}

/// Draws a class from `p` restricted to `mask`. Returns the class and the
/// probability of the mask under `p` (the importance weight).
fn draw_class(p: &[f64; 3], mask: u8, rng: &mut Rng) -> (u8, f64) {
    let sum: f64 = (0..3).filter(|&c| mask >> c & 1 == 1).map(|c| p[c]).sum();
    let mut u = (rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64 * sum;
    for c in 0..3 {
        if mask >> c & 1 == 1 {
            if u < p[c] {
                return (c as u8, sum);
            }
            u -= p[c];
        }
    }
    ((0..3).rev().find(|&c| mask >> c & 1 == 1).unwrap() as u8, sum)
}

fn floor(p: f64) -> f64 {
    p.max(PROB_FLOOR)
}

fn apply_floor_preflop(fold: f64, call: f64, raise: f64, free_check: bool) -> [f32; 3] {
    if free_check {
        // Fold is not a legal action with the free option; no floor needed there.
        let (c, r) = (call.max(PROB_FLOOR), raise.max(PROB_FLOOR));
        [0.0, (c / (c + r)) as f32, (r / (c + r)) as f32]
    } else {
        let (f, c, r) = (floor(fold), floor(call), floor(raise));
        let t = f + c + r;
        [(f / t) as f32, (c / t) as f32, (r / t) as f32]
    }
}

fn soft(x: f64) -> f64 {
    logistic(x / PREFLOP_SOFT)
}

/// Each 169 class's percentile (0 best, 100 worst) from the generated order,
/// at the midpoint of the class's combo count.
pub fn hand_percentiles() -> &'static [f64; 169] {
    static T: OnceLock<[f64; 169]> = OnceLock::new();
    T.get_or_init(|| {
        let mut out = [0.0; 169];
        let mut cum = 0.0;
        for &class in ORDER.iter() {
            let c = class_combos(class as usize) as f64;
            out[class as usize] = 100.0 * (cum + c / 2.0) / 1326.0;
            cum += c;
        }
        out
    })
}

/// Everything the preflop model needs to know about the node.
#[derive(Clone, Copy, Debug)]
pub struct PreflopCtx {
    pub position: Seat,
    pub raises: u8,
    pub limpers: u8,
    pub last_raise_to: f64,
    pub raised_earlier: bool,
    /// Big blind with no raise: fold not legal.
    pub free_check: bool,
    /// The seat already called voluntarily preflop without raising.
    pub limped: bool,
}

/// Action probability tables for the expectimax, by situation and class.
#[derive(Clone, Debug)]
pub struct VillainTables {
    pub data: Vec<[f32; 3]>,
    pub bet_frac: [f32; 3],
}

impl VillainTables {
    #[inline]
    fn base(street: usize, mw: usize, rl: usize, sit: usize) -> usize {
        ((((street * 3 + mw) * 3 + rl) * 4 + sit)) * NCLASS
    }

    #[inline]
    pub fn row(&self, street: usize, mw: usize, rl: usize, sit: usize) -> &[[f32; 3]] {
        let b = Self::base(street, mw, rl, sit);
        &self.data[b..b + NCLASS]
    }

    /// Identical action probabilities across every class and situation of
    /// their kind, used by exact toy tests. `[p_check, p_bet]` when
    /// unbet, `[p_fold, p_call, p_raise]` when facing a bet.
    pub fn uniform(check_bet: [f32; 2], fold_call_raise: [f32; 3], bet_frac: [f32; 3]) -> VillainTables {
        let mut t = VillainTables {
            data: vec![[0.0; 3]; 3 * 3 * 3 * 4 * NCLASS],
            bet_frac,
        };
        for street in 0..3 {
            for mw in 0..3 {
                for rl in 0..3 {
                    for sit in 0..4 {
                        let base = Self::base(street, mw, rl, sit);
                        for c in 0..NCLASS {
                            t.data[base + c] = if sit == 0 {
                                [check_bet[0], check_bet[1], 0.0]
                            } else {
                                fold_call_raise
                            };
                        }
                    }
                }
            }
        }
        t
    }

    /// Sets one class's row for one situation across all multiway/raise levels.
    pub fn set_class(&mut self, street: usize, sit: usize, class: usize, probs: [f32; 3]) {
        for mw in 0..3 {
            for rl in 0..3 {
                let b = Self::base(street, mw, rl, sit);
                self.data[b + class] = probs;
            }
        }
    }
}

/// The action index used by `PlayerModel::post_row` for an observed postflop act.
pub fn post_action_index(act: Act, facing: bool) -> usize {
    match (act, facing) {
        (Act::Check, false) => 0,
        (Act::Bet(_), false) => 1,
        (Act::Fold, true) => 0,
        (Act::Call, true) => 1,
        (Act::Raise(_), true) => 2,
        _ => panic!("action {act:?} illegal for facing={facing}"),
    }
}
