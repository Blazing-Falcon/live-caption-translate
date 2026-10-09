#!/usr/bin/env python3
"""Checks on a `lt-cli replay` event log (`<out>.events.jsonl`).

  python scripts/replay-checks.py out/juan-continuous.events.jsonl [--trace TRACE.json] [--expect 麒麟9050pro]

Reports per-id event ordering violations (see docs/ipc.md), drafts that repeat a phrase
three times in a row, the share of reference-trace clauses reproduced, and whether the final
clause texts contain an expected string. Exit status 1 when a hard check fails.
"""
import argparse
import json
import re
import sys

TERMINAL = {"translation_final", "translation_failed", "skipped", "dropped"}


def load(path):
    events = []
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            line = line.strip()
            if line:
                events.append(json.loads(line))
    return events


def ordering(events):
    problems = []
    state = {}  # id -> dict
    for item in events:
        event = item["event"]
        kind = event["type"]
        if kind == "joined":
            for absorbed in event["absorbed"]:
                state.setdefault(absorbed, {})["absorbed"] = True
            continue
        if "id" not in event:
            continue
        ident = event["id"]
        line = state.setdefault(ident, {"drafts_after_final": 0, "rev": 0})
        line.setdefault("drafts_after_final", 0)
        line.setdefault("rev", 0)
        if line.get("terminal"):
            problems.append(f"{kind} for id {ident} after its terminal event")
            continue
        if kind == "asr_partial":
            if line.get("final"):
                problems.append(f"asr_partial for id {ident} after its asr_final")
        elif kind == "translation_draft":
            if line.get("absorbed"):
                problems.append(f"draft for id {ident} after it was absorbed")
            if event["rev"] <= line["rev"]:
                problems.append(f"draft rev {event['rev']} not above {line['rev']} for id {ident}")
            line["rev"] = event["rev"]
            if line.get("final"):
                line["drafts_after_final"] += 1
                if line["drafts_after_final"] > 1:
                    problems.append(f"more than one draft after the final of id {ident}")
        elif kind == "asr_final":
            if line.get("final"):
                problems.append(f"repeated asr_final for id {ident}")
            line["final"] = True
        elif kind in TERMINAL:
            if kind != "dropped" and not line.get("final") and not line.get("absorbed"):
                problems.append(f"{kind} for id {ident} before its asr_final")
            line["terminal"] = True
    return problems


def repeats(text):
    words = re.findall(r"[\w']+", text.lower())
    for size in range(1, 7):
        for start in range(0, len(words) - 3 * size + 1):
            unit = words[start : start + size]
            if words[start + size : start + 2 * size] == unit and words[start + 2 * size : start + 3 * size] == unit:
                return " ".join(unit)
    return None


def draft_repeats(events):
    found = []
    for item in events:
        event = item["event"]
        if event["type"] == "translation_draft":
            unit = repeats(event["text"])
            if unit:
                found.append((event["id"], event["rev"], unit))
    return found


def strip(text):
    return re.sub(r"[\s，。、？！,.?!；;：:]", "", text)


def final_clauses(events):
    return [item["event"]["text"] for item in events if item["event"]["type"] == "asr_final"]


def trace_match(clauses, trace_path):
    trace = json.load(open(trace_path, encoding="utf-8"))
    wanted = [piece_clause["text"] for piece in trace["pieces"] for piece_clause in piece["clauses"]]
    got = {strip(text) for text in clauses}
    hits = sum(1 for text in wanted if strip(text) in got)
    return hits, len(wanted)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("events")
    parser.add_argument("--trace")
    parser.add_argument("--expect", action="append", default=[])
    args = parser.parse_args()
    events = load(args.events)
    failed = False
    problems = ordering(events)
    print(f"ordering violations: {len(problems)}")
    for problem in problems[:10]:
        print("  ", problem)
    failed |= bool(problems)
    repeated = draft_repeats(events)
    print(f"drafts repeating a phrase 3 times: {len(repeated)}")
    for item in repeated[:5]:
        print("  ", item)
    failed |= bool(repeated)
    clauses = final_clauses(events)
    print(f"clauses: {len(clauses)}")
    for expected in args.expect:
        present = any(expected.lower() in text.lower() for text in clauses)
        print(f"clause text contains {expected!r}: {present}")
        failed |= not present
    if args.trace:
        hits, total = trace_match(clauses, args.trace)
        share = hits / total if total else 0
        print(f"reference clauses reproduced: {hits}/{total} ({share:.0%})")
        failed |= share < 0.9
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
