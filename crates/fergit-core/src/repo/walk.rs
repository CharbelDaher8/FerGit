//! Collecting reachable commits and putting them in display order.
//!
//! Two passes rather than gix-traverse's incremental topological walk (`Sorting::DateOrder`):
//!
//! 1. Collect every commit reachable from the tips with its parents and committer time. With a
//!    commit-graph file this reads fixed-size records and decodes no commit objects; commits newer
//!    than the file fall back to the object database one by one.
//! 2. Emit Kahn-style: a commit becomes ready once all its children are emitted, and the ready
//!    commit with the latest committer time goes next, ties broken by id.
//!
//! Readiness, not time, guarantees children come before parents, so clock skew (a child dated
//! before its parent) can reorder commits but never break that rule. gix-traverse's walk would too,
//! but breaks date ties by queue insertion order, which depends on the order tips were given, and
//! looks up each commit's parents several times. Here the order depends only on the graph.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use gix::ObjectId;
use gix::hashtable::hash_map::Entry;

use super::{CommitTable, RepoError, git_error, to_oid};

pub(super) struct Walk {
    pub(super) commits: CommitTable,
    /// `tip_is_commit[i]` tells whether `tips[i]` names a commit that exists (as opposed to a
    /// missing object, a tree or a blob).
    pub(super) tip_is_commit: Vec<bool>,
}

/// Every commit reachable from `tips`, in display order.
pub(super) fn walk(repo: &gix::Repository, tips: &[ObjectId]) -> Result<Walk, RepoError> {
    let graph = collect(repo, tips)?;
    let tip_is_commit = tips
        .iter()
        .map(|tip| graph.index.get(tip).is_some_and(|&i| graph.found[i as usize]))
        .collect();
    Ok(Walk { commits: graph.display_order(), tip_is_commit })
}

/// Commits and the edges between them, addressed by dense `u32` indices.
#[derive(Default)]
struct Graph {
    ids: Vec<ObjectId>,
    index: gix::hashtable::HashMap<ObjectId, u32>,
    /// Whether the object exists and is a commit. False for parents cut off by a shallow clone and
    /// for tips that name trees or blobs; such nodes never appear in the output.
    found: Vec<bool>,
    /// Committer time, seconds since the epoch. Only meaningful where `found`.
    times: Vec<i64>,
    /// `parents[start..end]` for `(start, end) = parent_ranges[i]` are commit `i`'s parents in git order.
    parent_ranges: Vec<(u32, u32)>,
    parents: Vec<u32>,
}

impl Graph {
    /// The index of `id`, and whether it was just added.
    fn node(&mut self, id: ObjectId) -> (u32, bool) {
        match self.index.entry(id) {
            Entry::Occupied(entry) => (*entry.get(), false),
            Entry::Vacant(entry) => {
                let i = u32::try_from(self.ids.len()).expect("fewer than 4 billion commits");
                entry.insert(i);
                self.ids.push(id);
                self.found.push(false);
                self.times.push(0);
                self.parent_ranges.push((0, 0));
                (i, true)
            }
        }
    }

    fn parents_of(&self, i: u32) -> &[u32] {
        let (start, end) = self.parent_ranges[i as usize];
        &self.parents[start as usize..end as usize]
    }

    fn display_order(&self) -> CommitTable {
        let found = |i: u32| self.found[i as usize];
        let nodes = 0..u32::try_from(self.ids.len()).expect("fewer than 4 billion commits");

        let mut unemitted_children = vec![0u32; self.ids.len()];
        for i in nodes.clone().filter(|&i| found(i)) {
            for &parent in self.parents_of(i).iter().filter(|&&p| found(p)) {
                unemitted_children[parent as usize] += 1;
            }
        }

        let ready_entry = |i: u32| Ready { time: self.times[i as usize], id: self.ids[i as usize], index: i };
        let mut ready: BinaryHeap<Ready> = nodes
            .filter(|&i| found(i) && unemitted_children[i as usize] == 0)
            .map(ready_entry)
            .collect();
        let mut table = CommitTable::with_capacity(self.found.iter().filter(|&&f| f).count());
        while let Some(Ready { index, id, .. }) = ready.pop() {
            let parents = self.parents_of(index).iter().copied().filter(|&p| found(p));
            table.push(to_oid(&id), parents.clone().map(|p| to_oid(&self.ids[p as usize])));
            for parent in parents {
                let children = &mut unemitted_children[parent as usize];
                *children -= 1;
                if *children == 0 {
                    ready.push(ready_entry(parent));
                }
            }
        }
        // Commits on a cycle never become ready and are left out. Git histories can't contain
        // cycles, but a doctored commit-graph file could describe one; dropping those commits is
        // better than looping or panicking.
        table
    }
}

