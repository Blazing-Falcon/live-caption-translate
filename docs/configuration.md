# Configuration

The app reads `config.toml` from the app config folder (`%APPDATA%\app.livetranslation.desktop\`). `LT_CONFIG` sets a different file.

Rules:

- A missing key or a missing file gets the default value.
- The app keeps unknown keys and logs a warning.
- A value with the wrong type gets the default value. The app logs a warning.
- A number out of range is clamped to the nearest limit.
- An enum value that is not permitted gets the default value.
- A file that does not load (not valid TOML, not UTF-8, or not readable) is renamed to `config.toml.bad-<seconds>`, and the app starts with the defaults. A notice gives the new name.
- The app writes `config.toml.tmp` and renames it, so a save is atomic.
- A file with `config_version = 1` is migrated when it loads. Renamed keys keep their values (`asr.stream_chunk_ms` becomes `asr.chunk_ms`, `asr.stream_pause_s` becomes `asr.pause_s`, `latency.low_priority` becomes `performance.low_priority`). `translate.threads = 0` becomes `2`. Keys that version 2 does not have are removed, and the app logs one info line with their names. The next save writes version 2.
- Choices that the app removed load without a warning: `translate.model` and `translate.glossary` are dropped, and `asr.second_pass = "sensevoice"` becomes the default. The app logs one info line.

**Applies** tells when a change has an effect:

| Value | Effect |
|---|---|
| `live` | Immediately |
| `capture` | The capture sources open again. The pipeline continues. |
| `pipeline` | The pipeline restarts. The llama-server restarts only if one of these changes: `translate.server_url`, `translate.threads`, `translate.ctx` or `performance.low_priority`. |
| `restart` | At the next app start |

## Full Default File

```toml
config_version = 2

[capture]
mode = "system"                 # "system" | "apps"
device = "default"              # "default" (follow Windows default output) or a device id from list_audio_devices
apps = []                       # selected apps, persisted by executable name:
# apps = [{ exe = "chrome.exe", name = "Google Chrome" }, { exe = "Discord.exe", name = "Discord" }]
autostart = true                # start listening when the app opens (after models are ready)

[audio]
normalize = true
norm_attack_s = 1.0
norm_release_s = 5.0
norm_max_gain_db = 20.0
norm_gate_dbfs = -60.0

[asr]
chunk_ms = 480                  # 160 | 480 | 960
threads = 1                     # 1 to the logical CPUs of the PC
pause_s = 0.8                   # silence that ends a line
decoding = "greedy"             # "greedy" | "beam": the live recognizer's search
second_pass = "xasr"            # "off" | "xasr": read each closed line again offline
second_pass_threads = 2         # 1 to the logical CPUs of the PC
second_pass_max_wait_s = 1.5    # after this, the line keeps the live text
second_pass_min_logprob = 0.0   # skip the second pass for a line the live model was surer of than this; 0 reads every line

[filter]
skip_music = false              # do not translate a line that looks like song lyrics
single_char_max_s = 0.6
single_char_allow = ["对", "好", "是", "行", "不", "哦"]
fillers = ["呃", "嗯", "额", "uh", "um"]

[translate]
server_url = ""                 # "" = spawn the bundled llama-server; otherwise use this OpenAI-compatible base URL
threads = 4                     # 1 to the logical CPUs of the PC
ctx = 1024
temperature = 0.0
repeat_penalty = 1.05
max_tokens_cap = 256
timeout_s = 10.0
queue_join_max_chars = 120
skip_lag_s = 6.0
window_chars = 60               # 0 = translate each line alone
fragment_ellipsis = true        # mark a clause that stopped mid-thought with an ellipsis

[overlay]
visible = true
style = "panel"                 # "bar" (subtitle bar) | "panel" (caption panel) | "flow" (running text)
font_px = 26
background = 0.82               # 0.0 to 1.0
show_source = true              # show the Chinese line above the English
expire_s = 8.0
panel_edge = "right"            # "left" | "right"
panel_lines = 5
flow_lines = 3
live_source = true              # show the Chinese of the clause being spoken
# Saved positions, written by the app after move mode. Absent = default placement.
# [overlay.bar_rect]   monitor = "\\\\.\\DISPLAY1", x = 0, y = 0, w = 960, h = 140   (device-independent pixels)
# [overlay.panel_rect] monitor = "\\\\.\\DISPLAY1", x = 0, y = 0, w = 420, h = 600
# [overlay.flow_rect]  monitor = "\\\\.\\DISPLAY1", x = 0, y = 0, w = 960, h = 360

[performance]
low_priority = true             # below-normal priority for the translation server

[hotkeys]
move_lock = "Ctrl+Shift+L"
show_hide = "Ctrl+Shift+H"
pause = "Ctrl+Shift+P"
settings = "Ctrl+Shift+S"

[transcript]
enabled = true
retention_days = 30             # 0 = keep forever

[logging]
level = "info"                  # "error" | "warn" | "info" | "debug" | "trace"
keep_files = 5

[models]
source = "huggingface"          # "huggingface" | "modelscope"
dir = ""                        # "" = <local data>/models
```

## Keys

The default values are in the file above.

### capture

| Key | Range | Applies | Meaning |
|---|---|---|---|
| `mode` | `system`, `apps` | capture | Capture the full system mix, or only the selected apps. Below Windows build 20348, `apps` falls back to `system`. |
| `device` | `default` or a device id | capture | Output device for system mode. `default` follows the Windows default. |
| `apps` | list of `{ exe, name }` | capture | Selected apps. The match uses `exe` and ignores case. |
| `autostart` | bool | restart | Start to listen when the app opens and the models are ready. |

### audio

| Key | Range | Applies | Meaning |
|---|---|---|---|
| `normalize` | bool | pipeline | Slow automatic gain before the recognizer |
| `norm_attack_s` | 0.1–5 | pipeline | Time to decrease gain for loud input |
| `norm_release_s` | 0.5–20 | pipeline | Time to increase gain for quiet input |
| `norm_max_gain_db` | 0–30 | pipeline | Maximum gain |
| `norm_gate_dbfs` | -90 to -30 | pipeline | No gain below this level. Silence stays silent. |

### asr

| Key | Range | Applies | Meaning |
|---|---|---|---|
| `chunk_ms` | `160`, `480`, `960` | pipeline | Streaming X-ASR model. 160: words appear soonest, about twice the CPU of 480. 480: fewest errors. 960: least CPU. |
| `threads` | 1–64 | pipeline | ONNX Runtime threads for the recognizer. The app uses at most the PC's logical CPUs. |
| `pause_s` | 0.3–3.0 | pipeline | Silence after the last word that ends a line. Lines also end at sentence marks (the commit rule). |
| `decoding` | `greedy`, `beam` | pipeline | The live recognizer's search. `greedy` never changes a word once shown. `beam` (modified beam search, 4 paths) may revise the last words of the live line; on the four test sets it made 257 character errors against 261 and 0.0865 against 0.0962 on mixed Chinese and English, but wrote fewer sentence ends, which end lines (Python replay of the app's stream loop, sherpa-onnx 1.13.8, Ryzen 7 5700X, 2026-10-25). |
| `second_pass` | `off`, `xasr` | pipeline | When a line ends, a second recognizer reads its audio again. Its text replaces the live Chinese before translation if it matches the live text closely enough. `xasr`: the offline X-ASR model in the app, which also reads 5 s of speech before the line. |
| `second_pass_threads` | 1–64 | pipeline | Threads for the second-pass model. The app uses at most the PC's logical CPUs. Offline X-ASR per line, median: 406 ms on 1 thread, 197 on 2, 158 on 4 (40 conversation lines, Python, Ryzen 7 5700X, 2026-10-25). |
| `second_pass_max_wait_s` | 0.5–5.0 | pipeline | A line that waits longer than this for the second pass keeps the live text |
| `second_pass_min_logprob` | -5.0–0.0 | pipeline | Run the second pass only on a line whose mean token log-probability is below this. `0` decodes every line. Reading every line made the fewest errors (238 against 263 live, drama, conversation and FLEURS pooled). |

### filter

| Key | Range | Applies | Meaning |
|---|---|---|---|
| `skip_music` | true or false | pipeline | Do not translate a line the recognizer was much less sure of than the session's speech, which is what singing sounds like to it. The line still shows, marked as song; only its English is left out. |
| `single_char_max_s` | 0 or more | pipeline | Drop a one-character result from a clause shorter than this, unless the character is in `single_char_allow` |
| `single_char_allow` | list of strings | pipeline | One-character results that the filter always keeps |
| `fillers` | list of strings | pipeline | Words removed when they stand alone |

### translate

| Key | Range | Applies | Meaning |
|---|---|---|---|
| `server_url` | URL | pipeline | Empty: start and supervise the bundled llama-server. Set: use this OpenAI-compatible server. |
| `threads` | 1–64 | pipeline | llama-server threads. The app uses at most the PC's logical CPUs. |
| `ctx` | 512–4096 | pipeline | Context size (`-c`) |
| `temperature` | 0–1 | pipeline | Sampling temperature |
| `repeat_penalty` | 1.0–1.3 | pipeline | Repeat penalty |
| `max_tokens_cap` | 32–1024 | pipeline | Maximum output tokens: `min(cap, 2 × characters + 8)`, at least 24. Keep it at or above `2 × queue_join_max_chars + 8`, or a long joined line is cut short. |
| `timeout_s` | 2–60 | pipeline | Time limit for one request |
| `queue_join_max_chars` | 20–300 | pipeline | Maximum text joined from waiting items. Items are not joined across a pause, a device change or a gap of more than 10 s. |
| `skip_lag_s` | 2–30 | pipeline | Lag at which the translator skips older items |
| `window_chars` | 0–60 | pipeline | Translate each line together with the earlier lines that fit in this many characters, whole lines only. The window grows between resets and starts over when the next line would exceed it, so the server keeps its prompt cache. They go as earlier chat turns with their English as shown, so only the new line is translated. `0`: off. Measured on 300 subtitle lines with COMET, the window is a small quality *loss* (0.7754 alone against 0.7717 with two lines and 0.7718 with five, 75 lines worse against 59 better): keep it for what it gives the reader, not as a translation improvement. |
| `fragment_ellipsis` | true or false | pipeline | Append `…` to a clause that stopped mid-thought, so the translator does not invent an ending. A clause whose words say it is still going, or whose next clause opens with a connective, gets the mark; an echoed ellipsis is removed from the answer. |

### overlay

| Key | Range | Applies | Meaning |
|---|---|---|---|
| `visible` | bool | live | Show the overlay |
| `style` | `bar`, `panel`, `flow` | live | Subtitle bar, caption panel, or running text: the lines joined into one English paragraph (and one Chinese paragraph with `show_source`), newest at the end. The caption panel and running text translate differently from the bar: a sentence's clauses are translated together, and the words still being spoken are translated again whenever the translator is free, so English appears while someone speaks and may change until its sentence is translated for good. Whole sentences scored higher than their clauses translated one by one (COMET +0.0158 on 150 FLEURS sentences; +0.0096 on 50 replayed, with the 40-character limit), In real-time replays new words reached English after 489 ms (median) instead of 2028 ms on drama and 565 ms instead of 2484 ms on conversation, and the translation server used 128% of one core instead of 36% on drama and 269% instead of 55% on conversation (`lt-cli replay`, 4 translator threads, Ryzen 7 5700X, 2026-10-25). It runs at below-normal priority with "Give games priority" on. Both keep their text through silence. |
| `font_px` | 12–40 | live | English text size. Chinese is 0.65 × this size, at least 12 px. Older lines are 0.8 × this size. |
| `background` | 0–1 | live | Background opacity. The control window warns below 0.5. |
| `show_source` | bool | live | Show the Chinese line above the English line |
| `expire_s` | 3–60 | live | Subtitle bar only: remove lines after this time without new speech or translation. The caption panel and running text keep their text until newer text needs the room. |
| `panel_edge` | `left`, `right` | live | Default edge for the panel |
| `panel_lines` | 4–6 | live | Lines in the panel |
| `flow_lines` | 2–6 | live | Wrapped lines kept of each running-text paragraph. Older text scrolls out at the top. |
| `live_source` | bool | live | Show the Chinese of the clause that is spoken now |
| `bar_rect`, `panel_rect`, `flow_rect` | `{ monitor, x, y, w, h }` | live | Saved position in device-independent pixels. The app writes them after move mode. Absent: default position. |

### performance

All keys have the `pipeline` value: any change in `[performance]` restarts the pipeline.

| Key | Range | Meaning |
|---|---|---|
| `low_priority` | bool | Run the translation server at below-normal priority ("Give games priority") |

### hotkeys

Values use the Tauri global-shortcut format, for example `Ctrl+Shift+L`. An empty string disables the hotkey. All keys apply `live`.

| Key | Action |
|---|---|
| `move_lock` | Move or lock the overlay |
| `show_hide` | Show or hide the overlay |
| `pause` | Pause or resume |
| `settings` | Open the control window. The overlay lets clicks through, so this is the way back to it from a game |

### transcript

| Key | Range | Applies | Meaning |
|---|---|---|---|
| `enabled` | bool | live | Write one `transcripts/YYYY-MM-DD_HHMMSS.jsonl` file for each session |
| `retention_days` | 0 or more | restart | Delete older files at startup. `0`: keep all files. |

### logging

| Key | Range | Applies | Meaning |
|---|---|---|---|
| `level` | `error`, `warn`, `info`, `debug`, `trace` | restart | Log level of the app crates. Other crates log at `warn`. |
| `keep_files` | 1 or more | restart | Daily log files to keep |

### models

| Key | Range | Applies | Meaning |
|---|---|---|---|
| `source` | `huggingface`, `modelscope` | live | Download source |
| `dir` | folder | restart | Model folder. Empty: `models` in the local data folder. |

## Environment variables

For development only. See [development.md](development.md#environment-variables).
