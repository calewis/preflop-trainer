//! The optimized solver must match the naive reference implementation.

use poker_engine::reference::solve_reference;
use poker_engine::sim::simulate;
use poker_engine::solve::{solve_root, SolveCfg};
use poker_engine::spot::{build_runouts, seat_posteriors, table_beliefs, villains_for, GradeCfg, Spot};
use poker_trainer::rng::Rng;

fn fixed_spots() -> Vec<Spot> {
    // Up to 6 spots per (street, villains 1..=3), first found from fixed seeds.
    let mut counts = [[0usize; 4]; 3];
    let mut out = Vec::new();
    for seed in 1..400_000u64 {
        if let Some(res) = simulate(seed) {
            let street = res.spot.board.len() - 3;
            let nv = res.spot.replay().active_count();
            if (1..=3).contains(&nv) && counts[street][nv] < 6 {
                counts[street][nv] += 1;
                out.push(res.spot);
            }
        }
    }
    out
}

#[test]
fn solver_matches_reference() {
    let spots = fixed_spots();
    assert!(spots.len() >= 40, "only {} fixed spots", spots.len());
    let (mut max_abs, mut compared) = (0f64, 0usize);
    for (i, spot) in spots.iter().enumerate() {
        let rp = spot.replay();
        let nv = rp.active_count();
        let mut cfg = GradeCfg::standard();
        cfg.tables = 1;
        cfg.pool = 8;
        // Runouts small enough for the slow reference to finish.
        cfg.flop_samples = [(0, 0), (3, 2), (2, 2), (2, 1), (2, 1), (2, 1), (2, 1)];
        cfg.turn_samples = [0, 3, 3, 2, 2, 2, 2];
        cfg.solve = SolveCfg { reach_eps: 1e-6, line_decorrelate: true };
        let mut rng = Rng::new(0xE0 + i as u64);
        let pools = seat_posteriors(spot, &rp, &cfg, &mut rng);
        let runouts = build_runouts(spot, &cfg, nv.min(6), &mut rng);
        let options = rp.round.root_options(spot.hero_seat);
        let acts: Vec<_> = options.iter().map(|x| x.0).collect();
        let (base, corrected) = table_beliefs(&pools, 0);
        let villains = villains_for(&pools, 0, &base, &corrected);
        let fast = solve_root(&rp.round, spot.hero_seat, &villains, &runouts, cfg.solve, &acts);
        let slow = solve_reference(&rp.round, spot.hero_seat, &villains, &runouts, cfg.solve, &acts);
        for (j, (a, b)) in fast.iter().zip(&slow).enumerate() {
            let d = (a - b).abs();
            assert!(d < 2e-3 * rp.round.pot.max(1.0), "spot {i} option {j}: fast {a} slow {b} (pot {})", rp.round.pot);
            max_abs = max_abs.max(d / rp.round.pot.max(1.0));
            compared += 1;
        }
    }
    println!("compared {compared} option values; max |fast-slow|/pot = {max_abs:.2e}");
}

#[test]
fn joint_fast_matches_joint_reference() {
    use poker_engine::joint::solve_joint_fast;
    use poker_engine::reference::solve_joint;
    let spots = fixed_spots();
    let (mut max_abs, mut compared) = (0f64, 0usize);
    let mut worst = (0, 0);
    for (i, spot) in spots.iter().enumerate() {
        let rp = spot.replay();
        let nv = rp.active_count();
        let mut cfg = GradeCfg::standard();
        cfg.tables = 4;
        cfg.pool = 16;
        cfg.flop_samples = [(0, 0), (3, 2), (2, 2), (2, 1), (2, 1), (2, 1), (2, 1)];
        cfg.turn_samples = [0, 3, 3, 2, 2, 2, 2];
        let mut rng = Rng::new(0xF0 + i as u64);
        let pools = seat_posteriors(spot, &rp, &cfg, &mut rng);
        let runouts = build_runouts(spot, &cfg, nv.min(6), &mut rng);
        let options = rp.round.root_options(spot.hero_seat);
        let acts: Vec<_> = options.iter().map(|x| x.0).collect();
        let beliefs: Vec<_> = (0..cfg.tables).map(|t| table_beliefs(&pools, t)).collect();
        let tables: Vec<Vec<_>> = (0..cfg.tables).map(|t| villains_for(&pools, t, &beliefs[t].0, &beliefs[t].1)).collect();
        let fast = solve_joint_fast(&rp.round, spot.hero_seat, &tables, &runouts, cfg.solve, &acts);
        let slow = solve_joint(&rp.round, spot.hero_seat, &tables, &runouts, cfg.solve, &acts);
        for (rf, rs) in fast.iter().zip(&slow) {
            for (a, b) in rf.iter().zip(rs) {
                let d = (a - b).abs();
                assert!(d < 2e-3 * rp.round.pot.max(1.0), "spot {i}: joint fast {a} joint slow {b} (pot {})", rp.round.pot);
                if d / rp.round.pot.max(1.0) > max_abs {
                    worst = (spot.board.len(), rp.active_count());
                }
                max_abs = max_abs.max(d / rp.round.pot.max(1.0));
                compared += 1;
            }
        }
    }
    println!("compared {compared} joint values; max |fast-slow|/pot = {max_abs:.2e}, worst at (board cards, villains) = {worst:?}");
}

/// The tape cache only saves compile time, so clearing it on every lookup must
/// not change any value (exercises the cap-clearing path).
#[test]
fn tape_cap_clearing_does_not_change_values() {
    use poker_engine::joint::{set_tape_caps, solve_joint_fast};
    let spots = fixed_spots();
    let mut compared = 0;
    for (i, spot) in spots.iter().enumerate().filter(|(_, s)| s.board.len() <= 4).take(12) {
        let rp = spot.replay();
        let nv = rp.active_count();
        let mut cfg = GradeCfg::standard();
        cfg.tables = 4;
        cfg.pool = 16;
        cfg.flop_samples = [(0, 0), (3, 2), (2, 2), (2, 1), (2, 1), (2, 1), (2, 1)];
        cfg.turn_samples = [0, 3, 3, 2, 2, 2, 2];
        let mut rng = Rng::new(0xCA + i as u64);
        let pools = seat_posteriors(spot, &rp, &cfg, &mut rng);
        let runouts = build_runouts(spot, &cfg, nv.min(6), &mut rng);
        let options = rp.round.root_options(spot.hero_seat);
        let acts: Vec<_> = options.iter().map(|x| x.0).collect();
        let beliefs: Vec<_> = (0..cfg.tables).map(|t| table_beliefs(&pools, t)).collect();
        let tables: Vec<Vec<_>> = (0..cfg.tables).map(|t| villains_for(&pools, t, &beliefs[t].0, &beliefs[t].1)).collect();
        let normal = solve_joint_fast(&rp.round, spot.hero_seat, &tables, &runouts, cfg.solve, &acts);
        set_tape_caps(0, 0);
        let capped = solve_joint_fast(&rp.round, spot.hero_seat, &tables, &runouts, cfg.solve, &acts);
        set_tape_caps(500_000, 2_000_000);
        for (a, b) in normal.iter().flatten().zip(capped.iter().flatten()) {
            assert_eq!(a.to_bits(), b.to_bits());
            compared += 1;
        }
    }
    assert!(compared > 0);
    println!("tape-cap clearing: compared {compared} values, identical");
}
