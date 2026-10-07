# Live Translation

Live English captions for Chinese speech on your PC. It listens to what your computer is playing (a video, a call, a stream), recognizes the Chinese, translates it to English, and shows the result as a subtitle over your screen. Everything runs on your computer. After the first-run download there is no network use: no accounts, no telemetry, no uploads.

## Requirements

- Windows 10 or Windows 11, 64-bit.
- A CPU with AVX2, 4 cores or more, and 8 GB of RAM. We recommend 6 cores and 16 GB. On slower PCs the captions come later.
- About 2.5 GB of free disk space for the speech and translation models.
- The Microsoft Edge WebView2 Runtime. Windows 11 and most Windows 10 PCs have it. If it is missing, the app starts its setup.
- "Selected apps" capture needs Windows 10 build 20348 or later (usually Windows 11). Whole-system capture works on all supported versions.

## First run

1. Unzip the folder to a location where you can write. Start `live-translation.exe`. The app has no code signature, so Windows SmartScreen can ask you to confirm.
2. The first-run screen lists three models (voice detection, speech recognition, translation) and downloads them once. Choose Hugging Face or ModelScope as the source, pause and resume at any time, or point it to files you already have ("Use files I already have"). Every file is checked against a fixed SHA-256 before it is used.
   - Select Hugging Face or ModelScope as the source.

## Use the app

| Page | Settings |
- **Overlay** sets the style (a subtitle bar at the bottom of the screen, or a caption panel docked to a screen edge), text size, background opacity and whether the Chinese line shows above the English.
| **Capture** | Listen to the **whole system** (you can select an output device), or to **selected apps** such as Chrome and Discord. The app remembers apps by program name. |
- **Performance** shows how long a sentence takes from speech to English, the waiting queue, memory, and the translator thread count.
| **Languages** | Chinese to English. English speech shows as heard. You can turn on translation for Japanese and Korean. Otherwise they show untranslated. |

When you close the control window, the app continues in the system tray. Right-click the tray icon for Pause, Hide overlay, Move overlay, Listen to, Open transcript folder, Settings and Quit.

The overlay does not take focus. It does not show in Alt-Tab or the taskbar. Mouse clicks go through it to the window below.

| Keys | Action |
|---|---|
| Ctrl + Shift + L | Move or lock the overlay. In move mode, drag the overlay or its edges. Press again to lock it and keep the position. |
| Ctrl + Shift + H | Show or hide the overlay |
| Ctrl + Shift + P | Pause or resume |

If a different program uses a hotkey, the Overlay page tells you. The other hotkeys continue to work.

## Limitations

- The overlay does not show over games in exclusive fullscreen. Borderless and windowed modes work.
- The app cannot capture apps that use WASAPI exclusive mode.
- The app does not identify speakers. Two persons who speak quickly one after the other can be in one caption. 他 and 她 sound the same, so the translation often uses "he".
- The app recognizes English words in Chinese speech correctly only 55 to 66% of the time.
- If a sentence is longer than 10 seconds, the cut can remove part of a character.
- The translations are machine output. If the translator is more than 6 seconds late, it skips older lines to catch up. The Chinese of those lines stays visible.
- Captions appear after a sentence ends, usually within 1 to 2 seconds. They are not word-by-word live.

## Files

| Data | Location |
|---|---|
| Settings | `%APPDATA%\app.livetranslation.desktop\config.toml` |
| Models | `%LOCALAPPDATA%\app.livetranslation.desktop\models` |
| Transcripts (one `.jsonl` file for each session, kept 30 days) | `%LOCALAPPDATA%\app.livetranslation.desktop\transcripts` |
| Logs | `%LOCALAPPDATA%\app.livetranslation.desktop\logs` |

Transcripts contain what other people said. Turn them off on the Performance page or delete the folder if you do not want them. Logs never contain caption text at the default level.

## Remove the app

1. Quit the app from the tray menu.

## Licenses

See `THIRD-PARTY-NOTICES.txt`. The speech model has the FunASR Model Open Source License. Read it before you use this app for more than personal use.
