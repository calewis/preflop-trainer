//! All terminal text: prompts, reveals and reports.

use poker_trainer::anchors;
use poker_trainer::answer::{answer_format, AnswerFormat, AnswerOption};
use poker_trainer::cards::{rank_char, Card, Holding};
use poker_trainer::checkup::{CheckupMetric, CheckupScore};
use poker_trainer::equity_skill::{EquityBias, BIAS_MIN_ATTEMPTS};
use poker_trainer::grade::{EquityAnalysis, EvBand, Outcome, Reveal};
use poker_trainer::item::{EquityOpponents, EquityQuestion, HitStreet, Item, OpponentLabel, Question, Skill, SpotMode, SpotQuestion};
use poker_trainer::preflop::{action_label, Action, PreflopReason, Seat, Range, Situation, CARD};
use poker_trainer::progress::{pass_mark, BaselineRun, SkillStats, PASS_ATTEMPTS, STEPS};
use poker_trainer::table::{bb_text, checkup_form_reuses_spots, ev_bb_text, REUSE_NOTE, TableAction, TableActionKind, TableGrade, TableQuestion, TableSeat, TableStreetName, Verdict, ASSUMPTION_NOTE};

#[derive(Clone, Copy)]
pub struct Style {
    pub color: bool,
}

const RESET: &str = "\x1b[0m";
const SUIT_SYMBOLS: [char; 4] = ['♠', '♥', '♦', '♣'];
const SUIT_COLORS: [&str; 4] = ["\x1b[1m", "\x1b[31m", "\x1b[34m", "\x1b[32m"];

impl Style {
    pub fn card(self, card: Card) -> String {
        let rank = rank_char(card.rank());
        let suit = SUIT_SYMBOLS[card.suit() as usize];
        if self.color {
            format!("{}{rank}{suit}{RESET}", SUIT_COLORS[card.suit() as usize])
        } else {
            format!("{rank}{suit}")
        }
    }

    pub fn cards(self, cards: &[Card]) -> String {
        cards.iter().map(|&c| self.card(c)).collect::<Vec<_>>().join(" ")
    }

    pub fn holding(self, holding: Holding) -> String {
        format!("{}{}", self.card(holding[0]), self.card(holding[1]))
    }

    /// A short list of holdings: at most `limit`, then how many more.
    fn holdings(self, list: &[Holding], limit: usize) -> String {
        let shown: Vec<String> = list.iter().take(limit).map(|&h| self.holding(h)).collect();
        let mut text = shown.join(", ");
        if list.len() > limit {
            text.push_str(&format!(", and {} more", list.len() - limit));
        }
        text
    }
}

fn pct(value: f64) -> String {
    format!("{:.1}%", value * 100.0)
}

fn money(value: f64) -> String {
    if value < 0.0 {
        format!("-${:.2}", -value)
    } else {
        format!("${value:.2}")
    }
}

fn holdings_word(count: usize) -> &'static str {
    if count == 1 {
        "holding"
    } else {
        "holdings"
    }
}

fn needed_equity_line(pot: u32, call: u32, required: f64) -> String {
    format!(
        "Needed equity = call / (pot + call) = {call} / ({pot} + {call}) = {}. The pot already includes the bet.",
        pct(required)
    )
}

fn street_name(board_len: usize) -> &'static str {
    match board_len {
        3 => "flop",
        4 => "turn",
        _ => "river",
    }
}

fn seat_name(seat: Seat) -> &'static str {
    match seat {
        Seat::Early => "in early position",
        Seat::Middle => "in middle position",
        Seat::Cutoff => "in the cutoff",
        Seat::Button => "on the button",
        Seat::SmallBlind => "in the small blind",
        Seat::BigBlind => "in the big blind",
    }
}

fn players_word(n: u8) -> String {
    if n == 1 {
        "1 player".to_string()
    } else {
        format!("{n} players")
    }
}

/// The question text, ready to print above the answer prompt.
pub fn question(item: &Item, style: Style) -> String {
    match &item.question {
        Question::Preflop { players, behind, situation, hand } => {
            let seat = Seat::from_behind(*behind);
            let spot = match situation {
                Situation::FoldedTo => "It is folded to you.".to_string(),
                Situation::Limpers(1) => "1 player has limped in front of you.".to_string(),
                Situation::Limpers(n) => format!("{n} players have limped in front of you."),
                Situation::Raised => "Someone has raised in front of you.".to_string(),
            };
            format!(
                "Table of {players}. You are {} with {} still to act behind you.\nYour hand: {}\n{spot}",
                seat_name(seat),
                players_word(*behind),
                style.holding(*hand)
            )
        }
        Question::Nuts { board } => format!(
            "Board: {}\nName the best possible holding right now.",
            style.cards(board)
        ),
        Question::Possible { board } => format!(
            "Board: {}\nWhich can someone hold right now with two hole cards?",
            style.cards(board)
        ),
        Question::Made { hero, board } => format!(
            "Your hand: {}\nBoard: {}\nWhat do you have?",
            style.holding(*hero),
            style.cards(board)
        ),
        Question::Beats { hero, board } => format!(
            "Your hand: {}\nBoard: {}\nWhat share of all unseen 2-card holdings are strictly ahead of you right now?",
            style.holding(*hero),
            style.cards(board)
        ),
        Question::Outs { hero, board, villain } => format!(
            "Your hand: {}\nBoard: {}\nOpponent shows: {}\nYou are behind. How many next cards leave you strictly ahead?",
            style.holding(*hero),
            style.cards(board),
            style.holding(*villain)
        ),
        Question::Price { pot_before, bet, callers } => {
            let callers_text = match callers {
                0 => String::new(),
                1 => format!(" 1 player has already called the ${bet}."),
                n => format!(" {n} players have already called the ${bet}."),
            };
            format!(
                "The pot was ${pot_before}. Someone bets ${bet}.{callers_text}\nWhat equity do you need to call?"
            )
        }
        Question::Hit { outs, street, both } => {
            let cards = if *both {
                "All the money is in, so both remaining cards are coming."
            } else {
                "One card is coming."
            };
            let name = match street {
                HitStreet::Flop => "flop",
                HitStreet::Turn => "turn",
            };
            format!("You have {outs} outs on the {name}. {cards}\nWhat is your chance of hitting?")
        }
        Question::Spot(q) => spot_question(q, style),
        Question::Equity(q) => equity_question(q, style),
        Question::Anchor { card } => anchors::deck()[usize::from(*card)].prompt.clone(),
        Question::BoardType { board, .. } => {
            format!("Flop: {}\nName its suit pattern and rank pattern.", style.cards(board))
        }
        Question::Table(q) => table_question(q, style),
    }
}

