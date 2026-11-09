//! Turns the library's questions, answer formats and outcomes into JSON for the
//! page. Numbers are formatted here, so the page never does arithmetic or
//! rounding of its own. The wording of the labels follows the command-line app.

use poker_trainer::anchors::{self, AnchorCard, AnchorKind};
use poker_trainer::answer::{answer_format, AnswerFormat, AnswerOption};
use poker_trainer::board_type::{BoardCategory, RankPattern, SuitPattern, BOARD_TYPE_MISS_MS, BOARD_TYPE_TARGET_MS};
use poker_trainer::cards::{Card, Holding};
use poker_trainer::checkup::{CheckupMetric, CheckupScore, RepeatAgreement};
use poker_trainer::equity_skill::BiasBucket;
use poker_trainer::eval::HandRank;
use poker_trainer::grade::{AheadKind, EquityAnalysis, EvBand, Outcome, Reveal};
use poker_trainer::item::{
    EquityOpponents, EquityQuestion, HitStreet, Item, OpponentLabel, Question, Skill, SpotMode, SpotQuestion,
};
use poker_trainer::preflop::{action_label, Action, PreflopReason, Seat, Situation, CARD};

use crate::json::{obj, V};

pub fn skill_title(skill: Skill) -> &'static str {
    match skill {
        Skill::Preflop => "Preflop",
        Skill::Nuts => "The nuts",
        Skill::Possible => "What's possible",
        Skill::Made => "Your hand",
        Skill::Beats => "How often you're beaten",
        Skill::Outs => "Counting outs",
        Skill::Price => "Pot odds",
        Skill::Hit => "Chance of hitting",
        Skill::Spot => "Call or fold",
        Skill::SpotMw => "Call or fold, multiway",
        Skill::BoardType => "Reading the board",
        Skill::Equity => "Estimating equity",
        Skill::Anchor => "Key numbers",
    }
}

pub fn seat_title(seat: Seat) -> &'static str {
    match seat {
        Seat::Early => "Early position",
        Seat::Middle => "Middle position",
        Seat::Cutoff => "Cutoff",
        Seat::Button => "Button",
        Seat::SmallBlind => "Small blind",
        Seat::BigBlind => "Big blind",
    }
}

fn seat_sentence(seat: Seat) -> &'static str {
    match seat {
        Seat::Early => "in early position",
        Seat::Middle => "in middle position",
        Seat::Cutoff => "in the cutoff",
        Seat::Button => "on the button",
        Seat::SmallBlind => "in the small blind",
        Seat::BigBlind => "in the big blind",
    }
}

pub fn pct(fraction: f64) -> String {
    format!("{:.1}%", fraction * 100.0)
}

pub fn money(value: f64) -> String {
    if value < 0.0 {
        format!("-${:.2}", -value)
    } else {
        format!("${value:.2}")
    }
}

fn card_text(card: Card) -> V {
    V::Str(card.to_string())
}

fn cards(cards: &[Card]) -> V {
    V::Arr(cards.iter().map(|&c| card_text(c)).collect())
}

fn holding(h: &Holding) -> V {
    cards(&h[..])
}

/// A short list of holdings: at most `limit`, with how many are left over.
fn holdings_short(list: &[Holding], limit: usize) -> V {
    let shown: Vec<V> = list.iter().take(limit).map(holding).collect();
    obj! { "shown": shown, "more": list.len().saturating_sub(limit) }
}

fn street_name(board_len: usize) -> &'static str {
    match board_len {
        3 => "flop",
        4 => "turn",
        _ => "river",
    }
}

