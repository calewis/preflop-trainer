//! Runs the validation gates and prints a report.
//! Usage: validate THREADS BLOCKER_SPOTS RUNOUT_SPOTS DIRECTION_SPOTS

use poker_engine::models::{ANCHORS, ARCHETYPES};
use poker_engine::sim::simulate;
use poker_engine::solve::{solve_root, Villain};
use poker_engine::joint::solve_joint_fast;
use poker_engine::reference::{solve_joint, solve_reference};
use poker_engine::spot::{build_runouts, grade_spot, seat_posteriors, table_beliefs, verdict_stats, villains_for, GradeCfg, HistAct, Spot};
use poker_engine::game::Act;
use poker_trainer::cards::{parse_cards, parse_holding};
use poker_engine::validation::blocker::compare;
use poker_engine::validation::direction::{air_bluff, big_vs_small, bluff_vs_field, thin_value_gains};
use poker_trainer::rng::Rng;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

fn percentile(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((p * (v.len() - 1) as f64).round()) as usize]
}

fn parallel<T: Send>(n: usize, threads: usize, f: impl Fn(usize) -> Option<T> + Sync) -> Vec<T> {
    let next = AtomicU64::new(0);
    let out = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::SeqCst) as usize;
                if i >= n {
                    break;
                }
                if let Some(v) = f(i) {
                    out.lock().unwrap().push((i, v));
                }
            });
        }
    });
    let mut items = out.into_inner().unwrap();
    items.sort_by_key(|x| x.0);
    items.into_iter().map(|x| x.1).collect()
}

/// Simulated spots with a given number of active villains at a given street
/// (board length). Seeds are deterministic.
fn collect_spots(want: usize, board_len: usize, villains: usize, seed0: u64) -> Vec<Spot> {
    let mut out = Vec::new();
    let mut seed = seed0;
    while out.len() < want && seed < seed0 + 200_000_000 {
        if let Some(r) = simulate(seed) {
            if r.spot.board.len() == board_len {
                let rp = r.spot.replay();
                if rp.active_count() == villains {
                    out.push(r.spot);
                }
            }
        }
        seed += 1;
    }
    out
}

fn blocker_gate(threads: usize, want: usize) {
    let run = |villains: usize, seed0: u64, label: &str| {
        let spots = collect_spots(want, 5, villains, seed0);
        let results = parallel(spots.len(), threads, |i| {
            let spot = &spots[i];
            let mut rng = Rng::new(0xB10C + i as u64);
            let rp = spot.replay();
            let mut cfg = GradeCfg::standard();
            cfg.tables = 1;
            cfg.pool = 16;
            let pools = seat_posteriors(spot, &rp, &cfg, &mut rng);
            let parts: Vec<(Vec<f32>, Vec<f32>)> = pools.iter().map(|p| rp.line_parts(spot, p.seat(), p.chosen_model(0))).collect();
            let c = compare(spot.hero, &spot.board, &parts);
            Some((100.0 * (c.factored - c.exact).abs(), 100.0 * (c.corrected - c.exact).abs(), 100.0 * (c.along_line - c.exact).abs()))
        });
        let summarize = |mut err: Vec<f64>, name: &str| {
            let n = err.len();
            let mean = err.iter().sum::<f64>() / n as f64;
            let p50 = percentile(&mut err, 0.5);
            let p90 = percentile(&mut err, 0.9);
            let p95 = percentile(&mut err, 0.95);
            let p99 = percentile(&mut err, 0.99);
            let max = err[n - 1];
            let pass = p50 < 1.0 && p95 < 2.0;
            println!("blocker {label} {name}: spots={n} mean={mean:.3}% p50={p50:.3}% p90={p90:.3}% p95={p95:.3}% p99={p99:.3}% max={max:.3}% {}", if pass { "PASS" } else { "FAIL" });
        };
        summarize(results.iter().map(|x| x.0).collect(), "plain-factored");
        summarize(results.iter().map(|x| x.1).collect(), "corrected-at-root");
        summarize(results.iter().map(|x| x.2).collect(), "corrected-along-line");
    };
    run(2, 100_000, "3-way(2 villains)");
    run(3, 200_000, "4-way(3 villains)");
}

