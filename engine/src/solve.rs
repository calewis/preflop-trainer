//! Best-response expectimax for one exact hero hand from a given node.
//!
//! Villains are fixed stochastic models whose action probabilities depend on a
//! holding only through its street class (bucket and draw flag), so within a
//! street a villain's belief is its street-start belief times a per-class
//! multiplier. Full 1326-combo vectors are only rebuilt at street changes, and
//! the river subtree is compiled once per turn line and replayed per river.

use crate::board::{Runouts, TurnBranch, DEAD, KEY_SLOTS};
use crate::combos::NCOMBOS;
use crate::game::{Act, Round, MAX_PLAYERS};
use crate::models::{VillainTables, NBUCKET, NCLASS};
use crate::prof::{self, span};
use std::cell::RefCell;

pub const MAX_V: usize = 6;
/// Branches below this conditional probability are pruned (brief: 0.001).
pub const BRANCH_FLOOR: f32 = 0.001;

/// One active villain at the root: seat, beliefs over combos (zeros on hero's
/// and board cards) and action tables. `belief` is the root belief with the
/// card-removal correction; `base` is the same belief uncorrected, which the
/// line carries forward so that each later correction starts from it.
pub struct Villain<'a> {
    pub seat: usize,
    pub base: &'a [f32],
    pub belief: &'a [f32],
    pub tables: &'a VillainTables,
}

#[derive(Clone, Copy, Debug)]
pub struct SolveCfg {
    /// Prune villain branches whose reach from the root falls below this and
    /// renormalize the survivors. 0 disables it. The per-action floor always applies.
    pub reach_eps: f64,
    /// Re-run the villain-to-villain card-removal correction at each flop-to-turn
    /// transition, not only at the root.
    pub line_decorrelate: bool,
}

type Mult = [[f32; NCLASS]; MAX_V];
const UNIT: Mult = [[1.0; NCLASS]; MAX_V];
static UNIT_RIVER: [f32; NBUCKET] = [1.0; NBUCKET];

/// Per-villain class totals at the start of a street.
#[derive(Clone)]
pub(crate) struct Masses(pub(crate) [[f32; NCLASS]; MAX_V]);

const NO_CHILD: u32 = u32::MAX;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Terminal,
    Showdown,
    Hero,
    Villain,
}

/// One node of the compiled river betting tree. Everything that depends only
/// on the betting line is fixed here; replay supplies each river's masses.
#[derive(Clone, Copy)]
struct RiverNode {
    kind: Kind,
    /// Villain index (villain nodes).
    v: u8,
    facing: bool,
    nchild: u8,
    /// Showdown: bitmask of villain indices still in the hand.
    active: u8,
    /// Terminal: hero's final stack. Showdown: hero's behind. Villain, facing:
    /// the value if the villain's belief is empty.
    value: f64,
    pot: f64,
    children: [u32; 3],
    /// Villain: action probability per river class and child.
    acts: [[f32; NBUCKET]; 3],
}

impl RiverNode {
    fn terminal(value: f64) -> RiverNode {
        RiverNode { kind: Kind::Terminal, v: 0, facing: false, nchild: 0, active: 0, value, pot: 0.0, children: [NO_CHILD; 3], acts: [[0.0; NBUCKET]; 3] }
    }
}

struct Solver<'a> {
    hero: usize,
    villains: &'a [Villain<'a>],
    seat_to_v: [usize; MAX_PLAYERS],
    runouts: &'a Runouts,
    cfg: SolveCfg,
    /// Compiled river trees, shared across turn cards within a flop line:
    /// a tree depends only on the river-start betting state.
    tapes: RefCell<TapeCache>,
}

/// EVs of each root option relative to hero's stack at the root (fold = 0).
pub fn solve_root(root: &Round, hero: usize, villains: &[Villain], runouts: &Runouts, cfg: SolveCfg, options: &[Act]) -> Vec<f64> {
    assert!(villains.len() <= MAX_V, "too many villains");
    let mut seat_to_v = [usize::MAX; MAX_PLAYERS];
    for (i, v) in villains.iter().enumerate() {
        seat_to_v[v.seat] = i;
    }
    let s = Solver { hero, villains, seat_to_v, runouts, cfg, tapes: RefCell::new(TapeCache::default()) };
    let base_stack = root.stacks[hero];
    span(prof::SOLVE_TOTAL, || {
        let frame = s.root_frame(root.in_hand);
        options
            .iter()
            .map(|&act| {
                let mut r = *root;
                r.apply(hero, act);
                s.play_root(r, &frame) - base_stack
            })
            .collect()
    })
}

