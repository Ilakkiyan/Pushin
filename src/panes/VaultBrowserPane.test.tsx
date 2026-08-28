import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen } from "@testing-library/react";
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
});
