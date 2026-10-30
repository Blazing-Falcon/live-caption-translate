# Models and runtime

`models/manifest.json` is the source of truth. The app embeds it at build time. The repository and
the package do not contain models. The app downloads the three required models at first run and the
optional one when the user first asks for screenshot translation. Developers use
`scripts/setup.ps1 -Models` (every model) or `lt-cli models fetch` (the required models; add
`--optional` for the text recognition model).

## Models

| Id | Use | File |
|---|---|---|
| `xasr-stream-480ms-int8-2026-06-05` | Live speech recognition | `.tar.bz2`, 4 files |
| `xasr-offline-int8-2026-06-03` | Second pass, always on | `.tar.bz2`, 4 files |
| `hy-mt2-1.8b-ternary-tq2_0` | Translation | `Hy-MT2-1.8B-1.25Bit.gguf`, rewritten and kept as `Hy-MT2-1.8B-ternary-TQ2_0.gguf` |
| `pp-ocrv6-det-tiny-rec-small` | Screenshot text recognition, optional | `det.onnx`, `rec.onnx`, `dict.txt` |

Sizes and SHA-256 hashes are in the manifest. The text recognition model is 23 MB.

- Hy-MT2 1.25-bit: `tencent/Hy-MT2-1.8B-1.25Bit-GGUF`, revision `9df5c824a00a744fb0512a29c640466f4d97dfb0`.
- X-ASR streaming and offline: the `asr-models` release of `k2-fsa/sherpa-onnx` on GitHub. The int8
  files exist only in the archives.
- PP-OCRv6 detector (`det.onnx`): `PaddlePaddle/PP-OCRv6_tiny_det_onnx` on Hugging Face, revision
  `2ba1506c0380b8f0b03dd142459aac66d4421f6c`, file `inference.onnx`.
- PP-OCRv6 recognizer (`rec.onnx`): `PaddlePaddle/PP-OCRv6_small_rec_onnx` on Hugging Face, revision
  `b8f84f0b80c529de40b4fbb3544b84fa7233a513`, file `inference.onnx`.
- PP-OCRv6 dictionary (`dict.txt`): `ppocr/utils/dict/ppocrv6_dict.txt` in `PaddlePaddle/PaddleOCR` on
  GitHub, commit `dab3fe35379033fdcb2d0e9572fac0b36c9a9ebf`. The Hugging Face repositories have no
  dictionary file.

## Download rules

- The first-run download takes the three required models. Listening does not start while one is
  missing.
- The text recognition model is optional (`"optional": true` in the manifest). The first-run
  download, `models_download` and `lt-cli models fetch` skip it; `lt-cli models fetch --optional` and
  `scripts/setup.ps1 -Models` include it. The app downloads it when the user
  chooses **Download**: on the Performance page of the control window, or in the screenshot window,
  which offers it when a screenshot finds the model missing and then reads the same box again. Listening
  never needs it.
- Files go to `<models dir>/<id>/<file>`.
- Downloads continue from `<file>.part` with HTTP range requests. The app checks the SHA-256, then
  renames the file.
- `convert`: the download is a GGUF with STQ1_0 weights, a ternary type that only an unmerged ARM
  branch of llama.cpp reads. The app rewrites each block as TQ2_0 (the same 256 weights of -1, 0 or
  +1 times one fp16 scale, which llama.cpp runs on x86), checks the result, keeps it under the
  `convert` name and deletes the download. Every weight keeps its value.
- `extract`: the download is a `.tar.bz2`. The app takes the listed files from it by name, checks
  each one's size and SHA-256, and deletes the archive.
- The app uses the system proxy.
- At startup the app checks file sizes. It checks hashes after a download and for "Use files I
  already have".

## Engines

**X-ASR streaming:** sherpa-onnx online recognizer, zipformer transducer, greedy search,
`asr.threads` (1). It hears all audio while listening, silence included. The result JSON has token
times and log-probabilities but no language tag; the app classifies the text by its characters.
Pause rules: 2.4 s of silence with no words, `asr.pause_s` (0.8 s) after words, and 60 s at most. At
a break the app adds silence before the end of input so the encoder decodes the last words. The
engine reports each word late; the app subtracts a fixed delay from the token times.

