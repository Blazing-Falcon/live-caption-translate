# Evaluation

Scores a replay against the metrics this project cares about, per domain, never pooled.

## Commands

```powershell
# One domain, at the shipped settings, realtime.
./scripts/eval/run.ps1 -Id base -Domain drama

# One domain with a config override, quality only (no timing).
./scripts/eval/run.ps1 -Id t1-nocontext -Domain drama -Config .agent/experiments/configs/window-0.toml -Fast

# Conversation, the first 5 minutes.
./scripts/eval/run.ps1 -Id ramc -Domain ramc -Seconds 300

# Score a run that already exists.
.agent/venv/Scripts/python.exe -I scripts/eval/score.py .agent/experiments/runs/base-drama/out.jsonl `
    --label base-drama `
    --zh testdata/eval/drama-130s.zh.txt --en testdata/eval/drama-130s.en.txt
```

Each run writes `.agent/experiments/runs/<Id>-<domain>/` with `out.jsonl` (the transcript),
`out.events.jsonl` (every pipeline event, including the stats samples), `out.words.json` and
`score.json`.

## What is measured

From the transcript alone, so a run can be rescored without the audio:

- lines: final, failed, skipped, and lines per minute;
- cuts by reason, and how many live texts already carried a sentence mark;
- latency: speech end to English p50/p90, first token, and the stage split (close, second pass,
  translate, queued);
- prompt-cache misses: requests that reused no more than the frozen instruction;
- second-pass outcomes, and how many edits were identical, punctuation-only or character edits.
  `second_pass_p50` is only meaningful with the outcomes beside it: a `late` line reports a
  near-zero stage time, so a run with many of them shows a low p50 because it did less work, not
  because the decode got faster;
- shape: duration, Han characters, inter-line gaps, share of gaps at or above `asr.pause_s`;
- fragments: lines the pipeline flagged as stopping mid-thought and still translated. The flag is
  new: a transcript recorded before it has no field, so those runs report zero rather than a guess.

With a reference (`--zh` and `--en`) it also reports a character error rate over the joined text.
That is coarse: the reference and the replay do not cut the same clauses.

## Domains

| Domain | Audio | Reference |
|---|---|---|
| drama | `.agent/cn-translate-investigation/translate.wav` (130.8 s) | 26 Chinese sentences, hand-checked English, labels |
| ramc | `testdata/fetched/ramc/CTS-CN-F2F-2019-11-15-1449.wav` (33 min) | human transcript with speaker labels, no English |
| leijun | `testdata/fetched/leijun/lei-jun.wav` (4.5 min) | `lei-jun.srt`, 70 cues |
| ascend | `testdata/fetched/ascend/clip_*.wav` (20 clips) | `clips.json` |
| fleurs | `testdata/fetched/fleurs/fleurs-dev-50.wav` (664 s, 50 read sentences) | FLEURS Mandarin transcript and the human English of the same sentence |

`testdata/eval/drama-130s.en.txt` is a hand-checked draft by the agent that wrote it, not the
owner's: treat it as a regression reference, not as a gold standard.

## Timing runs

Latency and memory numbers are only valid with nothing else heavy running. Create
`.agent/experiments/measure.lock` before a timing run and delete it after; `-Fast` runs do not
need the lock because they do not measure time.

Report the run-to-run spread before claiming a difference: on the drama clip, three replays of
the same build spread 39 ms at p50 and 58 ms at p90.

## Adding a domain

1. Put the audio under `testdata/fetched/<name>/` (that path is git-ignored).
2. Add an entry to `$sources` in `run.ps1` with the wav and whatever references exist.
3. Add a reference file to `testdata/eval/` if you have one. Never edit an existing reference to
   make a run look better; add a new file.

## Translation quality, with a reference nobody here wrote

The drama reference is 26 lines drafted by this project. For ranking translators, use the subtitle
sample instead: each Chinese line is a human translation of an original English subtitle, so the
original is a real reference.

```powershell
# One time: the corpus (~345 MiB). curl.exe is named in full because PowerShell aliases
# `curl` to Invoke-WebRequest, which has no -L or -o; and the directory is made first,
# because .agent/ is not tracked and curl will not create it.
New-Item -ItemType Directory -Force .agent/tmp/opus | Out-Null
curl.exe -L -o .agent/tmp/opus/en-zh_cn.txt.zip `
    https://object.pouta.csc.fi/OPUS-OpenSubtitles/v2018/moses/en-zh_cn.txt.zip

