//! A deliberately simple reference implementation of the expectimax: every
//! node carries full 1326-combo belief vectors in f64, with no per-class
//! shortcuts, aggregation or shared frames. Slow; used to prove the optimized
//! solver computes the same numbers.

use crate::board::{Runouts, DEAD};
use crate::combos::NCOMBOS;
use crate::game::{Act, Round};
use crate::solve::{decorrelate, hero_share, SolveCfg, Villain, BRANCH_FLOOR};

/// Per villain: the corrected beliefs, followed by the uncorrected beliefs the
/// line carries forward for the next correction (2 x villains vectors).
type Beliefs = Vec<Vec<f64>>;

struct Ref<'a> {
    hero: usize,
    villains: &'a [Villain<'a>],
    runouts: &'a Runouts,
    cfg: SolveCfg,
}

/// EVs of each root option relative to hero's stack, computed naively.
pub fn solve_reference(root: &Round, hero: usize, villains: &[Villain], runouts: &Runouts, cfg: SolveCfg, options: &[Act]) -> Vec<f64> {
    let s = Ref { hero, villains, runouts, cfg };
    let base = root.stacks[hero];
    let beliefs: Beliefs = villains.iter().map(|v| v.belief.iter().map(|&x| f64::from(x)).collect()).chain(villains.iter().map(|v| v.base.iter().map(|&x| f64::from(x)).collect())).collect();
    options
        .iter()
        .map(|&act| {
            let mut r = *root;
            r.apply(hero, act);
            let street = usize::from(runouts.root_street) - 1;
            s.node(r, street, 0, 0, &beliefs, 1.0) - base
        })
        .collect()
}

