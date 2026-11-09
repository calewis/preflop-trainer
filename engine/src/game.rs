//! Betting state for one street. Stacks are always equal at hand start, so
//! side pots cannot occur and an all-in for less needs no reopening logic.

pub const MAX_PLAYERS: usize = 7;

/// Seat 0 is the small blind, seat 1 the big blind, then the rest in order to
/// the button at seat n-1. `Bet(x)` and `Raise(x)` carry the street total
/// the player is making it ("to x").
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Act {
    Fold,
    Check,
    Call,
    Bet(f64),
    Raise(f64),
}

#[derive(Clone, Copy, Debug)]
pub struct Round {
    pub n: usize,
    /// 0 preflop, 1 flop, 2 turn, 3 river.
    pub street: u8,
    pub stacks: [f64; MAX_PLAYERS],
    pub put: [f64; MAX_PLAYERS],
    pub pot: f64,
    pub current: f64,
    pub last_raise: f64,
    pub raises: u8,
    pub in_hand: u8,
    pub to_act: u8,
    pub next: u8,
}

pub const EPS: f64 = 1e-9;

impl Round {
    /// Blinds posted, preflop, action on seat 2 (first in), equal stacks.
    pub fn preflop(n: usize, stack: f64) -> Round {
        assert!((3..=MAX_PLAYERS).contains(&n), "table size must be 3 to 7");
        let mut stacks = [0.0; MAX_PLAYERS];
        let mut put = [0.0; MAX_PLAYERS];
        for s in stacks.iter_mut().take(n) {
            *s = stack;
        }
        stacks[0] -= 0.5;
        put[0] = 0.5;
        stacks[1] -= 1.0;
        put[1] = 1.0;
        let in_hand = ((1u32 << n) - 1) as u8;
        Round {
            n,
            street: 0,
            stacks,
            put,
            pot: 1.5,
            current: 1.0,
            last_raise: 1.0,
            raises: 0,
            in_hand,
            to_act: in_hand,
            next: 2,
        }
    }

    /// A round on a given street with given stacks and in-hand mask. Everyone
    /// is yet to act, first to act is the lowest seat. Used by tests and
    /// spot construction.
    pub fn postflop(n: usize, street: u8, stacks: [f64; MAX_PLAYERS], in_hand: u8, pot: f64) -> Round {
        let mut r = Round {
            n,
            street,
            stacks,
            put: [0.0; MAX_PLAYERS],
            pot,
            current: 0.0,
            last_raise: 0.0,
            raises: 0,
            in_hand,
            to_act: 0,
            next: 0,
        };
        r.to_act = r.chips_mask();
        if r.to_act.count_ones() < 2 {
            r.to_act = 0;
        } else {
            r.next = r.first_to_act(0);
        }
        r
    }

    pub fn chips_mask(&self) -> u8 {
        let mut m = 0u8;
        for s in 0..self.n {
            if self.in_hand >> s & 1 == 1 && self.stacks[s] > EPS {
                m |= 1 << s;
            }
        }
        m
    }

    fn first_to_act(&self, from: usize) -> u8 {
        let mut s = from % self.n;
        for _ in 0..self.n {
            if self.to_act >> s & 1 == 1 {
                return s as u8;
            }
            s = (s + 1) % self.n;
        }
        0
    }

    /// Starts the next street from a finished round.
    pub fn next_street(&self) -> Round {
        let mut r = *self;
        r.street += 1;
        r.put = [0.0; MAX_PLAYERS];
        r.current = 0.0;
        r.last_raise = 0.0;
        r.raises = 0;
        r.to_act = r.chips_mask();
        if r.to_act.count_ones() < 2 {
            r.to_act = 0;
        } else {
            r.next = r.first_to_act(0);
        }
        r
    }

    pub fn in_hand_count(&self) -> u32 {
        self.in_hand.count_ones()
    }

    pub fn call_amount(&self, seat: usize) -> f64 {
        (self.current - self.put[seat]).max(0.0).min(self.stacks[seat])
    }

    pub fn is_facing(&self, seat: usize) -> bool {
        self.current > self.put[seat] + EPS
    }

