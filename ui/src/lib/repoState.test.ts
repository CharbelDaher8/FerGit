import { describe, expect, it } from "vitest";
import type { ConflictFile } from "./bindings";
import { summarize } from "./repoState";

const A = "a".repeat(40);
const B = "b".repeat(40);
const file = (path: string, resolved = false): ConflictFile => ({ path, resolved });

describe("summarize", () => {
  it("has nothing to say about a clean repository", () => {
    expect(summarize({ kind: "clean" })).toBeNull();
  });

  it("counts the files still to resolve", () => {
    const summary = summarize({ kind: "merging", heads: [A], squash: false, message: "Merge branch 'topic'", conflicts: [file("a"), file("b", true)] });
    expect(summary).toEqual({
      title: "Merge stopped: Merge branch 'topic'",
      hint: "1 file with conflicts. Resolve it in your editor, then continue.",
      unresolved: 1,
      canSkip: false,
    });
  });

  it("says where a rebase stopped, and that its commit can be skipped", () => {
    const summary = summarize({ kind: "rebasing", branch: "topic", onto: A, at: B, step: 2, total: 3, conflicts: [file("a", true)] });
    expect(summary?.title).toBe("Rebasing topic onto aaaaaaaa stopped at bbbbbbbb (step 2 of 3)");
    expect(summary?.hint).toBe("All conflicts are resolved. Continue to carry on.");
    expect(summary?.canSkip).toBe(true);
  });

  it("explains a stash that applied with conflicts", () => {
    expect(summarize({ kind: "unmerged", conflicts: [file("a"), file("b")] })?.hint).toBe(
      "2 files with conflicts. Resolve them in your editor, then continue.",
    );
  });
});
