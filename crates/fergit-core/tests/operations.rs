//! Operations run through sessions against repositories and remotes built with the git CLI,
//! including the ways they fail.

mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use common::{Fixture, T0, commit, git, git_succeeds, rev_parse, write};
use fergit_core::askpass::{AskpassServer, PromptRequest, Prompter};
use fergit_core::session::journal::{Journal, JournalEntry, RefChange};
use fergit_core::session::{OpContext, Session};
use fergit_core::{CheckoutTarget, ForceMode, Oid, OpError, OpErrorKind, OpId, OpOutcome, Operation};

const MINUTE: i64 = 60;

/// A session over `repo` that journals into the fixture directory.
struct Harness {
    session: Session,
    context: OpContext,
    journal_path: PathBuf,
}

impl Harness {
    fn open(fx: &Fixture, repo: &Path) -> Harness {
        let journal_path = fx.path("journal.jsonl");
        let journal = Arc::new(Journal::open(&journal_path).expect("open the journal"));
        Harness {
            session: Session::open(repo).expect("open the repository"),
            context: OpContext { journal: Some(journal), askpass: None },
            journal_path,
        }
    }

    /// Runs `op` under a fresh id.
    fn run(&self, op: Operation) -> OpOutcome {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let id = OpId(format!("op-{}", NEXT.fetch_add(1, Ordering::Relaxed)));
        self.run_as(id, op)
    }

    fn run_as(&self, id: OpId, op: Operation) -> OpOutcome {
        self.session.run(id, op, &self.context, &mut |_| {})
    }

    fn journal(&self) -> Vec<JournalEntry> {
        Journal::read(&self.journal_path).expect("read the journal")
    }
}

#[track_caller]
fn assert_done(outcome: &OpOutcome) {
    assert!(matches!(outcome, OpOutcome::Done { .. }), "expected success, got {outcome:#?}");
}

#[track_caller]
fn failure(outcome: OpOutcome) -> OpError {
    match outcome {
        OpOutcome::Failed { error, .. } => error,
        OpOutcome::Done { .. } => panic!("expected a failure"),
    }
}

fn exists(repo: &Path, full_name: &str) -> bool {
    git_succeeds(repo, &["rev-parse", "--verify", "--quiet", full_name])
}

fn head_branch(repo: &Path) -> String {
    git(repo, &["rev-parse", "--abbrev-ref", "HEAD"]).trim().to_owned()
}

fn checkout(name: &str) -> Operation {
    Operation::Checkout { target: CheckoutTarget::Branch { name: name.to_owned() } }
}

fn create_branch(name: &str, at: Oid) -> Operation {
    Operation::CreateBranch { name: name.to_owned(), at, checkout: false, upstream: None }
}

fn delete_branch(name: &str, force: bool) -> Operation {
    Operation::DeleteBranch { name: name.to_owned(), force }
}

fn push(branch: &str, force: ForceMode) -> Operation {
    Operation::Push { branch: branch.to_owned(), remote: None, force, set_upstream: false }
}

/// A bare `origin.git` whose `main` has one commit, and a `clone` of it. Returns `(origin, clone)`.
fn cloned(fx: &Fixture) -> (PathBuf, PathBuf) {
    let origin = fx.init_bare("origin.git");
    let seed = fx.init("seed");
    commit(&seed, "a.txt", "1\n", "base", T0);
    git(&seed, &["push", "--quiet", origin.to_str().unwrap(), "main"]);
    (origin.clone(), clone_of(fx, &origin, "clone"))
}

fn clone_of(fx: &Fixture, origin: &Path, name: &str) -> PathBuf {
    git(&fx.path(""), &["clone", "--quiet", origin.to_str().unwrap(), name]);
    fx.path(name)
}

