//! The index and worktree sides: FerGit's view of the index, stat-based change detection through
//! gix's status machinery, and reading files from disk.
//!
//! # Security
//!
//! Worktree files are converted to git's stored form by gix's filter pipeline. The pipeline's
//! built-in conversions (end of line and `core.autocrlf`, `ident`, `working-tree-encoding`) run in
//! process; the only programs it could start are filter drivers, and those were removed from the
//! configuration when the repository was opened ([`crate::repo::disable_configured_programs`]).
//! Content is never passed through textconv, and submodules are only asked for their `HEAD`.

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use gix::bstr::{BStr, BString, ByteSlice};
use gix::objs::tree::{EntryKind, EntryMode};
use gix::status::plumbing::index_as_worktree::{Change, EntryStatus};
use gix::status::plumbing::index_as_worktree_with_renames::{Entry, VisitEntry};

use super::content::{self, Content, MAX_DIFF_BYTES};
use super::{Delta, Sides, Version};
use crate::repo::{RepoError, git_error};

/// The index as FerGit presents it: each path has at most one entry.
///
/// An unmerged path (a merge conflict) keeps only its stage 2 entry, "ours", which is the version
/// from `HEAD`; if there is none (the path was deleted on our side) the path is absent. The base
/// and "theirs" stages are dropped. So during a conflict `HEAD`→index shows nothing for the path,
/// and index→worktree shows the file on disk, conflict markers included, against our version.
///
/// The view is an in-memory copy; `.git/index` is never written.
pub(super) fn index_view(repo: &gix::Repository) -> Result<gix::index::File, RepoError> {
    let index = repo.index_or_empty().map_err(|err| git_error("Can't read the index", err))?;
    let mut index = gix::index::File::clone(&index);
    index.remove_entries(|_, _, entry| match entry.stage_raw() {
        0 => false,
        2 => {
            entry.flags.remove(gix::index::entry::Flags::STAGE_MASK);
            false
        }
        _ => true,
    });
    Ok(index)
}

fn index_mode(mode: gix::index::entry::Mode) -> EntryMode {
    // Only a sparse index's directory entries have no tree mode; treat them as directories.
    mode.to_tree_entry_mode().unwrap_or_else(|| EntryKind::Tree.into())
}

/// Changes from `tree` to `index`, with git's default rename detection.
pub(super) fn tree_index_deltas(
    repo: &gix::Repository,
    tree: gix::ObjectId,
    index: &gix::index::File,
) -> Result<Vec<Delta>, RepoError> {
    use gix::diff::index::ChangeRef;

    let mut deltas = Vec::new();
    let object = |mode, id: &gix::oid| Some(Version::Object { mode: index_mode(mode), id: id.to_owned() });
    repo.tree_index_status(
        &tree,
        index,
        None,
        gix::status::tree_index::TrackRenames::Given(gix::diff::Rewrites::default()),
        |change, _, _| {
            deltas.push(match change {
                ChangeRef::Addition { location, entry_mode, id, .. } => {
                    Delta { path: location.into_owned(), old_path: None, copy: false, old: None, new: object(entry_mode, &id) }
                }
                ChangeRef::Deletion { location, entry_mode, id, .. } => {
                    Delta { path: location.into_owned(), old_path: None, copy: false, old: object(entry_mode, &id), new: None }
                }
                ChangeRef::Modification { location, previous_entry_mode, previous_id, entry_mode, id, .. } => Delta {
                    path: location.into_owned(),
                    old_path: None,
                    copy: false,
                    old: object(previous_entry_mode, &previous_id),
                    new: object(entry_mode, &id),
                },
                ChangeRef::Rewrite { source_location, source_entry_mode, source_id, location, entry_mode, id, copy, .. } => {
                    Delta {
                        path: location.into_owned(),
                        old_path: Some(source_location.into_owned()),
                        copy,
                        old: object(source_entry_mode, &source_id),
                        new: object(entry_mode, &id),
                    }
                }
            });
            Ok::<_, std::convert::Infallible>(std::ops::ControlFlow::Continue(()))
        },
    )
    .map_err(|err| git_error("Can't compare the index with a commit", err))?;
    Ok(deltas)
}

/// Changes from the index to the worktree.
pub(super) fn index_worktree_deltas(sides: &mut Sides<'_>) -> Result<Vec<Delta>, RepoError> {
    Ok(index_worktree(sides)?.into_iter().map(|(delta, _)| delta).collect())
}

