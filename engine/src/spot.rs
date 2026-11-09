//! A decision spot: replay of a public history, beliefs and posterior over
//! sampled tables, per-option EVs, and the verdict.

use crate::board::{classify, Runouts, DEAD};
use crate::combos::{combos, NCOMBOS};
use crate::game::{Act, Round, EPS};
use crate::prof;
use crate::models::{open_target, post_action_index, PlayerModel, PreflopCtx, VillainTables, NCLASS, SIZE_FRACS};
use crate::solve::{solve_root, SolveCfg, Villain};
use poker_trainer::cards::{Card, Holding};
use poker_trainer::preflop::Seat;
use poker_trainer::rng::Rng;

/// Maximum number of preflop raises in the model and simulator.
pub const PREFLOP_MAX_RAISES: u8 = 4;

#[derive(Clone, Copy, Debug)]
pub struct HistAct {
    pub street: u8,
    pub seat: u8,
    pub act: Act,
}

#[derive(Clone, Debug)]
pub struct Spot {
    pub n: usize,
    pub hero_seat: usize,
    pub stack: f64,
    pub hero: Holding,
    /// Board at the decision: 3, 4 or 5 cards.
    pub board: Vec<Card>,
    pub history: Vec<HistAct>,
}

pub fn seat_label(n: usize, seat: usize) -> &'static str {
    let behind = behind_of(n, seat);
    match behind {
        0 => "BB",
        1 => "SB",
        2 => "BTN",
        3 => "CO",
        4 => "MP",
        5 if n == 7 => "UTG+1",
        _ => "UTG",
    }
}

/// Number of players still to act behind this seat, preflop order.
pub fn behind_of(n: usize, seat: usize) -> u8 {
    match seat {
        0 => 1,
        1 => 0,
        _ => (n - seat + 1) as u8,
    }
}

pub fn position_of(n: usize, seat: usize) -> Seat {
    Seat::from_behind(behind_of(n, seat))
}

/// Whether a seat has already called voluntarily preflop without raising.
pub fn limped_seat(r: &Round, seat: usize) -> bool {
    r.raises >= 1 && if seat == 1 { false } else if seat == 0 { r.put[0] > 0.5 + EPS && r.put[0] < r.current } else { r.put[seat] > 0.0 && r.put[seat] < r.current }
}

/// Preflop tracker shared by replay and simulation.
#[derive(Clone, Copy, Debug, Default)]
pub struct PreflopTrack {
    pub limpers: u8,
    pub raised_mask: u8,
}

impl PreflopTrack {
    pub fn ctx(&self, r: &Round, seat: usize, n: usize) -> PreflopCtx {
        PreflopCtx {
            position: position_of(n, seat),
            raises: r.raises,
            limpers: self.limpers,
            last_raise_to: r.current,
            raised_earlier: self.raised_mask >> seat & 1 == 1,
            free_check: seat == 1 && r.raises == 0 && r.current <= r.put[1] + EPS,
            limped: limped_seat(r, seat),
        }
    }

    pub fn note(&mut self, r: &Round, seat: usize, act: Act) {
        match act {
            Act::Call if r.raises == 0 && !(seat == 1 && r.current <= r.put[1] + EPS) => {
                self.limpers += 1;
            }
            Act::Raise(_) | Act::Bet(_) => {
                self.raised_mask |= 1 << seat;
            }
            _ => {}
        }
    }
}

/// Whether a preflop raise is legal at this node for this seat.
pub fn preflop_can_raise(r: &Round, seat: usize) -> bool {
    r.raises < PREFLOP_MAX_RAISES && r.stacks[seat] + r.put[seat] > r.current + EPS
}

/// Clamps a target raise-to: at least a legal min raise, at most all-in, and
/// shoves when the raise would commit nearly the whole stack.
pub fn clamp_preflop_raise(r: &Round, seat: usize, target: f64) -> f64 {
    let max = r.stacks[seat] + r.put[seat];
    let min = r.current + r.last_raise;
    let to = target.max(min);
    if to > 0.45 * max {
        max
    } else {
        to
    }
}

/// Size classes whose amount matches `to`, or the single nearest class if none
/// does (a size from outside the model's classes).
fn classes_matching(candidates: [f64; 3], to: f64) -> u8 {
    let mut mask = 0u8;
    for (c, x) in candidates.iter().enumerate() {
        if (x - to).abs() < 1e-3 {
            mask |= 1 << c;
        }
    }
    if mask == 0 {
        let mut best = 0;
        for c in 1..3 {
            if (candidates[c] - to).abs() < (candidates[best] - to).abs() {
                best = c;
            }
        }
        mask = 1 << best;
    }
    mask
}

