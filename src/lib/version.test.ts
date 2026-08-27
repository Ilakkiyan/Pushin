import { describe, it, expect } from "vitest";
import { compareVersions, isNewer } from "./version";

describe("compareVersions", () => {
  it("orders by each numeric segment in turn", () => {
    expect(compareVersions("0.8.3", "0.8.2")).toBeGreaterThan(0);
    expect(compareVersions("0.8.2", "0.8.3")).toBeLessThan(0);
    expect(compareVersions("0.9.0", "0.8.9")).toBeGreaterThan(0);
    expect(compareVersions("1.0.0", "0.99.99")).toBeGreaterThan(0);
    expect(compareVersions("0.8.2", "0.8.2")).toBe(0);
  });

  it("is numeric, not lexicographic", () => {
    // The bug a naive string compare ships with: "0.10.0" < "0.9.0" as text, so everyone updating
    // to 0.10 would be told about nothing at all.
    expect(compareVersions("0.10.0", "0.9.0")).toBeGreaterThan(0);
    expect(compareVersions("0.8.10", "0.8.9")).toBeGreaterThan(0);
    expect(compareVersions("2.0.0", "10.0.0")).toBeLessThan(0);
  });

  it("tolerates a leading v", () => {
    expect(compareVersions("v0.8.3", "0.8.2")).toBeGreaterThan(0);
    expect(compareVersions("V1.0.0", "v1.0.0")).toBe(0);
  });

  it("treats missing segments as zero", () => {
    expect(compareVersions("1.0", "1.0.0")).toBe(0);
    expect(compareVersions("1", "1.0.0")).toBe(0);
    expect(compareVersions("1.1", "1.0.9")).toBeGreaterThan(0);
  });

  it("sorts a prerelease before its own release", () => {
    expect(compareVersions("1.0.0-beta", "1.0.0")).toBeLessThan(0);
    expect(compareVersions("1.0.0", "1.0.0-beta")).toBeGreaterThan(0);
    expect(compareVersions("1.0.0-alpha", "1.0.0-beta")).toBeLessThan(0);
    expect(compareVersions("1.0.0-beta", "1.0.0-beta")).toBe(0);
  });

  it("ignores build metadata", () => {
    expect(compareVersions("1.0.0+build.7", "1.0.0")).toBe(0);
  });

  it("never throws on garbage, and sorts it oldest", () => {
    // A cleared or hand-edited localStorage value must not take the app down on launch.
    for (const junk of ["", "   ", "not-a-version", "..", "x.y.z", "0.8.z"]) {
      expect(() => compareVersions(junk, "0.8.3")).not.toThrow();
      expect(compareVersions(junk, "0.8.3"), `${JSON.stringify(junk)} should sort oldest`).toBeLessThan(0);
    }
  });

  it("is a usable sort comparator", () => {
    const sorted = ["0.10.0", "0.8.2", "1.0.0-beta", "0.9.0", "1.0.0"].sort(compareVersions);
    expect(sorted).toEqual(["0.8.2", "0.9.0", "0.10.0", "1.0.0-beta", "1.0.0"]);
  });

  it("is antisymmetric", () => {
    const pairs: Array<[string, string]> = [
      ["0.8.2", "0.8.3"],
      ["1.0.0", "1.0.0"],
      ["0.10.0", "0.9.9"],
      ["1.0.0-beta", "1.0.0"],
    ];
    // Normalise the sign: Math.sign(0) is 0 but -Math.sign(0) is -0, and Object.is tells them apart.
    const dir = (n: number) => (n === 0 ? 0 : Math.sign(n));
    for (const [a, b] of pairs) {
      expect(dir(compareVersions(a, b))).toBe(dir(-compareVersions(b, a)));
    }
  });
});

describe("isNewer", () => {
  it("is strict — equal is not newer", () => {
    expect(isNewer("0.8.3", "0.8.2")).toBe(true);
    expect(isNewer("0.8.2", "0.8.2")).toBe(false);
    expect(isNewer("0.8.1", "0.8.2")).toBe(false);
  });
});
