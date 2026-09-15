//! Times building a session snapshot on a repository.
//!
//! ```text
//! cargo run --release -p fergit-core --example snapshot -- <repository path> [runs]
//! ```
//!
//! `Session::open` reads the history and builds the first snapshot (graph layout, labels, and
//! everything else derived from the history); `Repo::read_history` alone is the read. Their
//! difference is what building the snapshot costs. Runs after the first see warm caches.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use fergit_core::repo::Repo;
use fergit_core::session::Session;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let path = PathBuf::from(args.next().ok_or("usage: snapshot <repository path> [runs]")?);
    let runs: u32 = match args.next() {
        Some(runs) => runs.to_str().ok_or("runs must be a number")?.parse()?,
        None => 3,
    };

    let repo = Repo::open(&path)?;
    for run in 1..=runs {
        let start = Instant::now();
        let history = repo.read_history()?;
        let read = start.elapsed();

        let start = Instant::now();
        let session = Session::open(&path)?;
        let open = start.elapsed();

        println!(
            "run {run}: read_history {:.1} ms, Session::open {:.1} ms, snapshot build ≈ {:.1} ms, {} rows, {} commits",
            millis(read),
            millis(open),
            millis(open.saturating_sub(read)),
            session.info().row_count,
            history.commits.len(),
        );
    }
    Ok(())
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1e3
}
