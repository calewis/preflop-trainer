//! Generates the training spot library.
//! Usage: library SEED HANDS QUICK_GRADES FULL_GRADES TARGET THREADS OUTPUT [MAX_VILLAINS [BATCHES_PER_HALF]]
//! Full grades use two independent halves of BATCHES_PER_HALF batches of 64 tables;
//! only spots whose halves agree on best option and verdict are kept.
//!
//! Phase 0 simulates hands and buckets captured spots into strata (street, pot
//! type, players left). Phase 1 grades candidates cheaply and drops spots with
//! no real choice. Phase 2 grades the survivors at full resolution. Selection
//! then fills each stratum, preferring depends spots up to a cap. Everything
//! is order-independent given the seed, so the file is reproducible.

use poker_engine::game::Act;
use poker_engine::output::{format_header, spot_line};
use poker_engine::sim::simulate;
use poker_engine::spot::{grade_split, grade_spot, Grade, GradeCfg, SplitGrade, Spot, Verdict};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

const STREET_SHARE: [f64; 3] = [0.40, 0.35, 0.25];
const NCELLS: usize = 27;
/// Teachable: a different action class within this much of the pot of the best.
const REAL_CHOICE_PCT: f64 = 25.0;
/// Looser threshold for the quick pass, to tolerate its noise.
const QUICK_CHOICE_PCT: f64 = 40.0;
/// Target ceiling for depends spots per stratum when something else is available.
const DEPENDS_CAP: f64 = 0.60;

