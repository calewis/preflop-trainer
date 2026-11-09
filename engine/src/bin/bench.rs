//! Throughput benchmark on a fixed spot set, with per-phase timing.
//! Usage: bench THREADS PER_CELL [quick]
//! Cells are (street, villains left 1/2/3+). Reported means are weighted to the
//! library's target mix (street 40/35/25, players 1/3 each).

use poker_engine::prof;
use poker_engine::sim::simulate;
use poker_engine::spot::{grade_spot, Grade, GradeCfg, Spot, Verdict};
use poker_trainer::cards::Card;
use poker_trainer::eval::evaluate;
use poker_trainer::rng::Rng;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

const STREET_SHARE: [f64; 3] = [0.40, 0.35, 0.25];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let threads: usize = args.get(1).map_or(30, |s| s.parse().unwrap());
    let per_cell: usize = args.get(2).map_or(10, |s| s.parse().unwrap());
    let quick = args.get(3).map(String::as_str) == Some("quick");

    let mut rng = Rng::new(1);
    let cards: Vec<[Card; 7]> = (0..1000)
        .map(|_| {
            let mut d: Vec<Card> = (0..52u8).map(Card::from_index).collect();
            rng.shuffle(&mut d);
            [d[0], d[1], d[2], d[3], d[4], d[5], d[6]]
        })
        .collect();
    let t = Instant::now();
    let (mut acc, mut count) = (0u64, 0u64);
    while t.elapsed().as_millis() < 300 {
        for c in &cards {
            acc = acc.wrapping_add(u64::from(evaluate(c).field(0)));
            count += 1;
        }
    }
    println!("evaluator: {:.1} M evals/s (sink {acc})", count as f64 / t.elapsed().as_secs_f64() / 1e6);

    // Simulation throughput.
    let t = Instant::now();
    let mut hits = 0u64;
    let sim_hands = 20_000u64;
    let spots_all: Vec<Option<Spot>> = (0..sim_hands).map(|s| prof::span(prof::SIM, || simulate(s + 1)).map(|r| r.spot)).collect();
    let secs = t.elapsed().as_secs_f64();
    for s in &spots_all {
        hits += u64::from(s.is_some());
    }
    println!("simulate: {:.0} hands/s/thread, {:.1}% yield a spot", sim_hands as f64 / secs, 100.0 * hits as f64 / sim_hands as f64);

    // Fixed spot set: first PER_CELL spots per cell from seeds 1, 2, ...
    let mut counts = [[0usize; 3]; 3];
    let mut work: Vec<(usize, usize, Spot)> = Vec::new();
    let mut seed = 1u64;
    let need = per_cell * 9;
    while work.len() < need && seed < 100_000_000 {
        if let Some(r) = simulate(seed) {
            let street = r.spot.board.len() - 3;
            let nv = r.spot.replay().active_count().clamp(1, 3) - 1;
            if counts[street][nv] < per_cell {
                counts[street][nv] += 1;
                work.push((street, nv, r.spot));
            }
        }
        seed += 1;
    }

    if args.get(3).map(String::as_str) == Some("sustained") {
        poker_engine::memguard::start(45.0);
        if args.get(5).map(String::as_str) == Some("uncapped") {
            poker_engine::joint::set_tape_caps(usize::MAX, usize::MAX);
        }
        let secs: f64 = args.get(4).map_or(60.0, |s| s.parse().unwrap());
        sustained(&work, threads, secs);
        return;
    }

    let mut cfg = GradeCfg::standard();
    if quick {
        cfg.tables = 8;
        cfg.pool = 32;
        cfg.flop_samples = [(0, 0), (6, 4), (5, 3), (4, 3), (4, 3), (4, 3), (4, 3)];
        cfg.turn_samples = [0, 12, 8, 6, 6, 6, 6];
    }

    let next = AtomicU64::new(0);
    let phases = Mutex::new([0u64; prof::NPHASE]);
    let per = Mutex::new(Vec::new());
    let grades: Mutex<Vec<(f64, Grade)>> = Mutex::new(Vec::new());
    let wall = Instant::now();
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                prof::take();
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst) as usize;
                    if i >= work.len() {
                        break;
                    }
                    let t0 = Instant::now();
                    let g = grade_spot(&work[i].2, &cfg, 1000 + i as u64);
                    let dt = t0.elapsed().as_secs_f64();
                    per.lock().unwrap().push((work[i].0, work[i].1, dt, g.tables * g.options.len()));
                    grades.lock().unwrap().push((dt, g));
                }
                let t = prof::take();
                let mut p = phases.lock().unwrap();
                for i in 0..prof::NPHASE {
                    p[i] += t[i];
                }
            });
        }
    });
    let wall_secs = wall.elapsed().as_secs_f64();
    let per = per.into_inner().unwrap();
    let solves: usize = per.iter().map(|x| x.3).sum();
    println!("graded {} spots, wall {:.1}s on {} threads, {:.0} option-table solves/s", per.len(), wall_secs, threads, solves as f64 / wall_secs);

    let mut weighted = 0.0;
    for street in 0..3 {
        for nv in 0..3 {
            let v: Vec<f64> = per.iter().filter(|x| x.0 == street && x.1 == nv).map(|x| x.2).collect();
            if v.is_empty() {
                continue;
            }
            let mean = v.iter().sum::<f64>() / v.len() as f64;
            let max = v.iter().cloned().fold(0.0, f64::max);
            weighted += mean * STREET_SHARE[street] / 3.0;
            println!("  {} villains{}: n={} mean_s={mean:.3} max_s={max:.2}", ["flop", "turn", "river"][street], ["1", "2", "3+"][nv], v.len());
        }
    }
    println!("weighted mean thread-seconds per spot (target mix): {weighted:.3}");
    println!("projected spots/hour at this thread count: {:.0}", 3600.0 * threads as f64 / weighted * (wall_secs * threads as f64 / per.iter().map(|x| x.2).sum::<f64>()).min(1.0));
    let p = phases.into_inner().unwrap();
    let total = p[prof::SOLVE_TOTAL] + p[prof::POOL] + p[prof::BUCKETS];
    if total > 0 {
        let leaves = p[prof::FLOP_VEC] + p[prof::DECORR] + p[prof::BUILD_TURN] + p[prof::FILL_RIVER] + p[prof::SHOWDOWN] + p[prof::TAPE];
        let tree = p[prof::SOLVE_TOTAL].saturating_sub(leaves);
        let pct = |x: u64| 100.0 * x as f64 / total as f64;
        println!(
            "phase share of grading cycles: bucketing {:.1}% | root posterior {:.1}% | flop vectors {:.1}% | decorrelate {:.1}% | build_turn {:.1}% | fill_river {:.1}% | showdown {:.1}% | tape compile {:.1}% | tree recursion {:.1}%",
            pct(p[prof::BUCKETS]), pct(p[prof::POOL]), pct(p[prof::FLOP_VEC]), pct(p[prof::DECORR]), pct(p[prof::BUILD_TURN]), pct(p[prof::FILL_RIVER]), pct(p[prof::SHOWDOWN]), pct(p[prof::TAPE]), pct(tree)
        );
    }
    if !quick {
        analyse_shortcuts(&grades.into_inner().unwrap());
    }
}

