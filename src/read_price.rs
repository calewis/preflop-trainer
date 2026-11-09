//! The "Read the price" ladder: five rungs that turn a bet into a price and a
//! price into a decision. Generation, truth, the ladder's rules and its reveals.
//! Every money amount is a whole dollar, and every price goes through [`crate::money`].

use std::sync::OnceLock;

use crate::grade::{hit_chance, pot_with_bet, PERCENT_TOLERANCE};
use crate::item::{read_price_rung_of_seed, HitStreet, PriceHand, ReadPriceAsk, ReadPriceQuestion, Skill, READ_PRICE_RUNGS};
use crate::log::{Attempt, ItemRef, Mode};
use crate::money;
use crate::rng::Rng;

/// Attempts at a rung considered for promotion.
pub const READ_PRICE_PROMOTE_WINDOW: usize = 20;
/// Promote once [`READ_PRICE_PROMOTE_WINDOW`] straight attempts at a rung reach this.
pub const READ_PRICE_PROMOTE_PCT: f64 = 85.0;
/// Rung 0: a price under this share is cheap, in percent.
pub const CHEAP_BELOW_PCT: f64 = 20.0;
/// Rung 0: a price of [`CHEAP_BELOW_PCT`] up to this share is fair, in percent.
pub const FAIR_BELOW_PCT: f64 = 30.0;
/// Rung 0: nothing is dealt within this many points of a band's edge.
pub const BAND_GAP_POINTS: f64 = 1.5;
/// Rungs 2-3: at least this far from the price is an easy call or a fold.
pub const EASY_POINTS: f64 = 8.0;
/// Rungs 2-3: within this many points of the price is close.
pub const CLOSE_POINTS: f64 = 4.0;
/// Rungs 2-3: dealt hands keep this much further inside those lines, so the
/// answer doesn't change when the share or price is rounded to a whole percent.
pub const DEAL_MARGIN_POINTS: f64 = 0.5;
/// Rung 4: no option is within this many points of break-even.
pub const OPTION_GAP_POINTS: f64 = 1.5;
/// Rung 4: wrong options stay within this many points of break-even, so an
/// answer can't be picked from how big the bets look against the pot.
pub const OPTION_BAND_POINTS: f64 = 10.0;

/// Each pot range with its weight, out of 100. The middle three span $6 to $60.
const POT_BUCKETS: [(u64, u64, u64); 6] = [(3, 5, 5), (6, 15, 30), (16, 30, 30), (31, 60, 20), (61, 120, 10), (121, 300, 5)];
/// Share of items with callers on rungs 1-3, out of 100. Callers' money going in
/// the pot is the point of this ladder, and the home game this is for is 4-5 handed.
const CALLERS_PERCENT: u64 = 40;
/// Rung 0's share with callers on cheap and fair items, out of 100. An expensive
/// price is out of reach once a caller's money is in the pot, so these make up the 40%.
const BAND_CALLERS_PERCENT: u64 = 60;
/// Of the items with callers that can be built this way, the share where leaving
/// the callers out would change the answer, out of 100.
pub const CALLERS_MATTER_PERCENT: u64 = 80;
/// Share of items whose largest allowed bet is two and a half times the pot
/// rather than a little over the pot, out of 100.
const OVERBET_PERCENT: u64 = 30;
/// Largest dollar amount offered as an answer, so option labels are built once.
const MAX_OPTION_DOLLARS: usize = 1000;

/// The three answers at rung 0.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PriceBand {
    Cheap,
    Fair,
    Expensive,
}

impl PriceBand {
    pub const ALL: [PriceBand; 3] = [PriceBand::Cheap, PriceBand::Fair, PriceBand::Expensive];

    pub fn code(self) -> &'static str {
        match self {
            PriceBand::Cheap => "c",
            PriceBand::Fair => "f",
            PriceBand::Expensive => "e",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PriceBand::Cheap => "Cheap",
            PriceBand::Fair => "Fair",
            PriceBand::Expensive => "Expensive",
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            PriceBand::Cheap => "cheap",
            PriceBand::Fair => "fair",
            PriceBand::Expensive => "expensive",
        }
    }

    pub fn from_pct(pct: f64) -> PriceBand {
        if pct < CHEAP_BELOW_PCT {
            PriceBand::Cheap
        } else if pct < FAIR_BELOW_PCT {
            PriceBand::Fair
        } else {
            PriceBand::Expensive
        }
    }
}

