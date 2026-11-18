#!/usr/bin/env python3
"""Make data/river_replies_v1.txt from the river library's JSONL export.

The river library (data/river_drills_v1.txt) is copied verbatim from the
generator's export. Its JSONL twin also records, for each of hero's bets and
raises in a heads-up spot, how the opponent answers: the share that folds, calls
or raises, the kinds of hand in each part, and hero's showdown share against the
callers. This script keeps only that, in the same one-line-per-record style, so
the app can say what calls and what folds. Standard library only; it reads data,
it does not use any of the generator's code.

    python3 -B tools/river_replies.py ~/.cache/river-drills/river_drills_v1.jsonl > data/river_replies_v1.txt
"""
import json
import sys

# Hand classes of the solver's report, folded into the groups the library's
# `split` field uses, strongest first.
GROUPS = [
    ("straights or better", {"straight_flush", "quads", "full_house", "flush", "straight"}),
    ("three of a kind", {"set", "trips"}),
    ("two pair", {"two_pair"}),
    ("top pair or an overpair", {"overpair", "top_pair_good", "top_pair_weak"}),
    ("weaker pairs", {"pp_below_top", "second_pair", "low_pair"}),
    ("no pair", {"ace_high", "air", "gutshot", "oesd", "flush_draw", "combo_draw"}),
]
# Groups smaller than this share of a reply are left out of its list.
MIN_GROUP = 0.05
AGGRESSIVE = ("bet33", "bet75", "bet150", "raise", "allin")


def groups_text(by_bucket):
    totals = []
    for name, members in GROUPS:
        share = sum(b["share"] for b in by_bucket if b["name"] in members)
        unknown = [b["name"] for b in by_bucket if not any(b["name"] in m for _, m in GROUPS)]
        if unknown:
            raise SystemExit(f"unknown hand class {unknown[0]}")
        totals.append((name, share))
    whole = sum(s for _, s in totals) or 1.0
    kept = [(n, s / whole) for n, s in totals if s / whole >= MIN_GROUP]
    return ",".join(f"{n}:{s:.2f}" for n, s in kept)


def main(path):
    out = [
        "#RIVER-REPLIES v1",
        "# How the opponent answers each of hero's bets and raises in the heads-up spots of",
        "# river_drills_v1.txt, from the same export's JSONL (tools/river_replies.py).",
        "# REPLY|id|option code|fold=share/groups|call=share:hero_beat/groups|raise=share/groups",
        "# share: of the opponent's range at the decision; groups: what:share of that part,",
        "# strongest first, parts under 5% left out; hero_beat: hero's showdown share against the callers.",
    ]
    lines = 0
    with open(path) as fh:
        for raw in fh:
            spot = json.loads(raw)
            if spot.get("players") != 2 or not spot.get("replies"):
                continue
            codes = {o["label"]: o["code"] for o in spot["options"]}
            for reply in spot["replies"]:
                code = codes.get(reply["hero_action"])
                if code not in AGGRESSIVE:
                    continue
                parts = {"fold": [0.0, None, []], "call": [0.0, None, []], "raise": [0.0, None, []]}
                for act in reply["actions"]:
                    kind = act["action"].split(" ")[0]
                    kind = {"all-in": "raise"}.get(kind, kind)
                    if kind not in parts:
                        raise SystemExit(f"{spot['lib_id']}: unexpected reply {act['action']}")
                    vs = act.get("versus_hero") or {}
                    part = parts[kind]
                    part[0] += act["share"]
                    part[2].extend(vs.get("by_bucket", []) and [
                        dict(b, share=b["share"] * act["share"]) for b in vs["by_bucket"]])
                    if kind == "call":
                        part[1] = vs.get("beat", 0.0) + vs.get("tie", 0.0) / 2
                fields = [f"REPLY|{spot['lib_id']}|{code}"]
                for kind in ("fold", "call", "raise"):
                    share, beat, buckets = parts[kind]
                    head = f"{share:.3f}" + (f":{beat:.3f}" if kind == "call" and beat is not None and share > 0 else "")
                    fields.append(f"{kind}={head}/{groups_text(buckets) if share > 0 else ''}")
                out.append("|".join(fields))
                lines += 1
    out.insert(6, f"#meta replies={lines}")
    sys.stdout.write("\n".join(out) + "\n")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "river_drills_v1.jsonl")