fn runout_gate(threads: usize, want: usize) {
    let flop_hu_3w: &[(usize, usize)] = &[(8, 6), (16, 10), (24, 10), (47, 6), (47, 10), (47, 20)];
    let flop_4w: &[(usize, usize)] = &[(12, 6), (16, 6), (24, 8), (32, 10), (47, 10)];
    let turn_samples: &[usize] = &[8, 12, 23, 46];
    let run = |board_len: usize, villains: usize, label: &str, seed0: u64, flop_samples: &[(usize, usize)], want: usize| {
        let spots = collect_spots(want, board_len, villains, seed0);
        let results = parallel(spots.len(), threads, |i| {
            let spot = &spots[i];
            let rp = spot.replay();
            let mut cfg = GradeCfg::standard();
            cfg.tables = 1;
            cfg.pool = 16;
            cfg.sample_runouts = false;
            let mut rng = Rng::new(0xC0DE + i as u64);
            let pools = seat_posteriors(spot, &rp, &cfg, &mut rng);
            let round = rp.round;
            let options = round.root_options(spot.hero_seat);
            let acts: Vec<_> = options.iter().map(|x| x.0).collect();
            let (base, corrected) = table_beliefs(&pools, 0);
            let villains_ref = villains_for(&pools, 0, &base, &corrected);
            let full = {
                let runouts = build_runouts(spot, &cfg, rp.active_count(), &mut Rng::new(1));
                solve_root(&round, spot.hero_seat, &villains_ref, &runouts, cfg.solve, &acts)
            };
            let best_full = argmax(&full);
            let mut errs = Vec::new();
            let targets: Vec<(usize, usize)> = if board_len == 4 {
                turn_samples.iter().map(|&j| (0, j)).collect()
            } else {
                flop_samples.to_vec()
            };
            for &(k, j) in &targets {
                let mut scfg = cfg.clone();
                scfg.sample_runouts = true;
                scfg.flop_samples = [(k, j); 7];
                scfg.turn_samples = [j; 7];
                let (mut abs_err, mut gap_err, mut flip) = (0.0, 0.0, 0.0);
                let repeats = 3u64;
                for rep in 0..repeats {
                    let runouts = build_runouts(spot, &scfg, rp.active_count(), &mut Rng::new(0xABC + rep + i as u64 * 7));
                    let ev = solve_root(&round, spot.hero_seat, &villains_ref, &runouts, cfg.solve, &acts);
                    let m = ev.len() as f64;
                    abs_err += ev.iter().zip(&full).map(|(a, b)| (a - b).abs()).sum::<f64>() / m;
                    // Error in each option's gap to the full-enumeration best option.
                    gap_err += ev.iter().zip(&full).map(|(a, b)| ((a - ev[best_full]) - (b - full[best_full])).abs()).sum::<f64>() / m;
                    if argmax(&ev) != best_full {
                        flip += 1.0;
                    }
                }
                let n = repeats as f64;
                errs.push((100.0 * abs_err / n / round.pot, 100.0 * gap_err / n / round.pot, flip / n));
            }
            Some(errs)
        });
        let n = results.len();
        let m = results[0].len();
        println!("runout error {label} (percent of pot): spots={n}");
        for j in 0..m {
            let mut abs: Vec<f64> = results.iter().map(|r| r[j].0).collect();
            let mut gap: Vec<f64> = results.iter().map(|r| r[j].1).collect();
            let flip: f64 = results.iter().map(|r| r[j].2).sum::<f64>() / n as f64;
            let mabs = abs.iter().sum::<f64>() / n as f64;
            let mgap = gap.iter().sum::<f64>() / n as f64;
            let p90g = percentile(&mut gap, 0.9);
            let p90a = percentile(&mut abs, 0.9);
            let lbl = if board_len == 4 { format!("rivers={}", turn_samples[j]) } else { format!("turns={},rivers={}", flop_samples[j].0, flop_samples[j].1) };
            println!("  {lbl}: abs mean={mabs:.2}% p90={p90a:.2}% | option-gap mean={mgap:.2}% p90={p90g:.2}% | best-option flip={flip:.3}");
        }
    };
    run(3, 1, "flop heads-up", 300_000, flop_hu_3w, want);
    run(3, 2, "flop 3-way", 400_000, flop_hu_3w, want);
    run(3, 3, "flop 4-way", 450_000, flop_4w, want / 2);
    run(4, 1, "turn heads-up", 500_000, flop_hu_3w, want);
    run(4, 2, "turn 3-way", 600_000, flop_hu_3w, want);
}

