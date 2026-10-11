# Architecture

Live Translation captures the audio that a Windows PC plays. It recognizes Chinese speech with a streaming X-ASR model and translates it with a local llama.cpp model. It shows the result in an overlay that lets mouse clicks pass through. All processing is on the PC. The only network use is the model download.

## Repository

| Path | Contents |
|---|---|
| `crates/lt-core` | Pipeline, types, config, event bus, metrics, transcripts. No OS code and no engine code. |
| `crates/lt-audio` | WASAPI capture (system mix or selected apps), WAV replay, resampling, normalization |
| `crates/lt-sherpa` | Streaming speech recognition (X-ASR) |
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
|  lt-core pipeline: capture -> recognizer -> scheduler                    |
+-------------------------|------------------------------------------------+
                          | HTTP 127.0.0.1
             +------------v------------+
             | llama-server            |
             | Hy-MT2 1.8B Q4_0        |
             +-------------------------+
```

- The server is a child process of the app. A model crash does not stop the overlay.
- A Windows Job Object stops the server when the app stops, also after a crash.
- The supervisor checks `/health`, sends one warm-up request, then reports the server ready. After a crash it restarts the server after 1, 2 and 4 s. Then it reports a failure.

## Data flow

1. **Capture.** The audio callback copies samples into a lock-free ring buffer.
2. **Recognizer.** The capture adapter resamples to 16 kHz mono. The frontend thread sends all audio to the streaming X-ASR engine in pieces of 128 ms (four 32 ms frames). The engine keeps its state and decodes each piece once; its emitted words never change. A clause opens at its first word and gets its `UtteranceId` there. The text of the open clause is the live Chinese line.
3. **Commit.** The recognizer cuts a clause at a sentence mark (。？！) that two more words follow. It also cuts at a comma that 8 word tokens precede, but only after the clause is 2 s old: casual speech often runs on with commas, and an earlier comma usually splits a short phrase. After 30 s, a clause with no punctuation for 20 word tokens is cut at the widest gap. A clause also ends at a pause of `asr.pause_s` (the engine's own rule) or when the audio stops. The engine reports each word late, so its token times are moved earlier by a fixed delay for each chunk size. At a commit the cut goes to just before the next word.
4. **Filter.** When a clause closes, the filter removes tags and fillers, classifies the text, and drops empty text and one-character noise.
5. **Translation.** One request at a time. If the oldest final is more than 6 s behind, the translator skips older items. English speech is shown as heard. With `translate.window_chars` set, a final is translated together with the earlier lines that fit in that many characters, joined by `<br>`. The answer is prefilled with the English already shown for those lines, so Hy-MT2 writes only the new line's English and stops at the next `<br>`. The window ends at a line that has no English (skipped or failed), and restarts after 10 s without lines and at a discontinuity. An empty answer is translated again without the window.
6. **Events.** Each stage sends events to the bus. The bridge sends them to the windows. The transcript writer and the metrics read the same events. See [ipc.md](ipc.md).

## Threads

| Thread | Work |
|---|---|
| `lt-wasapi-capture` and adapters | Capture |
| `lt-frontend` | Send audio and breaks (pause, device change, end) to the recognizer |
| `lt-asr` | Streaming recognizer, commit rule, filter, clause start and end notices |
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
