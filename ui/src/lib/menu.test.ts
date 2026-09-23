import { describe, expect, it } from "vitest";
import type { RefLabel, Row, Upstream } from "./bindings";
import { menuFor, splitRemote, type MenuContext, type MenuEntry } from "./menu";

const ID = "a".repeat(40);
const TIP = "b".repeat(40);
const HEAD = "c".repeat(40);

function row(refs: RefLabel[] = [], kind: Row["kind"] = "commit", id = ID): Row {
  return { kind, id, graph: { column: 0, color: 0, edges: [] }, summary: "", authorName: "", authorEmail: "", time: 0, refs, relations: [] };
}

const tracking: Upstream = { name: "origin/main", state: { kind: "tracking", ahead: 1, behind: 0, id: TIP } };

function local(name: string, isHead = false, upstream: Upstream | null = null): RefLabel {
  return { kind: "localBranch", name, fullName: `refs/heads/${name}`, isHead, upstream };
}

/** A detached HEAD in an empty repository: none of the history entries apply. */
const BARE: MenuContext = { branch: null, head: null, busy: false, compared: null };
/** On `main`, at HEAD, nothing in progress. */
const ON_MAIN: MenuContext = { branch: "main", head: HEAD, busy: false, compared: null };

const labels = (entries: MenuEntry[]) => entries.map((entry) => ("separator" in entry ? "---" : entry.label));
const action = (entries: MenuEntry[], label: string) =>
  entries.find((entry): entry is Extract<MenuEntry, { action: unknown }> => "label" in entry && entry.label === label)?.action;

describe("context menu", () => {
  it("offers commit actions on a commit", () => {
    expect(labels(menuFor(row(), null, BARE))).toEqual([
      "Check out this commit (detached)",
      "Create branch here…",
      "Create tag here…",
    ]);
  });

  it("lets the current branch pull and push, but not be checked out or deleted", () => {
    const main = local("main", true, tracking);
    const entries = menuFor(row([main]), main, BARE);
    expect(labels(entries).slice(0, 3)).toEqual([
      "Pull origin/main into main",
      "Push main",
      "Force push main (with lease)…",
    ]);
    expect(labels(entries)).not.toContain("Delete main…");
    const force = entries.find((entry) => "action" in entry && entry.action.kind === "forcePush");
    expect(force).toMatchObject({ danger: true, action: { expected: TIP, upstream: "origin/main" } });
  });

  it("lets another branch be checked out, pushed with a new upstream, or deleted", () => {
    const topic = local("topic");
    const entries = menuFor(row([topic]), topic, BARE);
    expect(labels(entries)).toEqual([
      "Check out topic",
      "Push topic and track it",
      "---",
      "Delete topic…",
      "---",
      "Check out this commit (detached)",
      "Create branch here…",
      "Create tag here…",
    ]);
    expect(entries[1]).toMatchObject({ action: { kind: "push", branch: "topic", setUpstream: true } });
  });

  it("offers no force push without an upstream tip to lease against", () => {
    const gone = local("old", false, { name: "origin/old", state: { kind: "gone" } });
    expect(labels(menuFor(row([gone]), gone, BARE)).some((label) => label.startsWith("Force push"))).toBe(false);
  });

  it("checks out a remote branch as a local branch of the same name", () => {
    const remote: RefLabel = { kind: "remoteBranch", name: "origin/feature/x", fullName: "refs/remotes/origin/feature/x", isHead: false, upstream: null };
    const [checkout, fetch] = menuFor(row([remote]), remote, BARE);
    expect(checkout).toMatchObject({
      action: { kind: "checkoutRemote", fullName: "refs/remotes/origin/feature/x", name: "feature/x", at: ID },
    });
    expect(fetch).toMatchObject({ label: "Fetch origin", action: { kind: "fetch", remote: "origin" } });
  });

  it("deletes tags only after asking", () => {
    const tag: RefLabel = { kind: "tag", name: "v1", fullName: "refs/tags/v1", isHead: false, upstream: null };
    expect(menuFor(row([tag]), tag, BARE)[0]).toMatchObject({ danger: true, action: { kind: "deleteTag", name: "v1" } });
  });

  it("splits a remote branch name at its first slash", () => {
    expect(splitRemote("origin/feature/x")).toEqual({ remote: "origin", branch: "feature/x" });
    expect(splitRemote("odd")).toEqual({ remote: "odd", branch: "odd" });
  });
});