pub fn question(q: &Question) -> V {
    match q {
        Question::Preflop { players, behind, situation, hand } => {
            let seat = Seat::from_behind(*behind);
            let (kind, limpers) = match situation {
                Situation::FoldedTo => ("folded", 0),
                Situation::Limpers(n) => ("limpers", *n),
                Situation::Raised => ("raised", 0),
            };
            obj! {
                "kind": "preflop",
                "players": *players,
                "behind": *behind,
                "seat_title": seat_title(seat),
                "seat_sentence": seat_sentence(seat),
                "situation": kind,
                "limpers": limpers,
                "hand": holding(hand),
            }
        }
        Question::Nuts { board } => obj! { "kind": "nuts", "board": cards(board) },
        Question::Possible { board } => obj! { "kind": "possible", "board": cards(board) },
        Question::Made { hero, board } => {
            obj! { "kind": "made", "hero": holding(hero), "board": cards(board) }
        }
        Question::Beats { hero, board } => {
            obj! { "kind": "beats", "hero": holding(hero), "board": cards(board) }
        }
        Question::Outs { hero, board, villain } => obj! {
            "kind": "outs",
            "hero": holding(hero),
            "board": cards(board),
            "villain": holding(villain),
        },
        Question::Price { pot_before, bet, callers } => obj! {
            "kind": "price",
            "pot_before": *pot_before,
            "bet": *bet,
            "callers": *callers,
        },
        Question::Hit { outs, street, both } => obj! {
            "kind": "hit",
            "outs": *outs,
            "street": match street { HitStreet::Flop => "flop", HitStreet::Turn => "turn" },
            "both": *both,
        },
        Question::Spot(spot) => spot_question(spot),
        Question::Equity(q) => equity_question(q),
        Question::Anchor { card } => anchor_question(usize::from(*card)),
        Question::BoardType { board, .. } => obj! {
            "kind": "board_type",
            "board": cards(board),
            "target_ms": BOARD_TYPE_TARGET_MS,
            "target_seconds": BOARD_TYPE_TARGET_MS / 1000,
        },
    }
}

/// The question as the player sees it. The tolerance band is left out: it is
/// fixed when the item is served and only appears in the reveal.
fn equity_question(q: &EquityQuestion) -> V {
    let (exact, classes): (Vec<V>, Vec<V>) = match &q.opponents {
        EquityOpponents::Exact(hands) => (hands.iter().map(holding).collect(), Vec::new()),
        EquityOpponents::Classes(list) => (
            Vec::new(),
            list.iter().map(|c| V::from(c.description())).collect(),
        ),
    };
    obj! {
        "kind": "equity",
        "rung": u32::from(q.rung()),
        "hero": holding(&q.hero),
        "board": cards(&q.board),
        "street": street_name(q.board.len()),
        "opponents": q.opponents.count(),
        "exact": exact,
        "classes": classes,
    }
}

/// `Top pair vs two pair: your AhKd against ...` splits into its label and the
/// sentence; preflop and price cards keep their whole prompt as the sentence.
fn anchor_question(index: usize) -> V {
    let card = anchors::deck().get(index).expect("a dealt card is in the deck");
    let matchup = match &card.kind {
        AnchorKind::Matchup { hero, villain, board } => {
            let label = card.prompt.split_once(": ").map_or(card.prompt.as_str(), |(label, _)| label);
            Some(obj! {
                "label": label,
                "hero": holding(hero),
                "villain": holding(villain),
                "board": cards(board),
            })
        }
        _ => None,
    };
    let prompt = match &card.kind {
        AnchorKind::Matchup { .. } => "What is your all-in equity?".to_string(),
        _ => card.prompt.clone(),
    };
    let hand = match &card.kind {
        AnchorKind::Preflop { hand, .. } => Some(holding(hand)),
        _ => None,
    };
    obj! { "kind": "anchor", "prompt": prompt, "matchup": matchup, "hand": hand }
}

fn spot_question(q: &SpotQuestion) -> V {
    let many = q.opponents.len() > 1;
    let label = match (q.opponent_label(), many) {
        (OpponentLabel::Tight, false) => "a tight player",
        (OpponentLabel::Average, false) => "an average player",
        (OpponentLabel::CallingStation, false) => "a calling station",
        (OpponentLabel::Tight, true) => "tight players",
        (OpponentLabel::Average, true) => "average players",
        (OpponentLabel::CallingStation, true) => "calling stations",
    };
    let opponents: Vec<V> = q.opponents.iter().map(|c| V::from(c.description())).collect();
    obj! {
        "kind": "spot",
        "hero": holding(&q.hero),
        "board": cards(&q.board),
        "street": street_name(q.board.len()),
        "pot": q.pot,
        "call": q.call,
        "all_in": matches!(q.mode, SpotMode::AllIn),
        "x": q.x(),
        "opponents": opponents,
        "label": label,
    }
}

fn option(o: &AnswerOption) -> V {
    obj! { "code": o.code, "label": o.label }
}

pub fn format(q: &Question) -> V {
    match answer_format(q) {
        AnswerFormat::Choice(options) => obj! {
            "type": "choice",
            "large": matches!(q, Question::BoardType { .. }),
            "options": options.iter().map(option).collect::<Vec<_>>(),
        },
        AnswerFormat::MultiSelect { options, none_code } => obj! {
            "type": "multi",
            "options": options.iter().map(option).collect::<Vec<_>>(),
            "none_code": none_code,
        },
        AnswerFormat::Integer { min, max } => obj! { "type": "integer", "min": min, "max": max },
        AnswerFormat::Percent => obj! { "type": "percent" },
        AnswerFormat::Holding => obj! { "type": "holding" },
    }
}

