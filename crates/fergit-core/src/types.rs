//! Plain value types shared between modules and sent to the UI.
//!
//! With the `specta` feature these also describe the IPC schema; TypeScript bindings are generated
//! from them. JavaScript numbers lose precision above 2^53, so object ids cross as hex strings and
//! 64-bit timestamps are declared as `number` (seconds since the epoch fit comfortably).

use std::fmt;
use std::str::FromStr;

use fergit_graph::GraphRow;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A git object id (SHA-1). Crosses IPC as a 40-character lowercase hex string.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "specta", derive(specta::Type), specta(type = String))]
pub struct Oid([u8; 20]);

impl Oid {
    /// The all-zero id. Git never assigns it to an object; FerGit uses it for the uncommitted-changes row.
    pub const ZERO: Oid = Oid([0; 20]);

    pub fn from_bytes(bytes: &[u8]) -> Option<Oid> {
        bytes.try_into().ok().map(Oid)
    }

    pub fn as_bytes(&self) -> &[u8; 20] {
        &self.0
    }

    fn write_hex<'a>(&self, buf: &'a mut [u8; 40]) -> &'a str {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        for (i, byte) in self.0.iter().enumerate() {
            buf[2 * i] = DIGITS[usize::from(byte >> 4)];
            buf[2 * i + 1] = DIGITS[usize::from(byte & 0x0f)];
        }
        std::str::from_utf8(buf).expect("hex digits are ASCII")
    }
}

impl fmt::Display for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.write_hex(&mut [0; 40]))
    }
}

impl fmt::Debug for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Oid({self})")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a 40-character hex object id: {0:?}")]
pub struct ParseOidError(String);

impl FromStr for Oid {
    type Err = ParseOidError;

    fn from_str(s: &str) -> Result<Oid, ParseOidError> {
        let invalid = || ParseOidError(s.to_owned());
        let hex = s.as_bytes();
        if hex.len() != 40 {
            return Err(invalid());
        }
        let nibble = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
        let mut bytes = [0u8; 20];
        for (i, pair) in hex.chunks_exact(2).enumerate() {
            let (hi, lo) = (nibble(pair[0]).ok_or_else(invalid)?, nibble(pair[1]).ok_or_else(invalid)?);
            bytes[i] = hi << 4 | lo;
        }
        Ok(Oid(bytes))
    }
}

impl Serialize for Oid {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.write_hex(&mut [0; 40]))
    }
}

impl<'de> Deserialize<'de> for Oid {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Oid, D::Error> {
        let s = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Identifies one snapshot of a repository. Increases every time the visible state changes, and
/// keeps increasing across repositories opened in the same process, so the UI can discard any
/// response or event from an older snapshot, including one of a previously open repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(transparent)]
pub struct Generation(pub u32);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct RepoInfo {
    /// Worktree root, or the git directory for a bare repository.
    pub root: String,
    /// Display name: the last component of `root`.
    pub name: String,
    pub generation: Generation,
    pub row_count: u32,
    /// The commit HEAD points to; `None` in a repository without commits.
    pub head: Option<Oid>,
    /// Short name of the branch HEAD names (even one without commits yet); `None` when detached.
    pub branch: Option<String>,
    /// Whether a merge, rebase, cherry-pick or revert is in progress, and which files conflict.
    pub state: RepoState,
}

/// What git is in the middle of. Stopping with conflicts is a normal state, not an error: the user
/// resolves the files in their editor, then continues or aborts (see [`Operation::Continue`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum RepoState {
    /// Nothing in progress, and no file has conflicts.
    Clean,
    /// A merge stopped before committing. `heads` are the commits being merged in (none for a
    /// squash merge, which records no merge); `message` is the first line of the prepared message.
    Merging { heads: Vec<Oid>, squash: bool, message: String, conflicts: Vec<ConflictFile> },
    /// A rebase of `branch` (`None`: of a detached HEAD) onto `onto` stopped at commit `at`, which
    /// is step `step` of `total`.
    Rebasing {
        branch: Option<String>,
        onto: Option<Oid>,
        at: Option<Oid>,
        step: u32,
        total: u32,
        conflicts: Vec<ConflictFile>,
    },
    /// A cherry-pick stopped at `commit` (`None` if git no longer records which).
    CherryPicking { commit: Option<Oid>, conflicts: Vec<ConflictFile> },
    /// A revert stopped at `commit` (`None` if git no longer records which).
    Reverting { commit: Option<Oid>, conflicts: Vec<ConflictFile> },
    /// Files have conflicts but nothing is in progress: a stash that didn't apply cleanly. The
    /// stash is kept.
    Unmerged { conflicts: Vec<ConflictFile> },
}

impl RepoState {
    /// The files with conflicts, sorted by path.
    pub fn conflicts(&self) -> &[ConflictFile] {
        match self {
            RepoState::Clean => &[],
            RepoState::Merging { conflicts, .. }
            | RepoState::Rebasing { conflicts, .. }
            | RepoState::CherryPicking { conflicts, .. }
            | RepoState::Reverting { conflicts, .. }
            | RepoState::Unmerged { conflicts } => conflicts,
        }
    }
}

/// A file git couldn't merge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct ConflictFile {
    /// `/`-separated, relative to the worktree root.
    pub path: String,
    /// The file on disk no longer has conflict markers (or was deleted): continuing will take it
    /// as it is.
    pub resolved: bool,
}

