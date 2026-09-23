import { beforeEach, describe, expect, it } from "vitest";
import type { RepoInfo, Row, RowLocation, RowsPage } from "./bindings";
import { ROW_HEIGHT } from "./graph";
import {
  RepoView,
  captureAnchor,
  followSelection,
  restoreAnchor,
  rowWindow,
  OVERSCAN,
  type Anchor,
} from "./view.svelte";

/**
 * A fake backend holding one snapshot (a generation and the ids of its rows, in order). Commands
 * answer from whatever snapshot is current when they are called, like the real one.
 */
const backend = {
  generation: 1,
  ids: [] as string[],
  /** Replaces `locate`'s answer, to simulate races. */
  locate: null as null | ((id: string) => RowLocation),
  locateCalls: [] as string[],
};

const client = {
  rows: async (start: number, len: number): Promise<RowsPage> => {
    const total = backend.ids.length;
    const from = Math.min(start, total);
    const rows = backend.ids.slice(from, from + len).map(makeRow);
    return { generation: backend.generation, start: from, total, rows };
  },
  locate: async (id: string): Promise<RowLocation> => {
    backend.locateCalls.push(id);
    if (backend.locate) return backend.locate(id);
    const row = backend.ids.indexOf(id);
    return { generation: backend.generation, row: row < 0 ? null : row };
  },
};

/** The id of the uncommitted-changes row. */
const ZERO = "0".repeat(40);

function makeRow(id: string): Row {
  return {
    kind: id === ZERO ? "workingTree" : "commit",
    id,
    graph: { column: 0, color: 0, edges: [] },
    summary: id,
    authorName: "Ada",
    authorEmail: "ada@example.com",
    time: 0,
    refs: [],
    relations: [],
  };
}

function ids(count: number, prefix = "c"): string[] {
  return Array.from({ length: count }, (_, i) => `${prefix}${i}`);
}

/** Makes `rowIds` the backend's current snapshot and returns what `refresh` would. */
function snapshot(generation: number, rowIds: string[]): RepoInfo {
  backend.generation = generation;
  backend.ids = rowIds;
  return { root: "/repo", name: "repo", head: null, filter: { refs: [], path: null }, branch: "main", state: { kind: "clean" }, generation, rowCount: rowIds.length };
}

/** Lets pending commands, their follow-ups and re-anchoring run. */
async function settle(): Promise<void> {
  for (let i = 0; i < 10; i++) await new Promise((resolve) => setTimeout(resolve, 0));
}

const HEIGHT = 10 * ROW_HEIGHT;

/** A view of 100 commits scrolled `offset` px down, with its rows loaded. */
async function openView(offset: number, selected: number | null): Promise<RepoView> {
  const view = new RepoView(snapshot(1, ids(100)), client);
  view.setViewport(offset, HEIGHT);
  await settle();
  view.select(selected);
  return view;
}

beforeEach(() => {
  backend.locate = null;
  backend.locateCalls = [];
});

