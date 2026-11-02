import { expect, test } from "@playwright/test";
import { calls, open } from "./support";

test.use({ viewport: { width: 360, height: 440 } });

test("the tray panel changes the everyday switches and opens the settings window", async ({ page }) => {
  const external = await open(page, "tray", { config: { translate: { engine: "microsoft" } } });
  await expect(page.getByTestId("tray-status")).toContainText("Listening");
  await expect(page.getByTestId("tray-status")).toContainText("Microsoft · online");
  await page.getByRole("switch", { name: "Chinese line" }).click();
  await expect.poll(async () => (await calls(page, "set_config")).at(-1)?.args.patch).toEqual({ overlay: { show_source: false } });
  await page.getByRole("radio", { name: "Subtitles" }).check();
  await expect.poll(async () => (await calls(page, "set_config")).at(-1)?.args.patch).toEqual({ overlay: { style: "subtitles" } });
  await page.getByRole("button", { name: "Pause" }).click();
  await expect.poll(async () => (await calls(page, "pause_listening")).length).toBe(1);
  await page.getByRole("button", { name: "All settings →" }).click();
  await expect.poll(async () => (await calls(page, "open_settings")).length).toBe(1);
  expect(await page.evaluate(() => document.documentElement.scrollHeight)).toBeLessThanOrEqual(440);
  expect(external).toEqual([]);
});
