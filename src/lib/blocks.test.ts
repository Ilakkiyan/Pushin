import { describe, it, expect } from "vitest";
import { blocksToPlainText, extractLinkTitles, pageToInitialContent } from "./blocks";
import type { Page } from "./ipc";

// Minimal BlockNote-shaped blocks (the helpers only read content/children).
const para = (...content: unknown[]) => ({ type: "paragraph", content });
const text = (t: string) => ({ type: "text", text: t });
const link = (title: string, pageId = "1") => ({ type: "pageLink", props: { title, pageId } });

function page(overrides: Partial<Page>): Page {
  return {
    id: 1,
    title: "T",
    content: "",
    sortOrder: 0,
    archived: false,
    inbox: false,
    createdAt: "2026-01-01T00:00:00",
    updatedAt: "2026-01-01T00:00:00",
    indexed: false,
    ...overrides,
  };
}

describe("blocksToPlainText", () => {
  it("flattens text, recurses children, and includes wikilink titles", () => {
    const blocks = [
      para(text("Hello "), text("world")),
      { ...para(text("parent")), children: [para(text("child line"))] },
      para(text("see "), link("Budget")),
    ] as never;
    expect(blocksToPlainText(blocks)).toBe("Hello world\nparent\nchild line\nsee Budget");
  });

  it("ignores empty/whitespace-only lines", () => {
    expect(blocksToPlainText([para(), para(text("   ")), para(text("real"))] as never)).toBe("real");
  });
});

describe("extractLinkTitles", () => {
  it("collects distinct pageLink titles across blocks + children", () => {
    const blocks = [
      para(text("a "), link("Budget")),
      para(link("Budget")), // duplicate collapses
      { ...para(text("p")), children: [para(link("Roadmap"))] },
    ] as never;
    expect(extractLinkTitles(blocks).sort()).toEqual(["Budget", "Roadmap"]);
  });

  it("returns [] when there are no links", () => {
    expect(extractLinkTitles([para(text("plain"))] as never)).toEqual([]);
  });
});

describe("pageToInitialContent", () => {
  it("parses stored content_json when present", () => {
    const json = JSON.stringify([{ type: "heading", content: "Hi" }]);
    const out = pageToInitialContent(page({ contentJson: json }));
    expect(out).toEqual([{ type: "heading", content: "Hi" }]);
  });

  it("falls back to legacy plaintext split into paragraphs", () => {
    const out = pageToInitialContent(page({ content: "line one\n\nline two" }));
    expect(out).toEqual([
      { type: "paragraph", content: "line one" },
      { type: "paragraph", content: "line two" },
    ]);
  });

  it("returns undefined for an empty page and on malformed JSON", () => {
    expect(pageToInitialContent(page({ content: "  " }))).toBeUndefined();
    // malformed JSON falls through to (empty) plaintext → undefined
    expect(pageToInitialContent(page({ contentJson: "{not json", content: "" }))).toBeUndefined();
  });
});
