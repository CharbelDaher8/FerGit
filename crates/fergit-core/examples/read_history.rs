//! Times `Repo::read_history` on a repository.
//!
//! ```text
//! cargo run --release -p fergit-core --example read_history -- <repository path> [runs]
//! ```
//!
//! The first run includes cold caches (packs, commit-graph, index); later runs show steady state.

use std::path::PathBuf;
use std::time::Instant;

use fergit_core::repo::Repo;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let path = PathBuf::from(args.next().ok_or("usage: read_history <repository path> [runs]")?);
    let runs: u32 = match args.next() {
        Some(runs) => runs.to_str().ok_or("runs must be a number")?.parse()?,
        None => 3,
    };

    let start = Instant::now();
    let repo = Repo::open(&path)?;
    println!("open: {:.1} ms ({})", start.elapsed().as_secs_f64() * 1e3, repo.root().display());

    for run in 1..=runs {
        let start = Instant::now();
        let history = repo.read_history()?;
        let elapsed = start.elapsed();
        println!(
            "run {run}: read_history {:.1} ms, {} commits, {} refs, {} stashes, worktree dirty: {}",
            elapsed.as_secs_f64() * 1e3,
            history.commits.len(),
            history.refs.len(),
            history.stashes.len(),
            history.worktree_dirty,
        );
    }
    Ok(())
}
