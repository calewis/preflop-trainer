//! Compares the factored (independent villains) showdown share with exact
//! enumeration of villain holdings under the joint no-overlap distribution.

use crate::board::outcomes_vs_hero;
use crate::combos::{combos, NCOMBOS};
use crate::solve::hero_share;
use poker_trainer::cards::{Card, Holding};

pub struct BlockerResult {
    pub factored: f64,
    /// Correction applied once, to the final river beliefs.
    pub corrected: f64,
    /// Correction applied at the flop-to-turn transition only, then the turn
    /// and river likelihoods multiplied on, as the expectimax does along a line.
    pub along_line: f64,
    pub exact: f64,
}

fn factored_share(outcome: &[u8], beliefs: &[Vec<f32>]) -> f64 {
    let (mut beat, mut tie) = (Vec::new(), Vec::new());
    for b in beliefs {
        let (mut total, mut wb, mut wt) = (0f64, 0f64, 0f64);
        for i in 0..NCOMBOS {
            if outcome[i] == crate::board::DEAD {
                continue;
            }
            let w = f64::from(b[i]);
            total += w;
            if outcome[i] == 2 {
                wb += w;
            } else if outcome[i] == 1 {
                wt += w;
            }
        }
        beat.push(wb / total);
        tie.push(wt / total);
    }
    hero_share(&beat, &tie)
}

/// `parts` are each villain's (through-flop, turn-and-river) factors.
pub fn compare(hero: Holding, board: &[Card], parts: &[(Vec<f32>, Vec<f32>)]) -> BlockerResult {
    let outcome = outcomes_vs_hero(hero, board);
    let combine = |early: &[f32], late: &[f32]| -> Vec<f32> {
        let mut b: Vec<f32> = early.iter().zip(late).map(|(a, c)| a * c).collect();
        let total: f32 = b.iter().sum();
        for w in b.iter_mut() {
            *w /= total;
        }
        b
    };
    let beliefs: Vec<Vec<f32>> = parts.iter().map(|(e, l)| combine(e, l)).collect();
    let factored = factored_share(&outcome, &beliefs);
    let mut adjusted = beliefs.clone();
    crate::solve::decorrelate(&mut adjusted, 2);
    let corrected = factored_share(&outcome, &adjusted);
    let mut early: Vec<Vec<f32>> = parts.iter().map(|(e, _)| e.clone()).collect();
    crate::solve::decorrelate(&mut early, 2);
    let line: Vec<Vec<f32>> = early.iter().zip(parts).map(|(e, (_, l))| combine(e, l)).collect();
    let along_line = factored_share(&outcome, &line);
    let exact = match beliefs.len() {
        2 => exact_two(&outcome, &beliefs[0], &beliefs[1]),
        3 => exact_three(&outcome, &beliefs[0], &beliefs[1], &beliefs[2]),
        n => panic!("exact enumeration supports 2 or 3 villains, got {n}"),
    };
    BlockerResult { factored, corrected, along_line, exact }
}

fn exact_two(outcome: &[u8], b1: &[f32], b2: &[f32]) -> f64 {
    let table = combos();
    let (mut mass, mut shared) = (0f64, 0f64);
    for i in 0..NCOMBOS {
        if b1[i] <= 0.0 || outcome[i] == crate::board::DEAD {
            continue;
        }
        let m1 = table.masks[i];
        for j in 0..NCOMBOS {
            if b2[j] <= 0.0 || outcome[j] == crate::board::DEAD || m1 & table.masks[j] != 0 {
                continue;
            }
            let w = f64::from(b1[i]) * f64::from(b2[j]);
            mass += w;
            if outcome[i] != 2 && outcome[j] != 2 {
                let ties = u32::from(outcome[i] == 1) + u32::from(outcome[j] == 1);
                shared += w / f64::from(1 + ties);
            }
        }
    }
    shared / mass
}

/// Three villains: enumerates two villains directly and the third by
/// inclusion-exclusion over the four removed cards.
fn exact_three(outcome: &[u8], b1: &[f32], b2: &[f32], b3: &[f32]) -> f64 {
    let table = combos();
    // Per outcome (0 hero beats villain, 1 tie, 2 villain beats hero):
    // total mass, mass by single card, and per-combo mass.
    let mut total = [0f64; 3];
    let mut by_card = [[0f64; 52]; 3];
    let mut combo_mass = vec![[0f64; 3]; NCOMBOS];
    for k in 0..NCOMBOS {
        if outcome[k] == crate::board::DEAD || b3[k] <= 0.0 {
            continue;
        }
        let o = outcome[k] as usize;
        let w = f64::from(b3[k]);
        total[o] += w;
        let (a, b) = table.cards[k];
        by_card[o][a.index()] += w;
        by_card[o][b.index()] += w;
        combo_mass[k][o] = w;
    }
    let (mut mass, mut shared) = (0f64, 0f64);
    for i in 0..NCOMBOS {
        if b1[i] <= 0.0 || outcome[i] == crate::board::DEAD {
            continue;
        }
        let (a, b) = table.cards[i];
        let m1 = table.masks[i];
        for j in 0..NCOMBOS {
            if b2[j] <= 0.0 || outcome[j] == crate::board::DEAD || m1 & table.masks[j] != 0 {
                continue;
            }
            let (c, d) = table.cards[j];
            let cards = [a.index(), b.index(), c.index(), d.index()];
            let w12 = f64::from(b1[i]) * f64::from(b2[j]);
            let mut s = [0f64; 3];
            for o in 0..3 {
                let mut v = total[o];
                for &x in &cards {
                    v -= by_card[o][x];
                }
                for p in 0..4 {
                    for q in p + 1..4 {
                        let idx = table.index[cards[p]][cards[q]] as usize;
                        v += combo_mass[idx][o];
                    }
                }
                s[o] = v;
            }
            mass += w12 * (s[0] + s[1] + s[2]);
            if outcome[i] != 2 && outcome[j] != 2 {
                let ties = u32::from(outcome[i] == 1) + u32::from(outcome[j] == 1);
                shared += w12 * (s[0] / f64::from(1 + ties) + s[1] / f64::from(2 + ties));
            }
        }
    }
    shared / mass
}