/// What the player answered, in words rather than as the graded code.
fn picked(q: &Question, answer: &str) -> String {
    match answer_format(q) {
        AnswerFormat::Choice(options) => options
            .iter()
            .find(|o| o.code == answer)
            .map_or_else(|| answer.to_string(), |o| o.label.to_string()),
        AnswerFormat::MultiSelect { options, none_code } => {
            if answer == none_code {
                return "none of these".to_string();
            }
            let labels: Vec<&str> = options
                .iter()
                .filter(|o| answer.contains(o.code))
                .map(|o| o.label)
                .collect();
            labels.join(", ")
        }
        AnswerFormat::Integer { .. } => answer.to_string(),
        AnswerFormat::Percent => format!("{answer}%"),
        AnswerFormat::Holding => holding_text(answer),
    }
}

/// `ahkh`, `aks`, `qj`, `ahx` or `ax`, written the way a player writes them.
fn holding_text(answer: &str) -> String {
    let rank = |c: char| c.to_ascii_uppercase();
    let chars: Vec<char> = answer.chars().collect();
    match chars.as_slice() {
        [a, x] if *x == 'x' => format!("{} with any other card", rank(*a)),
        [a, s, x] if *x == 'x' => format!("{}{s} with any other card", rank(*a)),
        [a, s, b, t] => format!("{}{s} {}{t}", rank(*a), rank(*b)),
        _ => chars.iter().enumerate().map(|(i, &c)| if i < 2 { rank(c) } else { c }).collect(),
    }
}

/// `response_ms` is 0 when the answer is graded ahead of time; only `board_type`
/// is timed, and it is graded when the answer arrives.
pub fn outcome(item: &Item, o: &Outcome, attempt_template: String, response_ms: u32) -> V {
    obj! {
        "correct": o.correct,
        "close": o.close,
        "ok": o.ok(),
        "picked": picked(&item.question, &o.answer),
        "reveal": reveal(item, o, response_ms),
        "attempt": attempt_template,
    }
}

fn describe(rank: HandRank) -> V {
    V::Str(rank.describe())
}

