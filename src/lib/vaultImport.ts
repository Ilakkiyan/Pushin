// Vault import (files → DB): apply an external `.md` change the Rust watcher saw to the SQLite vault.
// This is the inbound half of the two-way file vault; the outbound half is `vaultExport.ts`. Markdown→
// blocks runs through a headless BlockNote editor (same as `lib/import.ts`), so formatting survives.
import { BlockNoteEditor } from "@blocknote/core";
import { api, type VaultChange } from "./ipc";
import { schema } from "./editorSchema";
import { blocksToPlainText } from "./blocks";
import { wikilinkTitles } from "./import";

/** A page's title from its file: the filename without the `.md` extension. */
export function titleFromRelPath(relPath: string): string {
  const name = relPath.split("/").pop() ?? relPath;
  return name.replace(/\.md$/i, "") || "Untitled";
}

/** Apply one external file change to the DB. Update the page mapped to `relPath` (or create + link a
 *  new one); on remove, unlink the mapping (the page survives). Best-effort. Returns true when the set
 *  of pages changed (a create/remove) so the caller can refresh the tree. */
export async function applyVaultChange(change: VaultChange): Promise<boolean> {
  if (change.kind === "remove") {
    await api.vaultUnlinkPath(change.relPath);
    return true;
  }
  const editor = BlockNoteEditor.create({ schema });
  const blocks = await editor.tryParseMarkdownToBlocks(change.content);
  const text = blocksToPlainText(blocks);
  const title = titleFromRelPath(change.relPath);
  const links = wikilinkTitles(change.content);

  const existing = await api.vaultPageForPath(change.relPath);
  if (existing != null) {
    await api.updatePage(existing, title, null, text, JSON.stringify(blocks), links);
    return false;
  }
  const page = await api.createPage(title, null);
  await api.updatePage(page.id, title, null, text, JSON.stringify(blocks), links);
  await api.vaultLinkPath(page.id, change.relPath);
  return true;
}
