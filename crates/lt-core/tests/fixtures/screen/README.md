Raw text lines that `lt-ocr` found in each `testdata/screen/<name>.png`, recorded with

    lt-cli screen testdata/screen/<name>.png --no-translate --out crates/lt-core/tests/fixtures/screen/<name>.lines.json

(release build with `--features ocr`, `LT_MODELS_DIR` set). `tests/screen_group.rs` reads only `width`, `height` and `lines`; `events` shows the grouping at recording time and goes stale when the grouping changes. Record again when the recognizer changes.

`tests/screen_group.rs` decodes `testdata/screen/<name>.png`, runs the horizontal path of `run_job` on the recorded lines and checks every block against `testdata/screen/<name>.truth.json`. The `KNOWN_MISSES` list in that test names the truth blocks the grouping splits; the test fails when one stops or a new one appears.

`imaging-golden.json` contains frozen SHA-256 outputs for 30 deterministic image routines and 11 complete repair-and-plate scenes. Inputs use fixed integer seeds and half-even rounding. Every registered hash remains required.
