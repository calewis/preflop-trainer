# Pure preflop charts from simulation

**Status: these charts failed acceptance and are not used anywhere in the trainer. The hand-built card (`data/preflop_card.txt`) remains the preflop content. One calibration attempt (two revisions) did not meet its acceptance bar: hero's simulated win rate is +94 bb/100 against a ceiling of 40. These charts are derived against simulated opponents built on assumptions; no real players were observed. The original run below used the unmodified engine population. The recalibrated run is described in "Recalibration attempt".**

Output (not in the repo, under `target/preflop/`): `charts_v1.txt` (the charts), `cells_v1.tsv` (EV and standard error for every hand of every chart), `report_v1.txt` (validation run).

Code: `src/preflop_charts.rs` and `src/bin/preflop.rs`, tests in `tests/preflop_charts.rs`. Changes to existing modules: `pub` on four functions in `sim.rs`; new methods `PlayerModel::preflop_probs`, `preflop_probs_at` and `sample_cal`; a `limped` field on `PreflopCtx` and a `cal` field on `PlayerModel` (both inert for the base population); and, for the v3 spot library, the opponent-mixture changes in `spot.rs`, `sim.rs` (`simulate_with`), `output.rs`, `bin/library.rs` and `bin/regrade.rs`. The base population's output is unchanged; see `README.md`.

## Method

Each chart is one situation, 169 hands, one action per hand. The 30 charts are:

1. Folded to you: `open.*` for early, middle, cutoff, button, small blind.
2. Limpers ahead: `limp1.*` (exactly one limper) and `limp2.*` (two or more), for middle, cutoff, button, small blind, big blind.
3. Facing one open with no callers: `vs_em.*`, `vs_co.*` (opener early or middle, cutoff) with hero in position, small blind or big blind; `vs_late.sb` and `vs_late.bb` (opener button, or small blind against the big blind).
4. Facing an open plus at least one caller: `mw_em.*` (in position, small blind, big blind) and `mw_late.sb`, `mw_late.bb` (opener cutoff or button).
5. You opened and were 3-bet: `vs3bet.ip`, `vs3bet.oop`.

Seat groups are by players still to act behind: early 5 or more, middle 4, cutoff 3, button 2, small blind 1, big blind 0.

**Trial.** For one chart, hand and trial: draw table size (4/5/6/7 with weights 40/40/10/10) and hero's seat within the chart's seat group, uniformly over the seats of the table; deal hero a combo of the hand, a board and villain cards; draw a population model per villain seat; play the betting up to hero with the engine's preflop rules, rejecting and restarting the whole draw if it doesn't match the situation (`sample_deal`). Conditional sampling is exact by rejection. Hero's hand is fixed first, so only the villains and the situation are rejected on, never the hand.

**Actions.** Every action available at that node is played to the end of the hand with the same villains, cards, models and runout (common random numbers). Decisions draw their random number by trial, seat and that seat's decision count, so identical villain decisions in different branches use the same number.

**EV and fold baseline.** Hero's net is final stack minus 100 (the starting stack), so blinds and prior raises are already included: folding is -0.5 from the small blind, -1 from the big blind, -3 after opening to 3, and 0 elsewhere. A unit test checks this.

**Hero's later preflop decisions** use the charts of earlier stages, final (pure, monotone) form. Stage 0 derives the 3-bet charts (no later charts apply, so 4-bets and later use a fixed rule). Stage 1 derives facing-an-open and multiway charts. Stage 2 derives opens and limped pots. This matters because whether opening a hand pays depends on what hero does when re-raised. Where no chart applies, a deliberately simple fixed rule applies: raise AA and KK if allowed, call QQ and AK, fold the rest (`deep_rule`). Hero's postflop play is the engine's existing `hero_heuristic`, unchanged.

