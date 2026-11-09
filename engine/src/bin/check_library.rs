//! Strict checker for the spot library: parses every line, replays its
//! history through the betting rules, checks internal consistency, and compares
//! tallies with the header's metadata.
//! Usage: check_library FILE

use poker_engine::game::{Act, Round};
use poker_engine::models::SIZE_FRACS;
use poker_engine::spot::{preflop_can_raise, seat_label, PREFLOP_MAX_RAISES};
use poker_trainer::cards::parse_cards;

const TOL: f64 = 0.006; // Tolerance on printed two-decimal amounts, per comparison.

fn field<'a>(s: &'a str, key: &str, line: usize) -> &'a str {
    s.strip_prefix(key).unwrap_or_else(|| panic!("line {line}: expected {key} in {s:?}"))
}

fn seat_of(n: usize, label: &str, line: usize) -> usize {
    (0..n).find(|&s| seat_label(n, s) == label).unwrap_or_else(|| panic!("line {line}: bad seat {label}"))
}

fn parse_act(code: &str, line: usize) -> Act {
    match code.as_bytes()[0] {
        b'f' if code.len() == 1 => Act::Fold,
        b'x' if code.len() == 1 => Act::Check,
        b'c' if code.len() == 1 => Act::Call,
        b'b' => Act::Bet(code[1..].parse().unwrap_or_else(|_| panic!("line {line}: bad amount {code}"))),
        b'r' => Act::Raise(code[1..].parse().unwrap_or_else(|_| panic!("line {line}: bad amount {code}"))),
        _ => panic!("line {line}: bad action {code}"),
    }
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: check_library FILE");
    let text = std::fs::read_to_string(&path).expect("read file");
    let size = text.len();
    assert!(size < 2_000_000, "file is {size} bytes, over the 2 MB target");
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some("#SPOTS v2"), "missing version header");
    let mut meta = std::collections::HashMap::new();
    let (mut spots, mut street_n, mut pot_n, mut players_n, mut verdicts) = (0usize, [0usize; 3], [0usize; 3], [0usize; 3], [0usize; 3]);
    let mut max_len = 0;
    for (ln, line) in lines.enumerate() {
        let ln = ln + 2;
        if let Some(m) = line.strip_prefix("#meta ") {
            let (k, v) = m.split_once('=').expect("meta key=value");
            meta.insert(k.to_string(), v.to_string());
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        max_len = max_len.max(line.len());
        let f: Vec<&str> = line.split('|').collect();
        assert_eq!(f.len(), 17, "line {ln}: expected 17 fields, got {}", f.len());
        assert_eq!(f[0], "SPOT", "line {ln}");
        assert_eq!(f[1].parse::<usize>().expect("id"), spots, "line {ln}: ids must be sequential");
        let n: usize = field(f[2], "n=", ln).parse().expect("n");
        assert!((4..=7).contains(&n), "line {ln}: table size");
        let hero_seat = seat_of(n, f[3], ln);
        let stack: f64 = field(f[4], "stack=", ln).parse().expect("stack");
        assert!([60.0, 100.0, 150.0, 200.0].contains(&stack), "line {ln}: stack");
        let hand = parse_cards(f[5]).unwrap_or_else(|| panic!("line {ln}: bad hand"));
        let board = parse_cards(f[6]).unwrap_or_else(|| panic!("line {ln}: bad board"));
        assert_eq!(hand.len(), 2, "line {ln}");
        assert!((3..=5).contains(&board.len()), "line {ln}: board");
        assert!(hand.iter().all(|c| !board.contains(c)), "line {ln}: duplicate card");

        // Replay.
        let root_street = board.len() as u8 - 2;
        let groups: Vec<&str> = f[7].split('/').collect();
        assert_eq!(groups.len(), root_street as usize + 1, "line {ln}: expected one group per street");
        let mut r = Round::preflop(n, stack);
        let mut preflop_raises = 0;
        let mut acted = 0;
        for (street, g) in groups.iter().enumerate() {
            if street > 0 {
                assert_eq!(r.to_act, 0, "line {ln}: street {street} started before betting closed");
                r = r.next_street();
            }
            if g.is_empty() {
                continue;
            }
            for token in g.split(',') {
                let (lab, code) = token.split_once(':').unwrap_or_else(|| panic!("line {ln}: bad token {token}"));
                let seat = seat_of(n, lab, ln);
                assert_eq!(r.next as usize, seat, "line {ln}: out-of-turn action {token}");
                assert!(r.to_act >> seat & 1 == 1, "line {ln}: {token} has no action due");
                let act = parse_act(code, ln);
                let facing = r.is_facing(seat);
                match act {
                    Act::Fold => assert!(facing, "line {ln}: fold when not facing"),
                    Act::Check => assert!(!facing, "line {ln}: check when facing"),
                    Act::Call => assert!(facing, "line {ln}: call when not facing"),
                    Act::Bet(x) => {
                        assert!(!facing && r.current <= 1e-9, "line {ln}: bet into a bet");
                        assert!(x <= r.stacks[seat] + 0.001 && x > 0.0, "line {ln}: bet size");
                        assert!(SIZE_FRACS.iter().any(|&f| (r.bet_to(seat, f) - x).abs() <= 0.001), "line {ln}: bet {token} is not one of the size classes");
                    }
                    Act::Raise(x) => {
                        assert!(x > r.current, "line {ln}: raise must increase");
                        let all_in = (x - (r.stacks[seat] + r.put[seat])).abs() <= 0.001;
                        let min = r.current + r.last_raise;
                        assert!(all_in || x + 0.001 >= min, "line {ln}: under-minimum raise {token} (min {min:.2})");
                        if street == 0 {
                            assert!(preflop_can_raise(&r, seat) && r.raises < PREFLOP_MAX_RAISES, "line {ln}: preflop raise cap");
                            preflop_raises += 1;
                        } else {
                            assert!(r.raises < 2, "line {ln}: more than 2 raises on a street");
                        }
                    }
                }
                // Printed amounts are rounded, so clamp to the actual stack.
                let act = match act {
                    Act::Bet(x) => Act::Bet(x.min(r.stacks[seat] + r.put[seat])),
                    Act::Raise(x) => Act::Raise(x.min(r.stacks[seat] + r.put[seat])),
                    a => a,
                };
                r.apply(seat, act);
                acted += 1;
                assert!(r.in_hand >> hero_seat & 1 == 1, "line {ln}: hero folded before the spot");
            }
        }
        assert!(acted > 0, "line {ln}: empty history");
        assert_eq!(r.next as usize, hero_seat, "line {ln}: hero is not next to act");
        assert!(r.to_act >> hero_seat & 1 == 1, "line {ln}: hero has no action due");
        assert!(r.in_hand.count_ones() >= 2, "line {ln}: no opponent left");

        // Printed money. History amounts carry 4 decimals, so replay drift is tiny.
        let pot: f64 = field(f[8], "pot=", ln).parse().expect("pot");
        let call: f64 = field(f[9], "call=", ln).parse().expect("call");
        let price: f64 = field(f[10], "price=", ln).parse().expect("price");
        let slack = TOL + 0.0001 * f64::from(acted);
        assert!((pot - r.pot).abs() <= slack, "line {ln}: pot {pot} vs replay {:.3}", r.pot);
        assert!((call - r.call_amount(hero_seat)).abs() <= slack, "line {ln}: call {call} vs replay {:.3}", r.call_amount(hero_seat));
        let want_price = if call > 0.0 { 100.0 * call / (pot + call) } else { 0.0 };
        assert!((price - want_price).abs() <= 0.11, "line {ln}: price");

        // Options.
        let opts: Vec<Vec<&str>> = f[11].split(';').map(|o| o.split(':').collect()).collect();
        assert!(opts.len() >= 2, "line {ln}: too few options");
        let (mut sum_share, mut best_ev, mut best_label, mut second) = (0.0, f64::NEG_INFINITY, "", f64::NEG_INFINITY);
        let mut best_share = 0.0;
        for o in &opts {
            assert_eq!(o.len(), 4, "line {ln}: option fields");
            let ev: f64 = o[1].parse().expect("ev");
            let pct: f64 = o[2].parse().expect("pct");
            let share: f64 = o[3].parse().expect("share");
            // The percent uses the exact pot; the printed pot rounds by up to 0.005.
            assert!((pct - 100.0 * ev / pot).abs() <= 0.06 + 100.0 * 0.0006 / pot + pct.abs() * 0.0051 / pot, "line {ln}: EV percent inconsistent ({ev} bb, {pct}% of {pot})");
            assert!((0.0..=1.0005).contains(&share), "line {ln}: share");
            sum_share += share;
            if ev > best_ev {
                second = best_ev;
                best_ev = ev;
                best_label = o[0];
                best_share = share;
            } else if ev > second {
                second = ev;
            }
        }
        assert!((sum_share - 1.0).abs() <= 0.01, "line {ln}: option shares sum to {sum_share}");
        let labels: Vec<&str> = opts.iter().map(|o| o[0]).collect();
        // Raise labels carry the amount, printed to 2 decimals; the replayed amount
        // can differ slightly because history amounts are printed to 4 decimals.
        let want_labels: Vec<String> = r.root_options(hero_seat).into_iter().map(|x| x.1).collect();
        assert_eq!(labels.len(), want_labels.len(), "line {ln}: option count differs from the legal root options");
        for (got, want) in labels.iter().zip(&want_labels) {
            if let (Some(g), Some(w)) = (got.strip_prefix("raise"), want.strip_prefix("raise")) {
                let (g, w): (f64, f64) = (g.parse().expect("raise amount"), w.parse().expect("raise amount"));
                assert!((g - w).abs() <= 0.011, "line {ln}: raise label {got} vs replayed {want}");
            } else {
                assert_eq!(*got, want.as_str(), "line {ln}: option label does not match the legal root options");
            }
        }

        let best = field(f[13], "best=", ln);
        // Displayed EVs are rounded to 3 decimals; allow ties at that precision.
        let tied = opts.iter().any(|o| o[0] == best && (o[1].parse::<f64>().unwrap() - best_ev).abs() < 0.0015);
        assert!(best == best_label || tied, "line {ln}: best={best} but highest EV is {best_label}");
        let gap: f64 = field(f[14], "gap=", ln).parse().expect("gap");
        assert!(gap >= -0.01, "line {ln}: negative gap");
        let share_of_best: f64 = opts.iter().find(|o| o[0] == best).map_or(best_share, |o| o[3].parse().unwrap());
        let verdict = f[12];
        let idx = match verdict {
            "robust" => {
                assert!(share_of_best >= 0.8995 && gap >= 0.98, "line {ln}: robust inconsistent (share {share_of_best}, gap {gap})");
                0
            }
            "close" => {
                assert!(gap <= 1.02, "line {ln}: close with gap {gap}");
                1
            }
            "depends" => {
                assert!(gap >= 0.98, "line {ln}: depends with gap under 1%");
                assert!(share_of_best < 0.9005 , "line {ln}: depends but best share {share_of_best}");
                2
            }
            _ => panic!("line {ln}: bad verdict {verdict}"),
        };
        verdicts[idx] += 1;

        // Breakdown and metadata.
        let arch: Vec<Vec<&str>> = f[15].split(';').map(|a| a.split(':').collect()).collect();
        let mut w = 0.0;
        for a in &arch {
            assert_eq!(a.len(), 3, "line {ln}: breakdown fields");
            assert!(["station", "loose-passive", "nit", "maniac", "solid"].contains(&a[0]), "line {ln}: archetype {}", a[0]);
            assert!(labels.contains(&a[2]), "line {ln}: breakdown option {}", a[2]);
            w += a[1].parse::<f64>().expect("weight");
        }
        assert!((w - 1.0).abs() <= 0.01, "line {ln}: breakdown weights sum to {w}");
        let m = field(f[16], "meta=", ln);
        for key in ["tables=", "turns=", "rivers=", "ess=", "agree=best+verdict", "pool_match="] {
            assert!(m.contains(key), "line {ln}: metadata missing {key}");
        }

        street_n[board.len() - 3] += 1;
        pot_n[preflop_raises.min(2)] += 1;
        let villains = r.in_hand.count_ones() as usize - 1;
        players_n[villains.min(3) - 1] += 1;
        spots += 1;
    }
    println!("OK: {spots} spots, {size} bytes, longest line {max_len} chars");
    let check = |name: &str, want: String| {
        let got = meta.get(name).cloned().unwrap_or_default();
        assert_eq!(got, want, "header {name} disagrees with the lines");
        println!("{name}: {got}");
    };
    check("spots", spots.to_string());
    check("streets", format!("flop={},turn={},river={}", street_n[0], street_n[1], street_n[2]));
    check("pot_types", format!("limped={},single_raised={},reraised={}", pot_n[0], pot_n[1], pot_n[2]));
    check("players_left", format!("two={},three={},four_plus={}", players_n[0], players_n[1], players_n[2]));
    check("verdicts", format!("robust={},close={},depends={}", verdicts[0], verdicts[1], verdicts[2]));
}
