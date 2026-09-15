import { describe, expect, it } from "vitest";
import type { DiffLine, Hunk, LineKind } from "./bindings";
import { DiffModel, NO_NEWLINE_TEXT, displayColumns, hunkHeader, type DiffRow } from "./diff";

function line(kind: LineKind, text: string, noFinalNewline = false): DiffLine {
  return { kind, text, noFinalNewline };
}

function hunk(oldStart: number, oldLines: number, newStart: number, newLines: number, lines: DiffLine[]): Hunk {
  return { oldStart, oldLines, newStart, newLines, lines };
}

function rows(model: DiffModel): DiffRow[] {
  return Array.from({ length: model.rowCount }, (_, i) => model.row(i));
}

/** Each row as a compact string: `old new kind text`, `@@` headers, and `\` markers. */
function sketch(model: DiffModel): string[] {
  return rows(model).map((row) => {
    if (row.kind === "hunk") return row.header;
    if (row.kind === "noNewline") return "\\";
    return `${row.oldNumber ?? "-"} ${row.newNumber ?? "-"} ${row.line} ${row.text}`;
  });
}

describe("hunkHeader", () => {
  it("formats ranges like git, omitting counts of one", () => {
    expect(hunkHeader(hunk(5, 3, 5, 4, []))).toBe("@@ -5,3 +5,4 @@");
    expect(hunkHeader(hunk(1, 1, 1, 1, []))).toBe("@@ -1 +1 @@");
    expect(hunkHeader(hunk(0, 0, 1, 2, []))).toBe("@@ -0,0 +1,2 @@");
  });
});

describe("DiffModel", () => {
  it("numbers lines from each hunk's starts", () => {
    const model = new DiffModel([
      hunk(3, 4, 3, 5, [
        line("context", "a"),
        line("removed", "b"),
        line("added", "B1"),
        line("added", "B2"),
        line("context", "c"),
        line("context", "d"),
      ]),
      hunk(20, 3, 21, 2, [line("context", "x"), line("removed", "y"), line("context", "z")]),
    ]);
    expect(sketch(model)).toEqual([
      "@@ -3,4 +3,5 @@",
      "3 3 context a",
      "4 - removed b",
      "- 4 added B1",
      "- 5 added B2",
      "5 6 context c",
      "6 7 context d",
      "@@ -20,3 +21,2 @@",
      "20 21 context x",
      "21 - removed y",
      "22 22 context z",
    ]);
    expect(model.maxLineNumber).toBe(22);
  });

  it("numbers a new file's lines from 1 with no old numbers", () => {
    const model = new DiffModel([hunk(0, 0, 1, 2, [line("added", "one"), line("added", "two")])]);
    expect(sketch(model)).toEqual(["@@ -0,0 +1,2 @@", "- 1 added one", "- 2 added two"]);
  });

  it("puts a no-newline marker right after each line that ends its version without one", () => {
    const model = new DiffModel([
      hunk(1, 2, 1, 2, [
        line("context", "same"),
        line("removed", "old end", true),
        line("added", "new end", true),
      ]),
    ]);
    expect(sketch(model)).toEqual([
      "@@ -1,2 +1,2 @@",
      "1 1 context same",
      "2 - removed old end",
      "\\",
      "- 2 added new end",
      "\\",
    ]);
    expect(model.rowCount).toBe(6);
    expect(model.maxColumns).toBe(NO_NEWLINE_TEXT.length);
  });

  it("marks a context line without a final newline once", () => {
    const model = new DiffModel([hunk(4, 1, 4, 1, [line("context", "last", true)])]);
    expect(sketch(model)).toEqual(["@@ -4 +4 @@", "4 4 context last", "\\"]);
  });

  it("measures the widest row with tabs expanded", () => {
    expect(displayColumns("abc")).toBe(3);
    expect(displayColumns("\tab")).toBe(6);
    expect(displayColumns("ab\tc")).toBe(5);
    const model = new DiffModel([hunk(1, 1, 1, 1, [line("context", "\t\tlong line here")])]);
    expect(model.maxColumns).toBe(8 + "long line here".length);
  });

  it("has no rows for no hunks", () => {
    const model = new DiffModel([]);
    expect(model.rowCount).toBe(0);
    expect(model.maxLineNumber).toBe(0);
  });

  it("indexes huge hunks without walking them per row", () => {
    const count = 300_000;
    const lines = Array.from({ length: count }, (_, i) => line(i % 2 ? "added" : "context", `line ${i}`));
    const model = new DiffModel([hunk(1, count / 2, 1, count, lines)]);
    expect(model.rowCount).toBe(count + 1);
    // Row 200001 is line 200000 (context): the 100001st context line in both versions, after 100000 added.
    expect(model.row(200_001)).toEqual({
      kind: "line",
      line: "context",
      text: "line 200000",
      oldNumber: 100_001,
      newNumber: 200_001,
    });
    expect(model.maxLineNumber).toBe(count);
  });
});
