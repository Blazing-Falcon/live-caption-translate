#!/usr/bin/env python3
"""Fetch the public test audio used by the real-model tests and replay benchmarks (see docs/development.md).

usage: python fetch-test-audio.py [--out testdata/fetched] [--fixtures reference/fixtures]

Needs: pip install huggingface_hub pyarrow soundfile numpy
Writes (all git-ignored):
  <out>/leijun/lei-jun.wav                      keynote, 16 kHz mono, 4.5 min (replay: --start 28 --dur 180)
  <out>/ramc/CTS-CN-F2F-2019-11-15-1449.wav     two-person conversation (replay: --start 19 --dur 180)
  <out>/ramc/CTS-CN-F2F-2019-11-15-1449.txt     its human transcript
  <out>/ascend/clip_00.wav .. clip_19.wav       20 code-switching clips for the ASR parity test
  <out>/ascend/clips.json                       reference text + expected SenseVoice 2024 output per clip
Licenses: MagicData-RAMC CC BY-NC-ND 4.0, ASCEND CC BY-SA 4.0, lei-jun.wav from the csukuangfj/vad repo.
These files are for local testing only; do not commit or redistribute them.
"""
import argparse, hashlib, io, json, os, random, sys, tarfile

SHA = {
    "lei-jun.wav": "ad12c2ee3b2d60ad5214d22e8a3e9002f1bad9c61f60c4b404ee206d60a66ded",
    "CTS-CN-F2F-2019-11-15-1449.wav": "7bec068537a9ea2eac379475f6934073d5ceb4c293651af3c1d51853f61cc1d5",
}
RAMC_ID = "CTS-CN-F2F-2019-11-15-1449"


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def check(path, name):
    got = sha256(path)
    if got != SHA[name]:
        sys.exit(f"hash mismatch for {path}: {got}")
    print(f"ok  {path}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default="testdata/fetched")
    ap.add_argument("--fixtures", default=os.path.join(os.path.dirname(__file__), "..", "reference", "fixtures"))
    ap.add_argument("--n-ascend", type=int, default=20)
    a = ap.parse_args()
    from huggingface_hub import hf_hub_download

    raw = os.path.join(a.out, "raw")
    os.makedirs(raw, exist_ok=True)

    # 1. Keynote
    d = os.path.join(a.out, "leijun")
    os.makedirs(d, exist_ok=True)
    p = os.path.join(d, "lei-jun.wav")
    if not (os.path.exists(p) and sha256(p) == SHA["lei-jun.wav"]):
        hf_hub_download("csukuangfj/vad", "lei-jun.wav", local_dir=d)
    check(p, "lei-jun.wav")

    # 2. Conversation: one file out of the RAMC dev archive (~850 MB download)
    d = os.path.join(a.out, "ramc")
    os.makedirs(d, exist_ok=True)
    wav = os.path.join(d, RAMC_ID + ".wav")
    if not (os.path.exists(wav) and sha256(wav) == SHA[RAMC_ID + ".wav"]):
        tgz = hf_hub_download("EaseZh/magicdata_ramc", "dev.tar.gz", repo_type="dataset", local_dir=os.path.join(raw, "ramc"))
        with tarfile.open(tgz) as t:
            for m in t.getmembers():
                base = os.path.basename(m.name)
                if base in (RAMC_ID + ".wav", RAMC_ID + ".txt") and m.isfile():
                    with t.extractfile(m) as src, open(os.path.join(d, base), "wb") as dst:
                        dst.write(src.read())
    check(wav, RAMC_ID + ".wav")

    # 3. ASCEND clips, selected exactly as reference/python/mixed_lang_bench.py does (seed 0), first N of the mixed set
    import numpy as np, pyarrow.parquet as pq, soundfile as sf
    pqf = hf_hub_download("CAiRE/ASCEND", "main/test-00000-of-00001.parquet", repo_type="dataset", local_dir=os.path.join(raw, "ascend"))
    rows = [r for r in pq.read_table(pqf).to_pylist() if r["duration"] >= 1.5]
    random.seed(0)
    sel = []
    for lang, n in [("mixed", 150), ("zh", 100), ("en", 80)]:
        c = [r for r in rows if r["language"] == lang]
        random.shuffle(c)
        sel += c[:n]
    expected = json.load(open(os.path.join(a.fixtures, "asr-expected.json"), encoding="utf-8"))
    d = os.path.join(a.out, "ascend")
    os.makedirs(d, exist_ok=True)
    out = []
    for i, r in enumerate(sel[: a.n_ascend]):
        audio = r["audio"]
        x, sr = sf.read(io.BytesIO(audio["bytes"]), dtype="float32")
        if x.ndim > 1:
            x = x.mean(axis=1)
        assert sr == 16000, sr
        name = f"clip_{i:02d}.wav"
        sf.write(os.path.join(d, name), x, sr, subtype="PCM_16")
        exp = expected[i] if i < len(expected) else None
        if exp and exp["ref"] != r["transcription"]:
            sys.exit(f"selection drifted at clip {i}: {r['transcription']!r} != {exp['ref']!r}")
        out.append({"file": name, "ref": r["transcription"], "language": r["language"],
                    "sv2024_auto": exp["sv2024_auto"] if exp else None})
    json.dump(out, open(os.path.join(d, "clips.json"), "w", encoding="utf-8"), ensure_ascii=False, indent=1)
    print(f"ok  {d}: {len(out)} clips")


if __name__ == "__main__":
    main()