/// Changes from `tree` to the worktree: what changed in the index, with each path the worktree
/// changes further replaced by its file on disk.
pub(super) fn tree_worktree_deltas(sides: &mut Sides<'_>, tree: gix::ObjectId) -> Result<Vec<Delta>, RepoError> {
    let staged = tree_index_deltas(sides.repo, tree, sides.index()?)?;
    let unstaged = index_worktree(sides)?;

    let mut staged: HashMap<BString, Delta> = staged.into_iter().map(|delta| (delta.path.clone(), delta)).collect();
    let mut deltas = Vec::with_capacity(staged.len() + unstaged.len());
    for (on_disk, disk_id) in unstaged {
        // Untouched by the index, the path's index version is the tree's version.
        let Some(in_index) = staged.remove(&on_disk.path) else {
            deltas.push(on_disk);
            continue;
        };
        let combined = match (on_disk.new, in_index.old_path) {
            // Staged as a copy, then deleted from disk: nothing changed.
            (None, Some(_)) if in_index.copy => continue,
            // Staged as a rename, then deleted from disk: the source was deleted.
            (None, Some(source)) => Delta { path: source, old_path: None, copy: false, old: in_index.old, new: None },
            (new, old_path) => Delta { path: in_index.path, old_path, copy: in_index.copy, old: in_index.old, new },
        };
        if !unchanged(sides, &combined, disk_id)? {
            deltas.push(combined);
        }
    }
    deltas.extend(staged.into_values());
    Ok(deltas)
}

/// Whether `delta` turned out to change nothing: the worktree undid what the index changed.
/// `disk_id` is the hash of the new version's file on disk, if already known.
fn unchanged(sides: &mut Sides<'_>, delta: &Delta, disk_id: Option<gix::ObjectId>) -> Result<bool, RepoError> {
    if delta.old_path.is_some() {
        return Ok(false);
    }
    Ok(match (delta.old, delta.new) {
        (None, None) => true,
        (Some(Version::Object { mode: a, id: old }), Some(Version::Object { mode: b, id: new })) => a == b && old == new,
        (Some(Version::Object { mode: a, id: old }), Some(Version::Worktree { mode: b })) if a == b => {
            let disk_id = match disk_id {
                Some(id) => id,
                None => sides.worktree()?.blob_id(delta.path.as_bstr(), b)?,
            };
            disk_id == old
        }
        _ => false,
    })
}

/// Changes from the index to the worktree, each with the hash its new file would have in git when
/// status computed it.
///
/// Stat-based: only files whose stat data doesn't match the index are read and hashed. Uses
/// `HashEq` rather than the `FastEq` of `Repository::status`, which reports any size change as a
/// modification; a file that differs only in line endings `core.autocrlf` would normalize must hash
/// equal and not show up.
fn index_worktree(sides: &mut Sides<'_>) -> Result<Vec<(Delta, Option<gix::ObjectId>)>, RepoError> {
    use gix::status::plumbing::index_as_worktree::traits::HashEq;

    let repo = sides.repo;
    if repo.workdir().is_none() {
        return Err(bare_error(repo));
    }
    let context = "Can't compare the worktree with the index";
    let dirwalk = if shows_untracked_files(repo) {
        let options = repo.dirwalk_options().map_err(|err| git_error(context, err))?;
        Some(options.emit_untracked(gix::dir::walk::EmissionMode::Matching))
    } else {
        None
    };
    // A submodule counts as changed only if its checked-out commit differs from the recorded one;
    // looking inside it would run a status in the submodule with its configuration from disk.
    let submodules = gix::status::index_worktree::BuiltinSubmoduleStatus::new(
        repo.clone().into_sync(),
        gix::status::Submodule::Given { ignore: gix::submodule::config::Ignore::Dirty, check_dirty: true },
    )
    .map_err(|err| git_error(context, err))?;

    let index = sides.index()?;
    let mut collect = Collect { deltas: Vec::new(), untracked: Vec::new() };
    repo.index_worktree_status(
        index,
        Vec::<BString>::new(),
        &mut collect,
        HashEq,
        submodules,
        &mut gix::features::progress::Discard,
        &AtomicBool::new(false),
        gix::status::index_worktree::Options { sorting: None, dirwalk_options: dirwalk, rewrites: None, thread_limit: None },
    )
    .map_err(|err| git_error(context, err))?;

    let mut deltas = collect.deltas;
    for (path, kind) in collect.untracked {
        let new = match kind {
            gix::dir::entry::Kind::File => Version::Worktree { mode: EntryKind::Blob.into() },
            gix::dir::entry::Kind::Symlink => Version::Worktree { mode: EntryKind::Link.into() },
            // A repository nested in the worktree but unknown to the index reads like a submodule.
            gix::dir::entry::Kind::Repository => match sides.worktree()?.version_at(path.as_bstr(), None)? {
                Some(version) => version,
                None => continue,
            },
            gix::dir::entry::Kind::Directory | gix::dir::entry::Kind::Untrackable => continue,
        };
        deltas.push((Delta { path, old_path: None, copy: false, old: None, new: Some(new) }, None));
    }
    Ok(deltas)
}

