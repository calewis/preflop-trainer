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
4. When a skill's drill stats reach its pass mark, take `poker-trainer check --skill NAME`.
5. Check `poker-trainer progress` now and then. Each step unlocks once the one before it passes.

## Commands

- `baseline`: 36 frozen questions in shuffled order with a random suit permutation.
  No feedback until the end, then scores per section and step against earlier runs.
- `card`: prints the preflop card and its notes.
- `drill [--skill NAME]... [--minutes N] [--items N] [--seed N]`: defaults to 10 minutes.
  `--skill` deals only the named skills. Without it, `preflop` plus every skill up to
  your current step is in rotation.
- `check --skill NAME`: a held-out checkup. 40 new items plus 10 repeats, no feedback
  until the end, then a score with its standard error.
- `progress`: last-40 stats against the pass marks, current step, baseline and checkup
  history, and how your equity estimates are biased once there are 30 of them.

Skills are `preflop`, `nuts`, `possible`, `made`, `beats`, `board_type`, `outs`, `price`,
`hit`, `equity`, `anchor`, `spot`, `spot_mw` and `table`. Type `q` at any question to quit; every
answer is already saved.

Preflop drills are mostly 4-5 players with occasional 6-7. Your seat comes from how
many players are still to act behind you.

Attempts go to `$POKER_TRAINER_HOME/attempts.tsv` (default `~/.poker-trainer/`), an
append-only log. Colour is used only on a terminal with `NO_COLOR` unset.

## What the spot grading assumes

`spot` and `spot_mw` state what each opponent holds as a hand class, and grade against
that assumption. Your share is averaged over every combo in the class and every
remaining card, so the actual next card doesn't matter.

- All-in spots: no more betting, all remaining cards run out, X = 0.
- Next-card spots: you see one more card. If you then have the best hand you collect
  about $X more, otherwise you give up. X stands in for all later betting.

`EV(call) = share * (P + C + X) - C` and `EV(fold) = 0`, where P is the pot before you
act, including the bet and earlier callers, and C is the amount to call. Mistakes are
graded by EV lost as a percentage of P. Very large multiway cases use a deterministic
300,000-outcome sample, and the reveal says so.

A skill passes only after 40 drill attempts that meet its pass mark and a most recent
completed checkup that meets the same standard; baseline answers are scored separately.
Pass marks, EV bands and the other thresholds are starting guesses, kept as named
constants in the modules that use them, to be tuned with real data.

## Checkups

Seeds with bit 63 set are held out: drills and reviews never serve them. A checkup is
one skill, 40 new held-out items and 10 repeats placed at least 10 items after their
originals, all fixed by the skill and form number. Each new checkup uses the next form.
Attempts are logged in `check` mode and never count toward drill stats, miss rates or
reviews. The score uses only the 40 originals, with the skill's own measure and a
standard error; the repeats report how much your answers agree with themselves.

Forms are distinct in seed, not always in content. `price` and `hit` have too few
distinct questions to avoid overlap with drills, and `anchor` is a fixed deck, so its
checkups are the same 40 cards reshuffled. `hit` has only 24 distinct questions, so
every form contains duplicates among its 40 originals and its reported standard error
is optimistic. The 36 frozen baseline items predate the reservation and 12 of their
seeds have bit 63 set; they are served from the file and are never used as checkup
seeds. Drill attempts logged with bit 63 set, from before the reservation, are skipped
for review.

## New skills

- `equity` (step 2): estimate your all-in equity with 1 to 3 opponents, as a percent.
  Rung 0 shows exact hands; rung 1 states classes as in `spot`, averaged over every
  card-disjoint combo. Hits stay near 86% through a tolerance staircase per rung
  (start 10 points; tighten 1 on a hit, minimum 2; loosen 6 on a miss, maximum 20).
  Moves to rung 1 once 30 straight rung-0 attempts average within 8 points. Pass mark:
  mean absolute error at most 6 over 40. Checkups mix 20 of each rung at a fixed band of 5.
- `anchor` (step 2): 40 memorized numbers in `data/anchors_v1.txt`, covering bet prices,
  flop matchups and preflop equities. Leitner boxes of 1, 3, 7, 14 and 30 days, at most
  5 new cards per session. Pass mark: 90% within tolerance over 40.
- `board_type` (step 1): name a flop's suit and rank pattern from four choices. Eight
  categories exist, because a paired flop cannot be monotone. Answers over 12 seconds
  count as misses. A category retires after 3 straight correct answers inside 6 seconds,
  and comes back if a later answer is wrong or slower than 6 seconds. Pass mark: 95%
  correct, median at most 6 seconds.

## Table spots (step 4)

`table` deals realistic decisions from `data/spots_v2.txt`: 3,000 spots already graded
offline by the engine in `engine/`. Each verdict is about a family of simulated opponents
built on invented assumptions, not real players; `engine/README.md` says what the verdicts
do and don't mean, and every reveal repeats the short version.

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
queued for review. Robust and close spots with `id % 3 == 0` are reserved for checkups (268
spots); depends spots are never in checkups. A checkup is 26 robust and 14 close reserved
spots plus 10 suit-permuted repeats, fixed by the form number. The reserved spots last six
forms; from the seventh on, forms reuse some spots from earlier ones, and the result screen
says so.

Pass mark (starting guesses): over the last 40 graded attempts on robust or close spots, mean
EV loss at most 3% of the pot and at most 2 blunders, and the latest checkup meets the same
standard. Depends attempts do not count toward it.

The library is produced by the engine's `library` binary (`library 20261108 8000000 16000
5000 3000 30 target/library/spots_v2.txt 2 2` in `engine/`, about an hour on the 16-core
desktop) and copied here. The format is documented in the file's header. It is compiled into
the library with `include_str!` and parsed strictly on first use. To replace it, rerun
that, copy the new file over, and update the expected counts in `tests/table.rs`.

## Web app

`index.html` and `app/` are a phone-first web app over the same library, compiled to
`app/trainer.wasm`; JavaScript holds no poker rules or grading. Rebuild it with
`web/build.sh` (which also stamps the `sw.js` cache version), and test with
`web/test/run.py` and `web/test/mutation_check.py`. All of that runs on the desktop `hal`
through `tools/hal-run`.

Pass marks, EV bands, deal mixes and tolerances in this README are starting guesses.
