import { test, expect, type Page } from "@playwright/test";
import { installMockBridge } from "./_mockBridge";

// Scheduling in the REAL rendered app. `model_battery.rs` proves the scheduler picks the right time
// and CalendarPane.test.tsx proves the pane draws it there in jsdom; this proves the whole thing
// holds up in a browser — the block is on screen, a drag moves it, and the move survives the round
// trip through the IPC layer and back into a re-render.
//
// The mock bridge seeds a full day on `today`: 5 task blocks, a "Team meeting" event, and the
// scheduler's "why here" reasons. See _mockBridge.ts.

const nav = (page: Page) => page.locator("nav");

/** The positioned card for a titled block/event on the week grid. */
const card = (page: Page, title: string) =>
  page.locator("div[style*='top']").filter({ hasText: title }).first();

async function gotoCalendar(page: Page) {
  await installMockBridge(page);
  await page.goto("/");
  await expect(nav(page).getByRole("button", { name: "Today", exact: true })).toBeVisible();
  await page
    .getByRole("button", { name: /Explore/ })
    .click({ timeout: 4000 })
    .catch(() => {});
  await nav(page).getByRole("button", { name: "Calendar", exact: true }).click();
}

/** Read a card's pixel offset from the top of the hour grid. */
async function topOf(page: Page, title: string): Promise<number> {
  const box = await card(page, title).evaluate((el) => (el as HTMLElement).style.top);
  return parseFloat(box);
}

test.beforeEach(async ({ page }) => {
  await gotoCalendar(page);
});

test("shows the scheduled day: task blocks, a fixed event, and why each block is there", async ({ page }) => {
  // Every seeded task block is on screen.
  for (const title of ["Draft outline", "Write thesis", "Revise draft", "Prep slides"]) {
    await expect(card(page, title)).toBeVisible();
  }
  await expect(card(page, "Team meeting")).toBeVisible();

  // The scheduler's explanation rides along on the block (inline and/or as the hover title).
  await expect(card(page, "Revise draft")).toHaveAttribute("title", /Draft outline/);
});

test("orders blocks down the day by their start time", async ({ page }) => {
  const outline = await topOf(page, "Draft outline"); // 08:00
  const thesis = await topOf(page, "Write thesis"); // 10:00
  const revise = await topOf(page, "Revise draft"); // 13:00
  expect(outline).toBeLessThan(thesis);
  expect(thesis).toBeLessThan(revise);
});

test("drag-to-reschedule moves a block and the new time survives the round trip", async ({ page }) => {
  const target = card(page, "Draft outline");
  const before = await topOf(page, "Draft outline");

  const box = await target.boundingBox();
  expect(box).not.toBeNull();

  // Drag it down roughly two hours (56px per hour) using real pointer events.
  await page.mouse.move(box!.x + box!.width / 2, box!.y + 5);
  await page.mouse.down();
  await page.mouse.move(box!.x + box!.width / 2, box!.y + 5 + 112, { steps: 10 });
  await page.mouse.up();

  // The move round-trips through lock_block → load_all → re-render, so the card settles LOWER than
  // it started. (Exact placement is asserted in the unit tests; here we prove the trip happens.)
  await expect
    .poll(async () => topOf(page, "Draft outline"), { timeout: 5000 })
    .toBeGreaterThan(before);
});

test("dragging one chunk of a split task merges the whole task back into a single block", async ({ page }) => {
  // "Write thesis" is seeded as two chunks (10:00 and 15:00) — the split-around-a-meeting shape that
  // showed up as two identically-named events you could never reunite. Dragging either one is an
  // instruction about the TASK, so it re-lays as one block wherever there is room for all 3 hours.
  const cards = page.locator("div[style*='top']").filter({ hasText: "Write thesis" });
  await expect(cards).toHaveCount(2);

  // The 15:00 chunk sits ~1050px down a 24-hour grid, well below a 720px viewport — read its box
  // without scrolling first and `page.mouse.move` targets a point off-screen, so the drag silently
  // never starts and the test "fails" for reasons that have nothing to do with merging.
  const second = cards.nth(1);
  await second.scrollIntoViewIfNeeded();
  const box = await second.boundingBox();
  expect(box).not.toBeNull();
  await page.mouse.move(box!.x + box!.width / 2, box!.y + 5);
  await page.mouse.down();
  await page.mouse.move(box!.x + box!.width / 2, box!.y + 5 + 56 * 4, { steps: 10 }); // ~4h down, into the open evening
  await page.mouse.up();

  await expect(cards).toHaveCount(1, { timeout: 5000 });

  // ...and the merged block carries the task's whole estimate (3h at 56px/hour), not just the half
  // that was dragged.
  const height = await cards.first().evaluate((el) => parseFloat((el as HTMLElement).style.height));
  expect(height).toBeGreaterThan(56 * 2.5);
});

