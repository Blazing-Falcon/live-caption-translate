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
| `LT_DRAFT_MODEL` | Draft model file |
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

These tests skip if their variables are not set. `check.ps1` shows the skipped tests.

| Test | Needs | Checks |
|---|---|---|
| `lt-sherpa` `vad`, `native` | `LT_MODELS_DIR`, test audio | Silero and SenseVoice against the parity fixtures |
| `lt-sherpa` Juan trace | Also `LT_JUAN_CLIP` (default `testdata/fetched/juan-clip-16k.wav`) | Windowed decodes of a recorded trace. The clip is private. |
| `lt-llm` `real_server` | `LT_LLAMA_SERVER`, Hy-MT2 (`LT_MT_MODEL` or `LT_MODELS_DIR`) | 37 sentences against the fixture |
| `lt-llm` drafts and mode switches | `LT_LLAMA_SERVER`, `LT_DRAFT_MODEL` | 40 draft sentences; servers start and stop correctly |

`LT_REFERENCE_AUDIO` sets the test audio folder (default `testdata/fetched`).

```powershell
. ./scripts/env.ps1
$env:LT_MODELS_DIR = "$PWD/models"
$env:LT_LLAMA_SERVER = "$PWD/.deps/llama-b11429/llama-server.exe"
$env:LT_DRAFT_MODEL = "$PWD/models/lmt-60-0.6b-q4_k_m/LMT-60-0.6B.Q4_K_M.gguf"
cargo test --workspace --all-features
```

## Replay

`setup.ps1 -TestAudio` downloads the public clips to `testdata/fetched/`. Their licenses permit only local tests. Do not commit them.

```powershell
cargo build --release -p lt-cli --all-features
target\release\lt-cli.exe replay testdata\fetched\ramc\CTS-CN-F2F-2019-11-15-1449.wav --start 19 --dur 180 `
    --pace realtime --mode continuous --server .deps\llama-b11429\llama-server.exe --out out\conv.jsonl
python scripts\replay-checks.py out\conv.events.jsonl      # event order, repeats, expected text
target\release\lt-cli.exe live --mode system --dur 600 --out out\live.jsonl
target\release\lt-cli.exe bench asr testdata\fetched\ascend
```

`replay` writes the transcript, `<out>.events.jsonl` and `<out>.words.json`, and prints the per-word delays. When you report a number, also give the machine, the command and what else was running.

## Package

```powershell
./scripts/package.ps1 -Version 0.2.0
```

The script builds the UI and the release app. It makes `dist/LiveTranslation-<version>-win-x64.zip` with the app, the native DLLs, `llama-server`, the WebView2 bootstrapper (signature checked), the notices and `docs/USER-README.md`. It does not include models. `-SkipUi`, `-SkipBuild`, `-SkipBootstrapper` and `-Jobs` change the steps.
