# Architecture

Live Translation captures the audio that a Windows PC plays. It recognizes Chinese speech with a streaming X-ASR model and translates it with a local llama.cpp model. It shows the result in an overlay that lets mouse clicks pass through. All processing is on the PC. The only network use is the model download.

## Repository

| Path | Contents |
|---|---|
| `crates/lt-core` | Pipeline, types, config, event bus, metrics, transcripts. No OS code and no engine code. |
| `crates/lt-audio` | WASAPI capture (system mix or selected apps), WAV replay, resampling, normalization |
| `crates/lt-sherpa` | Streaming X-ASR speech recognition (the second pass runs in llama-server) |
| `crates/lt-llm` | llama-server clients, the server supervisor, model downloads |
| `crates/lt-cli` | Headless runner: `replay`, `live`, `bench`, `models fetch` |
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
+--------------------------------|-------------------------|--------------+
                                 | HTTP 127.0.0.1          | HTTP 127.0.0.1
                    +------------v------------+  +---------v---------------+
                    | llama-server            |  | llama-server            |
                    | Hy-MT2 1.8B (ternary)   |  | Qwen3-ASR 0.6B          |
                    +-------------------------+  +-------------------------+
```

- Each server is a child process of the app. A model crash does not stop the overlay. The Qwen3-ASR server runs only with `asr.second_pass = "qwen3"` and starts and stops with the pipeline.
- A Windows Job Object stops the server when the app stops, also after a crash.
- The supervisor checks `/health`, sends one warm-up request, then reports the server ready. After a crash it restarts the server after 1, 2 and 4 s. Then it reports a failure.

## Data flow

1. **Capture.** The audio callback copies samples into a lock-free ring buffer.
2. **Recognizer.** The capture adapter resamples to 16 kHz mono. The frontend thread sends all audio to the streaming X-ASR engine in pieces of 128 ms (four 32 ms frames). The engine keeps its state and decodes each piece once; its emitted words never change. A clause opens at its first word and gets its `UtteranceId` there. The text of the open clause is the live Chinese line.
3. **Commit.** The recognizer cuts a clause at a sentence mark (。？！) that two more words follow. It also cuts at a comma that 8 word tokens precede, but only after the clause is 2 s old: casual speech often runs on with commas, and an earlier comma usually splits a short phrase. After 30 s, a clause with no punctuation for 20 word tokens is cut at the widest gap. A clause also ends at a pause of `asr.pause_s` (the engine's own rule) or when the audio stops. The engine reports each word late, so its token times are moved earlier by a fixed delay for each chunk size. At a commit the cut goes to just before the next word.
4. **Second pass.** With `asr.second_pass` on, `lt-asr` keeps the last 45 s of audio. When a clause closes, it copies the clause's audio from 0.3 s before its start to 0.8 s after its end (as far as heard, never across a break), and `lt-refine` decodes it with Qwen3-ASR in its own llama-server. The streaming model reports words late, so the cut times are not exact, and the window includes words of the clauses before and after. The live text decides which part belongs to the clause: the part of the second pass's text with the smallest edit distance to the live text (letters and digits; ties go to the part that starts and ends at punctuation, then to the length closest to the live text). That part becomes the final text unless it keeps fewer than 60% of the live characters or differs in more than 60% of them. When Qwen3-ASR tags Japanese, Korean or Cantonese and the live text has Chinese characters, it decodes again with Chinese forced. A clause that waits longer than `asr.second_pass_max_wait_s` for the second pass keeps the live text without a decode, so the queue catches up when the PC is busy. The live line and the clause cuts do not change; the line's Chinese changes once, when its final arrives.
5. **Filter.** After the second pass, the filter removes tags and fillers, classifies the text, and drops empty text and one-character noise. It runs on the final text, so a line is not dropped on the live text and kept on the corrected text, or the other way round.
6. **Translation.** One request at a time. If the oldest final is more than 6 s behind, the translator skips older items. Lines that wait while a request runs are joined into one request, up to `translate.queue_join_max_chars`, but never across a discontinuity or a gap of more than 10 s. English speech is shown as heard. With `translate.window_chars` set, a final is translated together with the earlier lines that fit in that many characters. The ternary model gets them as earlier chat turns with the English already shown; only the first turn has the instruction. Q4_0 gets them joined by `<br>`, and its answer is prefilled with the English already shown, so it writes only the new line's English and stops at the next `<br>`. The window ends at a line that has no English (skipped or failed), and restarts after 10 s without lines and at a discontinuity. An empty answer is translated again without the window.
7. **Events.** Each stage sends events to the bus. The bridge sends them to the windows. The transcript writer and the metrics read the same events. See [ipc.md](ipc.md).

## Threads

| Thread | Work |
|---|---|
| `lt-wasapi-capture` and adapters | Capture |
| `lt-frontend` | Send audio and breaks (pause, device change, end) to the recognizer |
| `lt-asr` | Streaming recognizer, commit rule, clause start and end notices, the audio of the last 45 s |
| `lt-refine` | Second pass (Qwen3-ASR) and filter for each closed clause, in order |
| `lt-scheduler` | Translation queue, stats |
| `lt-translator` | HTTP requests to the server |
| `lt-transcript` | Transcript writer |
| `lt-bridge`, `lt-controller`, `lt-models`, `lt-hover` | App: events, start and stop, downloads, panel hover |

- The pipeline code has one module for each thread in `crates/lt-core/src/pipeline/`; `recognizer.rs` holds the clause state machine that `lt-asr` runs.
- Each engine has one owner thread.
- The core has no async runtime. Stages use bounded channels, so a slow stage stops the stage before it.
- Each sample has a position on one stream clock. Timestamps and latency use this clock.

## Data folders

| Data | Folder |
|---|---|
| `config.toml` | `%APPDATA%\app.livetranslation.desktop\` |
| Models, transcripts, logs | `%LOCALAPPDATA%\app.livetranslation.desktop\` |

The CLI uses `models/` in the repository. `--models` or `LT_MODELS_DIR` changes it.
