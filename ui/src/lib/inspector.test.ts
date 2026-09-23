import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { CommitDetails, FileChange, FileDiff, Row } from "./bindings";
import {
  Inspector,
  commitSides,
  describeSide,
  rangeSides,
  stagedSides,
  subjectOf,
  unstagedSides,
  type Selection,
} from "./inspector.svelte";

interface Call {
  command: string;
  args: unknown[];
  resolve: (value: unknown) => void;
}

/** A fake backend: every command stays pending until a test answers it. */
const backend = { calls: [] as Call[] };

const defer =
  <T>(command: string) =>
  (...args: unknown[]) =>
    new Promise<T>((resolve) => backend.calls.push({ command, args, resolve: resolve as (value: unknown) => void }));
const client = {
  commitDetails: defer<CommitDetails | null>("commitDetails"),
  changes: defer<FileChange[]>("changes"),
  fileDiff: defer<FileDiff>("fileDiff"),
};

/** Removes and returns the oldest pending call of `command`. */
function take(command: string): Call {
  const index = backend.calls.findIndex((call) => call.command === command);
  if (index < 0) throw new Error(`no pending ${command} call`);
  return backend.calls.splice(index, 1)[0];
}

function pending(command: string): unknown[][] {
  return backend.calls.filter((call) => call.command === command).map((call) => call.args);
}

async function answer(call: Call, value: unknown): Promise<void> {
  call.resolve(value);
  await wait(0);
}

