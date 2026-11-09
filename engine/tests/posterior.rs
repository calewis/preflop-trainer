//! Bet size as a soft signal, and the card-removal correction against an
//! independent exact enumeration.

use poker_engine::combos::{combo_index, combos, NCOMBOS};
use poker_engine::game::Act;
use poker_engine::models::{PlayerModel, MANIAC, NIT, POPULATION, STATION};
use poker_engine::solve::decorrelate;
use poker_engine::spot::{seat_posteriors, GradeCfg, HistAct, Spot};
use poker_trainer::cards::{parse_cards, parse_holding};
use poker_trainer::rng::Rng;

fn river_spot(bet: f64, stack: f64) -> Spot {
    let hero = parse_holding("AdQc").unwrap();
    let board = parse_cards("As9h5c3d2s").unwrap();
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
    history.push(HistAct { street: 3, seat: 1, act: Act::Bet(bet) });
    Spot { n: 5, hero_seat: 4, stack, hero, board, history }
}

fn posterior(spot: &Spot) -> [f64; 5] {
    let mut cfg = GradeCfg::standard();
    cfg.tables = 64;
    cfg.pool = 256;
    let rp = spot.replay();
    let mut out = [0.0; 5];
    for seed in 0..8u64 {
        let pools = seat_posteriors(spot, &rp, &cfg, &mut Rng::new(seed));
        for t in 0..cfg.tables {
            out[pools[0].chosen_model(t).archetype] += 1.0;
        }
    }
    let total: f64 = out.iter().sum();
    out.map(|x| x / total)
}

/// Bets of 1/3 pot and pot move weight in opposite directions, without proof:
/// every archetype remains possible.
#[test]
fn bet_size_shifts_belief_without_proving_it() {
    let small = posterior(&river_spot(6.5 / 3.0, 100.0));
    let large = posterior(&river_spot(6.5, 100.0));
    assert!(small[STATION] > large[STATION], "station: small {small:?} large {large:?}");
    assert!(large[MANIAC] > small[MANIAC], "maniac: small {small:?} large {large:?}");
    for a in [STATION, MANIAC, NIT] {
        assert!(small[a] > 0.0 && large[a] > 0.0, "archetype {a} eliminated: {small:?} {large:?}");
    }
}

/// An all-in that every size class reaches carries no size information.
#[test]
fn capped_bet_carries_no_size_information() {
    let mut rng = Rng::new(9);
    for a in 0..POPULATION.len() {
        let mut m = PlayerModel::sample_of(&mut rng, a);
        let log_w = m.constrain_sizes(&[(2, 0b111)], &mut rng);
        assert!(log_w.abs() < 1e-9, "capped bet changed weight: {log_w}");
    }
    let mut m = PlayerModel::sample_of(&mut rng, STATION);
    assert!(m.constrain_sizes(&[(2, 0b001)], &mut rng) < 0.0);
    assert!((m.bet_frac[1] - 1.0 / 3.0).abs() < 1e-12, "observed class is fixed on that street");
}

/// Exact marginals of each villain under the joint no-overlap distribution.
fn exact_marginals(beliefs: &[Vec<f32>]) -> Vec<Vec<f64>> {
    let t = combos();
    let mut out = vec![vec![0.0; NCOMBOS]; beliefs.len()];
    let live: Vec<Vec<usize>> = beliefs.iter().map(|b| (0..NCOMBOS).filter(|&i| b[i] > 0.0).collect()).collect();
    let mut total = 0.0;
    for &a in &live[0] {
        for &b in &live[1] {
            if t.masks[a] & t.masks[b] != 0 {
                continue;
            }
            for &c in &live[2] {
                if (t.masks[a] | t.masks[b]) & t.masks[c] != 0 {
                    continue;
                }
                let w = f64::from(beliefs[0][a]) * f64::from(beliefs[1][b]) * f64::from(beliefs[2][c]);
                total += w;
                out[0][a] += w;
                out[1][b] += w;
                out[2][c] += w;
            }
        }
    }
    for row in out.iter_mut() {
        for x in row.iter_mut() {
            *x /= total;
        }
    }
    out
}

