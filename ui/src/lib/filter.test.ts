import { describe, expect, it } from "vitest";
import type { RefKind, RefLabel } from "./bindings";
import { NO_FILTER, describeFilter, draftFilter, isFiltered, matchRefs, shortRefName } from "./filter";

function ref(kind: RefKind, fullName: string): RefLabel {
  return { kind, name: shortRefName(fullName), fullName, isHead: false, upstream: null };
}

const REFS = [
  ref("localBranch", "refs/heads/feature/login"),
  ref("localBranch", "refs/heads/main"),
  ref("remoteBranch", "refs/remotes/origin/main"),
  ref("tag", "refs/tags/v1.0"),
];

describe("describing filters", () => {
  it("shortens ref names the way the graph labels them", () => {
    expect(shortRefName("refs/heads/feature/login")).toBe("feature/login");
    expect(shortRefName("refs/remotes/origin/main")).toBe("origin/main");
    expect(shortRefName("refs/tags/v1.0")).toBe("v1.0");
    expect(shortRefName("HEAD")).toBe("HEAD");
  });

  it("lists refs and the path, and says nothing without a filter", () => {
    expect(describeFilter({ refs: ["refs/heads/main", "refs/remotes/origin/main"], path: "src" })).toBe(
      "main, origin/main · src",
    );
    expect(describeFilter({ refs: [], path: "docs/guide.md" })).toBe("docs/guide.md");
    expect(describeFilter(NO_FILTER)).toBe("");
  });

  it("tells whether a filter hides anything", () => {
    expect(isFiltered(NO_FILTER)).toBe(false);
    expect(isFiltered({ refs: [], path: "  " })).toBe(false);
    expect(isFiltered({ refs: ["HEAD"], path: null })).toBe(true);
    expect(isFiltered({ refs: [], path: "src" })).toBe(true);
  });
});

describe("the ref picker", () => {
  it("matches short names ignoring case, and leaves stashes out", () => {
    const refs = [...REFS, ref("stash", "refs/stash")];
    expect(matchRefs(refs, "MAIN", new Set()).refs.map((r) => r.name)).toEqual(["main", "origin/main"]);
    expect(matchRefs(refs, " ", new Set()).total).toBe(4);
  });

  it("lists checked refs first, even beyond the limit", () => {
    const checked = new Set(["refs/tags/v1.0"]);
    const { refs, total } = matchRefs(REFS, "", checked, 2);
    expect(refs.map((r) => r.name)).toEqual(["v1.0", "feature/login"]);
    expect(total).toBe(4);
    expect(matchRefs(REFS, "", new Set(REFS.map((r) => r.fullName)), 1).refs).toHaveLength(4);
  });

  it("drafts a filter from the choices", () => {
    expect(draftFilter(new Set(["refs/heads/main", "refs/heads/a"]), "  src/ ")).toEqual({
      refs: ["refs/heads/a", "refs/heads/main"],
      path: "src/",
    });
    expect(draftFilter(new Set(), " ")).toEqual(NO_FILTER);
  });
});
