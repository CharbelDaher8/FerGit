import { describe, expect, it } from "vitest";
import { formatLocalTime, formatSignatureTime, shortId } from "./format";

describe("format", () => {
  it("shortens ids to 8 characters", () => {
    expect(shortId("0123456789abcdef0123456789abcdef01234567")).toBe("01234567");
  });

  it("formats signature times in the signer's own offset", () => {
    // 2026-09-15T12:03:22Z
    const seconds = Date.UTC(2026, 8, 15, 12, 3, 22) / 1000;
    expect(formatSignatureTime(seconds, 120)).toBe("2026-09-15 14:03:22 +0200");
    expect(formatSignatureTime(seconds, -330)).toBe("2026-09-15 06:33:22 -0530");
    expect(formatSignatureTime(seconds, 0)).toBe("2026-09-15 12:03:22 +0000");
  });

  it("formats row dates compactly in local time", () => {
    const seconds = new Date(2026, 0, 5, 9, 7).getTime() / 1000;
    expect(formatLocalTime(seconds)).toBe("2026-01-05 09:07");
  });
});
