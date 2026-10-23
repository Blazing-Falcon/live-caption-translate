"""Build a read-speech domain from FLEURS Mandarin, with a human English reference per sentence.

FLEURS reads the same FLoRes sentences in every language, so the Mandarin recording of a sentence
and the English text with the same id are a source and its reference translation that nobody on
this project wrote. The clips are joined with silence into one file, because the pipeline is a
stream; `score_fleurs.py` maps each caption back to its clip by time.

  # One time, at the download cap (FLEURS is CC-BY-4.0):
  mkdir testdata/fetched/fleurs/download
  curl -L --limit-rate 2400k -o testdata/fetched/fleurs/download/cmn_hans_cn_dev.tsv \
      https://huggingface.co/datasets/google/fleurs/resolve/main/data/cmn_hans_cn/dev.tsv
  curl -L --limit-rate 2400k -o testdata/fetched/fleurs/download/en_us_dev.tsv \
      https://huggingface.co/datasets/google/fleurs/resolve/main/data/en_us/dev.tsv
  curl -L --limit-rate 2400k -o testdata/fetched/fleurs/download/cmn_hans_cn_dev.tar.gz \
      https://huggingface.co/datasets/google/fleurs/resolve/main/data/cmn_hans_cn/audio/dev.tar.gz

  python scripts/eval/make_fleurs.py --sentences 50

Writes `testdata/fetched/fleurs/fleurs-dev-<n>.{wav,zh.txt,en.txt,spans.json}`.
"""

import argparse
import csv
import io
import json
import random
import sys
import tarfile
from pathlib import Path

import numpy as np
import soundfile

sys.stdout.reconfigure(encoding="utf-8")

ROOT = Path(__file__).resolve().parents[2]
FETCHED = ROOT / "testdata/fetched/fleurs"
RATE = 16_000
# Longer than `asr.pause_s` (0.8 s), so every sentence ends in a pause cut, as a speaker's
# sentence would.
GAP_S = 1.2


def read_tsv(path):
    """FLEURS rows: id, file, raw text, normalized text, characters, samples, gender."""
    csv.field_size_limit(1 << 20)
    with open(path, encoding="utf-8", newline="") as handle:
        return list(csv.reader(handle, delimiter="\t", quoting=csv.QUOTE_NONE))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--sentences", type=int, default=50)
    parser.add_argument("--seed", type=int, default=7)
    parser.add_argument("--download", default=str(FETCHED / "download"))
    args = parser.parse_args()
    download = Path(args.download)

    english = {}
    for row in read_tsv(download / "en_us_dev.tsv"):
        english.setdefault(row[0], row[2])
    # One recording per sentence, the first listed, so a speaker is not chosen by the seed.
    recordings = {}
    for row in read_tsv(download / "cmn_hans_cn_dev.tsv"):
        if row[0] in english:
            recordings.setdefault(row[0], (row[1], row[2]))
    ids = sorted(recordings, key=int)
    chosen = sorted(random.Random(args.seed).sample(ids, args.sentences), key=int)
    wanted = {recordings[sentence][0]: sentence for sentence in chosen}

    clips = {}
    with tarfile.open(download / "cmn_hans_cn_dev.tar.gz", "r:gz") as archive:
        for member in archive:
            name = Path(member.name).name
            if member.isfile() and name in wanted:
                data = archive.extractfile(member).read()
                samples, rate = soundfile.read(io.BytesIO(data), dtype="float32")
                if rate != RATE or samples.ndim != 1:
                    raise SystemExit(f"{name}: expected 16 kHz mono, got {rate} Hz {samples.shape}")
                clips[wanted[name]] = samples
    missing = [sentence for sentence in chosen if sentence not in clips]
    if missing:
        raise SystemExit(f"audio missing for sentence ids {missing}")

    gap = np.zeros(int(GAP_S * RATE), dtype=np.float32)
    pieces, spans, at = [gap], [], len(gap)
    for sentence in chosen:
        samples = clips[sentence]
        end = at + len(samples)
        spans.append({"id": sentence, "start_ms": at * 1000 // RATE, "end_ms": end * 1000 // RATE})
        pieces += [samples, gap]
        at = end + len(gap)

    FETCHED.mkdir(parents=True, exist_ok=True)
    stem = FETCHED / f"fleurs-dev-{args.sentences}"
    soundfile.write(f"{stem}.wav", np.concatenate(pieces), RATE, subtype="PCM_16")
    chinese = "".join(recordings[sentence][1] + "\n" for sentence in chosen)
    Path(f"{stem}.zh.txt").write_text(chinese, encoding="utf-8")
    Path(f"{stem}.en.txt").write_text("".join(english[s] + "\n" for s in chosen), encoding="utf-8")
    Path(f"{stem}.spans.json").write_text(json.dumps(spans, indent=1), encoding="utf-8")
    print(f"{stem}.wav: {len(chosen)} sentences, {at / RATE:.1f} s")


if __name__ == "__main__":
    main()
