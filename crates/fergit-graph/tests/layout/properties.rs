//! Invariants over random DAGs.
//!
//! Node `i` has id `i` and is pushed `i`-th; its parents have larger ids, so every node comes
//! before its parents. Parent ids `>= n` are never pushed, which models a shallow clone.

use std::collections::BTreeSet;

use fergit_graph::{Edge, GraphRow, Half, Layout};
use proptest::prelude::*;
use proptest::test_runner::TestCaseError;

/// Parent lists for nodes `0..n`, children first. Mostly linear, with merges, octopus merges,
/// roots, duplicate parents and parents that are never pushed.
pub(crate) fn dag() -> impl Strategy<Value = Vec<Vec<u32>>> {
    #[derive(Debug, Clone, Copy)]
    enum Parent {
        /// `offset` rows below the child (never pushed if that runs past the end).
        Below(usize),
        /// One of a few ids that are never pushed, so several children can share one.
        Missing(usize),
    }
    let parent = prop_oneof![
        6 => (1usize..4).prop_map(Parent::Below),
        3 => (1usize..40).prop_map(Parent::Below),
        1 => (0usize..3).prop_map(Parent::Missing),
    ];
    let count = prop_oneof![2 => Just(0usize), 12 => Just(1), 5 => Just(2), 1 => 3usize..=5];
    let node = (count, prop::collection::vec(parent, 5)).prop_map(|(count, mut parents)| {
        parents.truncate(count);
        parents
    });
    prop::collection::vec(node, 1..80).prop_map(|nodes| {
        let n = nodes.len();
        let id = |i: usize| u32::try_from(i).unwrap();
        (0..n)
            .map(|i| {
                nodes[i]
                    .iter()
                    .map(|parent| match *parent {
                        Parent::Below(offset) => id(i + offset),
                        Parent::Missing(k) => id(n + k),
                    })
                    .collect()
            })
            .collect()
    })
}

fn layout_with<Id: Copy + Eq>(dag: &[Vec<u32>], label: impl Fn(u32) -> Id) -> Vec<GraphRow> {
    let mut layout = Layout::new();
    let mut ids = 0u32..;
    dag.iter()
        .map(|parents| {
            let parents: Vec<Id> = parents.iter().map(|&p| label(p)).collect();
            layout.push(label(ids.next().unwrap()), &parents)
        })
        .collect()
}

pub(crate) fn layout(dag: &[Vec<u32>]) -> Vec<GraphRow> {
    layout_with(dag, |id| id)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1024, ..ProptestConfig::default() })]

    #[test]
    fn rows_satisfy_invariants(dag in dag()) {
        check_invariants(&dag, &layout(&dag))?;
    }

    #[test]
    fn same_input_same_output(dag in dag()) {
        prop_assert_eq!(layout(&dag), layout(&dag));
    }

    /// Rows depend only on the shape of the history, not on id values or the id type.
    #[test]
    fn independent_of_id_values(dag in dag()) {
        let plain = layout(&dag);
        let scrambled = layout_with(&dag, |id| id.wrapping_mul(0x9E37_79B1) ^ 0x5BD1_E995);
        let oid_like = layout_with(&dag, |id| {
            let mut oid = [0xAB_u8; 20];
            oid[3..7].copy_from_slice(&id.to_le_bytes());
            oid
        });
        prop_assert_eq!(&plain, &scrambled);
        prop_assert_eq!(&plain, &oid_like);
    }

    /// Rows never depend on later pushes: laying out a prefix gives a prefix of the rows.
    #[test]
    fn prefix_gives_prefix(dag in dag(), cut in any::<prop::sample::Index>()) {
        let full = layout(&dag);
        let cut = cut.index(dag.len() + 1);
        let prefix = layout(&dag[..cut]);
        prop_assert_eq!(&prefix[..], &full[..cut]);
    }
}

