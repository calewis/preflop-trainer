//! The "What beats you?" ladder: four rungs that build up to a quick read of how
//! far ahead or behind a hand is. Generation, truth and the ladder's rules.

use crate::cards::{holding_mask, mask_of, rank_char, unseen_holdings, Card, Holding};
use crate::classes::{combos_in_class, mix_combos, rank_of, OppMix, PlayerType};
use crate::eval::{Category, HandRank};
use crate::generate::{deal_board, deal_holding, MIN_CLASS_COMBOS};
use crate::item::{beats_me_rung_of_seed, BeatsMeAsk, BeatsMeQuestion, Skill, BEATS_ME_RUNGS};
use crate::grade::nuts_analysis;
use crate::log::{Attempt, ItemRef, Mode};
use crate::rng::Rng;

/// Attempts at a rung considered for promotion.
pub const BEATS_ME_PROMOTE_WINDOW: usize = 20;
/// Promote once [`BEATS_ME_PROMOTE_WINDOW`] straight attempts at a rung reach this.
pub const BEATS_ME_PROMOTE_PCT: f64 = 85.0;
/// Rung 3: above this share of the pot the hand is usually ahead, in percent.
pub const MIX_AHEAD_PCT: f64 = 60.0;
/// Rung 3: below this share of the pot the hand is usually behind, in percent.
pub const MIX_BEHIND_PCT: f64 = 40.0;
/// Rung 0: how often 0, 1, 2 and 3 of the four shown hands are ahead, out of 10.
const WHICH_AHEAD_WEIGHTS: [(usize, u64); 4] = [(0, 1), (1, 3), (2, 3), (3, 3)];
/// Rung 1: one item in this many is a hand that nothing beats.
const KINDS_NUTS_ONE_IN: u64 = 8;

/// One attempt's rung, from its seed. `None` for a frozen baseline row.
fn rung_of(attempt: &Attempt) -> Option<u8> {
    match attempt.item {
        ItemRef::Seed(seed) => Some(beats_me_rung_of_seed(seed)),
        ItemRef::Baseline(_) => None,
    }
}

/// The player's rung: promoted past a rung once any [`BEATS_ME_PROMOTE_WINDOW`]
/// straight drill attempts there were at least [`BEATS_ME_PROMOTE_PCT`] correct.
/// A review of an old item cannot demote the player.
pub fn beats_me_rung(attempts: &[Attempt]) -> u8 {
    let mut rung = 0;
    while rung + 1 < BEATS_ME_RUNGS {
        let results: Vec<bool> = attempts
            .iter()
            .filter(|a| a.skill == Skill::BeatsMe && a.mode == Mode::Drill && rung_of(a) == Some(rung))
            .map(|a| a.correct)
            .collect();
        let promoted = results.windows(BEATS_ME_PROMOTE_WINDOW).any(|window| {
            100.0 * window.iter().filter(|&&right| right).count() as f64 / window.len() as f64 >= BEATS_ME_PROMOTE_PCT
        });
        if !promoted {
            break;
        }
        rung += 1;
    }
    rung
}

/// True for the attempts that count toward the `beats_me` pass mark: rungs 2-3.
pub fn counts_for_beats_me_mark(attempt: &Attempt) -> bool {
    rung_of(attempt).is_some_and(|rung| rung >= 2)
}

/// A shown hand against hero's right now.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Standing {
    Ahead,
    Tied,
    Behind,
}

pub fn standing(hero: Holding, other: Holding, board: &[Card]) -> Standing {
    match rank_of(other, board).cmp(&rank_of(hero, board)) {
        std::cmp::Ordering::Greater => Standing::Ahead,
        std::cmp::Ordering::Equal => Standing::Tied,
        std::cmp::Ordering::Less => Standing::Behind,
    }
}

/// The kinds of hand offered at rung 1.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum BeatKind {
    BiggerPair,
    BetterKicker,
    TwoPair,
    SetOrTrips,
    Straight,
    Flush,
    FullHouseOrBetter,
}

