// Helpers for the BlockNote editor: turning a stored page into editor content, and extracting the
// plaintext that backs semantic recall + keyword search (the `content` column in the DB).
import type { PartialBlock } from "@blocknote/core";
import type { Page } from "./ipc";

// A structural shape any BlockNote block satisfies (default or custom schema) — these walkers only
// read `content`/`children`, so they stay decoupled from the concrete schema types.
interface BlockLike {
  content?: unknown;
  children?: readonly BlockLike[];
}

/** Recursively pull the visible text out of BlockNote blocks (for the recall/search index). */
export function blocksToPlainText(blocks: readonly BlockLike[]): string {
  const lines: string[] = [];

  const inlineText = (content: unknown): string => {
    if (!content) return "";
    if (typeof content === "string") return content;
    if (Array.isArray(content)) return content.map(inlineText).join("");
    const c = content as Record<string, unknown>;
    // Wikilink chips contribute their target title to the recall/search index.
    if (c.type === "pageLink") return String((c.props as Record<string, unknown>)?.title ?? "");
    if (typeof c.text === "string") return c.text;
    if (c.content) return inlineText(c.content); // links carry nested inline content
    return "";
  };

  const walk = (bs: readonly BlockLike[]) => {
    for (const b of bs) {
      const line = inlineText(b.content);
      if (line.trim()) lines.push(line);
      if (b.children?.length) walk(b.children);
    }
  };

  walk(blocks);
  return lines.join("\n");
}

/** The distinct target titles of every wikilink (`pageLink` chip) in the document — sent on save so
 *  the backend can rebuild this page's outgoing edges (resolving each title to a page). */
export function extractLinkTitles(blocks: readonly BlockLike[]): string[] {
  const titles = new Set<string>();

  const scanInline = (content: unknown) => {
    if (!content || typeof content === "string") return;
    if (Array.isArray(content)) return content.forEach(scanInline);
    const c = content as Record<string, unknown>;
    if (c.type === "pageLink") {
      const t = String((c.props as Record<string, unknown>)?.title ?? "").trim();
      if (t) titles.add(t);
    }
  };

  const walk = (bs: readonly BlockLike[]) => {
    for (const b of bs) {
      scanInline(b.content);
      if (b.children?.length) walk(b.children);
    }
  };

  walk(blocks);
  return [...titles];
}

/** The initial editor content for a page: its saved block JSON, else legacy plaintext split into
 *  paragraphs, else undefined (a fresh empty page). Returned loosely typed — the editor's custom
 *  schema accepts these plain paragraph/JSON blocks; the caller casts to the schema's block type. */
export function pageToInitialContent(page: Page): PartialBlock[] | undefined {
  if (page.contentJson) {
    try {
      const parsed = JSON.parse(page.contentJson) as PartialBlock[];
      if (Array.isArray(parsed) && parsed.length) return parsed;
    } catch {
      /* fall through to plaintext */
    }
  }
  const text = page.content?.trim();
  if (text) {
    return text.split(/\n+/).map((line) => ({ type: "paragraph", content: line }) as PartialBlock);
  }
  return undefined;
}