/// The three answers at rungs 2-3.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PriceCall {
    Easy,
    Close,
    Fold,
}

impl PriceCall {
    pub const ALL: [PriceCall; 3] = [PriceCall::Easy, PriceCall::Close, PriceCall::Fold];

    pub fn code(self) -> &'static str {
        match self {
            PriceCall::Easy => "c",
            PriceCall::Close => "l",
            PriceCall::Fold => "f",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PriceCall::Easy => "Easy call",
            PriceCall::Close => "Close",
            PriceCall::Fold => "Fold",
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            PriceCall::Easy => "easy",
            PriceCall::Close => "close",
            PriceCall::Fold => "fold",
        }
    }
}

/// The class of a share against a price, by the edge in points. `None` for an
/// edge in the gap between the classes, which is never dealt.
pub fn call_from_edge(edge_points: f64) -> Option<PriceCall> {
    if edge_points >= EASY_POINTS {
        Some(PriceCall::Easy)
    } else if edge_points <= -EASY_POINTS {
        Some(PriceCall::Fold)
    } else if edge_points.abs() <= CLOSE_POINTS {
        Some(PriceCall::Close)
    } else {
        None
    }
}

/// A named draw, with the outs it is assumed to have.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum DrawKind {
    Flush,
    OpenEnded,
    Gutshot,
    Overcards,
    FlushOpenEnded,
    FlushGutshot,
}

impl DrawKind {
    pub const ALL: [DrawKind; 6] =
        [DrawKind::Flush, DrawKind::OpenEnded, DrawKind::Gutshot, DrawKind::Overcards, DrawKind::FlushOpenEnded, DrawKind::FlushGutshot];

    pub const fn outs(self) -> u32 {
        match self {
            DrawKind::Flush => 9,
            DrawKind::OpenEnded => 8,
            DrawKind::Gutshot => 4,
            DrawKind::Overcards => 6,
            DrawKind::FlushOpenEnded => 15,
            DrawKind::FlushGutshot => 12,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            DrawKind::Flush => "flush",
            DrawKind::OpenEnded => "open",
            DrawKind::Gutshot => "gutshot",
            DrawKind::Overcards => "overs",
            DrawKind::FlushOpenEnded => "flushopen",
            DrawKind::FlushGutshot => "flushgut",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            DrawKind::Flush => "Flush draw",
            DrawKind::OpenEnded => "Open-ended straight draw",
            DrawKind::Gutshot => "Gutshot straight draw",
            DrawKind::Overcards => "Two overcards",
            DrawKind::FlushOpenEnded => "Flush and open-ended draw",
            DrawKind::FlushGutshot => "Flush and gutshot draw",
        }
    }

    /// As a phrase that follows "They have".
    pub fn with_article(self) -> &'static str {
        match self {
            DrawKind::Flush => "a flush draw",
            DrawKind::OpenEnded => "an open-ended straight draw",
            DrawKind::Gutshot => "a gutshot straight draw",
            DrawKind::Overcards => "two overcards",
            DrawKind::FlushOpenEnded => "a flush and open-ended draw",
            DrawKind::FlushGutshot => "a flush and gutshot draw",
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            DrawKind::Flush => "flush draw",
            DrawKind::OpenEnded => "open-ended straight draw",
            DrawKind::Gutshot => "gutshot straight draw",
            DrawKind::Overcards => "two overcards",
            DrawKind::FlushOpenEnded => "flush and open-ended draw",
            DrawKind::FlushGutshot => "flush and gutshot draw",
        }
    }
}

/// "one card to come" or the all-in flop's two-card wording.
fn cards_to_come(street: HitStreet) -> &'static str {
    match street {
        HitStreet::Turn => "one card to come",
        HitStreet::Flop => "all-in on the flop, two cards to come",
    }
}

fn draw_share(kind: DrawKind, street: HitStreet) -> f64 {
    hit_chance(kind.outs(), street, street == HitStreet::Flop)
}

