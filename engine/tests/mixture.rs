//! The opponent-pool mixture used for the v3 spot library.

use poker_engine::game::Act;
use poker_engine::models::{PlayerModel, PreflopCtx};
use poker_engine::preflop_cal::CAL_V2;
use poker_engine::sim::{simulate, simulate_with};
use poker_engine::spot::{grade_spot, population_posterior, seat_posteriors, GradeCfg};
use poker_trainer::preflop::Seat;
use poker_trainer::rng::Rng;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// Regression fingerprints of the current base path, taken from this code, so an
/// unintended change to it fails here. They are not evidence by themselves that
/// the base was unchanged by the mixture work: that was a HEAD~1 vs HEAD review
/// comparison, byte-identical across 20,000 sampled models, 61,147 simulated
/// spots, and 120 full plus 40 split grades.
const BASE_MODEL_HASH: u64 = 11965280919241095397;
const BASE_SIM_HASH: u64 = 4168343072839903305;

fn cfg() -> GradeCfg {
    let mut c = GradeCfg::standard();
    c.tables = 16;
    c.pool = 16;
    c.mixture = Some(&CAL_V2);
    c
}

fn spots(n: usize) -> Vec<poker_engine::spot::Spot> {
    (1..50_000u64).filter_map(|s| simulate_with(s, Some(&CAL_V2))).map(|r| r.spot).filter(|s| { let v = s.replay().active_count(); (1..=2).contains(&v) && s.board.len() >= 4 }).take(n).collect()
}

/// A cheap hash of everything that defines a sampled base model.
fn model_hash(m: &PlayerModel, h: u64) -> u64 {
    let mut h = mix(h, m.archetype as u64);
    for x in [m.vpip, m.pfr, m.cold_call, m.three_bet, m.call_3bet, m.four_bet, m.call_4bet, m.five_bet, m.draw_bet, m.draw_cont, m.draw_raise, m.bet_street, m.cont_street, m.price_sens, m.multiway, m.raised_pen] {
        h = mix(h, x.to_bits());
    }
    for v in [&m.bet, &m.cont, &m.raise] {
        for x in v {
            h = mix(h, x.to_bits());
        }
    }
    mix(h, u64::from(m.open_class) + 16 * u64::from(m.size_class[0]) + 256 * u64::from(m.size_class[1]) + 4096 * u64::from(m.size_class[2]))
}

fn mix(a: u64, b: u64) -> u64 {
    let mut r = Rng::new(a ^ b.rotate_left(17));
    r.next_u64()
}

/// A base-population sample carries no recalibration, its numbers are exactly
/// the regression fingerprint of the current base path, and sampling the recalibrated
/// population draws the same random numbers before applying its changes.
#[test]
fn base_population_is_untouched() {
    let mut a = Rng::new(9);
    let mut hash = 7u64;
    for _ in 0..200 {
        let m = PlayerModel::sample(&mut a);
        assert!(m.cal.is_none());
        hash = model_hash(&m, hash);
    }
    assert_eq!(hash, BASE_MODEL_HASH, "base population changed");

    let (mut a, mut b) = (Rng::new(11), Rng::new(11));
    for _ in 0..200 {
        let m = PlayerModel::sample(&mut a);
        let c = PlayerModel::sample_cal(&mut b, &CAL_V2);
        assert!(c.cal.is_some());
        // Same noise and fields that the recalibration doesn't change.
        assert_eq!(m.vpip.to_bits(), c.vpip.to_bits());
        assert_eq!(m.pfr.to_bits(), c.pfr.to_bits());
        assert_eq!(m.archetype, c.archetype);
        let ctx = PreflopCtx { position: Seat::BigBlind, raises: 1, limpers: 0, last_raise_to: 3.0, raised_earlier: false, free_check: false, limped: false };
        let continues = |m: &PlayerModel| m.preflop_row(&ctx).iter().map(|p| f64::from(1.0 - p[0])).sum::<f64>();
        // The recalibrated big blind defends more than the base model with the same noise.
        assert!(continues(&c) > continues(&m));
    }
}

/// In one sampled table, every seat comes from the same population.
#[test]
fn a_table_has_one_population() {
    let cfg = cfg();
    let spots = spots(10);
    assert!(spots.len() >= 10);
    let (mut seen0, mut seen1) = (0, 0);
    for (i, s) in spots.iter().enumerate() {
        let rp = s.replay();
        let mut rng = Rng::new(40 + i as u64);
        let pools = seat_posteriors(s, &rp, &cfg, &mut rng);
        for t in 0..cfg.tables {
            let pop = pools[0].chosen_model(t).cal.is_some();
            if pop {
                seen1 += 1;
            } else {
                seen0 += 1;
            }
            for p in &pools {
                assert_eq!(p.chosen_model(t).cal.is_some(), pop, "seats from different populations");
            }
        }
    }
    assert!(seen0 > 0 && seen1 > 0, "both populations occur: {seen0} {seen1}");
}

#[test]
fn mixture_grading_is_deterministic() {
    let cfg = cfg();
    for (i, s) in spots(4).iter().enumerate() {
        let a = grade_spot(s, &cfg, 70 + i as u64);
        let b = grade_spot(s, &cfg, 70 + i as u64);
        for (x, y) in a.options.iter().zip(&b.options) {
            assert_eq!(x.mean_ev.to_bits(), y.mean_ev.to_bits());
            assert_eq!(x.best_share.to_bits(), y.best_share.to_bits());
        }
    }
}

