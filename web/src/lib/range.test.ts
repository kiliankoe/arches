import { describe, expect, it } from "vitest";
import { ALL_TIME, boundRange, matchingPreset, parseRange, presetRange } from "./range";

const TODAY = "2026-09-11";

describe("presetRange", () => {
  it("counts the last 30 days inclusive of today", () => {
    expect(presetRange("30d", TODAY)).toEqual({ from: "2026-08-13", to: TODAY });
  });

  it("takes 12 months back to the same day of the month", () => {
    expect(presetRange("12m", TODAY)).toEqual({ from: "2025-09-11", to: TODAY });
  });

  /** 12 months before a leap day is not the first of March. */
  it("clamps a day that the earlier month does not have", () => {
    expect(presetRange("12m", "2024-02-29").from).toBe("2023-02-28");
    expect(presetRange("12m", "2026-03-31").from).toBe("2025-03-31");
  });

  it("starts this year on the first of January", () => {
    expect(presetRange("year", TODAY)).toEqual({ from: "2026-01-01", to: TODAY });
    expect(presetRange("year", "2026-01-01")).toEqual({ from: "2026-01-01", to: "2026-01-01" });
  });

  it("leaves all time unbounded at both ends", () => {
    expect(presetRange("all", TODAY)).toEqual(ALL_TIME);
  });
});

describe("matchingPreset", () => {
  it("recognises a range a preset would have produced", () => {
    expect(matchingPreset(presetRange("30d", TODAY), TODAY)).toBe("30d");
    expect(matchingPreset(ALL_TIME, TODAY)).toBe("all");
  });

  it("is null for a range picked by hand", () => {
    expect(matchingPreset({ from: "2025-06-01", to: "2025-06-30" }, TODAY)).toBeNull();
    // The right length, the wrong end: still not the preset.
    expect(matchingPreset({ from: "2026-08-12", to: "2026-09-10" }, TODAY)).toBeNull();
  });
});

describe("parseRange", () => {
  it("reads both bounds", () => {
    const params = new URLSearchParams("from=2025-06-01&to=2025-06-30");
    expect(parseRange(params)).toEqual({ from: "2025-06-01", to: "2025-06-30" });
  });

  it("ignores what it cannot read rather than failing", () => {
    expect(parseRange(new URLSearchParams("from=last-june&to=2025-02-30"))).toEqual(ALL_TIME);
  });

  it("turns a backwards range the right way round", () => {
    const params = new URLSearchParams("from=2025-06-30&to=2025-06-01");
    expect(parseRange(params)).toEqual({ from: "2025-06-01", to: "2025-06-30" });
  });
});

describe("boundRange", () => {
  it("fills in only the end that is missing", () => {
    const fallback = presetRange("12m", TODAY);
    expect(boundRange({ from: "2025-06-01", to: null }, fallback)).toEqual({
      from: "2025-06-01",
      to: TODAY,
    });
    expect(boundRange(ALL_TIME, fallback)).toEqual(fallback);
  });
});
