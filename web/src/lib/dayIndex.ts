/**
 * Which local days actually recorded something.
 *
 * Recording has gaps, one of them four months long, so stepping a day at a time through the
 * calendar would mean clicking through weeks of nothing. The day navigation asks this instead,
 * and lands on the nearest day that exists. Windows are cached for the session because the
 * answer only changes when an ingest brings in a new day.
 */

import { api, type DaySummary } from "../api";
import { addDays, compareDates } from "./dates";

/** Wide enough to jump the longest recording gap on record in one hop, under the API's 400-day cap. */
const RADIUS_DAYS = 190;

type Window = { from: string; to: string; dates: string[] };

let cached: Window | null = null;
let pending: Promise<Window> | null = null;

export function resetDayIndex(): void {
  cached = null;
  pending = null;
}

function covers(window: Window | null, date: string): boolean {
  return window !== null && window.from <= date && date <= window.to;
}

async function load(date: string): Promise<Window> {
  const from = addDays(date, -RADIUS_DAYS);
  const to = addDays(date, RADIUS_DAYS);
  const summaries: DaySummary[] = await api.days(from, to);
  const window = { from, to, dates: summaries.map((summary) => summary.date) };
  cached = window;
  return window;
}

/** The window around `date`, fetched at most once per date range even when several views ask. */
export async function dayWindow(date: string): Promise<Window> {
  if (covers(cached, date)) return cached as Window;
  if (!pending) {
    pending = load(date).finally(() => {
      pending = null;
    });
  }
  const window = await pending;
  // A second caller may have been waiting on a window for a different date.
  return covers(window, date) ? window : load(date);
}

export type Neighbours = { previous: string | null; next: string | null };

export function neighbours(window: Window, date: string): Neighbours {
  let previous: string | null = null;
  let next: string | null = null;
  for (const candidate of window.dates) {
    if (compareDates(candidate, date) < 0) previous = candidate;
    else if (compareDates(candidate, date) > 0 && next === null) next = candidate;
  }
  return { previous, next };
}