**Sample sizes.** Each cell runs batches of 30,000 trials; each batch uses its own independent pool of runouts and models, so batch means give standard errors that include pool sampling error. A cell stops after at least 6 batches if the chosen action could not change at 3 standard errors, and at 24 batches otherwise. 3,791 cells of 5,070 stopped at 6 batches and 771 used all 24, for 1.42 billion trials in total. The final standard error of the difference between the best two actions: median 0.057 bb, mean 0.067, p90 0.122. 93.5% of chart cells have a best action more than 2 standard errors clear of second.

**Pure rule.** `MARGIN_BB = 0.1` (passed on the command line; about 1.8x the median standard error above). Within the margin of the best EV, take the simplest of the tied actions: fold, then call/limp/check, then raise. 8.8% of cells had their top two actions within the margin.

**Monotonicity.** Along each row (pairs, each suited row, each offsuit row) a stronger hand must not take a weaker action than a dominated hand. A dynamic programme finds the monotone chart with the least EV lost from the margin's choice. 110 of 5,070 cells changed.

**Merging.** Charts with the same action labels whose final ranges differ by at most 1% of hands are merged into the first, and the merged chart keeps its line with `merged_into=...` in the metadata. `limp2.big_blind` merged into `limp1.big_blind` (identical).

## Assumptions

Beyond the assumptions in `README.md` and `models.rs`:

- Stacks are 100bb, not the 60/100/150/200 mix used for the spot library, by request.
- Hero's raise size: 3bb plus 1bb per limper, 3x a raise; 4-bets use the same rule. Stacks shove when the raise commits over 45% (`clamp_preflop_raise`). Hero makes no further raises after a 4-bet.
- Villains act from the population models, with each model used for its seat for preflop and later streets.
- Hero's seat and the table size are drawn from a prior and then conditioned by rejection on the situation, so charts reflect the population's frequency of each table and seat among the hands that reach the situation.
- "Open" means a first raise with no limpers ahead; iso-raises over limpers are not their own chart and fall to the multiway chart in play. Situation 5 requires exactly one 3-bet and all other players folded.
- Limped-pot hero seats exclude early position (no chart; a rare 7-handed case falls back to the middle chart). Single-open and multiway charts exclude positions that can't occur.
- In `limp*` charts the "limp" action is calling 1bb or completing the small blind. In big-blind limp charts the options are check or raise.
- Preflop action at depth 4 or more raises, including cold-caller-then-squeezed, uses the fixed rule above.

## Efficiency

- Each batch precomputes every combo's bucket and draw flag per street and a dense river rank for a pool of 16,384 runouts, and postflop tables for 4,096 models, shared across all trials in the batch.
- Hero's hand is fixed, not rejection-sampled. Pre-hero villains are dealt only as their action is needed, so failed draws stop early.
- Preflop action probabilities are computed for one hand class instead of all 169.
- Early stopping saved about 60% of the possible trials (1.42 of 3.65 billion).

Measured: about 372,000 trials per second per thread including rejection (2.7 microseconds per trial; a trial is 2 or 3 actions played). Pool build about 0.1 s for 4,096 boards and 1,024 models at 16 threads. The final derivation took 258 seconds at 16 threads and peak resident memory 3.8 GB. The whole run, including head-to-head, took under 5 minutes.

## Validation

### Tests

`cargo test --release --test preflop_charts`, all pass: conditioning matches each situation exactly over 400 draws per chart (seats and history checked, no overlapping cards), fold baseline counts posted money, nets stay within stack limits, `preflop_probs` equals `preflop_row`, a chart-derivation run is deterministic and independent of thread count, chart lookup maps key nodes correctly, monotonicity and one-notch-tighter behave, and notation round-trips.

### Independent cross-check

