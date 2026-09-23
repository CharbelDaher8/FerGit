//! Repository sessions: an open repository and the immutable snapshot of it that the UI reads.
//!
//! Everything a session holds is derived from git and can be rebuilt at any time; the session never
//! patches its own copy of repository state. [`Session::refresh`] re-reads git and swaps in a new
//! snapshot, with a new generation, only if something visible changed.

mod registry;
mod relations;

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use fergit_graph::{GraphRow, Layout};

use crate::id_map::IdMap;
use crate::repo::{CommitTable, History, MergeNames, Repo, RepoError, RepoWatcher};
use crate::types::{
    CommitDetails, DiffSide, FileChange, FileDiff, Generation, Oid, RefKind, RefLabel, RepoInfo, Row, RowKind,
    RowLocation, RowsPage, Upstream, UpstreamState,
};
pub use registry::Sessions;
use relations::{Relations, RelationsBuilder, RowInput, branch_name};

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
    /// What each merge commit's message names, by commit. Messages never change, so entries stay
    /// valid across snapshots and a refresh reads only the merges it hasn't seen before.
    merge_names: Mutex<IdMap<Option<MergeNames>>>,
}

impl Session {
    /// Opens the repository containing `path` and reads its first snapshot.
    pub fn open(path: &Path) -> Result<Session, RepoError> {
        Session::with_repo(Repo::open(path)?)
    }

