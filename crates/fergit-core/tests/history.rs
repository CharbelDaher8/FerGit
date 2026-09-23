//! History-changing operations (merge, rebase, cherry-pick, revert, reset, stash), the conflicts
//! they stop with, and undo from the journal, against repositories built with the git CLI.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use common::{Fixture, T0, commit, git, git_succeeds, rev_parse, write};
use fergit_core::session::journal::{Journal, JournalEntry};
use fergit_core::session::{OpContext, Session};
use fergit_core::{
    CheckoutTarget, ConflictFile, MergeMode, Oid, OpError, OpErrorKind, OpId, OpOutcome, Operation, RepoInfo,
    RepoState, ResetMode, Rev, Undoable,
};

const MINUTE: i64 = 60;

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

    fn run(&self, op: Operation) -> OpOutcome {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let id = OpId(format!("op-{}", NEXT.fetch_add(1, Ordering::Relaxed)));
        self.session.run(id, op, &self.context, &mut |_| {})
    }

    /// Runs `op`, which must succeed, and returns the repository as re-read afterwards.
    #[track_caller]
    fn done(&self, op: Operation) -> RepoInfo {
        match self.run(op) {
            OpOutcome::Done { info } => info,
            OpOutcome::Failed { error, .. } => panic!("expected success, got {error:#?}"),
        }
    }

    #[track_caller]
    fn failed(&self, op: Operation) -> OpError {
        match self.run(op) {
            OpOutcome::Failed { error, .. } => error,
            OpOutcome::Done { info } => panic!("expected a failure, got success with {:#?}", info.state),
        }
    }

    fn undoable(&self) -> Undoable {
        self.session.undoable(self.context.journal.as_ref().unwrap()).expect("read the journal")
    }

    /// Undoes the most recent undoable operation, which must be `kind`.
    fn undo(&self) -> OpOutcome {
        let Undoable::Ready { entry, .. } = self.undoable() else {
            panic!("nothing to undo: {:#?}", self.undoable());
        };
        self.run(Operation::Undo { entry })
    }

    fn journal(&self) -> Vec<JournalEntry> {
        Journal::read(&self.journal_path).expect("read the journal")
    }
}

fn branch(name: &str) -> Rev {
    Rev::Ref { name: format!("refs/heads/{name}") }
}

fn read(repo: &Path, file: &str) -> String {
    std::fs::read_to_string(repo.join(file)).unwrap_or_else(|_| panic!("read {file}"))
}

fn parents(repo: &Path, rev: &str) -> usize {
    git(repo, &["rev-list", "--parents", "-n", "1", rev]).split_whitespace().count() - 1
}

fn unresolved(paths: &[&str]) -> Vec<ConflictFile> {
    paths.iter().map(|path| ConflictFile { path: (*path).to_owned(), resolved: false }).collect()
}

fn is_clean(repo: &Path) -> bool {
    git(repo, &["status", "--porcelain"]).trim().is_empty()
}

/// `main` and `topic` both change line 1 of `a.txt` after a common `base`. Returns
/// `(repo, base, main tip, topic tip)` with `main` checked out.
fn diverged(fx: &Fixture, name: &str) -> (PathBuf, Oid, Oid, Oid) {
    let repo = fx.init(name);
    let base = commit(&repo, "a.txt", "base\n", "base", T0);
    git(&repo, &["checkout", "--quiet", "-b", "topic"]);
    let topic = commit(&repo, "a.txt", "topic\n", "topic change", T0 + MINUTE);
    git(&repo, &["checkout", "--quiet", "main"]);
    let main = commit(&repo, "a.txt", "main\n", "main change", T0 + 2 * MINUTE);
    (repo, base, main, topic)
}

