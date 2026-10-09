# Configuration

The app reads `config.toml` from the app config folder (`%APPDATA%\app.livetranslation.desktop\`). `LT_CONFIG` sets a different file.

Rules:

- A missing key or a missing file gets the default value.
- The app keeps unknown keys and logs a warning.
- A value with the wrong type gets the default value. The app logs a warning.
- A number out of range is clamped to the nearest limit.
- An enum value that is not permitted gets the default value.
- A file that is not valid TOML does not load.
- The app writes `config.toml.tmp` and renames it, so a save is atomic.

**Applies** tells when a change has an effect:

| Value | Effect |
|---|---|
| `live` | Immediately |
| `capture` | The capture sources open again. The pipeline continues. |
| `pipeline` | The pipeline restarts. A llama-server restarts only if one of these changes: `translate.server_url`, `translate.threads`, `translate.ctx`, `translate.engine`, `latency.low_priority`, `latency.draft_server_url`, `latency.draft_engine`, or `latency.mode` to or from `off`. |
| `restart` | At the next app start |

## Full Default File

```toml
config_version = 1

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

[vad]
engine = "silero"
threshold = 0.5
min_speech_s = 0.25
min_silence_s = 0.4
pre_roll_s = 0.3
post_roll_s = 0.1
soft_cut_after_s = 7.0
soft_cut_prob = 0.35
hard_cut_s = 10.0

[asr]
engine = "sensevoice"
language = "auto"               # keep "auto": "zh" gives worse English
use_itn = true
threads = 1

[filter]
drop_music = true
single_char_max_s = 0.6
single_char_allow = ["对", "好", "是", "行", "不", "哦"]
fillers = ["呃", "嗯", "额", "uh", "um"]

[routing]
english = "passthrough"         # "passthrough" is the only value
translate_other = []            # e.g. ["ja", "ko"]; empty = show other languages untranslated

[join]
hold_max_chars = 8              # 0 disables short-phrase joining
hold_window_s = 1.0
max_segments = 3
max_chars = 40

[translate]
engine = "hymt2"
server_url = ""                 # "" = spawn the bundled llama-server; otherwise use this OpenAI-compatible base URL
threads = 0                     # 0 = automatic: 2 per server
ctx = 1024
temperature = 0.0
repeat_penalty = 1.05
max_tokens_cap = 256
timeout_s = 10.0
queue_join_max_chars = 120
skip_lag_s = 6.0
target = "en"

[overlay]
visible = true
style = "bar"                   # "bar" (subtitle bar) | "panel" (caption panel)
font_px = 26
background = 0.82               # 0.0 to 1.0
show_source = true              # show the Chinese line above the English
expire_s = 8.0
panel_edge = "right"            # "left" | "right"
panel_lines = 5
live_source = true              # show the Chinese of the clause being spoken
draft_display = "hold2"         # "hold2" | "settled" | "all"
# Saved positions, written by the app after move mode. Absent = default placement.
# [overlay.bar_rect]   monitor = "\\\\.\\DISPLAY1", x = 0, y = 0, w = 960, h = 140   (device-independent pixels)
# [overlay.panel_rect] monitor = "\\\\.\\DISPLAY1", x = 0, y = 0, w = 420, h = 600

[latency]
mode = "auto"                   # "auto" | "continuous" | "light" | "off"
decode_interval_s = 0.5
min_open_s = 1.0
comma_min_tokens = 8
stability = true
split_long = true               # wait cap on/off
cap_tokens = 20
draft_context_s = 1.5
final_context_s = 10.0
draft_min_chars = 3
draft_grow_chars = 3
draft_keep_back_words = 2
draft_timeout_s = 3.0
draft_engine = "lmt60"
draft_server_url = ""           # "" = spawn the bundled draft server
low_priority = true             # below-normal priority for both servers
step_down = true
step_down_cpu_pct = 80.0
step_up_cpu_pct = 65.0
step_down_lag_s = 3.0
auto_min_cores = 6

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
| `normalize` | bool | pipeline | Slow automatic gain before the VAD |
| `norm_attack_s` | 0.1–5 | pipeline | Time to decrease gain for loud input |
| `norm_release_s` | 0.5–20 | pipeline | Time to increase gain for quiet input |
| `norm_max_gain_db` | 0–30 | pipeline | Maximum gain |
| `norm_gate_dbfs` | -90 to -30 | pipeline | No gain below this level. Silence stays silent. |

### vad

| Key | Range | Applies | Meaning |
|---|---|---|---|
| `engine` | engine name | pipeline | VAD engine |
| `threshold` | 0.2–0.9 | pipeline | Speech probability that counts as speech |
| `min_speech_s` | 0.05–1.0 | pipeline | Speech necessary to open a segment |
| `min_silence_s` | 0.15–1.5 | pipeline | Pause that closes a segment. This is the main latency setting. |
| `pre_roll_s` | 0–1.0 | pipeline | Audio kept before speech starts |
| `post_roll_s` | 0–0.5 | pipeline | Audio kept after the last speech frame |
| `soft_cut_after_s` | 3 to `hard_cut_s` | pipeline | After this time, cut at the next breath |
| `soft_cut_prob` | 0.05 to `threshold` | pipeline | A frame below this probability is a breath |
| `hard_cut_s` | 5–20 | pipeline | Cut at this time, also during speech |

### asr

| Key | Range | Applies | Meaning |
|---|---|---|---|
| `engine` | engine name | pipeline | ASR engine |
| `language` | `auto`, `zh`, `en`, `ja`, `ko`, `yue` | pipeline | Language hint. Keep `auto`: `zh` gives worse English. |
| `use_itn` | bool | pipeline | Punctuation and number normalization |
| `threads` | 1–4 | pipeline | ONNX Runtime threads for ASR |

### filter

| Key | Range | Applies | Meaning |
|---|---|---|---|
| `drop_music` | bool | pipeline | Drop music results |
| `single_char_max_s` | 0 or more | pipeline | Drop a one-character result from a segment shorter than this, unless the character is in `single_char_allow` |
| `single_char_allow` | list of strings | pipeline | One-character results that the filter always keeps |
| `fillers` | list of strings | pipeline | Words removed when they stand alone |

### routing

| Key | Range | Applies | Meaning |
|---|---|---|---|
| `english` | `passthrough` | pipeline | Show English speech as heard |
| `translate_other` | list of language codes | pipeline | Other source languages to translate. The UI offers `ja` and `ko`. Empty: show them untranslated. |

### join

| Key | Range | Applies | Meaning |
|---|---|---|---|
| `hold_max_chars` | 0–20 | pipeline | Hold Chinese or mixed segments with this number of Chinese characters or fewer. `0` disables the hold. |
| `hold_window_s` | 0.2–2.0 | pipeline | Time that a held segment waits for the next segment |
| `max_segments` | 2–5 | pipeline | Maximum segments in one join |
| `max_chars` | 10–80 | pipeline | Maximum Chinese characters in one join |

### translate

| Key | Range | Applies | Meaning |
|---|---|---|---|
| `engine` | engine name | pipeline | Final translator |
| `server_url` | URL | pipeline | Empty: start and supervise the bundled llama-server. Set: use this OpenAI-compatible server. |
| `threads` | 0–16 | pipeline | Threads for each llama-server. `0`: 2 for each server. Other values are clamped to 1–4. |
| `ctx` | 512–4096 | pipeline | Context size (`-c`) |
| `temperature` | 0–1 | pipeline | Sampling temperature |
| `repeat_penalty` | 1.0–1.3 | pipeline | Repeat penalty |
| `max_tokens_cap` | 32–1024 | pipeline | Maximum output tokens: `min(cap, 4 × characters + 16)` |
| `timeout_s` | 2–60 | pipeline | Time limit for one request |
| `queue_join_max_chars` | 20–300 | pipeline | Maximum text joined from waiting items |
| `skip_lag_s` | 2–30 | pipeline | Lag at which the translator skips older items |
| `target` | `en` | pipeline | Target language |

### overlay

| Key | Range | Applies | Meaning |
|---|---|---|---|
| `visible` | bool | live | Show the overlay |
| `style` | `bar`, `panel` | live | Subtitle bar or caption panel |
| `font_px` | 18–40 | live | English text size. Chinese is 0.65 × this size. Older lines are 0.8 × this size. |
| `background` | 0–1 | live | Background opacity. The control window warns below 0.5. |
| `show_source` | bool | live | Show the Chinese line above the English line |
| `expire_s` | 3–60 | live | Remove lines after this time without new speech |
| `panel_edge` | `left`, `right` | live | Default edge for the panel |
| `panel_lines` | 4–6 | live | Lines in the panel |
| `live_source` | bool | live | Show the Chinese of the clause that is spoken now |
| `draft_display` | `hold2`, `settled`, `all` | live | Draft words to show. `hold2`: all words except the newest two. `settled`: words that the last two drafts agree on. `all`: all words. |
| `bar_rect`, `panel_rect` | `{ monitor, x, y, w, h }` | live | Saved position in device-independent pixels. The app writes them after move mode. Absent: default position. |

### latency

All keys have the `pipeline` value: any change in `[latency]` restarts the pipeline.

| Key | Range | Meaning |
|---|---|---|
| `mode` | `auto`, `continuous`, `light`, `off` | Caption speed. See [architecture.md](architecture.md#caption-speeds). |
| `decode_interval_s` | 0.3–2.0 | New audio between two decodes of the open clause |
| `min_open_s` | 0.5–3.0 | Audio necessary before the first decode |
| `comma_min_tokens` | 2–20 | Word tokens necessary before a comma can end a clause |
| `stability` | bool | Commit only text that the previous decode also had |
| `split_long` | bool | Enable the wait cap |
| `cap_tokens` | 0, 10–60 | Wait cap in word tokens. `0` disables it. Values 1–9 become 10. |
| `draft_context_s` | 0–5 | Left context for decodes after a commit |
| `final_context_s` | 0–20 | Left context for the final decode after a commit |
| `draft_min_chars` | 1–10 | Word characters necessary before the first draft |
| `draft_grow_chars` | 1–20 | New word characters necessary before the next draft |
| `draft_keep_back_words` | 0–5 | Words removed from the previous draft to make the prefill |
| `draft_timeout_s` | 1–10 | Time limit for one draft request |
| `draft_engine` | engine name | Draft translator |
| `draft_server_url` | URL | Empty: start and supervise the draft server. Set: use this server. |
| `low_priority` | bool | Run both servers at below-normal priority |
| `step_down` | bool | Enable automatic step-down |
| `step_down_cpu_pct` | 50–100 | System CPU that starts a step-down (3 samples) |
| `step_up_cpu_pct` | 30 to `step_down_cpu_pct` | System CPU below which step-up is possible (10 samples) |
| `step_down_lag_s` | 1–10 | Translation lag that starts a step-down (2 samples) |
| `auto_min_cores` | 2–64 | Physical cores necessary for `auto` to select Continuous |

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
