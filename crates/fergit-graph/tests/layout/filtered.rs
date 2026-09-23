//! Filtered histories: rows hidden by `Subgraph`, the rest laid out as a graph of their own.
//!
//! Uses the random DAGs of `properties` (node `i` has id `i`; parents `>= n` are never pushed).

use std::collections::BTreeSet;

use fergit_graph::Subgraph;
use proptest::prelude::*;

use crate::properties::{check_invariants, dag, layout};
use crate::render::draw;

/// The filtered DAG in the form `properties` uses: kept row `i` has id `i`, parents by kept index.
fn filtered(dag: &[Vec<u32>], keep: &[bool]) -> (Subgraph, Vec<Vec<u32>>) {
    let graph = Subgraph::new(dag.len(), |row| &dag[row], |row| keep[row]);
    let parents = (0..graph.len()).map(|i| graph.parents(i).to_vec()).collect();
    (graph, parents)
}

/// Kept rows reachable from `row` through hidden rows only: what its parents should become.
fn nearest_kept(dag: &[Vec<u32>], keep: &[bool], row: usize) -> BTreeSet<usize> {
    let mut found = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let mut stack: Vec<usize> = dag[row].iter().map(|&p| p as usize).collect();
    while let Some(r) = stack.pop() {
        if r >= dag.len() || !seen.insert(r) {
            continue;
        }
        if keep[r] {
            found.insert(r);
        } else {
            stack.extend(dag[r].iter().map(|&p| p as usize));
        }
    }
    found
}

fn dag_and_keep() -> impl Strategy<Value = (Vec<Vec<u32>>, Vec<bool>)> {
    dag().prop_flat_map(|dag| {
        let n = dag.len();
        (Just(dag), prop::collection::vec(prop::bool::weighted(0.6), n))
    })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1024, ..ProptestConfig::default() })]

    /// Each kept row's parents are exactly the kept rows its lines reach through hidden rows.
    #[test]
    fn parents_are_the_nearest_kept_ancestors((dag, keep) in dag_and_keep()) {
        let (graph, parents) = filtered(&dag, &keep);
        let kept: Vec<usize> = (0..dag.len()).filter(|&r| keep[r]).collect();
        prop_assert_eq!(graph.rows().iter().map(|&r| r as usize).collect::<Vec<_>>(), kept.clone());
        for (i, &row) in kept.iter().enumerate() {
            let got: BTreeSet<usize> = parents[i].iter().map(|&p| kept[p as usize]).collect();
            prop_assert_eq!(got.len(), parents[i].len(), "row {}: a parent is listed twice", row);
            prop_assert_eq!(got, nearest_kept(&dag, &keep, row), "row {}", row);
            prop_assert!(parents[i].iter().all(|&p| p as usize > i), "row {}: parent above its child", row);
        }
    }

    /// The filtered history is a valid history of its own, so its layout keeps every invariant.
    #[test]
    fn filtered_layout_satisfies_invariants((dag, keep) in dag_and_keep()) {
        let (_, parents) = filtered(&dag, &keep);
        check_invariants(&parents, &layout(&parents))?;
    }
}

#[test]
fn keeping_every_row_lays_out_the_same_graph() {
    let dag: Vec<Vec<u32>> = vec![vec![1, 3], vec![2], vec![4], vec![4], vec![]];
    let (_, parents) = filtered(&dag, &[true; 5]);
    assert_eq!(parents, dag);
}

#[test]
fn a_merged_branch_hidden_entirely_leaves_a_straight_line() {
    // The feature branch (f2, f1) and the main line's c are hidden: d reaches b both ways, once.
    let spec = ["e d", "d c f2", "f2 f1", "c b", "f1 b", "b a", "a"];
    let names: Vec<&str> = spec.iter().map(|line| line.split(' ').next().unwrap()).collect();
    let dag: Vec<Vec<u32>> = spec
        .iter()
        .map(|line| line.split(' ').skip(1).map(|p| names.iter().position(|n| n == &p).unwrap() as u32).collect())
        .collect();
    let keep: Vec<bool> = names.iter().map(|name| !["c", "f2", "f1"].contains(name)).collect();

    let (graph, parents) = filtered(&dag, &keep);
    let kept_spec: Vec<String> = (0..graph.len())
        .map(|i| {
            let name = names[graph.rows()[i] as usize];
            let parent_names = parents[i].iter().map(|&p| names[graph.rows()[p as usize] as usize]);
            std::iter::once(name).chain(parent_names).collect::<Vec<_>>().join(" ")
        })
        .collect();
    assert_eq!(kept_spec, ["e d", "d b", "b a", "a"]);
    assert_eq!(draw(&kept_spec.join("\n")), draw("e d\nd b\nb a\na"));
    assert!(!draw(&kept_spec.join("\n")).contains('\\'), "no lane branches off");
}
