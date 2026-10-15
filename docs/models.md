# Models and runtime

`models/manifest.json` is the source of truth. The app embeds it at build time. The repository and the package do not contain models. The app downloads them at first run. Developers use `scripts/setup.ps1 -Models` or `lt-cli models fetch`.

## Models

| Id | Use | File | Bytes | SHA-256 |
|---|---|---|---|---|
| `xasr-stream-480ms-int8-2026-06-05` | Live speech recognition (default) | archive, 4 files | 133,895,136 | in the manifest |
| `hy-mt2-1.8b-q4_0` | Final translation | `Hy-MT2-1.8B.i1-Q4_0.gguf` | 1,079,997,440 | `77b7db506ed5dddd79b30553d346af1bfbed5999838441964ade98c970512087` |
| `sensevoice-2024-07-17-int8` | Second pass (`asr.second_pass`) | `model.int8.onnx`, `tokens.txt` | 239,549,735 | in the manifest |
| `qwen3-asr-0.6b-int8-2026-03-25` | Second pass, optional (`asr.second_pass = "qwen3"`) | 6 files | 987,015,347 | in the manifest |
| `xasr-stream-960ms-int8-2026-06-05` | Live speech recognition (optional) | archive, 4 files | 133,895,831 | in the manifest |
| `xasr-stream-160ms-int8-2026-06-05` | Live speech recognition (optional) | archive, 4 files | 133,898,007 | in the manifest |

Sources (Hugging Face):

- Hy-MT2: `mradermacher/Hy-MT2-1.8B-i1-GGUF`
- Qwen3-ASR: `csukuangfj2/sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25`, revision `68818b2313fe77bd06f6a7c5068ff3ef59d02b8a` (the sherpa-onnx conversion of `Qwen/Qwen3-ASR-0.6B`)
- SenseVoice: `csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17`, revision `2365baeacb507f821a0c8120fcee3d484dba7a07` (the sherpa-onnx conversion of `FunAudioLLM/SenseVoiceSmall`)
- X-ASR streaming: the `asr-models` release of `k2-fsa/sherpa-onnx` on GitHub (`sherpa-onnx-x-asr-160ms-…`, `-480ms-…` and `-960ms-streaming-zipformer-transducer-zh-en-punct-int8-2026-06-05.tar.bz2`). The int8 files exist only in these archives.

Rules:

- The first-run download takes the required models, SenseVoice included. The optional models (other chunk sizes, Qwen3-ASR) download only when the user selects them on the Performance page. Listening does not start while a selected model is missing.
- Files go to `<models dir>/<id>/<file>`.
- Downloads continue from `<file>.part` with HTTP range requests. The app checks the SHA-256 and then renames the file.
- A file with an `extract` list is a `.tar.bz2` archive. After its SHA-256 check, the app takes the listed files from it by file name, checks each file's size and SHA-256, and deletes the archive.
- The app uses the system proxy.
- At startup the app checks file sizes. It checks hashes after a download and for "Use files I already have".
- The `mirror` field holds a ModelScope URL. An empty field disables ModelScope for that file.

## Licenses

| Component | License |
|---|---|
| X-ASR | Apache-2.0 (upstream `Gilgamesh-J/X-ASR`) |
| Qwen3-ASR 0.6B | Apache-2.0 (`Qwen/Qwen3-ASR-0.6B` model card, `QwenLM/Qwen3-ASR` LICENSE) |
| SenseVoice Small | FunASR Model Open Source License Agreement 1.1 (custom). It requires attribution and the model name. Commercial use is not confirmed by the license owner (`QwenAudio/SenseVoice` issue 334). |
| Hy-MT2 1.8B | Apache-2.0 (recorded; confirm on the `tencent/Hy-MT2-1.8B` model card) |
| sherpa-onnx 1.13.8 | Apache-2.0 |
| ONNX Runtime 1.30.0 | MIT |
| llama.cpp b11429 | MIT (includes LLVM OpenMP) |