impl BeatKind {
    pub const ALL: [BeatKind; 7] = [
        BeatKind::BiggerPair,
        BeatKind::BetterKicker,
        BeatKind::TwoPair,
        BeatKind::SetOrTrips,
        BeatKind::Straight,
        BeatKind::Flush,
        BeatKind::FullHouseOrBetter,
    ];

    pub fn code(self) -> char {
        match self {
            BeatKind::BiggerPair => 'p',
            BeatKind::BetterKicker => 'k',
            BeatKind::TwoPair => 't',
            BeatKind::SetOrTrips => 's',
            BeatKind::Straight => 'r',
            BeatKind::Flush => 'f',
            BeatKind::FullHouseOrBetter => 'h',
        }
    }

    pub fn from_code(code: char) -> Option<BeatKind> {
        BeatKind::ALL.into_iter().find(|k| k.code() == code)
    }

    pub fn label(self) -> &'static str {
        match self {
            BeatKind::BiggerPair => "a bigger pair",
            BeatKind::BetterKicker => "the same pair with a better kicker",
            BeatKind::TwoPair => "two pair",
            BeatKind::SetOrTrips => "a set or trips",
            BeatKind::Straight => "a straight",
            BeatKind::Flush => "a flush",
            BeatKind::FullHouseOrBetter => "a full house or better",
        }
    }
}

/// The kind of a holding that is ahead of hero. `None` for a better high card,
/// which rung 1 doesn't ask about: it is only dealt to pair-or-better hands.
fn kind_of(rank: HandRank, hero: HandRank) -> Option<BeatKind> {
    match rank.category() {
        Category::HighCard => None,
        Category::OnePair if hero.category() == Category::OnePair && hero.primary() == rank.primary() => {
            Some(BeatKind::BetterKicker)
        }
        Category::OnePair => Some(BeatKind::BiggerPair),
        Category::TwoPair => Some(BeatKind::TwoPair),
        Category::Trips => Some(BeatKind::SetOrTrips),
        Category::Straight => Some(BeatKind::Straight),
        Category::Flush => Some(BeatKind::Flush),
        Category::FullHouse | Category::Quads | Category::StraightFlush => Some(BeatKind::FullHouseOrBetter),
    }
}

/// One kind of hand that beats hero, and how many unseen holdings make it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct KindRow {
    pub kind: BeatKind,
    pub ways: usize,
    pub example: Holding,
    pub best: HandRank,
}

/// The kinds of hand with at least one unseen holding strictly ahead of hero.
pub fn kinds_analysis(hero: Holding, board: &[Card]) -> Vec<KindRow> {
    let hero_rank = rank_of(hero, board);
    let mut rows: Vec<KindRow> = Vec::new();
    for holding in unseen_holdings(holding_mask(&hero) | mask_of(board)) {
        let rank = rank_of(holding, board);
        if rank <= hero_rank {
            continue;
        }
        let Some(kind) = kind_of(rank, hero_rank) else { continue };
        match rows.iter_mut().find(|r| r.kind == kind) {
            Some(row) => {
                row.ways += 1;
                if rank > row.best {
                    row.best = rank;
                    row.example = holding;
                }
            }
            None => rows.push(KindRow { kind, ways: 1, example: holding, best: rank }),
        }
    }
    rows.sort_by_key(|r| BeatKind::ALL.iter().position(|k| *k == r.kind));
    rows
}

/// How many ways a holding type can be held after removing the cards you can see.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WaysCount {
    pub ways: u32,
    /// Unseen cards of the higher rank and of the lower rank.
    pub high_left: u32,
    pub low_left: u32,
}

fn unseen_of_rank(rank: u8, seen: u64) -> u32 {
    (0..52u8)
        .map(Card::from_index)
        .filter(|c| c.rank() == rank && seen & c.mask() == 0)
        .count() as u32
}

