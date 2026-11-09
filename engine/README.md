# poker-engine: grading hold'em decisions against plausible opponents

Experimental, and the core idea is unproven. The verdicts here are conditional
on a set of invented opponent models. They tell you what is right if real
opponents look like the models, and how much the answer depends on which kind
of opponent you face. They do not tell you what is correct poker, or what your
actual opponents do.

## Method

For one real decision (hero's cards, the board, the full action history):

1. Each opponent still in the hand gets a pool of 128 sampled `PlayerModel`s:
   an archetype drawn from the population mix, plus log-odds noise (about 0.5).
2. Each model's likelihood of the observed actions is computed over all 1326
   combos, with hero's and the board's cards removed. Bet sizes are a soft signal
   (see below): each observed size fixes that street's size class in the model,
   and the model is weighted by its archetype's probability of that class. Each
   seat is resampled in proportion to the resulting weight, so an aggressive
   line or an unusual size shifts belief about the player without proving it. Seats are independent given the public
   history. Library grading uses 256 equal-weight tables in all: two independent
   halves, each two batches of 64 with their own table draws and runout samples.
3. Opponents are fixed stochastic strategies, so hero's problem is a
   best-response expectimax against known stochastic environments: hero maximizes
   at his own nodes, villains branch on their model probabilities with beliefs
   updated by each action, cards are sampled or enumerated. All options and all
   tables at a spot share one set of runouts.
4. **Hero commits to one action per public information state**, across the tables
   of a batch that reach that state (batches are 64 tables, solved separately). Tables separate only where a villain's visible action
   or bet size differs, and table weights update on everything hero has seen
   (villain actions and cards). See "Strategy fusion" below.
5. The verdict is computed from per-table EVs under that joint continuation, on
   each half and on the pooled 256 tables. **Only spots whose two halves agree on
   the best option and the verdict are kept.** The file reports the pooled result.

Bet sizes: each archetype has a probability over three postflop size classes
(1/3, 2/3 and full pot) by street, and over three preflop open sizes (2.5, 3 and
4bb, plus 1bb per limper). A sampled model draws one class per street, so each
model uses one bet size per street. The simulator, the posterior and the forward
tree share this rule. A capped bet (all-in or minimum-sized), where several
classes give the same amount, carries no information about those classes.
Sizes outside the classes (from an external history) count as the nearest class.

Beliefs: each villain has a belief over 1326 combos. Villains are treated as
independent given the public history, apart from removing hero's and the board's
cards and a mean-field correction for villain-to-villain card removal (see the
blocker gate).

EVs are chip EV from the decision point in big blinds, no rake, so fold = 0.

### Verdicts

- **robust**: the option with the best mean EV is best on at least 90% of table
  weight and leads the next option by at least 1% of the pot.
- **close**: the mean EV gap between the top two options is under 1% of the pot.
- **depends**: anything else. The breakdown lists, for the main opponent (last
  aggressor still in the hand, otherwise the first active villain), which option
  is best for each archetype.

### What verdicts do and do not mean

- A verdict is a statement about this family of models only. A "robust" spot
  can be wrong against real opponents who are outside the family.
- "Robust" does not mean "large edge"; it means the option is usually best in
  the family and the mean gap passes 1% of the pot.
- "Depends" names which archetypes favour which option. It is a guess at how
  the answer changes with the opponent, not a read on a real player.
- The library's spots are selected to be teachable, so the verdict shares in the
  file are not the shares in ordinary play.
- EVs assume hero plays later streets as a best-responder to the mixture of
  tables reaching each decision, with tree-limited options deeper in the tree
  (bet 2/3 pot or raise 3x). Real play is less precise.

## Assumptions

**Every number in `src/models.rs` is an assumption.** None is fitted to data.
The VPIP/PFR anchors, population mix, table-size mix and stacks come from the
brief; everything else is invented to express the brief's descriptions.

- Archetype anchors VPIP/PFR: station 55/8, loose-passive 45/8, nit 14/11,
  maniac 60/35, solid 25/18. Population mix .25/.35/.10/.10/.20.
- Thresholds in `models.rs` are tuned so simulated VPIP/PFR land near the anchors.
- Postflop action probabilities by street, 6 hand buckets, situation (checked to,
  small/medium/large bet, facing a raise) and draw flag: invented per archetype.
- Hand buckets (`BUCKET_EDGES`): the holding's made-hand percentile among all
  holdings on this exact board, split at 5, 15, 30, 50, 75%. Draws get a flag on
  flop and turn; equity is otherwise not modelled.
- Price sensitivity, multiway tightening, raise penalties, re-raise sizes, and
  the bet-size and open-size class distributions: invented per archetype.
- Noise 0.5 log-odds, 0.25 per bucket, log-normal 0.3 on the scale parameters. Size classes are drawn, not noised.
- Action probability floor 0.001. Branches below 0.001 are pruned, and branches
  whose reach from the root falls below 1e-6, with the survivors renormalized.
- Hero before the graded decision: the library's preflop card, then a simple
  solid heuristic (`sim.rs`). This shapes which spots exist, not their grades.
- Game: cash, no rake, no ante, 4 to 7 players, equal stacks 60/100/150/200bb
  (no side pots), at most 2 raises per postflop street, 4 preflop.
- Deep-tree hero options: bet 2/3 pot, or raise to 3x.

## Validation (real numbers from the final code)

All numbers below are from the final code (`src/bin/validate.rs`), run on a 16-core desktop.

### Tests

`cargo test --release`: 6 hand-computed river cases, 3 hand-computed joint-solver
cases (a fused decision at a shared node, tables separating by bet size, tables
reweighted by an observed action), 4 betting-rule cases, 3 determinism tests,
4 direction tests, 4 posterior and card-removal tests, 3 equivalence tests. All pass.

- Fast solver vs naive reference, with decorrelated root beliefs as in grading:
  205 option values, max |fast - naive| / pot = 1.3e-6.
- Fast joint vs naive joint: 820 values, max 3.7e-4 of pot (the largest on a
  3-villain flop; f32 rounding around the card-removal correction is the likely
  cause, not confirmed).
- Clearing the compiled-tree cache on every lookup changed no value (184 compared).
- `decorrelate` has an independent check against exact joint enumeration on small
  ranges: it moved each villain's marginal closer to the exact answer in 573 of
  600 cases, and mean L1 error fell from 0.376 to 0.156. Iterating far beyond the 2
  iterations used can fail to converge on tiny, heavily overlapping ranges.

### Blocker error (showdown EV vs exact joint enumeration, 3000 river spots each)

Percent of pot. `plain` ignores villain card overlap, `at root` corrects the
final river beliefs once, as the engine does for river decisions, and
`along line` is the engine's path for flop decisions: corrected at the flop-to-
turn transition only, then turn and river likelihoods multiplied on.

| Spots | Method | Mean | p50 | p90 | p95 | p99 | Max | Gate |
|---|---|---|---|---|---|---|---|---|
| 3-way | plain | 0.564 | 0.418 | 1.309 | 1.553 | 2.283 | 4.105 | pass |
| 3-way | at root | 0.451 | 0.415 | 0.955 | 1.137 | 1.539 | 3.980 | pass |
| 3-way | along line | 0.519 | 0.416 | 1.132 | 1.350 | 1.871 | 4.933 | pass |
| 4-way | plain | 0.889 | 0.535 | 2.340 | 2.709 | 3.610 | 7.824 | fail |
| 4-way | at root | 0.651 | 0.506 | 1.490 | 1.709 | 2.403 | 5.727 | pass |
| 4-way | along line | 0.781 | 0.521 | 1.932 | 2.209 | 3.083 | 6.338 | **fail** |

**Because the along-line 4-way p95 (2.21%) is over the 2% limit, spots with four
or more players still in are excluded from the library.** River spots with four
players would pass on the at-root path, but they were excluded too under the rule
as set. The exclusion is an accuracy limit of this approximation, not a claim about
those spots.

The correction is applied at the root and at each flop-to-turn transition from
uncorrected beliefs carried along the line, and not at the turn-to-river transition.
Five or more players has no exact-enumeration check.

### Strategy fusion (hindsight bias)

Per-table best response lets hero use knowledge of which sampled table he is on at
later decisions, inflating continuing lines relative to folding. Measured on 100
spots (heads-up and 3-way across streets), 8 to 16 tables, per-table solve vs a
joint solve on the naive reference:

| Street | Continuing-line inflation (mean % pot) | p95 | Best-option changes | Verdict changes |
|---|---|---|---|---|
| All | 4.71 | 13.46 | 8.0% | 22.0% |
| Flop heads-up | 8.18 | 19.60 | 15.0% | 30.0% |
| Flop 3-way | 7.35 | 18.72 | 0.0% | 20.0% |
| Turn heads-up | 3.92 | 10.37 | 11.1% | 27.8% |
| Turn 3-way | 4.05 | 9.07 | 16.7% | 27.8% |
| River heads-up | 0.79 | 2.49 | 0.0% | 8.3% |
| River 3-way | 0.94 | 2.28 | 0.0% | 8.3% |

That fails the limits (under 1% inflation, under 5% changes), so the per-table solve
is not used. Grading uses the joint solve (`src/joint.rs`), which on those same
spots matched the naive joint solve with no best-option or verdict changes and a
maximum mean EV difference of 1.7e-4% of pot.

Residual: the joint solve is over the finite set of sampled tables. Deep in the tree
a single public branch can have few tables left, so some hindsight remains there.
Comparing batch sizes at a fixed 256 tables on 100 library spots fits a bias roughly
proportional to 1/K, implying about 0.24% of pot at 64 tables and about 0.4% on the flop,
with best-option and verdict changes no worse than seed noise. That is an extrapolation
from three points. Pooling four batches averages noise but not this bias, because hero
commits per batch of 64.

### Runout sampling error (one table, per-table solver, percent of pot, vs full enumeration)

The joint solve's own sampling error was not measured. Mean absolute error on each
option's EV and on its gap to the true best option, and how often the best changes:

| Case | Samples (turns x rivers) | EV error | Gap error | Best-option changes |
|---|---|---|---|---|
| Flop heads-up | 47 x 20 | 1.08 | 0.38 | 2.9% |
| Flop 3-way | 47 x 20 | 1.82 | 0.75 | 3.7% |
| Flop 4-way | 47 x 10 | 7.13 | 2.87 | 10.8% (not in the library) |
| Turn, any | all 46 rivers | 0 | 0 | 0% |

The library uses 47 x 20 on the flop and full river enumeration on the turn. Flop
sampling is the largest remaining numeric error, so do not over-read narrow flop gaps.

### Direction tests (percent of pot, final code)

- Thin value bet gain (bet67 minus check): station 17.65 vs nit 8.28.
- Air bluff gain: nit 69.67 (profitable 100%) vs station 3.10 (73.4%). The 100%
  reflects the checked-down test line, where villains are assumed weak.
- Bluff gain with 1, 2, 3 villains: 31.43, -3.81, -27.12.
- Pot-size minus third-size bet with strong hands: station 26.17 vs nit -1.00.

Determinism: same seed gives identical EVs and identical simulated hands.

Bet size as a soft signal, on one fixed river line with a bet into the button
(main-opponent posterior from 64 tables):

| Bet | Posterior |
|---|---|
| 1/3 pot | station 61%, loose-passive 38%, solid 2% |
| 2/3 pot | loose-passive 45%, station 34%, solid 14%, maniac 6% |
| pot | maniac 41%, station 31%, loose-passive 20%, solid 8% |

Preflop anchors, simulated VPIP/PFR (anchor in brackets): station 54.2 (55) / 8.0
(8), loose-passive 44.2 (45) / 8.3 (8), nit 13.9 (14) / 9.2 (11), maniac 57.3
(60) / 32.8 (35), solid 24.8 (25) / 16.0 (18).

## Performance (16-core desktop, 30 threads, unpinned)

Fixed seed set, target mix (street 40/35/25, players 2/3/4+ equal), thread-seconds
per spot and projected spots/hour at each stage of optimization:

| Stage | Thread-s/spot | Spots/hour |
|---|---|---|
| Baseline, per-table solve | 22.03 | 4,902 |
| River tree compiled once, scatter-add, shared root frames | 12.25 | 8,813 |
| Fat LTO, codegen-units=1, panic=abort | 12.20 | 8,854 |
| Tape cache across turn cards | 11.28 | 9,575 |
| Joint solve and final sample sizes (correct under the fusion gate) | 18.93 | 5,705 |

Those figures are from older sample sizes and an earlier version of the correction;
the library's own run graded 4,992 spots in 3,800 s at 256 tables on its mix (two and three players). Tried and not kept: direct-scatter frame storage
(slower), dropping dominated options after 8 tables (those options were often
best at a later table), early stopping (safe rules saved little, aggressive
rules changed verdicts), removing the per-line decorrelation (moved multiway
flop EVs by a mean 0.8 to 1.5% of pot).

Memory: a first full run was OOM-killed at about 60 GB. The cause, measured on
the flop job mix with 30 threads: the per-thread cache of compiled river trees
grew without bound, up to 10.7M nodes on one thread (3.1 GB in the current
layout, 16 GB in the layout that was killed, which stored action tables for all
64 tables at every villain node). Now each node stores actions only for its own
tables, and the per-thread cache is cleared past 500k nodes or 2M action entries;
that cost no measurable throughput (61 vs 62 jobs in 100 s). Peak RSS on that mix
is 4.1 GB. Candidate spots are about 1.4 GB in the full run and were not the
cause. Long runs also carry a watchdog (`memguard.rs`) that exits with an error
if RSS passes 45 GB, because the machine is shared.

Thread comparison, jobs completed in 150 s of the flop mix: 30 threads unpinned
103; 16 threads unpinned 77; 16 on physical cores 0-15, 78; 16 on the V-cache CCD
with SMT, 53; 16 on the other CCD with SMT, 59; 8 on V-cache cores, 42.
30 unpinned measured fastest. These are single runs with long jobs, so differences
under about 10% are not meaningful.

## The library

Generated with `library 20261108 8000000 16000 5000 3000 30 target/library/spots_v2.txt 2 2`
(seed 20261108, 30 threads, unpinned, at most 2 villains still in, 2 batches per
half). Format version 2, line-based, specified by its header. 1,235,648 bytes.
Each spot line's metadata gives `tables=256`, `agree=best+verdict` and `pool_match`,
which is 1 if the pooled result equals the two halves; it is 0 on 2 of the 3,000
spots (the header's `tables=64` is per batch).

Pipeline: 8,000,000 simulated hands; 15,990 candidates screened with 8 tables and
small runouts; 4,992 survivors graded in full (256 tables as two halves, joint
solve, sample sizes above); 3,000 selected. Wall clock 64 minutes generating,
peak RSS 1.5 GB. Regrading 600 spots took a further 8 minutes.

**Excluded: spots with four or more players still in**, under the blocker gate above.

Selection keeps only teachable spots whose halves agree: a depends verdict, or some
option of a different action class (fold, passive, aggressive) within 25% of the
pot of the best. Each stratum (street x pot type x players left) takes depends spots
first, up to 60% of its quota, then the others.

| Measure | Counts |
|---|---|
| Street | flop 1,200 (40%), turn 1,050 (35%), river 750 (25%) |
| Pot type | limped 1,000, single-raised 1,000, reraised 1,000 |
| Players left | two 1,500, three 1,500 |
| Verdict | robust 562 (18.7%), close 284 (9.5%), depends 2,154 (71.8%) |

**So the file is deliberately rich in hard spots, and its verdict shares are not
what ordinary play looks like.** It is also biased by the agreement filter:

Retention among teachable full-graded spots, the share whose two halves agree:

| By street | Kept | By verdict (pooled) | Kept |
|---|---|---|---|
| Flop | 1,488 of 1,889 (78.8%) | Robust | 634 of 715 (88.7%) |
| Turn | 1,351 of 1,621 (83.3%) | Close | 295 of 631 (46.8%) |
| River | 991 of 1,137 (87.2%) | Depends | 2,901 of 3,301 (87.9%) |

`check_library` replays every line through the betting rules and checks: field
counts, legal action order and sizes, that bets are one of the size classes, pot
and price against the replay, full option labels including raise amounts, option
EVs against their percent-of-pot, option shares, best option and verdict against
the printed numbers, archetype weights, metadata, and tallies against the header.
It passes on all 3,000 lines.

### Reproducibility and the agreement filter

Measured by regrading spots of each library with the same grader and fresh seeds:

| Grader and filter | Spots | Best option changed | Verdict changed |
|---|---|---|---|
| 64 tables, no filter (previous library) | 400 | 9.0% | 21.0% |
| 128 tables, halves must agree, kept spots | 600 | 1.2% | 8.7% |
| **256 tables, halves must agree (this library)** | 600 | **1.5%** | **3.8%** |

The target was at most 3% and 8%. On the 256-table library, the change rates were
1.2% / 3.8% on the flop, 2.9% / 4.8% on the turn, and 0% / 2.7% on the river. These
are from 600 spots, so the percentages carry about 1 to 2 points of sampling error.

What the agreement filter does: it keeps spots where two independent draws of
tables and runouts give the same best option and verdict, and the file then reports
the pooled answer over all 256 tables.

What it does **not** guarantee:

- It is a stability filter, not an accuracy check: both halves share the same
  model family, so they can agree on an answer that is wrong for real opponents.
- Agreement in one pair of draws is itself noisy. A few percent of kept verdicts
  still change on regrading, and 2 of the 3,000 spots have a pooled result that
  differs from the halves (`pool_match=0`).
- **It biases the library toward spots with stable answers.** "Close" spots, whose
  gap is near the 1% threshold, are kept only 46.8% of the time, so they are
  under-represented; their share of the file is 9.5%. A trainer built on this file
  will teach more clear-cut spots than occur in play and fewer close, borderline ones.

Earlier measurements, before the filter and with these parameters changed: regrading
400 spots of an older library (64 tables, per-size bet-size assumptions changed since)
with the new engine changed the best option on 12.5% and the verdict on 26.0%, close
to the 9.0% / 21.0% seed noise of that grader.

## Weaknesses

- The models are not fitted to data. Nothing here shows they match real players.
- Hand strength is a made-hand percentile, so strong draws and vulnerable made hands are coarse.
- Villains have no memory of their own earlier lines beyond the belief update,
  and do not adapt to hero.
- Hero's deep-tree options are limited to one bet size and one raise size,
  and villains use a single bet size per street.
- Sampling error on multiway flops is the largest numeric error.
- Verdicts are noisy without the agreement filter (about 21% change with a new seed at 64
  tables); with it, about 4% of kept verdicts still change at 256 tables, and the filter
  biases the file toward stable answers (above).
- The joint solve still has some hindsight inside the finite table set.
- The card-removal correction is mean-field and is not applied at the turn-to-river
  transition, and spots with four or more players are excluded for that reason.
- Bet sizes are modelled as three classes; real players use more, and sizes outside
  the classes are mapped to the nearest one.
- Verdict thresholds (90%, 1% of pot) are from the brief, not tuned to anything measured.
