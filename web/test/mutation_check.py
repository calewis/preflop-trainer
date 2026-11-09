"""Confirms the browser tests fail when a fix is removed. Works on a scratch copy
of the repo and leaves the checkout alone. Run with: python3 web/test/mutation_check.py"""
import os, pathlib, re, shutil, signal, subprocess, sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
SCRATCH = pathlib.Path(os.environ.get("TRAINER_MUTATION_DIR", "/tmp/trainer-mutation-check"))
TARGET_CACHE = pathlib.Path(os.environ.get("TRAINER_MUTATION_TARGET", "/tmp/trainer-mutation-target"))

MUTATIONS = [
    ("unsaved log dropped by readLog", "app/store.js", "  if (unsaved) return memory;\n", "", "quota", "export holds"),
    ("double-tap guard uses the active clock", "app/screens.js",
     "performance.now() - appearedAt < MIN_ANSWER_MS", "activeNow() - shownAt < MIN_ANSWER_MS", "hiddenevent", "answer did not register"),
    ("import does not reload the log first", "app/screens.js",
     "      // Another tab may have added attempts; merge into the log as it is now.\n      await syncFromStorage();\n", "", "twotabs", "import lost another tab"),
    ("drill dealing timer unguarded", "app/screens.js",
     "timer = setTimeout(() => { if (drill === state) renderLoading(leave); }, 150);",
     "timer = setTimeout(() => renderLoading(leave), 150);", "dealing", "Dealing replaced the end screen"),
    ("baseline dealing timer unguarded", "app/screens.js",
     "timer = setTimeout(() => { if (baseline === state && state.index === index) renderLoading(leave); }, 150);",
     "timer = setTimeout(() => renderLoading(leave), 150);", "dealing", "Dealing replaced the baseline"),
    ("restart leaves pending calls hanging", "app/engine.js",
     "      rejectPending('the worker restarted');\n", "      pending.clear();\n", "prepfail", "no recovery screen after the prep worker crashed"),
    ("failed bundles stay cached", "app/engine.js",
     "    promise.catch(() => { if (bundles.get(key) === created) bundles.delete(key); });",
     "    promise.catch(() => {});", "prepfail", "same item after a failure"),
    ("too-slow board answers are graded as if instant", "web/src/engine.rs",
     "        match grade_timed(&item, raw, response) {\n            Ok(outcome) => {\n                // A retry stays",
     "        match grade_timed(&item, raw, 0) {\n            Ok(outcome) => {\n                // A retry stays", "newskills", "a slow right answer was not a miss", True),
    ("board_type answers graded ahead, ignoring time", "web/src/engine.rs",
     "    if !matches!(item.skill, Skill::BoardType | Skill::Table) {\n        if let poker_trainer::answer::AnswerFormat::Choice",
     "    if item.skill != Skill::Table {\n        if let poker_trainer::answer::AnswerFormat::Choice", "newskills", "a slow right answer was not a miss", True),
    ("grade ignores the staircase band", "web/src/engine.rs",
     "            Some(live) => live.real.item_for(Planned { skill, seed, origin: Origin::New }),",
     "            Some(_) => drill_item(skill, seed),", "newskills", "third item band should be 15", True),
    ("end of deck bounces home instead of a summary", "app/screens.js",
     "if (next.finished) { await finishDrill(next.out_of_items); return; }", "if (next.finished) { await finishDrill(false); return; }", "noneleft", "no friendly end message"),
    ("empty session not explained", "app/screens.js",
     "if (summary.answered === 0 && !outOfItems) { location.hash = '#home'; return; }",
     "if (summary.answered === 0) { location.hash = '#home'; return; }", "noneleft", "an empty session did not end with a summary"),
    ("checkup shows a score when quit early", "app/screens.js",
     "  if (!r.summary.complete) {", "  if (false) {", "checkupquit", "stopping a checkup did not reach the saved screen"),
    ("resume starts a new form", "app/screens.js",
     "const opened = await engine.call('check_begin', info.skill, resume ? 1 : 0);",
     "const opened = await engine.call('check_begin', info.skill, 0);", "checkupquit", "resume did not continue at question 5"),
    ("back gesture drops the checkup silently", "app/main.js",
     "(checkupActive() && name !== 'checkup')", "false", "checkupquit", "after the back gesture the checkup was not offered"),
    ("two tabs both log the same checkup slot", "app/screens.js",
     "      await syncFromStorage();\n      let result = await engine.call('check_commit'",
     "      let result = await engine.call('check_commit'", "checkuptabs", "two tabs logged"),
    ("checkup duplicates not rejected", "web/src/engine.rs",
     "            return Ok(obj! { \"duplicate\": true });",
     "            ", "checkuptabs", "two tabs logged", True),
    ("a duplicate slot still writes a log line", "app/screens.js",
     "      if (result.line) store.appendLogLine(result.line);\n      noticeIfStorageLost();\n    } catch (err) {\n      if (checkup === state)",
     "      store.appendLogLine(result.line + '');\n      noticeIfStorageLost();\n    } catch (err) {\n      if (checkup === state)", "checkuptabs", "a malformed log line"),
    ("import loses checkup runs from another device", "web/src/engine.rs",
     "                if let Some(&target) = check_map.get(&check.run) {\n                    check.run = target;\n                }",
     "                let _ = &check_map;", "native", "a_checkup_from_another_device_gets_a_new_run_number", True),
    ("too-slow reveal has no explanation", "web/src/present.rs",
     "                \"slow_note\": matched.then(|| board_slow_note(*too_slow, response_ms)),",
     "                \"slow_note\": None::<String>,", "newskills", "explain the miss", True),
    ("equity shown before the player commits", "web/src/present.rs",
     "        \"opponents\": q.opponents.count(),\n",
     "        \"opponents\": q.opponents.count(), \"answer\": \"leak\",\n", "newskills", "exact equity is in the page before answering", True),
    ("checkup answers skip the double-tap guard", "app/screens.js",
     "    if (locked || performance.now() - appearedAt < MIN_ANSWER_MS) return;\n    locked = true;\n    const response = Math.round(activeNow() - shownAt);\n    const buttons = [...controls.el.querySelectorAll('button')];\n    buttons.forEach(b => { b.disabled = true; });\n    const keys = onKey;",
     "    if (locked) return;\n    locked = true;\n    const response = Math.round(activeNow() - shownAt);\n    const buttons = [...controls.el.querySelectorAll('button')];\n    buttons.forEach(b => { b.disabled = true; });\n    const keys = onKey;", "checkup", "a tap right after a checkup answer"),
    ("a run clash is logged under the wrong run", "web/src/engine.rs",
     "        if taken {\n", "        if false && taken {\n", "native", "a_checkup_run_taken_by_another_skill_is_rejected", True),
    ("a checkup run clash stops the second tab", "app/screens.js",
     "result.conflict && tries < 3", "false && tries < 3", "checkupconflict", "the second checkup did not continue after a run clash"),
    ("a slow checkup board answer still scores", "web/src/engine.rs",
     "        let item = plan.item(slot);\n        match grade_timed(&item, raw, response) {",
     "        let item = plan.item(slot);\n        match grade_timed(&item, raw, 0) {", "native", "a_slow_board_checkup_answer_is_a_miss", True),
    ("exact hands show a combo count", "web/src/present.rs",
     "averaged.then(|| combos.join(\" x \"))", "Some(combos.join(\" x \"))", "native", "exact_hands_have_no_combo_count_to_show", True),
    ("grading shows no pending state", "app/screens.js",
     "showPending(pendingSlot)", "(() => () => {})()", "newskills", "no pending state while grading"),
    ("Progress hides the bias table", "app/screens.js",
     "    bias.enough_data\n      ? [\n        h('p', {}, bias.overall),",
     "    false\n      ? [\n        h('p', {}, bias.overall),", "progressnew", "bias sentence missing"),
    # --- table: the library (compiled and tested without a wasm build)
    ("exactly 1% loss counts as correct", "src/table.rs",
     "i64::from(loss_milli_bb) * 10 < i64::from(pot_cents)", "i64::from(loss_milli_bb) * 10 <= i64::from(pot_cents)", "libtable", "the_one_percent_rule_uses_the_evs_in_bb"),
    ("close spots accept anything", "src/table.rs",
     "        Verdict::Close => inside_loss,\n", "        Verdict::Close => true,\n", "libtable", "close_spot_25_is_correct"),
    ("depends share threshold off by one", "src/table.rs",
     "share_milli >= DEPENDS_MIN_SHARE_MILLI || inside_loss", "share_milli > DEPENDS_MIN_SHARE_MILLI || inside_loss", "libtable", "the_correct_rule_reads_its_thresholds_exactly"),
    ("depends ignores the 1% loss rule", "src/table.rs",
     "share_milli >= DEPENDS_MIN_SHARE_MILLI || inside_loss", "share_milli >= DEPENDS_MIN_SHARE_MILLI", "libtable", "the_correct_rule_reads_its_thresholds_exactly"),
    ("a fair depends read is called a blunder", "src/table.rs",
     "let band = if is_best || (spot.verdict == Verdict::Depends && correct) { None }", "let band = if is_best { None }", "libtable", "a_depends_read_is_correct"),
    ("all robust and close spots reserved", "src/table.rs",
     "spot.verdict != Verdict::Depends && spot.id.is_multiple_of(HELD_OUT_MODULUS)", "spot.verdict != Verdict::Depends", "libtable", "reserved_spots_are_robust_or_close"),
    ("depends spots reach checkups", "src/table.rs",
     "spot.verdict != Verdict::Depends && spot.id.is_multiple_of(HELD_OUT_MODULUS)", "spot.id.is_multiple_of(HELD_OUT_MODULUS)", "libtable", "reserved_spots_are_robust_or_close"),
    ("reserved spots leak into the drill pool", "src/table.rs",
     "POOL.get_or_init(|| pool(|s| !is_reserved_for_checkups(s)))", "POOL.get_or_init(|| pool(|_| true))", "libtable", "reserved_spots_are_robust_or_close"),
    ("plain seeds can reach reserved spots", "src/table.rs",
     "    if is_held_out(seed) {\n        checkup_pool()\n    } else {\n        drill_pool()\n    }", "    let _ = is_held_out(seed);\n    drill_pool()", "libtable", "no_plain_seed_can_reach_a_reserved_spot"),
    ("hero's suits are not permuted", "src/table.rs",
     "        hero: [apply_perm(spot.hero[0], &perm), apply_perm(spot.hero[1], &perm)],\n", "        hero: spot.hero,\n", "libtable", "the_permutation_relabels_hero_and_board"),
    ("the board's suits are not permuted", "src/table.rs",
     "board: spot.board.iter().map(|&c| apply_perm(c, &perm)).collect(),", "board: spot.board.clone(),", "libtable", "the_permutation_relabels_hero_and_board"),
    ("checkup repeats ignore the permutation", "src/item.rs",
     "Question::Table(q) => Question::Table(q.with_suit_perm(perm)),", "Question::Table(_) => self.clone(),", "libtable", "a_checkup_is_forty_originals_and_ten_suit_permuted_repeats"),
    ("drill mix changed", "src/table.rs",
     "pub const DEAL_ROBUST_PCT: u64 = 45;", "pub const DEAL_ROBUST_PCT: u64 = 60;", "libtable", "the_drill_mix_is_about_45"),
    ("recent log spots are forgotten", "src/schedule.rs",
     "table_recent: table_recent_from(attempts),", "table_recent: VecDeque::new(),", "libtable", "recent_attempts_in_the_log"),
    ("in-session spots are forgotten", "src/schedule.rs",
     "            self.table_recent.push_back(id);\n", "            let _ = id;\n", "libtable", "a_spot_is_not_repeated_inside_the_window"),
    ("no-repeat window shrunk", "src/table.rs",
     "pub const NO_REPEAT_WINDOW: usize = 200;", "pub const NO_REPEAT_WINDOW: usize = 150;", "libtable", "recent_attempts_in_the_log"),
    ("the no-repeat window counts old-library rows", "src/schedule.rs",
     ".filter(|a| a.skill == Skill::Table && a.in_current_library())", ".filter(|a| a.skill == Skill::Table)", "libtable", "an_old_library_row_does_not_block"),
    ("table rows do not record the library", "src/log.rs",
     "gen_version: if skill == Skill::Table { table::LIBRARY_GEN_VERSION } else { GEN_VERSION },", "gen_version: GEN_VERSION,", "libtable", "new_table_rows_record"),
    ("old-library forms lift the next form", "src/checkup.rs",
     ".filter(|a| a.skill == skill && a.in_current_library())\n        .filter_map(|a| a.extra.check.map(|c| c.form))", ".filter(|a| a.skill == skill)\n        .filter_map(|a| a.extra.check.map(|c| c.form))", "libtable", "a_checkup_on_the_old_library"),
    ("old-library unfinished checkups can be resumed", "web/src/engine.rs",
     ".filter(|r| !r.complete && !r.stale && r.in_current_library);\n        let (run, form)", ".filter(|r| !r.complete && !r.stale);\n        let (run, form)", "native", "an_unfinished_table_checkup_from_the_old_library", True),
    ("home offers to resume an old-library run", "web/src/engine.rs",
     "rfind(|r| r.skill == s.skill).filter(|r| !r.complete && !r.stale && r.in_current_library);\n    if let Some(run) = unfinished", "rfind(|r| r.skill == s.skill).filter(|r| !r.complete && !r.stale);\n    if let Some(run) = unfinished", "native", "an_unfinished_table_checkup_from_the_old_library", True),
    ("the report claims reuse on an old-library run", "web/src/engine.rs",
     "(s.skill == Skill::Table && s.in_current_library && table::checkup_form_reuses_spots(s.form))", "(s.skill == Skill::Table && table::checkup_form_reuses_spots(s.form))", "native", "a_report_on_an_old_library_run", True),
    ("a read-only option is called fair with no read", "src/table.rs",
     "        } else if on_average {\n", "        } else if true {\n", "libtable", "the_reveal_carries_every_fact"),
    ("a missed spot comes back in the session", "src/schedule.rs",
     "if !ok && !matches!(planned.skill, Skill::BoardType | Skill::Table | Skill::Chain) {", "if !ok && !matches!(planned.skill, Skill::BoardType | Skill::Chain) {", "libtable", "table_spots_get_no_same_session_retry"),
    ("a missed spot is reviewed later", "src/schedule.rs",
     "!matches!(skill, Skill::Anchor | Skill::BoardType | Skill::Table | Skill::Chain)", "!matches!(skill, Skill::Anchor | Skill::BoardType | Skill::Chain)", "libtable", "table_spots_get_no_same_session_retry"),
    ("depends attempts count for the pass mark", "src/progress.rs",
     "        && attempt.extra.table_verdict != Some(Verdict::Depends)\n", "", "libtable", "depends_attempts_do_not_count"),
    ("blunder limit loosened", "src/progress.rs",
     "pub const TABLE_MAX_BLUNDERS: u32 = 2;", "pub const TABLE_MAX_BLUNDERS: u32 = 3;", "libtable", "more_than_two_blunders"),
    ("EV-loss limit loosened", "src/progress.rs",
     "pub const TABLE_MAX_MEAN_EV_LOSS_PCT: f64 = 3.0;", "pub const TABLE_MAX_MEAN_EV_LOSS_PCT: f64 = 3.5;", "libtable", "table_drill_mark_needs_forty"),
    ("the checkup does not gate the pass", "src/progress.rs",
     "passed: drill_passed && checkup_passed == Some(true),", "passed: drill_passed,", "libtable", "the_latest_checkup_must_meet_the_same_standard"),
    ("old log header treated as data", "src/log.rs",
     "line == LOG_HEADER || line == TABLE_LOG_HEADER || line == CHECKUP_LOG_HEADER || line == LEGACY_LOG_HEADER", "line == LOG_HEADER || line == TABLE_LOG_HEADER || line == LEGACY_LOG_HEADER", "libtable", "table_rows_round_trip"),
    ("table rows fail to parse", "src/log.rs",
     "        FULL_COLUMNS | TABLE_COLUMNS | NOTES_COLUMNS => parse_extra", "        FULL_COLUMNS | NOTES_COLUMNS => parse_extra", "libtable", "table_rows_round_trip"),
    ("reuse flag always off", "src/table.rs",
     "(form as usize + 1) * CHECKUP_ROBUST > order.robust.len() || (form as usize + 1) * CHECKUP_CLOSE > order.close.len()", "false && (form as usize + 1) * CHECKUP_ROBUST > order.robust.len()", "libtable", "checkup_forms_are_disjoint"),
    ("parser accepts a pot that doesn't replay", "src/table.rs",
     "if (replay.pot - pot).abs() > STACK_EPS || (replay.call_amount(seat) - call).abs() > STACK_EPS {", "if false {", "libtable", "parsing_is_strict"),
    ("bet labels ignore the 1bb minimum", "src/table.rs",
     "let amount = target.max(1.0).min(all_in_max);", "let amount = target.min(all_in_max);", "libtable", "option_labels_use_the_real_amounts"),
    ("the 1% rule falls back to the rounded percentages", "src/table.rs",
     "    let inside = inside_one_percent(loss_milli, spot.pot_cents);\n", "    let inside = best.ev_pct_tenths - picked.ev_pct_tenths < 10;\n", "libtable", "the_one_percent_rule_uses_the_evs_in_bb"),
    ("a correct answer reads 1.0%", "src/table.rs",
     "let pct = if self.loss_pct < 1.0 { format!(\"{:.2}\", self.loss_pct) } else { format!(\"{:.1}\", self.loss_pct) };", "let pct = format!(\"{:.1}\", self.loss_pct);", "libtable", "the_one_percent_rule_uses_the_evs_in_bb"),
    ("capped bets keep their size words", "src/table.rs",
     "if (amount - target).abs() < 0.005 {", "if true {", "libtable", "option_labels_use_the_real_amounts"),
    ("labels use the printed pot", "src/table.rs",
     "let options = parse_options(fields[11], replay.pot,", "let options = parse_options(fields[11], pot,", "libtable", "option_labels_use_the_real_amounts"),
    ("the stack shown is the starting stack", "src/table.rs",
     "let behind_cents = cents(replay.stack_left(seat));", "let behind_cents = cents(f64::from(stack_bb));", "libtable", "big_blind_can_be_hero"),
    ("the wrong-read note is vague again", "src/table.rs",
     "It was best at only {share:.1}% of simulated tables, so it is rarely a read worth making.", "This was rarely best ({share:.1}).", "libtable", "the_reveal_carries_every_fact"),
    ("EV can read -0.00", "src/table.rs",
     "    if hundredths == 0.0 {\n", "    if false {\n", "libtable", "the_reveal_carries_every_fact"),
    ("a late retry commit is logged as a drill", "web/src/engine.rs",
     "                    .map_or(Origin::New, |p| p.origin);\n", "                    .map_or(Origin::New, |_| Origin::New);\n", "native", "a_late_commit_of_a_retry_stays_a_retry", True),
    ("the start screen hides the reuse note", "app/screens.js",
     "      info.reuse_note ? h('p', { class: 'hint', 'data-reuse-start': '' }, info.reuse_note) : null,\n", "", "tablespots", "no reuse note on the start screen"),
    ("baseline gains a fourth step", "src/progress.rs",
     "let steps = (1..=BASELINE_STEPS)", "let steps = (1..=STEPS.len())", "libtable", "table_counts_as_step_four"),
    # --- table: the app
    ("table options are graded before the answer", "web/src/engine.rs",
     "    if !matches!(item.skill, Skill::BoardType | Skill::Table) {\n", "    if item.skill != Skill::BoardType {\n", "tablespots", "table options must not be graded", True),
    ("a depends reveal omits the archetypes", "web/src/present.rs",
     "    let archetypes: Vec<String> = if spot.verdict == Verdict::Depends {\n        spot.archetypes.iter().map(|row| row.sentence(spot)).collect()\n    } else {\n        Vec::new()\n    };\n", "    let archetypes: Vec<String> = Vec::new();\n", "tablespots", "depends needs the archetype breakdown", True),
    ("the reveal drops the simulation caveat", "app/reveal.js",
     "    h('p', { class: 'hint' }, r.note),\n", "", "tablespots", "no simulation caveat"),
    ("the checkup result hides the reuse note", "app/screens.js",
     "      r.reused_note ? h('p', { class: 'hint', 'data-reused': '' }, r.reused_note) : null,\n", "", "tablespots", "no reuse note on a late form"),
    ("the timeline is not shown", "app/reveal.js",
     "    h('ol', { class: 'timeline' }, q.streets.map(", "    h('ol', { class: 'timeline', hidden: 'hidden' }, [].map(", "tablespots", "no street timeline"),
    # --- end of drills and checkups during a slow grade, and status placement
    ("the status lines sit below the keypad", "app/screens.js",
     "h('section', { class: 'answers' }, errorSlot, pendingSlot, controls.el),\n    status);", "h('section', { class: 'answers' }, controls.el, errorSlot, pendingSlot),\n    status);", "midgrade", "must sit above the keypad"),
    ("ending a drill during a slow grade shows the reveal", "app/screens.js",
     "    if (drill !== owner) {\n", "    if (false) {\n", "midgrade", "End during a slow grade threw"),
    ("the back gesture during a grade loses the answer", "web/src/engine.rs",
     "        if !expected {\n", "        if false {\n", "midgrade", "the back gesture", True),
    ("the checkup report runs before the commit", "app/screens.js",
     "  checkup = null;\n  await answersSettled();\n  let result;\n", "  checkup = null;\n  let result;\n", "midgrade", "the report ran before the last answer"),
    ("the drill summary runs before the commit", "app/screens.js",
     "  drill = null;\n  await answersSettled();\n  let summary;\n", "  drill = null;\n  let summary;\n", "midgrade", "one-answer summary"),
    # --- the research-ranked batch: start-over note
    ("start over checks the unfinished form", "web/src/engine.rs",
     "table::checkup_form_reuses_spots(start_over_form)", "table::checkup_form_reuses_spots(upcoming_form)", "native", "starting_over_a_late_table_checkup_warns_about_the_next_form", True),
    ("the start over note is not shown", "app/screens.js",
     "      info.start_over_note ? h('p', { class: 'hint', 'data-start-over-note': '' }, info.start_over_note) : null),\n", "      null),\n", "research", "note missing"),
    # --- families and siblings
    ("the made family is the hand name, not its code", "src/family.rs",
     "Question::Made { hero, board } => Some(made_class(*hero, board).code().to_string()),", "Question::Made { hero, board } => Some(made_class(*hero, board).label().to_string()),", "libresearch", "made_family_is_the_answer_class"),
    ("outs bands off by one", "src/family.rs",
     "pub const OUTS_BANDS: [u32; 3] = [5, 9, 13];", "pub const OUTS_BANDS: [u32; 3] = [5, 8, 13];", "libresearch", "outs_family_has_four_bands"),
    ("spot family drops the opponent class", "src/family.rs",
     'format!("{}:{opponent}", mode_code(q.mode))', 'format!("{}:any", mode_code(q.mode))', "libresearch", "spot_family_is_mode_and_opponent_class"),
    ("hit family ignores one or both cards", "src/family.rs",
     'Some(format!("{outs}:{}", if *both { "both" } else { "one" }))', 'Some(format!("{outs}"))', "libresearch", "hit_family_is_outs_and_one_or_both_cards"),
    ("equity family ignores the band", "src/family.rs",
     't if t < EQUITY_FAMILY_BANDS[1] => "25-50",', 't if t < EQUITY_FAMILY_BANDS[1] => "x",', "libresearch", "equity_family_is_rung_opponents"),
    ("siblings can be held out", "src/family.rs",
     "let mut seed = rng.next_u64() & !HELD_OUT_BIT;\n        if let Some(rung)", "let mut seed = rng.next_u64();\n        if let Some(rung)", "libresearch", "a_sibling_is_a_new_non_held_out_seed"),
    ("the avoided seed is served", "src/family.rs",
     "        if seed == avoid {\n            continue;\n        }\n", "", "libresearch", "the_avoided_seed_is_skipped"),
    ("the sibling budget shrunk", "src/family.rs",
     "pub const SIBLING_TRIES: u32 = 200;", "pub const SIBLING_TRIES: u32 = 20;", "libresearch", "a_rare_family_is_found_within_the_budget"),
    ("the spot prefilter draws in the wrong order", "src/generate.rs",
     "let (next_card, _want_call) = (rng.chance(1, 2), rng.chance(1, 2));\n            let player", "let next_card = rng.chance(1, 2);\n            let player", "libresearch", "prefilter_agrees_with_full_generation"),
    ("a retry deals the same seed", "src/schedule.rs",
     "Some(family) => self.sibling(retry.skill, family, retry.seed, retry.seed),", "Some(_) => Some(retry.seed),", "libresearch", "a_same_session_retry_deals_a_sibling"),
    ("a due family serves no fallback", "src/schedule.rs",
     "                None => self.deal_fresh(skill),\n            });", "                None => self.deal(skill, base, Origin::Due),\n            });", "libresearch", "a_due_family_with_no_sibling_falls_back"),
    ("the ladder ends too soon", "src/schedule.rs",
     "pub const FAMILY_LADDER_DAYS: [u64; 5] = [1, 3, 7, 14, 30];", "pub const FAMILY_LADDER_DAYS: [u64; 5] = [1, 3, 7, 14, 21];", "libresearch", "review_ladder_runs_one_three"),
    ("a miss does not reset the ladder", "src/schedule.rs",
     "states.insert(key, LadderState::Waiting { stage: 0, due_at: at.saturating_add(DAY_MS) });", "states.insert(key, LadderState::Waiting { stage: 1, due_at: at.saturating_add(DAY_MS) });", "libresearch", "a_miss_resets_the_ladder"),
    ("retirement never happens", "src/schedule.rs",
     "                    _ => LadderState::Retired,", "                    _ => LadderState::Waiting { stage, due_at: at.saturating_add(DAY_MS) },", "libresearch", "review_ladder_runs_one_three"),
    ("a retired family stays retired forever", "src/schedule.rs",
     "        if !attempt.ok() {\n            if !states.contains_key(&key) {", "        if !attempt.ok() {\n            if matches!(states.get(&key), Some(LadderState::Retired)) { continue; }\n            if !states.contains_key(&key) {", "libresearch", "a_retired_family_comes_back"),
    ("slow families are not preferred", "src/schedule.rs",
     "due.sort_by(|a, b| b.slowness.total_cmp(&a.slowness).then(a.due_at_ms.cmp(&b.due_at_ms)));", "due.sort_by(|a, b| a.due_at_ms.cmp(&b.due_at_ms));", "libresearch", "due_families_put_the_slowest"),
    ("a correct retry counts as a review", "src/schedule.rs",
     "        if attempt.mode != Mode::Drill {\n            continue;\n        }\n", "", "libresearch", "a_correct_retry_does_not_move"),
    # --- flip feedback
    ("flip formula ignores X", "src/money.rs",
     "Some(share * (pot + extra) / (1.0 - share))", "Some(share * pot / (1.0 - share))", "libresearch", "the_break_even_call_solves"),
    ("flip counts cards on multiway spots", "src/grade.rs",
     "let cards_ahead = (q.opponents.len() == 1).then(|| {", "let cards_ahead = (q.opponents.len() >= 1).then(|| {", "libresearch", "multiway_spots_have_no_card_count"),
    ("all-in flops ignore the river in the card count", "src/grade.rs",
     "let extra = matches!(q.mode, SpotMode::AllIn) && board.len() == 3;", "let extra = false;", "libresearch", "flip_card_count_matches_a_brute_force_recount"),
    ("the card count compares with the wrong price", "src/grade.rs",
     ".filter(|(_, share)| *share > a.required)", ".filter(|(_, share)| *share > a.price)", "libresearch", "flip_card_count_matches_a_brute_force_recount"),
    ("the reveal hides the flip line", "app/reveal.js",
     "    r.flip_call ? p(r.flip_call) : null,\n", "", "research", "no flip call line"),
    # --- reflection
    ("every miss gets the form", "src/reflection.rs",
     "Rng::new(seed ^ OFFER_SALT).chance(1, 2)", "Rng::new(seed ^ OFFER_SALT).chance(1, 1)", "libresearch", "the_form_is_offered_for_half"),
    ("robust spots only count when actually wrong", "src/reflection.rs",
     "g.spot.verdict == Verdict::Robust && g.chosen != g.spot.best", "g.spot.verdict == Verdict::Robust && !g.correct", "libresearch", "a_robust_spot_that_counts_as_correct"),
    ("retries are offered the form", "src/reflection.rs",
     "if origin == Origin::SessionRetry || !reflection_eligible(item, outcome) {", "if !reflection_eligible(item, outcome) {", "libresearch", "only_misses_on_the_right_skills"),
    ("the comparison ignores the family", "src/reflection.rs",
     "matches!(a.mode, Mode::Drill | Mode::DrillRetry) && a.skill == miss.skill && a.extra.family.as_deref() == Some(family)", "matches!(a.mode, Mode::Drill | Mode::DrillRetry) && a.skill == miss.skill", "libresearch", "the_comparison_counts_later_siblings"),
    ("a skipped form counts as reflected", "src/reflection.rs",
     "        (_, true) => Group::Reflected,", "        (_, _) => Group::Reflected,", "libresearch", "the_comparison_counts_later_siblings"),
    ("the comparison needs too few misses", "src/reflection.rs",
     "pub const REFLECTION_MIN_MISSES: usize = 60;", "pub const REFLECTION_MIN_MISSES: usize = 59;", "libresearch", "the_comparison_needs_sixty"),
    ("a reflection takes time", "src/reflection.rs",
     "        response_ms: 0,\n        answer: if answers", "        response_ms: 1,\n        answer: if answers", "libresearch", "reflection_rows_round_trip"),
    ("the form is not shown", "app/screens.js",
     "h('section', { class: 'answers' }, revealNode, reflection.el),", "h('section', { class: 'answers' }, revealNode),", "research", "offered form not shown"),
    ("the Progress panel shows without enough data", "web/src/present.rs",
     "    let enough = c.enough_data();\n", "    let enough = true;\n", "research", "reflection panel before data", True),
    # --- chains
    ("chains ignore whether the card plays the hand", "src/generate.rs",
     "        if !CARD.plays(hero) {\n            continue;\n        }\n        let hero_rank", "        let hero_rank", "libresearch", "a_chain_is_a_hand_the_card_plays"),
    ("hero need not be behind", "src/generate.rs",
     ".filter(|&v| rank_of(v, &board) > hero_rank)", ".filter(|_| true)", "libresearch", "a_chain_is_a_hand_the_card_plays"),
    ("the price stage ignores callers", "src/grade.rs",
     "grade_price(hand.pot_before, hand.bet, hand.callers(), raw)?", "grade_price(hand.pot_before, hand.bet, 0, raw)?", "libresearch", "a_three_way_pot_counts"),
    ("chain stages come out of order", "src/schedule.rs",
     "            self.chain = ChainStage::from_index(stage.index() + 1).map(|next| (hand, next));", "            self.chain = None;", "libresearch", "a_session_deals_the_four_stages"),
    ("a missed chain stage is retried", "src/schedule.rs",
     "if !ok && !matches!(planned.skill, Skill::BoardType | Skill::Table | Skill::Chain) {", "if !ok && !matches!(planned.skill, Skill::BoardType | Skill::Table) {", "libresearch", "a_chain_miss_gets_no_retry"),
    ("chain has no tie line", "src/grade.rs",
     "            result.chain_tie = Some(tie);\n", "            let _ = tie;\n", "libresearch", "the_last_stage_reveal_ties"),
    ("stages are not logged", "src/log.rs",
     "        extra.stage = outcome.chain_stage;\n", "", "libresearch", "stages_log_one_row_each"),
    ("the chain mark is loose", "src/progress.rs",
     "    min_within_one_pct: Some(85.0),", "    min_within_one_pct: Some(80.0),", "libresearch", "the_chain_mark_is_85_percent"),
    ("chain checkup repeats ignore the gap", "src/checkup.rs",
     "                Some(at) if placed < quota && slots.len() >= at + MIN_REPEAT_GAP => {", "                Some(at) if placed < quota => {", "libresearch", "a_chain_checkup_keeps_hands_together"),
    ("chain checkup seeds are not held out", "src/checkup.rs",
     "        let hand = rng.next_u64() | HELD_OUT_BIT;\n        for stage in ChainStage::ALL {", "        let hand = rng.next_u64();\n        for stage in ChainStage::ALL {", "libresearch", "a_chain_checkup_keeps_hands_together"),
    ("the hand stage leaks the opponent", "web/src/present.rs",
     "        ChainStage::Hand => {}\n", "        ChainStage::Hand => add(\"villain\", holding(&h.villain)),\n", "research", "leaks the opponent", True),
    ("the chain tie line is not shown", "app/reveal.js",
     "    outcome.tie_line ? h('p', { class: 'reveal-tie' }, outcome.tie_line) : null);", "    null);", "research", "no tie line"),
    # --- confidence
    ("confidence on the near-break-even decks", "src/log.rs",
     "matches!(skill, Skill::Made | Skill::Outs | Skill::Price | Skill::Equity | Skill::ReadPrice)", "matches!(skill, Skill::Made | Skill::Outs | Skill::Price | Skill::Equity | Skill::ReadPrice | Skill::Spot)", "libresearch", "only_skills_with_a_clear_answer_ask"),
    ("high is treated as sure", "src/log.rs",
     "            Confidence::High => 95.0,", "            Confidence::High => 99.0,", "libresearch", "levels_are_treated_as_55_75_95"),
    ("the table counts retries", "src/progress.rs",
     "attempts.iter().filter(|a| a.mode == Mode::Drill && asks_confidence(a.skill))", "attempts.iter().filter(|a| asks_confidence(a.skill))", "libresearch", "the_reliability_table_counts"),
    ("the reveal comes before the confidence tap", "app/screens.js",
     "    if (bundle.confidence && store.readConfidence()) askConfidence(outcome, response);\n    else showReveal(outcome, response);", "    showReveal(outcome, response);", "research", "confidence prompt never shown"),
    ("ending during the tap loses the answer", "app/screens.js",
     "      const saved = trackAnswer(commitAnswer(outcome, response));\n", "      const saved = Promise.resolve();\n", "research", "ending during the tap lost the answer"),
    ("the tap is not logged", "web/src/engine.rs",
     "            attempt.extra.confidence = fields.get(4).and_then(|code| Confidence::from_code(code));\n", "            let _ = fields.get(4);\n", "research", "confidence column", True),
    ("the confidence setting starts on", "app/store.js",
     "  return safely(() => localStorage.getItem(CONFIDENCE_KEY), null) === '1';", "  return safely(() => localStorage.getItem(CONFIDENCE_KEY), null) !== '0';", "research", "confidence must default to off"),
    # --- family column and log order
    ("the family column is missing", "src/log.rs",
     "            extra.family.as_deref().unwrap_or(\"\"),\n", "            \"\",\n", "libresearch", "a_row_with_notes_but_no_table_spot"),
    ("the new header swallows old rows", "src/log.rs",
     "line == LOG_HEADER || line == TABLE_LOG_HEADER || line == CHECKUP_LOG_HEADER", "line == LOG_HEADER || line == CHECKUP_LOG_HEADER", "libresearch", "the_new_columns_go_at_the_end"),
    # --- spots: real decisions against realistic mixes
    ("a spot can be a hand with nothing", "src/generate.rs",
     "if !CARD.plays(hero) || !worth_a_decision(hero, &board) {", "if !CARD.plays(hero) {", "libresearch", "a_spot_is_never_a_hand_with_nothing"),
    ("the prompt doesn't name the player", "src/classes.rs",
     'format!("They\'re {}. Assume they have one of: {hands}.", player.described())', 'format!("Assume they have one of: {hands}.")', "libresearch", "the_prompt_names_the_player"),
    ("the side of break-even is ignored", "src/generate.rs",
     "let on_side = if want_call { margin > 0.0 } else { margin < 0.0 };", "let on_side = margin > 0.0;", "libresearch", "call_and_fold_are_both_real_answers"),
    ("spots can be hopeless", "src/generate.rs",
     "pub const SPOT_MIN_SHARE: f64 = 0.15;", "pub const SPOT_MIN_SHARE: f64 = 0.02;", "libresearch", "spots_exclude_hopeless"),
    ("the reveal hides the groups", "src/grade.rs",
     "        groups: groups_used(&q.opponents, hero, &board, 0),\n", "        groups: Vec::new(),\n", "libresearch", "the_reveal_lists_the_groups_used"),
    ("low pairs are not a class", "src/classes.rs",
     "            0 if a < min => Some(OppClass::Low),\n", "", "libresearch", "new_classes_are_what_they_say"),
    ("a draw counts as nothing", "src/classes.rs",
     "        && !is_flush_draw(holding, board)\n        && !is_oesd(holding, board)\n}", "}", "libresearch", "new_classes_are_what_they_say"),
    ("a chain's second opponent is one class", "src/generate.rs",
     "            let Some(mix) = mix_for(player, hero, &board) else { continue };\n            Some(mix)", "            let Some(mix) = mix_for(player, hero, &board) else { continue };\n            Some(OppMix::single(mix.classes[0]))", "libresearch", "a_chain_second_opponent_is_a_mix"),
    # --- what beats you
    ("the which-rung grades in the wrong order", "src/beats_me.rs",
     "    match rank_of(other, board).cmp(&rank_of(hero, board)) {", "    match rank_of(hero, board).cmp(&rank_of(other, board)) {", "libresearch", "rung_zero_grades_exactly"),
    ("better kickers count as a bigger pair", "src/beats_me.rs",
     "Category::OnePair if hero.category() == Category::OnePair && hero.primary() == rank.primary() => {\n            Some(BeatKind::BetterKicker)\n        }", "Category::OnePair if hero.category() == Category::OnePair && hero.primary() == rank.primary() => {\n            Some(BeatKind::BiggerPair)\n        }", "libresearch", "rung_one_kinds_come_from_every"),
    ("ways don't remove the cards you can see", "src/beats_me.rs",
     "let seen = holding_mask(&hero) | mask_of(board);\n    let high_left", "let seen = 0u64;\n    let _ = (hero, board);\n    let high_left", "libresearch", "rung_two_counts_ways"),
    ("rung 3 ties count as losses", "src/beats_me.rs",
     " + 50.0 * tie as f64 / total.max(1) as f64;", ";", "libresearch", "rung_three_ties_count_half"),
    ("the bands' edges move", "src/beats_me.rs",
     "        if pct > MIX_AHEAD_PCT {", "        if pct >= MIX_AHEAD_PCT {", "libresearch", "rung_three_bands"),
    ("promotion needs less", "src/beats_me.rs",
     "pub const BEATS_ME_PROMOTE_PCT: f64 = 85.0;", "pub const BEATS_ME_PROMOTE_PCT: f64 = 80.0;", "libresearch", "the_rung_rises"),
    ("low rungs count for the mark", "src/beats_me.rs",
     "rung_of(attempt).is_some_and(|rung| rung >= 2)", "rung_of(attempt).is_some_and(|rung| rung >= 1)", "libresearch", "only_rungs_two_and_three"),
    ("checkups take one rung", "src/checkup.rs",
     "beats_me_seed_with_rung(seed, (i % usize::from(BEATS_ME_RUNGS)) as u8)", "beats_me_seed_with_rung(seed, 0)", "libresearch", "checkups_take_ten_items"),
    ("beats still gates step 1", "src/progress.rs",
     "&[Skill::Nuts, Skill::Possible, Skill::Made, Skill::BeatsMe, Skill::BoardType],", "&[Skill::Nuts, Skill::Possible, Skill::Made, Skill::Beats, Skill::BoardType],", "libresearch", "beats_is_in_no_step"),
    ("beats leaves the drills", "src/progress.rs",
     "let mut out = vec![Skill::Preflop, Skill::Beats, Skill::ReadPrice];", "let mut out = vec![Skill::Preflop, Skill::ReadPrice];", "libresearch", "beats_is_in_no_step"),
    ("beats is dealt at full weight", "src/schedule.rs",
     "if s == Skill::Beats { weight * BEATS_WEIGHT_PER_MILLE / 1000 } else { weight }", "weight", "libresearch", "beats_is_dealt_at_about_half"),
    ("the random-hands note is gone", "app/reveal.js",
     "    p(r.random_note),\n", "", "research", "no random note"),
    # --- read the price
    ("cheap and fair move", "src/read_price.rs", "pub const CHEAP_BELOW_PCT: f64 = 20.0;", "pub const CHEAP_BELOW_PCT: f64 = 22.0;", "libreadprice", "rung_zero"),
    ("the band gap shrinks", "src/read_price.rs", "pub const BAND_GAP_POINTS: f64 = 1.5;", "pub const BAND_GAP_POINTS: f64 = 0.5;", "libreadprice", "rung_zero_stays_clear"),
    ("callers are left out of the pot", "src/read_price.rs", "f64::from(pot_with_bet(pot_before, bet, callers)), f64::from(bet)", "f64::from(pot_with_bet(pot_before, bet, 0)), f64::from(bet)", "libreadprice", "the_pot_is_stated_before"),
    ("the number's tolerance widens", "src/grade.rs", "((guess - price).abs() <= PERCENT_TOLERANCE, format!(\"{guess}\"))", "((guess - price).abs() <= 2.0 * PERCENT_TOLERANCE, format!(\"{guess}\"))", "libreadprice", "rung_one_uses_the_price_tolerance"),
    ("the gap is dealt", "src/read_price.rs", "} else if edge_points.abs() <= CLOSE_POINTS - DEAL_MARGIN_POINTS {", "} else if edge_points.abs() <= EASY_POINTS {", "libreadprice", "rung_two_never_deals"),
    ("an all-in flop counts one card", "src/read_price.rs", "hit_chance(kind.outs(), street, street == HitStreet::Flop)", "hit_chance(kind.outs(), street, false)", "libreadprice", "rung_two_shows_the_share"),
    ("the largest mistake is right", "src/read_price.rs", ".min_by_key(|&i| options[i])", ".max_by_key(|&i| options[i])", "libreadprice", "rung_four_has_exactly_one"),
    ("options sit on break-even", "src/read_price.rs", "price(bet) <= share - OPTION_GAP_POINTS && price(bet) >= share - OPTION_BAND_POINTS", "price(bet) < share && price(bet) >= share - OPTION_BAND_POINTS", "libreadprice", "rung_four_has_exactly_one"),
    ("promotion needs less", "src/read_price.rs", "pub const READ_PRICE_PROMOTE_PCT: f64 = 85.0;", "pub const READ_PRICE_PROMOTE_PCT: f64 = 79.0;", "libreadprice", "the_rung_rises"),
    ("practice rungs count for the mark", "src/read_price.rs", "rung_of(attempt).is_some_and(|rung| rung >= 2)", "rung_of(attempt).is_some_and(|rung| rung >= 0)", "libreadprice", "only_the_decision_rungs"),
    ("checkups take one rung", "src/checkup.rs", "read_price_seed_with_rung(seed, (i % usize::from(READ_PRICE_RUNGS)) as u8)", "read_price_seed_with_rung(seed, 0)", "libreadprice", "checkups_take_eight"),
    ("the share cap is gone", "src/schedule.rs", "        if dealt * 100 >= (self.dealt.len() as u64 + 1) * READ_PRICE_MAX_PERCENT {\n", "        if false && dealt > u64::MAX {\n", "libreadprice", "the_default_drill_deals_it"),
    ("new items ignore the player's rung", "src/schedule.rs", "            Skill::ReadPrice => seed = read_price_seed_with_rung(seed, self.read_price_rung),\n", "", "libreadprice", "a_new_item_is_dealt_at_the_players_rung"),
    ("every rung gets the form", "src/reflection.rs", "matches!(q.rung(), 2 | 3) && !outcome.ok()", "!outcome.ok()", "libreadprice", "reflection_is_offered"),
    ("read_price jumps ahead of an owed preflop", "src/schedule.rs",
     "        let skill = if preflop_owed {\n            Skill::Preflop\n        } else if self.read_price_turn(blocked) {\n            Skill::ReadPrice\n        } else {",
     "        let skill = if self.read_price_turn(blocked) {\n            Skill::ReadPrice\n        } else if preflop_owed {\n            Skill::Preflop\n        } else {",
     "libreadprice", "preflop_never_falls_below_its_floor"),
    ("sibling search ignores the rung", "src/family.rs",
     "matches!(skill, Skill::Equity | Skill::BeatsMe | Skill::ReadPrice).then(", "matches!(skill, Skill::Equity | Skill::BeatsMe).then(",
     "libreadprice", "the_sibling_search_targets_the_rung"),
    ("a low wrong pick gets no explanation", "src/read_price.rs",
     "    if asked > share {", "    if asked > 0.0 {", "libreadprice", "rung_four_explains_a_wrong_pick"),
    ("the shortcut's first band changes", "src/read_price.rs", "Under a third of the pot is cheap", "Under a quarter of the pot is cheap", "libreadprice", "rung_zero_shortcut_always_matches"),
    ("an all-in flop reveal says one card", "src/read_price.rs", "                HitStreet::Flop => \"two cards to come\",", "                HitStreet::Flop => \"one card to come\",", "libreadprice", "rung_two_shows_the_share"),
    ("a stated draw is misquoted in the reveal", "src/read_price.rs", "PriceHand::Draw { .. } => format!(\"You have about {:.0}%\", hand_share_points(hand)),", "PriceHand::Draw { .. } => format!(\"You have about {:.0}%\", hand_share_points(hand) + 5.0),", "libreadprice", "the_verdict_sentence"),
    ("one caller reads as callers", "src/read_price.rs", "if q.callers == 1 { \"the caller\".to_string() }", "if q.callers == 99 { \"the caller\".to_string() }", "libreadprice", "one_caller_reads_in_the_singular"),
    ("the dealing margin is gone", "src/read_price.rs", "pub const DEAL_MARGIN_POINTS: f64 = 0.5;", "pub const DEAL_MARGIN_POINTS: f64 = 0.0;", "libreadprice", "rung_two_never_deals"),
    ("callers don't change the answer", "src/read_price.rs", "pub const CALLERS_MATTER_PERCENT: u64 = 80;", "pub const CALLERS_MATTER_PERCENT: u64 = 0;", "libreadprice", "leaving_callers_out"),
    ("rung 4 options stray from break-even", "src/read_price.rs", "pub const OPTION_BAND_POINTS: f64 = 10.0;", "pub const OPTION_BAND_POINTS: f64 = 100.0;", "libreadprice", "rung_four"),
    ("a tiny price reads 0%", "src/read_price.rs", "    if pct < 0.5 {\n        \"under 1%\"", "    if pct < 0.0 {\n        \"under 1%\"", "libreadprice", "tiny_prices"),
    ("retries count toward promotion", "src/read_price.rs", ".filter(|a| a.skill == Skill::ReadPrice && a.mode == Mode::Drill && rung_of(a) == Some(rung))", ".filter(|a| a.skill == Skill::ReadPrice && a.mode != Mode::Check && rung_of(a) == Some(rung))", "libreadprice", "retries_do_not_count"),
    ("a third read_price in a row is dealt", "src/schedule.rs", "        if !self.read_price_by_share(Skill::ReadPrice) || blocked == Some(Skill::ReadPrice) {\n", "        let _ = blocked;\n        if !self.read_price_by_share(Skill::ReadPrice) {\n", "libreadprice", "no_skill_is_dealt_three_times"),
    ("reviews can take every slot", "src/schedule.rs", "(self.read_price_reviews + 1) * READ_PRICE_REVIEW_SHARE_DIVISOR <= slots", "self.read_price_reviews < slots", "libreadprice", "reviews_take_at_most_half"),
    ("the cost cue leaves out callers", "src/reflection.rs", "format!(\"You pay ${} to win ${}\", q.bet, pot_with_bet(q.pot_before, q.bet, q.callers))", "format!(\"You pay ${} to win ${}\", q.bet, pot_with_bet(q.pot_before, q.bet, 0))", "libreadprice", "the_reflection_cues_argue"),
    ("the share cue is on the wrong side", "src/reflection.rs", "let shares_for_call = share_points >= price_needed;", "let shares_for_call = share_points < price_needed;", "libreadprice", "the_reflection_cues_argue"),
    ("supported and against are swapped", "src/reflection.rs", "    let (supported, against) = if picked == PriceCall::Easy.code() {\n        (supports_call, supports_fold)\n    } else if picked == PriceCall::Fold.code() {\n        (supports_fold, supports_call)\n", "    let (supported, against) = if picked == PriceCall::Easy.code() {\n        (supports_fold, supports_call)\n    } else if picked == PriceCall::Fold.code() {\n        (supports_call, supports_fold)\n", "libreadprice", "the_reflection_cues_argue"),
    ("the price cue can be trimmed", "src/reflection.rs", "    let mut supports_call = vec![price.clone()];\n    supports_call.extend(shares_for_call.then(|| share.clone()));\n    supports_call.push(hand.clone());\n    supports_call.extend(callers.clone());\n", "    let mut supports_call = vec![hand.clone()];\n    supports_call.extend(callers.clone());\n    supports_call.extend(shares_for_call.then(|| share.clone()));\n    supports_call.push(price.clone());\n", "libreadprice", "the_price_cue_survives"),
    ("a wrong close is argued like a fold", "src/reflection.rs", "    } else if picked == PriceCall::Fold.code() {\n        (supports_fold, supports_call)\n    } else {", "    } else if picked != \"\" {\n        (supports_fold, supports_call)\n    } else {", "libreadprice", "a_wrong_close_gets_its_own"),
    ("the Progress panel is gone", "app/screens.js", "    readPricePanel(data.read_price),\n", "", "readprice", "no read the price panel"),
    ("the question hides its context", "app/reveal.js", "    q.context.map(line => h('p', { class: 'context' }, line)),\n", "", "readprice", "the context lines are not shown"),
    ("rung 3 shows its share", "src/read_price.rs", "lines.push(format!(\"{}, {}. Assume your outs win and nothing else does.\", kind.label(), cards_to_come(street)));", "lines.push(format!(\"{}, {}: about 20%. Assume your outs win and nothing else does.\", kind.label(), cards_to_come(street)));", "readprice", "rung 3 shows the share", True),
    # --- the nuts
    ("the nuts are only the best rank", "src/grade.rs",
     "let is_nut = |h: &Holding| analysis.nuts.contains(h);", "let is_nut = |h: &Holding| rank_of(*h, board) == analysis.top;", "libresearch", "card_removal_makes_a_lesser_holding"),
    ("any of a suit needs only one nut", "src/grade.rs",
     "NutsGuess::CardX(_) | NutsGuess::RankX(_) | NutsGuess::CardAnySuit(..)", "NutsGuess::CardX(_) | NutsGuess::RankX(_)", "libresearch", "a_card_and_any_spade"),
    ("the picker can't say any of a suit", "app/answers.js",
     "return a.suit ? { text: `${first}x${b.suit}` }", "return a.suit ? { text: `${first}x` }", "research", "no any-of-suit form on the picker"),
    # --- preflop share
    ("preflop loses its share", "src/schedule.rs",
     "        self.preflop_floor = true;\n", "", "libresearch", "the_default_drill_keeps_preflop"),
    ("the bridge never keeps the share", "web/src/engine.rs",
     "            real = real.with_preflop_floor();\n", "", "native", "the_default_drill_keeps_preflop_at_a_quarter", True),
    # --- review fixes
    ("a look-ahead copy can search", "src/schedule.rs",
     "        if !self.searches_siblings {\n            return None;\n        }\n", "", "libresearch", "a_look_ahead_copy_starts_no_sibling"),
    ("an old player drops a step", "src/progress.rs",
     "    today.max(earned)\n", "    today\n", "libresearch", "a_step_earned_under_the_old_gates"),
    ("passing beats today skips beats_me", "src/progress.rs",
     ".filter(|a| a.gen_version == OLD_GEN_VERSION)", ".filter(|_| true)", "libresearch", "passing_beats_now_does_not_skip"),
    ("next best can be a nut hand", "src/grade.rs",
     "let beatable = || ranked.iter().filter(|(h, _)| !nuts.contains(h));", "let beatable = || ranked.iter().filter(|(_, r)| *r < top);", "libresearch", "only_the_best_hand_is_held"),
    ("card removal hands are listed as holding the best hand", "src/render.rs",
     "style.holdings(&analysis.top_holdings, 6)),\n            });", "style.holdings(&analysis.nuts, 6)),\n            });", "libcli", "the_nuts_reveal_keeps_card_removal"),
    ("group counts ignore the shown hand", "src/grade.rs",
     "groups_used(std::slice::from_ref(mix), hand.hero, &hand.board, shown)", "groups_used(std::slice::from_ref(mix), hand.hero, &hand.board, 0)", "libresearch", "a_chain_group_count_leaves_out"),
    ("group counts double-count overlaps", "src/grade.rs",
     "if !*taken && matches_opp_class(class, *holding, board) {", "if matches_opp_class(class, *holding, board) {", "libresearch", "group_counts_add_up"),
    ("river spots ask about later cards", "src/reflection.rs",
     "    if q.board.len() < 5 {\n        otherwise.push", "    if true {\n        otherwise.push", "libresearch", "a_river_table_spot"),
    ("a mix still says up against", "src/render.rs",
     "let against = if q.opponents.iter().any(|m| m.player.is_some()) { String::new() } else { format!", "let against = if false { String::new() } else { format!", "libcli", "a_multiway_mix_names_each_opponent"),
    ("chains can break the preflop floor", "src/schedule.rs",
     "if skill == Skill::Chain && !self.chain_fits_preflop_floor() {", "if false {", "libresearch", "a_chain_cannot_push_preflop"),
    ("a rung 3 search regenerates", "src/generate.rs",
     "Skill::BeatsMe if beats_me_rung_of_seed(seed) == 3 => {", "Skill::BeatsMe if beats_me_rung_of_seed(seed) == 9 => {", "libresearch", "rung_three_sibling_search"),
    ("stale checkups aren't flagged", "src/checkup.rs",
     "stale: rows.iter().any(|a| if a.skill == Skill::Table { !a.in_current_library() } else { a.gen_version != GEN_VERSION }),", "stale: false,", "libresearch", "a_checkup_begun_under_an_older"),
    ("the mix method rounds across the band edge", "src/beats_me.rs",
     "That makes you {:.1}% against it", "That makes you {:.0}% against it", "libresearch", "the_mix_method_never_rounds"),
    ("a baseline byte changes", "data/baseline_v1.txt",
     "Do not regenerate", "Do not regenerate!", "libresearch", "the_frozen_baseline_file_is_byte_identical"),
    ("an old checkup resumes", "web/src/engine.rs",
     ".filter(|r| !r.complete && !r.stale);\n        let (run, form)", ".filter(|r| !r.complete);\n        let (run, form)", "native", "an_old_generators_unfinished_checkup", True),
    ("the beats_me status reads as no progress", "web/src/engine.rs",
     'format!("Drill mark counts levels 3 and 4 only: needs {PASS_ATTEMPTS} attempts there, has {}.", s.total_attempts)', 'format!("Drill mark needs {PASS_ATTEMPTS} attempts, has {}.", s.total_attempts)', "native", "the_beats_me_status_says", True),
    ("the any-of-suit echo shows a letter", "web/src/present.rs",
     'format!("{}{s} with any other {}", rank(*a), suit_word(*t))', 'format!("{}{s} with any other {t}", rank(*a))', "native", "a_card_with_any_of_a_suit", True),
    ("hiding the page leaves the tap up", "app/screens.js",
     "      if (reveal && drill === owner) showReveal(outcome, response, undefined, saved);\n", "", "research", "the tap stayed up after the page was hidden"),
    ("a tap after the page was hidden saves again", "app/screens.js",
     "      if (settled || performance.now() - tapShownAt < MIN_ANSWER_MS) return;\n", "      if (performance.now() - tapShownAt < MIN_ANSWER_MS) return;\n", "research", "hiding the page then tapping logged"),
    ("the confidence tap has no double-tap guard", "app/screens.js",
     "      if (settled || performance.now() - tapShownAt < MIN_ANSWER_MS) return;\n", "      if (settled) return;\n", "research", "a confidence tap inside the double-tap window counted"),
    ("reflection cues have no double-tap guard", "app/screens.js",
     "          if (tooSoon()) return;\n          picks.push", "          picks.push", "research", "a cue tapped inside the double-tap window counted"),
    ("End loses partial reflection cues", "app/screens.js",
     "  if (pendingReflection) { pendingReflection(); pendingReflection = null; }\n", "", "research", "partial cues lost on End"),
    # --- preflop: facing-action charts
    ("preflop siblings get the small budget", "src/family.rs",
     "let tries = if skill == Skill::Preflop { PREFLOP_SIBLING_TRIES } else { SIBLING_TRIES };", "let tries = SIBLING_TRIES;", "libpreflop", "rare_preflop_families_still_find_a_sibling"),
    ("the blinds use the position chart", "src/preflop.rs",
     "Situation::Raised if seat == Seat::SmallBlind => Some(Chart::Blinds),", "Situation::Raised if false && seat == Seat::SmallBlind => Some(Chart::Blinds),", "libpreflop", "the_right_chart_answers_each_spot"),
    ("3-bets in position use the other chart", "src/preflop.rs",
     "Situation::ThreeBet { in_position: true } => Some(Chart::ThreeBetIp),", "Situation::ThreeBet { in_position: true } => Some(Chart::ThreeBetOop),", "libpreflop", "the_right_chart_answers_each_spot"),
    ("callers use the lone-raise chart", "src/preflop.rs",
     "Situation::RaisedCalled(_) => Some(Chart::Callers),", "Situation::RaisedCalled(_) => Some(Chart::Position),", "libpreflop", "the_right_chart_answers_each_spot"),
    ("chart raises are never given", "src/preflop.rs",
     "        if raise.range.contains_holding(hand) {\n            Action::Raise", "        if false && raise.range.contains_holding(hand) {\n            Action::Raise", "libpreflop", "every_hand_matches_the_card_text_in_every_situation"),
    ("all in is not named", "src/preflop.rs",
     '(Action::Raise, Situation::FourBet) => "all in",', '(Action::Raise, Situation::FourBet) => "raise",', "libpreflop", "raise_words_name_the_situation"),
    ("suited connectors are not a kind", "src/preflop.rs",
     "            _ if suited && high - low == 1 => HandKind::SuitedConnector,\n", "", "libpreflop", "hand_kinds_are_what_they_say"),
    ("a chart gets a hole", "data/preflop_card.txt",
     "face_raise.call = 22 33 44 55 66 77 88 99 TT", "face_raise.call = 22 33 44 55 66 77 99 TT", "libpreflop", "charts_have_no_holes"),
    ("a hand is on both lines", "data/preflop_card.txt",
     "face_callers.reraise = QQ+ AKs AKo", "face_callers.reraise = JJ+ AKs AKo", "libpreflop", "no_card_line_lists_a_hand_twice_or_on_two_lines_of_a_chart"),
    ("3-bet hands can be ones you never opened", "src/generate.rs",
     'Situation::ThreeBet { .. } => Some(CARD.open_range(seat).expect("an opener has an open range").range),', "Situation::ThreeBet { .. } => None,", "libpreflop", "dealt_hands_are_ones_you_could_hold_in_the_spot"),
    ("4-bet hands can be ones you never 3-bet", "src/generate.rs",
     "Situation::FourBet => Some(chart_for_three_bet(seat)),", "Situation::FourBet => None,", "libpreflop", "dealt_hands_are_ones_you_could_hold_in_the_spot"),
    ("the small blind can be in position", "src/generate.rs",
     "Situation::ThreeBet { in_position: behind >= 2 && bettor >= u64::from(behind) - 2 }", "Situation::ThreeBet { in_position: bettor >= u64::from(behind).saturating_sub(2) }", "libpreflop", "only_possible_new_situations_are_dealt"),
    ("callers can outnumber the earlier players", "src/generate.rs",
     "Situation::RaisedCalled(rng.range(1, u64::from((acted - 1).min(3))) as u8)", "Situation::RaisedCalled(rng.range(1, u64::from(acted.min(3))) as u8)", "libpreflop", "only_possible_new_situations_are_dealt"),
    ("early seats use the wider chart", "src/preflop.rs",
     "Situation::Raised if matches!(seat, Seat::Early | Seat::Middle) => Some(Chart::PositionEarly),\n", "", "libpreflop", "an_early_seat_never_calls_a_raise_with_what_it_folds_cheaper"),
    ("the big blind loses its calls against one raise", "src/preflop.rs",
     "Situation::Raised if seat == Seat::BigBlind => Some(Chart::BlindsBb),\n", "", "libpreflop", "every_hand_matches_the_card_text_in_every_situation"),
    ("the reason line claims a domination it can't show", "src/preflop.rs",
     '"Offsuit high cards usually end with one pair or less, and that is not enough to call a raise with."', '"Offsuit high cards are dominated by the hands that raise, so top pair is often second best."', "libpreflop", "every_reason_line_passes_its_computed_check"),
    ("the big blind loses its extra calls", "src/preflop.rs",
     "Situation::RaisedCalled(_) if seat == Seat::BigBlind => Some(Chart::CallersBb),\n", "", "libpreflop", "every_hand_matches_the_card_text_in_every_situation"),
    ("the extra line is ignored", "src/preflop.rs",
     "|| self.extra_call().is_some_and(|l| l.range.contains_holding(hand))", "", "libpreflop", "every_hand_matches_the_card_text_in_every_situation"),
    ("a reason line lies about the odds", "src/preflop.rs",
     "Jacks are behind the big pairs that 4-bet here and only slightly ahead of ace-king.", "Jacks are behind the big pairs and ace-king hands that 4-bet here.", "libpreflop", "every_reason_line_passes_its_computed_check"),
    ("3-bets are dealt far too often", "src/generate.rs",
     "const SHARE_THREE_BET: u64 = 12;", "const SHARE_THREE_BET: u64 = 40;", "libpreflop", "drills_have_realistic_situation_frequencies"),
    ("facing a raise deals mostly trash", "src/generate.rs",
     "Situation::Raised | Situation::RaisedCalled(_) => Some(relevant_range(seat, situation)),", "Situation::Raised | Situation::RaisedCalled(_) => None,", "libpreflop", "raised_hands_are_never_obvious_folds"),
    ("in-position 3-bets share a family with out of position", "src/family.rs",
     'Situation::ThreeBet { in_position: true } => "3bet_ip",', 'Situation::ThreeBet { in_position: true } => "3bet_oop",', "libpreflop", "new_spots_have_families_that_keep_a_seat"),
    ("the reveal hides the chart line", "app/reveal.js",
     "r.chart ? h('p', { class: 'hint' }, r.chart) : null", "null", "preflopspots", "reveal has no chart line"),
    ("the bridge drops the chart line", "web/src/present.rs",
     '"chart": preflop_chart_line(*reason),', '"chart": None::<String>,', "preflopspots", "reveal has no chart line", True),
    ("the 4-bet prompt says the wrong thing", "app/reveal.js",
     "four_bet: 'You 3-bet and got 4-bet.',", "four_bet: 'Someone has raised.',", "preflopspots", "four_bet prompt read"),
    ("seat tabs stay on for charts that ignore the seat", "app/screens.js",
     "seatGroup.hidden = meta.situations.find(s => s.code === cardState.situation).one_chart;", "seatGroup.hidden = false;", "card", "seat tabs at situation"),
    ("the callers tab shows the lone-raise chart", "web/src/engine.rs",
     '"callers" => Situation::RaisedCalled(1),', '"callers" => Situation::Raised,', "card", "card chart title", True),
]

