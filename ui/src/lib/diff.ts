// The rendering model of a text diff: what each display row shows, with its line numbers. Pure,
// and sized for huge diffs: per-row bookkeeping lives in typed arrays, and row objects are only
// made for the rows asked for (the visible ones).

import type { Hunk, LineKind } from "./bindings";

/** Columns a tab advances to the next multiple of. The view uses the same `tab-size`. */
export const TAB_SIZE = 4;

/** The marker git prints after a line that ends its file without a newline. */
export const NO_NEWLINE_TEXT = "\\ No newline at end of file";

/** One display row of a text diff. */
export type DiffRow =
  | { kind: "hunk"; header: string }
  | {
      kind: "line";
      line: LineKind;
      text: string;
      /** Line number in the old version; `null` for added lines. */
      oldNumber: number | null;
      /** Line number in the new version; `null` for removed lines. */
      newNumber: number | null;
    }
  | { kind: "noNewline" };

const HUNK_ROW = -1;
const MARKER_ROW = -2;

/** `@@ -oldStart,oldLines +newStart,newLines @@`, omitting a count of 1 as git does. */
export function hunkHeader(hunk: Hunk): string {
  return `@@ -${range(hunk.oldStart, hunk.oldLines)} +${range(hunk.newStart, hunk.newLines)} @@`;
}

function range(start: number, lines: number): string {
  return lines === 1 ? String(start) : `${start},${lines}`;
}

/** Width of `text` in columns, with tabs expanded to `TAB_SIZE` stops. */
export function displayColumns(text: string): number {
  if (!text.includes("\t")) return text.length;
  let columns = 0;
  for (let i = 0; i < text.length; i++) {
    columns = text.charCodeAt(i) === 9 ? columns + TAB_SIZE - (columns % TAB_SIZE) : columns + 1;
  }
  return columns;
}

/**
 * The rows of a text diff: for every hunk a header row, then its lines, each followed by a
 * `noNewline` row where the line ends its version without a newline. Line numbers count from the
 * hunk's starts: context lines advance both, removed lines the old one, added lines the new one.
 */
export class DiffModel {
  /** Number of display rows. */
  readonly rowCount: number;
  /** Widest row in columns, for sizing horizontal scrolling. */
  readonly maxColumns: number;
  /** Largest line number shown, for sizing the gutters. */
  readonly maxLineNumber: number;

  readonly #hunks: readonly Hunk[];
  /** Per row: index of its hunk. */
  readonly #hunkOf: Int32Array;
  /** Per row: index of its line within the hunk, or HUNK_ROW / MARKER_ROW. */
  readonly #lineOf: Int32Array;
  /** Per row: old and new line numbers, 0 for none. */
  readonly #oldNumber: Int32Array;
  readonly #newNumber: Int32Array;

  constructor(hunks: readonly Hunk[]) {
    let count = 0;
    for (const hunk of hunks) {
      count += 1 + hunk.lines.length;
      for (const line of hunk.lines) if (line.noFinalNewline) count++;
    }
    this.#hunks = hunks;
    this.rowCount = count;
    this.#hunkOf = new Int32Array(count);
    this.#lineOf = new Int32Array(count);
    this.#oldNumber = new Int32Array(count);
    this.#newNumber = new Int32Array(count);

    let row = 0;
    let maxColumns = 0;
    let maxLineNumber = 0;
    for (let h = 0; h < hunks.length; h++) {
      const hunk = hunks[h];
      this.#hunkOf[row] = h;
      this.#lineOf[row] = HUNK_ROW;
      maxColumns = Math.max(maxColumns, hunkHeader(hunk).length);
      row++;

      let oldNumber = hunk.oldStart;
      let newNumber = hunk.newStart;
      for (let l = 0; l < hunk.lines.length; l++) {
        const line = hunk.lines[l];
        this.#hunkOf[row] = h;
        this.#lineOf[row] = l;
        if (line.kind !== "added") this.#oldNumber[row] = oldNumber++;
        if (line.kind !== "removed") this.#newNumber[row] = newNumber++;
        maxColumns = Math.max(maxColumns, displayColumns(line.text));
        row++;
        if (line.noFinalNewline) {
          this.#hunkOf[row] = h;
          this.#lineOf[row] = MARKER_ROW;
          maxColumns = Math.max(maxColumns, NO_NEWLINE_TEXT.length);
          row++;
        }
      }
      maxLineNumber = Math.max(maxLineNumber, oldNumber - 1, newNumber - 1);
    }
    this.maxColumns = maxColumns;
    this.maxLineNumber = maxLineNumber;
  }

  /** Display row `index`, which must be in `0..rowCount`. */
  row(index: number): DiffRow {
    const hunk = this.#hunks[this.#hunkOf[index]];
    const line = this.#lineOf[index];
    if (line === HUNK_ROW) return { kind: "hunk", header: hunkHeader(hunk) };
    if (line === MARKER_ROW) return { kind: "noNewline" };
    const diffLine = hunk.lines[line];
    return {
      kind: "line",
      line: diffLine.kind,
      text: diffLine.text,
      oldNumber: this.#oldNumber[index] || null,
      newNumber: this.#newNumber[index] || null,
    };
  }
}