The full texts are in `notices/`. The package contains `notices/THIRD-PARTY-NOTICES.txt`.

## Native runtime

| Item | Archive | SHA-256 |
|---|---|---|
| sherpa-onnx 1.13.8, shared, no TTS | `sherpa-onnx-v1.13.8-win-x64-shared-MT-Release-no-tts-lib.tar.bz2` | `a1253e665c4f236119c443c8932a8acfca32a546c78c05962d483c9a0eae21b7` |
| ONNX Runtime 1.30.0 | `onnxruntime-win-x64-1.30.0.zip` | `c6ba983baf5681af108599675d2a89c2d145512d02de28aed0bff177cd0ba949` |
| llama.cpp b11429, CPU x64 | `llama-b11429-bin-win-cpu-x64.zip` | `1283323272b04cd07905816a597a0da810918102de958f4ff6f7bbaa70ed2efe` |

- `scripts/prepare-native-runtime.py` downloads the first two archives to `.deps/`. It checks size and hash before it extracts files.
- It replaces the ONNX Runtime 1.17.1 in the sherpa archive with 1.30.0. The recognizer needs ONNX Runtime API 28, and 1.17.1 does not have it.
- The no-TTS sherpa build does not contain GPLv3 eSpeak NG.
- Builds find the libraries with `SHERPA_ONNX_LIB_DIR` (set by `scripts/env.ps1`).
- Executables need the DLLs in their folder. `scripts/install-native-runtime.ps1` copies them into `target/`.

## Engines

**X-ASR streaming:** sherpa-onnx online recognizer, zipformer transducer, greedy search, `asr.threads` (1). It hears all audio while listening, also silence. The result JSON gives token times and token log-probabilities, but no language tag: the share of Latin letters and Chinese characters sets the language. Pause rules: 2.4 s of silence with no words, `asr.pause_s` (0.8 s) after words, and 60 s at most. At a break the app adds silence (two chunks plus 0.5 s) before the end of input, so the encoder decodes the last words. The model reports a word after it starts: the first word after silence comes 0.44 s (160 ms), 0.34 s (480 ms) or 0.23 s (960 ms) after the speech onset that Silero VAD v5 finds (median of the 20 ASCEND clips). The app subtracts these delays from the token times.

**SenseVoice (second pass):** sherpa-onnx offline recognizer, greedy search, inverse text normalization on (punctuation and digits), `asr.second_pass_threads` (1). It decodes one window of audio at a time and detects the language. The language is fixed when the recognizer is created; a stream option does not change it. A decode with a forced language (`zh`) therefore uses a second recognizer, which the app creates the first time it needs one.

ASCEND clips (20, Chinese and English mixed), whole clips, 2026-10-13, `cargo test -p lt-sherpa --release --test sensevoice`: character error rate 0.184 with the detected language, 0.159 with `zh` forced. The streaming X-ASR on the same clips: 0.102.

**llama-server:** the supervisor starts the server with:

```
llama-server.exe -m <gguf> --host 127.0.0.1 --port <free port> --jinja -np 1 -c <ctx>
                 -t <threads> -tb <threads> [--override-kv tokenizer.ggml.eos_token_id=int:120020] --no-webui
```

- `--override-kv` corrects the GGUF's end token (the correct id is 120020).
- `<threads>` is `translate.threads` (2 by default), at most the PC's logical CPUs.
- Do not add `--no-repack`. llama.cpp repacks Q4_0 for x86, and that makes Hy-MT2 much faster.
- With `performance.low_priority`, the server runs at below-normal priority.

**Requests:**

| | |
|---|---|
| Endpoint | `/v1/chat/completions` |
| Streaming | Yes |
| `max_tokens` | `min(256, 4 × characters + 16)` |
| Timeout | 10 s |
| Other | temperature 0, repeat penalty 1.05, prompt cache, identical instruction text each time |

`translate.server_url` or `LT_LLAMA_SERVER` select a different server.
