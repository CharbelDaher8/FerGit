import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SearchResult } from "./bindings";
import { FIND_DELAY_MS, Finder, type FindTarget } from "./find.svelte";

/**
 * A fake backend whose `search` answers from `matches` (query → rows) in the current generation.
 * With `hold`, answers wait until `release` is called, to simulate slow searches.
 */
const backend = {
  generation: 1,
  matches: {} as Record<string, number[]>,
  calls: [] as string[],
  hold: false,
  held: [] as (() => void)[],
};

const client = {
  search: async (query: string): Promise<SearchResult> => {
    backend.calls.push(query);
    const answer = (): SearchResult => {
      const rows = backend.matches[query] ?? [];
      return { generation: backend.generation, rows, total: rows.length };
    };
    if (!backend.hold) return answer();
    // Read the answer when released, like a search that ran late.
    return new Promise((resolve) => backend.held.push(() => resolve(answer())));
  },
};

/** A stand-in for the view that records what gets selected. */
class Target implements FindTarget {
  generation = 1;
  selected: number | null = null;
  selections: number[] = [];

  select(index: number): void {
    this.selected = index;
    this.selections.push(index);
  }
}

async function settle(): Promise<void> {
  for (let i = 0; i < 5; i++) await new Promise((resolve) => setTimeout(resolve, 0));
}

function setup(): { finder: Finder; target: Target } {
  const target = new Target();
  const finder = new Finder(() => target, client);
  finder.show();
  return { finder, target };
}

beforeEach(() => {
  backend.generation = 1;
  backend.matches = { fix: [2, 5, 9], none: [] };
  backend.calls = [];
  backend.hold = false;
  backend.held = [];
});

afterEach(() => {
  vi.useRealTimers();
});

describe("typing", () => {
  it("searches once typing pauses and goes to the first match from the selection", async () => {
    vi.useFakeTimers();
    const { finder, target } = setup();
    target.selected = 4;
    finder.setQuery("f");
    finder.setQuery("fi");
    finder.setQuery("fix");
    expect(backend.calls).toEqual([]);
    await vi.advanceTimersByTimeAsync(FIND_DELAY_MS);
    expect(backend.calls).toEqual(["fix"]);
    expect(target.selections).toEqual([5]);
    expect(finder.current).toBe(1);
    expect(finder.total).toBe(3);
  });

  it("wraps to the first match when none is at or below the selection", async () => {
    const { finder, target } = setup();
    target.selected = 20;
    finder.query = "fix";
    await finder.search(true);
    expect(target.selections).toEqual([2]);
    expect(finder.current).toBe(0);
  });

  it("clears the matches when the query is blanked, without searching", async () => {
    const { finder } = setup();
    finder.query = "fix";
    await finder.search(true);
    finder.setQuery("   ");
    await settle();
    expect(backend.calls).toEqual(["fix"]);
    expect(finder.total).toBe(0);
    expect(finder.current).toBeNull();
    expect(finder.isMatch(2)).toBe(false);
  });

  it("reports no current match when nothing is found", async () => {
    const { finder, target } = setup();
    finder.query = "none";
    await finder.search(true);
    expect(finder.current).toBeNull();
    expect(target.selections).toEqual([]);
  });
});

describe("stepping", () => {
  it("goes to the next and previous match from the selection, wrapping around", async () => {
    const { finder, target } = setup();
    finder.query = "fix";
    await finder.search(true);
    expect(target.selected).toBe(2);
    await finder.step(1);
    expect(target.selected).toBe(5);
    await finder.step(2);
    expect(target.selected).toBe(2);
    await finder.step(-1);
    expect(target.selected).toBe(9);
    expect(finder.current).toBe(2);
  });

  it("steps from wherever the selection moved in between", async () => {
    const { finder, target } = setup();
    finder.query = "fix";
    await finder.search(true);
    target.selected = 6;
    await finder.step(1);
    expect(target.selected).toBe(9);
    target.selected = 6;
    await finder.step(-1);
    expect(target.selected).toBe(5);
    target.selected = null;
    await finder.step(1);
    expect(target.selected).toBe(2);
    target.selected = null;
    await finder.step(-1);
    expect(target.selected).toBe(9);
  });

  it("searches first when there are no matches yet (n after closing the bar)", async () => {
    const { finder, target } = setup();
    finder.query = "fix";
    finder.hide();
    await finder.step(1);
    expect(backend.calls).toEqual(["fix"]);
    expect(target.selected).toBe(2);
  });
});

describe("status", () => {
  it("says what the search found and which match is current", async () => {
    const { finder } = setup();
    expect(finder.status).toBe("");
    backend.hold = true;
    finder.query = "fix";
    const pending = finder.search(true);
    expect(finder.status).toBe("Searching…");
    backend.held[0]();
    await pending;
    expect(finder.status).toBe("1 of 3");
    await finder.step(1);
    expect(finder.status).toBe("2 of 3");

    backend.hold = false;
    finder.query = "none";
    await finder.search(true);
    expect(finder.status).toBe("No matches");
  });
});

describe("highlighting", () => {
  it("marks the matches and the current one", async () => {
    const { finder } = setup();
    finder.query = "fix";
    await finder.search(true);
    await finder.step(1);
    expect([0, 2, 5, 9].map((row) => finder.isMatch(row))).toEqual([false, true, true, true]);
    expect([2, 5, 9].map((row) => finder.isCurrent(row))).toEqual([false, true, false]);
  });

  it("forgets the matches when the bar is hidden", async () => {
    const { finder } = setup();
    finder.query = "fix";
    await finder.search(true);
    finder.hide();
    expect(finder.open).toBe(false);
    expect(finder.isMatch(2)).toBe(false);
    expect(finder.query).toBe("fix");
  });
});

describe("snapshots and races", () => {
  it("highlights nothing once the view moves to a newer snapshot, and sync searches again", async () => {
    const { finder, target } = setup();
    finder.query = "fix";
    await finder.search(true);
    const selections = target.selections.length;

    backend.generation = 2;
    backend.matches.fix = [3, 4];
    target.generation = 2;
    expect(finder.isMatch(2)).toBe(false);

    finder.sync();
    await settle();
    expect(backend.calls).toEqual(["fix", "fix"]);
    expect(finder.isMatch(3)).toBe(true);
    expect(target.selections.length).toBe(selections);

    finder.sync();
    await settle();
    expect(backend.calls.length).toBe(2);
  });

  it("drops the answer to an older query", async () => {
    const { finder, target } = setup();
    backend.hold = true;
    finder.query = "none";
    const first = finder.search(true);
    finder.query = "fix";
    const second = finder.search(true);
    backend.held[1]();
    await second;
    backend.held[0]();
    await first;
    expect(finder.total).toBe(3);
    expect(target.selections).toEqual([2]);
  });

  it("drops a search that lands after the bar was hidden", async () => {
    const { finder } = setup();
    backend.hold = true;
    finder.query = "fix";
    const pending = finder.search(true);
    finder.hide();
    backend.held[0]();
    await pending;
    expect(finder.total).toBe(0);
    expect(finder.searching).toBe(false);
  });

  it("steps by searching again when the matches are from an older snapshot", async () => {
    const { finder, target } = setup();
    finder.query = "fix";
    await finder.search(true);
    backend.generation = 2;
    backend.matches.fix = [7];
    target.generation = 2;
    target.selected = 0;
    await finder.step(1);
    expect(target.selected).toBe(7);
  });
});
