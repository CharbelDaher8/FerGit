//! Comparing two sides (a commit, the index, the worktree): which files differ, and how.
//!
//! Every comparison is first reduced to [`Delta`]s, one per changed path, whose versions are either
//! objects (from a tree or the index) or files on disk that are only read when their content is
//! needed. Listing changes and diffing one file then share the same loading, binary and size rules
//! ([`content`]) whatever the sides are.
//!
//! # Renames
//!
//! Commit↔commit and commit↔index comparisons detect renames with git's defaults (50% similarity,
//! no copies), independent of the user's `diff.renames`. Comparisons involving the worktree follow
//! what `git status` shows by default: renames recorded in the index (`git mv`, or a deletion and an
//! addition both staged) are reported, but a tracked file deleted on disk is never paired with an
//! untracked file, because `git status` compares the index with the worktree without rename
//! detection and lists untracked files separately.

mod content;
mod worktree;

use gix::bstr::{BStr, BString, ByteSlice};
use gix::object::tree::diff::ChangeDetached;
use gix::objs::tree::{EntryKind, EntryMode};

use super::{RepoError, git_error, lossy, to_object_id};
use crate::types::{ChangeStatus, DiffSide, FileChange, FileDiff, Oid};

/// What a side holds at a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Version {
    /// An entry recorded in a tree or the index, or the commit a checked-out submodule is at.
    Object { mode: EntryMode, id: gix::ObjectId },
    /// A file on disk, read (and converted to the form git would store) when its content is needed.
    Worktree { mode: EntryMode },
}

impl Version {
    fn mode(self) -> EntryMode {
        match self {
            Version::Object { mode, .. } | Version::Worktree { mode } => mode,
        }
    }
}

/// One path that differs between two sides. The old version lives at `old_path` if set, else at
/// `path`; the new version lives at `path`. `None` means the side has nothing at that path.
#[derive(Debug, Clone)]
struct Delta {
    path: BString,
    old_path: Option<BString>,
    copy: bool,
    old: Option<Version>,
    new: Option<Version>,
}

impl Delta {
    fn old_location(&self) -> &BStr {
        self.old_path.as_ref().unwrap_or(&self.path).as_bstr()
    }

    /// The same change seen from the other side.
    fn inverted(self) -> Delta {
        match self.old_path {
            // Git never reports copies here (they aren't tracked), but seen backwards a copy is
            // simply its destination disappearing.
            Some(_) if self.copy => Delta { old_path: None, copy: false, old: self.new, new: None, ..self },
            Some(old_path) => Delta { path: old_path, old_path: Some(self.path), copy: false, old: self.new, new: self.old },
            None => Delta { old: self.new, new: self.old, ..self },
        }
    }
}

/// Lazily loaded state shared by the reads of one call.
struct Sides<'repo> {
    repo: &'repo gix::Repository,
    index: Option<gix::index::File>,
    worktree: Option<worktree::Worktree<'repo>>,
}

impl<'repo> Sides<'repo> {
    fn new(repo: &'repo gix::Repository) -> Sides<'repo> {
        Sides { repo, index: None, worktree: None }
    }

    /// The index as FerGit presents it; see [`worktree::index_view`].
    fn index(&mut self) -> Result<&gix::index::File, RepoError> {
        if self.index.is_none() {
            self.index = Some(worktree::index_view(self.repo)?);
        }
        Ok(self.index.as_ref().expect("just loaded"))
    }

    fn worktree(&mut self) -> Result<&mut worktree::Worktree<'repo>, RepoError> {
        if self.worktree.is_none() {
            self.worktree = Some(worktree::Worktree::new(self.repo)?);
        }
        Ok(self.worktree.as_mut().expect("just created"))
    }
}

