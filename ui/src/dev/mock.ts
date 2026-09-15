// Dev-only: runs the real UI against a small fabricated repository, without the Rust backend, to
// look at graph rendering (relationship labels, upstream badges). Served by `npm run dev` at
// /mock.html; the production build doesn't include it.

import { mockIPC } from "@tauri-apps/api/mocks";
import { mount } from "svelte";
import App from "../App.svelte";
import "../app.css";
import type { Edge, RefLabel, Relation, RepoInfo, Row, RowsPage, Upstream } from "../lib/bindings";
import { session } from "../lib/session.svelte";

const up = (from: number, to: number, color: number): Edge => ({ half: "upper", from, to, color });
const down = (from: number, to: number, color: number): Edge => ({ half: "lower", from, to, color });
const through = (lane: number, color: number): Edge[] => [up(lane, lane, color), down(lane, lane, color)];

const tracking = (name: string, ahead: number, behind: number): Upstream => ({
  name,
  state: { kind: "tracking", ahead, behind },
});
const local = (name: string, upstream: Upstream | null = null, isHead = false): RefLabel => ({
  kind: "localBranch",
  name,
  fullName: `refs/heads/${name}`,
  isHead,
  upstream,
});
const remote = (name: string): RefLabel => ({
  kind: "remoteBranch",
  name,
  fullName: `refs/remotes/${name}`,
  isHead: false,
  upstream: null,
});
const tag = (name: string): RefLabel => ({ kind: "tag", name, fullName: `refs/tags/${name}`, isHead: false, upstream: null });
const forked = (lane: number, branch: string | null, from: string | null): Relation => ({ kind: "branchedFrom", lane, branch, from });
const merged = (lane: number, branch: string | null, into: string | null): Relation => ({ kind: "merges", lane, branch, into });

const LONG_BRANCH = "experiment/a-rather-long-branch-name-that-needs-truncating-in-the-graph";

interface Spec {
  summary: string;
  column: number;
  color: number;
  edges: Edge[];
  relations?: Relation[];
  refs?: RefLabel[];
}

// Lane colors: 0 main, 1 feature/login and topic, 2 fix/a and the experiment, 3 fix/b.
const specs: Spec[] = [
  {
    summary: "Update changelog",
    column: 0,
    color: 0,
    edges: [down(0, 0, 0)],
    refs: [local("main", tracking("origin/main", 2, 1), true), remote("origin/main")],
  },
  {
    summary: "Merge branch 'feature/login'",
    column: 0,
    color: 0,
    edges: [up(0, 0, 0), down(0, 0, 0), down(0, 1, 1)],
    relations: [merged(1, "feature/login", "main")],
  },
  {
    summary: "Validate the login form",
    column: 1,
    color: 1,
    edges: [...through(0, 0), up(1, 1, 1), down(1, 1, 1)],
    refs: [local("feature/login", { name: "origin/feature/login", state: { kind: "gone" } })],
  },
  { summary: "Bump dependencies", column: 0, color: 0, edges: [up(0, 0, 0), down(0, 0, 0), ...through(1, 1)] },
  { summary: "Add a login form", column: 1, color: 1, edges: [...through(0, 0), up(1, 1, 1), down(1, 1, 1)] },
  {
    summary: "Release 1.2",
    column: 0,
    color: 0,
    edges: [up(0, 0, 0), up(1, 0, 1), down(0, 0, 0)],
    relations: [forked(1, "feature/login", "main")],
    refs: [tag("v1.2"), local("release/1.2", tracking("origin/release/1.2", 3, 0))],
  },
  {
    summary: "Merge fixes a and b",
    column: 0,
    color: 0,
    edges: [up(0, 0, 0), down(0, 0, 0), down(0, 1, 2), down(0, 2, 3)],
    relations: [merged(1, "fix/a", "main"), merged(2, "fix/b", "main")],
  },
  { summary: "Fix a", column: 1, color: 2, edges: [...through(0, 0), up(1, 1, 2), down(1, 1, 2), ...through(2, 3)] },
  { summary: "Fix b", column: 2, color: 3, edges: [...through(0, 0), ...through(1, 2), up(2, 2, 3), down(2, 2, 3)] },
  {
    summary: "Prepare fixes",
    column: 0,
    color: 0,
    edges: [up(0, 0, 0), up(1, 0, 2), up(2, 0, 3), down(0, 0, 0)],
    relations: [forked(1, "fix/a", "main"), forked(2, null, "main")],
  },
  {
    summary: "Merge pull request #42",
    column: 0,
    color: 0,
    edges: [up(0, 0, 0), down(0, 0, 0), down(0, 1, 1)],
    relations: [merged(1, null, null)],
  },
  {
    summary: "Topic work",
    column: 1,
    color: 1,
    edges: [...through(0, 0), up(1, 1, 1), down(1, 1, 1)],
    refs: [local("topic", tracking("origin/topic", 0, 0)), local("behind-only", tracking("origin/behind-only", 0, 5))],
  },
  {
    summary: "Start topic",
    column: 0,
    color: 0,
    edges: [up(0, 0, 0), up(1, 0, 1), down(0, 0, 0)],
    relations: [forked(1, "topic", null)],
  },
  {
    summary: "Merge the experiment",
    column: 0,
    color: 0,
    edges: [up(0, 0, 0), down(0, 0, 0), down(0, 1, 2)],
    relations: [merged(1, LONG_BRANCH, null)],
  },
  { summary: "Experiment", column: 1, color: 2, edges: [...through(0, 0), up(1, 1, 2), down(1, 1, 2)] },
  {
    summary: "Base",
    column: 0,
    color: 0,
    edges: [up(0, 0, 0), up(1, 0, 2)],
    relations: [forked(1, LONG_BRANCH, "main")],
  },
];

const rows: Row[] = specs.map((spec, index) => ({
  kind: "commit",
  id: `${(index + 1).toString(16).padStart(2, "0")}${"5a".repeat(19)}`,
  graph: { column: spec.column, color: spec.color, edges: spec.edges },
  summary: spec.summary,
  authorName: "Mock Author",
  authorEmail: "mock@example.com",
  time: 1757930000 - index * 3600,
  refs: spec.refs ?? [],
  relations: spec.relations ?? [],
}));

const info: RepoInfo = {
  root: "C:\\mock\\relations-demo",
  name: "relations-demo",
  generation: 1,
  rowCount: rows.length,
  head: rows[0].id,
};

mockIPC((command, payload) => {
  const args = (payload ?? {}) as Record<string, unknown>;
  switch (command) {
    case "open_repo":
    case "refresh":
      return info;
    case "rows": {
      const start = Math.min(Number(args.start), rows.length);
      const page: RowsPage = { generation: 1, start, total: rows.length, rows: rows.slice(start, start + Number(args.len)) };
      return page;
    }
    case "locate": {
      const row = rows.findIndex((candidate) => candidate.id === args.id);
      return { generation: 1, row: row < 0 ? null : row };
    }
    case "changes":
      return [];
    case "plugin:event|listen":
      return 1;
    default:
      return null;
  }
});

window.addEventListener("unhandledrejection", (event) => {
  event.preventDefault();
  session.reportError(event.reason);
});

const target = document.getElementById("app");
if (!target) throw new Error("FerGit mock: #app element missing");
mount(App, { target });
void session.open(info.root);