# A broken browser or harness is not a caught bug: these failures prove nothing.
HARNESS = re.compile(r"raised \w*(Timeout|OSError|Connection|BrokenPipe|WebSocket)\w*|websocket closed")


def fresh_scratch():
    if SCRATCH.exists():
        shutil.rmtree(SCRATCH)
    # copyfile gives fresh timestamps, so cargo can't reuse a build of a mutated source.
    shutil.copytree(ROOT, SCRATCH, ignore=shutil.ignore_patterns("target", ".git"), copy_function=shutil.copyfile)


# Seconds one section may take before its mutant counts as caught. A mutant can make
# a generator's rejection loop spin forever, and it must not hold the machine.
SECTION_TIMEOUTS = {"libreadprice": 300, "readprice": 600}
DEFAULT_SECTION_TIMEOUT = 3600


class TimedOut:
    """Stands in for a finished process: the section was still running at its limit."""
    def __init__(self, stdout):
        self.stdout, self.returncode, self.timed_out = stdout, -signal.SIGKILL, True


def run_section(section):
    # Its own process group, so a timeout also stops cargo, the test binary and Chrome.
    proc = subprocess.Popen([sys.executable, "-u", str(SCRATCH / "web/test/run.py"), section],
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True,
                            env=dict(os.environ, CARGO_TARGET_DIR=str(TARGET_CACHE)))
    try:
        stdout, stderr = proc.communicate(timeout=SECTION_TIMEOUTS.get(section, DEFAULT_SECTION_TIMEOUT))
    except subprocess.TimeoutExpired:
        os.killpg(proc.pid, signal.SIGKILL)
        stdout, _ = proc.communicate()
        return TimedOut(stdout)
    return subprocess.CompletedProcess(proc.args, proc.returncode, stdout, stderr)


