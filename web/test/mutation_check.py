"""Confirms the browser tests fail when a fix is removed. Works on a scratch copy
of the repo and leaves the checkout alone. Run on hal: python3 web/test/mutation_check.py"""
import pathlib, shutil, subprocess, sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
SCRATCH = pathlib.Path("/tmp/trainer-mutation-check")

MUTATIONS = [
    ("unsaved log dropped by readLog", "app/store.js", "  if (unsaved) return memory;\n", "", "quota"),
    ("double-tap guard uses the active clock", "app/screens.js",
     "performance.now() - appearedAt < MIN_ANSWER_MS", "activeNow() - shownAt < MIN_ANSWER_MS", "hiddenevent"),
    ("import does not reload the log first", "app/screens.js",
     "      // Another tab may have added attempts; merge into the log as it is now.\n      await syncFromStorage();\n", "", "twotabs"),
    ("drill dealing timer unguarded", "app/screens.js",
     "timer = setTimeout(() => { if (drill === state) renderLoading(leave); }, 150);",
     "timer = setTimeout(() => renderLoading(leave), 150);", "dealing"),
    ("baseline dealing timer unguarded", "app/screens.js",
     "timer = setTimeout(() => { if (baseline === state && state.index === index) renderLoading(leave); }, 150);",
     "timer = setTimeout(() => renderLoading(leave), 150);", "dealing"),
    ("restart leaves pending calls hanging", "app/engine.js",
     "      rejectPending('the worker restarted');\n", "      pending.clear();\n", "prepfail"),
    ("failed bundles stay cached", "app/engine.js",
     "    promise.catch(() => { if (bundles.get(key) === created) bundles.delete(key); });",
     "    promise.catch(() => {});", "prepfail"),
]

def main():
    failures = 0
    for name, path, old, new, section in MUTATIONS:
        if SCRATCH.exists():
            shutil.rmtree(SCRATCH)
        shutil.copytree(ROOT, SCRATCH, ignore=shutil.ignore_patterns("target", ".git"))
        target = SCRATCH / path
        text = target.read_text()
        if old not in text:
            print(f"NOT APPLIED: {name}", flush=True)
            failures += 1
            continue
        target.write_text(text.replace(old, new))
        done = subprocess.run([sys.executable, "-u", str(SCRATCH / "web/test/run.py"), section],
                              capture_output=True, text=True)
        caught = [l for l in done.stdout.splitlines() if l.startswith("FAIL:")]
        if done.returncode != 0 and caught:
            print(f"caught: {name} [{section}]: {caught[0][:150]}", flush=True)
        else:
            print(f"NOT CAUGHT: {name} [{section}]", flush=True)
            failures += 1
    shutil.rmtree(SCRATCH, ignore_errors=True)
    print("MUTATIONS ALL CAUGHT" if not failures else f"{failures} MUTATIONS NOT CAUGHT", flush=True)
    return 1 if failures else 0

sys.exit(main())