pub fn open_mask(r: &Round, seat: usize, to: f64) -> u8 {
    let max = r.stacks[seat] + r.put[seat];
    let at = |c: usize| clamp_preflop_raise(r, seat, open_target(r, c)).min(max);
    classes_matching([at(0), at(1), at(2)], to)
}

pub fn bet_mask(r: &Round, seat: usize, to: f64) -> u8 {
    classes_matching([r.bet_to(seat, SIZE_FRACS[0]), r.bet_to(seat, SIZE_FRACS[1]), r.bet_to(seat, SIZE_FRACS[2])], to)
}

/// Observed action of one villain, enough to reproduce likelihoods under any model.
#[derive(Clone, Copy, Debug)]
enum Obs {
    Pre { ctx: PreflopCtx, cols: [bool; 3] },
    Post { street: usize, mw: usize, rl: usize, sit: usize, cols: [bool; 3] },
}

pub struct Replayed {
    /// State at the decision; `next` is hero.
    pub round: Round,
    /// Active villains by seat, with observed actions and observed sizes.
    active: Vec<(usize, Vec<Obs>, Vec<(u8, u8)>)>,
    /// Villains who have folded, with their observed actions and sizes. Used
    /// only for the population posterior of the opponent mixture.
    folded: Vec<(usize, Vec<Obs>, Vec<(u8, u8)>)>,
    /// Classes on the flop, turn and river as dealt so far.
    classes: [Vec<u8>; 3],
    pub pot_type: u8,
    pub last_aggressor: Option<usize>,
}

impl Spot {
    pub fn replay(&self) -> Replayed {
        let mut r = Round::preflop(self.n, self.stack);
        let mut track = PreflopTrack::default();
        let mut obs: Vec<Vec<Obs>> = vec![Vec::new(); self.n];
        let mut sizes: Vec<Vec<(u8, u8)>> = vec![Vec::new(); self.n];
        let root_street: u8 = match self.board.len() {
            3 => 1,
            4 => 2,
            5 => 3,
            _ => panic!("spot board must have 3, 4 or 5 cards"),
        };
        let mut raises_pre = 0u8;
        let mut last_aggressor = None;
        for a in &self.history {
            assert!(a.street <= root_street, "history runs past the spot's board");
            while r.street < a.street {
                r = r.next_street();
            }
            let seat = a.seat as usize;
            debug_assert_eq!(r.next as usize, seat, "history out of order");
            if a.street == 0 {
                let can_raise = preflop_can_raise(&r, seat);
                let cols = match a.act {
                    Act::Fold => [true, false, false],
                    Act::Check => [false, true, false],
                    Act::Call if can_raise => [false, true, false],
                    Act::Call => [false, true, true],
                    Act::Raise(_) | Act::Bet(_) => [false, false, true],
                };
                obs[seat].push(Obs::Pre { ctx: track.ctx(&r, seat, self.n), cols });
                if let (Act::Raise(to), 0) = (a.act, r.raises) {
                    sizes[seat].push((0, open_mask(&r, seat, to)));
                }
                track.note(&r, seat, a.act);
                if matches!(a.act, Act::Raise(_)) {
                    raises_pre += 1;
                }
            } else {
                let facing = r.is_facing(seat);
                let raise_to_ok = r.raises < 2 && r.raise_to_3x(seat).is_some();
                let idx = post_action_index(a.act, facing);
                let mut cols = [false; 3];
                cols[idx] = true;
                if facing && idx == 1 && !raise_to_ok {
                    cols[2] = true;
                }
                let rl = usize::from(r.raises).min(2);
                let mw = (r.other_active(seat) - 1).min(2);
                let sit = r.situation(seat);
                let street = a.street as usize - 1;
                obs[seat].push(Obs::Post { street, mw, rl, sit, cols });
                if let Act::Bet(to) = a.act {
                    sizes[seat].push((a.street, bet_mask(&r, seat, to)));
                }
            }
            if seat != self.hero_seat && matches!(a.act, Act::Bet(_) | Act::Raise(_)) {
                last_aggressor = Some(seat);
            }
            r.apply(seat, a.act);
        }
        while r.street < root_street {
            r = r.next_street();
        }
        assert!(r.in_hand >> self.hero_seat & 1 == 1, "hero is not in the hand at the spot");
        assert_eq!(r.next as usize, self.hero_seat, "hero must be next to act at the spot");

        let mut active = Vec::new();
        let mut folded = Vec::new();
        for (seat, (o, z)) in obs.into_iter().zip(sizes).enumerate() {
            if seat == self.hero_seat {
                continue;
            }
            if r.in_hand >> seat & 1 == 1 {
                active.push((seat, o, z));
            } else if !o.is_empty() {
                folded.push((seat, o, z));
            }
        }
        let classes = [
            classify(&self.board[..3], true),
            if self.board.len() >= 4 { classify(&self.board[..4], true) } else { Vec::new() },
            if self.board.len() >= 5 { classify(&self.board, false) } else { Vec::new() },
        ];
        Replayed {
            round: r,
            active,
            folded,
            classes,
            pot_type: raises_pre.min(2),
            last_aggressor: last_aggressor.filter(|&s| r.in_hand >> s & 1 == 1),
        }
    }
}

