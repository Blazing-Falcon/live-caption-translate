#!/usr/bin/env python3
"""Checks on a `lt-cli replay` event log (`<out>.events.jsonl`).

  python scripts/replay-checks.py out/conv.events.jsonl [--trace TRACE.json] [--expect 麒麟9050pro]

Reports per-id event ordering violations (see docs/ipc.md), the share of reference-trace
clauses reproduced, and whether the final clause texts contain an expected string. Exit status
1 when a hard check fails.
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
        line = state.setdefault(ident, {})
        if line.get("terminal"):
            problems.append(f"{kind} for id {ident} after its terminal event")
            continue
        if kind == "asr_partial":
            if line.get("final"):
                problems.append(f"asr_partial for id {ident} after its asr_final")
        elif kind == "asr_final":
            if line.get("final"):
                problems.append(f"repeated asr_final for id {ident}")
            line["final"] = True
        elif kind in TERMINAL:
            if kind != "dropped" and not line.get("final") and not line.get("absorbed"):
                problems.append(f"{kind} for id {ident} before its asr_final")
            line["terminal"] = True
    return problems


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