fn table_seat_name(seat: TableSeat, hero: TableSeat) -> &'static str {
    if seat == hero {
        "You"
    } else {
        seat.title()
    }
}

fn table_action_text(a: &TableAction, hero: TableSeat) -> String {
    let amount = bb_text(a.amount_cents);
    let you = a.seat == hero;
    let verb = |plain: &str, s: &str| if you { plain.to_string() } else { s.to_string() };
    let what = match a.kind {
        TableActionKind::Fold => verb("fold", "folds"),
        TableActionKind::Check => verb("check", "checks"),
        TableActionKind::Call => format!("{} {amount}bb", verb("call", "calls")),
        TableActionKind::Bet => format!("{} {amount}bb", verb("bet", "bets")),
        TableActionKind::Raise => format!("{} to {amount}bb", verb("raise", "raises")),
    };
    format!("{} {what}", table_seat_name(a.seat, hero))
}

fn table_question(q: &TableQuestion, style: Style) -> String {
    let mut lines = vec![
        format!("{} players, {}bb stacks, {}bb behind you. You are {}.", q.players, q.stack_bb, bb_text(q.behind_cents), q.seat.sentence()),
        format!("Your hand: {}", style.holding(q.hero)),
    ];
    for s in &q.streets {
        let cards = q.street_cards(s.street);
        let head = match s.street {
            TableStreetName::Preflop => "Preflop".to_string(),
            _ => format!("{}: {} (pot {}bb)", s.street.title(), style.cards(cards), bb_text(s.pot_start_cents)),
        };
        let actions: Vec<String> = s.actions.iter().map(|a| table_action_text(a, q.seat)).collect();
        if actions.is_empty() {
            lines.push(head);
        } else {
            lines.push(format!("{head}: {}", actions.join(", ")));
        }
    }
    let call = if q.call_cents > 0 { format!(", {}bb to call", bb_text(q.call_cents)) } else { String::new() };
    lines.push(format!("Pot {}bb{call}. What do you do?", bb_text(q.pot_cents)));
    lines.join("\n")
}

fn equity_question(q: &EquityQuestion, style: Style) -> String {
    let mut lines = vec![
        format!("Your hand: {}", style.holding(q.hero)),
        format!("Board: {} ({})", style.cards(&q.board), street_name(q.board.len())),
    ];
    match &q.opponents {
        EquityOpponents::Exact(hands) if hands.len() == 1 => {
            lines.push(format!("Opponent shows: {}", style.holding(hands[0])));
        }
        EquityOpponents::Exact(hands) => {
            for (i, &hand) in hands.iter().enumerate() {
                lines.push(format!("Opponent {} shows: {}", i + 1, style.holding(hand)));
            }
        }
        EquityOpponents::Classes(classes) if classes.len() == 1 => {
            lines.push(format!("Assume they have {}.", classes[0].description()));
        }
        EquityOpponents::Classes(classes) => {
            for (i, class) in classes.iter().enumerate() {
                lines.push(format!("Assume opponent {} has {}.", i + 1, class.description()));
            }
        }
    }
    lines.push(format!("All-in, all remaining cards run out. What is your equity? Within {} points counts.", q.band));
    lines.join("\n")
}

fn spot_question(q: &SpotQuestion, style: Style) -> String {
    let mut lines = vec![
        format!("Your hand: {}", style.holding(q.hero)),
        format!("Board: {} ({})", style.cards(&q.board), street_name(q.board.len())),
        format!(
            "Pot, including the bet{}: ${}. It costs you ${} to call.",
            if q.opponents.len() > 1 { " and earlier callers" } else { "" },
            q.pot,
            q.call
        ),
    ];
    if q.opponents.len() == 1 {
        lines.push(format!("Assume they have {}.", q.opponents[0].description()));
    } else {
        lines.push(format!(
            "{} opponents. Everyone else has acted and nobody can raise.",
            q.opponents.len()
        ));
        for (i, &class) in q.opponents.iter().enumerate() {
            lines.push(format!("Assume opponent {} has {}.", i + 1, class.description()));
        }
    }
    match q.mode {
        SpotMode::AllIn => lines.push(
            "All-in: there is no more betting and all remaining cards run out.".to_string(),
        ),
        SpotMode::NextCard { x } => {
            let label = match (q.opponent_label(), q.opponents.len() > 1) {
                (OpponentLabel::Tight, false) => "a tight player",
                (OpponentLabel::Average, false) => "an average player",
                (OpponentLabel::CallingStation, false) => "a calling station",
                (OpponentLabel::Tight, true) => "tight players",
                (OpponentLabel::Average, true) => "average players",
                (OpponentLabel::CallingStation, true) => "calling stations",
            };
            lines.push(format!(
                "More betting is coming. You see one more card. If you then have the best hand you collect about ${x} more; otherwise you give up. You are up against {label}."
            ));
        }
    }
    lines.join("\n")
}

