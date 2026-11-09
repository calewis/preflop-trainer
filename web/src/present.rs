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
use poker_trainer::beats_me::{mix_method, prompt_text, ways_method, Standing};
use poker_trainer::grade::{any_of_suit_text, AheadKind, BeatsMeReveal, EquityAnalysis, EvBand, FlipFacts, Outcome, Reveal, BEATS_RANDOM_NOTE};
use poker_trainer::item::{
    BeatsMeAsk, BeatsMeQuestion,
    ChainHand, ChainQuestion, ChainStage, EquityOpponents, EquityQuestion, HitStreet, Item, OpponentLabel, Question,
    ReadPriceAsk, ReadPriceQuestion, Skill, SpotMode, SpotQuestion, CHAIN_STAGES,
};
use poker_trainer::log::Confidence;
use poker_trainer::progress::{ReliabilityRow, RELIABILITY_MIN_RATINGS};
use poker_trainer::read_price;
use poker_trainer::reflection::{Prompt, ReflectionComparison, ReflectionGroup, REFLECTION_MIN_MISSES};
use poker_trainer::preflop::{action_label, why_line, Action, PreflopReason, Seat, Situation, CARD};
use poker_trainer::table::{bb_text, ev_bb_text, TableAction, TableActionKind, TableGrade, TableQuestion, TableSeat, TableStreetName, Verdict, ASSUMPTION_NOTE};

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
        Skill::Table => "Table spots",
        Skill::Chain => "Whole hand",
        Skill::BeatsMe => "What beats you?",
        Skill::ReadPrice => "Read the price",
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
            let (kind, limpers, callers, in_position) = match situation {
                Situation::FoldedTo => ("folded", 0, 0, false),
                Situation::Limpers(n) => ("limpers", *n, 0, false),
                Situation::Raised => ("raised", 0, 0, false),
                Situation::RaisedCalled(n) => ("callers", 0, *n, false),
                Situation::ThreeBet { in_position } => ("three_bet", 0, 0, *in_position),
                Situation::FourBet => ("four_bet", 0, 0, false),
            };
            obj! {
                "kind": "preflop",
                "players": *players,
                "behind": *behind,
                "seat_title": seat_title(seat),
                "seat_sentence": seat_sentence(seat),
                "situation": kind,
                "limpers": limpers,
                "callers": callers,
                "in_position": in_position,
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
        Question::BeatsMe(q) => beats_me_question(q),
        Question::ReadPrice(q) => read_price_question(q),
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
        Question::Table(q) => table_question(q),
        Question::Chain(q) => chain_question(q),
    }
}

/// One stage, as the player sees it. Each stage shows only what has been stated so
/// far, so an earlier stage can't give away a later one.
fn chain_question(q: &ChainQuestion) -> V {
    let h = &q.hand;
    let common = |stage: ChainStage| {
        vec![
            ("kind".to_string(), V::from("chain")),
            ("stage".to_string(), V::from(stage.code())),
            ("part".to_string(), V::from(stage.index() + 1)),
            ("parts".to_string(), V::from(CHAIN_STAGES as usize)),
            ("hero".to_string(), holding(&h.hero)),
            ("board".to_string(), cards(&h.board)),
            ("street".to_string(), V::from(street_name(h.board.len()))),
        ]
    };
    let mut fields = common(q.stage);
    let mut add = |key: &str, value: V| fields.push((key.to_string(), value));
    match q.stage {
        ChainStage::Hand => {}
        ChainStage::Outs => add("villain", holding(&h.villain)),
        ChainStage::Price => {
            add("pot_before", V::from(h.pot_before));
            add("bet", V::from(h.bet));
            add("callers", V::from(h.callers()));
        }
        ChainStage::Decide => {
            add("villain", holding(&h.villain));
            add("second", V::from(h.second.as_ref().map(|m| m.other_opponent())));
            add("pot", V::from(h.pot()));
            add("call", V::from(h.bet));
            add("all_in", V::from(matches!(h.mode, SpotMode::AllIn)));
            add("x", V::from(h.x()));
            add("next_card_line", V::from(chain_next_card_line(h)));
        }
    }
    V::Obj(fields)
}