#[test]
fn checkout_switches_branches_and_detaches_head() {
    let fx = Fixture::new();
    let repo = fx.init("checkout");
    let first = commit(&repo, "a.txt", "1\n", "first", T0);
    git(&repo, &["branch", "topic"]);
    let second = commit(&repo, "a.txt", "2\n", "second", T0 + MINUTE);
    let h = Harness::open(&fx, &repo);

    let OpOutcome::Done { info } = h.run(checkout("topic")) else { panic!("checkout failed") };
    assert_eq!(head_branch(&repo), "topic");
    assert_eq!(info.head, Some(first), "the outcome reflects the repository as re-read");

    assert_done(&h.run(Operation::Checkout { target: CheckoutTarget::Commit { id: second } }));
    assert_eq!(head_branch(&repo), "HEAD", "detached");
    assert_eq!(rev_parse(&repo, "HEAD"), second);

    let journal = h.journal();
    assert_eq!(journal.len(), 2);
    assert_eq!(journal[0].head_before, "ref: refs/heads/main");
    assert_eq!(journal[0].head_after, "ref: refs/heads/topic");
    assert_eq!(journal[1].head_after, second.to_string());
    assert_eq!(journal[1].exit_code, Some(0));
}

#[test]
fn checkout_keeps_local_changes_it_would_overwrite() {
    let fx = Fixture::new();
    let repo = fx.init("dirty");
    commit(&repo, "a.txt", "1\n", "first", T0);
    git(&repo, &["branch", "topic"]);
    commit(&repo, "a.txt", "2\n", "second", T0 + MINUTE);
    write(&repo, "a.txt", "mine\n");
    let h = Harness::open(&fx, &repo);

    let error = failure(h.run(checkout("topic")));

    assert_eq!(error.kind, OpErrorKind::LocalChanges, "{error:#?}");
    assert!(error.output.contains("would be overwritten"), "git's own words are kept: {}", error.output);
    assert_eq!(head_branch(&repo), "main");
    assert_eq!(std::fs::read_to_string(repo.join("a.txt")).unwrap(), "mine\n");
}

#[test]
fn checkout_of_a_missing_branch_fails_without_guessing() {
    let fx = Fixture::new();
    let (_origin, clone) = cloned(&fx);
    git(&clone, &["branch", "--quiet", "-m", "main", "trunk"]);
    let h = Harness::open(&fx, &clone);

    // `git switch main` alone would create `main` from `origin/main`.
    let error = failure(h.run(checkout("main")));

    assert_eq!(error.kind, OpErrorKind::Git);
    assert!(!exists(&clone, "refs/heads/main"));
}

#[test]
fn create_branch_at_a_commit_optionally_checking_it_out_and_following_an_upstream() {
    let fx = Fixture::new();
    let (_origin, clone) = cloned(&fx);
    let base = rev_parse(&clone, "HEAD");
    commit(&clone, "a.txt", "2\n", "second", T0 + MINUTE);
    let h = Harness::open(&fx, &clone);

    assert_done(&h.run(create_branch("plain", base)));
    assert_eq!(rev_parse(&clone, "refs/heads/plain"), base);
    assert_eq!(head_branch(&clone), "main", "not checked out");

    let tracking = Operation::CreateBranch {
        name: "feature/x".to_owned(),
        at: base,
        checkout: true,
        upstream: Some("refs/remotes/origin/main".to_owned()),
    };
    assert_done(&h.run(tracking));
    assert_eq!(head_branch(&clone), "feature/x");
    assert_eq!(git(&clone, &["rev-parse", "--abbrev-ref", "feature/x@{upstream}"]).trim(), "origin/main");

    let error = failure(h.run(create_branch("plain", base)));
    assert_eq!(error.kind, OpErrorKind::AlreadyExists);

    let journal = h.journal();
    assert_eq!(
        journal[0].ref_changes,
        [RefChange { name: "refs/heads/plain".to_owned(), before: None, after: Some(base) }]
    );
}

#[test]
fn names_that_could_be_options_or_invalid_refs_never_reach_git() {
    let fx = Fixture::new();
    let repo = fx.init("names");
    let first = commit(&repo, "a.txt", "1\n", "first", T0);
    let h = Harness::open(&fx, &repo);

    for name in ["--upload-pack=touch pwned", "-D", "a..b", "with space", "x.lock", "HEAD"] {
        let error = failure(h.run(create_branch(name, first)));
        assert_eq!(error.kind, OpErrorKind::InvalidInput, "{name:?}");
        assert!(error.output.is_empty(), "git never ran for {name:?}");
        let tag = Operation::CreateTag { name: name.to_owned(), at: first, message: None };
        assert_eq!(failure(h.run(tag)).kind, OpErrorKind::InvalidInput, "tag {name:?}");
        assert_eq!(failure(h.run(delete_branch(name, true))).kind, OpErrorKind::InvalidInput, "{name:?}");
    }
    let bad_upstream = Operation::CreateBranch {
        name: "ok".to_owned(),
        at: first,
        checkout: false,
        upstream: Some("--track".to_owned()),
    };
    assert_eq!(failure(h.run(bad_upstream)).kind, OpErrorKind::InvalidInput);
    let bad_remote = Operation::Fetch { remote: Some("--upload-pack=touch pwned".to_owned()), prune: false };
    assert_eq!(failure(h.run(bad_remote)).kind, OpErrorKind::InvalidInput);

    assert_eq!(git(&repo, &["for-each-ref", "--format=%(refname)"]).trim(), "refs/heads/main");
    assert!(!repo.join("pwned").exists());
    assert!(h.journal().iter().all(|entry| entry.exit_code.is_none() && entry.ref_changes.is_empty()));
}

