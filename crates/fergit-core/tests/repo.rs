//! Integration tests for `fergit_core::repo`, against repositories built with the git CLI.

mod common;

use std::collections::HashMap;
use std::path::Path;

use common::{AUTHOR_EMAIL, AUTHOR_NAME, Fixture, T0, commit, git, git_at, git_dated, rev_parse, write};
use fergit_core::repo::{Head, History, Repo, RepoError};
use fergit_core::{ChangeStatus, FileChange, Oid, RefKind, RefLabel};

const MINUTE: i64 = 60;

fn read(repo: &Path) -> History {
    Repo::open(repo).expect("open repository").read_history().expect("read history")
}

fn commits(history: &History) -> Vec<Oid> {
    (0..history.commits.len()).map(|i| history.commits.id(i)).collect()
}

/// `(commit, parents)` rows in display order.
fn rows(history: &History) -> Vec<(Oid, Vec<Oid>)> {
    (0..history.commits.len())
        .map(|i| (history.commits.id(i), history.commits.parents(i).to_vec()))
        .collect()
}

fn label(kind: RefKind, name: &str, full_name: &str, is_head: bool) -> RefLabel {
    RefLabel { kind, name: name.to_owned(), full_name: full_name.to_owned(), is_head }
}

fn branch(name: &str, is_head: bool) -> RefLabel {
    label(RefKind::LocalBranch, name, &format!("refs/heads/{name}"), is_head)
}

fn tag(name: &str) -> RefLabel {
    label(RefKind::Tag, name, &format!("refs/tags/{name}"), false)
}

fn assert_children_before_parents(history: &History) {
    let position: HashMap<Oid, usize> = commits(history).into_iter().enumerate().map(|(i, id)| (id, i)).collect();
    for (i, (id, parents)) in rows(history).into_iter().enumerate() {
        for parent in parents {
            assert!(position[&parent] > i, "{id} is listed after its parent {parent}");
        }
    }
}

struct MergeHistory {
    base: Oid,
    feature_1: Oid,
    main_1: Oid,
    feature_2: Oid,
    merge: Oid,
}

/// ```text
/// base ── main_1 ─────────── merge   (main)
///    └─ feature_1 ── feature_2 ┘      (feature)
/// ```
/// with commit times increasing in the order `base, feature_1, main_1, feature_2, merge`.
fn merge_history(repo: &Path) -> MergeHistory {
    let base = commit(repo, "base.txt", "base\n", "base", T0);
    git(repo, &["checkout", "--quiet", "-b", "feature"]);
    let feature_1 = commit(repo, "feature.txt", "one\n", "feature 1", T0 + MINUTE);
    git(repo, &["checkout", "--quiet", "main"]);
    let main_1 = commit(repo, "main.txt", "main\n", "main 1", T0 + 2 * MINUTE);
    git(repo, &["checkout", "--quiet", "feature"]);
    let feature_2 = commit(repo, "feature.txt", "one\ntwo\n", "feature 2", T0 + 3 * MINUTE);
    git(repo, &["checkout", "--quiet", "main"]);
    git_at(repo, &["merge", "--quiet", "--no-ff", "-m", "Merge feature", "feature"], T0 + 4 * MINUTE);
    MergeHistory { base, feature_1, main_1, feature_2, merge: rev_parse(repo, "HEAD") }
}

// ---------------------------------------------------------------------------------------------
// Opening

#[test]
fn opens_from_a_subdirectory() {
    let fx = Fixture::new();
    let repo = fx.init("repo");
    commit(&repo, "src/deep/file.txt", "x\n", "initial", T0);

    let opened = Repo::open(&repo.join("src").join("deep")).unwrap();

    let canonical = |path: &Path| std::fs::canonicalize(path).unwrap();
    assert_eq!(canonical(opened.root()), canonical(&repo));
    assert_eq!(opened.read_history().unwrap().commits.len(), 1);
}

