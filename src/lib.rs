//! Poker fundamentals trainer: pure library.
//!
//! Nothing in this crate touches stdin, stdout, the file system or the clock.
//! Every function takes plain inputs and returns structured data, so another
//! front end can render the same items and grades its own way.

pub mod anchors;
pub mod answer;
pub mod baseline;
pub mod beats_me;
pub mod board_type;
pub mod cards;
pub mod checkup;
pub mod classes;
pub mod equity;
pub mod equity_skill;
pub mod eval;
pub mod family;
pub mod generate;
pub mod groups;
pub mod grade;
pub mod item;
pub mod ladder;
pub mod log;
pub mod money;
pub mod preflop;
pub mod progress;
pub mod read_price;
pub mod reflection;
pub mod rng;
pub mod schedule;
pub mod table;
