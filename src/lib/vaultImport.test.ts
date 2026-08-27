import { describe, it, expect, vi, beforeEach } from "vitest";

vi.mock("./ipc", () => ({
  api: {
    vaultUnlinkPath: vi.fn().mockResolvedValue(undefined),
    vaultPageForPath: vi.fn().mockResolvedValue(null),
    vaultLinkPath: vi.fn().mockResolvedValue(undefined),
    createPage: vi.fn().mockResolvedValue({ id: 42, title: "New" }),
    updatePage: vi.fn().mockResolvedValue(undefined),
  },
}));

import { titleFromRelPath, applyVaultChange } from "./vaultImport";
import { api } from "./ipc";
import type { VaultChange } from "./ipc";

const change = (over: Partial<VaultChange> = {}): VaultChange =>
  ({ relPath: "Notes/Plan.md", content: "# Plan\n\nsome text", kind: "update", ...over }) as VaultChange;

beforeEach(() => vi.clearAllMocks());

describe("titleFromRelPath", () => {
  it("uses the filename without its extension", () => {
    expect(titleFromRelPath("Work/Q3/Roadmap.md")).toBe("Roadmap");
    expect(titleFromRelPath("Roadmap.md")).toBe("Roadmap");
  });

  it("matches the extension case-insensitively", () => {
    expect(titleFromRelPath("Notes.MD")).toBe("Notes");
    expect(titleFromRelPath("Notes.Md")).toBe("Notes");
  });

  it("only strips the final extension", () => {
    expect(titleFromRelPath("release.notes.md")).toBe("release.notes");
    expect(titleFromRelPath("2026-08-27.md")).toBe("2026-08-27");
  });

  it("never returns an empty title", () => {
    // A file literally named `.md`, or a path that ends in a separator, must still name a page —
    // an empty title renders as a blank row the user cannot click.
    expect(titleFromRelPath(".md")).toBe("Untitled");
    expect(titleFromRelPath("")).toBe("Untitled");
    expect(titleFromRelPath("Daily/")).toBe("Untitled");
  });

  it("keeps a name that has no extension at all", () => {
    expect(titleFromRelPath("Notes/README")).toBe("README");
  });
});

describe("applyVaultChange", () => {
  it("unlinks the mapping on a remove and reports the tree changed", async () => {
    const changed = await applyVaultChange(change({ kind: "remove", content: "" }));
    expect(api.vaultUnlinkPath).toHaveBeenCalledWith("Notes/Plan.md");
    expect(changed).toBe(true);
    // Deleting the FILE must not delete the page — that is the documented contract, and the
    // opposite would make an external `rm` destroy vault content irrecoverably.
    expect(api.createPage).not.toHaveBeenCalled();
    expect(api.updatePage).not.toHaveBeenCalled();
  });

  it("updates the page already mapped to the path, without creating another", async () => {
    vi.mocked(api.vaultPageForPath).mockResolvedValueOnce(7 as never);

    const changed = await applyVaultChange(change());

    expect(api.updatePage).toHaveBeenCalledTimes(1);
    expect(vi.mocked(api.updatePage).mock.calls[0][0]).toBe(7);
    expect(api.createPage).not.toHaveBeenCalled();
    expect(api.vaultLinkPath).not.toHaveBeenCalled();
    expect(changed).toBe(false, "an in-place edit does not change the set of pages");
  });

  it("creates and links a page for a file it has never seen", async () => {
    const changed = await applyVaultChange(change({ relPath: "Inbox/Fresh.md" }));

    expect(api.createPage).toHaveBeenCalledWith("Fresh", null);
    expect(api.vaultLinkPath).toHaveBeenCalledWith(42, "Inbox/Fresh.md");
    expect(changed).toBe(true);
  });

  it("titles the page from the filename, not from the markdown heading", async () => {
    // The filename is the identity — it is what `rel_path` matches on. Taking the H1 instead would
    // make renaming a heading orphan the file.
    await applyVaultChange(change({ relPath: "Work/Roadmap.md", content: "# Something else\n\nbody" }));
    expect(api.createPage).toHaveBeenCalledWith("Roadmap", null);
  });

  it("carries wikilinks through so the graph stays connected", async () => {
    await applyVaultChange(change({ content: "See [[Other page]] and [[Third]]." }));
    const links = vi.mocked(api.updatePage).mock.calls[0][5] as string[];
    expect(links).toEqual(expect.arrayContaining(["Other page", "Third"]));
  });

  it("handles an empty file without throwing", async () => {
    await expect(applyVaultChange(change({ content: "" }))).resolves.toBe(true);
    expect(api.updatePage).toHaveBeenCalled();
  });

  it("stores the parsed blocks as JSON alongside the plain text", async () => {
    await applyVaultChange(change({ content: "# Heading\n\nA paragraph." }));
    const [, , , text, json] = vi.mocked(api.updatePage).mock.calls[0];
    expect(typeof text).toBe("string");
    expect(() => JSON.parse(json as string)).not.toThrow();
    expect(Array.isArray(JSON.parse(json as string))).toBe(true);
  });
});
