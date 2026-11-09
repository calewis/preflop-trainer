//! Per-board tables, computed once per board and shared across all models:
//! each combo's strength class and, on the river, whether it beats hero.
//!
//! A class is a bucket 0..6 (0 strongest), plus 6 if the draw flag is set.

use crate::combos::{combos, NCOMBOS};
use crate::models::{bucket_of_share, NBUCKET, NCLASS};
use poker_trainer::cards::{Card, Holding};
use poker_trainer::eval::{evaluate, straight_top, HandRank};

/// Marks combos blocked by board cards.
pub const DEAD: u8 = 255;

/// Bucket of each live combo given the board (no draw flag).
pub fn classify(board: &[Card], with_draw: bool) -> Vec<u8> {
    let table = combos();
    let dead: u64 = board.iter().fold(0, |m, c| m | c.mask());
    let mut ranks: Vec<(HandRank, u16)> = Vec::with_capacity(NCOMBOS);
    let mut scratch = [Card::from_index(0); 7];
    let nb = board.len();
    scratch[2..2 + nb].copy_from_slice(board);
    for (i, &(a, b)) in table.cards.iter().enumerate() {
        if table.masks[i] & dead != 0 {
            continue;
        }
        scratch[0] = a;
        scratch[1] = b;
        ranks.push((evaluate(&scratch[..2 + nb]), i as u16));
    }
    ranks.sort_unstable_by_key(|x| x.0);
    let total = ranks.len() as f64;
    let mut out = vec![DEAD; NCOMBOS];
    let mut i = ranks.len();
    while i > 0 {
        let rank = ranks[i - 1].0;
        let mut j = i;
        while j > 0 && ranks[j - 1].0 == rank {
            j -= 1;
        }
        // Holdings strictly stronger, plus half the ties, as a share of all.
        let stronger = (ranks.len() - i) as f64;
        let ties = (i - j) as f64;
        let share = (stronger + 0.5 * ties) / total;
        let bucket = bucket_of_share(share);
        for &(_, combo) in &ranks[j..i] {
            out[combo as usize] = bucket;
        }
        i = j;
    }
    if with_draw {
        for (i, &(a, b)) in table.cards.iter().enumerate() {
            if out[i] != DEAD && has_draw(a, b, board) {
                out[i] += (NCLASS / 2) as u8;
            }
        }
    }
    out
}

/// Flush draw (four to a flush using at least one hole card, none complete)
/// or open-ended straight draw (at least two ranks complete a straight that
/// the board alone does not).
pub fn has_draw(a: Card, b: Card, board: &[Card]) -> bool {
    let mut suit_count = [0u8; 4];
    let mut ranks: u16 = 0;
    let mut board_ranks: u16 = 0;
    for c in board {
        suit_count[c.suit() as usize] += 1;
        ranks |= 1 << c.rank();
        board_ranks |= 1 << c.rank();
    }
    let board_suits = suit_count;
    suit_count[a.suit() as usize] += 1;
    suit_count[b.suit() as usize] += 1;
    ranks |= (1 << a.rank()) | (1 << b.rank());
    for s in 0..4 {
        if suit_count[s] == 4 && suit_count[s] > board_suits[s] {
            return true;
        }
    }
    if straight_top(ranks).is_some() {
        return false;
    }
    let completes = |m: u16| {
        (0..13).filter(|&r| m >> r & 1 == 0 && straight_top(m | (1 << r)).is_some()).count()
    };
    let with_hole = completes(ranks);
    with_hole >= 2 && with_hole > completes(board_ranks)
}

/// Each live combo's showdown result against hero on a complete board:
/// 0 hero wins, 1 tie, 2 villain wins. Combos blocked by hero or the board
/// are marked DEAD.
pub fn outcomes_vs_hero(hero: Holding, board: &[Card]) -> Vec<u8> {
    let table = combos();
    let dead: u64 = board.iter().fold(hero[0].mask() | hero[1].mask(), |m, c| m | c.mask());
    let hero_rank = evaluate(&[hero[0], hero[1], board[0], board[1], board[2], board[3], board[4]]);
    let mut scratch = [board[0], board[1], board[2], board[3], board[4], board[0], board[0]];
    let mut out = vec![DEAD; NCOMBOS];
    for (i, &(a, b)) in table.cards.iter().enumerate() {
        if table.masks[i] & dead != 0 {
            continue;
        }
        scratch[5] = a;
        scratch[6] = b;
        let r = evaluate(&scratch);
        out[i] = match r.cmp(&hero_rank) {
            std::cmp::Ordering::Less => 0,
            std::cmp::Ordering::Equal => 1,
            std::cmp::Ordering::Greater => 2,
        };
    }
    out
}

