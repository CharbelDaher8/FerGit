//! The registry of open sessions, one per tab.

mod common;

use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use common::{Fixture, T0, commit};
use fergit_core::session::Sessions;
use fergit_core::{RepoInfo, SessionId};

const MINUTE: i64 = 60;

fn ignore(_: SessionId, _: RepoInfo) {}

#[test]
fn each_repository_gets_its_own_session() {
    let fx = Fixture::new();
    let a = fx.init("a");
    commit(&a, "a.txt", "1\n", "only in a", T0);
    let b = fx.init("b");
    commit(&b, "b.txt", "1\n", "first in b", T0);
    commit(&b, "b.txt", "2\n", "second in b", T0 + MINUTE);
    let sessions = Sessions::default();

    let (a_id, _) = sessions.open(&a, ignore).unwrap();
    let (b_id, _) = sessions.open(&b, ignore).unwrap();

    assert_ne!(a_id, b_id);
    assert_eq!(sessions.ids(), vec![a_id, b_id]);
    assert_eq!(sessions.get(a_id).unwrap().info().row_count, 1);
    assert_eq!(sessions.get(b_id).unwrap().info().row_count, 2);
}

#[test]
fn opening_an_open_repository_again_finds_its_session() {
    let fx = Fixture::new();
    let repo = fx.init("repo");
    commit(&repo, "dir/a.txt", "1\n", "first", T0);
    let sessions = Sessions::default();
    let (id, first) = sessions.open(&repo, ignore).unwrap();

    let (again, session) = sessions.open(&repo.join("dir"), ignore).unwrap();

    assert_eq!(again, id, "a path inside the repository names the same repository");
    assert!(Arc::ptr_eq(&session, &first), "the history isn't read twice");
    assert_eq!(sessions.ids(), vec![id]);
}

#[test]
fn closing_a_session_forgets_it_and_its_id() {
    let fx = Fixture::new();
    let repo = fx.init("repo");
    commit(&repo, "a.txt", "1\n", "first", T0);
    let sessions = Sessions::default();
    let (id, _) = sessions.open(&repo, ignore).unwrap();

    assert!(sessions.close(id));

    assert!(sessions.get(id).is_none());
    assert!(sessions.ids().is_empty());
    assert!(!sessions.close(id), "closing a closed session does nothing");
    let (reopened, _) = sessions.open(&repo, ignore).unwrap();
    assert_ne!(reopened, id, "ids are never reused");
}

#[test]
fn a_failed_open_leaves_the_open_sessions_alone() {
    let fx = Fixture::new();
    let repo = fx.init("repo");
    commit(&repo, "a.txt", "1\n", "first", T0);
    let not_a_repo = fx.path("plain");
    std::fs::create_dir(&not_a_repo).unwrap();
    let sessions = Sessions::default();
    let (id, _) = sessions.open(&repo, ignore).unwrap();

    assert!(sessions.open(&not_a_repo, ignore).is_err());

    assert_eq!(sessions.ids(), vec![id]);
}

/// Waits for a change report whose info has `rows` rows, skipping earlier reports of the same burst.
fn wait_for_rows(changes: &mpsc::Receiver<(SessionId, RepoInfo)>, rows: u32) -> SessionId {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let (id, info) = changes.recv_timeout(remaining).expect("the change is reported within 20 seconds");
        if info.row_count == rows {
            return id;
        }
    }
}

#[test]
fn changes_are_reported_with_the_id_of_the_session_that_changed() {
    let fx = Fixture::new();
    let a = fx.init("a");
    commit(&a, "a.txt", "1\n", "first in a", T0);
    let b = fx.init("b");
    commit(&b, "b.txt", "1\n", "first in b", T0);
    let sessions = Sessions::default();
    let (sender, changes) = mpsc::channel();
    let to_a = sender.clone();
    sessions.open(&a, move |id, info| drop(to_a.send((id, info)))).unwrap();
    let (b_id, _) = sessions.open(&b, move |id, info| drop(sender.send((id, info)))).unwrap();

    commit(&b, "b.txt", "2\n", "second in b", T0 + MINUTE);

    assert_eq!(wait_for_rows(&changes, 2), b_id);
}

#[test]
fn a_closed_session_stops_reporting_changes() {
    let fx = Fixture::new();
    let repo = fx.init("repo");
    commit(&repo, "a.txt", "1\n", "first", T0);
    let sessions = Sessions::default();
    let (sender, changes) = mpsc::channel();
    let (id, _) = sessions.open(&repo, move |id, info| drop(sender.send((id, info)))).unwrap();

    sessions.close(id);
    commit(&repo, "a.txt", "2\n", "second", T0 + MINUTE);

    // Well past the watcher's quiet period. Once the watcher's thread ends the channel disconnects,
    // which also counts: either way nothing is delivered.
    let deadline = Duration::from_secs(3);
    assert!(changes.recv_timeout(deadline).is_err(), "no change is reported for a closed session");
}