/// Street-start data at the root, computed once and shared by every option.
enum RootFrame {
    Flop(Masses),
    Turn(TurnFrame),
    River(RiverFrame),
}

struct Level<'a> {
    street: usize,
    mass: &'a Masses,
    next: &'a dyn Fn(Round, &Mult, f64) -> f64,
}

/// Compiled trees keyed by the betting state at the start of the river.
#[derive(Default)]
struct TapeCache {
    nodes: Vec<RiverNode>,
    entries: Vec<(Round, u32)>,
}

impl TapeCache {
    fn clear(&mut self) {
        self.nodes.clear();
        self.entries.clear();
    }
}

fn same_river_state(a: &Round, b: &Round) -> bool {
    a.pot == b.pot && a.in_hand == b.in_hand && a.stacks == b.stacks
}

impl<'a> Solver<'a> {
    /// Root of the compiled river tree for `r`, compiling it if not cached.
    fn tape_root(&self, cache: &mut TapeCache, r: Round) -> usize {
        if let Some(&(_, root)) = cache.entries.iter().find(|(k, _)| same_river_state(k, &r)) {
            return root as usize;
        }
        let root = span(prof::TAPE, || self.compile_river(r, &mut cache.nodes));
        cache.entries.push((r, root));
        root as usize
    }

    fn root_frame(&self, in_hand: u8) -> RootFrame {
        match self.runouts.root_street {
            1 => {
                let mut mass = Masses([[0.0; NCLASS]; MAX_V]);
                for (i, v) in self.villains.iter().enumerate() {
                    add_masses(&mut mass.0[i], v.belief, &self.runouts.flop_class);
                }
                RootFrame::Flop(mass)
            }
            2 => {
                let beliefs: Vec<&[f32]> = self.villains.iter().map(|v| v.belief).collect();
                RootFrame::Turn(span(prof::BUILD_TURN, || build_turn(self.villains, &self.runouts.turns[0], &beliefs, in_hand)))
            }
            3 => {
                let river = &self.runouts.turns[0].rivers[0];
                let mut f = RiverFrame::zero();
                for (i, v) in self.villains.iter().enumerate() {
                    fill_river_from_belief(&mut f, i, v.belief, &river.class, &river.outcome);
                }
                RootFrame::River(f)
            }
            _ => unreachable!(),
        }
    }

    fn play_root(&self, r: Round, frame: &RootFrame) -> f64 {
        match frame {
            RootFrame::Flop(mass) => {
                let next = |r: Round, m: &Mult, reach: f64| self.flop_to_turn(r, m, reach);
                let l = Level { street: 0, mass, next: &next };
                self.betting(&l, r, &UNIT, 1.0)
            }
            RootFrame::Turn(f) => self.run_turn(r, f, 1.0),
            RootFrame::River(f) => {
                let mut tapes = self.tapes.borrow_mut();
                tapes.clear();
                let root = self.tape_root(&mut tapes, r);
                self.eval_river(&tapes.nodes, root, f, &[&UNIT_RIVER; MAX_V], 1.0)
            }
        }
    }

