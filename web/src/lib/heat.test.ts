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
