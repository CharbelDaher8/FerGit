// What the graph's context menu offers for a commit or one of its ref labels. Pure: which entries
// apply is decided here from the row alone; carrying one out is `actions.ts`'s job.

import type { Oid, RefLabel, Row } from "./bindings";

/** Something the user can pick from a menu, before any dialog has asked for details. */
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
  | { kind: "fetch"; remote: string | null };

export type MenuEntry = { label: string; action: MenuAction; danger?: boolean } | { separator: true };

/** A request for the context menu of `row`, or of its label `ref`, at a point on screen. */
export interface MenuRequest {
  row: Row;
  ref: RefLabel | null;
  x: number;
  y: number;
}

const SEPARATOR: MenuEntry = { separator: true };

/**
 * Entries for a right click on `row`, or on its label `ref`: the label's own entries first, then
 * those for the commit. Rows that aren't commits (uncommitted changes, stashes) get none.
 */
export function menuFor(row: Row, ref: RefLabel | null): MenuEntry[] {
  if (row.kind !== "commit") return [];
  const own = ref ? refEntries(row, ref) : [];
  const commit: MenuEntry[] = [
    { label: "Check out this commit (detached)", action: { kind: "checkoutCommit", id: row.id } },
    { label: "Create branch here…", action: { kind: "createBranch", at: row.id } },
    { label: "Create tag here…", action: { kind: "createTag", at: row.id } },
  ];
  return own.length ? [...own, SEPARATOR, ...commit] : commit;
}

function refEntries(row: Row, ref: RefLabel): MenuEntry[] {
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
      ];
    }
    case "tag":
      return [{ label: `Delete tag ${ref.name}…`, action: { kind: "deleteTag", name: ref.name }, danger: true }];
    case "head":
    case "stash":
      return [];
  }
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
