//! Keeping some rows of a history and reconnecting them, so a filtered history lays out as a graph
//! of its own.

/// The rows of a history that a filter keeps, each with its parents rewritten to the nearest kept
/// rows below it, the way `git log --parents` rewrites parents when it hides commits.
///
/// Rows are numbered in display order, so feeding [`Subgraph::rows`] to [`crate::Layout`] with
/// [`Subgraph::parents`] (mapped back to ids) lays out the filtered history, every line still
/// joining a commit to its nearest shown ancestors.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Subgraph {
    rows: Vec<u32>,
    /// `parent_ends[i]` is one past the last index into `parents` belonging to kept row `i`.
    parent_ends: Vec<u32>,
    parents: Vec<u32>,
}

impl Subgraph {
    /// Keeps the rows `0..len` for which `keep` is true.
    ///
    /// `parents(row)` lists the rows a row's line continues to, each greater than `row` (display
    /// order puts children first); rows `>= len` are ignored, like parents that are never pushed to
    /// a layout. A hidden row passes its line on to the kept rows below it through every parent
    /// listed, so a caller that wants a hidden merge followed down one side only (git's history
    /// simplification) lists just that side for it.
    ///
    /// Costs O(rows + links) plus, for each hidden row, the number of kept rows it leads to; that
    /// is one or two unless many parallel lines are hidden at once.
    ///
    /// Panics if a row lists a parent above itself.
    pub fn new<'a>(len: usize, parents: impl Fn(usize) -> &'a [u32], keep: impl Fn(usize) -> bool) -> Subgraph {
        let row_number = |i: usize| u32::try_from(i).expect("fewer than 4 billion rows");
        // New index of each kept row.
        let mut index = vec![u32::MAX; len];
        let mut rows = Vec::new();
        for (row, slot) in index.iter_mut().enumerate() {
            if keep(row) {
                *slot = row_number(rows.len());
                rows.push(row_number(row));
            }
        }

        // Bottom-up, so every parent's targets are known before its children ask: a kept row is its
        // own target; a hidden row's targets are its parents' targets.
        let mut targets: Vec<Vec<u32>> = vec![Vec::new(); len];
        let mut kept_parents: Vec<Vec<u32>> = vec![Vec::new(); rows.len()];
        for row in (0..len).rev() {
            let mut found = Vec::new();
            for &parent in parents(row) {
                let parent = parent as usize;
                assert!(parent > row, "fergit-graph: row {row} lists parent {parent}, which isn't below it");
                if parent >= len {
                    continue;
                }
                if index[parent] != u32::MAX {
                    push_new(&mut found, index[parent]);
                } else {
                    for &target in &targets[parent] {
                        push_new(&mut found, target);
                    }
                }
            }
            match index[row] {
                u32::MAX => targets[row] = found,
                kept => kept_parents[kept as usize] = found,
            }
        }

        let mut parent_ends = Vec::with_capacity(rows.len());
        let mut flat = Vec::new();
        for list in kept_parents {
            flat.extend(list);
            parent_ends.push(u32::try_from(flat.len()).expect("fewer than 4 billion parent links"));
        }
        Subgraph { rows, parent_ends, parents: flat }
    }

    /// Number of kept rows.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The kept rows, in order, as row numbers of the original history.
    pub fn rows(&self) -> &[u32] {
        &self.rows
    }

    /// The parents of kept row `i` (an index into [`Subgraph::rows`]), as indices into
    /// [`Subgraph::rows`]: first the targets of the first original parent, and so on, each once.
    pub fn parents(&self, i: usize) -> &[u32] {
        let start = if i == 0 { 0 } else { self.parent_ends[i - 1] as usize };
        &self.parents[start..self.parent_ends[i] as usize]
    }
}

fn push_new(list: &mut Vec<u32>, value: u32) {
    if !list.contains(&value) {
        list.push(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subgraph(parents: &[&[u32]], keep: &[bool]) -> Vec<(u32, Vec<u32>)> {
        let graph = Subgraph::new(parents.len(), |row| parents[row], |row| keep[row]);
        (0..graph.len()).map(|i| (graph.rows()[i], graph.parents(i).to_vec())).collect()
    }

    #[test]
    fn keeping_everything_changes_nothing() {
        let parents: &[&[u32]] = &[&[1, 2], &[3], &[3], &[]];
        assert_eq!(
            subgraph(parents, &[true; 4]),
            [(0, vec![1, 2]), (1, vec![3]), (2, vec![3]), (3, vec![])]
        );
    }

    #[test]
    fn hidden_rows_pass_their_line_through() {
        // 0 → 1 → 2 → 3, with 1 and 2 hidden.
        let parents: &[&[u32]] = &[&[1], &[2], &[3], &[]];
        assert_eq!(subgraph(parents, &[true, false, false, true]), [(0, vec![1]), (3, vec![])]);
    }

    #[test]
    fn a_hidden_merge_leads_to_both_sides_once() {
        // 0 → 1 (hidden merge of 2 and 3) → both → 4. 2 and 3 are hidden too.
        let parents: &[&[u32]] = &[&[1], &[2, 3], &[4], &[4], &[]];
        assert_eq!(subgraph(parents, &[true, false, false, false, true]), [(0, vec![1]), (4, vec![])]);
        // With 3 kept: the merge leads to 4 (through 2) and to 3.
        assert_eq!(
            subgraph(parents, &[true, false, false, true, true]),
            [(0, vec![2, 1]), (3, vec![2]), (4, vec![])]
        );
    }

    #[test]
    fn parents_past_the_end_are_ignored() {
        let parents: &[&[u32]] = &[&[1, 9], &[7]];
        assert_eq!(subgraph(parents, &[true, false]), [(0, vec![])]);
    }
}
