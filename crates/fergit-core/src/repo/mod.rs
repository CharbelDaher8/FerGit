//! Reading git repositories into plain data.
//!
//! This is the only module that knows how git is accessed: gix for reads (and, later, the git CLI
//! for mutations). Nothing outside it sees gix types, command lines, or git output formats.
//!
//! # Untrusted repositories
//!
//! Anyone who hands the user a repository (an archive, a shared drive) controls its `.git/config`,
//! and `.gitattributes` arrives with every checkout, so a read must never run a program either of
//! them names. gix's read path implements neither hooks nor `core.fsmonitor`, and only network
//! operations reach `core.sshCommand` or credential helpers. The configured programs a read *can*
//! reach are removed when the repository is opened; see [`disable_configured_programs`].

mod details;
mod history;
mod status;
mod walk;

use std::fmt;
use std::path::{Path, PathBuf};

use crate::types::{CommitDetails, Oid, RefLabel};

#[derive(Debug, thiserror::Error)]
pub enum RepoError {
    #[error("{} is not inside a git repository", path.display())]
    NotARepository { path: PathBuf },
    /// Any other failure reading the repository; the message is written for the user.
    #[error("{0}")]
    Git(String),
}

/// An open repository. Cheap to share across threads (`Send + Sync`).
pub struct Repo {
    /// Every call works on a thread-local view of this handle, which shares its object database,
    /// ref store and (already sanitized) configuration.
    repo: gix::ThreadSafeRepository,
    root: PathBuf,
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Repo>();
};

impl Repo {
    /// Opens the repository containing `path`, searching parent directories like git does.
    pub fn open(path: &Path) -> Result<Repo, RepoError> {
        let start = std::path::absolute(path)
            .map_err(|err| RepoError::Git(format!("Can't resolve the path {}: {err}", path.display())))?;
        // Like git, refuse a repository owned by another user unless `safe.directory` lists it.
        // gix would otherwise open it with reduced trust, but the git CLI FerGit uses for
        // mutations refuses it anyway, and the plan is to respect `safe.directory`.
        let mut options = gix::sec::trust::Mapping::<gix::open::Options>::default();
        options.full = options.full.bail_if_untrusted(true);
        options.reduced = options.reduced.bail_if_untrusted(true);
        let repo = gix::ThreadSafeRepository::discover_opts(&start, Default::default(), options)
            .map_err(|err| open_error(path, err))?;

        let mut repo = repo.to_thread_local();
        if repo.object_hash() != gix::hash::Kind::Sha1 {
            return Err(RepoError::Git(format!(
                "{} uses SHA-256 object ids, which FerGit doesn't support yet",
                path.display()
            )));
        }
        disable_configured_programs(&mut repo)?;
        let root = repo.workdir().unwrap_or_else(|| repo.git_dir()).to_owned();
        Ok(Repo { repo: repo.into_sync(), root })
    }

    /// The directory the user thinks of as the repository: the worktree root, or the git
    /// directory for a bare repository.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Reads everything needed to draw the graph, as of one moment.
    pub fn read_history(&self) -> Result<History, RepoError> {
        history::read(&self.repo.to_thread_local())
    }

    /// Summary line and author of each commit in `ids`, in the same order. Ids that don't name a
    /// commit yield `None`.
    pub fn commit_summaries(&self, ids: &[Oid]) -> Result<Vec<Option<CommitSummary>>, RepoError> {
        details::summaries(&self.repo.to_thread_local(), ids)
    }

    /// Full details of one commit, including the files it changed relative to its first parent (or
    /// the empty tree for a root commit). `None` if `id` doesn't name a commit.
    pub fn commit_details(&self, id: Oid) -> Result<Option<CommitDetails>, RepoError> {
        details::details(&self.repo.to_thread_local(), id)
    }
}

/// The state of a repository relevant to drawing its graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct History {
    pub head: Head,
    /// Refs whose target peels to a commit, one entry per ref; several refs may share a commit.
    /// Includes local branches, remote-tracking branches (without symbolic ones like `origin/HEAD`),
    /// tags, and a [`crate::RefKind::Head`] label when HEAD is detached. The local branch HEAD
    /// points to has `is_head` set.
    pub refs: Vec<(Oid, RefLabel)>,
    /// Stash entries, newest (`stash@{0}`) first.
    pub stashes: Vec<StashEntry>,
    /// Commits reachable from HEAD, every ref in `refs`, and every stash base, in display order.
    pub commits: CommitTable,
    /// Whether the worktree or index differs from HEAD, untracked files included. Always false for
    /// bare repositories.
    pub worktree_dirty: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    /// HEAD names a branch with no commits yet (a fresh repository).
    Unborn { branch: String },
    Branch { name: String, id: Oid },
    Detached { id: Oid },
}