pub fn ways_count(high: u8, low: u8, hero: Holding, board: &[Card]) -> WaysCount {
    let seen = holding_mask(&hero) | mask_of(board);
    let high_left = unseen_of_rank(high, seen);
    let low_left = unseen_of_rank(low, seen);
    let ways = if high == low { high_left * high_left.saturating_sub(1) / 2 } else { high_left * low_left };
    WaysCount { ways, high_left, low_left }
}

/// `pocket kings` or `king-queen`, for a holding type.
pub fn type_name(high: u8, low: u8) -> String {
    if high == low {
        format!("a pair of {}s", rank_char(high))
    } else {
        format!("{}-{}", rank_char(high), rank_char(low))
    }
}

/// The three answers at rung 3.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum MixBand {
    Ahead,
    Even,
    Behind,
}

impl MixBand {
    pub const ALL: [MixBand; 3] = [MixBand::Ahead, MixBand::Even, MixBand::Behind];

    /// Over [`MIX_AHEAD_PCT`], under [`MIX_BEHIND_PCT`], otherwise about even.
    pub fn from_pct(pct: f64) -> MixBand {
        if pct > MIX_AHEAD_PCT {
            MixBand::Ahead
        } else if pct < MIX_BEHIND_PCT {
            MixBand::Behind
        } else {
            MixBand::Even
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            MixBand::Ahead => "a",
            MixBand::Even => "e",
            MixBand::Behind => "b",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            MixBand::Ahead => "usually ahead (over 60%)",
            MixBand::Even => "about even (40-60%)",
            MixBand::Behind => "usually behind (under 40%)",
        }
    }
}

/// Hero against every hand in a mix, as things stand now.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct MixStanding {
    pub total: usize,
    pub beat_you: usize,
    pub tie: usize,
    /// Hero's share of showdowns against the mix, ties counted half, in percent.
    pub pct: f64,
    pub band: MixBand,
}

pub fn mix_standing(hero: Holding, board: &[Card], mix: &OppMix) -> MixStanding {
    let hero_rank = rank_of(hero, board);
    let combos = mix_combos(mix, hero, board);
    let beat_you = combos.iter().filter(|&&h| rank_of(h, board) > hero_rank).count();
    let tie = combos.iter().filter(|&&h| rank_of(h, board) == hero_rank).count();
    let total = combos.len();
    let pct = 100.0 * (total - beat_you - tie) as f64 / total.max(1) as f64 + 50.0 * tie as f64 / total.max(1) as f64;
    MixStanding { total, beat_you, tie, pct, band: MixBand::from_pct(pct) }
}

/// Picks a mix for `player`, keeping only the groups this board can hold.
pub fn mix_for(player: PlayerType, hero: Holding, board: &[Card]) -> Option<OppMix> {
    let classes: Vec<_> = player
        .groups()
        .iter()
        .copied()
        .filter(|&c| combos_in_class(c, hero, board).len() >= MIN_CLASS_COMBOS)
        .collect();
    (classes.len() >= 2).then_some(OppMix { player: Some(player), classes })
}

/// Generates the question at the seed's rung.
pub fn gen_beats_me(rng: &mut Rng, seed: u64) -> BeatsMeQuestion {
    match beats_me_rung_of_seed(seed) {
        0 => gen_which(rng),
        1 => gen_kinds(rng),
        2 => gen_ways(rng),
        _ => gen_mix(rng),
    }
}

fn deal_hero_and_board(rng: &mut Rng) -> (Holding, Vec<Card>) {
    let mut dead = 0;
    let len = rng.range(3, 5) as usize;
    let board = deal_board(rng, len, &mut dead);
    let hero = deal_holding(rng, dead);
    (hero, board)
}

/// A hand worth asking about, by the shapes the ladder's rungs expect.
fn has_a_pair_or_better(hero: Holding, board: &[Card]) -> bool {
    rank_of(hero, board).category() >= Category::OnePair
}