/// The answer format reminder shown on the prompt line.
pub fn answer_hint(item: &Item) -> String {
    let joined = |options: &[AnswerOption]| -> String {
        options.iter().map(|o| format!("{} {}", o.code, o.label)).collect::<Vec<_>>().join(", ")
    };
    match answer_format(&item.question) {
        // A long list of codes only fits on the prompt line without its labels.
        AnswerFormat::Choice(options) if options.len() > 6 => {
            options.iter().map(|o| o.code).collect::<Vec<_>>().join(" ")
        }
        AnswerFormat::Choice(options) => joined(&options),
        AnswerFormat::MultiSelect { options, none_code } => {
            format!("any of {}, or {none_code}", joined(&options))
        }
        AnswerFormat::Integer { .. } => "whole number".to_string(),
        AnswerFormat::Percent => "percent, such as 25".to_string(),
        AnswerFormat::Holding => "QJ, AKs, AhKh, Ahx or Ax".to_string(),
    }
}

fn verdict(outcome: &Outcome) -> &'static str {
    if outcome.correct {
        "Correct."
    } else if outcome.close {
        "Close."
    } else {
        "Not quite."
    }
}

/// The teaching text after an answer.
pub fn reveal(outcome: &Outcome, style: Style) -> String {
    let mut lines = vec![verdict(outcome).to_string()];
    match &outcome.reveal {
        Reveal::Preflop { seat, situation, action, reason } => {
            preflop_reveal(&mut lines, *seat, *situation, *action, *reason);
        }
        Reveal::Nuts { analysis, used_x, consistent, consistent_nuts } => {
            lines.push(format!(
                "The nuts: {}, held by {}.",
                analysis.top.describe(),
                style.holdings(&analysis.top_holdings, 6)
            ));
            if let Some((rank, holdings)) = &analysis.next {
                lines.push(format!(
                    "Next best: {}, held by {}.",
                    rank.describe(),
                    style.holdings(holdings, 6)
                ));
            }
            let rule = if *used_x {
                "With x, every holding you describe has to make the nuts"
            } else {
                "Without x, one holding you describe making the nuts is enough"
            };
            let verb = if *consistent_nuts == 1 { "makes" } else { "make" };
            lines.push(format!(
                "{rule}. Your answer covers {consistent} unseen {}, of which {consistent_nuts} {verb} the nuts.",
                holdings_word(*consistent)
            ));
        }
        Reveal::Possible(a) => {
            lines.push(format!("Answer: {}.", a.set.code()));
            match a.flush_example {
                Some(h) => lines.push(format!("Flush: yes, for example {}.", style.holding(h))),
                None => lines.push(format!(
                    "Flush: no. A flush needs three of one suit on the board because you hold just two cards, and no suit appears more than {}.",
                    if a.max_suit_on_board <= 1 { "once" } else { "twice" }
                )),
            }
            match a.straight_example {
                Some(h) => lines.push(format!("Straight: yes, for example {}.", style.holding(h))),
                None => lines.push(
                    "Straight: no. No run of five consecutive ranks (ace counts high or low) contains three board ranks.".to_string(),
                ),
            }
            match a.full_example {
                Some(h) => lines.push(format!("Full house or quads: yes, for example {}.", style.holding(h))),
                None => lines.push("Full house or quads: no. Without a pair on the board, two hole cards can only reach a set or two pair.".to_string()),
            }
        }
        Reveal::Made { class, rank } => {
            lines.push(format!("Answer: {}. {}", class.code(), class.description()));
            lines.push(format!("Your best five cards make {}.", rank.describe()));
        }
        Reveal::Beats(a) => {
            lines.push(format!(
                "Answer: {}. Strictly ahead right now: {} of {} unseen holdings, {:.1}%. You have {}.",
                a.bucket.code(),
                a.ahead,
                a.total,
                a.pct,
                a.hero_rank.describe()
            ));
            if a.kinds.is_empty() {
                lines.push("Nothing beats you right now.".to_string());
            } else {
                lines.push("Strongest holdings that beat you:".to_string());
                for kind in a.kinds.iter().take(3) {
                    lines.push(format!(
                        "  {}: {} {}, the best being {} ({}).",
                        kind.category.name(),
                        kind.count,
                        holdings_word(kind.count),
                        style.holding(kind.example),
                        kind.best.describe()
                    ));
                }
            }
        }
        Reveal::Outs { analysis, hero_rank, villain_rank } => {
            lines.push(format!(
                "Answer: {}. You have {} and they have {}. There are {} unseen cards.",
                analysis.outs.len(),
                hero_rank.describe(),
                villain_rank.describe(),
                analysis.unseen
            ));
            lines.push(format!("Outs: {}.", style.cards(&analysis.outs)));
            if !analysis.tempting.is_empty() {
                lines.push("Tempting non-outs (they improve you but don't put you ahead):".to_string());
                for t in analysis.tempting.iter().take(5) {
                    let tie = if t.ties { " (you would tie)" } else { "" };
                    lines.push(format!("  {} gives you {}{tie}.", style.card(t.card), t.new_rank.describe()));
                }
            }
        }
        Reveal::Price { pot, call, required } => {
            lines.push(needed_equity_line(*pot, *call, *required));
        }
        Reveal::Hit { outs, unseen, both, exact, shortcut } => {
            if *both {
                lines.push(format!(
                    "Exact: {}. That is 1 minus the chance of missing twice, ({unseen}-{outs})/{unseen} x ({}-{outs})/{}.",
                    pct(*exact),
                    unseen - 1,
                    unseen - 1
                ));
                lines.push(format!(
                    "Times-4 shortcut: {outs} x 4 = {shortcut:.0}%. Use it only when both cards are sure to come, as when all the money is in on the flop. It overshoots with many outs."
                ));
            } else {
                lines.push(format!("Exact: {outs}/{unseen} = {}.", pct(*exact)));
                lines.push(format!(
                    "Times-2 shortcut: {outs} x 2 = {shortcut:.0}%. Use it when one card is coming, or whenever more betting could stop you seeing the second card."
                ));
            }
        }
        Reveal::Spot { analysis, chosen, best, loss_pct, loss_money, band } => {
            let a = analysis;
            lines.push(format!("Best play: {}.", if *best == Action::Call { "call" } else { "fold" }));
            lines.push(needed_equity_line(a.pot, a.call, a.price));
            if a.x > 0 {
                lines.push(format!("With about ${} more to come, you need {}.", a.x, pct(a.required)));
            }
            lines.push(format!(
                "{} {}{}, averaged over {} possible combos.",
                if a.next_card { "Your share of the pot after the next card:" } else { "Your equity:" },
                pct(a.share),
                if a.sampled { " (estimated from a 300,000-outcome sample)" } else { "" },
                a.combos_per_opponent.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(" x ")
            ));
            lines.push(format!("EV of calling: {}. EV of folding: {}.", money(a.ev_call), money(a.ev_fold)));
            if *chosen == *best {
                lines.push("EV lost: $0.00. You chose the better action.".to_string());
            } else {
                let band_text = match band {
                    Some(EvBand::Close) => "close, so it counts as correct",
                    Some(EvBand::Small) => "a small error",
                    Some(EvBand::Mistake) => "a mistake",
                    Some(EvBand::Blunder) => "a blunder",
                    None => "",
                };
                lines.push(format!(
                    "EV lost: {} ({loss_pct:.1}% of the pot), {band_text}.",
                    money(*loss_money)
                ));
            }
            if a.next_card {
                match a.break_even_x {
                    Some(x) if x > 0.0 => lines.push(format!(
                        "You would need to win about ${x:.0} more when you hit for calling to break even."
                    )),
                    Some(_) => lines.push("Calling already breaks even without winning anything more.".to_string()),
                    None => {}
                }
            }
            lines.push("This decision was averaged over all possible remaining cards and combos, so the actual next card doesn't matter.".to_string());
        }
        Reveal::Equity { analysis, estimate_pct, signed_error, band, hit } => {
            equity_reveal(&mut lines, analysis, *estimate_pct, *signed_error, *band, *hit);
        }
        Reveal::Anchor { card, estimate_pct, signed_error } => {
            lines.push(format!(
                "Answer: {:.1}%. You said {estimate_pct}% ({signed_error:+.1} points). Within {} points counts.",
                card.answer_pct, card.tolerance
            ));
        }
        Reveal::BoardType { category, chosen, too_slow } => {
            lines.push(format!("Answer: {}. You chose {}.", category.label(), chosen.label()));
            if *too_slow {
                lines.push("Too slow: answers over 12 seconds count as misses even when right.".to_string());
            }
        }
        Reveal::Table(graded) => table_reveal(&mut lines, graded),
    }
    lines.join("\n")
}