impl Ref<'_> {
    fn classes(&self, street: usize, turn: usize, river: usize) -> &[u8] {
        match street {
            0 => &self.runouts.flop_class,
            1 => &self.runouts.turns[turn].class,
            _ => &self.runouts.turns[turn].rivers[river].class,
        }
    }

    fn node(&self, r: Round, street: usize, turn: usize, river: usize, b: &Beliefs, reach: f64) -> f64 {
        if r.in_hand >> self.hero & 1 == 0 {
            return r.stacks[self.hero];
        }
        if r.in_hand.count_ones() == 1 {
            return r.stacks[self.hero] + r.pot;
        }
        if r.to_act == 0 {
            return self.street_end(r, street, turn, river, b, reach);
        }
        let seat = r.next as usize;
        if seat == self.hero {
            let mut opts = [Act::Fold; 3];
            let n = r.deep_options(seat, &mut opts);
            let mut best = f64::NEG_INFINITY;
            for &a in &opts[..n] {
                let mut r2 = r;
                r2.apply(seat, a);
                best = best.max(self.node(r2, street, turn, river, b, reach));
            }
            return best;
        }
        let v = self.villains.iter().position(|x| x.seat == seat).unwrap();
        let villain = &self.villains[v];
        let facing = r.is_facing(seat);
        let row = villain.tables.row(street, (r.other_active(seat) - 1).min(2), usize::from(r.raises).min(2), r.situation(seat));
        let classes = self.classes(street, turn, river);
        let raise_to = if facing && r.raises < 2 { r.raise_to_3x(seat) } else { None };
        // Per-combo probability of each branch, as (action, column weights).
        let mut branches: Vec<(Act, [f64; 3])> = Vec::new();
        if !facing {
            branches.push((Act::Check, [1.0, 0.0, 0.0]));
            branches.push((Act::Bet(r.bet_to(seat, f64::from(villain.tables.bet_frac[street]))), [0.0, 1.0, 0.0]));
        } else {
            branches.push((Act::Fold, [1.0, 0.0, 0.0]));
            match raise_to {
                Some(to) => {
                    branches.push((Act::Call, [0.0, 1.0, 0.0]));
                    branches.push((Act::Raise(to), [0.0, 0.0, 1.0]));
                }
                None => branches.push((Act::Call, [0.0, 1.0, 1.0])),
            }
        }
        let weight = |c: usize, col: &[f64; 3]| -> f64 {
            let p = &row[c];
            col[0] * f64::from(p[0]) + col[1] * f64::from(p[1]) + col[2] * f64::from(p[2])
        };
        let total: f64 = b[v].iter().zip(classes).filter(|(_, &c)| c != DEAD).map(|(w, _)| *w).sum();
        let (mut val, mut sum) = (0.0, 0.0);
        for (act, col) in &branches {
            let mut mass = 0.0;
            for i in 0..NCOMBOS {
                if classes[i] != DEAD {
                    mass += b[v][i] * weight(classes[i] as usize, col);
                }
            }
            let p = mass / total;
            if (p as f32) < BRANCH_FLOOR || reach * p < self.cfg.reach_eps {
                continue;
            }
            let mut b2 = b.clone();
            let carry = self.villains.len() + v;
            for i in 0..NCOMBOS {
                let f = if classes[i] != DEAD { weight(classes[i] as usize, col) / p } else { 0.0 };
                b2[v][i] = b[v][i] * f;
                b2[carry][i] = b[carry][i] * f;
            }
            let mut r2 = r;
            r2.apply(seat, *act);
            val += p * self.node(r2, street, turn, river, &b2, reach * p);
            sum += p;
        }
        if sum > 0.0 {
            val / sum
        } else {
            let mut r2 = r;
            r2.apply(seat, if facing { Act::Call } else { Act::Check });
            self.node(r2, street, turn, river, b, reach)
        }
    }

    fn street_end(&self, r: Round, street: usize, turn: usize, river: usize, b: &Beliefs, reach: f64) -> f64 {
        match street {
            2 => showdown(self.hero, self.villains, self.runouts, &r, turn, river, b),
            1 => {
                let r2 = r.next_street();
                let n = self.runouts.turns[turn].rivers.len();
                let (mut total, mut wsum) = (0.0, 0.0);
                for j in 0..n {
                    let classes = &self.runouts.turns[turn].rivers[j].class;
                    let (b2, w) = restrict(self.villains, &r, b, classes);
                    if w <= 0.0 {
                        continue;
                    }
                    total += w * self.node(r2, 2, turn, j, &b2, reach);
                    wsum += w;
                }
                if wsum > 0.0 { total / wsum } else { r2.stacks[self.hero] }
            }
            _ => {
                let r2 = r.next_street();
                // Same correction as the solver: applied to the active villains at the turn start.
                let mut base = b.clone();
                let nv = self.villains.len();
                if self.cfg.line_decorrelate {
                    for i in 0..nv {
                        base[i] = b[nv + i].clone();
                    }
                }
                let active: Vec<usize> = (0..nv).filter(|&i| r.in_hand >> self.villains[i].seat & 1 == 1).collect();
                if self.cfg.line_decorrelate && active.len() >= 2 {
                    let mut vecs: Vec<Vec<f32>> = active.iter().map(|&i| base[i].iter().map(|&x| x as f32).collect()).collect();
                    decorrelate(&mut vecs, 2);
                    for (s, &i) in active.iter().enumerate() {
                        base[i] = vecs[s].iter().map(|&x| f64::from(x)).collect();
                    }
                }
                let (mut total, mut wsum) = (0.0, 0.0);
                for k in 0..self.runouts.turns.len() {
                    let classes = &self.runouts.turns[k].class;
                    let (b2, w) = restrict(self.villains, &r, &base, classes);
                    if w <= 0.0 {
                        continue;
                    }
                    total += w * self.node(r2, 1, k, 0, &b2, reach);
                    wsum += w;
                }
                if wsum > 0.0 { total / wsum } else { r2.stacks[self.hero] }
            }
        }
    }
}

/// One table's state as it travels through the joint solve.
struct Entry {
    table: usize,
    b: Beliefs,
    /// Prior table weight times the probability of everything observed since
    /// the root (villain actions and cards), so it weights hero's choice.
    weight: f64,
    /// Villain-action reach only, as used for pruning in the per-table solver.
    reach: f64,
}

