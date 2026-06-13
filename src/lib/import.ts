// Vault importer: pick an Obsidian/Markdown folder, convert each file to BlockNote blocks (so
// formatting survives), and create a page. `[[wikilinks]]` in the markdown become page links —
// resolved by title in the graph/backlinks even if the target imports later (ghost resolution).
import { open } from "@tauri-apps/plugin-dialog";
import { BlockNoteEditor } from "@blocknote/core";
import { api } from "./ipc";
import { schema } from "./editorSchema";
import { blocksToPlainText } from "./blocks";

/** Distinct `[[target]]` titles in raw markdown, stripping Obsidian `|alias` and `#heading` parts. */
function wikilinkTitles(md: string): string[] {
  const out = new Set<string>();
  for (const m of md.matchAll(/\[\[([^\]\n]+)\]\]/g)) {
    const t = m[1].split("|")[0].split("#")[0].trim();
    if (t) out.add(t);
  }
  return [...out];
}

/** Let the user pick a folder, import every Markdown file under it, and return how many landed.
 *  Returns null if the user cancelled the folder picker. */
export async function importMarkdownFolder(onProgress?: (done: number, total: number) => void): Promise<number | null> {
  const picked = await open({ directory: true, multiple: false, title: "Choose a vault / Markdown folder" });
  if (!picked || Array.isArray(picked)) return null;

  const docs = await api.readMarkdownDir(picked);
  if (docs.length === 0) return 0;

  // One headless editor, reused to parse markdown → blocks for every file.
  const editor = BlockNoteEditor.create({ schema });

  let done = 0;
  for (const doc of docs) {
    try {
      const blocks = await editor.tryParseMarkdownToBlocks(doc.markdown);
      const text = blocksToPlainText(blocks);
      const page = await api.createPage(doc.title, null);
      await api.updatePage(page.id, doc.title, null, text, JSON.stringify(blocks), wikilinkTitles(doc.markdown));
    } catch {
      /* skip a file that won't parse rather than aborting the whole import */
    }
    onProgress?.(++done, docs.length);
  }
  return done;
}