describe("anchor math", () => {
  const anchor = (offset: number, selected: number | null, compared: number | null = null): Anchor =>
    captureAnchor(offset, selected, compared, (index) => makeRow(`c${index}`));

  it("captures the top row, the offset within it, the selection and the compared row", () => {
    expect(anchor(70, 12, 30)).toEqual({
      offset: 70,
      topIndex: 2,
      topId: "c2",
      selected: 12,
      selectedId: "c12",
      compared: 30,
      comparedId: "c30",
    });
    const unloaded = captureAnchor(70, 12, 30, () => undefined);
    expect(unloaded.topId).toBeNull();
    expect(unloaded.selectedId).toBeNull();
    expect(unloaded.comparedId).toBeNull();
    expect(captureAnchor(70, null, null, () => undefined).selectedId).toBeNull();
  });

  it("moves the compared row with its commit, and clears it with the commit or the selection", () => {
    expect(restoreAnchor(anchor(70, 12, 30), 2, 15, 33).compared).toBe(33);
    expect(restoreAnchor(anchor(70, 12, 30), 2, 15, null).compared).toBeNull();
    expect(restoreAnchor(anchor(70, 12, 30), 2, 15, undefined).compared).toBe(30);
    expect(restoreAnchor(anchor(70, 12, 30), 2, null, 33).compared).toBeNull();
  });

  it("keeps the top row's pixel offset", () => {
    // The viewport's top was 14px into row 2; that row moved to index 5.
    expect(restoreAnchor(anchor(70, null), 5, undefined).offset).toBe(5 * ROW_HEIGHT + 14);
  });

  it("stays pinned to the very top", () => {
    expect(restoreAnchor(anchor(0, null), 3, undefined).offset).toBe(0);
  });

  it("keeps the offset when the top row is gone or unknown", () => {
    expect(restoreAnchor(anchor(70, null), null, undefined).offset).toBe(70);
    expect(restoreAnchor(anchor(70, null), undefined, undefined).offset).toBe(70);
  });

  it("moves the selection with its row, clears it when the row is gone, keeps it when unknown", () => {
    expect(restoreAnchor(anchor(70, 12), 2, 15).selected).toBe(15);
    expect(restoreAnchor(anchor(70, 12), 2, null).selected).toBeNull();
    expect(restoreAnchor(anchor(70, 12), 2, undefined).selected).toBe(12);
    expect(restoreAnchor(anchor(70, null), 2, undefined).selected).toBeNull();
  });

  it("follows the selection to its new row, keeping it where it was on screen", () => {
    // Selected row 8 sat 8 * ROW_HEIGHT - 70 px below the viewport's top.
    const onScreen = 8 * ROW_HEIGHT - 70;
    expect(followSelection(anchor(70, 8), 40, HEIGHT)).toBe(40 * ROW_HEIGHT - onScreen);
    // Near the top the offset can't go negative.
    expect(followSelection(anchor(70, 8), 1, HEIGHT)).toBe(0);
  });

  it("pulls a selection that was off screen into view", () => {
    // Row 50 was below a viewport showing rows 0..9: it lands on the viewport's last row.
    expect(followSelection(anchor(0, 50), 60, HEIGHT)).toBe(61 * ROW_HEIGHT - HEIGHT);
    // Row 1 was above a viewport scrolled far down: it lands on the first row.
    expect(followSelection(anchor(80 * ROW_HEIGHT, 1), 30, HEIGHT)).toBe(30 * ROW_HEIGHT);
  });

  it("has nothing to follow without a selection or its row", () => {
    expect(followSelection(anchor(70, null), 3, HEIGHT)).toBeNull();
    expect(followSelection(anchor(70, 12), null, HEIGHT)).toBeNull();
    expect(followSelection(anchor(70, 12), undefined, HEIGHT)).toBeNull();
  });

  it("computes the row window with overscan, clamped", () => {
    expect(rowWindow(70, HEIGHT, 100)).toEqual({ first: 0, end: 13 + OVERSCAN });
    expect(rowWindow(90 * ROW_HEIGHT, HEIGHT, 100)).toEqual({ first: 90 - OVERSCAN, end: 100 });
  });

  it("computes windows for other item heights and overscans (the diff view's lines)", () => {
    // 20px lines, 400px viewport scrolled to 1010px: lines 50..71 are visible, plus 30 on each side.
    expect(rowWindow(1010, 400, 100_000, 20, 30)).toEqual({ first: 20, end: 101 });
    expect(rowWindow(0, 400, 10, 20, 30)).toEqual({ first: 0, end: 10 });
    expect(rowWindow(0, 0, 0, 20, 30)).toEqual({ first: 0, end: 0 });
  });
});

