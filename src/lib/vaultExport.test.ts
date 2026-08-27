import { describe, it, expect } from "vitest";
import { slug, pageRelPath } from "./vaultExport";
import type { Page } from "./ipc";

// The vault's file paths are a CONTRACT, not a convenience: `notes.rel_path` maps a page to a file
// on disk, and the same rules run on every paired device. A path that changes shape, or that the OS
// refuses to create, silently orphans the page from its file.

const page = (over: Partial<Page> = {}): Page =>
  ({
    id: 1,
    title: "Untitled",
    parentId: null,
    dailyDate: null,
    archived: false,
    content: "",
    contentJson: null,
    sortOrder: 0,
    inbox: false,
    createdAt: "",
    updatedAt: "",
    ...over,
  }) as unknown as Page;

describe("slug", () => {
  it("keeps letters, numbers, spaces, hyphens and underscores", () => {
    expect(slug("Q3 Planning - draft_2")).toBe("Q3 Planning - draft_2");
  });

  it("replaces path separators and reserved characters so a title cannot escape the vault", () => {
    // The Rust side rejects traversal as a second line of defence (`vault::safe_join`), but a title
    // must not produce a path that needs rejecting in the first place.
    expect(slug("a/b")).toBe("a b");
    expect(slug("..")).toBe("Untitled");
    expect(slug("../../etc/passwd")).toBe("etc passwd");
    expect(slug("C:\\Windows\\System32")).toBe("C Windows System32");
    for (const ch of ['"', "*", ":", "<", ">", "?", "|"]) {
      expect(slug(`x${ch}y`), `${ch} should not survive`).toBe("x y");
    }
  });

  it("falls back to Untitled when nothing usable is left", () => {
    expect(slug("")).toBe("Untitled");
    expect(slug("   ")).toBe("Untitled");
    expect(slug("///")).toBe("Untitled");
    expect(slug("!!!")).toBe("Untitled");
  });

  it("keeps non-Latin scripts, which are perfectly good filenames", () => {
    expect(slug("\u8a08\u753b")).toBe("\u8a08\u753b");
    expect(slug("\u0417\u0430\u043c\u0435\u0442\u043a\u0438 2")).toBe("\u0417\u0430\u043c\u0435\u0442\u043a\u0438 2");
  });

  it("caps the length without leaving a trailing space", () => {
    // Windows silently strips trailing spaces and dots from filenames, so a slug that ends in one
    // names a file that will never exist — and the page loses its mapping on the next sync.
    const long = `${"a".repeat(79)} more words`;
    const out = slug(long);
    expect(out.length).toBeLessThanOrEqual(80);
    expect(out).not.toMatch(/[ .]$/);
  });

  it("does not emit a Windows reserved device name", () => {
    // `CON.md`, `NUL.md`, `COM1.md` and friends cannot be created on Windows at all — the write
    // fails and the page is silently never mirrored.
    for (const name of ["CON", "con", "PRN", "AUX", "NUL", "COM1", "com9", "LPT1", "lpt9"]) {
      expect(slug(name).toLowerCase(), `${name} must be escaped`).not.toBe(name.toLowerCase());
    }
    // ...but a name that merely starts with one is fine.
    expect(slug("Console design")).toBe("Console design");
    expect(slug("Nullable types")).toBe("Nullable types");
  });

  it("is stable — the same title always yields the same path", () => {
    expect(slug("Weekly review")).toBe(slug("Weekly review"));
  });
});

describe("pageRelPath", () => {
  it("files a daily note under Daily/<year-month>/", () => {
    const p = page({ id: 1, title: "2026-08-27", dailyDate: "2026-08-27" });
    expect(pageRelPath(p, [p])).toBe("Daily/2026-08/2026-08-27.md");
  });

  it("puts a root page at the top of the vault", () => {
    const p = page({ id: 1, title: "Inbox notes" });
    expect(pageRelPath(p, [p])).toBe("Inbox notes.md");
  });

  it("mirrors the parent chain as nested folders, outermost first", () => {
    const gp = page({ id: 1, title: "Work" });
    const parent = page({ id: 2, title: "Q3", parentId: 1 });
    const child = page({ id: 3, title: "Roadmap", parentId: 2 });
    expect(pageRelPath(child, [gp, parent, child])).toBe("Work/Q3/Roadmap.md");
  });

  it("survives a broken parent chain rather than throwing", () => {
    // A parent that is not in the list (archived, filtered out, mid-delete) truncates the path.
    const orphan = page({ id: 3, title: "Roadmap", parentId: 99 });
    expect(pageRelPath(orphan, [orphan])).toBe("Roadmap.md");
  });

  it("does not hang on a parent cycle", () => {
    // A cycle should be impossible, but a corrupt row must not lock the UI thread forever.
    const a = page({ id: 1, title: "A", parentId: 2 });
    const b = page({ id: 2, title: "B", parentId: 1 });
    const out = pageRelPath(a, [a, b]);
    expect(out.endsWith("A.md")).toBe(true);
    expect(out.split("/").length).toBeLessThan(5);
  });

  it("slugs every folder segment, not just the filename", () => {
    const parent = page({ id: 1, title: "Notes/2026" });
    const child = page({ id: 2, title: "Plan: v2", parentId: 1 });
    expect(pageRelPath(child, [parent, child])).toBe("Notes 2026/Plan v2.md");
  });

  it("gives an untitled page a filename anyway", () => {
    const p = page({ id: 1, title: "" });
    expect(pageRelPath(p, [p])).toBe("Untitled.md");
  });

  it("prefers the daily tree even when the page also has a parent", () => {
    const parent = page({ id: 1, title: "Journal" });
    const p = page({ id: 2, title: "2026-01-05", parentId: 1, dailyDate: "2026-01-05" });
    expect(pageRelPath(p, [parent, p])).toBe("Daily/2026-01/2026-01-05.md");
  });
});
