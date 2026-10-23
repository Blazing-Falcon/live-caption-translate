"""Sample zh->en subtitle pairs from OpenSubtitles, for a translation scoreboard with a reference.

The Chinese side of a subtitle corpus is a human translation of an original English line, so
translating it back and scoring against the original measures the translator on real subtitle
register, with a reference nobody on this project wrote. Only 1-to-1 alignments are kept: the
corpus's `ids` file also lists many-to-one rows, and a merged row pairs one Chinese line with two
English ones, which would score as a miss.

  # One time: download the corpus (~345 MiB) into .agent/tmp/opus/.
  curl -L -o .agent/tmp/opus/en-zh_cn.txt.zip \
      https://object.pouta.csc.fi/OPUS-OpenSubtitles/v2018/moses/en-zh_cn.txt.zip

  python scripts/eval/make_corpus.py --pairs 600

Writes `.agent/experiments/corpus/opus-casual.{zh,en}.txt`. The sample is third-party subtitle text,
so it stays out of the repository; the rule and the seed are what make it reproducible.
"""

import argparse
import hashlib
import random
import re
import sys
import zipfile
from pathlib import Path

sys.stdout.reconfigure(encoding="utf-8")

ROOT = Path(__file__).resolve().parents[2]
ZIP = ROOT / ".agent/tmp/opus/en-zh_cn.txt.zip"
OUT = ROOT / ".agent/experiments/corpus"

HAN = re.compile(r"[㐀-䶿一-鿿]")
LATIN_WORD = re.compile(r"[A-Za-z]{2,}")
# Our captions: p50 12 Han characters, p90 20. Keep a band around that, and drop lines that are a
# fragment of a longer sentence or carry no Han at all.
MIN_HAN, MAX_HAN = 6, 24


def han_count(text):
    return len(HAN.findall(text))


def usable(zh, en):
    if not (MIN_HAN <= han_count(zh) <= MAX_HAN):
        return False
    # Latin inside the Chinese side usually means a name left untranslated or a misaligned pair.
    if LATIN_WORD.search(zh):
        return False
    if len(en) < 12 or len(en) > 120:
        return False
    if not re.search(r"[.!?…]$", en.strip()):
        return False
    # A pair with an unbalanced quote is usually misaligned.
    if zh.count("「") != zh.count("」") or zh.count("“") != zh.count("”"):
        return False
    return True


def one_to_one(ids_line):
    """Whether the row pairs exactly one English index with one Chinese index."""
    fields = ids_line.rstrip("\n").split("\t")
    if len(fields) < 4:
        return False
    return len(fields[2].split()) == 1 and len(fields[3].split()) == 1


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--pairs", type=int, default=600)
    parser.add_argument("--seed", type=int, default=20261023)
    args = parser.parse_args()
    if not ZIP.exists():
        raise SystemExit(f"No corpus at {ZIP}; see this file's docstring for the download.")

    rng = random.Random(args.seed)
    kept = []
    seen = set()
    with zipfile.ZipFile(ZIP) as archive:
        with archive.open("OpenSubtitles.en-zh_cn.zh_cn") as zh_raw, archive.open(
            "OpenSubtitles.en-zh_cn.en"
        ) as en_raw, archive.open("OpenSubtitles.en-zh_cn.ids") as ids_raw:
            for zh, en, ids in zip(zh_raw, en_raw, ids_raw):
                if not one_to_one(ids.decode("utf-8", errors="replace")):
                    continue
                zh = zh.decode("utf-8", errors="replace").strip()
                en = en.decode("utf-8", errors="replace").strip()
                if not usable(zh, en):
                    continue
                # One pair per line on either side: the corpus repeats popular films.
                key = hashlib.sha1(zh.encode()).hexdigest()
                other = hashlib.sha1(en.encode()).hexdigest()
                if key in seen or other in seen:
                    continue
                seen.update((key, other))
                kept.append((zh, en))

    print(f"{len(kept)} usable 1-to-1 pairs")
    sample = rng.sample(kept, min(args.pairs, len(kept)))
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / "opus-casual.zh.txt").write_text(
        "\n".join(zh for zh, _ in sample) + "\n", encoding="utf-8", newline="\n"
    )
    (OUT / "opus-casual.en.txt").write_text(
        "\n".join(en for _, en in sample) + "\n", encoding="utf-8", newline="\n"
    )
    lengths = sorted(han_count(zh) for zh, _ in sample)
    print(
        f"sampled {len(sample)}; Han chars p50 {lengths[len(lengths) // 2]}, "
        f"p90 {lengths[int(len(lengths) * 0.9)]}"
    )


if __name__ == "__main__":
    main()
