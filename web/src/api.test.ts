import { describe, expect, it } from "vitest";
import { formatVersion } from "./api";

describe("formatVersion", () => {
  it("shows a placeholder before the status arrives", () => {
    expect(formatVersion(null)).toBe("connecting…");
  });

  it("prefixes the version once loaded", () => {
    expect(formatVersion({ version: "0.1.0", ok: true })).toBe("v0.1.0");
  });
});