/// Effect of skipping the flop-to-turn card-removal correction: EV difference
/// on multiway flop spots, as percent of the pot, per option and best-option flips.
fn decor_experiment(threads: usize, want: usize) {
    for (villains, seed0) in [(2usize, 700_000u64), (3, 800_000)] {
        let spots = collect_spots(want, 3, villains, seed0);
        let res = parallel(spots.len(), threads, |i| {
            let spot = &spots[i];
            let rp = spot.replay();
            let mut cfg = GradeCfg::standard();
            cfg.tables = 4;
            cfg.pool = 16;
            cfg.flop_samples = [(0, 0), (47, 10), (24, 8), (16, 4), (12, 3), (12, 3), (12, 3)];
            let mut rng = Rng::new(0xDE + i as u64);
            let pools = seat_posteriors(spot, &rp, &cfg, &mut rng);
            let runouts = build_runouts(spot, &cfg, rp.active_count(), &mut rng);
            let options = rp.round.root_options(spot.hero_seat);
            let acts: Vec<_> = options.iter().map(|x| x.0).collect();
            let (mut diff, mut flips, mut n) = (0.0, 0.0, 0.0);
            for t in 0..cfg.tables {
                let (base, corrected) = table_beliefs(&pools, t);
                let vs = villains_for(&pools, t, &base, &corrected);
                let mut with = cfg.solve;
                with.line_decorrelate = true;
                let mut without = cfg.solve;
                without.line_decorrelate = false;
                let a = solve_root(&rp.round, spot.hero_seat, &vs, &runouts, with, &acts);
                let b = solve_root(&rp.round, spot.hero_seat, &vs, &runouts, without, &acts);
                diff += a.iter().zip(&b).map(|(x, y)| (x - y).abs()).sum::<f64>() / a.len() as f64 / rp.round.pot;
                flips += f64::from(argmax(&a) != argmax(&b));
                n += 1.0;
            }
            Some((100.0 * diff / n, flips / n))
        });
        let mut d: Vec<f64> = res.iter().map(|x| x.0).collect();
        let f: f64 = res.iter().map(|x| x.1).sum::<f64>() / res.len() as f64;
        let mean = d.iter().sum::<f64>() / d.len() as f64;
        let p50 = percentile(&mut d, 0.5);
        let p95 = percentile(&mut d, 0.95);
        let max = d[d.len() - 1];
        println!("line-decorrelate effect, villains={villains}: spots={} mean EV change={mean:.3}% pot p50={p50:.3}% p95={p95:.3}% max={max:.3}%, best-option flips={f:.3}", d.len());
    }
}