    fn flop_to_turn(&self, r: Round, m: &Mult, reach: f64) -> f64 {
        let flop = &self.runouts.flop_class;
        let r2 = r.next_street();
        self.tapes.borrow_mut().clear();
        let mut beliefs: Vec<Vec<f32>> = span(prof::FLOP_VEC, || {
            self.villains
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    if r.in_hand >> v.seat & 1 == 0 {
                        return Vec::new();
                    }
                    let mut b = vec![0f32; NCOMBOS];
                    let src = if self.cfg.line_decorrelate { v.base } else { v.belief };
                    for ((out, &base), &c) in b.iter_mut().zip(src).zip(flop) {
                        if base != 0.0 && c != DEAD {
                            *out = base * m[i][c as usize];
                        }
                    }
                    b
                })
                .collect()
        });
        let active: Vec<usize> = (0..beliefs.len()).filter(|&i| !beliefs[i].is_empty()).collect();
        if self.cfg.line_decorrelate && active.len() >= 2 {
            let mut vecs: Vec<Vec<f32>> = active.iter().map(|&i| std::mem::take(&mut beliefs[i])).collect();
            span(prof::DECORR, || decorrelate(&mut vecs, 2));
            for (slot, &i) in active.iter().enumerate() {
                beliefs[i] = std::mem::take(&mut vecs[slot]);
            }
        }
        let refs: Vec<&[f32]> = beliefs.iter().map(|b| b.as_slice()).collect();
        let (mut total, mut w_sum) = (0.0f64, 0.0f64);
        for turn in &self.runouts.turns {
            let frame = span(prof::BUILD_TURN, || build_turn(self.villains, turn, &refs, r2.in_hand));
            if frame.weight <= 0.0 {
                continue;
            }
            let v = self.run_turn(r2, &frame, reach);
            total += frame.weight * v;
            w_sum += frame.weight;
        }
        if w_sum > 0.0 {
            total / w_sum
        } else {
            r2.stacks[self.hero]
        }
    }

    fn run_turn(&self, r: Round, frame: &TurnFrame, reach: f64) -> f64 {
        let next = |r: Round, m: &Mult, reach: f64| self.turn_to_river(r, m, reach, frame);
        let l = Level { street: 1, mass: &frame.mass, next: &next };
        self.betting(&l, r, &UNIT, reach)
    }

    fn turn_to_river(&self, r: Round, m: &Mult, reach: f64, turn: &TurnFrame) -> f64 {
        let r2 = r.next_street();
        let mut guard = self.tapes.borrow_mut();
        let tapes = &mut *guard;
        let root = self.tape_root(tapes, r2);
        let tape = &tapes.nodes;
        let (mut total, mut w_sum) = (0.0f64, 0.0f64);
        for j in 0..turn.nrivers {
            let mut rf = RiverFrame::zero();
            let mut w = 1.0f64;
            for (v, villain) in self.villains.iter().enumerate() {
                if r.in_hand >> villain.seat & 1 == 0 {
                    continue;
                }
                let mass_total = span(prof::FILL_RIVER, || turn.fill_river(j, v, &m[v], &mut rf));
                w *= f64::from(mass_total);
            }
            if w <= 0.0 {
                continue;
            }
            total += w * self.eval_river(tape, root, &rf, &[&UNIT_RIVER; MAX_V], reach);
            w_sum += w;
        }
        if w_sum > 0.0 {
            total / w_sum
        } else {
            r2.stacks[self.hero]
        }
    }

    /// Compiles the betting tree from a river-street round. Returns the root index.
    fn compile_river(&self, r: Round, tape: &mut Vec<RiverNode>) -> u32 {
        let idx = tape.len() as u32;
        let hero = self.hero;
        if r.in_hand >> hero & 1 == 0 {
            tape.push(RiverNode::terminal(r.stacks[hero]));
            return idx;
        }
        if r.in_hand.count_ones() == 1 {
            tape.push(RiverNode::terminal(r.stacks[hero] + r.pot));
            return idx;
        }
        if r.to_act == 0 {
            let mut active = 0u8;
            for (v, villain) in self.villains.iter().enumerate() {
                if r.in_hand >> villain.seat & 1 == 1 {
                    active |= 1 << v;
                }
            }
            let mut node = RiverNode::terminal(r.stacks[hero]);
            node.kind = Kind::Showdown;
            node.active = active;
            node.pot = r.pot;
            tape.push(node);
            return idx;
        }
        let seat = r.next as usize;
        if seat == hero {
            let mut node = RiverNode::terminal(0.0);
            node.kind = Kind::Hero;
            tape.push(node);
            let mut opts = [Act::Fold; 3];
            let n = r.deep_options(seat, &mut opts);
            for (k, &act) in opts[..n].iter().enumerate() {
                let mut r2 = r;
                r2.apply(seat, act);
                let c = self.compile_river(r2, tape);
                tape[idx as usize].children[k] = c;
            }
            tape[idx as usize].nchild = n as u8;
            return idx;
        }
        let v = self.seat_to_v[seat];
        let villain = &self.villains[v];
        let facing = r.is_facing(seat);
        let row = villain.tables.row(2, (r.other_active(seat) - 1).min(2), usize::from(r.raises).min(2), r.situation(seat));
        let mut node = RiverNode::terminal(r.stacks[hero] + r.pot);
        node.kind = Kind::Villain;
        node.v = v as u8;
        node.facing = facing;
        let mut acts = [[0f32; NBUCKET]; 3];
        let mut steps: [Option<Act>; 3] = [None; 3];
        if !facing {
            for c in 0..NBUCKET {
                acts[0][c] = row[c][0];
                acts[1][c] = row[c][1];
            }
            steps[0] = Some(Act::Check);
            steps[1] = Some(Act::Bet(r.bet_to(seat, f64::from(villain.tables.bet_frac[2]))));
            node.nchild = 2;
        } else {
            let raise_to = if r.raises < 2 { r.raise_to_3x(seat) } else { None };
            for c in 0..NBUCKET {
                acts[0][c] = row[c][0];
                acts[1][c] = if raise_to.is_some() { row[c][1] } else { row[c][1] + row[c][2] };
                acts[2][c] = row[c][2];
            }
            steps[0] = Some(Act::Fold);
            steps[1] = Some(Act::Call);
            steps[2] = raise_to.map(Act::Raise);
            node.nchild = if raise_to.is_some() { 3 } else { 2 };
        }
        node.acts = acts;
        tape.push(node);
        for (k, step) in steps.iter().enumerate() {
            if let Some(act) = step {
                let mut r2 = r;
                r2.apply(seat, *act);
                let c = self.compile_river(r2, tape);
                tape[idx as usize].children[k] = c;
            }
        }
        idx
    }

    /// Replays a compiled river tree for one river's masses.
    fn eval_river(&self, tape: &[RiverNode], i: usize, f: &RiverFrame, m: &[&[f32; NBUCKET]; MAX_V], reach: f64) -> f64 {
        let n = &tape[i];
        match n.kind {
            Kind::Terminal => n.value,
            Kind::Showdown => span(prof::SHOWDOWN, || self.river_showdown(n, f, m)),
            Kind::Hero => {
                let mut best = f64::NEG_INFINITY;
                for k in 0..usize::from(n.nchild) {
                    best = best.max(self.eval_river(tape, n.children[k] as usize, f, m, reach));
                }
                best
            }
            Kind::Villain => {
                let v = usize::from(n.v);
                let mass = &f.mass[v];
                let mults = m[v];
                let mut w = [0f32; NBUCKET];
                let mut total = 0f32;
                for c in 0..NBUCKET {
                    w[c] = mass[c] * mults[c];
                    total += w[c];
                }
                if total <= 0.0 {
                    return if n.facing { n.value } else { self.eval_river(tape, n.children[0] as usize, f, m, reach) };
                }
                let nchild = usize::from(n.nchild);
                let mut p = [0f32; 3];
                for (a, slot) in p.iter_mut().enumerate().take(nchild) {
                    let mut s = 0f32;
                    for c in 0..NBUCKET {
                        s += w[c] * n.acts[a][c];
                    }
                    *slot = s / total;
                }
                let (mut val, mut sum) = (0.0f64, 0.0f64);
                for a in 0..nchild {
                    let pa = p[a];
                    if pa < BRANCH_FLOOR || reach * f64::from(pa) < self.cfg.reach_eps {
                        continue;
                    }
                    let pr = pa.max(1e-30);
                    let mut child_m = [0f32; NBUCKET];
                    for c in 0..NBUCKET {
                        child_m[c] = mults[c] * n.acts[a][c] / pr;
                    }
                    let mut m2 = *m;
                    m2[v] = &child_m;
                    let child = self.eval_river(tape, n.children[a] as usize, f, &m2, reach * f64::from(pa));
                    val += f64::from(pa) * child;
                    sum += f64::from(pa);
                }
                if sum > 0.0 {
                    val / sum
                } else {
                    let fallback = if n.facing { 1 } else { 0 };
                    self.eval_river(tape, n.children[fallback] as usize, f, m, reach)
                }
            }
        }
    }

    #[inline]
    fn river_showdown(&self, n: &RiverNode, f: &RiverFrame, m: &[&[f32; NBUCKET]; MAX_V]) -> f64 {
        let (mut beat, mut tie) = ([0f64; MAX_V], [0f64; MAX_V]);
        let mut k = 0;
        for v in 0..self.villains.len() {
            if n.active >> v & 1 == 0 {
                continue;
            }
            let mult = m[v];
            let (mut t, mut b, mut ti) = (0f32, 0f32, 0f32);
            for c in 0..NBUCKET {
                let x = mult[c];
                t += f.mass[v][c] * x;
                b += f.beat[v][c] * x;
                ti += f.tie[v][c] * x;
            }
            if t <= 0.0 {
                continue;
            }
            beat[k] = f64::from(b) / f64::from(t);
            tie[k] = f64::from(ti) / f64::from(t);
            k += 1;
        }
        n.value + hero_share(&beat[..k], &tie[..k]) * n.pot
    }

    /// Betting within a flop or turn street; the river uses the compiled tape.
    fn betting(&self, l: &Level, r: Round, m: &Mult, reach: f64) -> f64 {
        if r.in_hand >> self.hero & 1 == 0 {
            return r.stacks[self.hero];
        }
        if r.in_hand.count_ones() == 1 {
            return r.stacks[self.hero] + r.pot;
        }
        if r.to_act == 0 {
            return (l.next)(r, m, reach);
        }
        let seat = r.next as usize;
        if seat == self.hero {
            let mut opts = [Act::Fold; 3];
            let n = r.deep_options(seat, &mut opts);
            let mut best = f64::NEG_INFINITY;
            for &act in &opts[..n] {
                let mut r2 = r;
                r2.apply(seat, act);
                best = best.max(self.betting(l, r2, m, reach));
            }
            return best;
        }

        let v = self.seat_to_v[seat];
        debug_assert!(v < MAX_V, "villain not in tree");
        let villain = &self.villains[v];
        let facing = r.is_facing(seat);
        let sit = r.situation(seat);
        let rl = usize::from(r.raises).min(2);
        let mw = (r.other_active(seat) - 1).min(2);
        let row = villain.tables.row(l.street, mw, rl, sit);
        let mass = &l.mass.0[v];
        let mults = &m[v];

        let mut p = [0f32; 3];
        let mut total = 0f32;
        for c in 0..NCLASS {
            let w = mass[c] * mults[c];
            total += w;
            p[0] += w * row[c][0];
            p[1] += w * row[c][1];
            p[2] += w * row[c][2];
        }
        if total <= 0.0 {
            if facing {
                return r.stacks[self.hero] + r.pot;
            }
            let mut r2 = r;
            r2.apply(seat, Act::Check);
            return self.betting(l, r2, m, reach);
        }
        for x in p.iter_mut() {
            *x /= total;
        }

        let raise_to = if facing && r.raises < 2 { r.raise_to_3x(seat) } else { None };
        let call_p = if raise_to.is_some() { p[1] } else { p[1] + p[2] };
        let (acts, probs): ([Option<(Act, usize)>; 3], [f32; 3]) = if facing {
            ([Some((Act::Fold, 0)), Some((Act::Call, 1)), raise_to.map(|to| (Act::Raise(to), 2))], [p[0], call_p, p[2]])
        } else {
            let to = r.bet_to(seat, f64::from(villain.tables.bet_frac[l.street]));
            ([Some((Act::Check, 0)), Some((Act::Bet(to), 1)), None], [p[0], p[1], 0.0])
        };
        let (mut val, mut sum) = (0.0f64, 0.0f64);
        for (slot, pa) in acts.iter().zip(probs) {
            let Some((act, col)) = slot else { continue };
            if pa < BRANCH_FLOOR || reach * f64::from(pa) < self.cfg.reach_eps {
                continue;
            }
            let mut r2 = r;
            r2.apply(seat, *act);
            let mut m2 = *m;
            let pr = pa.max(1e-30);
            for c in 0..NCLASS {
                let f = if *col == 1 && facing && raise_to.is_none() { row[c][1] + row[c][2] } else { row[c][*col] };
                m2[v][c] = mults[c] * f / pr;
            }
            let child = self.betting(l, r2, &m2, reach * f64::from(pa));
            val += f64::from(pa) * child;
            sum += f64::from(pa);
        }
        if sum > 0.0 {
            val / sum
        } else {
            // Everything was pruned: treat the villain as checking or calling.
            let mut r2 = r;
            r2.apply(seat, if facing { Act::Call } else { Act::Check });
            self.betting(l, r2, m, reach)
        }
    }
}

