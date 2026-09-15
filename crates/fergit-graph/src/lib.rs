//! Commit graph layout.
//!
//! [`Layout`] turns commits, fed one at a time in display order (children before parents), into
//! [`GraphRow`]s: which lane each commit sits in and which line segments to draw in its row.
//! Pure and deterministic: no I/O, no clock, no dependence on hash iteration order.

use serde::Serialize;

/// Drawable geometry for one row of the graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct GraphRow {
    /// Lane holding this row's node; lane 0 is leftmost.
    pub column: u16,
    /// Color index of the node. Unbounded; the UI maps it onto its palette (e.g. modulo its size).
    pub color: u16,
    /// Line segments drawn within this row.
    pub edges: Vec<Edge>,
}

/// A line segment inside one row.
///
/// A row is split at its node's vertical center. An [`Half::Upper`] segment runs from the row's
/// top edge at lane `from` to the center at lane `to`; a [`Half::Lower`] segment runs from the
/// center at lane `from` to the row's bottom edge at lane `to`. A lane passing straight through a
/// row is one upper and one lower segment with `from == to`. Segments with `from == to ==
/// GraphRow::column` are the node's own lane arriving or leaving, not a pass-through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct Edge {
    pub half: Half,
    pub from: u16,
    pub to: u16,
    pub color: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum Half {
    Upper,
    Lower,
}

/// Incremental graph layout.
///
/// Feed nodes with [`Layout::push`] in display order; each call returns that node's row. Rows never
/// depend on nodes pushed later, so a prefix of the history can be laid out and shown before the
/// rest is known.
///
/// Internally each lane waits for one node id. Lanes never shift sideways: a lane that ends leaves
/// a free slot that the next new lane reuses, so a line keeps its column for its whole length.
/// Each lane gets a fresh color when it starts and hands it to its first parent, so a branch line
/// keeps one color. Colors wrap after 65 536 lanes have been started.
///
/// A push costs O(lanes × parents) with plain `Eq` comparisons, which is effectively
/// O(lanes + parents) since commits rarely have more than two parents.
///
/// Panics if more than 65 535 lanes are active at once, the largest column a `u16` can hold.
#[derive(Debug, Clone)]
pub struct Layout<Id> {
    /// Slot `i` is lane `i`; `None` is a free slot. Trailing free slots are trimmed.
    lanes: Vec<Option<Lane<Id>>>,
    next_color: u16,
}

#[derive(Debug, Clone, Copy)]
struct Lane<Id> {
    /// The node this lane runs down to.
    expects: Id,
    color: u16,
}

impl<Id: Copy + Eq> Layout<Id> {
    pub fn new() -> Self {
        Self { lanes: Vec::new(), next_color: 0 }
    }

    /// Places the next node and returns its row.
    ///
    /// `parents` are in git order (first parent first); duplicates are ignored. Every node must be
    /// pushed before any of its parents. A parent that is never pushed (for example one outside a
    /// shallow clone) keeps its lane running to the bottom of the rows laid out so far.
    pub fn push(&mut self, id: Id, parents: &[Id]) -> GraphRow {
        // Lanes waiting for this node converge into it; the leftmost one holds the node and lends
        // it its color. A node nobody waits for (a branch tip) starts a new lane.
        let converging = self.lanes.iter().position(|lane| lane.is_some_and(|l| l.expects == id));
        let top_width = self.lanes.len();
        let (column, color) = match converging {
            Some(c) => (c, self.lanes[c].map_or(0, |l| l.color)),
            None => (self.free_slot(), self.new_color()),
        };
        let column_u16 = lane_index(column);

        let mut edges = Vec::with_capacity(2 * top_width + parents.len());

        // Upper half. Converging lanes bend into the node and are freed; the rest pass through.
        for j in 0..top_width {
            let Some(lane) = self.lanes[j] else { continue };
            let from = lane_index(j);
            if lane.expects == id {
                edges.push(Edge { half: Half::Upper, from, to: column_u16, color: lane.color });
                self.lanes[j] = None;
            } else {
                edges.push(Edge { half: Half::Upper, from, to: from, color: lane.color });
            }
        }
        let upper_count = edges.len();

        // Lower half, built in sorted order: pass-throughs left of the node, the node's own
        // segments (sorted by target), then pass-throughs right of it. A pass-through never sits
        // in the node's column, which was either converging or free at the top.
        let is_pass_through = |e: &Edge| e.from == e.to && e.from != column_u16;
        for i in 0..upper_count {
            let e = edges[i];
            if is_pass_through(&e) && e.from < column_u16 {
                edges.push(Edge { half: Half::Lower, ..e });
            }
        }

        let node_lowers_start = edges.len();
        for (i, &parent) in parents.iter().enumerate() {
            if parents[..i].contains(&parent) {
                continue;
            }
            let (to, color) = if i == 0 {
                // The first parent continues the node's lane and color, even when another lane
                // already waits for it; the two lanes then meet at the parent.
                self.lanes[column] = Some(Lane { expects: parent, color });
                (column, color)
            } else if let Some(k) =
                self.lanes.iter().position(|lane| lane.is_some_and(|l| l.expects == parent))
            {
                // Merge into the lane already waiting for this parent.
                (k, self.lanes[k].map_or(0, |l| l.color))
            } else {
                // A new lane. Reusing a slot freed in the upper half is fine: the upper segment
                // ends at the center and this one starts there, so they never overlap.
                let k = self.free_slot();
                let color = self.new_color();
                self.lanes[k] = Some(Lane { expects: parent, color });
                (k, color)
            };
            edges.push(Edge { half: Half::Lower, from: column_u16, to: lane_index(to), color });
        }
        edges[node_lowers_start..].sort_unstable_by_key(|e| e.to);

        for i in 0..upper_count {
            let e = edges[i];
            if is_pass_through(&e) && e.from > column_u16 {
                edges.push(Edge { half: Half::Lower, ..e });
            }
        }

        while matches!(self.lanes.last(), Some(None)) {
            self.lanes.pop();
        }

        debug_assert!(edges.is_sorted_by_key(|e| (e.half == Half::Lower, e.from, e.to)));
        GraphRow { column: column_u16, color, edges }
    }

