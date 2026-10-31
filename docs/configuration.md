# Configuration

The app reads `config.toml` from the app config folder (`%APPDATA%\app.livetranslation.desktop\`). `LT_CONFIG` sets a different file.

Rules:

- A missing key or a missing file gets the default value.
- A value with the wrong type gets the default value, and the app logs a warning.
- A number out of range is clamped to the nearest limit, and the app logs a warning.
- A choice that is not permitted gets the default value, and the app logs a warning.
- A key the app does not know is dropped on load and named in one info line.
- A file that does not load (not valid TOML, not UTF-8, or not readable) is renamed to `config.toml.bad-<seconds>`, and the app starts with the defaults. A notice gives the new name.
- The app writes `config.toml.tmp` and renames it, so a save is atomic.
- The file is version 3. A file at version 1 or 2 loads like any other: known keys keep their values, unknown keys drop, `overlay.style = "bar"` becomes `panel`, and the next save writes version 3.

**Applies** tells when a change has an effect:

| Value | Effect |
|---|---|
| `live` | Immediately |
| `capture` | The capture sources open again. The pipeline continues. |
| `pipeline` | The pipeline restarts. The llama-server restarts only if one of these changes: `translate.threads` or `performance.low_priority`. |
| `restart` | At the next app start |

## Full Default File

```toml
config_version = 3

[capture]
mode = "system"                 # "system" | "apps"
device = "default"              # "default" (follow Windows default output) or a device id from list_audio_devices
apps = []                       # selected apps, persisted by executable name:
# apps = [{ exe = "chrome.exe", name = "Google Chrome" }, { exe = "Discord.exe", name = "Discord" }]
autostart = true                # start listening when the app opens (after models are ready)

[audio]
normalize = true

[asr]
threads = 1                     # 1 to the logical CPUs of the PC
pause_s = 0.8                   # silence that ends a line
second_pass_threads = 2         # 1 to the logical CPUs of the PC

[translate]
threads = 4                     # 1 to the logical CPUs of the PC

[overlay]
visible = true
style = "panel"                 # "panel" (caption panel) | "flow" (running text)
font_px = 26
background = 0.82               # 0.0 to 1.0
show_source = true              # show the Chinese line above the English
panel_edge = "right"            # "left" | "right"
panel_lines = 5
flow_lines = 3
live_source = true              # show the Chinese of the clause being spoken
# Saved positions, written by the app after move mode. Absent = default placement.
# [overlay.panel_rect] monitor = "\\\\.\\DISPLAY1", x = 0, y = 0, w = 420, h = 600
# [overlay.flow_rect]  monitor = "\\\\.\\DISPLAY1", x = 0, y = 0, w = 960, h = 360

[performance]
low_priority = true             # below-normal priority for the translation server

[screen]
direction = "horizontal"        # "horizontal" | "vertical"; remembered screenshot picker

[hotkeys]
move_lock = "Ctrl+Shift+L"
show_hide = "Ctrl+Shift+H"
pause = "Ctrl+Shift+P"
settings = "Ctrl+Shift+S"
screen_translate = "Ctrl+Shift+A"

[transcript]
enabled = true
retention_days = 30             # 0 = keep forever

[logging]
level = "info"                  # "error" | "warn" | "info" | "debug" | "trace"
keep_files = 5

[models]
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
| `normalize` | bool | pipeline | Slow automatic gain before the recognizer. The gain settings are fixed. |

### asr

The second pass always runs: each closed line is decoded again by the offline X-ASR model.

| Key | Range | Applies | Meaning |
|---|---|---|---|
| `threads` | 1–64 | pipeline | ONNX Runtime threads for the recognizer. The app uses at most the PC's logical CPUs. |
| `pause_s` | 0.3–3.0 | pipeline | Silence after the last word that ends a line. Lines also end at sentence marks (the commit rule). |
| `second_pass_threads` | 1–64 | pipeline | Threads for the second-pass model. The app uses at most the PC's logical CPUs. |

### translate

The app always starts and supervises the bundled llama-server.

| Key | Range | Applies | Meaning |
|---|---|---|---|
| `threads` | 1–64 | pipeline | llama-server threads. The app uses at most the PC's logical CPUs. |

### overlay

| Key | Range | Applies | Meaning |
|---|---|---|---|
| `visible` | bool | live | Show the overlay |
| `style` | `panel`, `flow` | live | Caption panel, or running text: the lines joined into one English paragraph (and one Chinese paragraph with `show_source`), newest at the end. |
| `font_px` | 12–40 | live | English text size. Chinese is 0.65 × this size, at least 12 px. Older lines are 0.8 × this size. |
| `background` | 0–1 | live | Background opacity. The control window warns below 0.5. |
| `show_source` | bool | live | Show the Chinese line above the English line |
| `panel_edge` | `left`, `right` | live | Default edge for the panel |
| `panel_lines` | 1–10 | live | Blocks kept in the panel. The panel fills from the top and rolls up. |
| `flow_lines` | 1–10 | live | Wrapped lines kept of each running-text paragraph. Both paragraphs fill from the top and roll up. |
| `live_source` | bool | live | Show the Chinese of the clause that is spoken now |
| `panel_rect`, `flow_rect` | `{ monitor, x, y, w, h }` | live | Saved position in device-independent pixels. The app writes them after move mode. Absent: default position. |

### Screen

| Key | Range | Applies | Meaning |
|---|---|---|---|
| `direction` | `horizontal`, `vertical` | live | Remembered screenshot direction. The picker is available before dragging; changing it with a result reads the same selection again. English is set across the source columns. |

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
| `screen_translate` | Freeze the monitor under the cursor to translate a box of it; pressed again, close the frozen view. Not `Ctrl+Alt+A`, which QQ uses for its screenshot |

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
| `dir` | folder | restart | Model folder. Empty: `models` in the local data folder. |

## Environment variables

For development only. See [development.md](development.md#environment-variables).
