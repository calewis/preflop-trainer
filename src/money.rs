//! The one money formula used everywhere.
//!
//! `EV(call) = share * (P + C + X) - C` and `EV(fold) = 0`, where `P` is the pot
//! before hero acts (including the bet and any earlier callers), `C` the amount
//! to call and `X` any extra hero collects later.

pub const EV_FOLD: f64 = 0.0;

pub fn ev_call(share: f64, pot: f64, call: f64, extra: f64) -> f64 {
    share * (pot + call + extra) - call
}

/// Equity at which calling exactly breaks even, given `extra` collected later.
pub fn break_even_with_x(pot: f64, call: f64, extra: f64) -> f64 {
    call / (pot + call + extra)
}

/// Break-even equity with no later winnings: `C / (P + C)`.
pub fn break_even(pot: f64, call: f64) -> f64 {
    break_even_with_x(pot, call, 0.0)
}

/// The largest call that still breaks even, solving `share * (P + C + X) = C`
/// for `C`. `None` if `share` is 1 (a call can never lose) or 0.
pub fn break_even_call(share: f64, pot: f64, extra: f64) -> Option<f64> {
    if share <= 0.0 || share >= 1.0 {
        None
    } else {
        Some(share * (pot + extra) / (1.0 - share))
    }
}

/// The later winnings at which a call breaks even, or `None` if `share` is 0
/// (no amount of later winnings can help a hand that never gets there).
pub fn break_even_x(share: f64, pot: f64, call: f64) -> Option<f64> {
    if share <= 0.0 {
        None
    } else {
        Some(call / share - pot - call)
    }
}
