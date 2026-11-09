//! Conditioning, fold baseline, helper equivalence, determinism and notation
//! for the preflop chart derivation.

use poker_engine::game::{Act, Round};
use poker_engine::models::{PlayerModel, PreflopCtx};
use poker_engine::preflop_charts::*;
use poker_engine::spot::{position_of, PreflopTrack};
use poker_trainer::preflop::Seat;
use poker_trainer::rng::Rng;

fn pool() -> Pool {
    Pool::build(5, 1024, 256, 4)
}

fn deals(chart: usize, n: usize, pool: &Pool) -> Vec<Deal> {
    let ch = &CHARTS[chart];
    let seats = SeatTable::for_mask(ch.hero);
    let mut rng = Rng::new(99 + chart as u64);
    let mut at = 0;
    (0..n).map(|i| sample_deal(ch, &seats, 12 + (i % 150), pool, &mut rng, &mut at)).collect()
}

#[test]
fn chart_index_constants_match() {
    let check = [(VS3BET_IP, "vs3bet.ip"), (VS3BET_OOP, "vs3bet.oop"), (VS_EM_IP, "vs_em.ip"), (VS_EM_SB, "vs_em.sb"), (VS_EM_BB, "vs_em.bb"), (VS_CO_IP, "vs_co.ip"), (VS_CO_SB, "vs_co.sb"), (VS_CO_BB, "vs_co.bb"), (VS_LATE_SB, "vs_late.sb"), (VS_LATE_BB, "vs_late.bb"), (MW_EM_IP, "mw_em.ip"), (MW_EM_SB, "mw_em.sb"), (MW_EM_BB, "mw_em.bb"), (MW_LATE_SB, "mw_late.sb"), (MW_LATE_BB, "mw_late.bb"), (OPEN_E, "open.early"), (OPEN_M, "open.middle"), (OPEN_CO, "open.cutoff"), (OPEN_BTN, "open.button"), (OPEN_SB, "open.small_blind"), (LIMP1_M, "limp1.middle"), (LIMP1_BB, "limp1.big_blind"), (LIMP2_M, "limp2.middle"), (LIMP2_BB, "limp2.big_blind")];
    for (i, id) in check {
        assert_eq!(CHARTS[i].id, id);
    }
    // Stage order is the array order.
    for w in CHARTS.windows(2) {
        assert!(w[0].stage <= w[1].stage);
    }
    // Limp chart indices run middle, cutoff, button, small blind, big blind.
    for (off, id) in ["limp1.middle", "limp1.cutoff", "limp1.button", "limp1.small_blind", "limp1.big_blind"].iter().enumerate() {
        assert_eq!(CHARTS[LIMP1_M + off].id, *id);
    }
    for (off, id) in ["limp2.middle", "limp2.cutoff", "limp2.button", "limp2.small_blind", "limp2.big_blind"].iter().enumerate() {
        assert_eq!(CHARTS[LIMP2_M + off].id, *id);
    }
}

/// Every action before hero's decision fits the chart's situation exactly.
#[test]
fn conditioning_matches_situation() {
    let pool = pool();
    for (i, ch) in CHARTS.iter().enumerate() {
        for d in deals(i, 400, &pool) {
            assert!(ch.hero >> (position_of(d.n, d.hero) as usize) & 1 == 1, "{} seat", ch.id);
            let hist = &d.hist[..d.hist_len];
            match ch.sit {
                Sit::FoldedTo => {
                    assert!(hist.iter().all(|h| h.kind == 0), "{}: {hist:?}", ch.id);
                    assert_eq!(d.r.raises, 0);
                    assert_eq!(hist.len(), before_count(d.n, d.hero));
                }
                Sit::Limped { lo, hi } => {
                    let limps = hist.iter().filter(|h| h.kind == 2).count() as u8;
                    assert!(hist.iter().all(|h| h.kind == 0 || h.kind == 2));
                    assert!(limps >= lo && limps <= hi, "{}: {limps}", ch.id);
                    assert_eq!(d.r.raises, 0);
                    assert!(d.track.limpers == limps);
                }
                Sit::FacingOpen { opener, lo, hi } => {
                    assert_eq!(hist.iter().filter(|h| h.kind == 3).count(), 1);
                    let idx = hist.iter().position(|h| h.kind == 3).unwrap();
                    assert!(hist[..idx].iter().all(|h| h.kind == 0), "{}: limper before open", ch.id);
                    let opener_pos = position_of(d.n, hist[idx].seat as usize) as usize;
                    assert!(opener >> opener_pos & 1 == 1, "{}: opener", ch.id);
                    let callers = hist[idx + 1..].iter().filter(|h| h.kind == 2).count() as u8;
                    assert!(hist[idx + 1..].iter().all(|h| h.kind == 0 || h.kind == 2));
                    assert!(callers >= lo && callers <= hi);
                    assert_eq!(d.r.raises, 1);
                }
                Sit::Opened3bet { ip } => {
                    let open_idx = hist.iter().position(|h| h.kind == 4).unwrap();
                    assert!(hist[..open_idx].iter().all(|h| h.kind == 0), "{}: before open", ch.id);
                    let raises: Vec<_> = hist[open_idx + 1..].iter().filter(|h| h.kind == 3).collect();
                    assert_eq!(raises.len(), 1, "{}: one 3-bet", ch.id);
                    assert!(hist[open_idx + 1..].iter().all(|h| h.kind == 0 || h.kind == 3));
                    let three = raises[0].seat as usize;
                    assert_eq!(d.hero > three, ip, "{}: position", ch.id);
                    assert_eq!(d.r.raises, 2);
                }
            }
            assert_eq!(d.r.next as usize, d.hero, "{} decision on hero", ch.id);
            // Cards distinct across seats and board.
            let mut mask = pool.masks[d.board];
            for s in 0..d.n {
                let (a, b) = poker_engine::combos::combos().cards[d.combos[s] as usize];
                for c in [a, b] {
                    assert_eq!(mask & c.mask(), 0, "card overlap");
                    mask |= c.mask();
                }
            }
        }
    }
}

