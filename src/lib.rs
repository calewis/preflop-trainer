//! Poker fundamentals trainer: pure library.
//!
//! Nothing in this crate touches stdin, stdout, the file system or the clock.
//! Every function takes plain inputs and returns structured data, so another
//! front end can render the same items and grades its own way.

pub mod answer;
pub mod baseline;
pub mod cards;
pub mod classes;
pub mod equity;
pub mod eval;
pub mod generate;
pub mod grade;
pub mod item;
pub mod log;
pub mod money;
pub mod preflop;
pub mod progress;
pub mod rng;
pub mod schedule;
