import { describe, expect, it } from "vitest";
import {
  ACTIVITY_COLORS,
  ACTIVITY_KEYS,
  activityColor,
  activityKey,
  activityLabel,
  activityMatchExpression,
  paletteCssVariables,
  VISIT_COLOR,
} from "./activity";

describe("the palette", () => {
  it("has one distinct colour per key", () => {
    const colors = Object.values(ACTIVITY_COLORS);
    expect(colors).toHaveLength(ACTIVITY_KEYS.length);
    expect(new Set(colors).size).toBe(colors.length);
  });

  it("keeps visits out of the activity hues", () => {
    expect(Object.values(ACTIVITY_COLORS)).not.toContain(VISIT_COLOR);
  });
});

describe("activityKey", () => {
  it("passes known types through and funnels the rest into other", () => {
    expect(activityKey("tram")).toBe("tram");
    expect(activityKey("songthaew")).toBe("other");
    expect(activityKey(null)).toBe("other");
  });
});

describe("activityColor", () => {
  it("draws a visit's own type in ink rather than a hue", () => {
    expect(activityColor("stationary")).toBe(VISIT_COLOR);
    expect(activityColor("cycling")).toBe(ACTIVITY_COLORS.cycling);
  });
});

describe("activityLabel", () => {
  it("is sentence case and splits Arc's camelCase names", () => {
    expect(activityLabel("cycling")).toBe("Cycling");
    expect(activityLabel("skateboarding")).toBe("Skateboarding");
    expect(activityLabel("horsebackRiding")).toBe("Horseback riding");
    expect(activityLabel(null)).toBe("Trip");
  });
});

describe("activityMatchExpression", () => {
  it("matches every hue and falls back to other", () => {
    const expression = activityMatchExpression();
    expect(expression[0]).toBe("match");
    expect(expression).toContain("tram");
    expect(expression).toContain(ACTIVITY_COLORS.tram);
    expect(expression.at(-1)).toBe(ACTIVITY_COLORS.other);
    // "other" is the fallback, never a case of its own.
    expect(expression).not.toContain("other");
  });
});

describe("paletteCssVariables", () => {
  it("names a variable per key plus the visit ink", () => {
    const variables = paletteCssVariables();
    expect(variables["--activity-car"]).toBe(ACTIVITY_COLORS.car);
    expect(variables["--visit"]).toBe(VISIT_COLOR);
    expect(Object.keys(variables)).toHaveLength(ACTIVITY_KEYS.length + 1);
  });
});