fn table_reveal(lines: &mut Vec<String>, g: &TableGrade) {
    let spot = g.spot;
    lines.push(spot.verdict.explanation().to_string());
    lines.push(format!("Best: {}. You chose {}.", g.best().label, g.chosen_option().label));
    for option in &spot.options {
        lines.push(format!(
            "  {}: {}bb, {:.1}% of the pot, best at {:.1}% of simulated tables",
            option.label,
            ev_bb_text(option.ev_milli_bb),
            f64::from(option.ev_pct_tenths) / 10.0,
            f64::from(option.best_share_milli) / 10.0,
        ));
    }
    if g.chosen != spot.best {
        let band = match g.band {
            Some(EvBand::Close) => ", close, so it counts as correct",
            Some(EvBand::Small) => ", a small error",
            Some(EvBand::Mistake) => ", a mistake",
            Some(EvBand::Blunder) => ", a blunder",
            None => ", a fair read",
        };
        lines.push(format!("You gave up {}{band}.", g.loss_text()));
    }
    if let Some(note) = g.read_note() {
        lines.push(note);
    }
    if spot.verdict == Verdict::Depends {
        for row in &spot.archetypes {
            lines.push(format!("  {}", row.sentence(spot)));
        }
    }
    lines.push(ASSUMPTION_NOTE.to_string());
}

fn equity_reveal(
    lines: &mut Vec<String>,
    a: &EquityAnalysis,
    estimate_pct: f64,
    signed_error: f64,
    band: u8,
    hit: bool,
) {
    lines.push(format!(
        "Exact equity: {:.1}%{}. You said {estimate_pct}% ({signed_error:+.1} points).",
        a.truth_pct,
        if a.sampled { " (estimated from a 300,000-outcome sample)" } else { "" }
    ));
    lines.push(format!("Band of {band} points: {}.", if hit { "hit" } else { "miss" }));
    let combos: Vec<String> = a.combos_per_opponent.iter().map(|n| n.to_string()).collect();
    lines.push(format!("Averaged over {} combos.", combos.join(" x ")));
}

