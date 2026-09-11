import { describe, expect, it } from "vitest";
import {
  addDays,
  addMonths,
  addWeeks,
  daysBetween,
  isDate,
  isMonth,
  isoWeekDates,
  isoWeekday,
  isoWeekOf,
  isoWeekStart,
  isWeek,
  monthEnd,
  monthGrid,
  today,
  weeksInIsoYear,
} from "./dates";

describe("addDays", () => {
  it("crosses month and year boundaries", () => {
    expect(addDays("2026-01-31", 1)).toBe("2026-02-01");
    expect(addDays("2026-12-31", 1)).toBe("2027-01-01");
    expect(addDays("2026-01-01", -1)).toBe("2025-12-31");
  });

  it("handles leap years", () => {
    expect(addDays("2024-02-28", 1)).toBe("2024-02-29");
    expect(addDays("2025-02-28", 1)).toBe("2025-03-01");
  });

  it("does not lose a day to a DST transition", () => {
    // Europe/Berlin springs forward on 29 March 2026 and falls back on 25 October.
    expect(addDays("2026-03-28", 1)).toBe("2026-03-29");
    expect(addDays("2026-03-29", 1)).toBe("2026-03-30");
    expect(addDays("2026-10-25", 1)).toBe("2026-10-26");
  });
});

describe("daysBetween", () => {
  it("counts whole days in both directions", () => {
    expect(daysBetween("2026-09-07", "2026-09-14")).toBe(7);
    expect(daysBetween("2026-09-14", "2026-09-07")).toBe(-7);
    expect(daysBetween("2026-03-01", "2026-04-01")).toBe(31);
  });
});

describe("isDate", () => {
  it("rejects malformed and impossible dates", () => {
    expect(isDate("2026-09-07")).toBe(true);
    expect(isDate("2026-02-30")).toBe(false);
    expect(isDate("2026-13-01")).toBe(false);
    expect(isDate("2026-9-7")).toBe(false);
    expect(isDate("today")).toBe(false);
  });
});

describe("isoWeekday", () => {
  it("numbers Monday as 1 and Sunday as 7", () => {
    expect(isoWeekday("2026-09-07")).toBe(1);
    expect(isoWeekday("2026-09-13")).toBe(7);
  });
});

describe("today", () => {
  it("reads the wall-clock date, whatever the hour", () => {
    expect(today(new Date(2026, 8, 7, 22, 30))).toBe("2026-09-07");
    expect(today(new Date(2026, 0, 1, 0, 5))).toBe("2026-01-01");
  });
});

describe("isoWeekOf", () => {
  it("names the week a mid-week date falls in", () => {
    expect(isoWeekOf("2026-09-07")).toBe("2026-W37");
    expect(isoWeekOf("2026-09-13")).toBe("2026-W37");
  });

  it("puts early January into the previous year's last week", () => {
    expect(isoWeekOf("2021-01-01")).toBe("2020-W53");
    expect(isoWeekOf("2016-01-01")).toBe("2015-W53");
    expect(isoWeekOf("2023-01-01")).toBe("2022-W52");
  });

  it("puts late December into the next year's first week", () => {
    expect(isoWeekOf("2019-12-30")).toBe("2020-W01");
    expect(isoWeekOf("2024-12-30")).toBe("2025-W01");
    expect(isoWeekOf("2025-12-29")).toBe("2026-W01");
  });

  it("round-trips through the week's Monday", () => {
    for (const date of ["2020-12-31", "2021-01-04", "2026-01-01", "2026-12-31"]) {
      expect(isoWeekDates(isoWeekOf(date))).toContain(date);
    }
  });
});

describe("isoWeekStart", () => {
  it("returns the Monday of the week", () => {
    expect(isoWeekStart("2026-W37")).toBe("2026-09-07");
    expect(isoWeekStart("2020-W53")).toBe("2020-12-28");
    expect(isoWeekStart("2026-W01")).toBe("2025-12-29");
  });
});

describe("weeksInIsoYear", () => {
  it("knows the 53-week years", () => {
    expect(weeksInIsoYear(2020)).toBe(53);
    expect(weeksInIsoYear(2015)).toBe(53);
    expect(weeksInIsoYear(2026)).toBe(53);
    expect(weeksInIsoYear(2021)).toBe(52);
    expect(weeksInIsoYear(2025)).toBe(52);
  });
});

describe("addWeeks", () => {
  it("steps across a 53-week year end", () => {
    expect(addWeeks("2020-W52", 1)).toBe("2020-W53");
    expect(addWeeks("2020-W53", 1)).toBe("2021-W01");
    expect(addWeeks("2021-W01", -1)).toBe("2020-W53");
  });

  it("steps across a 52-week year end", () => {
    expect(addWeeks("2021-W52", 1)).toBe("2022-W01");
    expect(addWeeks("2022-W01", -1)).toBe("2021-W52");
  });
});

describe("isWeek", () => {
  it("rejects a week number the year does not have", () => {
    expect(isWeek("2026-W53")).toBe(true);
    expect(isWeek("2025-W53")).toBe(false);
    expect(isWeek("2026-W00")).toBe(false);
    expect(isWeek("2026-37")).toBe(false);
  });
});

describe("months", () => {
  it("validates and steps", () => {
    expect(isMonth("2026-09")).toBe(true);
    expect(isMonth("2026-13")).toBe(false);
    expect(addMonths("2026-12", 1)).toBe("2027-01");
    expect(addMonths("2026-01", -1)).toBe("2025-12");
  });

  it("finds the last day of the month", () => {
    expect(monthEnd("2026-09")).toBe("2026-09-30");
    expect(monthEnd("2024-02")).toBe("2024-02-29");
    expect(monthEnd("2025-02")).toBe("2025-02-28");
  });
});

describe("monthGrid", () => {
  it("lays the month out in Monday-first rows with padding left blank", () => {
    const grid = monthGrid("2026-09");
    expect(grid[0]).toEqual([
      null,
      "2026-09-01",
      "2026-09-02",
      "2026-09-03",
      "2026-09-04",
      "2026-09-05",
      "2026-09-06",
    ]);
    expect(grid.at(-1)?.filter(Boolean)).toEqual(["2026-09-28", "2026-09-29", "2026-09-30"]);
    expect(grid.flat().filter(Boolean)).toHaveLength(30);
  });

  it("needs six rows for a month that starts on a Sunday", () => {
    expect(monthGrid("2026-03")).toHaveLength(6);
  });
});
