import { beforeEach, describe, expect, it, vi } from "vitest";
import type { OpOutcome, Operation, RepoInfo } from "./bindings";
import { planFor, undoDialog } from "./actions";
import { Dialogs, canConfirm, initialValues } from "./dialogs.svelte";
import { Operations } from "./operations.svelte";

interface Call {
  id: string;
  op: Operation;
  resolve: (outcome: OpOutcome) => void;
}

// Nothing here reaches the app's backend: operations run through the fake client below.
vi.mock("./bindings", () => ({ commands: {}, events: {} }));

/** A fake backend: every operation stays pending until a test answers it. */
const backend = { calls: [] as Call[] };
const client = {
  runOperation: (id: string, op: Operation) =>
    new Promise<OpOutcome>((resolve) => backend.calls.push({ id, op, resolve })),
};

const TIP = "d".repeat(40);
const INFO: RepoInfo = { root: "/r", name: "r", generation: 7, rowCount: 3, head: null, branch: "main", state: { kind: "clean" }, filter: { refs: [], path: null } };
const FETCH: Operation = { kind: "fetch", remote: null, prune: true };
const ID = "c".repeat(40);

beforeEach(() => {
  backend.calls.length = 0;
});

describe("Operations", () => {
  it("runs a repeated request once while it is on its way", async () => {
    const adopted: RepoInfo[] = [];
    const operations = new Operations(client, (info) => adopted.push(info));

    const first = operations.run(FETCH, "Fetching");
    const second = operations.run(FETCH, "Fetching");

    expect(second).toBe(first);
    expect(backend.calls).toHaveLength(1);
    expect(operations.active.map((op) => op.title)).toEqual(["Fetching"]);

    backend.calls[0].resolve({ kind: "done", info: INFO });
    await first;
    expect(adopted).toEqual([INFO]);
    expect(operations.active).toEqual([]);

    void operations.run(FETCH, "Fetching");
    expect(backend.calls).toHaveLength(2);
    expect(backend.calls[1].id).not.toBe(backend.calls[0].id);
  });

  it("keeps the last failure until the next operation starts", async () => {
    const operations = new Operations(client, () => {});
    const running = operations.run(FETCH, "Fetching");
    const error = { kind: "authFailed" as const, message: "No credentials.", output: "fatal: …" };
    backend.calls[0].resolve({ kind: "failed", info: INFO, error });
    await running;
    expect(operations.failure).toEqual({ title: "Fetching", error });

    void operations.run({ kind: "pull" }, "Pulling");
    expect(operations.failure).toBeNull();
  });

  it("shows progress for the operation it belongs to", () => {
    const operations = new Operations(client, () => {});
    void operations.run(FETCH, "Fetching");
    operations.progress(backend.calls[0].id, "Receiving objects: 50% (1/2)");
    operations.progress("someone-else", "ignored");
    expect(operations.active[0].progress).toBe("Receiving objects: 50% (1/2)");
  });
});

describe("Dialogs", () => {
  it("shows one dialog at a time, in order", async () => {
    const dialogs = new Dialogs();
    const spec = (title: string) => ({ title, fields: [], confirm: "OK" });
    const first = dialogs.ask(spec("first"));
    const second = dialogs.ask(spec("second"));
    expect(dialogs.current?.spec.title).toBe("first");

    dialogs.close({});
    expect(await first).toEqual({});
    expect(dialogs.current?.spec.title).toBe("second");
    dialogs.close(null);
    expect(await second).toBeNull();
    expect(dialogs.current).toBeNull();
  });

  it("needs required fields filled in", () => {
    const { dialog } = planFor({ kind: "createBranch", at: ID });
    const values = initialValues(dialog!);
    expect(values).toEqual({ name: "", checkout: true });
    expect(canConfirm(dialog!, values)).toBe(false);
    expect(canConfirm(dialog!, { ...values, name: "  " })).toBe(false);
    expect(canConfirm(dialog!, { ...values, name: "topic" })).toBe(true);
  });
});