test("a block dropped on the team meeting does not end up overlapping it", async ({ page }) => {
  // "Prep slides" sits at 16:45, right after the 15:00–16:45 meeting. Drag it up onto the meeting.
  const target = card(page, "Prep slides");
  const box = await target.boundingBox();
  expect(box).not.toBeNull();

  await page.mouse.move(box!.x + box!.width / 2, box!.y + 5);
  await page.mouse.down();
  await page.mouse.move(box!.x + box!.width / 2, box!.y + 5 - 84, { steps: 10 }); // ~1.5h up, into the meeting
  await page.mouse.up();

  // The calendar slides it to the nearest free time rather than stacking it on the meeting.
  const meetingTop = await topOf(page, "Team meeting");
  const meetingHeight = await card(page, "Team meeting").evaluate((el) => parseFloat((el as HTMLElement).style.height));
  const slidesTop = await topOf(page, "Prep slides");
  const slidesHeight = await card(page, "Prep slides").evaluate((el) => parseFloat((el as HTMLElement).style.height));

  const overlaps = slidesTop < meetingTop + meetingHeight && slidesTop + slidesHeight > meetingTop;
  expect(overlaps).toBe(false);
});

test("the what's-new intro renders its cards rather than an empty shell", async ({ page }) => {
  // The dev server forces the intro on every launch so it can be iterated on, and the other specs
  // dismiss it via the Explore button. Version-aware filtering nearly turned that preview into a
  // heading over empty space: the forced path has to show the FULL list, not the (empty) diff
  // between the running version and itself.
  await page.goto("/?whatsnew=1");
  const explore = page.getByRole("button", { name: /Explore/ });
  await expect(explore).toBeVisible({ timeout: 10_000 });
  await expect(page.getByText(/One task, however it's split/)).toBeVisible();
  await expect(page.getByText(/Opens on your day/)).toBeVisible();
  await explore.click();
  await expect(explore).toHaveCount(0);
});

test("the done bin is collapsed by default and expands on click", async ({ page }) => {
  // "Ship the release" is seeded as finished with NO block — ticking a task off takes it off the
  // calendar, and the done bin in the task list is where it lives instead. The bin only grows, so
  // it must not bury the active list.
  await expect(page.getByText("Draft outline").first()).toBeVisible();
  await expect(page.getByText("Ship the release")).toHaveCount(0);

  const bin = page.getByRole("button", { name: /^Done/ });
  await expect(bin).toBeVisible();
  await expect(bin).toHaveAttribute("aria-expanded", "false");

  await bin.click();
  await expect(page.getByText("Ship the release")).toBeVisible();
  await expect(bin).toHaveAttribute("aria-expanded", "true");

  await bin.click();
  await expect(page.getByText("Ship the release")).toHaveCount(0);
});

test("navigates weeks and returns with Today", async ({ page }) => {
  await expect(card(page, "Draft outline")).toBeVisible();

  await page.getByTitle("Next", { exact: true }).click();
  await expect(card(page, "Draft outline")).toHaveCount(0); // next week is empty

  await page.getByRole("button", { name: "Today", exact: true }).last().click();
  await expect(card(page, "Draft outline")).toBeVisible();
});

test("switches to the month view and back", async ({ page }) => {
  await page.getByRole("button", { name: "Month" }).click();
  // The month grid replaces the hour grid; the week toolbar's hour rail is gone.
  await expect(page.getByRole("button", { name: "Week" })).toBeVisible();

  await page.getByRole("button", { name: "Week" }).click();
  await expect(card(page, "Draft outline")).toBeVisible();
});