A slow reference (`preflop check`, fresh models every hand, `classify` every street, real `evaluate` at showdown, the engine's own `villain_preflop`, no pools) matched the fast simulator on nine hand and seat cases: largest difference 0.29 bb, on AA from early position, against an estimated standard error of about 0.2 bb on the fast side.

### Determinism and seed sensitivity

Rerunning the final run with the same seed at 12 threads gave identical charts. A run with a different seed changed the chosen action in 61 of 5,070 cells (1.2%). The changes were limp1 17, limp2 10, vs_co 15, the rest scattered.

### Sanity checks

| Check | Result |
|---|---|
| AA and KK never fold | Pass |
| 72o never raises or calls except a big-blind check | **Fail**, see below |
| Opens widen by seat (early <= middle <= cutoff <= button) | Pass: 59.3%, 66.5%, 74.7%, 87.3%; small blind 100.0% |
| Facing an early open is tighter than a button open | Pass: small blind 27.0% vs 46.0%, big blind 27.3% vs 54.8%, in position 58.4% vs 59.3% (cutoff) |
| One legal action per hand | Pass |
| Range notation round-trips | Pass |
| 3-bet subset of continuing range | Pass, by construction |
| Final charts monotone | Pass |

**The 72o check fails.** 72o raises in `open.small_blind`, `limp1.small_blind` and the big-blind limped charts, and calls in `mw_late.bb`, `limp2.button` and `limp2.small_blind`. Its gap is real, not noise: raising from the small blind has EV -0.07 bb (`open.small_blind`) against -0.50 for folding, so raising wins by about 0.43 bb, far more than its 0.016 bb standard error. The cross-check confirms the simulator gets the same number. The cause is the population rather than the code: the big blind folds to a 3bb open about 55% of the time in the model, and hero's heuristic loses little after the flop. I did not change the model to remove the failure.

### Comparison with the current hand-tuned card

Opens, percent of hands (card vs derived): early 10.7 vs 59.3, middle 14.3 vs 66.5, cutoff 19.2 vs 74.7, button 24.6 vs 87.3, small blind 19.5 vs 100. No hand the card opens is folded by the derived charts. The derived opens are 5 times wider early. This is badly off from any standard strategy and is the biggest warning about the charts.

### Head to head

Fresh seeds, fresh pools of tables and models not used in the derivation. Same hero postflop heuristic; common random numbers across arms; 96 million hands, 100bb, table sizes 40/40/10/10. Hero seat uniform. bb/100 with standard errors of the paired differences:

| Table | Charts | Card | One notch tighter | Charts - card | Tighter - card | Tighter - charts |
|---|---|---|---|---|---|---|
| All | +146.91 | +63.96 | +138.57 | +82.95 (0.24) | +74.61 (0.22) | -8.34 (0.10) |
| 4 players | +132.51 | +55.85 | +124.67 | +76.66 (0.32) | +68.82 (0.30) | -7.84 (0.12) |
| 5 players | +151.13 | +66.66 | +142.66 | +84.47 (0.38) | +76.00 (0.35) | -8.47 (0.15) |
| 6 players | +162.32 | +72.25 | +153.10 | +90.07 (0.86) | +80.85 (0.80) | -9.22 (0.36) |
| 7 players | +172.19 | +77.27 | +163.22 | +94.92 (0.96) | +85.94 (0.89) | -8.98 (0.41) |

The new charts beat the card in this simulation by a large margin. This is the same population family as the derivation, so it shows the charts exploit the assumed opponents, not that they would do better against real ones. An 83 bb/100 edge against a 64 bb/100 card is also implausibly large for real games: these win rates reflect how exploitable the invented population is. The "one notch tighter" arm moves the weakest hand of each action level in each row down one notch; it is only about 8 bb/100 worse, so it does not model a learner who plays much worse after the flop.

### Audit: cells furthest from a rule-of-thumb reference

The reference below is mine, ordinary poker judgment by the engine's hand strength order, and is not derived: `open.*` raises the top 12 / 16 / 25 / 40 / 30 percent (early / middle / cutoff / button / small blind); limped pots raise the top 12 percent and continue with the top 30 (big blind: raise 10, otherwise check); facing an open 3-bet the top 3 and continue 12 (early-middle opener), 4 and 18 (cutoff), 6 and 25 (late), with the big blind 8 percentage points looser and the small blind 2 tighter; multiway 3-bet 2.5 and continue 12; versus a 3-bet 4-bet 2.5 and continue 7. Each line shows the chart, hand, chosen action, reference action and EVs.

```
mw_late.bb         82o  action=  3bet reference=  fold (looser or more aggressive than reference, score 92.52, equity percentile 95.0) fold=-1.00(se 0.000) call=-0.92(se 0.042) 3bet=-0.70(se 0.031) 
vs_co.ip           AA   action=  call reference=  3bet (tighter or more passive than reference, score 94.34, equity percentile 0.2) fold=+0.00(se 0.000) call=+35.14(se 0.124) 3bet=+32.33(se 0.212) 
mw_em.sb           32o  action=  call reference=  fold (looser or more aggressive than reference, score 89.55, equity percentile 99.5) fold=-0.50(se 0.000) call=+0.11(se 0.081) 3bet=-1.52(se 0.068) 
vs_em.ip           AA   action=  call reference=  3bet (tighter or more passive than reference, score 92.46, equity percentile 0.2) fold=+0.00(se 0.000) call=+41.08(se 0.232) 3bet=+38.01(se 0.123) 
mw_late.sb         32o  action=  call reference=  fold (looser or more aggressive than reference, score 89.55, equity percentile 99.5) fold=-0.50(se 0.000) call=+0.10(se 0.060) 3bet=-0.51(se 0.040) 
vs3bet.ip          AA   action=  call reference=  4bet (tighter or more passive than reference, score 90.95, equity percentile 0.2) fold=-3.00(se 0.000) call=+43.73(se 0.140) 4bet=+41.93(se 0.155) 
limp1.big_blind    32o  action= raise reference= check (looser or more aggressive than reference, score 89.55, equity percentile 99.5) check=-0.26(se 0.013) raise=+0.18(se 0.025) 
mw_em.ip           AA   action=  call reference=  3bet (tighter or more passive than reference, score 90.95, equity percentile 0.2) fold=+0.00(se 0.000) call=+56.55(se 0.206) 3bet=+46.27(se 0.098) 
limp2.big_blind    32o  action= raise reference= check (looser or more aggressive than reference, score 89.55, equity percentile 99.5) check=+0.16(se 0.016) raise=+0.34(se 0.022) 
mw_em.sb           AA   action=  call reference=  3bet (tighter or more passive than reference, score 90.95, equity percentile 0.2) fold=-0.50(se 0.000) call=+47.55(se 0.297) 3bet=+40.41(se 0.190) 
mw_late.bb         83o  action=  3bet reference=  fold (looser or more aggressive than reference, score 88.90, equity percentile 91.4) fold=-1.00(se 0.000) call=-0.88(se 0.065) 3bet=-0.46(se 0.058) 
mw_late.sb         AA   action=  call reference=  3bet (tighter or more passive than reference, score 90.95, equity percentile 0.2) fold=-0.50(se 0.000) call=+42.55(se 0.209) 3bet=+34.88(se 0.101) 
mw_em.sb           42o  action=  call reference=  fold (looser or more aggressive than reference, score 88.64, equity percentile 98.6) fold=-0.50(se 0.000) call=+0.17(se 0.094) 3bet=-1.48(se 0.052) 
mw_late.bb         AA   action=  call reference=  3bet (tighter or more passive than reference, score 90.95, equity percentile 0.2) fold=-1.00(se 0.000) call=+37.14(se 0.198) 3bet=+32.47(se 0.180) 
mw_late.sb         42o  action=  call reference=  fold (looser or more aggressive than reference, score 88.64, equity percentile 98.6) fold=-0.50(se 0.000) call=+0.10(se 0.105) 3bet=-0.50(se 0.041) 
vs_co.ip           KK   action=  call reference=  3bet (tighter or more passive than reference, score 83.03, equity percentile 0.7) fold=+0.00(se 0.000) call=+23.20(se 0.187) 3bet=+21.51(se 0.208) 
limp1.big_blind    42o  action= raise reference= check (looser or more aggressive than reference, score 88.64, equity percentile 98.6) check=-0.21(se 0.006) raise=+0.24(se 0.018) 
vs_em.ip           KK   action=  call reference=  3bet (tighter or more passive than reference, score 77.38, equity percentile 0.7) fold=+0.00(se 0.000) call=+26.17(se 0.088) 3bet=+24.26(se 0.197) 
limp2.big_blind    42o  action= raise reference= check (looser or more aggressive than reference, score 88.64, equity percentile 98.6) check=+0.23(se 0.021) raise=+0.48(se 0.021) 
limp1.middle       AKs  action=  limp reference= raise (tighter or more passive than reference, score 76.12, equity percentile 2.9) fold=+0.00(se 0.000) limp=+14.72(se 0.100) raise=+14.35(se 0.104)
```

## Recalibration attempt

A separate config, `src/preflop_cal.rs`, drives the chart derivation (`preflop run ... v1|v2`). The base population in `models.rs` is unchanged and `spots_v2` used it alone. Revision 2 (`CAL_V2`) is also used for half of the tables behind the v3 spot library, through `POKER_POPULATION=mixture`. No published chart was used to set any number.

Preflop targets, by reasoning about a loose no-rake home game, are the numbers in `CAL_V1` (percent of hands, archetypes station, loose-passive, nit, maniac, solid): cold-call in position 45/35/7/35/15 (scaled by seat as in the population model); small blind 50/42/10/40/20; big blind 66/64/22/60/40; 3-bet 3/2.5/2.5/15/6; opener's call of a 3-bet 20/16/5/20/11; 4-bet 1.5/1.5/1.5/7/3; share of a limp range that calls a raise 85/80/70/80/70%. Each model keeps its per-seat noise. Measured continue rates against a 3bb open (`preflop calcheck`):

| Archetype | Big blind | Small blind | Cutoff | Button | 3-bet (cutoff) | Limper calls raise |
|---|---|---|---|---|---|---|
| Station | 71.3 | 55.1 | 50.0 | 62.4 | 3.7 | 76.5 |
| Loose-passive | 68.0 | 46.5 | 39.2 | 49.0 | 3.1 | 72.0 |
| Nit | 27.7 | 14.2 | 10.9 | 13.5 | 3.3 | 58.4 |
| Maniac | 75.4 | 56.8 | 51.8 | 64.2 | 16.4 | 69.8 |
| Solid | 48.7 | 27.6 | 22.4 | 28.0 | 6.8 | 62.0 |

Callers after a 3bb early open, 6-handed: none 4.4%, one 21.4%, two 36.3%, three 27.4%, four or more 10.6%.

Revision 1 changed preflop responses only (`CAL_V1`). Hero's win rate went up, to +190.2 bb/100 for the new charts and +86.3 for the card, so the postflop model drives the excess, not preflop folding alone. Revision 2 (`CAL_V2`) also lowered postflop continue rates with weak holdings, by reasoning that soft players still fold air, and raised their betting and raising with strong hands, all documented in the file.

### Result (revision 2, `target/preflop_cal_v2/`, same seeds)

Derivation 253 s at 16 threads, 1.33 billion trials, 354,000 trials per second per thread. Standard error of best-vs-second median 0.058 bb, p90 0.121. 8.8% of cells within the margin, 94.6% more than 2 standard errors clear. 69 cells changed by monotonicity.

Opens 32.1%, 42.1%, 44.8%, 53.2% (early, middle, cutoff, button), small blind 76.5%. Compared with the card (10.7, 14.3, 19.2, 24.6, 19.5).

Head to head, 96 million hands, bb/100: new charts +94.01, card +50.19, one notch tighter +88.39; charts minus card +43.82 (SE 0.20). By table size, charts minus card: 4-handed +39.73, 5 +45.42, 6 +47.18, 7 +50.39.

| Acceptance check | Result |
|---|---|
| Win rate at most about 40 bb/100 | **Fails: +94.0 for the charts, +50.2 for the card** |
| 72o doesn't raise from the small blind or call in the listed spots | Pass |
| AA and KK re-raise | Pass in every chart (0 charts where they do not) |
| No trash 3-bets | Pass; 3-bets are premium hands (widest 55+ and 88+/AQs, single-raised and multiway) |
| Opens narrow with position, recognizably | Partly: order is right, but the early open is still three times the card |
| Folding 22-TT to a 3-bet while calling A2s goes away or is explained | Not gone. Measured: hero's heuristic continues TT on 99% of flops facing a medium bet and A2s on 41%, so the pairs are not being given up early; a likely cause (unmeasured) is over-continuing with one pair against narrow raising ranges, which suited hands avoid by folding unimproved. |

**The acceptance bar is not met.** I stopped after two revisions, as instructed. The fallback is to keep the hand-tuned card. The simulation's own win rates are still implausibly high with a learner-level heuristic, which is the main evidence that the model family, including the postflop model, is too soft.

### Effect on the spot library

Regrade of 300 spots of `spots_v2.txt` with the same engine, in a throwaway copy with the population's preflop and postflop numbers replaced by revision 2's: best option changed on 12.0% of spots (flop 12.5, turn 12.4, river 10.7) and verdict on 26.7% (flop 20.0, turn 29.5, river 33.3). The baseline for regrading the same library with the unmodified engine and fresh seeds is 1.5% and 3.8% (README). So the library's verdicts are materially sensitive to these assumptions, and if the assumed passivity and over-folding are wrong, a meaningful minority of verdicts would be too. Revision 2's numbers are themselves invented and failed the bar, so this is sensitivity, not a corrected answer. That comparison is cheap and approximate: the preflop part uses the shared model formula, so it does not include the separate big-blind and limper-call settings.

## What this does and doesn't mean

- The charts are the best response, under a pure-action simplification, to the population model given hero's postflop heuristic. They are not solved poker and not advice for any real table.
- The model population folds often to raises and passively pays off made hands. Against that, raising very wide and slow-playing premiums both show profit in the simulation. Treat the width of the opens, the 100% small-blind raises, the absence of 3-bets with AA in position, the trash 3-bets in `mw_late.bb`, and small pairs folding to 3-bets while weak suited hands call, as results to question, not to teach.
- Hero's postflop heuristic is a fixed, mostly honest rule. It is a realistic level for a learner, not a best response, and it biases every preflop decision: hands that are easy to play under the heuristic look better, hands that need skill look worse.
- The margin rule prefers simpler actions when top EVs are close. A margin decision is close, not wrong.
- Monotonicity imposes rows on the strategy. Standard poker does reject some of it (A5s can be better than A8s), and those cells show up in the 110 changes.
- Sizes are the engine's. Different sizes or stacks would change the charts.
- The standard errors are for the model, not for the model's correctness.

## Reproduce

`preflop run 20261109 16 30000 6 24 0.1 16384 4096 target/preflop 96 1000000` (seed, threads, trials per batch, min and max batches, margin, runouts, models, output directory, head-to-head blocks, hands per block), then `preflop check SEED THREADS HANDS` for the cross-check and `preflop bench SEED THREADS` for throughput.

## Known weaknesses

- The population model is not fitted to data, and these charts are strongly driven by its preflop fold rates and passive postflop play. That is the main reason not to trust their width.
- Hero's heuristic doesn't bluff and folds easily; a stronger or weaker hero would change the charts.
- Hero's decisions after a 4-bet, and the fixed rule for those, are simple assumptions.
- Situation grouping (for example, the single in-position chart facing an early or middle open) averages over seats and table sizes.
- Limpers before an open, and limp-then-3-bet lines, aren't separate charts.
