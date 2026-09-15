//! The commits of a history in display order, and questions answered from them alone.

use crate::id_map::IdMap;
use crate::types::Oid;

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
    /// `parent_rows[k]` is the row of `parents[k]` ([`NO_ROW`] if it has none), once the table is
    /// indexed; empty before.
    parent_rows: Vec<u32>,
}

/// The row recorded for a parent that isn't in the table.
const NO_ROW: u32 = u32::MAX;

impl CommitTable {
    pub fn with_capacity(commits: usize) -> CommitTable {
        let links = commits + commits / 8;
        CommitTable {
            ids: Vec::with_capacity(commits),
            parent_ends: Vec::with_capacity(commits),
            parents: Vec::with_capacity(links),
            parent_rows: Vec::with_capacity(links),
        }
    }

    /// Appends a commit. The caller is responsible for display order and for filtering parents.
    /// The table is no longer indexed afterwards; see [`CommitTable::index_parents`].
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
        &self.parents[self.parent_range(index)]
    }

    /// Records the row of every parent, which [`CommitTable::ahead_behind`] needs. Tables read by
    /// [`super::Repo::read_history`] come indexed; a table built with `push` needs this once after
    /// the last push.
    pub fn index_parents(&mut self) {
        if self.is_indexed() {
            return;
        }
        let rows: IdMap<u32> = self.ids.iter().enumerate().map(|(row, id)| (*id, row_number(row))).collect();
        self.parent_rows = self.parents.iter().map(|parent| rows.get(parent).copied().unwrap_or(NO_ROW)).collect();
    }

    /// Sets the parent rows directly, for a builder that already knows them.
    pub(super) fn set_parent_rows(&mut self, rows: Vec<u32>) {
        debug_assert_eq!(rows.len(), self.parents.len());
        self.parent_rows = rows;
    }

    /// The row of each of `ids`, `None` for ids not in the table, found in one pass over the table.
    pub fn rows_of(&self, ids: &[Oid]) -> Vec<Option<usize>> {
        let mut rows: IdMap<Option<usize>> = ids.iter().map(|&id| (id, None)).collect();
        let mut missing = rows.len();
        for (row, id) in self.ids.iter().enumerate() {
            if missing == 0 {
                break;
            }
            if let Some(slot) = rows.get_mut(id)
                && slot.is_none()
            {
                *slot = Some(row);
                missing -= 1;
            }
        }
        ids.iter().map(|id| rows[id]).collect()
    }

    /// For each `(local, upstream)` pair of rows, how many commits are reachable from `local` but not
    /// from `upstream` (ahead), and the reverse (behind), like
    /// `git rev-list --left-right --count local...upstream`.
    ///
    /// Walks down the rows from the higher of the two, marking each commit with the side(s) it is
    /// reachable from. Every commit sits above its parents, so a row's marks are complete when the
    /// walk reaches it. The walk stops once every marked row still below is reachable from both
    /// sides, because nothing further down can then be reachable from only one. Two branches close
    /// to each other cost a few rows however long the history is; the worst case is a branch far
    /// from its upstream, which costs about the rows down to where the two meet.
    ///
    /// Panics if the table isn't indexed ([`CommitTable::index_parents`]) or a row is out of range.
    pub fn ahead_behind(&self, pairs: &[(usize, usize)]) -> Vec<(u32, u32)> {
        const LOCAL: u8 = 1;
        const UPSTREAM: u8 = 2;
        const BOTH: u8 = LOCAL | UPSTREAM;

        assert!(self.is_indexed(), "CommitTable::ahead_behind needs an indexed table");
        let mut marks = vec![0u8; self.len()];
        let mut marked: Vec<usize> = Vec::new();
        pairs
            .iter()
            .map(|&(local, upstream)| {
                assert!(local < self.len() && upstream < self.len(), "row out of range");
                marks[local] |= LOCAL;
                marks[upstream] |= UPSTREAM;
                marked.extend([local, upstream]);
                // Marked rows the walk hasn't reached yet that are reachable from one side only.
                let mut one_sided = if local == upstream { 0 } else { 2 };
                let (mut ahead, mut behind) = (0, 0);
                let mut row = local.min(upstream);
                while one_sided > 0 && row < self.len() {
                    let mark = marks[row];
                    if mark == LOCAL || mark == UPSTREAM {
                        one_sided -= 1;
                        if mark == LOCAL {
                            ahead += 1;
                        } else {
                            behind += 1;
                        }
                    }
                    if mark != 0 {
                        for &parent in &self.parent_rows[self.parent_range(row)] {
                            if parent == NO_ROW {
                                continue;
                            }
                            let parent = parent as usize;
                            let (old, new) = (marks[parent], marks[parent] | mark);
                            if old == new {
                                continue;
                            }
                            if old != 0 {
                                // One side's commit is now reachable from both.
                                one_sided -= 1;
                            } else {
                                marked.push(parent);
                                if new != BOTH {
                                    one_sided += 1;
                                }
                            }
                            marks[parent] = new;
                        }
                    }
                    row += 1;
                }
                for row in marked.drain(..) {
                    marks[row] = 0;
                }
                (ahead, behind)
            })
            .collect()
    }

    fn parent_range(&self, index: usize) -> std::ops::Range<usize> {
        let start = if index == 0 { 0 } else { self.parent_ends[index - 1] as usize };
        start..self.parent_ends[index] as usize
    }

    fn is_indexed(&self) -> bool {
        self.parent_rows.len() == self.parents.len()
    }
}