fn before_count(n: usize, seat: usize) -> usize {
    match seat {
        0 => n - 2,
        1 => n - 1,
        s => s - 2,
    }
}

/// Fold net is minus exactly what hero has already put in.
#[test]
fn fold_baseline_counts_posted_money() {
    let pool = pool();
    let pol = HeroPolicy::Charts(&[None; 30]);
    for (i, ch) in CHARTS.iter().enumerate() {
        if ch.labels[0].is_none() {
            continue;
        }
        for d in deals(i, 200, &pool) {
            let net = play(&pool, &d, Some(Pure::Fold), &pol, 123);
            let expect = -d.r.put[d.hero];
            assert!((net - expect).abs() < 1e-9, "{}: {net} vs {expect}", ch.id);
            match ch.sit {
                Sit::Opened3bet { .. } => assert!((net + 3.0).abs() < 1e-9, "{}: open size {net}", ch.id),
                _ => match position_of(d.n, d.hero) {
                    Seat::SmallBlind => assert!((net + 0.5).abs() < 1e-9),
                    Seat::BigBlind => assert!((net + 1.0).abs() < 1e-9),
                    _ => assert!(net.abs() < 1e-9),
                },
            }
        }
    }
}

/// Money is conserved: net is at least -100 and at most what the pot could hold.
#[test]
fn nets_are_within_stack_limits() {
    let pool = pool();
    let policy: Vec<Option<[Pure; NCLASSES]>> = vec![None; CHARTS.len()];
    let pol = HeroPolicy::Charts(&policy);
    for (i, ch) in CHARTS.iter().enumerate() {
        for (j, d) in deals(i, 200, &pool).iter().enumerate() {
            for a in 0..3 {
                if ch.labels[a].is_some() {
                    let net = play(&pool, d, Some(PURES[a]), &pol, j as u64);
                    assert!(net >= -100.0 - 1e-9, "{net}");
                    assert!(net <= 100.0 * (d.n as f64 - 1.0) + 1.5 + 1e-9, "{net}");
                }
            }
        }
    }
}

/// `preflop_probs` is the same arithmetic as `preflop_row`.
#[test]
fn preflop_probs_match_preflop_row() {
    let mut rng = Rng::new(11);
    for _ in 0..50 {
        let m = PlayerModel::sample(&mut rng);
        for raises in 0..=3u8 {
            for &(position, free_check) in &[(Seat::Early, false), (Seat::Button, false), (Seat::SmallBlind, false), (Seat::BigBlind, true), (Seat::BigBlind, false)] {
                for limpers in [0u8, 2] {
                    let ctx = PreflopCtx { position, raises, limpers, last_raise_to: 6.0, raised_earlier: raises % 2 == 0, free_check, limped: false };
                    let row = m.preflop_row(&ctx);
                    for c in 0..NCLASSES {
                        assert_eq!(row[c], m.preflop_probs(&ctx, c));
                    }
                }
            }
        }
    }
}

