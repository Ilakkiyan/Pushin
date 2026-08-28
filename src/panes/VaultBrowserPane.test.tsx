import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, within, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { Page } from "../lib/ipc";

vi.mock("../lib/ipc", () => ({
  api: {
    listPages: vi.fn().mockResolvedValue([]),
    createPage: vi.fn().mockResolvedValue({ id: 90, title: "Untitled" }),
    createFolder: vi.fn().mockResolvedValue({ id: 91, title: "New folder", isFolder: true }),
    renamePage: vi.fn().mockResolvedValue([]),
    deletePage: vi.fn().mockResolvedValue([]),
    movePage: vi.fn().mockResolvedValue([]),
    dailyNote: vi.fn().mockResolvedValue({ id: 10, title: "2026-06-14", dailyDate: "2026-06-14" }),
  },
}));
// The Markdown importer drags in BlockNote; stub it so this stays a light jsdom test.
vi.mock("../lib/import", () => ({ importMarkdownFolder: vi.fn() }));

import VaultBrowserPane from "./VaultBrowserPane";
import { useStore } from "../state/store";
import { JOURNAL_ID } from "../lib/pageTree";

const mk = (id: number, over: Partial<Page> = {}): Page => ({
  id,
  title: `P${id}`,
  content: "",
  sortOrder: 0,
  archived: false,
  inbox: false,
  createdAt: "",
  updatedAt: "2026-06-14T10:00:00",
  indexed: false,
  ...over,
});

const seed = [
  mk(1, { title: "Work", isFolder: true }),
  mk(2, { title: "Roadmap" }),
  mk(3, { title: "Kickoff", parentId: 1 }),
  mk(10, { title: "2026-06-14", dailyDate: "2026-06-14" }),
];

/** A drag from `srcId` dropped on `target`, with the dataTransfer the pane actually reads. */
function dropOn(target: Element, srcId: number) {
  const dataTransfer = { getData: (k: string) => (k === "text/page" ? String(srcId) : ""), setData: () => {} };
  fireEvent.dragOver(target, { dataTransfer });
  fireEvent.drop(target, { dataTransfer });
}

/** The card/row for an entry, by page id — unambiguous where a title alone is not (the Journal is
 *  not draggable, and a title string also matches the inner label element). */
const entry = (id: number): HTMLElement => {
  const el = document.querySelector(`[data-entry-id="${id}"]`);
  if (!el) throw new Error(`no entry rendered for id ${id}`);
  return el as HTMLElement;
};

beforeEach(() => {
  vi.clearAllMocks();
  useStore.setState({ pages: seed as never, vaultFolderId: null, currentPageId: null, openPageIds: [], view: "files" });
});

