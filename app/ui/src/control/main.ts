import { mount } from "svelte";
import { createSession } from "../lib/events";
import "../lib/fonts.css";
import "../lib/tokens.css";
import "./control.css";
import ControlApp from "./ControlApp.svelte";

type ThemeName = "light" | "dark";

/** The Windows theme, when Tauri reports one; otherwise prefers-color-scheme decides. */
let windowsTheme: ThemeName | null = null;
let chosen: "system" | ThemeName = "system";

function applyTheme(): void {
  const theme = chosen === "system" ? windowsTheme : chosen;
  if (theme === null) delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = theme;
}

/** Windows theme changes normally arrive through prefers-color-scheme; the Tauri event covers forced themes. */
async function followTauriTheme(): Promise<void> {
  if (!("__TAURI_INTERNALS__" in window)) return;
  try {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    const current = getCurrentWindow();
    windowsTheme = await current.theme();
    applyTheme();
    await current.onThemeChanged(({ payload }) => {
      windowsTheme = payload;
      applyTheme();
    });
  } catch {
    windowsTheme = null;
    applyTheme();
  }
}

const session = createSession({ kind: "control" });
session.config.subscribe((config) => {
  chosen = config?.app.theme ?? "system";
  applyTheme();
});
const target = document.getElementById("app");
if (!target) throw new Error("Missing #app mount point");

mount(ControlApp, { target, props: { session } });
void followTauriTheme();

window.addEventListener("pagehide", () => session.dispose(), { once: true });
