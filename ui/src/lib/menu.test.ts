import { describe, expect, it } from "vitest";
import type { RefLabel, Row, Upstream } from "./bindings";
import { menuFor, splitRemote, type MenuEntry } from "./menu";

const ID = "a".repeat(40);
const TIP = "b".repeat(40);

function row(refs: RefLabel[] = [], kind: Row["kind"] = "commit"): Row {
  return { kind, id: ID, graph: { column: 0, color: 0, edges: [] }, summary: "", authorName: "", authorEmail: "", time: 0, refs, relations: [] };
}

const tracking: Upstream = { name: "origin/main", state: { kind: "tracking", ahead: 1, behind: 0, id: TIP } };

function local(name: string, isHead = false, upstream: Upstream | null = null): RefLabel {
  return { kind: "localBranch", name, fullName: `refs/heads/${name}`, isHead, upstream };
}

const labels = (entries: MenuEntry[]) => entries.map((entry) => ("separator" in entry ? "---" : entry.label));

describe("context menu", () => {
  it("offers commit actions on a commit", () => {
    expect(labels(menuFor(row(), null))).toEqual([
      "Check out this commit (detached)",
      "Create branch here…",
      "Create tag here…",
    ]);
  });

  it("offers nothing for uncommitted changes or stashes", () => {
    expect(menuFor(row([], "workingTree"), null)).toEqual([]);
    expect(menuFor(row([], "stash"), null)).toEqual([]);
  });

  it("lets the current branch pull and push, but not be checked out or deleted", () => {
    const main = local("main", true, tracking);
    const entries = menuFor(row([main]), main);
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
    const entries = menuFor(row([topic]), topic);
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
    expect(labels(menuFor(row([gone]), gone)).some((label) => label.startsWith("Force push"))).toBe(false);
  });

  it("checks out a remote branch as a local branch of the same name", () => {
    const remote: RefLabel = { kind: "remoteBranch", name: "origin/feature/x", fullName: "refs/remotes/origin/feature/x", isHead: false, upstream: null };
    const [checkout, fetch] = menuFor(row([remote]), remote);
    expect(checkout).toMatchObject({
      action: { kind: "checkoutRemote", fullName: "refs/remotes/origin/feature/x", name: "feature/x", at: ID },
    });
    expect(fetch).toMatchObject({ label: "Fetch origin", action: { kind: "fetch", remote: "origin" } });
  });

  it("deletes tags only after asking", () => {
    const tag: RefLabel = { kind: "tag", name: "v1", fullName: "refs/tags/v1", isHead: false, upstream: null };
    expect(menuFor(row([tag]), tag)[0]).toMatchObject({ danger: true, action: { kind: "deleteTag", name: "v1" } });
  });

  it("splits a remote branch name at its first slash", () => {
    expect(splitRemote("origin/feature/x")).toEqual({ remote: "origin", branch: "feature/x" });
    expect(splitRemote("odd")).toEqual({ remote: "odd", branch: "odd" });
  });
});
