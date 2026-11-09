//! The versioned, line-based spot file.

use crate::game::{fmt_amt, money, Act, Round};
use crate::models::ARCHETYPES;
use crate::spot::{seat_label, Grade, HistAct, Spot};
use poker_trainer::cards::{cards_to_string, parse_cards};

pub const FORMAT_VERSION: u32 = 2;

fn act_text(a: &Act) -> String {
    match a {
        Act::Fold => "f".to_string(),
        Act::Check => "x".to_string(),
        Act::Call => "c".to_string(),
        Act::Bet(x) => format!("b{}", money(*x, 4)),
        Act::Raise(x) => format!("r{}", money(*x, 4)),
    }
}

/// The action history, streets separated by '/', each action `SEAT:code`.
pub fn history_text(spot: &Spot) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut street = 0;
    let mut current: Vec<String> = Vec::new();
    for a in &spot.history {
        while street < a.street {
            parts.push(current.join(","));
            current = Vec::new();
            street += 1;
        }
        current.push(format!("{}:{}", seat_label(spot.n, a.seat as usize), act_text(&a.act)));
    }
    // One group per street reached, including the decision street.
    let root_street = spot.board.len() as u8 - 2;
    while street < root_street {
        parts.push(current.join(","));
        current = Vec::new();
        street += 1;
    }
    parts.push(current.join(","));
    parts.join("/")
}

pub fn format_header(meta: &[(&str, String)]) -> String {
    let mut s = String::new();
    s.push_str(&format!("#SPOTS v{FORMAT_VERSION}\n"));
    s.push_str("# Every SPOT line has one '|' separated field per key below, in this order.\n");
    s.push_str("# id n=table size, seat=hero seat name, stack=starting stack in bb, hand, board, hist,\n");
    s.push_str("# pot=chips in the middle at the decision, call=amount to call, price=call/(pot+call) in percent,\n");
    s.push_str("# opts, verdict=robust|close|depends, best=best option by mean, gap=best minus next option as percent of pot,\n");
    s.push_str("# arch=main opponent archetype breakdown, meta=generation metadata.\n");
    s.push_str("# hist: one '/' separated group per street reached (preflop first, the decision street last, possibly empty),\n");
    s.push_str("#   actions comma separated as SEAT:code,\n");
    s.push_str("#   code = f fold, x check, c call, b<to> bet to amount, r<to> raise to amount (street totals, bb).\n");
    s.push_str("# opts: ';' separated label:ev_bb:ev_pct_pot:best_share (share of table weight where that option is best).\n");
    s.push_str("# arch: ';' separated name:weight:best_option.\n");
    s.push_str("# meta: tables=tables pooled, turns/rivers=runouts sampled, ess=per-seat effective sample size range,\n");
    s.push_str("#   agree=best+verdict (two independent halves agree on both), pool_match=1 if the pooled result equals the halves.\n");
    for (k, v) in meta {
        s.push_str(&format!("#meta {k}={v}\n"));
    }
    s
}

pub fn spot_line(id: usize, spot: &Spot, g: &Grade, turns: usize, rivers: usize, extra: &str) -> String {
    let opts = g
        .options
        .iter()
        .map(|o| format!("{}:{:.3}:{:.1}:{:.3}", o.label, o.mean_ev, o.pct_pot, o.best_share))
        .collect::<Vec<_>>()
        .join(";");
    let arch = g
        .breakdown
        .iter()
        .map(|b| format!("{}:{:.3}:{}", ARCHETYPES[b.archetype].name, b.weight, b.best_option))
        .collect::<Vec<_>>()
        .join(";");
    let (mn, mx) = g.ess.iter().fold((f64::MAX, 0f64), |(a, b), &e| (a.min(e), b.max(e)));
    format!(
        "SPOT|{}|n={}|{}|stack={}|{}|{}|{}|pot={:.2}|call={:.2}|price={:.1}|{}|{}|best={}|gap={:.2}|{}|meta=tables={},turns={},rivers={},ess={:.0}-{:.0}{}",
        id,
        spot.n,
        seat_label(spot.n, spot.hero_seat),
        fmt_amt(spot.stack),
        cards_to_string(&spot.hero),
        cards_to_string(&spot.board),
        history_text(spot),
        g.pot,
        g.to_call,
        g.price_pct,
        opts,
        g.verdict.code(),
        g.options[g.best].label,
        g.gap_pct,
        arch,
        g.tables,
        turns,
        rivers,
        mn,
        mx,
        extra,
    )
}

/// Rebuilds the `Spot` of a SPOT line (either format version). Amounts are
/// clamped to the replayed stacks, since the printed amounts are rounded.
pub fn parse_spot(line: &str) -> Spot {
    let f: Vec<&str> = line.split('|').collect();
    assert_eq!(f.len(), 17, "bad SPOT line");
    let n: usize = f[2].strip_prefix("n=").expect("n=").parse().expect("n");
    let hero_seat = (0..n).find(|&s| seat_label(n, s) == f[3]).expect("hero seat");
    let stack: f64 = f[4].strip_prefix("stack=").expect("stack=").parse().expect("stack");
    let cards = parse_cards(f[5]).expect("hand");
    let hero = [cards[0], cards[1]];
    let board = parse_cards(f[6]).expect("board");
    let mut r = Round::preflop(n, stack);
    let mut history = Vec::new();
    for (street, group) in f[7].split('/').enumerate() {
        if street > 0 {
            r = r.next_street();
        }
        for token in group.split(',').filter(|t| !t.is_empty()) {
            let (lab, code) = token.split_once(':').expect("token");
            let seat = (0..n).find(|&s| seat_label(n, s) == lab).expect("seat");
            let cap = r.stacks[seat] + r.put[seat];
            let amount = || code[1..].parse::<f64>().expect("amount").min(cap);
            let act = match &code[..1] {
                "f" => Act::Fold,
                "x" => Act::Check,
                "c" => Act::Call,
                "b" => Act::Bet(amount()),
                "r" => Act::Raise(amount()),
                other => panic!("bad action {other}"),
            };
            r.apply(seat, act);
            history.push(HistAct { street: street as u8, seat: seat as u8, act });
        }
    }
    Spot { n, hero_seat, stack, hero, board, history }
}