#[test]
fn merge_fast_forwards_or_records_a_merge_or_squashes() {
    let fx = Fixture::new();
    let repo = fx.init("merge");
    let base = commit(&repo, "a.txt", "1\n", "base", T0);
    git(&repo, &["checkout", "--quiet", "-b", "topic"]);
    let topic = commit(&repo, "b.txt", "1\n", "topic work", T0 + MINUTE);
    git(&repo, &["checkout", "--quiet", "main"]);
    let h = Harness::open(&fx, &repo);

    let info = h.done(Operation::Merge { from: branch("topic"), mode: MergeMode::Ff });
    assert_eq!(info.head, Some(topic), "fast-forwarded");
    assert_eq!(info.state, RepoState::Clean);

    git(&repo, &["reset", "--quiet", "--hard", &base.to_string()]);
    h.done(Operation::Merge { from: branch("topic"), mode: MergeMode::NoFf });
    assert_eq!(parents(&repo, "HEAD"), 2, "a merge commit, even though it could fast-forward");
    assert_eq!(git(&repo, &["log", "-1", "--format=%s"]).trim(), "Merge branch 'topic'");

    git(&repo, &["reset", "--quiet", "--hard", &base.to_string()]);
    h.done(Operation::Merge { from: branch("topic"), mode: MergeMode::Squash });
    assert_eq!(parents(&repo, "HEAD"), 1, "a squash records no merge");
    assert_ne!(rev_parse(&repo, "HEAD"), topic);
    assert_eq!(read(&repo, "b.txt"), "1\n");
    assert!(is_clean(&repo), "the squashed changes were committed");

    // Squashing what is already merged has nothing to commit.
    let before = rev_parse(&repo, "HEAD");
    h.done(Operation::Merge { from: branch("topic"), mode: MergeMode::Squash });
    assert_eq!(rev_parse(&repo, "HEAD"), before);
}

#[test]
fn a_conflicting_merge_stops_and_continues_once_resolved() {
    let fx = Fixture::new();
    let (repo, _, main, topic) = diverged(&fx, "conflict");
    let h = Harness::open(&fx, &repo);

    let info = h.done(Operation::Merge { from: branch("topic"), mode: MergeMode::NoFf });
    let RepoState::Merging { heads, squash, message, conflicts } = info.state else {
        panic!("expected a merge in progress, got {:#?}", info.state);
    };
    assert_eq!((heads, squash), (vec![topic], false));
    assert_eq!(message, "Merge branch 'topic'");
    assert_eq!(conflicts, unresolved(&["a.txt"]));
    assert_eq!(info.head, Some(main), "nothing committed yet");

    // Markers left in the file: continuing would commit them.
    let error = h.failed(Operation::Continue);
    assert_eq!(error.kind, OpErrorKind::InvalidInput);
    assert!(error.message.contains("a.txt"), "{}", error.message);

    write(&repo, "a.txt", "both\n");
    let info = h.session.refresh().unwrap();
    assert_eq!(info.state.conflicts(), [ConflictFile { path: "a.txt".to_owned(), resolved: true }]);

    let info = h.done(Operation::Continue);
    assert_eq!(info.state, RepoState::Clean);
    assert_eq!(parents(&repo, "HEAD"), 2);
    assert_eq!(rev_parse(&repo, "HEAD^2"), topic);
    assert_eq!(read(&repo, "a.txt"), "both\n");
    assert!(is_clean(&repo));
}

#[test]
fn a_conflicting_merge_can_be_aborted() {
    let fx = Fixture::new();
    let (repo, _, main, _) = diverged(&fx, "abort");
    let h = Harness::open(&fx, &repo);

    assert!(matches!(h.done(Operation::Merge { from: branch("topic"), mode: MergeMode::Ff }).state, RepoState::Merging { .. }));
    let info = h.done(Operation::Abort);

    assert_eq!(info.state, RepoState::Clean);
    assert_eq!(info.head, Some(main));
    assert_eq!(read(&repo, "a.txt"), "main\n");
    assert!(is_clean(&repo));
    h.done(Operation::Abort);
    assert_eq!(h.failed(Operation::Continue).kind, OpErrorKind::InvalidInput, "nothing to continue");
}

