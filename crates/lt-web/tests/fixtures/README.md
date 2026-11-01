# Online Translation Recordings

Recorded on 2026-11-01 with `LT_WEB_LIVE=1`:

```powershell
cargo run -p lt-web --example record -- microsoft
cargo run -p lt-web --example record -- google
cargo run -p lt-web --example record -- google_beta
cargo run -p lt-web --example record -- google_alt
```

The fixed three-sentence list is public test prose, including Simplified and Traditional Chinese. `public.json` is the raw response body; `public.expected.json` is its inspected translation list. Microsoft's `signature.json` records the URL, UTC date, nonce and signature accepted with this response. It contains no user credential.

Parse tests require the exact count and text. Regenerate only after an intentional contract change and report the command and date.

Google alternate records the original position-tagged batchexecute response. It is an internal transport, without a config or dropdown value.

Google Beta recordings contain translation bodies only. Runtime script keys and request headers are not saved.
