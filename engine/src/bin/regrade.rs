//! Regrades the spots of an existing library with the current engine and counts
//! how many best options and verdicts changed.
//! Usage: regrade FILE LIMIT THREADS [BATCHES_PER_HALF]

use poker_engine::output::parse_spot;
use poker_engine::spot::{grade_split, GradeCfg};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

fn kind(label: &str) -> &str {
    label.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.')
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let text = std::fs::read_to_string(&args[1]).expect("read");
    let limit: usize = args[2].parse().expect("limit");
    let threads: usize = args[3].parse().expect("threads");
    let batches: usize = args.get(4).map_or(1, |s| s.parse().unwrap());
    poker_engine::memguard::start(45.0);
    let lines: Vec<&str> = text.lines().filter(|l| l.starts_with("SPOT|")).collect();
    let step = (lines.len() / limit.max(1)).max(1);
    let sample: Vec<&str> = lines.iter().step_by(step).copied().take(limit).collect();
    let cfg = GradeCfg::standard();
    println!(
        "regrade args: file={} limit={limit} threads={threads} batches_per_half={batches}; tables per batch={}, tables total={}; population={}",
        args[1],
        cfg.tables,
        cfg.tables * batches * 2,
        cfg.mixture.map_or("base".to_string(), |c| format!("mixture with {}", c.name)),
    );
    let next = AtomicUsize::new(0);
    let out = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                if i >= sample.len() {
                    break;
                }
                let f: Vec<&str> = sample[i].split('|').collect();
                let spot = parse_spot(sample[i]);
                let sg = grade_split(&spot, &cfg, batches, 0x5EED_0000 + i as u64);
                let g = sg.pooled;
                let old_best = f[13].strip_prefix("best=").expect("best=");
                let new_best = &g.options[g.best].label;
                out.lock().unwrap().push((spot.board.len(), kind(old_best) != kind(new_best), f[12] != g.verdict.code(), spot.replay().active_count(), f[12].to_string(), g.verdict.code()));
            });
        }
    });
    let r = out.into_inner().unwrap();
    let count = |board: Option<usize>| {
        let sel: Vec<_> = r.iter().filter(|x| board.is_none_or(|b| x.0 == b)).collect();
        let n = sel.len().max(1) as f64;
        (sel.len(), 100.0 * sel.iter().filter(|x| x.1).count() as f64 / n, 100.0 * sel.iter().filter(|x| x.2).count() as f64 / n)
    };
    for (label, board) in [("all", None), ("flop", Some(3)), ("turn", Some(4)), ("river", Some(5))] {
        let (n, b, v) = count(board);
        println!("regrade {label}: spots={n} best-option changed {b:.1}%, verdict changed {v:.1}%");
    }
    for (label, keep) in [("two or three players", true), ("four or more players", false)] {
        let sel: Vec<_> = r.iter().filter(|x| (x.3 <= 2) == keep).collect();
        let n = sel.len().max(1) as f64;
        println!("regrade {label}: spots={} best-option changed {:.1}%, verdict changed {:.1}%", sel.len(), 100.0 * sel.iter().filter(|x| x.1).count() as f64 / n, 100.0 * sel.iter().filter(|x| x.2).count() as f64 / n);
    }
    let names = ["robust", "close", "depends"];
    for from in names {
        let sel: Vec<_> = r.iter().filter(|x| x.4 == from).collect();
        let parts: Vec<String> = names.iter().map(|to| format!("{to} {}", sel.iter().filter(|x| x.5 == *to).count())).collect();
        println!("regrade verdict {from} ({}) -> {}", sel.len(), parts.join(", "));
    }
    println!("peak RSS {:.2} GB", poker_engine::memguard::peak_rss_gb());
}
