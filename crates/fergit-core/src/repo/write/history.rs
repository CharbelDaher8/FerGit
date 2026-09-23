//! Operations that make new commits from existing ones (merge, rebase, cherry-pick, revert), the
//! Continue / Abort / Skip that follow one that stopped for conflicts, and the stash.
//!
//! A command that stops with conflicts succeeds: git leaves the repository in a state the user
//! resolves in their own editor ([`RepoState`]), and that state is what the UI shows.

use super::{
    GitFailed, OpFailure, Writer, git_failure, integration_failure, rev_arg, rev_name,
};
use crate::repo::history::read_stashes;
use crate::types::{MergeMode, Oid, OpErrorKind, RepoState, Rev};

impl Writer<'_> {
    pub(super) fn merge(&mut self, from: &Rev, mode: MergeMode) -> Result<(), OpFailure> {
        let rev = self.shortest_unambiguous(from)?;
        let context = format!("Can't merge {}", rev_name(from));
        let mode_arg = match mode {
            MergeMode::Ff => "--ff",
            MergeMode::NoFf => "--no-ff",
            MergeMode::Squash => "--squash",
        };
        let merged = self.git(&["merge", "--no-edit", mode_arg, "--end-of-options", &rev]);
        self.stopped_for_conflicts(merged, |out| integration_failure(out, &context))?;
        if mode == MergeMode::Squash && self.state()?.conflicts().is_empty() {
            // A squash merge only stages the combined changes; commit them with the message git
            // prepared, unless there was nothing to merge.
            self.commit_prepared(&context)?;
        }
        Ok(())
    }

    pub(super) fn rebase(&mut self, onto: &Rev) -> Result<(), OpFailure> {
        let rev = rev_arg(onto)?;
        let context = format!("Can't rebase onto {}", rev_name(onto));
        let rebased = self.git(&["rebase", "--end-of-options", &rev]);
        self.stopped_for_conflicts(rebased, |out| integration_failure(out, &context))
    }

    pub(super) fn cherry_pick(&mut self, commits: &[Oid]) -> Result<(), OpFailure> {
        if commits.is_empty() {
            return Err(OpFailure::before_git(OpErrorKind::InvalidInput, "Choose at least one commit to cherry-pick."));
        }
        let ids: Vec<String> = commits.iter().map(Oid::to_string).collect();
        let mut args = vec!["cherry-pick", "--end-of-options"];
        args.extend(ids.iter().map(String::as_str));
        let context = match ids.as_slice() {
            [one] => format!("Can't cherry-pick {}", &one[..7]),
            many => format!("Can't cherry-pick {} commits", many.len()),
        };
        let picked = self.git(&args);
        self.stopped_for_conflicts(picked, |out| integration_failure(out, &context))
    }

    pub(super) fn revert(&mut self, commit: Oid) -> Result<(), OpFailure> {
        let id = commit.to_string();
        let context = format!("Can't revert {}", &id[..7]);
        let reverted = self.git(&["revert", "--no-edit", "--end-of-options", &id]);
        self.stopped_for_conflicts(reverted, |out| integration_failure(out, &context))
    }

    /// Stages the conflicted files, refusing while any still has conflict markers, then lets git
    /// carry on with what is in progress.
    pub(super) fn continue_in_progress(&mut self) -> Result<(), OpFailure> {
        let state = self.state()?;
        let conflicts = state.conflicts();
        let unresolved: Vec<&str> =
            conflicts.iter().filter(|file| !file.resolved).map(|file| file.path.as_str()).collect();
        if !unresolved.is_empty() {
            let message = format!(
                "These files still have conflict markers: {}. Resolve them in your editor, then continue.",
                unresolved.join(", ")
            );
            return Err(OpFailure::before_git(OpErrorKind::InvalidInput, message));
        }
        if !conflicts.is_empty() {
            // Literal pathspecs: a file named `:(glob)*` is a file, not a pattern. `--all` stages a
            // deletion too.
            let mut args = vec!["--literal-pathspecs", "add", "--all", "--"];
            args.extend(conflicts.iter().map(|file| file.path.as_str()));
            self.git(&args).map_err(|out| git_failure(out, "Can't stage the resolved files"))?;
        }
        let context = "Can't continue";
        let continued = match &state {
            RepoState::Clean => {
                return Err(OpFailure::before_git(OpErrorKind::InvalidInput, "Nothing is in progress to continue."));
            }
            // Staging the files was all there was to do: a stash that conflicted is applied now.
            RepoState::Unmerged { .. } => return Ok(()),
            RepoState::Merging { .. } => return self.commit_prepared(context),
            RepoState::Rebasing { .. } => self.git(&["rebase", "--continue"]),
            RepoState::CherryPicking { .. } => self.git(&["cherry-pick", "--continue"]),
            RepoState::Reverting { .. } => self.git(&["revert", "--continue"]),
        };
        self.stopped_for_conflicts(continued, |out| integration_failure(out, context))
    }

    /// Ensures nothing is in progress, putting the branch and files back as they were before it
    /// started. With nothing in progress, there is nothing to do.
    pub(super) fn abort_in_progress(&mut self) -> Result<(), OpFailure> {
        let aborted = match self.state()? {
            RepoState::Clean => return Ok(()),
            RepoState::Merging { squash: false, .. } => self.git(&["merge", "--abort"]),
            // Neither records anything to abort: put the index and the conflicted files back to
            // HEAD, keeping other uncommitted changes. A stash that conflicted is still stashed.
            RepoState::Merging { squash: true, .. } | RepoState::Unmerged { .. } => self.git(&["reset", "--merge"]),
            RepoState::Rebasing { .. } => self.git(&["rebase", "--abort"]),
            RepoState::CherryPicking { .. } => self.git(&["cherry-pick", "--abort"]),
            RepoState::Reverting { .. } => self.git(&["revert", "--abort"]),
        };
        aborted.map_err(|out| git_failure(out, "Can't abort"))
    }

    pub(super) fn skip_in_progress(&mut self) -> Result<(), OpFailure> {
        let skipped = match self.state()? {
            RepoState::Rebasing { .. } => self.git(&["rebase", "--skip"]),
            RepoState::CherryPicking { .. } => self.git(&["cherry-pick", "--skip"]),
            RepoState::Reverting { .. } => self.git(&["revert", "--skip"]),
            _ => {
                let message = "Only a rebase, cherry-pick or revert that stopped has a commit to skip.";
                return Err(OpFailure::before_git(OpErrorKind::InvalidInput, message));
            }
        };
        self.stopped_for_conflicts(skipped, |out| integration_failure(out, "Can't skip"))
    }

    pub(super) fn stash_push(&mut self, message: Option<&str>, untracked: bool) -> Result<(), OpFailure> {
        let message = message.map(str::trim).filter(|m| !m.is_empty()).map(|m| format!("--message={m}"));
        let mut args = vec!["stash", "push"];
        if untracked {
            args.push("--include-untracked");
        }
        args.extend(message.as_deref());
        let output = self.git_output(&args).map_err(|out| git_failure(out, "Can't stash the changes"))?;
        if output.stdout.contains("No local changes to save") {
            return Err(OpFailure::before_git(OpErrorKind::InvalidInput, "There are no uncommitted changes to stash."));
        }
        Ok(())
    }

    /// Applies `stash@{index}`, checked to still be `id`, and drops it afterwards if `pop`. A stash
    /// that conflicts is applied with conflicts and kept.
    pub(super) fn stash_apply(&mut self, index: u32, id: Oid, pop: bool) -> Result<(), OpFailure> {
        let name = self.stash_at(index, id)?;
        let context = format!("Can't apply {name}");
        let applied = self.git(&["stash", if pop { "pop" } else { "apply" }, &name]);
        self.stopped_for_conflicts(applied, |out| integration_failure(out, &context))
    }

    pub(super) fn stash_drop(&mut self, index: u32, id: Oid) -> Result<(), OpFailure> {
        let name = self.stash_at(index, id)?;
        self.git(&["stash", "drop", &name]).map_err(|out| git_failure(out, &format!("Can't drop {name}")))
    }

    /// `stash@{index}`, if that is still the stash `id`: the stash list renumbers whenever a stash
    /// is pushed or dropped, and the user picked a stash, not a number.
    fn stash_at(&self, index: u32, id: Oid) -> Result<String, OpFailure> {
        let stashes = read_stashes(self.repo).map_err(|err| OpFailure::before_git(OpErrorKind::Git, err.to_string()))?;
        if stashes.iter().any(|stash| stash.index == index && stash.id == id) {
            return Ok(format!("stash@{{{index}}}"));
        }
        let message = "The stash list changed since you last saw it, and that stash isn't where it was. \
                       Look at the list again, then retry.";
        Err(OpFailure::before_git(OpErrorKind::Moved, message))
    }

    /// The argument naming `rev`, shortened to the name the user knows (`topic`, `origin/topic`)
    /// if git resolves that name to the same ref. Git words merge messages after the name it was
    /// given, so this makes `Merge branch 'topic'` rather than `Merge branch 'refs/heads/topic'`.
    fn shortest_unambiguous(&self, rev: &Rev) -> Result<String, OpFailure> {
        let full = rev_arg(rev)?;
        if let Rev::Ref { name } = rev {
            let short = rev_name(rev);
            let resolved = self.repo.try_find_reference(short.as_str()).ok().flatten();
            if resolved.is_some_and(|found| found.name().as_bstr() == name.as_bytes()) {
                return Ok(short);
            }
        }
        Ok(full)
    }

    /// Commits the staged changes with the message git prepared for a merge (`MERGE_MSG`, or
    /// `SQUASH_MSG` for a squash merge). With nothing staged, a squash merge has nothing to commit.
    fn commit_prepared(&mut self, context: &str) -> Result<(), OpFailure> {
        let merging = self.repo.git_dir().join("MERGE_HEAD").is_file();
        if !merging && self.git(&["diff", "--cached", "--quiet"]).is_ok() {
            return Ok(());
        }
        self.git(&["commit", "--no-edit"]).map_err(|out: GitFailed| git_failure(out, context))
    }
}
