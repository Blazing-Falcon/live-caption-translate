# Guide for coding agents

Chinese-to-English live captions for Windows. Rust core in `crates/`, Tauri shell in `app/src-tauri`, Svelte UI in `app/ui`. Read [docs/architecture.md](docs/architecture.md) first. [docs/development.md](docs/development.md) has all commands.

## Commands

```powershell
./scripts/setup.ps1          # one time for each clone
./scripts/check.ps1          # all checks
. ./scripts/env.ps1          # in each new shell, before cargo
cargo test -p lt-core        # fast core tests
npm --prefix app/ui test     # fast UI tests
```

## Rules

- `scripts/check.ps1` must pass before a change is complete. Do not disable a lint or a test to make it pass.
- Fixtures are the reference (`reference/fixtures`, `testdata`, `crates/*/tests/fixtures`, `app/ui/src/lib/__fixtures__`). Do not edit a fixture to make a test pass. Change the code. If a fixture is wrong, stop and tell why.
- Do not invent numbers. A latency, CPU, memory or accuracy value must come from a run. Give the machine, the command and the date. If you did not measure it, write "not measured".
- Add a test only if a failure means a real defect that nothing else finds: a contract between parts (wire format, config defaults, event order, prompts), a recorded reference (fixtures), a failure that is otherwise invisible (data loss, leaked processes, deadlocks, unbounded memory), or logic that broke before. Do not test constants, small helpers or UI layout.
- Real-model tests skip without their variables. A skipped test is not a pass. Tell which tests you ran.
- If you change a config key, update `docs/configuration.md`. A test compares its default block with `DEFAULT_CONFIG`. If you change an event or a command, update `docs/ipc.md`.
- Keep OS code in `lt-audio` and the app. Keep engine code in `lt-sherpa` and `lt-llm`. `lt-core` has neither.
- Each engine has one owner thread. The core has no async runtime. Use bounded channels.
- No network use except model downloads and the local llama-servers. No telemetry.
- Do not commit models, `testdata/fetched/`, `.deps/` or build output.
- Comments tell why. They do not refer to tasks or documents.

## Where to find code

| Part | Location |
|---|---|
| Config | `crates/lt-core/src/config.rs` |
| Segment builder | `crates/lt-core/src/segment.rs` |
| Recognizer, commit rule | `crates/lt-core/src/recognizer.rs`, `commit.rs` |
| Filter, joiner, queue | `crates/lt-core/src/text.rs`, `join.rs`, `queue.rs` |
| Drafts, scheduler, step-down | `crates/lt-core/src/draft.rs`, `drafts.rs`, `pipeline.rs`, `stepdown.rs` |
| SenseVoice, Silero | `crates/lt-sherpa/src/` |
| llama-server, model downloads | `crates/lt-llm/src/` |
| Windows capture | `crates/lt-audio/src/windows/` |
| App commands and bridge | `app/src-tauri/src/` |
| Caption store, overlay | `app/ui/src/lib/captions.ts`, `app/ui/src/overlay/` |
| Control window | `app/ui/src/control/` |
