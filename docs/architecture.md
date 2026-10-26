# Architecture

Live Translation captures the audio that a Windows PC plays. It recognizes Chinese speech with a streaming X-ASR model and translates it with a local llama.cpp model. It shows the result in an overlay that lets mouse clicks pass through. All processing is on the PC. The only network use is the model download.

## Repository

| Path | Contents |
|---|---|
| `crates/lt-core` | Pipeline, types, config, event bus, metrics, transcripts. No OS code and no engine code. |
| `crates/lt-audio` | WASAPI capture (system mix or selected apps), WAV replay, resampling, normalization |
| `crates/lt-sherpa` | X-ASR speech recognition: the streaming recognizer and its offline second pass |
| `crates/lt-llm` | llama-server clients, the server supervisor, model downloads |
| `crates/lt-cli` | Headless runner: `replay`, `models fetch` |
| `app/src-tauri` | Tauri 2 shell (Windows only): windows, tray, hotkeys, commands, event bridge |
| `app/ui` | Svelte UI: `overlay` and `control` |
| `models/manifest.json` | Model files, URLs and hashes |
| `reference/fixtures`, `testdata`, `*/tests/fixtures`, `app/ui/src/lib/__fixtures__` | Recorded test data |

`cargo build` without `--workspace` does not build `app/src-tauri`.

## Processes

```
+------------------------- live-translation.exe ---------------------------+
|  Tauri shell: overlay, control window, tray, hotkeys                     |
|  lt-core pipeline: capture -> recognizer -> second pass -> scheduler     |
+--------------------------------|-----------------------------------------+
                                 | HTTP 127.0.0.1
                    +------------v------------+
                    | llama-server            |
                    | Hy-MT2 1.8B (1.25-bit)  |
                    +-------------------------+
```

- Both X-ASR models (streaming and offline) run inside the app through sherpa-onnx.
- The server is a child process of the app. A model crash does not stop the overlay.
- A Windows Job Object stops the server when the app stops, also after a crash.
- The supervisor checks `/health`, sends one warm-up request, then reports the server ready. After a crash it restarts the server after 1, 2 and 4 s. Then it reports a failure.
- The controller loads the server once and keeps it across a pipeline restart. Only a change to `translate.threads` or `performance.low_priority`, a failed server, or a quit stops it.

## Data flow

