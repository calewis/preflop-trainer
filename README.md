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
`hit`, `equity`, `anchor`, `spot` and `spot_mw`. Type `q` at any question to quit; every
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
