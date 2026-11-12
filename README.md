# poker-trainer

Daily 10-15 minute fundamentals drills for no-limit hold'em home games: reading
boards, counting outs, pot odds, estimating equity and call-or-fold spots, with exact answers. It
assumes loose, unknown opponents, not GTO ones. No win rates and no solver.

It is a pure Rust library (no I/O or clock, so it can be compiled to WASM) plus a
thin command-line front end, with no external crates. The previous app is at tag
`archive/preflop-app`.

## Build and run

    cargo build --release
    ./target/release/poker-trainer drill

Tests: `cargo test --release`, and `cargo test --release -- --ignored` for the
exhaustive 7-card count and regenerating the anchor deck with its full preflop simulation.

## Daily routine

1. Run `poker-trainer baseline` once to get a starting score, then again every few weeks.
2. Run `poker-trainer drill` for 10 minutes each day.
3. Read every reveal. It explains why, which is the point.
4. About once a month, take the mixed checkup with `poker-trainer check`. It is optional.
5. Check `poker-trainer progress` now and then. Every drill is open from the first day.

## Commands

- `baseline`: 32 frozen questions in shuffled order with a random suit permutation.
  No feedback until the end, then scores per section and part (board reading; odds and outs;
  decisions, the sections of the old steps, kept so runs stay comparable) against earlier runs. Four `beats` percentage questions (b13-b16) are retired: rows answering them stay in the log but no run, old or new, is scored on them.
- `card`: prints the preflop card and its notes.
- `drill [--skill NAME]... [--minutes N] [--items N] [--seed N]`: defaults to 10 minutes.
  `--skill` deals only the named skills. Without it, every practice group is dealt at its
  fixed share (see "Practice groups" below).
- `check`: the mixed held-out checkup, 40 items across the groups with no feedback until
  the end, then a score by group. `check --skill NAME` takes the older one-skill checkup:
  40 new items plus 10 repeats, scored against the skill's pass mark.
- `progress`: last-40 stats against the pass marks, baseline and checkup history, and how
  your equity estimates are biased once there are 30 of them.

Skills are `preflop`, `nuts`, `possible`, `made`, `beats`, `beats_me`, `board_type`, `outs`, `price`,
`hit`, `equity`, `anchor`, `spot`, `spot_mw`, `chain`, `table` and `read_price`. `price` and `beats`
are no longer dealt by default: their history still loads and shows, and `--skill` still asks for
them. Type `q` at any question to quit; every answer is already saved.

## Practice groups

Every drill is open from the first day; there are no steps or gates between skills. The ladders
inside a skill (`read_price`, `beats_me`, `equity`) work as before. The default drill deals seven
groups at fixed shares of the items (`src/groups.rs`, starting guesses):

| Group | Share | Skills (weight inside the group) |
|---|---|---|
| Preflop | 20% | `preflop` |
| Read the price | 15% | `read_price` |
| Equity and anchors | 15% | `equity` (3), `anchor` (1) |
| Whole hands | 15% | `chain` |
| Board reading | 15% | `nuts`, `possible`, `made`, `board_type`, `beats_me` |
| Outs | 10% | `outs` (3), `hit` (2) |
| Spots | 10% | `spot` (2), `spot_mw` (2), `table` (1) |

