//! The operation journal: an append-only record of every operation FerGit ran.
//!
//! One JSON object per line (JSONL), appended and never rewritten. Each entry records what was
//! asked, how every ref moved, and how it ended, so the journal serves as an audit trail and as the
//! basis for undo, which is a *new* operation restoring recorded values rather than an edit of the
//! log (see [`crate::Operation::Undo`]).
//!
//! Entries outlive the binary that wrote them, so the format only grows: fields are added with
//! `#[serde(default)]`, never renamed or removed, and `schemaVersion` changes only if an entry's
//! meaning does. Credentials never reach an entry: git's output is scrubbed before anything sees
//! it, and answers to credential prompts don't pass through operations at all.

use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};

use crate::repo::RefValues;
pub use crate::types::RefChange;
use crate::types::{OpErrorKind, OpId, Operation};

/// The `schemaVersion` of the entries this build writes.
pub const SCHEMA_VERSION: u32 = 1;

/// One operation, as recorded after it finished.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalEntry {
    pub schema_version: u32,
    pub id: OpId,
    /// The repository's worktree root (the git directory for a bare repository).
    pub repo: String,
    pub operation: Operation,
    /// Wall-clock start, milliseconds since the Unix epoch. For people reading the journal; never
    /// used to order entries, which the file's order already does.
    pub started_at_ms: u64,
    pub duration_ms: u64,
    /// HEAD before and after: `ref: refs/heads/<branch>`, or a commit id when detached.
    pub head_before: String,
    pub head_after: String,
    /// Every ref the operation created, moved or deleted, sorted by name.
    #[serde(default)]
    pub ref_changes: Vec<RefChange>,
    /// Git's exit code: 0 on success, `None` if the operation failed before git ran (or git was
    /// killed).
    #[serde(default)]
    pub exit_code: Option<i32>,
    /// Why it failed; `None` if it succeeded.
    #[serde(default)]
    pub error: Option<JournalError>,
    /// Git's output with credentials scrubbed; failures only, as successful output is progress
    /// noise.
    #[serde(default)]
    pub output: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalError {
    pub kind: OpErrorKind,
    pub message: String,
}

/// The refs that differ between `before` and `after`, sorted by name.
pub(super) fn ref_changes(before: &RefValues, after: &RefValues) -> Vec<RefChange> {
    let names = before.refs.keys().chain(after.refs.keys().filter(|name| !before.refs.contains_key(*name)));
    let mut changes: Vec<RefChange> = names
        .filter_map(|name| {
            let (was, is) = (before.refs.get(name).copied(), after.refs.get(name).copied());
            (was != is).then(|| RefChange { name: name.clone(), before: was, after: is })
        })
        .collect();
    changes.sort_by(|a, b| a.name.cmp(&b.name));
    changes
}

/// An open journal file. Appends from any thread are serialized, one whole line at a time.
pub struct Journal {
    path: PathBuf,
    file: Mutex<File>,
}

impl Journal {
    /// Opens the journal at `path` for appending, creating it (and its directory) if needed.
    pub fn open(path: impl Into<PathBuf>) -> io::Result<Journal> {
        let path = path.into();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Journal { path, file: Mutex::new(file) })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends `entry` as one line and flushes it to the operating system.
    pub fn append(&self, entry: &JournalEntry) -> io::Result<()> {
        let mut line = serde_json::to_string(entry).map_err(io::Error::other)?;
        line.push('\n');
        let mut file = self.file.lock().unwrap_or_else(PoisonError::into_inner);
        // One write per entry: with the file in append mode, a line is never interleaved with
        // another process's.
        file.write_all(line.as_bytes())?;
        file.flush()
    }

    /// Every entry in the journal at `path`, oldest first. A line that doesn't parse (cut short by
    /// a crash, or written by a newer FerGit in a format this one can't read) is skipped; a missing
    /// file has no entries.
    pub fn read(path: &Path) -> io::Result<Vec<JournalEntry>> {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(err),
        };
        let mut entries = Vec::new();
        for line in BufReader::new(file).lines() {
            if let Ok(entry) = serde_json::from_str(&line?) {
                entries.push(entry);
            }
        }
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Oid;

    fn oid(byte: u8) -> Oid {
        Oid::from_bytes(&[byte; 20]).unwrap()
    }

    fn values(refs: &[(&str, u8)]) -> RefValues {
        RefValues {
            head: "ref: refs/heads/main".to_owned(),
            refs: refs.iter().map(|&(name, byte)| (name.to_owned(), oid(byte))).collect(),
        }
    }

    #[test]
    fn ref_changes_lists_created_moved_and_deleted_refs() {
        let before = values(&[("refs/heads/gone", 1), ("refs/heads/main", 2), ("refs/tags/same", 3)]);
        let after = values(&[("refs/heads/main", 4), ("refs/heads/new", 5), ("refs/tags/same", 3)]);
        let change = |name: &str, before: Option<u8>, after: Option<u8>| RefChange {
            name: name.to_owned(),
            before: before.map(oid),
            after: after.map(oid),
        };
        assert_eq!(
            ref_changes(&before, &after),
            [
                change("refs/heads/gone", Some(1), None),
                change("refs/heads/main", Some(2), Some(4)),
                change("refs/heads/new", None, Some(5)),
            ]
        );
    }

    #[test]
    fn entries_round_trip_and_bad_lines_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/journal.jsonl");
        let entry = JournalEntry {
            schema_version: SCHEMA_VERSION,
            id: OpId("op-1".to_owned()),
            repo: "/r".to_owned(),
            operation: Operation::DeleteTag { name: "v1".to_owned() },
            started_at_ms: 1,
            duration_ms: 2,
            head_before: "ref: refs/heads/main".to_owned(),
            head_after: "ref: refs/heads/main".to_owned(),
            ref_changes: vec![RefChange { name: "refs/tags/v1".to_owned(), before: Some(oid(1)), after: None }],
            exit_code: Some(0),
            error: None,
            output: String::new(),
        };
        let journal = Journal::open(&path).unwrap();
        journal.append(&entry).unwrap();
        std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(b"{\"cut short\n").unwrap();
        journal.append(&entry).unwrap();

        assert_eq!(Journal::read(&path).unwrap(), [entry.clone(), entry]);
        assert_eq!(Journal::read(&dir.path().join("missing.jsonl")).unwrap(), []);
    }

    #[test]
    fn older_entries_without_optional_fields_still_read() {
        let line = r#"{"schemaVersion":1,"id":"x","repo":"/r","operation":{"kind":"pull"},"startedAtMs":0,"durationMs":0,"headBefore":"","headAfter":""}"#;
        let entry: JournalEntry = serde_json::from_str(line).unwrap();
        assert_eq!(entry.operation, Operation::Pull);
        assert!(entry.ref_changes.is_empty());
    }
}
