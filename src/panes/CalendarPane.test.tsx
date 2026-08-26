import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, fireEvent, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

// The calendar is the one pane where "the scheduler put it at 2pm" and "the user SEES it at 2pm" are
// different claims. `model_battery.rs` proves the first against a live model; these prove the second.
// Layout maths mirrored from CalendarPane: START_HOUR = 0, PX_PER_HOUR = 56, so a block's `top` is
// (minutes-from-midnight / 60) * 56, and its rendered height is (duration / 60) * 56, less a 2px gap.
const PX_PER_HOUR = 56;
const topFor = (h: number, m = 0) => ((h * 60 + m) / 60) * PX_PER_HOUR;
const heightFor = (mins: number) => Math.max(Math.max(20, (mins / 60) * PX_PER_HOUR) - 2, 6);

// Partial mock: `../lib/ipc` also exports pure helpers the pane calls directly (e.g.
// `formatPlacementReason`), so keep the real module and swap only the Tauri-backed `api`.
vi.mock("../lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../lib/ipc")>()),
  api: {
    labelsForEntities: vi.fn().mockResolvedValue({}),
    dailyBriefing: vi.fn().mockResolvedValue(null),
    meetingBrief: vi.fn().mockResolvedValue(null),
    extractActionItems: vi.fn().mockResolvedValue([]),
    labelsFor: vi.fn().mockResolvedValue([]),
    listLabels: vi.fn().mockResolvedValue([]),
  },
}));

import CalendarPane from "./CalendarPane";
import { useStore } from "../state/store";

