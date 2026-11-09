//! Exact hand-computed river spots.

use poker_engine::board::Runouts;
use poker_engine::combos::combo_index;
use poker_engine::game::{Act, Round, MAX_PLAYERS};
use poker_engine::models::VillainTables;
use poker_engine::solve::{solve_root, SolveCfg, Villain};
use poker_trainer::cards::{parse_cards, parse_holding, Holding};

fn runouts(hero: Holding, board: &str) -> Runouts {
    let b = parse_cards(board).unwrap();
    Runouts::build(hero, &b, None, None, |d, _| d.to_vec())
}

fn belief(items: &[(&str, f32)]) -> Vec<f32> {
    let mut b = vec![0.0; poker_engine::combos::NCOMBOS];
    for (h, w) in items {
        let c = parse_holding(h).unwrap();
        b[combo_index(c[0], c[1])] = *w;
    }
    b
}

fn cfg() -> SolveCfg {
    SolveCfg { reach_eps: 0.0, line_decorrelate: true }
}

fn stacks(n: usize, s: f64) -> [f64; MAX_PLAYERS] {
    let mut a = [0.0; MAX_PLAYERS];
    for v in a.iter_mut().take(n) {
        *v = s;
    }
    a
}

/// Heads-up river, hero acts first, pot 10, stacks 90. Villain holds AA
/// (beats hero's KK) half the time and QQ (loses) half the time.
/// Hero's EV (relative to starting the river) is worked out by hand.
#[test]
fn river_call_and_check() {
    let hero = parse_holding("KsKd").unwrap();
    let r = runouts(hero, "2c3d7h9sJc");
    let b = belief(&[("AsAd", 0.5), ("QsQd", 0.5)]);
    // Villain folds never, calls always, never raises.
    let t = VillainTables::uniform([1.0, 0.0], [0.0, 1.0, 0.0], [0.5, 0.5, 0.5]);
    let v = [Villain { seat: 1, base: &b, belief: &b, tables: &t }];
    let root = Round::postflop(2, 3, stacks(2, 90.0), 0b11, 10.0);
    assert_eq!(root.next, 0);
    let opts = [Act::Check, Act::Bet(10.0)];
    let ev = solve_root(&root, 0, &v, &r, cfg(), &opts);
    // Check, villain checks: 50% win pot 10 => EV of 5 relative to the 90 behind.
    assert!((ev[0] - 5.0).abs() < 1e-5, "{}", ev[0]);
    // Bet 10, called: pot 30; win 50% => +30*0.5 - 10 = 5.
    assert!((ev[1] - 5.0).abs() < 1e-5, "{}", ev[1]);
}

/// Hero bets, villain folds with a fixed probability: EV is a simple mix.
#[test]
fn river_fold_equity() {
    let hero = parse_holding("KsKd").unwrap();
    let r = runouts(hero, "2c3d7h9sJc");
    let b = belief(&[("AsAd", 1.0)]);
    let t = VillainTables::uniform([1.0, 0.0], [0.6, 0.4, 0.0], [0.5, 0.5, 0.5]);
    let v = [Villain { seat: 1, base: &b, belief: &b, tables: &t }];
    let root = Round::postflop(2, 3, stacks(2, 90.0), 0b11, 10.0);
    let ev = solve_root(&root, 0, &v, &r, cfg(), &[Act::Bet(10.0)]);
    // 60% fold, hero wins pot 10: +10. 40% call, hero loses 10: -10. EV = 0.6*10 - 0.4*10 = 2.
    assert!((ev[0] - 2.0).abs() < 1e-5, "{}", ev[0]);
}

/// Exact three-way tie handling: both villains tie hero on the board.
#[test]
fn river_split_pot_three_way() {
    let hero = parse_holding("2s3s").unwrap();
    // Royal flush on board: all three players split.
    let r = runouts(hero, "AhKhQhJhTh");
    let b = belief(&[("7c8d", 1.0)]);
    let b2 = belief(&[("9c6d", 1.0)]);
    let t = VillainTables::uniform([1.0, 0.0], [0.0, 1.0, 0.0], [0.5, 0.5, 0.5]);
    let v = [
        Villain { seat: 1, base: &b, belief: &b, tables: &t },
        Villain { seat: 2, base: &b2, belief: &b2, tables: &t },
    ];
    let root = Round::postflop(3, 3, stacks(3, 80.0), 0b111, 30.0);
    let opts = [Act::Check, Act::Bet(10.0)];
    let ev = solve_root(&root, 0, &v, &r, cfg(), &opts);
    // Check-check-check: three-way split of pot 30 => +10.
    assert!((ev[0] - 10.0).abs() < 1e-5, "{}", ev[0]);
    // Bet 10, called twice: pot 60, split three ways = 20 back, hero invested 10 => +10.
    assert!((ev[1] - 10.0).abs() < 1e-5, "{}", ev[1]);
}

