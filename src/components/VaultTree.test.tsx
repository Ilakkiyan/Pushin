import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { Page } from "../lib/ipc";

vi.mock("../lib/ipc", () => ({
  api: {
    createPage: vi.fn().mockResolvedValue({ id: 9 }),
    createFolder: vi.fn().mockResolvedValue({ id: 8, title: "New folder", isFolder: true }),
    listPages: vi.fn().mockResolvedValue([]),
  },
}));
// Stub the BlockNote-backed importer so this stays a light jsdom test.
vi.mock("../lib/import", () => ({ importMarkdownFolder: vi.fn() }));

import VaultTree, { isAncestor } from "./VaultTree";
import { useStore } from "../state/store";

const mk = (id: number, over: Partial<Page> = {}): Page => ({
  id,
  title: `P${id}`,
  content: "",
  sortOrder: 0,
  archived: false,
  inbox: false,
  createdAt: "",
  updatedAt: "",
  indexed: false,
  ...over,
});

describe("isAncestor (drag-reparent cycle guard)", () => {
  // tree: 1 → 2 → 3 (root → child → grandchild)
  const pages = [mk(1), mk(2, { parentId: 1 }), mk(3, { parentId: 2 })];
  it("detects an ancestor up the chain", () => {
    expect(isAncestor(pages, 1, 3)).toBe(true); // 1 is grandparent of 3
    expect(isAncestor(pages, 2, 3)).toBe(true);
  });
  it("is false for descendants / unrelated / self", () => {
    expect(isAncestor(pages, 3, 1)).toBe(false); // 3 is below 1, not above
    expect(isAncestor(pages, 1, 1)).toBe(false); // root has no parent
    expect(isAncestor(pages, 99, 3)).toBe(false);
  });
});

describe("VaultTree", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useStore.setState({
      pages: [
        mk(1, { title: "Roadmap" }),
        mk(2, { title: "Work", isFolder: true }),
        mk(3, { title: "Kickoff", parentId: 2 }),
        mk(50, { dailyDate: "2026-06-14", title: "2026-06-14" }),
      ] as never,
      currentPageId: null,
      openPageIds: [],
      vaultFolderId: null,
    });
  });

  it("renders manual pages in the tree and daily notes under Journal", () => {
    render(<VaultTree />);
    expect(screen.getByText("Roadmap")).toBeInTheDocument();
    expect(screen.getByText("Journal")).toBeInTheDocument();
    // Daily page is in the Journal list (formatted), not the manual Pages tree.
    expect(screen.getByText("Jun 14")).toBeInTheDocument();
  });

  it("the New page button creates a page", async () => {
    render(<VaultTree />);
    await userEvent.click(screen.getByTitle("New page"));
    const { api } = await import("../lib/ipc");
    expect(api.createPage).toHaveBeenCalled();
  });

  it("the New folder button creates a folder at the top level", async () => {
    render(<VaultTree />);
    await userEvent.click(screen.getByTitle("New folder"));
    const { api } = await import("../lib/ipc");
    expect(api.createFolder).toHaveBeenCalledWith("New folder", null);
  });

  it("clicking a folder browses to it instead of opening an editor", async () => {
    render(<VaultTree />);
    await userEvent.click(screen.getByText("Work"));
    expect(useStore.getState().vaultFolderId).toBe(2);
    expect(useStore.getState().currentPageId).toBe(null);
  });

  describe("the Open switcher", () => {
    it("is absent until something is open, then lists it", () => {
      const { rerender } = render(<VaultTree />);
      expect(screen.queryByText("Open")).not.toBeInTheDocument();
      useStore.getState().openPage(1);
      rerender(<VaultTree />);
      expect(screen.getByText("Open")).toBeInTheDocument();
    });

    it("closing the active page falls back to the previous one, not to nothing", async () => {
      useStore.getState().openPage(1);
      useStore.getState().openPage(3);
      render(<VaultTree />);
      // Close the ACTIVE one (Kickoff) — both open pages carry a Close button.
      await userEvent.click(within(screen.getByText("Kickoff").closest("div")!).getByTitle("Close"));
      const s = useStore.getState();
      expect(s.openPageIds).toEqual([1]);
      expect(s.currentPageId).toBe(1);
    });

    it("closing the last open page drops back to the file browser", async () => {
      useStore.getState().openPage(1);
      render(<VaultTree />);
      await userEvent.click(screen.getByTitle("Close"));
      const s = useStore.getState();
      expect(s.openPageIds).toEqual([]);
      expect(s.currentPageId).toBe(null);
      expect(s.view).toBe("files");
    });
  });
});