function wait(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/** Longer than the inspector's delay before loading commit details. */
const DETAILS_WAIT_MS = 90;

const commit = (id: string) => ({ kind: "commit" as const, id });
const INDEX = { kind: "index" as const };
const WORKTREE = { kind: "worktree" as const };

function file(path: string, oldPath: string | null = null): FileChange {
  return { path, oldPath, status: oldPath ? "renamed" : "modified", additions: 1, deletions: 0 };
}

function details(id: string, parents: string[], files: FileChange[]): CommitDetails {
  const who = { name: "Ada", email: "ada@example.com", time: 0, offsetMinutes: 0 };
  return { id, parents, author: who, committer: who, message: id, files };
}

const text = (marker: string): FileDiff => ({
  kind: "text",
  hunks: [{ oldStart: 1, oldLines: 1, newStart: 1, newLines: 1, lines: [{ kind: "context", text: marker, noFinalNewline: false }] }],
});

function row(id: string, kind: Row["kind"] = "commit"): Row {
  return { kind, id, graph: { column: 0, color: 0, edges: [] }, summary: id, authorName: "", authorEmail: "", time: 0, refs: [], relations: [] };
}

const inspectors: Inspector[] = [];
function inspector(): Inspector {
  const created = new Inspector(client);
  inspectors.push(created);
  return created;
}

beforeEach(() => {
  backend.calls.length = 0;
});

afterEach(() => {
  for (const created of inspectors.splice(0)) created.dispose();
});

describe("sides", () => {
  it("compares a commit with its first parent, and a root commit with nothing", () => {
    expect(commitSides("c", ["p"])).toEqual({ from: commit("p"), to: commit("c") });
    expect(commitSides("merge", ["first", "second"])).toEqual({ from: commit("first"), to: commit("merge") });
    expect(commitSides("root", [])).toEqual({ from: null, to: commit("root") });
  });

  it("compares a stash with its base, its first parent", () => {
    expect(commitSides("stash", ["base", "index-commit"]).from).toEqual(commit("base"));
  });

  it("stages against HEAD, or against nothing before the first commit", () => {
    expect(stagedSides("h")).toEqual({ from: commit("h"), to: INDEX });
    expect(stagedSides(null)).toEqual({ from: null, to: INDEX });
    expect(unstagedSides()).toEqual({ from: INDEX, to: WORKTREE });
  });

  it("describes sides briefly", () => {
    expect(describeSide(commit("0123456789abcdef"))).toBe("01234567");
    expect(describeSide(INDEX)).toBe("index");
    expect(describeSide(WORKTREE)).toBe("working tree");
    expect(describeSide(null)).toBe("nothing");
  });
});

describe("subjectOf", () => {
  const view = (selected: number | null, comparison: Selection["comparison"], rows: Record<number, Row>): Selection => ({
    selected,
    comparison,
    row: (index) => rows[index],
  });

  it("describes nothing without a selection, and waits for rows that aren't loaded", () => {
    expect(subjectOf(view(null, null, {}))).toEqual({ kind: "none" });
    expect(subjectOf(view(3, null, {}))).toBeNull();
    expect(subjectOf(view(3, { older: 9, newer: 3 }, { 3: row("b") }))).toBeNull();
  });

  it("tells uncommitted changes from commits and stashes", () => {
    expect(subjectOf(view(0, null, { 0: row("0000", "workingTree") }))).toEqual({ kind: "worktree" });
    expect(subjectOf(view(1, null, { 1: row("s", "stash") }))).toEqual({ kind: "commit", id: "s" });
    expect(subjectOf(view(2, null, { 2: row("c") }))).toEqual({ kind: "commit", id: "c" });
  });

  it("compares the lower (older) row to the upper (newer) one", () => {
    const rows = { 3: row("newer"), 9: row("older") };
    const subject = subjectOf(view(3, { older: 9, newer: 3 }, rows));
    expect(subject).toEqual({ kind: "range", older: "older", newer: "newer" });
    expect(rangeSides("older", "newer")).toEqual({ from: commit("older"), to: commit("newer") });
  });
});

describe("Inspector: commits", () => {
  it("loads a commit's details and opens its files against the first parent", async () => {
    const inspect = inspector();
    inspect.show({ kind: "commit", id: "c" }, "h", 1);
    expect(inspect.details).toBeUndefined();
    await wait(DETAILS_WAIT_MS);
    const call = take("commitDetails");
    expect(call.args).toEqual(["c"]);
    await answer(call, details("c", ["p1", "p2"], [file("a.txt"), file("new.txt", "old.txt")]));

    const [list] = inspect.lists;
    expect(list.key).toBe("commit");
    expect(list.sides).toEqual({ from: commit("p1"), to: commit("c") });

    inspect.openFile(list, list.files![1]);
    expect(inspect.diff?.result).toBeUndefined();
    const diffCall = take("fileDiff");
    expect(diffCall.args).toEqual([commit("p1"), commit("c"), "new.txt", "old.txt"]);
    await answer(diffCall, text("renamed"));
    expect(inspect.diff?.result).toEqual(text("renamed"));
  });

  it("opens a root commit's files against nothing", async () => {
    const inspect = inspector();
    inspect.show({ kind: "commit", id: "root" }, "root", 1);
    await wait(DETAILS_WAIT_MS);
    await answer(take("commitDetails"), details("root", [], [file("a.txt")]));
    const [list] = inspect.lists;
    inspect.openFile(list, list.files![0]);
    expect(take("fileDiff").args).toEqual([null, commit("root"), "a.txt", null]);
  });

  it("drops details that arrive after the selection moved on", async () => {
    const inspect = inspector();
    inspect.show({ kind: "commit", id: "a" }, null, 1);
    await wait(DETAILS_WAIT_MS);
    const first = take("commitDetails");
    inspect.show({ kind: "commit", id: "b" }, null, 1);
    await wait(DETAILS_WAIT_MS);
    await answer(first, details("a", [], []));
    expect(inspect.details).toBeUndefined();
    await answer(take("commitDetails"), details("b", [], []));
    expect(inspect.details?.id).toBe("b");
  });

  it("doesn't load details for a selection that only flashed by", async () => {
    const inspect = inspector();
    inspect.show({ kind: "commit", id: "a" }, null, 1);
    inspect.show({ kind: "commit", id: "b" }, null, 1);
    await wait(DETAILS_WAIT_MS);
    expect(pending("commitDetails")).toEqual([["b"]]);
  });
});

describe("Inspector: diffs", () => {
  async function withCommitList(inspect: Inspector) {
    inspect.show({ kind: "commit", id: "c" }, null, 1);
    await wait(DETAILS_WAIT_MS);
    await answer(take("commitDetails"), details("c", ["p"], [file("a"), file("b")]));
    return inspect.lists[0];
  }

  it("closes the diff, and ignores its late response, when the selection changes while it loads", async () => {
    const inspect = inspector();
    const list = await withCommitList(inspect);
    inspect.openFile(list, list.files![0]);
    const loading = take("fileDiff");
    inspect.show({ kind: "commit", id: "other" }, null, 1);
    expect(inspect.diff).toBeNull();
    await answer(loading, text("stale"));
    expect(inspect.diff).toBeNull();
  });

  it("keeps only the diff opened last", async () => {
    const inspect = inspector();
    const list = await withCommitList(inspect);
    inspect.openFile(list, list.files![0]);
    inspect.openFile(list, list.files![1]);
    const [first, second] = [take("fileDiff"), take("fileDiff")];
    await answer(second, text("b"));
    await answer(first, text("a"));
    expect(inspect.diff?.request.path).toBe("b");
    expect(inspect.diff?.result).toEqual(text("b"));
  });

  it("ignores a response for a diff that was closed", async () => {
    const inspect = inspector();
    const list = await withCommitList(inspect);
    inspect.openFile(list, list.files![0]);
    const loading = take("fileDiff");
    inspect.closeDiff();
    await answer(loading, text("a"));
    expect(inspect.diff).toBeNull();
  });

  it("keeps the subject and diff while the selected row reloads", async () => {
    const inspect = inspector();
    const list = await withCommitList(inspect);
    inspect.openFile(list, list.files![0]);
    await answer(take("fileDiff"), text("a"));
    inspect.show(null, null, 2);
    expect(inspect.subject).toEqual({ kind: "commit", id: "c" });
    expect(inspect.diff?.result).toEqual(text("a"));
  });
});

describe("Inspector: uncommitted changes", () => {
  it("lists staged changes against HEAD and unstaged changes against the index", async () => {
    const inspect = inspector();
    inspect.show({ kind: "worktree" }, "h", "1:0");
    expect(pending("changes")).toEqual([
      [commit("h"), INDEX],
      [INDEX, WORKTREE],
    ]);
    const [staged, unstaged] = inspect.lists;
    expect(staged.files).toBeUndefined();
    take("changes").resolve([file("staged.txt")]);
    await answer(take("changes"), [file("unstaged.txt")]);

    const lists = inspect.lists;
    expect(lists.map((list) => list.key)).toEqual(["staged", "unstaged"]);
    expect(lists[0].files).toEqual([file("staged.txt")]);
    expect(lists[1].files).toEqual([file("unstaged.txt")]);
    expect(staged.sides).toEqual({ from: commit("h"), to: INDEX });
    expect(unstaged.sides).toEqual({ from: INDEX, to: WORKTREE });

    inspect.openFile(lists[0], lists[0].files![0]);
    expect(take("fileDiff").args).toEqual([commit("h"), INDEX, "staged.txt", null]);
    inspect.openFile(lists[1], lists[1].files![0]);
    expect(take("fileDiff").args).toEqual([INDEX, WORKTREE, "unstaged.txt", null]);
  });

  it("stages against nothing in a repository without commits", () => {
    const inspect = inspector();
    inspect.show({ kind: "worktree" }, null, "1:0");
    expect(pending("changes")[0]).toEqual([null, INDEX]);
  });

  it("re-reads both lists when the version or HEAD changes, keeping the old lists meanwhile", async () => {
    const inspect = inspector();
    inspect.show({ kind: "worktree" }, "h", "1:0");
    take("changes").resolve([file("one")]);
    await answer(take("changes"), []);

    inspect.show({ kind: "worktree" }, "h", "1:0");
    expect(pending("changes")).toEqual([]);

    inspect.show({ kind: "worktree" }, "h", "1:1"); // e.g. a focus refresh with the same generation
    expect(pending("changes")).toHaveLength(2);
    expect(inspect.lists[0].files).toEqual([file("one")]);
    take("changes").resolve([file("two")]);
    await answer(take("changes"), []);
    expect(inspect.lists[0].files).toEqual([file("two")]);

    inspect.show({ kind: "worktree" }, "h2", "1:1"); // a new commit moved HEAD
    expect(pending("changes")[0]).toEqual([commit("h2"), INDEX]);
  });

  it("uses the latest read when reads finish out of order", async () => {
    const inspect = inspector();
    inspect.show({ kind: "worktree" }, "h", 1);
    inspect.show({ kind: "worktree" }, "h", 2);
    const [oldStaged, oldUnstaged, newStaged, newUnstaged] = backend.calls.splice(0);
    newStaged.resolve([file("new")]);
    await answer(newUnstaged, []);
    oldStaged.resolve([file("old")]);
    await answer(oldUnstaged, []);
    expect(inspect.lists[0].files).toEqual([file("new")]);
  });

  it("reloads an open worktree diff in place when the version changes", async () => {
    const inspect = inspector();
    inspect.show({ kind: "worktree" }, "h", 1);
    take("changes").resolve([]);
    await answer(take("changes"), [file("edited.txt")]);
    const unstaged = inspect.lists[1];
    inspect.openFile(unstaged, unstaged.files![0]);
    await answer(take("fileDiff"), text("before"));
    const request = inspect.diff!.request;

    inspect.show({ kind: "worktree" }, "h", 2);
    expect(pending("fileDiff")).toEqual([[INDEX, WORKTREE, "edited.txt", null]]);
    expect(inspect.diff?.result).toEqual(text("before"));
    await answer(take("fileDiff"), text("after"));
    expect(inspect.diff?.result).toEqual(text("after"));
    expect(inspect.diff?.request).toBe(request); // same request object: the view keeps its scroll
  });
});

describe("Inspector: comparing commits", () => {
  it("lists and opens files from the older commit to the newer one", async () => {
    const inspect = inspector();
    inspect.show({ kind: "range", older: "old", newer: "new" }, "h", 1);
    const call = take("changes");
    expect(call.args).toEqual([commit("old"), commit("new")]);
    await answer(call, [file("x")]);
    const [list] = inspect.lists;
    expect(list.key).toBe("range");
    inspect.openFile(list, list.files![0]);
    expect(take("fileDiff").args).toEqual([commit("old"), commit("new"), "x", null]);
  });

  it("doesn't reload a range when only the version changes", async () => {
    const inspect = inspector();
    inspect.show({ kind: "range", older: "old", newer: "new" }, "h", 1);
    await answer(take("changes"), []);
    inspect.show({ kind: "range", older: "old", newer: "new" }, "h", 2);
    expect(pending("changes")).toEqual([]);
  });
});
