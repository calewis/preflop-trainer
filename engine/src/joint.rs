//! Joint expectimax: hero commits to one action per public information state,
//! across every table that reaches it. Tables separate only where a villain's
//! visible action (and so its bet size) differs. This removes the hindsight
//! ("strategy fusion") in the per-table solve in `solve.rs`, where hero would
//! see which table it is on when choosing later streets.
//!
//! Same arithmetic per table as `solve.rs`; a table's state lives in arrays
//! indexed by table id and sets of tables are bitmasks, so at most 64 tables.

use crate::board::{Runouts, DEAD};
use crate::combos::NCOMBOS;
use crate::game::{Act, Round};
use crate::models::{NBUCKET, NCLASS};
use crate::solve::{add_masses, build_turn, decorrelate, fill_river_from_belief, hero_share, Masses, RiverFrame, SolveCfg, TurnFrame, Villain, BRANCH_FLOOR, MAX_V};
use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};

pub const MAX_T: usize = 64;

type Vals = [f64; MAX_T];
type MArr = [[f32; NCLASS]; MAX_T];
type RArr = [[f32; NBUCKET]; MAX_T];

const UNIT_M: MArr = [[1.0; NCLASS]; MAX_T];
const UNIT_R: RArr = [[1.0; NBUCKET]; MAX_T];

#[inline]
fn bits(mut s: u64) -> impl Iterator<Item = usize> {
    std::iter::from_fn(move || {
        if s == 0 {
            None
        } else {
            let t = s.trailing_zeros() as usize;
            s &= s - 1;
            Some(t)
        }
    })
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Terminal,
    Showdown,
    Hero,
    Villain,
}

const MAX_G: usize = 8;

/// Memory bounds on the compiled-tree cache per thread. Trees are only reused
/// to save compile time, so clearing is safe whenever these are reached.
const TAPE_NODE_CAP: usize = 500_000;
const TAPE_ACT_CAP: usize = 2_000_000;

/// Children of a node: one per public action. `mask` is the tables that can
/// take this child; a child of a bet-size-specific action holds fewer tables.
#[derive(Clone, Copy)]
struct Group {
    col: u8,
    child: u32,
    mask: u64,
}

#[derive(Clone, Copy)]
struct Node {
    kind: Kind,
    v: u8,
    facing: bool,
    active: u8,
    n: u8,
    value: f64,
    pot: f64,
    groups: [Group; MAX_G],
    /// Tables that can reach this node (villain nodes).
    set: u64,
    /// Villain node: start of its actions, one entry per table in `set`.
    acts_off: u32,
}

impl Node {
    fn terminal(value: f64) -> Node {
        Node {
            kind: Kind::Terminal,
            v: 0,
            facing: false,
            active: 0,
            n: 0,
            value,
            pot: 0.0,
            groups: [Group { col: 0, child: 0, mask: 0 }; MAX_G],
            set: 0,
            acts_off: 0,
        }
    }
}

static NODE_CAP: AtomicUsize = AtomicUsize::new(TAPE_NODE_CAP);
static ACT_CAP: AtomicUsize = AtomicUsize::new(TAPE_ACT_CAP);
static PEAK_NODES: AtomicUsize = AtomicUsize::new(0);
static PEAK_VILLAIN_NODES: AtomicUsize = AtomicUsize::new(0);
static PEAK_ACTS: AtomicUsize = AtomicUsize::new(0);

/// Overrides the cache bounds; used only to measure uncapped growth.
pub fn set_tape_caps(nodes: usize, acts: usize) {
    NODE_CAP.store(nodes, Ordering::Relaxed);
    ACT_CAP.store(acts, Ordering::Relaxed);
}

