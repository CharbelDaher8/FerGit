//! Behavioral tests for `fergit_graph::Layout`, compiled as one test binary.
//!
//! - `snapshots`: readable ASCII drawings of typical and tricky histories (`insta`).
//! - `properties`: invariants over random DAGs (`proptest`).
//! - `edge_cases`: exact rows for cases the drawings don't pin down.
//! - `perf`: an ignored smoke test over a million commits.

mod edge_cases;
mod perf;
mod properties;
mod render;
mod snapshots;
