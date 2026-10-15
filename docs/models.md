# Models and runtime

`models/manifest.json` is the source of truth. The app embeds it at build time. The repository and the package do not contain models. The app downloads them at first run. Developers use `scripts/setup.ps1 -Models` or `lt-cli models fetch`.

## Models

| Id | Use | File | Bytes | SHA-256 |
|---|---|---|---|---|
| `xasr-stream-480ms-int8-2026-06-05` | Live speech recognition (default) | archive, 4 files | 133,895,136 | in the manifest |
| `hy-mt2-1.8b-ternary-tq2_0` | Translation | download `Hy-MT2-1.8B-1.25Bit.gguf`, kept as `Hy-MT2-1.8B-ternary-TQ2_0.gguf` | 461,860,800 (606,564,288 kept) | in the manifest |
| `qwen3-asr-0.6b-q4_k_m` | Second pass (`asr.second_pass = "qwen3"`) | `Qwen3-ASR-0.6B.Q4_K_M.gguf`, `mmproj-Qwen3-ASR-0.6B-Q8_0.gguf` | 698,608,768 | in the manifest |
| `xasr-stream-960ms-int8-2026-06-05` | Live speech recognition (optional) | archive, 4 files | 133,895,831 | in the manifest |
| `xasr-stream-160ms-int8-2026-06-05` | Live speech recognition (optional) | archive, 4 files | 133,898,007 | in the manifest |

Sources (Hugging Face):

- Hy-MT2 1.25-bit: `tencent/Hy-MT2-1.8B-1.25Bit-GGUF`, revision `9df5c824a00a744fb0512a29c640466f4d97dfb0` (AngelSlim 1.25-bit quantization)
- Qwen3-ASR: `mradermacher/Qwen3-ASR-0.6B-GGUF` revision `6cf166e9f1c5bd2521108e3410ee5358de5eabab` (the language model, Q4_K_M) and `ggml-org/Qwen3-ASR-0.6B-GGUF` revision `928ab958557df9aa2ef1c93e0e83c7ad0933fae2` (the audio encoder, Q8_0), conversions of `Qwen/Qwen3-ASR-0.6B`
- X-ASR streaming: the `asr-models` release of `k2-fsa/sherpa-onnx` on GitHub (`sherpa-onnx-x-asr-160ms-…`, `-480ms-…` and `-960ms-streaming-zipformer-transducer-zh-en-punct-int8-2026-06-05.tar.bz2`). The int8 files exist only in these archives.

Rules:

- The first-run download takes the required models, Qwen3-ASR included. The optional models (the other chunk sizes) download only when the user selects them on the Performance page. Listening does not start while a selected model is missing.
- Files go to `<models dir>/<id>/<file>`.
- Downloads continue from `<file>.part` with HTTP range requests. The app checks the SHA-256 and then renames the file.
- A file with `convert` is a GGUF whose weights are STQ1_0, a ternary type that only an unmerged ARM branch of llama.cpp reads. After its SHA-256 check, the app rewrites those weights as TQ2_0 (the same blocks of 256 weights of -1, 0 or +1 times one fp16 scale, which llama.cpp runs on x86), checks the result's size and SHA-256, keeps it under the `convert` name and deletes the download. Every weight keeps its value; the rest of the file is copied unchanged.
- A file with an `extract` list is a `.tar.bz2` archive. After its SHA-256 check, the app takes the listed files from it by file name, checks each file's size and SHA-256, and deletes the archive.
- The app uses the system proxy.
- At startup the app checks file sizes. It checks hashes after a download and for "Use files I already have".
- The `mirror` field holds a ModelScope URL. An empty field disables ModelScope for that file.

## Licenses

| Component | License |
|---|---|
| X-ASR | Apache-2.0 (upstream `Gilgamesh-J/X-ASR`) |
| Qwen3-ASR 0.6B | Apache-2.0 (`Qwen/Qwen3-ASR-0.6B` model card, `QwenLM/Qwen3-ASR` LICENSE; the mradermacher conversion declares apache-2.0) |
| Hy-MT2 1.8B | Apache-2.0 (recorded; confirm on the `tencent/Hy-MT2-1.8B` model card) |
| Hy-MT2 1.8B 1.25-bit | Apache-2.0 (`tencent/Hy-MT2-1.8B-1.25Bit-GGUF` LICENSE.txt and model card) |
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

