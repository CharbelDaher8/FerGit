// Geometry and drawing for the commit graph. Pure: no DOM, no state. The view passes in the
// visible rows, scroll offset and palette and gets pixels on a 2D context.

import type { Relation, Row, RowKind } from "./bindings";

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

/** Relationship labels: small and italic, since the branch names in them are inferred. */
export const LABEL_FONT = 'italic 10.5px system-ui, -apple-system, "Segoe UI", Roboto, sans-serif';
/** Space between the rightmost lane a row draws and its labels. */
export const LABEL_GAP = 2;
/** Labels wider than this are cut with an ellipsis. */
export const MAX_LABEL_WIDTH = 220;
/** Labels with less room than this aren't drawn at all. */
const MIN_LABEL_WIDTH = 12;
/** Labels are drawn in their lane's color at this opacity, to stay secondary to the lines. */
const LABEL_ALPHA = 0.8;
const ELLIPSIS = "…";

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

/** A row's relationship labels: the branch label for its upper half, the merge label for its lower. */
export interface RowLabels {
  upper: string | null;
  lower: string | null;
}

const NO_LABELS: RowLabels = { upper: null, lower: null };

type BranchedFrom = Extract<Relation, { kind: "branchedFrom" }>;
type Merges = Extract<Relation, { kind: "merges" }>;

/**
 * The wording of a row's relations. Branches starting here label the upper half ("branched from
 * main", "branch point", or "forks: a, b" for several); merges label the lower half ("feature
 * merged into main", "merges feature", "merge into main", "merge", or "octopus merge of a, b, c").
 */
export function relationLabels(relations: readonly Relation[]): RowLabels {
  if (relations.length === 0) return NO_LABELS;
  const forks = relations.filter((relation): relation is BranchedFrom => relation.kind === "branchedFrom");
  const merges = relations.filter((relation): relation is Merges => relation.kind === "merges");
  return { upper: forkLabel(forks), lower: mergeLabel(merges) };
}

function forkLabel(forks: readonly BranchedFrom[]): string | null {
  if (forks.length === 0) return null;
  if (forks.length > 1) return `forks: ${forks.map((fork) => fork.branch ?? "unnamed").join(", ")}`;
  return forks[0].from === null ? "branch point" : `branched from ${forks[0].from}`;
}

function mergeLabel(merges: readonly Merges[]): string | null {
  if (merges.length === 0) return null;
  if (merges.length > 1) return `octopus merge of ${merges.map((merge) => merge.branch ?? "unnamed").join(", ")}`;
  const { branch, into } = merges[0];
  if (branch !== null && into !== null) return `${branch} merged into ${into}`;
  if (branch !== null) return `merges ${branch}`;
  return into !== null ? `merge into ${into}` : "merge";
}

/** Left edge of a row's labels: just right of the rightmost lane the row draws, clear of its lines. */
export function labelX(row: Row): number {
  return GRAPH_PADDING + lanesUsed(row) * LANE_WIDTH + LABEL_GAP;
}

/** Vertical center of a label within its row: the middle of the upper or the lower half. */
export function labelY(half: "upper" | "lower"): number {
  return half === "upper" ? ROW_HEIGHT / 4 : (ROW_HEIGHT * 3) / 4;
}

/**
 * `text` shortened to fit `maxWidth` as `measure`d, ending in an ellipsis when cut; empty when not
 * even the ellipsis fits.
 */
export function fitText(text: string, maxWidth: number, measure: (text: string) => number): string {
  if (measure(text) <= maxWidth) return text;
  if (measure(ELLIPSIS) > maxWidth) return "";
  // The longest prefix that still fits with the ellipsis.
  let low = 0;
  let high = text.length - 1;
  while (low < high) {
    const mid = Math.ceil((low + high) / 2);
    if (measure(text.slice(0, mid) + ELLIPSIS) <= maxWidth) low = mid;
    else high = mid - 1;
  }
  return text.slice(0, low).trimEnd() + ELLIPSIS;
}

/**
 * Width of graph column `row` needs: its lanes, plus its labels (capped at `MAX_LABEL_WIDTH`) when
 * labels are shown. `measure` gives a label's width in `LABEL_FONT`.
 */