#[test]
fn a_directory_outside_any_repository_is_not_a_repository() {
    let fx = Fixture::new();
    let plain = fx.path("plain");
    std::fs::create_dir(&plain).unwrap();

    match Repo::open(&plain) {
        Err(RepoError::NotARepository { path }) => assert_eq!(path, plain),
        Err(other) => panic!("expected NotARepository, got {other:?}"),
        Ok(_) => panic!("expected NotARepository, got a repository"),
    }
}

// ---------------------------------------------------------------------------------------------
// History

#[test]
fn empty_repository_has_an_unborn_head_and_no_commits() {
    let fx = Fixture::new();
    let repo = fx.init("empty");

    let history = read(&repo);

    assert_eq!(history.tips.head, Head::Unborn { branch: "main".to_owned() });
    assert!(history.tips.refs.is_empty());
    assert!(history.tips.stashes.is_empty());
    assert!(history.commits.is_empty());
    assert!(!history.tips.worktree_dirty);
}

#[test]
fn linear_history_lists_newest_first() {
    let fx = Fixture::new();
    let repo = fx.init("linear");
    let first = commit(&repo, "a.txt", "1\n", "first", T0);
    let second = commit(&repo, "a.txt", "2\n", "second", T0 + MINUTE);
    let third = commit(&repo, "a.txt", "3\n", "third", T0 + 2 * MINUTE);

    let history = read(&repo);

    assert_eq!(history.tips.head, Head::Branch { name: "main".to_owned(), id: third });
    assert_eq!(history.tips.refs, [(third, branch("main", true))]);
    assert_eq!(rows(&history), [(third, vec![second]), (second, vec![first]), (first, vec![])]);
    assert!(!history.tips.worktree_dirty);
}

#[test]
fn branch_and_merge_are_ordered_topologically_then_by_date() {
    let fx = Fixture::new();
    let repo = fx.init("merge");
    let h = merge_history(&repo);

    let history = read(&repo);

    assert_eq!(
        rows(&history),
        [
            (h.merge, vec![h.main_1, h.feature_2]),
            (h.feature_2, vec![h.feature_1]),
            (h.main_1, vec![h.base]),
            (h.feature_1, vec![h.base]),
            (h.base, vec![]),
        ]
    );
    assert_eq!(history.tips.refs, [(h.feature_2, branch("feature", false)), (h.merge, branch("main", true))]);
}

#[test]
fn tags_peel_and_non_commit_tags_match_no_commit() {
    let fx = Fixture::new();
    let repo = fx.init("tags");
    let first = commit(&repo, "a.txt", "1\n", "first", T0);
    let second = commit(&repo, "a.txt", "2\n", "second", T0 + MINUTE);
    git(&repo, &["tag", "light", first.to_string().as_str()]);
    git(&repo, &["tag", "--annotate", "-m", "Release 1.0", "v1.0"]);
    git(&repo, &["tag", "--annotate", "-m", "A tag of a tag", "nested", "v1.0"]);
    git(&repo, &["tag", "tree-tag", "HEAD^{tree}"]);
    assert_ne!(rev_parse(&repo, "refs/tags/v1.0"), second, "v1.0 is an annotated tag object");
    let tree = rev_parse(&repo, "HEAD^{tree}");

    let history = read(&repo);

    assert_eq!(
        history.tips.refs,
        [
            (second, branch("main", true)),
            (first, tag("light")),
            (second, tag("nested")),
            (tree, tag("tree-tag")),
            (second, tag("v1.0")),
        ]
    );
    assert_eq!(commits(&history), [second, first], "the tree tag adds no commit");
}

#[test]
fn detached_head_gets_a_head_label() {
    let fx = Fixture::new();
    let repo = fx.init("detached");
    let first = commit(&repo, "a.txt", "1\n", "first", T0);
    let second = commit(&repo, "a.txt", "2\n", "second", T0 + MINUTE);
    git(&repo, &["checkout", "--quiet", "--detach", first.to_string().as_str()]);

    let history = read(&repo);

    assert_eq!(history.tips.head, Head::Detached { id: first });
    assert_eq!(
        history.tips.refs,
        [(first, label(RefKind::Head, "HEAD", "HEAD", true)), (second, branch("main", false))]
    );
    assert_eq!(commits(&history), [second, first]);
}

