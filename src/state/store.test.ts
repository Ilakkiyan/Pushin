import { describe, it, expect, beforeEach, vi } from "vitest";
import type { Page } from "../lib/ipc";

// Mock the entire IPC surface; the store should never touch a real Tauri command in a unit test.
vi.mock("../lib/ipc", () => {
  const page = (id: number, over: Partial<Page> = {}): Page => ({
    id,
    title: `P${id}`,
    content: "",
    sortOrder: 0,
    archived: false,
    inbox: false,
    createdAt: "2026-01-01T00:00:00",
    updatedAt: "2026-01-01T00:00:00",
    indexed: false,
    ...over,
  });
  return {
    __page: page,
    api: {
      listPages: vi.fn().mockResolvedValue([page(1), page(2)]),
      listInbox: vi.fn().mockResolvedValue([]),
      dailyNote: vi.fn().mockResolvedValue(page(9, { dailyDate: "2026-06-14" })),
      getPage: vi.fn().mockResolvedValue(page(1)),
      createPage: vi.fn().mockResolvedValue(page(5)),
      updatePage: vi.fn().mockResolvedValue(page(1)),
      deletePage: vi.fn().mockResolvedValue([page(2)]),
      movePage: vi.fn().mockResolvedValue([page(1)]),
      entityPages: vi.fn().mockResolvedValue([]),
      linkPageEntity: vi.fn().mockResolvedValue(undefined),
      captureNote: vi.fn().mockResolvedValue(undefined),
      keepInboxNote: vi.fn().mockResolvedValue(undefined),
      vaultAsk: vi.fn().mockResolvedValue({ answer: "ok", citations: [] }),
      hermesAddNote: vi.fn().mockResolvedValue([]),
    },
  };
});

import { useStore } from "./store";
import { api } from "../lib/ipc";

const reset = () =>
  useStore.setState({ view: "calendar", currentPageId: null, pages: [], inbox: [], captureOpen: false });

beforeEach(() => {
  vi.clearAllMocks();
  reset();
});

describe("store navigation + vault actions", () => {
  it("setView / setSidebarCollapsed / setCaptureOpen update flags", () => {
    useStore.getState().setView("graph");
    expect(useStore.getState().view).toBe("graph");
    useStore.getState().setSidebarCollapsed(true);
    expect(useStore.getState().sidebarCollapsed).toBe(true);
    useStore.getState().setCaptureOpen(true);
    expect(useStore.getState().captureOpen).toBe(true);
  });

  it("openPage selects a page and switches to the vault view", () => {
    useStore.getState().openPage(7);
    expect(useStore.getState().currentPageId).toBe(7);
    expect(useStore.getState().view).toBe("vault");
  });

  it("openDaily creates/opens the day's note and refreshes the tree", async () => {
    await useStore.getState().openDaily("2026-06-14");
    expect(api.dailyNote).toHaveBeenCalledWith("2026-06-14");
    expect(useStore.getState().currentPageId).toBe(9);
    expect(useStore.getState().view).toBe("vault");
    expect(useStore.getState().pages).toHaveLength(2); // refreshed via listPages
  });

  it("createPage opens the new page", async () => {
    await useStore.getState().createPage(null);
    expect(api.createPage).toHaveBeenCalled();
    expect(useStore.getState().currentPageId).toBe(5);
    expect(useStore.getState().view).toBe("vault");
  });

  it("openEntityNote creates + links a page when none exists yet", async () => {
    await useStore.getState().openEntityNote("task", 42, "Write slides");
    expect(api.entityPages).toHaveBeenCalledWith("task", 42);
    expect(api.createPage).toHaveBeenCalledWith("Write slides", null);
    expect(api.linkPageEntity).toHaveBeenCalledWith(5, "task", 42);
    expect(useStore.getState().currentPageId).toBe(5);
  });

  it("openEntityNote reuses the existing linked page (no create)", async () => {
    (api.entityPages as ReturnType<typeof vi.fn>).mockResolvedValueOnce([{ id: 3, title: "Notes" }]);
    await useStore.getState().openEntityNote("event", 8, "Meeting");
    expect(api.createPage).not.toHaveBeenCalled();
    expect(api.linkPageEntity).not.toHaveBeenCalled();
    expect(useStore.getState().currentPageId).toBe(3);
  });

  it("savePage persists then refreshes the tree", async () => {
    await useStore.getState().savePage(1, "Title", null, "body", "[]", ["Other"]);
    expect(api.updatePage).toHaveBeenCalledWith(1, "Title", null, "body", "[]", ["Other"]);
    expect(api.listPages).toHaveBeenCalled();
  });

  it("deletePage clears currentPageId only when the open page is deleted", async () => {
    useStore.setState({ currentPageId: 1 });
    await useStore.getState().deletePage(1);
    expect(useStore.getState().currentPageId).toBeNull();

    useStore.setState({ currentPageId: 2 });
    await useStore.getState().deletePage(99);
    expect(useStore.getState().currentPageId).toBe(2);
  });
});

describe("store inbox actions", () => {
  it("captureNote saves then refreshes the inbox", async () => {
    (api.listInbox as ReturnType<typeof vi.fn>).mockResolvedValueOnce([{ id: 1, content: "x" }]);
    await useStore.getState().captureNote("a thought");
    expect(api.captureNote).toHaveBeenCalledWith("a thought");
    expect(useStore.getState().inbox).toHaveLength(1);
  });

  it("keepInboxNote graduates a capture and refreshes inbox + pages", async () => {
    await useStore.getState().keepInboxNote(4);
    expect(api.keepInboxNote).toHaveBeenCalledWith(4);
    expect(api.listInbox).toHaveBeenCalled();
    expect(api.listPages).toHaveBeenCalled();
  });
});
