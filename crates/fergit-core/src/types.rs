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
