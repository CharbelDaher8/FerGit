//! Performance smoke test. Run with:
//! `cargo test -p fergit-graph --release -- --ignored --nocapture`

use std::time::Instant;

use fergit_graph::Layout;

#[test]
#[ignore = "slow in debug builds; run in release with --ignored"]
fn million_commit_history() {
    let history = realistic_history(1_000_000);

    let start = Instant::now();
    let mut layout = Layout::new();
    let (mut max_column, mut edges) = (0, 0);
    for (id, parents) in &history {
        let row = layout.push(*id, parents);
        max_column = max_column.max(row.column);
        edges += row.edges.len();
    }
    let elapsed = start.elapsed();

    let merges = history.iter().filter(|(_, parents)| parents.len() > 1).count();
    eprintln!(
        "laid out {} commits ({merges} merges, {edges} edges, max column {max_column}) in {elapsed:?}",
        history.len()
    );
    assert!(max_column < 32, "lanes should stay narrow, got column {max_column}");
}

/// A deterministic history in display order: a main line with short-lived feature branches that
/// are merged in, fork from main, and sometimes merge main back in, interleaved like a date order.
fn realistic_history(len: usize) -> Vec<(u32, Vec<u32>)> {
    let mut rng = SplitMix64(0x0F3E_6175);
    // Ids already referenced as parents but not yet emitted. Index 0 is always the main line.
    let mut open = vec![0u32];
    let mut next_id = 1u32;
    let mut fresh = || {
        next_id += 1;
        next_id - 1
    };
    let mut history = Vec::with_capacity(len);

    while history.len() < len {
        let pick = if open.len() == 1 || rng.below(2) == 0 { 0 } else { 1 + rng.below(open.len() - 1) };
        let id = open[pick];
        let roll = rng.below(1000);
        let parents = if roll < 30 && open.len() < 8 {
            // Merge commit: a branch opens below it.
            let (first, second) = (fresh(), fresh());
            open[pick] = first;
            open.push(second);
            vec![first, second]
        } else if pick != 0 && roll < 100 {
            // A feature branch reaches its fork point on main.
            open.remove(pick);
            vec![open[0]]
        } else if pick != 0 && roll < 115 {
            // Main merged into a feature branch.
            let first = fresh();
            open[pick] = first;
            vec![first, open[0]]
        } else {
            let first = fresh();
            open[pick] = first;
            vec![first]
        };
        history.push((id, parents));
    }
    history
}

struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}
