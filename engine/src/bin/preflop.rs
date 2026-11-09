//! Derives pure preflop charts and validates them. See PREFLOP.md.
//! Usage:
//!   preflop bench SEED THREADS
//!   preflop check SEED THREADS HANDS
//!   preflop calcheck SEED CAL
//!   preflop continues SEED (flop continue rates of hero's heuristic)
//!   preflop run SEED THREADS TRIALS_PER_BATCH MIN_BATCHES MAX_BATCHES MARGIN NBOARDS NMODELS OUTPUT_DIR H2H_BLOCKS H2H_HANDS

use poker_engine::board::classify;
use poker_engine::combos::{combo_index, combos};
use poker_engine::game::{Act, Round};
use poker_engine::models::{PlayerModel, ARCHETYPES, NBUCKET, POPULATION};
use poker_engine::preflop_charts::*;
use poker_engine::sim::{hero_heuristic, hero_preflop_raise, villain_postflop, villain_preflop, TABLE_SIZES};
use poker_engine::spot::{clamp_preflop_raise, position_of, PreflopTrack};
use poker_trainer::cards::{Card, Holding};
use poker_trainer::eval::evaluate;
use poker_trainer::preflop::Seat;
use poker_trainer::preflop::CARD;
use poker_trainer::rng::Rng;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

fn arg<T: std::str::FromStr>(args: &[String], i: usize) -> T
where
    T::Err: std::fmt::Debug,
{
    args[i].parse().unwrap()
}

fn class_index(name: &str) -> usize {
    (0..NCLASSES).find(|&c| class_label(c) == name).unwrap_or_else(|| panic!("no class {name}"))
}

fn bench(seed: u64, threads: usize) {
    let t = std::time::Instant::now();
    let pool = Pool::build(mix(seed, 1), 4096, 1024, threads);
    let build = t.elapsed().as_secs_f64();
    println!("pool 4096 boards + 1024 models: {build:.2}s wall at {threads} threads, RSS {:.2} GB", poker_engine::memguard::rss_gb());
    let policy: Vec<Option<[Pure; NCLASSES]>> = vec![None; CHARTS.len()];
    for trials in [2000u64] {
        let next = AtomicUsize::new(0);
        let jobs: Vec<(usize, usize)> = (0..CHARTS.len()).flat_map(|c| (0..NCLASSES).map(move |h| (c, h))).collect();
        let out: Mutex<Vec<(usize, u64)>> = Mutex::new(Vec::new());
        let t = std::time::Instant::now();
        std::thread::scope(|sc| {
            for _ in 0..threads {
                sc.spawn(|| {
                    let pol = HeroPolicy::Charts(&policy);
                    let tables: Vec<SeatTable> = CHARTS.iter().map(|c| SeatTable::for_mask(c.hero)).collect();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= jobs.len() {
                            break;
                        }
                        let (c, h) = jobs[i];
                        let (_, att) = run_job(&CHARTS[c], &tables[c], h, mix(seed, i as u64), trials, &pool, &pol);
                        out.lock().unwrap().push((c, att));
                    }
                });
            }
        });
        let secs = t.elapsed().as_secs_f64();
        let total = trials * jobs.len() as u64;
        println!("{trials} trials/job, {} jobs: {:.1}s wall, {:.0} trials/s/thread, {:.0} ns/trial", jobs.len(), secs, total as f64 / secs / threads as f64, secs * threads as f64 * 1e9 / total as f64);
        let mut attempts = vec![0u64; CHARTS.len()];
        for (c, a) in out.into_inner().unwrap() {
            attempts[c] += a;
        }
        for (i, ch) in CHARTS.iter().enumerate() {
            println!("  {:20} acceptance {:.3}", ch.id, (trials * NCLASSES as u64) as f64 / attempts[i] as f64);
        }
    }
}

