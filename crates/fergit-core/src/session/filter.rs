//! Choosing the commits a [`Filter`] shows.

use crate::id_map::IdMap;
use crate::repo::{CommitTable, History, PathChange, Repo, RepoError};
use crate::types::{Filter, Oid};

/// How commits change one path, by commit, as read so far. Commit contents never change, so the
/// entries stay valid across snapshots until the path filtered on changes.
///
/// A result also depends on which parents the history holds, which differs only across a shallow
/// clone's boundary; deepening the clone can leave a stale entry for a boundary commit.
#[derive(Debug, Default)]
pub(super) struct PathChanges {
    path: String,
    changes: IdMap<PathChange>,
}

/// The commits of `history` that `filter` shows, reconnected into a history of their own, or
/// `None` if it shows them all. `cache` holds what earlier calls read about the path.
pub(super) fn apply(
    repo: &Repo,
    history: &History,
    filter: &Filter,
    cache: &mut PathChanges,
) -> Result<Option<CommitTable>, RepoError> {
    if filter.is_empty() {
        return Ok(None);
    }
    let commits = &history.commits;

    let reachable = (!filter.refs.is_empty()).then(|| {
        let head = filter.refs.iter().any(|name| name == "HEAD").then(|| history.tips.head.id()).flatten();
        let tips: Vec<Oid> = history
            .tips
            .refs
            .iter()
            .filter(|(_, label)| filter.refs.contains(&label.full_name))
            .map(|(id, _)| *id)
            .chain(head)
            .collect();
        let rows: Vec<usize> = commits.rows_of(&tips).into_iter().flatten().collect();
        commits.reachable_from(&rows)
    });
    let selected = |row: usize| reachable.as_ref().is_none_or(|reachable| reachable[row]);

    let Some(path) = &filter.path else {
        return Ok(Some(commits.subset(selected, |_| None)));
    };
    if cache.path != *path {
        *cache = PathChanges { path: path.clone(), changes: IdMap::default() };
    }
    let unread: Vec<usize> = (0..commits.len())
        .filter(|&row| selected(row) && !cache.changes.contains_key(&commits.id(row)))
        .collect();
    let read = repo.path_changes(commits, &unread, path)?;
    cache.changes.extend(unread.iter().map(|&row| commits.id(row)).zip(read));

    let change = |row: usize| cache.changes.get(&commits.id(row)).copied();
    Ok(Some(commits.subset(
        |row| selected(row) && change(row) == Some(PathChange::Changed),
        |row| match change(row) {
            Some(PathChange::SameAs(parent)) => Some(parent),
            _ => None,
        },
    )))
}
