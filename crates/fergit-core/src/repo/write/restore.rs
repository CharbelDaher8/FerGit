//! Moving refs with compare-and-set: resetting a branch, and undoing an operation from the
//! journal.
//!
//! Every move names the value a ref must still have, and all refs move in one `git update-ref
//! --stdin` transaction, so a ref that moved since the user looked (another program committed,
//! someone ran git in a terminal) fails the whole operation instead of losing that work. When the
//! current branch moves, the index and worktree follow afterwards; if they can't (uncommitted
//! changes are in the way), the refs are moved back.

use gix::bstr::BStr;

use super::{OpFailure, Writer, git_failure};
use crate::repo::history::read_stashes;
use crate::repo::{refs, to_object_id, to_oid};
use crate::types::{Oid, OpErrorKind, RepoState, ResetMode};

/// What an undo puts back, worked out by the session from a journal entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Restore {
    /// Move `refs` and HEAD back, then bring the index and worktree along as `worktree` says.
    Refs { refs: Vec<RefMove>, head: Option<HeadMove>, worktree: WorktreeMode },
    /// Undo a stash push: apply the stash `id` again, with its staged changes, and remove it from
    /// the stash list. It must still be `stash@{0}`.
    Unstash { id: Oid },
    /// Undo a stash drop: put the stash commit `id` back on the stash list, as `stash@{0}`.
    Restash { id: Oid },
}

/// Move the ref `name` (a full name) from `from` to `to`; `None` means it doesn't exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefMove {
    pub name: String,
    pub from: Option<Oid>,
    pub to: Option<Oid>,
}

/// Point HEAD at `to` instead of `from`; each is `ref: refs/heads/<branch>` or a commit id, as in
/// [`crate::repo::RefValues::head`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadMove {
    pub from: String,
    pub to: String,
}

/// What happens to the index and worktree when HEAD ends up at a different commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeMode {
    /// Leave both alone (a soft reset).
    Soft,
    /// Make the index match the new commit, leaving files alone (a mixed reset).
    Mixed,
    /// Update the files that differ between the two commits, keeping uncommitted changes; refused
    /// if one of them has uncommitted changes (like switching branches, or `git reset --keep`).
    Keep,
    /// Make the index and files match the new commit, discarding uncommitted changes to tracked
    /// files (a hard reset).
    Hard,
}

