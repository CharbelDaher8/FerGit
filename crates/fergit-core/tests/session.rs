//! Sessions over repositories built with the git CLI.

mod common;

use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use common::{Fixture, T0, commit, git, git_at, git_succeeds, rev_parse, write};
use fergit_core::session::Session;
use fergit_core::{ConflictFile, Oid, RefKind, Relation, RepoState, Row, Upstream, UpstreamState};

const MINUTE: i64 = 60;

fn all_rows(session: &Session) -> Vec<Row> {
    session.rows(0, session.info().row_count).unwrap().rows
}

/// The upstream shown on the label of local branch `branch`.
fn upstream_of(session: &Session, branch: &str) -> Option<Upstream> {
    all_rows(session)
        .into_iter()
        .flat_map(|row| row.refs)
        .find(|label| label.kind == RefKind::LocalBranch && label.name == branch)
        .unwrap_or_else(|| panic!("no local branch {branch}"))
        .upstream
}

/// Following `name`, which points where git in `repo` says it does.
fn tracking(repo: &Path, name: &str, ahead: u32, behind: u32) -> Option<Upstream> {
    let id = rev_parse(repo, name);
    Some(Upstream { name: name.to_owned(), state: UpstreamState::Tracking { ahead, behind, id } })
}

/// A bare `origin.git` with one commit on `main`, a `work` repository that pushes to it, and a
/// `clone` of it whose `main` follows `origin/main`. Returns `(origin, work, clone)`.
fn cloned(fx: &Fixture) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let origin = fx.init_bare("origin.git");
    let work = fx.init("work");
    commit(&work, "a.txt", "1\n", "base", T0);
    git(&work, &["push", "--quiet", origin.to_str().unwrap(), "main"]);
    git(fx.path("").as_path(), &["clone", "--quiet", origin.to_str().unwrap(), "clone"]);
    (origin, work, fx.path("clone"))
}

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

#[test]
fn watch_reports_a_commit_made_by_another_program() {
    let fx = Fixture::new();
    let repo = fx.init("watch");
    commit(&repo, "a.txt", "1\n", "first", T0);
    let session = Arc::new(Session::open(&repo).unwrap());
    let opened = session.info().generation;
    let (sender, changes) = mpsc::channel();
    let _watcher = session.watch(move |info| drop(sender.send(info))).unwrap();

    let second = commit(&repo, "a.txt", "2\n", "second", T0 + MINUTE);

    // One git command writes several files, so there may be more than one report; wait for the one
    // that shows the finished commit.
    let deadline = Instant::now() + Duration::from_secs(20);
    let info = loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let info = changes.recv_timeout(remaining).expect("the commit is reported within 20 seconds");
        assert!(info.generation > opened, "a commit is a new generation");
        if info.row_count == 2 {
            break info;
        }
    };
    assert_eq!(session.locate(second).row, Some(0));
    assert_eq!(session.info(), info);
}

#[test]
fn a_merge_in_progress_and_its_resolved_files_are_reported() {
    let fx = Fixture::new();
    let repo = fx.init("conflicts");
    commit(&repo, "a.txt", "base\n", "base", T0);
    git(&repo, &["checkout", "--quiet", "-b", "topic"]);
    commit(&repo, "a.txt", "topic\n", "topic", T0 + MINUTE);
    git(&repo, &["checkout", "--quiet", "main"]);
    commit(&repo, "a.txt", "main\n", "main", T0 + 2 * MINUTE);
    assert!(!git_succeeds(&repo, &["merge", "--quiet", "topic"]), "the merge conflicts");
    let session = Arc::new(Session::open(&repo).unwrap());
    let opened = session.info();
    let conflict = |resolved| vec![ConflictFile { path: "a.txt".to_owned(), resolved }];
    assert!(matches!(&opened.state, RepoState::Merging { conflicts, .. } if *conflicts == conflict(false)));
    assert_eq!(opened.branch.as_deref(), Some("main"));
    let (sender, changes) = mpsc::channel();
    let _watcher = session.watch(move |info| drop(sender.send(info))).unwrap();

    // Resolving the file changes no ref, but the state is reported all the same.
    write(&repo, "a.txt", "both\n");
    let deadline = Instant::now() + Duration::from_secs(20);
    let info = loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let info = changes.recv_timeout(remaining).expect("the resolution is reported within 20 seconds");
        if info.state.conflicts() == conflict(true) {
            break info;
        }
    };
    assert_eq!(info.generation, opened.generation, "the graph didn't change");

    git(&repo, &["merge", "--abort"]);
    assert_eq!(session.refresh().unwrap().state, RepoState::Clean);
}