fn preflop_reveal(lines: &mut Vec<String>, seat: Seat, situation: Situation, action: Action, reason: PreflopReason) {
    lines.push(format!("Answer: {}.", action_label(action, seat, situation)));
    let open_line = CARD.open_range(seat).map_or("", |l| l.text);
    let open_name = opening_range_name(seat);
    let (raise_iso, limp) = (CARD.iso_raise.text, CARD.iso_limp.text);
    let (reraise, call) = (CARD.reraise.text, CARD.call.text);
    let why = match reason {
        PreflopReason::InOpen => format!("The hand is in {open_name}: {open_line}."),
        PreflopReason::NotInOpen => format!("The hand is not in {open_name}: {open_line}."),
        PreflopReason::BigBlindNoRaise => plain_note(CARD.note("note.big_blind")),
        PreflopReason::IsoRaise => format!("With limpers, raise hands in {RAISE_ISO_NAME}: {raise_iso}."),
        PreflopReason::IsoLimp => format!(
            "The hand is in {LIMP_NAME}: {limp}. You may limp along from the cutoff, button or small blind, or when two or more have limped."
        ),
        PreflopReason::IsoLimpNeedsLateOrTwoLimpers => format!(
            "The hand is in {LIMP_NAME} ({limp}), but you only limp along from the cutoff, button or small blind, or when two or more have limped."
        ),
        PreflopReason::NotInIso => format!(
            "The hand is in neither {RAISE_ISO_NAME} ({raise_iso}) nor {LIMP_NAME} ({limp})."
        ),
        PreflopReason::BigBlindIsoRaise => format!(
            "In the big blind with limpers, raise hands in {RAISE_ISO_NAME}: {raise_iso}."
        ),
        PreflopReason::BigBlindCheck => format!(
            "In the big blind with limpers, check anything outside {RAISE_ISO_NAME} ({raise_iso}). Checking is free."
        ),
        PreflopReason::Reraise => format!("Facing a raise, reraise hands in {RERAISE_NAME}: {reraise}."),
        PreflopReason::FaceRaiseCall => format!("Facing a raise, call hands in {CALL_NAME}: {call}."),
        PreflopReason::NotInFaceRaise => format!(
            "Facing a raise, the hand is in neither {RERAISE_NAME} ({reraise}) nor {CALL_NAME} ({call})."
        ),
    };
    lines.push(why);
}

const RAISE_ISO_NAME: &str = "the raise-over-limpers range";
const LIMP_NAME: &str = "the limp-along range";
const RERAISE_NAME: &str = "the reraise range";
const CALL_NAME: &str = "the calling range";

fn opening_range_name(seat: Seat) -> &'static str {
    match seat {
        Seat::Early => "your early-position opening range",
        Seat::Middle => "your middle-position opening range",
        Seat::Cutoff => "your cutoff opening range",
        Seat::Button => "your button opening range",
        Seat::SmallBlind => "your small-blind opening range",
        Seat::BigBlind => "your big-blind options",
    }
}

fn seat_title(seat: Seat) -> &'static str {
    match seat {
        Seat::Early => "Early position",
        Seat::Middle => "Middle position",
        Seat::Cutoff => "Cutoff",
        Seat::Button => "Button",
        Seat::SmallBlind => "Small blind",
        Seat::BigBlind => "Big blind",
    }
}

/// A card note with the file's internal keys replaced by plain range names.
fn plain_note(text: &str) -> String {
    text.replace("raise iso.raise hands", &format!("raise hands in {RAISE_ISO_NAME}"))
        .replace("iso.raise", RAISE_ISO_NAME)
        .replace("iso.limp", LIMP_NAME)
        .replace("the face_raise lines", "the reraise and calling ranges")
}

/// `142 combos (10.7% of hands)`.
fn combos_text(range: &Range) -> String {
    format!("{:>4} combos ({:>4.1}% of hands)", range.combo_count(), range.percent_of_hands())
}

/// The preflop card, formatted for reading.
pub fn card_text() -> String {
    let mut out = String::from("PREFLOP CARD\n\n");
    for line in CARD.header.lines() {
        out.push_str(line.trim_start_matches('#').trim_start());
        out.push('\n');
    }
    out.push_str("\nFolded to you: opening ranges\n");
    for seat in Seat::ALL {
        if let Some(line) = CARD.open_range(seat) {
            out.push_str(&format!(
                "  {:<26} {}  {}\n",
                seat_title(seat),
                combos_text(&line.range),
                line.text
            ));
        }
    }
    let rows = [
        ("One or more limpers", [("Raise-over-limpers range", &CARD.iso_raise), ("Limp-along range", &CARD.iso_limp)]),
        ("Someone raised", [("Reraise range", &CARD.reraise), ("Calling range", &CARD.call)]),
    ];
    for (title, lines) in rows {
        out.push_str(&format!("\n{title}\n"));
        for (name, line) in lines {
            out.push_str(&format!(
                "  {:<26} {}  {}\n",
                name,
                combos_text(&line.range),
                line.text
            ));
        }
    }
    out.push_str("\nNotes\n");
    for note in CARD.notes() {
        out.push_str(&format!("  {}\n", plain_note(note.text)));
    }
    out
}

fn mark_text(stats: &SkillStats) -> String {
    let mark = pass_mark(stats.skill);
    let mut parts = Vec::new();
    if let Some(m) = mark.min_correct_pct {
        parts.push(format!("at least {m:.0}% correct"));
    }
    if let Some(m) = mark.min_within_one_pct {
        parts.push(format!("at least {m:.0}% within 1"));
    }
    if let Some(m) = mark.max_median_ms {
        parts.push(format!("median at most {} s", m / 1000));
    }
    if let Some(m) = mark.max_mean_ev_loss_pct {
        parts.push(format!("mean EV loss at most {m:.0}% of pot"));
    }
    if let Some(m) = mark.max_blunders {
        parts.push(format!("at most {m} blunders"));
    }
    if let Some(m) = mark.max_mean_abs_error {
        parts.push(format!("mean absolute error at most {m:.0} points"));
    }
    parts.join(", ")
}

