//! Undo: working out, from the journal alone, what the most recent operation changed and how to
//! put it back.
//!
//! The journal is never edited. An undo is a new operation that restores the values an entry
//! recorded from before it ran, and is journaled like any other; an entry an undo restored counts
//! as undone, so undoing again reaches further back.
//!
//! What undo restores, and what it can't:
//! - Local branches and tags go back to where they were, and HEAD to the branch it named when the
//!   operation was a checkout. Remote-tracking branches are left alone: they mirror the remote,
//!   which undo doesn't touch.
//! - A push can't be undone: the remote already has the commits. Undo stops there rather than
//!   reaching past it.
//! - Uncommitted changes a hard reset discarded were never stored by git; undoing the reset brings
//!   back the commits, not those changes. Applying a stash can't be undone either: the journal
//!   doesn't record the files it changed.
//! - Nothing is restored over a newer change: if a ref isn't where the entry left it, the undo is
//!   refused (compare-and-set, as for a reset).

use std::collections::HashSet;

use crate::repo::{HeadMove, RefMove, Restore, WorktreeMode};
use crate::session::journal::JournalEntry;
use crate::types::{OpId, Operation, RefChange, ResetMode, Undoable};

/// What undo would do about the most recent operation on one repository.
pub(super) enum Decision<'a> {
    Ready(&'a JournalEntry, Restore),
    Blocked(&'a JournalEntry, String),
    Nothing,
}

impl Decision<'_> {
    pub fn undoable(&self) -> Undoable {
        match self {
            Decision::Ready(entry, restore) => Undoable::Ready {
                entry: entry.id.clone(),
                operation: entry.operation.clone(),
                started_at_ms: entry.started_at_ms,
                changes: restored_changes(&entry.ref_changes),
                head: match restore {
                    Restore::Refs { head: Some(head), .. } => Some(head.to.clone()),
                    _ => None,
                },
            },
            Decision::Blocked(entry, reason) => {
                Undoable::Blocked { operation: entry.operation.clone(), reason: reason.clone() }
            }
            Decision::Nothing => Undoable::Nothing,
        }
    }
}

/// Undo's decision about the repository at `root`, given every journal entry, oldest first.
///
/// Walks back from the newest entry of that repository, passing over undos (and the entries they
/// undid) and entries that changed nothing undo restores (a fetch, a failed operation, a merge that
/// stopped for conflicts), and decides on the first one that remains.
pub(super) fn decide<'a>(entries: &'a [JournalEntry], root: &str) -> Decision<'a> {
    let mut undone: HashSet<&OpId> = HashSet::new();
    for entry in entries.iter().rev().filter(|entry| entry.repo == root) {
        if let Operation::Undo { entry: target } = &entry.operation {
            if entry.error.is_none() {
                undone.insert(target);
            }
            continue;
        }
        if undone.contains(&entry.id) {
            continue;
        }
        match plan(entry) {
            Plan::PassOver => continue,
            Plan::Blocked(reason) => return Decision::Blocked(entry, reason.to_owned()),
            Plan::Restore(restore) => return Decision::Ready(entry, restore),
        }
    }
    Decision::Nothing
}