/// The share of a stated hand, in percent.
pub fn hand_share_points(hand: PriceHand) -> f64 {
    match hand {
        PriceHand::Read { share_pct } => f64::from(share_pct),
        PriceHand::Draw { kind, street } => 100.0 * draw_share(kind, street),
    }
}

/// What it costs to call this bet, in percent of the money that pot then holds.
pub fn price_points(pot_before: u32, bet: u32, callers: u32) -> f64 {
    100.0 * money::break_even(f64::from(pot_with_bet(pot_before, bet, callers)), f64::from(bet))
}

/// What the opponent needs to call a bet of `bet` into `pot_before`, in percent.
pub fn opponent_price_points(pot_before: u32, bet: u32) -> f64 {
    price_points(pot_before, bet, 0)
}

/// The share hero has at rungs 2-3, in percent. `None` at the other rungs.
pub fn question_share_points(q: &ReadPriceQuestion) -> Option<f64> {
    match q.ask {
        ReadPriceAsk::Stated(hand) => Some(hand_share_points(hand)),
        ReadPriceAsk::Recall { kind, street } => Some(100.0 * draw_share(kind, street)),
        _ => None,
    }
}

/// The right call at rungs 2-3. `None` at the other rungs and for an edge in the gap.
pub fn question_call(q: &ReadPriceQuestion) -> Option<PriceCall> {
    question_share_points(q).and_then(|share| call_from_edge(share - price_points(q.pot_before, q.bet, q.callers)))
}

/// The band of the price at rungs 0-1, ready to grade against.
pub fn question_band(q: &ReadPriceQuestion) -> PriceBand {
    PriceBand::from_pct(price_points(q.pot_before, q.bet, q.callers))
}

/// The smallest of rung 4's options that makes the opponent's call a mistake.
pub fn smallest_mistake(pot_before: u32, kind: DrawKind, options: &[u32; 4]) -> Option<usize> {
    let share = 100.0 * draw_share(kind, HitStreet::Turn);
    (0..4).filter(|&i| opponent_price_points(pot_before, options[i]) > share).min_by_key(|&i| options[i])
}

/// `"$8"`, for dollars up to [`MAX_OPTION_DOLLARS`], without allocating per call.
pub fn dollar_label(amount: u32) -> &'static str {
    static LABELS: OnceLock<Vec<String>> = OnceLock::new();
    let labels = LABELS.get_or_init(|| (0..=MAX_OPTION_DOLLARS).map(|n| format!("${n}")).collect());
    &labels[amount as usize]
}

/// One attempt's rung, from its seed. `None` for a frozen baseline row.
fn rung_of(attempt: &Attempt) -> Option<u8> {
    match attempt.item {
        ItemRef::Seed(seed) => Some(read_price_rung_of_seed(seed)),
        ItemRef::Baseline(_) => None,
    }
}

/// The player's rung: promoted past a rung once any [`READ_PRICE_PROMOTE_WINDOW`]
/// straight drill attempts there were at least [`READ_PRICE_PROMOTE_PCT`] correct.
pub fn read_price_rung(attempts: &[Attempt]) -> u8 {
    let mut rung = 0;
    while rung + 1 < READ_PRICE_RUNGS {
        let results = rung_results(attempts, rung);
        let promoted = results.windows(READ_PRICE_PROMOTE_WINDOW).any(|window| {
            100.0 * window.iter().filter(|&&right| right).count() as f64 / window.len() as f64 >= READ_PRICE_PROMOTE_PCT
        });
        if !promoted {
            break;
        }
        rung += 1;
    }
    rung
}

fn rung_results(attempts: &[Attempt], rung: u8) -> Vec<bool> {
    attempts
        .iter()
        .filter(|a| a.skill == Skill::ReadPrice && a.mode == Mode::Drill && rung_of(a) == Some(rung))
        .map(|a| a.correct)
        .collect()
}

/// Where a player stands: the rung, and how many of the recent attempts there
/// (at most [`READ_PRICE_PROMOTE_WINDOW`]) were right.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ReadPriceStatus {
    pub rung: u8,
    pub recent_right: usize,
    pub recent_total: usize,
}

