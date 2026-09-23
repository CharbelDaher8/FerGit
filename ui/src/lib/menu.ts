// What the graph's context menu offers for a commit or one of its ref labels. Pure: which entries
// apply is decided here from the row and the repository's state; carrying one out is `actions.ts`'s
// job.

import type { Oid, RefLabel, Rev, Row } from "./bindings";
import { shortId } from "./format";

/** Something the user can pick from a menu (or a banner button), before any dialog asked for details. */
export type MenuAction =
  | { kind: "checkoutBranch"; name: string }
  | { kind: "checkoutCommit"; id: Oid }
  /** Create a local branch following the remote-tracking branch `fullName`, and switch to it. */
  | { kind: "checkoutRemote"; fullName: string; name: string; at: Oid }
  | { kind: "createBranch"; at: Oid }
  | { kind: "createTag"; at: Oid }
  | { kind: "deleteBranch"; name: string }
  | { kind: "deleteTag"; name: string }
  | { kind: "pull"; branch: string }
  | { kind: "push"; branch: string; setUpstream: boolean }
  /** Replace `upstream` on the remote with `branch`, but only if it's still at `expected`. */
  | { kind: "forcePush"; branch: string; upstream: string; expected: Oid }
  | { kind: "fetch"; remote: string | null }
  /** Merge `from` (called `name` on screen) into the current branch `into`. */
  | { kind: "merge"; from: Rev; name: string; into: string }
  /** Rebase the current branch `branch` onto `onto` (called `name` on screen). */
  | { kind: "rebase"; onto: Rev; name: string; branch: string }
  /** Apply `commits`, oldest first. */
  | { kind: "cherryPick"; commits: Oid[] }
  | { kind: "revert"; commit: Oid }
  /** Move `branch` from `expected`, where the user sees it, to `to`. */
  | { kind: "reset"; branch: string; to: Oid; expected: Oid }
  | { kind: "stashPush" }
  /** `name` is the stash's label, `stash@{index}`. */
  | { kind: "stashApply" | "stashPop" | "stashDrop"; index: number; id: Oid; name: string }
  /** Carry on with, give up, or skip a commit of what is in progress. */
  | { kind: "continue" | "abort" | "skip" }
  /** Undo the most recent operation that can be undone, after saying what that is. */
  | { kind: "undo" };

export type MenuEntry = { label: string; action: MenuAction; danger?: boolean } | { separator: true };

/** A request for the context menu of `row`, or of its label `ref`, at a point on screen. */
export interface MenuRequest {
  row: Row;
  ref: RefLabel | null;
  x: number;
  y: number;
}

/** What the menu needs to know about the repository beyond the row. */
export interface MenuContext {
  /** The current branch; `null` when HEAD is detached. */
  branch: string | null;
  /** The commit HEAD points to. */
  head: Oid | null;
  /** A merge, rebase, cherry-pick or revert is in progress, or files have conflicts. */
  busy: boolean;
  /** The two commits being compared, if any. */
  compared: { older: Oid; newer: Oid } | null;
}

const SEPARATOR: MenuEntry = { separator: true };

/**
 * Entries for a right click on `row`, or on its label `ref`: the label's own entries first, then
 * those for the row. Commits, stashes and uncommitted changes each have their own.
 */
export function menuFor(row: Row, ref: RefLabel | null, context: MenuContext): MenuEntry[] {
  if (row.kind === "workingTree") {
    return context.busy ? [] : [{ label: "Stash changes…", action: { kind: "stashPush" } }];
  }
  if (row.kind === "stash") return stashEntries(row);
  const own = ref ? refEntries(row, ref, context) : [];
  const commit: MenuEntry[] = [
    { label: "Check out this commit (detached)", action: { kind: "checkoutCommit", id: row.id } },
    { label: "Create branch here…", action: { kind: "createBranch", at: row.id } },
    { label: "Create tag here…", action: { kind: "createTag", at: row.id } },
  ];
  const history = historyEntries(row, context);
  const sections = [own, commit, history].filter((section) => section.length > 0);
  return sections.flatMap((section, i) => (i === 0 ? section : [SEPARATOR, ...section]));
}