#[test]
fn deleting_a_branch_that_is_already_gone_succeeds() {
    let fx = Fixture::new();
    let repo = fx.init("delete");
    let first = commit(&repo, "a.txt", "1\n", "first", T0);
    git(&repo, &["branch", "old"]);
    let h = Harness::open(&fx, &repo);

    assert_done(&h.run(delete_branch("old", false)));
    assert!(!exists(&repo, "refs/heads/old"));
    assert_done(&h.run(delete_branch("old", false)));
    assert_done(&h.run(delete_branch("never-existed", true)));

    let journal = h.journal();
    assert_eq!(journal[0].ref_changes, [RefChange { name: "refs/heads/old".to_owned(), before: Some(first), after: None }]);
    assert!(journal[1].ref_changes.is_empty());
    assert_eq!(journal[1].error, None);
}

#[test]
fn deleting_an_unmerged_branch_needs_force() {
    let fx = Fixture::new();
    let repo = fx.init("unmerged");
    commit(&repo, "a.txt", "1\n", "first", T0);
    git(&repo, &["checkout", "--quiet", "-b", "topic"]);
    let work = commit(&repo, "b.txt", "1\n", "unmerged work", T0 + MINUTE);
    git(&repo, &["checkout", "--quiet", "main"]);
    let h = Harness::open(&fx, &repo);

    let error = failure(h.run(delete_branch("topic", false)));
    assert_eq!(error.kind, OpErrorKind::NotFullyMerged, "{error:#?}");
    assert_eq!(rev_parse(&repo, "refs/heads/topic"), work, "kept");

    assert_done(&h.run(delete_branch("topic", true)));
    assert!(!exists(&repo, "refs/heads/topic"));
}

#[test]
fn tags_are_created_lightweight_or_annotated_and_deleted_idempotently() {
    let fx = Fixture::new();
    let repo = fx.init("tags");
    let first = commit(&repo, "a.txt", "1\n", "first", T0);
    git(&repo, &["config", "user.name", "Tess Tagger"]);
    git(&repo, &["config", "user.email", "tess@example.com"]);
    let h = Harness::open(&fx, &repo);

    assert_done(&h.run(Operation::CreateTag { name: "light".to_owned(), at: first, message: None }));
    let annotated = Operation::CreateTag { name: "v1.0".to_owned(), at: first, message: Some("-- First release".to_owned()) };
    assert_done(&h.run(annotated));
    assert_eq!(git(&repo, &["cat-file", "-t", "refs/tags/light"]).trim(), "commit");
    assert_eq!(git(&repo, &["cat-file", "-t", "refs/tags/v1.0"]).trim(), "tag");
    assert_eq!(git(&repo, &["tag", "-l", "--format=%(contents:subject)", "v1.0"]).trim(), "-- First release");

    let again = Operation::CreateTag { name: "light".to_owned(), at: first, message: None };
    assert_eq!(failure(h.run(again)).kind, OpErrorKind::AlreadyExists);

    let tag_object = rev_parse(&repo, "refs/tags/v1.0");
    assert_done(&h.run(Operation::DeleteTag { name: "v1.0".to_owned() }));
    assert!(!exists(&repo, "refs/tags/v1.0"));
    assert_done(&h.run(Operation::DeleteTag { name: "v1.0".to_owned() }));

    let deleted = &h.journal()[3];
    assert_eq!(
        deleted.ref_changes,
        [RefChange { name: "refs/tags/v1.0".to_owned(), before: Some(tag_object), after: None }],
        "the journal records the tag object itself, so it can be restored exactly"
    );
}