1. **Capture.** The audio callback copies samples into a lock-free ring buffer.
2. **Recognizer.** The capture adapter resamples to 16 kHz mono. The frontend thread sends all audio to the streaming X-ASR engine in pieces of 128 ms (four 32 ms frames). The engine keeps its state and decodes each piece once with greedy search, so its emitted words never change. A clause opens at its first word and gets its `UtteranceId` there. The text of the open clause is the live Chinese line.
3. **Commit.** The recognizer cuts a clause at a sentence mark (。？！) that two more words follow. It also cuts at a comma that 8 word tokens precede, but only after the clause is 2 s old: casual speech often runs on with commas, and an earlier comma usually splits a short phrase. After 30 s, a clause with no punctuation for 20 word tokens is cut at the widest gap. A clause also ends at a pause of `asr.pause_s` (the engine's own rule) or when the audio stops. The engine reports each word late, so its token times are moved earlier by a fixed delay. At a commit the cut goes to just before the next word.
4. **Second pass.** `lt-asr` keeps the last 45 s of audio, and every closed clause goes through the offline X-ASR model. When a clause closes, `lt-asr` copies the clause's audio from 0.3 s before its start to 0.8 s after its end (as far as heard, never across a break), plus the context the engine asks for before that, and `lt-refine` decodes it. The engine is the offline X-ASR model in the app: the same zipformer transducer as the live recognizer, decoded over the whole window with full attention, with 5 s of context. A short clause alone loses its commas and guesses the words its context decides; with the speech before it, the offline model made fewer errors than the live text on drama, conversation, FLEURS and ASCEND. The streaming model reports words late, so the cut times are not exact, and the window includes words of the clauses before and after. The live text decides which part belongs to the clause: the part of the second pass's text with the smallest edit distance to the live text (letters and digits; ties go to the part that starts and ends at punctuation, then to the length closest to the live text). That part becomes the final text unless it keeps fewer than 60% of the live characters or differs in more than 60% of them. A clause that waits longer than 1.5 s for the second pass keeps the live text without a decode, so the queue catches up when the PC is busy. The live line and the clause cuts do not change; the line's Chinese changes once, when its final arrives.
5. **Filter.** After the second pass, the filter removes tags and fillers, classifies the text, and drops empty text and one-character noise. It runs on the final text, so a line is not dropped on the live text and kept on the corrected text, or the other way round.
6. **Translation.** One request at a time. The scheduler joins closed lines into one unit up to a sentence end, a pause or break cut, 10 s of silence, or 40 characters, and seals it with `unit_sealed`; a line shown as heard seals the unit before it. While nothing else waits, it translates the open unit's lines again as a preview: no deltas, at least 4 letters, and after the last preview ended it waits as long as that preview took, at least 300 ms. A preview answer with Chinese in it is not shown. The queue skips older lines when the oldest final is more than 6 s behind, and joins lines that wait while a request runs, up to 120 characters, never across a discontinuity or a gap of more than 10 s. Each request carries only its own text, as one user message with the frozen instruction. An empty answer, one far longer than its source, or one that repeats the instruction fails the line; an answer that keeps a Chinese proper noun is shown and counted. A final always goes before a preview.
7. **Events.** Each stage sends events to the bus. The bridge sends them to the windows. The transcript writer and the metrics read the same events. The overlay keeps a line for each id that events address and groups the lines into caption blocks: one block for each unit the core seals with `unit_sealed`, and one open block for the lines after the last seal. A block never splits or merges after it appears, so its row stays the same element and the text above it does not move. See [ipc.md](ipc.md).

## Overlay layout

Both styles fill from the top of a fixed box and scroll up, never down. Each block of the panel, and each paragraph of running text, sits in a `CaptionViewport`: an `overflow: hidden` box around a column moved by `translateY(-offset)`. The offset is the largest it has ever been, rounded up to the next block top (panel) or whole line (running text), so the top edge never cuts a block in half. Text that gets shorter leaves blank space at the bottom; the next block fills it. A block pruned above the viewport takes its height out of the offset in the same frame, so nothing visible moves. One 180 ms ease-out transition covers a scroll step; with `prefers-reduced-motion` the column jumps instead. The status is a pill over the box, so it never changes the box's height.

## Threads

| Thread | Work |
|---|---|
| `lt-wasapi-capture` and adapters | Capture |
| `lt-frontend` | Send audio and breaks (pause, device change, end) to the recognizer |
| `lt-asr` | Streaming recognizer, commit rule, clause start and end notices, the audio of the last 45 s |
| `lt-refine` | Second pass (offline X-ASR) and filter for every closed clause, in order |
| `lt-scheduler` | Translation queue, previews, stats |
| `lt-translator` | HTTP requests to the server |
| `lt-transcript` | Transcript writer |
| `lt-bridge`, `lt-controller`, `lt-models` | App: events, start and stop, downloads |

- The pipeline code has one module for each thread in `crates/lt-core/src/pipeline/`; `recognizer.rs`
  holds the clause state machine that `lt-asr` runs, and `cancel.rs` the stop flag they share.
- Each engine has one owner thread.
- The core has no async runtime. Stages use bounded channels, so a slow stage stops the stage before it.
- Each sample has a position on one stream clock. Timestamps and latency use this clock.

## Data folders

| Data | Folder |
|---|---|
| `config.toml` | `%APPDATA%\app.livetranslation.desktop\` |
| Models, transcripts, logs | `%LOCALAPPDATA%\app.livetranslation.desktop\` |

The CLI uses `models/` in the repository. `--models` or `LT_MODELS_DIR` changes it.
