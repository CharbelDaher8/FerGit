import { describe, expect, it, vi } from "vitest";
import type { RepoInfo } from "./bindings";
import type { RepoClient } from "./client";
import type { SavedTabs } from "./settings.svelte";
import { Tabs, type TabsBackend, type TabsMemory } from "./tabs.svelte";

function info(root: string, generation = 1): RepoInfo {
  return { root, name: root.slice(1), head: null, branch: "main", state: { kind: "clean" }, filter: { refs: [], path: null }, generation, rowCount: 0 };
}

/**
 * A fake backend with one session per repository, like the real one: repositories are named by
 * their root, and any path under a root opens that repository. Paths not under `repos` fail.
 */
function fakeBackend(repos: string[]) {
  const sessions = new Map<string, number>();
  let nextSession = 1;
  const closed: number[] = [];
  const opened: string[] = [];
  const backend: TabsBackend = {
    open: async (path) => {
      opened.push(path);
      const root = repos.find((repo) => path === repo || path.startsWith(`${repo}/`));
      if (!root) throw { kind: "notARepository", message: `${path} is not inside a git repository` };
      let session = sessions.get(root);
      if (session === undefined) {
        session = nextSession++;
        sessions.set(root, session);
      }
      return { session, info: info(root) };
    },
    client: (session) => fakeClient(session, closed),
  };
  return { backend, closed, opened };
}

/** A client whose reads never answer: these tests are about tabs, not what they show. */
function fakeClient(session: number, closed: number[]): RepoClient {
  const never = () => new Promise<never>(() => {});
  return {
    refresh: never,
    rows: never,
    locate: never,
    commitDetails: never,
    changes: never,
    fileDiff: never,
    search: never,
    setFilter: never,
    refs: never,
    runOperation: never,
    undoable: never,
    close: async () => void closed.push(session),
  };
}

function memory(saved: SavedTabs = { paths: [], active: null }): TabsMemory & { writes: number } {
  let tabs = saved;
  return {
    writes: 0,
    get tabs() {
      return tabs;
    },
    set tabs(value: SavedTabs) {
      tabs = value;
      this.writes++;
    },
  };
}

const roots = (tabs: Tabs) => tabs.all.map((tab) => tab.info.root);

describe("Tabs: opening", () => {
  it("opens each repository in a new tab at the end, and brings it to the front", async () => {
    const { backend } = fakeBackend(["/a", "/b"]);
    const tabs = new Tabs(backend, memory());
    expect(tabs.active).toBeNull();

    const a = await tabs.open("/a");
    expect(tabs.active).toBe(a);
    const b = await tabs.open("/b");
    expect(roots(tabs)).toEqual(["/a", "/b"]);
    expect(tabs.active).toBe(b);
    expect(b.session).not.toBe(a.session);
  });

  it("brings the tab of a repository that is already open to the front instead of opening another", async () => {
    const { backend } = fakeBackend(["/a", "/b"]);
    const tabs = new Tabs(backend, memory());
    const a = await tabs.open("/a");
    await tabs.open("/b");

    await expect(tabs.open("/a/src")).resolves.toBe(a);
    expect(roots(tabs)).toEqual(["/a", "/b"]);
    expect(tabs.active).toBe(a);
  });

  it("leaves the tabs alone when a repository fails to open", async () => {
    const { backend } = fakeBackend(["/a"]);
    const tabs = new Tabs(backend, memory());
    const a = await tabs.open("/a");

    await expect(tabs.open("/nowhere")).rejects.toMatchObject({ kind: "notARepository" });
    expect(roots(tabs)).toEqual(["/a"]);
    expect(tabs.active).toBe(a);
  });

  it("keeps each tab's selection while another is in front", async () => {
    const { backend } = fakeBackend(["/a", "/b"]);
    const tabs = new Tabs(backend, memory());
    const a = await tabs.open("/a");
    a.detailsOpen = false;
    const b = await tabs.open("/b");

    tabs.activate(a);
    expect(a.detailsOpen).toBe(false);
    expect(b.detailsOpen).toBe(true);
    expect(a.view).not.toBe(b.view);
    expect(a.inspector).not.toBe(b.inspector);
  });
});

describe("Tabs: switching and closing", () => {
  async function three() {
    const fake = fakeBackend(["/a", "/b", "/c"]);
    const tabs = new Tabs(fake.backend, memory());
    const [a, b, c] = [await tabs.open("/a"), await tabs.open("/b"), await tabs.open("/c")];
    return { ...fake, tabs, a, b, c };
  }

  it("cycles through the tabs in both directions, wrapping around", async () => {
    const { tabs, a, b, c } = await three();
    tabs.cycle(1);
    expect(tabs.active).toBe(a);
    tabs.cycle(1);
    expect(tabs.active).toBe(b);
    tabs.cycle(-1);
    tabs.cycle(-1);
    expect(tabs.active).toBe(c);
  });

  it("brings the tab to the right of a closed front tab to the front, or the one to the left", async () => {
    const { tabs, a, b, c } = await three();
    tabs.activate(b);
    await tabs.close(b);
    expect(roots(tabs)).toEqual(["/a", "/c"]);
    expect(tabs.active).toBe(c);
    await tabs.close(c);
    expect(tabs.active).toBe(a);
    await tabs.close(a);
    expect(tabs.active).toBeNull();
    expect(tabs.all).toEqual([]);
  });

  it("keeps the front tab when closing another", async () => {
    const { tabs, a, c } = await three();
    await tabs.close(a);
    expect(tabs.active).toBe(c);
  });

  it("closes the backend session of a closed tab, once", async () => {
    const { tabs, b, closed } = await three();
    await tabs.close(b);
    await tabs.close(b);
    expect(closed).toEqual([b.session]);
  });

  it("opens a repository again in a new tab after its tab was closed", async () => {
    const { tabs, a } = await three();
    await tabs.close(a);
    const again = await tabs.open("/a");
    expect(again).not.toBe(a);
    expect(roots(tabs)).toEqual(["/b", "/c", "/a"]);
  });
});