struct Joint<'a> {
    hero: usize,
    tables: &'a [Vec<Villain<'a>>],
    runouts: &'a Runouts,
    cfg: SolveCfg,
}

/// Hero commits to one action per public information state across all tables
/// that reach it. Tables only separate where a villain's visible bet size or
/// action differs. Returns EV by table and root option, relative to hero's
/// root stack. Root options are compared on equal terms: each is solved with
/// a fused continuation.
pub fn solve_joint(root: &Round, hero: usize, tables: &[Vec<Villain>], runouts: &Runouts, cfg: SolveCfg, options: &[Act]) -> Vec<Vec<f64>> {
    let s = Joint { hero, tables, runouts, cfg };
    let base = root.stacks[hero];
    let weight = 1.0 / tables.len() as f64;
    let street = usize::from(runouts.root_street) - 1;
    let mut out = vec![vec![0.0; options.len()]; tables.len()];
    for (o, &act) in options.iter().enumerate() {
        let set: Vec<Entry> = tables
            .iter()
            .enumerate()
            .map(|(t, vs)| Entry { table: t, b: vs.iter().map(|v| v.belief.iter().map(|&x| f64::from(x)).collect()).chain(vs.iter().map(|v| v.base.iter().map(|&x| f64::from(x)).collect())).collect(), weight, reach: 1.0 })
            .collect();
        let mut r = *root;
        r.apply(hero, act);
        let vals = s.node(r, street, 0, 0, set);
        for (t, v) in vals.into_iter().enumerate() {
            out[t][o] = v - base;
        }
    }
    out
}

