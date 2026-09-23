// Carrying out what the user picks from a menu: ask for details or confirmation if the action needs
// them, then run the operation. `planFor` holds the decisions and is pure; `perform` wires it to the
// dialogs and the backend.

import {
  commands,
  events,
  type CredentialPrompt,
  type MergeMode,
  type Operation,
  type RefChange,
  type ResetMode,
  type Rev,
  type Undoable,
} from "./bindings";
import { dialogs, type DialogSpec, type DialogValues } from "./dialogs.svelte";
import { shortId } from "./format";
import type { MenuAction } from "./menu";
import { Operations } from "./operations.svelte";
import { session } from "./session.svelte";

export const operations = new Operations((info) => session.adopt(info));

/** An operation to run, described for the user. */
export interface Run {
  op: Operation;
  title: string;
}

/**
 * How to carry out `action`: a dialog to show first (`null` to run at once), a second one to
 * confirm what was entered if that turns out to be destructive, and how to build the operation.
 */
export interface Plan {
  dialog: DialogSpec | null;
  /** A confirmation to ask for after `dialog`, given what was entered there; `null` for none. */
  confirm?: (values: DialogValues) => DialogSpec | null;
  build: (values: DialogValues) => Run;
}

const text = (values: DialogValues, key: string): string => String(values[key] ?? "").trim();
const flag = (values: DialogValues, key: string): boolean => values[key] === true;

const MERGE_MODES: { value: MergeMode; label: string; hint: string }[] = [
  { value: "ff", label: "Fast-forward if possible", hint: "otherwise create a merge commit" },
  { value: "noFf", label: "Always create a merge commit", hint: "records the merge even when it could fast-forward" },
  { value: "squash", label: "Squash", hint: "commit all the changes as one ordinary commit" },
];

const RESET_MODES: { value: ResetMode; label: string; hint: string }[] = [
  { value: "soft", label: "Soft", hint: "keep the changes, staged" },
  { value: "mixed", label: "Mixed", hint: "keep the changes in the files, unstaged" },
  { value: "hard", label: "Hard", hint: "discard all uncommitted changes to tracked files" },
];