/// Holdings a real opponent might show: a hole card pairs the board, a pocket
/// pair, a draw, or a straight or better.
fn believable(holding: Holding, board: &[Card]) -> bool {
    crate::classes::worth_a_decision(holding, board)
}

fn pick_disjoint(rng: &mut Rng, pool: &[Holding], count: usize, used: &mut u64) -> Option<Vec<Holding>> {
    let mut shuffled = pool.to_vec();
    rng.shuffle(&mut shuffled);
    let mut out = Vec::with_capacity(count);
    for holding in shuffled {
        if out.len() == count {
            break;
        }
        let mask = holding_mask(&holding);
        if *used & mask == 0 {
            *used |= mask;
            out.push(holding);
        }
    }
    (out.len() == count).then_some(out)
}

fn gen_which(rng: &mut Rng) -> BeatsMeQuestion {
    let total: u64 = WHICH_AHEAD_WEIGHTS.iter().map(|&(_, w)| w).sum();
    let mut roll = rng.below(total);
    let mut ahead_wanted = 1;
    for (count, weight) in WHICH_AHEAD_WEIGHTS {
        if roll < weight {
            ahead_wanted = count;
            break;
        }
        roll -= weight;
    }
    loop {
        let (hero, board) = deal_hero_and_board(rng);
        if !has_a_pair_or_better(hero, &board) {
            continue;
        }
        let hero_rank = rank_of(hero, &board);
        let pool: Vec<Holding> = unseen_holdings(holding_mask(&hero) | mask_of(&board))
            .into_iter()
            .filter(|&h| believable(h, &board))
            .collect();
        let (ahead, not_ahead): (Vec<Holding>, Vec<Holding>) =
            pool.into_iter().partition(|&h| rank_of(h, &board) > hero_rank);
        let mut used = holding_mask(&hero) | mask_of(&board);
        let Some(mut hands) = pick_disjoint(rng, &ahead, ahead_wanted, &mut used) else { continue };
        let Some(others) = pick_disjoint(rng, &not_ahead, 4 - ahead_wanted, &mut used) else { continue };
        hands.extend(others);
        rng.shuffle(&mut hands);
        let options = [hands[0], hands[1], hands[2], hands[3]];
        return BeatsMeQuestion { hero, board, ask: BeatsMeAsk::Which(options) };
    }
}

fn gen_kinds(rng: &mut Rng) -> BeatsMeQuestion {
    let want_nuts = rng.chance(1, KINDS_NUTS_ONE_IN);
    loop {
        let (hero, board) = deal_hero_and_board(rng);
        if want_nuts {
            let top = nuts_analysis(&board);
            if top.all_tie {
                continue;
            }
            let nut = rng.pick(&top.top_holdings);
            if rank_of(nut, &board).category() < Category::OnePair {
                continue;
            }
            return BeatsMeQuestion { hero: nut, board, ask: BeatsMeAsk::Kinds };
        }
        if has_a_pair_or_better(hero, &board) && !kinds_analysis(hero, &board).is_empty() {
            return BeatsMeQuestion { hero, board, ask: BeatsMeAsk::Kinds };
        }
    }
}

fn gen_ways(rng: &mut Rng) -> BeatsMeQuestion {
    loop {
        let (hero, board) = deal_hero_and_board(rng);
        if !has_a_pair_or_better(hero, &board) {
            continue;
        }
        let hero_rank = rank_of(hero, &board);
        let holdings = unseen_holdings(holding_mask(&hero) | mask_of(&board));
        // A rank-pair is a candidate only when all its unseen combos beat hero.
        let mut types: Vec<(u8, u8, bool)> = Vec::new();
        for h in holdings {
            let (a, b) = (h[0].rank(), h[1].rank());
            let (high, low) = (a.max(b), a.min(b));
            let ahead = rank_of(h, &board) > hero_rank;
            match types.iter_mut().find(|t| t.0 == high && t.1 == low) {
                Some(t) => t.2 &= ahead,
                None => types.push((high, low, ahead)),
            }
        }
        let candidates: Vec<(u8, u8)> = types.iter().filter(|t| t.2).map(|t| (t.0, t.1)).collect();
        if candidates.is_empty() {
            continue;
        }
        let (high, low) = rng.pick(&candidates);
        return BeatsMeQuestion { hero, board, ask: BeatsMeAsk::Ways { high, low } };
    }
}

