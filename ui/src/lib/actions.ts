// Carrying out what the user picks from a menu: ask for details or confirmation if the action needs
// them, then run the operation. `planFor` holds the decisions and is pure; `perform` wires it to the
// dialogs and the backend.

import { commands, events, type CredentialPrompt, type Operation } from "./bindings";
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
 * How to carry out `action`: a dialog to show first (`null` to run at once), and how to build the
 * operation from what was entered.
 */
export interface Plan {
  dialog: DialogSpec | null;
  build: (values: DialogValues) => Run;
}

const text = (values: DialogValues, key: string): string => String(values[key] ?? "").trim();
const flag = (values: DialogValues, key: string): boolean => values[key] === true;

export function planFor(action: MenuAction): Plan {
  const now = (run: Run): Plan => ({ dialog: null, build: () => run });
  switch (action.kind) {
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
  const plan = planFor(action);
  const values = plan.dialog ? await dialogs.ask(plan.dialog) : {};
  if (!values) return;
  const { op, title } = plan.build(values);
  await operations.run(op, title);
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
