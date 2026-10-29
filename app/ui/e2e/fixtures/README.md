`article@1.5.screen.json` is what `lt-cli screen` printed for `testdata/screen/article@1.5.png`, recorded 2026-10-29:

    lt-cli screen testdata/screen/article@1.5.png --out app/ui/e2e/fixtures/article@1.5.screen.json

(release build with `--features ocr`, `LT_MODELS_DIR` and `LT_LLAMA_SERVER` set; the translations are the real Hy-MT2 answers). `specs/screen.spec.ts` serves the PNG as the frozen frame (1200 x 720 px at scale 1.5) and replays `events`. Record it again when the recognizer, the grouping or the translator changes on purpose.