/// A contiguous run of graph rows from one snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct RowsPage {
    pub generation: Generation,
    /// Index of `rows[0]`. Requests past the end are clamped, so this may be less than requested.
    pub start: u32,
    /// Total rows in this snapshot.
    pub total: u32,
    pub rows: Vec<Row>,
}

/// Where a row is in one snapshot. A row index means nothing without its generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct RowLocation {
    pub generation: Generation,
    /// Index of the row; `None` if no row of this snapshot shows the requested id.
    pub row: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub kind: RowKind,
    /// Commit id; [`Oid::ZERO`] for the uncommitted-changes row.
    pub id: Oid,
    pub graph: GraphRow,
    /// First line of the commit message.
    pub summary: String,
    pub author_name: String,
    pub author_email: String,
    /// Author time, seconds since the Unix epoch. 0 for the uncommitted-changes row.
    #[cfg_attr(feature = "specta", specta(type = specta_typescript::Number))]
    pub time: i64,
    /// Labels to show on this row, in display order (HEAD's branch first).
    pub refs: Vec<RefLabel>,
    /// Branches splitting off or joining at this row, in lane order. Empty for most rows.
    pub relations: Vec<Relation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum RowKind {
    Commit,
    /// Uncommitted changes in the worktree or index, drawn as a child of HEAD.
    WorkingTree,
    Stash,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct RefLabel {
    pub kind: RefKind,
    /// Short name as the user reads it: `main`, `origin/main`, `v1.0`, `HEAD`, `stash@{0}`.
    pub name: String,
    /// Full ref name: `refs/heads/main`, `refs/remotes/origin/main`, `refs/tags/v1.0`, `HEAD`, `refs/stash`.
    pub full_name: String,
    /// True for the branch HEAD points to, and for the `HEAD` label of a detached HEAD.
    pub is_head: bool,
    /// For a local branch with an upstream configured (`branch.<name>.merge`), that upstream and
    /// how far the branch has diverged from it. `None` for every other ref.
    pub upstream: Option<Upstream>,
}

/// A branch splitting off or joining another, drawn where its line meets this row's commit.
///
/// Git doesn't record which branch a commit was made on, so branch names are inferred: from the
/// refs at a branch's tip and from merge commit messages (`Merge branch 'feature/x'`), carried down
/// the branch's line. A name is `None` when nothing names the branch.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Relation {
    /// The branch whose line comes down lane `lane` starts at this row's commit: this row's upper
    /// segment from lane `lane` into the node. `from` names the branch the commit is on.
    BranchedFrom { lane: u16, branch: Option<String>, from: Option<String> },
    /// This merge commit brings in the branch continuing in lane `lane`: this row's lower segment
    /// from the node to lane `lane`, one per non-first parent. `into` names the branch receiving
    /// the merge.
    Merges { lane: u16, branch: Option<String>, into: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct Upstream {
    /// Short name of the upstream remote-tracking branch, e.g. `origin/main`.
    pub name: String,
    pub state: UpstreamState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum UpstreamState {
    /// The upstream ref exists locally and points to `id`. `ahead` counts commits on the branch the
    /// upstream lacks; `behind` counts commits on the upstream the branch lacks.
    Tracking { ahead: u32, behind: u32, id: Oid },
    /// The upstream is configured but its remote-tracking ref doesn't exist (deleted on the remote
    /// and pruned, or never fetched).
    Gone,
}

/// Declaration order is display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum RefKind {
    /// Detached HEAD.
    Head,
    LocalBranch,
    RemoteBranch,
    Tag,
    Stash,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct CommitDetails {
    pub id: Oid,
    pub parents: Vec<Oid>,
    pub author: Signature,
    pub committer: Signature,
    /// Full commit message.
    pub message: String,
    /// Files changed relative to the first parent (or the empty tree for a root commit).
    pub files: Vec<FileChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct Signature {
    pub name: String,
    pub email: String,
    /// Seconds since the Unix epoch.
    #[cfg_attr(feature = "specta", specta(type = specta_typescript::Number))]
    pub time: i64,
    /// The author's UTC offset in minutes (e.g. +120 for UTC+2).
    pub offset_minutes: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    /// Path after the change, `/`-separated.
    pub path: String,
    /// Path before the change, for renames and copies.
    pub old_path: Option<String>,
    pub status: ChangeStatus,
    /// Added lines; `None` for binary files (a NUL byte in the first 8000 bytes), submodules, and
    /// files over 8 MiB on either side.
    pub additions: Option<u32>,
    /// Deleted lines; `None` in the same cases as `additions`.
    pub deletions: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum ChangeStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
}

/// One side of a comparison between two versions of the repository's files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum DiffSide {
    /// The files of a commit.
    Commit { id: Oid },
    /// The staged files: what the next commit would contain.
    Index,
    /// The files on disk as git would store them (line endings converted the way git would), plus
    /// untracked files that aren't ignored.
    Worktree,
}

/// How one file differs between two sides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum FileDiff {
    /// Changed lines in hunks with three lines of context, as `git diff` shows them. No hunks means
    /// the contents are identical (a mode-only change, for example).
    Text { hunks: Vec<Hunk> },
    /// A side is binary: a NUL byte within its first 8000 bytes.
    Binary,
    /// A side is larger than 8 MiB.
    TooLarge,
    /// The path is a submodule on at least one side; `old` and `new` are the commits it points to,
    /// `None` where that side has no submodule at the path.
    Submodule { old: Option<Oid>, new: Option<Oid> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct Hunk {
    /// First line of the hunk in the old version, 1-based; 0 when the hunk has no old lines (git's
    /// `@@ -0,0 +1,3 @@`).
    pub old_start: u32,
    pub old_lines: u32,
    /// First line of the hunk in the new version, 1-based; 0 when the hunk has no new lines.
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    pub kind: LineKind,
    /// The line without its terminator.
    pub text: String,
    /// True if this is the last line of its version and that version doesn't end with a newline
    /// (git's `\ No newline at end of file`).
    pub no_final_newline: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

/// Identifies one request to run an operation. The UI generates it; running the same id twice runs
/// the operation once, so a double click or a retried request can't push twice.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(transparent)]
pub struct OpId(pub String);

/// Something the user asked to do to the repository, described by intent rather than as a git
/// command line. Names are short ref names as shown on labels (`main`, `v1.0`); they are validated
/// before git sees them, and never parsed as options.
///
/// Serialized into the operation journal, which outlives the binary: variants and fields may be
/// added, but never renamed or removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Operation {
    /// Switch the worktree to a branch, or detach HEAD at a commit. Fails, changing nothing, if
    /// uncommitted changes would be overwritten.
    Checkout { target: CheckoutTarget },
    /// Create a local branch at `at`, optionally switching to it and making it follow `upstream`
    /// (a full remote-tracking ref name such as `refs/remotes/origin/main`).
    CreateBranch { name: String, at: Oid, checkout: bool, upstream: Option<String> },
    /// Ensure the local branch `name` doesn't exist: deleting one that is already gone succeeds.
    /// Without `force`, a branch with commits not merged into its upstream (or HEAD) is kept.
    DeleteBranch { name: String, force: bool },
    /// Create a tag at `at`: annotated with `message` if there is one, lightweight otherwise.
    CreateTag { name: String, at: Oid, message: Option<String> },
    /// Ensure the tag `name` doesn't exist: deleting one that is already gone succeeds.
    DeleteTag { name: String },
    /// Download from `remote`, or from every remote when `None`, removing remote-tracking branches
    /// whose remote branch is gone if `prune` is set.
    Fetch { remote: Option<String>, prune: bool },
    /// Fetch the current branch's upstream and fast-forward the branch to it. Fails, changing no
    /// branch, if the two have diverged; merging and rebasing are separate operations.
    Pull,
    /// Push the local branch `branch` to the branch of the same name on `remote` (the branch's push
    /// remote when `None`), making it the branch's upstream if `set_upstream`.
    Push {
        branch: String,
        remote: Option<String>,
        force: ForceMode,
        #[serde(rename = "setUpstream")]
        set_upstream: bool,
    },
    /// Merge `from` into the current branch. Stopping with conflicts succeeds, leaving
    /// [`RepoState::Merging`].
    Merge { from: Rev, mode: MergeMode },
    /// Replay the current branch's commits onto `onto`, without an editor. Stopping with conflicts
    /// succeeds, leaving [`RepoState::Rebasing`].
    Rebase { onto: Rev },
    /// Apply the changes of `commits`, in this order, as new commits on the current branch.
    /// Stopping with conflicts succeeds, leaving [`RepoState::CherryPicking`].
    CherryPick { commits: Vec<Oid> },
    /// Add a commit undoing the changes of `commit`. Stopping with conflicts succeeds, leaving
    /// [`RepoState::Reverting`].
    Revert { commit: Oid },
    /// Move the local branch `branch` to `to`, but only if it is still at `expected`, the tip the
    /// user was looking at; otherwise it fails as [`OpErrorKind::Moved`], changing nothing. For the
    /// current branch, `mode` says what happens to the index and worktree.
    Reset { branch: String, to: Oid, mode: ResetMode, expected: Oid },
    /// Finish what is in progress: stage the conflicted files (refused while any still has conflict
    /// markers), then commit and carry on with the rest of a rebase, cherry-pick or revert.
    Continue,
    /// Give up what is in progress, putting the branch and files back as they were before it.
    Abort,
    /// Leave out the commit a rebase, cherry-pick or revert stopped at, and carry on.
    Skip,
    /// Save uncommitted changes (and untracked files, if `untracked`) as a new stash, leaving the
    /// worktree clean.
    StashPush { message: Option<String>, untracked: bool },
    /// Apply the changes of stash `stash@{index}`, which must still be the stash `id`, keeping it.
    StashApply { index: u32, id: Oid },
    /// Apply stash `stash@{index}` (still `id`) and remove it; a stash that conflicts is kept.
    StashPop { index: u32, id: Oid },
    /// Remove stash `stash@{index}`, which must still be the stash `id`.
    StashDrop { index: u32, id: Oid },
    /// Put back what the journal entry `entry` recorded before it ran: the refs it moved (not
    /// remote-tracking ones), HEAD if it switched branches, a stash it dropped or pushed. Refused,
    /// changing nothing, if any of those has moved since.
    Undo { entry: OpId },
}

/// A commit given by a ref (`refs/heads/x`, `refs/remotes/origin/x`, `refs/tags/v1`) or by id.
/// Naming the ref lets git word the merge message after it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Rev {
    /// A ref by full name.
    Ref { name: String },
    Commit { id: Oid },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum MergeMode {
    /// Fast-forward when the branch has no commits of its own, else create a merge commit.
    Ff,
    /// Always create a merge commit.
    NoFf,
    /// Commit the combined changes as one ordinary commit, recording no merge.
    Squash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum ResetMode {
    /// Move the branch only; the undone commits' changes stay staged.
    Soft,
    /// Move the branch and reset the index; the changes stay in the files, unstaged.
    Mixed,
    /// Move the branch and make the index and files match it, discarding uncommitted changes to
    /// tracked files. Discarded changes are gone for good: git never stored them.
    Hard,
}

/// A ref that differed between before and after an operation. `None` means the ref didn't exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct RefChange {
    pub name: String,
    pub before: Option<Oid>,
    pub after: Option<Oid>,
}

/// What [`Operation::Undo`] would do now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Undoable {
    /// The operation `entry` can be undone: `changes` are the refs that go back to `before`, and
    /// `head` is the HEAD that is restored if HEAD moves too.
    Ready {
        entry: OpId,
        operation: Operation,
        /// When it started, milliseconds since the Unix epoch.
        #[serde(rename = "startedAtMs")]
        #[cfg_attr(feature = "specta", specta(type = specta_typescript::Number))]
        started_at_ms: u64,
        changes: Vec<RefChange>,
        head: Option<String>,
    },
    /// The most recent operation can't be undone, for `reason`.
    Blocked { operation: Operation, reason: String },
    /// No operation on this repository was recorded, or all have been undone.
    Nothing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CheckoutTarget {
    /// A local branch, by short name.
    Branch { name: String },
    /// A commit, detaching HEAD.
    Commit { id: Oid },
}

/// Whether a push may replace commits on the remote. There is deliberately no unconditional force.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ForceMode {
    /// Only fast-forward the remote branch.
    None,
    /// Replace the remote branch, but only if it is still at `expected` (`None`: only if it doesn't
    /// exist), the tip the user was looking at. A remote branch that moved since, because someone
    /// else pushed, fails the push as [`OpErrorKind::StaleLease`] instead of losing their work.
    WithLease { expected: Option<Oid> },
}

/// How an operation ended. The repository is re-read either way (a failed fetch may still have
/// updated some refs), and `info` describes the snapshot current afterwards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum OpOutcome {
    Done { info: RepoInfo },
    Failed { info: RepoInfo, error: OpError },
}

/// Why an operation failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct OpError {
    pub kind: OpErrorKind,
    /// What went wrong and what to do about it, written for the user.
    pub message: String,
    /// Git's output (errors first, then anything it printed to stdout), with credentials removed.
    /// Empty if git never ran.
    pub output: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum OpErrorKind {
    /// A name isn't a valid ref name, a remote doesn't exist, or the repository isn't in a state
    /// the operation applies to (no current branch to pull, say). Git didn't run.
    InvalidInput,
    /// The branch or tag to create already exists.
    AlreadyExists,
    /// The branch to delete has commits that would become unreachable; deleting it needs `force`.
    NotFullyMerged,
    /// Uncommitted changes would be overwritten.
    LocalChanges,
    /// The remote has commits the local branch lacks (push), or the two diverged (pull), or a hook
    /// on the remote declined the push.
    Rejected,
    /// A lease-protected push found the remote branch somewhere other than expected.
    StaleLease,
    /// A branch, HEAD or stash wasn't where the operation expected it (a reset's `expected` tip,
    /// what an undo would restore from), so nothing was changed.
    Moved,
    /// The remote wanted credentials that were missing, cancelled or wrong.
    AuthFailed,
    /// Git couldn't be started.
    GitNotFound,
    /// Any other failure; `message` has git's own words.
    Git,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oid_hex_round_trips() {
        let hex = "0123456789abcdef0123456789abcdef01234567";
        let oid: Oid = hex.parse().unwrap();
        assert_eq!(oid.to_string(), hex);
        assert_eq!(serde_json::to_string(&oid).unwrap(), format!("\"{hex}\""));
    }

    #[test]
    fn oid_rejects_bad_hex() {
        assert!("abc".parse::<Oid>().is_err());
        assert!("zz23456789abcdef0123456789abcdef01234567".parse::<Oid>().is_err());
    }
}