/** Merging, rebasing, cherry-picking, reverting and resetting relative to the commit `row`. */
function historyEntries(row: Row, { branch, head, busy, compared }: MenuContext): MenuEntry[] {
  if (busy || head === null) return [];
  const entries: MenuEntry[] = [];
  const short = shortId(row.id);
  const isHead = row.id === head;
  if (branch && !isHead) {
    const from: Rev = { kind: "commit", id: row.id };
    entries.push(
      { label: `Merge commit ${short} into ${branch}…`, action: { kind: "merge", from, name: short, into: branch } },
      { label: `Rebase ${branch} onto commit ${short}…`, action: { kind: "rebase", onto: from, name: short, branch } },
    );
  }
  if (compared && (row.id === compared.older || row.id === compared.newer)) {
    entries.push({ label: "Cherry-pick both compared commits", action: { kind: "cherryPick", commits: [compared.older, compared.newer] } });
  } else if (!isHead) {
    entries.push({ label: "Cherry-pick this commit", action: { kind: "cherryPick", commits: [row.id] } });
  }
  entries.push({ label: "Revert this commit", action: { kind: "revert", commit: row.id } });
  if (branch && !isHead) {
    entries.push({
      label: `Reset ${branch} to this commit…`,
      action: { kind: "reset", branch, to: row.id, expected: head },
      danger: true,
    });
  }
  return entries;
}

function stashEntries(row: Row): MenuEntry[] {
  const label = row.refs.find((ref) => ref.kind === "stash");
  const index = Number(/^stash@\{(\d+)\}$/.exec(label?.name ?? "")?.[1]);
  if (!label || !Number.isInteger(index)) return [];
  const stash = { index, id: row.id, name: label.name };
  return [
    { label: `Apply ${label.name}`, action: { kind: "stashApply", ...stash } },
    { label: `Pop ${label.name}`, action: { kind: "stashPop", ...stash } },
    SEPARATOR,
    { label: `Drop ${label.name}…`, action: { kind: "stashDrop", ...stash }, danger: true },
  ];
}

function refEntries(row: Row, ref: RefLabel, context: MenuContext): MenuEntry[] {
  switch (ref.kind) {
    case "localBranch": {
      const entries: MenuEntry[] = [];
      const upstream = ref.upstream;
      const tracking = upstream?.state.kind === "tracking" ? upstream.state : null;
      if (!ref.isHead) {
        entries.push({ label: `Check out ${ref.name}`, action: { kind: "checkoutBranch", name: ref.name } });
      }
      if (ref.isHead && tracking) {
        entries.push({ label: `Pull ${upstream!.name} into ${ref.name}`, action: { kind: "pull", branch: ref.name } });
      }
      entries.push({
        label: upstream ? `Push ${ref.name}` : `Push ${ref.name} and track it`,
        action: { kind: "push", branch: ref.name, setUpstream: !upstream },
      });
      if (tracking) {
        entries.push({
          label: `Force push ${ref.name} (with lease)…`,
          action: { kind: "forcePush", branch: ref.name, upstream: upstream!.name, expected: tracking.id },
          danger: true,
        });
      }
      if (!ref.isHead) entries.push(...integrateEntries(ref, context));
      if (!ref.isHead) {
        entries.push(SEPARATOR, {
          label: `Delete ${ref.name}…`,
          action: { kind: "deleteBranch", name: ref.name },
          danger: true,
        });
      }
      return entries;
    }
    case "remoteBranch": {
      const { remote, branch } = splitRemote(ref.name);
      return [
        {
          label: `Check out ${ref.name} as a new branch…`,
          action: { kind: "checkoutRemote", fullName: ref.fullName, name: branch, at: row.id },
        },
        { label: `Fetch ${remote}`, action: { kind: "fetch", remote } },
        ...integrateEntries(ref, context),
      ];
    }
    case "tag":
      return [
        ...integrateEntries(ref, context),
        { label: `Delete tag ${ref.name}…`, action: { kind: "deleteTag", name: ref.name }, danger: true },
      ];
    case "head":
    case "stash":
      return [];
  }
}

/** Merging `ref` into the current branch, and rebasing the current branch onto it. */
function integrateEntries(ref: RefLabel, { branch, busy }: MenuContext): MenuEntry[] {
  if (!branch || busy) return [];
  const rev: Rev = { kind: "ref", name: ref.fullName };
  const entries: MenuEntry[] = [
    { label: `Merge ${ref.name} into ${branch}…`, action: { kind: "merge", from: rev, name: ref.name, into: branch } },
  ];
  if (ref.kind !== "tag") {
    entries.push({ label: `Rebase ${branch} onto ${ref.name}…`, action: { kind: "rebase", onto: rev, name: ref.name, branch } });
  }
  return entries;
}

/**
 * `origin/feature/x` → remote `origin`, branch `feature/x`. Remote names can themselves contain a
 * slash, which a short name can't show; those are rare enough to name wrongly here (the fetch then
 * reports an unknown remote, and the branch name can be edited before checking out).
 */
export function splitRemote(name: string): { remote: string; branch: string } {
  const slash = name.indexOf("/");
  return slash < 0 ? { remote: name, branch: name } : { remote: name.slice(0, slash), branch: name.slice(slash + 1) };
}
