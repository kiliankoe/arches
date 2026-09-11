import { describe, expect, it } from "vitest";
import type { DayFeature, DayGeoJson } from "../api";
import { tracksOnly } from "./tracks";

function feature(type: "LineString" | "Point", itemId: string): DayFeature {
  return {
    type: "Feature",
    geometry:
      type === "LineString"
        ? { type, coordinates: [[13.7, 51.0] as [number, number]] }
        : { type, coordinates: [13.7, 51.0] },
    properties: {
      date: "2025-06-10",
      itemId,
      startDate: null,
      endDate: null,
      confirmed: true,
    },
  };
}

describe("tracksOnly", () => {
  it("keeps the lines and drops the visit points", () => {
    const geojson: DayGeoJson = {
      type: "FeatureCollection",
      features: [feature("Point", "a"), feature("LineString", "b"), feature("Point", "c")],
    };

    const tracks = tracksOnly(geojson);

    expect(tracks?.features.map((one) => one.properties.itemId)).toEqual(["b"]);
    expect(geojson.features).toHaveLength(3);
  });

  it("passes nothing through as nothing", () => {
    expect(tracksOnly(null)).toBe(null);
  });
});
