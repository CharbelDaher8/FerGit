//! Repository sessions: an open repository and the immutable snapshot of it that the UI reads.
//!
//! Everything a session holds is derived from git and can be rebuilt at any time; the session never
//! patches its own copy of repository state. [`Session::refresh`] re-reads git and swaps in a new
//! snapshot, with a new generation, only if something visible changed.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use fergit_graph::{GraphRow, Layout};

use crate::repo::{History, Repo, RepoError};
use crate::types::{
    CommitDetails, Generation, Oid, RefKind, RefLabel, RepoInfo, Row, RowKind, RowLocation, RowsPage,
};

/// The generation of the next snapshot any session in this process builds. Sharing it across
/// sessions means a generation from a previously open repository (in a late response or event) is
/// always older than every generation of the current one, so it can be recognized as stale.
static NEXT_GENERATION: AtomicU32 = AtomicU32::new(1);

fn next_generation() -> Generation {
    Generation(NEXT_GENERATION.fetch_add(1, Ordering::Relaxed))
}

/// An open repository. All methods are safe to call concurrently.
pub struct Session {
    repo: Repo,
    current: RwLock<Arc<Snapshot>>,
    /// Serializes refreshes so two of them can't install snapshots out of order.
    refresh_lock: Mutex<()>,
}

impl Session {
    /// Opens the repository containing `path` and reads its first snapshot.
    pub fn open(path: &Path) -> Result<Session, RepoError> {
        let repo = Repo::open(path)?;
        let snapshot = Snapshot::build(repo.read_history()?, next_generation());
        Ok(Session {
            repo,
            current: RwLock::new(Arc::new(snapshot)),
            refresh_lock: Mutex::new(()),
        })
    }

    pub fn info(&self) -> RepoInfo {
        self.info_for(&self.snapshot())
    }

    /// Re-reads the repository and returns info for the snapshot that is current afterwards. The
    /// generation is unchanged if nothing visible changed.
    ///
    /// Only the tips are read at first; the commit walk, the expensive part of a long history, runs
    /// only when they differ from the current snapshot's.
    pub fn refresh(&self) -> Result<RepoInfo, RepoError> {
        let _refreshing = self.refresh_lock.lock().unwrap_or_else(PoisonError::into_inner);
        let current = self.snapshot();
        if self.repo.read_tips()? == current.history.tips {
            return Ok(self.info_for(&current));
        }
        let next = Arc::new(Snapshot::build(self.repo.read_history()?, next_generation()));
        *self.current.write().unwrap_or_else(PoisonError::into_inner) = Arc::clone(&next);
        Ok(self.info_for(&next))
    }

    /// Rows `start..start + len` of the current snapshot, clamped to the rows that exist.
    pub fn rows(&self, start: u32, len: u32) -> Result<RowsPage, RepoError> {
        let snapshot = self.snapshot();
        let total = snapshot.row_count();
        let start = start.min(total);
        let end = start.saturating_add(len).min(total);
        let range = start as usize..end as usize;

        let slots = &snapshot.slots[range.clone()];
        let commit_ids: Vec<Oid> = slots
            .iter()
            .filter(|slot| !matches!(slot, Slot::WorkingTree))
            .map(|&slot| snapshot.id(slot))
            .collect();
        let mut summaries = self.repo.commit_summaries(&commit_ids)?.into_iter();

        let rows = slots
            .iter()
            .zip(&snapshot.graph[range])
            .map(|(&slot, graph)| {
                if slot == Slot::WorkingTree {
                    return Row {
                        kind: RowKind::WorkingTree,
                        id: Oid::ZERO,
                        graph: graph.clone(),
                        summary: "Uncommitted changes".to_owned(),
                        author_name: String::new(),
                        author_email: String::new(),
                        time: 0,
                        refs: Vec::new(),
                    };
                }
                let id = snapshot.id(slot);
                let commit = summaries.next().flatten().unwrap_or_default();
                let (kind, refs) = match slot {
                    Slot::Stash(s) => (RowKind::Stash, vec![stash_label(snapshot.history.tips.stashes[s as usize].index)]),
                    _ => (RowKind::Commit, snapshot.labels.get(&id).cloned().unwrap_or_default()),
                };
                Row {
                    kind,
                    id,
                    graph: graph.clone(),
                    summary: commit.summary,
                    author_name: commit.author_name,
                    author_email: commit.author_email,
                    time: commit.author_time,
                    refs,
                }
            })
            .collect();

        Ok(RowsPage { generation: snapshot.generation, start, total, rows })
    }