#[test]
fn remote_tracking_branches_are_listed_without_origin_head() {
    let fx = Fixture::new();
    let origin = fx.init_bare("origin.git");
    let work = fx.init("work");
    let first = commit(&work, "a.txt", "1\n", "first", T0);
    git(&work, &["branch", "topic"]);
    let second = commit(&work, "a.txt", "2\n", "second", T0 + MINUTE);
    git(&work, &["push", "--quiet", origin.to_str().unwrap(), "main", "topic"]);
    git(fx.path("").as_path(), &["clone", "--quiet", origin.to_str().unwrap(), "clone"]);
    let clone = fx.path("clone");
    assert_eq!(git(&clone, &["symbolic-ref", "refs/remotes/origin/HEAD"]).trim(), "refs/remotes/origin/main");

    let history = read(&clone);

    let remote = |name: &str| label(RefKind::RemoteBranch, name, &format!("refs/remotes/{name}"), false);
    assert_eq!(
        history.tips.refs,
        [(second, branch("main", true)), (second, remote("origin/main")), (first, remote("origin/topic"))]
    );
    assert_eq!(commits(&history), [second, first]);
}

#[test]
fn stashes_are_listed_newest_first_with_their_base() {
    let fx = Fixture::new();
    let repo = fx.init("stash");
    let base = commit(&repo, "a.txt", "base\n", "base", T0);
    write(&repo, "a.txt", "first change\n");
    git_at(&repo, &["stash", "push", "--quiet", "-m", "first"], T0 + MINUTE);
    write(&repo, "a.txt", "second change\n");
    git_at(&repo, &["stash", "push", "--quiet", "-m", "second"], T0 + 2 * MINUTE);
    let messages = git(&repo, &["log", "--walk-reflogs", "--format=%gs", "refs/stash"]);
    let messages: Vec<&str> = messages.lines().collect();

    let history = read(&repo);

    assert_eq!(history.tips.stashes.len(), 2);
    for (index, stash) in history.tips.stashes.iter().enumerate() {
        assert_eq!(stash.index, index as u32);
        assert_eq!(stash.id, rev_parse(&repo, &format!("stash@{{{index}}}")));
        assert_eq!(stash.base, base);
        assert_eq!(stash.message, messages[index]);
    }
    assert!(history.tips.stashes[0].message.ends_with("second"));
    // Stash commits aren't refs, so only the base is in the graph.
    assert_eq!(commits(&history), [base]);
    assert!(!history.tips.worktree_dirty);
}

#[test]
fn worktree_dirty_tracks_modified_staged_and_untracked_files() {
    let fx = Fixture::new();
    let repo = fx.init("dirty");
    commit(&repo, "a.txt", "committed\n", "initial", T0);
    let dirty = |repo: &Path| read(repo).tips.worktree_dirty;
    assert!(!dirty(&repo), "clean after committing");

    write(&repo, "a.txt", "modified in the worktree\n");
    assert!(dirty(&repo), "modified tracked file");

    git(&repo, &["add", "a.txt"]);
    assert!(dirty(&repo), "staged change, worktree matches the index");

    git(&repo, &["reset", "--quiet", "--hard"]);
    assert!(!dirty(&repo), "clean after reset");

    write(&repo, "new/untracked.txt", "untracked\n");
    assert!(dirty(&repo), "untracked file");

    git(&repo, &["config", "status.showUntrackedFiles", "no"]);
    assert!(!dirty(&repo), "untracked files hidden by status.showUntrackedFiles");
}