describe("VaultBrowserPane", () => {
  it("shows the root's folders and documents, plus the virtual Journal folder", () => {
    render(<VaultBrowserPane />);
    expect(screen.getByText("Work")).toBeInTheDocument();
    expect(screen.getByText("Roadmap")).toBeInTheDocument();
    expect(screen.getByText("Journal")).toBeInTheDocument();
    // A child page belongs to its folder, not the root listing.
    expect(screen.queryByText("Kickoff")).not.toBeInTheDocument();
  });

  it("opening a folder browses into it rather than opening an editor", async () => {
    render(<VaultBrowserPane />);
    await userEvent.click(screen.getByText("Work"));
    expect(useStore.getState().vaultFolderId).toBe(1);
    expect(useStore.getState().view).toBe("files");
    expect(useStore.getState().currentPageId).toBe(null);
  });

  it("opening a document hands off to the editor and tracks it as open", async () => {
    render(<VaultBrowserPane />);
    await userEvent.click(screen.getByText("Roadmap"));
    const s = useStore.getState();
    expect(s.currentPageId).toBe(2);
    expect(s.view).toBe("vault");
    expect(s.openPageIds).toEqual([2]);
  });

  it("the Journal folder holds the daily notes", async () => {
    render(<VaultBrowserPane />);
    await userEvent.click(screen.getByText("Journal"));
    expect(useStore.getState().vaultFolderId).toBe(JOURNAL_ID);
  });

  it("shows the Journal folder even before the first daily note exists", () => {
    useStore.setState({ pages: [mk(2, { title: "Roadmap" })] as never });
    render(<VaultBrowserPane />);
    // A folder that only appears once you have already used the thing it holds is undiscoverable.
    expect(screen.getByText("Journal")).toBeInTheDocument();
  });

  it("offers today's note rather than a blank page inside the Journal", async () => {
    useStore.setState({ vaultFolderId: JOURNAL_ID });
    render(<VaultBrowserPane />);
    expect(screen.queryByTitle("New page")).not.toBeInTheDocument();
    await userEvent.click(screen.getByTitle("Open today's note"));
    const { api } = await import("../lib/ipc");
    expect(api.dailyNote).toHaveBeenCalled();
  });

  it("creates a folder inside the folder you are standing in", async () => {
    useStore.setState({ vaultFolderId: 1 });
    render(<VaultBrowserPane />);
    await userEvent.click(screen.getByTitle("New folder"));
    const { api } = await import("../lib/ipc");
    expect(api.createFolder).toHaveBeenCalledWith("New folder", 1);
  });

  it("creates a page inside the folder you are standing in", async () => {
    useStore.setState({ vaultFolderId: 1 });
    render(<VaultBrowserPane />);
    await userEvent.click(screen.getByTitle("New page"));
    const { api } = await import("../lib/ipc");
    expect(api.createPage).toHaveBeenCalledWith("Untitled", 1);
  });

  it("renames in place without losing focus mid-word", async () => {
    render(<VaultBrowserPane />);
    await userEvent.click(screen.getAllByTitle("Rename")[0]);
    const field = screen.getByDisplayValue("Work");
    // Typing the whole name is the real test: a nested-component rename field would remount on the
    // first keystroke and drop the rest of the word on the floor.
    await userEvent.clear(field);
    await userEvent.type(field, "Archive{Enter}");
    const { api } = await import("../lib/ipc");
    expect(api.renamePage).toHaveBeenCalledWith(1, "Archive");
  });

  it("filters the current folder by name", async () => {
    render(<VaultBrowserPane />);
    await userEvent.type(screen.getByPlaceholderText("Filter this folder"), "road");
    expect(screen.getByText("Roadmap")).toBeInTheDocument();
    expect(screen.queryByText("Work")).not.toBeInTheDocument();
  });

  it("breadcrumbs back to the vault root from inside a folder", async () => {
    useStore.setState({ vaultFolderId: 1 });
    render(<VaultBrowserPane />);
    expect(screen.getByText("Kickoff")).toBeInTheDocument(); // we're inside Work
    await userEvent.click(screen.getByText("Vault"));
    expect(useStore.getState().vaultFolderId).toBe(null);
  });

  it("shows the folder you are inside in the breadcrumb", () => {
    useStore.setState({ vaultFolderId: 1 });
    render(<VaultBrowserPane />);
    // Both the trail root and the current folder: where you are, and the way back.
    expect(screen.getByText("Vault")).toBeInTheDocument();
    expect(screen.getByText("Work")).toBeInTheDocument();
  });

  it("counts what a folder holds, singular and plural", () => {
    useStore.setState({
      pages: [
        mk(1, { title: "Work", isFolder: true }),
        mk(3, { title: "Kickoff", parentId: 1 }),
        mk(4, { title: "Empty", isFolder: true }),
      ] as never,
    });
    render(<VaultBrowserPane />);
    // Scoped per entry: the always-present Journal is also empty, so a bare "0 items" is ambiguous.
    expect(within(entry(1)).getByText("1 item")).toBeInTheDocument();
    expect(within(entry(4)).getByText("0 items")).toBeInTheDocument();
  });
});