describe("history entries", () => {
  it("merge, rebase, cherry-pick, revert and reset relative to another commit", () => {
    const entries = menuFor(row(), null, ON_MAIN);
    expect(labels(entries).slice(3)).toEqual([
      "---",
      "Merge commit aaaaaaaa into main…",
      "Rebase main onto commit aaaaaaaa…",
      "Cherry-pick this commit",
      "Revert this commit",
      "Reset main to this commit…",
    ]);
    expect(action(entries, "Reset main to this commit…")).toEqual({ kind: "reset", branch: "main", to: ID, expected: HEAD });
    expect(action(entries, "Merge commit aaaaaaaa into main…")).toMatchObject({ from: { kind: "commit", id: ID }, into: "main" });
  });

  it("offers only reverting the commit HEAD is at", () => {
    const entries = menuFor(row([], "commit", HEAD), null, ON_MAIN);
    expect(labels(entries).filter((label) => /Merge|Rebase|Cherry|Reset|Revert/.test(label))).toEqual(["Revert this commit"]);
  });

  it("merges and rebases onto branches and tags by their full names", () => {
    const remote: RefLabel = { kind: "remoteBranch", name: "origin/x", fullName: "refs/remotes/origin/x", isHead: false, upstream: null };
    const entries = menuFor(row([remote]), remote, ON_MAIN);
    expect(action(entries, "Merge origin/x into main…")).toEqual({
      kind: "merge",
      from: { kind: "ref", name: "refs/remotes/origin/x" },
      name: "origin/x",
      into: "main",
    });
    expect(action(entries, "Rebase main onto origin/x…")).toMatchObject({ onto: { kind: "ref", name: "refs/remotes/origin/x" } });

    const tag: RefLabel = { kind: "tag", name: "v1", fullName: "refs/tags/v1", isHead: false, upstream: null };
    expect(labels(menuFor(row([tag]), tag, ON_MAIN)).slice(0, 2)).toEqual(["Merge v1 into main…", "Delete tag v1…"]);
  });

  it("cherry-picks both compared commits, oldest first", () => {
    const compared = { older: TIP, newer: ID };
    const entries = menuFor(row(), null, { ...ON_MAIN, compared });
    expect(action(entries, "Cherry-pick both compared commits")).toEqual({ kind: "cherryPick", commits: [TIP, ID] });
  });

  it("offers nothing that changes history while something is in progress", () => {
    const topic = local("topic");
    const entries = menuFor(row([topic]), topic, { ...ON_MAIN, busy: true });
    expect(labels(entries).some((label) => /Merge|Rebase|Cherry|Reset|Revert/.test(label))).toBe(false);
    expect(menuFor(row([], "workingTree", "0".repeat(40)), null, { ...ON_MAIN, busy: true })).toEqual([]);
  });

  it("stashes uncommitted changes, and applies, pops or drops a stash", () => {
    expect(labels(menuFor(row([], "workingTree", "0".repeat(40)), null, ON_MAIN))).toEqual(["Stash changes…"]);
    const label: RefLabel = { kind: "stash", name: "stash@{2}", fullName: "refs/stash", isHead: false, upstream: null };
    const entries = menuFor(row([label], "stash"), null, ON_MAIN);
    expect(labels(entries)).toEqual(["Apply stash@{2}", "Pop stash@{2}", "---", "Drop stash@{2}…"]);
    expect(entries[3]).toMatchObject({ danger: true, action: { kind: "stashDrop", index: 2, id: ID, name: "stash@{2}" } });
  });
});
