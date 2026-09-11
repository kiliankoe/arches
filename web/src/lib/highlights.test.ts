import { describe, expect, it } from "vitest";
import type { Highlight } from "../api";
import { byMonth, highlightSearch, parseHighlightRange } from "./highlights";

const TODAY = "2026-09-11";

function event(date: string, title: string): Highlight {
  return { kind: "country", date, title, confirmed: true };
}

describe("parseHighlightRange", () => {
  it("reads both ends from the URL", () => {
    const params = new URLSearchParams("from=2025-06-01&to=2025-06-30");
    expect(parseHighlightRange(params, TODAY)).toEqual({ from: "2025-06-01", to: "2025-06-30" });
  });

  /** The endpoint refuses an open range, so a missing end is the default one, not nothing. */
  it("falls back to the last twelve months", () => {
    expect(parseHighlightRange(new URLSearchParams(), TODAY)).toEqual({
      from: "2025-09-11",
      to: TODAY,
    });
    expect(parseHighlightRange(new URLSearchParams("from=2026-01-01"), TODAY)).toEqual({
      from: "2026-01-01",
      to: TODAY,
    });
  });

  it("round trips through the query string", () => {
    const range = { from: "2025-06-01", to: "2025-06-30" };
    const search = highlightSearch(range);
    expect(search).toBe("?from=2025-06-01&to=2025-06-30");
    expect(parseHighlightRange(new URLSearchParams(search), TODAY)).toEqual(range);
  });
});

describe("byMonth", () => {
  it("groups consecutive events and keeps their order", () => {
    const grouped = byMonth([
      event("2025-06-10", "First time in Germany"),
      event("2025-06-12", "First time in Czechia"),
      event("2025-07-01", "First time in Portugal"),
    ]);

    expect(grouped.map(({ month }) => month)).toEqual(["2025-06", "2025-07"]);
    expect(grouped[0].events.map(({ title }) => title)).toEqual([
      "First time in Germany",
      "First time in Czechia",
    ]);
  });

  it("has nothing to group when there is nothing", () => {
    expect(byMonth([])).toEqual([]);
  });
});
