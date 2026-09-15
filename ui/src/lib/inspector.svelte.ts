import {
  commands,
  type CommitDetails,
  type DiffSide,
  type FileChange,
  type FileDiff,
  type Oid,
  type Row,
} from "./bindings";
import { shortId } from "./format";

/** Holding an arrow key moves the selection faster than commit details need to load. */
const DETAILS_DELAY_MS = 60;
/**
 * While the uncommitted-changes row is shown, its lists are re-read this often, but only when the
 * page is visible and the previous read has finished. Change events and window focus (through
 * `show`'s `version`) cover most edits; this catches the rest, such as editing an already-modified
 * file in an editor next to FerGit, which doesn't change the generation.
 */
const WORKTREE_POLL_MS = 5000;

/** The two sides of a comparison; `from: null` compares against nothing (every file is added). */
export interface Sides {
  from: DiffSide | null;
  to: DiffSide;
}

export function commitSide(id: Oid): DiffSide {
  return { kind: "commit", id };
}

/**
 * What a commit changed: from its first parent (also a merge's, matching `commitDetails`' file
 * list, and a stash's base) to the commit itself; a root commit compares against nothing.
 */
export function commitSides(id: Oid, parents: readonly Oid[]): Sides {
  return { from: parents.length > 0 ? commitSide(parents[0]) : null, to: commitSide(id) };
}

/** What is staged: HEAD's commit to the index, or nothing to the index before the first commit. */
export function stagedSides(head: Oid | null): Sides {
  return { from: head === null ? null : commitSide(head), to: { kind: "index" } };
}

/** What is changed but not staged: the index to the worktree. */
export function unstagedSides(): Sides {
  return { from: { kind: "index" }, to: { kind: "worktree" } };
}

/** Comparing two commits: the older one to the newer one. */
export function rangeSides(older: Oid, newer: Oid): Sides {
  return { from: commitSide(older), to: commitSide(newer) };
}

/** A short label for one side, for headers: a short id, "index", "working tree" or "nothing". */
export function describeSide(side: DiffSide | null): string {
  if (side === null) return "nothing";
  if (side.kind === "commit") return shortId(side.id);
  return side.kind === "index" ? "index" : "working tree";
}

/** What the details panel describes. */
export type Subject =
  | { kind: "none" }
  | { kind: "commit"; id: Oid }
  | { kind: "worktree" }
  | { kind: "range"; older: Oid; newer: Oid };

/** The parts of a `RepoView` a subject is derived from. */
export interface Selection {
  readonly selected: number | null;
  readonly comparison: { older: number; newer: number } | null;
  row(index: number): Row | undefined;
}

/**
 * The subject for a view's selection; `null` while a row it needs isn't loaded (the inspector then
 * keeps what it shows).
 */
export function subjectOf(view: Selection): Subject | null {
  if (view.selected === null) return { kind: "none" };
  const comparison = view.comparison;
  if (comparison) {
    const older = view.row(comparison.older);
    const newer = view.row(comparison.newer);
    return older && newer ? { kind: "range", older: older.id, newer: newer.id } : null;
  }
  const row = view.row(view.selected);
  if (!row) return null;
  return row.kind === "workingTree" ? { kind: "worktree" } : { kind: "commit", id: row.id };
}

/** Identifies a subject: equal keys mean the same thing is shown. */
export function subjectKey(subject: Subject): string {
  switch (subject.kind) {
    case "commit":
      return `commit:${subject.id}`;
    case "range":
      return `range:${subject.older}:${subject.newer}`;
    default:
      return subject.kind;
  }
}

/** Names a file list within its subject. */
export type ListKey = "commit" | "staged" | "unstaged" | "range";

/** A list of changed files and the sides it was computed between. */
export interface FileList {
  key: ListKey;
  sides: Sides;
  /** `undefined` while loading. */
  files: FileChange[] | undefined;
}

/** A file diff to show. */
export interface DiffRequest extends Sides {
  /** The list the file was opened from. */
  list: ListKey;
  path: string;
  oldPath: string | null;
}

/** The diff shown in the main area. */
export interface OpenDiff {
  request: DiffRequest;
  /** `undefined` while loading. */
  result: FileDiff | undefined;
}

interface Loaded {
  sides: Sides;
  files: FileChange[];
}

/**
 * Everything shown about the selection, and the file diff opened from it.
 *
 * The owner calls `show` whenever the subject, HEAD or `version` might have changed; the inspector
 * works out what to load. Per subject:
 * - a commit or stash: its details (after a short delay), with one file list against its first
 *   parent;
 * - uncommitted changes: a staged and an unstaged list, re-read whenever `head` or `version`
 *   changes and on a light timer, since snapshots don't cover the index and worktree;
 * - two compared commits: one list from the older to the newer.
 *
 * `openFile` opens a file's diff with the sides of the list it came from. Only the latest request
 * of each kind is kept: responses for a subject no longer shown, or a diff no longer open, are
 * dropped. Changing the subject closes the diff. Reads are reactive.
 */
export class Inspector {
  #subject = $state.raw<Subject>({ kind: "none" });
  #details = $state.raw<CommitDetails | null | undefined>(undefined);
  #staged = $state.raw<Loaded | undefined>(undefined);
  #unstaged = $state.raw<Loaded | undefined>(undefined);
  #range = $state.raw<FileChange[] | undefined>(undefined);
  #diff = $state.raw<OpenDiff | null>(null);

  #key = "none";
  #head: Oid | null = null;
  #version: string | number = "";
  /** Bumped when the subject changes; responses for an earlier subject are dropped. */
  #subjectToken = 0;
  /** Bumped per worktree read; only the latest read's answer is used. */
  #worktreeToken = 0;
  #worktreeReading = false;
  #diffToken = 0;
  #detailsTimer: ReturnType<typeof setTimeout> | undefined;
  #pollTimer: ReturnType<typeof setInterval> | undefined;

