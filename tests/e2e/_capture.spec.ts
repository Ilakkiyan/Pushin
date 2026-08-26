// Dev-only screenshot harness (NOT a real test) — boots the app via the mock bridge and captures
// the themed UI so progress can be reviewed remotely. Run explicitly:
//   npx playwright test tests/e2e/_capture.spec.ts --project=chromium
// PNGs land in target/ui-shots/ (gitignored). Delete this file before committing.
import { test } from "@playwright/test";
import { installMockBridge } from "./_mockBridge";
import fs from "node:fs";

const OUT = "target/ui-shots";

// In `npm run dev`, App.tsx forces the "What's New" intro on every launch (import.meta.env.DEV), which
// overlays the whole app. Click "Explore" to clear it if it's up; no-op otherwise.
async function dismissWhatsNew(page: import("@playwright/test").Page) {
  await page.getByRole("button", { name: "Explore" }).click({ timeout: 3000 }).catch(() => {});
}

test("capture themed views", async ({ page }) => {
  fs.mkdirSync(OUT, { recursive: true });
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(`pageerror: ${e.message}`));
  await page.setViewportSize({ width: 1280, height: 820 });
  await installMockBridge(page);

  // --- opening wordmark, frozen on its settled frame ---
  await page.goto("/?splash=logo&whatsnew=0");
  await page.waitForTimeout(1600);
  await page.screenshot({ path: `${OUT}/03-splash.png` });
  await page.screenshot({ path: `${OUT}/03-splash.jpg`, type: "jpeg", quality: 85 });

  // --- returning-user welcome (splash skipped, not yet entered) ---
  // In `npm run dev` the forced What's New overlay suppresses WelcomeBack (App renders it only when
  // !whatsNew), so this is best-effort in dev — skipped (not fatal) if it can't appear.
  try {
    await page.goto("/?splash=off&whatsnew=0");
    await page.getByText(/Good (morning|afternoon|evening)/).waitFor({ timeout: 6000 });
    await page.waitForTimeout(700); // welcome-in settle
    await page.screenshot({ path: `${OUT}/04-welcome.png` });
  } catch (e) {
    errors.push(`welcome (skipped in dev — forced What's New): ${e}`);
  }

  // --- new-user guide (un-onboarded) ---
  await page.goto("/?splash=off&new=1&whatsnew=0");
  await page.waitForTimeout(2500);
  await page.screenshot({ path: `${OUT}/05-guide.png` });
  const guideBody = (await page.locator("body").innerText().catch(() => "")).slice(0, 200);
  errors.push(`guide body: ${guideBody.replace(/\n/g, " | ")}`);

  // --- inner app (splash skipped + entered) ---
  await page.goto("/?splash=off&enter=1&whatsnew=0");
  await dismissWhatsNew(page);
  await page.getByText("Today").first().waitFor({ timeout: 20000 }).catch(() => {});
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

  // Scheduler explainability (Item C): seeded task blocks show a derived "why here" reason line. Tall
  // viewport so the whole day column is visible without scrolling.
  try {
    await page.setViewportSize({ width: 1280, height: 1400 });
    await page.goto("/?splash=off&enter=1&whatsnew=0");
    await dismissWhatsNew(page);
    await page.getByText("Today").first().waitFor({ timeout: 20000 });
    await page.waitForTimeout(500);
    await page.screenshot({ path: `${OUT}/10-why-here.png` });
  } catch (e) {
    errors.push(`why-here: ${e}`);
  }

  // Settings ▸ On-device AI (Item A): the idle-unload control + the "switch to the tuned model" nudge.
  try {
    await page.setViewportSize({ width: 1280, height: 1000 });
    await page.goto("/?splash=off&enter=1&whatsnew=0");
    await dismissWhatsNew(page);
    await page.getByText("Today").first().waitFor({ timeout: 15000 }).catch(() => {});
    await page.getByText("Settings", { exact: true }).first().click({ timeout: 5000 }).catch((e) => errors.push(`settings click: ${e}`));
    // The On-device AI section holds Item A's UI (tuned-model nudge + idle-unload control). It lives
    // below the fold in an inner scroll container — scroll its heading to the top, then capture the
    // viewport so the whole section shows.
    const heading = page.getByRole("heading", { name: "On-device AI" });
    await heading.waitFor({ timeout: 8000 }).catch(() => {});
    await heading.evaluate((el) => el.scrollIntoView({ block: "start" })).catch(() => {});
    await page.waitForTimeout(400);
    await page.screenshot({ path: `${OUT}/11-settings-ai.png` });

    // Subscribed calendars (.ics) section — Stage 2 ingestion.
    const icsHeading = page.getByRole("heading", { name: "Subscribed calendars" });
    await icsHeading.waitFor({ timeout: 6000 }).catch(() => {});
    await icsHeading.evaluate((el) => el.scrollIntoView({ block: "start" })).catch(() => {});
    await page.waitForTimeout(400);
    await page.screenshot({ path: `${OUT}/12-ics-subs.png` });
  } catch (e) {
    errors.push(`settings: ${e}`);
  }

  // Mobile shell.
  try {
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto("/?splash=off&enter=1&whatsnew=0");
    await dismissWhatsNew(page);
    await page.waitForTimeout(900);
    await page.screenshot({ path: `${OUT}/07-mobile.png` });
  } catch (e) {
    errors.push(`mobile: ${e}`);
  }

  fs.writeFileSync(`${OUT}/_diag.txt`, `errors:\n${errors.join("\n") || "(none)"}\n`);
});
