//! Sessions over repositories built with the git CLI.

mod common;

use common::{Fixture, T0, commit};
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
