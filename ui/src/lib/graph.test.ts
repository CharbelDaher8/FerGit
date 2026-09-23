import { describe, expect, it } from "vitest";
import type { Edge, Relation, Row, RowKind } from "./bindings";
import {
  GRAPH_PADDING,
  LABEL_FONT,
  LABEL_GAP,
  LANE_WIDTH,
  LANE_COLORS,
  MAX_LABEL_WIDTH,
  MAX_SCROLL_HEIGHT,
  ROW_HEIGHT,
  drawGraph,
  fitText,
  graphWidth,
  labelX,
  labelY,
  laneX,
  lanesUsed,
  nextColumnWidth,
  laneToken,
  paletteColor,
  readLanePalette,
  relationLabels,
  rowGraphWidth,
  scrollMap,
  type GraphContext,
} from "./graph";

type Call = [string, ...unknown[]];

/** A 2D context that records method calls and property writes, in order. */
function recordingContext(): { ctx: GraphContext; calls: Call[] } {
  const calls: Call[] = [];
  const state: Record<string | symbol, unknown> = {};
  const ctx = new Proxy(state, {
    get: (target, key) => {
      if (key in target) return target[key];
      // Every character is 6px wide, so text widths are predictable.
      if (key === "measureText") return (text: string) => ({ width: text.length * CHAR_WIDTH });
      return (...args: unknown[]) => calls.push([String(key), ...args]);
    },
    set: (target, key, value) => {
      calls.push([`${String(key)}=`, value]);
      target[key] = value;
      return true;
    },
  }) as unknown as GraphContext;
  return { ctx, calls };
}

const CHAR_WIDTH = 6;
const measure = (text: string) => text.length * CHAR_WIDTH;

function row(kind: RowKind, column: number, color: number, edges: Edge[] = [], relations: Relation[] = []): Row {
  return {
    kind,
    id: "0".repeat(40),
    graph: { column, color, edges },
    summary: "",
    authorName: "",
    authorEmail: "",
    time: 0,
    refs: [],
    relations,
  };
}

const PALETTE = ["#l0", "#l1", "#l2", "#l3", "#l4", "#l5", "#l6", "#l7"];

function draw(rows: (Row | undefined)[], first = 0, offset = 0, options = { labels: false, width: 100 }): Call[] {
  const { ctx, calls } = recordingContext();
  drawGraph(ctx, { first, rows, offset, width: options.width, height: 400, palette: PALETTE, labels: options.labels });
  return calls;
}

const branchedFrom = (lane: number, branch: string | null, from: string | null): Relation => ({
  kind: "branchedFrom",
  lane,
  branch,
  from,
});
const merges = (lane: number, branch: string | null, into: string | null): Relation => ({
  kind: "merges",
  lane,
  branch,
  into,
});
const up = (from: number, to: number, color: number): Edge => ({ half: "upper", from, to, color });
const down = (from: number, to: number, color: number): Edge => ({ half: "lower", from, to, color });

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
    expect(paletteColor(PALETTE, 0)).toBe(PALETTE[0]);
    expect(paletteColor(PALETTE, PALETTE.length + 1)).toBe(PALETTE[1]);
    expect(laneToken(0)).toBe("--lane-0");
    expect(laneToken(LANE_COLORS + 2)).toBe("--lane-2");
  });

  it("reads the lane palette from the theme's tokens", () => {
    const style = { getPropertyValue: (name: string) => ` ${name.replace("--lane-", "#c")}` };
    const palette = readLanePalette(style);
    expect(palette).toHaveLength(LANE_COLORS);
    expect(palette[0]).toBe("#c0");
    expect(palette[LANE_COLORS - 1]).toBe(`#c${LANE_COLORS - 1}`);
  });
});