/// A commit whose children have all been emitted. Ordered so that `BinaryHeap::pop` yields the
/// latest committer time first and, among equal times, the smallest id.
#[derive(PartialEq, Eq)]
struct Ready {
    time: i64,
    id: ObjectId,
    index: u32,
}

impl Ord for Ready {
    fn cmp(&self, other: &Ready) -> Ordering {
        self.time.cmp(&other.time).then_with(|| other.id.cmp(&self.id))
    }
}

impl PartialOrd for Ready {
    fn partial_cmp(&self, other: &Ready) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn collect(repo: &gix::Repository, tips: &[ObjectId]) -> Result<Graph, RepoError> {
    let commit_graph = repo
        .commit_graph_if_enabled()
        .map_err(|err| git_error("Can't read the commit-graph file", err))?;
    let mut lookup = repo.revision_graph::<()>(commit_graph.as_ref());

    let mut graph = Graph::default();
    let mut pending = Vec::new();
    for &tip in tips {
        let (i, new) = graph.node(tip);
        if new {
            pending.push(i);
        }
    }
    while let Some(i) = pending.pop() {
        let id = graph.ids[i as usize];
        let read_error = |err| git_error(format!("Can't read commit {id}"), err);
        // `None` for a missing object (a shallow clone's boundary) or one that isn't a commit.
        let Some(commit) = lookup.try_lookup(&id).map_err(read_error)? else {
            continue;
        };
        let time = commit
            .committer_timestamp()
            .map_err(|err| git_error(format!("Can't read commit {id}"), err))?;
        let start = graph.parents.len();
        for parent in commit.iter_parents() {
            let parent = parent.map_err(|err| git_error(format!("Can't read the parents of commit {id}"), err))?;
            let (p, new) = graph.node(parent);
            graph.parents.push(p);
            if new {
                pending.push(p);
            }
        }
        let end = graph.parents.len();
        let link = |n: usize| u32::try_from(n).expect("fewer than 4 billion parent links");
        graph.parent_ranges[i as usize] = (link(start), link(end));
        graph.times[i as usize] = time;
        graph.found[i as usize] = true;
    }
    Ok(graph)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> ObjectId {
        ObjectId::from_bytes_or_panic(&[n; 20])
    }

    /// A graph of `(id, committer time, parents)`; ids that are only named as parents are missing.
    fn graph(commits: &[(u8, i64, &[u8])]) -> Graph {
        let mut graph = Graph::default();
        for &(n, time, parents) in commits {
            let (i, _) = graph.node(id(n));
            let start = graph.parents.len() as u32;
            for &parent in parents {
                let (p, _) = graph.node(id(parent));
                graph.parents.push(p);
            }
            graph.parent_ranges[i as usize] = (start, graph.parents.len() as u32);
            graph.times[i as usize] = time;
            graph.found[i as usize] = true;
        }
        graph
    }

    fn order(table: &CommitTable) -> Vec<u8> {
        (0..table.len()).map(|i| table.id(i).as_bytes()[0]).collect()
    }

    #[test]
    fn children_come_first_even_when_dated_before_their_parent() {
        // 1 is the newest by date but the parent of both others.
        let graph = graph(&[(1, 30, &[]), (2, 10, &[1]), (3, 20, &[1])]);
        assert_eq!(order(&graph.display_order()), [3, 2, 1]);
    }

    #[test]
    fn later_committer_dates_come_first_and_ties_go_to_the_smaller_id() {
        let graph = graph(&[(5, 10, &[]), (9, 10, &[]), (3, 10, &[]), (7, 11, &[])]);
        assert_eq!(order(&graph.display_order()), [7, 3, 5, 9]);
    }

    #[test]
    fn missing_parents_are_omitted() {
        // 1 is missing, as at a shallow clone's boundary.
        let graph = graph(&[(2, 10, &[1, 3]), (3, 5, &[])]);
        let table = graph.display_order();
        assert_eq!(order(&table), [2, 3]);
        assert_eq!(table.parents(0), [to_oid(&id(3))]);
        assert!(table.parents(1).is_empty());
    }

    #[test]
    fn commits_on_a_cycle_are_dropped_instead_of_looping() {
        let graph = graph(&[(1, 10, &[2]), (2, 10, &[1]), (3, 10, &[])]);
        assert_eq!(order(&graph.display_order()), [3]);
    }
}
