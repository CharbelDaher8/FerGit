import { describe, expect, it } from "vitest";
import type { Edge, Row, RowKind } from "./bindings";
import {
  DARK_PALETTE,
  GRAPH_PADDING,
  LANE_WIDTH,
  LIGHT_PALETTE,
  MAX_SCROLL_HEIGHT,
  ROW_HEIGHT,
  drawGraph,
  graphWidth,
  laneX,
  lanesUsed,
  paletteColor,
  scrollMap,
  type GraphContext,
} from "./graph";

type Call = [string, ...unknown[]];

/** A 2D context that records method calls and property writes, in order. */
function recordingContext(): { ctx: GraphContext; calls: Call[] } {
  const calls: Call[] = [];
  const state: Record<string | symbol, unknown> = {};
  const ctx = new Proxy(state, {
    get: (target, key) =>
      key in target ? target[key] : (...args: unknown[]) => calls.push([String(key), ...args]),
    set: (target, key, value) => {
      calls.push([`${String(key)}=`, value]);
      target[key] = value;
      return true;
    },
  }) as unknown as GraphContext;
  return { ctx, calls };
}

function row(kind: RowKind, column: number, color: number, edges: Edge[] = []): Row {
  return {
    kind,
    id: "0".repeat(40),
    graph: { column, color, edges },
    summary: "",
    authorName: "",
    authorEmail: "",
    time: 0,
    refs: [],
  };
}

function draw(rows: (Row | undefined)[], first = 0, offset = 0): Call[] {
  const { ctx, calls } = recordingContext();
  drawGraph(ctx, { first, rows, offset, width: 100, height: 400, palette: LIGHT_PALETTE });
  return calls;
}

const named = (calls: Call[], name: string) => calls.filter((call) => call[0] === name);

describe("geometry", () => {
  it("centers lanes after the padding", () => {
    expect(laneX(0)).toBe(GRAPH_PADDING + LANE_WIDTH / 2);
    expect(laneX(3)).toBe(GRAPH_PADDING + 3 * LANE_WIDTH + LANE_WIDTH / 2);
    expect(graphWidth(2)).toBe(2 * GRAPH_PADDING + 2 * LANE_WIDTH);
  });

  it("counts the lanes a row touches, edges included", () => {
    expect(lanesUsed(row("commit", 0, 0))).toBe(1);
    const merge = row("commit", 1, 0, [
      { half: "lower", from: 1, to: 3, color: 2 },
      { half: "upper", from: 0, to: 0, color: 0 },
    ]);
    expect(lanesUsed(merge)).toBe(4);
  });

  it("wraps color indices onto the palette", () => {
    expect(paletteColor(LIGHT_PALETTE, 0)).toBe(LIGHT_PALETTE[0]);
    expect(paletteColor(DARK_PALETTE, DARK_PALETTE.length + 1)).toBe(DARK_PALETTE[1]);
    expect(LIGHT_PALETTE).toHaveLength(DARK_PALETTE.length);
  });
});

describe("scrollMap", () => {
  it("is the identity for histories that fit", () => {
    const map = scrollMap(1000, 600);
    expect(map.height).toBe(1000 * ROW_HEIGHT);
    expect(map.toContent(1234)).toBe(1234);
    expect(map.toScroll(1234)).toBe(1234);
  });

  it("compresses tall histories so both ends stay reachable", () => {
    const rows = 2_000_000;
    const view = 600;
    const map = scrollMap(rows, view);
    expect(map.height).toBe(MAX_SCROLL_HEIGHT);
    expect(map.toContent(0)).toBe(0);
    expect(map.toContent(MAX_SCROLL_HEIGHT - view)).toBeCloseTo(rows * ROW_HEIGHT - view, 3);
    expect(map.toScroll(map.toContent(777_777))).toBeCloseTo(777_777, 6);
  });
});