/// Largest single-thread cache seen: (nodes, villain nodes, action entries,
/// bytes per node, bytes per action entry).
pub fn tape_peaks() -> (usize, usize, usize, usize, usize) {
    (
        PEAK_NODES.load(Ordering::Relaxed),
        PEAK_VILLAIN_NODES.load(Ordering::Relaxed),
        PEAK_ACTS.load(Ordering::Relaxed),
        std::mem::size_of::<Node>(),
        std::mem::size_of::<[[f32; NBUCKET]; 3]>(),
    )
}

#[derive(Default)]
struct TapeCache {
    nodes: Vec<Node>,
    acts: Vec<[[f32; NBUCKET]; 3]>,
    entries: Vec<(Round, u64, u32)>,
    villain_nodes: usize,
}

impl TapeCache {
    fn clear(&mut self) {
        PEAK_NODES.fetch_max(self.nodes.len(), Ordering::Relaxed);
        PEAK_VILLAIN_NODES.fetch_max(self.villain_nodes, Ordering::Relaxed);
        PEAK_ACTS.fetch_max(self.acts.len(), Ordering::Relaxed);
        self.villain_nodes = 0;
        self.nodes.clear();
        self.acts.clear();
        self.entries.clear();
    }
}

struct Joint<'a> {
    hero: usize,
    tables: &'a [Vec<Villain<'a>>],
    runouts: &'a Runouts,
    cfg: SolveCfg,
    seat_to_v: [usize; crate::game::MAX_PLAYERS],
    tapes: RefCell<TapeCache>,
    root_mass: Vec<Masses>,
}

/// EV by table and root option, relative to hero's root stack. Hero's later
/// decisions are shared across tables in the same information state.
pub fn solve_joint_fast(root: &Round, hero: usize, tables: &[Vec<Villain>], runouts: &Runouts, cfg: SolveCfg, options: &[Act]) -> Vec<Vec<f64>> {
    assert!(tables.len() <= MAX_T, "at most {MAX_T} tables");
    let mut seat_to_v = [usize::MAX; crate::game::MAX_PLAYERS];
    for (i, v) in tables[0].iter().enumerate() {
        seat_to_v[v.seat] = i;
    }
    let root_mass: Vec<Masses> = tables
        .iter()
        .map(|vs| {
            let mut mass = Masses([[0.0; NCLASS]; MAX_V]);
            if runouts.root_street == 1 {
                for (i, v) in vs.iter().enumerate() {
                    add_masses(&mut mass.0[i], v.belief, &runouts.flop_class);
                }
            }
            mass
        })
        .collect();
    let j = Joint { hero, tables, runouts, cfg, seat_to_v, tapes: RefCell::new(TapeCache::default()), root_mass };
    let base = root.stacks[hero];
    let set: u64 = if tables.len() == 64 { u64::MAX } else { (1u64 << tables.len()) - 1 };
    let weight = 1.0 / tables.len() as f64;
    let w = [weight; MAX_T];
    let reach = [1.0; MAX_T];

    // Root turn frames are shared across root options.
    let root_turn: Vec<TurnFrame> = if runouts.root_street == 2 {
        tables
            .iter()
            .map(|vs| {
                let beliefs: Vec<&[f32]> = vs.iter().map(|v| v.belief).collect();
                build_turn(vs, &runouts.turns[0], &beliefs, root.in_hand)
            })
            .collect()
    } else {
        Vec::new()
    };
    let root_river: Vec<RiverFrame> = if runouts.root_street == 3 {
        let river = &runouts.turns[0].rivers[0];
        tables
            .iter()
            .map(|vs| {
                let mut f = RiverFrame::zero();
                for (i, v) in vs.iter().enumerate() {
                    fill_river_from_belief(&mut f, i, v.belief, &river.class, &river.outcome);
                }
                f
            })
            .collect()
    } else {
        Vec::new()
    };

    let mut out = vec![vec![0.0; options.len()]; tables.len()];
    for (o, &act) in options.iter().enumerate() {
        let mut r = *root;
        r.apply(hero, act);
        let vals = match runouts.root_street {
            1 => {
                let mass: Vec<&Masses> = j.root_mass.iter().collect();
                let next = |r: Round, set: u64, m: &[&MArr; MAX_V], w: &Vals, reach: &Vals| j.flop_to_turn(r, set, m, w, reach);
                j.betting(0, &mass, r, set, &[&UNIT_M; MAX_V], &w, &reach, &next)
            }
            2 => j.run_turn(r, set, &root_turn, &w, &reach),
            3 => {
                let mut guard = j.tapes.borrow_mut();
                let tapes = &mut *guard;
                tapes.clear();
                let tape_root = j.tape_root(tapes, r, set);
                let frames: Vec<&RiverFrame> = root_river.iter().collect();
                j.eval_river(&tapes.nodes, &tapes.acts, tape_root, set, &frames, &[&UNIT_R; MAX_V], &w, &reach)
            }
            _ => unreachable!(),
        };
        for (t, row) in out.iter_mut().enumerate() {
            row[o] = vals[t] - base;
        }
    }
    out
}

