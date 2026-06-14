import { describe, expect, it } from "vitest";
import { suggestAutoLabels } from "./autoLabels";

describe("suggestAutoLabels", () => {
  it("maps health, work, and errands keywords to label suggestions", () => {
    const suggestions = suggestAutoLabels(
      [
        { kind: "task", id: 1, title: "Gym workout" },
        { kind: "event", id: 2, title: "Team standup" },
        { kind: "task", id: 3, title: "Buy groceries" },
      ],
      "",
    );

    expect(suggestions.map((s) => [s.kind, s.entityId, s.labelName])).toEqual([
      ["task", 1, "Health"],
      ["event", 2, "Work"],
      ["task", 3, "Errands"],
    ]);
  });

  it("uses the original message only when there is one created target", () => {
    expect(suggestAutoLabels([{ kind: "task", id: 1, title: "Training" }], "go to the gym tomorrow")).toHaveLength(1);
    expect(
      suggestAutoLabels(
        [
          { kind: "task", id: 1, title: "Training" },
          { kind: "task", id: 2, title: "Review notes" },
        ],
        "go to the gym tomorrow",
      ),
    ).toHaveLength(0);
  });

  it("does not suggest labels when no deterministic keyword matches", () => {
    expect(suggestAutoLabels([{ kind: "task", id: 1, title: "Read chapter 4" }], "")).toEqual([]);
  });
});
