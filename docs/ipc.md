# Core ↔ UI contract

This file gives the data that goes between `lt-core`, the Tauri shell (`app/src-tauri`) and the UI (`app/ui`). The Rust types are the source of truth. Keep `app/ui/src/lib/types.ts` aligned with them.

## Conventions

- Enums with data use `#[serde(tag = "type", rename_all = "snake_case")]`. Unit enums are snake_case strings.
- Events and stats send optional values as `null`. They do not omit them.
- `UtteranceId` is a JSON number. The maximum is 2^53 − 1.
- All `*_ms` fields are milliseconds on the session clock. The clock starts at 0 when listening starts.
- `crates/lt-core/tests/fixtures/events.json` and `app/ui/src/lib/__fixtures__/events.json` contain one example of each event. Tests on both sides read them.

## Pipeline events

| `type` | Fields | Sent when |
|---|---|---|
| `speech_started` | `id`, `at_ms` | A clause opens at its first word |
| `asr_partial` | `id`, `text`, `class`, `end_ms` | The open clause's text grew |
| `asr_final` | `id`, `text`, `class`, `start_ms`, `end_ms`, `asr_ms`, `cut`, `logprob`?, `second_pass` | A clause is complete. `text` is the final text, after the second pass. `logprob` (mean token log-probability) is present only when the live engine gives one (X-ASR). `second_pass` is always present (see below). |
| `unit_sealed` | `id`, `ids[]`, `text` | One translation unit is complete: every line it covers, `id` first. Sent for every unit sent for final translation, including one of a single line. |
| `translation_delta` | `id`, `text_so_far` | The final translation streams |
| `translation_final` | `id`, `text`, `timing` | The final translation is complete |
| `translation_preview` | `id`, `ids[]`, `text` | Every session: the English of the lines not yet translated for good, `id` being the first of `ids`; the last of them may have no final text yet. A newer preview replaces it; so does the final translation of any of its lines. |
| `translation_failed` | `id`, `reason`, `message` | The final translation failed |
| `skipped` | `id`, `reason` | The translator skipped the line to catch up |
| `dropped` | `id`, `reason` | The clause has no usable text, or listening stopped |
| `source_changed` | `info` (`mode`, `label`, `sample_rate`, `channels`) | The capture source changed |
| `source_state` | `state`, `detail` | The capture state changed |
| `listening_state` | `state` | Listening starts, runs or pauses. `paused` also follows when the audio ends by itself (end of a file, capture that failed for good). |
| `engine_status` | `engine`, `state`, `message` | An engine changed state |
| `stats` | `PipelineStats` fields | One time each second while listening |

`second_pass` fields:

| Field | Meaning |
|---|---|
| `outcome` | `SecondPassOutcome` |
| `live_text`? | The live text, when `text` differs from it |
| `window_text`? | The second pass's text for the whole window, when it decoded |
| `window_start_ms`, `window_end_ms` | The decoded audio: the clause with a margin on both sides |
| `distance`? | Edit distance from the chosen text to the live text, over letters and digits (`used` only) |
| `decode_ms`? | Decode time, when it decoded |
| `refine_ms` | From the clause's close to the final text, waiting included |

Enum values:

| Enum | Values |
|---|---|
| `TextClass` | `chinese`, `mixed`, `english` |
| `CutReason` | `pause`, `discontinuity`, `end`, `commit` |
| `SkipReason` | `catch_up` |
| `FailReason` | `timeout`, `server_unavailable`, `echo`, `runaway`, `error` |
| `DropReason` | `empty`, `single_char` |
| `SecondPassOutcome` | `used` (the final text comes from the second pass), `mismatch` (no part of its text matched the live text closely enough), `late` (the line waited longer than 1.5 s for the second pass), `failed` |
| `EngineKind` | `asr`, `translator` |
| `EngineState` | `loading`, `ready`, `restarting`, `failed` |
| `SourceStateKind` | `playing`, `silent`, `no_device`, `no_apps_selected`, `apps_not_running`, `unsupported` |
| `ListeningStateKind` | `starting`, `listening`, `paused` |

