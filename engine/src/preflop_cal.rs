//! Recalibrated opponent behaviour. `CAL_V2` is used for the preflop chart
//! derivation (`preflop_charts.rs`, `bin/preflop.rs`), and, through `active()`
//! with `POKER_POPULATION=mixture`, for half of the tables behind the v3 spot
//! library. The base population in `models.rs` and `spots_v2` don't change.
//!
//! EVERY NUMBER HERE IS AN ASSUMPTION, set by plain reasoning about a loose,
//! no-rake home game (see PREFLOP.md). None is fitted to data or tuned to any
//! published chart. Archetype order is station, loose-passive, nit, maniac, solid.

use crate::models::{PlayerModel, PreflopCtx, ARCHETYPES};
use poker_trainer::preflop::Seat;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug)]
pub struct Calibration {
    pub name: &'static str,
    /// Percent of hands that cold-call a single raise from a cutoff-like seat
    /// (scaled by seat as in the population model).
    pub cold_ip: [f64; 5],
    /// Same, from the small blind and big blind (no further multiplier).
    pub cold_sb: [f64; 5],
    pub cold_bb: [f64; 5],
    /// Percent of hands that 3-bet from a cutoff-like seat (scaled by seat).
    pub three_bet: [f64; 5],
    /// Share of a limper's limping range that calls a raise.
    pub limp_call: [f64; 5],
    /// Opener facing a 3-bet: percent of all hands that call and that 4-bet.
    pub call_3bet: [f64; 5],
    pub four_bet: [f64; 5],
    /// Postflop P(continue | medium bet) replaced by bucket, strongest first.
    /// `None` keeps the population's vector.
    pub cont: Option<[[f64; 6]; 5]>,
    /// Postflop P(bet when checked to) and P(raise | continue), same treatment.
    pub bet: Option<[[f64; 6]; 5]>,
    pub raise: Option<[[f64; 6]; 5]>,
    /// Added to each archetype's log-odds shift of continuing per later street.
    pub cont_street_delta: [f64; 5],
}

/// No postflop change: the population's postflop model.
const NO_STREET: [f64; 5] = [0.0; 5];

/// Revision 1: preflop responses only.
pub const CAL_V1: Calibration = Calibration {
    name: "v1-preflop",
    cold_ip: [45.0, 35.0, 7.0, 35.0, 15.0],
    cold_sb: [50.0, 42.0, 10.0, 40.0, 20.0],
    cold_bb: [66.0, 64.0, 22.0, 60.0, 40.0],
    three_bet: [3.0, 2.5, 2.5, 15.0, 6.0],
    limp_call: [0.85, 0.80, 0.70, 0.80, 0.70],
    call_3bet: [20.0, 16.0, 5.0, 20.0, 11.0],
    four_bet: [1.5, 1.5, 1.5, 7.0, 3.0],
    cont: None,
    bet: None,
    raise: None,
    cont_street_delta: NO_STREET,
};

/// Revision 2: as revision 1, plus postflop fold rates for weak holdings.
/// Revision 1 raised hero's win rate, so the postflop model drives the excess.
/// Reasoning: soft players still fold air and most weak holdings to a bet of
/// two-thirds pot, do not pay two streets with bottom pair, and bet and raise
/// their strong hands more often than the population assumes.
pub const CAL_V2: Calibration = Calibration {
    name: "v2-preflop-postflop",
    cont: Some([
        [0.995, 0.985, 0.90, 0.60, 0.33, 0.15],
        [0.98, 0.95, 0.80, 0.45, 0.20, 0.08],
        [0.97, 0.90, 0.55, 0.18, 0.05, 0.02],
        [0.98, 0.95, 0.85, 0.60, 0.40, 0.30],
        [0.98, 0.95, 0.75, 0.38, 0.15, 0.06],
    ]),
    bet: Some([
        [0.70, 0.60, 0.40, 0.15, 0.08, 0.05],
        [0.75, 0.65, 0.45, 0.20, 0.12, 0.08],
        [0.90, 0.80, 0.50, 0.15, 0.05, 0.03],
        [0.90, 0.85, 0.75, 0.60, 0.50, 0.45],
        [0.90, 0.80, 0.60, 0.25, 0.18, 0.18],
    ]),
    raise: Some([
        [0.35, 0.15, 0.05, 0.01, 0.01, 0.01],
        [0.35, 0.18, 0.06, 0.02, 0.01, 0.01],
        [0.65, 0.35, 0.08, 0.01, 0.01, 0.01],
        [0.70, 0.55, 0.35, 0.25, 0.20, 0.20],
        [0.60, 0.35, 0.12, 0.05, 0.04, 0.04],
    ]),
    cont_street_delta: [-0.25, -0.15, 0.0, 0.0, -0.1],
    ..CAL_V1
};