type Next<'a> = &'a dyn Fn(Round, u64, &[&MArr; MAX_V], &Vals, &Vals) -> Vals;

impl<'a> Joint<'a> {
    /// Flop or turn betting for a set of tables in the same information state.
    #[allow(clippy::too_many_arguments)]
    fn betting(&self, street: usize, mass: &[&Masses], r: Round, set: u64, m: &[&MArr; MAX_V], w: &Vals, reach: &Vals, next: Next) -> Vals {
        if r.in_hand >> self.hero & 1 == 0 {
            return [r.stacks[self.hero]; MAX_T];
        }
        if r.in_hand.count_ones() == 1 {
            return [r.stacks[self.hero] + r.pot; MAX_T];
        }
        if r.to_act == 0 {
            return next(r, set, m, w, reach);
        }
        let seat = r.next as usize;
        if seat == self.hero {
            let mut opts = [Act::Fold; 3];
            let n = r.deep_options(seat, &mut opts);
            let mut best: Option<(f64, Vals)> = None;
            for &a in &opts[..n] {
                let mut r2 = r;
                r2.apply(seat, a);
                let vals = self.betting(street, mass, r2, set, m, w, reach, next);
                let score: f64 = bits(set).map(|t| w[t] * vals[t]).sum();
                if best.as_ref().is_none_or(|(b, _)| score > *b) {
                    best = Some((score, vals));
                }
            }
            return best.expect("hero has an option").1;
        }

        let v = self.seat_to_v[seat];
        let facing = r.is_facing(seat);
        let sit = r.situation(seat);
        let rl = usize::from(r.raises).min(2);
        let mw = (r.other_active(seat) - 1).min(2);
        let raise_to = if facing && r.raises < 2 { r.raise_to_3x(seat) } else { None };
        let mut val = [0.0; MAX_T];
        let mut sum = [0.0f64; MAX_T];
        let mut groups: Vec<(Act, usize, u64, Vals)> = Vec::with_capacity(4);
        let mut fallback = 0u64;
        for t in bits(set) {
            let villain = &self.tables[t][v];
            let row = villain.tables.row(street, mw, rl, sit);
            let (ms, mults) = (&mass[t].0[v], &m[v][t]);
            let mut p = [0f32; 3];
            let mut total = 0f32;
            for c in 0..NCLASS {
                let x = ms[c] * mults[c];
                total += x;
                p[0] += x * row[c][0];
                p[1] += x * row[c][1];
                p[2] += x * row[c][2];
            }
            if total <= 0.0 {
                if facing {
                    val[t] = r.stacks[self.hero] + r.pot;
                    sum[t] = 1.0;
                } else {
                    fallback |= 1 << t;
                }
                continue;
            }
            for x in p.iter_mut() {
                *x /= total;
            }
            let call_p = if raise_to.is_some() { p[1] } else { p[1] + p[2] };
            let acts: [Option<(Act, usize, f32)>; 3] = if facing {
                [Some((Act::Fold, 0, p[0])), Some((Act::Call, 1, call_p)), raise_to.map(|to| (Act::Raise(to), 2, p[2]))]
            } else {
                let to = r.bet_to(seat, f64::from(villain.tables.bet_frac[street]));
                [Some((Act::Check, 0, p[0])), Some((Act::Bet(to), 1, p[1])), None]
            };
            let mut kept = false;
            for (act, col, pa) in acts.iter().flatten() {
                if *pa < BRANCH_FLOOR || reach[t] * f64::from(*pa) < self.cfg.reach_eps {
                    continue;
                }
                kept = true;
                match groups.iter_mut().find(|g| g.0 == *act) {
                    Some(g) => {
                        g.2 |= 1 << t;
                        g.3[t] = f64::from(*pa);
                    }
                    None => {
                        let mut ps = [0.0; MAX_T];
                        ps[t] = f64::from(*pa);
                        groups.push((*act, *col, 1 << t, ps));
                    }
                }
            }
            if !kept {
                fallback |= 1 << t;
            }
        }
        for (act, col, mask, ps) in &groups {
            let mut r2 = r;
            r2.apply(seat, *act);
            let mut child_m = *m[v];
            for t in bits(*mask) {
                let row = self.tables[t][v].tables.row(street, mw, rl, sit);
                let pr = (ps[t] as f32).max(1e-30);
                for c in 0..NCLASS {
                    let f = if *col == 1 && facing && raise_to.is_none() { row[c][1] + row[c][2] } else { row[c][*col] };
                    child_m[t][c] = m[v][t][c] * f / pr;
                }
            }
            let mut m2 = *m;
            m2[v] = &child_m;
            let (mut w2, mut reach2) = ([0.0; MAX_T], [0.0; MAX_T]);
            for t in bits(*mask) {
                w2[t] = w[t] * ps[t];
                reach2[t] = reach[t] * ps[t];
            }
            let vals = self.betting(street, mass, r2, *mask, &m2, &w2, &reach2, next);
            for t in bits(*mask) {
                val[t] += ps[t] * vals[t];
                sum[t] += ps[t];
            }
        }
        if fallback != 0 {
            let mut r2 = r;
            r2.apply(seat, if facing { Act::Call } else { Act::Check });
            let vals = self.betting(street, mass, r2, fallback, m, w, reach, next);
            for t in bits(fallback) {
                val[t] = vals[t];
                sum[t] = 1.0;
            }
        }
        for t in bits(set) {
            val[t] /= sum[t];
        }
        val
    }

