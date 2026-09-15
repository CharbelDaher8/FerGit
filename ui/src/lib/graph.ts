// Geometry and drawing for the commit graph. Pure: no DOM, no state. The view passes in the
// visible rows, scroll offset and palette and gets pixels on a 2D context.

import type { Row, RowKind } from "./bindings";

/** Height of one row, in CSS pixels. Rows in the DOM and on the canvas share it. */
export const ROW_HEIGHT = 28;
/** Width of one lane. */
export const LANE_WIDTH = 16;
/** Space left of lane 0 and right of the last lane. */
export const GRAPH_PADDING = 6;

const LINE_WIDTH = 2;
const NODE_RADIUS = 4.5;
/** Half-diagonal of the diamond that marks a stash. */
const STASH_RADIUS = 5.5;

/**
 * Tallest scrollable height the view creates. Browsers can't lay out elements much taller than
 * ~33 million pixels, so beyond this the scrollbar is scaled (see `scrollMap`).
 */
export const MAX_SCROLL_HEIGHT = 16_000_000;

/** Lane colors on a light background. Index with `paletteColor`. */
export const LIGHT_PALETTE: readonly string[] = [
  "#0969da", // blue
  "#1a7f37", // green
  "#c2410c", // orange
  "#8250df", // purple
  "#cf222e", // red
  "#0e7c86", // teal
  "#9a6700", // ochre
  "#bf3989", // pink
];

/** Lane colors on a dark background. */
export const DARK_PALETTE: readonly string[] = [
  "#58a6ff",
  "#3fb950",
  "#f0883e",
  "#bc8cff",
  "#ff7b72",
  "#39c5cf",
  "#d29922",
  "#f778ba",
];

/** The palette entry for a backend color index, which is unbounded. */
export function paletteColor(palette: readonly string[], color: number): string {
  return palette[color % palette.length];
}

/** Horizontal center of lane `column`. */
export function laneX(column: number): number {
  return GRAPH_PADDING + column * LANE_WIDTH + LANE_WIDTH / 2;
}

/** Width of a graph column that fits `lanes` lanes. */
export function graphWidth(lanes: number): number {
  return GRAPH_PADDING * 2 + lanes * LANE_WIDTH;
}

/** Number of lanes `row` touches: one more than the rightmost lane of its node or edges. */
export function lanesUsed(row: Row): number {
  let widest = row.graph.column;
  for (const edge of row.graph.edges) widest = Math.max(widest, edge.from, edge.to);
  return widest + 1;
}

/**
 * Maps between a scroll container's `scrollTop` and the content offset (item index × item height,
 * `ROW_HEIGHT` unless given) it stands for. The two are equal unless the content is taller than
 * `MAX_SCROLL_HEIGHT`, in which case the scroll range is compressed linearly so that its ends still
 * reach the first and last items.
 */
export interface ScrollMap {
  /** Height to give the scrollable content. */
  readonly height: number;
  toContent(scrollTop: number): number;
  toScroll(contentOffset: number): number;
}

export function scrollMap(rowCount: number, viewHeight: number, itemHeight = ROW_HEIGHT): ScrollMap {
  const contentHeight = rowCount * itemHeight;
  if (contentHeight <= MAX_SCROLL_HEIGHT || viewHeight >= MAX_SCROLL_HEIGHT) {
    return { height: contentHeight, toContent: (top) => top, toScroll: (offset) => offset };
  }
  const ratio = (contentHeight - viewHeight) / (MAX_SCROLL_HEIGHT - viewHeight);
  return {
    height: MAX_SCROLL_HEIGHT,
    toContent: (top) => top * ratio,
    toScroll: (offset) => offset / ratio,
  };
}

/** The parts of a canvas 2D context the graph uses (so tests can record the calls). */
export type GraphContext = Pick<
  CanvasRenderingContext2D,
  | "clearRect"
  | "beginPath"
  | "closePath"
  | "moveTo"
  | "lineTo"
  | "bezierCurveTo"
  | "arc"
  | "fill"
  | "stroke"
  | "fillStyle"
  | "strokeStyle"
  | "lineWidth"
  | "lineCap"
  | "globalCompositeOperation"
