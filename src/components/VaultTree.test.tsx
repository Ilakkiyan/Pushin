import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { Page } from "../lib/ipc";

vi.mock("../lib/ipc", () => ({
  api: {
    createPage: vi.fn().mockResolvedValue({ id: 9 }),
    createFolder: vi.fn().mockResolvedValue({ id: 8, title: "New folder", isFolder: true }),
    listPages: vi.fn().mockResolvedValue([]),
    deletePage: vi.fn().mockResolvedValue([]),
  },
}));
// Stub the BlockNote-backed importer so this stays a light jsdom test.
vi.mock("../lib/import", () => ({ importMarkdownFolder: vi.fn() }));

import VaultTree, { isAncestor } from "./VaultTree";
import { JOURNAL_ID } from "../lib/pageTree";
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

    it("lists open pages in the order they were opened, most recent last", () => {
      useStore.getState().openPage(3);
      useStore.getState().openPage(1);
      render(<VaultTree />);
      const open = screen.getByText("Open").parentElement!;
      const names = within(open)
        .getAllByText(/Kickoff|Roadmap/)
        .map((el) => el.textContent);
      expect(names).toEqual(["Kickoff", "Roadmap"]);
    });

    it("marks the page you are actually looking at", () => {
      useStore.getState().openPage(1);
      useStore.getState().openPage(3);
      render(<VaultTree />);
      const open = screen.getByText("Open").parentElement!;
      const active = within(open).getByText("Kickoff").closest("div")!;
      expect(active.className).toContain("is-selected");
    });

    it("drops a page that no longer exists rather than rendering a ghost row", () => {
      // Deleted on another device, or by a sync that arrived while it was open.
      useStore.getState().openPage(1);
      useStore.setState({ openPageIds: [1, 999] });
      render(<VaultTree />);
      const open = screen.getByText("Open").parentElement!;
      expect(within(open).getAllByText(/Roadmap|Kickoff|Work/)).toHaveLength(1);
    });

    it("switching from the switcher opens that page without leaving the vault", async () => {
      useStore.getState().openPage(1);
      useStore.getState().openPage(3);
      render(<VaultTree />);
      const open = screen.getByText("Open").parentElement!;
      await userEvent.click(within(open).getByText("Roadmap"));
      expect(useStore.getState().currentPageId).toBe(1);
      expect(useStore.getState().view).toBe("vault");
    });
  });

  describe("folders in the tree", () => {
    it("keeps its twisty while empty, unlike a childless page", () => {
      useStore.setState({
        pages: [mk(1, { title: "Roadmap" }), mk(7, { title: "Empty", isFolder: true })] as never,
      });
      render(<VaultTree />);
      // The twisty is the affordance that says things go in here. A page only earns one once it
      // has children; a folder keeps it from the moment it exists.
      const folderRow = screen.getByText("Empty").closest("div")!;
      const pageRow = screen.getByText("Roadmap").closest("div")!;
      expect(folderRow.querySelector("button")!.className).not.toContain("invisible");
      expect(pageRow.querySelector("button")!.className).toContain("invisible");
    });

    it("expands to reveal what it holds", async () => {
      render(<VaultTree />);
      expect(screen.queryByText("Kickoff")).not.toBeInTheDocument();
      const row = screen.getByText("Work").closest("div")!;
      await userEvent.click(within(row).getAllByRole("button")[0]); // the twisty
      expect(screen.getByText("Kickoff")).toBeInTheDocument();
    });

    it("offers a nested folder from a folder row", async () => {
      render(<VaultTree />);
      const row = screen.getByText("Work").closest("div")!;
      await userEvent.click(within(row).getByTitle("New folder inside"));
      const { api } = await import("../lib/ipc");
      expect(api.createFolder).toHaveBeenCalledWith("New folder", 2);
    });

    it("offers a nested page from a folder row", async () => {
      render(<VaultTree />);
      const row = screen.getByText("Work").closest("div")!;
      await userEvent.click(within(row).getByTitle("New page inside"));
      const { api } = await import("../lib/ipc");
      expect(api.createPage).toHaveBeenCalledWith("Untitled", 2);
    });

    it("only pages offer a sub-page, and only folders offer a sub-folder", () => {
      render(<VaultTree />);
      const pageRow = screen.getByText("Roadmap").closest("div")!;
      expect(within(pageRow).getByTitle("Add sub-page")).toBeInTheDocument();
      expect(within(pageRow).queryByTitle("New folder inside")).not.toBeInTheDocument();
    });

    it("warns that a folder's contents survive deleting it", async () => {
      const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(false);
      render(<VaultTree />);
      const row = screen.getByText("Work").closest("div")!;
      await userEvent.click(within(row).getByTitle("Delete folder"));
      expect(String(confirmSpy.mock.calls[0][0])).toMatch(/move to the top level/);
      const { api } = await import("../lib/ipc");
      expect(api.deletePage).not.toHaveBeenCalled();
    });

    it("deletes a page without any reparenting warning", async () => {
      const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(false);
      render(<VaultTree />);
      const row = screen.getByText("Roadmap").closest("div")!;
      await userEvent.click(within(row).getByTitle("Delete page"));
      expect(String(confirmSpy.mock.calls[0][0])).not.toMatch(/top level/);
    });
  });

  describe("the Journal section", () => {
    it("opens the Journal folder from its header", async () => {
      render(<VaultTree />);
      await userEvent.click(screen.getByTitle("Open the Journal folder"));
      expect(useStore.getState().vaultFolderId).toBe(JOURNAL_ID);
      expect(useStore.getState().view).toBe("files");
    });

    it("keeps daily notes out of the Pages tree", () => {
      render(<VaultTree />);
      // The daily note is listed under Journal by date, never as a page row titled 2026-06-14.
      expect(screen.queryByText("2026-06-14")).not.toBeInTheDocument();
      expect(screen.getByText("Jun 14")).toBeInTheDocument();
    });

    it("is absent entirely when nothing has been journalled", () => {
      useStore.setState({ pages: [mk(1, { title: "Roadmap" })] as never });
      render(<VaultTree />);
      expect(screen.queryByText("Journal")).not.toBeInTheDocument();
    });
  });
});
