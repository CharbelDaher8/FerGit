// Helpers for the filter picker: which refs to offer and how to describe a filter. The backend
// owns what a filter means; see `Filter` in the bindings.

import type { Filter, RefKind, RefLabel } from "./bindings";

/** The filter that shows every commit. */
export const NO_FILTER: Filter = { refs: [], path: null };

/** Refs the picker lists at most at once; typing narrows them down. */
export const MAX_LISTED_REFS = 200;

/** Whether `filter` hides anything. */
export function isFiltered(filter: Filter): boolean {
  return filter.refs.length > 0 || (filter.path ?? "").trim() !== "";
}

/** A full ref name as the graph labels it: `refs/heads/main` → `main`, `refs/remotes/o/x` → `o/x`. */
export function shortRefName(fullName: string): string {
  for (const prefix of ["refs/heads/", "refs/remotes/", "refs/tags/"]) {
    if (fullName.startsWith(prefix)) return fullName.slice(prefix.length);
  }
  return fullName;
}

/** One line for the toolbar, e.g. `main, origin/main · src/parser`; "" for no filter. */
export function describeFilter(filter: Filter): string {
  const parts: string[] = [];
  if (filter.refs.length > 0) parts.push(filter.refs.map(shortRefName).join(", "));
  const path = (filter.path ?? "").trim();
  if (path !== "") parts.push(path);
  return parts.join(" · ");
}

/** Headings of the picker's groups, in display order. */
export const REF_GROUPS: { kind: RefKind; title: string }[] = [
  { kind: "head", title: "Detached HEAD" },
  { kind: "localBranch", title: "Branches" },
  { kind: "remoteBranch", title: "Remote branches" },
  { kind: "tag", title: "Tags" },
];

/**
 * The refs to list for `text` (matched case-insensitively against short names, blank matches all),
 * checked ones first so a narrowed list still shows them, then in the backend's order (by kind, then
 * name). At most `limit`; `total` counts every match.
 */
export function matchRefs(
  refs: readonly RefLabel[],
  text: string,
  checked: ReadonlySet<string>,
  limit = MAX_LISTED_REFS,
): { refs: RefLabel[]; total: number } {
  const needle = text.trim().toLowerCase();
  const matches = refs.filter(
    (ref) => ref.kind !== "stash" && (needle === "" || ref.name.toLowerCase().includes(needle)),
  );
  const first = matches.filter((ref) => checked.has(ref.fullName));
  const rest = matches.filter((ref) => !checked.has(ref.fullName));
  return { refs: [...first, ...rest].slice(0, Math.max(limit, first.length)), total: matches.length };
}

/** The filter the picker's choices make: checked refs in a stable order, and the path if any. */
export function draftFilter(checked: ReadonlySet<string>, path: string): Filter {
  const trimmed = path.trim();
  return { refs: [...checked].sort(), path: trimmed === "" ? null : trimmed };
}
