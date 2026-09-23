import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Row, RowsPage } from "./bindings";
import { MAX_IN_FLIGHT, PAGE_SIZE, RowStore } from "./rows.svelte";

interface PendingRequest {
  start: number;
  len: number;
  resolve: (page: RowsPage) => void;
}

/** A fake backend whose `rows` requests stay pending until a test answers them. */
const backend = vi.hoisted(() => ({ requests: [] as PendingRequest[] }));

vi.mock("./bindings", () => ({
  commands: {
    rows: (start: number, len: number) =>
      new Promise<RowsPage>((resolve) => backend.requests.push({ start, len, resolve })),
  },
}));

function makeRow(index: number, generation: number): Row {
  return {
    kind: "commit",
    id: `g${generation}-r${index}`,
    graph: { column: 0, color: 0, edges: [] },
    summary: `row ${index}`,
    authorName: "Ada",
    authorEmail: "ada@example.com",
    time: 0,
    refs: [],
    relations: [],
  };
}

/** Answers `request` from a snapshot with `total` rows, clamping like the backend does. */
async function answer(request: PendingRequest, generation: number, total: number): Promise<void> {
  const start = Math.min(request.start, total);
  const rows: Row[] = [];
  for (let i = start; i < Math.min(total, start + request.len); i++) rows.push(makeRow(i, generation));
  request.resolve({ generation, start, total, rows });
  await settle();
}

async function answerAll(generation: number, total: number): Promise<void> {
  const requests = backend.requests.splice(0);
  for (const request of requests) await answer(request, generation, total);
}