describe("planFor", () => {
  it("runs harmless actions without asking", () => {
    const plan = planFor({ kind: "push", branch: "main", setUpstream: false });
    expect(plan.dialog).toBeNull();
    expect(plan.build({}).op).toEqual({ kind: "push", branch: "main", remote: null, force: { kind: "none" }, setUpstream: false });
  });

  it("asks before deleting a branch, and forces only when told to", () => {
    const plan = planFor({ kind: "deleteBranch", name: "topic" });
    expect(plan.dialog?.danger).toBe(true);
    expect(plan.build(initialValues(plan.dialog!)).op).toEqual({ kind: "deleteBranch", name: "topic", force: false });
    expect(plan.build({ force: true }).op).toEqual({ kind: "deleteBranch", name: "topic", force: true });
  });

  it("force pushes only with a lease on the tip that was seen", () => {
    const plan = planFor({ kind: "forcePush", branch: "main", upstream: "origin/main", expected: ID });
    expect(plan.dialog?.danger).toBe(true);
    expect(plan.build({}).op).toMatchObject({ force: { kind: "withLease", expected: ID } });
  });

  it("merges in the mode chosen, fast-forwarding by default", () => {
    const from = { kind: "ref" as const, name: "refs/heads/topic" };
    const plan = planFor({ kind: "merge", from, name: "topic", into: "main" });
    expect(initialValues(plan.dialog!)).toEqual({ mode: "ff" });
    expect(plan.build({ mode: "squash" }).op).toEqual({ kind: "merge", from, mode: "squash" });
  });

  it("confirms a hard reset a second time, and resets only from the tip that was seen", () => {
    const plan = planFor({ kind: "reset", branch: "main", to: ID, expected: TIP });
    expect(initialValues(plan.dialog!)).toEqual({ mode: "mixed" });
    expect(plan.confirm!({ mode: "mixed" })).toBeNull();
    const hard = plan.confirm!({ mode: "hard" });
    expect(hard?.danger).toBe(true);
    expect(hard?.message).toContain("never stored them");
    expect(plan.build({ mode: "hard" }).op).toEqual({ kind: "reset", branch: "main", to: ID, mode: "hard", expected: TIP });
  });

  it("asks before dropping a stash or aborting, but not before continuing", () => {
    expect(planFor({ kind: "stashDrop", index: 1, id: ID, name: "stash@{1}" }).dialog?.danger).toBe(true);
    expect(planFor({ kind: "abort" }).dialog?.danger).toBe(true);
    expect(planFor({ kind: "continue" }).dialog).toBeNull();
    const stash = planFor({ kind: "stashPush" });
    expect(stash.build({ message: " wip ", untracked: true }).op).toEqual({ kind: "stashPush", message: "wip", untracked: true });
  });

  it("builds branches and tags from what was entered", () => {
    const branch = planFor({ kind: "createBranch", at: ID }).build({ name: " topic ", checkout: false });
    expect(branch.op).toEqual({ kind: "createBranch", name: "topic", at: ID, checkout: false, upstream: null });

    const tag = planFor({ kind: "createTag", at: ID });
    expect(tag.build({ name: "v1", message: "" }).op).toEqual({ kind: "createTag", name: "v1", at: ID, message: null });
    expect(tag.build({ name: "v1", message: "Release" }).op).toMatchObject({ message: "Release" });

    const remote = planFor({ kind: "checkoutRemote", fullName: "refs/remotes/origin/x", name: "x", at: ID });
    expect(initialValues(remote.dialog!)).toEqual({ name: "x" });
    expect(remote.build({ name: "x" }).op).toEqual({
      kind: "createBranch",
      name: "x",
      at: ID,
      checkout: true,
      upstream: "refs/remotes/origin/x",
    });
  });
});

describe("undoDialog", () => {
  const reset: Operation = { kind: "reset", branch: "main", to: ID, mode: "hard", expected: TIP };

  it("says what goes back, and what can't", () => {
    const dialog = undoDialog({
      kind: "ready",
      entry: "op-1",
      operation: reset,
      startedAtMs: 0,
      changes: [
        { name: "refs/heads/main", before: TIP, after: ID },
        { name: "refs/tags/v1", before: null, after: ID },
      ],
      head: null,
    });
    expect(dialog.title).toBe("Undo the hard reset of main to cccccccc?");
    expect(dialog.message).toContain("main goes back from cccccccc to dddddddd.");
    expect(dialog.message).toContain("tag v1 is deleted again.");
    expect(dialog.message).toContain("never stored them");
    expect(dialog.message).toContain("already pushed stays on the remote");
    expect(dialog.cancel).toBeUndefined();
  });

  it("only informs when nothing can be undone", () => {
    const blocked = undoDialog({ kind: "blocked", operation: { kind: "pull" }, reason: "A push can't be undone." });
    expect(blocked).toMatchObject({ title: "Can't undo the pull", message: "A push can't be undone.", cancel: null });
    expect(undoDialog({ kind: "nothing" }).cancel).toBeNull();
  });
});