/// `status.showUntrackedFiles`, read the way `Repository::status` reads it.
fn shows_untracked_files(repo: &gix::Repository) -> bool {
    use gix::config::tree::Status;
    let config = repo.config_snapshot();
    let value = config.string(Status::SHOW_UNTRACKED_FILES);
    !value.is_some_and(|value| {
        matches!(Status::SHOW_UNTRACKED_FILES.try_into_show_untracked_files(value), Ok(gix::status::UntrackedFiles::None))
    })
}

struct Collect {
    deltas: Vec<(Delta, Option<gix::ObjectId>)>,
    untracked: Vec<(BString, gix::dir::entry::Kind)>,
}

impl<'index> VisitEntry<'index> for Collect {
    type ContentChange = gix::ObjectId;
    type SubmoduleStatus = gix::submodule::Status;

    fn visit_entry(&mut self, entry: Entry<'index, gix::ObjectId, gix::submodule::Status>) {
        match entry {
            Entry::Modification { entry, rela_path, status, .. } => {
                let mode = index_mode(entry.mode);
                let old = Some(Version::Object { mode, id: entry.id });
                let (new, disk_id) = match status {
                    EntryStatus::Change(Change::Removed) => (None, None),
                    EntryStatus::Change(Change::Type { worktree_mode }) => {
                        (Some(Version::Worktree { mode: index_mode(worktree_mode) }), None)
                    }
                    EntryStatus::Change(Change::Modification { executable_bit_changed, content_change, .. }) => {
                        let mode = match (executable_bit_changed, mode.kind()) {
                            (true, EntryKind::Blob) => EntryKind::BlobExecutable.into(),
                            (true, EntryKind::BlobExecutable) => EntryKind::Blob.into(),
                            _ => mode,
                        };
                        // No content change means only the executable bit changed.
                        (Some(Version::Worktree { mode }), Some(content_change.unwrap_or(entry.id)))
                    }
                    EntryStatus::Change(Change::SubmoduleModification(submodule)) => {
                        let Some(head) = submodule.checked_out_head_id else { return };
                        (Some(Version::Object { mode, id: head }), None)
                    }
                    // `git add --intent-to-add`: the file is new, whatever the placeholder entry says.
                    EntryStatus::IntentToAdd => {
                        let delta = Delta { path: rela_path.to_owned(), old_path: None, copy: false, old: None, new: Some(Version::Worktree { mode }) };
                        self.deltas.push((delta, None));
                        return;
                    }
                    // Unchanged (stat data merely stale), or unmerged, which `index_view` rules out.
                    EntryStatus::NeedsUpdate(_) | EntryStatus::Conflict { .. } => return,
                };
                let delta = Delta { path: rela_path.to_owned(), old_path: None, copy: false, old, new };
                self.deltas.push((delta, disk_id));
            }
            Entry::DirectoryContents { entry, .. } => {
                if entry.status == gix::dir::entry::Status::Untracked {
                    if let Some(kind) = entry.disk_kind {
                        self.untracked.push((entry.rela_path, kind));
                    }
                }
            }
            // Rename tracking between the index and the worktree is off.
            Entry::Rewrite { .. } => {}
        }
    }
}

fn bare_error(repo: &gix::Repository) -> RepoError {
    RepoError::Git(format!("The repository at {} is bare, so it has no worktree", repo.git_dir().display()))
}

/// Reads files from the worktree.
pub(super) struct Worktree<'repo> {
    root: PathBuf,
    capabilities: gix::fs::Capabilities,
    filter: gix::filter::Pipeline<'repo>,
    /// The index the filter consults for `core.autocrlf=true`'s "was it committed with CRLF" check.
    filter_index: gix::worktree::IndexPersistedOrInMemory,
}