fn reveal(item: &Item, o: &Outcome, response_ms: u32) -> V {
    match &o.reveal {
        Reveal::Preflop { seat, situation, action, reason } => obj! {
            "kind": "preflop",
            "answer": action_label(*action, *seat, *situation),
            "why": preflop_why(*seat, *reason),
        },
        Reveal::Nuts { analysis, used_x, consistent, consistent_nuts } => {
            let next = analysis.next.as_ref().map(|(rank, list)| {
                obj! { "rank": describe(*rank), "holdings": holdings_short(list, 6) }
            });
            obj! {
                "kind": "nuts",
                "top": describe(analysis.top),
                "holdings": holdings_short(&analysis.top_holdings, 6),
                "next": next,
                "used_x": *used_x,
                "consistent": *consistent,
                "consistent_nuts": *consistent_nuts,
            }
        }
        Reveal::Possible(a) => {
            let example = |h: &Option<Holding>| h.as_ref().map(holding);
            // The labels come from the answer format, which names each option.
            let answer = match answer_format(&item.question) {
                AnswerFormat::MultiSelect { options, .. } => options
                    .iter()
                    .filter(|o| {
                        (o.code == "f" && a.set.flush)
                            || (o.code == "s" && a.set.straight)
                            || (o.code == "h" && a.set.full)
                    })
                    .map(|o| o.label)
                    .collect::<Vec<_>>(),
                _ => Vec::new(),
            };
            obj! {
                "kind": "possible",
                "answer": answer,
                "flush": example(&a.flush_example),
                "straight": example(&a.straight_example),
                "full": example(&a.full_example),
                "max_suit": a.max_suit_on_board,
            }
        }
        Reveal::Made { class, rank } => obj! {
            "kind": "made",
            "answer": class.label(),
            "description": class.description(),
            "rank": describe(*rank),
        },
        Reveal::Beats(a) => {
            let answer = match answer_format(&item.question) {
                AnswerFormat::Choice(options) => options
                    .iter()
                    .find(|o| o.code == a.bucket.code())
                    .map_or(String::new(), |o| o.label.to_string()),
                _ => String::new(),
            };
            let kinds: Vec<V> = a.kinds.iter().take(3).map(ahead_kind).collect();
            obj! {
                "kind": "beats",
                "answer": answer,
                "ahead": a.ahead,
                "total": a.total,
                "pct": format!("{:.1}%", a.pct),
                "hero": describe(a.hero_rank),
                "kinds": kinds,
            }
        }
        Reveal::Outs { analysis, hero_rank, villain_rank } => {
            let tempting: Vec<V> = analysis
                .tempting
                .iter()
                .take(5)
                .map(|t| {
                    obj! { "card": card_text(t.card), "rank": describe(t.new_rank), "ties": t.ties }
                })
                .collect();
            obj! {
                "kind": "outs",
                "answer": analysis.outs.len(),
                "hero": describe(*hero_rank),
                "villain": describe(*villain_rank),
                "unseen": analysis.unseen,
                "outs": cards(&analysis.outs),
                "tempting": tempting,
            }
        }
        Reveal::Price { pot, call, required } => obj! {
            "kind": "price",
            "pot": *pot,
            "call": *call,
            "required": pct(*required),
        },
        Reveal::Hit { outs, unseen, both, exact, shortcut } => obj! {
            "kind": "hit",
            "outs": *outs,
            "unseen": *unseen,
            "unseen_next": unseen.saturating_sub(1),
            "both": *both,
            "exact": pct(*exact),
            "shortcut": format!("{shortcut:.0}%"),
            "multiplier": if *both { 4u32 } else { 2u32 },
        },
        Reveal::Spot { analysis: a, chosen, best, loss_pct, loss_money, band } => {
            let band_text = band.map(|b| match b {
                EvBand::Close => "close, so it counts as correct",
                EvBand::Small => "a small error",
                EvBand::Mistake => "a mistake",
                EvBand::Blunder => "a blunder",
            });
            let break_even = if a.next_card {
                a.break_even_x.map(|x| if x > 0.0 { format!("${x:.0}") } else { String::new() })
            } else {
                None
            };
            let combos: Vec<String> = a.combos_per_opponent.iter().map(|n| n.to_string()).collect();
            obj! {
                "kind": "spot",
                "answer": action_name(*best),
                "matched": *chosen == *best,
                "price": pct(a.price),
                "required": pct(a.required),
                "x": a.x,
                "share": pct(a.share),
                "sampled": a.sampled,
                "sample_size": with_commas(poker_trainer::equity::SAMPLE_SIZE),
                "combos": combos.join(" x "),
                "ev_call": money(a.ev_call),
                "ev_fold": money(a.ev_fold),
                "loss_money": money(*loss_money),
                "loss_pct": format!("{loss_pct:.1}%"),
                "band": band_text,
                "next_card": a.next_card,
                "break_even": break_even,
            }
        }
        Reveal::Equity { analysis, estimate_pct, signed_error, band, hit } => {
            equity_reveal(analysis, *estimate_pct, *signed_error, *band, *hit)
        }
        Reveal::Anchor { card, estimate_pct, signed_error } => {
            anchor_reveal(card, *estimate_pct, *signed_error, o.correct)
        }
        Reveal::BoardType { category, chosen, too_slow } => {
            let board = match &item.question {
                Question::BoardType { board, .. } => board,
                _ => unreachable!("a board_type reveal comes from a board_type item"),
            };
            let matched = *category == *chosen;
            obj! {
                "kind": "board_type",
                "answer": category.label(),
                "matched": matched,
                "chosen": chosen.label(),
                "why_suit": board_suit_why(board, *category),
                "why_rank": board_rank_why(*category),
                "too_slow": *too_slow,
                "slow_note": matched.then(|| board_slow_note(*too_slow, response_ms)),
            }
        }
    }
}

/// "You were 7 points high", "1 point low", or "spot on" within a twentieth.
pub fn signed_error_words(signed: f64) -> String {
    let abs = signed.abs();
    if abs < 0.05 {
        return "You were spot on".to_string();
    }
    let side = if signed > 0.0 { "high" } else { "low" };
    let rounded = (abs * 10.0).round() / 10.0;
    format!("You were {} {side}", plural_points(rounded))
}

