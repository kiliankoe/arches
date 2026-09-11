/**
 * Rendering figures and times for a reader who knows the data.
 *
 * Times are always shown in the offset the day was recorded in, never the browser's. A day spent
 * in Bangkok reads in Bangkok time whether it is browsed from there or from home, which is the
 * only reading that matches the memory.
 */

import { parseDate } from "./dates";

const WEEKDAYS = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
const MONTHS = [
  "January",
  "February",
  "March",
  "April",
  "May",
  "June",
  "July",
  "August",
  "September",
  "October",
  "November",
  "December",
];

/** Under a kilometre the metre is the honest unit; above it a single decimal is plenty. */
export function formatDistance(metres: number | null | undefined): string {
  if (metres == null || !Number.isFinite(metres)) return "";
  if (metres < 1000) return `${Math.round(metres)} m`;
  const km = metres / 1000;
  return `${km >= 100 ? Math.round(km) : km.toFixed(1)} km`;
}

/** "1 h 05 min", "18 min", "40 s". Minutes are zero-padded only when an hour precedes them. */
export function formatDuration(seconds: number | null | undefined): string {
  if (seconds == null || !Number.isFinite(seconds) || seconds < 0) return "";
  const total = Math.round(seconds);
  if (total < 60) return `${total} s`;
  const minutes = Math.round(total / 60);
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.floor(minutes / 60);
  return `${hours} h ${String(minutes % 60).padStart(2, "0")} min`;
}

/**
 * The wall-clock time an instant showed on the recording device. `offsetSeconds` comes from the
 * API; without one the instant is rendered in UTC rather than silently in the browser's zone.
 */
export function formatTime(
  timestamp: string | null | undefined,
  offsetSeconds: number | null | undefined,
): string {
  const shifted = shift(timestamp, offsetSeconds);
  if (!shifted) return "";
  return `${pad(shifted.getUTCHours())}:${pad(shifted.getUTCMinutes())}`;
}

/**
 * Minutes from a given day's local midnight, which is how the week view positions its blocks.
 * Negative for an item that started yesterday and past 1440 for one that runs into tomorrow, so
 * the caller can clip rather than guess.
 */
export function minutesFromMidnight(
  timestamp: string | null | undefined,
  offsetSeconds: number | null | undefined,
  date: string,
): number | null {
  const shifted = shift(timestamp, offsetSeconds);
  if (!shifted) return null;
  return (shifted.getTime() - Date.parse(`${date}T00:00:00Z`)) / 60_000;
}

function shift(
  timestamp: string | null | undefined,
  offsetSeconds: number | null | undefined,
): Date | null {
  if (!timestamp) return null;
  const at = new Date(timestamp);
  if (Number.isNaN(at.getTime())) return null;
  return new Date(at.getTime() + (offsetSeconds ?? 0) * 1000);
}

export function formatTimeRange(
  start: string | null | undefined,
  end: string | null | undefined,
  startOffset: number | null | undefined,
  endOffset: number | null | undefined,
): string {
  const from = formatTime(start, startOffset);
  const to = formatTime(end, endOffset);
  if (!from) return to;
  if (!to) return from;
  return `${from}–${to}`;
}

function pad(value: number): string {
  return String(value).padStart(2, "0");
}

export function formatDayTitle(date: string): string {
  const { year, month, day } = parseDate(date);
  const weekday = WEEKDAYS[(new Date(Date.UTC(year, month - 1, day)).getUTCDay() + 6) % 7];
  return `${weekday}, ${day} ${MONTHS[month - 1]} ${year}`;
}

export function formatWeekdayShort(date: string): string {
  const { year, month, day } = parseDate(date);
  return WEEKDAYS[(new Date(Date.UTC(year, month - 1, day)).getUTCDay() + 6) % 7].slice(0, 3);
}

/** The top bar has no room for a weekday name; "7 Sep 2026" is what fits. */
export function formatDayShort(date: string): string {
  const { year, month, day } = parseDate(date);
  return `${day} ${MONTHS[month - 1].slice(0, 3)} ${year}`;
}

export function formatMonthTitle(month: string): string {
  return `${MONTHS[Number(month.slice(5, 7)) - 1]} ${month.slice(0, 4)}`;
}

export function formatWeekTitle(week: string): string {
  return `Week ${Number(week.slice(6, 8))}, ${week.slice(0, 4)}`;
}

/** How long ago something happened, in the coarsest unit that still says something. */
export function formatRelative(
  timestamp: string | null | undefined,
  now: Date = new Date(),
): string {
  if (!timestamp) return "never";
  const at = new Date(timestamp);
  if (Number.isNaN(at.getTime())) return "never";
  const seconds = (now.getTime() - at.getTime()) / 1000;
  if (seconds < 0) return "just now";
  if (seconds < 90) return "just now";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours} h ago`;
  const days = Math.round(hours / 24);
  if (days < 31) return `${days} ${days === 1 ? "day" : "days"} ago`;
  const months = Math.round(days / 30);
  return `${months} ${months === 1 ? "month" : "months"} ago`;
}

/** "3 of 7 items unconfirmed", or nothing at all when there is nothing left to review. */
export function formatUnconfirmed(unconfirmed: number, total: number): string {
  if (unconfirmed <= 0) return "";
  return `${unconfirmed} of ${total} ${total === 1 ? "item" : "items"} unconfirmed`;
}

export function formatCount(count: number, singular: string, plural = `${singular}s`): string {
  return `${count} ${count === 1 ? singular : plural}`;
}