pub fn read_price_status(attempts: &[Attempt]) -> ReadPriceStatus {
    let rung = read_price_rung(attempts);
    let results = rung_results(attempts, rung);
    let recent = &results[results.len().saturating_sub(READ_PRICE_PROMOTE_WINDOW)..];
    ReadPriceStatus { rung, recent_right: recent.iter().filter(|&&right| right).count(), recent_total: recent.len() }
}

/// True for the attempts that count toward the pass mark: rungs 2-4, the skill
/// with a hand and with the player's own bet.
pub fn counts_for_read_price_mark(attempt: &Attempt) -> bool {
    rung_of(attempt).is_some_and(|rung| rung >= 2)
}

/// A rung's name, shown in Progress.
pub fn rung_label(rung: u8) -> &'static str {
    match rung {
        0 => "Cheap, fair or expensive",
        1 => "The number",
        2 => "With a hand, share stated",
        3 => "With a hand, recalled",
        _ => "Set the price",
    }
}

fn gen_pot(rng: &mut Rng) -> u32 {
    let total: u64 = POT_BUCKETS.iter().map(|b| b.2).sum();
    let mut roll = rng.below(total);
    for (low, high, weight) in POT_BUCKETS {
        if roll < weight {
            return rng.range(low, high) as u32;
        }
        roll -= weight;
    }
    unreachable!("the roll is below the total weight")
}

/// The largest bet this item may use: most items stay near pot-sized, some overbet.
fn bet_cap(rng: &mut Rng, pot_before: u32) -> u32 {
    let cap = if rng.chance(OVERBET_PERCENT, 100) { pot_before * 5 / 2 } else { pot_before * 5 / 4 };
    cap.max(1)
}

/// Generates the question at the seed's rung.
pub fn gen_read_price(rng: &mut Rng, seed: u64) -> ReadPriceQuestion {
    match read_price_rung_of_seed(seed) {
        0 => gen_band(rng),
        1 => gen_number(rng),
        2 => gen_with_hand(rng, false),
        3 => gen_with_hand(rng, true),
        _ => gen_set_price(rng),
    }
}

fn clear_of_band_lines(pct: f64) -> bool {
    (pct - CHEAP_BELOW_PCT).abs() >= BAND_GAP_POINTS && (pct - FAIR_BELOW_PCT).abs() >= BAND_GAP_POINTS
}

fn gen_band(rng: &mut Rng) -> ReadPriceQuestion {
    let target = PriceBand::ALL[rng.below(3) as usize];
    let callers_percent = if target == PriceBand::Expensive { 0 } else { BAND_CALLERS_PERCENT };
    let want_callers = rng.chance(callers_percent, 100);
    let callers_matter = want_callers && rng.chance(CALLERS_MATTER_PERCENT, 100);
    loop {
        let pot_before = gen_pot(rng);
        let callers = if want_callers { rng.range(1, 2) as u32 } else { 0 };
        let cap = bet_cap(rng, pot_before);
        let fits: Vec<u32> = (1..=cap)
            .filter(|&bet| {
                let pct = price_points(pot_before, bet, callers);
                let ignored = price_points(pot_before, bet, 0);
                clear_of_band_lines(pct)
                    && PriceBand::from_pct(pct) == target
                    && (!callers_matter || (clear_of_band_lines(ignored) && PriceBand::from_pct(ignored) != target))
            })
            .collect();
        if !fits.is_empty() {
            let bet = rng.pick(&fits);
            return ReadPriceQuestion { pot_before, bet, callers, ask: ReadPriceAsk::Band };
        }
    }
}

fn gen_number(rng: &mut Rng) -> ReadPriceQuestion {
    let want_callers = rng.chance(CALLERS_PERCENT, 100);
    let callers_matter = want_callers && rng.chance(CALLERS_MATTER_PERCENT, 100);
    loop {
        let pot_before = gen_pot(rng);
        let callers = if want_callers { rng.range(1, 2) as u32 } else { 0 };
        let cap = bet_cap(rng, pot_before);
        let fits: Vec<u32> = (1..=cap)
            .filter(|&bet| {
                let gap = price_points(pot_before, bet, 0) - price_points(pot_before, bet, callers);
                !callers_matter || gap > PERCENT_TOLERANCE
            })
            .collect();
        if !fits.is_empty() {
            let bet = rng.pick(&fits);
            return ReadPriceQuestion { pot_before, bet, callers, ask: ReadPriceAsk::Number };
        }
    }
}