#[test]
fn upstream_state_matches_git() {
    let fx = Fixture::new();
    let (origin, work, clone) = cloned(&fx);
    // Two commits on the branch that the upstream lacks, and one on the upstream the branch lacks.
    commit(&clone, "b.txt", "1\n", "local 1", T0 + MINUTE);
    commit(&clone, "b.txt", "2\n", "local 2", T0 + 2 * MINUTE);
    commit(&work, "c.txt", "1\n", "remote 1", T0 + 3 * MINUTE);
    git(&work, &["push", "--quiet", origin.to_str().unwrap(), "main"]);
    git(&clone, &["fetch", "--quiet"]);
    // A branch following a local branch, one whose upstream is gone, and one without an upstream.
    git(&clone, &["branch", "--quiet", "--track", "follower", "main"]);
    git(&clone, &["branch", "--quiet", "stale"]);
    git(&clone, &["config", "branch.stale.remote", "origin"]);
    git(&clone, &["config", "branch.stale.merge", "refs/heads/deleted"]);
    git(&clone, &["branch", "--quiet", "loner"]);

    let counts = git(&clone, &["rev-list", "--left-right", "--count", "main...origin/main"]);
    let (ahead, behind) = counts.trim().split_once('\t').expect("git prints two counts");
    assert_eq!((ahead, behind), ("2", "1"), "the fixture diverged as intended");

    let session = Session::open(&clone).unwrap();

    assert_eq!(upstream_of(&session, "main"), tracking(&clone, "origin/main", 2, 1));
    assert_eq!(upstream_of(&session, "follower"), tracking(&clone, "main", 0, 0));
    assert_eq!(
        upstream_of(&session, "stale"),
        Some(Upstream { name: "origin/deleted".to_owned(), state: UpstreamState::Gone })
    );
    assert_eq!(upstream_of(&session, "loner"), None);
}

#[test]
fn refresh_notices_an_upstream_being_set() {
    let fx = Fixture::new();
    let (_origin, _work, clone) = cloned(&fx);
    git(&clone, &["branch", "--quiet", "topic"]);
    let session = Session::open(&clone).unwrap();
    let opened = session.info().generation;
    assert_eq!(upstream_of(&session, "topic"), None);

    // Only `.git/config` changes; no ref moves.
    git(&clone, &["branch", "--quiet", "--set-upstream-to", "origin/main", "topic"]);

    assert!(session.refresh().unwrap().generation > opened, "an upstream change is visible state");
    assert_eq!(upstream_of(&session, "topic"), tracking(&clone, "origin/main", 0, 0));
}

#[test]
fn relations_name_a_merged_and_deleted_branch() {
    let fx = Fixture::new();
    let repo = fx.init("relations");
    commit(&repo, "a.txt", "1\n", "base", T0);
    git(&repo, &["checkout", "--quiet", "-b", "feature"]);
    commit(&repo, "f.txt", "1\n", "feature work", T0 + MINUTE);
    git(&repo, &["checkout", "--quiet", "main"]);
    commit(&repo, "a.txt", "2\n", "main work", T0 + 2 * MINUTE);
    git_at(&repo, &["merge", "--quiet", "--no-ff", "--no-edit", "feature"], T0 + 3 * MINUTE);
    git(&repo, &["branch", "--quiet", "-D", "feature"]);

    let rows = all_rows(&Session::open(&repo).unwrap());
    let row = |summary: &str| {
        rows.iter()
            .find(|row| row.summary == summary)
            .unwrap_or_else(|| panic!("no row {summary:?}"))
    };

    // The branch is gone, so only the merge message names it.
    let merge = row("Merge branch 'feature'");
    assert!(
        matches!(merge.relations.as_slice(), [Relation::Merges { branch: Some(b), into: Some(i), .. }] if b == "feature" && i == "main"),
        "{:?}",
        merge.relations
    );
    let base = row("base");
    assert!(
        matches!(base.relations.as_slice(), [Relation::BranchedFrom { branch: Some(b), from: Some(f), .. }] if b == "feature" && f == "main"),
        "{:?}",
        base.relations
    );
    assert!(row("main work").relations.is_empty());
    assert!(row("feature work").relations.is_empty());
}

#[test]
fn a_merged_branch_behind_its_upstream_keeps_its_local_name() {
    let fx = Fixture::new();
    let (origin, work, clone) = cloned(&fx);
    git(&work, &["checkout", "--quiet", "-b", "feature"]);
    commit(&work, "f.txt", "1\n", "feature 1", T0 + MINUTE);
    git(&work, &["push", "--quiet", origin.to_str().unwrap(), "feature"]);
    git(&clone, &["fetch", "--quiet"]);
    git(&clone, &["checkout", "--quiet", "-b", "feature", "--track", "origin/feature"]);
    git(&clone, &["checkout", "--quiet", "main"]);
    git_at(&clone, &["merge", "--quiet", "--no-ff", "--no-edit", "feature"], T0 + 2 * MINUTE);
    // The remote branch moves on after the merge, so its tip is the newest commit on the line.
    commit(&work, "f.txt", "2\n", "feature 2", T0 + 3 * MINUTE);
    git(&work, &["push", "--quiet", origin.to_str().unwrap(), "feature"]);
    git(&clone, &["fetch", "--quiet"]);

    let session = Session::open(&clone).unwrap();
    assert_eq!(upstream_of(&session, "feature"), tracking(&clone, "origin/feature", 0, 1), "the fixture is behind as intended");
    let rows = all_rows(&session);
    let merge = rows.iter().find(|row| row.summary == "Merge branch 'feature'").expect("the merge row");

    assert!(
        matches!(merge.relations.as_slice(), [Relation::Merges { branch: Some(b), into: Some(i), .. }] if b == "feature" && i == "main"),
        "{:?}",
        merge.relations
    );
}
