/**
 * The heatmap's weighting and how it and the date range travel in the URL. The range itself,
 * and the presets both range-based views offer, live in `range.ts`.
 */

import { type DateRange, parseRange } from "./range";

/** What each cell's number counts. Days is the default; see the README. */
export type HeatWeight = "days" | "samples";

/** The range and weight a URL asks for. Anything unreadable falls back to the default. */
export function parseHeatParams(search: URLSearchParams): {
  range: DateRange;
  weight: HeatWeight;
} {
  return {
    range: parseRange(search),
    weight: search.get("weight") === "samples" ? "samples" : "days",
  };
}

/** The canonical query string for a range and weight; the defaults are simply left out. */
export function heatSearch(range: DateRange, weight: HeatWeight): string {
  const search = new URLSearchParams();
  if (range.from) search.set("from", range.from);
  if (range.to) search.set("to", range.to);
  if (weight !== "days") search.set("weight", weight);
  const rendered = search.toString();
  return rendered ? `?${rendered}` : "";
}