    /// Index of the leftmost free slot, appending one if every slot is taken.
    fn free_slot(&mut self) -> usize {
        if let Some(i) = self.lanes.iter().position(Option::is_none) {
            return i;
        }
        assert!(
            self.lanes.len() <= usize::from(u16::MAX),
            "fergit-graph: more than 65 535 simultaneous lanes"
        );
        self.lanes.push(None);
        self.lanes.len() - 1
    }

    fn new_color(&mut self) -> u16 {
        let color = self.next_color;
        self.next_color = color.wrapping_add(1);
        color
    }
}

impl<Id: Copy + Eq> Default for Layout<Id> {
    fn default() -> Self {
        Self::new()
    }
}

/// Converts a slot index into a column. [`Layout::free_slot`] never lets an index exceed `u16`.
fn lane_index(i: usize) -> u16 {
    u16::try_from(i).expect("fergit-graph: lane index exceeds u16")
}

#[cfg(test)]
mod tests {
    //! White-box tests for state that rows don't expose.

    use super::*;

    #[test]
    fn trailing_free_slots_are_trimmed() {
        let mut layout = Layout::new();
        layout.push(0, &[1, 2, 3]);
        assert_eq!(layout.lanes.len(), 3);
        layout.push(3, &[]); // the rightmost lane ends
        assert_eq!(layout.lanes.len(), 2);
        layout.push(1, &[]); // a middle lane ends: its slot stays, lane 2 does not shift
        assert_eq!(layout.lanes.len(), 2);
        assert!(layout.lanes[0].is_none());
        layout.push(2, &[]);
        assert!(layout.lanes.is_empty());
    }

    #[test]
    fn colors_wrap_around() {
        let mut layout = Layout::new();
        layout.next_color = u16::MAX;
        assert_eq!(layout.push(0, &[9]).color, u16::MAX);
        assert_eq!(layout.push(1, &[9]).color, 0);
    }

    #[test]
    fn widest_column_is_u16_max() {
        let mut layout = Layout::new();
        let full = usize::from(u16::MAX);
        layout.lanes = (0..full as u32).map(|i| Some(Lane { expects: i + 1, color: 0 })).collect();
        let row = layout.push(0, &[u32::MAX]);
        assert_eq!(row.column, u16::MAX);
        assert_eq!(layout.lanes.len(), full + 1);
    }

    #[test]
    #[should_panic(expected = "more than 65 535 simultaneous lanes")]
    fn too_many_lanes_panics() {
        let mut layout = Layout::new();
        let full = usize::from(u16::MAX) + 1;
        layout.lanes = (0..full as u32).map(|i| Some(Lane { expects: i + 1, color: 0 })).collect();
        layout.push(0, &[u32::MAX]);
    }
}
