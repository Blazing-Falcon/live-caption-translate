# Models and runtime

`models/manifest.json` is the source of truth. The app embeds it at build time. The repository and the package do not contain models. The app downloads them at first run. Developers use `scripts/setup.ps1 -Models` or `lt-cli models fetch`.

## Models

| Id | Use | File | Bytes | SHA-256 |
|---|---|---|---|---|
| `sensevoice-2024-07-17-int8` | Speech recognition | `model.int8.onnx` | 239,233,841 | `c71f0ce00bec95b07744e116345e33d8cbbe08cef896382cf907bf4b51a2cd51` |
| | | `tokens.txt` | 315,894 | `f449eb28dc567533d7fa59be34e2abca8784f771850c78a47fb731a31429a1dc` |
| `silero-vad-v5` | Voice activity | `silero_vad_v5.onnx` | 2,313,101 | `6b99cbfd39246b6706f98ec13c7c50c6b299181f2474fa05cbc8046acc274396` |
| `hy-mt2-1.8b-q4_0` | Final translation | `Hy-MT2-1.8B.i1-Q4_0.gguf` | 1,079,997,440 | `77b7db506ed5dddd79b30553d346af1bfbed5999838441964ade98c970512087` |
| `lmt-60-0.6b-q4_k_m` | Drafts (optional) | `LMT-60-0.6B.Q4_K_M.gguf` | 484,220,000 | `743c6cdc13294b5c470ec0e89e79d64f96ef0cab94ce1285c78da4dc0f56be5c` |

Sources (Hugging Face):

- SenseVoice: `csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17`
- Silero VAD: `csukuangfj/vad`
- Hy-MT2: `mradermacher/Hy-MT2-1.8B-i1-GGUF`
- LMT-60: `mradermacher/LMT-60-0.6B-GGUF` (base model `NiuTrans/LMT-60-0.6B`)

**Do not use** the SenseVoice export `2025-09-09`. It is a Cantonese model. It tags all speech as `yue`, has no punctuation and gives worse English.

Rules:

- The first-run download includes the draft model on PCs with 6 or more physical cores. On other PCs the app offers it when the user selects Continuous.
- Files go to `<models dir>/<id>/<file>`.
- Downloads continue from `<file>.part` with HTTP range requests. The app checks the SHA-256 and then renames the file.
- The app uses the system proxy.
- At startup the app checks file sizes. It checks hashes after a download and for "Use files I already have".
- The `mirror` field holds a ModelScope URL. An empty field disables ModelScope for that file.

## Licenses

| Component | License |
|---|---|
| SenseVoice Small | FunASR model license v1.1. Read it before you use the app for more than personal use. |
| Silero VAD v5 | MIT |
| Hy-MT2 1.8B | Apache-2.0 (recorded; confirm on the `tencent/Hy-MT2-1.8B` model card) |
| LMT-60 0.6B | Apache-2.0 |
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

**SenseVoice:** sherpa-onnx C API, `language = auto`, `use_itn = true`, 1 thread, greedy search, 16 kHz mono input. The result JSON gives the text, the language tag, the audio event and token times. The commit rule uses the token times.

**Silero VAD:** runs directly on ONNX Runtime, because the sherpa VAD does not give a probability for each frame. Frames are 512 samples with 64 samples of context. Speech opens and closes at probability 0.5. A soft cut uses 0.35.

**llama-server:** one binary, two processes. The supervisor starts each server with:

```
llama-server.exe -m <gguf> --host 127.0.0.1 --port <free port> --jinja -np 1 -c <ctx>
                 -t <threads> -tb <threads> [--override-kv tokenizer.ggml.eos_token_id=int:120020] --no-webui
```

- Only the Hy-MT2 server gets `--override-kv`. The GGUF has an incorrect end token (the correct id is 120020).
- `<threads>` is 2 for each server by default. `translate.threads` sets 1–4.
- Do not add `--no-repack`. llama.cpp repacks Q4_0 for x86, and that makes Hy-MT2 much faster.
- With `latency.low_priority`, the servers run at below-normal priority (not in Off mode).

**Requests:**

| | Final | Draft |
|---|---|---|
| Endpoint | `/v1/chat/completions` | `/completion` |
| Streaming | Yes | No |
| `max_tokens` | `min(256, 4 × characters + 16)` | `min(64, 3 × characters + 8)` |
| Timeout | 10 s | 3 s |
| Other | temperature 0, repeat penalty 1.05, prompt cache, identical instruction text each time | Fixed `Chinese: … English:` prompt. A prefill continues from the previous draft. |

`translate.server_url`, `latency.draft_server_url` or `LT_LLAMA_SERVER` select a different server.