/// A turn frame from per-villain beliefs (over 1326 combos, zero if dead).
/// Combos dead on this turn's board are dropped. Folded villains are skipped.
pub(crate) fn build_turn(villains: &[Villain], turn: &TurnBranch, beliefs: &[&[f32]], in_hand: u8) -> TurnFrame {
    let nr = turn.rivers.len();
    let nv = villains.len();
    let mut f = TurnFrame::new(nv, nr);
    let mut w = 1.0f64;
    for (v, villain) in villains.iter().enumerate() {
        if in_hand >> villain.seat & 1 == 0 {
            continue;
        }
        let b = beliefs[v];
        let mut by_class = [0f32; NCLASS + 1];
        for (&x, &ct) in b.iter().zip(&turn.class) {
            by_class[usize::from(ct).min(NCLASS)] += x;
        }
        f.mass.0[v][..NCLASS].copy_from_slice(&by_class[..NCLASS]);
        w *= f64::from(by_class[..NCLASS].iter().sum::<f32>());
        for (j, river) in turn.rivers.iter().enumerate() {
            let mut bins = [0f32; KEY_SLOTS + 1];
            for (&x, &k) in b.iter().zip(&river.key) {
                bins[usize::from(k)] += x;
            }
            let start = f.slot(j, v);
            for cell in 0..NCLASS * NBUCKET {
                let base = cell * 3;
                f.jm[start + cell] = bins[base] + bins[base + 1] + bins[base + 2];
                f.jt[start + cell] = bins[base + 1];
                f.jb[start + cell] = bins[base + 2];
            }
        }
    }
    f.weight = w;
    f
}