    fn flop_to_turn(&self, r: Round, set: u64, m: &[&MArr; MAX_V], w: &Vals, reach: &Vals) -> Vals {
        let flop = &self.runouts.flop_class;
        let r2 = r.next_street();
        self.tapes.borrow_mut().clear();
        // Beliefs at the turn, per table, then the card-removal correction.
        let mut beliefs: Vec<Vec<Vec<f32>>> = vec![Vec::new(); self.tables.len()];
        for t in bits(set) {
            let mut per_v: Vec<Vec<f32>> = Vec::with_capacity(self.tables[t].len());
            for (i, v) in self.tables[t].iter().enumerate() {
                if r.in_hand >> v.seat & 1 == 0 {
                    per_v.push(Vec::new());
                    continue;
                }
                let mut b = vec![0f32; NCOMBOS];
                let src = if self.cfg.line_decorrelate { v.base } else { v.belief };
                for ((out, &base), &c) in b.iter_mut().zip(src).zip(flop) {
                    if base != 0.0 && c != DEAD {
                        *out = base * m[i][t][c as usize];
                    }
                }
                per_v.push(b);
            }
            let active: Vec<usize> = (0..per_v.len()).filter(|&i| !per_v[i].is_empty()).collect();
            if self.cfg.line_decorrelate && active.len() >= 2 {
                let mut vecs: Vec<Vec<f32>> = active.iter().map(|&i| std::mem::take(&mut per_v[i])).collect();
                decorrelate(&mut vecs, 2);
                for (s, &i) in active.iter().enumerate() {
                    per_v[i] = std::mem::take(&mut vecs[s]);
                }
            }
            beliefs[t] = per_v;
        }
        // Probability of each turn card per table, before any frame is built.
        let nt = self.runouts.turns.len();
        let mut card_w = vec![[0f64; MAX_T]; nt];
        let mut totals = [0.0f64; MAX_T];
        for (k, turn) in self.runouts.turns.iter().enumerate() {
            for t in bits(set) {
                let mut product = 1.0f64;
                for (v, b) in beliefs[t].iter().enumerate() {
                    if r.in_hand >> self.tables[t][v].seat & 1 == 0 {
                        continue;
                    }
                    let mut total = 0f32;
                    for (&x, &c) in b.iter().zip(&turn.class) {
                        if c != DEAD {
                            total += x;
                        }
                    }
                    product *= f64::from(total);
                }
                card_w[k][t] = product;
                totals[t] += product;
            }
        }
        let mut val = [0.0; MAX_T];
        for (k, turn) in self.runouts.turns.iter().enumerate() {
            let mut sub = 0u64;
            let mut frames: Vec<Option<TurnFrame>> = (0..self.tables.len()).map(|_| None).collect();
            let (mut w2, mut frac) = ([0.0; MAX_T], [0.0; MAX_T]);
            for t in bits(set) {
                if card_w[k][t] <= 0.0 {
                    continue;
                }
                let refs: Vec<&[f32]> = beliefs[t].iter().map(|b| b.as_slice()).collect();
                frames[t] = Some(build_turn(&self.tables[t], turn, &refs, r2.in_hand));
                sub |= 1 << t;
                frac[t] = card_w[k][t] / totals[t];
                w2[t] = w[t] * frac[t];
            }
            if sub == 0 {
                continue;
            }
            let frames_ref: Vec<Option<&TurnFrame>> = frames.iter().map(|f| f.as_ref()).collect();
            let vals = self.run_turn_opt(r2, sub, &frames_ref, &w2, reach);
            for t in bits(sub) {
                val[t] += frac[t] * vals[t];
            }
        }
        for t in bits(set) {
            if totals[t] <= 0.0 {
                val[t] = r2.stacks[self.hero];
            }
        }
        val
    }