fn chain_next_card_line(h: &ChainHand) -> String {
    match h.mode {
        SpotMode::AllIn => "All-in: there is no more betting and all remaining cards run out.".to_string(),
        SpotMode::NextCard { x: 0 } => {
            "You see one more card. If you then have the best hand you win the pot and nothing more; otherwise you give up.".to_string()
        }
        SpotMode::NextCard { x } => {
            format!("You see one more card. If you then have the best hand you collect about ${x} more; otherwise you give up.")
        }
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
    let verb = |plain: &str, third: &str| if you { plain.to_string() } else { third.to_string() };
    let what = match a.kind {
        TableActionKind::Fold => verb("fold", "folds"),
        TableActionKind::Check => verb("check", "checks"),
        TableActionKind::Call => format!("{} {amount}bb", verb("call", "calls")),
        TableActionKind::Bet => format!("{} {amount}bb", verb("bet", "bets")),
        TableActionKind::Raise => format!("{} to {amount}bb", verb("raise", "raises")),
    };
    format!("{} {what}", table_seat_name(a.seat, hero))
}

fn table_action_kind(kind: TableActionKind) -> &'static str {
    match kind {
        TableActionKind::Fold => "fold",
        TableActionKind::Check => "check",
        TableActionKind::Call => "call",
        TableActionKind::Bet => "bet",
        TableActionKind::Raise => "raise",
    }
}

fn table_question(q: &TableQuestion) -> V {
    let streets: Vec<V> = q
        .streets
        .iter()
        .map(|s| {
            let actions: Vec<V> = s
                .actions
                .iter()
                .map(|a| {
                    obj! {
                        "seat": table_seat_name(a.seat, q.seat),
                        "hero": a.seat == q.seat,
                        "kind": table_action_kind(a.kind),
                        "amount": (a.amount_cents > 0).then(|| format!("{}bb", bb_text(a.amount_cents))),
                        "text": table_action_text(a, q.seat),
                    }
                })
                .collect();
            obj! {
                "name": s.street.title(),
                "cards": cards(q.street_cards(s.street)),
                "pot": (s.street != TableStreetName::Preflop).then(|| format!("Pot {}bb", bb_text(s.pot_start_cents))),
                "actions": actions,
            }
        })
        .collect();
    obj! {
        "kind": "table",
        "players": q.players,
        "seat": q.seat.title(),
        "seat_sentence": q.seat.sentence(),
        "behind": format!("{}bb", bb_text(q.behind_cents)),
        "hero": holding(&q.hero),
        "board": cards(&q.board),
        "street": street_name(q.board.len()),
        "streets": streets,
        "pot": format!("{}bb", bb_text(q.pot_cents)),
        "call": (q.call_cents > 0).then(|| format!("{}bb", bb_text(q.call_cents))),
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

fn beats_me_question(q: &BeatsMeQuestion) -> V {
    let hands: Vec<V> = match &q.ask {
        BeatsMeAsk::Which(list) => list
            .iter()
            .zip('A'..='D')
            .map(|(h, letter)| obj! { "letter": letter.to_string(), "cards": holding(h) })
            .collect(),
        _ => Vec::new(),
    };
    obj! {
        "kind": "beats_me",
        "rung": u32::from(q.rung()),
        "hero": holding(&q.hero),
        "board": cards(&q.board),
        "street": street_name(q.board.len()),
        "hands": hands,
        "prompt": prompt_text(q),
    }
}

fn read_price_question(q: &ReadPriceQuestion) -> V {
    let set_price = matches!(q.ask, ReadPriceAsk::SetPrice { .. });
    let context: Vec<V> = read_price::context_lines(q).into_iter().map(V::from).collect();
    obj! {
        "kind": "read_price",
        "rung": u32::from(q.rung()),
        "pot_before": q.pot_before,
        "bet": (!set_price).then_some(q.bet),
        "callers": q.callers,
        "context": context,
        "prompt": read_price::prompt_text(q),
    }
}

fn standing_words(standing: Standing) -> &'static str {
    match standing {
        Standing::Ahead => "ahead of you",
        Standing::Tied => "tied with you",
        Standing::Behind => "behind you",
    }
}

fn standing_verb(standing: Standing) -> &'static str {
    match standing {
        Standing::Ahead => "beats",
        Standing::Tied => "matches",
        Standing::Behind => "loses to",
    }
}