pub(crate) fn check_invariants(dag: &[Vec<u32>], rows: &[GraphRow]) -> Result<(), TestCaseError> {
    prop_assert_eq!(rows.len(), dag.len());
    let row_of = |id: u32| Some(id as usize).filter(|&i| i < dag.len());

    for (r, row) in rows.iter().enumerate() {
        let column = row.column;
        let upper: Vec<Edge> = row.edges.iter().copied().filter(|e| e.half == Half::Upper).collect();
        let lower: Vec<Edge> = row.edges.iter().copied().filter(|e| e.half == Half::Lower).collect();
        let is_pass_through = |e: &Edge| e.from == e.to && e.from != column;

        let key = |e: &Edge| (e.half == Half::Lower, e.from, e.to);
        prop_assert!(
            row.edges.windows(2).all(|w| key(&w[0]) < key(&w[1])),
            "row {}: edges not sorted or duplicated: {:?}", r, row
        );

        for e in &upper {
            prop_assert!(e.to == column || e.from == e.to, "row {}: stray upper edge {:?}", r, e);
        }
        for e in &lower {
            prop_assert!(e.from == column || e.from == e.to, "row {}: stray lower edge {:?}", r, e);
        }

        let upper_from: BTreeSet<u16> = upper.iter().map(|e| e.from).collect();
        prop_assert_eq!(upper_from.len(), upper.len(), "row {}: duplicate upper lanes", r);

        // Continuity: lanes leaving the bottom of one row enter the top of the next.
        let entering = match r {
            0 => BTreeSet::new(),
            _ => rows[r - 1].edges.iter().filter(|e| e.half == Half::Lower).map(|e| e.to).collect(),
        };
        prop_assert_eq!(&entering, &upper_from, "row {}: lanes don't continue from the row above", r);

        let pass_up: BTreeSet<u16> = upper.iter().filter(|e| is_pass_through(e)).map(|e| e.from).collect();
        let pass_down: BTreeSet<u16> = lower.iter().filter(|e| is_pass_through(e)).map(|e| e.from).collect();
        prop_assert_eq!(pass_up, pass_down, "row {}: pass-through halves don't pair up", r);

        // Each active lane has its own color.
        let colors: BTreeSet<u16> = upper.iter().map(|e| e.color).collect();
        prop_assert_eq!(colors.len(), upper.len(), "row {}: two lanes share a color", r);

        let converging: Vec<Edge> = upper.iter().copied().filter(|e| e.to == column).collect();
        if converging.is_empty() {
            // A new lane takes the leftmost free slot.
            for lane in 0..column {
                prop_assert!(upper_from.contains(&lane), "row {}: node skipped free lane {}", r, lane);
            }
            prop_assert!(!colors.contains(&row.color), "row {}: new lane reuses an active color", r);
        } else {
            // The node sits in the leftmost converging lane and takes its color.
            prop_assert_eq!(converging.iter().map(|e| e.from).min(), Some(column), "row {}", r);
            let own = converging.iter().find(|e| e.from == column).map(|e| e.color);
            prop_assert_eq!(own, Some(row.color), "row {}: node color differs from its lane", r);
        }

        // Every parent is reached by exactly one segment leaving the node.
        let node_lowers: Vec<Edge> = lower.iter().copied().filter(|e| e.from == column).collect();
        let mut reached = node_lowers.iter().map(|e| trace(rows, r, e)).collect::<Result<Vec<_>, _>>()?;
        let mut parents: Vec<u32> = Vec::new();
        for &p in &dag[r] {
            if !parents.contains(&p) {
                parents.push(p);
            }
        }
        let mut expected: Vec<Option<usize>> = parents.iter().map(|&p| row_of(p)).collect();
        reached.sort_unstable();
        expected.sort_unstable();
        prop_assert_eq!(reached, expected, "row {}: segments don't lead to the parents", r);

        // The first parent continues straight down in the node's color.
        match dag[r].first() {
            Some(&first) => {
                let own = node_lowers.iter().find(|e| e.to == column);
                prop_assert!(own.is_some_and(|e| e.color == row.color), "row {}: node lane doesn't continue", r);
                prop_assert_eq!(trace(rows, r, own.unwrap())?, row_of(first), "row {}: first parent lane", r);
            }
            None => prop_assert!(node_lowers.is_empty(), "row {}: root has lower segments", r),
        }
    }
    Ok(())
}

/// Follows the lane leaving row `r` along lower segment `start` down through later rows. Returns
/// the row whose node it ends in, or `None` if it runs off the bottom. Fails if the lane breaks or
/// changes color on the way.
fn trace(rows: &[GraphRow], r: usize, start: &Edge) -> Result<Option<usize>, TestCaseError> {
    let lane = start.to;
    for (t, row) in rows.iter().enumerate().skip(r + 1) {
        let upper = row.edges.iter().find(|e| e.half == Half::Upper && e.from == lane);
        let Some(upper) = upper else {
            return Err(TestCaseError::fail(format!("lane {lane} from row {r} breaks at row {t}")));
        };
        prop_assert_eq!(upper.color, start.color, "lane {} from row {} changes color at row {}", lane, r, t);
        if upper.to == row.column {
            return Ok(Some(t));
        }
        let continues = row.edges.iter().any(|e| e.half == Half::Lower && e.from == lane && e.to == lane);
        prop_assert!(continues, "lane {} from row {} stops at row {}", lane, r, t);
    }
    Ok(None)
}
