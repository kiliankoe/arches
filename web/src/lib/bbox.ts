/** Bounding boxes in the GeoJSON order the API uses: `[minLon, minLat, maxLon, maxLat]`. */

import type { Bbox } from "../api";

/** The box that contains all of them, for framing a month or a week of days at once. */
export function unionBbox(boxes: (Bbox | null | undefined)[]): Bbox | null {
  let union: Bbox | null = null;
  for (const box of boxes) {
    if (!box) continue;
    union = union
      ? [
          Math.min(union[0], box[0]),
          Math.min(union[1], box[1]),
          Math.max(union[2], box[2]),
          Math.max(union[3], box[3]),
        ]
      : [...box];
  }
  return union;
}