    pub fn apply(&mut self, seat: usize, act: Act) {
        let bit = 1u8 << seat;
        match act {
            Act::Fold => {
                self.in_hand &= !bit;
                self.to_act &= !bit;
            }
            Act::Check => {
                self.to_act &= !bit;
            }
            Act::Call => {
                let a = self.call_amount(seat);
                self.put[seat] += a;
                self.stacks[seat] -= a;
                self.pot += a;
                self.to_act &= !bit;
            }
            Act::Bet(to) | Act::Raise(to) => {
                let a = to - self.put[seat];
                let raise_by = to - self.current;
                debug_assert!(a > 0.0, "bet/raise must add chips");
                debug_assert!(a <= self.stacks[seat] + 1e-6, "bet/raise exceeds stack");
                if raise_by + EPS >= self.last_raise {
                    self.last_raise = raise_by;
                }
                self.current = to;
                if matches!(act, Act::Raise(_)) {
                    self.raises += 1;
                }
                self.put[seat] = to;
                self.stacks[seat] -= a;
                self.pot += a;
                self.to_act = self.chips_mask() & !bit;
            }
        }
        if self.in_hand.count_ones() == 1 {
            self.to_act = 0;
        }
        if self.to_act != 0 {
            let mut s = (seat + 1) % self.n;
            loop {
                if self.to_act >> s & 1 == 1 {
                    self.next = s as u8;
                    break;
                }
                s = (s + 1) % self.n;
            }
        }
    }

    /// Bet total for a fraction of the pot: at least 1bb, capped all-in.
    pub fn bet_to(&self, seat: usize, frac: f64) -> f64 {
        let max = self.stacks[seat] + self.put[seat];
        (self.pot * frac).max(1.0).min(max)
    }

    /// Raise total at 3x the current bet: at least a full min-raise, capped
    /// all-in. None when the seat cannot add chips beyond a call.
    pub fn raise_to_3x(&self, seat: usize) -> Option<f64> {
        let max = self.stacks[seat] + self.put[seat];
        if max <= self.current + EPS {
            return None;
        }
        let to = (self.current * 3.0).max(self.current + self.last_raise);
        Some(to.min(max))
    }

    /// Hero's root options. Bets of 1/3, 2/3 and pot, deduplicated once all-in
    /// caps them; raise-to-3x when a raise is still allowed on this street.
    pub fn root_options(&self, seat: usize) -> Vec<(Act, String)> {
        let mut out = Vec::new();
        if self.is_facing(seat) {
            out.push((Act::Fold, "fold".to_string()));
            out.push((Act::Call, "call".to_string()));
            if self.raises < 2 {
                if let Some(to) = self.raise_to_3x(seat) {
                    out.push((Act::Raise(to), format!("raise{}", money(to, 2))));
                }
            }
        } else {
            out.push((Act::Check, "check".to_string()));
            let mut last_to = -1.0;
            for (frac, name) in [(1.0 / 3.0, "bet33"), (2.0 / 3.0, "bet67"), (1.0, "bet100")] {
                let to = self.bet_to(seat, frac);
                if (to - last_to).abs() > 1e-6 && to > 1e-9 {
                    out.push((Act::Bet(to), name.to_string()));
                    last_to = to;
                }
            }
        }
        out
    }

    /// Hero's options deep in the tree: check or bet 2/3; fold, call or raise 3x.
    pub fn deep_options(&self, seat: usize, out: &mut [Act; 3]) -> usize {
        if self.is_facing(seat) {
            out[0] = Act::Fold;
            out[1] = Act::Call;
            if self.raises < 2 {
                if let Some(to) = self.raise_to_3x(seat) {
                    out[2] = Act::Raise(to);
                    return 3;
                }
            }
            2
        } else {
            out[0] = Act::Check;
            out[1] = Act::Bet(self.bet_to(seat, 2.0 / 3.0));
            2
        }
    }

    /// Situation index: 0 unbet, 1 small, 2 medium, 3 large bet faced.
    pub fn situation(&self, seat: usize) -> usize {
        let a = self.call_amount(seat);
        if a <= EPS {
            return 0;
        }
        let before = (self.pot - a).max(1e-9);
        let f = a / before;
        if f < 0.45 {
            1
        } else if f < 0.8 {
            2
        } else {
            3
        }
    }

    pub fn other_active(&self, seat: usize) -> usize {
        self.in_hand_count() as usize - usize::from(self.in_hand >> seat & 1 == 1)
    }
}

/// Short money text: integers bare, otherwise one decimal (e.g. 12 or 7.5).
pub fn fmt_amt(x: f64) -> String {
    if (x - x.round()).abs() < 1e-6 {
        format!("{}", x.round() as i64)
    } else {
        format!("{x:.1}")
    }
}

/// Amount with up to `d` decimals, trailing zeros trimmed.
pub fn money(x: f64, d: usize) -> String {
    let s = format!("{x:.d$}");
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}
