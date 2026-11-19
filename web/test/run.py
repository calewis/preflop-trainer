"""Full browser test suite: python3 web/test/run.py [sections...]
Sections: river libriver limpers liblimpers samebuild phonefit hiddendrills native libgroups groups libresearch libreadprice readprice libcli research researchshots (on request only) libtable tablespots midgrade wasmclean doubletap twotabs dealing prepfail export swscope quota hiddenevent differential timing e2e baseline newskills noneleft checkup checkupquit checkuptabs checkupconflict progressnew card preflopspots libpreflop offline nostorage legacy screens
Results go to target/webtest/results.json and screenshots to target/webtest/screens/."""
import itertools, json, os, pathlib, random, re, shutil, subprocess, sys, time

sys.path.insert(0, str(pathlib.Path(__file__).parent))
import cdp, server

ROOT = pathlib.Path(__file__).resolve().parents[2]
OUT = ROOT / "target/webtest"
SHOTS = OUT / "screens"
# Lets a second run share the machine: every browser and server port moves by this much.
PORT_SHIFT = int(os.environ.get("TRAINER_TEST_PORT_SHIFT", "0"))
DEBUG_PORT = 9341 + PORT_SHIFT
results = {}
failures = []


def fail(message):
    failures.append(message)
    print("FAIL:", message)


class SectionStopped(Exception):
    """A must_see failed: already reported, and the rest of the section can't run."""