export function planFor(action: MenuAction): Plan {
  const now = (run: Run): Plan => ({ dialog: null, build: () => run });
  switch (action.kind) {
    case "merge":
      return {
        dialog: {
          title: `Merge ${action.name} into ${action.into}`,
          message: "If files conflict, the merge stops so you can resolve them in your editor, then continue or abort.",
          fields: [{ kind: "choice", key: "mode", label: "How", options: MERGE_MODES, value: "ff" }],
          confirm: "Merge",
        },
        build: (values) => ({
          op: { kind: "merge", from: action.from, mode: String(values.mode) as MergeMode },
          title: `Merging ${action.name} into ${action.into}`,
        }),
      };
    case "rebase":
      return {
        dialog: {
          title: `Rebase ${action.branch} onto ${action.name}?`,
          message:
            `Replays the commits of ${action.branch} that ${action.name} doesn't have on top of it, as new commits. ` +
            `If ${action.branch} was pushed, pushing it again needs a force push. ` +
            "If files conflict, the rebase stops so you can resolve them, then continue, skip the commit, or abort.",
          fields: [],
          confirm: "Rebase",
        },
        build: () => ({ op: { kind: "rebase", onto: action.onto }, title: `Rebasing ${action.branch} onto ${action.name}` }),
      };
    case "cherryPick":
      return now({
        op: { kind: "cherryPick", commits: action.commits },
        title:
          action.commits.length === 1
            ? `Cherry-picking ${shortId(action.commits[0])}`
            : `Cherry-picking ${action.commits.length} commits`,
      });
    case "revert":
      return now({ op: { kind: "revert", commit: action.commit }, title: `Reverting ${shortId(action.commit)}` });
    case "reset":
      return {
        dialog: {
          title: `Reset ${action.branch} to ${shortId(action.to)}`,
          message: `Moves ${action.branch} from ${shortId(action.expected)}. If it has moved since, nothing is reset.`,
          fields: [{ kind: "choice", key: "mode", label: "Uncommitted changes", options: RESET_MODES, value: "mixed" }],
          confirm: "Reset",
        },
        confirm: (values) =>
          values.mode === "hard"
            ? {
                title: "Discard uncommitted changes?",
                message:
                  `A hard reset makes the files match ${shortId(action.to)}, discarding every uncommitted change ` +
                  "to tracked files. Undo can move the branch back, but not bring those changes back: git never " +
                  "stored them.",
                fields: [],
                confirm: "Reset and discard",
                danger: true,
              }
            : null,
        build: (values) => ({
          op: { kind: "reset", branch: action.branch, to: action.to, mode: String(values.mode) as ResetMode, expected: action.expected },
          title: `Resetting ${action.branch} to ${shortId(action.to)}`,
        }),
      };
    case "stashPush":
      return {
        dialog: {
          title: "Stash changes",
          message: "Saves the uncommitted changes as a stash and makes the files match the last commit.",
          fields: [
            { kind: "text", key: "message", label: "Message (optional)" },
            { kind: "checkbox", key: "untracked", label: "Include untracked files", value: true },
          ],
          confirm: "Stash",
        },
        build: (values) => ({
          op: { kind: "stashPush", message: text(values, "message") || null, untracked: flag(values, "untracked") },
          title: "Stashing changes",
        }),
      };
    case "stashApply":
      return now({ op: { kind: "stashApply", index: action.index, id: action.id }, title: `Applying ${action.name}` });
    case "stashPop":
      return now({ op: { kind: "stashPop", index: action.index, id: action.id }, title: `Popping ${action.name}` });
    case "stashDrop":
      return {
        dialog: {
          title: `Drop ${action.name}?`,
          message: "Its changes are deleted from the stash list. Undo can put it back right after.",
          fields: [],
          confirm: "Drop stash",
          danger: true,
        },
        build: () => ({ op: { kind: "stashDrop", index: action.index, id: action.id }, title: `Dropping ${action.name}` }),
      };
    case "continue":
      return now({ op: { kind: "continue" }, title: "Continuing" });
    case "skip":
      return now({ op: { kind: "skip" }, title: "Skipping the commit" });
    case "abort":
      return {
        dialog: {
          title: "Abort?",
          message: "Puts the branch and files back as they were before it started. Conflict resolutions made so far are discarded.",
          fields: [],
          confirm: "Abort",
          danger: true,
        },
        build: () => ({ op: { kind: "abort" }, title: "Aborting" }),
      };
    case "undo":
      // Needs the journal first; see `perform`.
      throw new Error("undo is planned from the journal");
    case "checkoutBranch":
      return now({
        op: { kind: "checkout", target: { kind: "branch", name: action.name } },
        title: `Checking out ${action.name}`,
      });
    case "checkoutCommit":
      return now({
        op: { kind: "checkout", target: { kind: "commit", id: action.id } },
        title: `Checking out ${shortId(action.id)}`,
      });
    case "checkoutRemote":
      return {
        dialog: {
          title: "Check out as a new branch",
          message: `Creates a local branch at ${shortId(action.at)} that follows ${action.fullName.replace(/^refs\/remotes\//, "")}, and switches to it.`,
          fields: [{ kind: "text", key: "name", label: "Branch name", value: action.name, required: true }],
          confirm: "Check out",
        },
        build: (values) => ({
          op: { kind: "createBranch", name: text(values, "name"), at: action.at, checkout: true, upstream: action.fullName },
          title: `Checking out ${text(values, "name")}`,
        }),
      };
    case "createBranch":
      return {
        dialog: {
          title: "Create branch",
          message: `At commit ${shortId(action.at)}.`,
          fields: [
            { kind: "text", key: "name", label: "Branch name", placeholder: "feature/name", required: true },
            { kind: "checkbox", key: "checkout", label: "Check it out", value: true },
          ],
          confirm: "Create branch",
        },
        build: (values) => ({
          op: {
            kind: "createBranch",
            name: text(values, "name"),
            at: action.at,
            checkout: flag(values, "checkout"),
            upstream: null,
          },
          title: `Creating ${text(values, "name")}`,
        }),
      };
    case "createTag":
      return {
        dialog: {
          title: "Create tag",
          message: `At commit ${shortId(action.at)}. With a message, the tag is annotated.`,
          fields: [
            { kind: "text", key: "name", label: "Tag name", placeholder: "v1.0.0", required: true },
            { kind: "text", key: "message", label: "Message (optional)", multiline: true },
          ],
          confirm: "Create tag",
        },
        build: (values) => ({
          op: { kind: "createTag", name: text(values, "name"), at: action.at, message: text(values, "message") || null },
          title: `Creating tag ${text(values, "name")}`,
        }),
      };
    case "deleteBranch":
      return {
        dialog: {
          title: `Delete ${action.name}?`,
          message: `The branch is deleted locally only. Commits that no other branch or tag contains stop showing in the graph.`,
          fields: [{ kind: "checkbox", key: "force", label: "Delete even if it has commits that aren't merged", value: false }],
          confirm: "Delete branch",
          danger: true,
        },
        build: (values) => ({
          op: { kind: "deleteBranch", name: action.name, force: flag(values, "force") },
          title: `Deleting ${action.name}`,
        }),
      };
    case "deleteTag":
      return {
        dialog: {
          title: `Delete tag ${action.name}?`,
          message: "The tag is deleted locally only.",
          fields: [],
          confirm: "Delete tag",
          danger: true,
        },
        build: () => ({ op: { kind: "deleteTag", name: action.name }, title: `Deleting tag ${action.name}` }),
      };
    case "pull":
      return now({ op: { kind: "pull" }, title: `Pulling into ${action.branch}` });
    case "push":
      return now({
        op: { kind: "push", branch: action.branch, remote: null, force: { kind: "none" }, setUpstream: action.setUpstream },
        title: `Pushing ${action.branch}`,
      });
    case "forcePush":
      return {
        dialog: {
          title: `Force push ${action.branch}?`,
          message:
            `Replaces ${action.upstream} on the remote with your ${action.branch}, discarding commits only the remote has. ` +
            `If the remote branch has moved on from ${shortId(action.expected)}, the commit you last saw, nothing is replaced.`,
          fields: [],
          confirm: "Force push",
          danger: true,
        },
        build: () => ({
          op: {
            kind: "push",
            branch: action.branch,
            remote: null,
            force: { kind: "withLease", expected: action.expected },
            setUpstream: false,
          },
          title: `Force pushing ${action.branch}`,
        }),
      };
    case "fetch":
      return now({
        op: { kind: "fetch", remote: action.remote, prune: true },
        title: action.remote ? `Fetching ${action.remote}` : "Fetching all remotes",
      });
  }
}

