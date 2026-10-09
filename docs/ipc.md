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
| `speech_started` | `id`, `at_ms` | A speech segment opens |
| `asr_partial` | `id`, `text`, `class`, `end_ms` | Each decode of the open clause (not in Off mode) |
| `asr_final` | `id`, `text`, `class`, `lang`, `start_ms`, `end_ms`, `asr_ms`, `cut` | A clause is complete |
| `translation_draft` | `id`, `rev`, `text`, `end_ms` | A draft is ready (Continuous mode, Chinese or mixed text only). `rev` starts at 1. |
| `joined` | `id`, `absorbed[]`, `text`, `kind` | Clauses join into the leader `id` |
| `translation_delta` | `id`, `text_so_far` | The final translation streams |
| `translation_final` | `id`, `text`, `timing` | The final translation is complete |
| `translation_failed` | `id`, `reason`, `message` | The final translation failed |
| `skipped` | `id`, `reason` | The translator skipped the line to catch up |
| `dropped` | `id`, `reason` | The segment has no usable text, or listening stopped |
| `source_changed` | `info` (`mode`, `label`, `sample_rate`, `channels`) | The capture source changed |
| `source_state` | `state`, `detail` | The capture state changed |
| `listening_state` | `state` | Listening starts, runs or pauses |
| `engine_status` | `engine`, `state`, `message` | An engine changed state |
| `stats` | `PipelineStats` fields | One time each second while listening |

Enum values:

| Enum | Values |
|---|---|
| `TextClass` | `chinese`, `mixed`, `english`, `other` |
| `CutReason` | `pause`, `soft_cut`, `hard_cut`, `discontinuity`, `end`, `commit` |
| `JoinKind` | `hold`, `queue` |
| `SkipReason` | `catch_up` |
| `FailReason` | `timeout`, `server_unavailable`, `echo`, `runaway`, `error` |
| `DropReason` | `empty`, `music`, `single_char` |
| `EngineKind` | `vad`, `asr`, `translator`, `draft_translator` |
| `EngineState` | `loading`, `ready`, `restarting`, `failed` |
| `SourceStateKind` | `playing`, `silent`, `no_device`, `no_apps_selected`, `apps_not_running`, `unsupported` |
| `ListeningStateKind` | `starting`, `listening`, `paused` |
| `EffectiveMode` | `continuous`, `light`, `off` |
| `ModeReason` | `user`, `auto`, `cpu`, `lag`, `draft_unavailable` |

`timing` contains `speech_end_ms`, `asr_done_ms`, `queued_ms`, `sent_ms`, `first_token_ms` (nullable), `done_ms`, `prompt_tokens`, `cached_tokens` and `generated_tokens`.

Notes:

- The core translates `other` text only if its language is in `routing.translate_other`.
- `dropped` can come for an id that already has a live line.
- If an ASR decode fails, the core sends `engine_status` (`asr`, `failed`) and then `dropped` (`empty`).
- When listening stops, each open id gets a terminal event: `translation_failed` (`error`) if a translation is in progress, otherwise `dropped` (`empty`).
- `source_state` becomes `silent` after 3 s without real audio.
- `engine_status` for a translator stays `loading` until its warm-up request is complete.

### Stats

| Field | Meaning |
|---|---|
| `lag_ms` | Live position minus the end of the oldest unfinished clause |
| `queue_depth`, `held` | Waiting translation items; segments that the joiner holds |
| `done_p50_ms`, `done_p95_ms`, `first_p50_ms` | Speech end to final and to first delta, last 50 finals |
| `word_first_p50_ms`, `word_final_p50_ms` | Median word delay to first and to final English, last 60 s |
| `skipped_total`, `failed_total`, `drafts_total`, `drafts_failed` | Session counters |
| `cpu_app_pct`, `cpu_translator_pct`, `cpu_draft_pct` | Percent of one core, last second |
| `cpu_system_pct` | Whole-PC CPU, last second |
| `rss_app_mb`, `rss_translator_mb`, `rss_draft_mb` | Memory |
| `mode`, `mode_reason` | Effective caption speed and the reason |

## Order of events

These rules apply to each id. `crates/lt-core/tests/v2.rs` checks them with random delays and failures.

```
speech_started? -> (asr_partial | translation_draft)* -> dropped                     (end)
speech_started? -> (asr_partial | translation_draft)* -> asr_final -> ...
asr_final (english, or other not translated)                                         (end)
asr_final -> joined (as absorbed)                                                     (end)
asr_final -> joined (as leader)* -> translation_draft? -> translation_delta*
          -> translation_final | translation_failed | skipped                         (end)
```

- No event follows a terminal event.
- `translation_draft.rev` increases by 1 each time.
- `joined` comes after the `asr_final` of each id that it names. A leader can get more than one `joined`.
- Absorbed ids get no translation events.

## Tauri events

