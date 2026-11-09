//! Same seed, same output.

use poker_engine::sim::simulate;
use poker_engine::spot::{grade_spot, GradeCfg};

#[test]
fn grading_is_deterministic() {
    let mut cfg = GradeCfg::standard();
    cfg.tables = 8;
    cfg.pool = 16;
    let mut found = 0;
    for seed in 1..2000u64 {
        if let Some(sim) = simulate(seed) {
            let a = grade_spot(&sim.spot, &cfg, seed * 3);
            let b = grade_spot(&sim.spot, &cfg, seed * 3);
            for (x, y) in a.options.iter().zip(&b.options) {
                assert_eq!(x.mean_ev.to_bits(), y.mean_ev.to_bits());
                assert_eq!(x.best_share.to_bits(), y.best_share.to_bits());
            }
            found += 1;
            if found >= 8 {
                break;
            }
        }
    }
    assert!(found >= 8);
}

#[test]
fn simulation_is_deterministic() {
    for seed in 0..500 {
        let a = simulate(seed).map(|r| format!("{:?} {:?}", r.spot.board, r.spot.history));
        let b = simulate(seed).map(|r| format!("{:?} {:?}", r.spot.board, r.spot.history));
        assert_eq!(a, b);
    }
}

#[test]
fn split_grading_is_consistent_and_deterministic() {
    use poker_engine::spot::{grade_split, verdict_stats};
    let mut cfg = GradeCfg::standard();
    cfg.tables = 8;
    cfg.pool = 16;
    cfg.flop_samples = [(0, 0), (4, 2), (3, 2), (3, 2), (3, 2), (3, 2), (3, 2)];
    let mut found = 0;
    for seed in 1..2000u64 {
        if let Some(sim) = simulate(seed) {
            let a = grade_split(&sim.spot, &cfg, 1, seed * 5);
            let b = grade_split(&sim.spot, &cfg, 1, seed * 5);
            assert_eq!(a.pooled.tables, 16);
            for (x, y) in a.pooled.options.iter().zip(&b.pooled.options) {
                assert_eq!(x.mean_ev.to_bits(), y.mean_ev.to_bits());
            }
            let pot = a.pooled.pot;
            let (h0, h1) = a.pooled.ev_by_table.split_at(8);
            let (s0, s1) = (verdict_stats(h0, pot), verdict_stats(h1, pot));
            assert_eq!((s0.best, s0.verdict), a.halves[0]);
            assert_eq!((s1.best, s1.verdict), a.halves[1]);
            assert_eq!(a.agree, a.halves[0] == a.halves[1]);
            found += 1;
            if found >= 6 {
                break;
            }
        }
    }
    assert!(found >= 6);
}
