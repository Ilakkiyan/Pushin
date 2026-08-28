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
      createFolder: vi.fn().mockResolvedValue(page(6, { title: "Work", isFolder: true })),
      renamePage: vi.fn().mockResolvedValue([page(1, { title: "Renamed" })]),
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
  useStore.setState({
    view: "calendar",
    space: "planner",
    currentPageId: null,
    pages: [],
    inbox: [],
    captureOpen: false,
    openPageIds: [],
    vaultFolderId: null,
  });

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

// The "Open" switcher is a working set, so its invariants are about ORDER and about never
// outliving the pages it names. Every route into the editor funnels through one helper
// (`openState`) precisely so these hold no matter which entry point was used.
describe("store — the open-pages switcher", () => {
  it("tracks each opened page, most-recently-opened last", () => {
    const s = useStore.getState();
    s.openPage(1);
    s.openPage(2);
    expect(useStore.getState().openPageIds).toEqual([1, 2]);
  });

  it("re-opening a page moves it to the end instead of duplicating it", () => {
    const s = useStore.getState();
    s.openPage(1);
    s.openPage(2);
    s.openPage(1);
    expect(useStore.getState().openPageIds).toEqual([2, 1]);
  });

  it("registers pages opened by every other route, not just openPage", async () => {
    // A new entry point that forgets to register leaves the switcher lying about what is open.
    await useStore.getState().createPage(null);
    await useStore.getState().openDaily("2026-06-14");
    await useStore.getState().openEntityNote("task", 42, "Write slides");
    // 5 from createPage, 9 from the daily note, then openEntityNote reuses 5 — which moves it back
    // to the end rather than duplicating it, across two different entry points.
    expect(useStore.getState().openPageIds).toEqual([9, 5]);
    expect(useStore.getState().currentPageId).toBe(5);
  });

  it("closing a background page leaves the active one alone", () => {
    const s = useStore.getState();
    s.openPage(1);
    s.openPage(2);
    s.closePage(1);
    const after = useStore.getState();
    expect(after.openPageIds).toEqual([2]);
    expect(after.currentPageId).toBe(2);
    expect(after.view).toBe("vault");
  });

  it("closing the active page falls back to the previous one", () => {
    const s = useStore.getState();
    s.openPage(1);
    s.openPage(2);
    s.closePage(2);
    const after = useStore.getState();
    expect(after.openPageIds).toEqual([1]);
    expect(after.currentPageId).toBe(1);
    expect(after.view).toBe("vault"); // still in the editor, on the neighbour
  });

  it("closing the last open page returns to the browser rather than an empty editor", () => {
    const s = useStore.getState();
    s.openPage(1);
    s.closePage(1);
    const after = useStore.getState();
    expect(after.openPageIds).toEqual([]);
    expect(after.currentPageId).toBeNull();
    expect(after.view).toBe("files");
  });

  it("closing a page that was never open changes nothing", () => {
    useStore.getState().openPage(1);
    useStore.getState().closePage(99);
    expect(useStore.getState().openPageIds).toEqual([1]);
    expect(useStore.getState().currentPageId).toBe(1);
  });

  it("deleting a page drops it from the switcher", async () => {
    const s = useStore.getState();
    s.openPage(1);
    s.openPage(2);
    await useStore.getState().deletePage(1);
    expect(useStore.getState().openPageIds).toEqual([2]);
  });
});

describe("store — the file browser", () => {
  it("openFolder points the browser at a folder and enters the vault space", () => {
    useStore.getState().openFolder(6);
    const s = useStore.getState();
    expect(s.vaultFolderId).toBe(6);
    expect(s.view).toBe("files");
    expect(s.space).toBe("vault");
  });

  it("openFolder(null) is the vault root", () => {
    useStore.setState({ vaultFolderId: 6 });
    useStore.getState().openFolder(null);
    expect(useStore.getState().vaultFolderId).toBeNull();
  });

  it("creating a folder refreshes the tree WITHOUT opening an editor", async () => {
    // A folder is a container, not a document — creating one must leave you where you are.
    useStore.setState({ view: "files", vaultFolderId: null });
    const folder = await useStore.getState().createFolder("Work", null);
    expect(api.createFolder).toHaveBeenCalledWith("Work", null);
    expect(folder.isFolder).toBe(true);
    const s = useStore.getState();
    expect(s.currentPageId).toBeNull();
    expect(s.view).toBe("files");
    expect(s.openPageIds).toEqual([]);
    expect(api.listPages).toHaveBeenCalled();
  });

  it("createFolder defaults to the vault root when no parent is given", async () => {
    await useStore.getState().createFolder("Work");
    expect(api.createFolder).toHaveBeenCalledWith("Work", null);
  });

  it("renamePage stores the refreshed tree the backend returns", async () => {
    await useStore.getState().renamePage(1, "Renamed");
    expect(api.renamePage).toHaveBeenCalledWith(1, "Renamed");
    expect(useStore.getState().pages[0].title).toBe("Renamed");
  });

  it("deleting the folder you are standing in returns you to the root", async () => {
    useStore.setState({ vaultFolderId: 6, view: "files" });
    await useStore.getState().deletePage(6);
    expect(useStore.getState().vaultFolderId).toBeNull();
  });

  it("deleting some other folder leaves your location alone", async () => {
    useStore.setState({ vaultFolderId: 6 });
    await useStore.getState().deletePage(99);
    expect(useStore.getState().vaultFolderId).toBe(6);
  });

  it("the files view belongs to the vault space, and Back still restores the planner", () => {
    useStore.getState().setView("projects");
    useStore.getState().setView("files");
    expect(useStore.getState().space).toBe("vault");
    useStore.getState().exitVault();
    const s = useStore.getState();
    expect(s.space).toBe("planner");
    expect(s.view).toBe("projects");
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
