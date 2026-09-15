//! Per-commit reads: summaries for the visible rows, details for the selected commit.

use gix::diff::blob::{Algorithm, Diff, InternedInput};
use gix::object::tree::diff::ChangeDetached;
use gix::objs::tree::{EntryKind, EntryMode};

use super::{CommitSummary, RepoError, git_error, lossy, to_object_id, to_oid};
use crate::types::{ChangeStatus, CommitDetails, FileChange, Oid, Signature};

/// Blobs larger than this aren't line-diffed; their files report no line counts. A diff this size
/// takes tens of milliseconds, and a commit may touch many such files.
const MAX_LINE_COUNT_BLOB_BYTES: u64 = 8 * 1024 * 1024;

/// Git's binary heuristic: a NUL byte within the first 8000 bytes.
const BINARY_PROBE_BYTES: usize = 8000;

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
fn summary_line(message: &[u8]) -> String {
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

    // Rename detection with git's defaults (50% similarity, no copies), independent of the user's
    // `diff.renames`. Similarity is computed on blobs as stored, so no filter or textconv runs.
    let options = gix::diff::Options::default().with_rewrites(Some(gix::diff::Rewrites::default()));
    let changes = repo
        .diff_tree_to_tree(old_tree.as_ref(), &new_tree, options)
        .map_err(|err| git_error(&context, err))?;

    let mut files = Vec::with_capacity(changes.len());
    for change in changes {
        if let Some(file) = file_change(repo, change)? {
            files.push(file);
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

type Side = Option<(EntryMode, gix::ObjectId)>;

fn file_change(repo: &gix::Repository, change: ChangeDetached) -> Result<Option<FileChange>, RepoError> {
    let (path, old_path, status, old, new): (_, _, _, Side, Side) = match change {
        ChangeDetached::Addition { location, entry_mode, id, .. } => {
            (location, None, ChangeStatus::Added, None, Some((entry_mode, id)))
        }
        ChangeDetached::Deletion { location, entry_mode, id, .. } => {
            (location, None, ChangeStatus::Deleted, Some((entry_mode, id)), None)
        }
        ChangeDetached::Modification { location, previous_entry_mode, previous_id, entry_mode, id } => {
            let status = if same_kind(previous_entry_mode, entry_mode) {
                ChangeStatus::Modified
            } else {
                ChangeStatus::TypeChanged
            };
            (location, None, status, Some((previous_entry_mode, previous_id)), Some((entry_mode, id)))
        }
        ChangeDetached::Rewrite {
            source_location,
            source_entry_mode,
            source_id,
            entry_mode,
            id,
            location,
            copy,
            ..
        } => {
            let status = if copy { ChangeStatus::Copied } else { ChangeStatus::Renamed };
            let old = Some((source_entry_mode, source_id));
            (location, Some(source_location), status, old, Some((entry_mode, id)))
        }
    };

    // The tree diff reports directories alongside the files in them; only files are listed. If a
    // directory replaced a file (or the reverse), what's left is that file's deletion (or addition).
    let is_file = |side: &(EntryMode, gix::ObjectId)| !side.0.is_tree();
    let (old, new) = (old.filter(is_file), new.filter(is_file));
    let status = match (old, new) {
        (None, None) => return Ok(None),
        (None, Some(_)) => ChangeStatus::Added,
        (Some(_), None) => ChangeStatus::Deleted,
        (Some(_), Some(_)) => status,
    };

    let (additions, deletions) = line_counts(repo, old, new)?;
    Ok(Some(FileChange {
        path: lossy(&path),
        old_path: old_path.map(|path| lossy(&path)),
        status,
        additions,
        deletions,
    }))
}

/// Whether both modes are the same kind of entry. Toggling the executable bit is a modification;
/// turning a file into a symlink or a submodule is a type change.
fn same_kind(a: EntryMode, b: EntryMode) -> bool {
    let class = |mode: EntryMode| match mode.kind() {
        EntryKind::Blob | EntryKind::BlobExecutable => 0,
        EntryKind::Link => 1,
        EntryKind::Commit => 2,
        EntryKind::Tree => 3,
    };
    class(a) == class(b)
}

/// Added and deleted lines between two file versions (absent sides count as empty), or `None`
/// for submodules, binary files and files too large to diff.
fn line_counts(repo: &gix::Repository, old: Side, new: Side) -> Result<(Option<u32>, Option<u32>), RepoError> {
    // A submodule entry names a commit in another repository; there are no lines to count.
    if [old, new].iter().flatten().any(|(mode, _)| mode.is_commit()) {
        return Ok((None, None));
    }
    if old.map(|(_, id)| id) == new.map(|(_, id)| id) {
        return Ok((Some(0), Some(0)));
    }
    let (Some(before), Some(after)) = (diffable_blob(repo, old)?, diffable_blob(repo, new)?) else {
        return Ok((None, None));
    };
    // Blobs as stored in git, without filters or textconv; Myers matches git's default algorithm.
    let input = InternedInput::new(before.as_slice(), after.as_slice());
    let diff = Diff::compute(Algorithm::Myers, &input);
    Ok((Some(diff.count_additions()), Some(diff.count_removals())))
}

/// The content of `side`'s blob (empty if absent), or `None` if it's binary or too large.
fn diffable_blob(repo: &gix::Repository, side: Side) -> Result<Option<Vec<u8>>, RepoError> {
    let Some((_, id)) = side else {
        return Ok(Some(Vec::new()));
    };
    let read_error = |err| git_error(format!("Can't read blob {id}"), err);
    if repo.find_header(id).map_err(read_error)?.size() > MAX_LINE_COUNT_BLOB_BYTES {
        return Ok(None);
    }
    let data = repo.find_object(id).map_err(read_error)?.detach().data;
    if data[..data.len().min(BINARY_PROBE_BYTES)].contains(&0) {
        return Ok(None);
    }
    Ok(Some(data))
}
