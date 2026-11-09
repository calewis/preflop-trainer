//! Direction tests: river spots where everyone has checked to hero on the
//! button with a chosen class of hand, faced by one archetype only.

use crate::board::classify;
use crate::combos::combo_index;
use crate::game::Act;
use crate::models::{NBUCKET, NIT, STATION};
use crate::solve::SolveCfg;
use crate::spot::{grade_spot, GradeCfg, HistAct, Spot};
use poker_trainer::cards::{Card, Holding};
use poker_trainer::rng::Rng;

/// A river spot, limped pot, everyone in, checked down to hero on the button.
pub fn checked_down_river(villains: usize, hero: Holding, board: &[Card]) -> Spot {
    // Five seats: hero on the button, big blind plus up to two limpers against him.
    assert!((1..=3).contains(&villains), "supports 1 to 3 villains");
    let n = 5;
    let hero_seat = 4;
    let mut history = Vec::new();
    history.push(HistAct { street: 0, seat: 2, act: if villains >= 2 { Act::Call } else { Act::Fold } });
    history.push(HistAct { street: 0, seat: 3, act: if villains >= 3 { Act::Call } else { Act::Fold } });
    history.push(HistAct { street: 0, seat: hero_seat as u8, act: Act::Call });
    history.push(HistAct { street: 0, seat: 0, act: Act::Fold });
    history.push(HistAct { street: 0, seat: 1, act: Act::Check });
    let active: Vec<usize> = (1..=villains).collect();
    for street in 1..=2u8 {
        for &s in &active {
            history.push(HistAct { street, seat: s as u8, act: Act::Check });
        }
        history.push(HistAct { street, seat: hero_seat as u8, act: Act::Check });
    }
    for &s in &active {
        history.push(HistAct { street: 3, seat: s as u8, act: Act::Check });
    }
    Spot { n, hero_seat, stack: 100.0, hero, board: board.to_vec(), history }
}

/// Deals random river spots with hero's bucket in `[lo, hi]`, at most `tries`.
pub fn sample_spots(villains: usize, lo: usize, hi: usize, want: usize, seed: u64) -> Vec<Spot> {
    let mut rng = Rng::new(seed);
    let mut out = Vec::new();
    let mut tries = 0;
    while out.len() < want && tries < want * 500 {
        tries += 1;
        let mut deck: Vec<Card> = (0..52u8).map(Card::from_index).collect();
        rng.shuffle(&mut deck);
        let hero = [deck[0], deck[1]];
        let board = deck[2..7].to_vec();
        let classes = classify(&board, false);
        let bucket = classes[combo_index(hero[0], hero[1])] as usize % NBUCKET;
        if bucket >= lo && bucket <= hi {
            out.push(checked_down_river(villains, hero, &board));
        }
    }
    out
}

fn cfg(archetype: usize) -> GradeCfg {
    GradeCfg {
        tables: 1,
        pool: 1,
        flop_samples: [(1, 1); 7],
        turn_samples: [1; 7],
        solve: SolveCfg { reach_eps: 0.0, line_decorrelate: true },
        sample_runouts: true,
        force_archetype: Some(archetype),
        exact_models: true,
        joint: true,
    }
}

fn option_ev(g: &crate::spot::Grade, label: &str) -> f64 {
    g.options.iter().find(|o| o.label == label).map_or(f64::NAN, |o| o.mean_ev)
}

/// For `spots`, each with one EV delta `f(grade)`, returns the average delta
/// under archetype `a` of "bet67 minus check" as percent of pot.
pub fn avg_bet_gain(spots: &[Spot], a: usize) -> f64 {
    let c = cfg(a);
    let mut total = 0.0;
    for (i, s) in spots.iter().enumerate() {
        let g = grade_spot(s, &c, 7 + i as u64);
        total += (option_ev(&g, "bet67") - option_ev(&g, "check")) / g.pot * 100.0;
    }
    total / spots.len() as f64
}

/// Value-bet "more against stations than nits": thin hands, bet67 gain.
pub fn thin_value_gains(want: usize, seed: u64) -> (f64, f64) {
    let spots = sample_spots(1, 2, 3, want, seed);
    (avg_bet_gain(&spots, STATION), avg_bet_gain(&spots, NIT))
}

/// Air bluff: bet67 minus check as percent of pot, and how often it is
/// profitable, against stations and nits.
pub fn air_bluff(want: usize, seed: u64) -> ((f64, f64), (f64, f64)) {
    let spots = sample_spots(1, 5, 5, want, seed);
    let one = |a: usize| -> (f64, f64) {
        let c = cfg(a);
        let (mut total, mut pos) = (0.0, 0.0);
        for (i, s) in spots.iter().enumerate() {
            let g = grade_spot(s, &c, 7 + i as u64);
            let d = (option_ev(&g, "bet67") - option_ev(&g, "check")) / g.pot * 100.0;
            total += d;
            if d > 0.0 {
                pos += 1.0;
            }
        }
        (total / spots.len() as f64, pos / spots.len() as f64)
    };
    (one(NIT), one(STATION))
}

/// Air bluff gain with 1, 2, 3 villains (mean over models, population mix).
pub fn bluff_vs_field(want: usize, seed: u64) -> Vec<f64> {
    (1..=3)
        .map(|v| {
            let spots = sample_spots(v, 5, 5, want, seed + v as u64);
            let mut total = 0.0;
            for (i, s) in spots.iter().enumerate() {
                let mut c = cfg(0);
                c.force_archetype = None;
                c.exact_models = false;
                c.tables = 8;
                c.pool = 8;
                let g = grade_spot(s, &c, 7 + i as u64);
                total += (option_ev(&g, "bet67") - option_ev(&g, "check")) / g.pot * 100.0;
            }
            total / spots.len() as f64
        })
        .collect()
}

/// Big value bets gain relative to small: pot-size minus one-third-size bet
/// EV for strong hands, percent of pot, against price-insensitive stations and nits.
pub fn big_vs_small(want: usize, seed: u64) -> (f64, f64) {
    let spots = sample_spots(1, 0, 1, want, seed);
    let one = |a: usize| -> f64 {
        let c = cfg(a);
        let mut total = 0.0;
        for (i, s) in spots.iter().enumerate() {
            let g = grade_spot(s, &c, 7 + i as u64);
            total += (option_ev(&g, "bet100") - option_ev(&g, "bet33")) / g.pot * 100.0;
        }
        total / spots.len() as f64
    };
    (one(STATION), one(NIT))
}