# 600 lines, 6-24 Han characters, one-to-one alignments only, fixed seed.
.agent/venv/Scripts/python.exe scripts/eval/make_corpus.py --pairs 600

# Translate with a model, then score.
.agent/venv/Scripts/python.exe scripts/eval/translate_file.py `
    --in .agent/experiments/corpus/opus-casual.zh.txt `
    --out .agent/experiments/corpus/opus-casual.ternary.txt
.agent/venv/Scripts/python.exe scripts/eval/score_corpus.py `
    --hyp .agent/experiments/corpus/opus-casual.ternary.txt --label ternary
```

`score_corpus.py` needs `torch` and `unbabel-comet` in `.agent/venv`; it downloads COMET
(wmt22-comet-da, about 2.2 GiB) into `.agent/tmp/comet/` on first use. With two `--hyp`/`--label`
pairs it prints the paired bootstrap interval, which is what decides whether a difference is real:
600 lines at a 0.003 COMET gap is inside the interval.

Measured 2026-10-23 on a Ryzen 7 5700X with nothing else running, with
`translate_file.py` and `score_corpus.py` at their defaults: the shipping 1.25-bit model scores
0.7706 and Q4_K_M 0.7738, a mean of +0.0032 with a 95% CI of [-0.0015, +0.0081] over 600 lines
(200 better, 180 worse). Q4_K_M's translation p50 was 467 ms against 279 ms for the shipping model
over 80 lines of the same sample, measured with `.agent/research/tmp/q4_speed.py` and its server
flags. The sample is third-party text and stays in `.agent/experiments/corpus/`, which is not
committed.

## Speech to English, with a reference nobody here wrote

The subtitle sample scores the translator alone. FLEURS scores the whole pipeline: it reads the same
FLoRes sentences aloud in every language, so a Mandarin recording and the English text with the
same sentence id are a source and a human reference translation (CC-BY-4.0). It is read speech of
encyclopedic text, so it says nothing about conversation or drama; use it beside them, not instead.

```powershell
# One time: the transcripts and the dev audio (217 MB), at the download cap.
New-Item -ItemType Directory -Force testdata/fetched/fleurs/download | Out-Null
$fleurs = 'https://huggingface.co/datasets/google/fleurs/resolve/main/data'
curl.exe -L --limit-rate 2400k -o testdata/fetched/fleurs/download/cmn_hans_cn_dev.tsv "$fleurs/cmn_hans_cn/dev.tsv"
curl.exe -L --limit-rate 2400k -o testdata/fetched/fleurs/download/en_us_dev.tsv "$fleurs/en_us/dev.tsv"
curl.exe -L --limit-rate 2400k -o testdata/fetched/fleurs/download/cmn_hans_cn_dev.tar.gz "$fleurs/cmn_hans_cn/audio/dev.tar.gz"

# 50 sentences joined with 1.2 s of silence, fixed seed.
.agent/venv/Scripts/python.exe -I scripts/eval/make_fleurs.py --sentences 50

# Replay, then map the captions back to their sentences and score.
./scripts/eval/run.ps1 -Id base -Domain fleurs
.agent/venv/Scripts/python.exe -I scripts/eval/score_fleurs.py .agent/experiments/runs/base-fleurs/out.jsonl `
    --spans testdata/fetched/fleurs/fleurs-dev-50.spans.json --zh testdata/fetched/fleurs/fleurs-dev-50.zh.txt `
    --hyp-out .agent/experiments/runs/base-fleurs/fleurs.en.txt
.agent/venv/Scripts/python.exe scripts/eval/score_corpus.py --hyp .agent/experiments/runs/base-fleurs/fleurs.en.txt `
    --label base --src testdata/fetched/fleurs/fleurs-dev-50.zh.txt --ref testdata/fetched/fleurs/fleurs-dev-50.en.txt `
    --out .agent/experiments/runs/base-fleurs/comet.json
```

`score_fleurs.py` gives a caption to the sentence whose time span holds its midpoint, joins the
English of a sentence cut into clauses, and reports a per-sentence CER over Han characters. A
sentence with no English is written as an empty line, so `score_corpus.py` counts it as a miss.