describe("Tabs: change events", () => {
  it("routes each event to the tab of its session, ignoring closed sessions and older news", async () => {
    const { backend } = fakeBackend(["/a", "/b"]);
    const tabs = new Tabs(backend, memory());
    const a = await tabs.open("/a");
    const b = await tabs.open("/b");

    tabs.adopt(a.session, { ...info("/a", 5), rowCount: 3 });
    expect(a.info.rowCount).toBe(3);
    expect(a.updates).toBe(1);
    expect(b.updates).toBe(0);

    tabs.adopt(a.session, info("/a", 4));
    expect(a.info.generation).toBe(5);

    await tabs.close(b);
    tabs.adopt(b.session, info("/b", 9));
    expect(b.info.generation).toBe(1);
  });
});

describe("Tabs: remembering", () => {
  it("remembers the open repositories in order and which one is in front", async () => {
    const { backend } = fakeBackend(["/a", "/b", "/c"]);
    const saved = memory();
    const tabs = new Tabs(backend, saved);
    const a = await tabs.open("/a/deep/path");
    await tabs.open("/b");
    const c = await tabs.open("/c");
    expect(saved.tabs).toEqual({ paths: ["/a", "/b", "/c"], active: 2 });

    tabs.activate(a);
    expect(saved.tabs.active).toBe(0);
    await tabs.close(c);
    expect(saved.tabs).toEqual({ paths: ["/a", "/b"], active: 0 });
    await tabs.close(a);
    await tabs.close(tabs.all[0]);
    expect(saved.tabs).toEqual({ paths: [], active: null });
  });

  it("restores the remembered tabs in order, with the remembered one in front", async () => {
    const { backend } = fakeBackend(["/a", "/b", "/c"]);
    const saved = memory({ paths: ["/a", "/b", "/c"], active: 1 });
    const tabs = new Tabs(backend, saved);

    const restoring = tabs.restore();
    expect(tabs.restoring).toBe(true);
    await restoring;

    expect(tabs.restoring).toBe(false);
    expect(roots(tabs)).toEqual(["/a", "/b", "/c"]);
    expect(tabs.active?.info.root).toBe("/b");
    expect(saved.writes).toBe(1);
  });

  it("drops repositories that no longer open, and reports the first failure", async () => {
    const { backend } = fakeBackend(["/a", "/c"]);
    const saved = memory({ paths: ["/a", "/gone", "/c"], active: 1 });
    const tabs = new Tabs(backend, saved);

    await expect(tabs.restore()).rejects.toMatchObject({ message: "/gone is not inside a git repository" });

    expect(roots(tabs)).toEqual(["/a", "/c"]);
    expect(tabs.active?.info.root).toBe("/a");
    expect(saved.tabs).toEqual({ paths: ["/a", "/c"], active: 0 });
  });

  it("does nothing without remembered tabs", async () => {
    const { backend, opened } = fakeBackend(["/a"]);
    const saved = memory();
    const tabs = new Tabs(backend, saved);
    await tabs.restore();
    expect(opened).toEqual([]);
    expect(tabs.all).toEqual([]);
    expect(saved.writes).toBe(0);
  });

  it("keeps a tab opened while restoring in front", async () => {
    const { backend } = fakeBackend(["/a", "/b", "/c"]);
    const saved = memory({ paths: ["/a", "/b"], active: 0 });
    const tabs = new Tabs(backend, saved);

    const restoring = tabs.restore();
    const c = await tabs.open("/c");
    await restoring;

    expect(roots(tabs)).toEqual(["/c", "/a", "/b"]);
    expect(tabs.active).toBe(c);
    expect(saved.tabs).toEqual({ paths: ["/c", "/a", "/b"], active: 0 });
  });

  it("doesn't duplicate a remembered repository listed twice", async () => {
    const { backend } = fakeBackend(["/a"]);
    const tabs = new Tabs(backend, memory({ paths: ["/a", "/a/sub"], active: 1 }));
    await tabs.restore();
    expect(roots(tabs)).toEqual(["/a"]);
    expect(tabs.active?.info.root).toBe("/a");
  });
});

describe("Tab", () => {
  it("refreshes through its own client, one refresh at a time", async () => {
    const { backend } = fakeBackend(["/a"]);
    const tabs = new Tabs(backend, memory());
    const tab = await tabs.open("/a");
    let answer: (info: RepoInfo) => void = () => {};
    const refresh = vi.spyOn(tab.client, "refresh").mockImplementation(
      () => new Promise((resolve) => (answer = resolve)),
    );

    const first = tab.refresh();
    void tab.refresh();
    expect(refresh).toHaveBeenCalledTimes(1);
    expect(tab.refreshing).toBe(true);
    answer({ ...info("/a", 2), rowCount: 7 });
    await first;
    expect(tab.refreshing).toBe(false);
    expect(tab.info.rowCount).toBe(7);
  });
});