    /// Where the row showing `id` is in the current snapshot: the row of a commit or a stash, or,
    /// for [`Oid::ZERO`], the uncommitted-changes row. Scans the rows, which takes milliseconds even
    /// for a million-commit history.
    pub fn locate(&self, id: Oid) -> RowLocation {
        let snapshot = self.snapshot();
        let row = snapshot.slots.iter().position(|&slot| snapshot.id(slot) == id);
        RowLocation {
            generation: snapshot.generation,
            row: row.map(|i| u32::try_from(i).expect("fewer than 4 billion rows")),
        }
    }

    /// Details of the commit `id`, or `None` if it isn't a commit (including [`Oid::ZERO`]).
    pub fn commit_details(&self, id: Oid) -> Result<Option<CommitDetails>, RepoError> {
        if id == Oid::ZERO {
            return Ok(None);
        }
        self.repo.commit_details(id)
    }

    fn snapshot(&self) -> Arc<Snapshot> {
        Arc::clone(&self.current.read().unwrap_or_else(PoisonError::into_inner))
    }

    fn info_for(&self, snapshot: &Snapshot) -> RepoInfo {
        let root = self.repo.root();
        RepoInfo {
            root: root.display().to_string(),
            name: root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            generation: snapshot.generation,
            row_count: snapshot.row_count(),
        }
    }
}

/// What one graph row shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    WorkingTree,
    /// Index into `history.commits`.
    Commit(u32),
    /// Index into `history.tips.stashes`.
    Stash(u32),
}

/// One immutable, fully laid-out view of the repository.
struct Snapshot {
    generation: Generation,
    history: History,
    /// Row order. Stash rows sit directly above their base commit; the working-tree row is first.
    slots: Vec<Slot>,
    /// `graph[i]` is the geometry of `slots[i]`.
    graph: Vec<GraphRow>,
    /// Ref labels by commit, each list in display order. Refs that don't peel to a commit in the
    /// history have entries too; no row ever looks them up.
    labels: HashMap<Oid, Vec<RefLabel>>,
}

impl Snapshot {
    fn build(history: History, generation: Generation) -> Snapshot {
        let tips = &history.tips;
        let mut stashes_by_base: HashMap<Oid, Vec<u32>> = HashMap::new();
        for (i, stash) in tips.stashes.iter().enumerate() {
            stashes_by_base.entry(stash.base).or_default().push(i as u32);
        }

        // A stash whose base isn't in the history gets no row: nothing to draw it on.
        let mut slots = Vec::with_capacity(history.commits.len() + tips.stashes.len() + 1);
        if tips.worktree_dirty {
            slots.push(Slot::WorkingTree);
        }
        for i in 0..history.commits.len() {
            if let Some(stashes) = stashes_by_base.get(&history.commits.id(i)) {
                slots.extend(stashes.iter().map(|&s| Slot::Stash(s)));
            }
            slots.push(Slot::Commit(i as u32));
        }

        let head = tips.head.id();
        let mut layout = Layout::new();
        let graph = slots
            .iter()
            .map(|&slot| match slot {
                Slot::WorkingTree => layout.push(Oid::ZERO, head.as_slice()),
                Slot::Commit(i) => layout.push(history.commits.id(i as usize), history.commits.parents(i as usize)),
                Slot::Stash(s) => {
                    let stash = &tips.stashes[s as usize];
                    layout.push(stash.id, std::slice::from_ref(&stash.base))
                }
            })
            .collect();

        let mut labels: HashMap<Oid, Vec<RefLabel>> = HashMap::new();
        for (id, label) in &tips.refs {
            labels.entry(*id).or_default().push(label.clone());
        }
        for list in labels.values_mut() {
            list.sort_by(|a, b| (!a.is_head, a.kind, &a.name).cmp(&(!b.is_head, b.kind, &b.name)));
        }

        Snapshot { generation, history, slots, graph, labels }
    }

    fn row_count(&self) -> u32 {
        u32::try_from(self.slots.len()).expect("fewer than 4 billion rows")
    }

    fn id(&self, slot: Slot) -> Oid {
        match slot {
            Slot::WorkingTree => Oid::ZERO,
            Slot::Commit(i) => self.history.commits.id(i as usize),
            Slot::Stash(s) => self.history.tips.stashes[s as usize].id,
        }
    }
}

fn stash_label(index: u32) -> RefLabel {
    RefLabel {
        kind: RefKind::Stash,
        name: format!("stash@{{{index}}}"),
        full_name: "refs/stash".to_owned(),
        is_head: false,
    }
}