#[test]
fn a_conflicting_squash_merge_is_a_merge_in_progress() {
    let fx = Fixture::new();
    let (repo, _, main, _) = diverged(&fx, "squash");
    let h = Harness::open(&fx, &repo);

    let info = h.done(Operation::Merge { from: branch("topic"), mode: MergeMode::Squash });
    assert!(matches!(&info.state, RepoState::Merging { squash: true, heads, .. } if heads.is_empty()), "{:#?}", info.state);

    write(&repo, "a.txt", "both\n");
    h.done(Operation::Continue);
    assert_eq!(parents(&repo, "HEAD"), 1);
    assert_eq!(rev_parse(&repo, "HEAD~1"), main);
    assert!(git(&repo, &["log", "-1", "--format=%B"]).starts_with("Squashed commit of the following:"));
}

#[test]
fn a_conflicting_rebase_stops_then_continues_without_an_editor() {
    let fx = Fixture::new();
    let (repo, _, main, _) = diverged(&fx, "rebase");
    // An editor that leaves a trace: git must never start it.
    git(&repo, &["config", "core.editor", "touch editor-ran &&"]);
    git(&repo, &["config", "sequence.editor", "touch sequence-editor-ran &&"]);
    git(&repo, &["checkout", "--quiet", "topic"]);
    let extra = commit(&repo, "b.txt", "1\n", "topic extra", T0 + 3 * MINUTE);
    let h = Harness::open(&fx, &repo);

    let info = h.done(Operation::Rebase { onto: branch("main") });
    let RepoState::Rebasing { branch: rebasing, onto, step, total, conflicts, .. } = &info.state else {
        panic!("expected a rebase in progress, got {:#?}", info.state);
    };
    assert_eq!((rebasing.as_deref(), *onto, *step, *total), (Some("topic"), Some(main), 1, 2));
    assert_eq!(conflicts, &unresolved(&["a.txt"]));
    assert_eq!(info.branch, None, "HEAD is detached while rebasing");

    write(&repo, "a.txt", "main and topic\n");
    let info = h.done(Operation::Continue);

    assert_eq!(info.state, RepoState::Clean);
    assert_eq!(info.branch.as_deref(), Some("topic"));
    assert_eq!(rev_parse(&repo, "topic~2"), main, "both topic commits were replayed onto main");
    assert_ne!(rev_parse(&repo, "topic"), extra);
    assert_eq!(read(&repo, "b.txt"), "1\n");
    assert!(!repo.join("editor-ran").exists() && !repo.join("sequence-editor-ran").exists());
}

#[test]
fn a_rebase_can_skip_the_conflicting_commit_or_be_aborted() {
    let fx = Fixture::new();
    let (repo, _, main, topic) = diverged(&fx, "skip");
    git(&repo, &["checkout", "--quiet", "topic"]);
    let h = Harness::open(&fx, &repo);

    assert!(matches!(h.done(Operation::Rebase { onto: branch("main") }).state, RepoState::Rebasing { .. }));
    let info = h.done(Operation::Abort);
    assert_eq!(info.state, RepoState::Clean);
    assert_eq!(rev_parse(&repo, "topic"), topic, "the branch is as it was");
    assert_eq!(info.branch.as_deref(), Some("topic"));

    assert!(matches!(h.done(Operation::Rebase { onto: branch("main") }).state, RepoState::Rebasing { .. }));
    let info = h.done(Operation::Skip);
    assert_eq!(info.state, RepoState::Clean);
    assert_eq!(rev_parse(&repo, "topic"), main, "the only commit was skipped");
}

#[test]
fn rebase_refuses_uncommitted_changes() {
    let fx = Fixture::new();
    let (repo, _, _, topic) = diverged(&fx, "dirty");
    git(&repo, &["checkout", "--quiet", "topic"]);
    write(&repo, "a.txt", "mine\n");
    let h = Harness::open(&fx, &repo);

    let error = h.failed(Operation::Rebase { onto: branch("main") });

    assert_eq!(error.kind, OpErrorKind::LocalChanges, "{error:#?}");
    assert_eq!(rev_parse(&repo, "topic"), topic);
    assert_eq!(read(&repo, "a.txt"), "mine\n");
}

