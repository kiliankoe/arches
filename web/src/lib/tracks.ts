/** Cutting a range's GeoJSON down to what the scale it is drawn at can carry. */

import type { DayGeoJson } from "../api";

/**
 * The lines without the visit points. A month is a few hundred visits and they all sit in the
 * same town, so at the zoom a whole month frames they read as a carpet of dots rather than as
 * places; a week is sparse enough to keep them.
 */
export function tracksOnly(geojson: DayGeoJson | null): DayGeoJson | null {
  if (!geojson) return null;
  return {
    type: "FeatureCollection",
    features: geojson.features.filter((feature) => feature.geometry.type === "LineString"),
  };
}
