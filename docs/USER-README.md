# Live Translation

Live Translation shows English captions for Chinese speech on your PC. It listens to the audio that your computer plays (a video, a call, a stream). It recognizes the Chinese, translates it to English, and shows the result as a subtitle on your screen.

On a PC with 6 or more cores, the first English words appear in approximately one second, in a dimmer color. The final translation replaces them a short time later.

All processing is on your computer. After the first download, the app does not use the network. There are no accounts, no telemetry and no uploads.

## Requirements

- Windows 10 or Windows 11, 64-bit.
- A CPU with AVX2, 4 cores or more, and 8 GB of RAM. We recommend 6 cores and 16 GB. On slower PCs the captions come later.
- Approximately 3 GB of free disk space for the models.
- The Microsoft Edge WebView2 Runtime. Windows 11 and most Windows 10 PCs have it. If it is missing, the app starts its setup.
- "Selected apps" capture needs Windows 10 build 20348 or later (usually Windows 11). Whole-system capture works on all supported versions.

## First run

1. Unzip the folder to a location where you can write. Start `live-translation.exe`. The app has no code signature, so Windows SmartScreen can ask you to confirm.
2. The first-run screen shows the models and downloads them one time.
   - Select Hugging Face or ModelScope as the source.
   - You can pause and resume the download.
   - "Use files I already have" uses model files that are on your PC.
   - The app checks each file with a SHA-256 hash before it uses it.
3. When the download is complete, the window shows **Capture** and the app starts to listen.

## Use the app

| Page | Settings |
|---|---|
| **Capture** | Listen to the **whole system** (you can select an output device), or to **selected apps** such as Chrome and Discord. The app remembers apps by program name. |
| **Overlay** | Style (subtitle bar at the bottom, caption panel at a screen edge, or running text that joins the lines into one paragraph, with 2 to 6 lines kept), text size, background opacity, Chinese line above the English, "Show Chinese while someone is speaking" |
| **Languages** | Chinese to English. English speech shows as heard. You can turn on translation for Japanese and Korean. Otherwise they show untranslated. |
| **Performance** | "Give games priority", "Translate with the previous lines", word delay ("Word to English", median for the last minute), memory, CPU, translator threads |

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

## How captions appear

1. The Chinese appears while the person speaks.
2. The line ends at the end of a sentence (。？！) or when the person pauses. If a sentence runs on with commas, a comma can end the line after about 6 seconds. After 30 seconds, the app cuts the line at a gap between words.
3. The English appears below the Chinese. The transcript keeps only the final text.

**Give games priority** on the Performance page (on by default) runs the translator at low priority, so a game keeps its frame rate. If you turn it off, the translator restarts.

## Limitations

- The overlay does not show over games in exclusive fullscreen. Borderless and windowed modes work.
- The app cannot capture apps that use WASAPI exclusive mode.
- The app does not identify speakers. Two persons who speak quickly one after the other can be in one caption. 他 and 她 sound the same, so the translation often uses "he".
- The app recognizes English words in Chinese speech correctly only 55 to 66% of the time.
- If a sentence is longer than 10 seconds, the cut can remove part of a character.
- The translations are machine output. If the translator is more than 6 seconds late, it skips older lines to catch up. The Chinese of those lines stays visible.
- The English comes after a sentence or phrase ends, so it trails the speaker by a few seconds.

## Files

| Data | Location |
|---|---|
| Settings | `%APPDATA%\app.livetranslation.desktop\config.toml` |
| Models | `%LOCALAPPDATA%\app.livetranslation.desktop\models` |
| Transcripts (one `.jsonl` file for each session, kept 30 days) | `%LOCALAPPDATA%\app.livetranslation.desktop\transcripts` |
| Logs | `%LOCALAPPDATA%\app.livetranslation.desktop\logs` |

Transcripts contain only final text, never live Chinese. They contain what other persons said. To stop them, turn them off on the Performance page, or delete the folder. At the default level, logs do not contain caption text.

## Remove the app

1. Quit the app from the tray menu.
2. Delete the unzipped folder.
3. Delete the two `app.livetranslation.desktop` folders in the table above. Paste each path into the Explorer address bar to find them.

The app writes nothing more. It has no installer and no registry entries.

## Licenses

See `THIRD-PARTY-NOTICES.txt`.