#[test]
fn bare_repository_root_is_the_git_dir_and_never_dirty() {
    let fx = Fixture::new();
    let origin = fx.init_bare("bare.git");
    let work = fx.init("work");
    let only = commit(&work, "a.txt", "1\n", "only", T0);
    git(&work, &["push", "--quiet", origin.to_str().unwrap(), "main"]);

    let opened = Repo::open(&origin).unwrap();
    let history = opened.read_history().unwrap();

    let canonical = |path: &Path| std::fs::canonicalize(path).unwrap();
    assert_eq!(canonical(opened.root()), canonical(&origin));
    assert_eq!(history.tips.head, Head::Branch { name: "main".to_owned(), id: only });
    assert_eq!(commits(&history), [only]);
    assert!(!history.tips.worktree_dirty);
}

#[test]
fn clock_skew_never_puts_a_parent_before_its_child() {
    let fx = Fixture::new();
    let repo = fx.init("skew");
    // `root` has the latest date, yet both other commits are its children. A plain sort by
    // committer date would list it first.
    let root = commit(&repo, "a.txt", "root\n", "root", T0 + 30 * MINUTE);
    let old_child = commit(&repo, "a.txt", "old child\n", "dated before its parent", T0 + 10 * MINUTE);
    git(&repo, &["checkout", "--quiet", "-b", "side", root.to_string().as_str()]);
    let side = commit(&repo, "b.txt", "side\n", "side", T0 + 20 * MINUTE);

    let history = read(&repo);

    assert_eq!(commits(&history), [side, old_child, root]);
    assert_children_before_parents(&history);
}

#[test]
fn order_matches_git_log_date_order() {
    let fx = Fixture::new();
    let repo = fx.init("order");
    // Distinct commit times, so git's order is fully determined and tie-breaks can't differ.
    let mut time = T0;
    let mut next = || {
        time += MINUTE;
        time
    };
    commit(&repo, "main.txt", "1\n", "c1", next());
    git(&repo, &["branch", "b"]);
    commit(&repo, "main.txt", "2\n", "c2", next());
    git(&repo, &["branch", "a"]);
    git(&repo, &["checkout", "--quiet", "a"]);
    commit(&repo, "a.txt", "1\n", "a1", next());
    git(&repo, &["checkout", "--quiet", "b"]);
    let b1 = commit(&repo, "b.txt", "1\n", "b1", next());
    git(&repo, &["checkout", "--quiet", "a"]);
    commit(&repo, "a.txt", "2\n", "a2", next());
    git(&repo, &["tag", "a2"]);
    git(&repo, &["checkout", "--quiet", "main"]);
    commit(&repo, "main.txt", "3\n", "c3", next());
    git(&repo, &["checkout", "--quiet", "b"]);
    commit(&repo, "b.txt", "2\n", "b2", next());
    git(&repo, &["checkout", "--quiet", "a"]);
    commit(&repo, "a.txt", "3\n", "a3", next());
    git(&repo, &["checkout", "--quiet", "main"]);
    git_at(&repo, &["merge", "--quiet", "--no-ff", "-m", "merge a", "a"], next());
    git(&repo, &["checkout", "--quiet", "b"]);
    commit(&repo, "b.txt", "3\n", "b3", next());
    git(&repo, &["checkout", "--quiet", "main"]);
    git_at(&repo, &["merge", "--quiet", "--no-ff", "-m", "merge b", "b"], next());
    git(&repo, &["checkout", "--quiet", "-b", "x", b1.to_string().as_str()]);
    commit(&repo, "x.txt", "1\n", "x1", next());
    git(&repo, &["checkout", "--quiet", "main"]);

    let expected: Vec<(Oid, Vec<Oid>)> = git(&repo, &["log", "--date-order", "--all", "--format=%H %P"])
        .lines()
        .map(|line| {
            let mut ids = line.split_whitespace().map(|hex| hex.parse::<Oid>().unwrap());
            (ids.next().unwrap(), ids.collect())
        })
        .collect();

    let history = read(&repo);

    assert_eq!(expected.len(), 12);
    assert_eq!(rows(&history), expected);
}

// ---------------------------------------------------------------------------------------------
// Commit summaries and details

