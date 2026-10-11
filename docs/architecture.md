# Architecture

Live Translation captures the audio that a Windows PC plays. It recognizes Chinese speech with a streaming X-ASR model and translates it with two local llama.cpp models. It shows the result in an overlay that lets mouse clicks pass through. All processing is on the PC. The only network use is the model download.

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
+-------------------------|---------------------------|--------------------+
                          | HTTP 127.0.0.1            | HTTP 127.0.0.1
             +------------v------------+  +-----------v-------------+
             | llama-server (final)    |  | llama-server (draft)    |
             | Hy-MT2 1.8B Q4_0        |  | LMT-60 0.6B Q4_K_M      |
             +-------------------------+  +-------------------------+
```

- Both servers are child processes of the app. A model crash does not stop the overlay.
- A Windows Job Object stops the servers when the app stops, also after a crash.
- The draft server runs only in Continuous mode.
- The supervisor checks `/health`, sends one warm-up request, then reports the server ready. After a crash it restarts the server after 1, 2 and 4 s. Then it reports a failure.

## Data flow

1. **Capture.** The audio callback copies samples into a lock-free ring buffer.
2. **Recognizer.** The frontend thread resamples to 16 kHz mono and sends all audio to the streaming X-ASR engine in pieces of about 100 ms. The engine keeps its state and decodes each piece once; its emitted words never change. A clause opens at its first word and gets its `UtteranceId` there. The text of the open clause is the live Chinese line.
3. **Commit.** If a sentence mark is followed by two more words, the recognizer cuts the clause there. A comma can also end a clause after 8 word tokens. With no punctuation for 20 word tokens, it cuts at the widest gap. A clause also ends at a pause of `asr.stream_pause_s` (the engine's own rule) or when the audio stops. The engine reports each word late, so its token times are moved earlier by a fixed delay for each chunk size. At a commit the cut goes to just before the next word.
4. **Filter and joiner.** The filter removes tags and fillers, classifies the text, and drops empty text and one-character noise. The joiner holds short pause-closed phrases (8 Chinese characters or fewer) for up to 1 s and joins them to the next phrase. It does not hold committed clauses.
5. **Translation.** Finals and drafts share one slot: one request at a time, finals first, and only the newest draft waits. If the oldest final is more than 6 s behind, the translator skips older items. English speech is shown as heard. With `translate.window_chars` set, a final is translated together with the earlier lines that fit in that many characters, joined by `<br>`. The answer is prefilled with the English already shown for those lines, so Hy-MT2 writes only the new line's English and stops at the next `<br>`. The window ends at a line that has no English (skipped or failed), and restarts after 10 s without lines and at a discontinuity. An empty answer is translated again without the window.
6. **Drafts.** In Continuous mode, LMT-60 translates each longer Chinese prefix into a dimmed draft. The final translation replaces the draft.
7. **Events.** Each stage sends events to the bus. The bridge sends them to the windows. The transcript writer and the metrics read the same events. See [ipc.md](ipc.md).

## Threads

| Thread | Work |
|---|---|
| `lt-wasapi-capture` and adapters | Capture |
| `lt-frontend` | Resample, send audio to the recognizer |
| `lt-asr` | Streaming recognizer, commit rule, clause start and end notices |
| `lt-scheduler` | Filter, joiner, queue, drafts, step-down, stats |
| `lt-translator`, `lt-draft` | HTTP requests to the two servers |
| `lt-transcript` | Transcript writer |
| `lt-bridge`, `lt-controller`, `lt-models`, `lt-hover` | App: events, start and stop, downloads, panel hover |

- Each engine has one owner thread.
- The core has no async runtime. Stages use bounded channels, so a slow stage stops the stage before it.
- Each sample has a position on one stream clock. Timestamps and latency use this clock.

## Caption speeds

`latency.mode` sets the caption speed:

| Mode | Behavior | Servers |
|---|---|---|
| Continuous | Live Chinese, early commits, drafts, finals | 2 |
| Light | Live Chinese, early commits, finals | 1 |
| Off | Translate when the speaker pauses. With a streaming engine: live Chinese, and a clause ends at 。？！, a pause, or after 6 s at a comma | 1 |

- `auto` selects Continuous if the PC has 6 or more physical cores and the draft model is present. Otherwise it selects Light.
- `continuous` without the draft model runs Light, with the reason `draft_unavailable`.
- In Off with a streaming engine, a clause still open after 30 s is also cut by the word-cap rule.
- A change to a `latency` key restarts the pipeline.

## Step-down

In Continuous mode the scheduler takes one sample each second.

| Change | Condition |
|---|---|
| Down to Light | System CPU ≥ 80% for 3 samples, or translation lag ≥ 3 s for 2 samples |
| Up to Continuous | Only after an automatic step-down: CPU < 65% for 10 samples, lag < 1.5 s, and 10 s since the last change |
| Lock in Light | 3 step-downs in 5 minutes. The lock stays until listening starts again. |

- The lag counts only finals that wait for or are in translation. Phrases that the joiner holds do not count.
- The draft server continues to run after a step-down, so a step-up is immediate.
- `latency.step_down = false` disables step-down.

## Data folders

| Data | Folder |
|---|---|
| `config.toml` | `%APPDATA%\app.livetranslation.desktop\` |
| Models, transcripts, logs | `%LOCALAPPDATA%\app.livetranslation.desktop\` |

The CLI uses `models/` in the repository. `--models` or `LT_MODELS_DIR` changes it.
