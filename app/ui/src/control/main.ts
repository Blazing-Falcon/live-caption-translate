import { mount } from "svelte";
import { createSession } from "../lib/events";
import "../lib/tokens.css";
import "./control.css";
import ControlApp from "./ControlApp.svelte";

type ThemeName = "light" | "dark";

function applyTheme(theme: ThemeName | null): void {
  if (theme === null) delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = theme;
}

/** Windows theme changes normally arrive through prefers-color-scheme; the Tauri event covers forced themes. */
async function followTauriTheme(): Promise<void> {
  if (!("__TAURI_INTERNALS__" in window)) return;
  try {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    const current = getCurrentWindow();
    applyTheme(await current.theme());
    await current.onThemeChanged(({ payload }) => applyTheme(payload));
  } catch {
    applyTheme(null);
  }
}

const session = createSession({ kind: "control" });
const target = document.getElementById("app");
if (!target) throw new Error("Missing #app mount point");

mount(ControlApp, { target, props: { session } });
void followTauriTheme();

window.addEventListener("pagehide", () => session.dispose(), { once: true });
