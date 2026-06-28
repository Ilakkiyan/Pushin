// Dev-only screenshot harness (NOT a real test) — boots the app via the mock bridge and captures
// the themed UI so progress can be reviewed remotely. Run explicitly:
//   npx playwright test tests/e2e/_capture.spec.ts --project=chromium
// PNGs land in target/ui-shots/ (gitignored). Delete this file before committing.
import { test } from "@playwright/test";
import { installMockBridge } from "./_mockBridge";
import fs from "node:fs";

const OUT = "target/ui-shots";

test("capture themed views", async ({ page }) => {
  fs.mkdirSync(OUT, { recursive: true });
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(`pageerror: ${e.message}`));
  await page.setViewportSize({ width: 1280, height: 820 });
  await installMockBridge(page);

  // --- opening wordmark, frozen on its settled frame ---
  await page.goto("/?splash=logo");
  await page.waitForTimeout(1600);
  await page.screenshot({ path: `${OUT}/03-splash.png` });
  await page.screenshot({ path: `${OUT}/03-splash.jpg`, type: "jpeg", quality: 85 });

  // --- returning-user welcome (splash skipped, not yet entered) ---
  await page.goto("/?splash=off");
  await page.getByText(/Good (morning|afternoon|evening)/).waitFor({ timeout: 20000 });
  await page.waitForTimeout(700); // welcome-in settle
  await page.screenshot({ path: `${OUT}/04-welcome.png` });

  // --- new-user guide (un-onboarded) ---
  await page.goto("/?splash=off&new=1");
  await page.waitForTimeout(2500);
  await page.screenshot({ path: `${OUT}/05-guide.png` });
  const guideBody = (await page.locator("body").innerText().catch(() => "")).slice(0, 200);
  errors.push(`guide body: ${guideBody.replace(/\n/g, " | ")}`);

  // --- inner app (splash skipped + entered) ---
  await page.goto("/?splash=off&enter=1");
  await page.getByText("Today").first().waitFor({ timeout: 20000 });
  await page.waitForTimeout(400);
  await page.screenshot({ path: `${OUT}/01-calendar.png` });
  await page.screenshot({ path: `${OUT}/01-calendar.jpg`, type: "jpeg", quality: 85 });

  // Calendar slot selection: click an empty time cell → cursor ring; arrows move it.
  try {
    await page.mouse.click(500, 470);
    await page.waitForTimeout(150);
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowDown");
    await page.waitForTimeout(200);
    await page.screenshot({ path: `${OUT}/09-cal-select.png` });
    await page.keyboard.press("Escape");
  } catch (e) {
    errors.push(`cal-select: ${e}`);
  }

  // Command palette modal (Ctrl+K) — opened from the calendar (settings pane crashes headless).
  try {
    await page.keyboard.press("Control+k");
    await page.waitForTimeout(400);
    await page.screenshot({ path: `${OUT}/06-palette.png` });
    await page.keyboard.press("Escape");
  } catch (e) {
    errors.push(`palette: ${e}`);
  }

  // g-then-key navigation: `g v` should jump to the Vault. Blur any field first (the hook ignores
  // keys while typing — by design).
  try {
    await page.keyboard.press("Escape");
    await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
    await page.keyboard.press("g");
    await page.keyboard.press("v");
    await page.waitForTimeout(400);
    await page.screenshot({ path: `${OUT}/08-gnav-vault.png` });
  } catch (e) {
    errors.push(`gnav: ${e}`);
  }

  // Mobile shell.
  try {
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto("/?splash=off&enter=1");
    await page.waitForTimeout(900);
    await page.screenshot({ path: `${OUT}/07-mobile.png` });
  } catch (e) {
    errors.push(`mobile: ${e}`);
  }

  fs.writeFileSync(`${OUT}/_diag.txt`, `errors:\n${errors.join("\n") || "(none)"}\n`);
});
