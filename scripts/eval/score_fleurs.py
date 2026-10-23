"""Map a FLEURS replay's captions back to their sentences, then score recognition per sentence.

    python scripts/eval/score_fleurs.py RUN/out.jsonl \
        --spans testdata/fetched/fleurs/fleurs-dev-50.spans.json \
        --zh testdata/fetched/fleurs/fleurs-dev-50.zh.txt --hyp-out RUN/fleurs.en.txt

A caption belongs to the sentence whose time span holds its midpoint; a sentence cut into several
clauses gets their English joined. The written hypothesis has one line per sentence, in reference
order, for `score_corpus.py --src ... --ref ... --hyp ...`. A sentence with no English is an
empty line and scores as a miss there.
"""

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from score import HAN_ONLY, edit_distance, load_records  # noqa: E402

sys.stdout.reconfigure(encoding="utf-8")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("transcript")
    parser.add_argument("--spans", required=True)
    parser.add_argument("--zh", required=True)
    parser.add_argument("--hyp-out", required=True)
    args = parser.parse_args()

    spans = json.loads(Path(args.spans).read_text(encoding="utf-8"))
    # The gap after a sentence belongs to it: a caption's end carries the last word's margin.
    starts = [span["start_ms"] for span in spans] + [float("inf")]
    references = Path(args.zh).read_text(encoding="utf-8").splitlines()
    heard = [[] for _ in spans]
    english = [[] for _ in spans]
    unplaced = 0
    for record in load_records(args.transcript):
        if record.get("type") != "utterance":
            continue
        middle = (record["start_ms"] + record["end_ms"]) / 2
        index = next((i for i in range(len(spans)) if starts[i] <= middle < starts[i + 1]), None)
        if index is None:
            unplaced += 1
            continue
        heard[index].append(record.get("source", ""))
        if record.get("status") == "final" and record.get("english"):
            english[index].append(record["english"])

    errors = total = 0
    for reference, parts in zip(references, heard):
        reference = HAN_ONLY.sub("", reference)
        errors += edit_distance(reference, HAN_ONLY.sub("", "".join(parts)))
        total += len(reference)
    hypotheses = "".join(" ".join(parts) + "\n" for parts in english)
    Path(args.hyp_out).write_text(hypotheses, encoding="utf-8")
    report = {
        "sentences": len(spans),
        "without_english": sum(1 for parts in english if not parts),
        "captions_before_first_sentence": unplaced,
        "cer": round(errors / max(1, total), 4),
        "reference_han": total,
    }
    print(json.dumps(report, ensure_ascii=False, indent=1))


if __name__ == "__main__":
    main()