#[test]
fn fetch_downloads_and_prunes() {
    let fx = Fixture::new();
    let (origin, clone) = cloned(&fx);
    let other = clone_of(&fx, &origin, "other");
    git(&other, &["push", "--quiet", "origin", "main:doomed"]);
    let h = Harness::open(&fx, &clone);

    assert_done(&h.run(Operation::Fetch { remote: Some("origin".to_owned()), prune: true }));
    assert!(exists(&clone, "refs/remotes/origin/doomed"));

    git(&other, &["push", "--quiet", "origin", "--delete", "doomed"]);
    let new = commit(&other, "a.txt", "2\n", "new", T0 + MINUTE);
    git(&other, &["push", "--quiet", "origin", "main"]);

    assert_done(&h.run(Operation::Fetch { remote: None, prune: true }));
    assert!(!exists(&clone, "refs/remotes/origin/doomed"), "pruned");
    assert_eq!(rev_parse(&clone, "refs/remotes/origin/main"), new);
    assert_eq!(rev_parse(&clone, "refs/heads/main"), rev_parse(&origin, "HEAD~1"), "fetch moves no local branch");

    let unknown = failure(h.run(Operation::Fetch { remote: Some("upstream".to_owned()), prune: false }));
    assert_eq!(unknown.kind, OpErrorKind::InvalidInput);
}

#[test]
fn pull_fast_forwards_but_refuses_to_merge_diverged_history() {
    let fx = Fixture::new();
    let (origin, clone) = cloned(&fx);
    let other = clone_of(&fx, &origin, "other");
    let theirs = commit(&other, "b.txt", "1\n", "theirs", T0 + MINUTE);
    git(&other, &["push", "--quiet", "origin", "main"]);
    let h = Harness::open(&fx, &clone);

    assert_done(&h.run(Operation::Pull));
    assert_eq!(rev_parse(&clone, "HEAD"), theirs);

    commit(&other, "b.txt", "2\n", "theirs again", T0 + 2 * MINUTE);
    git(&other, &["push", "--quiet", "origin", "main"]);
    let mine = commit(&clone, "c.txt", "1\n", "mine", T0 + 3 * MINUTE);

    let error = failure(h.run(Operation::Pull));
    assert_eq!(error.kind, OpErrorKind::Rejected, "{error:#?}");
    assert_eq!(rev_parse(&clone, "HEAD"), mine, "no merge was made");

    git(&clone, &["checkout", "--quiet", "--detach"]);
    assert_eq!(failure(h.run(Operation::Pull)).kind, OpErrorKind::InvalidInput);
}

#[test]
fn push_publishes_a_new_branch_and_sets_its_upstream() {
    let fx = Fixture::new();
    let (origin, clone) = cloned(&fx);
    git(&clone, &["checkout", "--quiet", "-b", "topic"]);
    let work = commit(&clone, "t.txt", "1\n", "topic work", T0 + MINUTE);
    let h = Harness::open(&fx, &clone);

    let op = Operation::Push { branch: "topic".to_owned(), remote: None, force: ForceMode::None, set_upstream: true };
    assert_done(&h.run(op));

    assert_eq!(rev_parse(&origin, "refs/heads/topic"), work);
    assert_eq!(git(&clone, &["rev-parse", "--abbrev-ref", "topic@{upstream}"]).trim(), "origin/topic");
    let changes = &h.journal()[0].ref_changes;
    assert_eq!(
        changes,
        &[RefChange { name: "refs/remotes/origin/topic".to_owned(), before: None, after: Some(work) }],
        "the push updated the remote-tracking branch"
    );
}

#[test]
fn push_is_rejected_when_the_remote_has_commits_the_branch_lacks() {
    let fx = Fixture::new();
    let (origin, clone) = cloned(&fx);
    let other = clone_of(&fx, &origin, "other");
    let theirs = commit(&other, "b.txt", "1\n", "theirs", T0 + MINUTE);
    git(&other, &["push", "--quiet", "origin", "main"]);
    commit(&clone, "c.txt", "1\n", "mine", T0 + 2 * MINUTE);
    let h = Harness::open(&fx, &clone);

    let error = failure(h.run(push("main", ForceMode::None)));

    assert_eq!(error.kind, OpErrorKind::Rejected, "{error:#?}");
    assert!(error.output.contains("[rejected]"), "{}", error.output);
    assert_eq!(rev_parse(&origin, "refs/heads/main"), theirs, "the remote kept its commits");
    let entry = &h.journal()[0];
    assert_eq!(entry.error.as_ref().map(|e| e.kind), Some(OpErrorKind::Rejected));
    assert_ne!(entry.exit_code, Some(0));
    assert!(entry.output.contains("[rejected]"));
}

