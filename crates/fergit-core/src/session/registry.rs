//! The sessions open at once, one per tab, each keeping itself current with its own watcher.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, PoisonError, RwLock};

use super::Session;
use crate::repo::{Repo, RepoError, RepoWatcher};
use crate::types::{RepoInfo, SessionId};

/// The open sessions, by id. All methods are safe to call concurrently.
///
/// A repository is open at most once: opening one that is already open, by whatever path inside it,
/// returns the session it already has. Ids are never reused, so a late request naming a closed
/// session finds nothing rather than whichever session came after it.
#[derive(Default)]
pub struct Sessions {
    open: RwLock<HashMap<SessionId, Entry>>,
    next_id: AtomicU32,
}

struct Entry {
    session: Arc<Session>,
    /// Identifies the repository, to find it when it is opened again.
    key: PathBuf,
    /// `None` if watching failed; changes then show up on the next explicit refresh instead.
    _watcher: Option<RepoWatcher>,
}

impl Sessions {
    /// Opens the repository containing `path`, or finds the session that already has it open.
    ///
    /// A new session keeps itself current: whenever the repository changes on disk, `on_change` is
    /// called on a background thread with the session's id and new info, until the session is
    /// closed. For a session that was already open, `on_change` is dropped unused.
    pub fn open(
        &self,
        path: &Path,
        on_change: impl Fn(SessionId, RepoInfo) + Send + 'static,
    ) -> Result<(SessionId, Arc<Session>), RepoError> {
        let repo = Repo::open(path)?;
        let key = repository_key(repo.root());
        if let Some(found) = self.find(&key) {
            return Ok(found);
        }

        // Reading the history is the slow part; it runs without holding the lock.
        let session = Arc::new(Session::with_repo(repo)?);
        let mut open = self.open.write().unwrap_or_else(PoisonError::into_inner);
        // Another call may have opened the same repository meanwhile; keep the first.
        if let Some((&id, entry)) = open.iter().find(|(_, entry)| entry.key == key) {
            return Ok((id, Arc::clone(&entry.session)));
        }
        let id = SessionId(self.next_id.fetch_add(1, Ordering::Relaxed));
        let watcher = session.watch(move |info| on_change(id, info)).ok();
        open.insert(id, Entry { session: Arc::clone(&session), key, _watcher: watcher });
        Ok((id, session))
    }

    /// The open session `id`; `None` if it was closed or never existed.
    pub fn get(&self, id: SessionId) -> Option<Arc<Session>> {
        let open = self.open.read().unwrap_or_else(PoisonError::into_inner);
        open.get(&id).map(|entry| Arc::clone(&entry.session))
    }

    /// Closes session `id` and stops watching its repository. Requests already running on it finish.
    /// Returns whether it was open; closing a closed session does nothing.
    pub fn close(&self, id: SessionId) -> bool {
        // Drop the entry (and with it the watcher, which may block briefly) outside the lock.
        let closed = self.open.write().unwrap_or_else(PoisonError::into_inner).remove(&id);
        closed.is_some()
    }

    /// Ids of the open sessions, in the order they were opened.
    pub fn ids(&self) -> Vec<SessionId> {
        let mut ids: Vec<SessionId> = self.open.read().unwrap_or_else(PoisonError::into_inner).keys().copied().collect();
        ids.sort();
        ids
    }

    fn find(&self, key: &Path) -> Option<(SessionId, Arc<Session>)> {
        let open = self.open.read().unwrap_or_else(PoisonError::into_inner);
        open.iter().find(|(_, entry)| entry.key == key).map(|(&id, entry)| (id, Arc::clone(&entry.session)))
    }
}

/// What identifies a repository: its root with symlinks, `.` and `..` resolved and, where the file
/// system ignores case, the case on disk. Falls back to `root` itself if it can't be resolved.
fn repository_key(root: &Path) -> PathBuf {
    std::fs::canonicalize(root).unwrap_or_else(|_| root.to_owned())
}
