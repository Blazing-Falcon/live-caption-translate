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
    --label base-drama --names testdata/eval/drama-130s.names.toml `
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
- second-pass outcomes, and how many edits were identical, punctuation-only or character edits;
- shape: duration, Han characters, inter-line gaps, share of gaps at or above `asr.pause_s`;
- name forms per person, from a names file;
- lyric lines that were translated anyway;
- fragments: an unfinished-looking source whose English reads as a complete sentence.

With a reference (`--zh` and `--en`) it also reports a character error rate over the joined text.
That is coarse: the reference and the replay do not cut the same clauses.

## Domains

| Domain | Audio | Reference |
|---|---|---|
| drama | `.agent/cn-translate-investigation/translate.wav` (130.8 s) | 26 Chinese sentences, hand-checked English, names, labels |
| ramc | `testdata/fetched/ramc/CTS-CN-F2F-2019-11-15-1449.wav` (33 min) | human transcript with speaker labels, no English |
| leijun | `testdata/fetched/leijun/lei-jun.wav` (4.5 min) | `lei-jun.srt`, 70 cues |
| ascend | `testdata/fetched/ascend/clip_*.wav` (20 clips) | `clips.json` |

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
