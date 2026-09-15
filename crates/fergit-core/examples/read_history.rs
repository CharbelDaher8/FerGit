//! Times `Repo::read_history` and `Repo::read_tips` on a repository.
//!
//! ```text
//! cargo run --release -p fergit-core --example read_history -- <repository path> [runs]
//! ```
//!
//! `read_history` is what opening a repository (or a refresh after a change) costs; `read_tips` is
//! what a refresh costs when nothing changed. The first run includes cold caches (packs,
//! commit-graph, index); later runs show steady state.

use std::path::PathBuf;
use std::time::{Duration, Instant};

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
    println!("open: {:.1} ms ({})", millis(start.elapsed()), repo.root().display());

    for run in 1..=runs {
        let start = Instant::now();
        let history = repo.read_history()?;
        let history_elapsed = start.elapsed();

        let start = Instant::now();
        let tips = repo.read_tips()?;
        let tips_elapsed = start.elapsed();

        println!(
            "run {run}: read_history {:.1} ms, read_tips {:.1} ms, {} commits, {} refs, {} stashes, worktree dirty: {}",
            millis(history_elapsed),
            millis(tips_elapsed),
            history.commits.len(),
            tips.refs.len(),
            tips.stashes.len(),
            tips.worktree_dirty,
        );
    }
    Ok(())
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1e3
}