impl Joint<'_> {
    fn classes(&self, street: usize, turn: usize, river: usize) -> &[u8] {
        match street {
            0 => &self.runouts.flop_class,
            1 => &self.runouts.turns[turn].class,
            _ => &self.runouts.turns[turn].rivers[river].class,
        }
    }

    /// Values aligned with `set`.
    fn node(&self, r: Round, street: usize, turn: usize, river: usize, set: Vec<Entry>) -> Vec<f64> {
        if r.in_hand >> self.hero & 1 == 0 {
            return vec![r.stacks[self.hero]; set.len()];
        }
        if r.in_hand.count_ones() == 1 {
            return vec![r.stacks[self.hero] + r.pot; set.len()];
        }
        if r.to_act == 0 {
            return self.street_end(r, street, turn, river, set);
        }
        let seat = r.next as usize;
        if seat == self.hero {
            let mut opts = [Act::Fold; 3];
            let n = r.deep_options(seat, &mut opts);
            let mut best: Option<(f64, Vec<f64>)> = None;
            for &a in &opts[..n] {
                let mut r2 = r;
                r2.apply(seat, a);
                let child: Vec<Entry> = set.iter().map(|e| Entry { table: e.table, b: e.b.clone(), weight: e.weight, reach: e.reach }).collect();
                let vals = self.node(r2, street, turn, river, child);
                let score: f64 = vals.iter().zip(&set).map(|(v, e)| v * e.weight).sum();
                if best.as_ref().is_none_or(|(b, _)| score > *b) {
                    best = Some((score, vals));
                }
            }
            return best.expect("hero has an option").1;
        }
        self.villain(r, seat, street, turn, river, set)
    }

    fn villain(&self, r: Round, seat: usize, street: usize, turn: usize, river: usize, set: Vec<Entry>) -> Vec<f64> {
        let facing = r.is_facing(seat);
        let v = self.tables[0].iter().position(|x| x.seat == seat).expect("villain seat");
        let classes = self.classes(street, turn, river);
        let n = set.len();
        let mut val = vec![0.0; n];
        let mut sum = vec![0.0; n];
        // Public action -> (entry index, branch probability, child entry).
        let mut groups: Vec<(Act, Vec<(usize, f64)>, Vec<Entry>)> = Vec::new();
        let mut push = |act: Act, i: usize, p: f64, e: Entry| match groups.iter_mut().find(|g| g.0 == act) {
            Some(g) => {
                g.1.push((i, p));
                g.2.push(e);
            }
            None => groups.push((act, vec![(i, p)], vec![e])),
        };
        let raise_to = if facing && r.raises < 2 { r.raise_to_3x(seat) } else { None };
        for (i, e) in set.iter().enumerate() {
            let villain = &self.tables[e.table][v];
            let row = villain.tables.row(street, (r.other_active(seat) - 1).min(2), usize::from(r.raises).min(2), r.situation(seat));
            let mut branches: Vec<(Act, [f64; 3])> = Vec::new();
            if !facing {
                branches.push((Act::Check, [1.0, 0.0, 0.0]));
                branches.push((Act::Bet(r.bet_to(seat, f64::from(villain.tables.bet_frac[street]))), [0.0, 1.0, 0.0]));
            } else {
                branches.push((Act::Fold, [1.0, 0.0, 0.0]));
                match raise_to {
                    Some(to) => {
                        branches.push((Act::Call, [0.0, 1.0, 0.0]));
                        branches.push((Act::Raise(to), [0.0, 0.0, 1.0]));
                    }
                    None => branches.push((Act::Call, [0.0, 1.0, 1.0])),
                }
            }
            let weight = |c: usize, col: &[f64; 3]| -> f64 {
                let p = &row[c];
                col[0] * f64::from(p[0]) + col[1] * f64::from(p[1]) + col[2] * f64::from(p[2])
            };
            let total: f64 = e.b[v].iter().zip(classes).filter(|(_, &c)| c != DEAD).map(|(w, _)| *w).sum();
            if total <= 0.0 {
                if facing {
                    val[i] = r.stacks[self.hero] + r.pot;
                    sum[i] = 1.0;
                } else {
                    push(Act::Check, i, 1.0, Entry { table: e.table, b: e.b.clone(), weight: e.weight, reach: e.reach });
                }
                continue;
            }
            let mut kept = false;
            for (act, col) in &branches {
                let mut mass = 0.0;
                for c in 0..NCOMBOS {
                    if classes[c] != DEAD {
                        mass += e.b[v][c] * weight(classes[c] as usize, col);
                    }
                }
                let p = mass / total;
                if (p as f32) < BRANCH_FLOOR || e.reach * p < self.cfg.reach_eps {
                    continue;
                }
                let mut b2 = e.b.clone();
                let carry = self.tables[e.table].len() + v;
                for c in 0..NCOMBOS {
                    let f = if classes[c] != DEAD { weight(classes[c] as usize, col) / p } else { 0.0 };
                    b2[v][c] = e.b[v][c] * f;
                    b2[carry][c] = e.b[carry][c] * f;
                }
                push(*act, i, p, Entry { table: e.table, b: b2, weight: e.weight * p, reach: e.reach * p });
                kept = true;
            }
            if !kept {
                let fallback = if facing { Act::Call } else { Act::Check };
                push(fallback, i, 1.0, Entry { table: e.table, b: e.b.clone(), weight: e.weight, reach: e.reach });
            }
        }
        for (act, who, entries) in groups {
            let mut r2 = r;
            r2.apply(seat, act);
            let vals = self.node(r2, street, turn, river, entries);
            for ((i, p), v) in who.into_iter().zip(vals) {
                val[i] += p * v;
                sum[i] += p;
            }
        }
        val.iter().zip(&sum).map(|(v, s)| v / s).collect()
    }

    fn street_end(&self, r: Round, street: usize, turn: usize, river: usize, mut set: Vec<Entry>) -> Vec<f64> {
        let n = set.len();
        if street == 2 {
            return set.iter().map(|e| showdown(self.hero, &self.tables[e.table], self.runouts, &r, turn, river, &e.b)).collect();
        }
        if street == 0 {
            for e in set.iter_mut() {
                let vs = &self.tables[e.table];
                if self.cfg.line_decorrelate {
                    for i in 0..vs.len() {
                        e.b[i] = e.b[vs.len() + i].clone();
                    }
                }
                let active: Vec<usize> = (0..vs.len()).filter(|&i| r.in_hand >> vs[i].seat & 1 == 1).collect();
                if self.cfg.line_decorrelate && active.len() >= 2 {
                    let mut vecs: Vec<Vec<f32>> = active.iter().map(|&i| e.b[i].iter().map(|&x| x as f32).collect()).collect();
                    decorrelate(&mut vecs, 2);
                    for (s, &i) in active.iter().enumerate() {
                        e.b[i] = vecs[s].iter().map(|&x| f64::from(x)).collect();
                    }
                }
            }
        }
        let r2 = r.next_street();
        let cards = if street == 1 { self.runouts.turns[turn].rivers.len() } else { self.runouts.turns.len() };
        let class_of = |k: usize| -> &[u8] {
            if street == 1 { &self.runouts.turns[turn].rivers[k].class } else { &self.runouts.turns[k].class }
        };
        let mut restricted: Vec<Vec<(Beliefs, f64)>> = Vec::with_capacity(n);
        let mut totals = vec![0.0; n];
        for (i, e) in set.iter().enumerate() {
            let row: Vec<(Beliefs, f64)> = (0..cards).map(|k| restrict(&self.tables[e.table], &r, &e.b, class_of(k))).collect();
            totals[i] = row.iter().map(|x| x.1).sum();
            restricted.push(row);
        }
        let mut val = vec![0.0; n];
        for k in 0..cards {
            let mut who = Vec::new();
            let mut child = Vec::new();
            for (i, e) in set.iter().enumerate() {
                let w = restricted[i][k].1;
                if w > 0.0 {
                    let frac = w / totals[i];
                    who.push((i, frac));
                    child.push(Entry { table: e.table, b: std::mem::take(&mut restricted[i][k].0), weight: e.weight * frac, reach: e.reach });
                }
            }
            if child.is_empty() {
                continue;
            }
            let (t2, r2c) = if street == 1 { (turn, k) } else { (k, 0) };
            let vals = self.node(r2, street + 1, t2, r2c, child);
            for ((i, frac), v) in who.into_iter().zip(vals) {
                val[i] += frac * v;
            }
        }
        for (i, v) in val.iter_mut().enumerate() {
            if totals[i] <= 0.0 {
                *v = r2.stacks[self.hero];
            }
        }
        val
    }
}