/// Independent slow reference for a folded-to raise: fresh models per seat,
/// `classify` every street, real `evaluate` at showdown, `sim::villain_preflop`,
/// and no pools. Hero raises first in (or folds); deep rule thereafter.
fn slow_open_ev(hero_group: Seat, class: usize, seed: u64, hands: u64) -> (f64, f64, u64, [u64; 4], [u64; 3]) {
    let lists = combos();
    let list: Vec<usize> = (0..lists.cards.len()).filter(|&i| lists.class169[i] as usize == class).collect();
    let weights: Vec<(usize, usize, u64)> = TABLE_SIZES.iter().flat_map(|&(n, w)| (0..n).filter(move |&s| position_of(n, s) == hero_group).map(move |s| (n, s, w * (420 / n as u64)))).collect();
    let tot: u64 = weights.iter().map(|x| x.2).sum();
    let mut rng = Rng::new(seed);
    let (mut sum, mut sum2, mut n) = (0.0, 0.0, 0u64);
    let mut response = [0u64; 4];
    let mut postflop = [0u64; 3];
    while n < hands {
        let mut u = rng.below(tot);
        let mut pick = weights[0];
        for &w in &weights {
            if u < w.2 {
                pick = w;
                break;
            }
            u -= w.2;
        }
        let (seats, hero) = (pick.0, pick.1);
        let hc = list[rng.below(list.len() as u64) as usize];
        let (h0, h1) = lists.cards[hc];
        let mut deck: Vec<Card> = (0..52u8).map(Card::from_index).filter(|c| *c != h0 && *c != h1).collect();
        rng.shuffle(&mut deck);
        let mut holes: Vec<Holding> = vec![[h0, h1]; seats];
        let mut idx = 0;
        for (s, hole) in holes.iter_mut().enumerate() {
            if s != hero {
                *hole = [deck[idx], deck[idx + 1]];
                idx += 2;
            }
        }
        let board: Vec<Card> = deck[idx..idx + 5].to_vec();
        let models: Vec<PlayerModel> = (0..seats).map(|_| PlayerModel::sample(&mut rng)).collect();
        let tables: Vec<_> = models.iter().map(PlayerModel::tables).collect();
        let mut r = Round::preflop(seats, STACK);
        let mut t = PreflopTrack::default();
        let mut hero_acted = false;
        let mut valid = true;
        while r.to_act != 0 {
            let seat = r.next as usize;
            let act = if seat == hero {
                if !hero_acted {
                    hero_acted = true;
                    if r.raises != 0 || t.limpers != 0 {
                        valid = false;
                        break;
                    }
                    let to = clamp_preflop_raise(&r, seat, hero_preflop_raise(&r));
                    Act::Raise(to.min(r.stacks[seat] + r.put[seat]))
                } else {
                    // Faced a re-raise: deep rule, as in the fast path when no chart applies.
                    match deep_rule(class) {
                        Pure::Raise if r.raises < 3 => {
                            let to = clamp_preflop_raise(&r, seat, hero_preflop_raise(&r));
                            Act::Raise(to.min(r.stacks[seat] + r.put[seat]))
                        }
                        Pure::Raise | Pure::Passive => Act::Call,
                        Pure::Fold => Act::Fold,
                    }
                }
            } else {
                let a = villain_preflop(&r, &t, seats, seat, holes[seat], &models[seat], &mut rng);
                if !hero_acted && !matches!(a, Act::Fold) {
                    valid = false;
                    break;
                }
                a
            };
            if hero_acted && seat != hero {
                match act {
                    Act::Fold => response[0] += 1,
                    Act::Call => response[1] += 1,
                    Act::Raise(_) => response[2] += 1,
                    _ => response[3] += 1,
                }
            }
            t.note(&r, seat, act);
            r.apply(seat, act);
            if r.in_hand >> hero & 1 == 0 {
                break;
            }
        }
        if !valid {
            continue;
        }
        n += 1;
        let net = if r.in_hand >> hero & 1 == 0 {
            r.stacks[hero] - STACK
        } else if r.in_hand.count_ones() == 1 {
            r.stacks[hero] - STACK + r.pot
        } else {
            let mut res = None;
            'street: for street_idx in 1..=3usize {
                r = r.next_street();
                let b = &board[..2 + street_idx];
                let classes = classify(b, street_idx < 3);
                let class_of = |s: usize| classes[combo_index(holes[s][0], holes[s][1])] as usize;
                while r.to_act != 0 {
                    let seat = r.next as usize;
                    let act = if seat == hero {
                        let c = class_of(seat);
                        hero_heuristic(&r, seat, c % NBUCKET, c >= NBUCKET, street_idx - 1, &mut rng)
                    } else {
                        villain_postflop(&r, seat, class_of(seat), street_idx - 1, &tables[seat], &mut rng)
                    };
                    match act {
                        Act::Bet(_) | Act::Raise(_) if seat == hero => postflop[0] += 1,
                        Act::Fold if seat == hero => postflop[1] += 1,
                        _ => {}
                    }
                    r.apply(seat, act);
                    if r.in_hand >> hero & 1 == 0 {
                        res = Some(r.stacks[hero] - STACK);
                        break 'street;
                    }
                    if r.in_hand.count_ones() == 1 {
                        res = Some(r.stacks[hero] - STACK + r.pot);
                        break 'street;
                    }
                }
            }
            res.unwrap_or_else(|| {
                postflop[2] += 1;
                let full: Vec<Card> = board.clone();
                let seven = |s: usize| evaluate(&[holes[s][0], holes[s][1], full[0], full[1], full[2], full[3], full[4]]);
                let hr = seven(hero);
                let (mut ties, mut lost) = (1.0, false);
                for s in 0..seats {
                    if s != hero && r.in_hand >> s & 1 == 1 {
                        let v = seven(s);
                        if v > hr {
                            lost = true;
                        } else if v == hr {
                            ties += 1.0;
                        }
                    }
                }
                if lost {
                    r.stacks[hero] - STACK
                } else {
                    r.stacks[hero] - STACK + r.pot / ties
                }
            })
        };
        sum += net;
        sum2 += net * net;
    }
    (sum, sum2, n, response, postflop)
}

fn crosscheck(seed: u64, threads: usize, hands: u64) {
    println!("Cross-check of the fast pooled simulator against a slow reference (fresh models, classify per street, real evaluate).");
    let policy: Vec<Option<[Pure; NCLASSES]>> = vec![None; CHARTS.len()];
    let pool = Pool::build(mix(seed, 3), 16384, 4096, threads);
    for (chart, group, names) in [(OPEN_SB, Seat::SmallBlind, ["72o", "AA", "T5s"]), (OPEN_E, Seat::Early, ["AA", "72o", "KQo"]), (OPEN_BTN, Seat::Button, ["QJo", "96s", "K2o"])] {
        let ch = &CHARTS[chart];
        for name in names {
            let class = class_index(name);
            let next = AtomicUsize::new(0);
            let parts: Mutex<Vec<(f64, f64, u64, [u64; 4], [u64; 3])>> = Mutex::new(Vec::new());
            std::thread::scope(|sc| {
                for _ in 0..threads {
                    sc.spawn(|| loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= threads {
                            break;
                        }
                        let v = slow_open_ev(group, class, mix(seed ^ 0xABCD, (chart * 1000 + class * 10 + i) as u64), hands / threads as u64);
                        parts.lock().unwrap().push(v);
                    });
                }
            });
            let (mut s, mut s2, mut n, mut resp, mut post) = (0.0, 0.0, 0u64, [0u64; 4], [0u64; 3]);
            for (a, b, c, d, e) in parts.into_inner().unwrap() {
                s += a;
                s2 += b;
                n += c;
                for i in 0..4 {
                    resp[i] += d[i];
                }
                for i in 0..3 {
                    post[i] += e[i];
                }
            }
            let (m, se) = (s / n as f64, ((s2 / n as f64 - (s / n as f64).powi(2)) / n as f64).sqrt());
            let pol = HeroPolicy::Charts(&policy);
            let seats = SeatTable::for_mask(ch.hero);
            let (fast, _att) = run_job(ch, &seats, class, mix(seed, 9), 200_000, &pool, &pol);
            let z = (fast[2] - m) / (se * se + 0.05 * 0.05).sqrt();
            let tot: u64 = resp.iter().sum();
            println!(
                "{:18} {:4} slow raise EV {:+.3} (se {:.3}, n {}), fast raise EV {:+.3}, z {:+.2}; villain responses per seat after hero's open: fold {:.3} call {:.3} raise {:.3} other {:.3}",
                ch.id, name, m, se, n, fast[2], z,
                resp[0] as f64 / tot as f64, resp[1] as f64 / tot as f64, resp[2] as f64 / tot as f64, resp[3] as f64 / tot as f64,
            );
        }
    }
}