/// `7` not `7.0`, but `6.7` stays `6.7`.
fn points_number(value: f64) -> String {
    if (value - value.round()).abs() < 1e-9 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

fn plain_percent(value: f64) -> String {
    format!("{}%", points_number((value * 10.0).round() / 10.0))
}

fn equity_reveal(a: &EquityAnalysis, estimate_pct: f64, signed_error: f64, band: u8, hit: bool) -> V {
    let combos: Vec<String> = a.combos_per_opponent.iter().map(|n| n.to_string()).collect();
    let tuples = a.tuples.map(with_commas);
    // Exact hands are a single matchup; "1 x 1 combos" would mean nothing.
    let averaged = a.combos_per_opponent.iter().any(|&n| n != 1);
    obj! {
        "kind": "equity",
        "answer": format!("{:.1}%", a.truth_pct),
        "estimate": plain_percent(estimate_pct),
        "error": signed_error_words(signed_error),
        "band": band,
        "hit": hit,
        "combos": averaged.then(|| combos.join(" x ")),
        "tuples": tuples,
        "sampled": a.sampled,
        "sample_size": with_commas(poker_trainer::equity::SAMPLE_SIZE),
    }
}

fn anchor_reveal(card: &AnchorCard, estimate_pct: f64, signed_error: f64, hit: bool) -> V {
    obj! {
        "kind": "anchor",
        "answer": format!("{:.1}%", card.answer_pct),
        "estimate": plain_percent(estimate_pct),
        "error": signed_error_words(signed_error),
        "tolerance": card.tolerance,
        "hit": hit,
    }
}

fn board_suit_why(board: &[Card], category: BoardCategory) -> String {
    let mut suits: Vec<u8> = board.iter().map(|c| c.suit()).collect();
    suits.sort_unstable();
    suits.dedup();
    let which = |n: usize| match n {
        1 => "All three cards are one suit",
        2 => "Two of the three cards share a suit",
        _ => "All three cards are different suits",
    };
    let name = match category.suit {
        SuitPattern::Rainbow => "rainbow",
        SuitPattern::TwoTone => "two-tone",
        SuitPattern::Monotone => "monotone",
    };
    format!("{}, so it is {name}.", which(suits.len()))
}

fn board_rank_why(category: BoardCategory) -> &'static str {
    match category.rank {
        RankPattern::Paired => "Two cards have the same rank, so it is paired.",
        RankPattern::Connected => "The ranks are close enough that a straight is possible, so it is connected.",
        RankPattern::Disconnected => "The ranks are too spread out to make a straight, so it is disconnected.",
    }
}

fn board_slow_note(too_slow: bool, response_ms: u32) -> String {
    if too_slow {
        format!(
            "You had the right category, but answers over {} seconds count as a miss.",
            BOARD_TYPE_MISS_MS / 1000
        )
    } else if response_ms > BOARD_TYPE_TARGET_MS {
        format!(
            "Right, but slower than the {}-second target, so it doesn't count toward making this category solid.",
            BOARD_TYPE_TARGET_MS / 1000
        )
    } else {
        String::new()
    }
}

/// Plain words for the bias tables.
fn opponents_phrase(label: &str) -> Option<&'static str> {
    match label {
        "1 opponent" => Some("heads-up pots"),
        "2 opponents" => Some("three-way pots"),
        "3 opponents" => Some("four-way pots"),
        _ => None,
    }
}

/// Mean signed errors under this size read as "about right".
const BIAS_FLAT_POINTS: f64 = 1.0;
/// Buckets smaller than this get a table row but no sentence.
const BIAS_MIN_BUCKET: usize = 10;

fn plural_points(rounded: f64) -> String {
    format!("{} {}", points_number(rounded), if (rounded - 1.0).abs() < 1e-9 { "point" } else { "points" })
}

fn bias_sentence(error: f64, subject: &str) -> String {
    if error.abs() < BIAS_FLAT_POINTS {
        return format!("You estimate {subject} about right.");
    }
    let verb = if error > 0.0 { "overestimate" } else { "underestimate" };
    format!("You {verb} {subject} by about {}.", plural_points(error.abs().round()))
}

pub fn overall_bias(mean: f64) -> String {
    if mean.abs() < BIAS_FLAT_POINTS {
        "Overall your estimates are about right.".to_string()
    } else {
        let verb = if mean > 0.0 { "overestimate" } else { "underestimate" };
        format!("Overall you {verb} your equity by about {}.", plural_points(mean.abs().round()))
    }
}