/// The class this edge counts as when dealt: a little inside the grading lines.
fn dealt_call(edge_points: f64) -> Option<PriceCall> {
    if edge_points >= EASY_POINTS + DEAL_MARGIN_POINTS {
        Some(PriceCall::Easy)
    } else if edge_points <= -(EASY_POINTS + DEAL_MARGIN_POINTS) {
        Some(PriceCall::Fold)
    } else if edge_points.abs() <= CLOSE_POINTS - DEAL_MARGIN_POINTS {
        Some(PriceCall::Close)
    } else {
        None
    }
}

/// True when pricing this bet without the callers lands clearly on another answer.
fn ignoring_callers_changes_call(share: f64, pot_before: u32, bet: u32, target: PriceCall) -> bool {
    call_from_edge(share - price_points(pot_before, bet, 0)).is_some_and(|call| call != target)
}

fn gen_with_hand(rng: &mut Rng, draws_only: bool) -> ReadPriceQuestion {
    let target = PriceCall::ALL[rng.below(3) as usize];
    let by_read = !draws_only && rng.chance(1, 2);
    let want_callers = rng.chance(CALLERS_PERCENT, 100);
    // A fold stays a fold when the price is overstated, so only the other two can flip.
    let callers_matter = want_callers && target != PriceCall::Fold && rng.chance(CALLERS_MATTER_PERCENT, 100);
    loop {
        let hand = if by_read {
            PriceHand::Read { share_pct: rng.range(3, 14) as u32 * 5 }
        } else {
            let street = if rng.chance(1, 2) { HitStreet::Turn } else { HitStreet::Flop };
            PriceHand::Draw { kind: rng.pick(&DrawKind::ALL), street }
        };
        let pot_before = gen_pot(rng);
        let callers = if want_callers { rng.range(1, 2) as u32 } else { 0 };
        let cap = bet_cap(rng, pot_before);
        let share = hand_share_points(hand);
        let fits: Vec<u32> = (1..=cap)
            .filter(|&bet| {
                dealt_call(share - price_points(pot_before, bet, callers)) == Some(target)
                    && (!callers_matter || ignoring_callers_changes_call(share, pot_before, bet, target))
            })
            .collect();
        if !fits.is_empty() {
            let bet = rng.pick(&fits);
            let ask = match (draws_only, hand) {
                (true, PriceHand::Draw { kind, street }) => ReadPriceAsk::Recall { kind, street },
                _ => ReadPriceAsk::Stated(hand),
            };
            return ReadPriceQuestion { pot_before, bet, callers, ask };
        }
    }
}

fn gen_set_price(rng: &mut Rng) -> ReadPriceQuestion {
    loop {
        let kind = rng.pick(&DrawKind::ALL);
        let pot_before = gen_pot(rng);
        let share = 100.0 * draw_share(kind, HitStreet::Turn);
        let cap = (pot_before * 5 / 2).max(1);
        let price = |bet: u32| opponent_price_points(pot_before, bet);
        let Some(right) = (1..=cap).find(|&bet| price(bet) >= share + OPTION_GAP_POINTS) else { continue };
        let mut lows: Vec<u32> =
            (1..right).filter(|&bet| price(bet) <= share - OPTION_GAP_POINTS && price(bet) >= share - OPTION_BAND_POINTS).collect();
        let mut highs: Vec<u32> = (right + 1..=cap).filter(|&bet| price(bet) <= share + OPTION_BAND_POINTS).collect();
        // Only a pot with enough bets near break-even on both sides is dealt, so every
        // position is possible for every pot and the right answer can sit anywhere.
        if lows.len() < 3 || highs.len() < 3 {
            continue;
        }
        let below_count = rng.below(4) as usize;
        rng.shuffle(&mut lows);
        rng.shuffle(&mut highs);
        let mut options = Vec::with_capacity(4);
        options.extend_from_slice(&lows[..below_count]);
        options.push(right);
        options.extend_from_slice(&highs[..3 - below_count]);
        options.sort_unstable();
        let options = [options[0], options[1], options[2], options[3]];
        return ReadPriceQuestion { pot_before, bet: 0, callers: 0, ask: ReadPriceAsk::SetPrice { kind, options } };
    }
}