impl Head {
    /// The commit HEAD points to, if there is one.
    pub fn id(&self) -> Option<Oid> {
        match self {
            Head::Unborn { .. } => None,
            Head::Branch { id, .. } | Head::Detached { id } => Some(*id),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StashEntry {
    /// The stash commit itself.
    pub id: Oid,
    /// The commit the stash was made on top of (the stash commit's first parent).
    pub base: Oid,
    /// Position in the stash list: `n` in `stash@{n}`.
    pub index: u32,
    /// The stash's message, e.g. `WIP on main: 1a2b3c4 Fix parser`.
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommitSummary {
    /// First line of the message, without the line terminator.
    pub summary: String,
    pub author_name: String,
    pub author_email: String,
    /// Author time, seconds since the Unix epoch.
    pub author_time: i64,
}

/// Commits in display order, stored as flat arrays.
///
/// Display order: every commit appears before all of its parents; where that leaves the order
/// free, more recent committer dates come first (like `git log --date-order`). `parents(i)` lists,
/// in git order, only those parents that are themselves in the table — parents cut off by a
/// shallow clone are omitted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommitTable {
    ids: Vec<Oid>,
    /// `parent_ends[i]` is one past the last index into `parents` belonging to commit `i`.
    parent_ends: Vec<u32>,
    parents: Vec<Oid>,
}

impl CommitTable {
    pub fn with_capacity(commits: usize) -> CommitTable {
        CommitTable {
            ids: Vec::with_capacity(commits),
            parent_ends: Vec::with_capacity(commits),
            parents: Vec::with_capacity(commits + commits / 8),
        }
    }

    /// Appends a commit. The caller is responsible for display order and for filtering parents.
    pub fn push(&mut self, id: Oid, parents: impl IntoIterator<Item = Oid>) {
        self.ids.push(id);
        self.parents.extend(parents);
        let end = u32::try_from(self.parents.len()).expect("fewer than 4 billion parent links");
        self.parent_ends.push(end);
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn id(&self, index: usize) -> Oid {
        self.ids[index]
    }

    pub fn parents(&self, index: usize) -> &[Oid] {
        let start = if index == 0 { 0 } else { self.parent_ends[index - 1] as usize };
        &self.parents[start..self.parent_ends[index] as usize]
    }
}

/// Removes from `repo`'s in-memory configuration every section that names a program a read could
/// run. Thread-local handles made from `repo` afterwards inherit the edited configuration; the
/// files on disk are untouched, so git commands run for mutations still see them.
///
/// Two kinds of configured program are reachable from gix's read path:
/// - Filter drivers (`[filter "lfs"] clean = …`, `process = …`). Status pipes a tracked file
///   through its clean filter whenever the file's stat data doesn't match the index.
/// - Diff drivers (`[diff "x"] textconv = …`, `command = …`). The tree diffs FerGit performs don't
///   run them, but gix runs `textconv` in its worktree-oriented diff modes; removing them means a
///   later change of mode can't start doing so silently.
///
/// Trade-off: git compares an LFS-tracked file through the LFS clean filter. Without the filter, a
/// tracked file whose stat data is stale (touched, or written in the same second as the index) is
/// compared byte for byte against the stored pointer and counts as modified, so the worktree can
/// look dirty when git says it's clean. A spurious "uncommitted changes" row is harmless; running a
/// command from a stranger's repository is not.
fn disable_configured_programs(repo: &mut gix::Repository) -> Result<(), RepoError> {
    let mut config = repo.config_snapshot_mut();
    let driver_sections: Vec<_> = config
        .sections_and_ids()
        .filter(|(section, _)| {
            let header = section.header();
            let name: &[u8] = header.name().as_ref();
            // A plain `[diff]` section holds settings like `diff.renames`, not a driver.
            name.eq_ignore_ascii_case(b"filter")
                || (name.eq_ignore_ascii_case(b"diff") && header.subsection_name().is_some())
        })
        .map(|(_, id)| id)
        .collect();
    for id in driver_sections {
        config.remove_section_by_id(id);
    }
    config
        .commit()
        .map_err(|err| git_error("Can't apply the repository's configuration", err))?;
    Ok(())
}

fn open_error(path: &Path, err: gix::discover::Error) -> RepoError {
    use gix::discover::upwards::Error as Discover;
    match err {
        gix::discover::Error::Discover(
            Discover::NoGitRepository { .. }
            | Discover::NoGitRepositoryWithinCeiling { .. }
            | Discover::NoGitRepositoryWithinFs { .. },
        )
        | gix::discover::Error::Open(gix::open::Error::NotARepository { .. }) => {
            RepoError::NotARepository { path: path.to_owned() }
        }
        gix::discover::Error::Open(gix::open::Error::UnsafeGitDir { path: dir }) => RepoError::Git(format!(
            "{dir} belongs to another user, so FerGit won't read it (git refuses it too). If you trust it, run: \
             git config --global --add safe.directory \"{dir}\"",
            dir = dir.display()
        )),
        other => git_error(format!("Can't open the repository at {}", path.display()), other),
    }
}

/// A [`RepoError::Git`] whose message is `context` followed by each distinct cause in `err`'s
/// source chain, e.g. `Can't read commit 1a2b…: object not found`.
fn git_error(context: impl fmt::Display, err: impl std::error::Error + 'static) -> RepoError {
    let mut message = context.to_string();
    let mut previous = String::new();
    let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(&err);
    while let Some(err) = cause {
        let text = err.to_string();
        // Wrapper errors often repeat their cause's message; show it once.
        if !text.is_empty() && !previous.contains(&text) {
            message.push_str(": ");
            message.push_str(&text);
        }
        previous = text;
        cause = err.source();
    }
    RepoError::Git(message)
}

fn to_oid(id: &gix::oid) -> Oid {
    Oid::from_bytes(id.as_bytes()).expect("repositories are checked to use SHA-1 ids when opened")
}

fn to_object_id(id: Oid) -> gix::ObjectId {
    gix::ObjectId::from_bytes_or_panic(id.as_bytes())
}

/// Repository text (names, messages, paths) is arbitrary bytes; show it as UTF-8, replacing
/// invalid sequences.
fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}