describe("relation labels: wording", () => {
  it("has no labels without relations", () => {
    expect(relationLabels([])).toEqual({ upper: null, lower: null });
  });

  it("names where one branch starts", () => {
    expect(relationLabels([branchedFrom(1, "topic", "main")])).toEqual({ upper: "branched from main", lower: null });
    expect(relationLabels([branchedFrom(1, "topic", null)]).upper).toBe("branch point");
  });

  it("lists several branches starting at one commit", () => {
    const labels = relationLabels([branchedFrom(1, "fix/a", "main"), branchedFrom(2, null, "main"), branchedFrom(3, "fix/c", null)]);
    expect(labels.upper).toBe("forks: fix/a, unnamed, fix/c");
  });

  it("describes one merge by what is known", () => {
    expect(relationLabels([merges(1, "feature", "main")])).toEqual({ upper: null, lower: "feature merged into main" });
    expect(relationLabels([merges(1, "feature", null)]).lower).toBe("merges feature");
    expect(relationLabels([merges(1, null, "main")]).lower).toBe("merge into main");
    expect(relationLabels([merges(1, null, null)]).lower).toBe("merge");
  });

  it("calls a merge of two or more branches an octopus merge", () => {
    expect(relationLabels([merges(1, "a", "main"), merges(2, null, "main"), merges(3, "c", "main")]).lower).toBe(
      "octopus merge of a, unnamed, c",
    );
  });

  it("labels both halves of a row that starts a branch and merges one", () => {
    expect(relationLabels([branchedFrom(1, "x", "main"), merges(2, "y", "main")])).toEqual({
      upper: "branched from main",
      lower: "y merged into main",
    });
  });
});

describe("relation labels: placement", () => {
  it("starts just right of the rightmost lane the row draws", () => {
    const wide = row("commit", 0, 0, [up(0, 0, 0), up(2, 0, 1), down(0, 0, 0)], [branchedFrom(2, "b", "main")]);
    expect(labelX(wide)).toBe(GRAPH_PADDING + 3 * LANE_WIDTH + LABEL_GAP);
    expect(labelX(wide)).toBeGreaterThan(laneX(2) + LANE_WIDTH / 2);
  });

  it("centers branch labels in the upper half and merge labels in the lower half", () => {
    expect(labelY("upper")).toBe(ROW_HEIGHT / 4);
    expect(labelY("lower")).toBe((ROW_HEIGHT * 3) / 4);
    expect(labelY("upper") + ROW_HEIGHT / 4).toBeLessThanOrEqual(labelY("lower") - ROW_HEIGHT / 4 + 0.001);
  });

  it("cuts text that doesn't fit with an ellipsis", () => {
    expect(fitText("merge", 100, measure)).toBe("merge");
    const cut = fitText("feature/login merged into main", 60, measure);
    expect(cut.endsWith("…")).toBe(true);
    expect(measure(cut)).toBeLessThanOrEqual(60);
    expect(cut).toBe("feature/l…");
    expect(fitText("merge", 3, measure)).toBe("");
  });

  it("widens the column for labels, capped, and not when labels are off", () => {
    const plain = row("commit", 0, 0, [up(0, 0, 0), down(0, 0, 0)]);
    expect(rowGraphWidth(plain, true, measure)).toBe(graphWidth(1));

    const merge = row("commit", 0, 0, [up(0, 0, 0), down(0, 0, 0), down(0, 1, 1)], [merges(1, "feature", "main")]);
    const text = measure("feature merged into main");
    expect(rowGraphWidth(merge, true, measure)).toBe(labelX(merge) + text + GRAPH_PADDING);
    expect(rowGraphWidth(merge, false, measure)).toBe(graphWidth(2));

    const long = row("commit", 0, 0, [down(0, 0, 0), down(0, 1, 1)], [merges(1, "x".repeat(200), null)]);
    expect(rowGraphWidth(long, true, measure)).toBe(labelX(long) + MAX_LABEL_WIDTH + GRAPH_PADDING);
  });

  it("grows the column within one generation and setting, and starts over on a new one", () => {
    let width = nextColumnWidth({ key: "", width: 44 }, "1:true", 120);
    expect(width).toEqual({ key: "1:true", width: 120 });
    width = nextColumnWidth(width, "1:true", 60); // narrower rows scrolled into view: no shrink
    expect(width.width).toBe(120);
    width = nextColumnWidth(width, "1:true", 180);
    expect(width.width).toBe(180);
    width = nextColumnWidth(width, "1:false", 44); // labels switched off
    expect(width).toEqual({ key: "1:false", width: 44 });
    expect(nextColumnWidth(width, "2:false", 0)).toBe(width); // nothing loaded yet: keep
  });
});