#[test]
fn cherry_pick_applies_several_commits_and_stops_at_a_conflict() {
    let fx = Fixture::new();
    let (repo, _, main, _) = diverged(&fx, "pick");
    git(&repo, &["checkout", "--quiet", "topic"]);
    let clean = commit(&repo, "b.txt", "1\n", "adds b", T0 + 3 * MINUTE);
    let conflicting = commit(&repo, "a.txt", "topic again\n", "changes a", T0 + 4 * MINUTE);
    let after = commit(&repo, "c.txt", "1\n", "adds c", T0 + 5 * MINUTE);
    git(&repo, &["checkout", "--quiet", "main"]);
    let h = Harness::open(&fx, &repo);

    let info = h.done(Operation::CherryPick { commits: vec![clean, conflicting, after] });
    assert_eq!(info.state, RepoState::CherryPicking { commit: Some(conflicting), conflicts: unresolved(&["a.txt"]) });
    assert_eq!(rev_parse(&repo, "main~1"), main, "the first pick is committed");

    write(&repo, "a.txt", "resolved\n");
    let info = h.done(Operation::Continue);
    assert_eq!(info.state, RepoState::Clean);
    assert_eq!(rev_parse(&repo, "main~3"), main, "all three picked");
    assert_eq!((read(&repo, "b.txt"), read(&repo, "c.txt")), ("1\n".to_owned(), "1\n".to_owned()));
}

#[test]
fn a_cherry_pick_can_be_aborted_back_to_where_it_started() {
    let fx = Fixture::new();
    let (repo, _, main, topic) = diverged(&fx, "pick-abort");
    let h = Harness::open(&fx, &repo);

    assert!(matches!(h.done(Operation::CherryPick { commits: vec![topic] }).state, RepoState::CherryPicking { .. }));
    let info = h.done(Operation::Abort);

    assert_eq!((info.state, info.head), (RepoState::Clean, Some(main)));
    assert_eq!(h.failed(Operation::CherryPick { commits: vec![] }).kind, OpErrorKind::InvalidInput);
}

#[test]
fn revert_adds_a_commit_and_stops_at_a_conflict() {
    let fx = Fixture::new();
    let repo = fx.init("revert");
    commit(&repo, "a.txt", "1\n", "base", T0);
    let added = commit(&repo, "b.txt", "1\n", "adds b", T0 + MINUTE);
    let changed = commit(&repo, "a.txt", "2\n", "changes a", T0 + 2 * MINUTE);
    commit(&repo, "a.txt", "3\n", "changes a again", T0 + 3 * MINUTE);
    let h = Harness::open(&fx, &repo);

    h.done(Operation::Revert { commit: added });
    assert!(!repo.join("b.txt").exists());
    assert_eq!(git(&repo, &["log", "-1", "--format=%s"]).trim(), "Revert \"adds b\"");

    let info = h.done(Operation::Revert { commit: changed });
    assert_eq!(info.state, RepoState::Reverting { commit: Some(changed), conflicts: unresolved(&["a.txt"]) });
    h.done(Operation::Abort);
    assert_eq!(read(&repo, "a.txt"), "3\n");
}