    fn run_turn(&self, r: Round, set: u64, frames: &[TurnFrame], w: &Vals, reach: &Vals) -> Vals {
        let refs: Vec<Option<&TurnFrame>> = frames.iter().map(Some).collect();
        self.tapes.borrow_mut().clear();
        self.run_turn_opt(r, set, &refs, w, reach)
    }

    fn run_turn_opt(&self, r: Round, set: u64, frames: &[Option<&TurnFrame>], w: &Vals, reach: &Vals) -> Vals {
        static ZERO: Masses = Masses([[0.0; NCLASS]; MAX_V]);
        let mass: Vec<&Masses> = frames.iter().map(|f| f.map_or(&ZERO, |x| &x.mass)).collect();
        let next = |r: Round, set: u64, m: &[&MArr; MAX_V], w: &Vals, reach: &Vals| self.turn_to_river(r, set, m, w, reach, frames);
        self.betting(1, &mass, r, set, &[&UNIT_M; MAX_V], w, reach, &next)
    }

    fn turn_to_river(&self, r: Round, set: u64, m: &[&MArr; MAX_V], w: &Vals, reach: &Vals, frames: &[Option<&TurnFrame>]) -> Vals {
        let r2 = r.next_street();
        let nr = frames[set.trailing_zeros() as usize].expect("table in set has a frame").nrivers;
        let nv = self.tables[0].len();
        let mut rivers: Vec<RiverFrame> = vec![RiverFrame::zero(); self.tables.len() * nr];
        let mut card_w = vec![[0f64; MAX_T]; nr];
        let mut totals = [0.0f64; MAX_T];
        for t in bits(set) {
            let frame = frames[t].expect("table in set has a frame");
            for j in 0..nr {
                let rf = &mut rivers[t * nr + j];
                let mut w_j = 1.0f64;
                for v in 0..nv {
                    if r.in_hand >> self.tables[t][v].seat & 1 == 0 {
                        continue;
                    }
                    w_j *= f64::from(frame.fill_river(j, v, &m[v][t], rf));
                }
                card_w[j][t] = w_j;
                totals[t] += w_j;
            }
        }
        let mut guard = self.tapes.borrow_mut();
        let tapes = &mut *guard;
        let root = self.tape_root(tapes, r2, set);
        let mut val = [0.0; MAX_T];
        let mut frame_refs: Vec<&RiverFrame> = vec![&rivers[0]; self.tables.len()];
        for j in 0..nr {
            let (mut sub, mut w2, mut frac) = (0u64, [0.0; MAX_T], [0.0; MAX_T]);
            for t in bits(set) {
                if card_w[j][t] > 0.0 {
                    sub |= 1 << t;
                    frac[t] = card_w[j][t] / totals[t];
                    w2[t] = w[t] * frac[t];
                    frame_refs[t] = &rivers[t * nr + j];
                }
            }
            if sub == 0 {
                continue;
            }
            let vals = self.eval_river(&tapes.nodes, &tapes.acts, root, sub, &frame_refs, &[&UNIT_R; MAX_V], &w2, reach);
            for t in bits(sub) {
                val[t] += frac[t] * vals[t];
            }
        }
        for t in bits(set) {
            if totals[t] <= 0.0 {
                val[t] = r2.stacks[self.hero];
            }
        }
        val
    }

