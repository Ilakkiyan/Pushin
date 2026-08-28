import type { Page } from "./ipc";

/** The Journal's stand-in folder id. Daily notes are NOT parented under a real folder row — they're
 *  identified by `dailyDate` and created by the backend with no parent, so giving them a physical
 *  home would mean teaching `daily_note`, the file mirror and the graph about it. Instead the browser
 *  shows one virtual folder at the vault root that gathers them, which is what "journal entries live
 *  in a folder" means to the person looking at the screen. Negative so it can never collide with a
 *  real page id. */
export const JOURNAL_ID = -1;

/** True if `ancestorId` sits somewhere above `nodeId` in the tree — the drag-reparent cycle guard
 *  (you can't drop a folder into its own subtree). */
export function isAncestor(pages: Page[], ancestorId: number, nodeId: number): boolean {
  let cur = pages.find((p) => p.id === nodeId);
  while (cur?.parentId != null) {
    if (cur.parentId === ancestorId) return true;
    const parentId: number = cur.parentId;
    cur = pages.find((p) => p.id === parentId);
  }
  return false;
}

/** Build a parentId → children map once per render so tree walks are O(n). */
export function childrenOf(pages: Page[]): Map<number | null, Page[]> {
  const map = new Map<number | null, Page[]>();
  for (const p of pages) {
    const key = p.parentId ?? null;
    const arr = map.get(key) ?? [];
    arr.push(p);
    map.set(key, arr);
  }
  return map;
}

/** The pages that make up the vault's browsable tree: user documents and folders. Daily notes live
 *  in the Journal and quick captures live in the Inbox, so neither belongs here. */
export function browsablePages(pages: Page[]): Page[] {
  return pages.filter((p) => !p.dailyDate && !p.inbox && !p.archived);
}

/** Root → `id` breadcrumb chain (excluding the root itself). Empty at the vault root; a single
 *  synthetic entry for the virtual Journal. Guards against a cycle in malformed data so a bad
 *  parent_id can spin the breadcrumb forever instead of rendering. */
export function folderPath(pages: Page[], id: number | null): Page[] {
  if (id == null) return [];
  if (id === JOURNAL_ID) {
    return [{ id: JOURNAL_ID, title: "Journal", isFolder: true } as Page];
  }
  const chain: Page[] = [];
  const seen = new Set<number>();
  let cur = pages.find((p) => p.id === id);
  while (cur && !seen.has(cur.id)) {
    seen.add(cur.id);
    chain.unshift(cur);
    cur = cur.parentId == null ? undefined : pages.find((p) => p.id === cur!.parentId);
  }
  return chain;
}

/** Sort a folder's contents Drive-style: folders first, then by name (case-insensitive, natural
 *  numeric order so "Week 2" precedes "Week 10"). */
export function sortEntries(entries: Page[]): Page[] {
  const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });
  return entries.slice().sort((a, b) => {
    if (!!a.isFolder !== !!b.isFolder) return a.isFolder ? -1 : 1;
    return collator.compare(a.title, b.title);
  });
}

/** Daily notes, newest first — the Journal folder's contents. */
export function journalEntries(pages: Page[]): Page[] {
  return pages.filter((p) => p.dailyDate).sort((a, b) => (a.dailyDate! < b.dailyDate! ? 1 : -1));
}

/** How many items a folder directly holds — the "3 items" line on a folder card. */
export function folderCount(pages: Page[], folderId: number): number {
  if (folderId === JOURNAL_ID) return journalEntries(pages).length;
  return browsablePages(pages).filter((p) => (p.parentId ?? null) === folderId).length;
}
