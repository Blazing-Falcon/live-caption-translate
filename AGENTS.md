# Guide for coding agents

Chinese-to-English live captions and screenshot translation for Windows. Rust core in `crates/`, Tauri shell in `app/src-tauri`, Svelte UI in `app/ui`. Read [docs/architecture.md](docs/architecture.md) first. [docs/development.md](docs/development.md) has all commands.

## Commands

```powershell
./scripts/setup.ps1          # one time for each clone
./scripts/check.ps1          # all checks
. ./scripts/env.ps1          # in each new shell, before cargo
cargo test -p lt-core        # fast core tests
npm --prefix app/ui test     # fast UI tests
```

## Rules

These are defaults. When one makes a change worse, break it. Say why in the commit body and in
`.agent/decisions.md`, and propose a better rule here.

- A change is done when `scripts/check.ps1` passes. A wrong check is fixed or deleted in the same
  change, with the reason; never silence a check to hide a defect.
- Fixtures record real behavior. Regenerate one when the contract it records changes on purpose,
  and say how. Never edit one to hide a bug.
- A latency, CPU, memory or accuracy number comes from a run: machine, command, date. Otherwise
  write "not measured".
- Tests guard contracts between parts, recorded references, failures nothing else would show (data
  loss, leaks, deadlocks, layout jumps), and bugs that came back. Prefer end-to-end tests. A skipped
  real-model test is not a pass; say which tests ran.
- A change to a config key, event or command updates `docs/configuration.md` or `docs/ipc.md` in
  the same commit.
- OS code lives in `lt-audio` and the app, engine code in `lt-sherpa`, `lt-llm` and `lt-ocr`.
  `lt-core` has neither.
- One owner thread per in-process engine. llama-server takes requests from the caption translator
  and the screen thread, one at a time each. No async runtime in the core. Bounded channels.
- No network use except model downloads and the local llama-server. No telemetry.
- Never commit models, `testdata/fetched/`, `.deps/` or build output.
- Write no comment unless the reason is not obvious from the code. One or two lines. No history,
  measurements, task names or document references in code; those go in `.agent/` or `docs/`.

## Where to find code

| Part | Location |
|---|---|
| Config (version 3, one limits table) | `crates/lt-core/src/config.rs` |
| Recognizer, commit rule | `crates/lt-core/src/recognizer.rs`, `commit.rs` |
| Filter | `crates/lt-core/src/text.rs` |
| Pipeline start, threads, shutdown | `crates/lt-core/src/pipeline/mod.rs`, `handle.rs`, `cancel.rs` |
| Scheduler steps | `crates/lt-core/src/pipeline/scheduler.rs` |
| Join, skip and break policy | `crates/lt-core/src/pipeline/queue.rs` |
| Translator thread | `crates/lt-core/src/pipeline/translator.rs` |
| Unit seal and previews | `crates/lt-core/src/pipeline/running.rs` |
| Second pass (trim, refine thread) | `crates/lt-core/src/refine.rs`, `pipeline/refine.rs` |
| X-ASR streaming and offline | `crates/lt-sherpa/src/` |
| Screen text recognition (PP-OCRv6 detector and recognizer) | `crates/lt-ocr/src/` |
| Screen blocks, colors, pieces, job runner, events | `crates/lt-core/src/screen/` |
| Translator client, loopback HTTP | `crates/lt-llm/src/client.rs`, `http.rs` |
| llama-server, model downloads | `crates/lt-llm/src/supervisor.rs`, `models/` |
| Windows capture | `crates/lt-audio/src/windows/` |
| Screenshot capture, `ltshot` frame, window, `lt-screen` thread | `app/src-tauri/src/screen/` |
| App commands, controller, bridge | `app/src-tauri/src/` |
| Caption blocks, overlay viewport | `app/ui/src/lib/captions.ts`, `app/ui/src/overlay/` |
| Control window | `app/ui/src/control/` |
| Screenshot window, store, coordinate math | `app/ui/src/screen/`, `app/ui/src/lib/screen.ts` |