#[test]
fn reset_moves_the_current_branch_with_the_index_and_files_as_asked() {
    let fx = Fixture::new();
    let repo = fx.init("reset");
    let first = commit(&repo, "a.txt", "1\n", "first", T0);
    let second = commit(&repo, "a.txt", "2\n", "second", T0 + MINUTE);
    let h = Harness::open(&fx, &repo);
    let reset = |mode, to, expected| Operation::Reset { branch: "main".to_owned(), to, mode, expected };

    h.done(reset(ResetMode::Soft, first, second));
    assert_eq!(rev_parse(&repo, "main"), first);
    assert_eq!(git(&repo, &["status", "--porcelain"]), "M  a.txt\n", "the change is staged");

    git(&repo, &["reset", "--quiet", "--hard", &second.to_string()]);
    h.done(reset(ResetMode::Mixed, first, second));
    assert_eq!(git(&repo, &["status", "--porcelain"]), " M a.txt\n", "the change is in the file, unstaged");

    git(&repo, &["reset", "--quiet", "--hard", &second.to_string()]);
    write(&repo, "a.txt", "uncommitted\n");
    h.done(reset(ResetMode::Hard, first, second));
    assert_eq!(read(&repo, "a.txt"), "1\n");
    assert!(is_clean(&repo));

    let entry = h.journal().pop().unwrap();
    assert_eq!(entry.ref_changes[0].before, Some(second));
    assert_eq!(entry.ref_changes[0].after, Some(first));
}

#[test]
fn reset_of_another_branch_moves_only_the_branch() {
    let fx = Fixture::new();
    let repo = fx.init("other");
    let first = commit(&repo, "a.txt", "1\n", "first", T0);
    let second = commit(&repo, "a.txt", "2\n", "second", T0 + MINUTE);
    git(&repo, &["branch", "topic"]);
    write(&repo, "a.txt", "uncommitted\n");
    let h = Harness::open(&fx, &repo);

    h.done(Operation::Reset { branch: "topic".to_owned(), to: first, mode: ResetMode::Hard, expected: second });

    assert_eq!(rev_parse(&repo, "topic"), first);
    assert_eq!(rev_parse(&repo, "main"), second);
    assert_eq!(read(&repo, "a.txt"), "uncommitted\n", "the checked-out files are untouched");
}

#[test]
fn reset_is_refused_when_the_branch_moved_since_it_was_seen() {
    let fx = Fixture::new();
    let repo = fx.init("stale");
    let first = commit(&repo, "a.txt", "1\n", "first", T0);
    let seen = commit(&repo, "a.txt", "2\n", "second", T0 + MINUTE);
    // Someone commits after the user looked.
    let theirs = commit(&repo, "a.txt", "3\n", "theirs", T0 + 2 * MINUTE);
    let h = Harness::open(&fx, &repo);

    let error = h.failed(Operation::Reset { branch: "main".to_owned(), to: first, mode: ResetMode::Hard, expected: seen });

    assert_eq!(error.kind, OpErrorKind::Moved, "{error:#?}");
    assert!(error.message.contains("main has moved"), "{}", error.message);
    assert_eq!(rev_parse(&repo, "main"), theirs, "their commit survives");
    assert_eq!(read(&repo, "a.txt"), "3\n");
    assert!(h.journal()[0].ref_changes.is_empty());

    let bad = Operation::Reset { branch: "--force".to_owned(), to: first, mode: ResetMode::Soft, expected: theirs };
    assert_eq!(h.failed(bad).kind, OpErrorKind::InvalidInput);
}

#[test]
fn reset_waits_for_a_merge_in_progress_to_end() {
    let fx = Fixture::new();
    let (repo, base, main, _) = diverged(&fx, "busy");
    let h = Harness::open(&fx, &repo);
    h.done(Operation::Merge { from: branch("topic"), mode: MergeMode::Ff });

    let error = h.failed(Operation::Reset { branch: "main".to_owned(), to: base, mode: ResetMode::Hard, expected: main });

    assert_eq!(error.kind, OpErrorKind::InvalidInput);
    assert_eq!(rev_parse(&repo, "main"), main);
}