`timing` contains `speech_end_ms`, `asr_done_ms`, `queued_ms`, `sent_ms`, `first_token_ms` (nullable), `done_ms`, `prompt_tokens`, `cached_tokens` and `generated_tokens`.

Notes:

- The core translates only `chinese` and `mixed` text. `english` lines show as heard.
- `dropped` can come for an id that already has a live line.
- If an ASR decode fails, the core sends `engine_status` (`asr`, `failed`) and then `dropped` (`empty`).
- When listening stops, each open id gets a terminal event: `translation_failed` (`error`) if a translation is in progress, otherwise `dropped` (`empty`).
- `source_state` becomes `silent` after 3 s without real audio.
- `engine_status` for a translator stays `loading` until its warm-up request is complete.

### Stats

| Field | Meaning |
|---|---|
| `lag_ms` | Live position minus the end of the oldest unfinished clause |
| `queue_depth` | Waiting translation items |
| `done_p50_ms`, `done_p95_ms`, `first_p50_ms` | Speech end to final and to first delta, last 50 finals |
| `word_final_p50_ms` | Median delay from a spoken word to its English, last 60 s |
| `skipped_total`, `failed_total` | Session counters |
| `cpu_app_pct`, `cpu_translator_pct` | Percent of one core, last second: the app and the translation server (the offline X-ASR second pass counts as the app) |
| `cpu_system_pct` | Whole-PC CPU, last second |
| `rss_app_mb`, `rss_translator_mb` | Memory (working set) of the same two processes |
| `private_app_mb`, `private_translator_mb` | Private bytes (commit charge) of the same two processes |

## Order of events

These rules apply to each id. `crates/lt-core/tests/pipeline.rs` checks them with random translation timing and failures.

```
speech_started? -> asr_partial* -> dropped                                           (end)
speech_started? -> asr_partial* -> asr_final -> unit_sealed -> ...
asr_final (english)                                                                  (end)
asr_final -> unit_sealed (as a later line of the unit)                               (end)
asr_final -> unit_sealed (as leader)*
          -> translation_delta* -> translation_final | translation_failed | skipped   (end)
```

- No event follows a terminal event.
- Every translated line is named by a `unit_sealed` after its own `asr_final` and before any
  `translation_delta` or `translation_final` of its unit's `id`. A heard line seals the running
  unit before its own `asr_final`.
- A `unit_sealed` names every line of its unit, `id` first. A line named as anything but `id` gets no
  translation events. The queue can seal the same leader again with more lines; `ids` is then the
  union of the unit's lines.
- `translation_preview` names only ids that have started and have no terminal event. It can come before an id's `asr_final`.
- In every session, the scheduler joins closed lines into one translation up to a sentence end (。？！), a pause or break cut, a line shown as heard, 10 s of silence, or 40 characters, and sends `unit_sealed` for each unit it sends. While no line waits for translation, it translates the lines not yet joined plus the live text of the lines without a final again (at least 4 letters; after the last preview ended it waits as long as that preview took, at least 300 ms). A preview answer with Chinese in it is not sent.

## Tauri events