/// What the player reads before answering: the pot, the bet, any callers, the hand.
pub fn context_lines(q: &ReadPriceQuestion) -> Vec<String> {
    if let ReadPriceAsk::SetPrice { kind, .. } = q.ask {
        return vec![
            format!("You are betting into a pot of ${} against one opponent.", q.pot_before),
            format!("They have {}, one card to come. Assume their outs win and nothing else does.", kind.with_article()),
        ];
    }
    let callers = match q.callers {
        0 => String::new(),
        1 => format!(" 1 player has already called the ${}.", q.bet),
        n => format!(" {n} players have already called the ${}.", q.bet),
    };
    let mut lines = vec![format!("The pot was ${}. Someone bets ${}.{callers}", q.pot_before, q.bet)];
    match q.ask {
        ReadPriceAsk::Stated(PriceHand::Read { share_pct }) => {
            lines.push(format!("You figure you're ahead about {share_pct}% of the time."));
        }
        ReadPriceAsk::Stated(PriceHand::Draw { kind, street }) => {
            lines.push(format!("{}, {}: about {:.0}%. Assume your outs win and nothing else does.", kind.label(), cards_to_come(street), 100.0 * draw_share(kind, street)));
        }
        ReadPriceAsk::Recall { kind, street } => {
            lines.push(format!("{}, {}. Assume your outs win and nothing else does.", kind.label(), cards_to_come(street)));
        }
        _ => {}
    }
    lines
}

/// The question at each rung.
pub fn prompt_text(q: &ReadPriceQuestion) -> String {
    match q.ask {
        ReadPriceAsk::Band => "Is the price cheap, fair or expensive?".to_string(),
        ReadPriceAsk::Number => "What equity do you need to call?".to_string(),
        ReadPriceAsk::Stated(_) | ReadPriceAsk::Recall { .. } => "Easy call, close or fold?".to_string(),
        ReadPriceAsk::SetPrice { .. } => "What is the smallest of these bets that makes their call a mistake, ignoring later bets?".to_string(),
    }
}

/// A whole percent for a reveal: a price that rounds to 0% still costs something.
pub fn whole_percent(pct: f64) -> String {
    if pct < 0.5 {
        "under 1%".to_string()
    } else {
        format!("{pct:.0}%")
    }
}

/// `"Call $5 to win $19: 5 ÷ 24 = 21%."`, spelling out where the winnings come from when there are callers.
fn call_sum_line(q: &ReadPriceQuestion, pct_text: &str) -> String {
    let win = pot_with_bet(q.pot_before, q.bet, q.callers);
    let parts = if q.callers > 0 {
        let who = if q.callers == 1 { "the caller".to_string() } else { format!("{} callers", q.callers) };
        format!(" (the ${} pot, the ${} bet and ${} from {who})", q.pot_before, q.bet, q.bet * q.callers)
    } else {
        String::new()
    };
    format!("Call ${} to win ${win}{parts}: {} \u{f7} {} = {pct_text}.", q.bet, q.bet, win + q.bet)
}

/// The sentence in a rung 2-3 reveal that says how far the share is from the price.
fn verdict_sentence(call: PriceCall, share_text: String) -> String {
    match call {
        PriceCall::Easy => format!("{share_text}, at least {EASY_POINTS:.0} points above what you need, so it's an easy call."),
        PriceCall::Close => format!("{share_text}, within {CLOSE_POINTS:.0} points of what you need, so it's close."),
        PriceCall::Fold => format!("{share_text}, at least {EASY_POINTS:.0} points below what you need, so fold."),
    }
}

