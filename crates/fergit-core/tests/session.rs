//! Sessions over repositories built with the git CLI.

mod common;

use common::{Fixture, T0, commit, git_at, rev_parse, write};
use fergit_core::Oid;
use fergit_core::session::Session;

const MINUTE: i64 = 60;

#[test]
fn refresh_keeps_the_generation_until_something_changes() {
    let fx = Fixture::new();
    let repo = fx.init("refresh");
    commit(&repo, "a.txt", "1\n", "first", T0);
    let session = Session::open(&repo).unwrap();
    let opened = session.info();

    assert_eq!(session.refresh().unwrap(), opened, "nothing changed");

    commit(&repo, "a.txt", "2\n", "second", T0 + MINUTE);
    let refreshed = session.refresh().unwrap();
    assert!(refreshed.generation > opened.generation);
    assert_eq!(refreshed.row_count, 2);
    assert_eq!(session.refresh().unwrap(), refreshed, "nothing changed since");
}

#[test]
fn generations_keep_increasing_across_sessions() {
    let fx = Fixture::new();
    let repo = fx.init("sessions");
    commit(&repo, "a.txt", "1\n", "first", T0);
    let first = Session::open(&repo).unwrap();
    commit(&repo, "a.txt", "2\n", "second", T0 + MINUTE);
    let refreshed = first.refresh().unwrap().generation;

    let second = Session::open(&repo).unwrap();

    assert!(second.info().generation > refreshed, "a reopened repository never reuses an older generation");
}

#[test]
fn locate_finds_commits_stashes_and_uncommitted_changes() {
    let fx = Fixture::new();
    let repo = fx.init("locate");
    let first = commit(&repo, "a.txt", "1\n", "first", T0);
    let second = commit(&repo, "a.txt", "2\n", "second", T0 + MINUTE);
    write(&repo, "a.txt", "stashed\n");
    git_at(&repo, &["stash", "push", "--quiet"], T0 + 2 * MINUTE);
    let stash = rev_parse(&repo, "stash@{0}");
    write(&repo, "b.txt", "uncommitted\n");

    let session = Session::open(&repo).unwrap();
    let row = |id| session.locate(id).row;

    // Uncommitted changes first, then the stash directly above its base, then the commits.
    assert_eq!(row(Oid::ZERO), Some(0));
    assert_eq!(row(stash), Some(1));
    assert_eq!(row(second), Some(2));
    assert_eq!(row(first), Some(3));
    assert_eq!(row(Oid::from_bytes(&[7; 20]).unwrap()), None, "not in the repository");
    assert_eq!(session.locate(first).generation, session.info().generation);
}