| Event | Payload | Sent when |
|---|---|---|
| `pipeline://event` | `PipelineEvent` | Each pipeline event, through the coalescer |
| `config://changed` | `Config` | After a save or a change by the app (overlay position, visibility) |
| `overlay://visible` | `{ visible }` | The overlay shows or hides |
| `overlay://mode` | `{ moving }` | Move mode starts or stops |
| `models://progress` | `ModelStatus[]` | Download progress |
| `models://error` | `{ message }` | A download failed |
| `hotkeys://error` | `{ action, accelerator, message }` | A hotkey is not available |
| `screen://frame` | `{ id, width, height, scale }` | To the `screen` window, before it shows a new frozen frame. `width` and `height` are frame pixels; `scale` is the monitor's DPI / 96 |
| `screen://event` | `ScreenEvent` | To the `screen` window, for each step of a screen job (see [Screen jobs](#screen-jobs)) |

`models://progress` and `models://error` go to the `control` and `screen` windows.

The coalescer (`coalesce.rs`):

- It sends a maximum of one `translation_delta` for each id each 33 ms. It keeps only the newest text.
- It sends all other events immediately, in order. Before it sends an event, it sends the pending deltas.
- It discards a pending delta when its id gets `translation_final`, `translation_failed`, `skipped`, or is named as a later line of a `unit_sealed`.

If a bus subscriber is full, the bus removes the oldest `stats` event. If there is no `stats` event, the bus disconnects the subscriber, and the subscriber connects again.

## Tauri commands

All commands return `Result<T, String>`. The error string is a sentence for the user.

| Command | Arguments | Returns | Effect |
|---|---|---|---|
| `get_state` | | `AppState` | Current state |
| `startup_notices` | | `string[]` | Config problems and hardware warnings found at startup |
| `start_listening` | | | Start capture and the pipeline |
| `pause_listening` | | | Close capture. Pending translations finish. |
| `get_config` | | `Config` | Full config |
| `set_config` | `patch` | `{ config, applied, messages }` | Merge, validate, save and apply. `applied` is `live`, `capture`, `pipeline` or `restart`. |
| `reset_config` | | `{ config, applied, messages }` | Save and apply the defaults. The overlay positions, `capture.apps` and `models.dir` stay. |
| `get_stats` | | `PipelineStats` | Latest stats |
| `list_audio_devices` | | `{ id, name, is_default }[]` | Output devices |
| `list_audio_apps` | | `{ supported, reason, apps[] }` | Apps with audio, and recent selections |
| `models_status` | | `ModelStatus[]` | State of each model |
| `models_download` | | | Download every required model that is missing. Optional models are skipped. Does nothing while this download runs; fails while a `models_download_optional` download runs |
| `models_pause` | | | Pause downloads. Partial files stay. |
| `models_use_existing` | `folder` | `ModelStatus[]` | Find, verify and copy model files from a folder |
| `models_download_optional` | `id` | | Download one model, also an optional one (the screen text recognizer). Fails while any download runs |
| `set_overlay_moving` | `moving` | | Start or stop move mode |
| `set_overlay_visible` | `visible` | | Show or hide the overlay |
| `open_folder` | `which` | | Open `transcripts`, `logs` or `models` |
| `screen_select` | `frame`, `rect` | `number` | Cancel any screen job, cut `rect` (frame pixels) from frame `frame` and start a job on it; returns the job id |
| `screen_close` | | | Cancel the screen job, hide the `screen` window and drop the frame |
| `quit` | | | Stop the pipeline and the translation server, then exit |

`AppState` contains `listening`, `source`, `source_state`, `engines` (`asr`, `translator`), `models_ready`, `overlay_moving`, `overlay_visible` and `logical_cpus` (the most threads a setting can use).

`ModelStatus` contains `id`, `name`, `bytes_total`, `bytes_done`, `state` (`missing`, `downloading`, `paused`, `verifying`, `ready`, `corrupt`) and `optional`.

## Screen jobs

The `screen_translate` hotkey captures the monitor under the cursor, sends `screen://frame`, and shows the `screen` window over that monitor. The page loads the frame from `http://ltshot.localhost/<id>` (a 32-bit BMP; any other id is 404). The UI converts CSS pixels to frame pixels by multiplying by `scale`, and frame pixels back by dividing; every rect the app sends is in frame pixels.

Each `screen_select` starts a job with a new, larger id. A new job or `screen_close` cancels the running one, which then sends nothing more. A job's first events can arrive before `screen_select` returns its id.

`ScreenEvent` (`type`):

| `type` | Fields | Sent when |
|---|---|---|
| `reading` | `job` | The job starts |
| `blocks` | `job`, `blocks[]`, `ocr_ms` | The text is read and grouped |
| `translated` | `job`, `id`, `text` | One block's English |
| `failed` | `job`, `id`, `message` | One block could not be translated |
| `done` | `job`, `total_ms` | Every block is answered |
| `error` | `job`, `message` | The job could not run: the text recognition model is missing or could not load, or reading failed |

Order for each job:

```
reading -> blocks -> (translated | failed)* -> done
reading -> error
```

There is one `translated` or `failed` for each block with `translate: true`, in id order. When `error` follows `reading` because the model is missing, `models_status` shows `pp-ocrv6-det-tiny-rec-small` as not `ready`; `models_download_optional` downloads it.

`Block`:

| Field | Meaning |
|---|---|
| `id` | Reading order, from 0 |
| `rect` | `{ x, y, w, h }`, the union of `lines`, frame pixels |
| `lines` | The rect of each text line |
| `intent` | `single` for one source line, `group_reflow` for a group assembled from several source lines |
| `line_height` | Median line height, frame pixels |
| `source` | Same-row fragments joined at their text boundary; consecutive lines of a group joined with one space |
| `background`, `foreground` | `[r, g, b]` of the surface behind the text and of text that reads on it |
| `translate` | `false` when `source` has no Han character; such a block gets no translation events |

`app/ui/src/lib/__fixtures__/screen-events.json` has one example of each event. A Rust test and `app/ui/tests/screen.test.ts` read it.

## Caption blocks

`app/ui/src/lib/captions.ts` is the only code that changes caption lines. A line has one of these states: `live`, `pending`, `streaming`, `final`, `english`, `skipped`, `failed`. The last four are terminal.

| Event | Effect on the line |
|---|---|
| `asr_partial` | New id: add a `live` line. `live` line: update the Chinese. |
| `asr_final` | `english` (terminal), else `pending` |
| `unit_sealed` | Remove the other lines of the unit. The leader gets the unit's text. |
| `translation_delta` | `streaming` |
| `translation_final` | `final` |
| `translation_failed` | `failed`, with a short reason text |
| `skipped` | `skipped` |
| `dropped` | Remove the line if it is `live` |
| `translation_preview` | No state change. The store keeps the newest preview; the block it covers shows it in place of its English, dots or streaming text. `translation_final`, `translation_failed`, `skipped` or `dropped` of a covered line clears it. |

The store emits **blocks** (`CaptionBlock`), not lines. A block is one sealed unit (`key` is its leader, `ids` every line it covers, `sealed` true), the run of lines after the last seal (`sealed` false, `state` `open` while they are still live), or one heard English line. The store keeps the seals, so a second `unit_sealed` for the same leader adds ids and keeps the key. A block's `state` is `open`, `pending`, `streaming`, `final`, `failed`, `skipped` or `english`, and it carries `source` (Chinese), `live` (Chinese still being heard), `english`, `reason` and `preview`.

Other rules:

- Terminal blocks stay until newer text needs their room; unfinished blocks never leave on their own.
- The store keeps a maximum of 30 terminal lines and 32 unfinished lines.
- The panel shows `panel_lines` blocks (1–10); running text keeps `flow_lines` wrapped lines of each paragraph.

## Transcript records

The app writes one JSONL file for each session: `transcripts/YYYY-MM-DD_HHMMSS.jsonl`. `lt-cli replay` writes the same format.

- Line 1 is the session header: `type: "session"`, `app_version`, `started_at`, `source`, `asr`, `asr_second`, `translator` and `config` (`skip_lag_s`).
- Each other line is one `utterance` record. The writer writes it when the line becomes terminal.
- The writer does not write dropped lines or partials.

```json
{"type":"utterance","id":12,"joined":[13],"start_ms":45210,"end_ms":47980,"cut":"commit","wall":"2026-10-07T21:14:51+08:00","class":"chinese","source":"我也想办一个，伟大的公司。","english":"I also want to run a great company.","status":"final","timing":{...},"asr_live":{"engine":"xasr_stream","text":"我也想办一个。伟大的公司。","logprob":-0.12},"asr_second":[{"text":"我也想办一个。","outcome":"used","window_start_ms":44910,"window_end_ms":48700,"refine_ms":104}]}
```

| Field | Meaning |
|---|---|
| `status` | `final`, `english`, `skipped` or `failed` |
| `english` | The translation. `null` if the status is not `final`. |
| `joined` | Ids of the other lines of the unit |
| `timing` | Only for `final` |
| `reason`, `message` | Only for `failed` |
| `asr_live` | The live recognizer's `engine`, `text` and mean token `logprob` (null when the engine gives none) |
| `asr_second` | One record for each line of the unit: the text the second pass chose, its `outcome`, the decoded window and `refine_ms` |

A line with `joined` ids concatenates the texts of its parts. Its `logprob` is the mean of the parts, `start_ms` is the earliest and `done_ms` the latest.
