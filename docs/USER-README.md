# Live Translation

Live Translation shows English captions for Chinese speech on your PC. It listens to the audio that your computer plays (a video, a call, a stream). It recognizes the Chinese, translates it to English, and shows the result as a subtitle on your screen.

On a PC with 6 or more cores, the English for a sentence appears about two and a half seconds after the speaker stops saying it.

The app can also translate Chinese text that is on your screen (a web page, a game menu, a chat, video subtitles). Press a hotkey, drag a box over the text, and the English appears over it.

All processing is on your computer. After the downloads, the app does not use the network. There are no accounts, no telemetry and no uploads.

## Requirements

- Windows 10 or Windows 11, 64-bit.
- A CPU with AVX2, 4 cores or more, and 8 GB of RAM. We recommend 6 cores and 16 GB. On slower PCs the captions come later.
- Free disk space for the models: 303 MB for speech, 311 MB more for "Double-check each line", and 1.07 GB more to translate on this PC. Screen translation adds a model of 23 MB.
- The Microsoft Edge WebView2 Runtime. Windows 11 and most Windows 10 PCs have it. If it is missing, the app starts its setup.
- "Selected apps" capture needs Windows 10 build 20348 or later (usually Windows 11). Whole-system capture works on all supported versions.

## First run

1. Unzip the folder to a location where you can write. Start `live-translation.exe`. The app has no code signature, so Windows SmartScreen can ask you to confirm.
2. The first start asks three things: how to translate (a free online service, on this PC, or your own AI server), whether to double-check each line, and what to listen to. It shows the download size, then downloads only the models those choices need.
   - You can pause and resume the download.
   - "Use files I already have" uses model files that are on your PC.
   - The app checks each file with a SHA-256 hash before it uses it.
3. When the download is complete, the app starts to listen.

## Use the app

| Page | Settings |
|---|---|
| **Home** | What the app hears and who translates, Start or Pause, quick switches (show captions, Chinese line, move captions), and how it's going: how soon English appears, whether it keeps up, CPU and memory |
| **Captions** | A live preview, layout (side panel, running text or film subtitles), card (Night or Outline), font, English and Chinese colors, size, card see-through, Chinese line, Chinese while someone is talking, lines kept, side of the screen, move |
| **Listening** | Everything on this PC (with a speaker or headset) or only the apps you pick, start when the app opens, **Double-check each line**, the pause that starts a new line, even out voices, threads |
| **Translation** | A free online service (Microsoft, Google, Google Beta, Bing, Youdao, with a backup), on this PC (downloads the translator first), or your own AI server. A note always says who sees the Chinese text. |
| **Screenshot** | How to use it, text direction, the text reader download |
| **Shortcuts** | The five hotkeys. Press **Change**, then the new keys. |
| **History** | Save transcripts, how long to keep them, read past sessions in the app, open the folder |
| **About & help** | App theme, downloads and the model folder, log detail and folder, **Reset settings** |

**Reset settings** sets all settings back to their defaults. The caption position, the selected apps and the downloaded models stay.

When you close the control window, the app continues in the system tray. Right-click the tray icon for Pause, Hide overlay, Move overlay, Listen to, Open transcript folder, Settings and Quit.

The overlay does not take focus. It does not show in Alt-Tab or the taskbar. Mouse clicks go through it to the window below.

### Hotkeys

You can change the hotkeys on the Shortcuts page.

| Keys | Action |
|---|---|
| Ctrl + Shift + L | Move or lock the overlay. In move mode, drag the overlay or its edges. Press again to lock it and keep the position. |
| Ctrl + Shift + H | Show or hide the overlay |
| Ctrl + Shift + P | Pause or resume |
| Ctrl + Shift + S | Open the settings window. The overlay lets clicks through, so this is the way back to it from a game. |
| Ctrl + Shift + A | Translate text on the screen (see below). Press it again to close the frozen screen. |

If a different program uses a hotkey, the Shortcuts page tells you. The other hotkeys continue to work.

## Translate text on the screen

1. Press Ctrl + Shift + A. The app freezes the monitor that has the mouse pointer. A video behind the frozen picture keeps playing, but you see the picture from the moment you pressed the key.
2. Drag a box over the Chinese text. The app reads the text and translates it in blocks, one after the other. Each block of English appears over the Chinese when it is ready, and the Chinese stays until then.
3. A bar under the box shows the progress. **Copy English** and **Copy Chinese** copy the blocks in reading order. **Show original** hides the English while you hold it (the Space key does the same). **Close**, Esc, a right click or the hotkey again closes the frozen screen. Drag a new box to read another area.

The first time, the app needs the text recognition model (23 MB). The bar shows a **Download** button, and then reads your box again. The Screenshot page also has this download. The translator that makes the captions translates the text, so the first screen translation can take about a second longer while the model loads, if you were not listening. The app does not save the screenshot. It stays in memory until you close the frozen screen.

## How captions appear

1. The Chinese appears while the person speaks.
2. The line ends at the end of a sentence (。？！) or when the person pauses. If a sentence runs on with commas, a comma can end the line after about 2 seconds. After 30 seconds, the app cuts the line at a gap between words.
3. The English appears below the Chinese. The transcript keeps only the final text.

English speech shows as heard. It is not translated.

**Give games priority** on the Translation page (on by default, for the translator on this PC) runs the translator at low priority, so a game keeps its frame rate. If you turn it off, the translator restarts.

## Limitations

- The overlay does not show over games in exclusive fullscreen. Borderless and windowed modes work.
- The app cannot capture apps that use WASAPI exclusive mode.
- The app does not identify speakers. Two persons who speak quickly one after the other can be in one caption. 他 and 她 sound the same, so the translation often uses "he".
- The app recognizes English words in Chinese speech correctly only 55 to 66% of the time.
- If a sentence is longer than 10 seconds, the cut can remove part of a character.
- The translations are machine output. If the translator is more than 6 seconds late, it skips older lines to catch up. The Chinese of those lines stays visible.
- The English comes after a sentence or phrase ends, so it trails the speaker by a few seconds.
- Screen translation reads horizontal text only. Vertical and rotated text (manga, some posters) is not supported. It works on one monitor at a time.
- Short labels have no context, so a few are wrong. In a test on a settings dialog, 4 of 34 labels came out clearly wrong (for example 撤销 became "Cancel").

## Files

| Data | Location |
|---|---|
| Settings | `%APPDATA%\app.livetranslation.desktop\config.toml` |
| Models | `%LOCALAPPDATA%\app.livetranslation.desktop\models` |
| Transcripts (one `.jsonl` file for each session, kept 30 days) | `%LOCALAPPDATA%\app.livetranslation.desktop\transcripts` |
| Logs | `%LOCALAPPDATA%\app.livetranslation.desktop\logs` |

Transcripts contain only final text, never live Chinese. They contain what other persons said. To stop them, turn them off on the History page, or delete the folder. At the default level, logs do not contain caption text.

## Remove the app

1. Quit the app from the tray menu.
2. Delete the unzipped folder.
3. Delete the two `app.livetranslation.desktop` folders in the table above. Paste each path into the Explorer address bar to find them.

The app writes nothing more. It has no installer and no registry entries.

## Licenses

See `THIRD-PARTY-NOTICES.txt`.
