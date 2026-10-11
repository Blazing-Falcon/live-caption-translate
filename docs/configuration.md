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

[filter]
single_char_max_s = 0.6
single_char_allow = ["对", "好", "是", "行", "不", "哦"]
fillers = ["呃", "嗯", "额", "uh", "um"]

[translate]
server_url = ""                 # "" = spawn the bundled llama-server; otherwise use this OpenAI-compatible base URL
threads = 2                     # 1 to the logical CPUs of the PC
ctx = 1024
temperature = 0.0
repeat_penalty = 1.05
max_tokens_cap = 256
timeout_s = 10.0
queue_join_max_chars = 120
skip_lag_s = 6.0
window_chars = 30               # 0 = translate each line alone

[overlay]
visible = true
style = "bar"                   # "bar" (subtitle bar) | "panel" (caption panel) | "flow" (running text)
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

### filter

| Key | Range | Applies | Meaning |
|---|---|---|---|
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
| `max_tokens_cap` | 32–1024 | pipeline | Maximum output tokens: `min(cap, 4 × characters + 16)` |
| `timeout_s` | 2–60 | pipeline | Time limit for one request |
| `queue_join_max_chars` | 20–300 | pipeline | Maximum text joined from waiting items |
| `skip_lag_s` | 2–30 | pipeline | Lag at which the translator skips older items |
| `window_chars` | 0–60 | pipeline | Translate each line together with the earlier lines that fit in this many characters. Their English is given as the start of the answer, so only the new line is translated. Whole lines only. `0`: off. |

### overlay

| Key | Range | Applies | Meaning |
|---|---|---|---|
| `visible` | bool | live | Show the overlay |
| `style` | `bar`, `panel`, `flow` | live | Subtitle bar, caption panel, or running text: the lines joined into one English paragraph (and one Chinese paragraph with `show_source`), newest at the end |
| `font_px` | 18–40 | live | English text size. Chinese is 0.65 × this size. Older lines are 0.8 × this size. |
| `background` | 0–1 | live | Background opacity. The control window warns below 0.5. |
| `show_source` | bool | live | Show the Chinese line above the English line |
| `expire_s` | 3–60 | live | Remove lines after this time without new speech |
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