/// Strategy-fusion gate: per-table best response versus hero committing to one
/// action per public information state across tables, on the naive solver.
fn fusion_gate(threads: usize, scale: usize) {
    // (board, villains, spots, tables, turns, rivers); same runouts for both solves.
    let cases: [(usize, usize, usize, usize, usize, usize); 6] = [
        (3, 1, 20, 8, 8, 4),
        (3, 2, 20, 8, 4, 3),
        (4, 1, 18, 12, 0, 12),
        (4, 2, 18, 12, 0, 8),
        (5, 1, 12, 16, 0, 0),
        (5, 2, 12, 16, 0, 0),
    ];
    let mut all: Vec<(usize, usize, Vec<f64>, Vec<bool>, bool, bool, f64, bool)> = Vec::new();
    for (ci, &(board, villains, want, tables, turns, rivers)) in cases.iter().enumerate() {
        let want = (want * scale).div_ceil(100).max(1);
        let spots = collect_spots(want, board, villains, 900_000 + ci as u64 * 100_000);
        let res = parallel(spots.len(), threads, |i| {
            let spot = &spots[i];
            let rp = spot.replay();
            let mut cfg = GradeCfg::standard();
            cfg.tables = tables;
            cfg.pool = 64;
            cfg.flop_samples = [(turns, rivers); 7];
            cfg.turn_samples = [rivers; 7];
            let mut rng = Rng::new(0xF5 + i as u64 * 11 + ci as u64 * 1000);
            let pools = seat_posteriors(spot, &rp, &cfg, &mut rng);
            let runouts = build_runouts(spot, &cfg, rp.active_count(), &mut rng);
            let opts = rp.round.root_options(spot.hero_seat);
            let acts: Vec<_> = opts.iter().map(|x| x.0).collect();
            let beliefs: Vec<(Vec<Vec<f32>>, Vec<Vec<f32>>)> = (0..tables).map(|t| table_beliefs(&pools, t)).collect();
            let table_sets: Vec<Vec<Villain>> = (0..tables).map(|t| villains_for(&pools, t, &beliefs[t].0, &beliefs[t].1)).collect();
            let joint = solve_joint(&rp.round, spot.hero_seat, &table_sets, &runouts, cfg.solve, &acts);
            let per: Vec<Vec<f64>> = table_sets.iter().map(|vs| solve_reference(&rp.round, spot.hero_seat, vs, &runouts, cfg.solve, &acts)).collect();
            let pot = rp.round.pot;
            let a = verdict_stats(&per, pot);
            let b = verdict_stats(&joint, pot);
            // The solver actually used for grading, against the naive joint solve.
            let fast = solve_joint_fast(&rp.round, spot.hero_seat, &table_sets, &runouts, cfg.solve, &acts);
            let c = verdict_stats(&fast, pot);
            let fast_diff = c.mean.iter().zip(&b.mean).map(|(x, y)| 100.0 * (x - y).abs() / pot).fold(0.0, f64::max);
            let fast_changed = c.best != b.best || c.verdict != b.verdict;
            let infl: Vec<f64> = a.mean.iter().zip(&b.mean).map(|(x, y)| 100.0 * (x - y) / pot).collect();
            let folds: Vec<bool> = acts.iter().map(|x| matches!(x, poker_engine::game::Act::Fold)).collect();
            Some((board, villains, infl, folds, a.best != b.best, a.verdict != b.verdict, fast_diff, fast_changed))
        });
        all.extend(res);
    }
    let report = |label: &str, f: &dyn Fn(usize, usize) -> bool| {
        let sel: Vec<_> = all.iter().filter(|x| f(x.0, x.1)).collect();
        if sel.is_empty() {
            return;
        }
        let (mut cont, mut folds, mut best, mut verdict) = (Vec::new(), Vec::new(), 0.0, 0.0);
        for s in &sel {
            for (x, f) in s.2.iter().zip(&s.3) {
                if *f { folds.push(*x) } else { cont.push(*x) }
            }
            best += f64::from(s.4);
            verdict += f64::from(s.5);
        }
        let n = sel.len() as f64;
        let mean_c = cont.iter().sum::<f64>() / cont.len().max(1) as f64;
        let mean_f = folds.iter().sum::<f64>() / folds.len().max(1) as f64;
        let p95 = percentile(&mut cont, 0.95);
        println!("fusion {label}: spots={} continuing-line inflation mean={mean_c:.2}% pot p95={p95:.2}% | fold mean={mean_f:.2}% | best-option changes {:.1}% | verdict changes {:.1}%", sel.len(), 100.0 * best / n, 100.0 * verdict / n);
    };
    let maxd = all.iter().map(|x| x.6).fold(0.0, f64::max);
    let changed = all.iter().filter(|x| x.7).count();
    println!("fusion after correction: fast joint vs naive joint on {} spots: max mean-EV difference {maxd:.2e}% pot, best/verdict changes {changed}", all.len());
    report("per-table vs joint, ALL", &|_, _| true);
    report("flop heads-up", &|b, v| b == 3 && v == 1);
    report("flop 3-way", &|b, v| b == 3 && v == 2);
    report("turn heads-up", &|b, v| b == 4 && v == 1);
    report("turn 3-way", &|b, v| b == 4 && v == 2);
    report("river heads-up", &|b, v| b == 5 && v == 1);
    report("river 3-way", &|b, v| b == 5 && v == 2);
}

fn argmax(v: &[f64]) -> usize {
    let mut b = 0;
    for i in 0..v.len() {
        if v[i] > v[b] {
            b = i;
        }
    }
    b
}

fn direction_gate(n: usize) {
    let (a, b) = thin_value_gains(n, 11);
    println!("direction thin value bet67 minus check %pot: station {a:.2} nit {b:.2} {}", ok(a > b));
    let (nit, station) = air_bluff(n, 13);
    println!("direction air bluff bet67 minus check %pot: nit {:.2} (profitable {:.1}%), station {:.2} (profitable {:.1}%) {}", nit.0, nit.1 * 100.0, station.0, station.1 * 100.0, ok(nit.0 > station.0 && nit.1 >= station.1));
    let fv = bluff_vs_field(n, 17);
    println!("direction air bluff vs field size (1,2,3 villains) %pot: {:.2} {:.2} {:.2} {}", fv[0], fv[1], fv[2], ok(fv[0] > fv[1] && fv[1] > fv[2]));
    let (s, ni) = big_vs_small(n, 19);
    println!("direction big bet minus small bet (strong hands) %pot: station {s:.2} nit {ni:.2} {}", ok(s > ni));
}

