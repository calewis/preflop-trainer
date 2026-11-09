//! Direction tests. A failure means a bug or a model that does not express
//! its stated behaviour. Sample sizes keep this to a few seconds.

use poker_engine::validation::direction::{air_bluff, big_vs_small, bluff_vs_field, thin_value_gains};

#[test]
fn thin_value_is_better_against_stations() {
    let (station, nit) = thin_value_gains(120, 11);
    assert!(station > nit, "thin value bet gain: station {station:.2} nit {nit:.2}");
}

#[test]
fn air_bluffs_better_against_nits() {
    let (nit, station) = air_bluff(120, 13);
    assert!(nit.0 > station.0, "air bluff gain: nit {:.2} station {:.2}", nit.0, station.0);
    assert!(nit.1 >= station.1, "air bluff win rate: nit {:.3} station {:.3}", nit.1, station.1);
}

#[test]
fn bluffing_gets_worse_with_more_opponents() {
    let fv = bluff_vs_field(80, 17);
    assert!(fv[0] > fv[1] && fv[1] > fv[2], "bluff gain by field: {fv:?}");
}

#[test]
fn big_bets_gain_relative_to_small_against_stations() {
    let (station, nit) = big_vs_small(120, 19);
    assert!(station > nit, "big minus small bet: station {station:.2} nit {nit:.2}");
}