/// `simulate` uses the process's configured population, whatever the
/// environment says, and an explicit `None` is always the base population.
#[test]
fn simulate_uses_the_active_population() {
    let active = poker_engine::preflop_cal::active();
    for s in 1..2_000u64 {
        let a = simulate(s).map(|r| (r.spot.hero, r.spot.board));
        let b = simulate_with(s, active).map(|r| (r.spot.hero, r.spot.board));
        assert_eq!(a, b);
    }
}

/// An explicit `None` is the base population in every detail: the fingerprint
/// of simulated hands matches the regression fingerprint of the current base path, and tables
/// differ from the mixture on some seeds.
#[test]
fn explicit_none_is_the_base_population() {
    let mut h = 13u64;
    let mut differs = 0;
    for s in 1..3_000u64 {
        let b = simulate_with(s, None);
        let m = simulate_with(s, Some(&CAL_V2));
        if let Some(r) = &b {
            h = mix(h, s);
            for c in r.spot.hero.iter().chain(r.spot.board.iter()) {
                h = mix(h, c.index() as u64);
            }
            for a in &r.spot.history {
                h = mix(h, u64::from(a.seat) + 64 * u64::from(a.street));
                h = mix(h, match a.act {
                    Act::Fold => 1,
                    Act::Check => 2,
                    Act::Call => 3,
                    Act::Bet(x) => 4 ^ x.to_bits(),
                    Act::Raise(x) => 5 ^ x.to_bits(),
                });
            }
            assert!(!r.recalibrated);
        }
        let key = |r: &Option<poker_engine::sim::SimResult>| r.as_ref().map(|x| (x.spot.hero, x.spot.board.clone()));
        if key(&b) != key(&m) {
            differs += 1;
        }
    }
    assert_eq!(h, BASE_SIM_HASH, "base simulation changed");
    assert!(differs > 0, "mixture should sometimes differ");
}

/// With simulated tables of known population, the posterior probability that a
/// table is recalibrated should match how often it really is.
#[test]
fn population_posterior_is_calibrated() {
    let mut cfg = GradeCfg::standard();
    cfg.tables = 4;
    cfg.mixture = Some(&CAL_V2);
    let n = 4_000usize;
    let threads = std::thread::available_parallelism().map_or(4, |p| p.get()).min(16);
    let next = AtomicUsize::new(0);
    let out = Mutex::new(Vec::with_capacity(n));
    std::thread::scope(|sc| {
        for _ in 0..threads {
            sc.spawn(|| {
                let (cfg, next, out) = (&cfg, &next, &out);
                loop {
                    let seed = next.fetch_add(1, Ordering::Relaxed) as u64 + 1;
                    if out.lock().unwrap().len() >= n {
                        break;
                    }
                    let Some(res) = simulate_with(seed, Some(&CAL_V2)) else { continue };
                    let rp = res.spot.replay();
                    if rp.active_count() > 2 {
                        continue;
                    }
                    let mut rng = Rng::new(seed ^ 0xAB);
                    let p = population_posterior(&res.spot, &rp, cfg, &CAL_V2, &mut rng);
                    out.lock().unwrap().push((seed, p, res.recalibrated));
                }
            });
        }
    });
    let mut rows = out.into_inner().unwrap();
    rows.sort_by_key(|x| x.0);
    rows.truncate(n);
    let nf = rows.len() as f64;
    let (mut sum_p, mut sum_y, mut brier) = (0.0f64, 0.0f64, 0.0f64);
    let mut bins = [(0.0f64, 0.0f64, 0.0f64); 10];
    for &(_, p, y) in &rows {
        let y = f64::from(u8::from(y));
        sum_p += p;
        sum_y += y;
        brier += (p - y) * (p - y);
        let b = ((p * 10.0) as usize).min(9);
        bins[b].0 += 1.0;
        bins[b].1 += p;
        bins[b].2 += y;
    }
    let (mean_p, mean_y, brier) = (sum_p / nf, sum_y / nf, brier / nf);
    println!("n={nf} mean posterior {mean_p:.4} true share {mean_y:.4} Brier {brier:.4}");
    for (i, b) in bins.iter().enumerate() {
        if b.0 > 0.0 {
            println!("bin {i}: n={} predicted {:.3} observed {:.3}", b.0, b.1 / b.0, b.2 / b.0);
        }
    }
    assert!((mean_p - mean_y).abs() < 0.02, "mean posterior {mean_p:.4} vs true share {mean_y:.4}");
    assert!(brier < 0.245, "Brier {brier:.4}");
    // Each decile with enough spots agrees within sampling error (3 standard
    // errors of the observed frequency) plus a small allowance.
    for b in bins.iter().filter(|b| b.0 >= 100.0) {
        let (pred, obs, n) = (b.1 / b.0, b.2 / b.0, b.0);
        let limit = 3.0 * (pred * (1.0 - pred) / n).sqrt() + 0.02;
        assert!((pred - obs).abs() <= limit, "decile miscalibrated: predicted {pred:.3} observed {obs:.3} (n={n}, limit {limit:.3})");
    }
}