#[test]
fn commit_summaries_keep_order_and_skip_non_commits() {
    let fx = Fixture::new();
    let repo = fx.init("summaries");
    let first = commit(&repo, "a.txt", "1\n", "first", T0);
    write(&repo, "a.txt", "2\n");
    git(&repo, &["add", "a.txt"]);
    let message_file = fx.path("message.txt");
    std::fs::write(&message_file, "Subject line\r\n\r\nBody text\r\n").unwrap();
    git_at(
        &repo,
        &["commit", "--quiet", "--cleanup=verbatim", "-F", message_file.to_str().unwrap()],
        T0 + MINUTE,
    );
    let second = rev_parse(&repo, "HEAD");
    let tree = rev_parse(&repo, "HEAD^{tree}");
    let missing: Oid = "0123456789abcdef0123456789abcdef01234567".parse().unwrap();

    let summaries = Repo::open(&repo).unwrap().commit_summaries(&[second, tree, missing, first]).unwrap();

    assert_eq!(summaries.len(), 4);
    let second_summary = summaries[0].as_ref().expect("a commit");
    assert_eq!(second_summary.summary, "Subject line");
    assert_eq!(second_summary.author_name, AUTHOR_NAME);
    assert_eq!(second_summary.author_email, AUTHOR_EMAIL);
    assert_eq!(second_summary.author_time, T0 + MINUTE);
    assert_eq!(summaries[1], None, "a tree");
    assert_eq!(summaries[2], None, "a missing object");
    assert_eq!(summaries[3].as_ref().expect("a commit").summary, "first");
}

#[test]
fn commit_details_of_a_root_commit() {
    let fx = Fixture::new();
    let repo = fx.init("root");
    write(&repo, "a.txt", "one\ntwo\n");
    write(&repo, "dir/b.txt", "x\n");
    git(&repo, &["add", "."]);
    git_dated(&repo, &["commit", "--quiet", "-m", "Root commit\n\nWith a body."], &format!("{T0} +0200"));
    let root = rev_parse(&repo, "HEAD");

    let details = Repo::open(&repo).unwrap().commit_details(root).unwrap().expect("a commit");

    assert_eq!(details.id, root);
    assert!(details.parents.is_empty());
    assert_eq!(details.author.name, AUTHOR_NAME);
    assert_eq!(details.author.email, AUTHOR_EMAIL);
    assert_eq!(details.author.time, T0);
    assert_eq!(details.author.offset_minutes, 120);
    assert_eq!(details.committer.name, "Carl Committer");
    assert_eq!(details.message, "Root commit\n\nWith a body.\n");
    assert_eq!(
        details.files,
        [file("a.txt", None, ChangeStatus::Added, Some((2, 0))), file("dir/b.txt", None, ChangeStatus::Added, Some((1, 0)))]
    );
}

#[test]
fn commit_details_detects_renames_modifications_and_deletions() {
    let fx = Fixture::new();
    let repo = fx.init("rename");
    let lines: String = (1..=10).map(|n| format!("line {n}\n")).collect();
    write(&repo, "old.txt", &lines);
    write(&repo, "keep.txt", "keep\n");
    write(&repo, "gone.txt", "bye\n");
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "initial"]);
    git(&repo, &["mv", "old.txt", "new.txt"]);
    write(&repo, "new.txt", lines.replace("line 5\n", "line five\n"));
    write(&repo, "keep.txt", "kept\n");
    git(&repo, &["rm", "--quiet", "gone.txt"]);
    let changed = {
        git(&repo, &["add", "."]);
        git_at(&repo, &["commit", "--quiet", "-m", "rename"], T0 + MINUTE);
        rev_parse(&repo, "HEAD")
    };

    let details = Repo::open(&repo).unwrap().commit_details(changed).unwrap().expect("a commit");

    assert_eq!(
        details.files,
        [
            file("gone.txt", None, ChangeStatus::Deleted, Some((0, 1))),
            file("keep.txt", None, ChangeStatus::Modified, Some((1, 1))),
            file("new.txt", Some("old.txt"), ChangeStatus::Renamed, Some((1, 1))),
        ]
    );
}