export function rowGraphWidth(row: Row, labels: boolean, measure: (text: string) => number): number {
  const lanes = graphWidth(lanesUsed(row));
  if (!labels || row.relations.length === 0) return lanes;
  const { upper, lower } = relationLabels(row.relations);
  const text = Math.max(upper === null ? 0 : measure(upper), lower === null ? 0 : measure(lower));
  return Math.max(lanes, Math.ceil(labelX(row) + Math.min(text, MAX_LABEL_WIDTH) + GRAPH_PADDING));
}

/** A graph column width and what it was computed for. */
export interface ColumnWidth {
  /** Identifies the generation and label setting the width belongs to. */
  key: string;
  width: number;
}

/**
 * The graph column's next width, given the widest visible row (`widest`, 0 while nothing is loaded).
 * Within one key it only grows, so scrolling never makes the text columns jump; a new key (a new
 * generation, or labels toggled) starts over from the visible rows. While nothing is loaded the
 * previous width is kept.
 */
export function nextColumnWidth(previous: ColumnWidth, key: string, widest: number): ColumnWidth {
  if (widest === 0) return previous;
  if (key !== previous.key) return { key, width: widest };
  return widest > previous.width ? { key, width: widest } : previous;
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
  | "fillText"
  | "measureText"
  | "fillStyle"
  | "strokeStyle"
  | "lineWidth"
  | "lineCap"
  | "font"
  | "textBaseline"
  | "globalAlpha"
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
  /** Whether to draw relationship labels. */
  labels: boolean;
}

/**
 * Draws one frame: all edges of the scene's rows, then their nodes on top, then (if enabled) their
 * relationship labels. The context's transform must map CSS pixels to canvas pixels.
 *
 * Within a row spanning `top..top + ROW_HEIGHT` with center `mid`, an upper edge runs from
 * `(laneX(from), top)` to `(laneX(to), mid)` and a lower edge from `(laneX(from), mid)` to
 * `(laneX(to), top + ROW_HEIGHT)`. Edges that change lanes are curves with vertical tangents at
 * both ends, so they join the straight segments of neighbouring rows smoothly.
 *
 * Labels start at `labelX(row)`, centered in the upper half (branches) or lower half (merges), in
 * the color of the line they describe, and are cut with an ellipsis at `MAX_LABEL_WIDTH` or the
 * canvas edge.
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

  if (!scene.labels) return;
  const measure = (text: string) => ctx.measureText(text).width;
  ctx.font = LABEL_FONT;
  ctx.textBaseline = "middle";
  ctx.globalAlpha = LABEL_ALPHA;
  for (let k = 0; k < rows.length; k++) {
    const row = rows[k];
    if (!row || row.relations.length === 0) continue;
    const x = labelX(row);
    const maxWidth = Math.min(MAX_LABEL_WIDTH, scene.width - x - GRAPH_PADDING);
    if (maxWidth < MIN_LABEL_WIDTH) continue;
    const top = (first + k) * ROW_HEIGHT - offset;
    const { upper, lower } = relationLabels(row.relations);
    if (upper !== null) {
      ctx.fillStyle = paletteColor(palette, relationColor(row, "branchedFrom"));
      ctx.fillText(fitText(upper, maxWidth, measure), x, top + labelY("upper"));
    }
    if (lower !== null) {
      ctx.fillStyle = paletteColor(palette, relationColor(row, "merges"));
      ctx.fillText(fitText(lower, maxWidth, measure), x, top + labelY("lower"));
    }
  }
  ctx.globalAlpha = 1;
}

/**
 * Color index of the line the row's first relation of `kind` describes: the upper edge coming in
 * from its lane for a branch, the lower edge going out to its lane for a merge; the node's color if
 * no such edge exists.
 */
function relationColor(row: Row, kind: Relation["kind"]): number {
  const relation = row.relations.find((candidate) => candidate.kind === kind);
  if (!relation) return row.graph.color;
  const { column } = row.graph;
  const edge = row.graph.edges.find((candidate) =>
    kind === "branchedFrom"
      ? candidate.half === "upper" && candidate.from === relation.lane && candidate.to === column
      : candidate.half === "lower" && candidate.from === column && candidate.to === relation.lane,
  );
  return edge ? edge.color : row.graph.color;
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