**X-ASR offline (second pass):** sherpa-onnx offline recognizer in the app, the same zipformer
transducer over the whole window, greedy search, `asr.second_pass_threads` (2). The window carries 5 s
of earlier speech as context; `trim` keeps the part that belongs to the clause. The result JSON has
no token times or log-probabilities, so the app uses only the text. It shares the ONNX Runtime with
the streaming model.

**Translation model:** Hy-MT2 1.8B, Tencent's 1.25-bit quantization, as TQ2_0. One request per unit,
one user message with a frozen instruction.

**Screenshot text recognition:** the PaddleOCR PP-OCRv6 tiny detector and small recognizer, run by
`lt-ocr` through the ONNX Runtime that sherpa-onnx loads. The detector sees the box with an 8 px border
and no stretching, at most 2048 px on the longest side, and returns a probability map. The app turns
that into axis-aligned boxes (threshold 0.2, box score at least 0.4, unclip ratio 1.4). The recognizer
reads each box at a height of 48 px with greedy CTC decoding and drops lines with a score under 0.6.
There is no angle classifier: it reads horizontal text only, so vertical and rotated text is not
supported. Threads are half the logical CPUs, from 1 to 4. The `lt-screen` thread loads the model
when it is first needed and drops it after 5 minutes without a job.

**llama-server:** the supervisor starts it with:

```
llama-server.exe -m <gguf> --host 127.0.0.1 --port <free port> --jinja -np 1 -c 1024
                 -ub 128 -b 512 -t <threads> -tb <threads>
                 --override-kv tokenizer.ggml.eos_token_id=int:120020
                 --load-mode none --cache-ram 0 --no-webui
```

- `<threads>` is `translate.threads` (4 by default), at most the PC's logical CPUs.
- Do not add `--no-repack`: llama.cpp repacks some weight types for x86.
- `--load-mode none` reads the model into memory instead of mapping it, so the repacked copy is the
  only copy.
- `--cache-ram 0` turns off the host prompt cache, which otherwise keeps earlier prompts in memory.
  One slot still reuses the prefix it shares with the last request.
- `-ub 128 -b 512` lower the prefill activation buffers, which lowers commit charge.
- With `performance.low_priority`, the server runs at below-normal priority.

Memory figures for these choices are working set; the app also samples private bytes (commit charge)
for itself and the server.

**Requests:**

| | |
|---|---|
| Endpoint | `/v1/chat/completions` |
| Streaming | Yes |
| `max_tokens` | `2 × characters + 8`, at least 24, at most 256 |
| Timeout | 10 s |
| Other | temperature 0, repeat penalty 1.05, prompt cache, identical instruction text each time |

Screenshot translation sends the same requests, one for each piece of text of at most 120 characters.

`LT_LLAMA_SERVER` selects a different `llama-server` binary.

## Licenses

| Component | License |
|---|---|
| X-ASR | Apache-2.0 (upstream `Gilgamesh-J/X-ASR`) |
| Hy-MT2 1.8B | Apache-2.0 (recorded; confirm on the `tencent/Hy-MT2-1.8B` model card) |
| Hy-MT2 1.8B 1.25-bit | Apache-2.0 (`tencent/Hy-MT2-1.8B-1.25Bit-GGUF` LICENSE.txt and model card) |
| PP-OCRv6 tiny detector, small recognizer | Apache-2.0 (the model cards of `PaddlePaddle/PP-OCRv6_tiny_det_onnx` and `PaddlePaddle/PP-OCRv6_small_rec_onnx`, and the `PaddlePaddle/PaddleOCR` LICENSE) |
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

- `scripts/prepare-native-runtime.py` downloads the first two archives to `.deps/` and checks size
  and hash before extracting.
- It replaces the ONNX Runtime 1.17.1 in the sherpa archive with 1.30.0: the recognizer needs API 28.
- The no-TTS sherpa build does not contain GPLv3 eSpeak NG.
- Builds find the libraries with `SHERPA_ONNX_LIB_DIR` (set by `scripts/env.ps1`).
- Executables need the DLLs in their folder; `scripts/install-native-runtime.ps1` copies them into
  `target/`.
