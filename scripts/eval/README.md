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