    fn tape_root(&self, cache: &mut TapeCache, r: Round, set: u64) -> usize {
        if cache.nodes.len() > NODE_CAP.load(Ordering::Relaxed) || cache.acts.len() > ACT_CAP.load(Ordering::Relaxed) {
            cache.clear();
        }
        if let Some(&(_, _, root)) = cache.entries.iter().find(|(k, m, _)| *m == set && k.pot == r.pot && k.in_hand == r.in_hand && k.stacks == r.stacks) {
            return root as usize;
        }
        let root = self.compile(r, set, cache);
        cache.entries.push((r, set, root));
        root as usize
    }

    fn compile(&self, r: Round, set: u64, cache: &mut TapeCache) -> u32 {
        let idx = cache.nodes.len() as u32;
        let hero = self.hero;
        if r.in_hand >> hero & 1 == 0 {
            cache.nodes.push(Node::terminal(r.stacks[hero]));
            return idx;
        }
        if r.in_hand.count_ones() == 1 {
            cache.nodes.push(Node::terminal(r.stacks[hero] + r.pot));
            return idx;
        }
        if r.to_act == 0 {
            let mut active = 0u8;
            for (v, villain) in self.tables[0].iter().enumerate() {
                if r.in_hand >> villain.seat & 1 == 1 {
                    active |= 1 << v;
                }
            }
            let mut n = Node::terminal(r.stacks[hero]);
            n.kind = Kind::Showdown;
            n.active = active;
            n.pot = r.pot;
            cache.nodes.push(n);
            return idx;
        }
        let seat = r.next as usize;
        if seat == hero {
            let mut n = Node::terminal(0.0);
            n.kind = Kind::Hero;
            cache.nodes.push(n);
            let mut opts = [Act::Fold; 3];
            let c = r.deep_options(seat, &mut opts);
            for (k, &act) in opts[..c].iter().enumerate() {
                let mut r2 = r;
                r2.apply(seat, act);
                let child = self.compile(r2, set, cache);
                cache.nodes[idx as usize].groups[k] = Group { col: 0, child, mask: set };
            }
            cache.nodes[idx as usize].n = c as u8;
            return idx;
        }
        let v = self.seat_to_v[seat];
        let facing = r.is_facing(seat);
        let (mw, rl, sit) = ((r.other_active(seat) - 1).min(2), usize::from(r.raises).min(2), r.situation(seat));
        let raise_to = if facing && r.raises < 2 { r.raise_to_3x(seat) } else { None };
        let acts_off = cache.acts.len() as u32;
        cache.acts.resize(acts_off as usize + set.count_ones() as usize, [[0.0; NBUCKET]; 3]);
        for (rank, t) in bits(set).enumerate() {
            let row = self.tables[t][v].tables.row(2, mw, rl, sit);
            let a = &mut cache.acts[acts_off as usize + rank];
            for c in 0..NBUCKET {
                a[0][c] = row[c][0];
                a[1][c] = if facing && raise_to.is_none() { row[c][1] + row[c][2] } else { row[c][1] };
                a[2][c] = row[c][2];
            }
        }
        let mut n = Node::terminal(r.stacks[hero] + r.pot);
        n.kind = Kind::Villain;
        n.v = v as u8;
        n.facing = facing;
        n.acts_off = acts_off;
        n.set = set;
        cache.nodes.push(n);
        cache.villain_nodes += 1;
        // Public actions and the tables taking each.
        let mut steps: Vec<(Act, u8, u64)> = Vec::with_capacity(MAX_G);
        if facing {
            steps.push((Act::Fold, 0, set));
            steps.push((Act::Call, 1, set));
            if let Some(to) = raise_to {
                steps.push((Act::Raise(to), 2, set));
            }
        } else {
            steps.push((Act::Check, 0, set));
            for t in bits(set) {
                let to = r.bet_to(seat, f64::from(self.tables[t][v].tables.bet_frac[2]));
                match steps.iter_mut().skip(1).find(|s| s.0 == Act::Bet(to)) {
                    Some(s) => s.2 |= 1 << t,
                    None => steps.push((Act::Bet(to), 1, 1 << t)),
                }
            }
        }
        assert!(steps.len() <= MAX_G, "too many distinct bet sizes at one node");
        cache.nodes[idx as usize].n = steps.len() as u8;
        for (k, (act, col, mask)) in steps.into_iter().enumerate() {
            let mut r2 = r;
            r2.apply(seat, act);
            let child = self.compile(r2, mask, cache);
            cache.nodes[idx as usize].groups[k] = Group { col, child, mask };
        }
        idx
    }

