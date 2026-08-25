import { test, expect } from "@playwright/test";
import { installMockBridge } from "./_mockBridge";

// Scope nav clicks to the sidebar <nav> so a matching label in pane content can't clash (strict mode).
const nav = (page: import("@playwright/test").Page) => page.locator("nav");

/** Step into the vault space — notes/journal/inbox/graph/labels live there, not in the planner nav. */
async function enterVault(page: import("@playwright/test").Page) {
  await nav(page).getByRole("button", { name: /^Vault/ }).click();
  await expect(nav(page).getByRole("button", { name: "Back to app", exact: true })).toBeVisible();
}

test.beforeEach(async ({ page }) => {
  await installMockBridge(page);
  await page.goto("/");
  // App boots past the loading screen once load_all resolves. Assert the NAV's Today row specifically:
  // a bare getByText("Calendar") now matches pane copy and the What's New cards too (strict-mode fail).
  await expect(nav(page).getByRole("button", { name: "Today", exact: true })).toBeVisible();
  // The dev build force-shows the post-update "What's New" intro (a full-screen z-60 overlay that
  // appears async after boot and intercepts clicks). Dismiss it if it shows so it can't flake slower
  // flows; if it never appears (prod build), the short wait just no-ops.
  await page
    .getByRole("button", { name: /Explore/ })
    .click({ timeout: 4000 })
    .catch(() => {});
});

test("boots and navigates the sidebar across views", async ({ page }) => {
  // The nav is two SPACES, not one list: the planner holds run-your-day views, and the vault (entered
  // via the Vault row) holds the second brain. Walk both.
  for (const label of ["Calendar", "Projects", "Habits", "Booking", "People", "Today"]) {
    await nav(page).getByRole("button", { name: label, exact: true }).click();
  }
  await enterVault(page);
  for (const label of ["Graph", "Notes"]) {
    await nav(page).getByRole("button", { name: label, exact: true }).click();
  }
  // Landing on Notes with an empty vault shows the empty state.
  await expect(page.getByText(/vault is empty|Select a page/i)).toBeVisible();

  // Back returns to the planner view we left (Today), not a hardcoded default.
  await nav(page).getByRole("button", { name: "Back to app", exact: true }).click();
  await expect(nav(page).getByRole("button", { name: "Calendar", exact: true })).toBeVisible();
});

test("creates a vault page from the sidebar", async ({ page }) => {
  await enterVault(page);
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
  // Inbox lives in the vault space now. Its sidebar item badges a count, so the accessible name becomes
  // "Inbox 1" — match the prefix.
  await enterVault(page);
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