describe("RepoView re-anchoring", () => {
  it("keeps the top row and the selection on the same commits when commits are added", async () => {
    const view = await openView(70, 10);
    view.adopt(snapshot(2, ["new1", "new0", ...ids(100)]));

    // Until the new rows are in place, the old ones stay on screen.
    expect(view.row(2)?.id).toBe("c2");
    expect(view.total).toBe(100);

    await settle();
    expect(view.total).toBe(102);
    expect(view.generation).toBe(2);
    expect(view.row(12)?.id).toBe("c10");
    expect(view.selected).toBe(12);
    expect(view.scrollRequest).toEqual({ offset: 70 + 2 * ROW_HEIGHT });
  });

  it("shows new commits when scrolled to the top", async () => {
    const view = await openView(0, 5);
    view.adopt(snapshot(2, ["new0", ...ids(100)]));
    await settle();
    expect(view.scrollRequest).toEqual({ offset: 0 });
    expect(view.row(0)?.id).toBe("new0");
    expect(view.selected).toBe(6);
  });

  it("clears the selection when its commit is gone", async () => {
    const view = await openView(70, 10);
    view.adopt(snapshot(2, ids(100).filter((id) => id !== "c10")));
    await settle();
    expect(view.selected).toBeNull();
    expect(view.scrollRequest).toEqual({ offset: 70 });
  });

  it("keeps the offset when the top row is gone", async () => {
    const view = await openView(70, 10);
    view.adopt(snapshot(2, ["new0", ...ids(100).filter((id) => id !== "c2")]));
    await settle();
    expect(view.scrollRequest).toEqual({ offset: 70 });
    expect(view.selected).toBe(10);
  });

  it("ignores locations from an older generation, and re-anchors on the next change", async () => {
    const view = await openView(70, 10);
    backend.locate = () => ({ generation: 1, row: 0 });
    view.adopt(snapshot(2, ["new0", ...ids(100)]));
    await settle();
    expect(view.selected).toBe(10);
    expect(view.scrollRequest).toBeNull();
    expect(view.row(10)?.id).toBe("c10"); // still showing the old rows

    backend.locate = null;
    view.adopt(snapshot(3, ["new1", "new0", ...ids(100)]));
    await settle();
    expect(view.generation).toBe(3);
    expect(view.selected).toBe(12);
    expect(view.scrollRequest).toEqual({ offset: 70 + 2 * ROW_HEIGHT });
  });

  it("ignores locations from a newer generation until that generation arrives", async () => {
    const view = await openView(70, 10);
    const info = snapshot(2, ["new0", ...ids(100)]);
    backend.locate = () => ({ generation: 3, row: 40 });
    view.adopt(info);
    await settle();
    expect(view.selected).toBe(10);
    expect(view.scrollRequest).toBeNull();
  });

  it("anchors on what is on screen when the repository changes again while settling", async () => {
    const view = await openView(70, 10);
    const second = snapshot(2, ["new0", ...ids(100)]);
    const third = snapshot(3, ["new2", "new1", "new0", ...ids(100)]);
    view.adopt(second);
    view.adopt(third);
    await settle();
    expect(view.generation).toBe(3);
    expect(view.row(13)?.id).toBe("c10");
    expect(view.selected).toBe(13);
    expect(view.scrollRequest).toEqual({ offset: 70 + 3 * ROW_HEIGHT });
  });

  it("re-anchors when a page reveals the change before any event does", async () => {
    const view = new RepoView(snapshot(1, ids(1000)), client);
    view.setViewport(0, HEIGHT);
    await settle();
    view.select(10);
    snapshot(2, ["new0", ...ids(1000)]);
    view.setViewport(450 * ROW_HEIGHT, HEIGHT); // needs a page that isn't cached
    await settle();
    expect(view.generation).toBe(2);
    expect(view.selected).toBe(11);
  });

  it("keeps the selection in view when asked, as when a filter is applied and cleared", async () => {
    // Rows 0..99, scrolled to show 40..49, row 45 selected.
    const view = await openView(40 * ROW_HEIGHT, 45);
    // A filter keeps every fifth commit: c45 is now row 9.
    view.followSelectionOnNextChange();
    view.adopt(snapshot(2, ids(100).filter((_, i) => i % 5 === 0)));
    await settle();
    expect(view.selected).toBe(9);
    expect(view.scrollRequest).toEqual({ offset: 4 * ROW_HEIGHT });

    // Clearing it: c45 goes back to row 45, in the same place on screen.
    view.followSelectionOnNextChange();
    view.adopt(snapshot(3, ids(100)));
    await settle();
    expect(view.selected).toBe(45);
    expect(view.scrollRequest).toEqual({ offset: 40 * ROW_HEIGHT });
  });

  it("follows the selection only for the change it was asked for", async () => {
    const view = await openView(70, 10);
    view.followSelectionOnNextChange();
    view.followSelectionOnNextChange(false);
    view.adopt(snapshot(2, ["new1", "new0", ...ids(100)]));
    await settle();
    expect(view.scrollRequest).toEqual({ offset: 70 + 2 * ROW_HEIGHT });
  });

  it("does nothing on a refresh with the same generation", async () => {
    const view = await openView(70, 10);
    view.adopt({ root: "/repo", name: "repo", head: null, filter: { refs: [], path: null }, branch: "main", state: { kind: "clean" }, generation: 1, rowCount: 100 });
    await settle();
    expect(backend.locateCalls).toEqual([]);
    expect(view.scrollRequest).toBeNull();
  });
});

