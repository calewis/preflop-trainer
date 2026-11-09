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
     "        match grade_timed(&item, raw, response) {\n            Ok(outcome) => {\n                let planned",
     "        match grade_timed(&item, raw, 0) {\n            Ok(outcome) => {\n                let planned", "newskills", "a slow right answer was not a miss", True),
    ("board_type answers graded ahead, ignoring time", "web/src/engine.rs",
     "    if item.skill != Skill::BoardType {\n        if let poker_trainer::answer::AnswerFormat::Choice",
     "    if true {\n        if let poker_trainer::answer::AnswerFormat::Choice", "newskills", "a slow right answer was not a miss", True),
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
    broken = [l for l in fails if HARNESS.search(l)]
    if broken:
        print(f"HARNESS FAILURE: {name} [{section}]: {broken[0][:150]}", flush=True)
        return False
    if done.returncode != 0 and expected in done.stdout:
        print(f"caught: {name} [{section}]", flush=True)
        return True
    print(f"NOT CAUGHT: {name} [{section}]: wanted {expected!r}, got {[f[:120] for f in fails[:3]]}", flush=True)
    return False


def main():
    """mutation_check.py [--check] [sections...]: --check only proves every pattern still applies."""
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