/// Why a wrong rung 4 option was wrong: too small to be a mistake, or bigger than needed.
fn wrong_pick_line(pot_before: u32, chosen: u32, right: u32, share: f64) -> String {
    let asked = opponent_price_points(pot_before, chosen);
    if asked > share {
        format!("${chosen} also makes their call a mistake, but ${right} is the smallest of these that does.")
    } else {
        format!("${chosen} only asks them for {}, and their hand gets there about {share:.0}%, so they can still call.", whole_percent(asked))
    }
}

/// The right answer in words and the reveal's lines, kept to a line or two.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ReadPriceReveal {
    pub answer: String,
    pub lines: Vec<String>,
}

/// What the reveal shows for a question. `picked` is the option chosen at rung
/// 4, so a wrong pick can be explained; the other rungs read the same for any answer.
pub fn reveal_for(q: &ReadPriceQuestion, picked: Option<usize>) -> ReadPriceReveal {
    let price = price_points(q.pot_before, q.bet, q.callers);
    match q.ask {
        ReadPriceAsk::Band => ReadPriceReveal {
            answer: question_band(q).label().to_string(),
            lines: vec![
                call_sum_line(q, &whole_percent(price)),
                "Shortcut: compare the bet to the pot before it, with any callers added to the pot first. Under a third of the pot is cheap, a third to three-quarters is fair, more is expensive.".to_string(),
            ],
        },
        ReadPriceAsk::Number => ReadPriceReveal {
            answer: format!("{price:.1}%"),
            lines: vec![call_sum_line(q, &format!("{price:.1}%"))],
        },
        ReadPriceAsk::Stated(hand) => {
            let call = question_call(q).unwrap_or(PriceCall::Close);
            let share_text = match hand {
                PriceHand::Read { share_pct } => format!("You figure about {share_pct}%"),
                PriceHand::Draw { .. } => format!("You have about {:.0}%", hand_share_points(hand)),
            };
            ReadPriceReveal {
                answer: call.label().to_string(),
                lines: vec![call_sum_line(q, &whole_percent(price)), verdict_sentence(call, share_text)],
            }
        }
        ReadPriceAsk::Recall { kind, street } => {
            let call = question_call(q).unwrap_or(PriceCall::Close);
            let share = 100.0 * draw_share(kind, street);
            let cards = match street {
                HitStreet::Turn => "one card to come",
                HitStreet::Flop => "two cards to come",
            };
            let share_text = format!("{}: {} outs, about {share:.0}% with {cards}", kind.label(), kind.outs());
            ReadPriceReveal {
                answer: call.label().to_string(),
                lines: vec![call_sum_line(q, &whole_percent(price)), verdict_sentence(call, share_text)],
            }
        }
        ReadPriceAsk::SetPrice { kind, options } => {
            let index = smallest_mistake(q.pot_before, kind, &options).unwrap_or(0);
            let bet = options[index];
            let need = opponent_price_points(q.pot_before, bet);
            let share = 100.0 * draw_share(kind, HitStreet::Turn);
            let mut lines = vec![format!(
                "They call ${bet} to win ${}: they need {}. Their hand gets there about {share:.0}% of the time, so calling is a mistake, ignoring later bets.",
                q.pot_before + bet,
                whole_percent(need)
            )];
            if let Some(chosen) = picked.filter(|&p| p != index).map(|p| options[p]) {
                lines.push(wrong_pick_line(q.pot_before, chosen, bet, share));
            }
            lines.push("Real opponents often call anyway, which still pays you because at this price they are paying too much.".to_string());
            ReadPriceReveal { answer: format!("Bet ${bet}"), lines }
        }
    }
}

/// Family key for a `read_price` item: the rung, then its answer class or draw.
pub fn read_price_family(q: &ReadPriceQuestion) -> String {
    match q.ask {
        ReadPriceAsk::Band => format!("0:{}", question_band(q).word()),
        ReadPriceAsk::Number => format!("1:{}", question_band(q).word()),
        ReadPriceAsk::Stated(_) => format!("2:{}", question_call(q).unwrap_or(PriceCall::Close).word()),
        ReadPriceAsk::Recall { kind, .. } => format!("3:{}", kind.code()),
        ReadPriceAsk::SetPrice { kind, .. } => format!("4:{}", kind.code()),
    }
}
