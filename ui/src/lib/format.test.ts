import { describe, expect, it } from "vitest";
import type { Upstream } from "./bindings";
import { formatLocalTime, formatSignatureTime, shortId, upstreamSuffix, upstreamTitle } from "./format";

const tracking = (ahead: number, behind: number): Upstream => ({
  name: "origin/main",
  state: { kind: "tracking", ahead, behind, id: "0".repeat(40) },
});
const gone: Upstream = { name: "origin/main", state: { kind: "gone" } };

describe("upstream status", () => {
  it("shows ahead and behind counts, omitting zeros", () => {
    expect(upstreamSuffix(tracking(2, 1))).toBe("↑2 ↓1");
    expect(upstreamSuffix(tracking(3, 0))).toBe("↑3");
    expect(upstreamSuffix(tracking(0, 4))).toBe("↓4");
  });

  it("shows nothing when in sync or without an upstream, and 'gone' when it is gone", () => {
    expect(upstreamSuffix(tracking(0, 0))).toBe("");
    expect(upstreamSuffix(null)).toBe("");
    expect(upstreamSuffix(gone)).toBe("gone");
  });

  it("explains the status in the tooltip", () => {
    expect(upstreamTitle(tracking(2, 1))).toBe("2 ahead, 1 behind origin/main");
    expect(upstreamTitle(tracking(2, 0))).toBe("2 ahead of origin/main");
    expect(upstreamTitle(tracking(0, 1))).toBe("1 behind origin/main");
    expect(upstreamTitle(tracking(0, 0))).toBe("up to date with origin/main");
    expect(upstreamTitle(gone)).toBe("origin/main is gone");
  });
});

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