describe("drawGraph", () => {
  it("clears the canvas and draws nothing for rows that aren't loaded", () => {
    const calls = draw([undefined, undefined]);
    expect(named(calls, "clearRect")).toEqual([["clearRect", 0, 0, 100, 400]]);
    expect(named(calls, "stroke")).toHaveLength(0);
    expect(named(calls, "fill")).toHaveLength(0);
  });

  it("draws a straight upper edge from the row's top to its center", () => {
    const calls = draw([row("commit", 0, 0, [{ half: "upper", from: 0, to: 0, color: 0 }])], 2);
    const top = 2 * ROW_HEIGHT;
    expect(named(calls, "moveTo")[0]).toEqual(["moveTo", laneX(0), top]);
    expect(named(calls, "lineTo")[0]).toEqual(["lineTo", laneX(0), top + ROW_HEIGHT / 2]);
  });

  it("draws a lane change as a curve with vertical tangents, shifted by the scroll offset", () => {
    const calls = draw([row("commit", 0, 0, [{ half: "upper", from: 1, to: 0, color: 0 }])], 3, 20);
    const top = 3 * ROW_HEIGHT - 20;
    const mid = top + ROW_HEIGHT / 2;
    const yMid = (top + mid) / 2;
    expect(named(calls, "moveTo")[0]).toEqual(["moveTo", laneX(1), top]);
    expect(named(calls, "bezierCurveTo")).toEqual([
      ["bezierCurveTo", laneX(1), yMid, laneX(0), yMid, laneX(0), mid],
    ]);
  });

  it("draws a lower edge from the center to the row's bottom", () => {
    const calls = draw([row("commit", 0, 0, [{ half: "lower", from: 0, to: 2, color: 0 }])], 1);
    const mid = ROW_HEIGHT + ROW_HEIGHT / 2;
    const bottom = 2 * ROW_HEIGHT;
    expect(named(calls, "moveTo")[0]).toEqual(["moveTo", laneX(0), mid]);
    expect(named(calls, "bezierCurveTo")[0].slice(-2)).toEqual([laneX(2), bottom]);
  });

  it("colors each edge from the palette", () => {
    const calls = draw([
      row("commit", 0, 0, [
        { half: "upper", from: 0, to: 0, color: 1 },
        { half: "lower", from: 0, to: 0, color: 9 },
      ]),
    ]);
    const strokes = named(calls, "strokeStyle=").map((call) => call[1]);
    expect(strokes.slice(0, 2)).toEqual([LIGHT_PALETTE[1], LIGHT_PALETTE[1]]);
  });

  it("draws nodes after all edges", () => {
    const edge: Edge = { half: "lower", from: 0, to: 0, color: 0 };
    const calls = draw([row("commit", 0, 0, [edge]), row("commit", 0, 0, [edge])]);
    const lastStroke = calls.map((call) => call[0]).lastIndexOf("stroke");
    const firstArc = calls.findIndex((call) => call[0] === "arc");
    expect(firstArc).toBeGreaterThan(lastStroke);
  });

  it("gives each row kind its own node shape at the lane center", () => {
    const mid = ROW_HEIGHT / 2;

    const commit = draw([row("commit", 2, 3)]);
    expect(named(commit, "arc")).toHaveLength(1);
    expect(named(commit, "arc")[0].slice(1, 3)).toEqual([laneX(2), mid]);
    expect(named(commit, "fill")).toHaveLength(1);
    expect(named(commit, "fillStyle=")[0][1]).toBe(LIGHT_PALETTE[3]);

    const workingTree = draw([row("workingTree", 0, 0)]);
    expect(named(workingTree, "globalCompositeOperation=").map((call) => call[1])).toEqual([
      "destination-out",
      "source-over",
    ]);
    expect(named(workingTree, "stroke")).toHaveLength(1); // a ring, not a dot

    const stash = draw([row("stash", 1, 0)]);
    expect(named(stash, "arc")).toHaveLength(0);
    expect(named(stash, "closePath")).toHaveLength(1);
    expect(named(stash, "moveTo")[0]).toEqual(["moveTo", laneX(1), expect.any(Number)]);
  });
});