/// Sanity display: the same river line with a third-pot, two-thirds and pot bet.
/// Bet size should shift belief about the player's type but not prove it.
fn size_check() {
    let hero = parse_holding("AdQc").unwrap();
    let board = parse_cards("As9h5c3d2s").unwrap();
    for (label, frac) in [("third-pot", 1.0 / 3.0), ("two-thirds", 2.0 / 3.0), ("pot", 1.0)] {
        // BTN opens, BB calls, checked to the river, BB bets into the button.
        let pot = 6.5;
        let mut history = vec![
            HistAct { street: 0, seat: 2, act: Act::Fold },
            HistAct { street: 0, seat: 3, act: Act::Fold },
            HistAct { street: 0, seat: 4, act: Act::Raise(3.0) },
            HistAct { street: 0, seat: 0, act: Act::Fold },
            HistAct { street: 0, seat: 1, act: Act::Call },
        ];
        for street in 1..=2u8 {
            history.push(HistAct { street, seat: 1, act: Act::Check });
            history.push(HistAct { street, seat: 4, act: Act::Check });
        }
        history.push(HistAct { street: 3, seat: 1, act: Act::Bet(pot * frac) });
        let spot = Spot { n: 5, hero_seat: 4, stack: 100.0, hero, board: board.clone(), history };
        let g = grade_spot(&spot, &GradeCfg::standard(), 42);
        let arch: Vec<String> = g.breakdown.iter().map(|b| format!("{} {:.0}%", ARCHETYPES[b.archetype].name, 100.0 * b.weight)).collect();
        println!("size-check {label} bet: main-opponent posterior [{}], best {} ({}), ess {:.0}", arch.join(", "), g.options[g.best].label, g.verdict.code(), g.ess[0]);
    }
}

fn ok(b: bool) -> &'static str {
    if b {
        "PASS"
    } else {
        "FAIL"
    }
}

fn population_stats(threads: usize, hands: usize) {
    // Empirical VPIP/PFR of each archetype in full-population simulations: drawn
    // by replaying each sampled hand's preflop actions by seat archetype.
    let res = parallel(hands, threads, |i| Some(poker_engine::sim::preflop_population(0x7000_0000 + i as u64)));
    for a in 0..ARCHETYPES.len() {
        let (mut seats, mut vp, mut pf) = (0u64, 0u64, 0u64);
        for hand in &res {
            for &(arch, v, p) in hand {
                if arch == a {
                    seats += 1;
                    vp += u64::from(v);
                    pf += u64::from(p);
                }
            }
        }
        println!(
            "archetype {}: hands={} VPIP={:.1}% (anchor {:.0}) PFR={:.1}% (anchor {:.0})",
            ARCHETYPES[a].name,
            seats,
            100.0 * vp as f64 / seats as f64,
            ANCHORS[a].0,
            100.0 * pf as f64 / seats as f64,
            ANCHORS[a].1
        );
    }

}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let threads: usize = args.get(1).map_or(30, |s| s.parse().unwrap());
    let blocker: usize = args.get(2).map_or(2000, |s| s.parse().unwrap());
    let runout: usize = args.get(3).map_or(200, |s| s.parse().unwrap());
    let direction: usize = args.get(4).map_or(500, |s| s.parse().unwrap());
    if args.get(5).map(String::as_str) == Some("fusion") {
        fusion_gate(threads, args.get(6).map_or(100, |s| s.parse().unwrap()));
        return;
    }
    if args.get(5).map(String::as_str) == Some("sizes") {
        size_check();
        return;
    }
    if args.get(5).map(String::as_str) == Some("decor") {
        decor_experiment(threads, 30);
        return;
    }
    if blocker > 0 {
        blocker_gate(threads, blocker);
    }
    if runout > 0 {
        runout_gate(threads, runout);
    }
    if direction > 0 {
        direction_gate(direction);
    }
    population_stats(threads, 400_000);
}