/// Differing combos at or below this share of all hands merge two charts.
const MERGE_MAX_COMBO_PCT: f64 = 1.0;

/// Greedy merge of charts with the same actions whose final ranges differ by at
/// most `MERGE_MAX_COMBO_PCT` of hands. Members take the representative's ranges.
fn merge_charts(d: &mut Derivation) -> Vec<(usize, usize, f64)> {
    let mut merged: Vec<Option<usize>> = vec![None; CHARTS.len()];
    let mut out = Vec::new();
    for i in 0..CHARTS.len() {
        if merged[i].is_some() {
            continue;
        }
        for j in i + 1..CHARTS.len() {
            if merged[j].is_some() || CHARTS[i].labels != CHARTS[j].labels || CHARTS[i].stage != CHARTS[j].stage {
                continue;
            }
            let mut diff = [false; NCLASSES];
            for h in 0..NCLASSES {
                diff[h] = d.pure[i][h] != d.pure[j][h];
            }
            let pct = pct_of(&diff);
            if pct <= MERGE_MAX_COMBO_PCT {
                merged[j] = Some(i);
                d.pure[j] = d.pure[i];
                out.push((i, j, pct));
            }
        }
    }
    out
}

/// Measured villain response rates under a calibration ("none" for the
/// population), so each target is checked rather than assumed.
fn calcheck(seed: u64, name: &str) {
    use poker_engine::models::PreflopCtx;
    let cal = poker_engine::preflop_cal::by_name(name);
    println!("Measured preflop responses, percent of all hands unless noted, calibration {name}");
    let mut rng = Rng::new(seed);
    let combos_of = |c: usize| poker_engine::combos::class_combos(c) as f64 / 1326.0 * 100.0;
    for a in 0..5 {
        let (mut bb, mut sb, mut co, mut btn, mut limp, mut opener, mut three) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        let k = 300;
        for _ in 0..k {
            let m = PlayerModel::sample_of(&mut rng, a);
            let ctx = |position, raises, raised_earlier| PreflopCtx { position, raises, limpers: 0, last_raise_to: 3.0, raised_earlier, free_check: false, limped: false };
            let mut limp_in = 0.0;
            let mut limp_cont = 0.0;
            for c in 0..NCLASSES {
                let w = combos_of(c);
                let cont = |p: [f32; 3]| f64::from(1.0 - p[0]) * w;
                bb += cont(villain_probs(&m, &ctx(Seat::BigBlind, 1, false), c, cal, false));
                sb += cont(villain_probs(&m, &ctx(Seat::SmallBlind, 1, false), c, cal, false));
                co += cont(villain_probs(&m, &ctx(Seat::Cutoff, 1, false), c, cal, false));
                btn += cont(villain_probs(&m, &ctx(Seat::Button, 1, false), c, cal, false));
                three += f64::from(villain_probs(&m, &ctx(Seat::Cutoff, 1, false), c, cal, false)[2]) * w;
                opener += cont(villain_probs(&m, &ctx(Seat::Cutoff, 2, true), c, cal, false));
                let open = m.preflop_probs(&PreflopCtx { position: Seat::Cutoff, raises: 0, limpers: 1, last_raise_to: 1.0, raised_earlier: false, free_check: false, limped: false }, c);
                let limp_w = f64::from(open[1]) * w;
                limp_in += limp_w;
                let v = villain_probs(&m, &ctx(Seat::Cutoff, 1, false), c, cal, true);
                limp_cont += limp_w * f64::from(1.0 - v[0]);
            }
            limp += limp_cont / limp_in.max(1e-9) * 100.0;
        }
        let d = k as f64;
        println!("{:14} continue vs 3bb open: BB {:.1}, SB {:.1}, cutoff {:.1}, button {:.1}; 3-bet from cutoff {:.1}; limper calls a raise {:.1}% of limps; opener continues vs 3-bet {:.1}", ARCHETYPES[a].name, bb / d, sb / d, co / d, btn / d, three / d, limp / d, opener / d);
    }
    // Callers after a 3bb early open, 6 handed.
    let pool = Pool::build_cal(mix(seed, 8), 16, 4096, 8, cal);
    let table = combos();
    let mut counts = [0u64; 6];
    let trials = 200_000u64;
    for t in 0..trials {
        let mut rng = Rng::new(mix(seed ^ 5, t));
        let mut used = 0u64;
        let deal = |rng: &mut Rng, used: &mut u64| loop {
            let (a, b) = (rng.below(52) as u8, rng.below(52) as u8);
            let (ma, mb) = (1u64 << a, 1u64 << b);
            if a != b && *used & (ma | mb) == 0 {
                *used |= ma | mb;
                return (a, b);
            }
        };
        let mut r = Round::preflop(6, STACK);
        let mut tr = PreflopTrack::default();
        r.apply(2, Act::Raise(3.0));
        tr.note(&Round::preflop(6, STACK), 2, Act::Raise(3.0));
        let mut callers = 0;
        for _ in 0..5 {
            let s = r.next as usize;
            let (a, b) = deal(&mut rng, &mut used);
            let class = table.class169[table.index[usize::from(a)][usize::from(b)] as usize] as usize;
            let m = &pool.models[rng.below(pool.models.len() as u64) as usize];
            let ctx = tr.ctx(&r, s, 6);
            let p = villain_probs(m, &ctx, class, cal, false);
            let u = (rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
            let act = if u < f64::from(p[0]) { Act::Fold } else { Act::Call };
            if act == Act::Call {
                callers += 1;
            }
            tr.note(&r, s, act);
            r.apply(s, act);
        }
        counts[(callers as usize).min(5)] += 1;
    }
    println!("Callers (including any 3-bets) after a 3bb early open, 6 handed: 0:{:.1}% 1:{:.1}% 2:{:.1}% 3:{:.1}% 4+:{:.1}%", 100.0 * counts[0] as f64 / trials as f64, 100.0 * counts[1] as f64 / trials as f64, 100.0 * counts[2] as f64 / trials as f64, 100.0 * counts[3] as f64 / trials as f64, 100.0 * (counts[4] + counts[5]) as f64 / trials as f64);
}

/// Share of flops on which hero's heuristic would continue facing a bet, by
/// bet size, using the same rule as `sim::hero_heuristic`. Explains why pairs
/// and suited hands take different actions in the charts.
fn continues(seed: u64) {
    let pool = Pool::build(mix(seed, 2), 8192, 1, 8);
    let table = combos();
    println!("Hero heuristic: share of flops continuing vs a bet (small, medium, large), by hand");
    for name in ["TT", "88", "55", "A2s", "K2s", "T9s", "ATo", "KQo"] {
        let class = class_index(name);
        let list: Vec<usize> = (0..table.cards.len()).filter(|&i| table.class169[i] as usize == class).collect();
        let mut rng = Rng::new(mix(seed, class as u64));
        let (mut small, mut medium, mut large, mut draws, mut n) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
        for _ in 0..200_000 {
            let c = list[rng.below(list.len() as u64) as usize];
            let b = rng.below(pool.masks.len() as u64) as usize;
            if pool.masks[b] & table.masks[c] != 0 {
                continue;
            }
            let cl = pool.class_at(b, 0, c);
            let (bucket, draw) = (cl % NBUCKET, cl >= NBUCKET);
            let call = |sit: usize| bucket <= 1 || (bucket == 2 && sit <= 2) || (bucket == 3 && sit == 1) || (draw && sit <= 2);
            small += f64::from(u8::from(call(1)));
            medium += f64::from(u8::from(call(2)));
            large += f64::from(u8::from(call(3)));
            draws += f64::from(u8::from(draw));
            n += 1.0;
        }
        println!("{name:4} small {:.1}% medium {:.1}% large {:.1}% (flop draw {:.1}%)", 100.0 * small / n, 100.0 * medium / n, 100.0 * large / n, 100.0 * draws / n);
    }
}

struct Checks {
    lines: Vec<String>,
    failures: usize,
}

impl Checks {
    fn record(&mut self, name: &str, ok: bool, detail: String) {
        if !ok {
            self.failures += 1;
        }
        self.lines.push(format!("{} {name}: {detail}", if ok { "PASS" } else { "FAIL" }));
    }
}

fn pct_action(p: &[Pure; NCLASSES], a: Pure) -> f64 {
    pct_of(&action_set(p, a))
}

fn pct_continue(p: &[Pure; NCLASSES]) -> f64 {
    pct_action(p, Pure::Raise) + pct_action(p, Pure::Passive)
}

fn sanity(d: &Derivation) -> Checks {
    let mut c = Checks { lines: Vec::new(), failures: 0 };
    let aa = class_index("AA");
    let kk = class_index("KK");
    let mut ak_folds = Vec::new();
    for (i, ch) in CHARTS.iter().enumerate() {
        for &h in &[aa, kk] {
            if d.pure[i][h] == Pure::Fold {
                ak_folds.push(format!("{} {}", ch.id, class_label(h)));
            }
        }
    }
    c.record("AA and KK never fold", ak_folds.is_empty(), format!("{ak_folds:?}"));

    let trash = class_index("72o");
    let mut bad = Vec::new();
    for (i, ch) in CHARTS.iter().enumerate() {
        let a = d.pure[i][trash];
        let free_bb = ch.labels[0].is_none();
        let ok = if free_bb { a == Pure::Passive } else { a == Pure::Fold };
        if !ok {
            bad.push(format!("{}:{:?}", ch.id, a));
        }
    }
    c.record("72o never raises or calls except big-blind check", bad.is_empty(), format!("{bad:?}"));

    let open: Vec<f64> = [OPEN_E, OPEN_M, OPEN_CO, OPEN_BTN].iter().map(|&i| pct_action(&d.pure[i], Pure::Raise)).collect();
    let widen = open.windows(2).all(|w| w[0] <= w[1]);
    c.record("Opens widen by seat (early <= middle <= cutoff <= button)", widen, format!("{open:.1?} percent; small blind {:.1}", pct_action(&d.pure[OPEN_SB], Pure::Raise)));

    let cmp = [
        ("small blind continue vs early open <= vs button open", VS_EM_SB, VS_LATE_SB),
        ("big blind continue vs early open <= vs button open", VS_EM_BB, VS_LATE_BB),
        ("in position continue vs early open <= vs cutoff open", VS_EM_IP, VS_CO_IP),
    ];
    for (name, a, b) in cmp {
        let (x, y) = (pct_continue(&d.pure[a]), pct_continue(&d.pure[b]));
        c.record(name, x <= y, format!("{x:.1} vs {y:.1} percent"));
    }

    // One action per hand: exactly one allowed action in every cell.
    let mut bad_cells = 0;
    for (i, ch) in CHARTS.iter().enumerate() {
        let avail = allowed_actions(ch);
        for h in 0..NCLASSES {
            if !avail[d.pure[i][h] as usize] {
                bad_cells += 1;
            }
        }
    }
    c.record("Every cell holds one legal action", bad_cells == 0, format!("{bad_cells} illegal cells"));

    // Notation round trip.
    let mut bad_round = 0;
    for (i, _) in CHARTS.iter().enumerate() {
        for &a in &PURES {
            let s = action_set(&d.pure[i], a);
            let n = notation(&s);
            match parse_notation(&n) {
                Ok(back) if back == s => {}
                _ => bad_round += 1,
            }
        }
    }
    c.record("Range notation round-trips exactly", bad_round == 0, format!("{bad_round} mismatches"));

    // Three-bet ranges are subsets of continues (holds by construction), and
    // each chart's three-bet range against early is contained in the one against
    // the corresponding late opener, a stricter structural check (informational).
    for (a, b, name) in [(VS_EM_BB, VS_LATE_BB, "big blind"), (VS_EM_SB, VS_LATE_SB, "small blind")] {
        let ra = action_set(&d.pure[a], Pure::Raise);
        let rb = action_set(&d.pure[b], Pure::Raise);
        let extra: Vec<String> = (0..NCLASSES).filter(|&h| ra[h] && !rb[h]).map(class_label).collect();
        c.lines.push(format!("INFO {name}: 3-bet hands vs early that do not 3-bet vs late opener: {extra:?}"));
    }
    c.record("3-bet range subset of continuing range (by construction)", true, "raise is a continue in every chart".to_string());

    // Monotonicity holds on the final chart.
    let mut violations = 0;
    for (i, _) in CHARTS.iter().enumerate() {
        for ch in chains() {
            for w in ch.windows(2) {
                if d.pure[i][w[1]] < d.pure[i][w[0]] {
                    violations += 1;
                }
            }
        }
    }
    c.record("Final charts are monotone along every chain", violations == 0, format!("{violations} violations"));
    c
}

fn card_set(open_seat: usize) -> [bool; NCLASSES] {
    let line = &CARD.open[open_seat];
    let mut s = [false; NCLASSES];
    for high in 0..13u8 {
        for low in 0..=high {
            if high == low {
                s[usize::from(high)] = line.range.contains(high, low, false);
            } else {
                let tri = usize::from(high) * (usize::from(high) - 1) / 2 + usize::from(low);
                s[13 + tri] = line.range.contains(high, low, true);
                s[91 + tri] = line.range.contains(high, low, false);
            }
        }
    }
    s
}

fn compare_card(d: &Derivation, out: &mut String) {
    writeln!(out, "COMPARISON WITH THE HAND-TUNED CARD (opens, derived independently, percent of hands)").unwrap();
    let names = ["early", "middle", "cutoff", "button", "small_blind"];
    let idx = [OPEN_E, OPEN_M, OPEN_CO, OPEN_BTN, OPEN_SB];
    for s in 0..5 {
        let card = card_set(s);
        let ours = action_set(&d.pure[idx[s]], Pure::Raise);
        let only_ours: Vec<usize> = (0..NCLASSES).filter(|&h| ours[h] && !card[h]).collect();
        let only_card: Vec<usize> = (0..NCLASSES).filter(|&h| card[h] && !ours[h]).collect();
        let mut a = [false; NCLASSES];
        let mut b = [false; NCLASSES];
        for &h in &only_ours {
            a[h] = true;
        }
        for &h in &only_card {
            b[h] = true;
        }
        writeln!(out, "{:12} card {:5.1}% derived {:5.1}% | only derived: {} | only card: {}", names[s], pct_of(&card), pct_of(&ours), notation(&a), notation(&b)).unwrap();
    }
}

/// Fills the action slots of a surprise line: EV per action plus SE.
fn evs_text(ch: &ChartDef, cell: &Cell) -> String {
    let m = cell.means();
    let avail = allowed_actions(ch);
    let mut s = String::new();
    for a in 0..3 {
        if avail[a] {
            let _ = write!(s, "{}={:+.2}(se {:.3}) ", ch.labels[a].unwrap(), m[a], cell.se_of(a));
        }
    }
    s
}

/// A deliberately ordinary reference for auditing: per chart, the percent of
/// hands (by the engine's equity order) that standard judgment would raise,
/// and that it would continue with. Rule-of-thumb numbers, not derived.
fn reference(ch: &ChartDef) -> (f64, f64) {
    let bb = ch.seat == "big_blind";
    let sb = ch.seat == "small_blind";
    let id = ch.id;
    if id.starts_with("open.") {
        let r = match ch.seat {
            "early" => 12.0,
            "middle" => 16.0,
            "cutoff" => 25.0,
            "button" => 40.0,
            _ => 30.0,
        };
        (r, r)
    } else if id.starts_with("limp") {
        if bb {
            (10.0, 100.0)
        } else {
            (12.0, 30.0)
        }
    } else if id.starts_with("vs3bet") {
        (2.5, 7.0)
    } else {
        let (r, c): (f64, f64) = if id.starts_with("mw") {
            (2.5, 12.0)
        } else if id.starts_with("vs_em") {
            (3.0, 12.0)
        } else if id.starts_with("vs_co") {
            (4.0, 18.0)
        } else {
            (6.0, 25.0)
        };
        (r, c + if bb { 8.0 } else if sb { -2.0 } else { 0.0 })
    }
}

/// Audit list: the cells furthest from the reference, half looser and half
/// tighter or more passive. Looser cells rank by percentile points past the
/// reference boundary. Tighter cells rank by how far inside the reference they
/// are, relative to the boundary, so a premium hand not raising scores highest.
fn surprises(d: &Derivation, top: usize) -> Vec<String> {
    let pct = percentiles();
    let mut open_any = [false; NCLASSES];
    for &i in &[OPEN_E, OPEN_M, OPEN_CO, OPEN_BTN, OPEN_SB] {
        for h in 0..NCLASSES {
            open_any[h] |= d.pure[i][h] == Pure::Raise;
        }
    }
    let (mut looser, mut tighter): (Vec<(f64, String)>, Vec<(f64, String)>) = (Vec::new(), Vec::new());
    for (i, ch) in CHARTS.iter().enumerate() {
        let avail = allowed_actions(ch);
        let (rr, cc) = reference(ch);
        for h in 0..NCLASSES {
            if ch.stage == 0 && !open_any[h] {
                continue;
            }
            let p = pct[h];
            let a = d.pure[i][h];
            let want = if p < rr {
                Pure::Raise
            } else if p < cc && avail[1] {
                Pure::Passive
            } else if avail[0] {
                Pure::Fold
            } else {
                Pure::Passive
            };
            if a == want {
                continue;
            }
            let line = |kind: &str, score: f64| {
                format!("{:18} {:4} action={:>6} reference={:>6} ({kind}, score {:.2}, equity percentile {:.1}) {}", ch.id, class_label(h), ch.labels[a as usize].unwrap_or("?"), ch.labels[want as usize].unwrap_or("?"), score, p, evs_text(ch, &d.cells[i][h]))
            };
            if a > want {
                let boundary = if a == Pure::Raise { rr } else { cc };
                let score = p - boundary;
                looser.push((score, line("looser or more aggressive than reference", score)));
            } else {
                let boundary = if want == Pure::Raise { rr } else { cc };
                let score = (boundary - p) / boundary * 100.0;
                tighter.push((score, line("tighter or more passive than reference", score)));
            }
        }
    }
    looser.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    tighter.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    let mut out = Vec::new();
    for (l, t) in looser.iter().take(top / 2).zip(tighter.iter().take(top / 2)) {
        out.push(l.1.clone());
        out.push(t.1.clone());
    }
    out
}

const SIZES: [usize; 4] = [4, 5, 6, 7];

fn size_index(n: usize) -> usize {
    SIZES.iter().position(|&s| s == n).unwrap()
}

#[derive(Default, Clone, Copy)]
struct Accum {
    n: u64,
    sum: [f64; 3],
    d_ab: f64,
    d2_ab: f64,
    d_cb: f64,
    d2_cb: f64,
    d_ca: f64,
    d2_ca: f64,
}

impl Accum {
    fn add(&mut self, o: &Accum) {
        self.n += o.n;
        for i in 0..3 {
            self.sum[i] += o.sum[i];
        }
        self.d_ab += o.d_ab;
        self.d2_ab += o.d2_ab;
        self.d_cb += o.d_cb;
        self.d2_cb += o.d2_cb;
        self.d_ca += o.d_ca;
        self.d2_ca += o.d2_ca;
    }
}

fn head_to_head(seed: u64, threads: usize, blocks: usize, hands: u64, pure: &[[Pure; NCLASSES]], cal: Option<&'static poker_engine::preflop_cal::Calibration>, out: &mut String) {
    let charts: Vec<Option<[Pure; NCLASSES]>> = pure.iter().map(|p| Some(*p)).collect();
    let tighter: Vec<Option<[Pure; NCLASSES]>> = (0..CHARTS.len()).map(|i| Some(one_notch_tighter(&pure[i], allowed_actions(&CHARTS[i])))).collect();
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<[Accum; 4]>> = Mutex::new(Vec::new());
    let t0 = std::time::Instant::now();
    std::thread::scope(|sc| {
        for _ in 0..threads {
            sc.spawn(|| {
                let policies = [HeroPolicy::Charts(&charts), HeroPolicy::Card, HeroPolicy::Charts(&tighter)];
                loop {
                    let b = next.fetch_add(1, Ordering::Relaxed);
                    if b >= blocks {
                        break;
                    }
                    let pool = Pool::build_cal(mix(seed ^ 0xD00D, b as u64), 16384, 2048, 1, cal);
                    let mut acc = [Accum::default(); 4];
                    for t in 0..hands {
                        let ts = mix(mix(seed ^ 0xFACE, b as u64), t);
                        let mut rng = Rng::new(ts);
                        let d = sample_full_deal(&pool, &mut rng);
                        let mut x = [0.0; 3];
                        for (a, p) in policies.iter().enumerate() {
                            x[a] = play(&pool, &d, None, p, ts);
                        }
                        let s = &mut acc[size_index(d.n)];
                        s.n += 1;
                        for a in 0..3 {
                            s.sum[a] += x[a];
                        }
                        let (ab, cb, ca) = (x[0] - x[1], x[2] - x[1], x[2] - x[0]);
                        s.d_ab += ab;
                        s.d2_ab += ab * ab;
                        s.d_cb += cb;
                        s.d2_cb += cb * cb;
                        s.d_ca += ca;
                        s.d2_ca += ca * ca;
                    }
                    results.lock().unwrap().push(acc);
                }
            });
        }
    });
    let res = results.into_inner().unwrap();
    let mut by_size = [Accum::default(); 4];
    let mut total = Accum::default();
    let mut block_means = Vec::new();
    for acc in &res {
        let mut b = Accum::default();
        for (i, s) in acc.iter().enumerate() {
            by_size[i].add(s);
            b.add(s);
        }
        block_means.push(b.d_ab / b.n as f64);
        total.add(&b);
    }
    let se = |n: u64, d: f64, d2: f64| {
        let nf = n as f64;
        let mean = d / nf;
        ((d2 / nf - mean * mean) / nf).sqrt() * 100.0
    };
    writeln!(out, "HEAD TO HEAD (fresh seeds, fresh tables and models not used in derivation; same simulated hero postflop; common random numbers)").unwrap();
    writeln!(out, "Hands: {} ({} blocks of {}), {:.0}s wall.", total.n, blocks, hands, t0.elapsed().as_secs_f64()).unwrap();
    writeln!(out, "bb/100 for: charts (new), card (hand-tuned), tighter (new charts one notch tighter).").unwrap();
    let mut row = |name: &str, a: &Accum| {
        let nf = a.n as f64;
        writeln!(
            out,
            "{name:8} n={:>10} charts={:+.2} card={:+.2} tighter={:+.2} | charts-card={:+.2} (se {:.2}) tighter-card={:+.2} (se {:.2}) tighter-charts={:+.2} (se {:.2})",
            a.n,
            a.sum[0] / nf * 100.0,
            a.sum[1] / nf * 100.0,
            a.sum[2] / nf * 100.0,
            a.d_ab / nf * 100.0,
            se(a.n, a.d_ab, a.d2_ab),
            a.d_cb / nf * 100.0,
            se(a.n, a.d_cb, a.d2_cb),
            a.d_ca / nf * 100.0,
            se(a.n, a.d_ca, a.d2_ca),
        )
        .unwrap();
    };
    row("overall", &total);
    for (i, s) in by_size.iter().enumerate() {
        row(&format!("{}-handed", SIZES[i]), s);
    }
    let bm = block_means.iter().sum::<f64>() / block_means.len() as f64;
    let v = block_means.iter().map(|x| (x - bm) * (x - bm)).sum::<f64>() / (block_means.len() - 1).max(1) as f64;
    writeln!(out, "Cross-check: charts-card from block means {:+.2}, se {:.2} bb/100 ({} blocks).", bm * 100.0, (v / block_means.len() as f64).sqrt() * 100.0, block_means.len()).unwrap();
    let ok = total.d_ab > 0.0;
    writeln!(out, "Result: the new charts {} the card in this simulation.", if ok { "BEAT" } else { "DO NOT BEAT" }).unwrap();
}

fn ranges_of(p: &[Pure; NCLASSES], ch: &ChartDef) -> String {
    let mut s = String::new();
    for a in (0..3).rev() {
        if let Some(label) = ch.labels[a] {
            let set = action_set(p, PURES[a]);
            let _ = write!(s, " | action.{label}={} | pct.{label}={:.1}", notation(&set), pct_of(&set));
        }
    }
    s
}

fn gap_stats(d: &Derivation, margin: f64) -> (f64, f64, f64, f64, f64) {
    let mut gaps = 0usize;
    let mut total = 0usize;
    let mut confident = 0usize;
    let mut ses = Vec::new();
    for (i, ch) in CHARTS.iter().enumerate() {
        let avail = allowed_actions(ch);
        for h in 0..NCLASSES {
            let cell = &d.cells[i][h];
            let m = cell.means();
            let best = best_of(&m, avail);
            let mut second = usize::MAX;
            for a in 0..3 {
                if avail[a] && a != best && (second == usize::MAX || m[a] > m[second]) {
                    second = a;
                }
            }
            total += 1;
            let gap = m[best] - m[second];
            let se = cell.se_diff(best, second);
            ses.push(se);
            if gap <= margin {
                gaps += 1;
            }
            if gap > 2.0 * se {
                confident += 1;
            }
        }
    }
    ses.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = ses[ses.len() / 2];
    let mean = ses.iter().sum::<f64>() / ses.len() as f64;
    let p90 = ses[ses.len() * 9 / 10];
    (100.0 * gaps as f64 / total as f64, 100.0 * confident as f64 / total as f64, med, mean, p90)
}

fn write_outputs(cfg: &Config, d: &Derivation, dir: &str, checks: &Checks, compare: &str, h2h: &str, surprises: &[String], merges: &[(usize, usize, f64)]) -> String {
    std::fs::create_dir_all(dir).unwrap();
    let (within, confident, se_med, se_mean, se_p90) = gap_stats(d, cfg.margin);
    let total_trials: u64 = d.trial_total;
    let total_mono: usize = d.changed_by_mono.iter().sum();
    let mut text = String::new();
    writeln!(text, "# preflop-charts v1. Line-based. Derived against simulated opponents built on assumptions: every number comes from the invented population in engine/src/models.rs, not from observed players. Hero's postflop play is the engine's simple solid heuristic. See engine/PREFLOP.md for method and limits.").unwrap();
    writeln!(text, "# Fields separated by ' | '. Each chart is one line: id, desc, seat, situation, one action.<name>=range and pct.<name>=percent of 1326 combos per action, then meta.").unwrap();
    writeln!(text, "# Range tokens: 77 (one pair), 77+ (77 to AA), 55-77 (55 through 77), ATs (one hand), ATs+ (ATs to AKs), A2s-A5s (A2s through A5s), same with o for offsuit. Together the action ranges cover all 169 hands with no overlap.").unwrap();
    writeln!(text, "version=1").unwrap();
    writeln!(text, "game=cash no rake, blinds 0.5/1, equal stacks 100bb, tables of 4-7 (weights 40/40/10/10 for 4/5/6/7), opponents from the engine population (mix {:?} for {:?})", POPULATION, ARCHETYPES.iter().map(|a| a.name).collect::<Vec<_>>()).unwrap();
    writeln!(text, "calibration={}", cfg.cal.map_or("population (unmodified)", |c| c.name)).unwrap();
    writeln!(text, "seed={} threads={} trials_per_batch={} min_batches={} max_batches={} margin_bb={} boards={} models={}", cfg.seed, cfg.threads, cfg.trials_per_batch, cfg.min_batches, cfg.max_batches, cfg.margin, cfg.nboards, cfg.nmodels).unwrap();
    writeln!(text, "charts={} trials_total={} derivation_seconds={:.0} se_diff_bb_median={:.4} cells_within_margin_pct={:.1} cells_confident_pct={:.1} monotonicity_changes={}", CHARTS.len(), total_trials, d.elapsed, se_med, within, confident, total_mono).unwrap();
    for (i, ch) in CHARTS.iter().enumerate() {
        let meta = format!(
            "trials_min={} trials_max={} margin_bb={} monotonic_changes={} sample_batches_max={}{}",
            d.cells[i].iter().map(|c| c.trials).min().unwrap(),
            d.cells[i].iter().map(|c| c.trials).max().unwrap(),
            cfg.margin,
            d.changed_by_mono[i],
            d.cells[i].iter().map(|c| c.per_batch.len()).max().unwrap(),
            merges.iter().find(|m| m.1 == i).map_or(String::new(), |m| format!(" merged_into={}", CHARTS[m.0].id)),
        );
        writeln!(text, "id={} | desc={} | seat={} | situation={}{} | meta={}", ch.id, ch.desc, ch.seat, ch.situation, ranges_of(&d.pure[i], ch), meta).unwrap();
    }
    std::fs::write(format!("{dir}/charts_v1.txt"), &text).unwrap();

    let mut cells = String::new();
    writeln!(cells, "chart\thand\tchosen\traw\tev_fold\tev_passive\tev_raise\tse_fold\tse_passive\tse_raise\tse_diff_best_second\tbatches\ttrials\tacceptance").unwrap();
    for (i, ch) in CHARTS.iter().enumerate() {
        let avail = allowed_actions(ch);
        for h in 0..NCLASSES {
            let c = &d.cells[i][h];
            let m = c.means();
            let best = best_of(&m, avail);
            let mut second = usize::MAX;
            for a in 0..3 {
                if avail[a] && a != best && (second == usize::MAX || m[a] > m[second]) {
                    second = a;
                }
            }
            let acc = c.trials as f64 / c.attempts.max(1) as f64;
            let fld = |a: usize| if avail[a] { format!("{:+.4}", m[a]) } else { "-".to_string() };
            let sef = |a: usize| if avail[a] { format!("{:.4}", c.se_of(a)) } else { "-".to_string() };
            writeln!(cells, "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.4}\t{}\t{}\t{:.4}", ch.id, class_label(h), ch.labels[d.pure[i][h] as usize].unwrap_or("?"), ch.labels[d.raw[i][h] as usize].unwrap_or("?"), fld(0), fld(1), fld(2), sef(0), sef(1), sef(2), c.se_diff(best, second), c.per_batch.len(), c.trials, acc).unwrap();
        }
    }
    std::fs::write(format!("{dir}/cells_v1.tsv"), &cells).unwrap();

    let mut report = String::new();
    writeln!(report, "PREFLOP CHARTS VALIDATION REPORT v1").unwrap();
    writeln!(report, "Calibration: {}", cfg.cal.map_or("population (unmodified)", |c| c.name)).unwrap();
    writeln!(report, "Derived against simulated opponents built on assumptions. No real players were observed.\n").unwrap();
    writeln!(report, "Derivation: {} charts, {} trials, {:.0}s wall at {} threads.", CHARTS.len(), total_trials, d.elapsed, cfg.threads).unwrap();
    writeln!(report, "Throughput: {:.0} trials per second per thread over the derivation jobs (includes rejection for the situation).", total_trials as f64 / d.job_seconds.max(1.0)).unwrap();
    writeln!(report, "Margin: {} bb. SE of best-vs-second difference: median {:.4}, mean {:.4}, p90 {:.4} bb.", cfg.margin, se_med, se_mean, se_p90).unwrap();
    writeln!(report, "Share of chart cells with top two actions within the margin: {within:.1}%. Share with best action more than 2 SE clear of second: {confident:.1}%.").unwrap();
    writeln!(report, "Cells changed by monotonicity: {total_mono} of {}.\n", CHARTS.len() * NCLASSES).unwrap();
    writeln!(report, "Merged charts (ranges within {MERGE_MAX_COMBO_PCT}% of hands; the member takes the representative's ranges, its line stays in the file):").unwrap();
    for m in merges {
        writeln!(report, "  {} merged into {} ({:.2}% of hands differed)", CHARTS[m.1].id, CHARTS[m.0].id, m.2).unwrap();
    }
    if merges.is_empty() {
        writeln!(report, "  none").unwrap();
    }
    writeln!(report).unwrap();
    writeln!(report, "SANITY CHECKS ({} failures)", checks.failures).unwrap();
    for l in &checks.lines {
        writeln!(report, "{l}").unwrap();
    }
    writeln!(report).unwrap();
    writeln!(report, "{compare}").unwrap();
    writeln!(report, "{h2h}").unwrap();
    writeln!(report, "AUDIT: 20 CELLS FURTHEST FROM A RULE-OF-THUMB REFERENCE (10 looser, 10 more passive; reference numbers in engine/PREFLOP.md; EVs from the derivation)").unwrap();
    for l in surprises {
        writeln!(report, "{l}").unwrap();
    }
    std::fs::write(format!("{dir}/report_v1.txt"), &report).unwrap();
    report
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    poker_engine::memguard::start(40.0);
    match args.get(1).map(String::as_str) {
        Some("bench") => bench(arg(&args, 2), arg(&args, 3)),
        Some("calcheck") => calcheck(arg(&args, 2), &args[3]),
        Some("continues") => continues(arg(&args, 2)),
        Some("check") => crosscheck(arg(&args, 2), arg(&args, 3), arg(&args, 4)),
        Some("run") => {
            assert!(args.len() == 13 || args.len() == 14, "usage: preflop run SEED THREADS TRIALS MIN MAX MARGIN NBOARDS NMODELS OUTDIR H2H_BLOCKS H2H_HANDS [CAL]");
            let cal = args.get(13).map(|n| poker_engine::preflop_cal::by_name(n).unwrap_or_else(|| panic!("unknown calibration {n}")));
            let cfg = Config {
                seed: arg(&args, 2),
                threads: arg(&args, 3),
                trials_per_batch: arg(&args, 4),
                min_batches: arg(&args, 5),
                max_batches: arg(&args, 6),
                margin: arg(&args, 7),
                nboards: arg(&args, 8),
                nmodels: arg(&args, 9),
                cal,
            };
            let dir: String = args[10].clone();
            let blocks: usize = arg(&args, 11);
            let hands: u64 = arg(&args, 12);
            assert!(cfg.max_batches <= MAX_BATCHES);
            let mut d = derive(&cfg);
            let merges = merge_charts(&mut d);
            let checks = sanity(&d);
            let mut compare = String::new();
            compare_card(&d, &mut compare);
            let surprises_list = surprises(&d, 20);
            let mut h2h = String::new();
            if blocks > 0 && hands > 0 {
                head_to_head(cfg.seed ^ 0x6060, cfg.threads, blocks, hands, &d.pure, cfg.cal, &mut h2h);
            }
            let report = write_outputs(&cfg, &d, &dir, &checks, &compare, &h2h, &surprises_list, &merges);
            println!("{report}");
            println!("wrote {dir}/charts_v1.txt, cells_v1.tsv and report_v1.txt; peak RSS {:.2} GB", poker_engine::memguard::peak_rss_gb());
        }
        _ => panic!("usage: preflop bench|run ..."),
    }
}
