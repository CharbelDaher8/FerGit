// Dev-only: runs the real UI against a small fabricated repository, without the Rust backend, to
// look at graph rendering (relationship labels, upstream badges) and at operations: pushes fail,
// merges, rebases, cherry-picks and reverts stop with conflicts until continued or aborted, and undo
// always offers to undo a reset. Served by `npm run dev` at
// /mock.html; the production build doesn't include it.

import { mockIPC } from "@tauri-apps/api/mocks";
import { mount } from "svelte";
import App from "../App.svelte";
import "../app.css";
import type {
  Edge,
  OpOutcome,
  Operation,
  RefLabel,
  Relation,
  RepoInfo,
  RepoState,
  Row,
  RowsPage,
  Undoable,
  Upstream,
} from "../lib/bindings";
import { session } from "../lib/session.svelte";

const up = (from: number, to: number, color: number): Edge => ({ half: "upper", from, to, color });
const down = (from: number, to: number, color: number): Edge => ({ half: "lower", from, to, color });
const through = (lane: number, color: number): Edge[] => [up(lane, lane, color), down(lane, lane, color)];

const tracking = (name: string, ahead: number, behind: number): Upstream => ({
  name,
  state: { kind: "tracking", ahead, behind, id: "0".repeat(40) },
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

let info: RepoInfo = {
  root: "C:\\mock\\relations-demo",
  name: "relations-demo",
  generation: 1,
  rowCount: rows.length,
  head: rows[0].id,
  branch: "main",
  state: { kind: "clean" },
};

mockIPC((command, payload) => {
  const args = (payload ?? {}) as Record<string, unknown>;
  switch (command) {
    case "open_repo":
      return { session: 1, info };
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
    case "undoable": {
      const undoable: Undoable = {
        kind: "ready",
        entry: "mock-reset",
        operation: { kind: "reset", branch: "main", to: rows[2].id, mode: "hard", expected: rows[0].id },
        startedAtMs: Date.now() - 60_000,
        changes: [{ name: "refs/heads/main", before: rows[0].id, after: rows[2].id }],
        head: null,
      };
      return undoable;
    }
    case "run_operation": {
      // Pushes are rejected, to show how failures look; everything else succeeds after a moment.
      const op = args.operation as Operation;
      info = { ...info, state: nextState(op, info.state) };
      const outcome: OpOutcome =
        op.kind === "push"
          ? {
              kind: "failed",
              info,
              error: {
                kind: "rejected",
                message: `origin has commits on ${op.branch} that you don't have. Pull or fetch and integrate them first, or force-push with lease to replace them.`,
                output: `To https://example.com/repo.git\n!\trefs/heads/${op.branch}:refs/heads/${op.branch}\t[rejected] (fetch first)\nDone`,
              },
            }
          : { kind: "done", info };
      return new Promise((resolve) => setTimeout(() => resolve(outcome), 800));
    }
    case "plugin:event|listen":
      return 1;
    default:
      return null;
  }
});

/** What the mock repository is in the middle of after `op`. */
function nextState(op: Operation, state: RepoState): RepoState {
  const conflicts = [
    { path: "src/parser.rs", resolved: false },
    { path: "README.md", resolved: true },
  ];
  switch (op.kind) {
    case "merge":
      return { kind: "merging", heads: [rows[3].id], squash: op.mode === "squash", message: "Merge branch 'topic'", conflicts };
    case "rebase":
      return { kind: "rebasing", branch: "main", onto: rows[3].id, at: rows[1].id, step: 1, total: 3, conflicts };
    case "cherryPick":
      return { kind: "cherryPicking", commit: op.commits[0], conflicts };
    case "revert":
      return { kind: "reverting", commit: op.commit, conflicts };
    case "continue":
    case "abort":
    case "skip":
      return { kind: "clean" };
    default:
      return state;
  }
}

window.addEventListener("unhandledrejection", (event) => {
  event.preventDefault();
  session.reportError(event.reason);
});

const target = document.getElementById("app");
if (!target) throw new Error("FerGit mock: #app element missing");
mount(App, { target });
void session.tabs.open(info.root);