def patterns_apply(mutations):
    missing = [m[0] for m in mutations if m[2] not in (ROOT / m[1]).read_text()]
    for name in missing:
        print(f"NOT APPLIED: {name}", flush=True)
    return not missing


def healthy(sections):
    """Each section must pass unmutated, or a failure under a mutation means nothing."""
    for section in sorted(sections):
        fresh_scratch()
        done = run_section(section)
        if done.returncode != 0:
            lines = [l for l in done.stdout.splitlines() if l.startswith("FAIL:")]
            print(f"ABORTED: section {section} fails with no mutation: {(lines or [getattr(done, 'stderr', 'timed out')[-300:]])[0][:300]}", flush=True)
            return False
        print(f"healthy: {section}", flush=True)
    return True


def judge(name, section, expected, done):
    if getattr(done, "timed_out", False):
        print(f"caught (timed out): {name} [{section}]", flush=True)
        return True
    fails = [l for l in done.stdout.splitlines() if l.startswith("FAIL:")]
    # The expected message means the test really caught it; a later cascade timeout proves nothing.
    if done.returncode != 0 and expected in done.stdout:
        print(f"caught: {name} [{section}]", flush=True)
        return True
    broken = [l for l in fails if HARNESS.search(l)]
    if broken:
        print(f"HARNESS FAILURE: {name} [{section}]: {broken[0][:150]}", flush=True)
        return False
    print(f"NOT CAUGHT: {name} [{section}]: wanted {expected!r}, got {[f[:120] for f in fails[:3]]}", flush=True)
    return False


