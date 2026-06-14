import { test, expect } from "@playwright/test";
import { installMockBridge } from "./_mockBridge";

test.beforeEach(async ({ page }) => {
  await installMockBridge(page);
  await page.goto("/");
  // App boots past the loading screen once load_all resolves.
  await expect(page.getByText("Calendar")).toBeVisible();
});

test("boots and navigates the sidebar across views", async ({ page }) => {
  for (const label of ["Projects", "Habits", "Booking", "Graph", "Inbox", "Notes"]) {
    await page.getByText(label, { exact: true }).click();
  }
  // Landing on Notes with an empty vault shows the empty state.
  await expect(page.getByText(/vault is empty|Select a page/i)).toBeVisible();
});

test("creates a vault page from the sidebar", async ({ page }) => {
  await page.getByText("Notes", { exact: true }).click();
  await page.getByRole("button", { name: "New page" }).click();
  // The editor opens with an editable title; type one and it shows up.
  const title = page.getByPlaceholder("Untitled");
  await expect(title).toBeVisible();
  await title.fill("My first note");
  await expect(title).toHaveValue("My first note");
});

test("quick-capture lands in the Inbox", async ({ page }) => {
  await page.keyboard.press("Control+Shift+KeyN");
  await expect(page.getByText("Quick capture")).toBeVisible();
  await page.getByRole("textbox").fill("remember the milk");
  await page.keyboard.press("Control+Enter");
  // The Inbox sidebar item should now badge a count; open it and see the capture.
  await page.getByText("Inbox", { exact: true }).click();
  await expect(page.getByText("remember the milk")).toBeVisible();
});

test("command palette opens with Cmd/Ctrl-K and can ask the vault", async ({ page }) => {
  await page.keyboard.press("Control+KeyK");
  const input = page.getByPlaceholder(/Search pages/);
  await expect(input).toBeVisible();
  await input.fill("what did I note");
  await page.getByText(/Ask your vault:/).click();
  await expect(page.getByText("(mock answer)")).toBeVisible();
});
