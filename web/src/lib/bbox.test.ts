import { describe, expect, it } from "vitest";
import type { Bbox } from "../api";
import { unionBbox } from "./bbox";

describe("unionBbox", () => {
  it("grows to contain every box", () => {
    const boxes: Bbox[] = [
      [13.7, 51.0, 13.8, 51.1],
      [13.6, 51.05, 13.75, 51.2],
    ];
    expect(unionBbox(boxes)).toEqual([13.6, 51.0, 13.8, 51.2]);
  });

  it("ignores the days with no fix at all", () => {
    const box: Bbox = [13.7, 51.0, 13.8, 51.1];
    expect(unionBbox([null, box, undefined])).toEqual(box);
    expect(unionBbox([null, null])).toBe(null);
    expect(unionBbox([])).toBe(null);
  });

  it("does not modify the box it started from", () => {
    const box: Bbox = [13.7, 51.0, 13.8, 51.1];
    unionBbox([box, [0, 0, 1, 1]]);
    expect(box).toEqual([13.7, 51.0, 13.8, 51.1]);
  });
});