    /// Reads the first snapshot of an already opened repository.
    fn with_repo(repo: Repo) -> Result<Session, RepoError> {
        let history = repo.read_history()?;
        let mut merge_names = IdMap::default();
        read_merge_names(&repo, &history.commits, &mut merge_names)?;
        let snapshot = Snapshot::build(history, next_generation(), &merge_names);
        Ok(Session {
            repo,
            current: RwLock::new(Arc::new(snapshot)),
            refresh_lock: Mutex::new(()),
            merge_names: Mutex::new(merge_names),
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
        let history = self.repo.read_history()?;
        let mut merge_names = self.merge_names.lock().unwrap_or_else(PoisonError::into_inner);
        read_merge_names(&self.repo, &history.commits, &mut merge_names)?;
        let next = Arc::new(Snapshot::build(history, next_generation(), &merge_names));
        *self.current.write().unwrap_or_else(PoisonError::into_inner) = Arc::clone(&next);
        Ok(self.info_for(&next))
    }

    /// Keeps the session current: refreshes, on a background thread, whenever the repository may
    /// have changed, and calls `on_change` with the new info when the generation changed.
    ///
    /// A refresh that fails in the background is dropped rather than reported: the repository may be
    /// halfway through a git operation, and the next change or an explicit refresh surfaces a lasting
    /// problem. Watching stops when the returned watcher is dropped; it doesn't keep the session alive.
    pub fn watch(self: &Arc<Self>, on_change: impl Fn(RepoInfo) + Send + 'static) -> Result<RepoWatcher, RepoError> {
        let session = Arc::downgrade(self);
        self.repo.watch(move || {
            let Some(session) = session.upgrade() else {
                return;
            };
            let before = session.info().generation;
            if let Ok(info) = session.refresh()
                && info.generation != before
            {
                on_change(info);
            }
        })
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
            .zip(start..)
            .map(|((&slot, graph), row)| {
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
                        relations: Vec::new(),
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
                    relations: snapshot.relations.row(row),
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

    /// Files that differ between `from` and `to`; see [`Repo::changes`]. Reads the repository as it
    /// is now, not the snapshot: the index and worktree have no history to snapshot.
    pub fn changes(&self, from: Option<DiffSide>, to: DiffSide) -> Result<Vec<FileChange>, RepoError> {
        self.repo.changes(from, to)
    }

    /// How one file differs between `from` and `to`; see [`Repo::file_diff`].
    pub fn file_diff(
        &self,
        from: Option<DiffSide>,
        to: DiffSide,
        path: &str,
        old_path: Option<&str>,
    ) -> Result<FileDiff, RepoError> {
        self.repo.file_diff(from, to, path, old_path)
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
            head: snapshot.history.tips.head.id(),
        }
    }
}

/// Adds to `cache` what the message of each merge commit in `commits` names, for the merges it
/// doesn't hold yet. Only merges are read: their messages are what names branches that were merged
/// and deleted.
fn read_merge_names(repo: &Repo, commits: &CommitTable, cache: &mut IdMap<Option<MergeNames>>) -> Result<(), RepoError> {
    let unread: Vec<Oid> = (0..commits.len())
        .filter(|&i| commits.parents(i).len() > 1)
        .map(|i| commits.id(i))
        .filter(|id| !cache.contains_key(id))
        .collect();
    let names = repo.merge_names(&unread)?;
    cache.extend(unread.into_iter().zip(names));
    Ok(())
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
    /// Ref labels by commit, each list in display order, local branches with their upstream state.
    /// Refs that don't peel to a commit in the history have entries too; no row ever looks them up.
    labels: HashMap<Oid, Vec<RefLabel>>,
    /// Where branches split off and join, by row.
    relations: Relations,
}

impl Snapshot {
    fn build(mut history: History, generation: Generation, merge_names: &IdMap<Option<MergeNames>>) -> Snapshot {
        history.commits.index_parents();
        let labels = labels(&history);
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
        let mut relations = RelationsBuilder::with_aliases(relations::upstream_aliases(&tips.upstreams));
        let mut parent_lanes = Vec::new();
        let mut graph = Vec::with_capacity(slots.len());
        for &slot in &slots {
            let (kind, id, parents): (RowKind, Oid, &[Oid]) = match slot {
                Slot::WorkingTree => (RowKind::WorkingTree, Oid::ZERO, head.as_slice()),
                Slot::Commit(i) => (RowKind::Commit, history.commits.id(i as usize), history.commits.parents(i as usize)),
                Slot::Stash(s) => {
                    let stash = &tips.stashes[s as usize];
                    (RowKind::Stash, stash.id, std::slice::from_ref(&stash.base))
                }
            };
            let row = layout.push_with_parent_lanes(id, parents, &mut parent_lanes);
            // Only commits name branches; a stash's id could coincide with nothing a label names anyway.
            let is_commit = kind == RowKind::Commit;
            relations.push(RowInput {
                kind,
                graph: &row,
                parent_lanes: &parent_lanes,
                branch: labels.get(&id).filter(|_| is_commit).and_then(|labels| branch_name(labels)),
                merge: merge_names.get(&id).filter(|_| is_commit).and_then(Option::as_ref),
            });
            graph.push(row);
        }

        Snapshot { generation, relations: relations.finish(), history, slots, graph, labels }
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

/// Ref labels by commit, each list in display order (HEAD's branch first), with every local
/// branch's upstream state filled in.
fn labels(history: &History) -> HashMap<Oid, Vec<RefLabel>> {
    let upstreams = upstream_states(history);
    let mut labels: HashMap<Oid, Vec<RefLabel>> = HashMap::new();
    for (id, label) in &history.tips.refs {
        let mut label = label.clone();
        if label.kind == RefKind::LocalBranch {
            label.upstream = upstreams.get(label.full_name.as_str()).cloned();
        }
        labels.entry(*id).or_default().push(label);
    }
    for list in labels.values_mut() {
        list.sort_by(|a, b| (!a.is_head, a.kind, &a.name).cmp(&(!b.is_head, b.kind, &b.name)));
    }
    labels
}

/// The upstream state of each local branch that has an upstream configured, by the branch's full
/// name. A branch whose own commit isn't in the history (it names a tree, say) is left out, and so
/// is one whose upstream names something other than a commit.
fn upstream_states(history: &History) -> HashMap<&str, Upstream> {
    let tips = &history.tips;
    let branch_id = |full_name: &str| tips.refs.iter().find(|(_, label)| label.full_name == full_name).map(|(id, _)| *id);

    let ids: Vec<Oid> = tips
        .upstreams
        .iter()
        .flat_map(|upstream| branch_id(&upstream.branch).into_iter().chain(upstream.id))
        .collect();
    let rows: HashMap<Oid, usize> = ids
        .iter()
        .zip(history.commits.rows_of(&ids))
        .filter_map(|(&id, row)| Some((id, row?)))
        .collect();
    let row = |id: Option<Oid>| id.and_then(|id| rows.get(&id).copied());

    let mut states = HashMap::new();
    let mut tracked = Vec::new();
    let mut pairs = Vec::new();
    for upstream in &tips.upstreams {
        let Some(branch_row) = row(branch_id(&upstream.branch)) else {
            continue;
        };
        if upstream.id.is_none() {
            states.insert(upstream.branch.as_str(), Upstream { name: upstream.name.clone(), state: UpstreamState::Gone });
        } else if let Some(upstream_row) = row(upstream.id) {
            tracked.push(upstream);
            pairs.push((branch_row, upstream_row));
        }
    }
    if !pairs.is_empty() {
        for (upstream, (ahead, behind)) in tracked.into_iter().zip(history.commits.ahead_behind(&pairs)) {
            let state = UpstreamState::Tracking { ahead, behind };
            states.insert(upstream.branch.as_str(), Upstream { name: upstream.name.clone(), state });
        }
    }
    states
}

fn stash_label(index: u32) -> RefLabel {
    RefLabel {
        kind: RefKind::Stash,
        name: format!("stash@{{{index}}}"),
        full_name: "refs/stash".to_owned(),
        is_head: false,
        upstream: None,
    }
}
