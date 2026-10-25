# Models and runtime

`models/manifest.json` is the source of truth. The app embeds it at build time. The repository and the package do not contain models. The app downloads them at first run. Developers use `scripts/setup.ps1 -Models` or `lt-cli models fetch`.

## Models

| Id | Use | File | Bytes | SHA-256 |
|---|---|---|---|---|
| `xasr-stream-480ms-int8-2026-06-05` | Live speech recognition | archive, 4 files | 133,895,136 | in the manifest |
| `hy-mt2-1.8b-ternary-tq2_0` | Translation | download `Hy-MT2-1.8B-1.25Bit.gguf`, kept as `Hy-MT2-1.8B-ternary-TQ2_0.gguf` | 461,860,800 (606,564,288 kept) | in the manifest |
| `xasr-offline-int8-2026-06-03` | Second pass (always on) | archive, 4 files | 136,396,739 | in the manifest |

Sources (Hugging Face):

- Hy-MT2 1.25-bit: `tencent/Hy-MT2-1.8B-1.25Bit-GGUF`, revision `9df5c824a00a744fb0512a29c640466f4d97dfb0` (AngelSlim 1.25-bit quantization)
- X-ASR streaming: the `asr-models` release of `k2-fsa/sherpa-onnx` on GitHub (`sherpa-onnx-x-asr-480ms-streaming-zipformer-transducer-zh-en-punct-int8-2026-06-05.tar.bz2`). The int8 files exist only in this archive.
- X-ASR offline: the same release, `sherpa-onnx-x-asr-zipformer-transducer-zh-en-punct-int8-2026-06-03.tar.bz2`.

Rules:

- The first-run download takes the required models: the 480 ms streaming model, the offline X-ASR model and the translator. Listening does not start while one of them is missing.
- Files go to `<models dir>/<id>/<file>`.
- Downloads continue from `<file>.part` with HTTP range requests. The app checks the SHA-256 and then renames the file.
- A file with `convert` is a GGUF whose weights are STQ1_0, a ternary type that only an unmerged ARM branch of llama.cpp reads. After its SHA-256 check, the app rewrites those weights as TQ2_0 (the same blocks of 256 weights of -1, 0 or +1 times one fp16 scale, which llama.cpp runs on x86), checks the result's size and SHA-256, keeps it under the `convert` name and deletes the download. Every weight keeps its value; the rest of the file is copied unchanged.
- A file with an `extract` list is a `.tar.bz2` archive. After its SHA-256 check, the app takes the listed files from it by file name, checks each file's size and SHA-256, and deletes the archive.
- The app uses the system proxy.
- At startup the app checks file sizes. It checks hashes after a download and for "Use files I already have".

## Licenses

| Component | License |
|---|---|
| X-ASR | Apache-2.0 (upstream `Gilgamesh-J/X-ASR`) |
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

**X-ASR streaming:** sherpa-onnx online recognizer, zipformer transducer, greedy search, `asr.threads` (1). It hears all audio while listening, also silence. The result JSON gives token times and token log-probabilities, but no language tag: the share of Latin letters and Chinese characters sets the language. Pause rules: 2.4 s of silence with no words, `asr.pause_s` (0.8 s) after words, and 60 s at most. At a break the app adds silence (two 480 ms chunks plus 0.5 s) before the end of input, so the encoder decodes the last words. The model reports a word after it starts: the first word after silence comes 0.34 s after the speech onset that Silero VAD v5 finds (median of the 20 ASCEND clips). The app subtracts this delay from the token times.

**X-ASR offline (second pass):** sherpa-onnx offline recognizer in the app, the same zipformer transducer decoded over the whole window, greedy search, `asr.second_pass_threads` (2). The window gets 5 s of earlier speech as context; `trim` keeps the part that belongs to the clause. Its result JSON has no token times or log-probabilities; the app uses only the text. It shares the ONNX Runtime with the streaming model.

ASCEND clips (20), whole clips, 2026-10-25, `cargo test -p lt-sherpa --release --test offline`: character error rate 0.091, against 0.102 for the streaming model (`--test stream`). Clause windows of the v0.7.0 review runs, Python replay with the app's trim rules, Ryzen 7 5700X, 2026-10-25: character errors over drama, 5 min of conversation and FLEURS (3,262 characters) 238 with the offline pass on every line and 5 s of context, against 263 for the live text; context 0 s: 259, 2 s: 246, 10 s: 239, 20 s: 240 on the same lines.

