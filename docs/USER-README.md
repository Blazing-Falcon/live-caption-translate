# Live Translation

Live Translation shows English captions for Chinese speech on your PC. It listens to the audio that your computer plays (a video, a call, a stream). It recognizes the Chinese, translates it to English, and shows the result as a subtitle on your screen.

On a PC with 6 or more cores, the first English words appear in approximately one second, in a dimmer color. The final translation replaces them a short time later.

All processing is on your computer. After the first download, the app does not use the network. There are no accounts, no telemetry and no uploads.

## Requirements

- Windows 10 or Windows 11, 64-bit.
- A CPU with AVX2, 4 cores or more, and 8 GB of RAM. We recommend 6 cores and 16 GB. On slower PCs the captions come later.
- Approximately 3 GB of free disk space for the models. The optional "Faster captions" model needs 480 MB more.
- The Microsoft Edge WebView2 Runtime. Windows 11 and most Windows 10 PCs have it. If it is missing, the app starts its setup.
- "Selected apps" capture needs Windows 10 build 20348 or later (usually Windows 11). Whole-system capture works on all supported versions.

## First run

1. Unzip the folder to a location where you can write. Start `live-translation.exe`. The app has no code signature, so Windows SmartScreen can ask you to confirm.
2. The first-run screen shows the models and downloads them one time. On PCs with 6 or more cores, the list includes "Faster captions" (480 MB). This model is optional. Without it, the app uses the Light caption speed.
   - Select Hugging Face or ModelScope as the source.
   - You can pause and resume the download.
   - "Use files I already have" uses model files that are on your PC.
   - The app checks each file with a SHA-256 hash before it uses it.
3. When the download is complete, the window shows **Capture** and the app starts to listen.

## Use the app

| Page | Settings |
|---|---|
| **Capture** | Listen to the **whole system** (you can select an output device), or to **selected apps** such as Chrome and Discord. The app remembers apps by program name. |
| **Overlay** | Style (subtitle bar at the bottom, or caption panel at a screen edge), text size, background opacity, Chinese line above the English, "Show Chinese while someone is speaking", "Draft text" |
| **Languages** | Chinese to English. English speech shows as heard. You can turn on translation for Japanese and Korean. Otherwise they show untranslated. |
| **Performance** | **Caption speed**, word delay ("Word to first English", "Word to final English", medians for the last minute), memory, CPU, translator threads |

When you close the control window, the app continues in the system tray. Right-click the tray icon for Pause, Hide overlay, Move overlay, Listen to, Open transcript folder, Settings and Quit.

The overlay does not take focus. It does not show in Alt-Tab or the taskbar. Mouse clicks go through it to the window below.

### Hotkeys

You can change the hotkeys on the Overlay page.

| Keys | Action |
|---|---|
| Ctrl + Shift + L | Move or lock the overlay. In move mode, drag the overlay or its edges. Press again to lock it and keep the position. |
| Ctrl + Shift + H | Show or hide the overlay |
| Ctrl + Shift + P | Pause or resume |

If a different program uses a hotkey, the Overlay page tells you. The other hotkeys continue to work.

## Caption speed and draft text

Set the **Caption speed** on the Performance page:

| Setting | Result |
|---|---|
| Automatic (recommended) | Continuous on PCs with 6 or more cores and the Faster captions model. Light on other PCs. |
| Continuous | The English follows the speaker in approximately one second. This uses the most CPU (approximately two cores in our test). |
| Light | The app translates each phrase when it ends. No draft text. Less CPU. |
| Wait for full sentences | The app translates when the speaker pauses. |

In Continuous mode, a line has three steps:

1. The Chinese appears while the app recognizes it.
2. A dimmed English draft grows below it. The draft can change while the person speaks.
3. The final translation replaces the draft. The transcript keeps only the final text.

**Draft text** on the Overlay page sets how much of the draft you see: all words except the newest (default, least flicker), only words that do not change, or all words.

Three options on the Performance page change the CPU use:

- **Give games priority** (on by default): both translators run at low priority, so a game keeps its frame rate. If you turn it off, the translators restart.
- **Slow down when the PC is busy** (on by default): the app changes from Continuous to Light when the PC is busy, and back when it is free. The status line under Caption speed tells you when and why.
- **Split long sentences** (on by default): the app translates a long sentence in parts when the speaker does not pause.

If you select Continuous and the Faster captions model is missing, the status line tells you and shows a **Download (480 MB)** button. Captions continue in Light mode until the download is complete.

## Limitations

- The overlay does not show over games in exclusive fullscreen. Borderless and windowed modes work.
- The app cannot capture apps that use WASAPI exclusive mode.
- The app does not identify speakers. Two persons who speak quickly one after the other can be in one caption. 他 and 她 sound the same, so the translation often uses "he".
- The app recognizes English words in Chinese speech correctly only 55 to 66% of the time.
- If a sentence is longer than 10 seconds, the cut can remove part of a character.
- The translations are machine output. If the translator is more than 6 seconds late, it skips older lines to catch up. The Chinese of those lines stays visible.
- Draft English comes from a small model (0.6B). It can be wrong or change before the final text comes. If you want only final text, use Light or Wait for full sentences.
- In Light and Wait for full sentences, captions come after a phrase or sentence ends, usually in 1 to 3 seconds.

## Files

| Data | Location |
|---|---|
| Settings | `%APPDATA%\app.livetranslation.desktop\config.toml` |
| Models | `%LOCALAPPDATA%\app.livetranslation.desktop\models` |
| Transcripts (one `.jsonl` file for each session, kept 30 days) | `%LOCALAPPDATA%\app.livetranslation.desktop\transcripts` |
| Logs | `%LOCALAPPDATA%\app.livetranslation.desktop\logs` |

Transcripts contain only final text, never drafts or live Chinese. They contain what other persons said. To stop them, turn them off on the Performance page, or delete the folder. At the default level, logs do not contain caption text.

## Remove the app

1. Quit the app from the tray menu.
2. Delete the unzipped folder.
3. Delete the two `app.livetranslation.desktop` folders in the table above. Paste each path into the Explorer address bar to find them.

The app writes nothing more. It has no installer and no registry entries.

## Licenses

See `THIRD-PARTY-NOTICES.txt`. The speech model has the FunASR Model Open Source License. Read it before you use this app for more than personal use.