/// Mean EV and best-share over the first `t` tables.
fn prefix(g: &Grade, t: usize) -> (Vec<f64>, Vec<f64>) {
    let m = g.options.len();
    let (mut mean, mut wins) = (vec![0.0; m], vec![0.0; m]);
    for row in &g.ev_by_table[..t] {
        let mut b = 0;
        for i in 0..m {
            mean[i] += row[i] / t as f64;
            if row[i] > row[b] + 1e-9 {
                b = i;
            }
        }
        wins[b] += 1.0 / t as f64;
    }
    (mean, wins)
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

/// Post-hoc estimate of two shortcuts, weighting each spot by its measured time.
fn analyse_shortcuts(grades: &[(f64, Grade)]) {
    let total: f64 = grades.iter().map(|x| x.0).sum();
    for &(min_cp, min_gap) in &[(16usize, 0.05f64), (24, 0.08), (32, 0.10), (40, 0.10)] {
        let (mut saved, mut stopped, mut wrong_best, mut wrong_verdict) = (0.0, 0, 0, 0);
        for (dt, g) in grades {
            let t = g.tables;
            for &cp in &[16usize, 24, 32, 40, 48, 56] {
                if cp >= t || cp < min_cp {
                    continue;
                }
                let (mean, wins) = prefix(g, cp);
                let lead = argmax(&mean);
                let mut sorted = mean.clone();
                sorted.sort_by(|a, b| b.partial_cmp(a).unwrap());
                if wins[lead] >= 0.999 && sorted.len() > 1 && (sorted[0] - sorted[1]) / g.pot >= min_gap {
                    stopped += 1;
                    saved += dt * (1.0 - cp as f64 / t as f64);
                    wrong_best += usize::from(lead != g.best);
                    wrong_verdict += usize::from(g.verdict != Verdict::Robust);
                    break;
                }
            }
        }
        println!("shortcut early-stop (from {min_cp} tables, gap>={:.0}% pot): would stop {stopped}/{} spots, saves {:.1}% of time; best changes {wrong_best}, verdict not robust at 64: {wrong_verdict}", min_gap * 100.0, grades.len(), 100.0 * saved / total);
    }
    let (mut drop_saved, mut dropped, mut drop_harm) = (0.0, 0, 0);
    for (dt, g) in grades {
        let t = g.tables;
        if t > 8 {
            let (mean, wins) = prefix(g, 8);
            let lead = argmax(&mean);
            let paid: Vec<usize> = (0..mean.len()).filter(|&i| !matches!(g.options[i].act, poker_engine::game::Act::Fold)).collect();
            let gone: Vec<usize> = paid.iter().cloned().filter(|&i| i != lead && wins[i] == 0.0 && (mean[lead] - mean[i]) / g.pot >= 0.30).collect();
            if !gone.is_empty() {
                dropped += gone.len();
                drop_saved += dt * (gone.len() as f64 / paid.len() as f64) * (1.0 - 8.0 / t as f64);
                for &i in &gone {
                    if g.ev_by_table[8..].iter().any(|row| argmax(row) == i) {
                        drop_harm += 1;
                    }
                }
            }
        }
    }
    println!("shortcut drop-dominated (>=30% pot behind after 8 tables): {dropped} options dropped, saves {:.1}% of time; later best in some table: {drop_harm}", 100.0 * drop_saved / total);
}

/// Fixed-duration throughput on the expensive flop spots, cycled in a fixed
/// order so every thread configuration runs the same job mix. Counts jobs
/// finished inside the window, so a thread that is still busy at the end does
/// not distort the rate.
fn sustained(work: &[(usize, usize, Spot)], threads: usize, secs: f64) {
    let jobs: Vec<&Spot> = work.iter().filter(|x| x.0 == 0).map(|x| &x.2).collect();
    let cfg = GradeCfg::standard();
    let next = AtomicU64::new(0);
    let done = AtomicU64::new(0);
    let t0 = Instant::now();
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                if t0.elapsed().as_secs_f64() >= secs {
                    break;
                }
                let i = next.fetch_add(1, Ordering::SeqCst) as usize;
                let job = jobs[i % jobs.len()];
                let g = grade_spot(job, &cfg, 7777 + (i % jobs.len()) as u64);
                std::hint::black_box(g.tables);
                if t0.elapsed().as_secs_f64() <= secs {
                    done.fetch_add(1, Ordering::SeqCst);
                }
            });
        }
    });
    let n = done.load(Ordering::SeqCst);
    println!("sustained: threads={threads} jobs_completed={n} in {secs:.0}s = {:.1} flop-spots/hour", n as f64 * 3600.0 / secs);
    let (nodes, villains, acts, node_b, act_b) = poker_engine::joint::tape_peaks();
    let gb = |x: usize| x as f64 / 1_073_741_824.0;
    println!(
        "tape cache, largest single-thread: nodes={nodes} villain_nodes={villains} action_entries={acts}; {:.2} GB now, {:.2} GB under the old 64-entries-per-node layout",
        gb(nodes * node_b + acts * act_b),
        gb(nodes * node_b + villains * 64 * act_b)
    );
    println!("peak RSS {:.2} GB", poker_engine::memguard::peak_rss_gb());
}