impl Writer<'_> {
    pub(super) fn restore(&mut self, restore: &Restore) -> Result<(), OpFailure> {
        match restore {
            Restore::Refs { refs, head, worktree } => {
                for change in refs {
                    let restorable = ["refs/heads/", "refs/tags/"].iter().any(|prefix| change.name.starts_with(prefix));
                    if !restorable || gix::validate::reference::name(BStr::new(&change.name)).is_err() {
                        let message = format!("{:?} isn't a branch or tag that undo can restore.", change.name);
                        return Err(OpFailure::before_git(OpErrorKind::InvalidInput, message));
                    }
                }
                self.require_nothing_in_progress("undoing")?;
                self.move_refs(refs, head.as_ref(), *worktree, "fergit: undo")
            }
            Restore::Unstash { id } => {
                let top = read_stashes(self.repo).map_err(|err| OpFailure::before_git(OpErrorKind::Git, err.to_string()))?;
                if top.first().is_none_or(|stash| stash.index != 0 || stash.id != *id) {
                    let message = "The stash that operation made isn't the newest stash any more, so it wasn't taken back.";
                    return Err(OpFailure::before_git(OpErrorKind::Moved, message));
                }
                let popped = self.git(&["stash", "pop", "--index", "stash@{0}"]);
                self.stopped_for_conflicts(popped, |out| git_failure(out, "Can't apply the stash again"))
            }
            Restore::Restash { id } => {
                let stashes = read_stashes(self.repo).map_err(|err| OpFailure::before_git(OpErrorKind::Git, err.to_string()))?;
                if stashes.iter().any(|stash| stash.id == *id) {
                    return Ok(());
                }
                let message = self.summary(*id)?;
                let message = format!("--message={message}");
                let id = id.to_string();
                self.git(&["stash", "store", &message, &id]).map_err(|out| git_failure(out, "Can't put the stash back"))
            }
        }
    }

    pub(super) fn reset(&mut self, branch: &str, to: Oid, mode: ResetMode, expected: Oid) -> Result<(), OpFailure> {
        let full_name = super::branch_ref(branch)?;
        self.summary(to)?;
        let current = self.ref_values()?;
        let is_head = current.head == format!("ref: {full_name}");
        let worktree = match mode {
            _ if !is_head => WorktreeMode::Soft,
            ResetMode::Soft => WorktreeMode::Soft,
            ResetMode::Mixed => WorktreeMode::Mixed,
            ResetMode::Hard => WorktreeMode::Hard,
        };
        if is_head {
            self.require_nothing_in_progress("resetting")?;
        }
        let change = RefMove { name: full_name, from: Some(expected), to: Some(to) };
        let message = format!("reset: moving to {to}");
        self.move_refs(&[change], None, worktree, &message).map_err(|mut failure| {
            if failure.error.kind == OpErrorKind::Moved {
                let now = current.refs.get(&format!("refs/heads/{branch}"));
                let now = now.map_or("deleted".to_owned(), |id| format!("at {}", &id.to_string()[..7]));
                failure.error.message = format!(
                    "{branch} has moved since you last saw it (it's {now}, not at {}), so it wasn't reset. \
                     Look at what changed, then decide.",
                    &expected.to_string()[..7]
                );
            }
            failure
        })
    }

    /// Moves `refs` and HEAD, each only from the value it is expected to have, then the index and
    /// worktree if HEAD's commit changed. Changes nothing if anything isn't where it's expected;
    /// moves everything back if the worktree can't follow.
    fn move_refs(
        &mut self,
        refs: &[RefMove],
        head: Option<&HeadMove>,
        worktree: WorktreeMode,
        message: &str,
    ) -> Result<(), OpFailure> {
        let current = self.ref_values()?;
        let mut moved: Vec<&str> = refs
            .iter()
            .filter(|change| current.refs.get(&change.name).copied() != change.from)
            .map(|change| change.name.as_str())
            .collect();
        if head.is_some_and(|head| current.head != head.from) {
            moved.push("HEAD");
        }
        if !moved.is_empty() {
            return Err(moved_since(&moved));
        }

        let old_commit = self.head_commit()?;
        self.update_refs(refs, false, message)?;
        if let Some(head) = head
            && let Err(failure) = self.set_head(&head.to, message)
        {
            let _ = self.update_refs(refs, true, message);
            return Err(failure);
        }
        let new_commit = self.head_commit()?;
        if let Err(failure) = self.move_worktree(old_commit, new_commit, worktree) {
            // Put everything back: the operation either happens whole or not at all.
            if let Some(head) = head {
                let _ = self.set_head(&head.from, message);
            }
            let _ = self.update_refs(refs, true, message);
            return Err(failure);
        }
        Ok(())
    }

    /// Moves every ref in `refs` (backwards, from `to` to `from`, if `reverse`) in one transaction
    /// that fails as a whole if any ref isn't at its starting value.
    fn update_refs(&mut self, refs: &[RefMove], reverse: bool, message: &str) -> Result<(), OpFailure> {
        let mut input = String::new();
        for change in refs {
            let (from, to) = if reverse { (change.to, change.from) } else { (change.from, change.to) };
            let name = &change.name;
            match (from, to) {
                (Some(from), Some(to)) => input.push_str(&format!("update {name} {to} {from}\n")),
                (None, Some(to)) => input.push_str(&format!("create {name} {to}\n")),
                (Some(from), None) => input.push_str(&format!("delete {name} {from}\n")),
                (None, None) => {}
            }
        }
        if input.is_empty() {
            return Ok(());
        }
        let args = ["update-ref", "--create-reflog", "-m", message, "--stdin"];
        self.git_with_input(&args, input.as_bytes()).map(drop).map_err(|out| {
            let stderr = out.stderr();
            if stderr.contains("but expected") || stderr.contains("reference already exists") {
                let names: Vec<&str> = refs.iter().map(|change| change.name.as_str()).collect();
                return OpFailure { exit_code: None, ..moved_since(&names) };
            }
            git_failure(out, "Can't move the refs")
        })
    }

    fn set_head(&mut self, to: &str, message: &str) -> Result<(), OpFailure> {
        let set = match to.strip_prefix("ref: ") {
            Some(name) => {
                let branch = name.strip_prefix("refs/heads/").unwrap_or_default();
                super::branch_ref(branch)?;
                self.git(&["symbolic-ref", "-m", message, "HEAD", name])
            }
            None => {
                let id: Oid = to
                    .parse()
                    .map_err(|_| OpFailure::before_git(OpErrorKind::InvalidInput, format!("{to:?} isn't a commit id.")))?;
                self.git(&["update-ref", "--no-deref", "-m", message, "HEAD", &id.to_string()])
            }
        };
        set.map_err(|out| git_failure(out, "Can't move HEAD"))
    }

    fn move_worktree(&mut self, old: Option<Oid>, new: Option<Oid>, mode: WorktreeMode) -> Result<(), OpFailure> {
        let (Some(old), Some(new)) = (old, new) else {
            return Ok(());
        };
        if old == new || self.repo.workdir().is_none() {
            return Ok(());
        }
        let (old, new) = (old.to_string(), new.to_string());
        let moved = match mode {
            WorktreeMode::Soft => return Ok(()),
            WorktreeMode::Mixed => self.git(&["read-tree", "--reset", &new]),
            // A two-tree merge, as `git checkout` does: files that differ between the commits are
            // updated, and one with uncommitted changes stops it.
            WorktreeMode::Keep => self.git(&["read-tree", "-m", "-u", &old, &new]),
            WorktreeMode::Hard => self.git(&["read-tree", "--reset", "-u", &new]),
        };
        moved.map_err(|out| {
            let stderr = out.stderr();
            if stderr.contains("not uptodate") || stderr.contains("would be overwritten") {
                let message = "Uncommitted changes are in the way of the files that would change. \
                               Commit or stash them first.";
                return super::failure(out, OpErrorKind::LocalChanges, message.to_owned());
            }
            git_failure(out, "Can't update the files")
        })?;
        // The index's cached file stats were dropped for changed entries; refresh them so the files
        // don't all look modified. Failing just means some still need a look.
        let _ = self.git(&["update-index", "-q", "--refresh"]);
        Ok(())
    }

    /// Refuses while a merge, rebase, cherry-pick or revert is in progress, or files have
    /// conflicts: moving the branch under it would leave git's record of it inconsistent.
    fn require_nothing_in_progress(&self, doing: &str) -> Result<(), OpFailure> {
        let what = match self.state()? {
            RepoState::Clean => return Ok(()),
            RepoState::Merging { .. } => "A merge is",
            RepoState::Rebasing { .. } => "A rebase is",
            RepoState::CherryPicking { .. } => "A cherry-pick is",
            RepoState::Reverting { .. } => "A revert is",
            RepoState::Unmerged { .. } => "Resolving conflicts is",
        };
        let message = format!("{what} in progress. Continue or abort it before {doing}.");
        Err(OpFailure::before_git(OpErrorKind::InvalidInput, message))
    }

    fn ref_values(&self) -> Result<refs::RefValues, OpFailure> {
        refs::read(self.repo).map_err(|err| OpFailure::before_git(OpErrorKind::Git, err.to_string()))
    }

    /// The commit HEAD points to now; `None` on an unborn branch.
    fn head_commit(&self) -> Result<Option<Oid>, OpFailure> {
        let mut head = self.repo.head().map_err(|err| OpFailure::before_git(OpErrorKind::Git, err.to_string()))?;
        let id = head.try_peel_to_id().map_err(|err| OpFailure::before_git(OpErrorKind::Git, err.to_string()))?;
        Ok(id.map(|id| to_oid(&id)))
    }

    /// The first line of commit `id`'s message; fails if `id` isn't a commit.
    fn summary(&self, id: Oid) -> Result<String, OpFailure> {
        let not_a_commit = || {
            OpFailure::before_git(OpErrorKind::InvalidInput, format!("{} isn't a commit in this repository.", &id.to_string()[..7]))
        };
        let commit = self
            .repo
            .try_find_object(to_object_id(id))
            .ok()
            .flatten()
            .and_then(|object| object.try_into_commit().ok())
            .ok_or_else(not_a_commit)?;
        let message = commit.message_raw_sloppy();
        Ok(String::from_utf8_lossy(message).lines().next().unwrap_or_default().trim().to_owned())
    }
}

fn moved_since(names: &[&str]) -> OpFailure {
    let short: Vec<&str> = names
        .iter()
        .map(|name| name.strip_prefix("refs/heads/").or_else(|| name.strip_prefix("refs/tags/")).unwrap_or(name))
        .collect();
    let message = format!(
        "{} moved since, so nothing was changed: restoring would lose what happened there.",
        short.join(", ")
    );
    OpFailure::before_git(OpErrorKind::Moved, message)
}