describe("relation labels: drawing", () => {
  const fork = () =>
    row("commit", 0, 0, [up(0, 0, 0), up(1, 0, 2), down(0, 0, 0)], [branchedFrom(1, "topic", "main")]);
  const merge = () =>
    row("commit", 0, 0, [up(0, 0, 0), down(0, 0, 0), down(0, 1, 3)], [merges(1, "feature", "main")]);

  it("draws nothing when labels are off", () => {
    expect(named(draw([fork()], 0, 0, { labels: false, width: 400 }), "fillText")).toHaveLength(0);
  });

  it("draws labels in the upper and lower halves, in the color of the line they describe", () => {
    const calls = draw([fork(), merge()], 2, 10, { labels: true, width: 400 });
    const texts = named(calls, "fillText");
    const x = GRAPH_PADDING + 2 * LANE_WIDTH + LABEL_GAP;
    expect(texts).toEqual([
      ["fillText", "branched from main", x, 2 * ROW_HEIGHT - 10 + ROW_HEIGHT / 4],
      ["fillText", "feature merged into main", x, 3 * ROW_HEIGHT - 10 + (ROW_HEIGHT * 3) / 4],
    ]);
    const colorBefore = (index: number) =>
      calls.slice(0, index).filter((call) => call[0] === "fillStyle=").pop()?.[1];
    expect(colorBefore(calls.indexOf(texts[0]))).toBe(PALETTE[2]);
    expect(colorBefore(calls.indexOf(texts[1]))).toBe(PALETTE[3]);
    expect(named(calls, "font=")).toEqual([["font=", LABEL_FONT]]);
    expect(named(calls, "globalAlpha=").pop()).toEqual(["globalAlpha=", 1]);
  });

  it("cuts labels at the canvas edge", () => {
    const x = GRAPH_PADDING + 2 * LANE_WIDTH + LABEL_GAP;
    const width = x + 60 + GRAPH_PADDING;
    const [text] = named(draw([merge()], 0, 0, { labels: true, width }), "fillText");
    expect(text[1]).toBe("feature m…");
    expect(measure(text[1] as string)).toBeLessThanOrEqual(60);
    expect((text[1] as string).endsWith("…")).toBe(true);
  });

  it("skips labels with no room at all", () => {
    expect(named(draw([merge()], 0, 0, { labels: true, width: 50 }), "fillText")).toHaveLength(0);
  });
});

describe("scrollMap", () => {
  it("is the identity for histories that fit", () => {
    const map = scrollMap(1000, 600);
    expect(map.height).toBe(1000 * ROW_HEIGHT);
    expect(map.toContent(1234)).toBe(1234);
    expect(map.toScroll(1234)).toBe(1234);
  });

  it("sizes content by the given item height", () => {
    expect(scrollMap(1000, 600, 20).height).toBe(20_000);
    const tall = scrollMap(1_000_000, 600, 20);
    expect(tall.height).toBe(MAX_SCROLL_HEIGHT);
    expect(tall.toContent(MAX_SCROLL_HEIGHT - 600)).toBeCloseTo(1_000_000 * 20 - 600, 3);
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
    expect(strokes.slice(0, 2)).toEqual([PALETTE[1], PALETTE[1]]);
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
    expect(named(commit, "fillStyle=")[0][1]).toBe(PALETTE[3]);

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