/** Asks for whatever `action` needs, then runs it. Cancelling a dialog runs nothing. */
export async function perform(action: MenuAction): Promise<void> {
  if (action.kind === "undo") return undo();
  const plan = planFor(action);
  const values = plan.dialog ? await dialogs.ask(plan.dialog) : {};
  if (!values) return;
  const confirmation = plan.confirm?.(values);
  if (confirmation && !(await dialogs.ask(confirmation))) return;
  const { op, title } = plan.build(values);
  await operations.run(op, title);
}

/** Says what undo would do (or why it can't), and does it once confirmed. */
async function undo(): Promise<void> {
  const undoable = await commands.undoable();
  const confirmed = await dialogs.ask(undoDialog(undoable));
  if (!confirmed || undoable.kind !== "ready") return;
  await operations.run({ kind: "undo", entry: undoable.entry }, `Undoing ${describe(undoable.operation)}`);
}

/** The dialog that asks to undo `undoable`, or explains that nothing can be. */
export function undoDialog(undoable: Undoable): DialogSpec {
  switch (undoable.kind) {
    case "nothing":
      return { title: "Nothing to undo", message: "No operation made here from FerGit is left to undo.", fields: [], confirm: "OK", cancel: null };
    case "blocked":
      return { title: `Can't undo ${describe(undoable.operation)}`, message: undoable.reason, fields: [], confirm: "OK", cancel: null };
    case "ready": {
      const lines = undoable.changes.filter((change) => change.name !== "refs/stash").map(describeRestore);
      if (undoable.head) lines.push(`HEAD goes back to ${headName(undoable.head)}.`);
      const op = undoable.operation;
      if (op.kind === "stashPush") lines.push("The stashed changes go back into the files, and the stash is removed.");
      if (op.kind === "stashDrop") lines.push("The dropped stash goes back on the stash list, as stash@{0}.");
      if (op.kind === "reset" && op.mode === "hard") {
        lines.push("Uncommitted changes the hard reset discarded can't be brought back: git never stored them.");
      }
      lines.push("Only this repository changes: anything already pushed stays on the remote.");
      return {
        title: `Undo ${describe(op)}?`,
        message: lines.join("\n"),
        fields: [],
        confirm: "Undo",
      };
    }
  }
}