#[test]
fn commit_details_has_no_line_counts_for_binary_files() {
    let fx = Fixture::new();
    let repo = fx.init("binary");
    write(&repo, "image.bin", b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR");
    write(&repo, "text.txt", "hello\n");
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "binary"]);
    let id = rev_parse(&repo, "HEAD");

    let details = Repo::open(&repo).unwrap().commit_details(id).unwrap().expect("a commit");

    assert_eq!(
        details.files,
        [file("image.bin", None, ChangeStatus::Added, None), file("text.txt", None, ChangeStatus::Added, Some((1, 0)))]
    );
}

#[test]
fn commit_details_of_a_merge_lists_all_parents_and_diffs_against_the_first() {
    let fx = Fixture::new();
    let repo = fx.init("merge-details");
    let h = merge_history(&repo);
    let opened = Repo::open(&repo).unwrap();

    let details = opened.commit_details(h.merge).unwrap().expect("a commit");

    assert_eq!(details.parents, [h.main_1, h.feature_2]);
    assert_eq!(details.files, [file("feature.txt", None, ChangeStatus::Added, Some((2, 0)))]);
    assert_eq!(details.message, "Merge feature\n");
    let tree = rev_parse(&repo, "HEAD^{tree}");
    assert_eq!(opened.commit_details(tree).unwrap(), None, "a tree is not a commit");
}

fn file(path: &str, old_path: Option<&str>, status: ChangeStatus, lines: Option<(u32, u32)>) -> FileChange {
    FileChange {
        path: path.to_owned(),
        old_path: old_path.map(str::to_owned),
        status,
        additions: lines.map(|(added, _)| added),
        deletions: lines.map(|(_, deleted)| deleted),
    }
}

// ---------------------------------------------------------------------------------------------
// Security

/// A repository's configuration and attributes can name programs. Reading must not run them.
#[test]
fn reads_do_not_run_programs_named_by_the_repository() {
    let fx = Fixture::new();
    let repo = fx.init("hostile");
    write(&repo, ".gitattributes", "* filter=evil diff=evil\n");
    write(&repo, "a.txt", "same size\n");
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "initial"]);
    write(&repo, "a.txt", "same size\n");
    git_at(&repo, &["commit", "--quiet", "--allow-empty", "-m", "second"], T0 + MINUTE);
    let head = rev_parse(&repo, "HEAD");
    let root = rev_parse(&repo, "HEAD~1");

    // Configure the programs last: git itself would run them from here on.
    let marker = |name: &str| fx.path(name);
    let touch = |name: &str| format!("echo ran > \"{}\"", marker(name).to_str().unwrap().replace('\\', "/"));
    let config = repo.join(".git").join("config");
    let set = |key: &str, value: &str| git(fx.path("").as_path(), &["config", "--file", config.to_str().unwrap(), key, value]);
    set("filter.evil.clean", &touch("clean-filter-ran"));
    set("filter.evil.smudge", &touch("smudge-filter-ran"));
    set("filter.evil.required", "true");
    set("diff.evil.textconv", &touch("textconv-ran"));
    set("diff.external", &touch("external-diff-ran"));
    set("core.fsmonitor", &touch("fsmonitor-ran"));
    // Stat data that no longer matches the index makes status hash the file, through the clean
    // filter if one were configured.
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(120);
    std::fs::File::options().write(true).open(repo.join("a.txt")).unwrap().set_modified(later).unwrap();

    let opened = Repo::open(&repo).unwrap();
    let history = opened.read_history().unwrap();
    opened.commit_details(root).unwrap().expect("a commit");
    opened.commit_details(head).unwrap().expect("a commit");

    for name in ["clean-filter-ran", "smudge-filter-ran", "textconv-ran", "external-diff-ran", "fsmonitor-ran"] {
        assert!(!marker(name).exists(), "{name}: a configured program was run");
    }
    // Also catches a clean filter that ran without leaving a marker: its output differs from a.txt.
    assert!(!history.tips.worktree_dirty, "a.txt's content matches the index");
}
