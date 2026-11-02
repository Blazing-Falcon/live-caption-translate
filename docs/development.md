# Development

Run all commands in PowerShell from the repository root.

## Set up a PC

Install these tools:

| Tool | Version |
|---|---|
| Rust | Pinned in `rust-toolchain.toml`. Install [rustup](https://rustup.rs). It installs the pinned version. |
| Node.js | 22.12 or newer (`.nvmrc`) |
| Python | 3.11 or newer (`python`, `py -3`, uv, or `LT_PYTHON`) |
| Visual Studio Build Tools | "Desktop development with C++" |
| WebView2 Runtime | Only to run the app. Windows 11 has it. |

Then:

```powershell
git clone <repo-url> live-translation
cd live-translation
./scripts/setup.ps1        # -Models: download the models. -TestAudio: download the test clips.
./scripts/check.ps1
```

`setup.ps1` checks the tools and tells you what is missing. Then it downloads and checks the native libraries, `llama-server`, the npm packages and the Playwright browser. Downloads go to `.deps/`. `LT_DEPS_DIR` sets a different folder.

In each new shell, run this before you use `cargo`:

```powershell
. ./scripts/env.ps1        # sets SHERPA_ONNX_LIB_DIR and adds target/debug to PATH
```

## Build and run

```powershell
cargo build                                     # portable crates
npm --prefix app/ui run build                   # build the UI first: the app embeds it
cargo run -p live-translation                   # the app
cargo run -p lt-cli --all-features -- --help    # the CLI
./scripts/install-native-runtime.ps1            # copy the native DLLs into target/
```

`npm --prefix app/ui run dev` serves the UI only, on `http://127.0.0.1:1420`.

### Environment variables

| Variable | Effect |
|---|---|
| `LT_CONFIG` | Config file |
| `LT_MODELS_DIR` | Model folder (CLI default: `models/`) |
| `LT_LLAMA_SERVER` | `llama-server.exe` to use, for example `.deps/llama-b11429/llama-server.exe` |
| `LT_ONNXRUNTIME` | ONNX Runtime library to use |
| `LT_REPLAY` | The app plays this WAV file instead of the capture |
| `LT_LOG` | Log filter, for example `debug`. Logs are in `%LOCALAPPDATA%\app.livetranslation.desktop\logs`. |

## Test

`./scripts/check.ps1` runs all checks: UI type check, token lint, UI tests, UI build, e2e tests, script tests, `cargo fmt --check`, clippy and Rust tests. `-Jobs N` limits Cargo jobs.

Single steps:

```powershell
npm --prefix app/ui run check
npm --prefix app/ui run lint:tokens
npm --prefix app/ui test
npm --prefix app/ui run e2e
cargo test --workspace --all-features
```

**Fixtures are the reference.** Do not change a fixture to make a test pass. Change the code.

### Real-model tests

These tests skip if their variables are not set. `check.ps1` shows the skipped tests. A skipped test is not a pass: when you report a result, say which real-model tests ran. If the variable is set but the model files are missing, the test fails.

| Test | Needs | Checks |
|---|---|---|
| `lt-sherpa` `stream` | `LT_MODELS_DIR` with the 480 ms streaming model, test audio | Streaming X-ASR with greedy search: emitted tokens never change, accuracy |
| `lt-sherpa` `offline` | `LT_MODELS_DIR` with the offline X-ASR model, test audio | Offline X-ASR on whole clips: accuracy, spaces only next to English words |
| `lt-llm` `real_server` | `LT_LLAMA_SERVER`, Hy-MT2 (`LT_MT_MODEL` or `LT_MODELS_DIR`) | 37 sentences against the fixture |
| `lt-ocr` `screen` | `LT_MODELS_DIR` with the text recognition model | Every block of the 12 rendered screens in `testdata/screen` has a recognized line on it, and the character error rate is at most 2.4% |
| `lt-cli` `screen` | `LT_MODELS_DIR` with the text recognition and translation models, `LT_LLAMA_SERVER` | `lt-cli screen` on `article@1.png`: two blocks, both translated with no Chinese left, then `done` |

`LT_REFERENCE_AUDIO` sets the test audio folder (default `testdata/fetched`).

Cargo runs each test in its crate's folder, so `LT_MODELS_DIR` must be an absolute path. A relative `models` finds nothing from `crates/lt-ocr`.

The text recognition model is optional. `setup.ps1 -Models` downloads it with the required models (`lt-cli models fetch --optional`); `lt-cli models fetch` without `--optional` skips it.

```powershell
. ./scripts/env.ps1
$env:LT_MODELS_DIR = "$PWD/models"
$env:LT_LLAMA_SERVER = "$PWD/.deps/llama-b11429/llama-server.exe"
cargo test --workspace --all-features
```

## Replay

`setup.ps1 -TestAudio` downloads the public clips to `testdata/fetched/`. Their licenses permit only local tests. Do not commit them.

```powershell
cargo build --release -p lt-cli --all-features
target\release\lt-cli.exe replay testdata\fetched\ramc\CTS-CN-F2F-2019-11-15-1449.wav --start 19 --dur 180 `
    --pace realtime --server .deps\llama-b11429\llama-server.exe --out out\conv.jsonl
python scripts\replay-checks.py out\conv.events.jsonl      # event order, repeats, expected text
```

`replay` writes the transcript, `<out>.events.jsonl` and `<out>.words.json`, and prints the per-word delays. The delays need speech instants: `--instants <file>` reads a JSON array of milliseconds from `--start` at which a word is spoken. Without it the delay values are empty. Every closed line goes through the offline X-ASR second pass, so a real replay needs that model as well as the streaming one; `--fake` uses fakes for all three engines and reports no second-pass summary. When you report a number, also give the machine, the command and what else was running.

## Screenshots

`lt-cli screen` runs the app's screenshot path on a PNG: it reads the text, groups it into blocks and translates each block. It needs the `ocr` feature (which turns on `sherpa` and `llm`) and the text recognition model. Unless you pass `--no-translate`, the local engine also needs the translation model and `llama-server`. `--engine microsoft` or `--engine google` uses the selected online translator instead, without either local translation asset. Both `replay` and `screen` accept `--engine`, overriding `translate.engine` from `--config` or `LT_CONFIG`.

Pass `--explain` to include `decisions` and `grouping_trace` in the output JSON.
Each verdict names its source lines, rule, join/refusal and geometry. Row verdicts
use `vertical_gap` for horizontal spacing and `left_delta` for vertical overlap;
next-line verdicts use those fields for their vertical gap and left alignment.
Tilted cards are straightened only for grouping and restored afterward; blocks carry optional card geometry. `--explain` uses the same grouping path as the job.

Trace IDs distinguish detected boxes (`b0`, ...) from merged lines (`L0`, ...).

```powershell
cargo build --release -p lt-cli --features ocr
./scripts/install-native-runtime.ps1
target\release\lt-cli.exe screen testdata\screen\article@1.png --server .deps\llama-b11429\llama-server.exe --out out\screen.json
```

It prints a table of block, Chinese, English and milliseconds. The JSON file has the `lines` the recognizer found, every `ScreenEvent` in `events` with `t_ms` since the job started, `ocr_load_ms` and `server_ms`. `--models` (or `LT_MODELS_DIR`) and `--server` (or `LT_LLAMA_SERVER`) select the model folder and the server. `--no-translate` stops after `blocks`. `--threads` sets the recognizer threads (default: half the logical CPUs, from 1 to 4). `--out` defaults to `out/screen.json`.

Recognized lines include a `quad` of four source-pixel points and `char_scores` for the kept recognition steps. The job derives script, glyph heights and ink height from these lines, folds Latin accents, removes isolated icon ideographs and rejects misshapen detections before grouping.

D in the screenshot window toggles blue OCR line boxes and orange group boxes. Enable it before dragging to request raw rectangles in the first job; enabling it later reruns the same crop, using the last OCR result when the frame/crop/direction match. Empty reads are not cached. Raw rectangles include detections that the filters reject.

### Regenerate the screen fixtures

Do these only when the thing they record changes on purpose, in this order, and say so in the commit body. Then run `cargo test -p lt-core --test screen_group` and `npm --prefix app/ui run e2e -- screen`.

| Fixture | Records | Command |
|---|---|---|
| `testdata/screen/<name>@<scale>.png` and `.truth.json` | The pages in `testdata/screen/source/` at scale 1 and 1.5, with the text, rect and lines of each `data-block` element | `node app/ui/scripts/render-screen-fixtures.mjs` |
| `crates/lt-core/tests/fixtures/screen/<name>@<scale>.lines.json` | The lines `lt-ocr` found in each PNG. The grouping test reads them and needs no model. | `lt-cli screen testdata/screen/<name>@<scale>.png --no-translate --out crates/lt-core/tests/fixtures/screen/<name>@<scale>.lines.json` for each PNG |
| `app/ui/e2e/fixtures/article@1.5.screen.json` | The events `lt-cli screen` printed for `article@1.5.png`, with the real translations | `lt-cli screen testdata/screen/article@1.5.png --out app/ui/e2e/fixtures/article@1.5.screen.json` |
| `app/ui/src/lib/__fixtures__/screen-events.json` | One example of each `ScreenEvent`. Edit it by hand when the contract in [ipc.md](ipc.md#screen-jobs) changes. A Rust test and the UI tests read it. | |

The renderer uses the fonts of the PC it runs on, so another PC renders differently. The PNGs are the record. A new render changes the truth files and the recorded lines, so record the lines again after it. The `lt-cli` commands need the release build above and `LT_MODELS_DIR`, plus `LT_LLAMA_SERVER` for the e2e fixture.

Pass page names to render only selected fixtures, for example `node app/ui/scripts/render-screen-fixtures.mjs tilted-card tilted-sign`. An element with explicit `[data-line]` children records each child's bounds as one source line or vertical column. Other elements use character boxes to find horizontal lines.

The real-model horizontal OCR baseline covers `article`, `chat`, `dense`, `mixed`, `subtitle` and `ui-panel` at both scales, with a maximum character error rate of 2.4%. The same test then checks four tilted fixtures and four vertical fixtures for exact group membership and text. Vertical box matching requires IoU 0.9 on 98% of boxes and exact text on 95% of matches.

`lt-cli screen --direction vertical` reads columns down the page and groups them right to left. The default is `horizontal`. Output records `direction`; vertical raw lines also carry `glyph_pitch`, preserving metrics from the orientation in which each crop was read. `--explain` reports horizontal grouping decisions; vertical output includes its final groups in the usual events.

The screenshot toolbar remembers Horizontal/Vertical in `screen.direction` and reads the same area again when it changes. English is fitted across a column group. Tilted blocks retain their level layout and use rotation or shear for drawing. Direction layout fixtures in `app/ui/e2e/fixtures/directions` record native geometry and English; `recorded-vertical.screen.json` is a real `lt-cli screen --direction vertical` job checked at 150% display scale.

### Validate Direction And Drawing

Run `npm --prefix app/ui run e2e -- screen-directions` for column fitting, rotation,
shear, plate alignment and the recorded vertical job at 150% display scale. The
real-model OCR test runs through `scripts/check.ps1`; set absolute
`LT_MODELS_DIR` and `LT_LLAMA_SERVER` paths so model tests run from crate folders.

For a native comparison, display the same PNG at the same size in both builds,
capture it with the screenshot shortcut and drag the same area. Use Vertical for
vertical pages and Horizontal for tilted cards and ordinary screenshots. Change
the picker with a result visible to check that it reads the same area again, then
close and reopen the screenshot window to check the saved direction. Wait for
English and all background images before saving a comparison image.

Record pointer-up to blocks, first English and done separately. Keep raw events,
display dimensions and the chosen direction with each result. State whether the
process and translation cache are fresh; changing direction invalidates OCR reuse
but can reuse translations. Sample main-process private memory while OCR remains
loaded and identify excluded child processes. A native capture can contain cursor
effects and display resampling, so use the canonical PNG tests for numeric parity.

## Package

```powershell
./scripts/package.ps1 -Version 1.0.0
```

The script builds the UI and the release app. It makes `dist/LiveTranslation-<version>-win-x64.zip` with the app, the native DLLs, `llama-server`, the WebView2 bootstrapper (signature checked), the notices and `docs/USER-README.md`. It does not include models. `-SkipUi`, `-SkipBuild`, `-SkipBootstrapper` and `-Jobs` change the steps.

### Online Translation Tests

`cargo test -p lt-web` checks recorded answers and an accepted request signature without network access. Live tests are opt-in and are separate from `check.ps1`:

```powershell
$env:LT_WEB_LIVE = '1'
cargo test -p lt-web --test live -- --nocapture --test-threads=1
cargo test -p lt-cli --features ocr --test screen online_screen -- --nocapture
cargo run -p lt-web --example record -- microsoft
cargo run -p lt-web --example record -- google
cargo run -p lt-web --example record -- google_alt
cargo run -p lt-web --example record -- google_beta
cargo run -p lt-web --example record -- bing
cargo run -p lt-web --example record -- youdao
cargo run -p lt-web --example measure > out/online-latency.json
Remove-Item Env:LT_WEB_LIVE
```

The recorder writes fixed public sentences and raw answers under `crates/lt-web/tests/fixtures/`. Regenerate a recording only when the endpoint contract changes on purpose, inspect its parsed expected text, and record the command and date in the fixture README. Never send private screenshots, their recognized text or private transcripts to an online test.

### OpenAI-Compatible Live Test

Set `LT_WEB_LIVE=1`, `LT_OPENAI_BASE_URL`, `LT_OPENAI_MODEL`, and optionally `LT_OPENAI_API_KEY`, then run `cargo test -p lt-web --test live openai_compatible_sentence -- --nocapture`. It sends only the fixed test sentence. The test reports skipped when the endpoint or model is absent. Fake HTTP tests in `crates/lt-web/tests/openai.rs` run offline and cover streaming, headers, temperature compatibility and errors.

### Recorder And Measurements

The recorder also accepts `google_alt` for the internal Google alternate transport. Its batchexecute answers are matched by position tags; refused or untagged calls are retried individually once. It has no separate setting.

The measurement example sends 20 fixed caption-length Chinese sentences and one 30-text batch per keyless transport, sequentially. It requires `LT_WEB_LIVE=1`, uses primary transports directly so fallback cannot hide a failure, and reports individual, median and worst elapsed times plus failures. The first sentence includes lazy session initialization; the single batch result has one sample. Save machine, command and date alongside output. Tencent is deferred following endpoint certificate failure and is not measured. OpenAI-compatible measurements require a separately configured endpoint; its live test remains opt-in.

`translate.fallback` defaults to true for selectable keyless engines. Set it false in a test config to isolate one engine; the recorder and measurement example already isolate transports. Offline fallback tests use fake engines to verify primary retry, whole-batch backup ownership, first-error preservation and cancellation. Fallback never changes local or OpenAI-compatible behavior.