/** What restoring `change` does, in a sentence. */
function describeRestore(change: RefChange): string {
  const name = shortRef(change.name);
  if (change.before === null) return `${name} is deleted again.`;
  if (change.after === null) return `${name} is restored at ${shortId(change.before)}.`;
  return `${name} goes back from ${shortId(change.after)} to ${shortId(change.before)}.`;
}

/** `ref: refs/heads/main` → `main`; a commit id → its short form. */
function headName(head: string): string {
  return head.startsWith("ref: ") ? shortRef(head.slice(5)) : `commit ${shortId(head)}`;
}

function shortRef(name: string): string {
  if (name.startsWith("refs/heads/")) return name.slice(11);
  if (name.startsWith("refs/tags/")) return `tag ${name.slice(10)}`;
  return name;
}

function revName(rev: Rev): string {
  return rev.kind === "commit" ? shortId(rev.id) : shortRef(rev.name).replace(/^refs\/remotes\//, "");
}

/** `op` as a short noun phrase, for titles: "the reset of main to 1a2b3c4". */
export function describe(op: Operation): string {
  switch (op.kind) {
    case "checkout":
      return op.target.kind === "branch" ? `the checkout of ${op.target.name}` : `the checkout of ${shortId(op.target.id)}`;
    case "createBranch":
      return `creating branch ${op.name}`;
    case "deleteBranch":
      return `deleting branch ${op.name}`;
    case "createTag":
      return `creating tag ${op.name}`;
    case "deleteTag":
      return `deleting tag ${op.name}`;
    case "fetch":
      return op.remote ? `the fetch from ${op.remote}` : "the fetch";
    case "pull":
      return "the pull";
    case "push":
      return `the push of ${op.branch}`;
    case "merge":
      return `the merge of ${revName(op.from)}`;
    case "rebase":
      return `the rebase onto ${revName(op.onto)}`;
    case "cherryPick":
      return op.commits.length === 1 ? `the cherry-pick of ${shortId(op.commits[0])}` : `the cherry-pick of ${op.commits.length} commits`;
    case "revert":
      return `the revert of ${shortId(op.commit)}`;
    case "reset":
      return `the ${op.mode} reset of ${op.branch} to ${shortId(op.to)}`;
    case "continue":
      return "continuing after conflicts";
    case "abort":
      return "the abort";
    case "skip":
      return "skipping a commit";
    case "stashPush":
      return "stashing changes";
    case "stashApply":
      return `applying stash@{${op.index}}`;
    case "stashPop":
      return `popping stash@{${op.index}}`;
    case "stashDrop":
      return `dropping stash@{${op.index}}`;
    case "undo":
      return "an undo";
  }
}

/** The dialog that asks for what git's credential prompt `prompt` wants. */
export function credentialDialog(prompt: CredentialPrompt): DialogSpec {
  return {
    title: "Git needs credentials",
    message: prompt.text.trim(),
    fields: [
      {
        kind: "text",
        key: "answer",
        label: prompt.secret ? "Password or token" : "Answer",
        secret: prompt.secret,
      },
    ],
    confirm: "Continue",
  };
}

/**
 * Answers git's credential prompts with dialogs, until the returned function is called. Answers go
 * straight back to git; nothing keeps them.
 */
export function followCredentialPrompts(): () => void {
  let unlisten: (() => void) | undefined;
  let stopped = false;
  void events.credentialPrompt
    .listen((event) => {
      const prompt = event.payload;
      void dialogs.ask(credentialDialog(prompt)).then((values) => {
        void commands.answerPrompt(prompt.id, values ? String(values.answer ?? "") : null);
      });
    })
    .then((stop) => {
      if (stopped) stop();
      else unlisten = stop;
    });
  return () => {
    stopped = true;
    unlisten?.();
  };
}
