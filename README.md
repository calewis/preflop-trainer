# poker-trainer

Daily 10-15 minute fundamentals drills for no-limit hold'em home games: reading
boards, counting outs, pot odds and call-or-fold spots, with exact answers. It
assumes loose, unknown opponents, not GTO ones. No win rates and no solver.

It is a pure Rust library (no I/O or clock, so it can be compiled to WASM) plus a
thin command-line front end, with no external crates. The previous app is at tag
`archive/preflop-app`.

## Build and run

    cargo build --release
    ./target/release/poker-trainer drill

Tests: `cargo test --release`, and `cargo test --release -- --ignored` for the
exhaustive 7-card count.

## Daily routine

1. Run `poker-trainer baseline` once to get a starting score, then again every few weeks.
2. Run `poker-trainer drill` for 10 minutes each day.
3. Read every reveal. It explains why, which is the point.
4. Check `poker-trainer progress` now and then. Each step unlocks once the one before it passes.

## Commands

- `baseline`: 36 frozen questions in shuffled order with a random suit permutation.
  No feedback until the end, then scores per section and step against earlier runs.
- `card`: prints the preflop card and its notes.
- `drill [--skill NAME]... [--minutes N] [--items N] [--seed N]`: defaults to 10 minutes.
  `--skill` deals only the named skills. Without it, `preflop` plus every skill up to
  your current step is in rotation.
- `progress`: last-40 stats against the pass marks, current step and baseline history.

Skills are `preflop`, `nuts`, `possible`, `made`, `beats`, `outs`, `price`, `hit`,
`spot` and `spot_mw`. Type `q` at any question to quit; every answer is already saved.

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

A skill passes only after 40 drill attempts; baseline answers are scored separately. Pass marks and EV bands are starting guesses, kept as named constants in
`src/progress.rs` and `src/grade.rs`, to be tuned with real data.
