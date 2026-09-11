/**
 * The heatmap's date range and weighting, and how they travel in the URL.
 *
 * A range is a pair of local `YYYY-MM-DD` days, inclusive, with `null` meaning "no bound": all
 * time is `{ from: null, to: null }`. The presets are worked out relative to a given today
 * rather than the browser's, so they can be tested and so a deep link keeps meaning what it
 * said when it was copied.
 */

import { addDays, isDate, monthEnd, monthOf, parseDate, addMonths as shiftMonth } from "./dates";

export type HeatRange = { from: string | null; to: string | null };

/** What each cell's number counts. Days is the default; see the README. */
export type HeatWeight = "days" | "samples";

export type PresetKey = "30d" | "12m" | "year" | "all";

export const PRESETS: { key: PresetKey; label: string }[] = [
  { key: "30d", label: "Last 30 days" },
  { key: "12m", label: "Last 12 months" },
  { key: "year", label: "This year" },
  { key: "all", label: "All time" },
];

export const ALL_TIME: HeatRange = { from: null, to: null };

export function presetRange(key: PresetKey, today: string): HeatRange {
  switch (key) {
    // Inclusive of today, so "last 30 days" is 30 days and not 31.
    case "30d":
      return { from: addDays(today, -29), to: today };
    case "12m":
      return { from: addMonthsToDate(today, -12), to: today };
    case "year":
      return { from: `${parseDate(today).year}-01-01`, to: today };
    default:
      return ALL_TIME;
  }
}

/** Which preset a range is, if any, so the button row can show what is selected. */
export function matchingPreset(range: HeatRange, today: string): PresetKey | null {
  for (const { key } of PRESETS) {
    const preset = presetRange(key, today);
    if (preset.from === range.from && preset.to === range.to) return key;
  }
  return null;
}

/**
 * The same day of the month `months` later or earlier, clamped to the end of the month it lands
 * in: twelve months before a leap day is 28 February, not the first of March.
 */
function addMonthsToDate(date: string, months: number): string {
  const month = shiftMonth(monthOf(date), months);
  const day = Math.min(parseDate(date).day, parseDate(monthEnd(month)).day);
  return `${month}-${String(day).padStart(2, "0")}`;
}

/** The range and weight a URL asks for. Anything unreadable falls back to the default. */
export function parseHeatParams(search: URLSearchParams): {
  range: HeatRange;
  weight: HeatWeight;
} {
  const day = (name: string) => {
    const value = search.get(name);
    return value && isDate(value) ? value : null;
  };
  let [from, to] = [day("from"), day("to")];
  // A backwards range is a typo in the address bar, not a reason to show nothing.
  if (from && to && from > to) [from, to] = [to, from];
  return {
    range: { from, to },
    weight: search.get("weight") === "samples" ? "samples" : "days",
  };
}

/** The canonical query string for a range and weight; the defaults are simply left out. */
export function heatSearch(range: HeatRange, weight: HeatWeight): string {
  const search = new URLSearchParams();
  if (range.from) search.set("from", range.from);
  if (range.to) search.set("to", range.to);
  if (weight !== "days") search.set("weight", weight);
  const rendered = search.toString();
  return rendered ? `?${rendered}` : "";
}