#[test]
fn force_push_with_lease_replaces_only_the_tip_that_was_seen() {
    let fx = Fixture::new();
    let (origin, clone) = cloned(&fx);
    let seen = rev_parse(&clone, "refs/remotes/origin/main");
    git(&clone, &["commit", "--quiet", "--amend", "-m", "base, reworded"]);
    let rewritten = rev_parse(&clone, "HEAD");
    let h = Harness::open(&fx, &clone);

    // Without force, rewriting published history is refused.
    assert_eq!(failure(h.run(push("main", ForceMode::None))).kind, OpErrorKind::Rejected);

    assert_done(&h.run(push("main", ForceMode::WithLease { expected: Some(seen) })));
    assert_eq!(rev_parse(&origin, "refs/heads/main"), rewritten);
}

#[test]
fn force_push_with_a_stale_lease_is_rejected() {
    let fx = Fixture::new();
    let (origin, clone) = cloned(&fx);
    let seen = rev_parse(&clone, "refs/remotes/origin/main");
    // Someone else pushes after the user last looked...
    let other = clone_of(&fx, &origin, "other");
    let theirs = commit(&other, "b.txt", "1\n", "theirs", T0 + MINUTE);
    git(&other, &["push", "--quiet", "origin", "main"]);
    // ...and a background fetch brings their commit in, so git's own default lease (the
    // remote-tracking ref) would no longer protect it. The lease names what the user saw.
    git(&clone, &["fetch", "--quiet"]);
    git(&clone, &["commit", "--quiet", "--amend", "-m", "base, reworded"]);
    let h = Harness::open(&fx, &clone);

    let error = failure(h.run(push("main", ForceMode::WithLease { expected: Some(seen) })));

    assert_eq!(error.kind, OpErrorKind::StaleLease, "{error:#?}");
    assert_eq!(rev_parse(&origin, "refs/heads/main"), theirs, "their work survives");
}

#[test]
fn an_operation_id_runs_once() {
    let fx = Fixture::new();
    let repo = fx.init("once");
    let first = commit(&repo, "a.txt", "1\n", "first", T0);
    let h = Arc::new(Harness::open(&fx, &repo));
    let id = OpId("double-click".to_owned());
    let op = Operation::CreateTag { name: "v1".to_owned(), at: first, message: None };

    // Two requests racing, as from a double click: both see the one run's outcome.
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let (h, id, op) = (Arc::clone(&h), id.clone(), op.clone());
            thread::spawn(move || h.run_as(id, op))
        })
        .collect();
    let outcomes: Vec<OpOutcome> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_done(&outcomes[0]);
    assert_eq!(outcomes[0], outcomes[1]);
    // A late retry too.
    assert_eq!(h.run_as(id, op.clone()), outcomes[0]);
    assert_eq!(h.journal().len(), 1, "ran once");

    // A new id is a new request, which now fails: the tag exists.
    assert_eq!(failure(h.run(op)).kind, OpErrorKind::AlreadyExists);
}

#[test]
fn a_briefly_held_index_lock_is_waited_out() {
    let fx = Fixture::new();
    let repo = fx.init("locked");
    commit(&repo, "a.txt", "1\n", "first", T0);
    git(&repo, &["branch", "topic"]);
    commit(&repo, "a.txt", "2\n", "second", T0 + MINUTE);
    let h = Harness::open(&fx, &repo);
    let lock = repo.join(".git/index.lock");
    std::fs::write(&lock, "").unwrap();
    let release = thread::spawn(move || {
        thread::sleep(Duration::from_millis(250));
        std::fs::remove_file(lock).unwrap();
    });

    assert_done(&h.run(checkout("topic")));
    release.join().unwrap();
    assert_eq!(head_branch(&repo), "topic");
}

