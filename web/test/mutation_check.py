"""Confirms the browser tests fail when a fix is removed. Works on a scratch copy
of the repo and leaves the checkout alone. Run on hal: python3 web/test/mutation_check.py"""
import os, pathlib, re, shutil, subprocess, sys

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
     "        Verdict::Close => inside_loss,\n", "        Verdict::Close => true,\n", "libtable", "close_spot_5_is_correct"),
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
    ("a missed spot comes back in the session", "src/schedule.rs",
     "if !ok && !matches!(planned.skill, Skill::BoardType | Skill::Table) {", "if !ok && planned.skill != Skill::BoardType {", "libtable", "table_spots_get_no_same_session_retry"),
    ("a missed spot is reviewed later", "src/schedule.rs",
     "matches!(attempt.skill, Skill::Anchor | Skill::BoardType | Skill::Table)", "matches!(attempt.skill, Skill::Anchor | Skill::BoardType)", "libtable", "table_spots_get_no_same_session_retry"),
    ("depends attempts count for the pass mark", "src/progress.rs",
     "attempt.mode == Mode::Drill && attempt.extra.table_verdict != Some(Verdict::Depends)", "attempt.mode == Mode::Drill", "libtable", "depends_attempts_do_not_count"),
    ("blunder limit loosened", "src/progress.rs",
     "pub const TABLE_MAX_BLUNDERS: u32 = 2;", "pub const TABLE_MAX_BLUNDERS: u32 = 3;", "libtable", "more_than_two_blunders"),
    ("EV-loss limit loosened", "src/progress.rs",
     "pub const TABLE_MAX_MEAN_EV_LOSS_PCT: f64 = 3.0;", "pub const TABLE_MAX_MEAN_EV_LOSS_PCT: f64 = 3.5;", "libtable", "table_drill_mark_needs_forty"),
    ("the checkup does not gate the pass", "src/progress.rs",
     "passed: drill_passed && checkup_passed == Some(true),", "passed: drill_passed,", "libtable", "the_latest_checkup_must_meet_the_same_standard"),
    ("old log header treated as data", "src/log.rs",
     "line == LOG_HEADER || line == CHECKUP_LOG_HEADER || line == LEGACY_LOG_HEADER", "line == LOG_HEADER || line == LEGACY_LOG_HEADER", "libtable", "table_rows_round_trip"),
    ("table rows fail to parse", "src/log.rs",
     "        FULL_COLUMNS | TABLE_COLUMNS => parse_extra", "        FULL_COLUMNS => parse_extra", "libtable", "table_rows_round_trip"),
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
     "It was best at only {:.1}% of simulated tables, so it is rarely a read worth making.", "This was rarely best ({:.1}).", "libtable", "the_reveal_carries_every_fact"),
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
]

# A broken browser or harness is not a caught bug: these failures prove nothing.
HARNESS = re.compile(r"raised \w*(Timeout|OSError|Connection|BrokenPipe|WebSocket)\w*|websocket closed")


def fresh_scratch():
    if SCRATCH.exists():
        shutil.rmtree(SCRATCH)
    # copyfile gives fresh timestamps, so cargo can't reuse a build of a mutated source.
    shutil.copytree(ROOT, SCRATCH, ignore=shutil.ignore_patterns("target", ".git"), copy_function=shutil.copyfile)


def run_section(section):
    return subprocess.run([sys.executable, "-u", str(SCRATCH / "web/test/run.py"), section],
                          capture_output=True, text=True, env=dict(os.environ, CARGO_TARGET_DIR=str(TARGET_CACHE)))


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
            print(f"ABORTED: section {section} fails with no mutation: {(lines or [done.stderr[-300:]])[0][:300]}", flush=True)
            return False
        print(f"healthy: {section}", flush=True)
    return True


def judge(name, section, expected, done):
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