fn row_number(row: usize) -> u32 {
    u32::try_from(row).expect("fewer than 4 billion commits")
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn oid(n: usize) -> Oid {
        let mut bytes = [0u8; 20];
        bytes[..8].copy_from_slice(&(n as u64 + 1).to_le_bytes());
        Oid::from_bytes(&bytes).unwrap()
    }

    /// A table whose row `i` has parents `parents[i]` (rows below it).
    fn table(parents: &[Vec<usize>]) -> CommitTable {
        let mut table = CommitTable::default();
        for (row, parents) in parents.iter().enumerate() {
            assert!(parents.iter().all(|&p| p > row));
            table.push(oid(row), parents.iter().map(|&p| oid(p)));
        }
        table.index_parents();
        table
    }

    fn reachable(parents: &[Vec<usize>], from: usize) -> HashSet<usize> {
        let mut seen = HashSet::from([from]);
        let mut stack = vec![from];
        while let Some(row) = stack.pop() {
            for &parent in &parents[row] {
                if seen.insert(parent) {
                    stack.push(parent);
                }
            }
        }
        seen
    }

    #[test]
    fn counts_match_reachability_for_every_pair_of_a_branchy_history() {
        // A deterministic pseudo-random DAG with merges, forks and several roots.
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = |bound: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % bound as u64) as usize
        };
        let rows = 120;
        let parents: Vec<Vec<usize>> = (0..rows)
            .map(|row| {
                let below = rows - row - 1;
                if below == 0 || next(10) == 0 {
                    return Vec::new();
                }
                let mut parents = vec![row + 1 + next(below.min(4))];
                if next(4) == 0 {
                    let other = row + 1 + next(below);
                    if !parents.contains(&other) {
                        parents.push(other);
                    }
                }
                parents
            })
            .collect();
        let table = table(&parents);

        let pairs: Vec<(usize, usize)> = (0..rows).flat_map(|a| (0..rows).map(move |b| (a, b))).collect();
        let counts = table.ahead_behind(&pairs);
        for (&(local, upstream), &(ahead, behind)) in pairs.iter().zip(&counts) {
            let (l, u) = (reachable(&parents, local), reachable(&parents, upstream));
            assert_eq!(
                (ahead as usize, behind as usize),
                (l.difference(&u).count(), u.difference(&l).count()),
                "local row {local}, upstream row {upstream}"
            );
        }
    }

    #[test]
    fn missing_parents_and_identical_ends_count_nothing_extra() {
        let mut table = CommitTable::default();
        table.push(oid(0), [oid(1), oid(99)]);
        table.push(oid(1), []);
        table.index_parents();
        assert_eq!(table.ahead_behind(&[(0, 0), (0, 1), (1, 0)]), [(0, 0), (1, 0), (0, 1)]);
    }

    #[test]
    fn rows_of_finds_each_id_once() {
        let table = table(&[vec![1], vec![2], vec![]]);
        assert_eq!(table.rows_of(&[oid(2), oid(7), oid(0), oid(2)]), [Some(2), None, Some(0), Some(2)]);
    }
}