impl Replayed {
    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    pub fn active_seats(&self) -> Vec<usize> {
        self.active.iter().map(|x| x.0).collect()
    }

    pub fn belief_of(&self, spot: &Spot, seat: usize, model: &PlayerModel) -> Vec<f32> {
        let (_, obs, _) = self.active.iter().find(|x| x.0 == seat).expect("seat not active");
        let mut b = vec![0.0; NCOMBOS];
        self.belief_into(spot, obs, model, &mut b);
        b
    }

    /// Per-class action likelihoods of a villain's observed actions.
    fn likelihoods(obs: &[Obs], model: &PlayerModel) -> ([f32; 169], [[f32; NCLASS]; 3]) {
        let mut pre = [1f32; 169];
        let mut post = [[1f32; NCLASS]; 3];
        for o in obs {
            match o {
                Obs::Pre { ctx, cols } => {
                    let row = model.preflop_row(ctx);
                    for c in 0..169 {
                        pre[c] *= col_sum(&row[c], cols);
                    }
                }
                Obs::Post { street, mw, rl, sit, cols } => {
                    let row = model.post_row(*street, *mw, *rl, *sit);
                    for c in 0..NCLASS {
                        post[*street][c] *= col_sum(&row[c], cols);
                    }
                }
            }
        }
        (pre, post)
    }

    /// Replays a villain's history under `model` into `out`, normalized. Returns
    /// the log-likelihood of the observed actions, marginal over its holding.
    fn belief_into(&self, spot: &Spot, obs: &[Obs], model: &PlayerModel, out: &mut [f32]) -> f64 {
        let (pre, post) = Self::likelihoods(obs, model);
        let table = combos();
        let dead = spot.hero[0].mask() | spot.hero[1].mask() | spot.board.iter().fold(0u64, |m, c| m | c.mask());
        let mut sum = 0f64;
        let mut live = 0u32;
        for i in 0..NCOMBOS {
            if table.masks[i] & dead != 0 {
                out[i] = 0.0;
                continue;
            }
            live += 1;
            let mut w = pre[usize::from(table.class169[i])];
            for s in 0..3 {
                let cls = &self.classes[s];
                if !cls.is_empty() {
                    debug_assert!(cls[i] != DEAD, "live combo must have a class");
                    w *= post[s][usize::from(cls[i])];
                }
            }
            out[i] = w;
            sum += f64::from(w);
        }
        if sum > 0.0 {
            let s = sum as f32;
            for x in out.iter_mut() {
                *x /= s;
            }
        }
        (sum / f64::from(live)).ln()
    }

    /// A river spot's belief split at the flop-to-turn transition, as the
    /// expectimax carries it: the likelihood through the flop (hero and flop
    /// cards dead), and the turn and river factor (turn and river cards dead).
    pub fn line_parts(&self, spot: &Spot, seat: usize, model: &PlayerModel) -> (Vec<f32>, Vec<f32>) {
        assert_eq!(spot.board.len(), 5, "line parts need a river spot");
        let (_, obs, _) = self.active.iter().find(|x| x.0 == seat).expect("seat not active");
        let (pre, post) = Self::likelihoods(obs, model);
        let table = combos();
        let early_dead = spot.hero[0].mask() | spot.hero[1].mask() | spot.board[..3].iter().fold(0u64, |m, c| m | c.mask());
        let late_dead = spot.board[3].mask() | spot.board[4].mask();
        let (mut early, mut late) = (vec![0f32; NCOMBOS], vec![0f32; NCOMBOS]);
        for i in 0..NCOMBOS {
            if table.masks[i] & early_dead != 0 {
                continue;
            }
            early[i] = pre[usize::from(table.class169[i])] * post[0][usize::from(self.classes[0][i])];
            if table.masks[i] & late_dead == 0 {
                late[i] = post[1][usize::from(self.classes[1][i])] * post[2][usize::from(self.classes[2][i])];
            }
        }
        (early, late)
    }
}

fn col_sum(row: &[f32; 3], cols: &[bool; 3]) -> f32 {
    let mut s = 0.0;
    for i in 0..3 {
        if cols[i] {
            s += row[i];
        }
    }
    s
}