Each new item's group is picked at random, weighted by how far the group is behind its share of
the items dealt so far plus one item of slack (`SHARE_SLACK_ITEMS`), so the shares hold at every
point of a session, not just on average. Retries, reviews and a whole hand's four stages count
toward their group's share. Inside the group a skill is picked by its weight times `0.1 + miss
rate`. The rest of the scheduling is unchanged: `preflop` never drops below its 20% share
(`MIN_PREFLOP_PERCENT`); a `chain` picked when its four stages would break that share waits and
starts after the next one or two `preflop` items; missed items come back once in the session;
due families are reviewed; and no skill comes three times running except a hand's stages.

On the phone, Home also offers each group on its own. A one-group drill deals only that group's
skills, with the same weights and no shares or floor.

A skill's pass mark, once reached over 40 drill attempts in a row, stays reached: a later bad
run shows as below the mark for now, never as a lost pass.

Preflop drills are mostly 4-5 players with occasional 6-7. Your seat comes from how
many players are still to act behind you.

The card has one action for every hand in every spot, with no mixing. Besides opening and
limpers (where the cutoff and button also raise some limp-along hands) it covers one raise (from
an early or middle seat, from the cutoff or button, or from the blinds, with extra calls closing
the action from the big blind), a raise with callers (again with extra big blind calls), being
3-bet after you opened (in position or out of position) and being 4-bet after you 3-bet.
Hands not listed fold. The ranges are rules of thumb for a loose game with few light
3-bets, so raises are for value. You 3-bet, but against a 3-bet or a 4-bet only kings or
aces continue (4-bet or all in) and everything else folds, queens and ace-king included.
Drills deal hands you could really hold: after opening, the seat's open range; after
3-betting, the hands the chart 3-bets. Facing a 3-bet or a 4-bet, a third of the hands are
kings or aces and the rest are tempting folds (tens or better, ace-king, ace-queen, ace-jack
suited, king-queen suited), never trash. About a third of preflop items are folded to you,
a fifth limpers, a quarter a lone raise and a tenth a raise with callers; 3-bets (about 4%)
and 4-bets (about 1.5%) are rare.

Attempts go to `$POKER_TRAINER_HOME/attempts.tsv` (default `~/.poker-trainer/`), an
append-only log. Colour is used only on a terminal with `NO_COLOR` unset.

## What the spot grading assumes

`spot` and `spot_mw` state each opponent as a tight, average or loose player and a mix of
hands that player could hold, such as "They're a loose player. Assume they have one of: top
pair, second pair, a flush draw, or nothing much." Your share is averaged over every
card-disjoint combo in the mix and every remaining card, so the actual next card doesn't
matter, and the reveal names the groups used. Your hand is always worth a real decision: a
pair or better, a flush draw, an open-ended straight draw, or a gutshot with two overcards.
Call and fold are each right about half the time: the deal picks a side of break-even,
then a bet that puts the price on that side and within 20 points of break-even. Hands where
your all-in equity is under 15% or over 85% are left out. The frozen baseline keeps its
single stated hand class, and a chain's second opponent is a mix like these.

- All-in spots: no more betting, all remaining cards run out, X = 0.
- Next-card spots: you see one more card. If you then have the best hand you collect
  about $X more, otherwise you give up. X stands in for all later betting.

`EV(call) = share * (P + C + X) - C` and `EV(fold) = 0`, where P is the pot before you
act, including the bet and earlier callers, and C is the amount to call. Mistakes are
graded by EV lost as a percentage of P. Very large multiway cases use a deterministic
300,000-outcome sample, and the reveal says so.

A skill passes once 40 drill attempts in a row meet its pass mark, and stays passed;
checkups and baseline answers are scored separately and gate nothing.
Pass marks, EV bands and the other thresholds are starting guesses, kept as named
constants in the modules that use them, to be tuned with real data.

## Checkups

Seeds with bit 63 set are held out: drills and reviews never serve them. Attempts are logged
in `check` mode and never count toward drill stats, miss rates or reviews. No checkup gates
anything.

The monthly checkup is mixed: 40 held-out items across the groups, near the drill's shares
(preflop 8, read the price 7, equity 5 and anchors 2, one whole hand of 4 stages, board reading
6, outs 4, spots 4; `MIXED_CHECKUP_MIX` in `src/checkup.rs`). `table` is left out. A hand's
stages stay together and in order; there are no repeats. It is scored as the percent right
(close answers count) with a standard error, and by group. Its forms are stored from
`MIXED_FORM_BASE` (1,000,000) up, so its rows need no new log column. Home offers it, as an
option, when one is under way (to resume), 30 days after the last completed one ended
(`MIXED_CHECKUP_EVERY_DAYS`), or, before the first, once there are 100 drill answers and the
first was at least 7 days ago. A run begun before a generator change to any skill in it can't
be resumed.

The older one-skill checkup is kept for history and the command line: 40 new held-out items
and 10 repeats placed at least 10 items after their originals, all fixed by the skill and
form number. Each new checkup uses the next form. The score uses only the 40 originals, with
the skill's own measure and a standard error; the repeats report how much your answers agree
with themselves. Rows of these checkups from earlier releases still load and score.

Forms are distinct in seed, not always in content. `price` and `hit` have too few
distinct questions to avoid overlap with drills, and `anchor` is a fixed deck, so its
checkups are the same 40 cards reshuffled. `hit` has only 24 distinct questions, so
every form contains duplicates among its 40 originals and its reported standard error
is optimistic. The 32 frozen baseline items predate the reservation and 9 of their
seeds have bit 63 set; they are served from the file and are never used as checkup
seeds. Drill attempts logged with bit 63 set, from before the reservation, are skipped
for review.

## New skills

- `nuts`: a holding is the nuts if no other holding disjoint from it, given the
  board, beats it, so card removal can make a lesser holding unbeatable (K7 on K-K-7).
  Name a holding, a card or rank plus any card (`Ahx`, `Ax`), or a card plus any card of a
  suit (`Asxs`, the ace of spades with any spade). Forms that say "any" must hold for
  every unseen holding they describe. Others count if one holding they describe is the
  nuts.

- `equity`: estimate your all-in equity with 1 to 3 opponents, as a percent.
  Rung 0 shows exact hands; rung 1 states classes as in `spot`, averaged over every
  card-disjoint combo. Hits stay near 86% through a tolerance staircase per rung
  (start 10 points; tighten 1 on a hit, minimum 2; loosen 6 on a miss, maximum 20).
  Moves to rung 1 by the ladder points rule below: rung 0 starts at 30 points, each
  first-try drill estimate adds 8 less its error in whole points (an error over 20 counts
  as 20), and 110 moves the player up. Pass mark:
  mean absolute error at most 6 over 40. Checkups mix 20 of each rung at a fixed band of 5.
- `anchor`: 40 memorized numbers in `data/anchors_v1.txt`, covering bet prices,
  flop matchups and preflop equities. Leitner boxes of 1, 3, 7, 14 and 30 days, at most
  5 new cards per session. Pass mark: 90% within tolerance over 40.
- `beats_me` ("What beats you?"): a ladder that replaced `beats`.
  Rung 0 shows four specific hands to pick from, rung 1 asks which kinds of hand beat you,
  rung 2 asks how many ways a named holding type can be held, and rung 3 asks if you are
  ahead of a stated mix of hands (over 60%, 40-60%, under 40%, with ties counting half).
  A rung is passed by the ladder points rule below, and the ladder only goes up. Pass
  mark: at least 85% correct over the last 40 attempts at rungs 2-3. Checkups take 10 items per rung from held-out seeds.
- `beats` is no longer dealt; its rows still load and show on Progress.
  Its reveal notes that the percentage is against random hands.
- `board_type`: name a flop's suit and rank pattern from four choices. Eight
  categories exist, because a paired flop cannot be monotone. Answers over 12 seconds
  count as misses. A category retires after 3 straight correct answers inside 6 seconds,
  and comes back if a later answer is wrong or slower than 6 seconds. Pass mark: 95%
  correct, median at most 6 seconds.

## Read the price

`read_price` (web title "Read the price") is its own practice group, 15% of the default
drill's items; later-day reviews take at most half of its slots
(`READ_PRICE_REVIEW_SHARE_DIVISOR`), so new items at your rung keep coming. Amounts are whole dollars: the pot before the bet is $3 to $300, mostly $6 to $60,
and bets run from $1 to two and a half times the pot, tiny stabs and overbets included.
About 40% of items on rungs 0-3 have one or two callers, whose money goes in the pot, and on
most of those the amounts are chosen so that leaving the callers out changes the answer. With a
caller an expensive price can't be reached at these bet sizes, so at rung 0 callers appear only
on the cheap and fair bands. No board or hole
cards are shown, so its checkups have nothing to relabel. A five-rung ladder, by the seed:

- Rung 0: cheap (under 20% needed), fair (20% to under 30%) or expensive (30% or more). Never
  within 1.5 points of a line, and roughly a third each. The reveal shows the sum and the shortcut.
- Rung 1: the percent needed, within 2 points (`PERCENT_TOLERANCE`, as `price`).
- Rung 2: Easy call, Close or Fold with the hand's share stated: a named draw or a read in steps
  of 5. Easy call is at least 8 points above the price, Fold at least 8 below, Close within 4.
  Nothing between is dealt, and dealt hands are half a point further inside each line so
  rounding a percent never changes the answer. Roughly a third each.
- Rung 3: the same, draws only, without the share. Draws use the exact chance on the turn
  (one card, n/46) or all-in on the flop (two cards).
- Rung 4: hero bets into a pot against one opponent on a turn draw. Pick the smallest of four
  bets that makes their call a mistake, ignoring later bets. No option is within 1.5 points of
  break-even, wrong options stay within 10 points of it, and the right one is equally often
  first, second, third or last.

Later-day reviews take at most half of a session's `read_price` slots.
A rung is passed by the ladder points rule below, and the ladder only goes up.
The drill mark is 85% over the last 40 attempts at rungs 2-4; checkups take 8 items per rung
from held-out seeds. Reflection forms appear on missed rung 2 and 3 decisions; confidence
taps on any rung. Adding this skill doesn't change any existing seed's item.

## Moving up a ladder

`read_price`, `beats_me` and `equity` share one rule (`src/ladder.rs`). Each first-try drill
answer at the player's rung moves a points count that starts at a fixed number on every rung and
never goes below 0; the first time it reaches the goal the player moves up for good, so a review
of an old item can't demote. On `read_price` and `beats_me` a rung starts at 10, a right answer
adds 1, a miss takes off 4, and 32 moves the player up (22 straight right from a fresh rung).
`equity` scores error size instead, as above. Progress shows the count.

The rule it replaces, any 20 straight attempts at 85% (any 30 averaging within 8 points for
`equity`), was checked after every attempt, so luck added up: a player right 76% of the time
passed a rung within 100 attempts about 96% of the time. Simulated over 20,000 players per row,
the share who move up within 60 and within 100 attempts at a rung:

| Right | Within 60 | Within 100 |
|-------|-----------|------------|
| 70%   | 0.4%      | 0.7%       |
| 76%   | 3.9%      | 8.0%       |
| 80%   | 13%       | 27%        |
| 85%   | 42%       | 73%        |
| 90%   | 83%       | 98%        |
| 95%   | 99%       | 100%       |

For `equity`, with errors spread like a half-normal or an exponential, a mean error of 6 moves up
within 60 attempts 91-93% of the time and a mean error of 10 within 100 attempts at most 5%; the
old rule moved a mean error of 10 up within 100 attempts 48-69% of the time.

Attempts logged before `OLD_RULE_CUTOFF_MS` (2026-11-13 00:00 UTC) still count under the old
rule too, so nobody drops a rung they had reached. The numbers are starting guesses.

## Table spots

`table` deals realistic decisions from `data/spots_v3.txt`: 3,000 spots already graded
offline by the engine in `engine/`. Each verdict is about simulated opponents, drawn from a
50/50 per-table mixture of two invented populations, not real players; `engine/README.md`
says what the verdicts do and don't mean, and every reveal repeats the short version.

Spot ids are not stable between libraries, so `table` rows record the library in their
`gen_version` column (1003 for `spots_v3`, always above any generator version); rows from the earlier library keep the old value,
still count toward pass marks and earlier checkup scores, and are ignored by the no-repeat
window and the choice of the next checkup form.

A spot is chosen by seed, and a seed-derived suit permutation relabels your hand and the
board, so a spot isn't remembered by its suits. The answer is the option's code (`fold`,
`check`, `call`, `bet33`, `bet67`, `bet100`, `raise25.2`). The 1% rule below compares the
options' EVs in big blinds over the pot; losses of 1% or more use the file's printed EV as a
percent of the pot:

- `robust`: correct if it is the best option or within 1% of the pot of it.
- `close`: correct if within 1% of the pot of the best option.
- `depends`: a read question. Correct if the option is best at 15% or more of simulated
  tables, or within 1% of the pot of the best; other options are wrong.

Drills deal about 45% robust, 40% depends, 15% close and don't repeat a spot seen in the last
200 attempts while unseen spots remain, so missed spots are not asked again in the session or
queued for review. Robust and close spots with `id % 3 == 0` are reserved for checkups (245
spots); depends spots are never in checkups. A checkup is 26 robust and 14 close reserved
spots plus 10 suit-permuted repeats, fixed by the form number. The reserved spots last five
forms; from the sixth on, forms reuse some spots from earlier ones, and the result screen
says so.

Pass mark (starting guesses): over the last 40 graded attempts on robust or close spots, mean
EV loss at most 3% of the pot and at most 2 blunders. Depends attempts do not count toward it.
`table` is dealt in the spots group at the lowest weight there.

The library is produced by the engine's `library` binary (`POKER_POPULATION=mixture library
20261109 8000000 16000 5000 3000 30 target/library/spots_v3.txt 2 2` in `engine/`, about 80
minutes on the 16-core desktop) and copied here. The format is documented in the file's header.
It is compiled into the library with `include_str!` and parsed strictly on first use. To replace it, rerun
that, copy the new file over, and update the expected counts in `tests/table.rs`.

## Review by family, and other research-ranked additions

The additions in this section come from a research report that ranked small changes likely
to help a player weak at reading boards, pot odds and equity. None has been tested on poker
learners, so they are built to be judged later, and their thresholds are named constants
documented as starting guesses in the modules that use them.

Missed items no longer return as the same seed. `family_of` gives each item a family (a
`made` answer class, a `price` bet size, an `outs` band with whether a tempting card exists,
and so on), logged in a new column. A same-session retry and a later-day review deal a fresh
sibling: a new non-held-out seed in the same family, found by bounded rejection sampling (200
tries, 3,000 for `preflop`, and at most 3 exact equities for `equity`). If none turns up the player gets a plain new
item of that skill, and a later-day family waits for another session. `anchor`, `board_type`, `table` and `chain` keep
their own scheduling. Later-day review is per (skill, family): due 1, 3, 7, 14 and 30 days
after each correct review, reset to 1 day by a miss, retired after a correct 30-day review,
and a retired family can come back on a new miss. Among due families the slowest relative to
the skill's median come first. Rows logged before families existed are not reviewed.
A generator change resets queued reviews and unfinished checkups only for the skills it changed:
`spot`, `spot_mw` and `equity` at version 2, and `preflop` at versions 3 and 4. `table` checkups reset
when the spot library changes instead.

Spot reveals add library-computed flip facts: the largest call that breaks even, from
`C* = share * (P + X) / (1 - share)`, and on heads-up spots how many next cards put hero
ahead of the price. They never change the grade.

In the web app, a wrong `spot` or `spot_mw` answer, and a `table` robust spot that wasn't the
best play, may offer three one-tap reflection prompts with cues drawn from the item's facts.
Half of eligible misses are offered the form, by a pure function of the seed. Reflecting never
blocks the drill and adds no response time. A reflection is logged as its own row (mode
`reflect`), which everything that counts attempts ignores. Once 60 misses have had a later
sibling attempted, Progress compares how often those siblings were right after answering the
form and after no form; it is one player's noisy data and says so. Reflection and confidence
are web-only; the terminal drill neither shows them nor logs them as offered.

`chain` (the whole hands group) plays one hand through four stages, each committed before its reveal and
logged as one row with a stage column: name your hand, count outs against a stated opponent
hand, the equity a stated price needs, then call or fold with the all-in or next-card
assumption stated. Hands are ones the preflop card plays, with hero behind and at least 4
outs, 1 or 2 opponents and varied bet sizes. Pass mark: 85% of the last 40 stages right, outs
within 1 counting. A checkup counts stages as items: 10 hands (40 stages) plus 10 repeated
stages placed between hands. A missed stage is not retried or reviewed; the next chain is a
new hand.

Optional confidence tap (Home, default off): after answering `made`, `outs`, `price`,
`read_price` or `equity` in a drill, one tap for Low, Medium or High (treated as 55%, 75%, 95%) comes before
the reveal. Progress shows a reliability table by level and skill once a row has 30 ratings.
It is not asked on `spot`, `spot_mw` or `table`, whose decks are near break-even by design.

New log columns (`family`, `stage`, `confidence`, `reflect`) go after the table columns; older
rows, with 11, 18 or 20 columns, still parse.

## Web app

`index.html` and `app/` are a phone-first web app over the same library, compiled to
`app/trainer.wasm`; JavaScript holds no poker rules or grading. Rebuild it with
`web/build.sh` (which also stamps the `sw.js` cache version), and test with
`web/test/run.py` and `web/test/mutation_check.py`. The browser tests need headless Chrome,
and they fail if the machine's cores are saturated.

Pass marks, EV bands, deal mixes and tolerances in this README are starting guesses.
