import { describe, it, expect } from "vitest";
import { wikilinkTitles } from "./import";

describe("wikilinkTitles (Obsidian markdown import)", () => {
  it("extracts plain [[links]]", () => {
    expect(wikilinkTitles("see [[Budget]] and [[Roadmap]]")).toEqual(["Budget", "Roadmap"]);
  });

  it("strips |alias and #heading and trims", () => {
    expect(wikilinkTitles("[[ Budget | the budget ]]")).toEqual(["Budget"]);
    expect(wikilinkTitles("[[Roadmap#Q3]]")).toEqual(["Roadmap"]);
    expect(wikilinkTitles("[[Notes#Section|nick]]")).toEqual(["Notes"]);
  });

  it("dedupes repeated targets", () => {
    expect(wikilinkTitles("[[A]] [[A]] [[B]]")).toEqual(["A", "B"]);
  });

  it("returns [] when there are no links", () => {
    expect(wikilinkTitles("just prose, no links")).toEqual([]);
  });

  it("ignores empty brackets", () => {
    expect(wikilinkTitles("[[]] [[  ]] [[Real]]")).toEqual(["Real"]);
  });
});