/** Lets resolved promises (and the store's follow-up requests) run. */
function settle(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

function requestedStarts(): number[] {
  return backend.requests.map((request) => request.start);
}

function store(generation: number, rowCount: number): RowStore {
  return new RowStore({ root: "/repo", name: "repo", head: null, branch: "main", state: { kind: "clean" }, generation, rowCount });
}

beforeEach(() => {
  backend.requests.length = 0;
});

describe("RowStore paging", () => {
  it("fetches nothing until a viewport is set", () => {
    store(1, 1000);
    expect(backend.requests).toHaveLength(0);
  });

  it("fetches the visible page plus one page ahead when scrolling down", () => {
    const rows = store(1, 1000);
    rows.setViewport(0, 40);
    expect(requestedStarts()).toEqual([0, PAGE_SIZE]);
    expect(backend.requests.every((request) => request.len === PAGE_SIZE)).toBe(true);
  });

  it("fetches the page above when scrolling up", async () => {
    const rows = store(1, 5000);
    rows.setViewport(2000, 2040);
    await answerAll(1, 5000);
    rows.setViewport(1990, 2030);
    // Visible page first (1800..2000), then the prefetch above it.
    expect(requestedStarts()).toEqual([1800, 1600]);
  });

  it("never requests a page that is already in flight", () => {
    const rows = store(1, 1000);
    rows.setViewport(0, 40);
    rows.setViewport(0, 40);
    rows.setViewport(10, 50);
    expect(requestedStarts()).toEqual([0, PAGE_SIZE]);
  });

  it("serves rows once their page arrives", async () => {
    const rows = store(1, 1000);
    rows.setViewport(0, 40);
    expect(rows.get(5)).toBeUndefined();
    await answer(backend.requests[0], 1, 1000);
    expect(rows.get(5)?.summary).toBe("row 5");
    expect(rows.get(250)).toBeUndefined();
    expect(rows.get(-1)).toBeUndefined();
  });

  it("does not re-request cached pages", async () => {
    const rows = store(1, 1000);
    rows.setViewport(0, 40);
    await answerAll(1, 1000);
    rows.setViewport(20, 60);
    expect(backend.requests).toHaveLength(0);
  });

  it("handles a short last page", async () => {
    const rows = store(1, 250);
    rows.setViewport(200, 250);
    expect(requestedStarts()).toEqual([200]);
    await answerAll(1, 250);
    expect(rows.get(249)?.summary).toBe("row 249");
    expect(rows.get(250)).toBeUndefined();
  });

  it("evicts pages far from the viewport", async () => {
    const rows = store(1, 100_000);
    rows.setViewport(0, 40);
    await answerAll(1, 100_000);
    expect(rows.get(0)).toBeDefined();
    rows.setViewport(50_000, 50_040);
    expect(rows.get(0)).toBeUndefined();
  });

  it("limits requests in flight and plans the next one against the current viewport", async () => {
    const rows = store(1, 100_000);
    rows.setViewport(0, 40); // pages 0 and 1
    rows.setViewport(20_000, 20_040); // room for one more: page 100
    rows.setViewport(40_000, 40_040); // no room
    expect(requestedStarts()).toEqual([0, 200, 20_000]);
    expect(backend.requests).toHaveLength(MAX_IN_FLIGHT);

    await answer(backend.requests.shift()!, 1, 100_000);
    // The freed slot goes to what is visible now, not to the pages scrolled past.
    expect(requestedStarts()).toEqual([200, 20_000, 40_000]);
  });

  it("resolves whenLoaded once the range is cached", async () => {
    const rows = store(1, 1000);
    rows.setViewport(0, 40);
    let loaded = false;
    void rows.whenLoaded(190, 210).then(() => (loaded = true));
    await answer(backend.requests.shift()!, 1, 1000); // page 0 only
    expect(loaded).toBe(false);
    await answer(backend.requests.shift()!, 1, 1000); // page 1
    expect(loaded).toBe(true);
    await expect(rows.whenLoaded(1000, 1200)).resolves.toBeUndefined(); // past the end: nothing to wait for
  });
});

describe("RowStore generations", () => {
  it("drops the cache and adopts the total when a page comes from a newer generation", async () => {
    const rows = store(1, 1000);
    rows.setViewport(0, 40);
    await answer(backend.requests.shift()!, 1, 1000); // page 0 at generation 1
    expect(rows.get(0)?.id).toBe("g1-r0");

    await answer(backend.requests.shift()!, 2, 900); // page 1 at generation 2
    expect(rows.generation).toBe(2);
    expect(rows.total).toBe(900);
    expect(rows.get(0)).toBeUndefined();
    expect(rows.get(200)?.id).toBe("g2-r200");
    // The visible page is fetched again, from the new snapshot.
    expect(requestedStarts()).toEqual([0]);
    await answerAll(2, 900);
    expect(rows.get(0)?.id).toBe("g2-r0");
  });

  it("discards a page from an older generation and requests it again", async () => {
    const rows = store(1, 1000);
    rows.setViewport(0, 40);
    rows.adopt({ root: "/repo", name: "repo", head: null, branch: "main", state: { kind: "clean" }, generation: 2, rowCount: 1000 });
    // Both pages are still in flight, so nothing is requested twice.
    expect(requestedStarts()).toEqual([0, PAGE_SIZE]);

    await answer(backend.requests.shift()!, 1, 1000);
    expect(rows.get(0)).toBeUndefined();
    expect(rows.generation).toBe(2);
    expect(requestedStarts()).toEqual([PAGE_SIZE, 0]);
  });

  it("keeps the cache when a refresh reports the same generation", async () => {
    const rows = store(1, 1000);
    rows.setViewport(0, 40);
    await answerAll(1, 1000);
    rows.adopt({ root: "/repo", name: "repo", head: null, branch: "main", state: { kind: "clean" }, generation: 1, rowCount: 1000 });
    expect(rows.get(0)?.id).toBe("g1-r0");
    expect(backend.requests).toHaveLength(0);
  });

  it("resets and refetches what is visible when a refresh reports a new generation", async () => {
    const rows = store(1, 1000);
    rows.setViewport(400, 440);
    await answerAll(1, 1000);
    rows.adopt({ root: "/repo", name: "repo", head: null, branch: "main", state: { kind: "clean" }, generation: 3, rowCount: 1001 });
    expect(rows.total).toBe(1001);
    expect(rows.get(400)).toBeUndefined();
    expect(requestedStarts()).toEqual([400, 600]);
    await answerAll(3, 1001);
    expect(rows.get(400)?.id).toBe("g3-r400");
  });

  it("tells its owner about a newer generation while the old rows are still readable", async () => {
    const seen: (string | undefined)[] = [];
    const rows: RowStore = new RowStore(
      { root: "/repo", name: "repo", head: null, branch: "main", state: { kind: "clean" }, generation: 1, rowCount: 1000 },
      () => seen.push(rows.get(0)?.id),
    );
    rows.setViewport(0, 40);
    await answerAll(1, 1000);
    rows.adopt({ root: "/repo", name: "repo", head: null, branch: "main", state: { kind: "clean" }, generation: 1, rowCount: 1000 });
    expect(seen).toEqual([]);
    rows.adopt({ root: "/repo", name: "repo", head: null, branch: "main", state: { kind: "clean" }, generation: 2, rowCount: 1000 });
    expect(seen).toEqual(["g1-r0"]);
    expect(rows.get(0)).toBeUndefined();
  });

  it("wakes waiters when it moves to another snapshot", async () => {
    const rows = store(1, 1000);
    rows.setViewport(0, 40);
    let woken = false;
    void rows.whenLoaded(0, 40).then(() => (woken = true));
    rows.adopt({ root: "/repo", name: "repo", head: null, branch: "main", state: { kind: "clean" }, generation: 2, rowCount: 1000 });
    await settle();
    expect(woken).toBe(true);
  });

  it("replaces the repository without notifying, and discards the old repository's responses", async () => {
    let notified = false;
    const rows = new RowStore(
      { root: "/a", name: "a", head: null, branch: "main", state: { kind: "clean" }, generation: 1, rowCount: 1000 },
      () => (notified = true),
    );
    rows.setViewport(0, 40);
    rows.replace({ root: "/b", name: "b", head: null, branch: "main", state: { kind: "clean" }, generation: 5, rowCount: 300 });
    expect(notified).toBe(false);
    expect(rows.total).toBe(300);
    await answerAll(1, 1000); // answers for repository a arrive late
    expect(rows.get(0)).toBeUndefined();
    expect(rows.generation).toBe(5);
    // Re-requested from the new repository once the old requests left the in-flight set.
    expect(requestedStarts()).toEqual([0, 200]);
    await answerAll(5, 300);
    expect(rows.get(0)?.id).toBe("g5-r0");
  });

  it("ignores a refresh result older than what it already has", async () => {
    const rows = store(1, 1000);
    rows.setViewport(0, 40);
    await answer(backend.requests.shift()!, 2, 1000);
    rows.adopt({ root: "/repo", name: "repo", head: null, branch: "main", state: { kind: "clean" }, generation: 1, rowCount: 5 });
    expect(rows.generation).toBe(2);
    expect(rows.total).toBe(1000);
    expect(rows.get(0)?.id).toBe("g2-r0");
  });
});
