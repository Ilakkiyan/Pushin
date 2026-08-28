import { describe, it, expect } from "vitest";
import type { Page } from "./ipc";
import { JOURNAL_ID, browsablePages, childrenOf, folderCount, folderPath, isAncestor, journalEntries, sortEntries } from "./pageTree";

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
    expect(isAncestor(pages, 1, 3)).toBe(true);
    expect(isAncestor(pages, 2, 3)).toBe(true);
  });
  it("is false for descendants / unrelated / self", () => {
    expect(isAncestor(pages, 3, 1)).toBe(false);
    expect(isAncestor(pages, 1, 1)).toBe(false);
    expect(isAncestor(pages, 99, 3)).toBe(false);
  });
});

describe("childrenOf", () => {
  it("buckets pages under their parent, roots under null", () => {
    const byParent = childrenOf([mk(1), mk(2, { parentId: 1 }), mk(3)]);
    expect(byParent.get(null)!.map((p) => p.id)).toEqual([1, 3]);
    expect(byParent.get(1)!.map((p) => p.id)).toEqual([2]);
  });
});

describe("browsablePages", () => {
  it("keeps documents and folders, drops journal entries, captures and archived rows", () => {
    const pages = [
      mk(1, { title: "Doc" }),
      mk(2, { title: "Folder", isFolder: true }),
      mk(3, { dailyDate: "2026-06-14" }),
      mk(4, { inbox: true }),
      mk(5, { archived: true }),
    ];
    expect(browsablePages(pages).map((p) => p.id)).toEqual([1, 2]);
  });
});

describe("folderPath (breadcrumb)", () => {
  const pages = [mk(1, { title: "Work", isFolder: true }), mk(2, { title: "Q3", isFolder: true, parentId: 1 })];
  it("is empty at the vault root", () => {
    expect(folderPath(pages, null)).toEqual([]);
  });
  it("walks root → leaf", () => {
    expect(folderPath(pages, 2).map((p) => p.title)).toEqual(["Work", "Q3"]);
  });
  it("gives the virtual Journal a one-entry trail", () => {
    expect(folderPath(pages, JOURNAL_ID).map((p) => p.title)).toEqual(["Journal"]);
  });
  it("terminates on a cyclic parent chain instead of hanging", () => {
    // Malformed data (a sync collision, a hand-edited DB): 1 → 2 → 1. The breadcrumb must not spin.
    const cyclic = [mk(1, { parentId: 2 }), mk(2, { parentId: 1 })];
    expect(folderPath(cyclic, 1).map((p) => p.id).sort()).toEqual([1, 2]);
  });
});

describe("sortEntries", () => {
  it("puts folders first, then names in natural order", () => {
    const entries = [
      mk(1, { title: "Week 10" }),
      mk(2, { title: "Week 2" }),
      mk(3, { title: "zeta", isFolder: true }),
      mk(4, { title: "Alpha", isFolder: true }),
    ];
    expect(sortEntries(entries).map((p) => p.title)).toEqual(["Alpha", "zeta", "Week 2", "Week 10"]);
  });
  it("does not mutate its input", () => {
    const entries = [mk(1, { title: "b" }), mk(2, { title: "a" })];
    sortEntries(entries);
    expect(entries.map((p) => p.id)).toEqual([1, 2]);
  });
});

describe("journalEntries", () => {
  it("returns only daily notes, newest first", () => {
    const pages = [mk(1, { dailyDate: "2026-06-12" }), mk(2, { title: "Doc" }), mk(3, { dailyDate: "2026-06-14" })];
    expect(journalEntries(pages).map((p) => p.dailyDate)).toEqual(["2026-06-14", "2026-06-12"]);
  });
});

describe("browsablePages — the boundaries", () => {
  it("keeps a folder that is itself inside a folder", () => {
    const pages = [mk(1, { isFolder: true }), mk(2, { isFolder: true, parentId: 1 })];
    expect(browsablePages(pages).map((p) => p.id)).toEqual([1, 2]);
  });
  it("drops a daily note even if something parented it to a folder", () => {
    // Nothing should do this, but a daily note belongs to the Journal wherever its parent points.
    const pages = [mk(1, { isFolder: true }), mk(2, { dailyDate: "2026-06-14", parentId: 1 })];
    expect(browsablePages(pages).map((p) => p.id)).toEqual([1]);
  });
  it("is empty for an empty vault rather than throwing", () => {
    expect(browsablePages([])).toEqual([]);
  });
});

describe("folderPath — the awkward inputs", () => {
  it("is empty for an id that no longer exists", () => {
    // A folder deleted on another device while you were standing in it.
    expect(folderPath([mk(1)], 404)).toEqual([]);
  });
  it("handles a folder whose parent has been deleted", () => {
    const orphan = [mk(2, { title: "Q3", isFolder: true, parentId: 99 })];
    expect(folderPath(orphan, 2).map((p) => p.title)).toEqual(["Q3"]);
  });
  it("walks a deep chain in root-first order", () => {
    const deep = [
      mk(1, { title: "A", isFolder: true }),
      mk(2, { title: "B", isFolder: true, parentId: 1 }),
      mk(3, { title: "C", isFolder: true, parentId: 2 }),
      mk(4, { title: "D", isFolder: true, parentId: 3 }),
    ];
    expect(folderPath(deep, 4).map((p) => p.title)).toEqual(["A", "B", "C", "D"]);
  });
});

describe("JOURNAL_ID", () => {
  it("is negative so it can never collide with a real page id", () => {
    // Page ids come from SQLite rowids, which start at 1.
    expect(JOURNAL_ID).toBeLessThan(0);
  });
});

describe("sortEntries — the tie-breaks", () => {
  it("sorts case-insensitively", () => {
    const entries = [mk(1, { title: "banana" }), mk(2, { title: "Apple" })];
    expect(sortEntries(entries).map((p) => p.title)).toEqual(["Apple", "banana"]);
  });
  it("keeps folders ahead of pages even when the page sorts first alphabetically", () => {
    const entries = [mk(1, { title: "aaa" }), mk(2, { title: "zzz", isFolder: true })];
    expect(sortEntries(entries).map((p) => p.title)).toEqual(["zzz", "aaa"]);
  });
  it("handles an empty list", () => {
    expect(sortEntries([])).toEqual([]);
  });
});

describe("folderCount", () => {
  const pages = [
    mk(1, { isFolder: true }),
    mk(2, { parentId: 1 }),
    mk(3, { parentId: 1 }),
    mk(4),
    mk(5, { dailyDate: "2026-06-14" }),
  ];
  it("counts a folder's direct children only", () => {
    expect(folderCount(pages, 1)).toBe(2);
  });
  it("counts daily notes for the virtual Journal", () => {
    expect(folderCount(pages, JOURNAL_ID)).toBe(1);
  });
});