pub(super) fn changes(repo: &gix::Repository, from: Option<DiffSide>, to: DiffSide) -> Result<Vec<FileChange>, RepoError> {
    use DiffSide::{Commit, Index, Worktree};

    let mut sides = Sides::new(repo);
    let deltas = match (from, to) {
        (Some(Index), Index) | (Some(Worktree), Worktree) => Vec::new(),
        (Some(Commit { id: a }), Commit { id: b }) if a == b => Vec::new(),
        (None, Commit { id }) => tree_deltas(repo, None, Some(&tree_of(repo, id)?))?,
        (Some(Commit { id: old }), Commit { id: new }) => {
            tree_deltas(repo, Some(&tree_of(repo, old)?), Some(&tree_of(repo, new)?))?
        }
        (None | Some(Commit { .. }), Index) => {
            let tree = tree_id_of(repo, from)?;
            worktree::tree_index_deltas(repo, tree, sides.index()?)?
        }
        (Some(Index), Commit { id }) => {
            let tree = tree_id_of(repo, Some(Commit { id }))?;
            invert(worktree::tree_index_deltas(repo, tree, sides.index()?)?)
        }
        (Some(Index), Worktree) => worktree::index_worktree_deltas(&mut sides)?,
        (Some(Worktree), Index) => invert(worktree::index_worktree_deltas(&mut sides)?),
        (None | Some(Commit { .. }), Worktree) => {
            let tree = tree_id_of(repo, from)?;
            worktree::tree_worktree_deltas(&mut sides, tree)?
        }
        (Some(Worktree), Commit { id }) => {
            let tree = tree_id_of(repo, Some(Commit { id }))?;
            invert(worktree::tree_worktree_deltas(&mut sides, tree)?)
        }
    };
    file_changes(&mut sides, deltas)
}

pub(super) fn file_diff(
    repo: &gix::Repository,
    from: Option<DiffSide>,
    to: DiffSide,
    path: &str,
    old_path: Option<&str>,
) -> Result<FileDiff, RepoError> {
    let old_path = old_path.unwrap_or(path);
    let mut sides = Sides::new(repo);
    let old = match from {
        Some(side) => version_at(&mut sides, side, old_path.into())?,
        None => None,
    };
    let new = version_at(&mut sides, to, path.into())?;
    content::file_diff(&mut sides, old, old_path.into(), new, path.into())
}

/// Files changed between two trees (a missing tree is empty), sorted by path.
pub(super) fn tree_files(
    repo: &gix::Repository,
    old: Option<&gix::Tree<'_>>,
    new: Option<&gix::Tree<'_>>,
) -> Result<Vec<FileChange>, RepoError> {
    let deltas = tree_deltas(repo, old, new)?;
    file_changes(&mut Sides::new(repo), deltas)
}

fn invert(deltas: Vec<Delta>) -> Vec<Delta> {
    deltas.into_iter().map(Delta::inverted).collect()
}

fn not_a_commit(id: Oid) -> RepoError {
    RepoError::Git(format!("{id} is not a commit in this repository"))
}

fn tree_of(repo: &gix::Repository, id: Oid) -> Result<gix::Tree<'_>, RepoError> {
    let context = || format!("Can't read the files of commit {id}");
    let object = repo
        .try_find_object(to_object_id(id))
        .map_err(|err| git_error(context(), err))?
        .filter(|object| object.kind == gix::objs::Kind::Commit)
        .ok_or_else(|| not_a_commit(id))?;
    object
        .into_commit()
        .tree()
        .map_err(|err| git_error(context(), err))
}

/// The tree of a commit side, or the empty tree for `None`.
fn tree_id_of(repo: &gix::Repository, side: Option<DiffSide>) -> Result<gix::ObjectId, RepoError> {
    match side {
        Some(DiffSide::Commit { id }) => Ok(tree_of(repo, id)?.id),
        None => Ok(gix::ObjectId::empty_tree(gix::hash::Kind::Sha1)),
        Some(other) => unreachable!("only commit sides have a tree, not {other:?}"),
    }
}

/// What `side` holds at `path`: `None` if nothing, or a directory that isn't a submodule.
fn version_at(sides: &mut Sides<'_>, side: DiffSide, path: &BStr) -> Result<Option<Version>, RepoError> {
    match side {
        DiffSide::Commit { id } => {
            let tree = tree_of(sides.repo, id)?;
            let entry = tree
                .lookup_entry(path.split_str("/"))
                .map_err(|err| git_error(format!("Can't read {path} in commit {id}"), err))?;
            Ok(entry
                .map(|entry| Version::Object { mode: entry.mode(), id: entry.object_id() })
                .filter(|version| !version.mode().is_tree()))
        }
        DiffSide::Index => Ok(index_entry(sides.index()?, path)),
        DiffSide::Worktree => {
            let recorded = index_entry(sides.index()?, path);
            sides.worktree()?.version_at(path, recorded)
        }
    }
}