**Qwen3-ASR (second pass):** a second llama-server with the audio encoder (`--mmproj`), `asr.second_pass_threads` (2), context 1024 (a 30 s window takes about 400 audio tokens), `--load-mode none`, no warm-up. The engine starts it with the pipeline and stops it with the pipeline. Each window goes as 16-bit WAV to `/v1/chat/completions`; the answer is `language <Name><asr_text><text>`. A forced language is the start of the answer (`language Chinese<asr_text>`). Its failures show as `failed` second-pass lines, not as an engine state, because the live recognizer keeps working.

Against the same model in ONNX Runtime (sherpa-onnx int8), on the drama clip, real time, 2 threads, 2026-10-15, Ryzen 7 5700X: memory 275 MB for the app plus 1165 MB for the server, against 1760 MB in the app, which grew by about 33 MB for each second of the longest window read (2143 MB after a 31 s window); decode 845 ms against 916 ms (median); character error rate 6.2% against 7.0%.

ASCEND clips (20, Chinese and English mixed), whole clips, 2026-10-15, `cargo test -p lt-llm --release --test real_speech`: character error rate of Qwen3-ASR 0.131 with the detected language and with `zh` forced. The streaming X-ASR on the same clips: 0.102.

SenseVoice Small (sherpa-onnx int8) was the earlier second pass. On the drama clip it lowered the character error rate from 9.9% (live text) to 9.5% and changed some correct lines; Qwen3-ASR lowered it to 6.2% (real time, 2026-10-13..15, same PC). On the ASCEND clips its error rate was 0.184 (0.159 with `zh` forced).

**Translation model:** Hy-MT2 1.8B in Tencent's 1.25-bit quantization (AngelSlim), as TQ2_0. The app used the 4-bit Q4_0 file (mradermacher, imatrix) before:

| | 1.25-bit (used) | Q4_0 (removed) |
|---|---|---|
| Memory (llama-server working set) | 732 MB | 1181 MB |
| Time per line | 411 ms | 595 ms |

Measured on the 26 lines of a drama clip, each line alone, 2 threads, `--load-mode none`, 2026-10-15, Ryzen 7 5700X. Both translated all lines correctly; each was better on a few.

The 1.25-bit model and earlier lines, on 81 lines of a drama session, 30 characters of earlier lines, 2026-10-15, same PC:

| Earlier lines given as | Per line | Memory | Result |
|---|---|---|---|
| none | 418 ms | 787 MB | wrong subjects where they come from an earlier line |
| joined with `<br>`, English prefilled (the form Q4_0 used) | | | other scripts and runaway answers on some lines |
| Hy-MT2's "Background Information" prompt | 811 ms | 1016 MB | the prompt echoed or the background translated on 10 lines |
| chat turns, the instruction in every turn | 1391 ms | 1255 MB | no failures |
| chat turns, the instruction in the first turn only (used) | 852 ms | 1047 MB | no failures; "I really like it" (a comic) instead of "him", "you" instead of "myself" |

**llama-server:** the supervisor starts the server with:

```
llama-server.exe -m <gguf> --host 127.0.0.1 --port <free port> --jinja -np 1 -c <ctx>
                 -t <threads> -tb <threads> [--override-kv tokenizer.ggml.eos_token_id=int:120020]
                 --load-mode none --no-webui
```

- `--override-kv` corrects the GGUF's end token (the correct id is 120020).
- `<threads>` is `translate.threads` (2 by default), at most the PC's logical CPUs.
- Do not add `--no-repack`. llama.cpp repacks some weight types for x86; for the Q4_0 file the app used before, that made Hy-MT2 much faster.
- `--load-mode none` reads the model into memory instead of mapping the file. The repacked copy is the only copy then. With a mapped file, the original pages also stay in memory: for the Q4_0 file, 2074 MB working set against 1271 MB, with the same translations (26 lines, 2026-10-15, Ryzen 7 5700X).
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
