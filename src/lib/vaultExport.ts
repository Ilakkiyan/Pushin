// Vault export (DB → files): compute the rule-based file path for a page so the SQLite vault is
// mirrored to real markdown files in the user's chosen folder (two-way, Obsidian-style). The reverse
// (files → DB) is the Phase 3e watcher. Slugging lives here (the frontend owns path rules); Rust just
// writes the bytes to `<vault_dir>/<rel_path>` (see `vault.rs` / the `vault_write` command).
import { BlockNoteEditor } from "@blocknote/core";
import { api, type Page } from "./ipc";
import { schema } from "./editorSchema";

/** Filenames Windows refuses to create, whatever the extension. `CON.md` simply cannot exist. */
const WINDOWS_RESERVED = /^(con|prn|aux|nul|com[1-9]|lpt[1-9])$/i;

/** A safe, readable filename/folder segment: drop path separators + reserved chars, collapse spaces.
 *
 *  Two OS rules matter here because getting them wrong is silent: Windows strips trailing spaces and
 *  dots from filenames (so a slug ending in one names a file that never exists, and the page loses
 *  its `rel_path` mapping on the next pass), and it reserves a handful of legacy device names
 *  outright. Both are cheap to avoid and impossible to debug from the UI. */
export function slug(title: string): string {
  const cleaned = title
    .replace(/[^\p{L}\p{N} \-_]/gu, " ")
    .replace(/\s+/g, " ")
    .trim();
  // Trim again AFTER the length cap — the cut itself can land on a space.
  const capped = (cleaned || "Untitled").slice(0, 80).replace(/[ .]+$/, "");
  if (!capped) return "Untitled";
  return WINDOWS_RESERVED.test(capped) ? `${capped}_` : capped;
}

/** Where a page lives inside the vault folder. Daily notes get a dated `Daily/` tree; every other
 *  page mirrors its parent chain as nested folders, filename = slug(title). Relative + rule-based, so
 *  it's identical across devices even though the vault root differs. */
export function pageRelPath(page: Page, pages: Page[]): string {
  if (page.dailyDate) {
    return `Daily/${page.dailyDate.slice(0, 7)}/${page.dailyDate}.md`;
  }
  const byId = new Map(pages.map((p) => [p.id, p]));
  const folders: string[] = [];
  const seen = new Set<number>();
  let parentId = page.parentId ?? null;
  while (parentId != null && !seen.has(parentId)) {
    seen.add(parentId);
    const parent = byId.get(parentId);
    if (!parent) break;
    folders.unshift(slug(parent.title));
    parentId = parent.parentId ?? null;
  }
  return [...folders, `${slug(page.title || "Untitled")}.md`].join("/");
}

/** One-time bulk export: mirror every (non-archived) page to a file. Used when the user first picks a
 *  vault folder so it isn't empty. Best-effort per page (a page that won't convert is skipped, not
 *  fatal). Returns how many landed. md↔blocks is lossy (BlockNote) — structure/text survive. */
export async function exportAllPages(pages: Page[]): Promise<number> {
  const editor = BlockNoteEditor.create({ schema });
  let n = 0;
  for (const p of pages) {
    if (p.archived) continue;
    try {
      const full = await api.getPage(p.id); // the list may omit contentJson; fetch the full doc
      const blocks = full?.contentJson ? JSON.parse(full.contentJson) : [];
      const markdown = await editor.blocksToMarkdownLossy(blocks);
      await api.vaultWrite(p.id, pageRelPath(full ?? p, pages), markdown);
      n++;
    } catch {
      /* skip a page that won't convert rather than aborting the whole export */
    }
  }
  return n;
}