fn pass_status(stats: &SkillStats) -> String {
    if stats.passed {
        "Passed.".to_string()
    } else if stats.total_attempts < PASS_ATTEMPTS {
        format!("Not passed yet: needs {PASS_ATTEMPTS} attempts, has {}.", stats.total_attempts)
    } else if !stats.drill_passed {
        "Not passed yet.".to_string()
    } else if stats.checkup_passed.is_none() {
        format!("Drill mark met. Take a checkup to pass: poker-trainer check --skill {}", stats.skill.name())
    } else {
        "Drill mark met, but the latest checkup missed it.".to_string()
    }
}

fn stats_text(stats: &SkillStats) -> String {
    let mark = pass_mark(stats.skill);
    let mut parts = vec![format!("last {}", stats.window)];
    if mark.min_correct_pct.is_some() {
        parts.push(format!("{:.0}% correct", stats.correct_pct));
    }
    if mark.min_within_one_pct.is_some() {
        parts.push(format!("{:.0}% within 1 ({:.0}% exact)", stats.within_one_pct, stats.correct_pct));
    }
    if mark.max_median_ms.is_some() {
        parts.push(format!("median {:.1} s", f64::from(stats.median_ms) / 1000.0));
    }
    if mark.max_mean_ev_loss_pct.is_some() {
        parts.push(format!("mean EV loss {:.1}% of pot", stats.mean_ev_loss_pct));
    }
    if mark.max_blunders.is_some() {
        parts.push(format!("{} blunders", stats.blunders));
    }
    if let (true, Some(e)) = (mark.max_mean_abs_error.is_some(), stats.mean_abs_error) {
        parts.push(format!("mean absolute error {e:.1} points"));
    }
    parts.join(", ")
}

fn metric_text(score: &CheckupScore) -> String {
    match score.metric {
        CheckupMetric::PercentCorrect => format!("{:.0}% correct", score.score),
        CheckupMetric::PercentWithinOne => format!("{:.0}% within 1", score.score),
        CheckupMetric::MeanEvLoss => format!("mean EV loss {:.1}% of pot", score.score),
        CheckupMetric::MeanAbsError => format!("mean absolute error {:.1} points", score.score),
    }
}

/// One line per checkup, for history lists.
pub fn checkup_line(s: &CheckupScore) -> String {
    let status = if !s.complete {
        "unfinished".to_string()
    } else if s.passed {
        "passed".to_string()
    } else {
        "not passed".to_string()
    };
    let diff = s.agreement.mean_abs_diff.map_or(String::new(), |d| format!(", mean gap {d:.1}"));
    format!(
        "  {} form {} (run {}): {}, standard error {:.1}; repeats agreed {} of {}{diff}; {status}",
        s.skill.name(),
        s.form,
        s.run,
        metric_text(s),
        s.standard_error,
        s.agreement.agree,
        s.agreement.pairs,
    )
}

/// The end-of-checkup report.
pub fn checkup_report(score: &CheckupScore) -> String {
    let reused = if score.skill == Skill::Table && checkup_form_reuses_spots(score.form) {
        format!("{REUSE_NOTE}\n")
    } else {
        String::new()
    };
    format!(
        "\nCHECKUP RESULT\n\n{}\n\nScored on the {} original questions only; the repeats are a noise check.\n{reused}",
        checkup_line(score),
        poker_trainer::checkup::CHECKUP_ORIGINALS
    )
}

fn bias_section(bias: &EquityBias) -> String {
    let mut out = format!(
        "\nEquity calibration ({} estimates; mean signed error {:+.1} points)\n",
        bias.count, bias.mean_signed_error
    );
    for bucket in bias.by_opponents.iter().chain(&bias.by_truth_band).filter(|b| b.count > 0) {
        out.push_str(&format!("  {:<12} {:+5.1} points ({})\n", bucket.label, bucket.mean_signed_error, bucket.count));
    }
    if let Some(fit) = bias.fit {
        out.push_str(&format!(
            "  Log-odds fit of your estimate on the truth: intercept {:.2}, slope {:.2} (1.00 means no compression).\n",
            fit.intercept, fit.slope
        ));
    }
    out
}

/// The `progress` report: per-skill stats, the current step, and baseline and
/// checkup history.
pub fn progress_text(
    stats: &[SkillStats],
    step: usize,
    runs: &[BaselineRun],
    checkups: &[CheckupScore],
    bias: &EquityBias,
) -> String {
    let mut out = String::from("PROGRESS\n\n");
    for s in stats {
        out.push_str(&format!("{}\n", s.skill.name()));
        if s.enough_data {
            out.push_str(&format!(
                "  {}\n  Pass mark: {}. {}\n",
                stats_text(s),
                mark_text(s),
                pass_status(s)
            ));
        } else {
            out.push_str(&format!(
                "  Not enough data ({} of 20 attempts).\n  Pass mark: {}.\n",
                s.total_attempts,
                mark_text(s)
            ));
        }
    }
    let names: Vec<&str> = STEPS[step - 1].iter().map(|s| s.name()).collect();
    out.push_str(&format!("\nCurrent step: {step} of {} ({}).\n", STEPS.len(), names.join(", ")));
    out.push_str("Pass marks are starting guesses.\n\nBaseline history\n");
    if runs.is_empty() {
        out.push_str("  No baseline yet. Take one with: poker-trainer baseline\n");
    }
    for run in runs {
        out.push_str(&run_summary_line(run));
        out.push('\n');
    }
    out.push_str("\nCheckup history\n");
    if checkups.is_empty() {
        out.push_str("  No checkups yet. Take one with: poker-trainer check --skill NAME\n");
    }
    for score in checkups {
        out.push_str(&checkup_line(score));
        out.push('\n');
    }
    if bias.count >= BIAS_MIN_ATTEMPTS {
        out.push_str(&bias_section(bias));
    }
    out
}

