# UI mockup boards

The design boards the UI was built from. `specs/screenshots.spec.ts` renders them next to the app for a side-by-side look; they are not used by the app itself.

- Open any `*.dc.html` in a browser (file:// works). `support.js` is a small shim that renders the templates. Options go in the URL, for example `Main.dc.html?mode=Move&barOpacity=60` or `Controls.dc.html?tab=perf&source=apps&running=false`.

| File | Shows |
|---|---|
| `Main.dc.html` | Overlay style A, subtitle bar: two utterances (previous faded), Chinese line above English, amber streaming caret, status chip, move mode with dashed outline and Lock |
| `OverlayPanel.dc.html` | Overlay style B, caption panel docked right, history, header on hover |
| `CaptionStates.dc.html` | Every caption line state: pending, streaming, final, English speech, other language, skipped, failed, status |
| `Controls.dc.html` | Control window: header card, Capture (system / apps), Overlay, Languages, Performance, footer |
| `FirstRun.dc.html` | First-run model download |
| `Tray.dc.html` | Tray menu |

Differences from the real app:
- Fonts: the boards use IBM Plex Sans/Mono and Noto Sans SC from Google Fonts as stand-ins. The app uses Segoe UI Variable, Microsoft YaHei UI and Cascadia Mono (system fonts, nothing shipped).
- Window chrome is a placeholder; the real control window uses the normal Windows title bar.
- Numbers and app names are sample data. Only the light control window is shown.