describe("VaultBrowserPane - filing things by drag", () => {
  it("dropping a page on a folder files it there", async () => {
    render(<VaultBrowserPane />);
    dropOn(entry(1), 2); // drag Roadmap onto Work
    const { api } = await import("../lib/ipc");
    expect(api.movePage).toHaveBeenCalledWith(2, 1, 0);
  });

  it("refuses to file a folder into its own descendant", async () => {
    // Work > Sub. Dropping Work onto Sub would cut the whole subtree off from the root.
    useStore.setState({
      pages: [mk(1, { title: "Work", isFolder: true }), mk(5, { title: "Sub", isFolder: true, parentId: 1 })] as never,
      vaultFolderId: 1,
    });
    render(<VaultBrowserPane />);
    dropOn(entry(5), 1);
    const { api } = await import("../lib/ipc");
    expect(api.movePage).not.toHaveBeenCalled();
  });

  it("refuses to drop a folder onto itself", async () => {
    render(<VaultBrowserPane />);
    dropOn(entry(1), 1);
    const { api } = await import("../lib/ipc");
    expect(api.movePage).not.toHaveBeenCalled();
  });

  it("does not file anything into a document", async () => {
    render(<VaultBrowserPane />);
    dropOn(entry(2), 1); // Roadmap is a page, not a container
    const { api } = await import("../lib/ipc");
    expect(api.movePage).not.toHaveBeenCalled();
  });

  it("does not file anything into the virtual Journal", async () => {
    // The Journal is not a row, so it cannot be anyone's parent.
    render(<VaultBrowserPane />);
    dropOn(entry(JOURNAL_ID), 2);
    const { api } = await import("../lib/ipc");
    expect(api.movePage).not.toHaveBeenCalled();
  });
});

describe("VaultBrowserPane - renaming", () => {
  it("Escape abandons the rename without saving", async () => {
    render(<VaultBrowserPane />);
    await userEvent.click(screen.getAllByTitle("Rename")[0]);
    const field = screen.getByLabelText("Rename");
    await userEvent.clear(field);
    await userEvent.type(field, "Archive{Escape}");
    const { api } = await import("../lib/ipc");
    expect(api.renamePage).not.toHaveBeenCalled();
  });

  it("an empty name is refused rather than blanking the title", async () => {
    render(<VaultBrowserPane />);
    await userEvent.click(screen.getAllByTitle("Rename")[0]);
    const field = screen.getByLabelText("Rename");
    await userEvent.clear(field);
    await userEvent.type(field, "{Enter}");
    const { api } = await import("../lib/ipc");
    expect(api.renamePage).not.toHaveBeenCalled();
  });

  it("committing the unchanged name is a no-op, not a write", async () => {
    render(<VaultBrowserPane />);
    await userEvent.click(screen.getAllByTitle("Rename")[0]);
    await userEvent.type(screen.getByLabelText("Rename"), "{Enter}");
    const { api } = await import("../lib/ipc");
    expect(api.renamePage).not.toHaveBeenCalled();
  });

  it("a new folder opens straight into its rename field", async () => {
    // createFolder re-reads the tree, so the refreshed tree has to contain the new folder for it
    // to be on screen at all.
    const { api } = await import("../lib/ipc");
    (api.listPages as ReturnType<typeof vi.fn>).mockResolvedValueOnce([
      ...seed,
      mk(91, { title: "New folder", isFolder: true }),
    ]);
    render(<VaultBrowserPane />);
    await userEvent.click(screen.getByTitle("New folder"));
    // Naming it is the next thing you want to do; it should not need a second click.
    expect(await screen.findByDisplayValue("New folder")).toBeInTheDocument();
  });
});