#[test]
fn undo_restores_the_tips_an_operation_moved() {
    let fx = Fixture::new();
    let repo = fx.init("undo");
    let first = commit(&repo, "a.txt", "1\n", "first", T0);
    let second = commit(&repo, "a.txt", "2\n", "second", T0 + MINUTE);
    let h = Harness::open(&fx, &repo);
    assert_eq!(h.undoable(), Undoable::Nothing);

    h.done(Operation::CreateTag { name: "v1".to_owned(), at: second, message: None });
    h.done(Operation::Reset { branch: "main".to_owned(), to: first, mode: ResetMode::Hard, expected: second });
    h.done(Operation::Fetch { remote: None, prune: false });
    assert_eq!(read(&repo, "a.txt"), "1\n");

    let Undoable::Ready { operation, changes, head, .. } = h.undoable() else { panic!("{:#?}", h.undoable()) };
    assert!(matches!(operation, Operation::Reset { .. }), "the fetch is passed over: {operation:#?}");
    assert_eq!((changes.len(), head), (1, None));

    let OpOutcome::Done { info } = h.undo() else { panic!("undo failed") };
    assert_eq!(info.head, Some(second));
    assert_eq!(read(&repo, "a.txt"), "2\n", "the files follow the branch");
    assert!(is_clean(&repo));

    // The reset is undone; next comes the tag.
    assert!(matches!(h.undoable(), Undoable::Ready { operation: Operation::CreateTag { .. }, .. }));
    assert!(matches!(h.undo(), OpOutcome::Done { .. }));
    assert!(!git_succeeds(&repo, &["rev-parse", "--verify", "--quiet", "refs/tags/v1"]));
    assert_eq!(h.undoable(), Undoable::Nothing);

    // The journal only grew: every entry, undos included, is still there as written.
    let journal = h.journal();
    assert_eq!(journal.len(), 5);
    assert!(matches!(journal[3].operation, Operation::Undo { .. }));
    assert_eq!(journal[3].ref_changes[0].after, Some(second));
}

#[test]
fn undo_is_refused_after_the_refs_moved() {
    let fx = Fixture::new();
    let repo = fx.init("moved");
    let first = commit(&repo, "a.txt", "1\n", "first", T0);
    let second = commit(&repo, "a.txt", "2\n", "second", T0 + MINUTE);
    let h = Harness::open(&fx, &repo);
    h.done(Operation::Reset { branch: "main".to_owned(), to: first, mode: ResetMode::Soft, expected: second });
    let Undoable::Ready { entry, .. } = h.undoable() else { panic!() };

    // New work on top of the reset: restoring main would throw it away.
    git(&repo, &["commit", "--quiet", "-m", "new work"]);
    let new_work = rev_parse(&repo, "HEAD");

    let error = h.failed(Operation::Undo { entry: entry.clone() });

    assert_eq!(error.kind, OpErrorKind::Moved, "{error:#?}");
    assert_eq!(rev_parse(&repo, "main"), new_work);
    assert!(h.journal().last().unwrap().ref_changes.is_empty(), "nothing moved");
    assert!(matches!(h.undoable(), Undoable::Ready { .. }), "a refused undo undoes nothing");

    let unknown = h.failed(Operation::Undo { entry: OpId("no-such-entry".to_owned()) });
    assert_eq!(unknown.kind, OpErrorKind::InvalidInput);
}

#[test]
fn undo_of_a_soft_or_mixed_reset_keeps_the_files() {
    let fx = Fixture::new();
    let repo = fx.init("modes");
    let first = commit(&repo, "a.txt", "1\n", "first", T0);
    let second = commit(&repo, "a.txt", "2\n", "second", T0 + MINUTE);
    let h = Harness::open(&fx, &repo);

    for mode in [ResetMode::Soft, ResetMode::Mixed] {
        h.done(Operation::Reset { branch: "main".to_owned(), to: first, mode, expected: second });
        assert!(matches!(h.undo(), OpOutcome::Done { .. }), "{mode:?}");
        assert_eq!(rev_parse(&repo, "main"), second);
        assert!(is_clean(&repo), "{mode:?}: {}", git(&repo, &["status", "--porcelain"]));
    }
}

