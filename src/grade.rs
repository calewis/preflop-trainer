//! Answer parsing, exact truth and grading. Returns structured reveals.

use crate::anchors::{self, AnchorCard};
use crate::answer::MAX_OUTS;
use crate::board_type::{classify_flop, BoardCategory, BOARD_TYPE_MISS_MS};
use crate::cards::{holding_mask, mask_of, rank_from_byte, unseen_cards, unseen_holdings, Card, Holding};
use crate::classes::{combos_in_class, made_class, rank_of, MadeClass};
use crate::equity::{equity_all_in, equity_next_card, Equity};
use crate::eval::{straight_top, Category, HandRank};
use crate::item::{EquityOpponents, EquityQuestion, HitStreet, Item, Question, SpotMode, SpotQuestion};
use crate::money;
use crate::preflop::{preflop_answer, Action, PreflopReason, Seat, Situation};
use crate::table::{self, TableGrade, TableQuestion};

/// EV-loss bands for `spot` and `spot_mw`, as a percentage of P. Starting guesses.
pub const EV_CLOSE_PCT: f64 = 1.0;
pub const EV_SMALL_PCT: f64 = 5.0;
pub const EV_MISTAKE_PCT: f64 = 15.0;

/// Tolerance, in percentage points, for `price` and `hit` answers.
pub const PERCENT_TOLERANCE: f64 = 2.0;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum EvBand {
    Close,
    Small,
    Mistake,
    Blunder,
}

/// Bands: under 1% close, 1 to under 5% small, 5 to 15% mistake, over 15% blunder.
pub fn ev_band(loss_pct: f64) -> EvBand {
    if loss_pct < EV_CLOSE_PCT {
        EvBand::Close
    } else if loss_pct < EV_SMALL_PCT {
        EvBand::Small
    } else if loss_pct <= EV_MISTAKE_PCT {
        EvBand::Mistake
    } else {
        EvBand::Blunder
    }
}

/// The input could not be understood; the front end should ask again.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct InvalidAnswer(pub String);

#[derive(Clone, Debug)]
pub struct Outcome {
    pub correct: bool,
    /// `outs`: off by exactly one. `spot`: the two actions were within 1% of P.
    pub close: bool,
    pub ev_loss_pct: Option<f64>,
    /// Canonical form of what was answered.
    pub answer: String,
    pub reveal: Reveal,
    /// Set for `equity` items: what gets logged beyond the answer itself.
    pub equity_log: Option<EquityLog>,
}

/// The facts an `equity` attempt logs.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct EquityLog {
    pub band: u8,
    /// Estimate minus truth, in percentage points.
    pub signed_error: f64,
    pub truth_pct: f64,
    pub opponents: u8,
}