**Translation model:** Hy-MT2 1.8B in Tencent's 1.25-bit quantization (AngelSlim), as TQ2_0. The app used the 4-bit Q4_0 file (mradermacher, imatrix) before:

| | 1.25-bit (used) | Q4_0 (removed) |
|---|---|---|
| Memory (llama-server working set) | 732 MB | 1181 MB |
| Time per line | 411 ms | 595 ms |

Measured on the 26 lines of a drama clip, each line alone, 2 threads, `--load-mode none`, 2026-10-15, Ryzen 7 5700X. Both translated all lines correctly; each was better on a few.

The 1.25-bit model and earlier lines, on 81 lines of a drama session, 30 characters of earlier lines, 2026-10-15, same PC (earlier lines as prompt context were removed on 2026-10-26):

| Earlier lines given as | Per line | Memory | Result |
|---|---|---|---|
| none | 418 ms | 787 MB | wrong subjects where they come from an earlier line |
| joined with `<br>`, English prefilled (the form Q4_0 used) | | | other scripts and runaway answers on some lines |
| Hy-MT2's "Background Information" prompt | 811 ms | 1016 MB | the prompt echoed or the background translated on 10 lines |
| chat turns, the instruction in every turn | 1391 ms | 1255 MB | no failures |
| chat turns, the instruction in the first turn only | 852 ms | 1047 MB | no failures; "I really like it" (a comic) instead of "him", "you" instead of "myself" |

**llama-server:** the supervisor starts the server with:

```
llama-server.exe -m <gguf> --host 127.0.0.1 --port <free port> --jinja -np 1 -c <ctx>
                 -ub 128 -b 512 -t <threads> -tb <threads>
                 [--override-kv tokenizer.ggml.eos_token_id=int:120020]
                 --load-mode none --cache-ram 0 --no-webui
```

- `--override-kv` corrects the GGUF's end token (the correct id is 120020). Only the translator gets it.
- `<ctx>` is 1024, fixed in the app.
- `-ub 128 -b 512` lower the prefill activation buffers, which grow with the micro-batch, from llama.cpp's 512 (`-ub`) and 2048 (`-b`). This is commit charge, not working set: the weights are most of the working set and these flags do not touch them. Prefill may split into more batches.
- `<threads>` is `translate.threads` (4 by default), at most the PC's logical CPUs.
- Do not add `--no-repack`. llama.cpp repacks some weight types for x86; for the Q4_0 file the app used before, that made Hy-MT2 much faster.
- `--load-mode none` reads the model into memory instead of mapping the file. The repacked copy is the only copy then. With a mapped file, the original pages also stay in memory: for the Q4_0 file, 2074 MB working set against 1271 MB, with the same translations (26 lines, 2026-10-15, Ryzen 7 5700X).
- `--cache-ram 0` turns off the host prompt cache. By default llama-server keeps the state of each earlier prompt in memory, up to 8 GiB, so the server grew for the whole session. The one slot still reuses the prefix it shares with the last request. Measured 2026-10-15, Ryzen 7 5700X: the translator after the 150 lines of a drama session 1240 MB with the cache, 710 MB without, 463 and 466 ms per line, the same translations.
- With `performance.low_priority`, the server runs at below-normal priority.

The memory figures on this page are working set (`WorkingSetSize`), which counts pages shared with other processes and drops when Windows trims it. The app samples private bytes (`PROCESS_MEMORY_COUNTERS_EX.PrivateUsage`, commit charge) for itself and the translator alongside it. Compare a number only with a number of the same counter.

**Requests:**

| | |
|---|---|
| Endpoint | `/v1/chat/completions` |
| Streaming | Yes |
| `max_tokens` | `min(256, 4 × characters + 16)` |
| Timeout | 10 s |
| Other | temperature 0, repeat penalty 1.05, prompt cache, identical instruction text each time |

`LT_LLAMA_SERVER` selects a different `llama-server` binary.
