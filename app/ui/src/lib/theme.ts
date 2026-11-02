import type { Session } from "./events";

type ThemeName = "light" | "dark";

/** Applies `app.theme`; "system" follows the Windows theme Tauri reports, else prefers-color-scheme. */
export function followTheme(session: Session): void {
  let windowsTheme: ThemeName | null = null;
  let chosen: "system" | ThemeName = "system";
  const apply = (): void => {
    const theme = chosen === "system" ? windowsTheme : chosen;
    if (theme === null) delete document.documentElement.dataset.theme;
    else document.documentElement.dataset.theme = theme;
  };
  session.config.subscribe((config) => {
    chosen = config?.app.theme ?? "system";
    apply();
  });
  if (!("__TAURI_INTERNALS__" in window)) return;
  void (async () => {
    try {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      const current = getCurrentWindow();
      windowsTheme = await current.theme();
      apply();
      await current.onThemeChanged(({ payload }) => {
        windowsTheme = payload;
        apply();
      });
    } catch {
      windowsTheme = null;
      apply();
    }
  })();
}