/// Facing a raise, the sampled seat has been facing it; used for the ctx path.
#[test]
fn track_context_consistent() {
    let mut r = Round::preflop(6, STACK);
    let mut t = PreflopTrack::default();
    t.note(&r, 2, Act::Fold);
    r.apply(2, Act::Fold);
    assert_eq!(r.next, 3);
    assert_eq!(t.limpers, 0);
}

#[test]
fn run_job_is_deterministic_and_thread_independent() {
    let a = Pool::build(77, 512, 128, 1);
    let b = Pool::build(77, 512, 128, 4);
    assert_eq!(a.masks, b.masks);
    assert_eq!(a.class, b.class);
    assert_eq!(a.rank, b.rank);
    let policy: Vec<Option<[Pure; NCLASSES]>> = vec![None; CHARTS.len()];
    for ci in [OPEN_E, VS_EM_BB, LIMP2_BB, VS3BET_OOP] {
        let ch = &CHARTS[ci];
        let seats = SeatTable::for_mask(ch.hero);
        let p = HeroPolicy::Charts(&policy);
        let (s1, a1) = run_job(ch, &seats, 40, 5, 300, &a, &p);
        let (s2, a2) = run_job(ch, &seats, 40, 5, 300, &b, &p);
        assert_eq!(a1, a2);
        for i in 0..3 {
            assert_eq!(s1[i].to_bits(), s2[i].to_bits());
        }
    }
}

#[test]
fn derive_small_is_deterministic() {
    let cfg = Config { seed: 3, threads: 3, trials_per_batch: 120, min_batches: 2, max_batches: 3, margin: 0.05, nboards: 256, nmodels: 64, cal: None };
    let a = derive(&cfg);
    let mut cfg2 = cfg.clone();
    cfg2.threads = 2;
    let b = derive(&cfg2);
    assert_eq!(a.pure, b.pure);
    assert_eq!(a.raw, b.raw);
}

#[test]
fn monotone_produces_no_violations_and_keeps_legal_actions() {
    let pool = pool();
    let ch = &CHARTS[OPEN_BTN];
    let avail = allowed_actions(ch);
    let seats = SeatTable::for_mask(ch.hero);
    let policy: Vec<Option<[Pure; NCLASSES]>> = vec![None; CHARTS.len()];
    let pol = HeroPolicy::Charts(&policy);
    let mut cells = Vec::new();
    for h in 0..NCLASSES {
        let (m, _) = run_job(ch, &seats, h, 21, 200, &pool, &pol);
        let mut c = Cell { per_batch: vec![m, m], attempts: 1, trials: 1, stopped: true };
        c.per_batch[1] = m;
        cells.push(c);
    }
    let mut raw = [Pure::Fold; NCLASSES];
    for h in 0..NCLASSES {
        raw[h] = choose(&cells[h].means(), avail, 0.05);
    }
    let (out, _) = monotone(&raw, &cells, avail);
    for ch_list in chains() {
        for w in ch_list.windows(2) {
            assert!(out[w[1]] >= out[w[0]], "not monotone");
        }
    }
    assert!(out.iter().all(|p| avail[*p as usize]));
}

#[test]
fn one_notch_only_moves_down_and_stays_legal() {
    let mut p = [Pure::Fold; NCLASSES];
    for c in 0..NCLASSES {
        p[c] = if c % 3 == 0 { Pure::Raise } else { Pure::Passive };
    }
    let avail = allowed_actions(&CHARTS[VS_EM_IP]);
    let t = one_notch_tighter(&p, avail);
    for c in 0..NCLASSES {
        assert!(t[c] <= p[c]);
    }
    let bb_avail = allowed_actions(&CHARTS[LIMP1_BB]);
    let t = one_notch_tighter(&p, bb_avail);
    assert!(t.iter().all(|x| *x != Pure::Fold));
}

#[test]
fn notation_round_trips() {
    let mut rng = Rng::new(9);
    for _ in 0..200 {
        let mut s = [false; NCLASSES];
        for c in s.iter_mut() {
            *c = rng.below(3) == 0;
        }
        let text = notation(&s);
        assert_eq!(parse_notation(&text).unwrap(), s, "{text}");
    }
    let mut s = [false; NCLASSES];
    for c in ["AA", "KK", "QQ", "JJ", "AKs", "AQs", "AJs", "ATs", "KQs", "AKo", "AQo"] {
        let i = (0..NCLASSES).find(|&x| class_label(x) == c).unwrap();
        s[i] = true;
    }
    assert_eq!(notation(&s), "JJ+ ATs+ KQs AQo+");
    assert_eq!(parse_notation("66-99").unwrap().iter().filter(|x| **x).count(), 4);
    assert_eq!(parse_notation("A2s-A5s").unwrap().iter().filter(|x| **x).count(), 4);
}