impl Outcome {
    /// Correct, or close enough to count (outs within 1, spots within the close band).
    pub fn ok(&self) -> bool {
        self.correct || self.close
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct PossibleSet {
    pub flush: bool,
    pub straight: bool,
    pub full: bool,
}

impl PossibleSet {
    pub fn code(self) -> String {
        let mut out = String::new();
        if self.flush {
            out.push('f');
        }
        if self.straight {
            out.push('s');
        }
        if self.full {
            out.push('h');
        }
        if out.is_empty() {
            out.push_str("none");
        }
        out
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum BeatsBucket {
    A,
    B,
    C,
    D,
}

impl BeatsBucket {
    /// a under 2%, b 2 to under 10%, c 10 to 30%, d over 30%.
    pub fn from_pct(pct: f64) -> BeatsBucket {
        if pct < 2.0 {
            BeatsBucket::A
        } else if pct < 10.0 {
            BeatsBucket::B
        } else if pct <= 30.0 {
            BeatsBucket::C
        } else {
            BeatsBucket::D
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            BeatsBucket::A => "a",
            BeatsBucket::B => "b",
            BeatsBucket::C => "c",
            BeatsBucket::D => "d",
        }
    }
}

#[derive(Clone, Debug)]
pub struct AheadKind {
    pub category: Category,
    pub count: usize,
    pub best: HandRank,
    pub example: Holding,
}

#[derive(Clone, Debug)]
pub struct TemptingCard {
    pub card: Card,
    pub new_rank: HandRank,
    /// True when hero and the opponent would tie rather than hero being behind.
    pub ties: bool,
    /// True when the card connects with a hole card or makes a straight or better,
    /// rather than only pairing the board.
    pub genuine: bool,
}

#[derive(Clone, Debug)]
pub struct NutsAnalysis {
    pub top: HandRank,
    pub top_holdings: Vec<Holding>,
    /// The next-best distinct hand strength and who makes it.
    pub next: Option<(HandRank, Vec<Holding>)>,
    pub unseen_count: usize,
    pub all_tie: bool,
}

#[derive(Clone, Debug)]
pub struct PossibleAnalysis {
    pub set: PossibleSet,
    pub flush_example: Option<Holding>,
    pub straight_example: Option<Holding>,
    pub full_example: Option<Holding>,
    pub max_suit_on_board: u8,
    pub board_paired: bool,
}

#[derive(Clone, Debug)]
pub struct BeatsAnalysis {
    pub ahead: usize,
    pub total: usize,
    pub pct: f64,
    pub bucket: BeatsBucket,
    /// Holdings that beat hero, grouped by category, strongest first.
    pub kinds: Vec<AheadKind>,
    pub hero_rank: HandRank,
}

#[derive(Clone, Debug)]
pub struct OutsAnalysis {
    pub outs: Vec<Card>,
    /// Cards that improve hero's category but don't put hero strictly ahead.
    pub tempting: Vec<TemptingCard>,
    pub unseen: usize,
}

#[derive(Clone, Debug)]
pub struct SpotAnalysis {
    /// P: the pot before hero acts, including the bet and earlier callers.
    pub pot: u32,
    /// C: the amount to call.
    pub call: u32,
    pub share: f64,
    pub sampled: bool,
    pub combos_per_opponent: Vec<usize>,
    /// `C / (P + C)`.
    pub price: f64,
    /// `C / (P + C + X)`.
    pub required: f64,
    pub ev_call: f64,
    pub ev_fold: f64,
    pub break_even_x: Option<f64>,
    pub x: u32,
    pub next_card: bool,
}

/// The truth behind an `equity` question.
#[derive(Clone, PartialEq, Debug)]
pub struct EquityAnalysis {
    /// Hero's true all-in equity, in percent.
    pub truth_pct: f64,
    /// True when a huge multiway case was estimated from the fixed sample.
    pub sampled: bool,
    /// Card-disjoint opponent tuples behind the figure; `None` when sampled.
    pub tuples: Option<u64>,
    pub combos_per_opponent: Vec<usize>,
}

#[derive(Clone, Debug)]
pub enum Reveal {
    Preflop {
        seat: Seat,
        situation: Situation,
        action: Action,
        reason: PreflopReason,
    },
    Nuts {
        analysis: NutsAnalysis,
        used_x: bool,
        consistent: usize,
        consistent_nuts: usize,
    },
    Possible(PossibleAnalysis),
    Made {
        class: MadeClass,
        rank: HandRank,
    },
    Beats(BeatsAnalysis),
    Outs {
        analysis: OutsAnalysis,
        hero_rank: HandRank,
        villain_rank: HandRank,
    },
    Price {
        pot: u32,
        call: u32,
        required: f64,
    },
    Hit {
        outs: u32,
        unseen: u32,
        both: bool,
        exact: f64,
        shortcut: f64,
    },
    Spot {
        analysis: SpotAnalysis,
        chosen: Action,
        best: Action,
        loss_pct: f64,
        loss_money: f64,
        band: Option<EvBand>,
    },
    Equity {
        analysis: EquityAnalysis,
        estimate_pct: f64,
        signed_error: f64,
        band: u8,
        hit: bool,
    },
    Anchor {
        card: AnchorCard,
        estimate_pct: f64,
        signed_error: f64,
    },
    BoardType {
        category: BoardCategory,
        chosen: BoardCategory,
        /// The answer was right but took longer than [`BOARD_TYPE_MISS_MS`].
        too_slow: bool,
    },
    Table(TableGrade),
}

/// Grades a raw answer. `Err` means the input was not understood and nothing is counted.
pub fn grade(item: &Item, raw: &str) -> Result<Outcome, InvalidAnswer> {
    match &item.question {
        Question::Preflop { behind, situation, hand, .. } => {
            grade_preflop(Seat::from_behind(*behind), *situation, *hand, raw)
        }
        Question::Nuts { board } => grade_nuts(board, raw),
        Question::Possible { board } => grade_possible(board, raw),
        Question::Made { hero, board } => grade_made(*hero, board, raw),
        Question::Beats { hero, board } => grade_beats(*hero, board, raw),
        Question::Outs { hero, board, villain } => grade_outs(*hero, board, *villain, raw),
        Question::Price { pot_before, bet, callers } => grade_price(*pot_before, *bet, *callers, raw),
        Question::Hit { outs, street, both } => grade_hit(*outs, *street, *both, raw),
        Question::Spot(q) => grade_spot(q, item.seed, raw),
        Question::Equity(q) => grade_equity(q, item.seed, raw),
        Question::Anchor { card } => grade_anchor(usize::from(*card), raw),
        Question::BoardType { board, options } => grade_board_type(board, options, raw),
        Question::Table(q) => grade_table(q, raw),
    }
}

/// [`grade`], for front ends that time the player: a `board_type` answer slower
/// than [`BOARD_TYPE_MISS_MS`] is a miss even if the answer was right.
pub fn grade_timed(item: &Item, raw: &str, response_ms: u32) -> Result<Outcome, InvalidAnswer> {
    let mut result = grade(item, raw)?;
    if response_ms > BOARD_TYPE_MISS_MS {
        if let Reveal::BoardType { too_slow, .. } = &mut result.reveal {
            *too_slow = result.correct;
            result.correct = false;
        }
    }
    Ok(result)
}

fn normalize(raw: &str) -> String {
    raw.chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

fn outcome(correct: bool, answer: String, reveal: Reveal) -> Outcome {
    Outcome { correct, close: false, ev_loss_pct: None, answer, reveal, equity_log: None }
}

fn grade_preflop(
    seat: Seat,
    situation: Situation,
    hand: Holding,
    raw: &str,
) -> Result<Outcome, InvalidAnswer> {
    let chosen = match normalize(raw).as_str() {
        "r" => Action::Raise,
        "c" => Action::Call,
        "f" => Action::Fold,
        _ => return Err(InvalidAnswer("Answer r, c or f.".to_string())),
    };
    let (action, reason) = preflop_answer(seat, situation, hand);
    Ok(outcome(
        chosen == action,
        chosen.code().to_string(),
        Reveal::Preflop { seat, situation, action, reason },
    ))
}

/// A parsed `nuts` answer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NutsGuess {
    Ranks { a: u8, b: u8, suited: Option<bool> },
    Exact(Holding),
    CardX(Card),
    RankX(u8),
}

pub fn parse_nuts_guess(raw: &str) -> Option<NutsGuess> {
    let text = normalize(raw);
    let b = text.as_bytes();
    match b.len() {
        2 if b[1] == b'x' => Some(NutsGuess::RankX(rank_from_byte(b[0])?)),
        2 => Some(NutsGuess::Ranks {
            a: rank_from_byte(b[0])?,
            b: rank_from_byte(b[1])?,
            suited: None,
        }),
        3 if b[2] == b'x' => Some(NutsGuess::CardX(Card::parse(text.get(..2)?)?)),
        3 if b[2] == b's' || b[2] == b'o' => {
            let (r1, r2) = (rank_from_byte(b[0])?, rank_from_byte(b[1])?);
            if r1 == r2 {
                return None;
            }
            Some(NutsGuess::Ranks { a: r1, b: r2, suited: Some(b[2] == b's') })
        }
        4 => {
            let first = Card::parse(text.get(..2)?)?;
            let second = Card::parse(text.get(2..)?)?;
            if first == second {
                return None;
            }
            Some(NutsGuess::Exact([first, second]))
        }
        _ => None,
    }
}

/// Whether a holding is consistent with a guess.
pub fn guess_matches(guess: NutsGuess, holding: Holding) -> bool {
    let [h1, h2] = holding;
    match guess {
        NutsGuess::Ranks { a, b, suited } => {
            let ranks_match = (h1.rank() == a && h2.rank() == b) || (h1.rank() == b && h2.rank() == a);
            ranks_match
                && match suited {
                    None => true,
                    Some(want) => (h1.suit() == h2.suit()) == want,
                }
        }
        NutsGuess::Exact([c1, c2]) => (h1 == c1 && h2 == c2) || (h1 == c2 && h2 == c1),
        NutsGuess::CardX(card) => h1 == card || h2 == card,
        NutsGuess::RankX(rank) => h1.rank() == rank || h2.rank() == rank,
    }
}

/// Enumerates every unseen holding and finds the top two distinct strengths.
pub fn nuts_analysis(board: &[Card]) -> NutsAnalysis {
    let holdings = unseen_holdings(mask_of(board));
    let ranked: Vec<(Holding, HandRank)> =
        holdings.iter().map(|&h| (h, rank_of(h, board))).collect();
    let top = ranked.iter().map(|&(_, r)| r).max().expect("holdings exist");
    let top_holdings: Vec<Holding> =
        ranked.iter().filter(|&&(_, r)| r == top).map(|&(h, _)| h).collect();
    let second = ranked.iter().map(|&(_, r)| r).filter(|&r| r < top).max();
    let next = second.map(|s| {
        (s, ranked.iter().filter(|&&(_, r)| r == s).map(|&(h, _)| h).collect())
    });
    NutsAnalysis {
        top,
        all_tie: top_holdings.len() == holdings.len(),
        top_holdings,
        next,
        unseen_count: holdings.len(),
    }
}

fn grade_nuts(board: &[Card], raw: &str) -> Result<Outcome, InvalidAnswer> {
    const HELP: &str = "Give two ranks (QJ, KK), ranks with s or o (AKs), two exact cards (AhKh), or a card or rank plus x (Ahx, Ax).";
    let guess = parse_nuts_guess(raw).ok_or_else(|| InvalidAnswer(HELP.to_string()))?;
    let analysis = nuts_analysis(board);
    let holdings = unseen_holdings(mask_of(board));
    let consistent: Vec<Holding> =
        holdings.into_iter().filter(|&h| guess_matches(guess, h)).collect();
    if consistent.is_empty() {
        return Err(InvalidAnswer(
            "No unseen holding matches that (those cards are on the board). Try again.".to_string(),
        ));
    }
    let is_nut = |h: &Holding| rank_of(*h, board) == analysis.top;
    let consistent_nuts = consistent.iter().filter(|h| is_nut(h)).count();
    let used_x = matches!(guess, NutsGuess::CardX(_) | NutsGuess::RankX(_));
    let correct = if used_x {
        consistent_nuts == consistent.len()
    } else {
        consistent_nuts > 0
    };
    Ok(outcome(
        correct,
        normalize(raw),
        Reveal::Nuts {
            analysis,
            used_x,
            consistent: consistent.len(),
            consistent_nuts,
        },
    ))
}

/// Enumerates every unseen holding for flush, straight and full house or quads.
pub fn possible_analysis(board: &[Card]) -> PossibleAnalysis {
    let mut result = PossibleAnalysis {
        set: PossibleSet { flush: false, straight: false, full: false },
        flush_example: None,
        straight_example: None,
        full_example: None,
        max_suit_on_board: 0,
        board_paired: false,
    };
    let mut suit_counts = [0u8; 4];
    let mut rank_counts = [0u8; 13];
    for card in board {
        suit_counts[card.suit() as usize] += 1;
        rank_counts[card.rank() as usize] += 1;
    }
    result.max_suit_on_board = suit_counts.into_iter().max().unwrap_or(0);
    result.board_paired = rank_counts.iter().any(|&n| n >= 2);

    for holding in unseen_holdings(mask_of(board)) {
        let rank = rank_of(holding, board);
        let mut suits = [0u16; 4];
        for card in holding.iter().chain(board) {
            suits[card.suit() as usize] |= 1 << card.rank();
        }
        let ranks = suits[0] | suits[1] | suits[2] | suits[3];
        if suits.iter().any(|s| s.count_ones() >= 5) && result.flush_example.is_none() {
            result.flush_example = Some(holding);
        }
        if straight_top(ranks).is_some() && result.straight_example.is_none() {
            result.straight_example = Some(holding);
        }
        if matches!(rank.category(), Category::FullHouse | Category::Quads) && result.full_example.is_none() {
            result.full_example = Some(holding);
        }
    }
    result.set = PossibleSet {
        flush: result.flush_example.is_some(),
        straight: result.straight_example.is_some(),
        full: result.full_example.is_some(),
    };
    result
}

fn parse_possible(raw: &str) -> Option<PossibleSet> {
    let text: String = normalize(raw).chars().filter(|&c| c != ',').collect();
    if text == "none" {
        return Some(PossibleSet { flush: false, straight: false, full: false });
    }
    if text.is_empty() {
        return None;
    }
    let mut set = PossibleSet { flush: false, straight: false, full: false };
    for ch in text.chars() {
        let flag = match ch {
            'f' => &mut set.flush,
            's' => &mut set.straight,
            'h' => &mut set.full,
            _ => return None,
        };
        if *flag {
            return None;
        }
        *flag = true;
    }
    Some(set)
}

fn grade_possible(board: &[Card], raw: &str) -> Result<Outcome, InvalidAnswer> {
    let guess = parse_possible(raw)
        .ok_or_else(|| InvalidAnswer("Answer any of f, s, h together (such as fs), or none.".to_string()))?;
    let analysis = possible_analysis(board);
    Ok(outcome(guess == analysis.set, guess.code(), Reveal::Possible(analysis)))
}

fn grade_made(hero: Holding, board: &[Card], raw: &str) -> Result<Outcome, InvalidAnswer> {
    let guess = MadeClass::from_code(&normalize(raw)).ok_or_else(|| {
        InvalidAnswer("Answer one of hc, board, low, mid, top, over, 2p, set, trips, st, fl, fh, 4k, sf.".to_string())
    })?;
    let class = made_class(hero, board);
    Ok(outcome(
        guess == class,
        guess.code().to_string(),
        Reveal::Made { class, rank: rank_of(hero, board) },
    ))
}

/// Counts every unseen holding strictly ahead of hero right now.
pub fn beats_analysis(hero: Holding, board: &[Card]) -> BeatsAnalysis {
    let hero_rank = rank_of(hero, board);
    let holdings = unseen_holdings(holding_mask(&hero) | mask_of(board));
    let total = holdings.len();
    let mut ahead = 0;
    let mut kinds: Vec<AheadKind> = Vec::new();
    for holding in holdings {
        let rank = rank_of(holding, board);
        if rank <= hero_rank {
            continue;
        }
        ahead += 1;
        let category = rank.category();
        match kinds.iter_mut().find(|k| k.category == category) {
            Some(kind) => {
                kind.count += 1;
                if rank > kind.best {
                    kind.best = rank;
                    kind.example = holding;
                }
            }
            None => kinds.push(AheadKind { category, count: 1, best: rank, example: holding }),
        }
    }
    kinds.sort_by_key(|k| std::cmp::Reverse(k.category));
    let pct = 100.0 * ahead as f64 / total as f64;
    BeatsAnalysis { ahead, total, pct, bucket: BeatsBucket::from_pct(pct), kinds, hero_rank }
}

fn grade_beats(hero: Holding, board: &[Card], raw: &str) -> Result<Outcome, InvalidAnswer> {
    let guess = match normalize(raw).as_str() {
        "a" => BeatsBucket::A,
        "b" => BeatsBucket::B,
        "c" => BeatsBucket::C,
        "d" => BeatsBucket::D,
        _ => return Err(InvalidAnswer("Answer a, b, c or d.".to_string())),
    };
    let analysis = beats_analysis(hero, board);
    Ok(outcome(guess == analysis.bucket, guess.code().to_string(), Reveal::Beats(analysis)))
}

/// Next cards that leave hero strictly ahead, and tempting cards that do not.
pub fn outs_analysis(hero: Holding, board: &[Card], villain: Holding) -> OutsAnalysis {
    let dead = holding_mask(&hero) | holding_mask(&villain) | mask_of(board);
    let before = rank_of(hero, board).category();
    let unseen = unseen_cards(dead);
    let mut outs = Vec::new();
    let mut tempting = Vec::new();
    let mut next_board = board.to_vec();
    next_board.push(hero[0]);
    let last = next_board.len() - 1;
    for &card in &unseen {
        next_board[last] = card;
        let hero_rank = rank_of(hero, &next_board);
        let villain_rank = rank_of(villain, &next_board);
        if hero_rank > villain_rank {
            outs.push(card);
        } else if hero_rank.category() > before {
            let genuine = hero_rank.category() >= Category::Straight
                || card.rank() == hero[0].rank()
                || card.rank() == hero[1].rank();
            tempting.push(TemptingCard {
                card,
                new_rank: hero_rank,
                ties: hero_rank == villain_rank,
                genuine,
            });
        }
    }
    tempting.sort_by(|a, b| b.genuine.cmp(&a.genuine).then(b.new_rank.cmp(&a.new_rank)));
    OutsAnalysis { outs, tempting, unseen: unseen.len() }
}

fn grade_outs(hero: Holding, board: &[Card], villain: Holding, raw: &str) -> Result<Outcome, InvalidAnswer> {
    let guess: u32 = normalize(raw)
        .parse()
        .ok()
        .filter(|&n| n <= MAX_OUTS)
        .ok_or_else(|| InvalidAnswer("Answer a whole number of outs.".to_string()))?;
    let analysis = outs_analysis(hero, board, villain);
    let truth = analysis.outs.len() as u32;
    let correct = guess == truth;
    let hero_rank = rank_of(hero, board);
    let villain_rank = rank_of(villain, board);
    let mut result = outcome(
        correct,
        guess.to_string(),
        Reveal::Outs { analysis, hero_rank, villain_rank },
    );
    result.close = guess.abs_diff(truth) == 1;
    Ok(result)
}

/// Parses `25` or `25%`; rejects anything outside 0 to 100.
pub fn parse_percent(raw: &str) -> Option<f64> {
    let text = normalize(raw);
    let value: f64 = text.strip_suffix('%').unwrap_or(&text).parse().ok()?;
    if (0.0..=100.0).contains(&value) {
        Some(value)
    } else {
        None
    }
}

fn percent_guess(raw: &str) -> Result<f64, InvalidAnswer> {
    parse_percent(raw).ok_or_else(|| InvalidAnswer("Answer a percentage such as 25 or 25%.".to_string()))
}

/// The pot before hero acts when `callers` have already matched the bet.
pub fn pot_with_bet(pot_before: u32, bet: u32, callers: u32) -> u32 {
    pot_before + bet + callers * bet
}

fn grade_price(pot_before: u32, bet: u32, callers: u32, raw: &str) -> Result<Outcome, InvalidAnswer> {
    let guess = percent_guess(raw)?;
    let pot = pot_with_bet(pot_before, bet, callers);
    let required = money::break_even(f64::from(pot), f64::from(bet));
    Ok(outcome(
        (guess - required * 100.0).abs() <= PERCENT_TOLERANCE,
        format!("{guess}"),
        Reveal::Price { pot, call: bet, required },
    ))
}

/// Exact chance of hitting one of `outs`, from 47 unseen on the flop, 46 on the turn.
pub fn hit_chance(outs: u32, street: HitStreet, both: bool) -> f64 {
    let unseen = hit_unseen(street);
    if both {
        let miss = f64::from(unseen - outs) * f64::from(unseen - outs - 1)
            / (f64::from(unseen) * f64::from(unseen - 1));
        1.0 - miss
    } else {
        f64::from(outs) / f64::from(unseen)
    }
}

pub fn hit_unseen(street: HitStreet) -> u32 {
    match street {
        HitStreet::Flop => 47,
        HitStreet::Turn => 46,
    }
}

fn grade_hit(outs: u32, street: HitStreet, both: bool, raw: &str) -> Result<Outcome, InvalidAnswer> {
    let guess = percent_guess(raw)?;
    let exact = hit_chance(outs, street, both);
    Ok(outcome(
        (guess - exact * 100.0).abs() <= PERCENT_TOLERANCE,
        format!("{guess}"),
        Reveal::Hit {
            outs,
            unseen: hit_unseen(street),
            both,
            exact,
            shortcut: f64::from(outs) * if both { 4.0 } else { 2.0 },
        },
    ))
}

/// The relabelling behind [`canonical_suits`]: suits by first appearance across
/// hero then the board, with absent suits following in index order.
fn canonical_perm(hero: Holding, board: &[Card]) -> [u8; 4] {
    let mut perm = [u8::MAX; 4];
    let mut next = 0;
    for card in hero.iter().chain(board) {
        let slot = &mut perm[card.suit() as usize];
        if *slot == u8::MAX {
            *slot = next;
            next += 1;
        }
    }
    for slot in &mut perm {
        if *slot == u8::MAX {
            *slot = next;
            next += 1;
        }
    }
    perm
}

/// Relabels suits by first appearance across hero then the board. Two spots that
/// differ only in suit labels get the same canonical cards, so the combo lists,
/// and any sample drawn from them, come out identical.
fn canonical_suits(hero: Holding, board: &[Card]) -> (Holding, Vec<Card>) {
    let perm = canonical_perm(hero, board);
    (
        [hero[0].with_suit_perm(&perm), hero[1].with_suit_perm(&perm)],
        board.iter().map(|c| c.with_suit_perm(&perm)).collect(),
    )
}

/// Hero's exact all-in equity for an `equity` question, averaged over every
/// card-disjoint combo at rung 1 (sampled only for huge multiway cases). Every
/// suit permutation gives identical numbers. `None` only when no combo exists.
pub fn equity_analysis(q: &EquityQuestion, seed: u64) -> Option<EquityAnalysis> {
    let perm = canonical_perm(q.hero, &q.board);
    let hero = [q.hero[0].with_suit_perm(&perm), q.hero[1].with_suit_perm(&perm)];
    let board: Vec<Card> = q.board.iter().map(|c| c.with_suit_perm(&perm)).collect();
    let lists: Vec<Vec<Holding>> = match &q.opponents {
        EquityOpponents::Exact(hands) => hands
            .iter()
            .map(|h| vec![[h[0].with_suit_perm(&perm), h[1].with_suit_perm(&perm)]])
            .collect(),
        EquityOpponents::Classes(classes) => classes
            .iter()
            .map(|&class| combos_in_class(class, hero, &board))
            .collect(),
    };
    let eq = equity_all_in(hero, &board, &lists, seed)?;
    Some(EquityAnalysis {
        truth_pct: eq.share * 100.0,
        sampled: eq.sampled,
        tuples: eq.tuples,
        combos_per_opponent: lists.iter().map(Vec::len).collect(),
    })
}

fn grade_equity(q: &EquityQuestion, seed: u64, raw: &str) -> Result<Outcome, InvalidAnswer> {
    let estimate_pct = percent_guess(raw)?;
    let analysis = equity_analysis(q, seed)
        .ok_or_else(|| InvalidAnswer("This spot has no possible opponent hands.".to_string()))?;
    let signed_error = estimate_pct - analysis.truth_pct;
    let hit = signed_error.abs() <= f64::from(q.band);
    let truth_pct = analysis.truth_pct;
    let mut result = outcome(
        hit,
        format!("{estimate_pct}"),
        Reveal::Equity { analysis, estimate_pct, signed_error, band: q.band, hit },
    );
    result.equity_log = Some(EquityLog {
        band: q.band,
        signed_error,
        truth_pct,
        opponents: q.opponents.count() as u8,
    });
    Ok(result)
}

fn grade_anchor(card_index: usize, raw: &str) -> Result<Outcome, InvalidAnswer> {
    let estimate_pct = percent_guess(raw)?;
    let card = anchors::deck()
        .get(card_index)
        .ok_or_else(|| InvalidAnswer("That anchor card is not in the deck.".to_string()))?
        .clone();
    let signed_error = estimate_pct - card.answer_pct;
    let correct = signed_error.abs() <= f64::from(card.tolerance);
    Ok(outcome(correct, format!("{estimate_pct}"), Reveal::Anchor { card, estimate_pct, signed_error }))
}

fn grade_table(q: &TableQuestion, raw: &str) -> Result<Outcome, InvalidAnswer> {
    let spot = table::spot_by_id(q.spot_id);
    let code = normalize(raw);
    let chosen = spot.options.iter().position(|o| o.code == code).ok_or_else(|| {
        let codes: Vec<&str> = spot.options.iter().map(|o| o.code.as_str()).collect();
        InvalidAnswer(format!("Answer one of {}.", codes.join(", ")))
    })?;
    let graded = table::grade_option(spot, chosen);
    let (correct, loss_pct) = (graded.correct, graded.loss_pct);
    let mut result = outcome(correct, code, Reveal::Table(graded));
    result.ev_loss_pct = Some(loss_pct);
    Ok(result)
}

fn grade_board_type(board: &[Card], options: &[BoardCategory; 4], raw: &str) -> Result<Outcome, InvalidAnswer> {
    let code = normalize(raw);
    let chosen = options.iter().copied().find(|c| c.code() == code).ok_or_else(|| {
        let codes: Vec<&str> = options.iter().map(|c| c.code()).collect();
        InvalidAnswer(format!("Answer one of {}.", codes.join(", ")))
    })?;
    let category = classify_flop(board);
    Ok(outcome(
        chosen == category,
        chosen.code().to_string(),
        Reveal::BoardType { category, chosen, too_slow: false },
    ))
}

/// Exact (or, for huge multiway cases, sampled) share and money for a spot.
/// Every suit permutation of a question gives identical numbers.
/// `None` only if the assumed classes leave no card-disjoint combination.
pub fn spot_analysis(q: &SpotQuestion, seed: u64) -> Option<SpotAnalysis> {
    let (hero, board) = canonical_suits(q.hero, &q.board);
    let lists: Vec<Vec<Holding>> = q
        .opponents
        .iter()
        .map(|&class| combos_in_class(class, hero, &board))
        .collect();
    let eq: Equity = match q.mode {
        SpotMode::AllIn => equity_all_in(hero, &board, &lists, seed)?,
        SpotMode::NextCard { .. } => equity_next_card(hero, &board, &lists, seed)?,
    };
    let (pot, call, x) = (f64::from(q.pot), f64::from(q.call), f64::from(q.x()));
    Some(SpotAnalysis {
        pot: q.pot,
        call: q.call,
        share: eq.share,
        sampled: eq.sampled,
        combos_per_opponent: lists.iter().map(Vec::len).collect(),
        price: money::break_even(pot, call),
        required: money::break_even_with_x(pot, call, x),
        ev_call: money::ev_call(eq.share, pot, call, x),
        ev_fold: money::EV_FOLD,
        break_even_x: money::break_even_x(eq.share, pot, call),
        x: q.x(),
        next_card: matches!(q.mode, SpotMode::NextCard { .. }),
    })
}

fn grade_spot(q: &SpotQuestion, seed: u64, raw: &str) -> Result<Outcome, InvalidAnswer> {
    let chosen = match normalize(raw).as_str() {
        "c" => Action::Call,
        "f" => Action::Fold,
        _ => return Err(InvalidAnswer("Answer c or f.".to_string())),
    };
    let analysis = spot_analysis(q, seed)
        .ok_or_else(|| InvalidAnswer("This spot has no possible opponent hands.".to_string()))?;
    let gap = analysis.ev_call - analysis.ev_fold;
    let best = if gap >= 0.0 { Action::Call } else { Action::Fold };
    let loss_money = if chosen == best { 0.0 } else { gap.abs() };
    let pot = f64::from(q.pot);
    let loss_pct = 100.0 * loss_money / pot;
    let band = if chosen == best { None } else { Some(ev_band(loss_pct)) };
    let correct = loss_pct < EV_CLOSE_PCT;
    let mut result = outcome(
        correct,
        chosen.code().to_string(),
        Reveal::Spot { analysis, chosen, best, loss_pct, loss_money, band },
    );
    result.close = 100.0 * gap.abs() / pot < EV_CLOSE_PCT;
    result.ev_loss_pct = Some(loss_pct);
    Ok(result)
}
