//! HEAD, refs and stashes, and the commits they make reachable.

use gix::refs::{Category, TargetRef};

use super::{Head, History, RepoError, StashEntry, Tips, git_error, lossy, status, to_object_id, to_oid, upstream, walk};
use crate::types::{Oid, RefKind, RefLabel};

pub(super) fn read(repo: &gix::Repository) -> Result<History, RepoError> {
    let tips = read_tips(repo)?;
    // A ref that peels to a tree or blob, or a stash whose base is gone, is a starting point the
    // walk skips; it simply matches no commit in the result.
    let starts: Vec<gix::ObjectId> = tips
        .head
        .id()
        .into_iter()
        .chain(tips.refs.iter().map(|(id, _)| *id))
        .chain(tips.stashes.iter().map(|stash| stash.base))
        // Usually already a listed ref, but a fetch refspec can map an upstream anywhere; its commits
        // are needed to count how far the branch diverged.
        .chain(tips.upstreams.iter().filter_map(|upstream| upstream.id))
        .map(to_object_id)
        .collect();
    let commits = walk::walk(repo, &starts)?;
    Ok(History { tips, commits })
}

pub(super) fn read_tips(repo: &gix::Repository) -> Result<Tips, RepoError> {
    let (head, head_branch) = read_head(repo)?;
    let mut refs = read_refs(repo, head_branch.as_deref())?;
    if let Head::Detached { id } = head {
        let label = RefLabel {
            kind: RefKind::Head,
            name: "HEAD".to_owned(),
            full_name: "HEAD".to_owned(),
            is_head: true,
            upstream: None,
        };
        refs.push((id, label));
    }
    refs.sort_by(|(a_id, a), (b_id, b)| (a.kind, &a.full_name, a_id).cmp(&(b.kind, &b.full_name, b_id)));
    let upstreams = upstream::read(repo, &refs)?;
    Ok(Tips {
        head,
        refs,
        stashes: read_stashes(repo)?,
        worktree_dirty: status::is_dirty(repo)?,
        upstreams,
    })
}

/// HEAD, and the full name of the branch it names (if it names one).
fn read_head(repo: &gix::Repository) -> Result<(Head, Option<String>), RepoError> {
    let mut head = repo.head().map_err(|err| git_error("Can't read HEAD", err))?;
    let branch = head
        .referent_name()
        .map(|name| (lossy(name.shorten()), lossy(name.as_bstr())));
    let id = head
        .try_peel_to_id()
        .map_err(|err| git_error("Can't resolve HEAD", err))?;
    Ok(match (branch, id) {
        (Some((name, full_name)), Some(id)) => (Head::Branch { name, id: to_oid(&id) }, Some(full_name)),
        (Some((branch, full_name)), None) => (Head::Unborn { branch }, Some(full_name)),
        (None, Some(id)) => (Head::Detached { id: to_oid(&id) }, None),
        (None, None) => return Err(RepoError::Git("HEAD is detached but names no object".to_owned())),
    })
}

/// Local branches, remote-tracking branches and tags, peeled. Whether each target is a commit is
/// left to the walk, which has to look the commit up anyway.
fn read_refs(repo: &gix::Repository, head_branch: Option<&str>) -> Result<Vec<(Oid, RefLabel)>, RepoError> {
    let context = "Can't list the repository's refs";
    let platform = repo.references().map_err(|err| git_error(context, err))?;
    let mut refs = Vec::new();
    for reference in platform.all().map_err(|err| git_error(context, err))? {
        // Like `git log --all`, skip a ref that can't be read instead of failing the whole view.
        let Ok(mut reference) = reference else {
            continue;
        };
        let name = reference.name();
        let Some((category, short_name)) = name.category_and_short_name() else {
            continue;
        };
        let kind = match category {
            Category::LocalBranch => RefKind::LocalBranch,
            Category::RemoteBranch => RefKind::RemoteBranch,
            Category::Tag => RefKind::Tag,
            _ => continue,
        };
        // `refs/remotes/origin/HEAD` is a pointer to the remote's default branch, not a branch.
        if kind == RefKind::RemoteBranch && matches!(reference.target(), TargetRef::Symbolic(_)) {
            continue;
        }
        let full_name = lossy(name.as_bstr());
        let label = RefLabel {
            kind,
            name: lossy(short_name),
            is_head: kind == RefKind::LocalBranch && head_branch == Some(full_name.as_str()),
            full_name,
            // Counting how far a branch diverged needs the history; the session fills this in from
            // `Tips::upstreams`.
            upstream: None,
        };
        // Follows symbolic refs and annotated tags (including tags of tags) to the final object.
        // A ref whose target is missing is skipped, as above.
        let Ok(id) = reference.peel_to_id() else {
            continue;
        };
        refs.push((to_oid(&id), label));
    }
    Ok(refs)
}

/// Stashes from the `refs/stash` reflog, newest first.
fn read_stashes(repo: &gix::Repository) -> Result<Vec<StashEntry>, RepoError> {
    let context = "Can't read the stash list";
    let Some(stash) = repo
        .try_find_reference("refs/stash")
        .map_err(|err| git_error(context, err))?
    else {
        return Ok(Vec::new());
    };

    let mut entries: Vec<(gix::ObjectId, String)> = Vec::new();
    {
        let mut log = stash.log_iter();
        if let Some(lines) = log.all().map_err(|err| git_error(context, err))? {
            // The reflog is oldest first. A line that doesn't parse is skipped.
            entries.extend(lines.filter_map(Result::ok).map(|line| (line.new_oid(), lossy(line.message))));
        }
    }
    // Git always writes a reflog for stashes; without one the ref itself is the only entry.
    if entries.is_empty() {
        entries.extend(stash.try_id().map(|id| (id.detach(), String::new())));
    }

    let mut stashes = Vec::with_capacity(entries.len());
    for (index, (id, message)) in entries.into_iter().rev().enumerate() {
        // An entry whose commit is gone can't be drawn, but still occupies its `stash@{n}` slot.
        let Some(base) = first_parent(repo, id)? else {
            continue;
        };
        stashes.push(StashEntry {
            id: to_oid(&id),
            base: to_oid(&base),
            index: u32::try_from(index).expect("fewer than 4 billion stashes"),
            message,
        });
    }
    Ok(stashes)
}

fn first_parent(repo: &gix::Repository, id: gix::ObjectId) -> Result<Option<gix::ObjectId>, RepoError> {
    let object = repo
        .try_find_object(id)
        .map_err(|err| git_error(format!("Can't read stash commit {id}"), err))?;
    Ok(object
        .and_then(|object| object.try_into_commit().ok())
        .and_then(|commit| commit.parent_ids().next().map(|parent| parent.detach())))
}