/// One line per run, for history lists.
pub fn run_summary_line(run: &BaselineRun) -> String {
    let steps: Vec<String> = run.steps.iter().map(|s| format!("step {} {}/{}", s.step, s.ok, s.answered)).collect();
    format!(
        "  Run {}: {} of {} ({}{})",
        run.run,
        run.ok,
        run.answered,
        steps.join(", "),
        if run.complete { "" } else { ", unfinished" }
    )
}

/// The end-of-test report for one baseline run, compared with earlier runs.
pub fn baseline_report(run: &BaselineRun, earlier: &[BaselineRun]) -> String {
    let mut out = format!("\nBASELINE RESULT (run {})\n\nBy section\n", run.run);
    for section in &run.sections {
        out.push_str(&format!("  {:<9} {} of {}", section.skill.name(), section.ok, section.answered));
        if section.ok != section.exact {
            out.push_str(&format!(" ({} exact)", section.exact));
        }
        out.push('\n');
    }
    out.push_str("\nBy step\n");
    for step in &run.steps {
        out.push_str(&format!("  Step {}: {} of {}\n", step.step, step.ok, step.answered));
    }
    out.push_str(&format!("\nTotal: {} of {}", run.ok, run.answered));
    if !run.complete {
        out.push_str(" (unfinished)");
    }
    out.push('\n');
    if earlier.is_empty() {
        out.push_str("\nThis is your first baseline, so there is nothing to compare yet.\n");
    } else {
        out.push_str("\nEarlier runs\n");
        for prior in earlier {
            out.push_str(&run_summary_line(prior));
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use poker_trainer::cards::{parse_cards, parse_holding};
    use poker_trainer::classes::OppClass;
    use poker_trainer::generate::generate_preflop;
    use poker_trainer::grade::grade;
    use poker_trainer::item::Skill;

    const PLAIN: Style = Style { color: false };
    const INTERNAL_KEYS: [&str; 5] = ["iso.", "open.", "face_raise", "note.", "small_blind"];

    fn cards(text: &str) -> Vec<Card> {
        parse_cards(text).unwrap()
    }

    fn hand(text: &str) -> Holding {
        parse_holding(text).unwrap()
    }

    fn spot_item(mode: SpotMode, opponents: Vec<OppClass>) -> Item {
        let question = Question::Spot(SpotQuestion {
            hero: hand("AhQh"),
            board: cards("Kh7h2c"),
            pot: 150,
            call: 50,
            mode,
            opponents,
        });
        Item { skill: Skill::Spot, seed: 1, question }
    }

    fn revealed(item: &Item, answer: &str) -> String {
        reveal(&grade(item, answer).unwrap(), PLAIN)
    }

    fn assert_no_internal_keys(text: &str) {
        for key in INTERNAL_KEYS {
            assert!(!text.contains(key), "found {key} in: {text}");
        }
    }

    #[test]
    fn card_uses_plain_labels_and_shows_percent_of_hands() {
        let text = card_text();
        assert_no_internal_keys(&text);
        assert!(text.contains("Middle position"), "{text}");
        assert!(text.contains("Raise-over-limpers range"), "{text}");
        assert!(text.contains("Limp-along range"), "{text}");
        assert!(text.contains("Reraise range"), "{text}");
        assert!(text.contains("Calling range"), "{text}");
        // 142 of 1,326 combinations.
        assert!(text.contains("142 combos (10.7% of hands)"), "{text}");
        assert!(text.contains("raise hands in the raise-over-limpers range"), "{text}");
    }

    #[test]
    fn preflop_reveals_never_show_internal_keys() {
        for seed in 0..600 {
            let item = generate_preflop(seed, 5 + (seed % 3) as u8);
            let text = revealed(&item, "f");
            assert_no_internal_keys(&text);
        }
    }

    #[test]
    fn preflop_reveal_names_the_plain_ranges() {
        let item = |behind, situation, text: &str| Item {
            skill: Skill::Preflop,
            seed: 1,
            question: Question::Preflop { players: 5, behind, situation, hand: hand(text) },
        };
        assert!(revealed(&item(4, Situation::FoldedTo, "KhQd"), "r").contains("your middle-position opening range"));
        assert!(revealed(&item(3, Situation::Limpers(1), "AhJd"), "r").contains("the raise-over-limpers range"));
        assert!(revealed(&item(3, Situation::Limpers(1), "Ah2h"), "c").contains("the limp-along range"));
        assert!(revealed(&item(3, Situation::Raised, "JhJd"), "r").contains("the reraise range"));
        assert!(revealed(&item(3, Situation::Raised, "ThTd"), "c").contains("the calling range"));
    }

    #[test]
    fn opponent_classes_read_as_natural_words_without_codes() {
        let expected = [
            (OppClass::Over, "Assume they have an overpair."),
            (OppClass::Top, "Assume they have top pair."),
            (OppClass::Mid, "Assume they have a middle pair."),
            (OppClass::TwoPair, "Assume they have two pair."),
            (OppClass::Set, "Assume they have a set."),
            (OppClass::Trips, "Assume they have trips."),
            (OppClass::FlushDraw, "Assume they have a flush draw."),
            (OppClass::Oesd, "Assume they have an open-ended straight draw."),
        ];
        for (class, line) in expected {
            let text = question(&spot_item(SpotMode::AllIn, vec![class]), PLAIN);
            assert!(text.contains(line), "{text}");
            assert!(!text.contains(&format!("({})", class.code())), "{text}");
        }
    }

    #[test]
    fn tying_cards_are_marked_and_the_heading_is_accurate() {
        // K-4 v K-5 on Q-T-9: a jack gives both the same king-high straight.
        let item = Item {
            skill: Skill::Outs,
            seed: 1,
            question: Question::Outs { hero: hand("Kh4d"), board: cards("QsTd9c"), villain: hand("Ks5c") },
        };
        let text = revealed(&item, "3");
        assert!(text.contains("they improve you but don't put you ahead"), "{text}");
        assert!(!text.contains("still behind"), "{text}");
        assert!(text.contains("J♠ gives you a king-high straight (you would tie)."), "{text}");
    }

    #[test]
    fn a_card_that_stays_behind_is_not_marked_as_a_tie() {
        let item = Item {
            skill: Skill::Outs,
            seed: 1,
            question: Question::Outs { hero: hand("AhQh"), board: cards("Kh7h2c"), villain: hand("7c7d") },
        };
        let text = revealed(&item, "8");
        assert!(text.contains("2♥ gives you"), "{text}");
        assert!(!text.contains("you would tie"), "{text}");
    }

    #[test]
    fn break_even_x_is_written_in_plain_words() {
        let item = spot_item(SpotMode::NextCard { x: 0 }, vec![OppClass::Set]);
        let text = revealed(&item, "f");
        assert!(!text.contains("Break-even X"), "{text}");
        assert!(text.contains("You would need to win about $81 more when you hit for calling to break even."), "{text}");
    }

    #[test]
    fn spot_reveal_uses_plain_equity_wording() {
        let next = revealed(&spot_item(SpotMode::NextCard { x: 0 }, vec![OppClass::Set]), "f");
        assert!(next.contains("Needed equity = call / (pot + call) = 50 / (150 + 50) = 25.0%. The pot already includes the bet."), "{next}");
        assert!(next.contains("Your share of the pot after the next card: 17.8%, averaged over 9 possible combos."), "{next}");
        assert!(!next.contains("C / (P + C)") && !next.contains("Your share:"), "{next}");
        let all_in = revealed(&spot_item(SpotMode::AllIn, vec![OppClass::Set]), "f");
        assert!(all_in.contains("Your equity: "), "{all_in}");
        assert!(!all_in.contains("after the next card:"), "{all_in}");
    }

    #[test]
    fn price_reveal_uses_plain_equity_wording() {
        let item = Item {
            skill: Skill::Price,
            seed: 1,
            question: Question::Price { pot_before: 600, bet: 150, callers: 0 },
        };
        let text = revealed(&item, "17");
        assert!(text.contains("Needed equity = call / (pot + call) = 150 / (750 + 150) = 16.7%. The pot already includes the bet."), "{text}");
    }

    #[test]
    fn holding_counts_are_singular_or_plural() {
        assert_eq!(holdings_word(1), "holding");
        assert_eq!(holdings_word(2), "holdings");
        // Exact cards cover one holding; only the middle set's three kings beat it.
        let nuts = Item { skill: Skill::Nuts, seed: 1, question: Question::Nuts { board: cards("Kh7c2d") } };
        let text = revealed(&nuts, "KdKs");
        assert!(text.contains("covers 1 unseen holding, of which 1 makes the nuts."), "{text}");
        assert!(!text.contains("holding(s)"), "{text}");
        let beats = Item {
            skill: Skill::Beats,
            seed: 1,
            question: Question::Beats { hero: hand("7h7d"), board: cards("Kh7c2d") },
        };
        assert!(revealed(&beats, "a").contains("three of a kind: 3 holdings, the best being"));
    }

    #[test]
    fn flush_no_reason_is_a_clear_sentence() {
        let item = Item { skill: Skill::Possible, seed: 1, question: Question::Possible { board: cards("Ks9h2d") } };
        let text = revealed(&item, "none");
        assert!(text.contains("Flush: no. A flush needs three of one suit on the board because you hold just two cards, and no suit appears more than once."), "{text}");
    }

    #[test]
    fn opponent_labels_match_the_number_of_opponents() {
        let cases = [
            (0, "a tight player", "tight players"),
            (100, "an average player", "average players"),
            (900, "a calling station", "calling stations"),
        ];
        for (x, single, many) in cases {
            let one = question(&spot_item(SpotMode::NextCard { x }, vec![OppClass::Top]), PLAIN);
            assert!(one.contains(&format!("You are up against {single}.")), "{one}");
            let three = spot_item(SpotMode::NextCard { x }, vec![OppClass::Top, OppClass::Over, OppClass::FlushDraw]);
            let three = question(&three, PLAIN);
            assert!(three.contains(&format!("You are up against {many}.")), "{three}");
        }
    }

    #[test]
    fn made_reveal_does_not_repeat_the_label() {
        let item = Item {
            skill: Skill::Made,
            seed: 1,
            question: Question::Made { hero: hand("7h8s"), board: cards("7d7cKd") },
        };
        let text = revealed(&item, "trips");
        assert!(text.contains("Answer: trips. One hole card matches"), "{text}");
        assert!(!text.contains("trips, trips"), "{text}");
    }

    #[test]
    fn preflop_prompt_labels_c_for_the_situation() {
        let hint = |behind, situation| {
            answer_hint(&Item {
                skill: Skill::Preflop,
                seed: 1,
                question: Question::Preflop { players: 5, behind, situation, hand: hand("AhKd") },
            })
        };
        assert_eq!(hint(3, Situation::Limpers(1)), "r raise, c limp along, f fold");
        assert_eq!(hint(3, Situation::Raised), "r raise, c call, f fold");
        assert_eq!(hint(0, Situation::Limpers(1)), "r raise, c check");
        assert_eq!(hint(0, Situation::Raised), "r raise, c call, f fold");
    }
}
