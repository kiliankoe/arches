import { describe, expect, it } from "vitest";
import { neighbours } from "./dayIndex";

const window = {
  from: "2026-08-01",
  to: "2026-09-30",
  dates: ["2026-08-30", "2026-09-05", "2026-09-07", "2026-09-20"],
};

describe("neighbours", () => {
  it("skips the gap on either side of a day that exists", () => {
    expect(neighbours(window, "2026-09-07")).toEqual({
      previous: "2026-09-05",
      next: "2026-09-20",
    });
  });

  it("still navigates from a day with no data", () => {
    expect(neighbours(window, "2026-09-10")).toEqual({
      previous: "2026-09-07",
      next: "2026-09-20",
    });
  });

  it("reports the ends of the window as dead ends", () => {
    expect(neighbours(window, "2026-08-30").previous).toBe(null);
    expect(neighbours(window, "2026-09-20").next).toBe(null);
  });
});