fn mix(a: u64, b: u64) -> u64 {
    let mut z = a ^ b.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn cell_of(street: usize, pot_type: usize, players: usize) -> usize {
    street * 9 + pot_type * 3 + players
}

fn cell_weight(cell: usize) -> f64 {
    STREET_SHARE[cell / 9] / 9.0
}

fn street_of(spot: &Spot) -> usize {
    spot.board.len() - 3
}

fn allocate(total: usize, avail: &[usize; NCELLS]) -> [usize; NCELLS] {
    let mut out = [0usize; NCELLS];
    let mut remaining = total as f64;
    let mut open: Vec<usize> = (0..NCELLS).filter(|&c| avail[c] > 0).collect();
    for _ in 0..8 {
        if remaining < 1.0 || open.is_empty() {
            break;
        }
        let wsum: f64 = open.iter().map(|&c| cell_weight(c)).sum();
        let mut left = 0.0;
        for &c in &open {
            let want = remaining * cell_weight(c) / wsum;
            let extra = want.floor().min((avail[c] - out[c]) as f64);
            out[c] += extra as usize;
            left += want - extra;
        }
        remaining = left;
        open.retain(|&c| out[c] < avail[c]);
    }
    out
}

fn action_class(a: Act) -> u8 {
    match a {
        Act::Fold => 0,
        Act::Check | Act::Call => 1,
        _ => 2,
    }
}

fn teachable(g: &Grade, limit_pct: f64) -> bool {
    if g.verdict == Verdict::Depends {
        return true;
    }
    let best_class = action_class(g.options[g.best].act);
    let best_ev = g.options[g.best].mean_ev;
    g.options
        .iter()
        .any(|o| action_class(o.act) != best_class && 100.0 * (best_ev - o.mean_ev) / g.pot <= limit_pct)
}

fn grade_all<T: Send>(spots: &[(Spot, u64)], threads: usize, label: &str, f: impl Fn(&Spot, u64) -> T + Sync) -> Vec<T> {
    let next = AtomicU64::new(0);
    let results: Mutex<Vec<(usize, T)>> = Mutex::new(Vec::with_capacity(spots.len()));
    let start = Instant::now();
    std::thread::scope(|sc| {
        for _ in 0..threads {
            sc.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::SeqCst) as usize;
                if i >= spots.len() {
                    break;
                }
                let g = f(&spots[i].0, spots[i].1);
                let mut r = results.lock().unwrap();
                r.push((i, g));
                let done = r.len();
                drop(r);
                if done % 500 == 0 {
                    eprintln!("{label}: {done}/{} in {:.0}s, RSS {:.2} GB", spots.len(), start.elapsed().as_secs_f64(), poker_engine::memguard::rss_gb());
                }
            });
        }
    });
    let mut r = results.into_inner().unwrap();
    r.sort_by_key(|x| x.0);
    eprintln!("{label}: done {} in {:.0}s", spots.len(), start.elapsed().as_secs_f64());
    r.into_iter().map(|x| x.1).collect()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    assert!((8..=10).contains(&args.len()), "usage: library SEED HANDS QUICK FULL TARGET THREADS OUTPUT [MAX_VILLAINS [BATCHES_PER_HALF]]");
    let seed: u64 = args[1].parse().unwrap();
    let hands: u64 = args[2].parse().unwrap();
    let quick: usize = args[3].parse().unwrap();
    let full: usize = args[4].parse().unwrap();
    let target: usize = args[5].parse().unwrap();
    let threads: usize = args[6].parse().unwrap();
    let output = &args[7];
    // Spots with more villains still in than this are excluded (see README).
    let max_villains: usize = args.get(8).map_or(6, |s| s.parse().unwrap());
    let batches: usize = args.get(9).map_or(1, |s| s.parse().unwrap());
    // Stop and report rather than risk the OOM killer on a shared machine.
    poker_engine::memguard::start(45.0);

    // Phase 0: simulate.
    let start = Instant::now();
    let per_cell_cap = 20_000usize;
    let counter = AtomicU64::new(0);
    let collected: Mutex<Vec<(u64, Spot, usize)>> = Mutex::new(Vec::new());
    std::thread::scope(|sc| {
        for _ in 0..threads {
            sc.spawn(|| loop {
                let idx = counter.fetch_add(1, Ordering::SeqCst);
                if idx >= hands {
                    break;
                }
                let hand_seed = mix(seed, idx);
                if let Some(res) = simulate(hand_seed) {
                    let rp = res.spot.replay();
                    if rp.active_count() > max_villains {
                        continue;
                    }
                    let players = rp.active_count().min(3).saturating_sub(1);
                    let cell = cell_of(street_of(&res.spot), rp.pot_type as usize, players);
                    collected.lock().unwrap().push((idx, res.spot, cell));
                }
            });
        }
    });
    let mut candidates = collected.into_inner().unwrap();
    candidates.sort_by_key(|x| x.0);
    let mut by_cell: Vec<Vec<(u64, Spot)>> = (0..NCELLS).map(|_| Vec::new()).collect();
    // Truncate after sorting, so the kept candidates do not depend on thread timing.
    for (idx, spot, cell) in candidates {
        if by_cell[cell].len() < per_cell_cap {
            by_cell[cell].push((idx, spot));
        }
    }
    let mut avail = [0usize; NCELLS];
    for c in 0..NCELLS {
        avail[c] = by_cell[c].len();
    }
    eprintln!("simulated {hands} hands in {:.0}s; candidates by cell {avail:?}; RSS {:.2} GB", start.elapsed().as_secs_f64(), poker_engine::memguard::rss_gb());

    // Phase 1: quick grade.
    let quick_alloc = allocate(quick, &avail);
    let mut quick_spots: Vec<(Spot, u64)> = Vec::new();
    let mut quick_info: Vec<(usize, u64)> = Vec::new();
    for c in 0..NCELLS {
        for (idx, spot) in by_cell[c].iter().take(quick_alloc[c]) {
            quick_spots.push((spot.clone(), mix(seed ^ 0xA5A5, *idx)));
            quick_info.push((c, *idx));
        }
    }
    let mut qcfg = GradeCfg::standard();
    qcfg.tables = 8;
    qcfg.pool = 32;
    qcfg.flop_samples = [(0, 0), (6, 4), (5, 3), (4, 3), (4, 3), (4, 3), (4, 3)];
    qcfg.turn_samples = [0, 12, 8, 6, 6, 6, 6];
    let quick_grades = grade_all(&quick_spots, threads, "quick", |s, seed| grade_spot(s, &qcfg, seed));
    let mut survivors: Vec<Vec<usize>> = (0..NCELLS).map(|_| Vec::new()).collect();
    for (i, g) in quick_grades.iter().enumerate() {
        if teachable(g, QUICK_CHOICE_PCT) {
            survivors[quick_info[i].0].push(i);
        }
    }
    let mut surv_counts = [0usize; NCELLS];
    for c in 0..NCELLS {
        surv_counts[c] = survivors[c].len();
    }
    eprintln!("quick survivors by cell {surv_counts:?}");

    // Phase 2: full grade.
    let full_alloc = allocate(full, &surv_counts);
    let mut full_spots: Vec<(Spot, u64)> = Vec::new();
    let mut full_cell: Vec<usize> = Vec::new();
    for c in 0..NCELLS {
        for &i in survivors[c].iter().take(full_alloc[c]) {
            full_spots.push((quick_spots[i].0.clone(), mix(seed ^ 0x5A5A, quick_info[i].1)));
            full_cell.push(c);
        }
    }
    let cfg = final_cfg();
    let t2 = Instant::now();
    let full_grades: Vec<SplitGrade> = grade_all(&full_spots, threads, "full", |s, seed| grade_split(s, &cfg, batches, seed));
    let full_secs = t2.elapsed().as_secs_f64();

    // Selection.
    let mut sel_avail = [0usize; NCELLS];
    let mut teach: Vec<Vec<usize>> = (0..NCELLS).map(|_| Vec::new()).collect();
    // Retention tallies: (agreeing, all) over teachable candidates, by street and verdict.
    let mut by_street = [(0usize, 0usize); 3];
    let mut by_verdict = [(0usize, 0usize); 3];
    let (mut pool_mismatch, mut teachable_n) = (0usize, 0usize);
    for (i, sg) in full_grades.iter().enumerate() {
        if !teachable(&sg.pooled, REAL_CHOICE_PCT) {
            continue;
        }
        teachable_n += 1;
        let street = full_cell[i] / 9;
        let v = sg.pooled.verdict as usize;
        by_street[street].1 += 1;
        by_verdict[v].1 += 1;
        if sg.agree {
            by_street[street].0 += 1;
            by_verdict[v].0 += 1;
            if !sg.pool_matches {
                pool_mismatch += 1;
            }
            teach[full_cell[i]].push(i);
        }
    }
    for c in 0..NCELLS {
        sel_avail[c] = teach[c].len();
    }
    let quota = allocate(target, &sel_avail);
    let mut chosen: Vec<usize> = Vec::new();
    let mut verdict_counts = [0usize; 3];
    for c in 0..NCELLS {
        let q = quota[c];
        if q == 0 {
            continue;
        }
        let (depends, others): (Vec<usize>, Vec<usize>) = teach[c].iter().partition(|&&i| full_grades[i].pooled.verdict == Verdict::Depends);
        let dep_cap = ((q as f64) * DEPENDS_CAP).round() as usize;
        let mut pick: Vec<usize> = depends.iter().cloned().take(dep_cap).collect();
        let need = q - pick.len();
        pick.extend(others.iter().cloned().take(need));
        // If short on the others, fill with remaining depends.
        let mut extra = depends.iter().cloned().skip(dep_cap);
        while pick.len() < q {
            match extra.next() {
                Some(i) => pick.push(i),
                None => break,
            }
        }
        chosen.extend(pick);
    }
    chosen.sort_unstable();

    // Marginal tallies and output.
    let (mut street_n, mut pot_n, mut players_n) = ([0usize; 3], [0usize; 3], [0usize; 3]);
    let mut lines = String::new();
    for (id, &i) in chosen.iter().enumerate() {
        let c = full_cell[i];
        street_n[c / 9] += 1;
        pot_n[(c / 3) % 3] += 1;
        players_n[c % 3] += 1;
        let sg = &full_grades[i];
        let g = &sg.pooled;
        match g.verdict {
            Verdict::Robust => verdict_counts[0] += 1,
            Verdict::Close => verdict_counts[1] += 1,
            Verdict::Depends => verdict_counts[2] += 1,
        }
        let spot = &full_spots[i].0;
        let nv = spot.replay().active_count().min(6);
        let (turns, rivers) = match spot.board.len() {
            3 => cfg.flop_samples[nv],
            4 => (0, cfg.turn_samples[nv]),
            _ => (0, 0),
        };
        lines.push_str(&spot_line(id, spot, g, turns, rivers, &format!(",agree=best+verdict,pool_match={}", u8::from(sg.pool_matches))));
        lines.push('\n');
    }

    let meta = vec![
        ("seed", seed.to_string()),
        ("hands_simulated", hands.to_string()),
        ("quick_grades", quick_spots.len().to_string()),
        ("full_grades", full_spots.len().to_string()),
        ("spots", chosen.len().to_string()),
        ("max_villains", max_villains.to_string()),
        ("batches_per_half", batches.to_string()),
        ("teachable_full_grades", teachable_n.to_string()),
        ("retained_agreeing", retained_text(&by_street, &by_verdict)),
        ("pool_mismatch_among_kept", pool_mismatch.to_string()),
        ("tables", cfg.tables.to_string()),
        ("flop_samples", format!("{:?}", cfg.flop_samples)),
        ("turn_samples", format!("{:?}", cfg.turn_samples)),
        ("reach_eps", format!("{:e}", cfg.solve.reach_eps)),
        ("full_grade_seconds", format!("{full_secs:.0}")),
        ("peak_rss_gb", format!("{:.2}", poker_engine::memguard::peak_rss_gb())),
        ("verdicts", format!("robust={},close={},depends={}", verdict_counts[0], verdict_counts[1], verdict_counts[2])),
        ("streets", format!("flop={},turn={},river={}", street_n[0], street_n[1], street_n[2])),
        ("pot_types", format!("limped={},single_raised={},reraised={}", pot_n[0], pot_n[1], pot_n[2])),
        ("players_left", format!("two={},three={},four_plus={}", players_n[0], players_n[1], players_n[2])),
    ];
    let mut out = format_header(&meta);
    out.push_str(&lines);
    std::fs::create_dir_all(std::path::Path::new(output).parent().unwrap()).unwrap();
    std::fs::write(output, out).unwrap();
    eprintln!("wrote {} spots to {output}", chosen.len());
    eprintln!("verdicts robust/close/depends: {verdict_counts:?}");
    eprintln!("streets {street_n:?} pots {pot_n:?} players {players_n:?}");
    eprintln!("cell quotas {quota:?}");
    eprintln!("retention among teachable full-graded spots: {}; teachable {teachable_n}; pool disagrees with halves on {pool_mismatch} kept", retained_text(&by_street, &by_verdict));
    eprintln!("peak RSS {:.2} GB", poker_engine::memguard::peak_rss_gb());
}

fn final_cfg() -> GradeCfg {
    GradeCfg::standard()
}

fn retained_text(by_street: &[(usize, usize); 3], by_verdict: &[(usize, usize); 3]) -> String {
    let pct = |x: &(usize, usize)| if x.1 == 0 { 0.0 } else { 100.0 * x.0 as f64 / x.1 as f64 };
    format!(
        "flop {}/{} ({:.1}%), turn {}/{} ({:.1}%), river {}/{} ({:.1}%); robust {}/{} ({:.1}%), close {}/{} ({:.1}%), depends {}/{} ({:.1}%)",
        by_street[0].0, by_street[0].1, pct(&by_street[0]), by_street[1].0, by_street[1].1, pct(&by_street[1]), by_street[2].0, by_street[2].1, pct(&by_street[2]),
        by_verdict[0].0, by_verdict[0].1, pct(&by_verdict[0]), by_verdict[1].0, by_verdict[1].1, pct(&by_verdict[1]), by_verdict[2].0, by_verdict[2].1, pct(&by_verdict[2]),
    )
}