fn restrict(villains: &[Villain], r: &Round, b: &Beliefs, classes: &[u8]) -> (Beliefs, f64) {
    let mut out = b.clone();
    let mut w = 1.0;
    for (v, villain) in villains.iter().enumerate() {
        let mut total = 0.0;
        for i in 0..NCOMBOS {
            if classes[i] == DEAD {
                out[v][i] = 0.0;
            } else {
                total += out[v][i];
            }
        }
        if r.in_hand >> villain.seat & 1 == 1 {
            w *= total;
        }
    }
    (out, w)
}

fn showdown(hero: usize, villains: &[Villain], runouts: &Runouts, r: &Round, turn: usize, river: usize, b: &Beliefs) -> f64 {
    let rb = &runouts.turns[turn].rivers[river];
    let (mut beat, mut tie) = (Vec::new(), Vec::new());
    for (v, villain) in villains.iter().enumerate() {
        if r.in_hand >> villain.seat & 1 == 0 {
            continue;
        }
        let (mut t, mut wb, mut wt) = (0.0, 0.0, 0.0);
        for i in 0..NCOMBOS {
            if rb.class[i] == DEAD {
                continue;
            }
            let w = b[v][i];
            t += w;
            match rb.outcome[i] {
                2 => wb += w,
                1 => wt += w,
                _ => {}
            }
        }
        if t > 0.0 {
            beat.push(wb / t);
            tie.push(wt / t);
        }
    }
    r.stacks[hero] + hero_share(&beat, &tie) * r.pot
}
