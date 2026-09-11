import { describe, expect, it } from "vitest";
import {
  formatCount,
  formatDayShort,
  formatDayTitle,
  formatDistance,
  formatDuration,
  formatMonthTitle,
  formatRelative,
  formatTime,
  formatTimeRange,
  formatUnconfirmed,
  formatWeekdayShort,
  formatWeekTitle,
  minutesFromMidnight,
} from "./format";

describe("formatDistance", () => {
  it("switches from metres to kilometres at a kilometre", () => {
    expect(formatDistance(842)).toBe("842 m");
    expect(formatDistance(1000)).toBe("1.0 km");
    expect(formatDistance(5240)).toBe("5.2 km");
  });

  it("drops the decimal once the number is long", () => {
    expect(formatDistance(123_456)).toBe("123 km");
  });

  it("renders nothing for a missing figure", () => {
    expect(formatDistance(null)).toBe("");
    expect(formatDistance(undefined)).toBe("");
  });
});

describe("formatDuration", () => {
  it("reads as minutes below an hour", () => {
    expect(formatDuration(1080)).toBe("18 min");
    expect(formatDuration(45)).toBe("45 s");
  });

  it("pads the minutes once there are hours", () => {
    expect(formatDuration(3900)).toBe("1 h 05 min");
    expect(formatDuration(45_000)).toBe("12 h 30 min");
  });

  it("renders nothing for a missing or negative figure", () => {
    expect(formatDuration(null)).toBe("");
    expect(formatDuration(-5)).toBe("");
  });
});

describe("formatTime", () => {
  it("uses the day's offset, not the browser's timezone", () => {
    expect(formatTime("2026-09-07T06:05:00Z", 7200)).toBe("08:05");
    expect(formatTime("2026-09-07T06:05:00Z", 25_200)).toBe("13:05");
    expect(formatTime("2026-09-07T06:05:00Z", -18_000)).toBe("01:05");
  });

  it("falls back to UTC rather than to the browser", () => {
    expect(formatTime("2026-09-07T06:05:00Z", null)).toBe("06:05");
  });

  it("renders nothing without a timestamp", () => {
    expect(formatTime(null, 7200)).toBe("");
    expect(formatTime("not a date", 7200)).toBe("");
  });
});

describe("formatTimeRange", () => {
  it("joins both ends, each in its own offset", () => {
    // A flight that takes off in Berlin and lands in Bangkok.
    expect(formatTimeRange("2026-09-07T09:00:00Z", "2026-09-07T20:00:00Z", 7200, 25_200)).toBe(
      "11:00–03:00",
    );
  });

  it("shows the one end it has", () => {
    expect(formatTimeRange("2026-09-07T09:00:00Z", null, 0, 0)).toBe("09:00");
  });
});

describe("minutesFromMidnight", () => {
  it("counts from the given day's local midnight", () => {
    expect(minutesFromMidnight("2026-09-07T06:30:00Z", 7200, "2026-09-07")).toBe(510);
    expect(minutesFromMidnight("2026-09-06T22:00:00Z", 7200, "2026-09-07")).toBe(0);
  });

  it("goes negative before the day and past 1440 after it, so the caller can clip", () => {
    expect(minutesFromMidnight("2026-09-06T21:00:00Z", 7200, "2026-09-07")).toBe(-60);
    expect(minutesFromMidnight("2026-09-07T23:00:00Z", 7200, "2026-09-07")).toBe(1500);
  });

  it("has nothing to say without a timestamp", () => {
    expect(minutesFromMidnight(null, 0, "2026-09-07")).toBe(null);
  });
});

describe("titles", () => {
  it("spells the day, month and week out", () => {
    expect(formatDayTitle("2026-09-07")).toBe("Monday, 7 September 2026");
    expect(formatDayShort("2026-09-07")).toBe("7 Sep 2026");
    expect(formatWeekdayShort("2026-09-07")).toBe("Mon");
    expect(formatWeekdayShort("2026-09-13")).toBe("Sun");
    expect(formatMonthTitle("2026-09")).toBe("September 2026");
    expect(formatWeekTitle("2026-W37")).toBe("Week 37, 2026");
  });
});

describe("formatRelative", () => {
  const now = new Date("2026-09-07T12:00:00Z");

  it("coarsens as the gap grows", () => {
    expect(formatRelative("2026-09-07T11:59:30Z", now)).toBe("just now");
    expect(formatRelative("2026-09-07T11:30:00Z", now)).toBe("30 min ago");
    expect(formatRelative("2026-09-07T07:00:00Z", now)).toBe("5 h ago");
    expect(formatRelative("2026-09-06T12:00:00Z", now)).toBe("1 day ago");
    expect(formatRelative("2026-09-01T12:00:00Z", now)).toBe("6 days ago");
    expect(formatRelative("2026-06-07T12:00:00Z", now)).toBe("3 months ago");
  });

  it("says never when there is nothing to compare", () => {
    expect(formatRelative(null, now)).toBe("never");
  });
});

describe("counts", () => {
  it("stays quiet when everything is confirmed", () => {
    expect(formatUnconfirmed(0, 7)).toBe("");
    expect(formatUnconfirmed(3, 7)).toBe("3 of 7 items unconfirmed");
    expect(formatUnconfirmed(1, 1)).toBe("1 of 1 item unconfirmed");
  });

  it("pluralises", () => {
    expect(formatCount(1, "visit")).toBe("1 visit");
    expect(formatCount(4, "visit")).toBe("4 visits");
    expect(formatCount(2, "country", "countries")).toBe("2 countries");
  });
});