>;

/** Everything one frame of the graph depends on. */
export interface GraphScene {
  /** Row index of `rows[0]`. */
  first: number;
  /** Consecutive rows from `first`; `undefined` for rows not loaded yet, which draw nothing. */
  rows: readonly (Row | undefined)[];
  /** Content offset of the canvas's top edge: row `i` starts at `i * ROW_HEIGHT - offset`. */
  offset: number;
  /** Canvas size in CSS pixels. */
  width: number;
  height: number;
  palette: readonly string[];
}

/**
 * Draws one frame: all edges of the scene's rows, then their nodes on top. The context's transform
 * must map CSS pixels to canvas pixels.
 *
 * Within a row spanning `top..top + ROW_HEIGHT` with center `mid`, an upper edge runs from
 * `(laneX(from), top)` to `(laneX(to), mid)` and a lower edge from `(laneX(from), mid)` to
 * `(laneX(to), top + ROW_HEIGHT)`. Edges that change lanes are curves with vertical tangents at
 * both ends, so they join the straight segments of neighbouring rows smoothly.
 */
export function drawGraph(ctx: GraphContext, scene: GraphScene): void {
  const { first, rows, offset, palette } = scene;
  ctx.clearRect(0, 0, scene.width, scene.height);
  ctx.lineWidth = LINE_WIDTH;
  ctx.lineCap = "round";

  for (let k = 0; k < rows.length; k++) {
    const row = rows[k];
    if (!row) continue;
    const top = (first + k) * ROW_HEIGHT - offset;
    const mid = top + ROW_HEIGHT / 2;
    for (const edge of row.graph.edges) {
      ctx.strokeStyle = paletteColor(palette, edge.color);
      ctx.beginPath();
      if (edge.half === "upper") segment(ctx, laneX(edge.from), top, laneX(edge.to), mid);
      else segment(ctx, laneX(edge.from), mid, laneX(edge.to), top + ROW_HEIGHT);
      ctx.stroke();
    }
  }

  for (let k = 0; k < rows.length; k++) {
    const row = rows[k];
    if (!row) continue;
    const mid = (first + k) * ROW_HEIGHT - offset + ROW_HEIGHT / 2;
    node(ctx, row.kind, laneX(row.graph.column), mid, paletteColor(palette, row.graph.color));
  }
}

function segment(ctx: GraphContext, x1: number, y1: number, x2: number, y2: number): void {
  ctx.moveTo(x1, y1);
  if (x1 === x2) {
    ctx.lineTo(x2, y2);
  } else {
    const yMid = (y1 + y2) / 2;
    ctx.bezierCurveTo(x1, yMid, x2, yMid, x2, y2);
  }
}

/** Commits are solid dots, uncommitted changes a hollow ring, stashes a diamond. */
function node(ctx: GraphContext, kind: RowKind, x: number, y: number, color: string): void {
  if (kind === "workingTree") {
    // Punch out the lines under the ring so it reads as hollow over any row background.
    ctx.globalCompositeOperation = "destination-out";
    ctx.beginPath();
    ctx.arc(x, y, NODE_RADIUS + 1, 0, 2 * Math.PI);
    ctx.fill();
    ctx.globalCompositeOperation = "source-over";
    ctx.strokeStyle = color;
    ctx.beginPath();
    ctx.arc(x, y, NODE_RADIUS, 0, 2 * Math.PI);
    ctx.stroke();
  } else if (kind === "stash") {
    ctx.fillStyle = color;
    ctx.beginPath();
    ctx.moveTo(x, y - STASH_RADIUS);
    ctx.lineTo(x + STASH_RADIUS, y);
    ctx.lineTo(x, y + STASH_RADIUS);
    ctx.lineTo(x - STASH_RADIUS, y);
    ctx.closePath();
    ctx.fill();
  } else {
    ctx.fillStyle = color;
    ctx.beginPath();
    ctx.arc(x, y, NODE_RADIUS, 0, 2 * Math.PI);
    ctx.fill();
  }
}
