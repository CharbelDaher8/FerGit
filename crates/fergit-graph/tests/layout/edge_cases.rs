//! Exact rows for cases the snapshot drawings don't pin down.

use fergit_graph::{Edge, GraphRow, Half, Layout};

fn up(from: u16, to: u16, color: u16) -> Edge {
    Edge { half: Half::Upper, from, to, color }
}

fn down(from: u16, to: u16, color: u16) -> Edge {
    Edge { half: Half::Lower, from, to, color }
}

fn row(column: u16, color: u16, edges: Vec<Edge>) -> GraphRow {
    GraphRow { column, color, edges }
}

#[test]
fn single_root_has_no_edges() {
    let mut layout = Layout::new();
    assert_eq!(layout.push("a", &[]), row(0, 0, vec![]));
}

#[test]
fn default_matches_new() {
    let mut a = Layout::default();
    let mut b = Layout::new();
    for (id, parents) in [(1, &[2, 3][..]), (3, &[4][..]), (2, &[4][..]), (4, &[][..])] {
        assert_eq!(a.push(id, parents), b.push(id, parents));
    }
}

/// When another lane already waits for the first parent, the node still keeps its own lane; the
/// two meet at the parent (a branch-off drawn there).
#[test]
fn first_parent_keeps_own_lane() {
    let mut layout = Layout::new();
    assert_eq!(layout.push(1, &[3]), row(0, 0, vec![down(0, 0, 0)]));
    assert_eq!(layout.push(2, &[3]), row(1, 1, vec![up(0, 0, 0), down(0, 0, 0), down(1, 1, 1)]));
    assert_eq!(layout.push(3, &[]), row(0, 0, vec![up(0, 0, 0), up(1, 0, 1)]));
}

/// A second parent that a lane already waits for joins that lane in its color, without starting a
/// lane or using up a color.
#[test]
fn merge_joins_waiting_lane() {
    let mut layout = Layout::new();
    layout.push(1, &[2]);
    assert_eq!(
        layout.push(9, &[5, 2]),
        row(1, 1, vec![up(0, 0, 0), down(0, 0, 0), down(1, 0, 0), down(1, 1, 1)])
    );
    assert_eq!(layout.push(7, &[8]).color, 2);
}

/// When several lanes converge, the leftmost holds the node and gives it its color, even if a
/// lane further right is older.
#[test]
fn node_color_comes_from_leftmost_converging_lane() {
    let mut layout = Layout::new();
    layout.push(1, &[4]); // lane 0, c0
    layout.push(2, &[5]); // lane 1, c1
    layout.push(4, &[]); // lane 0 ends
    assert_eq!(layout.push(3, &[5]), row(0, 2, vec![up(1, 1, 1), down(0, 0, 2), down(1, 1, 1)]));
    assert_eq!(layout.push(5, &[]), row(0, 2, vec![up(0, 0, 2), up(1, 0, 1)]));
}

/// A lane freed in the upper half is taken by a new lane in the lower half of the same row.
#[test]
fn slot_freed_above_is_reused_below() {
    let mut layout = Layout::new();
    layout.push(0, &[5]);
    layout.push(1, &[5]);
    assert_eq!(
        layout.push(5, &[6, 7]),
        row(0, 0, vec![up(0, 0, 0), up(1, 0, 1), down(0, 0, 0), down(0, 1, 2)])
    );
}

/// The node's own lower segments land between pass-throughs to its left and right, in order.
#[test]
fn octopus_segments_sorted_among_pass_throughs() {
    let mut layout = Layout::new();
    layout.push(10, &[20]);
    layout.push(11, &[21]);
    layout.push(12, &[22]);
    assert_eq!(
        layout.push(21, &[23, 22, 20, 24]),
        row(
            1,
            1,
            vec![
                up(0, 0, 0),
                up(1, 1, 1),
                up(2, 2, 2),
                down(0, 0, 0),
                down(1, 0, 0),
                down(1, 1, 1),
                down(1, 2, 2),
                down(1, 3, 3),
                down(2, 2, 2),
            ]
        )
    );
}

/// A duplicate of the first parent later in the list must not add a merge segment into the node's
/// own lane.
#[test]
fn duplicate_of_first_parent_is_ignored() {
    let mut layout = Layout::new();
    assert_eq!(layout.push(1, &[2, 3, 2, 3]), row(0, 0, vec![down(0, 0, 0), down(0, 1, 1)]));
}

/// A root that lanes converge into frees all of them, and the next tip starts back at lane 0.
#[test]
fn converging_root_frees_its_lanes() {
    let mut layout = Layout::new();
    layout.push(1, &[3]);
    layout.push(2, &[3]);
    assert_eq!(layout.push(3, &[]), row(0, 0, vec![up(0, 0, 0), up(1, 0, 1)]));
    assert_eq!(layout.push(4, &[]), row(0, 2, vec![]));
}

/// Malformed input (cycles, self-parents, repeated ids) must not panic; the layout just draws
/// what it was told.
#[test]
fn malformed_input_does_not_panic() {
    let mut layout = Layout::new();
    layout.push(1, &[1]);
    layout.push(1, &[1, 2]);
    layout.push(2, &[1]);
    layout.push(3, &[2, 2, 1]);
    layout.push(2, &[]);
    layout.push(1, &[3]);
}
