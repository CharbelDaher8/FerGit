//! Per-commit reads: summaries for the visible rows, details for the selected commit.

use super::{CommitSummary, RepoError, diff, git_error, lossy, to_object_id, to_oid};
use crate::types::{CommitDetails, FileChange, Oid, Signature};

pub(super) fn summaries(repo: &gix::Repository, ids: &[Oid]) -> Result<Vec<Option<CommitSummary>>, RepoError> {
    ids.iter()
        .map(|&id| {
            let Some(object) = find_commit(repo, id)? else {
                return Ok(None);
            };
            let commit = decode(&object, id)?;
            let author = commit
                .author()
                .map_err(|err| git_error(format!("Can't read the author of commit {id}"), err))?
                .trim();
            Ok(Some(CommitSummary {
                summary: summary_line(commit.message),
                author_name: lossy(author.name),
                author_email: lossy(author.email),
                author_time: author.seconds(),
            }))
        })
        .collect()
}

pub(super) fn details(repo: &gix::Repository, id: Oid) -> Result<Option<CommitDetails>, RepoError> {
    let Some(object) = find_commit(repo, id)? else {
        return Ok(None);
    };
    let commit = decode(&object, id)?;
    let parents: Vec<gix::ObjectId> = commit.parents().collect();
    Ok(Some(CommitDetails {
        id,
        parents: parents.iter().map(|parent| to_oid(parent)).collect(),
        author: signature(commit.author(), id, "author")?,
        committer: signature(commit.committer(), id, "committer")?,
        message: lossy(commit.message),
        files: changed_files(repo, id, commit.tree(), parents.first().copied())?,
    }))
}

fn find_commit(repo: &gix::Repository, id: Oid) -> Result<Option<gix::Object<'_>>, RepoError> {
    let object = repo
        .try_find_object(to_object_id(id))
        .map_err(|err| git_error(format!("Can't read object {id}"), err))?;
    Ok(object.filter(|object| object.kind == gix::objs::Kind::Commit))
}

fn decode<'a>(object: &'a gix::Object<'_>, id: Oid) -> Result<gix::objs::CommitRef<'a>, RepoError> {
    gix::objs::CommitRef::from_bytes(&object.data, gix::hash::Kind::Sha1)
        .map_err(|err| git_error(format!("Can't parse commit {id}"), err))
}

/// The first non-blank line, without its terminator. Git never stores blank lines before the
/// subject, but a hand-made commit can, and git's `%s` skips them too.
pub(super) fn summary_line(message: &[u8]) -> String {
    let line = message
        .split(|&byte| byte == b'\n')
        .find(|line| !line.trim_ascii().is_empty())
        .unwrap_or_default();
    lossy(line.strip_suffix(b"\r").unwrap_or(line))
}

fn signature(
    signature: Result<gix::actor::SignatureRef<'_>, gix::objs::decode::Error>,
    id: Oid,
    role: &str,
) -> Result<Signature, RepoError> {
    let signature = signature
        .map_err(|err| git_error(format!("Can't read the {role} of commit {id}"), err))?
        .trim();
    // A malformed date shouldn't hide the commit; keep whatever seconds parse, in UTC.
    let time = signature.time().unwrap_or_else(|_| gix::date::Time {
        seconds: signature.seconds(),
        offset: 0,
    });
    Ok(Signature {
        name: lossy(signature.name),
        email: lossy(signature.email),
        time: time.seconds,
        offset_minutes: time.offset / 60,
    })
}

/// Files changed between the first parent's tree (or the empty tree) and `tree`, sorted by path.
fn changed_files(
    repo: &gix::Repository,
    id: Oid,
    tree: gix::ObjectId,
    first_parent: Option<gix::ObjectId>,
) -> Result<Vec<FileChange>, RepoError> {
    let context = format!("Can't compute the files changed by commit {id}");
    let new_tree = repo.find_tree(tree).map_err(|err| git_error(&context, err))?;
    // A first parent missing from a shallow clone is treated as no parent, as git does.
    let old_tree = match first_parent {
        Some(parent) => match repo.try_find_object(parent).map_err(|err| git_error(&context, err))? {
            Some(parent) => {
                let parent = parent.try_into_commit().map_err(|err| git_error(&context, err))?;
                Some(parent.tree().map_err(|err| git_error(&context, err))?)
            }
            None => None,
        },
        None => None,
    };

    // The same comparison as `Repo::changes` between the two commits, which also detects renames.
    diff::tree_files(repo, old_tree.as_ref(), Some(&new_tree))
}