  /** What is shown. */
  get subject(): Subject {
    return this.#subject;
  }

  /** The commit subject's details: `undefined` while loading, `null` if the id isn't a commit. */
  get details(): CommitDetails | null | undefined {
    return this.#details;
  }

  /** The subject's file lists, in display order. */
  get lists(): FileList[] {
    const subject = this.#subject;
    switch (subject.kind) {
      case "commit": {
        const details = this.#details;
        if (!details) return [];
        return [{ key: "commit", sides: commitSides(details.id, details.parents), files: details.files }];
      }
      case "worktree":
        return [
          { key: "staged", sides: this.#staged?.sides ?? stagedSides(this.#head), files: this.#staged?.files },
          { key: "unstaged", sides: this.#unstaged?.sides ?? unstagedSides(), files: this.#unstaged?.files },
        ];
      case "range":
        return [{ key: "range", sides: rangeSides(subject.older, subject.newer), files: this.#range }];
      default:
        return [];
    }
  }

  /** The open diff, if any. */
  get diff(): OpenDiff | null {
    return this.#diff;
  }

  /**
   * Shows `subject` (`null`: keep showing the current one, whose row is reloading). `head` is the
   * commit HEAD points to; `version` should change whenever the repository may have changed.
   */
  show(subject: Subject | null, head: Oid | null, version: string | number): void {
    const headChanged = head !== this.#head;
    const versionChanged = version !== this.#version;
    this.#head = head;
    this.#version = version;
    if (subject === null) return;

    const key = subjectKey(subject);
    if (key !== this.#key) {
      this.#switchTo(subject, key);
    } else if (subject.kind === "worktree" && (headChanged || versionChanged)) {
      this.#readWorktree();
    }
  }

  /** Opens the diff of `file` from `list`. */
  openFile(list: FileList, file: FileChange): void {
    this.#loadDiff(
      { list: list.key, from: list.sides.from, to: list.sides.to, path: file.path, oldPath: file.oldPath },
      false,
    );
  }

  closeDiff(): void {
    this.#diffToken++;
    this.#diff = null;
  }

  /** Stops timers; the inspector shows nothing afterwards. */
  dispose(): void {
    this.#switchTo({ kind: "none" }, "none");
  }

  #switchTo(subject: Subject, key: string): void {
    const token = ++this.#subjectToken;
    this.#key = key;
    this.#subject = subject;
    this.#details = undefined;
    this.#staged = undefined;
    this.#unstaged = undefined;
    this.#range = undefined;
    this.closeDiff();
    clearTimeout(this.#detailsTimer);
    clearInterval(this.#pollTimer);
    this.#pollTimer = undefined;

    switch (subject.kind) {
      case "commit":
        this.#detailsTimer = setTimeout(() => {
          void commands.commitDetails(subject.id).then((details) => {
            if (token === this.#subjectToken) this.#details = details;
          });
        }, DETAILS_DELAY_MS);
        break;
      case "worktree":
        this.#readWorktree();
        this.#pollTimer = setInterval(() => {
          if (!this.#worktreeReading && pageVisible()) this.#readWorktree();
        }, WORKTREE_POLL_MS);
        break;
      case "range": {
        const sides = rangeSides(subject.older, subject.newer);
        void commands.changes(sides.from, sides.to).then((files) => {
          if (token === this.#subjectToken) this.#range = files;
        });
        break;
      }
    }
  }

  /** Re-reads both worktree lists, keeping the current ones on screen until the new ones arrive. */
  #readWorktree(): void {
    const subjectToken = this.#subjectToken;
    const token = ++this.#worktreeToken;
    const staged = stagedSides(this.#head);
    const unstaged = unstagedSides();
    this.#worktreeReading = true;
    void Promise.all([
      commands.changes(staged.from, staged.to),
      commands.changes(unstaged.from, unstaged.to),
    ])
      .then(([stagedFiles, unstagedFiles]) => {
        if (subjectToken !== this.#subjectToken || token !== this.#worktreeToken) return;
        this.#staged = { sides: staged, files: stagedFiles };
        this.#unstaged = { sides: unstaged, files: unstagedFiles };
      })
      .finally(() => {
        if (token === this.#worktreeToken) this.#worktreeReading = false;
      });

    const open = this.#diff;
    if (open && (open.request.list === "staged" || open.request.list === "unstaged")) {
      const sides = open.request.list === "staged" ? staged : unstaged;
      this.#loadDiff({ ...open.request, ...sides }, true);
    }
  }

  /** Loads a diff; with `keep`, the current result stays on screen until the new one arrives. */
  #loadDiff(request: DiffRequest, keep: boolean): void {
    const token = ++this.#diffToken;
    const current = this.#diff;
    // Reloading the same file keeps the request object, so the view keeps its scroll position.
    const shown = keep && current && sameRequest(current.request, request) ? current.request : request;
    this.#diff = { request: shown, result: keep ? current?.result : undefined };
    void commands.fileDiff(request.from, request.to, request.path, request.oldPath).then((result) => {
      if (token === this.#diffToken) this.#diff = { request: shown, result };
    });
  }
}

function sameRequest(a: DiffRequest, b: DiffRequest): boolean {
  return (
    a.list === b.list &&
    a.path === b.path &&
    a.oldPath === b.oldPath &&
    JSON.stringify([a.from, a.to]) === JSON.stringify([b.from, b.to])
  );
}

function pageVisible(): boolean {
  return typeof document === "undefined" || document.visibilityState === "visible";
}
