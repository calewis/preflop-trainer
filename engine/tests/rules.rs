//! Betting rules: minimum raises, all-ins, acting order.

use poker_engine::game::{Act, Round};

#[test]
fn preflop_order_and_blinds() {
    let mut r = Round::preflop(5, 100.0);
    assert_eq!(r.pot, 1.5);
    assert_eq!(r.next, 2); // UTG first
    assert_eq!(r.current, 1.0);
    for seat in [2, 3, 4, 0] {
        assert_eq!(r.next as usize, seat);
        r.apply(seat, Act::Call);
    }
    // Big blind has the option even though everyone called.
    assert_eq!(r.next, 1);
    assert!(!r.is_facing(1));
    r.apply(1, Act::Check);
    assert_eq!(r.to_act, 0);
    assert_eq!(r.pot, 5.0);
    let r2 = r.next_street();
    assert_eq!(r2.street, 1);
    assert_eq!(r2.next, 0); // small blind acts first postflop
}

#[test]
fn min_raise_tracks_last_raise_and_postflop_order_wraps() {
    let mut r = Round::preflop(4, 100.0);
    r.apply(2, Act::Raise(4.0)); // open to 4, raise size 3
    r.apply(3, Act::Raise(7.0)); // 3-bet to 7: min legal is 4 + 3
    assert_eq!(r.last_raise, 3.0);
    assert_eq!(r.raise_to_3x(0), Some(21.0));
    // Action continues to SB, BB, then back to the opener.
    assert_eq!(r.next, 0);
    r.apply(0, Act::Fold);
    assert_eq!(r.next, 1);
    r.apply(1, Act::Fold);
    assert_eq!(r.next, 2);
    r.apply(2, Act::Call);
    assert_eq!(r.to_act, 0);
    let flop = r.next_street();
    assert_eq!(flop.next, 2); // first active postflop among remaining seats
    assert_eq!(flop.stacks[2], 93.0);
}

#[test]
fn min_raise_postflop_and_all_in_cap() {
    let mut stacks = [0.0; 7];
    stacks[0] = 30.0;
    stacks[1] = 30.0;
    let mut r = Round::postflop(2, 1, stacks, 0b11, 20.0);
    r.apply(0, Act::Bet(10.0));
    // Raise: 3x would be 30, equal to the stack (all-in).
    assert_eq!(r.raise_to_3x(1), Some(30.0));
    r.apply(1, Act::Raise(30.0));
    assert_eq!(r.call_amount(0), 20.0);
    r.apply(0, Act::Call);
    assert_eq!(r.to_act, 0);
    assert_eq!(r.pot, 80.0);
    // Both players all-in: no betting on later streets.
    let next = r.next_street();
    assert_eq!(next.to_act, 0);
}

#[test]
fn all_in_for_less_than_a_full_raise_is_capped() {
    let mut stacks = [0.0; 7];
    stacks[0] = 100.0;
    stacks[1] = 16.0;
    let mut r = Round::postflop(2, 1, stacks, 0b11, 10.0);
    r.apply(0, Act::Bet(10.0));
    // Seat 1 has 16 behind: a full 3x would be 30, min-raise 20, so it can only shove to 16.
    assert_eq!(r.raise_to_3x(1), Some(16.0));
}