pub(crate) fn add_masses(out: &mut [f32; NCLASS], belief: &[f32], classes: &[u8]) {
    for (&b, &c) in belief.iter().zip(classes) {
        if c != DEAD {
            out[c as usize] += b;
        }
    }
}

pub(crate) fn fill_river_from_belief(f: &mut RiverFrame, v: usize, belief: &[f32], class: &[u8], outcome: &[u8]) {
    for ((&b, &c), &oc) in belief.iter().zip(class).zip(outcome) {
        if b == 0.0 || c == DEAD {
            continue;
        }
        let i = c as usize % NBUCKET;
        f.mass[v][i] += b;
        if oc == 2 {
            f.beat[v][i] += b;
        } else if oc == 1 {
            f.tie[v][i] += b;
        }
    }
}

/// Expected share of the pot against independent villains with given
/// probabilities of beating hero and tying hero (exact under independence).
#[inline]
pub fn hero_share(beat: &[f64], tie: &[f64]) -> f64 {
    let n = beat.len();
    if n == 1 {
        return 1.0 - beat[0] - 0.5 * tie[0];
    }
    // dp[k] = P(no villain beats hero so far and exactly k tie).
    let mut dp = [0f64; MAX_V + 2];
    dp[0] = 1.0;
    for i in 0..n {
        let win = 1.0 - beat[i] - tie[i];
        for k in (0..=i + 1).rev() {
            let tie_in = if k > 0 { dp[k - 1] * tie[i] } else { 0.0 };
            dp[k] = dp[k] * win + tie_in;
        }
    }
    let mut share = 0.0;
    for (k, &p) in dp.iter().enumerate().take(n + 1) {
        share += p / (k + 1) as f64;
    }
    share
}

