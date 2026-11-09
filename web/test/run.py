"""Full browser test suite, run on hal: python3 web/test/run.py [sections...]
Sections: native wasmclean doubletap twotabs dealing prepfail export swscope quota hiddenevent differential timing e2e baseline card offline nostorage legacy screens
Results go to target/webtest/results.json and screenshots to target/webtest/screens/."""
import json, os, pathlib, random, re, shutil, subprocess, sys, time

sys.path.insert(0, str(pathlib.Path(__file__).parent))
import cdp, server

ROOT = pathlib.Path(__file__).resolve().parents[2]
OUT = ROOT / "target/webtest"
SHOTS = OUT / "screens"
DEBUG_PORT = 9341
results = {}
failures = []


def fail(message):
    failures.append(message)
    print("FAIL:", message)


def check(condition, message):
    if not condition:
        fail(message)
    return bool(condition)


LAST_ERRORS = []


class Browser:
    def __init__(self, name, port=DEBUG_PORT):
        subprocess.run(["pkill", "-f", f"remote-debugging-port={port}"], check=False)
        time.sleep(0.2)
        profile = OUT / f"profile-{name}"
        shutil.rmtree(profile, ignore_errors=True)
        self.proc = subprocess.Popen(["google-chrome", "--headless=new", "--no-sandbox",
            f"--remote-debugging-port={port}", f"--user-data-dir={profile}", "--no-first-run",
            "--remote-allow-origins=*", "--disable-background-timer-throttling",
            "--disable-renderer-backgrounding", "about:blank"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        for _ in range(50):
            try:
                self.chrome = cdp.Chrome(port)
                self.page = self.chrome.new_page()
                break
            except Exception:
                time.sleep(0.3)
        else:
            raise RuntimeError("chrome did not start")
        self.errors = self.chrome.errors
        LAST_ERRORS[:] = [self.errors, self.page]

    def close(self):
        try:
            LAST_ERRORS.append(self.page.eval("location.href + ' :: ' + document.title + ' :: ' + document.body.innerText", timeout=3)[:1500])
        except Exception as err:
            LAST_ERRORS.append(f"page unavailable: {err}")
        self.proc.terminate()
        try:
            self.proc.wait(10)
        except subprocess.TimeoutExpired:
            self.proc.kill()


def boot_app(page, port, viewport=(390, 844, True), theme="light"):
    page.viewport(*viewport)
    page.theme(theme)
    page.navigate(f"http://127.0.0.1:{port}/")
    page.wait("!!document.querySelector('.app-title') && !!window.__trainer", 30)


WORKER_DIFF = """
(async (commands) => {
  const worker = window.__diffWorker || (window.__diffWorker = new Worker('app/worker.js'));
  const out = [];
  for (const cmd of commands) {
    const reply = await new Promise((resolve) => {
      worker.onmessage = e => resolve(e.data);
      worker.postMessage({ id: 1, args: [cmd] });
    });
    out.push(reply.fatal ? 'FATAL ' + reply.fatal : reply.text);
  }
  return out;
})
"""


def run_differential():
    srv = server.start(8951)
    browser = Browser("diff")
    try:
        page = browser.page
        boot_app(page, 8951)
        native = subprocess.run([str(ROOT / "web/target/release/diff_native"), "60"],
                                capture_output=True, text=True, check=True).stdout.splitlines()
        pairs = [line.split("\t", 1) for line in native]
        commands = [c.replace("\\x1f", "\x1f").replace("\\t", "\t") for c, _ in pairs]
        expected = [r for _, r in pairs]
        got = []
        for start in range(0, len(commands), 300):
            chunk = commands[start:start + 300]
            got += page.eval(f"({WORKER_DIFF})({json.dumps(chunk)})")
        mismatches = [(commands[i], expected[i][:300], got[i][:300]) for i in range(len(commands)) if expected[i] != got[i]]
        by_kind = {}
        for c in commands:
            key = c.split("\x1f")[0] + ":" + (c.split("\x1f")[1] if c.startswith(("prepare\x1f", "grade\x1f")) else "")
            by_kind[key] = by_kind.get(key, 0) + 1
        results["differential"] = {"commands": len(commands), "mismatches": len(mismatches), "by_kind": by_kind}
        if mismatches:
            results["differential"]["first_mismatches"] = mismatches[:5]
            fail(f"differential: {len(mismatches)} of {len(commands)} differ")
        print("differential", len(commands), "commands,", len(mismatches), "mismatches")
        results["differential"]["errors"] = list(browser.errors)
    finally:
        browser.close()
        srv.shutdown()


TIMING = """
(async (skills, count) => {
  const worker = new Worker('app/worker.js');
  const ask = (args) => new Promise(resolve => { worker.onmessage = e => resolve(e.data); worker.postMessage({id: 1, args}); });
  const out = {};
  for (const skill of skills) {
    let total = 0, max = 0, worst = '';
    for (let i = 0; i < count; i++) {
      const seed = String(BigInt(i) * 2654435761n + 99n);
      const reply = await ask(['prepare', skill, seed]);
      if (reply.fatal) throw new Error(reply.fatal);
      total += reply.ms;
      if (reply.ms > max) { max = reply.ms; worst = seed; }
    }
    out[skill] = { mean: total / count, max, worst, count };
  }
  return out;
})
"""


def run_timing():
    srv = server.start(8952)
    browser = Browser("timing")
    try:
        page = browser.page
        boot_app(page, 8952)
        skills = ["preflop", "nuts", "possible", "made", "beats", "outs", "price", "hit"]
        fast = page.eval(f"({TIMING})({json.dumps(skills)}, 200)")
        slow = page.eval(f"({TIMING})(['spot'], 200)")
        mw = page.eval(f"({TIMING})(['spot_mw'], 150)")
        results["timing_ms"] = {**fast, **slow, **mw}
        for k, v in results["timing_ms"].items():
            print(f"timing {k:9} mean {v['mean']:8.1f} ms  max {v['max']:8.1f} ms  worst seed {v['worst']}")
        budgets = {"spot": (100, 1500), "spot_mw": (2000, 8000)}
        expected = {*skills, "spot", "spot_mw"}
        check(set(results["timing_ms"]) == expected, f"timing covered {sorted(results['timing_ms'])}")
        for skill, v in results["timing_ms"].items():
            mean_limit, max_limit = budgets.get(skill, (20, 100))
            check(v["count"] >= 150, f"timing {skill}: only {v['count']} samples")
            check(v["mean"] <= mean_limit, f"timing {skill}: mean {v['mean']:.1f} ms over {mean_limit}")
            check(v["max"] <= max_limit, f"timing {skill}: max {v['max']:.1f} ms over {max_limit}")
    finally:
        browser.close()
        srv.shutdown()


SHOT_JS = """
(() => {
  const bad = [];
  const visible = el => { const r = el.getBoundingClientRect(); return r.width > 0 && r.height > 0 && getComputedStyle(el).visibility !== 'hidden'; };
  document.querySelectorAll('button, a, label.btn, summary').forEach(el => {
    if (!visible(el) || el.closest('details:not([open]) > :not(summary)')) return;
    const r = el.getBoundingClientRect();
    if (r.width < 44 || r.height < 44) bad.push(el.tagName + ' "' + el.textContent.slice(0, 20) + '" ' + Math.round(r.width) + 'x' + Math.round(r.height));
  });
  return { overflow: document.documentElement.scrollWidth - window.innerWidth, bad };
})()
"""

INTERNAL = re.compile(r"\b(open\.\w+|iso\.\w+|face_raise|note\.\w+|small_blind|big_blind|spot_mw|2p|4k|oesd)\b")


class Inspector:
    def __init__(self, page, viewport, theme, capture):
        self.page = page
        self.viewport = viewport
        self.theme = theme
        self.capture = capture
        self.taken = set()

    def text_check(self, where):
        text = self.page.eval("document.body.innerText")
        found = INTERNAL.findall(text)
        check(not found, f"internal code {found[:3]} shown in {where}")
        return text

    def snapshot(self, name):
        self.text_check(name)
        if self.viewport[0] == 390:
            info = self.page.eval(SHOT_JS)
            check(info["overflow"] <= 0, f"horizontal overflow {info['overflow']}px at {name}")
            check(not info["bad"], f"tap targets too small at {name}: {info['bad'][:6]}")
        if self.capture and name not in self.taken:
            self.taken.add(name)
            mode = f"{self.viewport[0]}-{self.theme}"
            SHOTS.mkdir(parents=True, exist_ok=True)
            self.page.screenshot(str(SHOTS / f"{mode}-{name}.png"))


# --- answer driving --------------------------------------------------------------
CLICK_TEXT = """((sel, text) => { const el = [...document.querySelectorAll(sel)].find(e => e.textContent.trim() === text); if (!el) return false; el.click(); return true; })"""


def click_text(page, selector, text):
    check(page.eval(f"({CLICK_TEXT})({json.dumps(selector)}, {json.dumps(text)})"), f"no element {selector} '{text}'")


def click(page, selector):
    check(page.eval(f"(s => {{ const e = document.querySelector(s); if (!e) return false; e.click(); return true; }})({json.dumps(selector)})"),
          f"no element {selector}")


def grade_probe(page, skill, seed, answer):
    return page.eval(f"__trainer.engine.call('grade', {json.dumps(skill)}, {json.dumps(seed)}, {json.dumps(answer)})")


def pick_answer(page, current, want_ok):
    """Returns (kind, answer) with the wanted result, or (kind, None) if impossible."""
    bundle = current["bundle"]
    kind = bundle["format"]["type"]
    skill = bundle["skill"]
    seed = bundle["seed"]
    if kind == "choice":
        for code, outcome in bundle["graded"].items():
            if outcome["ok"] == want_ok:
                return kind, code
        return kind, next(iter(bundle["graded"]))
    if kind == "holding":
        probe = grade_probe(page, skill, seed, "AKs")
        for guess in ("AKs", "QJ", "22"):
            probe = grade_probe(page, skill, seed, guess)
            if "outcome" in probe:
                break
        top = probe["outcome"]["reveal"]["holdings"]["shown"][0]
        if want_ok:
            return kind, top[0] + top[1]
        for guess in ("72o", "32", "54s", "QJ", "TT", "88", "33", "22"):
            result = grade_probe(page, skill, seed, guess)
            if "outcome" in result and not result["outcome"]["ok"]:
                return kind, guess
        return kind, top[0] + top[1]
    if kind == "multi":
        for code in ("none", "f", "s", "h", "fs", "fh", "sh", "fsh"):
            result = grade_probe(page, skill, seed, code)
            if result["outcome"]["correct"] == want_ok:
                return kind, code
        return kind, "none"
    if kind == "integer":
        for n in range(0, 48):
            result = grade_probe(page, skill, seed, str(n))
            if result["outcome"]["correct"] == want_ok:
                return kind, str(n)
        return kind, "0"
    if kind == "percent":
        for n in range(1, 100):
            result = grade_probe(page, skill, seed, str(n))
            if result["outcome"]["correct"] == want_ok:
                return kind, str(n)
        return kind, "1"
    raise RuntimeError(kind)


def type_answer(page, current, kind, answer, keyboard=False):
    if kind == "choice":
        options = current["bundle"]["format"]["options"]
        index = next(i for i, o in enumerate(options) if o["code"] == answer)
        if keyboard and index < 9:
            press_key(page, str(index + 1))
        else:
            label = options[index]["label"]
            page.eval(f"[...document.querySelectorAll('.choice')].find(b => b.textContent.endsWith({json.dumps(label)})).click()")
    elif kind == "multi":
        if answer != "none":
            for ch in answer:
                index = {"f": 0, "s": 1, "h": 2}[ch]
                page.eval(f"document.querySelectorAll('.toggle')[{index}].click()")
        click(page, ".answers .btn.primary")
    elif kind in ("integer", "percent"):
        for ch in answer:
            click_text(page, ".keypad .key", ch)
        click(page, ".answers .btn.primary")
    elif kind == "holding":
        click_holding(page, answer)
    else:
        raise RuntimeError(kind)


def click_holding(page, answer):
    label = lambda r: "10" if r == "T" else r
    if answer.endswith("x"):
        click_text(page, ".rank-grid .rank", label(answer[0]))
        if len(answer) == 3:
            page.eval(f"document.querySelectorAll('.slot')[0].querySelector('.slot-suit.suit-{answer[1]}').click()")
        click(page, ".slot-any")
    elif len(answer) == 4:
        click_text(page, ".rank-grid .rank", label(answer[0]))
        page.eval(f"document.querySelectorAll('.slot')[0].querySelector('.slot-suit.suit-{answer[1]}').click()")
        click_text(page, ".rank-grid .rank", label(answer[2]))
        page.eval(f"document.querySelectorAll('.slot')[1].querySelector('.slot-suit.suit-{answer[3]}').click()")
    else:
        click_text(page, ".rank-grid .rank", label(answer[0]))
        click_text(page, ".rank-grid .rank", label(answer[1]))
        if len(answer) == 3:
            click(page, f".chip-toggle[data-code='{answer[2]}']")
    check(page.eval("!document.querySelector('.picker .btn.primary').disabled"), f"holding {answer} not accepted by the picker")
    click(page, ".picker .btn.primary")


def press_key(page, key):
    text = key if len(key) == 1 else ("\r" if key == "Enter" else "")
    code = "Enter" if key == "Enter" else f"Digit{key}" if key.isdigit() else f"Key{key.upper()}"
    vk = 13 if key == "Enter" else ord(key.upper())
    for kind in ("keyDown", "keyUp"):
        page.send("Input.dispatchKeyEvent", {"type": kind, "key": key, "code": code, "text": text if kind == "keyDown" else "",
                                             "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk})


# An answer within 300 ms of an item appearing is ignored, so wait out that window.
READY = """(() => { const q = document.querySelector('.question:not(.revealed)'); const live = document.querySelector('.answers button:not([disabled])'); const c = __trainer.current; return !!(q && live && c && c.bundle && performance.now() - c.appearedAt > 330 && !document.querySelector('.reveal')); })()"""

PHANTOM_TAP = """((isNew) => new Promise(resolve => {
  const started = performance.now();
  (function poll() {
    const c = __trainer.current;
    if (c && isNew(c)) {
      const button = document.querySelector('.choice:not([disabled]), .answers .btn.primary:not([disabled])');
      if (button) button.click();
      resolve(!!button);
    } else if (performance.now() - started > 8000) resolve(false);
    else setTimeout(poll, 2);
  })();
}))"""


def norm_answer(kind, answer):
    return answer.lower() if kind == "holding" else answer


def rows_of(page):
    text = page.eval(f"localStorage.getItem({json.dumps(LOG_KEY)}) || ''")
    return [line.split("\t") for line in text.split("\n") if line and not line.startswith("timestamp")]


def check_logged(rows, records, where):
    """Each answer given must appear in the log, in order, with the right fields."""
    check(len(rows) == len(records), f"{where}: {len(rows)} log lines for {len(records)} answers")
    for row, want in zip(rows, records):
        label = f"{where} {want['skill']} {want['seed']}"
        check(row[1] == want["mode"], f"{label}: mode {row[1]}, expected {want['mode']}")
        check(row[3] == want["skill"], f"{label}: skill {row[3]}")
        check(row[4] == want["seed"], f"{label}: item {row[4]}")
        check(row[10] == want["answer"], f"{label}: answer {row[10]!r}, expected {want['answer']!r}")
        # A correct spot answer can also be flagged close, so close is only pinned when wrong.
        check(int(row[6]) == want["correct"] and (want["correct"] == 1 or int(row[7]) == want["close"]),
              f"{label}: flags {row[6:8]}")
        response = int(row[9])
        check(response >= 300, f"{label}: response {response} under the double-tap window")
        check(want["low"] - 60 <= response <= want["high"] + 60,
              f"{label}: response {response} outside {want['low']:.0f}-{want['high']:.0f}")


def drill_session(page, inspector, want_all, max_items, keyboard=False, label_prefix="", phantom=False):
    """Plays a drill until every skill has been answered right and wrong (when asked)."""
    seen = {}
    records = []
    items = 0
    forms = ["AKs", "QJ", "Ahx", "Ax", "AhKh"]
    form_index = 0
    last_seed = None
    while items < max_items:
        page.wait(READY + " || !!document.querySelector('.big-score')", 60)
        if page.eval("!!document.querySelector('.big-score')"):
            break
        current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
        bundle = current["bundle"]
        skill = bundle["skill"]
        ident = (skill, bundle["seed"], items)
        last_seed = ident
        marks = seen.setdefault(skill, set())
        # Wrong answers raise a skill's weight, so finished skills go on being answered
        # right to keep the rare skills coming up.
        want_ok = "right" not in marks or ("wrong" in marks and "right" in marks)
        kind, answer = pick_answer(page, current, want_ok)
        if kind == "holding" and items % 3 == 0:
            # Exercise the other accepted forms too.
            alt = forms[form_index % len(forms)]
            form_index += 1
            if "outcome" in grade_probe(page, skill, bundle["seed"], alt):
                answer = alt
        inspector.snapshot(f"{label_prefix}q-{skill}")
        before_click = page.eval("__trainer.activeNow()")
        type_answer(page, current, kind, answer, keyboard=keyboard and items % 2 == 0)
        after_click = page.eval("__trainer.activeNow()")
        try:
            page.wait("!!document.querySelector('.reveal')", 30)
        except TimeoutError:
            print("  stuck:", kind, repr(answer), page.eval("JSON.stringify({vis: document.visibilityState, now: performance.now(), shownAt: __trainer.current.shownAt, seed: __trainer.current.bundle.seed, want: " + json.dumps(bundle["seed"]) + "})"), flush=True)
            raise
        outcome_ok = page.eval("!document.querySelector('.reveal').classList.contains('bad')")
        marks.add("right" if outcome_ok else "wrong")
        tone = page.eval("document.querySelector('.reveal').className")
        shown_at = current["shownAt"]
        records.append({
            "skill": skill, "seed": bundle["seed"],
            "mode": "drill-retry" if current["planned"]["origin"] == "retry" else "drill",
            "answer": norm_answer(kind, answer),
            "correct": int("good" in tone), "close": int("close" in tone),
            "low": before_click - shown_at, "high": after_click - shown_at,
        })
        check(page.eval("document.querySelector('.verdict').textContent.length > 3"), f"no verdict for {skill}")
        check(page.eval("document.querySelector('[role=status]').textContent.length > 3"), f"reveal not announced for {skill}")
        inspector.snapshot(f"{label_prefix}r-{skill}")
        items += 1
        shown_before = current["shownAt"]
        if keyboard:
            press_key(page, "Enter")
        else:
            click(page, ".action-bar .btn")
        if phantom:
            # The second tap of a double-tap on Next lands on the next item.
            page.eval(f"({PHANTOM_TAP})(c => c.shownAt !== {shown_before})")
            time.sleep(0.05)
            check(page.eval("!document.querySelector('.reveal')"), "a tap right after Next answered the new item")
        page.wait("!document.querySelector('.reveal')", 30)
        if not want_all:
            continue
        if len(seen) == 10 and all({"right", "wrong"} <= v for v in seen.values()):
            break
    return items, seen, records


def synthetic_log():
    """40 correct, fast attempts for steps 1 and 2, so step 3 unlocks."""
    now = int(time.time() * 1000) - 86400000 * 5
    lines = ["timestamp_ms\tmode\tbaseline_run\tskill\titem\tgen_version\tcorrect\tclose\tev_loss_pct\tresponse_ms\tanswer"]
    t = now
    for skill, answer in [("nuts", "AKs"), ("possible", "f"), ("made", "top"), ("beats", "a"), ("outs", "8"), ("price", "25"), ("hit", "20")]:
        for i in range(45):
            t += 1000
            lines.append(f"{t}\tdrill\t\t{skill}\t{1000 + i}\t1\t1\t0\t\t1500\t{answer}")
    return "\n".join(lines) + "\n"


LOG_KEY = "poker-trainer.attempts.v1"


def seed_step3(page, port):
    page.navigate(f"http://127.0.0.1:{port}/")
    page.wait("!!document.querySelector('.app-title') && !!window.__trainer", 30)
    page.eval(f"localStorage.setItem({json.dumps(LOG_KEY)}, {json.dumps(synthetic_log())})")
    page.send("Page.reload", {})
    page.wait("!!document.querySelector('.app-title') && !!window.__trainer", 30)
    time.sleep(0.2)


def start_drill(page):
    page.eval("document.querySelectorAll('.seg')[2].click()")
    page.wait("[...document.querySelectorAll('.panel .btn.primary')].some(b => b.textContent === 'Start drill')", 10)
    page.eval("[...document.querySelectorAll('.panel .btn.primary')].find(b => b.textContent === 'Start drill').click()")
    page.wait(READY, 60)


def log_lines(page):
    return page.eval(f"(localStorage.getItem({json.dumps(LOG_KEY)}) || '').split('\\n').filter(l => l && !l.startsWith('timestamp')).length")


def end_drill_summary(page, inspector):
    page.wait("!!document.querySelector('.top-bar button.back')", 30)
    page.eval("document.querySelector('.top-bar button.back').click()")
    page.wait("!!document.querySelector('.big-score') && document.querySelector('.big-score').textContent.includes(' right')", 20)
    inspector.snapshot("summary")


def import_file(page, path):
    page.send("DOM.enable")
    doc = page.send("DOM.getDocument")
    node = page.send("DOM.querySelector", {"nodeId": doc["root"]["nodeId"], "selector": "input[type=file]"})
    page.eval("(() => { const s = document.querySelector('p[role=status]'); if (s) s.textContent = ''; })()")
    page.send("DOM.setFileInputFiles", {"nodeId": node["nodeId"], "files": [str(path)]})
    page.wait("(document.querySelector('p[role=status]') || {}).textContent", 20)
    return page.eval("document.querySelector('p[role=status]').textContent")


def run_e2e():
    srv = server.start(8953)
    browser = Browser("e2e")
    try:
        page = browser.page
        page.viewport(390, 844, True)
        page.theme("light")
        inspector = Inspector(page, (390, 844), "light", False)
        seed_step3(page, 8953)
        inspector.snapshot("home")
        check(page.eval("document.body.innerText.includes('Step 3 of 3')"), "synthetic log did not unlock step 3")
        before = log_lines(page)
        start_drill(page)
        items, seen, records = drill_session(page, inspector, True, 500)
        print("e2e drill answered", items, {k: sorted(v) for k, v in seen.items()})
        missing = [s for s in ["preflop", "nuts", "possible", "made", "beats", "outs", "price", "hit", "spot", "spot_mw"]
                   if not {"right", "wrong"} <= seen.get(s, set())]
        check(not missing, f"skills not covered with right and wrong answers: {missing}")
        results["e2e_drill_items"] = items
        check_logged(rows_of(page)[before:], records, "e2e drill")
        end_drill_summary(page, inspector)
        summary_text = page.eval("document.querySelector('.big-score').textContent")
        print("summary:", summary_text)
        # Persistence across reload.
        page.send("Page.reload", {})
        page.wait("!!document.querySelector('.app-title') && !!window.__trainer", 30)
        after_reload = log_lines(page)
        check(after_reload == before + items, f"after reload {after_reload} lines, expected {before + items}")
        rows_before_export = rows_of(page)
        check_logged(rows_before_export[before:], records, "e2e after reload")
        page.eval("location.hash = '#progress'")
        page.wait("!!document.querySelector('.skill-row')", 20)
        inspector.snapshot("progress")
        text = page.eval("document.body.innerText")
        check("Pass mark" in text and re.search(r"Step \d of 3", text), f"progress text: {text[:200]}")
        check("win rate" not in text.lower(), "progress mentions a win rate")
        # Export, then import into empty storage, then re-import.
        exported = page.eval("__trainer.engine.call('export').then(r => r.text)")
        (OUT / "export.tsv").write_text(exported)
        exported_count = len([l for l in exported.split("\n") if l and not l.startswith("timestamp")])
        exported_rows = [l.split("\t") for l in exported.split("\n") if l and not l.startswith("timestamp")]
        check(sorted(exported_rows) == sorted(rows_before_export), "export differs from the stored log")
        page.eval(f"localStorage.removeItem({json.dumps(LOG_KEY)})")
        page.eval("location.hash = '#home'")
        page.wait("!!document.querySelector('.app-title')", 10)
        page.send("Page.reload", {})
        page.wait("!!document.querySelector('.app-title') && !!window.__trainer", 30)
        check(log_lines(page) == 0, "storage not cleared")
        page.eval("location.hash = '#backup'")
        page.wait("!!document.querySelector('input[type=file]')", 20)
        inspector.snapshot("backup")
        page.send("DOM.enable")
        first = import_file(page, OUT / "export.tsv")
        print("import 1:", first)
        check(f"Imported {exported_count} new" in first, f"import result: {first}")
        check(log_lines(page) == exported_count, f"after import {log_lines(page)} lines")
        check(sorted(rows_of(page)) == sorted(exported_rows), "imported log differs from the exported file")
        second = import_file(page, OUT / "export.tsv")
        print("import 2:", second)
        check("Imported 0 new" in second, f"reimport result: {second}")
        check(log_lines(page) == exported_count, "reimport changed the log")
        # A file with one bad line and a few new rows.
        extra = exported.strip().split("\n")[1:4]
        ts = int(time.time() * 1000)
        extra += [f"{ts}\tdrill\t\tprice\t7\t1\t1\t0\t\t1200\t25", "garbage line"]
        (OUT / "extra.tsv").write_text("\n".join(extra) + "\n")
        third = import_file(page, OUT / "extra.tsv")
        print("import 3:", third)
        check("Imported 1 new" in third and "malformed" in third, f"mixed import result: {third}")
        results["e2e_errors"] = list(browser.errors)
        check(not browser.errors, f"browser errors: {browser.errors[:5]}")
    finally:
        browser.close()
        srv.shutdown()


def answer_baseline_item(page, current):
    """Answers the shown item and returns the answer the log should hold."""
    kind = current["bundle"]["format"]["type"]
    if kind == "choice":
        options = current["bundle"]["format"]["options"]
        pick = options[random.randrange(len(options))]
        page.eval(f"[...document.querySelectorAll('.choice')].find(b => b.textContent.endsWith({json.dumps(pick['label'])})).click()")
        return pick["code"]
    if kind == "multi":
        click(page, ".answers .btn.primary")
        return "none"
    if kind in ("integer", "percent"):
        click_text(page, ".keypad .key", "7")
        click(page, ".answers .btn.primary")
        return "7"
    click_text(page, ".rank-grid .rank", "A")
    click(page, ".slot-any")
    click(page, ".picker .btn.primary")
    return "ax"


def run_baseline(port=8954, inspector_args=None):
    srv = server.start(port)
    browser = Browser("baseline")
    try:
        page = browser.page
        boot_app(page, port)
        inspector = Inspector(page, (390, 844), "light", False)
        check(page.eval("document.body.innerText.includes('Take the baseline')"), "no baseline prompt")
        answers = complete_baseline(page, inspector)
        check_baseline_rows(rows_of(page), answers, "1")
        text = page.eval("document.body.innerText")
        print("baseline result:", text[:400].replace("\n", " | "))
        check(re.search(r"\d+ of 36", text), "baseline report has no score")
        check("By section" in text and "By step" in text, "baseline report sections missing")
        check("first baseline" in text, "first baseline comparison text missing")
        inspector.snapshot("baseline-report")
        lines = log_lines(page)
        check(lines == 36, f"baseline logged {lines} attempts")
        # A second run compares with the first.
        page.eval("location.hash = '#home'")
        page.wait("!!document.querySelector('.app-title')", 20)
        check(not page.eval("document.body.innerText.includes('Take the baseline first')"), "baseline prompt still shown")
        page.eval("location.hash = '#baseline'")
        page.wait("[...document.querySelectorAll('.btn.primary')].some(b => b.textContent === 'Begin')", 20)
        answers = complete_baseline(page, inspector)
        check_baseline_rows(rows_of(page)[36:], answers, "2")
        text = page.eval("document.body.innerText")
        check("Earlier runs" in text and "Run 1" in text, f"second run missing comparison: {text[:300]}")
        # Quitting early keeps what was answered.
        page.eval("location.hash = '#home'")
        page.wait("!!document.querySelector('.app-title')", 20)
        page.eval("location.hash = '#baseline'")
        page.wait("[...document.querySelectorAll('.btn.primary')].some(b => b.textContent === 'Begin')", 20)
        page.eval("[...document.querySelectorAll('.btn.primary')].find(b => b.textContent === 'Begin').click()")
        page.wait(READY, 30)
        current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
        answer_baseline_item(page, current)
        page.wait("!!document.querySelector('.top-bar .count') && document.querySelector('.top-bar .count').textContent.startsWith('2 of')", 20)
        page.wait("!!document.querySelector('.top-bar button.back')", 20)
        page.eval("document.querySelector('.top-bar button.back').click()")
        page.wait("document.body.innerText.includes('unfinished')", 20)
        results["baseline_ok"] = True
        check(log_lines(page) == 73, f"after two runs and one answer: {log_lines(page)} lines")
        results["baseline_errors"] = list(browser.errors)
        check(not browser.errors, f"baseline browser errors: {browser.errors[:5]}")
    finally:
        browser.close()
        srv.shutdown()


def check_baseline_rows(rows, answers, run):
    check(len(rows) == 36, f"baseline logged {len(rows)} attempts")
    check(len({r[4] for r in rows}) == 36 and all(re.fullmatch(r"b\d\d", r[4]) for r in rows), "baseline item ids wrong")
    for row, (skill, answer) in zip(rows, answers):
        check(row[1] == "baseline" and row[2] == run, f"baseline row mode/run {row[1:3]}")
        check(row[3] == skill, f"baseline row skill {row[3]}, expected {skill}")
        check(row[10] == answer, f"baseline row answer {row[10]!r}, expected {answer!r}")
        check(int(row[9]) >= 300, f"baseline response {row[9]}")


def complete_baseline(page, inspector):
    answers = []
    if page.eval("!!document.querySelector('.callout .btn')"):
        page.eval("document.querySelector('.callout .btn').click()")
    if page.eval("location.hash") != "#baseline":
        page.eval("location.hash = '#baseline'")
    page.wait("[...document.querySelectorAll('.btn.primary')].some(b => b.textContent === 'Begin')", 20)
    page.eval("[...document.querySelectorAll('.btn.primary')].find(b => b.textContent === 'Begin').click()")
    for i in range(36):
        page.wait(READY, 60)
        inspector.snapshot("baseline-item")
        current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
        counter = page.eval("document.querySelector('.top-bar .count').textContent")
        check(counter == f"{i + 1} of 36", f"baseline counter {counter} at {i}")
        answers.append((current["bundle"]["skill"], answer_baseline_item(page, current)))
    page.wait("!!document.querySelector('.big-score')", 60)
    check(page.eval("document.querySelector('.big-score').textContent").endswith("of 36"), "baseline not scored of 36")
    return answers


def run_card():
    srv = server.start(8955)
    browser = Browser("card")
    try:
        page = browser.page
        boot_app(page, 8955)
        inspector = Inspector(page, (390, 844), "light", False)
        page.eval("location.hash = '#card'")
        page.wait("document.querySelectorAll('.cell').length === 169", 20)
        opening = page.eval("document.querySelector('.tabs .tab[aria-pressed=true]').textContent")
        check(opening == "Cutoff", f"card opened on {opening}")
        seats = page.eval("[...document.querySelectorAll('.tabs')[0].children].length")
        check(seats == 6, f"{seats} seat tabs")
        seen = 0
        for s in range(6):
            for k in range(3):
                page.eval(f"document.querySelectorAll('.tabs')[0].children[{s}].click(); document.querySelectorAll('.tabs')[1].children[{k}].click();")
                time.sleep(0.05)
                page.wait("document.querySelectorAll('.cell').length === 169", 5)
                inspector.snapshot(f"card-{s}-{k}")
                seen += 1
                if not (s == 5 and k == 0):
                    check(page.eval("/% of hands/.test(document.body.innerText)"), f"no percent on card view {s},{k}")
        text = page.eval("document.body.innerText")
        check("Notes" in text and "iso." not in text, "card notes")
        results["card_views"] = seen
        check(not browser.errors, f"card errors {browser.errors[:3]}")
    finally:
        browser.close()
        srv.shutdown()


def run_offline():
    port = 8956
    srv = server.start(port)
    browser = Browser("offline")
    try:
        page = browser.page
        boot_app(page, port)
        page.wait("navigator.serviceWorker.ready.then(() => true)", 30)
        page.wait("!!navigator.serviceWorker.controller || navigator.serviceWorker.ready.then(r => { r.active && r.active.state; return !!navigator.serviceWorker.controller; })", 30)
        page.eval("new Promise(r => { if (navigator.serviceWorker.controller) r(); else navigator.serviceWorker.addEventListener('controllerchange', r); })")
        time.sleep(0.5)
        srv.shutdown()
        srv.server_close()
        page.send("Page.reload", {})
        page.wait("!!document.querySelector('.app-title') && !!window.__trainer", 30)
        check(page.eval("document.body.innerText.includes('Poker Trainer')"), "offline reload did not show app")
        inspector = Inspector(page, (390, 844), "light", False)
        start_drill(page)
        items, *_ = drill_session(page, inspector, False, 4)
        page.wait("!!document.querySelector('.top-bar button.back')", 20)
        page.eval("document.querySelector('.top-bar button.back').click()")
        page.wait("!!document.querySelector('.big-score')", 20)
        results["offline_items"] = items
        check(items == 4, "offline drill incomplete")
        page.eval("location.hash = '#card'")
        page.wait("document.querySelectorAll('.cell').length === 169", 20)
        check(not browser.errors, f"offline errors: {browser.errors[:5]}")
        print("offline ok, items:", items)
    finally:
        browser.close()


def run_nostorage():
    port = 8957
    srv = server.start(port)
    browser = Browser("nostorage")
    try:
        page = browser.page
        page.send("Page.addScriptToEvaluateOnNewDocument", {
            "source": "Object.defineProperty(window, 'localStorage', { get() { throw new DOMException('denied', 'SecurityError'); } });"})
        boot_app(page, port)
        check(page.eval("!document.getElementById('notice').hidden"), "no storage notice")
        inspector = Inspector(page, (390, 844), "light", False)
        inspector.snapshot("notice-home")
        start_drill(page)
        items, *_ = drill_session(page, inspector, False, 3)
        check(items == 3, "drill without storage incomplete")
        check(not browser.errors, f"nostorage errors: {browser.errors[:5]}")
        print("nostorage ok")
    finally:
        browser.close()
        srv.shutdown()


def run_legacy():
    t0 = time.time()
    stamp = lambda label: print(f"  legacy {label} at {time.time() - t0:.1f}s", flush=True)
    port = 8958
    srv = server.start(port)
    srv.legacy = True
    browser = Browser("legacy")
    try:
        page = browser.page
        page.viewport(390, 844, True)
        page.navigate(f"http://127.0.0.1:{port}/")
        page.wait("document.title === 'Old Preflop App'", 20)
        page.wait("navigator.serviceWorker.ready.then(() => true)", 20)
        page.eval("new Promise(r => { if (navigator.serviceWorker.controller) r(); else navigator.serviceWorker.addEventListener('controllerchange', r); })")
        names = page.eval("caches.keys()")
        check(names == ["preflop-v32"], f"legacy cache: {names}")
        stamp("old app installed")
        page.eval("caches.open('other-project-v1').then(() => 1)")
        srv.legacy = False
        # The old app page checks for an update, as any navigation would.
        stamp("before update")
        page.eval("void navigator.serviceWorker.getRegistration().then(r => r.update()).catch(() => null); 1", timeout=20)
        stamp("after update")
        try:
            page.wait("document.title === 'Poker Trainer'", 30)
        except TimeoutError:
            print("  event counts:", sorted(page.event_counts.items(), key=lambda kv: -kv[1])[:8])
            print("  errors:", browser.errors[:5])
            raise
        stamp("title is new")
        page.wait("!!document.querySelector('.app-title') && !!window.__trainer", 30)
        stamp("new app on screen")
        time.sleep(0.5)
        names = page.eval("caches.keys()")
        ours = [n for n in names if n.startswith("poker-trainer-")]
        check(len(ours) == 1, f"caches after takeover: {names}")
        check("other-project-v1" in names, f"another project's cache was deleted: {names}")
        check(not any(n.startswith("preflop-v") for n in names), f"old cache survived: {names}")
        check(page.eval("!!navigator.serviceWorker.controller"), "no controller after takeover")
        page.navigate(f"http://127.0.0.1:{port}/trainer.html")
        page.wait("document.title === 'Poker Trainer' && !!window.__trainer", 30)
        stamp("old address serves new app")
        results["legacy_takeover"] = "ok"
        check(not browser.errors, f"legacy errors: {browser.errors[:5]}")
        print("legacy takeover ok")
    finally:
        browser.close()
        srv.shutdown()


def run_screens():
    if SHOTS.exists():
        shutil.rmtree(SHOTS)
    port = 8960
    for viewport, mobile in [((390, 844), True), ((1280, 800), False)]:
        for theme in ("light", "dark"):
            port += 1
            srv = server.start(port)
            browser = Browser(f"shots-{viewport[0]}-{theme}")
            try:
                capture_mode(browser, port, viewport, mobile, theme)
                check(not browser.errors, f"screenshot run errors ({viewport} {theme}): {browser.errors[:5]}")
            finally:
                browser.close()
                srv.shutdown()


def capture_mode(browser, port, viewport, mobile, theme):
    page = browser.page
    page.viewport(*viewport, mobile)
    page.theme(theme)
    inspector = Inspector(page, viewport, theme, True)
    boot_app(page, port, (*viewport, mobile), theme)
    inspector.snapshot("home-new")
    page.eval("location.hash = '#baseline'")
    page.wait("[...document.querySelectorAll('.btn.primary')].some(b => b.textContent === 'Begin')", 20)
    inspector.snapshot("baseline-intro")
    complete_baseline(page, inspector)
    inspector.snapshot("baseline-report")
    seed_with_baseline = page.eval(f"localStorage.getItem({json.dumps(LOG_KEY)})")
    combined = synthetic_log() + "\n".join(l for l in seed_with_baseline.split("\n") if l and not l.startswith("timestamp")) + "\n"
    page.eval(f"localStorage.setItem({json.dumps(LOG_KEY)}, {json.dumps(combined)})")
    page.eval("location.hash = '#home'")
    page.wait("!!document.querySelector('.app-title')", 20)
    page.send("Page.reload", {})
    page.wait("!!document.querySelector('.app-title') && !!window.__trainer", 30)
    inspector.snapshot("home")
    page.eval("location.hash = '#card'")
    page.wait("document.querySelectorAll('.cell').length === 169", 20)
    inspector.snapshot("card-early")
    page.eval("document.querySelectorAll('.tabs')[1].children[1].click()")
    time.sleep(0.1)
    inspector.snapshot("card-limpers")
    page.eval("document.querySelectorAll('.tabs')[0].children[3].click()")
    page.eval("document.querySelectorAll('.tabs')[1].children[2].click()")
    time.sleep(0.1)
    inspector.snapshot("card-raised")
    page.eval("location.hash = '#progress'")
    page.wait("!!document.querySelector('.skill-row')", 20)
    inspector.snapshot("progress")
    page.eval("location.hash = '#backup'")
    page.wait("!!document.querySelector('input[type=file]')", 20)
    inspector.snapshot("backup")
    page.eval("location.hash = '#home'")
    page.wait("!!document.querySelector('.app-title')", 20)
    start_drill(page)
    items, seen, _ = drill_session(page, inspector, True, 300, keyboard=not mobile)
    end_drill_summary(page, inspector)
    print(viewport, theme, "items", items, "screens", len(inspector.taken))



HOLD_WORKER = """
(() => {
  const original = Worker.prototype.postMessage;
  window.__held = [];
  window.__holdPrep = true;
  window.__failPrep = false;
  window.__passPrep = 0;
  Worker.prototype.postMessage = function (message) {
    const isPrep = message && message.args && String(message.args[0]).startsWith('prepare');
    if (isPrep && window.__failPrep) {
      const worker = this;
      setTimeout(() => worker.onmessage({ data: { id: message.id, fatal: 'simulated trap' } }), 0);
      return;
    }
    if (isPrep && window.__passPrep > 0) { window.__passPrep -= 1; return original.call(this, message); }
    if (isPrep && window.__holdPrep) { window.__held.push([this, message]); return; }
    return original.call(this, message);
  };
  window.__releasePrep = () => {
    window.__holdPrep = false;
    window.__held.splice(0).forEach(([worker, message]) => original.call(worker, message));
  };
})();
"""

DEALING = "document.body.innerText.includes('Dealing...')"

# Remembers the workers in creation order and makes the prep worker (the second)
# one that can really crash, so its onerror path can be driven.
CRASHABLE_PREP = """
(() => {
  const RealWorker = window.Worker;
  window.__workers = [];
  window.Worker = function (url, options) {
    const useCrashable = window.__workers.length === 1;
    const worker = new RealWorker(useCrashable ? new URL('/app/crash-worker.js', location.href) : url, options);
    window.__workers.push(worker);
    return worker;
  };
  window.Worker.prototype = RealWorker.prototype;
})();
"""

END_WITHIN_WINDOW = """(async () => {
  document.querySelector('.action-bar .btn').click();
  const started = performance.now();
  await new Promise(resolve => setTimeout(resolve, 60));
  document.querySelector('.top-bar button.back').click();
  return performance.now() - started;
})()"""


def run_native():
    done = subprocess.run(["cargo", "test", "--manifest-path", "web/Cargo.toml", "--release"],
                          capture_output=True, text=True, cwd=ROOT)
    summary = [l for l in done.stdout.splitlines() if l.startswith("test result")]
    results["native_tests"] = summary
    print("native:", summary)
    check(done.returncode == 0, f"native tests failed: {done.stdout[-1500:]}")


def run_wasm_clean():
    data = (ROOT / "app/trainer.wasm").read_bytes()
    home = os.path.expanduser("~").encode()
    check(home not in data, "the wasm contains the home directory path")
    check(b"poker_engine" not in data, "the wasm contains a local checkout path")
    print("wasm has no local paths")


def run_doubletap():
    port = 8971
    srv = server.start(port)
    browser = Browser("doubletap")
    try:
        page = browser.page
        boot_app(page, port)
        inspector = Inspector(page, (390, 844), "light", False)
        # Baseline: a second tap lands on each following question.
        page.eval("location.hash = '#baseline'")
        page.wait("[...document.querySelectorAll('.btn.primary')].some(b => b.textContent === 'Begin')", 20)
        page.eval("[...document.querySelectorAll('.btn.primary')].find(b => b.textContent === 'Begin').click()")
        fired = 0
        answers = []
        for i in range(36):
            page.wait(READY, 60)
            counter = page.eval("document.querySelector('.top-bar .count').textContent")
            check(counter == f"{i + 1} of 36", f"phantom tap skipped a question: {counter} at {i}")
            current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
            answers.append((current["bundle"]["skill"], answer_baseline_item(page, current)))
            if i < 35:
                fired += bool(page.eval(f"({PHANTOM_TAP})(c => c.baselineIndex === {i + 1})"))
        page.wait("!!document.querySelector('.big-score')", 60)
        check(fired >= 15, f"only {fired} phantom taps landed")
        check_baseline_rows(rows_of(page), answers, "1")
        results["doubletap_baseline_phantoms"] = fired
        # Start tapped twice still starts one drill that shows.
        page.eval("location.hash = '#home'")
        page.wait("!!document.querySelector('.app-title')", 20)
        before = len(rows_of(page))
        page.eval("(() => { const b = [...document.querySelectorAll('.panel .btn.primary')].find(x => x.textContent === 'Start drill'); b.click(); b.click(); })()")
        page.wait(READY, 60)
        # Next tapped twice answers nothing on the new item.
        items, _, records = drill_session(page, inspector, False, 12, phantom=True)
        check(items == 12, f"drill answered {items}")
        check_logged(rows_of(page)[before:], records, "double-tap drill")
        check(not browser.errors, f"doubletap errors: {browser.errors[:5]}")
        print("double-tap ok, phantom taps:", fired)
    finally:
        browser.close()
        srv.shutdown()


def run_twotabs():
    port = 8972
    srv = server.start(port)
    browser = Browser("twotabs")
    try:
        first = browser.page
        boot_app(first, port)
        second = browser.chrome.new_page()
        boot_app(second, port)
        # A hidden tab's clock stands still, so keep both tabs counting as visible.
        first.send("Emulation.setFocusEmulationEnabled", {"enabled": True})
        second.send("Emulation.setFocusEmulationEnabled", {"enabled": True})
        print("visibility:", first.eval("document.visibilityState"), second.eval("document.visibilityState"))
        inspector_a = Inspector(first, (390, 844), "light", False)
        inspector_b = Inspector(second, (390, 844), "light", False)
        seeds = []

        def play(page, inspector, count):
            page.send("Page.bringToFront")
            start_drill(page)
            _, _, records = drill_session(page, inspector, False, count)
            seeds.extend(r["seed"] for r in records)
            end_drill_summary(page, inspector)
            page.eval("location.hash = '#home'")
            page.wait("!!document.querySelector('.app-title')", 20)

        play(first, inspector_a, 2)
        play(second, inspector_b, 2)
        play(first, inspector_a, 2)
        logged = [r[4] for r in rows_of(first)]
        check(sorted(logged) == sorted(seeds), f"tabs lost attempts: logged {len(logged)}, answered {len(seeds)}")
        # An import in a tab that hasn't seen the other tab's work keeps that work.
        extra = OUT / "twotabs.tsv"
        extra.write_text(f"{int(time.time() * 1000)}\tdrill\t\tprice\t77777\t1\t1\t0\t\t1200\t25\n")
        first.send("Page.bringToFront")
        first.eval("location.hash = '#backup'")
        first.wait("!!document.querySelector('input[type=file]')", 20)
        # The other tab answers after Backup is open, so only the import can see it.
        play(second, inspector_b, 1)
        first.send("Page.bringToFront")
        message = import_file(first, extra)
        check("Imported 1 new" in message, f"two-tab import: {message}")
        logged = [r[4] for r in rows_of(second)]
        check(sorted(logged) == sorted(seeds + ["77777"]), f"import lost another tab's attempts: {len(logged)} vs {len(seeds) + 1}")
        check(not browser.errors, f"two-tab errors: {browser.errors[:5]}")
        results["twotabs_attempts"] = len(logged)
        print("two tabs ok,", len(logged), "attempts")
    finally:
        browser.close()
        srv.shutdown()


def run_dealing():
    port = 8973
    srv = server.start(port)
    browser = Browser("dealing")
    try:
        page = browser.page
        page.send("Page.addScriptToEvaluateOnNewDocument", {"source": HOLD_WORKER})
        boot_app(page, port)
        # A held item shows Dealing with a way out, and the end screen stays put.
        page.eval("[...document.querySelectorAll('.panel .btn.primary')].find(b => b.textContent === 'Start drill').click()")
        page.wait(DEALING, 10)
        check(page.eval("document.querySelector('.top-bar button.back').textContent") == "End", "Dealing has no End control")
        page.eval("document.querySelector('.top-bar button.back').click()")
        page.wait("!!document.querySelector('.app-title')", 10)
        page.eval("__releasePrep()")
        time.sleep(0.5)
        check(page.eval("!!document.querySelector('.app-title')"), "home was replaced after ending while dealing")
        check(not page.eval(DEALING), "Dealing came back after the drill ended")
        # The same for the baseline.
        page.eval("window.__holdPrep = true; location.hash = '#baseline'")
        page.wait("[...document.querySelectorAll('.btn.primary')].some(b => b.textContent === 'Begin')", 20)
        page.eval("[...document.querySelectorAll('.btn.primary')].find(b => b.textContent === 'Begin').click()")
        page.wait(DEALING, 10)
        check(page.eval("document.querySelector('.top-bar button.back').textContent") == "Stop", "Dealing has no Stop control")
        page.eval("document.querySelector('.top-bar button.back').click()")
        page.wait("document.body.innerText.includes('nothing to score')", 10)
        page.eval("__releasePrep()")
        time.sleep(0.5)
        check(page.eval("document.body.innerText.includes('nothing to score')"), "the end screen was replaced")
        check(not browser.errors, f"dealing errors: {browser.errors[:5]}")
    finally:
        browser.close()
        srv.shutdown()
    race_port = 8977
    srv = server.start(race_port)
    browser = Browser("dealrace")
    try:
        page = browser.page
        # Enough prepares pass for the first screens; the following item is held.
        page.send("Page.addScriptToEvaluateOnNewDocument", {"source": HOLD_WORKER.replace("__passPrep = 0", "__passPrep = 1")})
        boot_app(page, race_port)
        inspector = Inspector(page, (390, 844), "light", False)
        page.eval("[...document.querySelectorAll('.panel .btn.primary')].find(b => b.textContent === 'Start drill').click()")
        page.wait(READY, 30)
        # End the drill inside the 150 ms before Dealing would appear.
        current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
        kind, answer = pick_answer(page, current, True)
        type_answer(page, current, kind, answer)
        page.wait("!!document.querySelector('.reveal')", 30)
        waited = page.eval(END_WITHIN_WINDOW)
        check(waited < 150, f"End came {waited:.0f} ms after Next, outside the window")
        page.wait("!!document.querySelector('.big-score')", 10)
        time.sleep(0.4)
        check(page.eval("!!document.querySelector('.big-score')"), "Dealing replaced the end screen")
        check(not page.eval(DEALING), "Dealing appeared after the drill ended")
        page.eval("__releasePrep()")
        # The same race in the baseline: Stop right after answering.
        page.eval("location.hash = '#home'")
        page.wait("!!document.querySelector('.app-title')", 10)
        page.eval("window.__holdPrep = true; window.__passPrep = 2; location.hash = '#baseline'")
        page.wait("[...document.querySelectorAll('.btn.primary')].some(b => b.textContent === 'Begin')", 20)
        page.eval("[...document.querySelectorAll('.btn.primary')].find(b => b.textContent === 'Begin').click()")
        page.wait(READY, 30)
        current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
        answer_baseline_item(page, current)
        page.wait(READY + " && __trainer.current.baselineIndex === 1", 30)
        current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
        answer_baseline_item(page, current)
        time.sleep(0.06)
        page.eval("document.querySelector('.top-bar button.back').click()")
        page.wait("!!document.querySelector('.big-score')", 10)
        time.sleep(0.4)
        check(page.eval("!!document.querySelector('.big-score')"), "Dealing replaced the baseline result")
        check(not page.eval(DEALING), "Dealing appeared after the baseline stopped")
        check(not browser.errors, f"dealing race errors: {browser.errors[:5]}")
        print("dealing ok")
    finally:
        browser.close()
        srv.shutdown()


def run_prep_failure():
    port = 8974
    srv = server.start(port)
    browser = Browser("prepfail")
    try:
        page = browser.page
        page.send("Page.addScriptToEvaluateOnNewDocument", {"source": HOLD_WORKER.replace("__holdPrep = true", "__holdPrep = false").replace("__failPrep = false", "__failPrep = true")})
        boot_app(page, port)
        # A failed item must be prepared afresh when the same item is asked for again.
        outcome = page.eval("""(async () => {
          const item = { skill: 'price', seed: '4242' };
          let first = 'resolved';
          try { await __trainer.prepare(item, true); } catch (err) { first = 'failed'; }
          window.__failPrep = false;
          let second = 'failed';
          try { second = (await __trainer.prepare(item, true)).skill; } catch (err) { second = 'failed'; }
          window.__failPrep = true;
          return [first, second];
        })()""")
        check(outcome == ["failed", "price"], f"same item after a failure: {outcome}")
        page.eval("[...document.querySelectorAll('.panel .btn.primary')].find(b => b.textContent === 'Start drill').click()")
        page.wait("document.body.innerText.includes('could not be prepared')", 15)
        check(page.eval("!document.getElementById('notice').hidden && document.getElementById('notice').textContent.includes('simulated trap')"), "no fatal notice")
        check(page.eval("!!document.querySelector('#notice button')"), "no reload button in the notice")
        check(page.eval("[...document.querySelectorAll('.panel .btn')].some(b => b.textContent === 'Reload')"), "no reload control")
        # End gets out, and once prep works again a new drill deals normally.
        page.eval("document.querySelector('.top-bar button.back').click()")
        page.wait("!!document.querySelector('.app-title')", 15)
        page.eval("window.__failPrep = false")
        page.eval("location.hash = '#progress'")
        page.wait("!!document.querySelector('.skill-row')", 10)
        page.eval("location.hash = '#home'")
        page.wait("!!document.querySelector('.app-title')", 10)
        start_drill(page)
        items, *_ = drill_session(page, Inspector(page, (390, 844), "light", False), False, 2)
        check(items == 2, "drill did not recover after a prep failure")
        # The same failure in the baseline.
        page.eval("window.__failPrep = true")
        end_drill_summary(page, Inspector(page, (390, 844), "light", False))
        page.eval("location.hash = '#home'")
        page.wait("!!document.querySelector('.app-title')", 10)
        page.eval("location.hash = '#baseline'")
        page.wait("[...document.querySelectorAll('.btn.primary')].some(b => b.textContent === 'Begin')", 20)
        page.eval("[...document.querySelectorAll('.btn.primary')].find(b => b.textContent === 'Begin').click()")
        page.wait("document.body.innerText.includes('could not be prepared')", 15)
        check(page.eval("document.querySelector('.top-bar button.back').textContent") == "Stop", "baseline failure has no Stop")
        uncaught = [e for e in browser.errors if "uncaught" in e]
        check(not uncaught, f"prep failure raised uncaught errors: {uncaught[:3]}")
    finally:
        browser.close()
        srv.shutdown()
    crash_port = 8979
    srv = server.start(crash_port)
    browser = Browser("prepcrash")
    try:
        page = browser.page
        page.send("Page.addScriptToEvaluateOnNewDocument", {"source": CRASHABLE_PREP + HOLD_WORKER})
        boot_app(page, crash_port)
        # The item is held, so it is still pending when the worker really crashes.
        page.eval("[...document.querySelectorAll('.panel .btn.primary')].find(b => b.textContent === 'Start drill').click()")
        page.wait(DEALING, 10)
        check(page.eval("window.__workers.length") == 2, "expected the engine and prep workers")
        page.eval("window.__workers[1].postMessage('crash'); 1")
        page.wait("document.body.innerText.includes('could not be prepared')", 15)
        check(page.eval("window.__workers.length") == 3, "the prep worker was not replaced")
        print("prep failure ok")
    finally:
        browser.close()
        srv.shutdown()


def run_quota():
    port = 8980
    srv = server.start(port)
    browser = Browser("quota")
    try:
        page = browser.page
        boot_app(page, port)
        inspector = Inspector(page, (390, 844), "light", False)
        start_drill(page)
        _, _, before = drill_session(page, inspector, False, 2)
        page.eval("Storage.prototype.setItem = function () { throw new DOMException('full', 'QuotaExceededError'); }; 1")
        _, _, after = drill_session(page, inspector, False, 3)
        end_drill_summary(page, inspector)
        score = page.eval("document.querySelector('.big-score').textContent")
        check(score.endswith("of 5 right"), f"summary says: {score}")
        check(page.eval("!document.getElementById('notice').hidden"), "no notice after storage filled up")
        page.eval("location.hash = '#home'")
        page.wait("!!document.querySelector('.app-title')", 20)
        page.eval("location.hash = '#backup'")
        page.wait("!!document.querySelector('input[type=file]')", 20)
        exported = page.eval("__trainer.engine.call('export').then(r => r.text)")
        items = [l.split("\t")[4] for l in exported.split("\n") if l and not l.startswith("timestamp")]
        expected = [r["seed"] for r in before + after]
        check(items == expected, f"export holds {len(items)} attempts, answered {len(expected)}")
        results["quota_exported"] = len(items)
        check(not browser.errors, f"quota errors: {browser.errors[:5]}")
        print("quota ok,", len(items), "attempts exported")
    finally:
        browser.close()
        srv.shutdown()


HIDDEN_WITHOUT_VISIBLE = """(() => {
  Object.defineProperty(document, 'hidden', { configurable: true, value: true });
  document.dispatchEvent(new Event('visibilitychange'));
  Object.defineProperty(document, 'hidden', { configurable: true, value: false });
  return 1;
})()"""


def run_hidden_event():
    port = 8981
    srv = server.start(port)
    browser = Browser("hidden")
    try:
        page = browser.page
        boot_app(page, port)
        inspector = Inspector(page, (390, 844), "light", False)
        start_drill(page)
        page.eval(HIDDEN_WITHOUT_VISIBLE)
        items, *_ = drill_session(page, inspector, False, 3)
        check(items == 3, f"only {items} drill answers registered after a hidden event")
        check(len(rows_of(page)) == 3, f"drill logged {len(rows_of(page))} of 3 answers")
        page.wait("!!document.querySelector('.top-bar button.back')", 20)
        page.eval("document.querySelector('.top-bar button.back').click()")
        page.wait("!!document.querySelector('.big-score')", 20)
        page.eval("location.hash = '#baseline'")
        page.wait("[...document.querySelectorAll('.btn.primary')].some(b => b.textContent === 'Begin')", 20)
        page.eval("[...document.querySelectorAll('.btn.primary')].find(b => b.textContent === 'Begin').click()")
        for i in range(3):
            page.wait(READY + f" && __trainer.current.baselineIndex === {i}", 30)
            answer_baseline_item(page, json.loads(page.eval("JSON.stringify(__trainer.current)")))
        page.wait(READY + " && __trainer.current.baselineIndex === 3", 30)
        check(len(rows_of(page)) == 6, f"baseline answers did not register: {len(rows_of(page))} lines")
        check(not browser.errors, f"hidden event errors: {browser.errors[:5]}")
        print("hidden event ok")
    finally:
        browser.close()
        srv.shutdown()


def run_export():
    port = 8975
    srv = server.start(port)
    browser = Browser("export")
    try:
        page = browser.page
        boot_app(page, port)
        page.eval("location.hash = '#backup'")
        page.wait("!!document.querySelector('input[type=file]')", 20)
        press = "[...document.querySelectorAll('.btn.primary')].find(b => b.textContent === 'Export attempt log').click()"
        status = "document.querySelector('p[role=status]').textContent"
        page.eval("navigator.canShare = () => true; navigator.share = async d => { window.__shared = d.files[0].name; }")
        page.eval(press)
        page.wait(f"{status} === 'Exported.'", 5)
        check(page.eval("window.__shared") == "poker-trainer-attempts.tsv", "share got the wrong file")
        page.eval(f"{status.replace('.textContent', '')}.textContent = ''")
        page.eval("navigator.share = async () => { throw new DOMException('no', 'AbortError'); }")
        page.eval(press)
        page.wait(f"{status} === 'Export cancelled.'", 5)
        page.eval("window.__clicked = 0; HTMLAnchorElement.prototype.click = function () { window.__clicked++; }; navigator.share = async () => { throw new DOMException('no', 'NotAllowedError'); }")
        page.eval(press)
        page.wait(f"{status} === 'Download started.'", 5)
        check(page.eval("window.__clicked") == 1, "share failure did not fall back to a download")
        page.eval("navigator.canShare = () => false")
        page.eval(press)
        page.wait("window.__clicked === 2", 5)
        check(page.eval(status) == "Download started.", "plain download status wrong")
        check(not browser.errors, f"export errors: {browser.errors[:5]}")
        print("export ok")
    finally:
        browser.close()
        srv.shutdown()


def run_sw_scope():
    port = 8976
    srv = server.start(port)
    browser = Browser("swscope")
    try:
        page = browser.page
        page.viewport(390, 844, True)
        # Another project on the same origin already has a cache.
        page.navigate(f"http://127.0.0.1:{port}/app/icon.svg")
        page.wait("document.readyState === 'complete'", 10)
        page.eval("caches.open('other-project-v1').then(() => 1)")
        page.navigate(f"http://127.0.0.1:{port}/")
        page.wait("!!document.querySelector('.app-title') && !!window.__trainer", 30)
        page.eval("window.__stayed = true; 1")
        page.wait("navigator.serviceWorker.ready.then(() => true)", 30)
        page.wait("!!navigator.serviceWorker.controller", 30)
        time.sleep(1.0)
        check(page.eval("window.__stayed === true"), "a fresh install reloaded the page")
        names = page.eval("caches.keys()")
        check("other-project-v1" in names, f"another project's cache was deleted: {names}")
        # An old link lands on the app even for a visitor with no service worker.
        fresh = Browser("swscope2", port=DEBUG_PORT + 1)
        try:
            other = fresh.page
            other.navigate(f"http://127.0.0.1:{port}/trainer.html")
            other.wait("document.title === 'Poker Trainer' && !!window.__trainer", 30)
            check(other.eval("location.pathname") == "/", "trainer.html did not redirect to the app")
        finally:
            fresh.close()
        check(not browser.errors, f"sw scope errors: {browser.errors[:5]}")
        print("service worker scope ok")
    finally:
        browser.close()
        srv.shutdown()

SECTIONS = {
    "native": run_native, "wasmclean": run_wasm_clean, "doubletap": run_doubletap, "twotabs": run_twotabs,
    "dealing": run_dealing, "prepfail": run_prep_failure, "export": run_export, "swscope": run_sw_scope,
    "quota": run_quota, "hiddenevent": run_hidden_event,
    "differential": run_differential, "timing": run_timing, "e2e": run_e2e, "baseline": run_baseline,
    "card": run_card, "offline": run_offline, "nostorage": run_nostorage, "legacy": run_legacy, "screens": run_screens,
}

if __name__ == "__main__":
    OUT.mkdir(parents=True, exist_ok=True)
    os.chdir(ROOT)
    wanted = sys.argv[1:] or list(SECTIONS)
    started = time.time()
    for name in wanted:
        print(f"== {name}", flush=True)
        try:
            SECTIONS[name]()
        except Exception as err:
            fail(f"{name} raised {type(err).__name__}: {err}")
            print("ERRORS SEEN:", LAST_ERRORS[0])
            print("PAGE:", LAST_ERRORS[-1])
            import traceback
            traceback.print_exc()
    results["failures"] = failures
    results["seconds"] = round(time.time() - started)
    with open(OUT / "results.json", "w") as f:
        json.dump(results, f, indent=1)
    print("FAILED" if failures else "ALL PASSED", len(failures), "failures in", results["seconds"], "s")
    sys.exit(1 if failures else 0)
