//! Which commits change a path: the reads behind filtering the history by file or directory.

use super::{CommitTable, PathChange, RepoError, git_error, to_object_id};
use crate::id_map::IdMap;
use crate::types::Oid;

/// How each commit in `rows` of `commits` relates to its parents (those in the table) at `path`.
///
/// Compares the tree entry at `path` (its id and mode), which for a directory changes exactly when
/// something under it does; no diff is computed. Each commit's entry is read once per call.
pub(super) fn changes(
    repo: &gix::Repository,
    commits: &CommitTable,
    rows: &[usize],
    path: &str,
) -> Result<Vec<PathChange>, RepoError> {
    let components: Vec<&[u8]> = path.split('/').map(str::as_bytes).collect();
    let mut entries: IdMap<Option<(gix::objs::tree::EntryMode, gix::ObjectId)>> = IdMap::default();
    let mut entry = |id: Oid| -> Result<_, RepoError> {
        if let Some(&entry) = entries.get(&id) {
            return Ok(entry);
        }
        let context = || format!("Can't read {path} in commit {id}");
        let commit = repo.find_commit(to_object_id(id)).map_err(|err| git_error(context(), err))?;
        let tree = commit.tree().map_err(|err| git_error(context(), err))?;
        let found = tree
            .lookup_entry(components.iter().copied())
            .map_err(|err| git_error(context(), err))?
            .map(|entry| (entry.mode(), entry.object_id()));
        entries.insert(id, found);
        Ok(found)
    };

    rows.iter()
        .map(|&row| {
            let own = entry(commits.id(row))?;
            let parents = commits.parents(row);
            if parents.is_empty() {
                return Ok(if own.is_some() { PathChange::Changed } else { PathChange::Absent });
            }
            for (k, &parent) in parents.iter().enumerate() {
                if entry(parent)? == own {
                    return Ok(PathChange::SameAs(k));
                }
            }
            Ok(PathChange::Changed)
        })
        .collect()
}
