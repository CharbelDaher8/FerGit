//! What git is in the middle of: a merge, rebase, cherry-pick or revert that stopped, and the files
//! with conflicts.
//!
//! Git records an operation in progress as files in the git directory (`MERGE_HEAD`,
//! `rebase-merge/`, `CHERRY_PICK_HEAD`, `sequencer/`…) and conflicts as index entries at stages
//! 1–3. This reads both the way `git status` does, so the answer never disagrees with git's.

use std::collections::BTreeSet;
use std::path::Path;

use super::{RepoError, git_error, lossy};
use crate::types::{ConflictFile, Oid, RepoState};

pub(super) fn read(repo: &gix::Repository) -> Result<RepoState, RepoError> {
    let git_dir = repo.git_dir();
    let conflicts = conflicts(repo)?;
    let file = |name: &str| git_dir.join(name);

    for dir in ["rebase-merge", "rebase-apply"] {
        let dir = git_dir.join(dir);
        // `rebase-apply` alone (without `rebasing`) is `git am`, which FerGit doesn't run.
        if dir.is_dir() && (dir.ends_with("rebase-merge") || dir.join("rebasing").is_file()) {
            return Ok(rebasing(&dir, conflicts));
        }
    }
    if file("CHERRY_PICK_HEAD").is_file() {
        return Ok(RepoState::CherryPicking { commit: read_oid(&file("CHERRY_PICK_HEAD")), conflicts });
    }
    if file("MERGE_HEAD").is_file() {
        let heads = read_text(&file("MERGE_HEAD")).lines().filter_map(|line| line.trim().parse().ok()).collect();
        let message = first_line(&read_text(&file("MERGE_MSG")));
        return Ok(RepoState::Merging { heads, squash: false, message, conflicts });
    }
    if file("REVERT_HEAD").is_file() {
        return Ok(RepoState::Reverting { commit: read_oid(&file("REVERT_HEAD")), conflicts });
    }
    // A sequence of picks or reverts whose stopped commit was committed by hand: git still holds
    // the rest of the sequence, and continuing or aborting still applies.
    if let Some(command) = read_text(&file("sequencer/todo")).split_whitespace().next() {
        match command {
            "pick" | "p" => return Ok(RepoState::CherryPicking { commit: None, conflicts }),
            "revert" => return Ok(RepoState::Reverting { commit: None, conflicts }),
            _ => {}
        }
    }
    if conflicts.is_empty() {
        return Ok(RepoState::Clean);
    }
    // `git merge --squash` records no MERGE_HEAD, only the message it prepared.
    if file("SQUASH_MSG").is_file() {
        let message = first_line(&read_text(&file("SQUASH_MSG")));
        return Ok(RepoState::Merging { heads: Vec::new(), squash: true, message, conflicts });
    }
    Ok(RepoState::Unmerged { conflicts })
}

fn rebasing(dir: &Path, conflicts: Vec<ConflictFile>) -> RepoState {
    let text = |name: &str| read_text(&dir.join(name)).trim().to_owned();
    let number = |name: &str| text(name).parse().unwrap_or(0);
    let merge_backend = dir.ends_with("rebase-merge");
    let head_name = text("head-name");
    RepoState::Rebasing {
        branch: head_name.strip_prefix("refs/heads/").map(str::to_owned),
        onto: text("onto").parse().ok(),
        at: if merge_backend { text("stopped-sha").parse().ok() } else { read_oid(&dir.join("original-commit")) },
        step: number(if merge_backend { "msgnum" } else { "next" }),
        total: number(if merge_backend { "end" } else { "last" }),
        conflicts,
    }
}

/// Paths with unmerged index entries, each once, sorted, with whether the file on disk is free of
/// conflict markers.
fn conflicts(repo: &gix::Repository) -> Result<Vec<ConflictFile>, RepoError> {
    let Some(workdir) = repo.workdir() else {
        return Ok(Vec::new());
    };
    let index = match repo.open_index() {
        Ok(index) => index,
        // No index yet: a fresh repository.
        Err(gix::worktree::open_index::Error::IndexFile(gix::index::file::init::Error::Io(err)))
            if err.kind() == std::io::ErrorKind::NotFound =>
        {
            return Ok(Vec::new());
        }
        Err(err) => return Err(git_error("Can't read the index", err)),
    };
    let paths: BTreeSet<String> = index
        .entries()
        .iter()
        .filter(|entry| entry.stage_raw() != 0)
        .map(|entry| lossy(entry.path(&index)))
        .collect();
    Ok(paths
        .into_iter()
        .map(|path| {
            let resolved = !has_conflict_markers(&workdir.join(&path));
            ConflictFile { path, resolved }
        })
        .collect())
}

/// Whether the file at `path` has a line starting with git's `<<<<<<<` or `>>>>>>>` markers. A
/// missing file has none: deleting it is a resolution.
pub(super) fn has_conflict_markers(path: &Path) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    bytes
        .split(|&byte| byte == b'\n')
        .any(|line| line.starts_with(b"<<<<<<< ") || line.starts_with(b">>>>>>> "))
}

fn read_text(path: &Path) -> String {
    std::fs::read(path).map(|bytes| lossy(&bytes)).unwrap_or_default()
}

fn read_oid(path: &Path) -> Option<Oid> {
    read_text(path).trim().parse().ok()
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or_default().trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflict_markers_are_found_only_at_line_starts() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        std::fs::write(&file, "a\n<<<<<<< HEAD\nb\n=======\nc\n>>>>>>> topic\n").unwrap();
        assert!(has_conflict_markers(&file));
        std::fs::write(&file, "a\nsee <<<<<<< in the docs\n").unwrap();
        assert!(!has_conflict_markers(&file));
        assert!(!has_conflict_markers(&dir.path().join("deleted")));
    }
}