#[derive(Clone, Debug)]
pub struct GradeCfg {
    pub tables: usize,
    /// Pool size per seat before likelihood resampling.
    pub pool: usize,
    /// (turn cards, rivers per turn) for a flop root, indexed by villains left.
    pub flop_samples: [(usize, usize); 7],
    /// Rivers sampled for a turn root, indexed by villains left.
    pub turn_samples: [usize; 7],
    pub solve: SolveCfg,
    /// None enumerates all runouts. Some keeps sampling.
    pub sample_runouts: bool,
    /// Force this archetype on every seat, for direction tests.
    pub force_archetype: Option<usize>,
    /// Use noise-free archetype models, for direction tests.
    pub exact_models: bool,
    /// Hero commits to one action per information state across tables
    /// (`joint.rs`). Off means hero best-responds per table, with hindsight.
    pub joint: bool,
    /// Treat the opponent pool as uncertain: each table is drawn 50/50 from the
    /// base population and this recalibrated one. `None` is the base population.
    pub mixture: Option<&'static crate::preflop_cal::Calibration>,
}

impl GradeCfg {
    pub fn standard() -> GradeCfg {
        GradeCfg {
            tables: 64,
            pool: 128,
            flop_samples: [(0, 0), (47, 20), (47, 20), (47, 10), (16, 6), (16, 6), (16, 6)],
            turn_samples: [0, 46, 46, 46, 46, 46, 46],
            solve: SolveCfg { reach_eps: 1e-6, line_decorrelate: true },
            sample_runouts: true,
            force_archetype: None,
            exact_models: false,
            joint: true,
            mixture: crate::preflop_cal::active(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
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
}

#[derive(Clone, Debug)]
pub struct OptionResult {
    pub label: String,
    pub act: Act,
    pub mean_ev: f64,
    pub pct_pot: f64,
    pub best_share: f64,
}

pub struct VerdictStats {
    pub mean: Vec<f64>,
    pub wins: Vec<f64>,
    pub best: usize,
    pub gap_pct: f64,
    pub verdict: Verdict,
}

/// Mean EV, best-share, best option and verdict from per-table EVs.
pub fn verdict_stats(ev_by_table: &[Vec<f64>], pot: f64) -> VerdictStats {
    let t = ev_by_table.len();
    let m = ev_by_table[0].len();
    let mut mean = vec![0.0; m];
    let mut wins = vec![0.0; m];
    for row in ev_by_table {
        let mut best = 0;
        for i in 0..m {
            mean[i] += row[i];
            if row[i] > row[best] + 1e-9 {
                best = i;
            }
        }
        wins[best] += 1.0;
    }
    for i in 0..m {
        mean[i] /= t as f64;
        wins[i] /= t as f64;
    }
    let mut order: Vec<usize> = (0..m).collect();
    order.sort_by(|&a, &b| mean[b].partial_cmp(&mean[a]).unwrap());
    let best = order[0];
    let gap = if m > 1 { mean[best] - mean[order[1]] } else { f64::INFINITY };
    let gap_pct = if m > 1 { 100.0 * gap / pot } else { 100.0 };
    let verdict = if wins[best] >= 0.90 && gap >= 0.01 * pot {
        Verdict::Robust
    } else if gap < 0.01 * pot {
        Verdict::Close
    } else {
        Verdict::Depends
    };
    VerdictStats { mean, wins, best, gap_pct, verdict }
}

/// Per-archetype results for the main opponent.
#[derive(Clone, Debug)]
pub struct ArchRow {
    pub archetype: usize,
    pub weight: f64,
    pub best_option: String,
    pub mean_ev: Vec<f64>,
}

#[derive(Clone, Debug)]
pub struct Grade {
    pub pot: f64,
    pub to_call: f64,
    pub price_pct: f64,
    pub options: Vec<OptionResult>,
    pub verdict: Verdict,
    pub best: usize,
    pub gap_pct: f64,
    pub tables: usize,
    pub main_opp: Option<usize>,
    pub breakdown: Vec<ArchRow>,
    pub ess: Vec<f64>,
    pub ev_by_table: Vec<Vec<f64>>,
}

fn systematic_resample(weights: &[f64], rng: &mut Rng, out_n: usize) -> Vec<usize> {
    let total: f64 = weights.iter().sum();
    if total <= 0.0 {
        return (0..out_n).map(|i| i % weights.len()).collect();
    }
    let step = total / out_n as f64;
    let mut u = (rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64 * step;
    let mut idx = Vec::with_capacity(out_n);
    let mut acc = weights[0];
    let mut i = 0;
    for _ in 0..out_n {
        while u > acc && i + 1 < weights.len() {
            i += 1;
            acc += weights[i];
        }
        idx.push(i);
        u += step;
    }
    idx
}

pub struct SeatPool {
    seat: usize,
    models: Vec<PlayerModel>,
    beliefs: Vec<Vec<f32>>,
    tables: Vec<Option<VillainTables>>,
    chosen: Vec<usize>,
    ess: f64,
}

impl SeatPool {
    /// Belief vector of the resampled model at table index `t`.
    pub fn chosen_belief(&self, t: usize) -> &[f32] {
        &self.beliefs[self.chosen[t]]
    }

    pub fn seat(&self) -> usize {
        self.seat
    }

    pub fn chosen_model(&self, t: usize) -> &PlayerModel {
        &self.models[self.chosen[t]]
    }

    pub fn ess(&self) -> f64 {
        self.ess
    }
}

/// A table's root beliefs: as replayed (`base`) and with the villain-to-villain
/// card-removal correction (`corrected`). The tree starts from the corrected
/// beliefs and later corrections start again from `base`.
pub fn table_beliefs(pools: &[SeatPool], t: usize) -> (Vec<Vec<f32>>, Vec<Vec<f32>>) {
    let base: Vec<Vec<f32>> = pools.iter().map(|p| p.chosen_belief(t).to_vec()).collect();
    let mut corrected = base.clone();
    crate::solve::decorrelate(&mut corrected, 2);
    (base, corrected)
}

pub fn villains_for<'a>(pools: &'a [SeatPool], t: usize, base: &'a [Vec<f32>], corrected: &'a [Vec<f32>]) -> Vec<Villain<'a>> {
    pools
        .iter()
        .zip(base.iter().zip(corrected))
        .map(|(p, (b, c))| Villain {
            seat: p.seat,
            base: b,
            belief: c,
            tables: p.tables[p.chosen[t]].as_ref().expect("chosen model has tables"),
        })
        .collect()
}

/// Samples each active seat's model pool and resamples by likelihood.
pub fn seat_posteriors(spot: &Spot, rp: &Replayed, cfg: &GradeCfg, rng: &mut Rng) -> Vec<SeatPool> {
    if let (Some(cal), None) = (cfg.mixture, cfg.force_archetype) {
        return mixture_posteriors(spot, rp, cfg, cal, rng);
    }
    let mut pools = Vec::new();
    for (seat, obs, sizes) in &rp.active {
        let mut seat_rng = Rng::new(rng.next_u64());
        let mut models = Vec::with_capacity(cfg.pool);
        let mut logw = Vec::with_capacity(cfg.pool);
        let mut beliefs: Vec<Vec<f32>> = Vec::with_capacity(cfg.pool);
        for _ in 0..cfg.pool {
            let model = if let Some(a) = cfg.force_archetype {
                if cfg.exact_models { PlayerModel::exact(a) } else { PlayerModel::sample_of(&mut seat_rng, a) }
            } else {
                PlayerModel::sample(&mut seat_rng)
            };
            let mut model = model;
            // Observed sizes fix the classes on those streets and weight the model.
            let log_size = model.constrain_sizes(sizes, &mut seat_rng);
            let mut b = vec![0f32; NCOMBOS];
            let ll = rp.belief_into(spot, obs, &model, &mut b) + log_size;
            models.push(model);
            beliefs.push(b);
            logw.push(ll);
        }
        let max_ll = logw.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let weights: Vec<f64> = logw.iter().map(|l| (l - max_ll).exp()).collect();
        let sum: f64 = weights.iter().sum();
        let sum2: f64 = weights.iter().map(|w| w * w).sum();
        let ess = if sum2 > 0.0 { sum * sum / sum2 } else { 0.0 };
        let mut chosen = systematic_resample(&weights, &mut seat_rng, cfg.tables);
        seat_rng.shuffle(&mut chosen);
        // Only resampled models are ever solved against.
        let mut needed = vec![false; models.len()];
        for &i in &chosen {
            needed[i] = true;
        }
        let tables: Vec<Option<VillainTables>> =
            models.iter().enumerate().map(|(i, m)| needed[i].then(|| m.tables())).collect();
        pools.push(SeatPool { seat: *seat, models, beliefs, tables, chosen, ess });
    }
    pools
}

/// Log of the mean of `exp(l)` over a slice, computed without underflow.
fn log_mean(ls: &[f64]) -> f64 {
    let max = ls.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    if !max.is_finite() {
        return max;
    }
    max + (ls.iter().map(|l| (l - max).exp()).sum::<f64>() / ls.len() as f64).ln()
}

/// As `seat_posteriors`, for an uncertain opponent pool. A whole table comes
/// from one population: the two start equally likely, and the observed actions
/// of all seats at the table, including those who folded, update which one this
/// table is, so table counts per population follow that posterior. Within a
/// population, each active seat is resampled by likelihood as before, with
/// `cfg.pool` models from each population. Returns the pools and the posterior
/// probability of the recalibrated population.
fn mixture_posteriors(spot: &Spot, rp: &Replayed, cfg: &GradeCfg, cal: &'static crate::preflop_cal::Calibration, rng: &mut Rng) -> Vec<SeatPool> {
    mixture_detail(spot, rp, cfg, cal, rng).0
}

/// The posterior probability that a table comes from the recalibrated
/// population, given all the actions seen at that table.
pub fn population_posterior(spot: &Spot, rp: &Replayed, cfg: &GradeCfg, cal: &'static crate::preflop_cal::Calibration, rng: &mut Rng) -> f64 {
    mixture_detail(spot, rp, cfg, cal, rng).1
}

fn mixture_detail(spot: &Spot, rp: &Replayed, cfg: &GradeCfg, cal: &'static crate::preflop_cal::Calibration, rng: &mut Rng) -> (Vec<SeatPool>, f64) {
    struct SeatDraw {
        seat: usize,
        models: Vec<PlayerModel>,
        beliefs: Vec<Vec<f32>>,
        logw: Vec<f64>,
    }
    let mut draws = Vec::new();
    let mut log_marg = [0.0f64; 2];
    for (seat, obs, sizes) in &rp.active {
        let mut seat_rng = Rng::new(rng.next_u64());
        let (mut models, mut beliefs, mut logw) = (Vec::new(), Vec::new(), Vec::new());
        for (pop, marg) in log_marg.iter_mut().enumerate() {
            let start = logw.len();
            for _ in 0..cfg.pool {
                let mut model = if pop == 0 { PlayerModel::sample(&mut seat_rng) } else { PlayerModel::sample_cal(&mut seat_rng, cal) };
                let log_size = model.constrain_sizes(sizes, &mut seat_rng);
                let mut b = vec![0f32; NCOMBOS];
                let ll = rp.belief_into(spot, obs, &model, &mut b) + log_size;
                models.push(model);
                beliefs.push(b);
                logw.push(ll);
            }
            *marg += log_mean(&logw[start..]);
        }
        draws.push(SeatDraw { seat: *seat, models, beliefs, logw });
    }
    // Folded seats inform only the population. They use a copy of the stream so
    // the active seats' draws below don't depend on how many seats folded.
    let mut fold_rng = rng.clone();
    let mut scratch = vec![0f32; NCOMBOS];
    for (_, obs, sizes) in &rp.folded {
        let mut seat_rng = Rng::new(fold_rng.next_u64());
        for (pop, marg) in log_marg.iter_mut().enumerate() {
            let mut lls = Vec::with_capacity(cfg.pool);
            for _ in 0..cfg.pool {
                let mut model = if pop == 0 { PlayerModel::sample(&mut seat_rng) } else { PlayerModel::sample_cal(&mut seat_rng, cal) };
                let log_size = model.constrain_sizes(sizes, &mut seat_rng);
                lls.push(rp.belief_into(spot, obs, &model, &mut scratch) + log_size);
            }
            *marg += log_mean(&lls);
        }
    }
    // Population of each table: equal prior, posterior from all seats' evidence.
    let p1 = 1.0 / (1.0 + (log_marg[0] - log_marg[1]).exp());
    let u = (rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
    let n1 = ((p1 * cfg.tables as f64 + u).floor() as usize).min(cfg.tables);
    let mut table_pop: Vec<usize> = (0..cfg.tables).map(|t| usize::from(t < n1)).collect();
    rng.shuffle(&mut table_pop);
    let mut pools = Vec::new();
    for d in draws {
        let mut seat_rng = Rng::new(rng.next_u64());
        let pool = cfg.pool;
        let mut picks: [Vec<usize>; 2] = [Vec::new(), Vec::new()];
        let mut ess_weights = Vec::with_capacity(2 * pool);
        for (pop, pick) in picks.iter_mut().enumerate() {
            let (a, b) = (pop * pool, (pop + 1) * pool);
            let max_ll = d.logw[a..b].iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let weights: Vec<f64> = d.logw[a..b].iter().map(|l| (l - max_ll).exp()).collect();
            let count = table_pop.iter().filter(|&&x| x == pop).count();
            let sum: f64 = weights.iter().sum();
            let share = count as f64 / cfg.tables as f64;
            ess_weights.extend(weights.iter().map(|w| w / sum.max(1e-300) * share));
            let mut chosen = systematic_resample(&weights, &mut seat_rng, count);
            seat_rng.shuffle(&mut chosen);
            pick.extend(chosen.into_iter().map(|i| i + a));
        }
        let sum2: f64 = ess_weights.iter().map(|w| w * w).sum();
        let ess = if sum2 > 0.0 { 1.0 / sum2 } else { 0.0 };
        let mut next = [0usize; 2];
        let chosen: Vec<usize> = table_pop
            .iter()
            .map(|&pop| {
                let v = picks[pop][next[pop]];
                next[pop] += 1;
                v
            })
            .collect();
        let mut needed = vec![false; d.models.len()];
        for &i in &chosen {
            needed[i] = true;
        }
        let tables: Vec<Option<VillainTables>> = d.models.iter().enumerate().map(|(i, m)| needed[i].then(|| m.tables())).collect();
        pools.push(SeatPool { seat: d.seat, models: d.models, beliefs: d.beliefs, tables, chosen, ess });
    }
    (pools, p1)
}

/// Grades a spot. Deterministic in `seed`; all tables share runouts.
pub fn grade_spot(spot: &Spot, cfg: &GradeCfg, seed: u64) -> Grade {
    let mut rng = Rng::new(seed);
    let rp = prof::span(prof::BUCKETS, || spot.replay());
    let options = rp.round.root_options(spot.hero_seat);
    let batch = grade_batch(spot, &rp, cfg, &options, &mut rng);
    summarize(spot, &rp, &options, batch.ev_by_table, batch.main_archetype, batch.ess)
}

/// Per-table results of one batch of tables with its own posterior draws and runouts.
struct Batch {
    ev_by_table: Vec<Vec<f64>>,
    main_archetype: Vec<usize>,
    ess: Vec<f64>,
}

fn grade_batch(spot: &Spot, rp: &Replayed, cfg: &GradeCfg, options: &[(Act, String)], rng: &mut Rng) -> Batch {
    let round = rp.round;
    let nv = rp.active_count().min(6);
    let pools = prof::span(prof::POOL, || seat_posteriors(spot, rp, cfg, rng));
    let runouts = prof::span(prof::BUCKETS, || build_runouts(spot, cfg, nv, rng));
    let acts: Vec<Act> = options.iter().map(|x| x.0).collect();
    let t = cfg.tables;
    let ev_by_table: Vec<Vec<f64>> = if cfg.joint {
        let beliefs: Vec<(Vec<Vec<f32>>, Vec<Vec<f32>>)> = (0..t).map(|table| table_beliefs(&pools, table)).collect();
        let tables: Vec<Vec<Villain>> = (0..t).map(|table| villains_for(&pools, table, &beliefs[table].0, &beliefs[table].1)).collect();
        crate::joint::solve_joint_fast(&round, spot.hero_seat, &tables, &runouts, cfg.solve, &acts)
    } else {
        per_table_evs(&round, spot, cfg, &runouts, &acts, &pools)
    };
    let main_seat = rp.last_aggressor.or_else(|| rp.active.first().map(|x| x.0));
    let main_archetype = match main_seat.and_then(|m| pools.iter().find(|p| p.seat == m)) {
        Some(pool) => (0..t).map(|table| pool.chosen_model(table).archetype).collect(),
        None => Vec::new(),
    };
    Batch { ev_by_table, main_archetype, ess: pools.iter().map(|p| p.ess).collect() }
}

/// The result of grading a spot in two independent halves.
pub struct SplitGrade {
    /// Pooled over both halves; this is what the library reports.
    pub pooled: Grade,
    /// Best option and verdict of each half alone.
    pub halves: [(usize, Verdict); 2],
    /// Both halves agree on the best option and the verdict.
    pub agree: bool,
    /// The pooled result also matches the halves.
    pub pool_matches: bool,
}

/// Grades a spot as two independent halves, each `batches_per_half` batches of
/// `cfg.tables` tables with their own posterior draws and runout samples.
/// Hero's committed continuation (the joint solve) is per batch.
pub fn grade_split(spot: &Spot, cfg: &GradeCfg, batches_per_half: usize, seed: u64) -> SplitGrade {
    let mut master = Rng::new(seed);
    let rp = prof::span(prof::BUCKETS, || spot.replay());
    let options = rp.round.root_options(spot.hero_seat);
    let pot = rp.round.pot;
    let (mut all_ev, mut all_arch, mut ess) = (Vec::new(), Vec::new(), Vec::new());
    let mut halves = [(0, Verdict::Close); 2];
    for half in &mut halves {
        let mut half_ev: Vec<Vec<f64>> = Vec::new();
        for _ in 0..batches_per_half {
            let mut rng = Rng::new(master.next_u64());
            let b = grade_batch(spot, &rp, cfg, &options, &mut rng);
            half_ev.extend(b.ev_by_table);
            all_arch.extend(b.main_archetype);
            ess.extend(b.ess);
        }
        let s = verdict_stats(&half_ev, pot);
        *half = (s.best, s.verdict);
        all_ev.extend(half_ev);
    }
    let pooled = summarize(spot, &rp, &options, all_ev, all_arch, ess);
    let agree = halves[0] == halves[1];
    let pool_matches = agree && halves[0].0 == pooled.best && halves[0].1 == pooled.verdict;
    SplitGrade { pooled, halves, agree, pool_matches }
}

/// Each table solved separately, with hero knowing which table it is on.
fn per_table_evs(round: &Round, spot: &Spot, cfg: &GradeCfg, runouts: &Runouts, acts: &[Act], pools: &[SeatPool]) -> Vec<Vec<f64>> {
    let t = cfg.tables;
    let mut ev_by_table: Vec<Vec<f64>> = Vec::with_capacity(t);
    for table in 0..t {
        let (base, corrected) = table_beliefs(pools, table);
        let villains = villains_for(pools, table, &base, &corrected);
        ev_by_table.push(solve_root(round, spot.hero_seat, &villains, runouts, cfg.solve, acts));
    }
    ev_by_table
}

pub fn build_runouts(spot: &Spot, cfg: &GradeCfg, nv: usize, rng: &mut Rng) -> Runouts {
    let (k, j) = cfg.flop_samples[nv];
    let rivers = cfg.turn_samples[nv];
    let (turn_sample, river_sample) = if !cfg.sample_runouts {
        (None, None)
    } else if spot.board.len() == 4 {
        (None, Some(rivers))
    } else {
        (Some(k), Some(j))
    };
    let mut pick_rng = Rng::new(rng.next_u64());
    Runouts::build(spot.hero, &spot.board, turn_sample, river_sample, |deck, k| {
        let mut d = deck.to_vec();
        pick_rng.shuffle(&mut d);
        d.truncate(k);
        d
    })
}

fn summarize(spot: &Spot, rp: &Replayed, options: &[(Act, String)], ev_by_table: Vec<Vec<f64>>, main_archetype: Vec<usize>, ess: Vec<f64>) -> Grade {
    let round = rp.round;
    let t = ev_by_table.len();
    let m = options.len();
    let pot = round.pot;
    let VerdictStats { mean, wins, best, gap_pct, verdict } = verdict_stats(&ev_by_table, pot);
    let to_call = round.call_amount(spot.hero_seat);
    let price_pct = if to_call > EPS { 100.0 * to_call / (pot + to_call) } else { 0.0 };
    let option_results: Vec<OptionResult> = options
        .iter()
        .enumerate()
        .map(|(i, (act, label))| OptionResult {
            label: label.clone(),
            act: *act,
            mean_ev: mean[i],
            pct_pot: 100.0 * mean[i] / pot,
            best_share: wins[i],
        })
        .collect();

    // Main opponent: the last aggressor if still in, otherwise first active villain.
    let main_seat = rp.last_aggressor.or_else(|| rp.active.first().map(|x| x.0));
    let mut breakdown = Vec::new();
    if main_seat.is_some() && !main_archetype.is_empty() {
        {
            let mut sums: Vec<(usize, f64, Vec<f64>)> = Vec::new();
            for (table, row) in ev_by_table.iter().enumerate() {
                let arch = main_archetype[table];
                let entry = match sums.iter().position(|e| e.0 == arch) {
                    Some(p) => p,
                    None => {
                        sums.push((arch, 0.0, vec![0.0; m]));
                        sums.len() - 1
                    }
                };
                sums[entry].1 += 1.0;
                for i in 0..m {
                    sums[entry].2[i] += row[i];
                }
            }
            sums.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
            for (arch, count, ev) in sums {
                let means: Vec<f64> = ev.iter().map(|x| x / count).collect();
                let mut bo = 0;
                for i in 0..m {
                    if means[i] > means[bo] {
                        bo = i;
                    }
                }
                breakdown.push(ArchRow {
                    archetype: arch,
                    weight: count / t as f64,
                    best_option: options[bo].1.clone(),
                    mean_ev: means,
                });
            }
        }
    }
    Grade {
        pot,
        to_call,
        price_pct,
        options: option_results,
        verdict,
        best,
        gap_pct,
        tables: t,
        main_opp: main_seat,
        breakdown,
        ess,
        ev_by_table,
    }
}
