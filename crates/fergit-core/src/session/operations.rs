//! Running operations on a session: one at a time, each at most once, each journaled.

use std::collections::VecDeque;
use std::sync::{Arc, PoisonError};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use super::Session;
use super::journal::{Journal, JournalEntry, JournalError, SCHEMA_VERSION, ref_changes};
use crate::askpass::Askpass;
use crate::types::{OpId, OpOutcome, Operation};

/// How many finished operations a session remembers to recognize a repeated id. Repeats come from
/// double clicks and retried requests, seconds apart, so a short memory is plenty.
const REMEMBERED: usize = 64;

/// What running operations needs from the application.
#[derive(Clone, Default)]
pub struct OpContext {
    /// Where to record operations; `None` records nothing.
    pub journal: Option<Arc<Journal>>,
    /// How git asks the user for credentials; `None` lets operations that need credentials no
    /// helper supplies fail instead.
    pub askpass: Option<Askpass>,
}

/// The single-writer queue's state: the outcomes of the most recent operations, oldest first.
#[derive(Default)]
pub(super) struct Writer {
    finished: VecDeque<(OpId, OpOutcome)>,
}

impl Session {
    /// Runs `op` and returns how it ended, passing each line of git's progress to `progress`.
    ///
    /// - **One writer.** Operations on a session run one at a time, in the order they arrive; a
    ///   call waits for the running one to finish. Reads never wait: they use the current snapshot.
    /// - **At most once.** A call with the `id` of an operation that already ran (or is running)
    ///   doesn't run it again; it returns that operation's outcome.
    /// - **Re-read, never patched.** Afterwards the session re-reads the repository, whether the
    ///   operation succeeded or not, and the outcome carries the resulting [`crate::RepoInfo`].
    /// - **Journaled.** Every operation that runs is appended to `context.journal`, with the refs it
    ///   moved.
    pub fn run(&self, id: OpId, op: Operation, context: &OpContext, progress: &mut dyn FnMut(&str)) -> OpOutcome {
        let mut writer = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((_, outcome)) = writer.finished.iter().find(|(done, _)| *done == id) {
            return outcome.clone();
        }

        let started_at = SystemTime::now();
        let clock = Instant::now();
        // If the refs can't be read the journal can't say what moved, but the operation itself
        // may still work; git will report what is wrong if not.
        let before = self.repo.ref_values().unwrap_or_default();
        let result = self.repo.run(&op, context.askpass.as_ref(), progress);
        let duration = clock.elapsed();
        let after = self.repo.ref_values().unwrap_or_default();
        let info = self.refresh().unwrap_or_else(|_| self.info());

        if let Some(journal) = &context.journal {
            let entry = JournalEntry {
                schema_version: SCHEMA_VERSION,
                id: id.clone(),
                repo: info.root.clone(),
                operation: op,
                started_at_ms: started_at.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64),
                duration_ms: duration.as_millis() as u64,
                head_before: before.head.clone(),
                head_after: after.head.clone(),
                ref_changes: ref_changes(&before, &after),
                exit_code: match &result {
                    Ok(()) => Some(0),
                    Err(failure) => failure.exit_code,
                },
                error: result
                    .as_ref()
                    .err()
                    .map(|failure| JournalError { kind: failure.error.kind, message: failure.error.message.clone() }),
                output: result.as_ref().err().map(|failure| failure.error.output.clone()).unwrap_or_default(),
            };
            // The operation has happened whether or not it can be recorded, and failing it now
            // would misreport it. A journal that can't be written (a full disk) loses this entry.
            let _ = journal.append(&entry);
        }

        let outcome = match result {
            Ok(()) => OpOutcome::Done { info },
            Err(failure) => OpOutcome::Failed { info, error: failure.error },
        };
        if writer.finished.len() == REMEMBERED {
            writer.finished.pop_front();
        }
        writer.finished.push_back((id, outcome.clone()));
        outcome
    }
}