/// Mean-field correction for villain-to-villain card removal: each villain's
/// original belief is reweighted by the chance the others' (current, corrected)
/// beliefs do not block each combo, using per-card sums. Each iteration starts
/// again from the originals, so the correction does not compound. Exact joint
/// enumeration of holdings is the reference (validate binary, tests).
pub fn decorrelate(beliefs: &mut [Vec<f32>], iterations: usize) {
    let table = crate::combos::combos();
    let n = beliefs.len();
    if n < 2 {
        return;
    }
    assert!(n <= MAX_V, "too many villains");
    let orig = beliefs.to_vec();
    let mut totals = [0f32; MAX_V];
    let mut current = beliefs.to_vec();
    for _ in 0..iterations {
        let mut card = [[0f32; 52]; MAX_V];
        for (v, b) in current.iter().enumerate() {
            let total: f32 = b.iter().sum();
            totals[v] = total;
            if total <= 0.0 {
                continue;
            }
            for (i, &w) in b.iter().enumerate() {
                if w > 0.0 {
                    let share = w / total;
                    let (a, c) = table.cards[i];
                    card[v][a.index()] += share;
                    card[v][c.index()] += share;
                }
            }
        }
        for v in 0..n {
            for i in 0..NCOMBOS {
                let w = orig[v][i];
                if w <= 0.0 {
                    beliefs[v][i] = 0.0;
                    continue;
                }
                let (a, c) = table.cards[i];
                let mut factor = 1.0f32;
                for j in 0..n {
                    if j == v || totals[j] <= 0.0 {
                        continue;
                    }
                    let same = current[j][i] / totals[j];
                    let overlap = card[j][a.index()] + card[j][c.index()] - same;
                    factor *= (1.0 - overlap).max(1e-6);
                }
                beliefs[v][i] = w * factor;
            }
        }
        current.clone_from_slice(beliefs);
    }
    for b in beliefs.iter_mut() {
        let total: f32 = b.iter().sum();
        if total > 0.0 {
            for w in b.iter_mut() {
                *w /= total;
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct RiverFrame {
    pub(crate) mass: [[f32; NBUCKET]; MAX_V],
    pub(crate) beat: [[f32; NBUCKET]; MAX_V],
    pub(crate) tie: [[f32; NBUCKET]; MAX_V],
}

impl RiverFrame {
    pub(crate) fn zero() -> RiverFrame {
        RiverFrame { mass: [[0.0; NBUCKET]; MAX_V], beat: [[0.0; NBUCKET]; MAX_V], tie: [[0.0; NBUCKET]; MAX_V] }
    }
}

/// Per river card and villain, by turn class x river class: total mass (`jm`),
/// mass beating hero (`jb`) and mass tying hero (`jt`). Separate arrays so the
/// fill loop vectorizes.
pub(crate) struct TurnFrame {
    pub(crate) mass: Masses,
    pub(crate) nrivers: usize,
    nv: usize,
    jm: Vec<f32>,
    jb: Vec<f32>,
    jt: Vec<f32>,
    pub(crate) weight: f64,
}

impl TurnFrame {
    fn new(nv: usize, nrivers: usize) -> TurnFrame {
        let len = nv * nrivers * NCLASS * NBUCKET;
        TurnFrame { mass: Masses([[0.0; NCLASS]; MAX_V]), nrivers, nv, jm: vec![0.0; len], jb: vec![0.0; len], jt: vec![0.0; len], weight: 0.0 }
    }

    #[inline]
    fn slot(&self, j: usize, v: usize) -> usize {
        (j * self.nv + v) * NCLASS * NBUCKET
    }

    /// Writes this villain's river-frame row under per-turn-class multipliers
    /// `mults` and returns the villain's total mass for this river.
    #[inline]
    pub(crate) fn fill_river(&self, j: usize, v: usize, mults: &[f32; NCLASS], out: &mut RiverFrame) -> f32 {
        let start = self.slot(j, v);
        let unit = NCLASS * NBUCKET;
        let (jm, jb, jt) = (&self.jm[start..start + unit], &self.jb[start..start + unit], &self.jt[start..start + unit]);
        let (om, ob, ot) = (&mut out.mass[v], &mut out.beat[v], &mut out.tie[v]);
        for ct in 0..NCLASS {
            let m = mults[ct];
            let base = ct * NBUCKET;
            for cr in 0..NBUCKET {
                om[cr] += jm[base + cr] * m;
                ob[cr] += jb[base + cr] * m;
                ot[cr] += jt[base + cr] * m;
            }
        }
        om.iter().sum()
    }
}