    #[allow(clippy::too_many_arguments)]
    fn eval_river(&self, nodes: &[Node], acts: &[[[f32; NBUCKET]; 3]], i: usize, set: u64, f: &[&RiverFrame], m: &[&RArr; MAX_V], w: &Vals, reach: &Vals) -> Vals {
        let n = &nodes[i];
        match n.kind {
            Kind::Terminal => [n.value; MAX_T],
            Kind::Showdown => {
                let mut out = [0.0; MAX_T];
                let nv = self.tables[0].len();
                for t in bits(set) {
                    let (mut beat, mut tie) = ([0f64; MAX_V], [0f64; MAX_V]);
                    let mut k = 0;
                    for v in 0..nv {
                        if n.active >> v & 1 == 0 {
                            continue;
                        }
                        let (mult, fr) = (&m[v][t], f[t]);
                        let (mut tot, mut b, mut ti) = (0f32, 0f32, 0f32);
                        for c in 0..NBUCKET {
                            let x = mult[c];
                            tot += fr.mass[v][c] * x;
                            b += fr.beat[v][c] * x;
                            ti += fr.tie[v][c] * x;
                        }
                        if tot > 0.0 {
                            beat[k] = f64::from(b) / f64::from(tot);
                            tie[k] = f64::from(ti) / f64::from(tot);
                            k += 1;
                        }
                    }
                    out[t] = n.value + hero_share(&beat[..k], &tie[..k]) * n.pot;
                }
                out
            }
            Kind::Hero => {
                let mut best: Option<(f64, Vals)> = None;
                for g in &n.groups[..usize::from(n.n)] {
                    let vals = self.eval_river(nodes, acts, g.child as usize, set, f, m, w, reach);
                    let score: f64 = bits(set).map(|t| w[t] * vals[t]).sum();
                    if best.as_ref().is_none_or(|(b, _)| score > *b) {
                        best = Some((score, vals));
                    }
                }
                best.expect("hero has an option").1
            }
            Kind::Villain => {
                let v = usize::from(n.v);
                let off = n.acts_off as usize;
                let act_at = |t: usize| off + (n.set & ((1u64 << t) - 1)).count_ones() as usize;
                let (mut wm, mut total) = ([[0f32; NBUCKET]; MAX_T], [0f32; MAX_T]);
                for t in bits(set) {
                    let (mass, mults) = (&f[t].mass[v], &m[v][t]);
                    let mut s = 0f32;
                    for c in 0..NBUCKET {
                        wm[t][c] = mass[c] * mults[c];
                        s += wm[t][c];
                    }
                    total[t] = s;
                }
                let (mut val, mut sum) = ([0.0f64; MAX_T], [0.0f64; MAX_T]);
                let mut empty = 0u64;
                for t in bits(set) {
                    if total[t] <= 0.0 {
                        empty |= 1 << t;
                    }
                }
                for g in &n.groups[..usize::from(n.n)] {
                    let live = g.mask & set & !empty;
                    if live == 0 {
                        continue;
                    }
                    let col = usize::from(g.col);
                    let mut child_m = [[0f32; NBUCKET]; MAX_T];
                    let (mut w2, mut reach2, mut ps) = ([0.0; MAX_T], [0.0; MAX_T], [0.0f64; MAX_T]);
                    let mut kept = 0u64;
                    for t in bits(live) {
                        let row = &acts[act_at(t)][col];
                        let mut s = 0f32;
                        for c in 0..NBUCKET {
                            s += wm[t][c] * row[c];
                        }
                        let pa = s / total[t];
                        if pa < BRANCH_FLOOR || reach[t] * f64::from(pa) < self.cfg.reach_eps {
                            continue;
                        }
                        let pr = pa.max(1e-30);
                        for c in 0..NBUCKET {
                            child_m[t][c] = m[v][t][c] * row[c] / pr;
                        }
                        let p = f64::from(pa);
                        ps[t] = p;
                        w2[t] = w[t] * p;
                        reach2[t] = reach[t] * p;
                        kept |= 1 << t;
                    }
                    if kept == 0 {
                        continue;
                    }
                    let mut m2 = *m;
                    m2[v] = &child_m;
                    let vals = self.eval_river(nodes, acts, g.child as usize, kept, f, &m2, &w2, &reach2);
                    for t in bits(kept) {
                        val[t] += ps[t] * vals[t];
                        sum[t] += ps[t];
                    }
                }
                let mut fallback = 0u64;
                for t in bits(set) {
                    if total[t] <= 0.0 && n.facing {
                        val[t] = n.value;
                        sum[t] = 1.0;
                    } else if sum[t] <= 0.0 {
                        fallback |= 1 << t;
                    }
                }
                if fallback != 0 {
                    // Everything was pruned: treat the villain as checking or calling.
                    let col = u8::from(n.facing);
                    let g = n.groups[..usize::from(n.n)].iter().find(|g| g.col == col && g.mask & fallback == fallback);
                    if let Some(g) = g {
                        let vals = self.eval_river(nodes, acts, g.child as usize, fallback, f, m, w, reach);
                        for t in bits(fallback) {
                            val[t] = vals[t];
                            sum[t] = 1.0;
                        }
                    } else {
                        // Bet groups split by size, so a check is always shared; a fallback
                        // call is shared too. Reaching here means a broken tape.
                        unreachable!("no shared fallback child");
                    }
                }
                for t in bits(set) {
                    val[t] /= sum[t];
                }
                val
            }
        }
    }
}
