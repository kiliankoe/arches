import { describe, expect, it } from "vitest";
import { ALL_TIME, heatSearch, matchingPreset, parseHeatParams, presetRange } from "./heat";
import { heatRadiusExpression } from "./heatLayer";

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

describe("parseHeatParams", () => {
  it("reads a range and a weight", () => {
    const params = new URLSearchParams("from=2025-06-01&to=2025-06-30&weight=samples");
    expect(parseHeatParams(params)).toEqual({
      range: { from: "2025-06-01", to: "2025-06-30" },
      weight: "samples",
    });
  });

  it("defaults to all time counted in days", () => {
    expect(parseHeatParams(new URLSearchParams())).toEqual({
      range: ALL_TIME,
      weight: "days",
    });
  });

  it("ignores what it cannot read rather than failing", () => {
    const params = new URLSearchParams("from=last-june&to=2025-02-30&weight=hours");
    expect(parseHeatParams(params)).toEqual({ range: ALL_TIME, weight: "days" });
  });

  it("turns a backwards range the right way round", () => {
    const params = new URLSearchParams("from=2025-06-30&to=2025-06-01");
    expect(parseHeatParams(params).range).toEqual({ from: "2025-06-01", to: "2025-06-30" });
  });
});

describe("heatSearch", () => {
  it("leaves the defaults out", () => {
    expect(heatSearch(ALL_TIME, "days")).toBe("");
    expect(heatSearch(ALL_TIME, "samples")).toBe("?weight=samples");
  });

  it("round trips through parsing", () => {
    const range = { from: "2025-06-01", to: "2025-06-30" };
    const search = heatSearch(range, "samples");
    expect(search).toBe("?from=2025-06-01&to=2025-06-30&weight=samples");
    expect(parseHeatParams(new URLSearchParams(search))).toEqual({ range, weight: "samples" });
  });

  it("keeps a half-open range half open", () => {
    expect(heatSearch({ from: "2025-06-01", to: null }, "days")).toBe("?from=2025-06-01");
  });
});

describe("heatRadiusExpression", () => {
  /** At zoom 14 on the equator a pixel is about 9.55 m, so a 76 m cell is 8 px and the kernel 3.5x that. */
  it("scales the kernel with the cell's size on screen", () => {
    const [, , , zoom, radius, nextZoom, farRadius] = heatRadiusExpression(76.4, 14, 0) as number[];
    expect(zoom).toBe(14);
    expect(radius).toBeCloseTo(28, 0);
    expect(nextZoom).toBe(24);
    expect(farRadius).toBeCloseTo(radius * 1024, 3);
  });

  it("never drops below the minimum when cells are subpixel", () => {
    const [, , , , radius] = heatRadiusExpression(6, 3, 51) as number[];
    expect(radius).toBe(14);
  });
});
