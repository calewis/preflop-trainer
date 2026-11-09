//! Memory ceiling for long runs on a shared machine: a run that grows past the
//! limit stops itself and says so rather than risking the kernel OOM killer.
//! It reads `/proc/self/status`, so it works on Linux only; `start` warns on
//! other platforms, where no ceiling is enforced.

use std::time::Duration;

fn status_kb(key: &str) -> Option<u64> {
    let text = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = text.lines().find(|l| l.starts_with(key))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

/// Current resident set size in GB (0 if unavailable).
pub fn rss_gb() -> f64 {
    status_kb("VmRSS:").unwrap_or(0) as f64 / 1_048_576.0
}

/// Peak resident set size so far in GB (0 if unavailable).
pub fn peak_rss_gb() -> f64 {
    status_kb("VmHWM:").unwrap_or(0) as f64 / 1_048_576.0
}

/// Starts a watchdog that exits the process with status 3 if RSS passes `limit_gb`.
pub fn start(limit_gb: f64) {
    if status_kb("VmRSS:").is_none() {
        eprintln!("WARNING: memguard needs /proc/self/status (Linux); no memory ceiling is active.");
        return;
    }
    std::thread::spawn(move || loop {
        let rss = rss_gb();
        if rss > limit_gb {
            eprintln!("MEMORY CEILING: RSS {rss:.1} GB passed the {limit_gb:.0} GB limit; stopping this run.");
            std::process::exit(3);
        }
        std::thread::sleep(Duration::from_millis(500));
    });
}