describe("RepoView compare mode", () => {
  it("compares the selection with another loaded commit row", async () => {
    const view = await openView(0, null);
    expect(view.compare(5)).toBe(false); // nothing selected
    view.select(2);
    expect(view.compare(2)).toBe(false); // the selection itself
    expect(view.compare(5)).toBe(true);
    expect(view.compared).toBe(5);
    expect(view.comparison).toEqual({ older: 5, newer: 2 });

    view.select(8);
    view.compare(3);
    expect(view.comparison).toEqual({ older: 8, newer: 3 });
  });

  it("leaves compare mode on a plain selection or compare(null)", async () => {
    const view = await openView(0, 2);
    view.compare(5);
    view.select(4);
    expect(view.compared).toBeNull();
    expect(view.comparison).toBeNull();
    view.compare(6);
    view.compare(null);
    expect(view.comparison).toBeNull();
    expect(view.selected).toBe(4);
  });

  it("doesn't compare uncommitted changes", async () => {
    const view = new RepoView(snapshot(1, [ZERO, ...ids(20)]), client);
    view.setViewport(0, HEIGHT);
    await settle();
    view.select(0);
    expect(view.compare(3)).toBe(false);
    view.select(3);
    expect(view.compare(0)).toBe(false);
    expect(view.compared).toBeNull();
  });

  it("keeps both compared commits when commits are added", async () => {
    const view = await openView(70, 10);
    view.compare(20);
    view.adopt(snapshot(2, ["new1", "new0", ...ids(100)]));
    await settle();
    expect(view.selected).toBe(12);
    expect(view.compared).toBe(22);
    expect(view.row(22)?.id).toBe("c20");
  });

  it("stops comparing when the compared commit is gone", async () => {
    const view = await openView(70, 10);
    view.compare(20);
    view.adopt(snapshot(2, ids(100).filter((id) => id !== "c20")));
    await settle();
    expect(view.selected).toBe(10);
    expect(view.compared).toBeNull();
  });
});

describe("RepoView keyboard movement", () => {
  it("moves like the arrow keys, starting at the top visible row without a selection", async () => {
    const view = await openView(3 * ROW_HEIGHT, null);
    view.moveSelection(1);
    expect(view.selected).toBe(3);
    view.moveSelection(5);
    expect(view.selected).toBe(8);
    view.moveSelection(-20);
    expect(view.selected).toBe(0);
    expect(view.scrollRequest).toEqual({ offset: 0 }); // revealed
  });

  it("moves by pages and half pages of the viewport", async () => {
    const view = await openView(0, 10); // 10 rows visible: a page is 9 rows, half a page 5
    view.pageSelection(1);
    expect(view.selected).toBe(19);
    view.pageSelection(-0.5);
    expect(view.selected).toBe(14);
    view.pageSelection(0.5);
    expect(view.selected).toBe(19);
    view.pageSelection(-1);
    expect(view.selected).toBe(10);
  });

  it("jumps to a row or the last row, clamped, and reveals it", async () => {
    const view = await openView(0, 1);
    view.selectRow("last");
    expect(view.selected).toBe(99);
    expect(view.scrollRequest).toEqual({ offset: 100 * ROW_HEIGHT - HEIGHT });
    view.selectRow(19);
    expect(view.selected).toBe(19);
    view.selectRow(5000);
    expect(view.selected).toBe(99);
    view.selectRow(0);
    expect(view.scrollRequest).toEqual({ offset: 0 });
  });

  it("leaves compare mode, like any keyboard selection", async () => {
    const view = await openView(0, 2);
    view.compare(5);
    view.moveSelection(1);
    expect(view.compared).toBeNull();
    expect(view.selected).toBe(3);
  });
});

describe("RepoView navigation", () => {
  it("goes to a commit: selects its row and scrolls it into view", async () => {
    const view = await openView(0, 1);
    await expect(view.goTo("c50")).resolves.toBe(true);
    expect(view.selected).toBe(50);
    expect(view.scrollRequest).toEqual({ offset: 51 * ROW_HEIGHT - HEIGHT });
  });

  it("scrolls up to a commit above the viewport", async () => {
    const view = await openView(40 * ROW_HEIGHT, 45);
    await view.goTo("c3");
    expect(view.scrollRequest).toEqual({ offset: 3 * ROW_HEIGHT });
  });

  it("doesn't scroll to a commit already in view", async () => {
    const view = await openView(0, 1);
    await view.goTo("c4");
    expect(view.selected).toBe(4);
    expect(view.scrollRequest).toBeNull();
  });

  it("reports a commit no row shows, and leaves the selection alone", async () => {
    const view = await openView(0, 1);
    await expect(view.find("missing")).resolves.toBeNull();
    await expect(view.goTo("missing")).resolves.toBe(false);
    expect(view.selected).toBe(1);
  });

  it("doesn't trust a location from another generation", async () => {
    const view = await openView(0, 1);
    backend.locate = () => ({ generation: 7, row: 20 });
    await expect(view.find("c20")).resolves.toBeNull();
    await expect(view.goTo("c20")).resolves.toBe(false);
    expect(view.selected).toBe(1);
  });

  it("reveals a selection made before the viewport was measured, once it is", async () => {
    const view = new RepoView(snapshot(1, ids(100)), client);
    view.select(60, true);
    expect(view.scrollRequest).toBeNull();
    view.setViewport(0, HEIGHT);
    expect(view.scrollRequest).toEqual({ offset: 61 * ROW_HEIGHT - HEIGHT });
  });
});