fn index_entry(index: &gix::index::File, path: &BStr) -> Option<Version> {
    let entry = index.entry_by_path(path)?;
    let mode = entry.mode.to_tree_entry_mode()?;
    (!mode.is_tree()).then_some(Version::Object { mode, id: entry.id })
}

fn tree_deltas(
    repo: &gix::Repository,
    old: Option<&gix::Tree<'_>>,
    new: Option<&gix::Tree<'_>>,
) -> Result<Vec<Delta>, RepoError> {
    // Rename detection with git's defaults (50% similarity, no copies), independent of the user's
    // `diff.renames`. Similarity is computed on blobs as stored, so no filter or textconv runs.
    let options = gix::diff::Options::default().with_rewrites(Some(gix::diff::Rewrites::default()));
    let changes = repo
        .diff_tree_to_tree(old, new, options)
        .map_err(|err| git_error("Can't compare the two trees", err))?;
    Ok(changes.into_iter().map(tree_delta).collect())
}

fn tree_delta(change: ChangeDetached) -> Delta {
    let object = |mode, id| Some(Version::Object { mode, id });
    match change {
        ChangeDetached::Addition { location, entry_mode, id, .. } => {
            Delta { path: location, old_path: None, copy: false, old: None, new: object(entry_mode, id) }
        }
        ChangeDetached::Deletion { location, entry_mode, id, .. } => {
            Delta { path: location, old_path: None, copy: false, old: object(entry_mode, id), new: None }
        }
        ChangeDetached::Modification { location, previous_entry_mode, previous_id, entry_mode, id } => Delta {
            path: location,
            old_path: None,
            copy: false,
            old: object(previous_entry_mode, previous_id),
            new: object(entry_mode, id),
        },
        ChangeDetached::Rewrite { source_location, source_entry_mode, source_id, entry_mode, id, location, copy, .. } => {
            Delta {
                path: location,
                old_path: Some(source_location),
                copy,
                old: object(source_entry_mode, source_id),
                new: object(entry_mode, id),
            }
        }
    }
}

fn file_changes(sides: &mut Sides<'_>, deltas: Vec<Delta>) -> Result<Vec<FileChange>, RepoError> {
    let mut files = Vec::with_capacity(deltas.len());
    for delta in deltas {
        if let Some(file) = file_change(sides, delta)? {
            files.push(file);
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

fn file_change(sides: &mut Sides<'_>, delta: Delta) -> Result<Option<FileChange>, RepoError> {
    // Tree diffs report directories alongside the files in them; only files are listed. If a
    // directory replaced a file (or the reverse), what's left is that file's deletion (or addition).
    let is_file = |version: &Version| !version.mode().is_tree();
    let (old, new) = (delta.old.filter(is_file), delta.new.filter(is_file));
    let (status, old_path) = match (old, new) {
        (None, None) => return Ok(None),
        (None, Some(_)) => (ChangeStatus::Added, None),
        (Some(_), None) => (ChangeStatus::Deleted, None),
        (Some(old), Some(new)) => match &delta.old_path {
            Some(old_path) if delta.copy => (ChangeStatus::Copied, Some(old_path)),
            Some(old_path) => (ChangeStatus::Renamed, Some(old_path)),
            None if same_kind(old.mode(), new.mode()) => (ChangeStatus::Modified, None),
            None => (ChangeStatus::TypeChanged, None),
        },
    };
    let (path, old_location) = match status {
        // A deletion is reported at the path that disappeared.
        ChangeStatus::Deleted => (delta.old_location(), delta.old_location()),
        _ => (delta.path.as_bstr(), delta.old_location()),
    };
    let (additions, deletions) = content::line_counts(sides, old, old_location, new, path)?;
    Ok(Some(FileChange {
        path: lossy(path),
        old_path: old_path.map(|path| lossy(path)),
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
