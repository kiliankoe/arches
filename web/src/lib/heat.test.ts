import { describe, expect, it } from "vitest";
import { heatSearch, parseHeatParams } from "./heat";
import { heatRadiusExpression } from "./heatLayer";
import { ALL_TIME } from "./range";

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

  it("ignores a weight it cannot read rather than failing", () => {
    const params = new URLSearchParams("from=last-june&to=2025-02-30&weight=hours");
    expect(parseHeatParams(params)).toEqual({ range: ALL_TIME, weight: "days" });
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
  /** At zoom 14 on the equator a pixel is about 4.78 m, so a 38.2 m cell is 8 px and the kernel three cells. */
  it("puts the kernel at three cells and doubles it per zoom level in both directions", () => {
    const [, , , low, lowRadius, high, highRadius] = heatRadiusExpression(38.2, 14, 0) as number[];
    expect(low).toBe(0);
    expect(high).toBe(24);
    // Base-2 exponential interpolation between the two stops is r * 2^(z - 14).
    expect(lowRadius * 2 ** 14).toBeCloseTo(24, 1);
    expect(highRadius / 2 ** 10).toBeCloseTo(24, 1);
  });
});