impl<'repo> Worktree<'repo> {
    pub(super) fn new(repo: &'repo gix::Repository) -> Result<Worktree<'repo>, RepoError> {
        let root = repo.workdir().ok_or_else(|| bare_error(repo))?.to_owned();
        let context = "Can't prepare to read the worktree";
        let capabilities = repo.filesystem_options().map_err(|err| git_error(context, err))?;
        let (filter, filter_index) = repo.filter_pipeline(None).map_err(|err| git_error(context, err))?;
        Ok(Worktree { root, capabilities, filter, filter_index })
    }

    /// What is on disk at `path`: a file, a symlink, a submodule at its checked-out commit, or
    /// `None` for nothing or a plain directory. `recorded` is the index's version of the path, which
    /// supplies what the file system can't: the executable bit where it isn't tracked, whether a
    /// plain file stands for a symlink when `core.symlinks` is off, and the commit of a submodule
    /// that isn't checked out.
    pub(super) fn version_at(&mut self, path: &BStr, recorded: Option<Version>) -> Result<Option<Version>, RepoError> {
        let full_path = self.root.join(gix::path::from_bstr(path));
        let metadata = match std::fs::symlink_metadata(&full_path) {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(git_error(format!("Can't read {}", full_path.display()), err)),
        };
        let recorded_kind = recorded.map(|version| version.mode().kind());
        if metadata.is_dir() {
            // A submodule shows the commit checked out in it. If it isn't checked out (an empty
            // directory), it shows the recorded commit, so it doesn't count as changed, as in git.
            let head = submodule_head(&full_path);
            return Ok(match (head, recorded) {
                (Some(head), _) => Some(Version::Object { mode: EntryKind::Commit.into(), id: head }),
                (None, Some(recorded)) if recorded.mode().is_commit() => Some(recorded),
                (None, _) => None,
            });
        }
        let kind = if metadata.is_symlink() || (recorded_kind == Some(EntryKind::Link) && !self.capabilities.symlink) {
            EntryKind::Link
        } else if self.capabilities.executable_bit {
            if gix::fs::is_executable(&metadata) { EntryKind::BlobExecutable } else { EntryKind::Blob }
        } else if recorded_kind == Some(EntryKind::BlobExecutable) {
            EntryKind::BlobExecutable
        } else {
            EntryKind::Blob
        };
        Ok(Some(Version::Worktree { mode: kind.into() }))
    }

    /// The content git would store for the file at `path`.
    pub(super) fn content(&mut self, path: &BStr, mode: EntryMode) -> Result<Content, RepoError> {
        let full_path = self.root.join(gix::path::from_bstr(path));
        match std::fs::symlink_metadata(&full_path) {
            // Gone since it was listed: nothing to show.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Content::Text(Vec::new())),
            Err(err) => return Err(git_error(format!("Can't read {}", full_path.display()), err)),
            Ok(metadata) if !metadata.is_symlink() && metadata.len() > MAX_DIFF_BYTES => return Ok(Content::TooLarge),
            Ok(_) => {}
        }
        Ok(content::classify(self.read(path, mode)?))
    }

    /// The blob id the file at `path` would get if it was added.
    pub(super) fn blob_id(&mut self, path: &BStr, mode: EntryMode) -> Result<gix::ObjectId, RepoError> {
        let data = self.read(path, mode)?;
        gix::objs::compute_hash(gix::hash::Kind::Sha1, gix::objs::Kind::Blob, &data)
            .map_err(|err| git_error(format!("Can't hash {path}"), err))
    }

    /// The bytes git would store for `path`: a symlink's target, or the file's content after the
    /// filter pipeline's built-in conversions.
    fn read(&mut self, path: &BStr, mode: EntryMode) -> Result<Vec<u8>, RepoError> {
        let full_path = self.root.join(gix::path::from_bstr(path));
        let read_error = |err| git_error(format!("Can't read {}", full_path.display()), err);
        let metadata = std::fs::symlink_metadata(&full_path).map_err(read_error)?;
        if metadata.is_symlink() {
            let target = std::fs::read_link(&full_path).map_err(read_error)?;
            return Ok(gix::path::to_unix_separators_on_windows(gix::path::into_bstr(target)).into_owned().into());
        }
        let file = std::fs::File::open(&full_path).map_err(read_error)?;
        if mode.kind() == EntryKind::Link {
            // A symlink checked out as a plain file (`core.symlinks=false`) holds the target as text.
            let mut data = Vec::new();
            std::io::BufReader::new(file).read_to_end(&mut data).map_err(read_error)?;
            return Ok(data);
        }
        let mut converted = self
            .filter
            .convert_to_git(file, gix::path::from_bstr(path).as_ref(), &self.filter_index)
            .map_err(|err| git_error(format!("Can't convert {} to git's form", full_path.display()), err))?;
        let mut data = Vec::new();
        converted.read_to_end(&mut data).map_err(read_error)?;
        Ok(data)
    }
}

/// The commit checked out in the repository at `dir`, if `dir` holds one. Opening a repository and
/// reading its `HEAD` runs nothing.
fn submodule_head(dir: &std::path::Path) -> Option<gix::ObjectId> {
    let repo = gix::open_opts(dir, gix::open::Options::isolated()).ok()?;
    repo.head_id().ok().map(gix::Id::detach)
}