/// Mixed villain range: hero beats some holdings and loses to others, with
/// a class-dependent response. The class map is overridden so that holding
/// 'AA' is class 0 and everything else class 5; on the river the tree only
/// sees bucket classes.
#[test]
fn river_class_dependent_response() {
    let hero = parse_holding("KsKd").unwrap();
    let mut r = runouts(hero, "2c3d7h9sJc");
    let aa = combo_index(parse_holding("AsAd").unwrap()[0], parse_holding("AsAd").unwrap()[1]);
    let qq = combo_index(parse_holding("QsQd").unwrap()[0], parse_holding("QsQd").unwrap()[1]);
    let class = &mut r.turns[0].rivers[0].class;
    class[aa] = 0;
    class[qq] = 5;
    let b = belief(&[("AsAd", 0.25), ("QsQd", 0.75)]);
    let mut t = VillainTables::uniform([1.0, 0.0], [0.5, 0.5, 0.0], [0.5, 0.5, 0.5]);
    // Situation for a pot-size bet (10 into 10): large (3).
    t.set_class(2, 3, 0, [0.0, 1.0, 0.0]); // AA always calls.
    t.set_class(2, 3, 5, [1.0, 0.0, 0.0]); // QQ always folds.
    let v = [Villain { seat: 1, base: &b, belief: &b, tables: &t }];
    let root = Round::postflop(2, 3, stacks(2, 90.0), 0b11, 10.0);
    let ev = solve_root(&root, 0, &v, &r, cfg(), &[Act::Bet(10.0), Act::Check]);
    // Bet 10: 75% QQ folds, hero +10. 25% AA calls, hero -10. EV 5.
    assert!((ev[0] - 5.0).abs() < 1e-5, "{}", ev[0]);
    // Check, villain checks: hero wins pot 10 75% of the time. EV 7.5.
    assert!((ev[1] - 7.5).abs() < 1e-5, "{}", ev[1]);
}

/// Hero best-response: facing a bet, the correct fold/call/raise maximum
/// is taken at the deep node, with a raise that gets folded out sometimes.
#[test]
fn river_hero_best_response_at_depth() {
    let hero = parse_holding("KsKd").unwrap();
    let r = runouts(hero, "2c3d7h9sJc");
    let b = belief(&[("QsQd", 1.0)]);
    // Villain bets when checked to (always), folds 50% to a raise, calls 50%.
    let t = VillainTables::uniform([0.0, 1.0], [0.5, 0.5, 0.0], [0.5, 0.5, 0.5]);
    let v = [Villain { seat: 1, base: &b, belief: &b, tables: &t }];
    let root = Round::postflop(2, 3, stacks(2, 100.0), 0b11, 10.0);
    let ev = solve_root(&root, 0, &v, &r, cfg(), &[Act::Check]);
    // Hero checks, villain bets 5 (pot 15). Hero beats QQ.
    // Fold: 0. Call 5: pot 20, hero's stack ends +15. Raise to 15: pot 30;
    // 50% fold -> hero takes 30 having put 15, stack +15; 50% call -> pot 40, stack +25.
    // Raise averages +20, which beats calling (+15). Best at depth = raise => EV 20.
    assert!((ev[0] - 20.0).abs() < 1e-5, "{}", ev[0]);
}

/// Three-way with independent villains (no shared cards) and a known showdown.
#[test]
fn river_three_way_independent_showdown() {
    let hero = parse_holding("KsKd").unwrap();
    let r = runouts(hero, "2c3d7h9sJc");
    // V1 is 50% AA, 50% QQ. V2 is 50% AhAc, 50% QhQc.
    let b1 = belief(&[("AsAd", 0.5), ("QsQd", 0.5)]);
    let b2 = belief(&[("AhAc", 0.5), ("QhQc", 0.5)]);
    let t = VillainTables::uniform([1.0, 0.0], [0.0, 1.0, 0.0], [0.5, 0.5, 0.5]);
    let v = [
        Villain { seat: 1, base: &b1, belief: &b1, tables: &t },
        Villain { seat: 2, base: &b2, belief: &b2, tables: &t },
    ];
    let root = Round::postflop(3, 3, stacks(3, 90.0), 0b111, 20.0);
    let ev = solve_root(&root, 0, &v, &r, cfg(), &[Act::Check]);
    // Hero wins only if both have QQ: 0.25 of pot 20 => EV 5.
    assert!((ev[0] - 5.0).abs() < 1e-5, "{}", ev[0]);
}