pub fn bias_row(bucket: &BiasBucket) -> V {
    let opponents = opponents_phrase(&bucket.label);
    let (label, subject) = match opponents {
        Some(phrase) => (format!("{} ({})", capitalized(phrase), bucket.label), phrase.to_string()),
        None => (
            format!("True equity {}%", bucket.label),
            format!("spots where your true equity is {}%", bucket.label),
        ),
    };
    let sentence = (bucket.count >= BIAS_MIN_BUCKET).then(|| bias_sentence(bucket.mean_signed_error, &subject));
    obj! {
        "label": label,
        "count": bucket.count,
        "error": format!("{:+.1} points", bucket.mean_signed_error),
        "sentence": sentence,
    }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

/// A checkup score in its own measure.
pub fn checkup_score(s: &CheckupScore) -> String {
    match s.metric {
        CheckupMetric::PercentCorrect => format!("{:.0}% correct", s.score),
        CheckupMetric::PercentWithinOne => format!("{:.0}% within one out", s.score),
        CheckupMetric::MeanEvLoss => format!("{:.1}% of the pot lost on average", s.score),
        CheckupMetric::MeanAbsError => format!("average error {:.1} points", s.score),
    }
}

/// The margin as a plain phrase: about 2 standard errors either way.
fn margin_of(se: f64) -> f64 {
    1.96 * se
}

pub fn checkup_margin(s: &CheckupScore) -> String {
    let margin = margin_of(s.standard_error);
    if margin < 0.05 {
        return "margin of error too small to show".to_string();
    }
    match s.metric {
        CheckupMetric::PercentCorrect | CheckupMetric::PercentWithinOne => {
            format!("about plus or minus {} points", points_number(margin.round()))
        }
        CheckupMetric::MeanEvLoss => format!("about plus or minus {:.1}% of the pot", margin),
        CheckupMetric::MeanAbsError => format!("about plus or minus {:.1} points", margin),
    }
}

/// How the repeats went. Exact answers agree when both were right or both wrong.
pub fn repeats_line(a: &RepeatAgreement) -> String {
    if a.pairs == 0 {
        return "No repeat questions were answered.".to_string();
    }
    let mut line = format!(
        "Of {} repeated {}, you were consistent on {} ({}%).",
        a.pairs,
        if a.pairs == 1 { "question" } else { "questions" },
        a.agree,
        a.agree_pct.round() as u32
    );
    if let Some(gap) = a.mean_abs_diff {
        line.push_str(&format!(" Your two answers differed by {:.1} on average.", gap));
    }
    line
}

/// One line comparing a checkup with the previous one in the same measure.
pub fn comparison(prior: &CheckupScore, now: &CheckupScore) -> String {
    let lower_is_better = matches!(now.metric, CheckupMetric::MeanEvLoss | CheckupMetric::MeanAbsError);
    let delta = now.score - prior.score;
    let unit = match now.metric {
        CheckupMetric::MeanEvLoss => "% of the pot",
        _ => " points",
    };
    let amount = format!("{}{unit}", points_number((delta.abs() * 10.0).round() / 10.0));
    let combined = margin_of((now.standard_error.powi(2) + prior.standard_error.powi(2)).sqrt());
    let within = delta.abs() <= combined;
    let direction = match (delta.abs() < 0.05, (delta > 0.0) == !lower_is_better) {
        (true, _) => return "The same score as your last checkup.".to_string(),
        (false, true) => format!("Better than your last checkup by {amount}"),
        (false, false) => format!("Worse than your last checkup by {amount}"),
    };
    if within {
        format!("{direction}, which is within the margin of error.")
    } else {
        format!("{direction}.")
    }
}

/// `300000` as `300,000`.
fn with_commas(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn action_name(action: Action) -> &'static str {
    if action == Action::Call {
        "call"
    } else {
        "fold"
    }
}

fn ahead_kind(kind: &AheadKind) -> V {
    obj! {
        "category": kind.category.name(),
        "count": kind.count,
        "example": holding(&kind.example),
        "best": describe(kind.best),
    }
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

/// A card note with the file's internal keys replaced by plain range names.
pub fn plain_note(text: &str) -> String {
    text.replace("raise iso.raise hands", &format!("raise hands in {RAISE_ISO_NAME}"))
        .replace("iso.raise", RAISE_ISO_NAME)
        .replace("iso.limp", LIMP_NAME)
        .replace("the face_raise lines", "the reraise and calling ranges")
}

fn preflop_why(seat: Seat, reason: PreflopReason) -> String {
    let open_line = CARD.open_range(seat).map_or("", |l| l.text);
    let open_name = opening_range_name(seat);
    let (raise_iso, limp) = (CARD.iso_raise.text, CARD.iso_limp.text);
    let (reraise, call) = (CARD.reraise.text, CARD.call.text);
    match reason {
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
    }
}