fn beats_me_reveal(reveal: &BeatsMeReveal) -> V {
    let no_hands = V::Arr(Vec::new());
    let no_examples = V::Arr(Vec::new());
    match reveal {
        BeatsMeReveal::Which { hero_rank, hands, answer } => {
            let shown: Vec<V> = hands
                .iter()
                .map(|h| {
                    obj! {
                        "letter": h.letter.to_ascii_uppercase().to_string(),
                        "cards": holding(&h.holding),
                        "text": format!("has {}: {}, because it {} {}.", h.rank.describe(), standing_words(h.standing), standing_verb(h.standing), hero_rank.describe()),
                    }
                })
                .collect();
            let letters: Vec<String> = answer.chars().map(|c| c.to_ascii_uppercase().to_string()).collect();
            let answer_text = if answer == "none" { "None of them".to_string() } else { format!("Hand {}", letters.join(", ")) };
            obj! {
                "kind": "beats_me",
                "answer": answer_text,
                "hero": describe(*hero_rank),
                "lines": V::Arr(Vec::new()),
                "hands": shown,
                "examples": no_examples,
            }
        }
        BeatsMeReveal::Kinds { hero_rank, rows } => {
            let examples: Vec<V> = rows
                .iter()
                .map(|r| obj! { "label": r.kind.label(), "cards": holding(&r.example), "ways": r.ways })
                .collect();
            let answer = if rows.is_empty() {
                "Nothing beats you".to_string()
            } else {
                rows.iter().map(|r| r.kind.label()).collect::<Vec<_>>().join(", ")
            };
            obj! {
                "kind": "beats_me",
                "answer": answer,
                "hero": describe(*hero_rank),
                "lines": V::Arr(Vec::new()),
                "hands": no_hands,
                "examples": examples,
            }
        }
        BeatsMeReveal::Ways { high, low, count, hero_rank } => {
            let lines: Vec<V> = ways_method(*high, *low, *count).into_iter().map(V::from).collect();
            obj! {
                "kind": "beats_me",
                "answer": format!("{} ways", count.ways),
                "hero": describe(*hero_rank),
                "lines": lines,
                "hands": no_hands,
                "examples": no_examples,
            }
        }
        BeatsMeReveal::Mix { standing, mix, hero_rank } => {
            let lines = vec![V::from(mix_method(standing)), V::from(format!("Hands counted: {}.", mix.listed()))];
            obj! {
                "kind": "beats_me",
                "answer": standing.band.label(),
                "hero": describe(*hero_rank),
                "lines": lines,
                "hands": no_hands,
                "examples": no_examples,
            }
        }
    }
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
    let opponents: Vec<V> = q
        .opponents
        .iter()
        .enumerate()
        .map(|(i, mix)| V::from(mix.assumption(many.then_some(i + 1))))
        .collect();
    let label = if q.opponents.iter().any(|m| m.player.is_some()) { "" } else { label };
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
        AnswerFormat::MultiSelect { options, none_code, none_label } => obj! {
            "type": "multi",
            "options": options.iter().map(option).collect::<Vec<_>>(),
            "none_code": none_code,
            "none_label": none_label,
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
        AnswerFormat::MultiSelect { options, none_code, none_label } => {
            if answer == none_code {
                return none_label.to_lowercase();
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

fn suit_word(letter: char) -> &'static str {
    match letter {
        's' => "spade",
        'h' => "heart",
        'd' => "diamond",
        _ => "club",
    }
}

/// `ahkh`, `aks`, `qj`, `ahx` or `ax`, written the way a player writes them.
fn holding_text(answer: &str) -> String {
    let rank = |c: char| c.to_ascii_uppercase();
    let chars: Vec<char> = answer.chars().collect();
    match chars.as_slice() {
        [a, x] if *x == 'x' => format!("{} with any other card", rank(*a)),
        [a, s, x] if *x == 'x' => format!("{}{s} with any other card", rank(*a)),
        [a, s, x, t] if *x == 'x' => format!("{}{s} with any other {}", rank(*a), suit_word(*t)),
        [a, s, b, t] => format!("{}{s} {}{t}", rank(*a), rank(*b)),
        _ => chars.iter().enumerate().map(|(i, &c)| if i < 2 { rank(c) } else { c }).collect(),
    }
}

/// `response_ms` is 0 when the answer is graded ahead of time; only `board_type`
/// is timed, and it is graded when the answer arrives.
pub fn outcome(item: &Item, o: &Outcome, attempt_template: String, response_ms: u32, reflection: V) -> V {
    obj! {
        "correct": o.correct,
        "close": o.close,
        "ok": o.ok(),
        "picked": picked(&item.question, &o.answer),
        "reveal": reveal(item, o, response_ms),
        "attempt": attempt_template,
        "tie_line": o.chain_tie.clone(),
        "reflection": reflection,
    }
}

/// The three prompts of a reflection form, ready to show.
pub fn reflection_form(prompts: &[Prompt]) -> V {
    V::Arr(
        prompts
            .iter()
            .map(|p| {
                let cues: Vec<V> = p.cues.iter().map(|c| obj! { "id": c.id, "text": c.text.as_str() }).collect();
                obj! { "id": p.id, "text": p.text, "cues": cues }
            })
            .collect(),
    )
}

fn describe(rank: HandRank) -> V {
    V::Str(rank.describe())
}

fn reveal(item: &Item, o: &Outcome, response_ms: u32) -> V {
    match &o.reveal {
        Reveal::Preflop { seat, situation, action, reason } => obj! {
            "kind": "preflop",
            "answer": action_label(*action, *seat, *situation),
            "why": preflop_why(*seat, *action, *reason),
            "chart": preflop_chart_line(*reason),
        },
        Reveal::Nuts { analysis, used_x, consistent, consistent_nuts } => {
            let next = analysis.next.as_ref().map(|(rank, list)| {
                obj! { "rank": describe(*rank), "holdings": holdings_short(list, 6) }
            });
            obj! {
                "kind": "nuts",
                "top": analysis.any_of_suit.map_or_else(
                    || describe(analysis.top),
                    |(card, suit)| V::from(format!("{}, {}", analysis.top.describe().split(" (").next().unwrap_or(""), any_of_suit_text(card, suit))),
                ),
                "grouped": analysis.any_of_suit.is_some(),
                "holdings": holdings_short(&analysis.top_holdings, 6),
                "by_card_removal": (analysis.any_of_suit.is_none() && !analysis.by_card_removal.is_empty())
                    .then(|| holdings_short(&analysis.by_card_removal, 6)),
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
                "random_note": BEATS_RANDOM_NOTE,
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
        Reveal::Spot { analysis: a, chosen, best, loss_pct, loss_money, band, flip } => {
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
                "groups": V::Arr(a.groups.iter().map(|g| V::from(g.as_str())).collect()),
                "flip_call": flip.and_then(|f: FlipFacts| f.break_even_call).map(|c| format!("Calling stops being right above about ${c:.0}.")),
                "flip_cards": flip.and_then(|f: FlipFacts| f.cards_ahead).map(|(ahead, unseen)| format!("{ahead} of {unseen} next cards put you ahead of the price.")),
            }
        }
        Reveal::Table(graded) => table_reveal(graded),
        Reveal::BeatsMe(reveal) => beats_me_reveal(reveal),
        Reveal::ReadPrice(r) => obj! {
            "kind": "read_price",
            "answer": r.answer.as_str(),
            "lines": r.lines.iter().map(|l| V::from(l.as_str())).collect::<Vec<V>>(),
        },
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

fn band_text(band: EvBand) -> &'static str {
    match band {
        EvBand::Close => "close, so it counts as correct",
        EvBand::Small => "a small error",
        EvBand::Mistake => "a mistake",
        EvBand::Blunder => "a blunder",
    }
}

fn table_reveal(g: &TableGrade) -> V {
    let spot = g.spot;
    let options: Vec<V> = spot
        .options
        .iter()
        .enumerate()
        .map(|(i, option)| {
            obj! {
                "label": option.label.as_str(),
                "ev_bb": format!("{}bb", ev_bb_text(option.ev_milli_bb)),
                "ev_pct": format!("{:.1}% of the pot", f64::from(option.ev_pct_tenths) / 10.0),
                "share": format!("best at {:.1}% of simulated tables", f64::from(option.best_share_milli) / 10.0),
                "is_best": i == spot.best,
                "is_chosen": i == g.chosen,
            }
        })
        .collect();
    let archetypes: Vec<String> = if spot.verdict == Verdict::Depends {
        spot.archetypes.iter().map(|row| row.sentence(spot)).collect()
    } else {
        Vec::new()
    };
    let loss = (g.chosen != spot.best).then(|| g.loss_text());
    let read_note = g.read_note();
    obj! {
        "kind": "table",
        "verdict": spot.verdict.code(),
        "explanation": spot.verdict.explanation(),
        "note": ASSUMPTION_NOTE,
        "best": g.best().label.as_str(),
        "chosen": g.chosen_option().label.as_str(),
        "matched": g.chosen == spot.best,
        "options": options,
        "loss": loss,
        "band": g.band.map(band_text),
        "read_note": read_note,
        "archetypes": archetypes,
    }
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
    if s.skill == Skill::Chain {
        return format!("{:.0}% of stages right", s.score);
    }
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

fn group_row(label: &str, group: &ReflectionGroup) -> V {
    obj! {
        "label": label,
        "right": format!("{} of {}", group.right, group.misses),
        // No misses means no share: a 0% here would read as a result.
        "pct": if group.misses == 0 { "-".to_string() } else { format!("{:.0}%", group.pct()) },
    }
}

/// How the misses split by form, with the plain "one player's noisy data" framing.
pub fn reflection_comparison(c: &ReflectionComparison) -> V {
    let enough = c.enough_data();
    let summary = enough.then(|| {
        format!(
            "After reflecting, the next similar question was right {:.0}% of the time ({} misses). With no form it was right {:.0}% of the time ({} misses).",
            c.reflected.pct(),
            c.reflected.misses,
            c.not_offered.pct(),
            c.not_offered.misses,
        )
    });
    obj! {
        "enough_data": enough,
        "counted": c.counted,
        "needed": REFLECTION_MIN_MISSES,
        "waiting": c.waiting,
        "rows": vec![
            group_row("Reflected", &c.reflected),
            group_row("Shown, skipped", &c.skipped),
            group_row("Form not shown", &c.not_offered),
        ],
        "summary": summary,
    }
}

/// The three levels, as the confidence prompt offers them.
pub fn confidence_levels() -> V {
    V::Arr(
        Confidence::ALL
            .into_iter()
            .map(|level| {
                obj! {
                    "code": level.code(),
                    "label": confidence_name(level),
                    "hint": format!("about {:.0}%", level.treated_pct()),
                }
            })
            .collect(),
    )
}

fn confidence_name(level: Confidence) -> &'static str {
    match level {
        Confidence::Low => "Low",
        Confidence::Medium => "Medium",
        Confidence::High => "High",
    }
}

/// The reliability table: rows with enough ratings, and how close the rest are.
pub fn reliability(rows: &[ReliabilityRow]) -> V {
    let ready: Vec<V> = rows
        .iter()
        .filter(|r| r.ratings >= RELIABILITY_MIN_RATINGS)
        .map(|r| {
            obj! {
                "skill": skill_title(r.skill),
                "level": confidence_name(r.level),
                "treated": format!("{:.0}%", r.level.treated_pct()),
                "right": format!("{:.0}%", r.right_pct()),
                "ratings": r.ratings,
                "sentence": format!("When you said {} on {}, you were right {:.0}% of the time.", confidence_name(r.level), skill_title(r.skill), r.right_pct()),
            }
        })
        .collect();
    obj! {
        "enough_data": !ready.is_empty(),
        "needed": RELIABILITY_MIN_RATINGS,
        "most": rows.iter().map(|r| r.ratings).max().unwrap_or(0),
        "rows": ready,
    }
}

/// `300000` as `300,000`.
fn with_commas(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
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
}

/// The chart a reveal came from, written out, or nothing for the older charts.
fn preflop_chart_line(reason: PreflopReason) -> Option<String> {
    match reason {
        PreflopReason::Chart { chart, .. } => Some(chart.summary()),
        _ => None,
    }
}

fn preflop_why(seat: Seat, action: Action, reason: PreflopReason) -> String {
    let open_line = CARD.open_range(seat).map_or("", |l| l.text);
    let open_name = opening_range_name(seat);
    let (raise_iso, limp) = (CARD.iso_raise.text, CARD.iso_limp.text);
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
        PreflopReason::Chart { chart, kind } => why_line(chart, action, kind).to_string(),
    }
}
