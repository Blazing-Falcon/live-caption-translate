"""Score one replay run from its transcript JSONL.

    python scripts/eval/score.py <run>/out.jsonl --zh testdata/eval/drama-130s.zh.txt

Every metric comes from the file alone, so a run can be rescored without the audio.
Metrics that need a reference are omitted when none is given.
"""

import argparse
import json
import re
import sys
from collections import Counter
from pathlib import Path

sys.stdout.reconfigure(encoding="utf-8")

HAN = re.compile(r"[㐀-䶿一-鿿]")
HAN_ONLY = re.compile(r"[^㐀-䶿一-鿿]")
SENTENCE_MARK = re.compile(r"[。？！?!]")


def percentile(values, q):
    """Nearest-rank: a small run reports an observed value, never an interpolation."""
    if not values:
        return None
    ordered = sorted(values)
    rank = max(1, min(len(ordered), int(-(-len(ordered) * q // 1))))
    return ordered[rank - 1]


def load_records(path):
    records = []
    with Path(path).open(encoding="utf-8") as handle:
        for line in handle:
            line = line.strip()
            if line:
                records.append(json.loads(line))
    return records


def edit_distance(reference, hypothesis):
    """Levenshtein over two strings, one row at a time."""
    if not reference:
        return len(hypothesis)
    previous = list(range(len(hypothesis) + 1))
    for i, r in enumerate(reference, 1):
        current = [i]
        for j, h in enumerate(hypothesis, 1):
            current.append(
                min(previous[j] + 1, current[j - 1] + 1, previous[j - 1] + (r != h))
            )
        previous = current
    return previous[-1]


def cer(reference, hypothesis):
    return edit_distance(reference, hypothesis) / max(1, len(reference))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("jsonl", help="out.jsonl from lt-cli replay")
    parser.add_argument("--zh", help="reference Chinese, one line per sentence")
    parser.add_argument("--en", help="reference English, one line per sentence")
    parser.add_argument("--label", default="")
    args = parser.parse_args()

    records = load_records(args.jsonl)
    header = next((r for r in records if r.get("type") == "session"), {})
    utterances = [r for r in records if r.get("type") == "utterance"]
    finals = [r for r in utterances if r.get("status") == "final"]
    failed = [r for r in utterances if r.get("status") == "failed"]
    skipped = [r for r in utterances if r.get("status") == "skipped"]

    timings = [r.get("timing") or {} for r in finals]
    end_to_english = [
        t["done_ms"] - t["speech_end_ms"]
        for t in timings
        if t.get("done_ms") is not None and t.get("speech_end_ms") is not None
    ]
    first_token = [
        t["first_token_ms"] - t["speech_end_ms"]
        for t in timings
        if t.get("first_token_ms") is not None and t.get("speech_end_ms") is not None
    ]
    close = [
        t["asr_done_ms"] - t["speech_end_ms"]
        for t in timings
        if t.get("asr_done_ms") is not None and t.get("speech_end_ms") is not None
    ]
    second_pass_ms = [
        t["sent_ms"] - t["asr_done_ms"]
        for t in timings
        if t.get("sent_ms") is not None and t.get("asr_done_ms") is not None
    ]
    translate_ms = [
        t["done_ms"] - t["sent_ms"]
        for t in timings
        if t.get("done_ms") is not None and t.get("sent_ms") is not None
    ]
    queued_ms = [
        t["sent_ms"] - t["queued_ms"]
        for t in timings
        if t.get("sent_ms") is not None and t.get("queued_ms") is not None
    ]
    # A request missed the cache when it reused no more than the frozen instruction.
    instruction_tokens = 23
    misses = sum(
        1 for t in timings if (t.get("cached_tokens") or 0) <= instruction_tokens
    )

    second = [rec for r in finals for rec in (r.get("asr_second") or [])]
    outcomes = Counter(r.get("outcome") for r in second)
    identical = punctuation_only = character_edits = 0
    for record in second:
        if record.get("outcome") != "used":
            continue
        live = record.get("live_text")
        text = record.get("text") or ""
        if live is None:
            identical += 1
        elif HAN_ONLY.sub("", live) == HAN_ONLY.sub("", text):
            punctuation_only += 1
        else:
            character_edits += 1

    durations = [
        (r["end_ms"] - r["start_ms"]) / 1000.0 for r in finals if "end_ms" in r
    ]
    han_chars = [len(HAN.findall(r.get("source") or "")) for r in finals]
    gaps = []
    for previous, current in zip(finals, finals[1:]):
        gap = (current["start_ms"] - previous["end_ms"]) / 1000.0
        if gap >= 0:
            gaps.append(gap)
    span_minutes = (
        (finals[-1]["end_ms"] - finals[0]["start_ms"]) / 60000.0 if len(finals) > 1 else 0.0
    )
    cuts = Counter(r.get("cut") for r in finals)
    live_with_mark = sum(
        1
        for r in finals
        if SENTENCE_MARK.search(((r.get("asr_live") or {}).get("text") or ""))
    )

    # A fragment: the pipeline decided the clause stopped mid-thought, and the English was
    # published anyway.
    #
    # The transcript carries the pipeline's own flag, so this no longer infers the decision from
    # the text. The old inference broke when the confidence gate arrived: a gated line keeps live
    # text the recognizer never punctuated, so two complete sentences (好的，小吕 and 不用了)
    # counted as fragments on a gated run that had none without the gate.
    fragments = [
        r["id"]
        for r in finals
        if r.get("fragment") is True and (r.get("english") or "").strip()
    ]

    result = {
        "label": args.label,
        "file": str(args.jsonl),
        "header": header,
        "lines": {
            "final": len(finals),
            "failed": len(failed),
            "skipped": len(skipped),
            "per_minute": round(len(finals) / span_minutes, 1) if span_minutes else None,
            "span_minutes": round(span_minutes, 2),
        },
        "cuts": dict(cuts),
        "live_has_sentence_mark": live_with_mark,
        "latency_ms": {
            "end_to_english_p50": percentile(end_to_english, 0.5),
            "end_to_english_p90": percentile(end_to_english, 0.9),
            "first_token_p50": percentile(first_token, 0.5),
            "close_p50": percentile(close, 0.5),
            "second_pass_p50": percentile(second_pass_ms, 0.5),
            "translate_p50": percentile(translate_ms, 0.5),
            "queued_p50": percentile(queued_ms, 0.5),
        },
        "prompt_cache": {
            "requests": len(finals),
            "misses": misses,
            "miss_share": round(misses / len(finals), 3) if finals else None,
        },
        "second_pass": {
            "records": len(second),
            "outcomes": dict(outcomes),
            "identical": identical,
            "punctuation_only": punctuation_only,
            "character_edits": character_edits,
        },
        "shape": {
            "duration_p50_s": round(percentile(durations, 0.5) or 0, 2),
            "han_chars_p50": percentile(han_chars, 0.5),
            "han_chars_p90": percentile(han_chars, 0.9),
            "gap_p50_s": round(percentile(gaps, 0.5) or 0, 2) if gaps else None,
            "gap_share_ge_0_8": (
                round(sum(1 for g in gaps if g >= 0.8) / len(gaps), 3) if gaps else None
            ),
        },
        "fragments": {"count": len(fragments), "ids": fragments},
    }

    # Text quality against a reference, when one exists.
    if args.zh and args.en:
        reference_zh = Path(args.zh).read_text(encoding="utf-8").splitlines()
        reference_en = Path(args.en).read_text(encoding="utf-8").splitlines()
        reference_zh = [line.strip() for line in reference_zh if line.strip()]
        reference_en = [line.strip() for line in reference_en if line.strip()]
        if len(reference_zh) != len(reference_en):
            raise SystemExit("The Chinese and English references have different line counts")
        # The replay's own lines are the hypothesis; align by order when the counts match.
        hypothesis_zh = [r.get("source") or "" for r in finals]
        hypothesis_en = [r.get("english") or "" for r in finals]
        joined_zh = "".join(reference_zh)
        joined_hyp_zh = "".join(hypothesis_zh)
        result["reference"] = {
            "lines": {"reference": len(reference_zh), "hypothesis": len(hypothesis_zh)},
            "cer_source": round(cer(HAN_ONLY.sub("", joined_zh), HAN_ONLY.sub("", joined_hyp_zh)), 4),
            "cer_english": round(
                cer(" ".join(reference_en).lower(), " ".join(hypothesis_en).lower()), 4
            ),
        }

    print(json.dumps(result, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
