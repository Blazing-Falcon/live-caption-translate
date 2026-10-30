Raw text lines that `lt-ocr` found in each `testdata/screen/<name>.png`, recorded with

    lt-cli screen testdata/screen/<name>.png --no-translate --out crates/lt-core/tests/fixtures/screen/<name>.lines.json

(release build with `--features ocr`, `LT_MODELS_DIR` set). `tests/screen_group.rs` reads only `width`, `height` and `lines`; `events` shows the grouping at recording time and goes stale when the grouping changes. Record again when the recognizer changes.

`accepted-grouping.json` records exact accepted partitions and output geometry, text and intent for four historical recordings. The OCR recordings and hand-marked truth remain unchanged. The other eight recordings retain their truth assertions.