fn l1(a: &[f32], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| (f64::from(*x) - y).abs()).sum()
}

/// A villain who holds the blocking card with certainty rules the combo out.
#[test]
fn certain_blocker_removes_the_combo() {
    let mut a = vec![0f32; NCOMBOS];
    let c = |h: &str| { let x = parse_holding(h).unwrap(); combo_index(x[0], x[1]) };
    a[c("AsQs")] = 0.5;
    a[c("7c8c")] = 0.5;
    let mut b = vec![0f32; NCOMBOS];
    b[c("AsKs")] = 1.0;
    let mut beliefs = vec![a, b];
    decorrelate(&mut beliefs, 2);
    assert!(beliefs[0][c("AsQs")] < 1e-5, "{}", beliefs[0][c("AsQs")]);
    assert!((beliefs[0][c("7c8c")] - 1.0).abs() < 1e-5);
}

/// Small random ranges: the correction moves marginals towards the exact joint
/// answer, and more iterations do not compound away from it.
#[test]
fn decorrelate_approaches_exact_marginals_and_does_not_compound() {
    let mut rng = Rng::new(0xDEC);
    let (mut better, mut cases, mut plain_sum, mut fixed_sum) = (0, 0, 0.0, 0.0);
    let (mut max_drift, mut drift_sum) = (0f64, 0f64);
    for _ in 0..200 {
        // Three villains over a 12-card deck, so overlaps are frequent.
        let deck: Vec<u8> = (0..12).map(|i| 12 * 4 + i % 4 - (i / 4) * 4).collect();
        let t = combos();
        let mut beliefs = vec![vec![0f32; NCOMBOS]; 3];
        for b in beliefs.iter_mut() {
            for _ in 0..6 {
                let (x, y) = (deck[rng.below(12) as usize], deck[rng.below(12) as usize]);
                if x != y {
                    b[t.index[x as usize][y as usize] as usize] = 1.0 + rng.below(4) as f32;
                }
            }
            let s: f32 = b.iter().sum();
            b.iter_mut().for_each(|w| *w /= s);
        }
        let exact = exact_marginals(&beliefs);
        if exact[0].iter().any(|x| x.is_nan()) {
            continue;
        }
        let (mut fixed, mut many) = (beliefs.clone(), beliefs.clone());
        decorrelate(&mut fixed, 2);
        decorrelate(&mut many, 12);
        for v in 0..3 {
            let plain = l1(&beliefs[v], &exact[v]);
            let two = l1(&fixed[v], &exact[v]);
            plain_sum += plain;
            fixed_sum += two;
            cases += 1;
            if two < plain {
                better += 1;
            }
            let drift: f64 = fixed[v].iter().zip(&many[v]).map(|(a, b)| f64::from((a - b).abs())).sum();
            max_drift = max_drift.max(drift);
            drift_sum += drift;
        }
    }
    assert!(better * 100 >= cases * 70, "improved only {better} of {cases}");
    // Far more iterations than the 2 the engine uses need not converge on such
    // tiny, heavily overlapping ranges (single cases oscillate); on average they stay put.
    assert!(drift_sum / (cases as f64) < 0.25, "mean drift {}", drift_sum / cases as f64);
    assert!(fixed_sum < 0.8 * plain_sum, "mean L1: plain {plain_sum:.3} corrected {fixed_sum:.3}");
    println!("decorrelate drift, 2 vs 12 iterations (L1): mean {:.4} max {max_drift:.4}", drift_sum / cases as f64);
    println!("decorrelate vs exact marginals: improved {better}/{cases}, mean L1 {:.4} -> {:.4}", plain_sum / cases as f64, fixed_sum / cases as f64);
}