#[test]
fn undo_of_a_checkout_switches_back_and_undo_of_a_merge_after_continue_restores_the_branch() {
    let fx = Fixture::new();
    let (repo, _, main, _) = diverged(&fx, "switch");
    let h = Harness::open(&fx, &repo);

    h.done(Operation::Checkout { target: CheckoutTarget::Branch { name: "topic".to_owned() } });
    assert!(matches!(h.undoable(), Undoable::Ready { head: Some(ref head), .. } if head == "ref: refs/heads/main"));
    let OpOutcome::Done { info } = h.undo() else { panic!() };
    assert_eq!(info.branch.as_deref(), Some("main"));
    assert_eq!(read(&repo, "a.txt"), "main\n");

    h.done(Operation::Merge { from: branch("topic"), mode: MergeMode::NoFf });
    write(&repo, "a.txt", "both\n");
    h.done(Operation::Continue);
    assert!(matches!(h.undoable(), Undoable::Ready { operation: Operation::Continue, .. }));
    let OpOutcome::Done { info } = h.undo() else { panic!() };
    assert_eq!(info.head, Some(main));
    assert_eq!(read(&repo, "a.txt"), "main\n");
}

#[test]
fn undo_waits_for_what_is_in_progress_and_stops_at_a_push() {
    let fx = Fixture::new();
    let (repo, base, main, _) = diverged(&fx, "blocked");
    let origin = fx.init_bare("origin.git");
    git(&repo, &["remote", "add", "origin", origin.to_str().unwrap()]);
    let h = Harness::open(&fx, &repo);

    h.done(Operation::Reset { branch: "main".to_owned(), to: base, mode: ResetMode::Hard, expected: main });
    h.done(Operation::Merge { from: branch("topic"), mode: MergeMode::Ff });
    // A fast-forward: nothing conflicts. Undo it later; first try while a cherry-pick is stopped.
    let Undoable::Ready { entry, .. } = h.undoable() else { panic!() };
    git(&repo, &["checkout", "--quiet", "-b", "side", &base.to_string()]);
    commit(&repo, "a.txt", "side\n", "side", T0 + 5 * MINUTE);
    git(&repo, &["checkout", "--quiet", "main"]);
    let side = rev_parse(&repo, "side");
    assert!(matches!(h.done(Operation::CherryPick { commits: vec![side] }).state, RepoState::CherryPicking { .. }));
    assert_eq!(h.failed(Operation::Undo { entry: entry.clone() }).kind, OpErrorKind::InvalidInput);
    h.done(Operation::Abort);

    let push = Operation::Push {
        branch: "main".to_owned(),
        remote: Some("origin".to_owned()),
        force: fergit_core::ForceMode::None,
        set_upstream: false,
    };
    h.done(push);
    let Undoable::Blocked { reason, .. } = h.undoable() else { panic!("{:#?}", h.undoable()) };
    assert!(reason.contains("push"), "{reason}");
    assert_eq!(h.failed(Operation::Undo { entry }).kind, OpErrorKind::Moved);
}

