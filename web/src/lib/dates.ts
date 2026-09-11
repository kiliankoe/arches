/**
 * Date arithmetic on local `YYYY-MM-DD` strings.
 *
 * A day in arches is a local day decided by the recording device's offset, which has nothing to
 * do with the browser's timezone. So nothing here ever constructs a `Date` from a local
 * wall-clock string: every calculation goes through `Date.UTC`, where a day is exactly 86400 s
 * and no DST shift can move a date across midnight. The one exception is [`today`], which by
 * definition asks the browser what day it is.
 */

const DAY_MS = 86_400_000;

export type DateParts = { year: number; month: number; day: number };

const DATE_PATTERN = /^\d{4}-\d{2}-\d{2}$/;

export function isDate(value: string): boolean {
  if (!DATE_PATTERN.test(value)) return false;
  // `Date.UTC` rolls 30 February over into March, so a round trip is what catches the impossible.
  return fromEpoch(epoch(value)) === value;
}

export function parseDate(date: string): DateParts {
  return {
    year: Number(date.slice(0, 4)),
    month: Number(date.slice(5, 7)),
    day: Number(date.slice(8, 10)),
  };
}

export function toDate({ year, month, day }: DateParts): string {
  return `${pad(year, 4)}-${pad(month, 2)}-${pad(day, 2)}`;
}

function pad(value: number, width: number): string {
  return String(value).padStart(width, "0");
}

/** Midnight UTC of the date, the anchor every calculation here counts from. */
function epoch(date: string): number {
  const { year, month, day } = parseDate(date);
  return Date.UTC(year, month - 1, day);
}

function fromEpoch(millis: number): string {
  const at = new Date(millis);
  return toDate({
    year: at.getUTCFullYear(),
    month: at.getUTCMonth() + 1,
    day: at.getUTCDate(),
  });
}

export function addDays(date: string, days: number): string {
  return fromEpoch(epoch(date) + days * DAY_MS);
}

export function daysBetween(from: string, to: string): number {
  return Math.round((epoch(to) - epoch(from)) / DAY_MS);
}

export function compareDates(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0;
}

/** 1 for Monday through 7 for Sunday, the ISO numbering the week and month grids are built on. */
export function isoWeekday(date: string): number {
  return ((new Date(epoch(date)).getUTCDay() + 6) % 7) + 1;
}

/** The browser's idea of today, the only place a local timezone legitimately decides a date. */
export function today(now: Date = new Date()): string {
  return toDate({
    year: now.getFullYear(),
    month: now.getMonth() + 1,
    day: now.getDate(),
  });
}

// Months

export type MonthParts = { year: number; month: number };

const MONTH_PATTERN = /^\d{4}-(0[1-9]|1[0-2])$/;

export function isMonth(value: string): boolean {
  return MONTH_PATTERN.test(value);
}

export function parseMonth(month: string): MonthParts {
  return { year: Number(month.slice(0, 4)), month: Number(month.slice(5, 7)) };
}

export function toMonth({ year, month }: MonthParts): string {
  return `${pad(year, 4)}-${pad(month, 2)}`;
}

export function monthOf(date: string): string {
  return date.slice(0, 7);
}

export function addMonths(month: string, months: number): string {
  const { year, month: index } = parseMonth(month);
  const total = year * 12 + (index - 1) + months;
  return toMonth({ year: Math.floor(total / 12), month: (total % 12) + 1 });
}

export function monthStart(month: string): string {
  return `${month}-01`;
}

export function monthEnd(month: string): string {
  return addDays(monthStart(addMonths(month, 1)), -1);
}

/** The month as whole Monday-to-Sunday rows, with the days outside the month left out. */
export function monthGrid(month: string): (string | null)[][] {
  const first = monthStart(month);
  const last = monthEnd(month);
  const weeks: (string | null)[][] = [];
  let cursor = addDays(first, 1 - isoWeekday(first));
  while (cursor <= last) {
    const week: (string | null)[] = [];
    for (let i = 0; i < 7; i++) {
      const date = addDays(cursor, i);
      week.push(date >= first && date <= last ? date : null);
    }
    weeks.push(week);
    cursor = addDays(cursor, 7);
  }
  return weeks;
}

// ISO weeks

const WEEK_PATTERN = /^\d{4}-W(0[1-9]|[1-4]\d|5[0-3])$/;

export function isWeek(value: string): boolean {
  if (!WEEK_PATTERN.test(value)) return false;
  const { year, week } = parseWeek(value);
  return week <= weeksInIsoYear(year);
}

export function parseWeek(week: string): { year: number; week: number } {
  return { year: Number(week.slice(0, 4)), week: Number(week.slice(6, 8)) };
}

export function toWeek(year: number, week: number): string {
  return `${pad(year, 4)}-W${pad(week, 2)}`;
}

/**
 * The ISO-8601 week a date falls in, which is not always in the date's own year: the week
 * belongs to whichever year holds its Thursday.
 */
export function isoWeekOf(date: string): string {
  const thursday = addDays(date, 4 - isoWeekday(date));
  const year = parseDate(thursday).year;
  const week = Math.floor(daysBetween(isoWeekStart(toWeek(year, 1)), date) / 7) + 1;
  return toWeek(year, week);
}

/** The Monday of an ISO week. Week 1 is the one containing 4 January. */
export function isoWeekStart(week: string): string {
  const { year, week: index } = parseWeek(week);
  const fourth = `${pad(year, 4)}-01-04`;
  const firstMonday = addDays(fourth, 1 - isoWeekday(fourth));
  return addDays(firstMonday, (index - 1) * 7);
}

export function isoWeekDates(week: string): string[] {
  const monday = isoWeekStart(week);
  return Array.from({ length: 7 }, (_, i) => addDays(monday, i));
}

/** 52 or 53, so stepping past the last week of a year lands on week 1 of the next. */
export function weeksInIsoYear(year: number): number {
  const lastDay = `${pad(year, 4)}-12-28`; // 28 December is always in the last ISO week.
  return parseWeek(isoWeekOf(lastDay)).week;
}

export function addWeeks(week: string, weeks: number): string {
  return isoWeekOf(addDays(isoWeekStart(week), weeks * 7));
}
