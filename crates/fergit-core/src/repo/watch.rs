//! Noticing that a repository may have changed.
//!
//! Everything visible lives in files: HEAD, refs, packed-refs, the index, the stash reflog, and the
//! worktree. Watching them is much cheaper than polling, and git's own writes and a user's edits in
//! another program look the same.

use std::path::Path;
use std::time::Duration;

use notify_debouncer_mini::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{DebounceEventResult, Debouncer, new_debouncer};

use super::{RepoError, git_error};

/// How long filesystem activity must pause before a change is reported. One git operation writes
/// several files (objects, the index, a ref and its reflog); it should produce one report.
const QUIET_PERIOD: Duration = Duration::from_millis(300);

/// Watches a repository until dropped. See [`super::Repo::watch`].
pub struct RepoWatcher {
    _debouncer: Debouncer<RecommendedWatcher>,
}

pub(super) fn watch(
    git_dir: &Path,
    workdir: Option<&Path>,
    mut on_change: impl FnMut() + Send + 'static,
) -> Result<RepoWatcher, RepoError> {
    let objects = git_dir.join("objects");
    let mut debouncer = new_debouncer(QUIET_PERIOD, move |result: DebounceEventResult| {
        let changed = match result {
            Ok(events) => events.iter().any(|event| matters(&event.path, &objects)),
            // A watcher error (the OS event buffer overflowing, say) may have swallowed changes.
            Err(_) => true,
        };
        if changed {
            on_change();
        }
    })
    .map_err(|err| git_error("Can't watch the repository for changes", err))?;

    let mut watch_dir = |dir: &Path| {
        debouncer
            .watcher()
            .watch(dir, RecursiveMode::Recursive)
            .map_err(|err| git_error(format!("Can't watch {} for changes", dir.display()), err))
    };
    match workdir {
        Some(workdir) => {
            watch_dir(workdir)?;
            // The git dir is usually `<workdir>/.git` and already covered; not for linked worktrees
            // or `--separate-git-dir`.
            if !git_dir.starts_with(workdir) {
                watch_dir(git_dir)?;
            }
        }
        None => watch_dir(git_dir)?,
    }
    Ok(RepoWatcher { _debouncer: debouncer })
}

/// Whether a change to `path` can affect what the graph shows. New objects always arrive together
/// with a ref or index update, which is reported on its own; lock files exist only during an update.
/// Anything else might matter, so it counts.
fn matters(path: &Path, objects: &Path) -> bool {
    !path.starts_with(objects) && path.extension().is_none_or(|ext| ext != "lock")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_writes_and_lock_files_do_not_matter() {
        let git_dir = Path::new("/repo/.git");
        let objects = git_dir.join("objects");
        assert!(!matters(&git_dir.join("objects/ab/cdef"), &objects));
        assert!(!matters(&git_dir.join("refs/heads/main.lock"), &objects));
        assert!(!matters(&git_dir.join("index.lock"), &objects));
        assert!(matters(&git_dir.join("refs/heads/main"), &objects));
        assert!(matters(&git_dir.join("HEAD"), &objects));
        assert!(matters(&git_dir.join("index"), &objects));
        assert!(matters(Path::new("/repo/src/main.rs"), &objects));
    }
}