| Event | Payload | Sent when |
|---|---|---|
| `pipeline://event` | `PipelineEvent` | Each pipeline event, through the coalescer |
| `config://changed` | `Config` | After a save or a change by the app (overlay position, visibility) |
| `overlay://visible` | `{ visible }` | The overlay shows or hides |
| `overlay://mode` | `{ moving }` | Move mode starts or stops |
| `overlay://hover` | `{ hover }` | The cursor enters or leaves the panel (polled each 100 ms) |
| `models://progress` | `ModelStatus[]` | Download progress |
| `models://error` | `{ message }` | A download failed |
| `hotkeys://error` | `{ action, accelerator, message }` | A hotkey is not available |

The coalescer (`coalesce.rs`):

- It sends a maximum of one `translation_delta` for each id each 33 ms. It keeps only the newest text.
- It sends all other events immediately, in order. Before it sends an event, it sends the pending deltas.
- It discards a pending delta when its id gets `translation_final`, `translation_failed`, `skipped`, or is absorbed by `joined`.

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
| `get_stats` | | `PipelineStats` | Latest stats |
| `list_audio_devices` | | `{ id, name, is_default }[]` | Output devices |
| `list_audio_apps` | | `{ supported, reason, apps[] }` | Apps with audio, and recent selections |
| `models_status` | | `ModelStatus[]` | State of each model |
| `models_download` | `source`, `ids?` | | Download. Without `ids`: missing required models and recommended optional models. |
| `models_pause` | | | Pause downloads. Partial files stay. |
| `models_use_existing` | `folder` | `ModelStatus[]` | Find, verify and copy model files from a folder |
| `set_overlay_moving` | `moving` | | Start or stop move mode |
| `set_overlay_visible` | `visible` | | Show or hide the overlay |
| `open_folder` | `which` | | Open `transcripts`, `logs` or `models` |
| `show_control_window` | | | Show the control window |
| `quit` | | | Stop the pipeline and servers, then exit |

`AppState` contains `listening`, `source`, `source_state`, `engines` (`vad`, `asr`, `translator`, `draft_translator`), `models_ready`, `overlay_moving`, `overlay_visible`, `mode` and `mode_reason`.

`ModelStatus` contains `id`, `name`, `bytes_total`, `bytes_done`, `state` (`missing`, `downloading`, `paused`, `verifying`, `ready`, `corrupt`), `optional` and `recommended`.

## Caption lines

`app/ui/src/lib/captions.ts` is the only code that changes caption lines. A line has one of these states: `live`, `pending`, `streaming`, `final`, `english`, `other`, `skipped`, `failed`. The last five are terminal.

| Event | Effect on the line |
|---|---|
| `asr_partial` | New id: add a `live` line. `live` line: update the Chinese. |
| `translation_draft` | `live` or `pending` line: keep the last two drafts and update the shown words |
| `asr_final` | `english` or `other` (terminal), else `pending`. Show the full newest draft. |
| `joined` | Remove the absorbed lines. The leader gets the joined text. |
| `translation_delta` | `pending` if the line has a draft, else `streaming` |
| `translation_final` | `final` |
| `translation_failed` | `failed`, with a short reason text |
| `skipped` | `skipped` (the draft stays, dimmed) |
| `dropped` | Remove the line if it is `live` |

Draft display (`overlay.draft_display`):

- `all`: all words of the newest draft.
- `hold2`: all words except the last two.
- `settled`: the common word prefix of the last two drafts.
- After `asr_final`, all policies show the full draft.
- If a new draft would show less than half of the current words, the old words stay for one update.

Other rules:

- Terminal lines fade `overlay.expire_s` after the last speech. Unfinished lines do not expire.
- The store keeps a maximum of 6 terminal lines and 32 unfinished lines.
- The bar shows the last 2 lines. The panel shows `panel_lines` lines (4–6).

## Transcript records

The app writes one JSONL file for each session: `transcripts/YYYY-MM-DD_HHMMSS.jsonl`. `lt-cli replay` writes the same format.

- Line 1 is the session header: `type: "session"`, `app_version`, `started_at`, `source`, `asr`, `translator` and `config` (main latency settings and the draft model id).
- Each other line is one `utterance` record. The writer writes it when the line becomes terminal.
- The writer does not write dropped lines, partials or drafts.

```json
{"type":"utterance","id":12,"joined":[13],"start_ms":45210,"end_ms":47980,"cut":"commit","wall":"2026-10-07T21:14:51+08:00","class":"chinese","lang":"zh","source":"我也想办一个，伟大的公司。","english":"I also want to run a great company.","status":"final","timing":{...}}
```

| Field | Meaning |
|---|---|
| `status` | `final`, `english`, `other`, `skipped` or `failed` |
| `english` | The translation. `null` if the status is not `final`. |
| `joined` | Absorbed ids |
| `timing` | Only for `final` |
| `reason`, `message` | Only for `failed` |