#[test]
fn choose_prefers_simple_within_margin() {
    let avail = [true, true, true];
    assert_eq!(choose(&[0.0, 0.02, 0.04], avail, 0.05), Pure::Fold);
    assert_eq!(choose(&[0.0, 0.10, 0.12], avail, 0.05), Pure::Passive);
    assert_eq!(choose(&[0.0, 0.10, 0.20], avail, 0.05), Pure::Raise);
    assert_eq!(choose(&[0.0, 0.5, 0.54], [false, true, true], 0.05), Pure::Passive);
}

#[test]
fn deep_rule_known_hands() {
    let idx = |n: &str| (0..NCLASSES).find(|&x| class_label(x) == n).unwrap();
    assert_eq!(deep_rule(idx("AA")), Pure::Raise);
    assert_eq!(deep_rule(idx("KK")), Pure::Raise);
    assert_eq!(deep_rule(idx("QQ")), Pure::Passive);
    assert_eq!(deep_rule(idx("AKs")), Pure::Passive);
    assert_eq!(deep_rule(idx("AKo")), Pure::Passive);
    assert_eq!(deep_rule(idx("JJ")), Pure::Fold);
    assert_eq!(deep_rule(idx("72o")), Pure::Fold);
}

#[test]
fn chart_index_maps_key_nodes() {
    use poker_engine::preflop_charts::{chart_index, PfState};
    // 6-handed: seat 2 opens to 3, folds to the BB (seat 1): vs early/middle BB.
    let mut r = Round::preflop(6, STACK);
    let mut t = PreflopTrack::default();
    t.note(&r, 2, Act::Raise(3.0));
    r.apply(2, Act::Raise(3.0));
    for s in [3usize, 4, 5, 0] {
        t.note(&r, s, Act::Fold);
        r.apply(s, Act::Fold);
    }
    let pf = PfState { raiser: 2, callers: 0, hero_raised: false, hero_limped: false };
    assert_eq!(chart_index(&r, &t, &pf, 6, 1), Some(VS_EM_BB));
    let pf2 = PfState { raiser: 2, callers: 1, hero_raised: false, hero_limped: false };
    assert_eq!(chart_index(&r, &t, &pf2, 6, 1), Some(MW_EM_BB));

    // Hero (seat 4, 6-handed) opens, BB three-bets: in position.
    let mut r = Round::preflop(6, STACK);
    let mut t = PreflopTrack::default();
    for s in [2usize, 3] {
        t.note(&r, s, Act::Fold);
        r.apply(s, Act::Fold);
    }
    t.note(&r, 4, Act::Raise(3.0));
    r.apply(4, Act::Raise(3.0));
    for s in [5usize, 0] {
        t.note(&r, s, Act::Fold);
        r.apply(s, Act::Fold);
    }
    t.note(&r, 1, Act::Raise(9.0));
    r.apply(1, Act::Raise(9.0));
    let pf = PfState { raiser: 1, callers: 0, hero_raised: true, hero_limped: false };
    assert_eq!(chart_index(&r, &t, &pf, 6, 4), Some(VS3BET_IP));
    // Small blind opens, BB three-bets: out of position.
    let mut r = Round::preflop(4, STACK);
    let mut t = PreflopTrack::default();
    for s in [2usize, 3] {
        t.note(&r, s, Act::Fold);
        r.apply(s, Act::Fold);
    }
    t.note(&r, 0, Act::Raise(3.0));
    r.apply(0, Act::Raise(3.0));
    t.note(&r, 1, Act::Raise(9.0));
    r.apply(1, Act::Raise(9.0));
    let pf = PfState { raiser: 1, callers: 0, hero_raised: true, hero_limped: false };
    assert_eq!(chart_index(&r, &t, &pf, 4, 0), Some(VS3BET_OOP));
}

#[test]
fn calibration_does_not_touch_the_population_without_a_config() {
    let mut rng = Rng::new(4);
    for _ in 0..50 {
        let m = PlayerModel::sample(&mut rng);
        let ctx = PreflopCtx { position: Seat::BigBlind, raises: 1, limpers: 0, last_raise_to: 3.0, raised_earlier: false, free_check: false, limped: false };
        for c in 0..NCLASSES {
            assert_eq!(villain_probs(&m, &ctx, c, None, false), m.preflop_probs(&ctx, c));
        }
    }
}
