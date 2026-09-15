//! Snapshot drawings. Each input is one commit per line, `id parent parent...`, children first.
//! A snapshot holds the input followed by its drawing; every one was checked by hand. See
//! `render.rs` for how to read the drawings.

use crate::render::draw;

fn check(name: &str, spec: &str) {
    let spec: Vec<&str> = spec.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let spec = spec.join("\n");
    let snapshot = format!("{spec}\n\n{}", draw(&spec));
    insta::assert_snapshot!(name, snapshot);
}

#[test]
fn linear() {
    check(
        "linear",
        "
        d c
        c b
        b a
        a
        ",
    );
}

#[test]
fn feature_branch_merged() {
    check(
        "feature_branch_merged",
        "
        e d
        d c f2
        f2 f1
        c b
        f1 b
        b a
        a
        ",
    );
}

#[test]
fn fork_from_same_commit() {
    check(
        "fork_from_same_commit",
        "
        x2 x1
        y2 y1
        x1 base
        y1 base
        base root
        root
        ",
    );
}

#[test]
fn octopus_merge() {
    check(
        "octopus_merge",
        "
        top o
        o a b c
        a base
        b base
        c base
        base
        ",
    );
}

#[test]
fn criss_cross_merge() {
    check(
        "criss_cross_merge",
        "
        top a2 b2
        a2 a1 b1
        b2 b1 a1
        a1 base
        b1 base
        base
        ",
    );
}

/// Two unrelated histories merged at the top, then a third one below both roots.
#[test]
fn disjoint_histories() {
    check(
        "disjoint_histories",
        "
        top m2 o2
        m2 m1
        o2 o1
        m1
        o1
        x2 x1
        x1
        ",
    );
}

/// `x` and `z` are outside a shallow clone: their lanes run off the bottom.
#[test]
fn parent_never_pushed() {
    check(
        "parent_never_pushed",
        "
        m b x
        b a
        a z
        ",
    );
}

#[test]
fn duplicate_parents() {
    check(
        "duplicate_parents",
        "
        d c c
        c a a b a
        a base
        b base
        base
        ",
    );
}

/// The feature lane ends at `m1`, and `m1`'s new merge lane takes the slot in the same row.
#[test]
fn lane_reuse_same_row() {
    check(
        "lane_reuse_same_row",
        "
        m3 m2 f2
        f2 f1
        m2 m1
        f1 m1
        m1 m0 g1
        g1 m0
        m0
        ",
    );
}

/// Lane 1 ends at `m1` while lane 2 keeps running; the later tip `z1` fills the gap at lane 1
/// instead of lane 2 shifting left.
#[test]
fn lane_reuse_gap() {
    check(
        "lane_reuse_gap",
        "
        m2 m1 x1
        y3 y2
        x1 m1
        m1 m0
        z1 m0
        y2 m0
        m0
        ",
    );
}
