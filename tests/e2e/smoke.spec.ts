import { test, expect } from "@playwright/test";
import { installMockBridge } from "./_mockBridge";

test.beforeEach(async ({ page }) => {
  await installMockBridge(page);
  await page.goto("/");
  // App boots past the loading screen once load_all resolves.
  await expect(page.getByText("Calendar")).toBeVisible();
  // The dev build force-shows the post-update "What's New" intro (a full-screen z-60 overlay that
  // appears async after boot and intercepts clicks). Dismiss it if it shows so it can't flake slower
  // flows; if it never appears (prod build), the short wait just no-ops.
  await page
    .getByRole("button", { name: /Explore/ })
    .click({ timeout: 4000 })
    .catch(() => {});
});

// Scope nav clicks to the sidebar <nav> so a matching label in pane content can't clash (strict mode).
const nav = (page: import("@playwright/test").Page) => page.locator("nav");

test("boots and navigates the sidebar across views", async ({ page }) => {
  for (const label of ["Projects", "Habits", "Booking", "Graph", "Inbox", "Notes"]) {
    await nav(page).getByRole("button", { name: label, exact: true }).click();
  }
  // Landing on Notes with an empty vault shows the empty state.
  await expect(page.getByText(/vault is empty|Select a page/i)).toBeVisible();
});

test("creates a vault page from the sidebar", async ({ page }) => {
  await nav(page).getByRole("button", { name: "Notes", exact: true }).click();
  // "New page" exists as both a sidebar-tree icon button and the empty-state button — either creates a page.
  await page.getByRole("button", { name: "New page" }).first().click();
  // The editor opens with an editable title; type one and it shows up.
  const title = page.getByPlaceholder("Untitled");
  await expect(title).toBeVisible();
  await title.fill("My first note");
  await expect(title).toHaveValue("My first note");
});

test("quick-capture lands in the Inbox", async ({ page }) => {
  await page.keyboard.press("Control+Shift+KeyN");
  await expect(page.getByText("Quick capture")).toBeVisible();
  await page.getByPlaceholder(/Capture a thought/).fill("remember the milk");
  await page.keyboard.press("Control+Enter");
  // The Inbox sidebar item now badges a count, so its accessible name becomes "Inbox 1" — match the prefix.
  await nav(page).getByRole("button", { name: /^Inbox/ }).click();
  await expect(page.getByText("remember the milk")).toBeVisible();
});

test("command palette opens with Cmd/Ctrl-K and can ask the vault", async ({ page }) => {
  await page.keyboard.press("Control+KeyK");
  const input = page.getByPlaceholder(/Search, run a command/);
  await expect(input).toBeVisible();
  await input.fill("what did I note");
  await page.getByText(/Ask your vault:/).click();
  await expect(page.getByText("(mock answer)")).toBeVisible();
});
