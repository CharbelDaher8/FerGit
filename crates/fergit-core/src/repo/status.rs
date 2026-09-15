//! Whether the worktree has uncommitted changes.

use super::{RepoError, git_error};

/// True if the index differs from HEAD's tree, or the worktree from the index, counting untracked
/// files unless `status.showUntrackedFiles` is `no`. False for bare repositories.
///
/// Uses gix's status, which compares HEAD to the index and the index to the worktree (plus a
/// directory walk for untracked files) on parallel threads, and stops at the first change. Unlike
/// `Repository::is_dirty`, which ignores untracked files, this counts them, as git's
/// "uncommitted changes" do. Worktree files are hashed through gix's filter pipeline, from which
/// [`super::disable_configured_programs`] has removed external filter drivers.
pub(super) fn is_dirty(repo: &gix::Repository) -> Result<bool, RepoError> {
    if repo.workdir().is_none() {
        return Ok(false);
    }
    let context = "Can't read the worktree status";
    let changes = repo
        .status(gix::features::progress::Discard)
        .map_err(|err| git_error(context, err))?
        // Only whether something changed matters, not how, so skip rename detection.
        .tree_index_track_renames(gix::status::tree_index::TrackRenames::Disabled)
        .index_worktree_rewrites(None::<gix::diff::Rewrites>)
        // A submodule counts as changed only if its checked-out commit differs from the recorded
        // one. Looking inside it would run a full status in the submodule, which gix opens with its
        // configuration from disk, filter drivers included.
        .index_worktree_submodules(gix::status::Submodule::Given {
            ignore: gix::submodule::config::Ignore::Dirty,
            check_dirty: true,
        })
        .into_iter(Vec::new())
        .map_err(|err| git_error(context, err))?;
    for change in changes {
        match change.map_err(|err| git_error(context, err))? {
            gix::status::Item::TreeIndex(_) => return Ok(true),
            // `summary()` is `None` for items that aren't changes, like ignored files.
            gix::status::Item::IndexWorktree(item) => {
                if item.summary().is_some() {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}