/// The opponent pool this process uses, from `POKER_POPULATION`: unset means
/// the base population (`spots_v2`), `mixture` means each table is drawn 50/50
/// from the base population and `CAL_V2` (`spots_v3`).
pub fn active() -> Option<&'static Calibration> {
    static ACTIVE: OnceLock<Option<&'static Calibration>> = OnceLock::new();
    *ACTIVE.get_or_init(|| match std::env::var("POKER_POPULATION").as_deref() {
        Ok("mixture") => Some(&CAL_V2),
        Ok(other) => panic!("unknown POKER_POPULATION {other}"),
        Err(_) => None,
    })
}

pub fn by_name(name: &str) -> Option<&'static Calibration> {
    match name {
        "v1" => Some(&CAL_V1),
        "v2" => Some(&CAL_V2),
        _ => None,
    }
}

impl Calibration {
    /// Applies the postflop override to a freshly sampled model, keeping the
    /// model's per-seat noise as an offset in log-odds. Must run before `tables()`.
    pub fn apply_postflop(&self, m: &mut PlayerModel) {
        let a = m.archetype;
        if let Some(rows) = &self.cont {
            let base = &ARCHETYPES[a].cont;
            for b in 0..6 {
                let shift = logit(m.cont[b]) - logit(base[b]);
                m.cont[b] = logistic(logit(rows[a][b]) + shift);
            }
        }
        let shift_vec = |to: &mut [f64; 6], base: &[f64; 6], rows: &Option<[[f64; 6]; 5]>| {
            if let Some(r) = rows {
                for b in 0..6 {
                    let shift = logit(to[b]) - logit(base[b]);
                    to[b] = logistic(logit(r[a][b]) + shift);
                }
            }
        };
        shift_vec(&mut m.bet, &ARCHETYPES[a].bet, &self.bet);
        shift_vec(&mut m.raise, &ARCHETYPES[a].raise, &self.raise);
        m.cont_street += self.cont_street_delta[a];
    }

    /// Raise and continue thresholds (percent of hands) for a node, or `None`
    /// where the population's own thresholds apply. `limped` means the seat has
    /// already put in chips voluntarily without raising.
    pub fn thresholds(&self, m: &PlayerModel, ctx: &PreflopCtx, limped: bool) -> Option<(f64, f64)> {
        let a = m.archetype;
        let def = &ARCHETYPES[a];
        let pos = crate::models::position_factor(ctx.position);
        match ctx.raises {
            1 => {
                let three = self.three_bet[a] * ratio(m.three_bet, def.three_bet);
                let raise = three * pos;
                if limped {
                    // A limp range sits between the raise and call cutoffs, so the share
                    // applies to that band; the hands above it always continue.
                    let top = (m.pfr * pos).min(100.0);
                    let cont = top + self.limp_call[a] * ((m.vpip * pos).min(100.0) - top).max(0.0);
                    return Some((raise.min(cont), cont.max(raise).min(100.0)));
                }
                let cold = match ctx.position {
                    Seat::SmallBlind => self.cold_sb[a],
                    Seat::BigBlind => self.cold_bb[a],
                    _ => self.cold_ip[a] * pos,
                } * ratio(m.cold_call, def.cold_call);
                let size_ref = 3.0 / ctx.last_raise_to.max(2.0);
                let size_factor = size_ref.powf(0.5 * m.price_sens.min(3.0));
                Some((raise, (raise + cold * size_factor).min(100.0)))
            }
            2 => {
                let call = self.call_3bet[a] * ratio(m.call_3bet, def.call_3bet);
                let four = self.four_bet[a] * ratio(m.four_bet, def.four_bet);
                let (f, c) = if ctx.raised_earlier { (four, call) } else { (four * 0.6, call * 0.6) };
                Some((f, (f + c).min(100.0)))
            }
            _ => None,
        }
    }
}

fn ratio(sampled: f64, base: f64) -> f64 {
    sampled / base.max(1e-9)
}

fn logit(p: f64) -> f64 {
    let p = p.clamp(1e-4, 1.0 - 1e-4);
    (p / (1.0 - p)).ln()
}

fn logistic(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}