describe("VaultBrowserPane - deleting", () => {
  it("deletes once confirmed", async () => {
    vi.spyOn(window, "confirm").mockReturnValue(true);
    render(<VaultBrowserPane />);
    await userEvent.click(within(entry(2)).getByTitle("Delete"));
    const { api } = await import("../lib/ipc");
    expect(api.deletePage).toHaveBeenCalledWith(2);
  });

  it("does nothing when the confirmation is declined", async () => {
    vi.spyOn(window, "confirm").mockReturnValue(false);
    render(<VaultBrowserPane />);
    await userEvent.click(within(entry(2)).getByTitle("Delete"));
    const { api } = await import("../lib/ipc");
    expect(api.deletePage).not.toHaveBeenCalled();
  });

  it("warns that a folder's contents survive it", async () => {
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(false);
    render(<VaultBrowserPane />);
    await userEvent.click(within(entry(1)).getByTitle("Delete"));
    // Deleting a folder reparents its children; saying so is what stops it reading as destructive.
    expect(String(confirmSpy.mock.calls[0][0])).toMatch(/1 item[\s\S]*top level/);
  });

  it("the Journal offers neither rename nor delete", () => {
    render(<VaultBrowserPane />);
    const journal = entry(JOURNAL_ID);
    expect(within(journal).queryByTitle("Delete")).not.toBeInTheDocument();
    expect(within(journal).queryByTitle("Rename")).not.toBeInTheDocument();
  });
});

describe("VaultBrowserPane - layout and empty states", () => {
  it("switches to a list with a Modified column and back to a grid", async () => {
    render(<VaultBrowserPane />);
    await userEvent.click(screen.getByTitle("List view"));
    expect(screen.getByText("Modified")).toBeInTheDocument();
    expect(screen.getByText("Name")).toBeInTheDocument();
    await userEvent.click(screen.getByTitle("Grid view"));
    expect(screen.queryByText("Modified")).not.toBeInTheDocument();
  });

  it("lists the same entries in either layout", async () => {
    render(<VaultBrowserPane />);
    await userEvent.click(screen.getByTitle("List view"));
    for (const name of ["Work", "Roadmap", "Journal"]) {
      expect(screen.getByText(name)).toBeInTheDocument();
    }
  });

  it("says so when a folder is empty", () => {
    useStore.setState({ pages: [mk(1, { title: "Work", isFolder: true })] as never, vaultFolderId: 1 });
    render(<VaultBrowserPane />);
    expect(screen.getByText(/This folder is empty/)).toBeInTheDocument();
  });

  it("nudges a first-time user without hiding the Journal", () => {
    useStore.setState({ pages: [] as never, vaultFolderId: null });
    render(<VaultBrowserPane />);
    // The root is never an empty grid, so the nudge sits beside the listing rather than replacing it.
    expect(screen.getByText("Journal")).toBeInTheDocument();
    expect(screen.getByText(/Your vault is empty/)).toBeInTheDocument();
  });

  it("drops the nudge once the vault holds something of your own", () => {
    render(<VaultBrowserPane />);
    expect(screen.queryByText(/Your vault is empty/)).not.toBeInTheDocument();
  });

  it("reports a filter that matches nothing", async () => {
    render(<VaultBrowserPane />);
    await userEvent.type(screen.getByPlaceholderText("Filter this folder"), "zzz");
    expect(screen.getByText(/Nothing here matches/)).toBeInTheDocument();
    // ...and does not claim the folder is empty, because it is not.
    expect(screen.queryByText(/This folder is empty/)).not.toBeInTheDocument();
  });

  it("clears the filter from the field's own button", async () => {
    render(<VaultBrowserPane />);
    await userEvent.type(screen.getByPlaceholderText("Filter this folder"), "road");
    expect(screen.queryByText("Work")).not.toBeInTheDocument();
    await userEvent.click(screen.getByTitle("Clear filter"));
    expect(screen.getByText("Work")).toBeInTheDocument();
  });

  it("lists the daily notes inside the Journal, newest first", () => {
    useStore.setState({
      pages: [
        mk(10, { title: "2026-06-12", dailyDate: "2026-06-12" }),
        mk(11, { title: "2026-06-14", dailyDate: "2026-06-14" }),
      ] as never,
      vaultFolderId: JOURNAL_ID,
    });
    render(<VaultBrowserPane />);
    const names = screen.getAllByTitle(/^2026-06-1[24]$/).map((el) => el.textContent);
    expect(names).toEqual(["2026-06-14", "2026-06-12"]);
  });
});
