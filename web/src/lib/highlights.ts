/**
 * The highlights view's range and how it travels in the URL.
 *
 * `/api/highlights` insists on both ends and takes at most 400 days, so unlike the heatmap this
 * view has no all-time reading: a missing bound falls back to the last twelve months.
 */

import type { Highlight } from "../api";
import { monthOf } from "./dates";
import {
  boundRange,
  type DateRange,
  PRESETS,
  type PresetKey,
  parseRange,
  presetRange,
} from "./range";

/** All time needs a bound the endpoint would refuse, so it is not on offer here. */
export const HIGHLIGHT_PRESETS = PRESETS.filter(({ key }) => key !== "all");

export const DEFAULT_PRESET: PresetKey = "12m";

export function parseHighlightRange(search: URLSearchParams, today: string): DateRange {
  return boundRange(parseRange(search), presetRange(DEFAULT_PRESET, today));
}

/** The canonical query string for a range. Both ends are always in it, so a link is stable. */
export function highlightSearch(range: DateRange): string {
  const search = new URLSearchParams();
  if (range.from) search.set("from", range.from);
  if (range.to) search.set("to", range.to);
  const rendered = search.toString();
  return rendered ? `?${rendered}` : "";
}

export type HighlightMonth = { month: string; events: Highlight[] };

/**
 * The events grouped into the months they happened in, in the order they arrived. The API sorts
 * by date already, so a new month simply starts a new group.
 */
export function byMonth(events: Highlight[]): HighlightMonth[] {
  const months: HighlightMonth[] = [];
  for (const event of events) {
    const month = monthOf(event.date);
    const last = months.at(-1);
    if (last?.month === month) last.events.push(event);
    else months.push({ month, events: [event] });
  }
  return months;
}
