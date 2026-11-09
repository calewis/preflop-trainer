//! Hand-computed cases for the joint solver.

use poker_engine::board::Runouts;
use poker_engine::combos::combo_index;
use poker_engine::game::{Act, Round, MAX_PLAYERS};
use poker_engine::joint::solve_joint_fast;
use poker_engine::models::VillainTables;
use poker_engine::solve::{SolveCfg, Villain};
use poker_trainer::cards::{parse_cards, parse_holding, Holding};

fn runouts(hero: Holding, board: &str) -> Runouts {
    let b = parse_cards(board).unwrap();
    Runouts::build(hero, &b, None, None, |d, _| d.to_vec())
}

fn belief(h: &str) -> Vec<f32> {
    let mut b = vec![0.0; poker_engine::combos::NCOMBOS];
    let c = parse_holding(h).unwrap();
    b[combo_index(c[0], c[1])] = 1.0;
    b
}

fn cfg() -> SolveCfg {
    SolveCfg { reach_eps: 0.0, line_decorrelate: true }
}

fn root(stack: f64, pot: f64) -> Round {
    let mut s = [0.0; MAX_PLAYERS];
    s[0] = stack;
    s[1] = stack;
    Round::postflop(2, 3, s, 0b11, pot)
}

fn table<'a>(b: &'a [f32], t: &'a VillainTables) -> Vec<Villain<'a>> {
    vec![Villain { seat: 1, base: b, belief: b, tables: t }]
}

/// Hero checks, villain bets pot (10 into 10), and always calls a raise. Hero KK
/// beats QQ on table 0 and loses to AA on tables 1-3. A per-table best response
/// would raise on table 0 and fold elsewhere; committed across the four tables
/// that share the node, folding is best: call = (20 - 30) / 4 < 0.
#[test]
fn fused_decision_at_a_shared_node() {
    let hero = parse_holding("KsKd").unwrap();
    let run = runouts(hero, "2c3d7h9sJc");
    let (qq, aa) = (belief("QsQd"), belief("AsAd"));
    let t = VillainTables::uniform([0.0, 1.0], [0.0, 1.0, 0.0], [1.0, 1.0, 1.0]);
    let tables = vec![table(&qq, &t), table(&aa, &t), table(&aa, &t), table(&aa, &t)];
    let ev = solve_joint_fast(&root(90.0, 10.0), 0, &tables, &run, cfg(), &[Act::Check]);
    for row in &ev {
        assert!(row[0].abs() < 1e-6, "folding everywhere gives 0, got {ev:?}");
    }
}

/// A 1/3-pot bet and a pot bet are different public actions, so the two tables
/// separate and each gets its own decision. Pot 10. Table 0: villain QQ bets
/// 3.33; hero raises to 10, called: pot 30, stack 80 + 30 = +20. Table 1: villain
/// AA bets 10; hero folds, 0.
#[test]
fn visible_bet_size_separates_tables() {
    let hero = parse_holding("KsKd").unwrap();
    let run = runouts(hero, "2c3d7h9sJc");
    let (qq, aa) = (belief("QsQd"), belief("AsAd"));
    let small = VillainTables::uniform([0.0, 1.0], [0.0, 1.0, 0.0], [1.0 / 3.0; 3]);
    let large = VillainTables::uniform([0.0, 1.0], [0.0, 1.0, 0.0], [1.0; 3]);
    let tables = vec![table(&qq, &small), table(&aa, &large)];
    let ev = solve_joint_fast(&root(90.0, 10.0), 0, &tables, &run, cfg(), &[Act::Check]);
    assert!((ev[0][0] - 20.0).abs() < 1e-4, "{ev:?}");
    assert!(ev[1][0].abs() < 1e-6, "{ev:?}");
}

/// Table weights follow the observed action. Villain bets half pot with AA on
/// table 0 (probability 0.9) and with QQ on table 1 (probability 0.1). After
/// the bet the tables weigh 0.9 and 0.1, so a call has weighted EV
/// 0.9 * -5 + 0.1 * 15 < 0 and hero folds. Equal weights would call instead.
/// Table 0: check 0.1 -> showdown loses (0), bet -> fold (0): EV 0.
/// Table 1: check 0.9 -> showdown wins the 10 pot, bet -> fold 0: EV 9.
#[test]
fn observed_action_reweights_tables() {
    let hero = parse_holding("KsKd").unwrap();
    let run = runouts(hero, "2c3d7h9sJc");
    let (aa, qq) = (belief("AsAd"), belief("QsQd"));
    let bets_often = VillainTables::uniform([0.1, 0.9], [0.0, 1.0, 0.0], [0.5; 3]);
    let bets_rarely = VillainTables::uniform([0.9, 0.1], [0.0, 1.0, 0.0], [0.5; 3]);
    let tables = vec![table(&aa, &bets_often), table(&qq, &bets_rarely)];
    let ev = solve_joint_fast(&root(90.0, 10.0), 0, &tables, &run, cfg(), &[Act::Check]);
    assert!(ev[0][0].abs() < 1e-5, "{ev:?}");
    assert!((ev[1][0] - 9.0).abs() < 1e-5, "{ev:?}");
}
