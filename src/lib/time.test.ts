import { describe, it, expect } from "vitest";
import {
  toLocalIso,
  toLocalDate,
  parseLocal,
  addMinutes,
  minutesBetween,
  startOfWeek,
  addDays,
  startOfMonth,
  sameMonth,
  mondayIndex,
  sameDay,
  humanMinutes,
} from "./time";

describe("time helpers", () => {
  it("toLocalIso / toLocalDate zero-pad and stay naive-local", () => {
    const d = new Date(2026, 5, 7, 9, 4, 3); // Jun 7 2026 09:04:03
    expect(toLocalIso(d)).toBe("2026-06-07T09:04:03");
    expect(toLocalDate(d)).toBe("2026-06-07");
  });

  it("parseLocal round-trips toLocalIso and defaults missing time to midnight", () => {
    const d = new Date(2026, 0, 31, 23, 15, 0);
    expect(toLocalIso(parseLocal(toLocalIso(d)))).toBe(toLocalIso(d));
    const midnight = parseLocal("2026-06-14");
    expect(midnight.getHours()).toBe(0);
    expect(midnight.getDate()).toBe(14);
  });

  it("addMinutes / minutesBetween are inverse", () => {
    const a = new Date(2026, 5, 14, 12, 0, 0);
    const b = addMinutes(a, 90);
    expect(minutesBetween(a, b)).toBe(90);
    expect(minutesBetween(b, a)).toBe(-90);
  });

  it("startOfWeek snaps to Monday (and is idempotent)", () => {
    // 2026-06-14 is a Sunday → its week starts Mon 2026-06-08.
    const sun = new Date(2026, 5, 14);
    const mon = startOfWeek(sun);
    expect(toLocalDate(mon)).toBe("2026-06-08");
    expect(mondayIndex(mon)).toBe(0);
    expect(toLocalDate(startOfWeek(mon))).toBe("2026-06-08");
  });

  it("mondayIndex maps Mon..Sun to 0..6", () => {
    expect(mondayIndex(new Date(2026, 5, 8))).toBe(0); // Mon
    expect(mondayIndex(new Date(2026, 5, 14))).toBe(6); // Sun
  });

  it("addDays crosses month boundaries", () => {
    expect(toLocalDate(addDays(new Date(2026, 5, 30), 2))).toBe("2026-07-02");
    expect(toLocalDate(addDays(new Date(2026, 0, 1), -1))).toBe("2025-12-31");
  });

  it("startOfMonth / sameMonth", () => {
    expect(toLocalDate(startOfMonth(new Date(2026, 5, 20)))).toBe("2026-06-01");
    expect(sameMonth(new Date(2026, 5, 1), new Date(2026, 5, 30))).toBe(true);
    expect(sameMonth(new Date(2026, 5, 1), new Date(2026, 6, 1))).toBe(false);
  });

  it("sameDay ignores time", () => {
    expect(sameDay(new Date(2026, 5, 14, 1), new Date(2026, 5, 14, 23))).toBe(true);
    expect(sameDay(new Date(2026, 5, 14), new Date(2026, 5, 15))).toBe(false);
  });

  it("humanMinutes formats durations", () => {
    expect(humanMinutes(45)).toBe("45m");
    expect(humanMinutes(60)).toBe("1h");
    expect(humanMinutes(135)).toBe("2h 15m");
  });
});