fn gen_mix(rng: &mut Rng) -> BeatsMeQuestion {
    let target = MixBand::ALL[rng.below(3) as usize];
    let player = PlayerType::ALL[rng.below(3) as usize];
    loop {
        let (hero, board) = deal_hero_and_board(rng);
        if !has_a_pair_or_better(hero, &board) {
            continue;
        }
        let Some(mix) = mix_for(player, hero, &board) else { continue };
        if mix_standing(hero, &board, &mix).band == target {
            return BeatsMeQuestion { hero, board, ask: BeatsMeAsk::Mix(mix) };
        }
    }
}

/// The sentence asking the question at each rung, in plain language.
pub fn prompt_text(q: &BeatsMeQuestion) -> String {
    match &q.ask {
        BeatsMeAsk::Which(_) => "Which of these are ahead of you right now?".to_string(),
        BeatsMeAsk::Kinds => "Which kinds of hand beat you?".to_string(),
        BeatsMeAsk::Ways { high, low } => format!("How many ways can they hold {}?", type_name(*high, *low)),
        BeatsMeAsk::Mix(mix) => format!("{} Are you ahead of what they'd really have?", mix.assumption(None)),
    }
}

/// The teaching lines after a `Ways` answer.
pub fn ways_method(high: u8, low: u8, count: WaysCount) -> Vec<String> {
    let specific = if high == low {
        format!(
            "{} left: {} x {} / 2 = {} ways.",
            plural_cards(count.high_left, high),
            count.high_left,
            count.high_left.saturating_sub(1),
            count.ways
        )
    } else {
        format!(
            "{} and {} left: {} x {} = {} ways.",
            plural_cards(count.high_left, high),
            plural_cards(count.low_left, low),
            count.high_left,
            count.low_left,
            count.ways
        )
    };
    vec![
        specific,
        "The shortcut: a pair is 6 ways, 3 if one is on the board, 1 if two are.".to_string(),
        "Two different ranks is 16 ways, 4 suited and 12 offsuit, then remove the cards you can see.".to_string(),
    ]
}

fn plural_cards(count: u32, rank: u8) -> String {
    format!("{count} {}{}", rank_char(rank), if count == 1 { "" } else { "s" })
}

/// The one-line method after a `Mix` answer.
pub fn mix_method(standing: &MixStanding) -> String {
    let ties = if standing.tie > 0 { format!(", {} tie", standing.tie) } else { String::new() };
    format!(
        "Of the {} hands in that mix, {} beat you{ties}. That makes you {:.1}% against it.",
        standing.total, standing.beat_you, standing.pct
    )
}

/// Family key for a `beats_me` item: the rung, and something about its shape.
pub fn beats_me_family(q: &BeatsMeQuestion) -> String {
    let street = |board: &[Card]| match board.len() {
        3 => "flop",
        4 => "turn",
        _ => "river",
    };
    match &q.ask {
        BeatsMeAsk::Which(_) => format!("0:{}", street(&q.board)),
        BeatsMeAsk::Kinds => format!("1:{}", street(&q.board)),
        BeatsMeAsk::Ways { high, low } => format!("2:{}", if high == low { "pair" } else { "two" }),
        BeatsMeAsk::Mix(mix) => format!("3:{}", mix_standing(q.hero, &q.board, mix).band.code()),
    }
}