#[test]
fn credentials_in_remote_urls_are_scrubbed_from_output_and_journal() {
    let fx = Fixture::new();
    let repo = fx.init("scrub");
    commit(&repo, "a.txt", "1\n", "first", T0);
    // Nothing listens on port 1, so the fetch fails fast after printing the URL.
    git(&repo, &["remote", "add", "origin", "http://ada:hunter2@127.0.0.1:1/r.git"]);
    let mut progress = Vec::new();
    let h = Harness::open(&fx, &repo);

    let outcome = h.session.run(
        OpId("scrub".to_owned()),
        Operation::Fetch { remote: Some("origin".to_owned()), prune: false },
        &h.context,
        &mut |line| progress.push(line.to_owned()),
    );

    let error = failure(outcome);
    let journal = std::fs::read_to_string(&h.journal_path).unwrap();
    for text in [error.output.as_str(), error.message.as_str(), journal.as_str(), &progress.join("\n")] {
        assert!(!text.contains("hunter2"), "a secret leaked: {text}");
    }
}

/// Answers git's prompts: a user name, then a password.
struct User(Mutex<Vec<PromptRequest>>);

impl Prompter for User {
    fn prompt(&self, request: PromptRequest) -> Option<String> {
        let answer = if request.secret { "hunter2" } else { "ada" };
        self.0.lock().unwrap().push(request);
        Some(answer.to_owned())
    }
}

/// An HTTP server that answers every request with 401, recording the `Authorization` headers sent.
fn demanding_server() -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = Arc::clone(&seen);
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut request = Vec::new();
            let mut buf = [0u8; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => request.extend_from_slice(&buf[..n]),
                }
            }
            let request = String::from_utf8_lossy(&request).into_owned();
            if let Some(line) = request.lines().find(|line| line.to_ascii_lowercase().starts_with("authorization:")) {
                record.lock().unwrap().push(line.to_owned());
            }
            let _ = stream.write_all(
                b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"test\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });
    (port, seen)
}

#[test]
fn credential_prompts_reach_the_user_and_their_answers_reach_git() {
    let fx = Fixture::new();
    let repo = fx.init("askpass");
    commit(&repo, "a.txt", "1\n", "first", T0);
    let (port, authorizations) = demanding_server();
    git(&repo, &["remote", "add", "origin", &format!("http://127.0.0.1:{port}/r.git")]);
    let user = Arc::new(User(Mutex::default()));
    let server = AskpassServer::start(env!("CARGO_BIN_EXE_fergit-askpass").into(), user.clone()).unwrap();
    let mut h = Harness::open(&fx, &repo);
    h.context.askpass = Some(server.askpass().clone());

    let error = failure(h.run(Operation::Fetch { remote: Some("origin".to_owned()), prune: false }));

    // The server never accepts, so the fetch fails as an authentication failure after one try.
    assert_eq!(error.kind, OpErrorKind::AuthFailed, "{error:#?}");
    let prompts = user.0.lock().unwrap();
    assert_eq!(prompts.len(), 2, "{prompts:#?}");
    assert!(prompts[0].text.starts_with("Username for 'http://127.0.0.1:") && !prompts[0].secret);
    assert!(prompts[1].text.starts_with("Password for 'http://ada@127.0.0.1:") && prompts[1].secret);
    // `ada:hunter2` in Basic authentication.
    assert!(authorizations.lock().unwrap().iter().any(|header| header.ends_with("Basic YWRhOmh1bnRlcjI=")));
    let journal = std::fs::read_to_string(&h.journal_path).unwrap();
    assert!(!journal.contains("hunter2") && !journal.contains("YWRhOmh1bnRlcjI="));
}

#[test]
fn without_askpass_a_credential_request_fails_instead_of_hanging() {
    let fx = Fixture::new();
    let repo = fx.init("noprompt");
    commit(&repo, "a.txt", "1\n", "first", T0);
    let (port, _) = demanding_server();
    git(&repo, &["remote", "add", "origin", &format!("http://127.0.0.1:{port}/r.git")]);
    let h = Harness::open(&fx, &repo);

    let error = failure(h.run(Operation::Fetch { remote: Some("origin".to_owned()), prune: false }));

    assert_eq!(error.kind, OpErrorKind::AuthFailed, "{error:#?}");
}