def must_see(page, expression, timeout, message):
    """A wait the app has to satisfy, so timing out is the app's failure, not the harness's."""
    try:
        return page.wait(expression, timeout)
    except TimeoutError:
        fail(message)
        raise SectionStopped(message)


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
        self.proc = subprocess.Popen(["google-chrome", "--headless=new", "--no-sandbox", "--password-store=basic",
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


def unescape_command(text):
    """Undoes diff_native's escaping of one command."""
    codes = {"\\\\": "\\", "\\x1f": "\x1f", "\\t": "\t", "\\n": "\n", "\\r": "\r"}
    return re.sub(r"\\\\|\\x1f|\\t|\\n|\\r", lambda m: codes[m.group()], text)


def check_differential_coverage(commands, replies):
    """The compared set must keep exercising the commands the feature added."""
    fields = [c.split("\x1f") for c in commands]
    def count(kind, test=lambda f: True):
        return sum(1 for f in fields if f[0] == kind and test(f))
    check(count("prepare", lambda f: f[1] == "read_price") >= 50, "differential: read_price prepares missing")
    check(count("prepare", lambda f: f[1] == "limpers") >= 50, "differential: limpers prepares missing")
    check(count("grade", lambda f: f[1] == "limpers") >= 50, "differential: limpers grades missing")
    for rung in range(5):
        check(sum(f'"kind":"read_price","rung":{rung}' in r for r in replies) >= 5, f"differential: read_price rung {rung} never compared")
    check(count("check_grade", lambda f: f[1] == "read_price") >= 5, "differential: no read_price checkup answers")
    check(count("reflect_commit") >= 5, "differential: no reflect_commit")
    check(count("commit", lambda f: len(f) == 4) >= 5, "differential: no commit without a confidence")
    for code in ("high", "medium", "bogus", ""):
        check(count("commit", lambda f: len(f) == 5 and f[4] == code) >= 2, f"differential: no commit with confidence {code!r}")
    for origin in ("new", "due", "retry"):
        check(count("prepare", lambda f: len(f) == 4 and f[3] == origin) >= 5, f"differential: no prepare with origin {origin}")
    check(count("prepare", lambda f: len(f) == 3) >= 100, "differential: no prepare without an origin")
    for stage in ("hand", "outs", "price", "decide"):
        check(sum(f'"stage":"{stage}"' in r for r in replies) >= 3, f"differential: chain stage {stage} never compared")
    nexts = [r for f, r in zip(fields, replies) if f[0] == "session_next"]
    check(sum('"origin":"retry"' in r for r in nexts) >= 5, "differential: no family retry dealt")
    check(sum('"origin":"due"' in r for r in nexts) >= 1, "differential: no later-day family review dealt")
    lines = [r for f, r in zip(fields, replies) if f[0] in ("commit", "reflect_commit")]
    check(any("\\treflect\\t" in r for r in lines), "differential: no reflection row logged")
    check(any("\\tdrill-retry\\t" in r for r in lines), "differential: no retry row logged")
    prepared = [r for f, r in zip(fields, replies) if f[0] in ("prepare", "session_next") and '"kind":"preflop"' in r]
    for situation in ("folded", "limpers", "raised", "callers", "three_bet", "four_bet"):
        check(sum(f'"situation":"{situation}"' in r for r in prepared) >= 5, f"differential: preflop situation {situation} never compared")
    check(count("card_view") >= 42, "differential: not every card view is compared")
    views = [r for f, r in zip(fields, replies) if f[0] == "card_view"]
    for word in ("All in", "4-bet", "3-bet"):
        check(sum(word in r for r in views) >= 1, f"differential: card legend {word} never compared")


def run_differential():
    srv = server.start(8951 + PORT_SHIFT)
    browser = Browser("diff")
    try:
        page = browser.page
        boot_app(page, 8951 + PORT_SHIFT)
        # Always the bridge as it stands now: a stale binary would compare old output.
        subprocess.run(["cargo", "build", "--manifest-path", "web/Cargo.toml", "--release", "--bin", "diff_native"],
                       cwd=ROOT, check=True, capture_output=True)
        binary = pathlib.Path(os.environ.get("CARGO_TARGET_DIR") or ROOT / "web/target") / "release/diff_native"
        native = subprocess.run([str(binary), "60"],
                                capture_output=True, text=True, check=True).stdout.split("\n")[:-1]
        pairs = [line.split("\t", 1) for line in native]
        commands = [unescape_command(c) for c, _ in pairs]
        expected = [r for _, r in pairs]
        check_differential_coverage(commands, expected)
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


# Every rung, by the seed bits that carry it.
BEATS_ME_TIMING = """
(async (perRung) => {
  const worker = new Worker('app/worker.js');
  const ask = (args) => new Promise(resolve => { worker.onmessage = e => resolve(e.data); worker.postMessage({id: 1, args}); });
  let total = 0, max = 0, worst = '', count = 0;
  for (let rung = 0n; rung < 4n; rung++) {
    for (let i = 0; i < perRung; i++) {
      const seed = String((rung << 60n) + BigInt(i) * 2654435761n + 99n);
      const reply = await ask(['prepare', 'beats_me', seed]);
      if (reply.fatal) throw new Error(reply.fatal);
      total += reply.ms; count += 1;
      if (reply.ms > max) { max = reply.ms; worst = seed; }
    }
  }
  return { beats_me: { mean: total / count, max, worst, count } };
})
"""


TABLE_TIMING = """
(async (count) => {
  const worker = new Worker('app/worker.js');
  const ask = (args) => new Promise(resolve => { worker.onmessage = e => resolve(e.data); worker.postMessage({id: 1, args}); });
  const first = await ask(['prepare', 'table', '99']);
  if (first.fatal) throw new Error(first.fatal);
  let total = 0, max = 0, worst = '';
  for (let i = 0; i < count; i++) {
    const seed = String(BigInt(i) * 2654435761n + 99n);
    const reply = await ask(['prepare', 'table', seed]);
    if (reply.fatal) throw new Error(reply.fatal);
    total += reply.ms;
    if (reply.ms > max) { max = reply.ms; worst = seed; }
  }
  return { first_ms: first.ms, items: { mean: total / count, max, worst, count } };
})
"""


def run_timing():
    srv = server.start(8952 + PORT_SHIFT)
    browser = Browser("timing")
    try:
        page = browser.page
        boot_app(page, 8952 + PORT_SHIFT)
        skills = ["preflop", "limpers", "nuts", "possible", "made", "beats", "outs", "price", "hit", "board_type", "anchor", "read_price"]
        fast = page.eval(f"({TIMING})({json.dumps(skills)}, 200)")
        slow = page.eval(f"({TIMING})(['spot'], 200)")
        mw = page.eval(f"({TIMING})(['spot_mw'], 150)")
        equity = page.eval(f"({TIMING})(['equity'], 200)")
        chain = page.eval(f"({TIMING})(['chain'], 200)")
        beats_me = page.eval(f"({BEATS_ME_TIMING})(100)")
        table_worker = page.eval(f"({TABLE_TIMING})(200)")
        results["timing_ms"] = {**fast, **slow, **mw, **equity, **chain, **beats_me, "table": table_worker["items"]}
        results["table_first_use_ms"] = table_worker["first_ms"]
        print(f"table first use (parse + item) {table_worker['first_ms']:.1f} ms")
        check(table_worker["first_ms"] <= 1500, f"table first-use parse took {table_worker['first_ms']:.0f} ms")
        for k, v in results["timing_ms"].items():
            print(f"timing {k:9} mean {v['mean']:8.1f} ms  max {v['max']:8.1f} ms  worst seed {v['worst']}")
        # Measured with another CPU job running: spot 43 mean, 353 max; chain 5 mean, 70 max.
        budgets = {"spot": (100, 1500), "spot_mw": (2000, 8000), "equity": (60, 600), "chain": (20, 300)}
        expected = {*skills, "spot", "spot_mw", "equity", "chain", "table", "beats_me"}
        check(set(results["timing_ms"]) == expected, f"timing covered {sorted(results['timing_ms'])}")
        for skill, v in results["timing_ms"].items():
            mean_limit, max_limit = budgets.get(skill, (20, 100))
            check(v["count"] >= 100, f"timing {skill}: only {v['count']} samples")
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

INTERNAL = re.compile(r"\b(open\.\w+|iso\.\w+|face_raise|face_callers|three_bet\.\w*|four_bet\.\w*|note\.\w+|small_blind|big_blind|spot_mw|board_type|out_of_items|2p|4k|oesd)\b")


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

    def snapshot(self, name, full=False):
        self.text_check(name)
        if self.viewport[0] == 390:
            info = self.page.eval(SHOT_JS)
            check(info["overflow"] <= 0, f"horizontal overflow {info['overflow']}px at {name}")
            check(not info["bad"], f"tap targets too small at {name}: {info['bad'][:6]}")
        if self.capture and name not in self.taken:
            self.taken.add(name)
            mode = f"{self.viewport[0]}-{self.theme}"
            SHOTS.mkdir(parents=True, exist_ok=True)
            self.page.screenshot(str(SHOTS / f"{mode}-{name}.png"), full=full)


# --- answer driving --------------------------------------------------------------
CLICK_TEXT = """((sel, text) => { const el = [...document.querySelectorAll(sel)].find(e => e.textContent.trim() === text); if (!el) return false; el.click(); return true; })"""


def click_text(page, selector, text):
    check(page.eval(f"({CLICK_TEXT})({json.dumps(selector)}, {json.dumps(text)})"), f"no element {selector} '{text}'")


def click(page, selector):
    check(page.eval(f"(s => {{ const e = document.querySelector(s); if (!e) return false; e.click(); return true; }})({json.dumps(selector)})"),
          f"no element {selector}")


def grade_probe(page, skill, seed, answer, response=0):
    return page.eval(f"__trainer.engine.call('grade', {json.dumps(skill)}, {json.dumps(seed)}, {json.dumps(answer)}, {response})")


def check_probe(page, current, answer, response=0):
    args = [current["skill"], current["form"], current["checkSlot"], current["run"], answer, response]
    return page.eval(f"__trainer.engine.call('check_grade', {', '.join(json.dumps(a) for a in args)})")


def pick_answer(page, current, want_ok, probe=None):
    """Returns (kind, answer) with the wanted result, or (kind, None) if impossible.
    `probe` grades a trial answer; a drill item's grade by default."""
    bundle = current["bundle"]
    kind = bundle["format"]["type"]
    skill = bundle["skill"]
    seed = bundle["seed"]
    if probe is None:
        probe = lambda answer: grade_probe(page, skill, seed, answer)
    if kind == "choice":
        if not bundle["graded"]:  # board_type: graded when answered, with its time
            for option in bundle["format"]["options"]:
                outcome = probe(option["code"])["outcome"]
                if outcome["ok"] == want_ok:
                    return kind, option["code"]
            return kind, bundle["format"]["options"][0]["code"]
        for code, outcome in bundle["graded"].items():
            if outcome["ok"] == want_ok:
                return kind, code
        return kind, next(iter(bundle["graded"]))
    if kind == "holding":
        trial = probe("AKs")
        for guess in ("AKs", "QJ", "22"):
            trial = probe(guess)
            if "outcome" in trial:
                break
        top = trial["outcome"]["reveal"]["holdings"]["shown"][0]
        if want_ok:
            return kind, top[0] + top[1]
        for guess in ("72o", "32", "54s", "QJ", "TT", "88", "33", "22"):
            result = probe(guess)
            if "outcome" in result and not result["outcome"]["ok"]:
                return kind, guess
        return kind, top[0] + top[1]
    if kind == "multi":
        codes = [o["code"] for o in bundle["format"]["options"]]
        subsets = ["none"] + ["".join(c) for r in range(1, len(codes) + 1) for c in itertools.combinations(codes, r)]
        for code in subsets:
            result = probe(code)
            if result["outcome"]["correct"] == want_ok:
                return kind, code
        return kind, "none"
    if kind == "integer":
        for n in range(0, 48):
            result = probe(str(n))
            if result["outcome"]["correct"] == want_ok:
                return kind, str(n)
        return kind, "0"
    if kind == "percent":
        reveal = probe("50")["outcome"]["reveal"]
        if reveal["kind"] in ("equity", "anchor"):
            truth = float(reveal["answer"].rstrip("%"))
            if want_ok:
                return kind, str(int(round(truth)))
            return kind, str(int(round(truth + 45 if truth <= 50 else truth - 45)))
        for n in range(1, 100):
            result = probe(str(n))
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
            codes = [o["code"] for o in current["bundle"]["format"]["options"]]
            for ch in answer:
                page.eval(f"document.querySelectorAll('.toggle')[{codes.index(ch)}].click()")
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
        if want["skill"] == "equity":
            is_choice = want["answer"] in ("l", "m", "h")
            check(len(row) == len(RESEARCH_LOG_COLUMNS.split("\t"))
                  and (not row[14] and not row[15] if is_choice else bool(row[14] and row[15]))
                  and row[16] and row[17]
                  and not row[18] and not row[19] and not row[21] and not row[22]
                  and re.fullmatch(r"\d+:\d+:(0-25|25-50|50-75|75-100)", row[20]), f"{label}: equity columns {row[11:]}")
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
            fail(f"answer did not register: {skill} {kind} {answer!r}")
            raise SectionStopped()
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
        if all({"right", "wrong"} <= seen.get(s, set()) for s in ALL_SKILLS):
            break
    return items, seen, records


# Every skill the default drill deals except `table`, which comes too rarely to
# promise both a right and a wrong answer. `price` and `beats` are no longer dealt.
# `spot` and `spot_mw` are hidden for now (HIDDEN_SKILLS in src/groups.rs), so never dealt.
ALL_SKILLS = ["preflop", "limpers", "nuts", "possible", "made", "beats_me", "read_price", "outs", "hit",
              "board_type", "equity", "anchor", "chain"]
# The monthly checkup without the hidden drills' four items.
MIXED_LEN = 36
RETIRED_SKILLS = ["price", "beats"]
LOG_COLUMNS = "timestamp_ms\tmode\tbaseline_run\tskill\titem\tgen_version\tcorrect\tclose\tev_loss_pct\tresponse_ms\tanswer"
PASSING_SKILLS = [("nuts", "AKs"), ("possible", "f"), ("made", "top"), ("beats_me", "a"), ("board_type", "td"),
                  ("outs", "8"), ("price", "25"), ("hit", "20"), ("equity", "50"), ("anchor", "33")]


# Only rungs 2-3 count toward the beats_me mark; a seed carries its rung in bits 60-61.
BEATS_ME_RUNG_3_BASE = 3 << 60


def drill_seed(skill, i):
    return BEATS_ME_RUNG_3_BASE + 1000 + i if skill == "beats_me" else 1000 + i


def synthetic_drills(skills=PASSING_SKILLS, signed_error=1.0, now=None):
    """45 correct, fast drill attempts per skill. Equity rows carry their extra columns."""
    t = now if now is not None else int(time.time() * 1000) - 86400000 * 5
    lines = []
    for skill, answer in skills:
        for i in range(45):
            t += 1000
            extra = f"\t\t\t\t8\t{signed_error:.4f}\t50.0000\t1" if skill == "equity" else ""
            lines.append(f"{t}\tdrill\t\t{skill}\t{drill_seed(skill, i)}\t1\t1\t0\t\t1500\t{answer}{extra}")
    return lines, t


def synthetic_checkups(skills, start, first_run=1, correct=1):
    """One complete, passing checkup per skill."""
    t = start
    lines = []
    for run, (skill, answer) in enumerate(skills, first_run):
        for slot in range(50):
            t += 1000
            extra = "\t5\t1.0000\t50.0000\t1" if skill == "equity" else "\t\t\t\t"
            lines.append(f"{t}\tcheck\t\t{skill}\t{2000 + slot}\t1\t{correct}\t0\t\t1500\t{answer}\t{run}\t0\t{slot}{extra}")
    return lines, t


def synthetic_log():
    """Drill marks for ten skills and a passing old-style checkup for each: a long history."""
    drills, end = synthetic_drills()
    checks, _ = synthetic_checkups(PASSING_SKILLS, end)
    return "\n".join([LOG_COLUMNS] + drills + checks) + "\n"


def drills_only_log(skills, signed_error=1.0, now=None):
    drills, _ = synthetic_drills(skills, signed_error, now)
    return "\n".join([LOG_COLUMNS] + drills) + "\n"


LOG_KEY = "poker-trainer.attempts.v1"


def seed_step3(page, port):
    seed_log(page, port, synthetic_log())


def seed_log(page, port, text):
    page.navigate(f"http://127.0.0.1:{port}/")
    page.wait("!!document.querySelector('.app-title') && !!window.__trainer", 30)
    page.eval(f"localStorage.setItem({json.dumps(LOG_KEY)}, {json.dumps(text)})")
    page.send("Page.reload", {})
    page.wait("!!document.querySelector('.app-title') && !!window.__trainer", 30)
    time.sleep(0.2)


def open_limited_drill(page, skill, wait_ready=True):
    """Starts a drill dealing only `skill`, the way the library's skill filter does."""
    page.wait("!!document.querySelector('.app-title')", 20)
    page.eval(f"__trainer.engine.call('session_open', '1234567', Date.now(), {json.dumps(skill)})")
    page.wait("[...document.querySelectorAll('.panel .btn.primary')].some(b => b.textContent === 'Start drill')", 10)
    page.eval("[...document.querySelectorAll('.panel .btn.primary')].find(b => b.textContent === 'Start drill').click()")
    if wait_ready:
        page.wait(READY, 60)


def wait_rows(page, count):
    page.wait(f"(localStorage.getItem({json.dumps(LOG_KEY)}) || '').split('\\n').filter(l => l && !l.startsWith('timestamp')).length >= {count}", 15)


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
    srv = server.start(8953 + PORT_SHIFT)
    browser = Browser("e2e")
    try:
        page = browser.page
        page.viewport(390, 844, True)
        page.theme("light")
        inspector = Inspector(page, (390, 844), "light", False)
        seed_step3(page, 8953 + PORT_SHIFT)
        inspector.snapshot("home")
        home_text = page.eval("document.body.innerText")
        check("Practise one group" in home_text and "Step " not in home_text, f"home after a long history: {home_text[:300]}")
        before = log_lines(page)
        start_drill(page)
        items, seen, records = drill_session(page, inspector, True, 500)
        print("e2e drill answered", items, {k: sorted(v) for k, v in seen.items()})
        missing = [s for s in ALL_SKILLS if not {"right", "wrong"} <= seen.get(s, set())]
        check(not missing, f"skills not covered with right and wrong answers: {missing}")
        retired = [s for s in RETIRED_SKILLS if s in seen]
        check(not retired, f"the default drill dealt retired skills: {retired}")
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
        check("Pass mark" in text and "Every drill is open" in text and not re.search(r"Step \d", text), f"progress text: {text[:200]}")
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


def run_baseline(port=8954 + PORT_SHIFT, inspector_args=None):
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
        check(re.search(r"\d+ of 32", text), "baseline report has no score")
        check("By section" in text and "By part" in text, "baseline report sections missing")
        check("first baseline" in text, "first baseline comparison text missing")
        inspector.snapshot("baseline-report")
        lines = log_lines(page)
        check(lines == 32, f"baseline logged {lines} attempts")
        # A second run compares with the first.
        page.eval("location.hash = '#home'")
        page.wait("!!document.querySelector('.app-title')", 20)
        check(not page.eval("document.body.innerText.includes('Take the baseline first')"), "baseline prompt still shown")
        page.eval("location.hash = '#baseline'")
        page.wait("[...document.querySelectorAll('.btn.primary')].some(b => b.textContent === 'Begin')", 20)
        answers = complete_baseline(page, inspector)
        check_baseline_rows(rows_of(page)[32:], answers, "2")
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
        check(log_lines(page) == 65, f"after two runs and one answer: {log_lines(page)} lines")
        results["baseline_errors"] = list(browser.errors)
        check(not browser.errors, f"baseline browser errors: {browser.errors[:5]}")
    finally:
        browser.close()
        srv.shutdown()


def check_baseline_rows(rows, answers, run):
    check(len(rows) == 32, f"baseline logged {len(rows)} attempts")
    check(len({r[4] for r in rows}) == 32 and not {"b13", "b14", "b15", "b16"} & {r[4] for r in rows} and all(re.fullmatch(r"b\d\d", r[4]) for r in rows), "baseline item ids wrong")
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
    for i in range(32):
        page.wait(READY, 60)
        inspector.snapshot("baseline-item")
        current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
        counter = page.eval("document.querySelector('.top-bar .count').textContent")
        check(counter == f"{i + 1} of 32", f"baseline counter {counter} at {i}")
        answers.append((current["bundle"]["skill"], answer_baseline_item(page, current)))
    page.wait("!!document.querySelector('.big-score')", 60)
    check(page.eval("document.querySelector('.big-score').textContent").endswith("of 32"), "baseline not scored of 32")
    return answers


def answer_percent_keypad(page, text):
    for ch in text:
        click_text(page, ".keypad .key", ch)
    click(page, ".answers .btn.primary")


def answer_drill_item(page, want_ok):
    """Answers the shown drill item and returns (skill, answer, outcome text of the reveal)."""
    current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
    kind, answer = pick_answer(page, current, want_ok)
    type_answer(page, current, kind, answer)
    page.wait("!!document.querySelector('.reveal')", 30)
    return current, kind, answer


PRE_ANSWER = """(() => {
  const copy = document.documentElement.cloneNode(true);
  copy.querySelectorAll('svg, script, style').forEach(e => e.remove());
  return JSON.stringify({ bundle: __trainer.current.bundle, html: copy.outerHTML });
})()"""


def pre_answer_state(page):
    """Everything the page holds before the answer, minus SVG paths full of decimals."""
    return page.eval(PRE_ANSWER)


def reveal_equity(reveal):
    found = re.search(r"Exact equity:\s*([\d.]+)%", reveal)
    check(found, f"no exact equity in the reveal: {reveal}")
    return found.group(1) if found else "none"


def assert_no_leak(pre, number, where):
    if re.search(rf"(?<![\d.]){re.escape(number)}(?!\d)", pre):
        fail(f"exact equity {number} is in the page before answering at {where}")


def check_full_truth_absent(pre, row, where):
    check(len(row) == 24 and row[16] not in pre, f"exact equity {row[16:17]} is in the page before answering at {where}")


def next_drill_item(page):
    click(page, ".action-bar .btn")
    page.wait(READY + " || !!document.querySelector('.big-score')", 60)


def run_new_skills():
    port = 8982 + PORT_SHIFT
    srv = server.start(port)
    browser = Browser("newskills")
    try:
        page = browser.page
        page.viewport(390, 844, True)
        page.theme("light")
        # board_type: a fresh log, so its reveals name the categories.
        page.navigate(f"http://127.0.0.1:{port}/")
        page.wait("!!document.querySelector('.app-title') && !!window.__trainer", 30)
        inspector = Inspector(page, (390, 844), "light", False)
        open_limited_drill(page, "board_type")
        check(page.eval("!!document.querySelector('.target .target-text') && document.querySelector('.target-text').textContent.includes('6 seconds')"), "no 6-second target shown")
        check(page.eval("document.querySelectorAll('.choice').length") == 4, "board_type must offer four choices")
        inspector.snapshot("newskill-board-q")
        boards_right = 0
        for attempt in range(2):
            page.wait(READY, 30)
            current, _, answer = answer_drill_item(page, True)
            check(page.eval("document.querySelector('.verdict').textContent.startsWith('Correct')"), "right board answer not marked correct")
            text = page.eval("document.querySelector('.reveal').innerText")
            check("Answer:" in text and "so it is" in text, f"board reveal lacks the why: {text}")
            boards_right += 1
            next_drill_item(page)
        # Wrong answer.
        page.wait(READY, 30)
        answer_drill_item(page, False)
        check(page.eval("document.querySelector('.reveal').classList.contains('bad')"), "wrong board answer not marked wrong")
        next_drill_item(page)
        # Past 12 seconds a right answer is a miss. Answer the same way the player would, after waiting.
        page.wait(READY, 30)
        current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
        before = len(rows_of(page))
        kind, answer = pick_answer(page, current, True)
        time.sleep(12.3)
        type_answer(page, current, kind, answer)
        page.wait("!!document.querySelector('.reveal')", 30)
        check(page.eval("document.querySelector('.verdict').textContent.startsWith('Too slow')"), "a slow right answer was not a miss")
        check(page.eval("document.querySelector('.reveal').classList.contains('bad')"), "a slow answer was not marked as a miss")
        check("over 12 seconds" in page.eval("document.querySelector('.reveal').innerText"), "the slow reveal doesn't explain the miss")
        wait_rows(page, before + 1)
        row = rows_of(page)[before]
        check(row[6] == "0" and int(row[9]) >= 12000, f"slow board row logged correct={row[6]} ms={row[9]}")
        inspector.snapshot("newskill-board-slow")
        page.eval("location.hash = '#home'")
        page.wait("!!document.querySelector('.app-title')", 20)
        # equity defaults to 3-choice bands at both rungs, with a tight reveal and collapsed details.
        page.eval(f"localStorage.setItem({json.dumps(LOG_KEY)}, ''); localStorage.removeItem('poker-trainer.equity-exact');")
        page.send("Page.reload", {})
        page.wait("!!document.querySelector('.app-title') && !!window.__trainer", 30)
        check(page.eval("document.querySelector('[data-equity-exact]').textContent") == "Type exact equity %: off",
              "equity exact mode toggle must default to off")
        open_limited_drill(page, "equity")
        page.wait(READY, 60)
        check(page.eval("document.querySelector('.question').textContent.includes('Opponent')"), "rung 0 must show the opponent's cards")
        check(page.eval("document.querySelector('.card-opp') !== null"), "rung 0 must show opponent cards as cards")
        choices = page.eval("[...document.querySelectorAll('.choice')].map(b => b.textContent.replace(/^[0-9]/, '').trim())")
        check(choices == ["Low (under 30%)", "Medium (30-60%)", "High (over 60%)"], f"unexpected equity choices: {choices}")
        check(page.eval("!document.querySelector('.keypad')"), "default equity must not show the keypad")
        inspector.snapshot("newskill-equity-q")
        # Nothing the page holds may leak the exact equity or graded answers before committing.
        pre = pre_answer_state(page)
        check("\"answer\"" not in pre and "truth" not in pre, f"exact equity is in the page before answering: {pre[:300]}")
        visible = page.eval("document.body.innerText")
        before = len(rows_of(page))
        current, kind, answer = answer_drill_item(page, True)
        wait_rows(page, before + 1)
        short = page.eval("(() => { const c = document.querySelector('.reveal').cloneNode(true); c.querySelectorAll('details').forEach(d => d.remove()); return c.innerText; })()")
        check(re.search(r"Answer:\s*(Low|Medium|High) \(\d+%\)", short), f"short choice reveal missing band+pct: {short}")
        check("Exact equity" not in short, f"short choice reveal should fold exact equity into details: {short}")
        page.eval("document.querySelectorAll('.reveal details').forEach(d => { d.open = true; })")
        reveal = page.eval("document.querySelector('.reveal').innerText")
        check("Exact equity:" in reveal and "one matchup" in reveal, f"opened choice details incomplete: {reveal}")
        assert_no_leak(pre, reveal_equity(reveal), "choice rung 0")
        check_full_truth_absent(pre, rows_of(page)[before], "choice rung 0")
        check("Exact equity" not in visible and "Answer:" not in visible, "answer shown before committing")
        row = rows_of(page)[before]
        check(len(row) == 24 and row[3] == "equity" and row[10] in ("l", "m", "h") and not row[14] and not row[15] and row[16] and row[17],
              f"choice equity log columns wrong: {row}")
        next_drill_item(page)
        # A wrong choice answer.
        page.wait(READY, 60)
        answer_drill_item(page, False)
        wait_rows(page, before + 2)
        check(page.eval("document.querySelector('.reveal').classList.contains('bad')"), "wrong equity choice not marked bad")
        check(rows_of(page)[before + 1][6] == "0", "wrong equity answer logged correct")
        inspector.snapshot("newskill-equity-r")
        page.eval("location.hash = '#home'")
        page.wait("!!document.querySelector('.app-title')", 20)
        # Default 3-choice at rung 1 (old numeric log qualifies for rung 1) and in #checkup:equity.
        log = drills_only_log([("equity", "50")])
        seed_log(page, port, log)
        open_limited_drill(page, "equity")
        page.wait(READY, 60)
        check(page.eval("document.querySelector('.question').innerText.includes('Assume')"), "rung 1 must describe opponents in words")
        check(page.eval("document.querySelectorAll('.choice').length") == 3, "rung 1 choice equity must offer 3 choices")
        pre = pre_answer_state(page)
        before = len(rows_of(page))
        answer_drill_item(page, True)
        wait_rows(page, before + 1)
        row = rows_of(page)[before]
        page.eval("document.querySelectorAll('.reveal details').forEach(d => { d.open = true; })")
        reveal = page.eval("document.querySelector('.reveal').innerText")
        check("combos" in reveal, f"rung 1 reveal has no combo count: {reveal}")
        assert_no_leak(pre, reveal_equity(reveal), "choice rung 1")
        check_full_truth_absent(pre, row, "choice rung 1")
        page.eval("location.hash = '#home'")
        page.wait("!!document.querySelector('.app-title')", 20)
        # Harder typed-number mode: toggle on Home, persisted in localStorage, preserves keypad & tolerance staircase.
        page.eval(f"localStorage.setItem({json.dumps(LOG_KEY)}, '')")
        page.send("Page.reload", {})
        page.wait("!!document.querySelector('.app-title') && !!window.__trainer", 30)
        click(page, "[data-equity-exact]")
        must_see(page, "document.querySelector('[data-equity-exact]').textContent === 'Type exact equity %: on'", 5, "toggle did not turn on")
        check(page.eval("localStorage.getItem('poker-trainer.equity-exact')") == "1", "equity-exact setting not saved")
        page.send("Page.reload", {})
        page.wait("!!document.querySelector('.app-title') && !!window.__trainer", 30)
        check(page.eval("document.querySelector('[data-equity-exact]').textContent") == "Type exact equity %: on",
              "equity-exact setting lost across reload")
        open_limited_drill(page, "equity")
        page.wait(READY, 60)
        check(page.eval("!!document.querySelector('.keypad')"), "typed equity mode must show the keypad")
        pre = pre_answer_state(page)
        visible = page.eval("document.body.innerText")
        before = len(rows_of(page))
        current, kind, answer = answer_drill_item(page, True)
        wait_rows(page, before + 1)
        reveal = page.eval("document.querySelector('.reveal').innerText")
        for part in ("Exact equity", "You said", "points", "counts as a hit", "Hit", "one matchup"):
            check(part in reveal, f"equity reveal is missing '{part}': {reveal}")
        check("1 x 1" not in reveal, f"a meaningless combo count: {reveal}")
        assert_no_leak(pre, reveal_equity(reveal), "rung 0")
        check_full_truth_absent(pre, rows_of(page)[before], "rung 0")
        check("Exact equity" not in visible, "exact equity shown before committing")
        row = rows_of(page)[before]
        check(len(row) == 24 and row[3] == "equity" and row[14] and row[15] and row[16] and row[17], f"equity log columns missing: {row}")
        check(int(row[14]) == 10, f"first band should be 10, got {row[14]}")
        next_drill_item(page)
        # A wrong typed answer: an estimate far from the truth.
        page.wait(READY, 60)
        current, kind, answer = answer_drill_item(page, False)
        wait_rows(page, before + 2)
        reveal = page.eval("document.querySelector('.reveal').innerText")
        check("Miss" in reveal, f"equity miss not shown: {reveal}")
        check(("points high" in reveal or "points low" in reveal), f"signed error missing: {reveal}")
        check(rows_of(page)[before + 1][6] == "0", "wrong equity answer logged correct")
        next_drill_item(page)
        # The band tightened after the first hit and loosened after the miss: 10, 9, 15.
        page.wait(READY, 60)
        current, kind, answer = answer_drill_item(page, True)
        wait_rows(page, before + 3)
        row = rows_of(page)[before + 2]
        check(int(row[14]) == 15, f"third item band should be 15 after hit then miss, got {row[14]}")
        page.eval("location.hash = '#home'")
        page.wait("!!document.querySelector('.app-title')", 20)
        # Switch back to default 3-choice mode and verify rung 1 typed mode is separate if turned back off;
        # also test #checkup:equity in default 3-choice mode across both rungs.
        click(page, "[data-equity-exact]")
        must_see(page, "document.querySelector('[data-equity-exact]').textContent === 'Type exact equity %: off'", 5, "toggle did not turn off")
        seed_log(page, port, LOG_COLUMNS + "\n")
        page.eval("location.hash = '#checkup:equity'")
        page.wait("[...document.querySelectorAll('.btn.primary')].some(b => b.textContent === 'Begin')", 20)
        click_text(page, ".btn.primary", "Begin")
        rungs = set()
        number = 1
        while len(rungs) < 2 and number <= 12:
            page.wait(READY + f" && __trainer.current.number === {number}", 60)
            current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
            check(current["bundle"]["format"]["type"] == "choice", f"checkup equity should default to choice: {current['bundle']['format']}")
            rungs.add(current["bundle"]["question"]["rung"])
            pre = pre_answer_state(page)
            truth = check_probe(page, current, "m")["outcome"]["reveal"]["exact"].rstrip("%")
            assert_no_leak(pre, truth, f"checkup question {number}")
            check("Exact equity" not in page.eval("document.body.innerText"), "a checkup showed the exact equity")
            type_answer(page, current, "choice", "m")
            number += 1
        check(rungs == {0, 1}, f"checkup leak test saw rungs {rungs}")
        page.eval("document.querySelector('.top-bar button.back').click()")
        page.wait("document.body.innerText.includes('Checkup saved')", 20)
        page.eval("location.hash = '#home'")
        page.wait("!!document.querySelector('.app-title')", 20)
        # anchor.
        seed_log(page, port, "")
        open_limited_drill(page, "anchor")
        page.wait(READY, 60)
        check(page.eval("!!document.querySelector('.keypad')"), "anchor uses the percent keypad")
        inspector.snapshot("newskill-anchor-q")
        current, kind, answer = answer_drill_item(page, True)
        reveal = page.eval("document.querySelector('.reveal').innerText")
        check("Answer:" in reveal and "You said" in reveal, f"anchor reveal: {reveal}")
        inspector.snapshot("newskill-anchor-r")
        # A slow grade shows it is working, with the controls locked.
        next_drill_item(page)
        page.wait(READY, 60)
        page.eval("""(() => {
          const real = __trainer.engine.call;
          __trainer.engine.call = (...args) => args[0] === 'grade'
            ? new Promise(resolve => setTimeout(resolve, 900)).then(() => real(...args)) : real(...args);
        })()""")
        answer_percent_keypad(page, "33")
        must_see(page, "(document.querySelector('[data-pending]') || {}).textContent === 'Working it out...'", 3, "no pending state while grading")
        check(page.eval("[...document.querySelectorAll('.answers button')].every(b => b.disabled)"), "controls unlocked while grading")
        page.wait("!!document.querySelector('.reveal')", 30)
        check(not browser.errors, f"new skill errors: {browser.errors[:5]}")
        print("new skills ok, boards right:", boards_right)
    finally:
        browser.close()
        srv.shutdown()


TABLE_DRILL_LOG = "\n".join([LOG_COLUMNS] + [
    f"{1000 + i}\tdrill\t\ttable\t{1000 + i}\t1\t1\t0\t0.5000\t1500\tcheck\t\t\t\t\t\t\t\t{205 + i * 3}\trobust"
    for i in range(45)
]) + "\n"
VERDICTS = ["robust", "close", "depends"]


def table_outcomes(page, bundle):
    """Every option's outcome for the shown table spot, via the same path the app uses."""
    outcomes = {}
    for option in bundle["format"]["options"]:
        reply = grade_probe(page, "table", bundle["seed"], option["code"])
        outcomes[option["code"]] = reply["outcome"]
    return outcomes


def check_table_reveal(page, verdict, want_correct, outcome, where):
    # The breakdown by opponent type sits behind a toggle: read it opened.
    page.eval("document.querySelectorAll('.reveal details').forEach(d => { d.open = true; })")
    reveal = page.eval("document.querySelector('.reveal').innerText")
    check(page.eval("document.querySelector('.verdict').textContent").startswith("Correct" if want_correct else "Not quite"),
          f"{where}: verdict line {reveal[:80]}")
    check(reveal.startswith("Correct") == want_correct, f"{where}: reveal tone {reveal[:80]}")
    # The short reveal: best play, one EV per option, and on a depends spot one line per play.
    short = page.eval("(() => { const c = document.querySelector('.reveal').cloneNode(true); c.querySelectorAll('details').forEach(d => d.remove()); return c.innerText; })()")
    check("Best:" in short, f"{where}: no best option in {short}")
    check(len(re.findall(r"-?[+\d.]+bb", short)) >= len(outcome["reveal"]["options"]), f"{where}: each option needs its EV in bb: {short}")
    check("simulated tables" not in short and "simulated opponents" not in short and "% of the pot" not in short, f"{where}: the short reveal carries the long numbers: {short}")
    if verdict == "depends":
        check(re.search(r" vs [a-z-]+", short), f"{where}: depends needs one line per play: {short}")
    else:
        check(" vs " not in short, f"{where}: only depends spots show the breakdown: {short}")
    # The detail is still there, folded away.
    page.eval("document.querySelectorAll('.reveal details').forEach(d => { d.open = true; })")
    reveal = page.eval("document.querySelector('.reveal').innerText")
    check("simulated loose-table opponents" in reveal and "assumptions" in reveal, f"{where}: no simulation caveat in {reveal}")
    check(outcome["reveal"]["explanation"].split(":")[0] in reveal, f"{where}: no verdict explanation in {reveal}")
    check(len(re.findall(r"% of the pot, best at [\d.]+% of simulated tables", reveal)) == len(outcome["reveal"]["options"]), f"{where}: the detail needs percent and 'best at' per option: {reveal}")
    if verdict == "depends":
        check("Against a" in reveal and "of the simulated opponents here" in reveal, f"{where}: depends needs the archetype breakdown: {reveal}")
        check(("fair choice" in reveal or "the right read" in reveal) if want_correct else "rarely a read worth making" in reveal, f"{where}: no read note: {reveal}")
    if not want_correct and verdict != "depends":
        check("Cost:" in reveal and outcome["reveal"]["loss"].split(" ")[0] in reveal, f"{where}: no loss in {reveal}")



RESEARCH_LOG_COLUMNS = "timestamp_ms\tmode\tbaseline_run\tskill\titem\tgen_version\tcorrect\tclose\tev_loss_pct\tresponse_ms\tanswer\tcheck_run\tcheck_form\tcheck_slot\tband\tsigned_error\ttruth_pct\topponents\ttable_spot\ttable_verdict\tfamily\tstage\tconfidence\treflect"


def graded_outcomes(page, current):
    """Every choice's outcome for the shown item, as already prepared or via the engine."""
    bundle = current["bundle"]
    outcomes = {}
    for option in bundle["format"]["options"]:
        code = option["code"]
        outcomes[code] = bundle["graded"].get(code) or grade_probe(page, bundle["skill"], bundle["seed"], code)["outcome"]
    return outcomes


# Clicks SELECTOR the moment it appears, inside the double-tap window.
TAP_ON_SHOW = """(() => {
  window.__tappedOnShow = false;
  const timer = setInterval(() => {
    const el = document.querySelector('SELECTOR');
    if (!el) return;
    clearInterval(timer);
    el.click();
    window.__tappedOnShow = true;
  }, 4);
})()"""


def run_research(viewport=(390, 844), mobile=True, theme="light", port_offset=0):
    port = 8995 + PORT_SHIFT + port_offset
    srv = server.start(port)
    browser = Browser(f"research-{viewport[0]}-{theme}")
    try:
        page = browser.page
        page.viewport(*viewport, mobile)
        page.theme(theme)
        boot_app(page, port, (*viewport, mobile), theme)
        inspector = Inspector(page, viewport, theme, True)
        # --- 1. Start over warns about the next form, not the unfinished one.
        check_line = lambda form, slot, t: f"{t}\tcheck\t\ttable\t7\t1003\t1\t0\t\t1000\tfold\t1\t{form}\t{slot}\t\t\t\t"
        for form, expect_note in [(3, False), (4, True)]:
            seed_log(page, port, RESEARCH_LOG_COLUMNS + "\n" + check_line(form, 0, 1000) + "\n")
            page.eval("location.hash = '#checkup:table'")
            page.wait("!!document.querySelector('.btn.primary')", 20)
            has_note = page.eval("!!document.querySelector('[data-start-over-note]')")
            check(has_note == expect_note, f"form {form} in progress: note {'missing' if expect_note else 'should not show'}")
            if expect_note:
                text = page.eval("document.querySelector('[data-start-over-note]').textContent")
                check("reuses" in text, f"note text: {text}")
                check(page.eval("!document.querySelector('[data-reuse-start]')"), "form 4 itself must not carry the reuse note")
                inspector.snapshot("research-start-over-note")
            page.eval("location.hash = '#home'")
            wait_home(page)
        # --- Confidence setting: off by default, one toggle, stored.
        page.eval("document.querySelectorAll('.seg')[2].click()")
        toggle = "[...document.querySelectorAll('button')].find(b => b.textContent.startsWith('Ask how sure'))"
        check(page.eval(f"{toggle}.textContent") == "Ask how sure I am: off", "confidence must default to off")
        # --- The old beats drill says what its percentage is against.
        seed_log(page, port, LOG_COLUMNS + "\n")
        open_limited_drill(page, "beats")
        answer_drill_item(page, False)
        wait_rows(page, 1)
        text = page.eval("document.querySelector('.reveal').innerText")
        check("That's against random hands" in text and "first read" in text, f"no random note: {text}")
        inspector.snapshot("research-beats-note", full=True)
        page.eval("location.hash = '#home'")
        wait_home(page)
        # --- A card plus any of a suit on the nuts picker.
        seed_log(page, port, LOG_COLUMNS + "\n")
        open_limited_drill(page, "nuts")
        page.wait(READY, 60)
        click_text(page, ".rank-grid .rank", "A")
        page.eval("document.querySelectorAll('.slot')[0].querySelector('.slot-suit.suit-s').click()")
        click(page, ".slot-any")
        page.eval("document.querySelectorAll('.slot')[1].querySelector('.slot-suit.suit-s').click()")
        preview = page.eval("document.querySelector('.picker-preview').textContent")
        check("with any other spade" in preview, f"no any-of-suit form on the picker: {preview}")
        check(page.eval("!document.querySelector('.picker .btn.primary').disabled"), "any-of-suit form not submittable")
        inspector.snapshot("research-nuts-any-suit", full=True)
        page.eval("location.hash = '#home'")
        wait_home(page)
        # --- The ladder: a rung 0 item, its reveal, and a mix question.
        seed_log(page, port, LOG_COLUMNS + "\n")
        open_limited_drill(page, "beats_me")
        page.wait(READY, 60)
        current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
        check(current["bundle"]["question"]["kind"] == "beats_me" and current["bundle"]["question"]["rung"] == 0, f"rung {current['bundle']['question']}")
        check(page.eval("document.querySelector('.question').innerText").count("HAND") >= 4 or page.eval("document.querySelector('.question').innerText").count("Hand") >= 4, "the four hands are not shown")
        inspector.snapshot("research-beats-me-q", full=True)
        answer_drill_item(page, False)
        wait_rows(page, 1)
        reveal = page.eval("document.querySelector('.reveal').innerText")
        check("ahead of you" in reveal or "behind you" in reveal or "tied with you" in reveal, f"rung 0 reveal: {reveal}")
        inspector.snapshot("research-beats-me-r", full=True)
        page.eval("location.hash = '#home'")
        wait_home(page)
        # --- 2. Flip feedback on spot and spot_mw, and only there.
        seed_log(page, port, LOG_COLUMNS + "\n")
        open_limited_drill(page, "spot")
        answer_drill_item(page, False)
        wait_rows(page, 1)
        page.eval("document.querySelectorAll('.reveal details').forEach(d => { d.open = true; })")
        text = page.eval("document.querySelector('.reveal').innerText")
        check(re.search(r"Calling stops being right above about \$\d+\.", text), f"no flip call line: {text}")
        check(re.search(r"\d+ of 4[67] next cards put you ahead of the price\.", text), f"no flip card line: {text}")
        check(re.search(r"Hands counted: ", text), f"the reveal does not list the groups used: {text}")
        question_text = page.eval("document.querySelector('.question').innerText")
        check(re.search(r"The bettor is an? ", question_text), f"no player type in the question: {question_text}")
        check("Assume they have one of:" not in question_text, f"the question still lists the hands: {question_text}")
        inspector.snapshot("research-flip-spot", full=True)
        page.eval("location.hash = '#home'")
        wait_home(page)
        seed_log(page, port, LOG_COLUMNS + "\n")
        open_limited_drill(page, "spot_mw")
        answer_drill_item(page, True)
        page.eval("document.querySelectorAll('.reveal details').forEach(d => { d.open = true; })")
        text = page.eval("document.querySelector('.reveal').innerText")
        check("Calling stops being right above about" in text, f"multiway has no flip call line: {text}")
        check("The bettor:" in text and ("The caller:" in text or "Caller 1:" in text) and "Opponent 1" not in text,
              f"multiway reveal must name bettor/caller: {text}")
        check("next cards put you ahead" not in text, f"multiway must not show the card count: {text}")
        page.eval("location.hash = '#home'")
        wait_home(page)
        # --- 3. Reflection: offered on some misses, never blocking, logged.
        seed_log(page, port, LOG_COLUMNS + "\n")
        open_limited_drill(page, "spot")
        offered_row = withheld_row = next_row_ok = None
        for step in range(30):
            page.wait(READY, 60)
            current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
            outcomes = graded_outcomes(page, current)
            wrong = [c for c, o in outcomes.items() if not o["ok"]]
            if not wrong or current["planned"]["origin"] == "retry":
                type_answer(page, current, "choice", next(iter(outcomes)))
                page.wait("!!document.querySelector('.reveal')", 30)
                next_drill_item(page)
                continue
            code = wrong[0]
            form = outcomes[code]["reflection"]
            before = len(rows_of(page))
            guard_tap = bool(form) and offered_row is None
            if guard_tap:
                page.eval(TAP_ON_SHOW.replace("SELECTOR", "[data-reflect] .btn.cue"))
            type_answer(page, current, "choice", code)
            page.wait("!!document.querySelector('.reveal')", 30)
            wait_rows(page, before + 1)
            if guard_tap:
                must_see(page, "!!document.querySelector('[data-reflect]')", 10, "offered form not shown")
                page.wait("window.__tappedOnShow === true", 10)
                check(page.eval(f"document.querySelector('.reflect-title').textContent === {json.dumps(form[0]['text'])}"), "a cue tapped inside the double-tap window counted")
            response = int(rows_of(page)[before][9])
            page.wait("!document.querySelector('.action-bar .btn').disabled", 10)
            row_state = rows_of(page)[before][23]
            if form and offered_row is None:
                check(row_state == "offered", f"offered miss logged {row_state!r}")
                check(page.eval("!!document.querySelector('[data-reflect]')"), "offered form not shown")
                check(len(form) == 3 and all(3 <= len(p["cues"]) <= 4 for p in form), f"form shape {form}")
                inspector.snapshot("research-reflect-form", full=True)
                for i in range(3):
                    time.sleep(0.35)  # past the double-tap window
                    page.eval("document.querySelector('[data-reflect] .btn.cue').click()")
                    if i < 2:
                        page.wait(f"!!document.querySelector('[data-reflect] .btn.cue') && document.querySelector('.reflect-title').textContent === {json.dumps(form[i + 1]['text'])}", 5)
                wait_rows(page, before + 2)
                row = rows_of(page)[before + 1]
                check(row[1] == "reflect" and row[23] == "answered", f"reflect row: {row}")
                check(int(row[9]) == 0 and row[3] == "spot", f"reflect row has time or wrong skill: {row}")
                check(int(rows_of(page)[before][9]) == response, "reflecting changed the response time")
                offered_row = row
            elif not form and withheld_row is None:
                check(row_state == "withheld", f"unoffered miss logged {row_state!r}")
                check(page.eval("!document.querySelector('[data-reflect]')"), "form shown for a held-back miss")
                withheld_row = rows_of(page)[before]
            elif form and next_row_ok is None:
                # Next must work with the form showing and logs nothing.
                check(page.eval("!!document.querySelector('[data-reflect]')"), "second offered form not shown")
                next_row_ok = True
                click(page, ".action-bar .btn")
                page.wait("!document.querySelector('.reveal')", 30)
                check(len(rows_of(page)) == before + 1, "ignoring the form logged a reflect row")
                page.wait(READY + " || !!document.querySelector('.big-score')", 60)
                if offered_row and withheld_row:
                    break
                continue
            if offered_row and withheld_row and next_row_ok:
                break
            next_drill_item(page)
        check(offered_row is not None and withheld_row is not None and next_row_ok, f"reflection paths not all exercised: {offered_row}, {withheld_row}, {next_row_ok}")
        page.eval("location.hash = '#home'")
        wait_home(page)
        # Ending partway through the form keeps the cues already tapped.
        seed_log(page, port, LOG_COLUMNS + "\n")
        open_limited_drill(page, "spot")
        partial_kept = False
        for step in range(40):
            page.wait(READY, 60)
            current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
            outcomes = graded_outcomes(page, current)
            offered = [c for c, o in outcomes.items() if o["reflection"]] if current["planned"]["origin"] != "retry" else []
            type_answer(page, current, "choice", offered[0] if offered else next(iter(outcomes)))
            page.wait("!!document.querySelector('.reveal')", 30)
            if offered:
                before = len(rows_of(page))
                time.sleep(0.35)
                page.eval("document.querySelector('[data-reflect] .btn.cue').click()")
                page.eval("document.querySelector('.top-bar button.back').click()")
                page.wait("!!document.querySelector('.big-score')", 20)
                deadline = time.time() + 5
                while len(rows_of(page)) <= before and time.time() < deadline:
                    time.sleep(0.1)
                row = rows_of(page)[-1]
                check(len(rows_of(page)) > before and row[1] == "reflect" and row[23] == "answered" and ":" in row[10], f"partial cues lost on End: {row}")
                partial_kept = True
                break
            next_drill_item(page)
        check(partial_kept, "no offered form found for the partial End test")
        # Leaving by the browser's back gesture keeps the cues too, at once and only once.
        seed_log(page, port, LOG_COLUMNS + "\n")
        open_limited_drill(page, "spot")
        back_kept = False
        for step in range(40):
            page.wait(READY, 60)
            current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
            outcomes = graded_outcomes(page, current)
            offered = [c for c, o in outcomes.items() if o["reflection"]] if current["planned"]["origin"] != "retry" else []
            type_answer(page, current, "choice", offered[0] if offered else next(iter(outcomes)))
            page.wait("!!document.querySelector('.reveal')", 30)
            if offered:
                time.sleep(0.35)
                page.eval("document.querySelector('[data-reflect] .btn.cue').click()")
                before = len(rows_of(page))
                page.eval("location.hash = '#home'")
                wait_home(page)
                deadline = time.time() + 5
                while len(rows_of(page)) <= before and time.time() < deadline:
                    time.sleep(0.1)
                reflections = [r for r in rows_of(page) if r[1] == "reflect"]
                check(len(reflections) == 1 and reflections[0][23] == "answered" and ":" in reflections[0][10], f"cues lost on the back gesture: {reflections}")
                # A later session finishing must not write that row again.
                open_limited_drill(page, "spot")
                page.wait(READY, 60)
                page.eval("document.querySelector('.top-bar button.back').click()")
                page.wait("!!document.querySelector('.big-score') || location.hash === '#home'", 20)
                time.sleep(0.5)
                reflections = [r for r in rows_of(page) if r[1] == "reflect"]
                check(len(reflections) == 1, f"the reflection row was written twice: {reflections}")
                back_kept = True
                break
            next_drill_item(page)
        check(back_kept, "no offered form found for the back gesture test")
        page.eval("location.hash = '#home'")
        wait_home(page)
        # --- 4. Confidence: asked only when on, before the reveal, on its own skills.
        seed_log(page, port, LOG_COLUMNS + "\n")
        open_limited_drill(page, "made")
        current, kind, answer = answer_drill_item(page, True)
        wait_rows(page, 1)
        check(page.eval("!document.querySelector('.confidence-ask')"), "confidence asked while the setting is off")
        check(not rows_of(page)[0][22:][0], "confidence logged while the setting is off")
        page.eval("location.hash = '#home'")
        wait_home(page)
        page.eval(f"{toggle}.click()")
        check(page.eval(f"{toggle}.textContent") == "Ask how sure I am: on", "toggle did not switch on")
        seed_log(page, port, LOG_COLUMNS + "\n")
        check(page.eval("localStorage.getItem('poker-trainer.confidence')") == "1", "setting not stored")
        open_limited_drill(page, "made")
        current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
        kind, answer = pick_answer(page, current, True)
        page.eval(TAP_ON_SHOW.replace("SELECTOR", ".confidence-ask .choice"))
        type_answer(page, current, kind, answer)
        must_see(page, "!!document.querySelector('.confidence-ask')", 10, "confidence prompt never shown")
        page.wait("window.__tappedOnShow === true", 10)
        check(page.eval("!!document.querySelector('.confidence-ask')") and len(rows_of(page)) == 0, "a confidence tap inside the double-tap window counted")
        check(page.eval("!document.querySelector('.reveal')"), "the reveal came before the confidence tap")
        check(len(rows_of(page)) == 0, "the answer was logged before the confidence tap")
        check(page.eval("document.querySelectorAll('.confidence-ask .choice').length") == 3, "three levels expected")
        check("55" in page.eval("document.querySelector('.confidence-ask').innerText"), "levels should say what they mean")
        inspector.snapshot("research-confidence", full=True)
        time.sleep(0.35)  # past the double-tap window
        page.eval("[...document.querySelectorAll('.confidence-ask .choice')][2].click()")
        page.wait("!!document.querySelector('.reveal')", 10)
        wait_rows(page, 1)
        check(rows_of(page)[0][22] == "high", f"confidence column: {rows_of(page)[0][22:]}")
        next_drill_item(page)
        # Ending during the tap keeps the answer, unrated.
        page.wait(READY, 30)
        current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
        kind, answer = pick_answer(page, current, True)
        type_answer(page, current, kind, answer)
        page.wait("!!document.querySelector('.confidence-ask')", 10)
        page.eval("document.querySelector('.top-bar button.back').click()")
        page.wait("!!document.querySelector('.big-score')", 20)
        check(len(rows_of(page)) == 2, f"ending during the tap lost the answer: {len(rows_of(page))} rows")
        check(rows_of(page)[1][22] == "", f"an ended tap logged a rating: {rows_of(page)[1][22:]}")
        page.eval("location.hash = '#home'")
        wait_home(page)
        # Hiding the page during the tap saves the answer once and closes the tap.
        seed_log(page, port, LOG_COLUMNS + "\n")
        open_limited_drill(page, "made")
        page.wait(READY, 30)
        current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
        kind, answer = pick_answer(page, current, True)
        type_answer(page, current, kind, answer)
        page.wait("!!document.querySelector('.confidence-ask')", 10)
        time.sleep(0.35)
        page.eval("window.__heldChoice = [...document.querySelectorAll('.confidence-ask .choice')][2]")
        page.eval("Object.defineProperty(document, 'hidden', { configurable: true, get: () => true }); document.dispatchEvent(new Event('visibilitychange'))")
        wait_rows(page, 1)
        page.eval("window.__heldChoice.click()")
        time.sleep(0.5)
        page.eval("delete document.hidden")
        check(len(rows_of(page)) == 1, f"hiding the page then tapping logged {len(rows_of(page))} rows")
        check(rows_of(page)[0][22] == "", f"a flushed answer took a rating: {rows_of(page)[0][22:]}")
        check(page.eval("!!document.querySelector('.reveal') && !document.querySelector('.confidence-ask')"), "the tap stayed up after the page was hidden")
        page.eval("location.hash = '#home'")
        wait_home(page)
        # Not asked on spot, spot_mw or table even when on.
        seed_log(page, port, LOG_COLUMNS + "\n")
        open_limited_drill(page, "spot")
        answer_drill_item(page, True)
        check(page.eval("!document.querySelector('.confidence-ask')"), "confidence asked on a spot")
        page.eval("location.hash = '#home'")
        wait_home(page)
        page.eval(f"{toggle}.click()")
        # --- 5. Whole-hand chains: four committed stages, a tie line at the end.
        seed_log(page, port, LOG_COLUMNS + "\n")
        open_limited_drill(page, "chain")
        expected = ["Part 1 of 4", "Part 2 of 4", "Part 3 of 4", "Part 4 of 4"]
        stages = ["hand", "outs", "price", "decide"]
        seeds = []
        for index in range(4):
            page.wait(READY, 60)
            current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
            bundle = current["bundle"]
            check(bundle["skill"] == "chain", f"stage {index} skill {bundle['skill']}")
            check(bundle["question"]["kind"] == "chain" and bundle["question"]["stage"] == stages[index], f"stage {index}: {bundle['question'].get('stage')}")
            text = page.eval("document.querySelector('.question').innerText")
            check(expected[index] in text, f"no part marker: {text}")
            if index == 0:
                check("villain" not in bundle["question"], "the hand stage leaks the opponent's cards")
                check("opponent" not in text.lower(), f"the hand stage must not show the opponent yet: {text}")
                check(bundle["format"]["type"] == "choice" and any(o["label"] == "top pair" for o in bundle["format"]["options"]), "hand stage answers like made")
            if index == 1:
                check("opponent shows" in text.lower() and bundle["format"]["type"] == "integer", f"outs stage: {text}")
            if index == 2:
                check(bundle["format"]["type"] == "percent", f"price stage format {bundle['format']['type']}")
            if index == 3:
                check(any(s in text for s in ("All-in", "one more card")), f"decide stage states no assumption: {text}")
                check([o["code"] for o in bundle["format"]["options"]] == ["c", "f"], "decide stage asks call or fold")
            check("Your hand" not in page.eval("document.querySelector('.top-title').textContent"), "chain stages must not carry a skill label")
            seeds.append(bundle["seed"])
            inspector.snapshot(f"research-chain-{stages[index]}-q", full=True)
            before = len(rows_of(page))
            answer_drill_item(page, True)
            wait_rows(page, before + 1)
            row = rows_of(page)[before]
            check(row[21] == stages[index], f"stage column {row[21]} for {stages[index]}")
            reveal = page.eval("document.querySelector('.reveal').innerText")
            if index < 3:
                check("so calling was right" not in reveal and "so folding was right" not in reveal, f"tie line too early: {reveal}")
                inspector.snapshot(f"research-chain-{stages[index]}-r", full=True)
            else:
                check("so calling was right." in reveal or "so folding was right." in reveal, f"no tie line: {reveal}")
                inspector.snapshot("research-chain-decide-r", full=True)
            if index < 3:
                next_drill_item(page)
        check(len({int(s) & ~3 for s in seeds}) == 1, f"stages were not one hand: {seeds}")
        check(all(int(row[9]) >= 0 for row in rows_of(page)), "a stage has no time")
        check(len(rows_of(page)) == 4, f"{len(rows_of(page))} rows for 4 stages")
        page.eval("location.hash = '#home'")
        wait_home(page)
        # --- 6. Progress panels: not enough, then enough.
        page.eval("location.hash = '#progress'")
        page.wait("!!document.querySelector('[data-reflection-panel]')", 20)
        text = page.eval("document.querySelector('[data-reflection-panel]').innerText")
        check("Not enough data" in text, f"reflection panel before data: {text}")
        check("Not enough data" in page.eval("document.querySelector('[data-reliability]').innerText"), "reliability panel before data")
        t = int(time.time() * 1000) - 86400000 * 40
        def reflect_row(i, t):
            state = "offered" if i % 2 == 0 else "withheld"
            return f"{t}\tdrill\t\tspot\t{7000 + i}\t1\t0\t0\t8.0000\t3000\tc\t\t\t\t\t\t\t\t\t\tallin:top\t\t\t{state}"
        lines = [RESEARCH_LOG_COLUMNS]
        for i in range(70):
            t += 1000
            lines.append(reflect_row(i, t))
            if i % 2 == 0:
                t += 1000
                lines.append(f"{t}\treflect\t\tspot\t{7000 + i}\t1\t0\t0\t\t0\tsupported:price\t\t\t\t\t\t\t\t\t\tallin:top\t\t\tanswered")
            t += 1000
            lines.append(f"{t}\tdrill\t\tspot\t{8000 + i}\t1\t{i % 3 != 0 and 1 or 0}\t0\t0.5000\t2500\tf\t\t\t\t\t\t\t\t\t\tallin:top\t\t\t")
        for i in range(35):
            t += 1000
            right = 1 if i < 28 else 0
            lines.append(f"{t}\tdrill\t\tmade\t{9000 + i}\t1\t{right}\t0\t\t1500\ttop\t\t\t\t\t\t\t\t\t\ttop\t\thigh\t")
        seed_log(page, port, "\n".join(lines) + "\n")
        page.eval("location.hash = '#progress'")
        page.wait("!!document.querySelector('[data-reflection-panel]')", 20)
        text = page.eval("document.querySelector('[data-reflection-panel]').innerText")
        check("Not enough data" not in text and "noisy data" in text, f"reflection panel with data: {text}")
        check("After reflecting" in text and "no form" in text and "Reflected" in text and "Form not shown" in text, f"comparison missing: {text}")
        check("35 of 35" in text or "of 35" in text, f"comparison counts: {text}")
        check(re.search(r"Shown, skipped\s+0 of 0\s+-", text), f"an empty group should show no share: {text}")
        relia = page.eval("document.querySelector('[data-reliability]').innerText")
        check("When you said High on Your hand, you were right 80% of the time" in relia, f"reliability sentence: {relia}")
        check(not page.eval("!!document.querySelector('svg.chart, canvas')"), "progress must have no charts")
        inspector.snapshot("research-progress", full=True)
        check(not browser.errors, f"research errors: {browser.errors[:5]}")
        print("research ok")
    finally:
        browser.close()
        srv.shutdown()


def run_research_shots():
    """The new screens at both sizes and themes. Run on request, not in the full suite."""
    modes = [((390, 844), True, "light"), ((390, 844), True, "dark"), ((1280, 800), False, "light"), ((1280, 800), False, "dark")]
    for offset, (viewport, mobile, theme) in enumerate(modes):
        run_research(viewport, mobile, theme, port_offset=offset)


def run_table_spots():
    port = 8989 + PORT_SHIFT
    srv = server.start(port)
    browser = Browser("tablespots")
    try:
        page = browser.page
        page.viewport(390, 844, True)
        page.theme("light")
        inspector = Inspector(page, (390, 844), "light", True)
        seed_log(page, port, LOG_COLUMNS + "\n")
        home = page.eval("document.body.innerText")
        spots = page.eval("(document.querySelector('[data-group=spots]') || {}).title || ''")
        check("Spots" in home and "Table spots" in spots, f"home does not offer table spots in its group: {home[:300]} / {spots}")
        page.eval("location.hash = '#progress'")
        page.wait("!!document.querySelector('.skill-row')", 20)
        progress = page.eval("document.body.innerText")
        check("Table spots" in progress and "robust and close spots only" in progress, f"progress: {progress[:400]}")
        check("Every drill is open" in progress and "Step " not in progress, f"progress still shows steps: {progress[:200]}")
        page.eval("location.hash = '#home'")
        wait_home(page)
        open_limited_drill(page, "table")
        needed = {(v, ok) for v in VERDICTS for ok in (True, False)}
        seen_rows = []
        for attempt in range(220):
            page.wait(READY + " || !!document.querySelector('.big-score')", 60)
            current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
            bundle = current["bundle"]
            check(bundle["graded"] == {}, "table options must not be graded before the answer")
            if attempt == 0:
                check(page.eval("document.querySelectorAll('.tl-street').length") >= 2, "no street timeline")
                text = page.eval("document.querySelector('.question').innerText")
                for needle in ("Pot", "To call", "Your seat", "Behind", "What do you do"):
                    check(needle in text, f"question lacks {needle}: {text[:300]}")
                pre = pre_answer_state(page)
                for secret in ("best at", "verdict", "explanation", "ev_bb", "simulated"):
                    check(secret not in pre, f"the page holds '{secret}' before the answer")
                inspector.snapshot("table-question", full=True)
            outcomes = table_outcomes(page, bundle)
            verdict = next(iter(outcomes.values()))["reveal"]["verdict"]
            right = [c for c, o in outcomes.items() if o["correct"]]
            wrong = [c for c, o in outcomes.items() if not o["correct"]]
            wanted_ok = None
            if (verdict, True) in needed and right:
                wanted_ok = True
            elif (verdict, False) in needed and wrong:
                wanted_ok = False
            code = (right if wanted_ok in (True, None) else wrong)[0]
            outcome = outcomes[code]
            kind = "choice"
            before = len(rows_of(page))
            type_answer(page, current, kind, code)
            page.wait("!!document.querySelector('.reveal')", 30)
            wait_rows(page, before + 1)
            row = rows_of(page)[before]
            check(row[3] == "table" and len(row) == 24 and re.fullmatch(r"(flop|turn|river):(hu|mw)", row[20]) and not row[21] and not row[22], f"table row shape {row}")
            check(row[4] == bundle["seed"] and row[10] == code, f"table row item/answer {row[4]} {row[10]}")
            check(row[19] == verdict, f"logged verdict {row[19]}, spot is {verdict}")
            check(row[18].isdigit(), f"no spot id logged: {row}")
            check(row[6] == ("1" if outcome["correct"] else "0") and row[8] != "", f"flags or loss: {row[6:9]}")
            check_table_reveal(page, verdict, outcome["correct"], outcome, f"{verdict} {'right' if outcome['correct'] else 'wrong'}")
            needed.discard((verdict, outcome["correct"]))
            seen_rows.append(row)
            shot = f"table-reveal-{verdict}-{'right' if outcome['correct'] else 'wrong'}"
            if shot not in inspector.taken:
                inspector.snapshot(shot, full=True)
            else:
                inspector.snapshot(shot)
            if not needed:
                break
            click(page, ".action-bar .btn")
            page.wait("!document.querySelector('.reveal')", 30)
        check(not needed, f"table verdict and result pairs not seen: {sorted(needed)}")
        results["table_drill_items"] = len(seen_rows)
        # Back to Home: the drill's spots moved the log on.
        end_drill_summary(page, inspector)
        page.eval("location.hash = '#home'")
        wait_home(page)
        # A passing drill log passes the skill on its own: no per-skill checkup is offered.
        seed_log(page, port, TABLE_DRILL_LOG)
        page.eval("location.hash = '#progress'")
        page.wait("!!document.querySelector('.skill-row')", 20)
        text = page.eval("document.body.innerText")
        table_row = page.eval("[...document.querySelectorAll('.skill-row')].find(r => r.textContent.includes('Table spots')).innerText")
        check("Passed" in table_row and "Pass mark met" in table_row and "checkup" not in table_row.lower(), f"table status on Progress: {table_row}")
        page.eval("location.hash = '#home'")
        wait_home(page)
        check("Take checkup" not in page.eval("document.body.innerText"), "a per-skill checkup was offered on Home")
        # The older one-skill checkup still runs from its address, with its own machinery.
        page.eval("location.hash = '#checkup:table'")
        page.wait("[...document.querySelectorAll('.btn.primary')].some(b => b.textContent === 'Begin')", 20)
        click_text(page, ".btn.primary", "Begin")
        before = len(rows_of(page))
        play_checkup(page, inspector, True, shot="table-checkup-item")
        page.wait("!!document.querySelector('.big-score')", 30)
        text = page.eval("document.body.innerText")
        print("table checkup:", text[:400].replace("\n", " | "))
        check("Mark met" in text and "of the pot lost on average" in text, f"table checkup result: {text[:400]}")
        rows = rows_of(page)[before:]
        check_checkup_rows(rows, 50, "table checkup", "1", "0", columns=24)
        check(all(r[3] == "table" and r[19] in ("robust", "close") for r in rows), "depends spots leaked into the checkup")
        check(not page.eval("!!document.querySelector('[data-reused]')"), "an early form claimed reuse")
        inspector.snapshot("table-checkup-result", full=True)
        # A start screen for a late form says it reuses spots too.
        unfinished = "\n".join([LOG_COLUMNS] + [f"{i}\tcheck\t\ttable\t{i}\t1003\t1\t0\t0.5000\t1500\tcheck\t1\t6\t{i}\t\t\t\t\t205\trobust" for i in range(3)]) + "\n"
        seed_log(page, port, unfinished)
        page.eval("location.hash = '#checkup:table'")
        page.wait("[...document.querySelectorAll('.btn.primary')].some(b => b.textContent === 'Resume')", 20)
        check(page.eval("!!document.querySelector('[data-reuse-start]')") and "reuses some spots" in page.eval("document.body.innerText"), "no reuse note on the start screen of a late form")
        page.eval("location.hash = '#home'")
        wait_home(page)
        # An unfinished checkup from the old library can't be resumed: its form names different spots now.
        seed_log(page, port, unfinished.replace("\t1003\t1\t0\t0.5000", "\t2\t1\t0\t0.5000"))
        page.eval("location.hash = '#checkup:table'")
        page.wait("[...document.querySelectorAll('.btn.primary')].some(b => b.textContent === 'Begin')", 20)
        text = page.eval("document.body.innerText")
        check("Resume" not in text and "reuses some spots" not in text, f"an old-library checkup was offered for resume: {text[:300]}")
        check("older spot library" in text, f"an old-library run was not labelled in Earlier checkups: {text[:300]}")
        page.eval("location.hash = '#home'")
        wait_home(page)
        # A late form reuses reserved spots and says so on the result screen.
        late = "\n".join([LOG_COLUMNS] + [f"{i}\tcheck\t\ttable\t{i}\t1003\t1\t0\t0.5000\t1500\tcheck\t1\t5\t{i}\t\t\t\t\t205\trobust" for i in range(3)]) + "\n"
        seed_log(page, port, late)
        page.eval("location.hash = '#checkup:table'")
        page.wait("[...document.querySelectorAll('.btn.primary')].some(b => b.textContent === 'Resume')", 20)
        click_text(page, ".btn", "Start over with new questions")
        before = len(rows_of(page))
        page.wait(READY, 30)
        current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
        check(current["form"] == 6, f"late form is {current['form']}")
        play_checkup(page, inspector, False, count=50)
        page.wait("!!document.querySelector('.big-score')", 30)
        text = page.eval("document.body.innerText")
        check("reuses some spots" in text, f"no reuse note on a late form: {text[:400]}")
        check("Mark not met" in text, f"an all-wrong checkup should miss the mark: {text[:300]}")
        check(not browser.errors, f"table errors: {browser.errors[:5]}")
        print("table spots ok")
    finally:
        browser.close()
        srv.shutdown()


SLOW_CALL = """((name, ms) => {
  const real = __trainer.engine.call;
  __trainer.engine.call = (...args) => args[0] === name
    ? new Promise(resolve => setTimeout(resolve, ms)).then(() => real(...args)) : real(...args);
})"""


def run_mid_grade():
    port = 8990 + PORT_SHIFT
    srv = server.start(port)
    browser = Browser("midgrade")
    try:
        page = browser.page
        page.viewport(390, 844, True)
        page.theme("light")
        inspector = Inspector(page, (390, 844), "light", False)
        # D2: an invalid answer's message sits above the keypad and stays on screen.
        seed_log(page, port, LOG_COLUMNS + "\n")
        open_limited_drill(page, "price")
        answer_percent_keypad(page, "200")
        page.wait("!!document.querySelector('.hint.error') && document.querySelector('.hint.error').textContent.length > 2", 20)
        text = page.eval("document.querySelector('.hint.error').textContent")
        check("percent" in text.lower() or "25" in text, f"invalid message: {text}")
        check(page.eval("!!document.querySelector('.hint.error').compareDocumentPosition(document.querySelector('.keypad')) && (document.querySelector('.hint.error').compareDocumentPosition(document.querySelector('.keypad')) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0"),
              "the invalid-answer message must sit above the keypad")
        rect = page.eval("(() => { const r = document.querySelector('.hint.error').getBoundingClientRect(); return { top: r.top, bottom: r.bottom, height: innerHeight }; })()")
        check(rect["bottom"] <= rect["height"] and rect["top"] >= 0, f"the invalid-answer message is off screen: {rect}")
        inspector.snapshot("invalid-above-keypad")
        # D3: End during a slow grade commits the answer and skips the reveal.
        page.eval("location.hash = '#home'")
        wait_home(page)
        seed_log(page, port, LOG_COLUMNS + "\n")
        open_limited_drill(page, "price")
        page.eval(f"({SLOW_CALL})('grade', 1000)")
        answer_percent_keypad(page, "33")
        must_see(page, "(document.querySelector('[data-pending]') || {}).textContent === 'Working it out...'", 3, "no pending line during the slow grade")
        check(page.eval("(() => { const p = document.querySelector('[data-pending]').getBoundingClientRect(); const k = document.querySelector('.keypad').getBoundingClientRect(); return p.top < k.top && p.top >= 0; })()"),
              "the pending line must be visible above the keypad")
        page.eval("document.querySelector('.top-bar button.back').click()")
        must_see(page, "!!document.querySelector('.big-score') && document.querySelector('.big-score').textContent.includes(' of 1 right')", 20, "ending during a slow grade did not reach a one-answer summary")
        check(len(rows_of(page)) == 1, f"the ended drill logged {len(rows_of(page))} rows")
        check(page.eval("!document.querySelector('#notice.fatal')"), "End during a slow grade threw: a fatal notice appeared")
        check(not browser.errors, f"End during a slow grade threw: {browser.errors[:3]}")
        # D3: the back gesture during a slow grade commits, shows no reveal, and raises no error.
        page.eval("location.hash = '#home'")
        wait_home(page)
        seed_log(page, port, LOG_COLUMNS + "\n")
        open_limited_drill(page, "table")
        before = len(rows_of(page))
        page.eval(f"({SLOW_CALL})('grade', 1000)")
        page.eval("document.querySelector('.choice').click()")
        page.eval("location.hash = '#home'")
        wait_home(page)
        must_see(page, f"(localStorage.getItem({json.dumps(LOG_KEY)}) || '').split('\\n').filter(l => l && !l.startsWith('timestamp')).length >= {before + 1}", 20, "the back gesture lost the answer")
        check(page.eval("!document.querySelector('#notice.fatal')"), "a fatal notice appeared after the back gesture")
        check(not page.eval("!!document.querySelector('.reveal')"), "a reveal appeared after leaving")
        check(not browser.errors, f"the back gesture during a grade threw: {browser.errors[:3]}")
        # D4: Stop during a slow checkup grade counts the answer in the report.
        seed_log(page, port, drills_only_log([("price", "25")]))
        page.eval("location.hash = '#checkup:price'")
        page.wait("[...document.querySelectorAll('.btn.primary')].some(b => b.textContent === 'Begin')", 20)
        click_text(page, ".btn.primary", "Begin")
        page.wait(READY + " && __trainer.current.number === 1", 30)
        answer_percent_keypad(page, "33")
        page.wait(READY + " && __trainer.current.number === 2", 30)
        page.eval(f"({SLOW_CALL})('check_grade', 1000)")
        answer_percent_keypad(page, "33")
        page.wait("(document.querySelector('[data-pending]') || {}).textContent === 'Working it out...'", 5)
        page.eval("document.querySelector('.top-bar button.back').click()")
        must_see(page, "document.body.innerText.includes('Checkup saved')", 20, "stopping during a slow grade did not reach the saved screen")
        text = page.eval("document.body.innerText")
        check("2 of 50 answered" in text, f"the report ran before the last answer was committed: {text[:200]}")
        check(not browser.errors, f"Stop during a slow grade threw: {browser.errors[:3]}")
        print("mid-grade exits ok")
    finally:
        browser.close()
        srv.shutdown()


def run_lib_research():
    done = subprocess.run(["cargo", "test", "--release", "--test", "research", "--test", "skills"], capture_output=True, text=True, cwd=ROOT)
    print(done.stdout[-3000:])
    if done.returncode != 0:
        print(done.stderr[-1500:])
    check(done.returncode == 0, f"library research tests failed: {done.stdout[-400:]}")


def run_lib_cli():
    done = subprocess.run(["cargo", "test", "--release", "--bin", "poker-trainer"], capture_output=True, text=True, cwd=ROOT)
    print(done.stdout[-3000:])
    if done.returncode != 0:
        print(done.stderr[-1500:])
    check(done.returncode == 0, f"command-line tests failed: {done.stdout[-400:]}")
def run_lib_preflop():
    done = subprocess.run(["cargo", "test", "--release", "--test", "preflop", "--test", "skills"], capture_output=True, text=True, cwd=ROOT)
    print(done.stdout[-4000:])
    if done.returncode != 0:
        print(done.stderr[-1500:])
    check(done.returncode == 0, f"library preflop tests failed: {done.stdout[-400:]}")


def run_lib_groups():
    done = subprocess.run(["cargo", "test", "--release", "--test", "groups"], capture_output=True, text=True, cwd=ROOT)
    print(done.stdout[-3000:])
    if done.returncode != 0:
        print(done.stderr[-1500:])
    check(done.returncode == 0, f"library group tests failed: {done.stdout[-400:]}")


GROUP_SKILLS = {
    "preflop": {"preflop", "limpers"}, "read_price": {"read_price"}, "equity": {"equity", "anchor"}, "hands": {"chain"},
    "board": {"nuts", "possible", "made", "board_type", "beats_me"}, "outs": {"outs", "hit"}, "spots": {"spot", "spot_mw", "table"},
}


def run_groups():
    """Home: every group is open from the first visit; picking one deals only it."""
    port = 8991 + PORT_SHIFT
    srv = server.start(port)
    browser = Browser("groups")
    try:
        page = browser.page
        boot_app(page, port)
        inspector = Inspector(page, (390, 844), "light", False)
        home = page.eval("document.body.innerText")
        codes = page.eval("[...document.querySelectorAll('[data-group]')].map(b => b.dataset.group)")
        # The seven groups, then River and your own Preflop mistakes.
        check(codes == list(GROUP_SKILLS) + ["river", "mistakes"], f"group picker on a first visit: {codes}")
        check("Step " not in home and "Locked" not in home, f"home still shows steps: {home[:300]}")
        check("preflop 20%" in home and "spots 10%" in home, f"the mix is not described: {home[:300]}")
        check(not page.eval("!!document.querySelector('[data-checkup-offers]')"), "a checkup was offered on day one")
        inspector.snapshot("home-groups")
        for code in ("outs", "board"):
            page.eval("document.querySelectorAll('.seg')[0].click()")
            page.eval(f"document.querySelector('[data-group={code}]').click()")
            page.wait(READY, 60)
            items, seen, _ = drill_session(page, inspector, False, 14, label_prefix=f"group-{code}-")
            dealt = set(seen)
            check(items == 14 and dealt <= GROUP_SKILLS[code], f"group {code} dealt {sorted(dealt)}")
            check(len(dealt) >= 2, f"group {code} dealt only {sorted(dealt)}")
            end_drill_summary(page, inspector)
            page.eval("location.hash = '#home'")
            wait_home(page)
        # The default drill opens with the preflop group, which keeps its floor from the first item.
        before = len(rows_of(page))
        start_drill(page)
        first = page.eval("__trainer.current.bundle.skill")
        check(first in ("preflop", "limpers"), f"the mixed drill opened with {first}")
        items, seen, _ = drill_session(page, inspector, False, 30, label_prefix="mixed-")
        groups = {code for code, skills in GROUP_SKILLS.items() if skills & set(seen)}
        check(len(groups) >= 5, f"30 mixed items reached only {sorted(groups)}")
        check(not set(seen) & {"price", "beats"}, f"retired skills dealt: {sorted(seen)}")
        check(len(rows_of(page)) == before + items, "mixed drill rows not logged")
        check(not browser.errors, f"group errors: {browser.errors[:5]}")
        print("groups ok")
    finally:
        browser.close()
        srv.shutdown()


def run_lib_read_price():
    done = subprocess.run(["cargo", "test", "--release", "--test", "read_price"], capture_output=True, text=True, cwd=ROOT)
    print(done.stdout[-3000:])
    if done.returncode != 0:
        print(done.stderr[-1500:])
    check(done.returncode == 0, f"library read_price tests failed: {done.stdout[-400:]}")


READ_PRICE_RUNG_SEED = lambda rung, i: (rung << 60) + 1000 + i


# Right answers in a row that take a new rung from 10 points to 32.
READ_PRICE_PASS_RUN = 22


def read_price_log(rung):
    """A log that has passed every rung below `rung`, so a new drill deals at `rung`."""
    lines = [LOG_COLUMNS]
    t = int(time.time() * 1000) - 86400000 * 5
    for lower in range(rung):
        for i in range(READ_PRICE_PASS_RUN):
            t += 1000
            lines.append(f"{t}\tdrill\t\tread_price\t{READ_PRICE_RUNG_SEED(lower, i)}\t2\t1\t0\t\t1500\tc")
    return "\n".join(lines) + "\n"


def run_read_price():
    """Each rung, wrong and right, at phone width in both themes, then the Progress panel."""
    port = 8988 + PORT_SHIFT
    srv = server.start(port)
    browser = Browser("readprice")
    try:
        page = browser.page
        for width, height, mobile, theme in [(w, h, m, t) for (w, h, m) in ((390, 844, True), (1280, 800, False)) for t in ("light", "dark")]:
            page.viewport(width, height, mobile)
            page.theme(theme)
            page.navigate(f"http://127.0.0.1:{port}/")
            page.wait("!!document.querySelector('.app-title') && !!window.__trainer", 30)
            inspector = Inspector(page, (width, height), theme, True)
            wanted = {0: "a third to three-quarters", 1: "\u00f7", 2: "what you need", 3: "what you need", 4: "Bet $"}
            for rung in range(5):
                seed_log(page, port, read_price_log(rung))
                open_limited_drill(page, "read_price")
                page.wait(READY, 60)
                current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
                question = current["bundle"]["question"]
                check(question["kind"] == "read_price" and question["rung"] == rung, f"rung {rung}: {question}")
                text = page.eval("document.querySelector('.question').innerText")
                check("Pot before the bet" in text, f"rung {rung}: pot not labelled: {text}")
                check(len(question["context"]) >= 1 and all(line in text for line in question["context"]), f"rung {rung}: the context lines are not shown: {text}")
                if rung == 4:
                    check(question["bet"] is None and "Bet" not in text.replace("betting", "").replace("bets", ""), f"rung 4 shows a bet chip: {text}")
                else:
                    check(f"${question['bet']}" in text and question["context"][0].index("pot") < question["context"][0].index("bets"), f"rung {rung}: bet missing: {text}")
                check(rung != 3 or "%" not in text, f"rung 3 shows the share: {text}")
                inspector.snapshot(f"readprice-q{rung}-{theme}", full=True)
                answer_drill_item(page, False)
                wait_rows(page, READ_PRICE_PASS_RUN * rung + 1)
                reveal_text = page.eval("document.querySelector('.reveal').innerText")
                if rung == 1 and question["callers"] == 0:
                    check("Bet vs pot" in reveal_text and "Need" in reveal_text and "\u25b6" in reveal_text and "\u00f7" in reveal_text
                          and "to win $" not in reveal_text, f"rung 1 table reveal wrong: {reveal_text}")
                else:
                    check(("to win $" in reveal_text and "\u00f7" in reveal_text) if rung != 4 else "They call $" in reveal_text, f"rung {rung} reveal has no sum: {reveal_text}")
                check("Answer" in reveal_text, f"rung {rung} reveal has no answer: {reveal_text}")
                if rung == 0:
                    check("a third to three-quarters" in reveal_text, f"no shortcut: {reveal_text}")
                if rung == 4:
                    check("Real opponents often call anyway" in reveal_text, f"no real-opponent note: {reveal_text}")
                inspector.snapshot(f"readprice-r{rung}-{theme}", full=True)
                if page.eval("!!document.querySelector('.cue-skip')"):
                    clear = "(() => { window.scrollTo(0, document.documentElement.scrollHeight); const s = document.querySelector('.cue-skip').getBoundingClientRect(); const b = document.querySelector('.action-bar').getBoundingClientRect(); return s.bottom <= b.top + 1; })()"
                    check(page.eval(clear), f"rung {rung}: the reflection form can't be scrolled clear of the Next bar")
                page.eval("location.hash = '#home'")
                wait_home(page)
                # The next new item is a fresh sibling or a new deal; one right answer logs too.
                seed_log(page, port, read_price_log(rung))
                open_limited_drill(page, "read_price")
                page.wait(READY, 60)
                current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
                answer_drill_item(page, True)
                wait_rows(page, READ_PRICE_PASS_RUN * rung + 1)
                check(page.eval("document.querySelector('.reveal').classList.contains('good')"), f"rung {rung}: right answer not graded right")
                check(page.eval("document.querySelector('.reveal').innerText").count(wanted[rung]) >= 1, f"rung {rung}: reveal lacks {wanted[rung]!r}")
                page.eval("location.hash = '#home'")
                wait_home(page)
            if width == 390 and theme == "light":
                seen_standard = seen_inserted = False
                seed_log(page, port, read_price_log(1))
                open_limited_drill(page, "read_price")
                for _ in range(40):
                    page.wait(READY, 60)
                    current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
                    q1 = current["bundle"]["question"]
                    answer_drill_item(page, True)
                    page.wait("!!document.querySelector('.reveal')", 30)
                    if q1["callers"] == 0:
                        info = page.eval("({sw: document.documentElement.scrollWidth, sh: document.documentElement.scrollHeight, rows: document.querySelectorAll('.price-table tbody tr').length, marked: document.querySelector('.price-table tr.marked')?.innerText || '', rbot: document.querySelector('.reveal').getBoundingClientRect().bottom, btop: document.querySelector('.action-bar').getBoundingClientRect().top})")
                        check(info["sw"] <= 390 and info["sh"] <= 844 and info["rbot"] <= info["btop"] + 0.5, f"read_price table does not fit phone: {info}")
                        if "(this bet)" in info["marked"]:
                            check(info["rows"] == 10, f"inserted table should have 10 rows: {info}")
                            seen_inserted = True
                        else:
                            check(info["rows"] == 9 and info["marked"], f"standard table should have 9 rows: {info}")
                            seen_standard = True
                    if seen_standard and seen_inserted:
                        break
                    click(page, ".action-bar .btn")
                    page.wait("!document.querySelector('.reveal')", 30)
                check(seen_standard and seen_inserted, f"did not see both price table shapes: standard={seen_standard} inserted={seen_inserted}")
                page.eval("location.hash = '#home'")
                wait_home(page)
            # The Progress panel shows the rung and recent accuracy.
            seed_log(page, port, read_price_log(2))
            page.eval("location.hash = '#progress'")
            must_see(page, "!!document.querySelector('[data-read-price]')", 20, "no read the price panel at all")
            panel = page.eval("document.querySelector('[data-read-price]').innerText")
            check("Rung 3 of 5" in panel and "With a hand, share stated" in panel, f"no read the price panel: {panel}")
            check("No attempts at this rung yet" in panel, f"no read the price panel: {panel}")
            check("Points toward the next rung: 10 of 32." in panel, f"no points: {panel}")
            check("Every rung starts at 10 points. Each first-try drill answer at this rung adds 1 if right and takes off 4 if wrong, never going below 0. Reaching 32 moves you up." in panel, f"no promotion rule: {panel}")
            inspector.snapshot(f"readprice-progress-{theme}", full=True)
            seed_log(page, port, read_price_log(4))
            page.eval("location.hash = '#progress'")
            must_see(page, "!!document.querySelector('[data-read-price]')", 20, "no read the price panel at all")
            top = page.eval("document.querySelector('[data-read-price]').innerText")
            check("Rung 5 of 5" in top and "This is the top rung." in top and "Points" not in top and "moves you up" not in top, f"top rung panel: {top}")
            page.eval("location.hash = '#home'")
            wait_home(page)
        check(not browser.errors, f"read_price errors: {browser.errors[:5]}")
        print("read_price ok")
    finally:
        browser.close()
        srv.shutdown()


def run_lib_table():
    done = subprocess.run(["cargo", "test", "--release", "--test", "table"], capture_output=True, text=True, cwd=ROOT)
    print(done.stdout[-4000:])
    if done.returncode != 0:
        print(done.stderr[-1500:])
    check(done.returncode == 0, f"library table tests failed: {done.stdout[-400:]}")


def run_lib_river():
    done = subprocess.run(["cargo", "test", "--release", "--test", "river"], capture_output=True, text=True, cwd=ROOT)
    print(done.stdout[-4000:])
    if done.returncode != 0:
        print(done.stderr[-1500:])
    check(done.returncode == 0, f"library river tests failed: {done.stdout[-400:]}")


def run_none_left():
    port = 8983 + PORT_SHIFT
    srv = server.start(port)
    browser = Browser("noneleft")
    try:
        page = browser.page
        boot_app(page, port)
        inspector = Inspector(page, (390, 844), "light", False)
        # Five new anchors allowed per session, all answered right: then nothing is left.
        open_limited_drill(page, "anchor")
        answered = 0
        while answered < 8:
            page.wait(READY + " || !!document.querySelector('.big-score')", 60)
            if page.eval("!!document.querySelector('.big-score')"):
                break
            answer_drill_item(page, True)
            answered += 1
            click(page, ".action-bar .btn")
        page.wait("!!document.querySelector('.big-score')", 30)
        text = page.eval("document.body.innerText")
        check(answered == 5, f"expected 5 new cards then a stop, answered {answered}")
        check("gone through everything" in text, f"no friendly end message: {text[:300]}")
        check("right" in page.eval("document.querySelector('.big-score').textContent"), "no score on the summary")
        inspector.snapshot("noneleft-summary")
        check(len(rows_of(page)) == 5, f"logged {len(rows_of(page))} rows")
        page.eval("location.hash = '#home'")
        page.wait("!!document.querySelector('.app-title')", 20)
        # Burn this session's new-card allowance on a log where every card has been seen.
        log = page.eval(f"localStorage.getItem({json.dumps(LOG_KEY)})")
        rows = [line for line in log.split("\n") if line and not line.startswith("timestamp")]
        now = int(time.time() * 1000)
        extra = "\n".join(f"{now - 1000 + i}\tdrill\t\tanchor\t{n}\t1\t1\t0\t\t1500\t33" for i, n in enumerate(range(5, 40)))
        page.eval(f"localStorage.setItem({json.dumps(LOG_KEY)}, {json.dumps(log + extra + chr(10))})")
        page.send("Page.reload", {})
        page.wait("!!document.querySelector('.app-title') && !!window.__trainer", 30)
        open_limited_drill(page, "anchor", wait_ready=False)
        must_see(page, "!!document.querySelector('.big-score')", 30, "an empty session did not end with a summary")
        text = page.eval("document.body.innerText")
        check("Nothing is due" in text, f"empty session not explained: {text[:300]}")
        inspector.snapshot("noneleft-empty")
        click(page, ".action-bar .btn")
        page.wait("!!document.querySelector('.app-title')", 20)
        check(not browser.errors, f"none-left errors: {browser.errors[:5]}")
        print("end of deck ok")
    finally:
        browser.close()
        srv.shutdown()


def answer_check_item(page, current, want_ok):
    """Answers the shown checkup item and returns the raw answer used."""
    bundle = current["bundle"]
    fmt = bundle["format"]
    kind = fmt["type"]
    if kind == "choice":
        options = fmt["options"]
        code = options[0]["code"]
        for option in options:
            outcome = bundle["graded"].get(option["code"]) or check_probe(page, current, option["code"])["outcome"]
            if outcome["ok"] == want_ok:
                code = option["code"]
                break
        label = next(o["label"] for o in options if o["code"] == code)
        page.eval(f"[...document.querySelectorAll('.choice')].find(b => b.textContent.endsWith({json.dumps(label)})).click()")
        return code
    if kind == "percent":
        answer = "1"
        for n in range(1, 100):
            outcome = check_probe(page, current, str(n))["outcome"]
            if outcome["correct"] == want_ok:
                answer = str(n)
                break
        answer_percent_keypad(page, answer)
        return answer
    # The mixed checkup also asks for holdings, several picks and counts.
    kind, answer = pick_answer(page, current, want_ok, probe=lambda a: check_probe(page, current, a))
    type_answer(page, current, kind, answer)
    return answer


def play_checkup(page, inspector, want_ok, start_number=1, count=50, shot=None, total=50):
    """Answers `count` items of an open checkup, checking the counter as it goes."""
    answers = []
    for i in range(count):
        page.wait(READY + f" && __trainer.current.number === {start_number + i}", 60)
        current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
        if shot and i == 0:
            inspector.snapshot(shot)
        counter = page.eval("document.querySelector('.top-bar .count').textContent")
        check(counter == f"{start_number + i} of {total}", f"checkup counter {counter} at {start_number + i}")
        check(not page.eval("!!document.querySelector('.reveal')"), "checkup gave feedback before the end")
        answers.append(answer_check_item(page, current, want_ok))
    return answers


def check_checkup_rows(rows, count, where, run, form, slots=None, columns=24):
    """`columns` None allows any row width, as a mixed checkup's skills log differently."""
    check(len(rows) == count, f"{where}: {len(rows)} rows for {count} answers")
    for i, row in enumerate(rows):
        check(row[1] == "check" and (columns is None or len(row) == columns), f"{where}: row {i} mode/columns {row[1]}, {len(row)}")
        check(row[11] == str(run) and row[12] == str(form), f"{where}: run/form {row[11]}/{row[12]}, want {run}/{form}")
        check(int(row[9]) >= 300, f"{where}: response {row[9]}")
        if slots is not None:
            check(row[13] == str(slots[i]), f"{where}: slot {row[13]}, want {slots[i]}")
    slot_list = sorted(int(r[13]) for r in rows)
    check(len(slot_list) == len(set(slot_list)), f"{where}: duplicate slots {slot_list}")


def wait_home(page):
    page.wait("!!document.querySelector('.app-title')", 30)


def run_checkup():
    port = 8984 + PORT_SHIFT
    srv = server.start(port)
    browser = Browser("checkup")
    try:
        page = browser.page
        page.viewport(390, 844, True)
        page.theme("light")
        page.navigate(f"http://127.0.0.1:{port}/")
        page.wait("!!document.querySelector('.app-title') && !!window.__trainer", 30)
        inspector = Inspector(page, (390, 844), "light", False)
        # A week of practice behind them, and no checkup yet: the monthly one is offered.
        week_ago = int(time.time() * 1000) - 86400000 * 10
        seed_log(page, port, drills_only_log([("price", "25"), ("hit", "20"), ("nuts", "AKs")], now=week_ago))
        home = page.eval("document.body.innerText")
        check("Monthly checkup" in home and "Take the monthly checkup" in home and "Take checkup:" not in home, f"checkup not offered on Home: {home[:400]}")
        page.eval("[...document.querySelectorAll('[data-checkup-offers] a')].find(a => a.textContent.includes('monthly')).click()")
        page.wait("!!document.querySelector('.panel .btn.primary')", 20)
        text = page.eval("document.body.innerText")
        check(f"{MIXED_LEN} new questions from across the practice groups" in text, f"checkup intro: {text[:300]}")
        inspector.snapshot("checkup-intro")
        click_text(page, ".btn.primary", "Begin")
        before = len(rows_of(page))
        answers = play_checkup(page, inspector, True, count=MIXED_LEN, total=MIXED_LEN, shot="checkup-item")
        page.wait("!!document.querySelector('.big-score')", 30)
        text = page.eval("document.body.innerText")
        print("mixed checkup result:", text[:500].replace("\n", " | "))
        check("100% right" in text, "mixed checkup score wrong")
        check(f"Scored on all {MIXED_LEN} questions" in text, "mixed checkup scope missing")
        check(page.eval("!!document.querySelector('[data-by-group]')") and "Whole hands" in text and "4 of 4" in text and "8 of 8" in text,
              f"no result by group: {text[:500]}")
        check("Mark met" not in text and "repeated" not in text, f"a mixed checkup has no mark or repeats: {text[:300]}")
        check("first checkup" in text, "first-checkup comparison missing")
        inspector.snapshot("checkup-result")
        rows = rows_of(page)[before:]
        check_checkup_rows(rows, MIXED_LEN, "mixed checkup", "1", "1000000", columns=None)
        counts = {}
        for r in rows:
            counts[r[3]] = counts.get(r[3], 0) + 1
        check(counts == {"preflop": 4, "limpers": 4, "read_price": 7, "equity": 5, "anchor": 2, "chain": 4, "nuts": 1, "possible": 1, "made": 1,
                         "board_type": 1, "beats_me": 2, "outs": 2, "hit": 2}, f"mixed checkup skills (call or fold hidden): {counts}")
        # Taken: Home stops offering it for a month.
        page.eval("location.hash = '#home'")
        wait_home(page)
        check(not page.eval("!!document.querySelector('[data-checkup-offers]')"), "the monthly checkup was offered again straight away")
        # The older one-skill checkup still runs from its address and scores against its mark.
        page.eval("location.hash = '#checkup:price'")
        page.wait("[...document.querySelectorAll('.btn.primary')].some(b => b.textContent === 'Begin')", 20)
        text = page.eval("document.body.innerText")
        check("50 questions" in text and "40" in text and "10 repeats" in text, f"one-skill checkup intro: {text[:300]}")
        click_text(page, ".btn.primary", "Begin")
        before = len(rows_of(page))
        answers = play_checkup(page, inspector, True)
        page.wait("!!document.querySelector('.big-score')", 30)
        text = page.eval("document.body.innerText")
        print("checkup result:", text[:400].replace("\n", " | "))
        check("100% correct" in text, "checkup score wrong")
        check("Mark met" in text, "checkup mark not reported")
        check("margin of error too small" in text.lower(), f"a perfect score needs its margin explained: {text[:300]}")
        check("consistent" in text and "repeated" in text, "repeat consistency missing")
        check("first checkup" in text, "first-checkup comparison missing")
        rows = rows_of(page)[before:]
        check_checkup_rows(rows, 50, "first checkup", "2", "0")
        check([r[3] for r in rows] == ["price"] * 50, "checkup logged the wrong skill")
        check(rows[0][10] == answers[0], f"logged answer {rows[0][10]}, gave {answers[0]}")
        # A second form with wrong answers misses the mark and compares with the first.
        page.eval("location.hash = '#home'")
        wait_home(page)
        page.eval("location.hash = '#checkup:price'")
        page.wait("[...document.querySelectorAll('.btn.primary')].some(b => b.textContent === 'Begin')", 20)
        check("Earlier checkups" in page.eval("document.body.innerText"), "intro lacks history")
        click_text(page, ".btn.primary", "Begin")
        before = len(rows_of(page))
        play_checkup(page, inspector, False)
        page.wait("!!document.querySelector('.big-score')", 30)
        text = page.eval("document.body.innerText")
        print("second result:", text[:400].replace("\n", " | "))
        check("Mark not met" in text, "wrong-answer checkup should miss the mark")
        check("than your last checkup" in text or "same score" in text, f"no comparison: {text}")
        rows = rows_of(page)[before:]
        check_checkup_rows(rows, 50, "second checkup", "3", "1")
        # A full board_type checkup: choices graded as answered, with response time.
        page.eval("location.hash = '#home'")
        wait_home(page)
        page.eval("location.hash = '#checkup:board_type'")
        page.wait("[...document.querySelectorAll('.btn.primary')].some(b => b.textContent === 'Begin')", 20)
        click_text(page, ".btn.primary", "Begin")
        before = len(rows_of(page))
        # A tap landing right after the next question appears must not answer it.
        page.wait(READY + " && __trainer.current.number === 1", 30)
        answer_check_item(page, json.loads(page.eval("JSON.stringify(__trainer.current)")), True)
        fired = page.eval(f"({PHANTOM_TAP})(c => c.number === 2)")
        check(fired, "the phantom tap found no button")
        time.sleep(0.45)
        check(page.eval("__trainer.current.number") == 2, "a tap right after a checkup answer answered the next question")
        check(len(rows_of(page)) == before + 1, "a phantom tap logged a checkup answer")
        play_checkup(page, inspector, True, start_number=2, count=49)
        page.wait("!!document.querySelector('.big-score')", 30)
        text = page.eval("document.body.innerText")
        check("100% correct" in text and "Mark met" in text, f"board checkup result: {text[:300]}")
        check_checkup_rows(rows_of(page)[before:], 50, "board checkup", "4", "0", columns=18)
        # One-skill checkups are never offered on Home.
        page.eval("location.hash = '#home'")
        wait_home(page)
        home = page.eval("document.body.innerText")
        check("Pot odds" not in home and "Take checkup" not in home, f"a one-skill checkup was offered: {home[:300]}")
        # A mixed score gets a real margin of error in plain words.
        t = int(time.time() * 1000) - 3600000
        mixed = [f"{t + slot}\tcheck\t\thit\t{2000 + slot}\t2\t{slot % 2}\t0\t\t1500\t20\t1\t0\t{slot}\t\t\t\t" for slot in range(49)]
        seed_log(page, port, drills_only_log([("hit", "20")]) + "\n".join(mixed) + "\n")
        page.eval("location.hash = '#checkup:hit'")
        page.wait("[...document.querySelectorAll('.btn.primary')].some(b => b.textContent === 'Resume')", 20)
        click_text(page, ".btn.primary", "Resume")
        play_checkup(page, inspector, True, start_number=50, count=1)
        page.wait("!!document.querySelector('.big-score')", 30)
        text = page.eval("document.body.innerText")
        check(re.search(r"bout plus or minus \d+ points", text), f"no margin of error in points: {text[:300]}")
        check(not browser.errors, f"checkup errors: {browser.errors[:5]}")
        print("checkup ok")
    finally:
        browser.close()
        srv.shutdown()


def run_checkup_quit():
    port = 8985 + PORT_SHIFT
    srv = server.start(port)
    browser = Browser("checkupquit")
    try:
        page = browser.page
        page.viewport(390, 844, True)
        page.theme("light")
        page.navigate(f"http://127.0.0.1:{port}/")
        page.wait("!!document.querySelector('.app-title') && !!window.__trainer", 30)
        inspector = Inspector(page, (390, 844), "light", False)
        seed_log(page, port, drills_only_log([("price", "25"), ("hit", "20"), ("nuts", "AKs")], now=int(time.time() * 1000) - 86400000 * 10))
        page.eval("location.hash = '#checkup:mixed'")
        page.wait("[...document.querySelectorAll('.btn.primary')].some(b => b.textContent === 'Begin')", 20)
        click_text(page, ".btn.primary", "Begin")
        before = len(rows_of(page))
        play_checkup(page, inspector, True, start_number=1, count=4, total=MIXED_LEN)
        # Stop partway: the screen must not reveal a score.
        page.wait(READY + " && __trainer.current.number === 5", 30)
        page.eval("document.querySelector('.top-bar button.back').click()")
        must_see(page, "document.body.innerText.includes('Checkup saved')", 20, "stopping a checkup did not reach the saved screen")
        text = page.eval("document.body.innerText")
        check(f"4 of {MIXED_LEN} answered" in text, f"quit screen: {text[:300]}")
        check("correct" not in text and "plus or minus" not in text, f"a quit checkup leaked a score: {text[:300]}")
        inspector.snapshot("checkup-saved")
        # Home and Progress offer to resume, even after a reload.
        page.eval("location.hash = '#home'")
        wait_home(page)
        home = page.eval("document.body.innerText")
        check(f"Resume checkup (4 of {MIXED_LEN})" in home, f"no resume on home: {home[:400]}")
        page.send("Page.reload", {})
        page.wait("!!document.querySelector('.app-title') && !!window.__trainer", 30)
        check(f"Resume checkup (4 of {MIXED_LEN})" in page.eval("document.body.innerText"), "resume lost on reload")
        page.eval("location.hash = '#progress'")
        page.wait("!!document.querySelector('.skill-row')", 20)
        check(f"Resume checkup (4 of {MIXED_LEN})" in page.eval("document.body.innerText"), "resume missing from Progress")
        # Resume: the next question is the fifth, the first four stay as logged.
        page.eval("[...document.querySelectorAll('a.btn')].find(a => a.textContent.includes('Resume checkup')).click()")
        page.wait("[...document.querySelectorAll('.btn.primary')].some(b => b.textContent === 'Resume')", 20)
        check("Start over" in page.eval("document.body.innerText"), "no option to start over")
        click_text(page, ".btn.primary", "Resume")
        page.wait(READY, 30)
        check(page.eval("document.querySelector('.top-bar .count').textContent") == f"5 of {MIXED_LEN}", "resume did not continue at question 5")
        # Leave by the back gesture partway, then resume again and finish.
        page.eval("location.hash = '#home'")
        wait_home(page)
        check(f"Resume checkup (4 of {MIXED_LEN})" in page.eval("document.body.innerText"), "back gesture lost the checkup")
        page.eval("location.hash = '#checkup:mixed'")
        must_see(page, "[...document.querySelectorAll('.btn.primary')].some(b => b.textContent === 'Resume')", 20, "after the back gesture the checkup was not offered to resume")
        click_text(page, ".btn.primary", "Resume")
        answers = play_checkup(page, inspector, True, start_number=5, count=MIXED_LEN - 4, total=MIXED_LEN)
        page.wait("!!document.querySelector('.big-score')", 30)
        check("100% right" in page.eval("document.body.innerText"), "resumed checkup score wrong")
        rows = rows_of(page)[before:]
        check_checkup_rows(rows, MIXED_LEN, "resumed checkup", "1", "1000000", columns=None)
        check(sorted(int(r[13]) for r in rows) == list(range(MIXED_LEN)), f"slots not 0-{MIXED_LEN - 1}: {sorted(int(r[13]) for r in rows)}")
        results["checkup_resumed"] = MIXED_LEN
        # Starting over leaves the old run and begins a new form; the new run is resumable.
        page.eval("location.hash = '#home'")
        wait_home(page)
        page.eval("location.hash = '#checkup:mixed'")
        page.wait("[...document.querySelectorAll('.btn.primary')].some(b => b.textContent === 'Begin')", 20)
        click_text(page, ".btn.primary", "Begin")
        page.wait(READY, 30)
        answer_check_item(page, json.loads(page.eval("JSON.stringify(__trainer.current)")), True)
        page.wait(READY + " && __trainer.current.number === 2", 30)
        page.eval("document.querySelector('.top-bar button.back').click()")
        page.wait("document.body.innerText.includes('Checkup saved')", 20)
        page.eval("location.hash = '#home'")
        wait_home(page)
        page.eval("location.hash = '#checkup:mixed'")
        page.wait("[...document.querySelectorAll('.btn.primary')].some(b => b.textContent === 'Resume')", 20)
        click_text(page, ".btn", "Start over with new questions")
        page.wait(READY, 30)
        check(page.eval("document.querySelector('.top-bar .count').textContent") == f"1 of {MIXED_LEN}", "starting over did not reset to question 1")
        current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
        check(current["form"] == 1000002 and current["run"] == 3, f"start over used form {current['form']} run {current['run']}")
        check(not browser.errors, f"checkup quit errors: {browser.errors[:5]}")
        print("checkup quit and resume ok")
    finally:
        browser.close()
        srv.shutdown()


def run_checkup_tabs():
    """Two tabs answering the same slot: the second answer must not be logged."""
    port = 8986 + PORT_SHIFT
    srv = server.start(port)
    browser = Browser("checkuptabs")
    try:
        first = browser.page
        boot_app(first, port)
        seed_log(first, port, drills_only_log([("price", "25")]))
        # Both tabs open the monthly checkup.
        second = browser.chrome.new_page()
        boot_app(second, port)
        first.send("Emulation.setFocusEmulationEnabled", {"enabled": True})
        second.send("Emulation.setFocusEmulationEnabled", {"enabled": True})
        for page in (first, second):
            page.send("Page.bringToFront")
            page.eval("location.hash = '#checkup:mixed'")
            page.wait("[...document.querySelectorAll('.btn.primary')].some(b => b.textContent === 'Begin')", 20)
            click_text(page, ".btn.primary", "Begin")
            page.wait(READY, 30)
        # Both tabs are on slot 0 of the same run and form.
        first.send("Page.bringToFront")
        answer_check_item(first, json.loads(first.eval("JSON.stringify(__trainer.current)")), True)
        first.wait(READY + " && __trainer.current.number === 2", 30)
        second.send("Page.bringToFront")
        answer_check_item(second, json.loads(second.eval("JSON.stringify(__trainer.current)")), True)
        second.wait(READY + " && __trainer.current.number === 2", 30)
        rows = rows_of(first)
        check(all(len(r) >= 11 for r in rows), f"a malformed log line: {[r for r in rows if len(r) < 11][:2]}")
        checks = [r for r in rows if len(r) > 1 and r[1] == "check"]
        check(len(checks) == 1, f"two tabs logged {len(checks)} rows for one slot: {[r[13] for r in checks]}")
        check(not browser.errors, f"checkup tab errors: {browser.errors[:5]}")
        print("checkup two tabs ok")
    finally:
        browser.close()
        srv.shutdown()


def showcase_lines():
    """A log with equity bias, board categories, an anchor box spread and one checkup."""
    t = int(time.time() * 1000) - 86400000
    lines = [LOG_COLUMNS]
    for i in range(40):
        t += 1000
        opponents = 1 if i < 20 else 2
        error = 4.2 if i < 20 else 1.0
        lines.append(f"{t}\tdrill\t\tequity\t{4611686018427388904 + i}\t1\t1\t0\t\t1500\t54\t\t\t\t8\t{error:.1f}\t50.0\t{opponents}")
    for i in range(10):
        t += 1000
        lines.append(f"{t}\tdrill\t\tboard_type\t{(i % 8) << 56}\t1\t{1 if i < 8 else 0}\t0\t\t1500\ttd")
    for i in range(10):
        t += 1000
        lines.append(f"{t}\tdrill\t\tboard_type\t{i << 56}\t1\t1\t0\t\t1500\ttd")
    for slot in range(50):
        t += 1000
        lines.append(f"{t}\tcheck\t\tprice\t{2000 + slot}\t2\t1\t0\t\t1500\t25\t1\t0\t{slot}\t\t\t\t")
    for i in range(45):
        t += 1000
        lines.append(f"{t}\tdrill\t\tanchor\t{i % 40}\t1\t1\t0\t\t1500\t33")
    return lines


def run_checkup_conflict():
    """Two tabs start different kinds of checkup together and both get the same run number."""
    port = 8988 + PORT_SHIFT
    srv = server.start(port)
    browser = Browser("checkupconflict")
    try:
        first = browser.page
        boot_app(first, port)
        seed_log(first, port, drills_only_log([("price", "25"), ("hit", "20")]))
        second = browser.chrome.new_page()
        boot_app(second, port)
        first.send("Emulation.setFocusEmulationEnabled", {"enabled": True})
        second.send("Emulation.setFocusEmulationEnabled", {"enabled": True})
        for page, skill in ((first, "mixed"), (second, "hit")):
            page.send("Page.bringToFront")
            page.eval(f"location.hash = '#checkup:{skill}'")
            page.wait("[...document.querySelectorAll('.btn.primary')].some(b => b.textContent === 'Begin')", 20)
            click_text(page, ".btn.primary", "Begin")
            page.wait(READY, 30)
        check(first.eval("__trainer.current.run") == second.eval("__trainer.current.run"), "the tabs should start on the same run for this test")
        first.send("Page.bringToFront")
        answer_check_item(first, json.loads(first.eval("JSON.stringify(__trainer.current)")), True)
        first.wait(READY + " && __trainer.current.number === 2", 30)
        second.send("Page.bringToFront")
        answer_check_item(second, json.loads(second.eval("JSON.stringify(__trainer.current)")), True)
        must_see(second, READY + " && __trainer.current.number === 2", 30, "the second checkup did not continue after a run clash")
        current = json.loads(second.eval("JSON.stringify(__trainer.current)"))
        check(current["run"] == 2 and current["form"] == 0 and current["checkSlot"] == 1, f"the second tab lost its place: {current['run']}/{current['form']}/{current['checkSlot']}")
        answer_check_item(second, current, True)
        second.wait(READY + " && __trainer.current.number === 3", 30)
        rows = [r for r in rows_of(first) if len(r) > 1 and r[1] == "check"]
        check([(r[11], r[12], r[13]) for r in rows] == [("1", "1000000", "0"), ("2", "0", "0"), ("2", "0", "1")]
              and [r[3] for r in rows[1:]] == ["hit", "hit"],
              f"run clash left the wrong rows: {[(r[3], r[11], r[12], r[13]) for r in rows]}")
        check(not browser.errors, f"checkup conflict errors: {browser.errors[:5]}")
        print("checkup run clash ok")
    finally:
        browser.close()
        srv.shutdown()


def run_progress_new():
    """Progress and Home show drill and checkup state, bias, band, boxes and categories."""
    port = 8987 + PORT_SHIFT
    srv = server.start(port)
    browser = Browser("progressnew")
    try:
        page = browser.page
        page.viewport(390, 844, True)
        page.theme("light")
        page.navigate(f"http://127.0.0.1:{port}/")
        page.wait("!!document.querySelector('.app-title') && !!window.__trainer", 30)
        inspector = Inspector(page, (390, 844), "light", False)
        # Before any data: the "not enough data" states.
        page.eval("location.hash = '#progress'")
        page.wait("!!document.querySelector('.skill-row')", 20)
        text = page.eval("document.body.innerText")
        check(text.count("Not enough data") >= 4, f"expected not-enough-data states: {text.count('Not enough data')}")
        check("win rate" not in text.lower(), "progress mentions a win rate")
        inspector.snapshot("progress-empty")
        # With data: equity biased +4 points, heads-up (1 opponent) and three-way (2 opponents).
        seed_log(page, port, "\n".join(showcase_lines()) + "\n")
        page.eval("location.hash = '#progress'")
        page.wait("!!document.querySelector('.skill-row')", 20)
        text = page.eval("document.body.innerText")
        print("progress text:", text[:1200].replace("\n", " | "))
        check("overestimate heads-up pots" in text and "about 4 points" in text, f"bias sentence missing: {text}")
        check("Three-way pots" in text, "bias table by opponents missing")
        check("Hand types: within" in text and "Exact hands: within" in text, "staircase band missing")
        check("Checkup history" in text and "Pot odds" in text, "checkup history missing")
        anchor_row = page.eval("[...document.querySelectorAll('.skill-row')].find(r => r.textContent.includes('Key numbers')).innerText")
        check("Equity and anchors" in anchor_row and "Passed" in anchor_row, f"group and pass state missing: {anchor_row}")
        check("Not yet" in text and "Checkup passed" not in text and "Take checkup" not in text, "per-skill checkup state still shown")
        check("rainbow, connected" in text, "per-category table missing")
        check("Box 1" in text and "cards seen" in text, "anchor boxes missing")
        check(not page.eval("!!document.querySelector('svg.chart, canvas')"), "progress must have no charts")
        check("win rate" not in text.lower(), "progress mentions a win rate")
        inspector.snapshot("progress-data")
        page.eval("location.hash = '#home'")
        wait_home(page)
        home = page.eval("document.body.innerText")
        check("Board reading" in home and "Equity and anchors" in home and "Step " not in home, f"home group list: {home[:400]}")
        check(not browser.errors, f"progress errors: {browser.errors[:5]}")
        print("progress ok")
    finally:
        browser.close()
        srv.shutdown()


def run_card():
    srv = server.start(8955 + PORT_SHIFT)
    browser = Browser("card")
    try:
        page = browser.page
        boot_app(page, 8955 + PORT_SHIFT)
        inspector = Inspector(page, (390, 844), "light", False)
        page.eval("location.hash = '#card'")
        page.wait("document.querySelectorAll('.cell').length === 169", 20)
        opening = page.eval("document.querySelector('.tabs .tab[aria-pressed=true]').textContent")
        check(opening == "Cutoff", f"card opened on {opening}")
        seats = page.eval("[...document.querySelectorAll('.tabs')[0].children].length")
        check(seats == 6, f"{seats} seat tabs")
        tab_count = page.eval("[...document.querySelectorAll('.tabs')[1].children].length")
        check(tab_count == 7, f"{tab_count} situation tabs")
        seen = 0
        for s in range(6):
            for k in range(tab_count):
                page.eval(f"document.querySelectorAll('.tabs')[0].children[{s}].click(); document.querySelectorAll('.tabs')[1].children[{k}].click();")
                time.sleep(0.05)
                page.wait("document.querySelectorAll('.cell').length === 169", 5)
                inspector.snapshot(f"card-{s}-{k}")
                seen += 1
                if not (s == 5 and k == 0):
                    check(page.eval("/% of hands/.test(document.body.innerText)"), f"no percent on card view {s},{k}")
        text = page.eval("document.body.innerText")
        check("Notes" in text and "iso." not in text, "card notes")
        check_card_charts(page, inspector)
        results["card_views"] = seen
        check(not browser.errors, f"card errors {browser.errors[:3]}")
    finally:
        browser.close()
        srv.shutdown()


def check_card_charts(page, inspector):
    """The new charts are legible, and the seat tabs go away when the seat doesn't matter."""
    seat_group_visible = "getComputedStyle(document.querySelectorAll('.tabs')[0]).display !== 'none'"
    expected = {3: ("A raise and callers", "3-bet", True), 4: ("You opened and got 3-bet, in position", "4-bet", False),
                5: ("You opened and got 3-bet, out of position", "4-bet", False), 6: ("You 3-bet and got 4-bet", "All in", False)}
    for k, (title, word, seat_visible) in expected.items():
        page.eval(f"document.querySelectorAll('.tabs')[1].children[{k}].click();")
        must_see(page, f"document.body.innerText.includes({json.dumps(title)}) && document.querySelectorAll('.cell').length === 169", 5, f"card chart title {title} missing")
        check(page.eval(seat_group_visible) == seat_visible, f"seat tabs at situation {k}")
        check(page.eval(f"[...document.querySelectorAll('.legend li')].some(l => l.textContent.includes({json.dumps(word)}))"),
              f"legend misses {word} at situation {k}")
        # Facing a 3-bet or a 4-bet nothing calls, so the chart shows only its raise line.
        if k >= 4:
            check(page.eval("document.querySelectorAll('.range-lines h3').length") == 1, f"empty call line shown at situation {k}")
        inspector.snapshot(f"card-chart-{k}")
    # The callers chart adds two calls in the big blind.
    page.eval("document.querySelectorAll('.tabs')[1].children[3].click(); document.querySelectorAll('.tabs')[0].children[5].click();")
    must_see(page, "document.body.innerText.includes('A raise and callers, big blind: also call')", 5, "callers big blind line missing")
    # A lone raise depends on your seat again.
    page.eval("document.querySelectorAll('.tabs')[1].children[2].click();")
    page.wait("document.body.innerText.includes('One raise')", 5)
    check(page.eval(seat_group_visible), "seat tabs for a lone raise")
    page.eval("document.querySelectorAll('.tabs')[0].children[5].click();")
    page.wait("document.body.innerText.includes('One raise, from the big blind')", 5)
    check("3-bet" in page.eval("document.querySelector('.legend').textContent"), "legend for a lone raise")
    page.eval("document.querySelectorAll('.tabs')[1].children[0].click();")


def run_offline():
    port = 8956 + PORT_SHIFT
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
    port = 8957 + PORT_SHIFT
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
    port = 8958 + PORT_SHIFT
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
    port = 8960 + PORT_SHIFT
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
    for tab, name in [(3, "card-callers"), (4, "card-three-bet-ip"), (5, "card-three-bet-oop"), (6, "card-four-bet")]:
        page.eval(f"document.querySelectorAll('.tabs')[1].children[{tab}].click()")
        time.sleep(0.1)
        inspector.snapshot(name, full=True)
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
    capture_new_screens(page, port, inspector)


def capture_new_screens(page, port, inspector):
    drills, _ = synthetic_drills([("hit", "20"), ("price", "25")])
    log = "\n".join(showcase_lines() + drills) + "\n"
    seed_log(page, port, log)
    inspector.snapshot("home-with-offers", full=True)
    page.eval("location.hash = '#progress'")
    page.wait("!!document.querySelector('.skill-row')", 20)
    inspector.snapshot("progress-with-data", full=True)
    page.eval("location.hash = '#home'")
    wait_home(page)
    page.eval("location.hash = '#checkup:mixed'")
    page.wait("[...document.querySelectorAll('.btn.primary')].some(b => b.textContent === 'Begin')", 20)
    inspector.snapshot("checkup-intro")
    click_text(page, ".btn.primary", "Begin")
    page.wait(READY, 30)
    answer_check_item(page, json.loads(page.eval("JSON.stringify(__trainer.current)")), True)
    page.wait(READY + " && __trainer.current.number === 2", 30)
    inspector.snapshot("checkup-item")
    page.eval("document.querySelector('.top-bar button.back').click()")
    page.wait("document.body.innerText.includes('Checkup saved')", 20)
    inspector.snapshot("checkup-saved")
    page.eval("location.hash = '#home'")
    wait_home(page)
    page.eval("location.hash = '#checkup:mixed'")
    page.wait("[...document.querySelectorAll('.btn.primary')].some(b => b.textContent === 'Resume')", 20)
    click_text(page, ".btn.primary", "Resume")
    play_checkup(page, inspector, True, start_number=2, count=MIXED_LEN - 1, total=MIXED_LEN)
    page.wait("!!document.querySelector('.big-score')", 30)
    inspector.snapshot("checkup-result")
    page.eval("location.hash = '#home'")
    wait_home(page)
    # An equity spot at its first level, and a matchup anchor card.
    seed_log(page, port, LOG_COLUMNS + "\n")
    open_limited_drill(page, "equity")
    page.wait(READY, 60)
    inspector.snapshot("equity-rung0", full=True)
    page.eval("location.hash = '#home'")
    wait_home(page)
    t = int(time.time() * 1000) - 60000
    seen = "\n".join(f"{t + i}\tdrill\t\tanchor\t{i}\t1\t1\t0\t\t1500\t33" for i in range(8))
    seed_log(page, port, LOG_COLUMNS + "\n" + seen + "\n")
    open_limited_drill(page, "anchor")
    page.wait(READY, 60)
    inspector.snapshot("anchor-matchup", full=True)
    # Table spots: a question and a depends reveal in this theme.
    page.eval("location.hash = '#home'")
    wait_home(page)
    seed_log(page, port, LOG_COLUMNS + "\n")
    open_limited_drill(page, "table")
    page.wait(READY, 60)
    inspector.snapshot("table-question", full=True)
    for attempt in range(40):
        current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
        outcomes = table_outcomes(page, current["bundle"])
        if next(iter(outcomes.values()))["reveal"]["verdict"] == "depends":
            type_answer(page, current, "choice", next(iter(outcomes)))
            page.wait("!!document.querySelector('.reveal')", 30)
            inspector.snapshot("table-reveal-depends", full=True)
            break
        type_answer(page, current, "choice", next(iter(outcomes)))
        page.wait("!!document.querySelector('.reveal')", 30)
        click(page, ".action-bar .btn")
        page.wait(READY, 30)



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
    port = 8971 + PORT_SHIFT
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
        for i in range(32):
            page.wait(READY, 60)
            counter = page.eval("document.querySelector('.top-bar .count').textContent")
            check(counter == f"{i + 1} of 32", f"phantom tap skipped a question: {counter} at {i}")
            current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
            answers.append((current["bundle"]["skill"], answer_baseline_item(page, current)))
            if i < 31:
                fired += bool(page.eval(f"({PHANTOM_TAP})(c => c.baselineIndex === {i + 1})"))
        page.wait("!!document.querySelector('.big-score')", 60)
        check(fired >= 13, f"only {fired} phantom taps landed")
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
    port = 8972 + PORT_SHIFT
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
    port = 8973 + PORT_SHIFT
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
    race_port = 8977 + PORT_SHIFT
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
        # Let the drill's leftover look-ahead work drain first: the default drill's next
        # items can be slow ones, and they must not use up the passes meant for the baseline.
        page.eval("__trainer.prepare({ skill: 'nuts', seed: '12345', origin: 'new' }, false).then(() => true)")
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
    port = 8974 + PORT_SHIFT
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
    crash_port = 8979 + PORT_SHIFT
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
        must_see(page, "document.body.innerText.includes('could not be prepared')", 15, "no recovery screen after the prep worker crashed")
        check(page.eval("window.__workers.length") == 3, "the prep worker was not replaced")
        print("prep failure ok")
    finally:
        browser.close()
        srv.shutdown()


def run_quota():
    port = 8980 + PORT_SHIFT
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
    port = 8981 + PORT_SHIFT
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
    port = 8975 + PORT_SHIFT
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
    port = 8976 + PORT_SHIFT
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

def run_preflop_spots():
    """Every new situation is dealt in the real drill, gets its one-line why, and logs correctly."""
    port = 8983 + PORT_SHIFT
    srv = server.start(port)
    browser = Browser("preflopspots")
    try:
        page = browser.page
        page.viewport(390, 844, True)
        page.theme("light")
        page.navigate(f"http://127.0.0.1:{port}/")
        page.wait("!!document.querySelector('.app-title') && !!window.__trainer", 30)
        inspector = Inspector(page, (390, 844), "light", False)
        open_limited_drill(page, "preflop")
        wanted = {"callers": "called", "three_bet": "got 3-bet", "four_bet": "got 4-bet"}
        seen = set()
        # 3-bet and 4-bet spots are rare (about 4% and 1.5% of preflop items), so allow many items.
        for step in range(400):
            page.wait(READY, 30)
            current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
            situation = current["bundle"]["question"]["situation"]
            sentence = page.eval("document.querySelector('.prompt').textContent")
            if situation in wanted:
                check(wanted[situation] in sentence, f"{situation} prompt read {sentence!r}")
                check("Still to act" not in page.eval("document.querySelector('.chips').innerText") or situation == "callers",
                      f"{situation} shows a still-to-act chip")
            kind, answer = pick_answer(page, current, step % 3 != 0)
            type_answer(page, current, kind, answer)
            page.wait("!!document.querySelector('.reveal')", 30)
            reveal_text = page.eval("document.querySelector('.reveal').innerText")
            inspector.snapshot(f"preflop-{situation}", full=True)
            if situation in wanted or situation == "raised":
                check(page.eval("!!document.querySelector('.reveal .hint')"), f"{situation} reveal has no chart line: {reveal_text}")
                check("Fold the rest" in reveal_text, f"{situation} reveal has no chart: {reveal_text}")
                check(not re.search(r"\bbook\b|\bauthor\b|\bchapter\b|according to|\bsolver\b|%", reveal_text.lower()), f"{situation} reveal cites a source: {reveal_text}")
                seen.add(situation)
            if seen >= set(wanted) | {"raised"}:
                break
            next_drill_item(page)
        check(seen >= set(wanted) | {"raised"}, f"preflop drill never reached all spots: saw {sorted(seen)}")
        results["preflop_spots_seen"] = sorted(seen)
        check(not browser.errors, f"preflop spot errors {browser.errors[:3]}")
    finally:
        browser.close()
        srv.shutdown()


# --- the grade is always about the question on screen --------------------------
VISIBLE_AGAIN = """(() => { Object.defineProperty(document, 'hidden', { configurable: true, value: false }); document.dispatchEvent(new Event('visibilitychange')); return 1; })()"""

# Records every grade reply, so a test can hold it against the question shown.
GRADE_HOOK = """(() => {
  const e = __trainer.engine; const call = e.call;
  window.__grades = [];
  e.call = (...args) => { const p = call(...args); if (/grade$/.test(args[0])) p.then(r => __grades.push({ args, r })); return p; };
  return 1; })()"""

# Makes every worker but the first (the engine) deal a different item than asked
# for, `__tamper` times: a prep worker that disagrees with the engine.
TAMPER_PREP = """
(() => {
  const Original = window.Worker;
  window.__workers = [];
  window.__tamper = 0;
  window.Worker = function (...args) { const w = new Original(...args); window.__workers.push(w); return w; };
  window.Worker.prototype = Original.prototype;
  const post = Original.prototype.postMessage;
  Original.prototype.postMessage = function (message, ...rest) {
    if (this !== window.__workers[0] && window.__tamper > 0 && message && message.args && message.args[0] === 'prepare') {
      window.__tamper -= 1;
      const args = message.args.slice();
      args[2] = String(BigInt(args[2]) ^ 4n);
      message = { ...message, args };
    }
    return post.call(this, message, ...rest);
  };
})();
"""


def shown_cards(question):
    cards = list(question.get("board", []))
    for key in ("hero", "villain", "hand"):
        cards += question.get(key) or []
    return set(cards)


def check_grade_matches(page, bundle, where):
    """The last grade reply is about the question shown: same fingerprint, and an
    outs list that holds none of the cards on screen."""
    grades = json.loads(page.eval("JSON.stringify(__grades)"))
    mine = [g for g in grades if g["r"].get("outcome")]
    if not mine:
        return  # graded from the bundle itself
    reply = mine[-1]["r"]
    check(reply.get("check") == bundle.get("check"), f"{where}: graded {reply.get('check')} but shown {bundle.get('check')}")
    outs = reply["outcome"]["reveal"].get("outs")
    if outs is not None:
        clash = set(outs) & shown_cards(bundle["question"])
        check(not clash, f"{where}: outs {sorted(clash)} are cards on the screen")


def run_same_build():
    """A worker started after a new release is published still runs the page's
    build, and a grade about another question is never shown."""
    port = 8962 + PORT_SHIFT
    srv = server.start(port)
    older = OUT / "older-trainer.wasm"
    shown = subprocess.run(["git", "show", "0e2dd11:app/trainer.wasm"], capture_output=True, cwd=ROOT)
    if shown.returncode == 0 and shown.stdout[:4] == b"\0asm":
        older.write_bytes(shown.stdout)  # the release before the whole-hand deal changed
    else:
        older = OUT / "no-such-trainer.wasm"  # without history: a release that can't be fetched
    browser = Browser("samebuild")
    try:
        page = browser.page
        boot_app(page, port)
        # A new release goes up, then the app comes back to the foreground, which
        # replaces the prep worker.
        srv.wasm_override = older
        page.eval(VISIBLE_AGAIN)
        page.eval(GRADE_HOOK)
        page.eval("[...document.querySelectorAll('[data-group]')].find(b => b.dataset.group === 'hands').click()")
        outs_seen = 0
        for i in range(8):
            page.wait(READY, 60)
            if i % 3 == 1:
                page.eval(VISIBLE_AGAIN)
            current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
            bundle = current["bundle"]
            truth = page.eval(f"__trainer.engine.call('prepare', {json.dumps(bundle['skill'])}, {json.dumps(bundle['seed'])}, 'new').then(b => JSON.stringify(b.question))")
            check(json.loads(truth) == bundle["question"], f"item {i}: the question shown is not the engine's {bundle['skill']} {bundle['seed']}")
            outs_seen += bundle["question"].get("stage") == "outs"
            page.eval("__grades.length = 0")
            kind, answer = pick_answer(page, current, False)
            type_answer(page, current, kind, answer)
            page.wait("!!document.querySelector('.reveal')", 30)
            check_grade_matches(page, bundle, f"after a restart, item {i}")
            check("out of date" not in page.eval("document.body.innerText"), f"item {i}: the page and its workers disagreed")
            click(page, ".action-bar .btn")
            page.wait("!document.querySelector('.reveal')", 30)
        check(outs_seen >= 2, f"only {outs_seen} outs questions were dealt")
        check(not browser.errors, f"same build errors: {browser.errors[:5]}")
    finally:
        srv.wasm_override = None
        browser.close()
        srv.shutdown()

    # Whatever the cause, a grade about another question is refused: the item is
    # dealt again by the grading engine and only the answer to it is kept.
    port = 8963 + PORT_SHIFT
    srv = server.start(port)
    browser = Browser("tamper")
    try:
        page = browser.page
        page.send("Page.addScriptToEvaluateOnNewDocument", {"source": TAMPER_PREP})
        for skill in ("outs", "chain"):
            # A fresh page: once it has caught a mismatch, a page deals from the engine.
            boot_app(page, port)
            page.eval(GRADE_HOOK)
            rows_before = len(rows_of(page))
            page.eval("window.__tamper = 1000")
            open_limited_drill(page, skill)
            current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
            if skill == "chain":
                # The first stage is graded from its own bundle; the second goes to the engine.
                kind, answer = pick_answer(page, current, True)
                type_answer(page, current, kind, answer)
                page.wait("!!document.querySelector('.reveal')", 30)
                wait_rows(page, rows_before + 1)
                click(page, ".action-bar .btn")
                page.wait(READY, 60)
                rows_before = len(rows_of(page))
                current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
            stale = current["bundle"]
            check(stale["seed"] != current["planned"]["seed"], f"{skill}: the tampered item was not dealt")
            page.eval("__grades.length = 0")
            for ch in "3":
                click_text(page, ".keypad .key", ch)
            click(page, ".answers .btn.primary")
            page.wait("document.body.innerText.includes('out of date')", 30)
            page.wait(READY, 30)
            check(not page.eval("!!document.querySelector('.reveal')"), f"{skill}: a reveal about another question was shown")
            fresh = json.loads(page.eval("JSON.stringify(__trainer.current)"))
            check(fresh["bundle"]["seed"] == fresh["planned"]["seed"], f"{skill}: the item was not dealt again")
            check(fresh["bundle"]["check"] != stale["check"], f"{skill}: the same wrong item came back")
            check(len(rows_of(page)) == rows_before, f"{skill}: the answer to the wrong question was saved")
            page.eval("__grades.length = 0")
            kind, answer = pick_answer(page, fresh, False)
            type_answer(page, fresh, kind, answer)
            page.wait("!!document.querySelector('.reveal')", 30)
            check_grade_matches(page, fresh["bundle"], f"{skill} dealt again")
            wait_rows(page, rows_before + 1)
            page.eval("document.querySelector('.top-bar button.back').click()")
            page.wait("!!document.querySelector('.big-score')", 20)
        check(not browser.errors, f"tamper errors: {browser.errors[:5]}")
        print("same build ok")
    finally:
        browser.close()
        srv.shutdown()


# --- every screen fits a phone ----------------------------------------------------
FIT_JS = """(() => {
  const vw = window.innerWidth, vh = window.innerHeight;
  const wide = [...document.querySelectorAll('body *')].filter(e => {
    const r = e.getBoundingClientRect(); return r.width > 0 && r.right > vw + 0.5 && !e.closest('.tabs, .live-slot');
  }).slice(0, 3).map(e => e.tagName + '.' + e.className);
  const bar = document.querySelector('.action-bar');
  let covered = [];
  if (bar && getComputedStyle(bar).position === 'sticky') {
    const top = bar.getBoundingClientRect().top;
    covered = [...document.querySelectorAll('.reveal *, .question *, .reflect *')].filter(e => {
      if (e.children.length || !e.textContent.trim() || e.closest('details:not([open]) > :not(summary)')) return false;
      const r = e.getBoundingClientRect(); return r.height > 0 && r.bottom > top + 1 && r.top < vh;
    }).slice(0, 3).map(e => e.textContent.trim().slice(0, 30));
  }
  return JSON.stringify({ scrollWidth: document.documentElement.scrollWidth, vw, vh, height: document.documentElement.scrollHeight, wide, covered });
})()"""

# Drills whose question and feedback must fit a 390 x 844 screen without scrolling.
# Table spots run longer; there Next follows the feedback instead of covering it.
# River questions must fit as well; River feedback usually does (see run_river).
FIT_SKILLS = ["preflop", "limpers", "nuts", "possible", "made", "beats_me", "read_price", "outs", "hit", "board_type", "equity", "anchor", "chain"]


def check_fit(page, where, width, must_fit):
    info = json.loads(page.eval(FIT_JS))
    check(info["scrollWidth"] <= width and not info["wide"], f"{where}: wider than the screen ({info['scrollWidth']}px): {info['wide']}")
    check(info["vw"] == width, f"{where}: the page was laid out {info['vw']}px wide")
    check(not info["covered"], f"{where}: Next covers {info['covered']}")
    if must_fit:
        check(info["height"] <= info["vh"], f"{where}: {info['height']}px tall on a {info['vh']}px screen")
    return info


def run_phone_fit():
    port = 8964 + PORT_SHIFT
    srv = server.start(port)
    browser = Browser("phonefit")
    try:
        page = browser.page
        tall = {}
        for width, height in ((375, 667), (390, 844)):
            fits = width == 390
            boot_app(page, port, viewport=(width, height, True))
            page.eval("localStorage.clear()")
            boot_app(page, port, viewport=(width, height, True))
            check_fit(page, f"{width} home", width, False)
            # One session through every drill: the width set by a long screen must
            # not carry over to the next (on iOS it would stay).
            for skill in FIT_SKILLS + ["table", "river"]:
                page.eval("location.hash = '#home'")
                page.wait("!!document.querySelector('.app-title')", 20)
                if skill == "river":
                    start_river(page)  # its own entry in the picker
                else:
                    open_limited_drill(page, skill)
                for i in range({"outs": 4, "chain": 4, "beats_me": 4, "table": 4, "river": 6}.get(skill, 2)):
                    page.wait(READY, 60)
                    current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
                    stage = current["bundle"]["question"].get("stage", "")
                    where = f"{width} {skill}{(' ' + stage) if stage else ''} {i}"
                    check_fit(page, f"{where} question", width, fits and skill != "table")
                    kind, answer = pick_answer(page, current, False)
                    type_answer(page, current, kind, answer)
                    page.wait("!!document.querySelector('.reveal')", 30)
                    time.sleep(0.1)
                    # River feedback should fit too, but a long explanation may run a little
                    # past the screen; there Next follows it (checked by check_fit).
                    info = check_fit(page, f"{where} feedback", width, fits and skill not in ("table", "river"))
                    tall[where] = info["height"]
                    # Opening every "Show all" keeps the width and the bar clear.
                    page.eval("document.querySelectorAll('.reveal details').forEach(d => { d.open = true; })")
                    time.sleep(0.05)
                    check_fit(page, f"{where} feedback opened", width, False)
                    click(page, ".action-bar .btn")
                    page.wait("!document.querySelector('.reveal')", 30)
                page.eval("document.querySelector('.top-bar button.back').click()")
                page.wait("!!document.querySelector('.big-score')", 20)
                check_fit(page, f"{width} {skill} summary", width, False)
            for screen_hash in ("#progress", "#card", "#backup", "#checkup:mixed", "#baseline"):
                page.eval(f"location.hash = '{screen_hash}'")
                time.sleep(0.6)
                check_fit(page, f"{width} {screen_hash}", width, False)
        print("phone fit ok; tallest feedback:", sorted(tall.items(), key=lambda kv: -kv[1])[:3])
        check(not browser.errors, f"phone fit errors: {browser.errors[:5]}")
    finally:
        browser.close()
        srv.shutdown()


# --- hidden drills ------------------------------------------------------------------
def spot_history(count=30):
    """Drill rows for both call-or-fold drills, as a player who answered them has."""
    lines = []
    for i in range(count):
        skill = "spot" if i % 2 else "spot_mw"
        lines.append(f"{1_700_000_000_000 + i * 60_000}\tdrill\t\t{skill}\t{1000 + i}\t6\t{i % 3 != 0:d}\t0\t\t4000\tc")
    return "\n".join(lines) + "\n"


def run_hidden_drills():
    """'Call or fold' is switched off: nothing deals it, yet its history stays."""
    port = 8965 + PORT_SHIFT
    srv = server.start(port)
    browser = Browser("hidden-drills")
    try:
        page = browser.page
        boot_app(page, port)
        seed_log(page, port, spot_history())
        home = page.eval("document.body.innerText")
        check("Call or fold" not in page.eval("[...document.querySelectorAll('[data-group]')].map(b => b.title).join(' ')"), "the picker still offers call or fold")
        check("Spots" in home, f"the spots group (table spots) disappeared: {home[:300]}")
        dealt = page.eval("""(async () => {
          const seen = [];
          for (const group of ['', 'group:spots']) {
            await __trainer.engine.call('session_open', '99', Date.now(), group);
            await __trainer.engine.call('session_start', '60');
            for (let i = 0; i < 60; i++) {
              const next = await __trainer.engine.call('session_next', 0);
              if (next.finished) break;
              seen.push(next.planned.skill);
            }
          }
          return seen.join(',');
        })()""", timeout=120).split(",")
        check(len(dealt) >= 60 and not {"spot", "spot_mw"} & set(dealt), f"call or fold was dealt: {sorted(set(dealt))}")
        check("table" in dealt, "the spots group deals no table spots")
        page.eval("location.hash = '#progress'")
        page.wait("document.body.innerText.includes('Call or fold')", 20)
        check(len(rows_of(page)) == 30, "saved call-or-fold answers were lost")
        check(not browser.errors, f"hidden drill errors: {browser.errors[:5]}")
        print("hidden drills ok")
    finally:
        browser.close()
        srv.shutdown()


def run_leaks(leak_file=None, saved_log=None):
    """Preflop mistakes: says how to load when empty, loads a #LEAKS v1 file from the
    Files picker or pasted text (on this device only), drills only the preflop
    leaks, and clears cleanly. `leak_file` runs it on a real file instead."""
    port = 8992 + PORT_SHIFT
    srv = server.start(port)
    browser = Browser("leaks")
    try:
        page = browser.page
        boot_app(page, port, viewport=(390, 844, True))
        if saved_log:
            seed_log(page, port, saved_log)
        rows_before = len(rows_of(page))
        inspector = Inspector(page, (390, 844), "light", False)
        home_reply = page.eval("__trainer.engine.call('home', Date.now())")
        check(home_reply.get("leaks") is None, f"home leaks not null when unloaded: {home_reply}")
        # 1. The picker entry is there, and with nothing loaded it says how to load.
        check(page.eval("[...document.querySelectorAll('[data-group]')].some(b => b.textContent === 'Preflop mistakes')"), "no Preflop mistakes entry in the picker")
        page.eval("document.querySelector('[data-group=mistakes]').click()")
        page.wait("!!document.querySelector('[data-mistakes-empty]')", 20)
        text = page.eval("document.body.innerText")
        check("No mistakes loaded" in text and "Load my mistakes" in text and "paste" in text.lower(), f"empty state: {text[:400]}")
        check_fit(page, "mistakes empty", 390, True)
        inspector.snapshot("mistakes-empty")

        # 2. A bad paste is refused; a good file loads from the picker.
        page.eval("document.querySelector('[data-leak-paste]').value = 'not a leak file'")
        page.eval("document.querySelector('[data-leak-paste-load]').click()")
        page.wait("(document.querySelector('[data-leak-status]') || {}).textContent.includes('not a leak file')", 10)
        if leak_file is None:
            leak_path = OUT / "synthetic-leaks.txt"
            leak_path.write_text(
                "#LEAKS v1\n"
                "#meta generated_at=2026-01-15T12:00:00 hands=3 leaks=3 preflop=2 price=1 half_life_days=14\n"
                "LEAK|0|preflop|leak:pf:early:limpers:ATo|9000|1|2026-01-15|2026-01-15a|synth-01|4.00|-18.5|You raised to 4bb from UTG+1 (over 1 limper)|Put in 4.0bb here; lost 18.5bb on the hand|players=9|hero_pos=UTG+1|behind=7|sit=limpers:1|hand=AcTd|hero_act=r|expect=f\n"
                "LEAK|1|price|leak:price:call_below:river|9000|1|2026-01-15|2026-01-15a|synth-02|20.00|-20.0|You called 20bb into 120bb on the river (needed 14%, had 2%-4%)|Put in 20.0bb here; lost 20.0bb on the hand|street=river|hero_pos=BB|board=4sQc3s5s3c|hand=As9h|pot_before=100|bet=20|callers=0|share_pct=3|tight_pct=2|loose_pct=4|hero_act=c|expect=f\n"
                "LEAK|2|limpers|leak:limpers:btn:1:K7s|9000|1|2026-01-16|2026-01-16a|synth-03|4.00|-12.0|You raised K7s over a CO limp on the button|Put in 4.0bb here; lost 12.0bb on the hand|players=5|seat=BTN|limpers=CO|hand=Ks7s|hero_act=r|expect=f\n"
            )
        else:
            leak_path = pathlib.Path(leak_file)
        text = leak_path.read_text()
        hand_ids = [line.split("|")[8] for line in text.splitlines() if line.startswith("LEAK|")]
        preflop = sum(1 for line in text.splitlines() if line.startswith("LEAK|") and line.split("|")[2] in ("preflop", "limpers"))
        page.send("DOM.enable")
        doc = page.send("DOM.getDocument")
        node = page.send("DOM.querySelector", {"nodeId": doc["root"]["nodeId"], "selector": "input[data-leak-input]"})
        page.send("DOM.setFileInputFiles", {"nodeId": node["nodeId"], "files": [str(leak_path)]})
        page.wait("!!document.querySelector('[data-mistakes-ready]')", 30)
        status = page.eval("document.querySelector('[data-leak-status]').textContent")
        check(f"{preflop} preflop" in status, f"load status: {status}")
        check_fit(page, "mistakes loaded", 390, True)
        inspector.snapshot("mistakes-ready")

        # 3. It stays loaded across a reload; pasting the text loads it as well.
        page.send("Page.reload", {})
        page.wait("!!document.querySelector('[data-mistakes-ready]') && !!window.__trainer", 30)
        page.eval("document.querySelector('[data-leak-clear]').click()")
        page.wait("!!document.querySelector('[data-mistakes-empty]')", 10)
        page.eval(f"document.querySelector('[data-leak-paste]').value = {json.dumps(text)}")
        page.eval("document.querySelector('[data-leak-paste-load]').click()")
        page.wait("!!document.querySelector('[data-mistakes-ready]')", 30)
        page.eval("document.querySelector('[data-mistakes-start]').click()")

        # 4. Only preflop leaks, nothing graded in advance, the counter, and the note from your hand.
        seen_skills = set()
        count = 0
        for idx in range(15):
            page.wait(READY + " || !!document.querySelector('.big-score')", 30)
            if page.eval("!!document.querySelector('.big-score')"):
                break
            current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
            check(not current["bundle"]["graded"], f"leak item {idx} shipped pre-graded answers")
            check(page.eval("document.querySelector('.top-title').textContent").startswith("Preflop mistakes"), "title")
            counter = page.eval("document.querySelector('.top-bar .count').textContent")
            check(counter == f"{idx + 1} of 15", f"leak counter {counter} at step {idx}")
            seen_skills.add(current["bundle"]["skill"])
            check_fit(page, f"mistake {idx} question", 390, True)
            # Half wrong (the play he made), half right, so misses come back.
            options = [o["code"] for o in current["bundle"]["format"]["options"]]
            type_answer(page, current, "choice", options[idx % len(options)])
            page.wait("!!document.querySelector('.reveal [data-leak-note]')", 30)
            note = page.eval("document.querySelector('.reveal [data-leak-note]').innerText")
            check("In your hand" in note and ("Put in" in note or "Cost" in note or "Folded" in note), f"leak reveal note missing details: {note}")
            check_fit(page, f"mistake {idx} feedback", 390, True)
            if idx == 0:
                inspector.snapshot("mistakes-r", full=True)
            count += 1
            click(page, ".action-bar .btn")
            page.wait("!document.querySelector('.reveal')", 30)
        check(count == 15, f"only {count} mistakes were dealt")
        check(seen_skills <= {"preflop", "limpers"} and bool(seen_skills), f"Preflop mistakes dealt {seen_skills}")
        if leak_file is None:
            check(seen_skills == {"preflop", "limpers"}, f"synthetic Preflop mistakes dealt only {seen_skills}")
        page.wait("!!document.querySelector('.big-score')", 20)

        # 5. Rows say only "leak" and an anonymous tag: no hand ids or dates from the file.
        rows = rows_of(page)
        check(len(rows) == rows_before + 15, f"{len(rows) - rows_before} leak rows saved")
        log_text = page.eval(f"localStorage.getItem({json.dumps(LOG_KEY)}) || ''")
        check("\tleak\t" in log_text and not any(h in log_text for h in hand_ids), "leak rows leaked hand ids into the attempt log")
        page.eval("location.hash = '#backup'")
        page.wait("!!document.querySelector('[data-leak-clear]')", 20)
        page.eval("document.querySelector('[data-leak-clear]').click()")
        page.wait("(document.querySelector('[data-leak-status]') || {}).textContent.includes('Cleared')", 10)
        page.eval("location.hash = '#mistakes'")
        page.wait("!!document.querySelector('[data-mistakes-empty]')", 20)
        check(not browser.errors, f"leaks browser errors: {browser.errors[:5]}")
        print("leaks ok", f"({len(hand_ids)} leaks, {preflop} preflop)")
    finally:
        browser.close()
        srv.shutdown()

# --- the River drill ------------------------------------------------------------------
RIVER_CODES = re.compile(r"\b(bluff_catch|thin_value|air_vs_bet|strong_vs_bet|bet33|bet75|bet150|allin|checked_to|top_pair_good|top_pair_weak|pp_below_top|low_pair|second_pair|H\d{4}|T\d{4})\b")
PHONE_SHOTS = pathlib.Path("/tmp/phone-shots")


def river_history():
    """River rows as a player who has used the drill has them: right and wrong answers,
    including misses that should come back as reviews."""
    lines = []
    for i in range(12):
        ok = i % 3 != 0
        seed = 1000 + i  # spot index i... in the low bits
        lines.append(f"{1_700_000_000_000 + i * 60_000}\triver\t\triver\t{seed}\t3001\t{ok:d}\t0\t{'' if ok else '12.5'}\t5000\t{'call' if ok else 'fold'}"
                     + "\t" * 10 + f"river:H{(1000 + i) % 1328 + 1:04d}" + "\t" * 3)
    return "\n".join(lines) + "\n"


def start_river(page):
    page.wait("!!document.querySelector('[data-group=river]')", 20)
    page.eval("document.querySelector('[data-group=river]').click()")
    page.wait(READY, 60)


def river_walk(page, inspector, questions, where, shots=False, fit_widths=(390,)):
    """Answers `questions` River questions, alternately right and wrong, starting new
    sessions as each ends. Returns how many feedback screens needed scrolling."""
    rows_before = len(rows_of(page))
    tall = []
    shot_right = shot_wrong = not shots
    answered = 0
    in_session = 0
    reviews = 0
    either_seen = 0
    while answered < questions:
        if page.eval("!!document.querySelector('.big-score')"):
            summary = page.eval("document.body.innerText")
            check(" of 20 right" in summary, f"{where}: summary {summary[:200]}")
            check_fit(page, f"{where} summary", 390, False)
            page.eval("location.hash = '#home'")
            in_session = 0
            start_river(page)
        page.wait(READY, 60)
        current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
        bundle = current["bundle"]
        q = bundle["question"]
        check(bundle["skill"] == "river" and q["kind"] == "river", f"{where} {answered}: dealt {bundle['skill']}")
        check(not bundle["graded"], f"{where} {answered}: shipped graded answers")
        pre = json.dumps(bundle)
        check(not any(k in pre for k in ('"accept', '"why"', '"best"', '"ev"')), f"{where} {answered}: the bundle gives the answer away")
        title = page.eval("document.querySelector('.top-title').textContent")
        counter = page.eval("document.querySelector('.top-bar .count').textContent")
        check(title.startswith("River"), f"{where}: title {title}")
        check(counter == f"{in_session + 1} of 20", f"{where} {answered}: counter {counter}")
        reviews += current["planned"]["origin"] == "due"
        text = inspector.text_check(f"{where} question")
        check(not RIVER_CODES.findall(text), f"{where} {answered}: internal codes {RIVER_CODES.findall(text)[:3]}")
        check(q["line"] in text and q["pot"] in text and q["context"] in text, f"{where} {answered}: question text missing")
        info = check_fit(page, f"{where} {answered} question", 390, True)
        if not shot_right or not shot_wrong:
            if answered == 0:
                PHONE_SHOTS.mkdir(parents=True, exist_ok=True)
                page.screenshot(str(PHONE_SHOTS / "river-question.png"))
        want_ok = answered % 2 == 0
        if not shot_wrong and answered > 0:
            want_ok = False
        page.eval("__grades.length = 0")
        kind, answer = pick_answer(page, current, want_ok)
        type_answer(page, current, kind, answer)
        page.wait("!!document.querySelector('.reveal')", 30)
        time.sleep(0.1)
        check_grade_matches(page, bundle, f"{where} {answered}")
        reveal = page.eval("document.querySelector('.reveal').innerText")
        outcome = json.loads(page.eval("JSON.stringify(__grades.length ? __grades[__grades.length - 1].r.outcome : null)"))
        check(outcome is not None, f"{where} {answered}: no grade reply")
        r = outcome["reveal"]
        check("Best:" in reveal and r["beat_line"] in reveal, f"{where} {answered}: feedback {reveal[:300]}")
        check(not RIVER_CODES.findall(reveal), f"{where} {answered}: internal codes in feedback {RIVER_CODES.findall(reveal)[:3]}")
        check(page.eval("!!document.querySelector('.reveal .split-bar')"), f"{where} {answered}: no split bar")
        if outcome["ok"]:
            check("loses" not in reveal.split("Every option")[0], f"{where} {answered}: a right answer was taught as a loss")
            if r["verdict"] == "either":
                either_seen += 1
                check(reveal.startswith("Either is fine.") and "about the same" in reveal, f"{where} {answered}: either feedback {reveal[:200]}")
            else:
                check(reveal.startswith("Right."), f"{where} {answered}: {reveal[:80]}")
        else:
            check(reveal.startswith("Not best.") and "loses" in reveal and "% of the pot" in reveal, f"{where} {answered}: wrong feedback {reveal[:300]}")
        info = check_fit(page, f"{where} {answered} feedback", 390, False)
        if info["height"] > info["vh"]:
            tall.append(info["height"])
        if not shot_right and outcome["ok"] and r["verdict"] == "clear":
            page.screenshot(str(PHONE_SHOTS / "river-right.png"))
            shot_right = True
        if not shot_wrong and not outcome["ok"]:
            page.screenshot(str(PHONE_SHOTS / "river-wrong.png"))
            shot_wrong = True
        page.eval("document.querySelectorAll('.reveal details').forEach(d => { d.open = true; })")
        time.sleep(0.05)
        check_fit(page, f"{where} {answered} feedback opened", 390, False)
        answered += 1
        in_session += 1
        wait_rows(page, rows_before + answered)
        click(page, ".action-bar .btn")
        page.wait("!document.querySelector('.reveal')", 30)
        page.wait(READY + " || !!document.querySelector('.big-score')", 60)
    rows = rows_of(page)[rows_before:]
    check(len(rows) == questions, f"{where}: {len(rows)} rows for {questions} answers")
    check(all(row[1] == "river" and row[3] == "river" and row[5] == "3001" for row in rows), f"{where}: rows {rows[:2]}")
    check(all(row[20].startswith("river:") for row in rows), f"{where}: no spot in the family column: {rows[:1]}")
    return tall, reviews, either_seen


def run_river():
    """River: 30 questions on a fresh profile and on one with saved data; the
    question and feedback fit a phone, a grade for another question is never
    shown or saved, and River answers stay out of Progress and the checkups."""
    port = 8993 + PORT_SHIFT
    srv = server.start(port)
    browser = Browser("river")
    try:
        page = browser.page
        boot_app(page, port, viewport=(390, 844, True))
        page.eval("localStorage.clear()")
        boot_app(page, port, viewport=(390, 844, True))
        inspector = Inspector(page, (390, 844), "light", False)
        check(page.eval("[...document.querySelectorAll('[data-group]')].some(b => b.textContent === 'River')"), "no River entry in the picker")
        start_river(page)
        page.eval(GRADE_HOOK)
        tall, _, either = river_walk(page, inspector, 30, "fresh", shots=True)
        print(f"river fresh: {len(tall)} of 30 feedback screens scrolled {sorted(tall)[-3:]}; {either} either spots")
        check(len(tall) <= 6, f"river feedback ran past a 390x844 screen {len(tall)} times in 30")
        page.eval("document.querySelector('.top-bar button.back').click()")
        page.wait("!!document.querySelector('.big-score')", 20)
        # Nothing about River in Progress, and no checkup offered on River answers alone.
        page.eval("location.hash = '#progress'")
        time.sleep(0.8)
        check("River" not in page.eval("document.body.innerText"), "River answers showed up in Progress")
        home = page.eval("__trainer.engine.call('home', Date.now() + 90 * 86400000)")
        check(home.get("checkup") is None, f"River answers offered a checkup: {home.get('checkup')}")

        # A profile with saved data: other drills' history and earlier River misses.
        log = synthetic_log().rstrip("\n") + "\n" + river_history()
        seed_log(page, port, log)
        rows_before = len(rows_of(page))
        start_river(page)
        page.eval(GRADE_HOOK)
        tall, reviews, _ = river_walk(page, inspector, 30, "saved")
        print(f"river saved: {len(tall)} of 30 feedback screens scrolled; {reviews} reviews")
        check(len(tall) <= 6, f"river feedback ran past a 390x844 screen {len(tall)} times in 30")
        check(reviews >= 2, f"only {reviews} earlier misses came back")
        check(len(rows_of(page)) == rows_before + 30, "saved profile rows")
        check(not browser.errors, f"river browser errors: {browser.errors[:5]}")
    finally:
        browser.close()
        srv.shutdown()

    # A prep worker that deals the wrong spot: the grade is refused, nothing is saved,
    # and the spot is dealt again by the engine.
    port = 8994 + PORT_SHIFT
    srv = server.start(port)
    browser = Browser("rivertamper")
    try:
        page = browser.page
        page.send("Page.addScriptToEvaluateOnNewDocument", {"source": TAMPER_PREP})
        # Until the wrong spot shows a button the real one lacks (check vs fold, say):
        # that answer can't even be read for the real spot, and must still be caught.
        for attempt in range(12):
            boot_app(page, port)
            page.eval(GRADE_HOOK)
            rows_before = len(rows_of(page))
            page.eval("window.__tamper = 1000")
            start_river(page)
            current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
            stale = current["bundle"]
            real = page.eval(f"__trainer.engine.call('prepare', 'river', {json.dumps(current['planned']['seed'])}, 'new')")
            real_codes = {o["code"] for o in real["format"]["options"]}
            unreadable = [o["code"] for o in stale["format"]["options"] if o["code"] not in real_codes]
            if unreadable:
                break
        check(unreadable, "river: never dealt a wrong spot with other buttons")
        check(stale["seed"] != current["planned"]["seed"], "river: the tampered spot was not dealt")
        page.eval("__grades.length = 0")
        type_answer(page, current, "choice", (unreadable or [stale["format"]["options"][0]["code"]])[0])
        must_see(page, "document.body.innerText.includes('out of date')", 30, "river: a spot out of date was not dealt again")
        page.wait(READY, 30)
        check(not page.eval("!!document.querySelector('.reveal')"), "river: a reveal about another spot was shown")
        check(len(rows_of(page)) == rows_before, "river: the answer to the wrong spot was saved")
        fresh = json.loads(page.eval("JSON.stringify(__trainer.current)"))
        check(fresh["bundle"]["seed"] == fresh["planned"]["seed"], "river: the spot was not dealt again")
        check(fresh["bundle"]["check"] != stale["check"], "river: the same wrong spot came back")
        page.eval("__grades.length = 0")
        kind, answer = pick_answer(page, fresh, False)
        type_answer(page, fresh, kind, answer)
        page.wait("!!document.querySelector('.reveal')", 30)
        check_grade_matches(page, fresh["bundle"], "river dealt again")
        wait_rows(page, rows_before + 1)
        check(not browser.errors, f"river tamper errors: {browser.errors[:5]}")
        print("river ok")
    finally:
        browser.close()
        srv.shutdown()

def run_lib_limpers():
    done = subprocess.run(["cargo", "test", "--release", "--test", "limpers"], capture_output=True, text=True, cwd=ROOT)
    print(done.stdout[-3000:])
    if done.returncode != 0:
        print(done.stderr[-1500:])
    check(done.returncode == 0, f"library limpers tests failed: {done.stdout[-400:]}")


def run_limpers():
    """Limpers drill: compact question & reveal, phone fit at 390x844, BB Check, raise sizing, Details block."""
    port = 8995 + PORT_SHIFT
    srv = server.start(port)
    browser = Browser("limpers")
    try:
        page = browser.page
        boot_app(page, port, viewport=(390, 844, True))
        inspector = Inspector(page, (390, 844), "light", False)
        before = len(rows_of(page))
        open_limited_drill(page, "limpers")
        saw_bb = False
        saw_non_bb = False
        saw_raise = False
        saw_call_or_fold = False
        saved_q = False
        saved_r = False
        total = 24
        for step in range(total):
            page.wait(READY, 30)
            current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
            q = current["bundle"]["question"]
            check(q["kind"] == "limpers", f"expected limpers question, got {q}")
            check(q["players"] in (5, 6), f"bad table size: {q}")
            check(1 <= q["limpers_count"] <= 3, f"bad limpers count: {q}")
            # One short context line only (no extra .context paragraph).
            check(page.eval("document.querySelectorAll('.question .context').length") == 0, "limpers question has extra context paragraph")
            prompt_text = page.eval("document.querySelector('.question .prompt').textContent")
            check("You are " in prompt_text and "limp" in prompt_text, f"bad limpers context: {prompt_text!r}")
            labels = page.eval("[...document.querySelectorAll('.answers .choice')].map(b => b.childNodes[b.childNodes.length - 1].textContent.trim())")
            if q["seat_short"] == "BB":
                check(labels == ["Raise", "Check"], f"BB options were {labels}")
                saw_bb = True
            else:
                check(labels == ["Raise", "Call", "Fold"], f"non-BB options were {labels}")
                saw_non_bb = True
            check_fit(page, f"limpers question {step}", 390, True)
            inspector.snapshot(f"limpers-q-{step}")
            if not saved_q:
                page.screenshot("/tmp/limper-question.png", full=False)
                saved_q = True

            kind, answer = pick_answer(page, current, step % 3 != 0)
            type_answer(page, current, kind, answer)
            page.wait("!!document.querySelector('.reveal')", 30)
            time.sleep(0.05)
            reveal_text = page.eval("document.querySelector('.reveal').innerText")
            check("Best: " in reveal_text, f"limpers reveal missing Best line: {reveal_text}")
            details_closed = page.eval("document.querySelectorAll('.reveal details.more:not([open])').length")
            check(details_closed == 1, f"limpers reveal details block not collapsed: {details_closed}")
            best_text = page.eval("document.querySelector('.reveal .reveal-answer strong').textContent")
            if best_text.startswith("Raise to "):
                check(re.fullmatch(r"Raise to (6|7|8|9|10|11)bb", best_text), f"unexpected raise size: {best_text!r}")
                saw_raise = True
                if not saved_r:
                    page.screenshot("/tmp/limper-reveal.png", full=False)
                    saved_r = True
            else:
                check(best_text in ("Call", "Check", "Fold"), f"unexpected best text: {best_text!r}")
                saw_call_or_fold = True
            check_fit(page, f"limpers reveal {step}", 390, True)
            inspector.snapshot(f"limpers-r-{step}")
            # Expanding Details keeps horizontal fit.
            page.eval("document.querySelector('.reveal details.more').open = true")
            time.sleep(0.05)
            expanded_text = page.eval("document.querySelector('.reveal details.more').innerText")
            check("Size rule:" in expanded_text, f"limpers details missing size rule: {expanded_text}")
            check_fit(page, f"limpers reveal expanded {step}", 390, False)
            next_drill_item(page)

        if not saved_r:
            page.screenshot("/tmp/limper-reveal.png", full=False)
        check(saw_bb and saw_non_bb, f"saw_bb={saw_bb} saw_non_bb={saw_non_bb}")
        check(saw_raise and saw_call_or_fold, f"saw_raise={saw_raise} saw_call_or_fold={saw_call_or_fold}")
        rows = rows_of(page)[before:]
        check(len(rows) == total and all(r[3] == "limpers" for r in rows), f"limpers log rows: {len(rows)}")
        page.eval("location.hash = '#progress'")
        page.wait("document.body.innerText.includes('Limpers')", 20)
        check(not browser.errors, f"limpers browser errors: {browser.errors[:5]}")
        print("limpers ok")
    finally:
        browser.close()
        srv.shutdown()


SECTIONS = {
    "native": run_native, "wasmclean": run_wasm_clean, "doubletap": run_doubletap, "twotabs": run_twotabs,
    "libresearch": run_lib_research, "libcli": run_lib_cli, "libreadprice": run_lib_read_price, "readprice": run_read_price, "research": run_research, "researchshots": run_research_shots, "libtable": run_lib_table, "tablespots": run_table_spots, "midgrade": run_mid_grade,
    "dealing": run_dealing, "prepfail": run_prep_failure, "export": run_export, "swscope": run_sw_scope,
    "quota": run_quota, "hiddenevent": run_hidden_event,
    "differential": run_differential, "timing": run_timing, "e2e": run_e2e, "baseline": run_baseline,
    "newskills": run_new_skills, "noneleft": run_none_left, "checkup": run_checkup, "checkupquit": run_checkup_quit,
    "checkuptabs": run_checkup_tabs, "checkupconflict": run_checkup_conflict, "progressnew": run_progress_new,
    "libgroups": run_lib_groups, "groups": run_groups, "leaks": run_leaks, "river": run_river, "libriver": run_lib_river,
    "liblimpers": run_lib_limpers, "limpers": run_limpers,
    "samebuild": run_same_build, "phonefit": run_phone_fit, "hiddendrills": run_hidden_drills,
    "preflopspots": run_preflop_spots, "libpreflop": run_lib_preflop, "card": run_card, "offline": run_offline, "nostorage": run_nostorage, "legacy": run_legacy, "screens": run_screens,
}

if __name__ == "__main__":
    OUT.mkdir(parents=True, exist_ok=True)
    os.chdir(ROOT)
    wanted = sys.argv[1:] or [name for name in SECTIONS if name != "researchshots"]
    started = time.time()
    for name in wanted:
        print(f"== {name}", flush=True)
        try:
            SECTIONS[name]()
        except SectionStopped:
            pass
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