/// One complete sampled board under a turn branch.
pub struct RiverBranch {
    pub class: Vec<u8>,
    pub outcome: Vec<u8>,
    /// Scatter-add slot per combo: `(turn_class * NBUCKET + river_class) * 3 +
    /// outcome`, or `KEY_DEAD`. Empty for a river root.
    pub key: Vec<u16>,
}

/// Number of live slots in `RiverBranch::key`; `KEY_DEAD` is one past them.
pub const KEY_SLOTS: usize = NCLASS * NBUCKET * 3;
pub const KEY_DEAD: u16 = KEY_SLOTS as u16;

/// One turn under a spot (`class` is unused for root-river spots).
pub struct TurnBranch {
    pub class: Vec<u8>,
    pub rivers: Vec<RiverBranch>,
}

/// All sampled runouts for a spot, shared by every option and table. For a
/// river root there is one turn branch holding only the root river; for a turn
/// root one branch with the sampled rivers; for a flop root `turns` sampled
/// turn cards, each with its own river samples.
pub struct Runouts {
    pub root_street: u8,
    pub flop_class: Vec<u8>,
    pub turns: Vec<TurnBranch>,
}

impl Runouts {
    /// `turn_sample`/`river_sample` limit sampled cards; `None` enumerates all.
    /// `pick` must be a deterministic chooser of distinct cards.
    pub fn build(
        hero: Holding,
        board: &[Card],
        turn_sample: Option<usize>,
        river_sample: Option<usize>,
        mut pick: impl FnMut(&[Card], usize) -> Vec<Card>,
    ) -> Runouts {
        let dead: u64 = board.iter().fold(hero[0].mask() | hero[1].mask(), |m, c| m | c.mask());
        let unseen = |extra: u64| -> Vec<Card> {
            (0..52u8)
                .map(Card::from_index)
                .filter(|c| (dead | extra) & c.mask() == 0)
                .collect()
        };
        let river_branch = |b: &[Card], turn_class: &[u8]| {
            let class = classify(b, false);
            let outcome = outcomes_vs_hero(hero, b);
            let key = if turn_class.is_empty() {
                Vec::new()
            } else {
                (0..NCOMBOS)
                    .map(|i| {
                        if turn_class[i] == DEAD || class[i] == DEAD || outcome[i] == DEAD {
                            KEY_DEAD
                        } else {
                            ((usize::from(turn_class[i]) * NBUCKET + usize::from(class[i])) * 3 + usize::from(outcome[i])) as u16
                        }
                    })
                    .collect()
            };
            RiverBranch { class, outcome, key }
        };
        match board.len() {
            5 => Runouts {
                root_street: 3,
                flop_class: Vec::new(),
                turns: vec![TurnBranch { class: Vec::new(), rivers: vec![river_branch(board, &[])] }],
            },
            4 => {
                let deck = unseen(0);
                let cards = sampled(&deck, river_sample, &mut pick);
                let turn_class = classify(board, true);
                let mut rivers = Vec::with_capacity(cards.len());
                for c in cards {
                    let mut b = board.to_vec();
                    b.push(c);
                    rivers.push(river_branch(&b, &turn_class));
                }
                Runouts {
                    root_street: 2,
                    flop_class: Vec::new(),
                    turns: vec![TurnBranch { class: turn_class, rivers }],
                }
            }
            3 => {
                let flop_class = classify(board, true);
                let deck = unseen(0);
                let turns = sampled(&deck, turn_sample, &mut pick);
                let mut branches = Vec::with_capacity(turns.len());
                for t in turns {
                    let mut b4 = board.to_vec();
                    b4.push(t);
                    let class = classify(&b4, true);
                    let deck2 = unseen(t.mask());
                    let rc = sampled(&deck2, river_sample, &mut pick);
                    let mut rivers = Vec::with_capacity(rc.len());
                    for r in rc {
                        let mut b5 = b4.clone();
                        b5.push(r);
                        rivers.push(river_branch(&b5, &class));
                    }
                    branches.push(TurnBranch { class, rivers });
                }
                Runouts { root_street: 1, flop_class, turns: branches }
            }
            _ => panic!("Runouts expects 3, 4 or 5 board cards"),
        }
    }
}

fn sampled(deck: &[Card], k: Option<usize>, pick: &mut impl FnMut(&[Card], usize) -> Vec<Card>) -> Vec<Card> {
    match k {
        Some(k) if k < deck.len() => pick(deck, k),
        _ => deck.to_vec(),
    }
}