def judge_self_check():
    """The rule itself: a real catch counts even when a timeout cascades after it."""
    class Done:
        def __init__(self, stdout, returncode=1):
            self.stdout, self.returncode = stdout, returncode
    timeout = "FAIL: x raised TimeoutError: page did not load"
    caught_with_cascade = Done(f"FAIL: the real message here\n{timeout}\n")
    assert judge("selfcheck", "s", "the real message", caught_with_cascade), "a cascade timeout hid a real catch"
    assert not judge("selfcheck", "s", "the real message", Done(f"{timeout}\n")), "a timeout alone is a harness failure"
    assert not judge("selfcheck", "s", "the real message", Done("all good\n", 0)), "a passing run caught nothing"


def main():
    """mutation_check.py [--check] [sections...]: --check only proves every pattern still applies."""
    judge_self_check()
    args = sys.argv[1:]
    wanted = [a for a in args if a != "--check"]
    mutations = [m for m in MUTATIONS if not wanted or m[4] in wanted]
    if not patterns_apply(mutations):
        return 1
    if "--check" in args:
        print(f"{len(mutations)} patterns apply", flush=True)
        return 0
    if not healthy({m[4] for m in mutations}):
        return 1
    failures = 0
    for name, path, old, new, section, expected, *rest in mutations:
        fresh_scratch()
        target = SCRATCH / path
        target.write_text(target.read_text().replace(old, new))
        if rest and rest[0]:
            env = dict(os.environ, CARGO_TARGET_DIR=str(TARGET_CACHE))
            built = subprocess.run(["sh", str(SCRATCH / "web/build.sh")], capture_output=True, text=True, env=env)
            if built.returncode != 0:
                print(f"NOT BUILT: {name}: {built.stderr[-400:]}", flush=True)
                failures += 1
                continue
        if not judge(name, section, expected, run_section(section)):
            failures += 1
    shutil.rmtree(SCRATCH, ignore_errors=True)
    print("MUTATIONS ALL CAUGHT" if not failures else f"{failures} MUTATIONS NOT CAUGHT", flush=True)
    return 1 if failures else 0


sys.exit(main())