/** An ISO naive-local stamp (the only format Pushin stores) for today ± `dayOffset`, at h:m. */
function at(h: number, m = 0, dayOffset = 0): string {
  const d = new Date();
  d.setHours(0, 0, 0, 0);
  d.setDate(d.getDate() + dayOffset);
  d.setHours(h, m, 0, 0);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(d.getHours())}:${p(d.getMinutes())}:00`;
}

const settings = {
  googleConnected: false,
  sleepEnabled: false,
  sleepStart: null,
  sleepEnd: null,
  commitments: [],
  timezone: "UTC",
  workStart: "09:00",
  workEnd: "17:00",
};

const task = (id: number, title: string) => ({
  id,
  title,
  status: "todo",
  estimatedMinutes: 60,
  priority: 2,
  projectId: null,
  deadline: null,
  dependsOn: [],
});

const block = (id: number, taskId: number, start: string, end: string, locked = false) => ({
  id,
  taskId,
  start,
  end,
  locked,
  provider: null,
  externalId: null,
  syncState: null,
});

const event = (id: number, title: string, start: string, end: string, kind = "fixed") => ({
  id,
  title,
  start,
  end,
  kind,
  source: "manual",
  createdAt: start,
  provider: null,
  externalId: null,
  accountId: null,
  etag: null,
});

let moveBlock: ReturnType<typeof vi.fn>;
let unlockBlock: ReturnType<typeof vi.fn>;
let moveHabit: ReturnType<typeof vi.fn>;

function seed(overrides: Record<string, unknown> = {}) {
  moveBlock = vi.fn().mockResolvedValue(undefined);
  unlockBlock = vi.fn().mockResolvedValue(undefined);
  moveHabit = vi.fn().mockResolvedValue(undefined);
  useStore.setState({
    settings: settings as never,
    tasks: [],
    projects: [],
    events: [],
    blocks: [],
    conflicts: [],
    blockReasons: {},
    calMode: "week",
    calColorByLabel: false,
    calLabelFilterIds: [],
    focusDateIso: null,
    moveBlock,
    unlockBlock,
    moveHabit,
    ...overrides,
  } as never);
}

/** The draggable card for a task block — the outer positioned div, not the inner title span. */
function blockCard(title: string): HTMLElement {
  const span = screen.getByText(title);
  const card = span.closest("div[style]") as HTMLElement | null;
  if (!card) throw new Error(`no positioned card for "${title}"`);
  return card;
}

/** Drag a card by `dy` pixels and release. Mirrors the real gesture: pointerdown on the card, then
 *  pointermove / pointerup on window (that's where CalendarPane listens). */
function drag(card: HTMLElement, dy: number) {
  fireEvent.pointerDown(card, { clientY: 400 });
  fireEvent.pointerMove(window, { clientY: 400 + dy });
  fireEvent.pointerUp(window, { clientY: 400 + dy });
}

beforeEach(() => {
  vi.clearAllMocks();
  seed();
});

describe("CalendarPane — placement", () => {
  it("draws a task block at its scheduled time, titled with the task", async () => {
    seed({ tasks: [task(1, "Write slides")], blocks: [block(10, 1, at(9), at(10))] });
    render(<CalendarPane />);

    const card = blockCard("Write slides");
    expect(card.style.top).toBe(`${topFor(9)}px`);
    expect(card.style.height).toBe(`${heightFor(60)}px`);
  });

  it("scales block height with duration, so a 2h block is twice a 1h block", () => {
    seed({
      tasks: [task(1, "Short"), task(2, "Long")],
      blocks: [block(10, 1, at(9), at(10)), block(11, 2, at(13), at(15))],
    });
    render(<CalendarPane />);

    expect(blockCard("Short").style.height).toBe(`${heightFor(60)}px`);
    expect(blockCard("Long").style.height).toBe(`${heightFor(120)}px`);
    // And the later block sits lower on the grid.
    expect(parseFloat(blockCard("Long").style.top)).toBeGreaterThan(parseFloat(blockCard("Short").style.top));
  });

  it("draws a fixed event at its own time", () => {
    seed({ events: [event(5, "Dentist", at(14), at(15))] });
    render(<CalendarPane />);

    const card = blockCard("Dentist");
    expect(card.style.top).toBe(`${topFor(14)}px`);
  });

  it("keeps a half-hour block visible rather than collapsing it", () => {
    seed({ tasks: [task(1, "Standup")], blocks: [block(10, 1, at(9), at(9, 30))] });
    render(<CalendarPane />);

    // 30min would be 28px; the floor keeps short blocks readable.
    expect(parseFloat(blockCard("Standup").style.height)).toBeGreaterThanOrEqual(6);
    expect(screen.getByText("Standup")).toBeInTheDocument();
  });

  it("shows the scheduler's 'why here' reason on a roomy block", () => {
    seed({
      tasks: [task(1, "Deep work")],
      blocks: [block(10, 1, at(9), at(11))],
      blockReasons: { 10: { kind: "earliest" } },
    });
    render(<CalendarPane />);

    // The reason is rendered inline on tall blocks AND folded into the hover title.
    expect(blockCard("Deep work").getAttribute("title")).toMatch(/Deep work — /);
  });
});

describe("CalendarPane — pinning", () => {
  it("offers an unpin control on a pinned block and calls unlockBlock", async () => {
    seed({ tasks: [task(1, "Pinned work")], blocks: [block(10, 1, at(9), at(10), true)] });
    render(<CalendarPane />);

    const unpin = screen.getByTitle(/click to unpin/i);
    await userEvent.click(unpin);
    await waitFor(() => expect(unlockBlock).toHaveBeenCalledWith(10, at(9), at(10)));
  });

  it("shows no unpin control on an unpinned block", () => {
    seed({ tasks: [task(1, "Floating work")], blocks: [block(10, 1, at(9), at(10), false)] });
    render(<CalendarPane />);

    expect(screen.getByText("Floating work")).toBeInTheDocument();
    expect(screen.queryByTitle(/click to unpin/i)).toBeNull();
  });
});

describe("CalendarPane — drag to reschedule", () => {
  it("moves a block to the dropped time", async () => {
    seed({ tasks: [task(1, "Write slides")], blocks: [block(10, 1, at(9), at(10))] });
    render(<CalendarPane />);

    drag(blockCard("Write slides"), PX_PER_HOUR * 2); // +2h

    await waitFor(() => expect(moveBlock).toHaveBeenCalledTimes(1));
    expect(moveBlock).toHaveBeenCalledWith(10, at(11), at(12));
  });

  it("does not persist a drag that never moved", async () => {
    seed({ tasks: [task(1, "Write slides")], blocks: [block(10, 1, at(9), at(10))] });
    render(<CalendarPane />);

    drag(blockCard("Write slides"), 0);

    await waitFor(() => expect(moveBlock).not.toHaveBeenCalled());
  });

  it("slides off an occupied slot instead of overlapping an existing event", async () => {
    // Dropping the 9am block onto 14:00, where a meeting already sits, must not overlap it.
    seed({
      tasks: [task(1, "Write slides")],
      blocks: [block(10, 1, at(9), at(10))],
      events: [event(5, "Meeting", at(14), at(15))],
    });
    render(<CalendarPane />);

    drag(blockCard("Write slides"), PX_PER_HOUR * 5); // 9am -> 2pm, straight onto the meeting

    await waitFor(() => expect(moveBlock).toHaveBeenCalledTimes(1));
    const [, newStart, newEnd] = moveBlock.mock.calls[0];
    // Whatever the scheduler picks, the result must not intersect 14:00–15:00.
    const overlaps = newStart < at(15) && newEnd > at(14);
    expect(overlaps).toBe(false);
  });

  it("clamps a drag above midnight to the top of the day", async () => {
    seed({ tasks: [task(1, "Early")], blocks: [block(10, 1, at(1), at(2))] });
    render(<CalendarPane />);

    drag(blockCard("Early"), -PX_PER_HOUR * 6); // would land at -5:00

    await waitFor(() => expect(moveBlock).toHaveBeenCalledTimes(1));
    expect(moveBlock).toHaveBeenCalledWith(10, at(0), at(1));
  });

  it("drags a habit by its own handler, learning the new time", async () => {
    seed({ events: [event(7, "Morning run", at(7), at(7, 30), "habit")] });
    render(<CalendarPane />);

    drag(blockCard("Morning run"), PX_PER_HOUR); // +1h

    await waitFor(() => expect(moveHabit).toHaveBeenCalledTimes(1));
    expect(moveHabit).toHaveBeenCalledWith(7, at(8));
    expect(moveBlock).not.toHaveBeenCalled();
  });
});

describe("CalendarPane — navigation", () => {
  it("steps the visible range back and forward a week, and Today returns", async () => {
    render(<CalendarPane />);
    const label = () => screen.getByText(/–/).textContent ?? "";

    const start = label();
    await userEvent.click(screen.getByTitle("Next"));
    const next = label();
    expect(next).not.toBe(start);

    await userEvent.click(screen.getByTitle("Previous"));
    expect(label()).toBe(start);

    // Go somewhere else, then Today snaps back to the current week.
    await userEvent.click(screen.getByTitle("Next"));
    await userEvent.click(screen.getByText("Today"));
    expect(label()).toBe(start);
  });

  it("renders a single column in day mode instead of the week grid", () => {
    render(<CalendarPane days={1} />);
    // The day view labels one date rather than a start–end range.
    expect(screen.queryByText(/–/)).toBeNull();
  });

  it("switches the store to month mode from the toolbar toggle", async () => {
    render(<CalendarPane />);
    expect(useStore.getState().calMode).toBe("week");

    await userEvent.click(screen.getByText("Month"));
    await waitFor(() => expect(useStore.getState().calMode).toBe("month"));
  });
});

describe("CalendarPane — habits and events", () => {
  it("marks a habit as draggable to set its preferred time", () => {
    seed({ events: [event(7, "Morning run", at(7), at(7, 30), "habit")] });
    render(<CalendarPane />);

    expect(screen.getByTitle(/Morning run — drag to set your preferred time/)).toBeInTheDocument();
  });

  it("renders an all-day event as a spanning bar, not a timed card", () => {
    // An all-day event runs midnight to midnight; it belongs in the bar row above the grid.
    seed({ events: [event(9, "Conference", at(0, 0), at(0, 0, 1))] });
    render(<CalendarPane />);

    const bar = screen.getByText("Conference");
    expect(bar).toBeInTheDocument();
    // It is not positioned on the hour grid.
    expect((bar.closest("div[style]") as HTMLElement | null)?.style.top ?? "").not.toBe(`${topFor(0)}px`);
  });

  it("keeps a block and an event on the same day distinct", () => {
    seed({
      tasks: [task(1, "Write slides")],
      blocks: [block(10, 1, at(9), at(10))],
      events: [event(5, "Dentist", at(14), at(15))],
    });
    render(<CalendarPane />);

    expect(screen.getByText("Write slides")).toBeInTheDocument();
    expect(screen.getByText("Dentist")).toBeInTheDocument();
  });

  it("does not draw blocks from another week", () => {
    seed({ tasks: [task(1, "Next month")], blocks: [block(10, 1, at(9, 0, 30), at(10, 0, 30))] });
    render(<CalendarPane />);

    expect(screen.queryByText("Next month")).toBeNull();
  });
});
