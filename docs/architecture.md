# Architecture

Live Translation captures the audio that a Windows PC plays. It recognizes Chinese speech with a streaming X-ASR model and translates it with a local llama.cpp model. It shows the result in an overlay that lets mouse clicks pass through. A hotkey also freezes one monitor, reads the Chinese text in a box that the user drags, and paints the English over it. All processing is on the PC. The only network use is the model download.

## Repository

| Path | Contents |
|---|---|
| `crates/lt-core` | Pipeline, types, config, event bus, metrics, transcripts, and the screen logic (grouping text into blocks, the screen job). No OS code and no engine code. |
| `crates/lt-audio` | WASAPI capture (system mix or selected apps), WAV replay, resampling, normalization |
| `crates/lt-sherpa` | X-ASR speech recognition: the streaming recognizer and its offline second pass. It also loads the ONNX Runtime that `lt-ocr` uses. |
| `crates/lt-llm` | llama-server clients, the server supervisor, model downloads |
| `crates/lt-ocr` | PP-OCRv6 text recognition for screenshots: a detector and a recognizer on the shared ONNX Runtime |
| `crates/lt-cli` | Headless runner: `replay`, `screen`, `models fetch` |
| `app/src-tauri` | Tauri 2 shell (Windows only): windows, tray, hotkeys, commands, event bridge, screen capture |
| `app/ui` | Svelte UI: `overlay`, `control` and `screen` |
| `models/manifest.json` | Model files, URLs and hashes |
| `reference/fixtures`, `testdata`, `*/tests/fixtures`, `app/ui/src/lib/__fixtures__`, `app/ui/e2e/fixtures` | Recorded test data |

`cargo build` without `--workspace` does not build `app/src-tauri`.

## Processes

```
+------------------------- live-translation.exe ---------------------------+
|  Tauri shell: overlay, control window, screen window, tray, hotkeys      |
|  lt-core pipeline: capture -> recognizer -> second pass -> scheduler     |
|  lt-screen thread: screenshot text recognition and its translation       |
+--------------------------------|-----------------------------------------+
                                 | HTTP 127.0.0.1
                    +------------v------------+
                    | llama-server            |
                    | Hy-MT2 1.8B (1.25-bit)  |
                    +-------------------------+
```

- Both X-ASR models (streaming and offline) run inside the app through sherpa-onnx.
- The text recognition model for screenshots runs inside the app through `lt-ocr`, on the same ONNX Runtime. The `lt-screen` thread loads it when a screenshot first needs it and drops it after 5 minutes without a job.
- The server is a child process of the app. A model crash does not stop the overlay.
- A Windows Job Object stops the server when the app stops, also after a crash.
- The supervisor checks `/health`, sends one warm-up request, then reports the server ready. After a crash it restarts the server after 1, 2 and 4 s. Then it reports a failure.
- The controller starts the server when listening starts or when a screenshot needs it, whichever comes first. It keeps the server across a pipeline restart and a pause. Only a change to `translate.threads` or `performance.low_priority`, a failed server, or a quit stops it.
- The server has two clients: the caption translator and the `lt-screen` thread. It has one slot (`-np 1`), so each client sends one request at a time and a request from one client can wait for one request of the other.

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

## Screenshot translation

The `screen_translate` hotkey starts it. It needs the text recognition model, which downloads the first time (see [models.md](models.md)), and the translation model. It does not need listening to run.

```
hotkey -> capture the monitor -> screen window shows the frozen frame -> the user drags a box
       -> screen_select(frame, rect) -> lt-screen: crop, lt-ocr reads lines, lt-core groups blocks
       -> one request per block to llama-server -> screen window paints the English over each block
```

1. **Capture.** The app copies the monitor under the cursor with GDI `BitBlt` before any window of ours changes, so none of them is in the copy. The caption overlay is in it when it covers the area; its English is not translated. The frame is in physical pixels and carries the monitor's DPI scale.
2. **Frozen view.** The `screen` window is created hidden at startup. The hotkey moves it onto the monitor, sends `screen://frame` and shows it. The page loads the frame from the `ltshot` protocol as a 32-bit BMP. The hotkey again, Esc, a right click or Alt+F4 closes it and drops the frame.
3. **Preparing.** At the same moment, off the hotkey's thread, `lt-screen` loads the recognizer if it is not loaded, and the controller starts the translation server if it is not running. Both are usually ready before the user finishes the drag.
4. **Reading.** `screen_select` cuts the selected pixels out of the frozen frame and sends them to `lt-screen`. A new job cancels the previous one. The detector aligns and pads its input, returns quadrilaterals, and repairs split horizontal rows. The recognizer rectifies and reads each crop. The last nonempty recognition is reused for the same frame, crop and direction. Core logic folds accents, removes icon noise, checks box shape, measures glyphs and ink, and straightens consistent tilted cards for General grouping, restores their image geometry and levels tilts that cause extra collisions. It samples source colors, removes text once from the frame and retains the repaired backdrop for this job. The job sends `blocks`, optionally including raw line rectangles for debugging.
5. **Translating.** The job translates the blocks that contain a Han character, in reading order, one request at a time. A block longer than 120 characters is split into pieces at sentence marks, then at commas, then at 120 characters, and the English pieces are joined. The requests are the same as the caption translator's. The same text on one screen, or on a later one, comes from a cache of 512 entries. A block that fails does not stop the next one. The job sends `translated` or `failed` for each block, then `done`.
6. **Painting.** The browser measures English with the same font, weight and line spacing it draws. Pure layout matches source glyph size, fits single lines or groups, and limits expansion around neighbouring text. Final geometry requests a local background plate and foreground color from the retained repaired frame. Stale job requests and image revisions are rejected. English glyphs may extend beyond their logical box so browser rasterization preserves complete descenders; background geometry and fitting stay unchanged. D toggles raw line outlines and expanded group outlines; holding Space shows the original frame.

Coordinates have one rule each. A frame is in physical pixels from the monitor's top-left. The UI multiplies CSS pixels by `scale` to get frame pixels and divides to go back, and `lib/screen.ts` is the only code that does either. The recognizer returns pixels of the cropped box, and `lt-screen` adds the box's origin, so every rect that leaves the app is in frame pixels. The events and their order are in [ipc.md](ipc.md#screen-jobs).

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
| `lt-screen` | Screenshot translation: the text recognizer, one screen job at a time, the translation requests of that job |
| `lt-bridge`, `lt-controller`, `lt-models` | App: events, start and stop, downloads |

- The pipeline code has one module for each thread in `crates/lt-core/src/pipeline/`; `recognizer.rs`
  holds the clause state machine that `lt-asr` runs, and `cancel.rs` the stop flag they share.
- Each in-process engine has one owner thread: the streaming recognizer (`lt-asr`), the second pass (`lt-refine`) and the text recognizer (`lt-screen`). The llama-server process is the exception. It takes requests from `lt-translator` and `lt-screen`, one request at a time from each.
- The core has no async runtime. Stages use bounded channels, so a slow stage stops the stage before it.
- Each sample has a position on one stream clock. Timestamps and latency use this clock.

## Data folders

| Data | Folder |
|---|---|
| `config.toml` | `%APPDATA%\app.livetranslation.desktop\` |
| Models, transcripts, logs | `%LOCALAPPDATA%\app.livetranslation.desktop\` |

The CLI uses `models/` in the repository. `--models` or `LT_MODELS_DIR` changes it.
