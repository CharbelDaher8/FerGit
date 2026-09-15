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
mod diff;
mod history;
mod merges;
mod status;
mod table;
mod upstream;
mod walk;
mod watch;

use std::fmt;
use std::path::{Path, PathBuf};

pub use merges::MergeNames;
pub use table::CommitTable;
pub use watch::RepoWatcher;

use crate::types::{CommitDetails, DiffSide, FileChange, FileDiff, Oid, RefLabel};

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

    /// Reads the [`Tips`] alone: everything [`Repo::read_history`] reads except the commit walk, so
    /// it stays cheap however long the history is.
    pub fn read_tips(&self) -> Result<Tips, RepoError> {
        history::read_tips(&self.repo.to_thread_local())
    }

    /// Calls `on_change`, on a background thread, whenever the repository may have changed: HEAD,
    /// a ref, the index, the stash or a worktree file, reported once per burst of activity. A report
    /// can turn out not to matter (a file in an ignored directory changed); a change that matters
    /// is always reported. Watching stops when the returned watcher is dropped.
    pub fn watch(&self, on_change: impl FnMut() + Send + 'static) -> Result<RepoWatcher, RepoError> {
        let repo = self.repo.to_thread_local();
        watch::watch(repo.git_dir(), repo.workdir(), on_change)
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

    /// The branches the summary line of each commit in `ids` names as merged (see [`MergeNames`]),
    /// in the same order: `None` for ids that don't name a commit and for messages that name no
    /// branch. Commit messages never change, so results may be cached by id.
    pub fn merge_names(&self, ids: &[Oid]) -> Result<Vec<Option<MergeNames>>, RepoError> {
        merges::read(&self.repo.to_thread_local(), ids)
    }

    /// Files that differ between `from` and `to`, sorted by path, with renames detected as in
    /// [`Repo::commit_details`]. A `from` of `None` compares against nothing, so every file of `to`
    /// counts as added. Fails if a commit side doesn't name a commit.
    pub fn changes(&self, from: Option<DiffSide>, to: DiffSide) -> Result<Vec<FileChange>, RepoError> {
        diff::changes(&self.repo.to_thread_local(), from, to)
    }

    /// How one file differs between `from` and `to`. `path` names the file on the `to` side, and on
    /// the `from` side too unless `old_path` is given (for renames and copies). A side on which the
    /// file doesn't exist counts as empty. Fails if a commit side doesn't name a commit.
    pub fn file_diff(
        &self,
        from: Option<DiffSide>,
        to: DiffSide,
        path: &str,
        old_path: Option<&str>,
    ) -> Result<FileDiff, RepoError> {
        diff::file_diff(&self.repo.to_thread_local(), from, to, path, old_path)
    }
}

/// The state of a repository relevant to drawing its graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct History {
    pub tips: Tips,
    /// Commits reachable from HEAD, every ref and every stash base in `tips`, in display order.
    pub commits: CommitTable,
}

/// Where a repository's history starts, plus whether its worktree is dirty and which upstream each
/// branch follows.
///
/// Commits are immutable and reachable only from these starting points, so two equal `Tips` read at
/// different times describe the same history, drawn and labeled the same way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tips {
    pub head: Head,
    /// Local branches, remote-tracking branches (without symbolic ones like `origin/HEAD`) and
    /// tags, one entry per ref and each peeled, plus a [`crate::RefKind::Head`] label when HEAD is
    /// detached; sorted. The local branch HEAD points to has `is_head` set. A ref can peel to
    /// something other than a commit (`git tag t HEAD^{tree}`); its id then matches no commit.
    /// `upstream` is always `None` here: how far a branch diverged is counted from the history, so
    /// the session fills it in from [`Tips::upstreams`].
    pub refs: Vec<(Oid, RefLabel)>,
    /// Stash entries, newest (`stash@{0}`) first.
    pub stashes: Vec<StashEntry>,
    /// Whether the worktree or index differs from HEAD, untracked files included. Always false for
    /// bare repositories.
    pub worktree_dirty: bool,
    /// The upstream of each local branch that has one configured, sorted by branch. Read from the
    /// configuration as it is on disk now, not as it was when the repository was opened.
    pub upstreams: Vec<BranchUpstream>,
}

/// The ref a local branch follows: `branch.<name>.remote` and `branch.<name>.merge`, mapped to a
/// local ref through the remote's fetch refspecs, or the local branch `merge` names when the remote
/// is `.`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchUpstream {
    /// Full name of the local branch, e.g. `refs/heads/main`.
    pub branch: String,
    /// Short name of the upstream, e.g. `origin/main`.
    pub name: String,
    /// Full name of the upstream, e.g. `refs/remotes/origin/main`.
    pub full_name: String,
    /// What the upstream points to, peeled; `None` if it doesn't exist (the branch is gone upstream,
    /// or was never fetched). Its commits are part of the history even if no listed ref names it.
    pub id: Option<Oid>,
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
