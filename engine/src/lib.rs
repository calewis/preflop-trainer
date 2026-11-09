//! Grades hold'em decisions by best response against a sampled population of
//! fixed, stochastic, non-GTO opponents. See README.md.

pub mod board;
pub mod combos;
pub mod game;
pub mod joint;
pub mod memguard;
pub mod models;
pub mod output;
pub mod preflop_order;
pub mod prof;
pub mod reference;
pub mod sim;
pub mod solve;
pub mod spot;
pub mod validation;
