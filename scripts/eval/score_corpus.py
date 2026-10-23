"""Score translations against the subtitle sample with COMET, chrF and BLEU.

    python scripts/eval/score_corpus.py --hyp a.txt --label ternary [--hyp b.txt --label q4]

With two hypotheses it also prints the paired bootstrap confidence interval and the lines where the
second loses most. `--out` chooses where the report and per-line scores go, so scoring a different
corpus does not overwrite this one.

COMET (wmt22-comet-da) is downloaded on first use into `.agent/tmp/comet/` and needs `torch` and
`unbabel-comet` in the venv.
"""

import argparse
import json
import random
import statistics
import sys
from pathlib import Path

sys.stdout.reconfigure(encoding="utf-8")

ROOT = Path(__file__).resolve().parents[2]
CORPUS = ROOT / ".agent/experiments/corpus"


def read_lines(path):
    return [line.rstrip("\n") for line in Path(path).read_text(encoding="utf-8").splitlines()]


def bootstrap(diffs, rounds=2000, seed=11):
    """Mean of a paired difference, with a 95% interval from resampling the lines."""
    rng = random.Random(seed)
    means = sorted(
        sum(diffs[rng.randrange(len(diffs))] for _ in range(len(diffs))) / len(diffs)
        for _ in range(rounds)
    )
    return means[int(rounds * 0.025)], means[int(rounds * 0.975)]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--hyp", action="append", required=True)
    parser.add_argument("--label", action="append", required=True)
    parser.add_argument("--ref", default=str(CORPUS / "opus-casual.en.txt"))
    parser.add_argument("--src", default=str(CORPUS / "opus-casual.zh.txt"))
    parser.add_argument(
        "--out",
        default=str(CORPUS / "scores.json"),
        help="where to write the report and the per-line scores",
    )
    args = parser.parse_args()

    if len(args.hyp) != len(args.label):
        raise SystemExit(
            f"{len(args.hyp)} --hyp against {len(args.label)} --label; every hypothesis needs its "
            "own label"
        )
    if len(set(args.label)) != len(args.label):
        raise SystemExit(f"duplicate --label: {args.label}; labels key the report")

    source = read_lines(args.src)
    reference = read_lines(args.ref)
    if len(source) != len(reference):
        raise SystemExit(f"{len(source)} source lines against {len(reference)} references")
    hypotheses = []
    for path, label in zip(args.hyp, args.label):
        lines = read_lines(path)
        if len(lines) != len(reference):
            raise SystemExit(f"{label}: {len(lines)} lines against {len(reference)} references")
        hypotheses.append((label, lines))

    from comet import download_model, load_from_checkpoint

    checkpoint = download_model(
        "Unbabel/wmt22-comet-da", saving_directory=str(ROOT / ".agent/tmp/comet")
    )
    model = load_from_checkpoint(checkpoint)
    import sacrebleu

    report = {}
    per_line = {}
    for label, lines in hypotheses:
        data = [{"src": s, "mt": h, "ref": r} for s, h, r in zip(source, lines, reference)]
        result = model.predict(data, batch_size=8, gpus=0, progress_bar=False)
        scores = result["scores"]
        per_line[label] = scores
        report[label] = {
            "comet": round(result["system_score"], 4),
            "comet_p10": round(sorted(scores)[len(scores) // 10], 4),
            "comet_median": round(statistics.median(scores), 4),
            "chrf": round(sacrebleu.corpus_chrf(lines, [reference], word_order=2).score, 2),
            "bleu": round(sacrebleu.corpus_bleu(lines, [reference]).score, 2),
            "empty": sum(1 for line in lines if not line.strip()),
        }
    print(json.dumps(report, indent=1, ensure_ascii=False))

    if len(hypotheses) == 2:
        (a_label, a), (b_label, b) = hypotheses
        a_scores, b_scores = per_line[a_label], per_line[b_label]
        diffs = [y - x for x, y in zip(a_scores, b_scores)]
        low, high = bootstrap(diffs)
        better = sum(1 for d in diffs if d > 0.01)
        worse = sum(1 for d in diffs if d < -0.01)
        print(
            f"\n{b_label} against {a_label}: mean {statistics.mean(diffs):+.4f}, "
            f"95% CI [{low:+.4f}, {high:+.4f}], better on {better}, worse on {worse}, "
            f"within 0.01 on {len(diffs) - better - worse}"
        )
        print(f"\nLines where {b_label} loses most:")
        for index in sorted(range(len(diffs)), key=lambda i: diffs[i])[:12]:
            print(f"  {index:3d} {diffs[index]:+.3f} {source[index]}")
            print(f"      ref: {reference[index]}")
            print(f"      {a_label}: {a[index]}")
            print(f"      {b_label}: {b[index]}")

    Path(args.out).write_text(
        json.dumps({"report": report, "per_line": per_line}, indent=1), encoding="utf-8"
    )
    print(f"\nwrote {args.out}")


if __name__ == "__main__":
    main()
