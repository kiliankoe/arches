/**
 * Date ranges and the presets the range-based views offer, shared by the heatmap and the
 * highlights.
 *
 * A range is a pair of local `YYYY-MM-DD` days, inclusive, with `null` meaning "no bound": all
 * time is `{ from: null, to: null }`. The presets are worked out relative to a given today
 * rather than the browser's, so they can be tested and so a deep link keeps meaning what it
 * said when it was copied.
 */

import { addDays, isDate, monthEnd, monthOf, parseDate, addMonths as shiftMonth } from "./dates";

export type DateRange = { from: string | null; to: string | null };

export type PresetKey = "30d" | "12m" | "year" | "all";

export const PRESETS: { key: PresetKey; label: string }[] = [
  { key: "30d", label: "Last 30 days" },
  { key: "12m", label: "Last 12 months" },
  { key: "year", label: "This year" },
  { key: "all", label: "All time" },
];

export const ALL_TIME: DateRange = { from: null, to: null };

export function presetRange(key: PresetKey, today: string): DateRange {
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
export function matchingPreset(range: DateRange, today: string): PresetKey | null {
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

/** The range a URL asks for. A bound that cannot be read is simply absent. */
export function parseRange(search: URLSearchParams): DateRange {
  const day = (name: string) => {
    const value = search.get(name);
    return value && isDate(value) ? value : null;
  };
  let [from, to] = [day("from"), day("to")];
  // A backwards range is a typo in the address bar, not a reason to show nothing.
  if (from && to && from > to) [from, to] = [to, from];
  return { from, to };
}

/** A range with both ends, for an endpoint that insists on them. */
export function boundRange(range: DateRange, fallback: DateRange): DateRange {
  return { from: range.from ?? fallback.from, to: range.to ?? fallback.to };
}