#[test]
fn stashes_round_trip() {
    let fx = Fixture::new();
    let repo = fx.init("stash");
    commit(&repo, "a.txt", "1\n", "first", T0);
    let h = Harness::open(&fx, &repo);
    let stashes = || git(&repo, &["stash", "list", "--format=%H %gs"]);

    write(&repo, "a.txt", "work\n");
    write(&repo, "new.txt", "untracked\n");
    h.done(Operation::StashPush { message: Some("-- my work".to_owned()), untracked: true });
    assert!(is_clean(&repo));
    assert!(stashes().contains("-- my work"), "{}", stashes());
    let first = rev_parse(&repo, "stash@{0}");
    assert_eq!(h.failed(Operation::StashPush { message: None, untracked: false }).kind, OpErrorKind::InvalidInput);

    h.done(Operation::StashApply { index: 0, id: first });
    assert_eq!((read(&repo, "a.txt"), read(&repo, "new.txt")), ("work\n".to_owned(), "untracked\n".to_owned()));
    assert_eq!(rev_parse(&repo, "stash@{0}"), first, "apply keeps the stash");

    h.done(Operation::StashPush { message: None, untracked: true });
    let second = rev_parse(&repo, "stash@{0}");
    // The list renumbered: `first` is stash@{1} now, so a request made for stash@{0} is refused.
    assert_eq!(h.failed(Operation::StashDrop { index: 0, id: first }).kind, OpErrorKind::Moved);

    h.done(Operation::StashPop { index: 0, id: second });
    assert_eq!(read(&repo, "a.txt"), "work\n");
    assert_eq!(rev_parse(&repo, "stash@{0}"), first, "pop removes the stash");

    git(&repo, &["checkout", "--quiet", "--", "a.txt"]);
    std::fs::remove_file(repo.join("new.txt")).unwrap();
    h.done(Operation::StashDrop { index: 0, id: first });
    assert!(stashes().is_empty());

    // Undo brings the dropped stash back...
    assert!(matches!(h.undo(), OpOutcome::Done { .. }));
    assert_eq!(rev_parse(&repo, "stash@{0}"), first);
    assert!(stashes().contains("-- my work"), "with its message: {}", stashes());

    // ...and takes a pushed one back out into the files.
    write(&repo, "a.txt", "more work\n");
    h.done(Operation::StashPush { message: None, untracked: false });
    assert!(matches!(h.undo(), OpOutcome::Done { .. }));
    assert_eq!(read(&repo, "a.txt"), "more work\n");
    assert_eq!(rev_parse(&repo, "stash@{0}"), first);

    // A pop can't be undone.
    git(&repo, &["checkout", "--quiet", "--", "a.txt"]);
    h.done(Operation::StashPop { index: 0, id: first });
    assert!(matches!(h.undoable(), Undoable::Blocked { .. }));
}

#[test]
fn a_stash_that_conflicts_is_kept_and_its_conflicts_can_be_resolved_or_aborted() {
    let fx = Fixture::new();
    let repo = fx.init("stash-conflict");
    commit(&repo, "a.txt", "1\n", "first", T0);
    write(&repo, "a.txt", "stashed\n");
    git(&repo, &["stash", "push", "--quiet"]);
    let stash = rev_parse(&repo, "stash@{0}");
    commit(&repo, "a.txt", "committed\n", "second", T0 + MINUTE);
    let h = Harness::open(&fx, &repo);

    let info = h.done(Operation::StashPop { index: 0, id: stash });
    assert_eq!(info.state, RepoState::Unmerged { conflicts: unresolved(&["a.txt"]) });
    assert_eq!(rev_parse(&repo, "stash@{0}"), stash, "kept");

    let info = h.done(Operation::Abort);
    assert_eq!(info.state, RepoState::Clean);
    assert_eq!(read(&repo, "a.txt"), "committed\n");

    h.done(Operation::StashApply { index: 0, id: stash });
    write(&repo, "a.txt", "merged\n");
    let info = h.done(Operation::Continue);
    assert_eq!(info.state, RepoState::Clean);
    assert_eq!(git(&repo, &["status", "--porcelain"]), "M  a.txt\n", "resolved and staged");
}

#[test]
fn history_operations_validate_what_they_are_given() {
    let fx = Fixture::new();
    let repo = fx.init("validate");
    commit(&repo, "a.txt", "1\n", "first", T0);
    let h = Harness::open(&fx, &repo);

    for name in ["--upload-pack=touch pwned", "refs/heads/a..b", "main"] {
        let rev = Rev::Ref { name: name.to_owned() };
        let error = h.failed(Operation::Merge { from: rev.clone(), mode: MergeMode::Ff });
        assert_eq!(error.kind, OpErrorKind::InvalidInput, "{name:?}");
        assert!(error.output.is_empty(), "git never ran for {name:?}");
        assert_eq!(h.failed(Operation::Rebase { onto: rev }).kind, OpErrorKind::InvalidInput, "{name:?}");
    }
    assert_eq!(h.failed(Operation::Skip).kind, OpErrorKind::InvalidInput);
    assert!(!repo.join("pwned").exists());
}