enum Plan {
    /// Nothing to restore; look further back.
    PassOver,
    /// Can't be undone, and undoing anything older would skip over it.
    Blocked(&'static str),
    Restore(Restore),
}

fn plan(entry: &JournalEntry) -> Plan {
    let stash = entry.ref_changes.iter().find(|change| change.name == "refs/stash");
    match &entry.operation {
        // Only moves remote-tracking branches, which undo leaves alone.
        Operation::Fetch { .. } => Plan::PassOver,
        Operation::Push { .. } if entry.error.is_none() => {
            Plan::Blocked("A push can't be undone: the remote already has the commits.")
        }
        Operation::StashApply { .. } | Operation::StashPop { .. } if entry.error.is_none() => Plan::Blocked(
            "Applying a stash can't be undone: the journal doesn't record the files it changed. \
             Stash or discard those changes instead.",
        ),
        Operation::StashPush { .. } => match stash.and_then(|change| change.after) {
            Some(id) if entry.error.is_none() => Plan::Restore(Restore::Unstash { id }),
            _ => Plan::PassOver,
        },
        Operation::StashDrop { id, .. } if entry.error.is_none() => Plan::Restore(Restore::Restash { id: *id }),
        Operation::Abort if !restorable(&entry.ref_changes).is_empty() => {
            Plan::Blocked("Aborting can't be undone: what was in progress is gone.")
        }
        operation => {
            let refs: Vec<RefMove> = restorable(&entry.ref_changes)
                .into_iter()
                .map(|change| RefMove { name: change.name.clone(), from: change.after, to: change.before })
                .collect();
            // Only a checkout means to move HEAD; others (a rebase, say) leave it on the same
            // branch, detaching it only while they are in progress.
            let switches = matches!(operation, Operation::Checkout { .. } | Operation::CreateBranch { checkout: true, .. });
            let head = (switches && entry.head_before != entry.head_after)
                .then(|| HeadMove { from: entry.head_after.clone(), to: entry.head_before.clone() });
            if refs.is_empty() && head.is_none() {
                return Plan::PassOver;
            }
            let worktree = match operation {
                // Put the index and files back as the reset found them, as far as git can: a soft
                // or mixed reset left the files alone, so the undo does too.
                Operation::Reset { mode: ResetMode::Soft, .. } => WorktreeMode::Soft,
                Operation::Reset { mode: ResetMode::Mixed, .. } => WorktreeMode::Mixed,
                _ => WorktreeMode::Keep,
            };
            Plan::Restore(Restore::Refs { refs, head, worktree })
        }
    }
}

/// The changes undo puts back: local branches and tags.
fn restorable(changes: &[RefChange]) -> Vec<&RefChange> {
    changes
        .iter()
        .filter(|change| change.name.starts_with("refs/heads/") || change.name.starts_with("refs/tags/"))
        .collect()
}

fn restored_changes(changes: &[RefChange]) -> Vec<RefChange> {
    let mut restored: Vec<RefChange> = restorable(changes).into_iter().cloned().collect();
    // A stash push or drop is undone through the stash list.
    restored.extend(changes.iter().filter(|change| change.name == "refs/stash").cloned());
    restored
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::journal::SCHEMA_VERSION;
    use crate::types::{ForceMode, Oid};

    fn oid(byte: u8) -> Oid {
        Oid::from_bytes(&[byte; 20]).unwrap()
    }

    fn entry(id: &str, operation: Operation, changes: &[(&str, u8, u8)]) -> JournalEntry {
        JournalEntry {
            schema_version: SCHEMA_VERSION,
            id: OpId(id.to_owned()),
            repo: "/r".to_owned(),
            operation,
            started_at_ms: 0,
            duration_ms: 0,
            head_before: "ref: refs/heads/main".to_owned(),
            head_after: "ref: refs/heads/main".to_owned(),
            ref_changes: changes
                .iter()
                .map(|&(name, before, after)| RefChange {
                    name: name.to_owned(),
                    before: (before != 0).then(|| oid(before)),
                    after: (after != 0).then(|| oid(after)),
                })
                .collect(),
            exit_code: Some(0),
            error: None,
            output: String::new(),
        }
    }

    fn reset() -> Operation {
        Operation::Reset { branch: "main".to_owned(), to: oid(1), mode: ResetMode::Hard, expected: oid(2) }
    }

    fn decided(entries: &[JournalEntry]) -> Option<&str> {
        match decide(entries, "/r") {
            Decision::Ready(entry, _) => Some(&entry.id.0),
            Decision::Blocked(..) => Some("blocked"),
            Decision::Nothing => None,
        }
    }

    #[test]
    fn undo_reaches_back_past_undone_and_unchanged_entries() {
        let fetch = Operation::Fetch { remote: None, prune: true };
        let mut entries = vec![
            entry("reset", reset(), &[("refs/heads/main", 2, 1)]),
            entry("fetch", fetch, &[("refs/remotes/origin/main", 1, 3)]),
            entry("merge-stopped", Operation::Continue, &[]),
        ];
        assert_eq!(decided(&entries), Some("reset"));
        let Decision::Ready(_, restore) = decide(&entries, "/r") else { panic!() };
        assert_eq!(
            restore,
            Restore::Refs {
                refs: vec![RefMove { name: "refs/heads/main".to_owned(), from: Some(oid(1)), to: Some(oid(2)) }],
                head: None,
                worktree: WorktreeMode::Keep,
            }
        );

        entries.push(entry("undo", Operation::Undo { entry: OpId("reset".to_owned()) }, &[("refs/heads/main", 1, 2)]));
        assert_eq!(decided(&entries), None, "the reset is undone");

        let mut other_repo = entry("elsewhere", reset(), &[("refs/heads/main", 2, 1)]);
        other_repo.repo = "/other".to_owned();
        entries.push(other_repo);
        assert_eq!(decided(&entries), None, "other repositories' entries don't count");
    }

    #[test]
    fn pushes_block_undo() {
        let push = Operation::Push { branch: "main".to_owned(), remote: None, force: ForceMode::None, set_upstream: false };
        let entries = [
            entry("reset", reset(), &[("refs/heads/main", 2, 1)]),
            entry("push", push, &[("refs/remotes/origin/main", 2, 1)]),
        ];
        assert_eq!(decided(&entries), Some("blocked"));
    }

    #[test]
    fn a_checkout_restores_head_but_a_rebase_does_not() {
        let mut checkout = entry("checkout", Operation::Checkout { target: crate::CheckoutTarget::Commit { id: oid(1) } }, &[]);
        checkout.head_after = oid(1).to_string();
        let Decision::Ready(_, Restore::Refs { head, .. }) = decide(std::slice::from_ref(&checkout), "/r") else {
            panic!()
        };
        assert_eq!(head, Some(HeadMove { from: oid(1).to_string(), to: "ref: refs/heads/main".to_owned() }));

        let mut stopped = entry("rebase", Operation::Rebase { onto: crate::Rev::Commit { id: oid(1) } }, &[]);
        stopped.head_after = oid(1).to_string();
        assert_eq!(decided(&[stopped]), None, "a rebase that stopped moved no branch yet");
    }
}
