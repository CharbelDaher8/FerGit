// What to tell the user about a merge, rebase, cherry-pick or revert that stopped. Pure.

import type { RepoState } from "./bindings";
import { shortId } from "./format";

export interface StateSummary {
  /** What stopped, e.g. "Rebasing topic onto 1a2b3c4 stopped at step 2 of 3". */
  title: string;
  /** What to do next. */
  hint: string;
  /** Files that still have conflict markers. */
  unresolved: number;
  /** The stopped commit can be left out (rebase, cherry-pick, revert). */
  canSkip: boolean;
}

/** A summary of `state`; `null` when nothing is in progress. */
export function summarize(state: RepoState): StateSummary | null {
  if (state.kind === "clean") return null;
  const conflicts = state.conflicts;
  const unresolved = conflicts.filter((file) => !file.resolved).length;
  const hint =
    unresolved > 0
      ? `${plural(unresolved, "file")} with conflicts. Resolve ${unresolved === 1 ? "it" : "them"} in your editor, then continue.`
      : conflicts.length > 0
        ? "All conflicts are resolved. Continue to carry on."
        : "Nothing conflicts. Continue to carry on.";
  switch (state.kind) {
    case "merging":
      return {
        title: `${state.squash ? "Squash merge" : "Merge"} stopped${state.message ? `: ${state.message}` : ""}`,
        hint,
        unresolved,
        canSkip: false,
      };
    case "rebasing": {
      const what = `Rebasing ${state.branch ?? "HEAD"}${state.onto ? ` onto ${shortId(state.onto)}` : ""}`;
      const where = state.at ? ` at ${shortId(state.at)}` : "";
      const step = state.total > 0 ? ` (step ${state.step} of ${state.total})` : "";
      return { title: `${what} stopped${where}${step}`, hint, unresolved, canSkip: true };
    }
    case "cherryPicking":
      return { title: `Cherry-pick stopped${state.commit ? ` at ${shortId(state.commit)}` : ""}`, hint, unresolved, canSkip: true };
    case "reverting":
      return { title: `Revert stopped${state.commit ? ` at ${shortId(state.commit)}` : ""}`, hint, unresolved, canSkip: true };
    case "unmerged":
      return {
        title: "The stash applied with conflicts (it is still stashed)",
        hint: unresolved > 0 ? hint : "All conflicts are resolved. Continue to stage the files.",
        unresolved,
        canSkip: false,
      };
  }
}

function plural(count: number, noun: string): string {
  return `${count} ${noun}${count === 1 ? "" : "s"}`;
}
