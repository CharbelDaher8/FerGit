//! Integration tests for `Repo::changes` and `Repo::file_diff`, checked against the git CLI where
//! git can show the same comparison.

mod common;

use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use common::{Fixture, T0, commit, git, git_at, git_succeeds, rev_parse, write};
use fergit_core::repo::{Repo, RepoError};
use fergit_core::{ChangeStatus, DiffLine, DiffSide, FileChange, FileDiff, Hunk, LineKind, Oid};

const MINUTE: i64 = 60;
const INDEX: DiffSide = DiffSide::Index;
const WORKTREE: DiffSide = DiffSide::Worktree;

fn at(id: Oid) -> DiffSide {
    DiffSide::Commit { id }
}

fn open(repo: &Path) -> Repo {
    Repo::open(repo).expect("open repository")
}

fn change(path: &str, old_path: Option<&str>, status: ChangeStatus, lines: Option<(u32, u32)>) -> FileChange {
    FileChange {
        path: path.to_owned(),
        old_path: old_path.map(str::to_owned),
        status,
        additions: lines.map(|(added, _)| added),
        deletions: lines.map(|(_, deleted)| deleted),
    }
}

fn hunks_of(diff: FileDiff) -> Vec<Hunk> {
    match diff {
        FileDiff::Text { hunks } => hunks,
        other => panic!("expected a text diff, got {other:?}"),
    }
}

fn text_diff(repo: &Path, from: Option<DiffSide>, to: DiffSide, path: &str, old_path: Option<&str>) -> Vec<Hunk> {
    hunks_of(open(repo).file_diff(from, to, path, old_path).expect("file diff"))
}

/// The hunks `git diff -U3 <args>` prints for a single file.
fn git_hunks(repo: &Path, args: &[&str]) -> Vec<Hunk> {
    let mut full = vec!["diff", "--no-color", "--no-ext-diff", "--no-textconv", "-U3"];
    full.extend_from_slice(args);
    parse_hunks(&git(repo, &full))
}

fn parse_hunks(output: &str) -> Vec<Hunk> {
    let mut hunks = Vec::new();
    let mut lines = output.split('\n').peekable();
    while let Some(line) = lines.next() {
        let Some(header) = line.strip_prefix("@@ -") else {
            continue;
        };
        let (ranges, _) = header.split_once(" @@").expect("hunk header");
        let (old, new) = ranges.split_once(" +").expect("hunk ranges");
        let range = |range: &str| match range.split_once(',') {
            Some((start, count)) => (start.parse::<u32>().unwrap(), count.parse::<u32>().unwrap()),
            None => (range.parse().unwrap(), 1),
        };
        let ((old_start, old_lines), (new_start, new_lines)) = (range(old), range(new));
        let (mut old_left, mut new_left) = (old_lines, new_lines);
        let mut body: Vec<DiffLine> = Vec::new();
        while old_left > 0 || new_left > 0 || lines.peek().is_some_and(|line| line.starts_with('\\')) {
            let line = lines.next().expect("hunk line");
            if line.starts_with('\\') {
                body.last_mut().expect("a line before the marker").no_final_newline = true;
                continue;
            }
            let (prefix, text) = line.split_at(1);
            let kind = match prefix {
                " " => {
                    old_left -= 1;
                    new_left -= 1;
                    LineKind::Context
                }
                "-" => {
                    old_left -= 1;
                    LineKind::Removed
                }
                "+" => {
                    new_left -= 1;
                    LineKind::Added
                }
                other => panic!("unexpected hunk line prefix {other:?}"),
            };
            let text = text.strip_suffix('\r').unwrap_or(text);
            body.push(DiffLine { kind, text: text.to_owned(), no_final_newline: false });
        }
        hunks.push(Hunk { old_start, old_lines, new_start, new_lines, lines: body });
    }
    hunks
}

fn numbered_lines(count: usize) -> String {
    (1..=count).map(|n| format!("line {n}\n")).collect()
}

// ---------------------------------------------------------------------------------------------
// file_diff against `git diff`

#[test]
fn a_multi_hunk_modification_matches_git_on_every_side() {
    let fx = Fixture::new();
    let repo = fx.init("hunks");
    let original = numbered_lines(40);
    let first = commit(&repo, "f.txt", &original, "original", T0);
    // Lines 3 and 10 are six unchanged lines apart (one hunk); 25 and the last line are further.
    let committed = original
        .replace("line 3\n", "line three\n")
        .replace("line 10\n", "line ten\n")
        .replace("line 25\n", "")
        .replace("line 40\n", "line 40\nline 41\nline 42\n");
    let second = commit(&repo, "f.txt", &committed, "second", T0 + MINUTE);
    let code_before = "fn a() {\n    one();\n}\n\nfn c() {\n    three();\n}\n";
    let code_after = "fn a() {\n    one();\n}\n\nfn b() {\n    two();\n}\n\nfn c() {\n    three();\n}\n";
    let code_first = commit(&repo, "code.rs", code_before, "code", T0 + 2 * MINUTE);
    let code_second = commit(&repo, "code.rs", code_after, "more code", T0 + 3 * MINUTE);

    let (a, b) = (first.to_string(), second.to_string());
    assert_eq!(text_diff(&repo, Some(at(first)), at(second), "f.txt", None), git_hunks(&repo, &[&a, &b, "--", "f.txt"]));
    assert_eq!(text_diff(&repo, Some(at(second)), at(first), "f.txt", None), git_hunks(&repo, &[&b, &a, "--", "f.txt"]));
    let (c, d) = (code_first.to_string(), code_second.to_string());
    assert_eq!(
        text_diff(&repo, Some(at(code_first)), at(code_second), "code.rs", None),
        git_hunks(&repo, &[&c, &d, "--", "code.rs"]),
        "ambiguous insertions slide like git's indent heuristic"
    );

    let head = rev_parse(&repo, "HEAD");
    let staged = committed.replace("line 20\n", "line twenty\n");
    write(&repo, "f.txt", &staged);
    git(&repo, &["add", "f.txt"]);
    write(&repo, "f.txt", staged.replace("line 30\n", "line thirty\nextra\n").replace("line 1\n", ""));
    let head_hex = head.to_string();
    assert_eq!(text_diff(&repo, Some(at(head)), INDEX, "f.txt", None), git_hunks(&repo, &["--cached", &head_hex, "--", "f.txt"]));
    assert_eq!(text_diff(&repo, Some(INDEX), WORKTREE, "f.txt", None), git_hunks(&repo, &["--", "f.txt"]));
    assert_eq!(text_diff(&repo, Some(at(head)), WORKTREE, "f.txt", None), git_hunks(&repo, &[&head_hex, "--", "f.txt"]));
    assert!(git_hunks(&repo, &["--", "f.txt"]).len() >= 2);
}

#[test]
fn added_and_deleted_files_diff_against_nothing() {
    let fx = Fixture::new();
    let repo = fx.init("added");
    write(&repo, "old.txt", "gone\nsoon\n");
    write(&repo, "keep.txt", "keep\n");
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "first"]);
    let first = rev_parse(&repo, "HEAD");
    git(&repo, &["rm", "--quiet", "old.txt"]);
    write(&repo, "new.txt", "brand\nnew\nfile\n");
    git(&repo, &["add", "new.txt"]);
    git_at(&repo, &["commit", "--quiet", "-m", "second"], T0 + MINUTE);
    let second = rev_parse(&repo, "HEAD");

    let (a, b) = (first.to_string(), second.to_string());
    let added = text_diff(&repo, Some(at(first)), at(second), "new.txt", None);
    assert_eq!(added, git_hunks(&repo, &[&a, &b, "--", "new.txt"]));
    assert_eq!((added[0].old_start, added[0].old_lines, added[0].new_start, added[0].new_lines), (0, 0, 1, 3));
    let deleted = text_diff(&repo, Some(at(first)), at(second), "old.txt", None);
    assert_eq!(deleted, git_hunks(&repo, &[&a, &b, "--", "old.txt"]));
    assert_eq!((deleted[0].new_start, deleted[0].new_lines), (0, 0));

    let empty_tree = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
    assert_eq!(text_diff(&repo, None, at(first), "old.txt", None), git_hunks(&repo, &[empty_tree, &a, "--", "old.txt"]));
    assert_eq!(text_diff(&repo, Some(at(first)), at(second), "never-existed.txt", None), []);
}

#[test]
fn a_rename_diffs_the_old_path_against_the_new_one() {
    let fx = Fixture::new();
    let repo = fx.init("rename");
    let lines = numbered_lines(12);
    let first = commit(&repo, "a.txt", &lines, "first", T0);
    git(&repo, &["mv", "a.txt", "b.txt"]);
    write(&repo, "b.txt", lines.replace("line 6\n", "line six\n"));
    git(&repo, &["add", "b.txt"]);
    git_at(&repo, &["commit", "--quiet", "-m", "rename"], T0 + MINUTE);
    let second = rev_parse(&repo, "HEAD");
    let opened = open(&repo);

    let changes = opened.changes(Some(at(first)), at(second)).unwrap();
    assert_eq!(changes, [change("b.txt", Some("a.txt"), ChangeStatus::Renamed, Some((1, 1)))]);
    let (a, b) = (first.to_string(), second.to_string());
    assert_eq!(
        text_diff(&repo, Some(at(first)), at(second), "b.txt", Some("a.txt")),
        git_hunks(&repo, &["-M", &a, &b, "--", "a.txt", "b.txt"])
    );
    // Seen backwards, the rename goes the other way.
    let backwards = opened.changes(Some(at(second)), at(first)).unwrap();
    assert_eq!(backwards, [change("a.txt", Some("b.txt"), ChangeStatus::Renamed, Some((1, 1)))]);
}

#[test]
fn a_missing_final_newline_is_marked_on_either_side() {
    let fx = Fixture::new();
    let repo = fx.init("newline");
    write(&repo, "gains.txt", "a\nb\nc");
    write(&repo, "loses.txt", "a\nb\nc\n");
    write(&repo, "neither.txt", "a\nb\nc");
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "first"]);
    let first = rev_parse(&repo, "HEAD");
    write(&repo, "gains.txt", "a\nb\nc\n");
    write(&repo, "loses.txt", "a\nb\nc");
    write(&repo, "neither.txt", "a\nB\nc");
    git(&repo, &["add", "."]);
    git_at(&repo, &["commit", "--quiet", "-m", "second"], T0 + MINUTE);
    let second = rev_parse(&repo, "HEAD");

    let (a, b) = (first.to_string(), second.to_string());
    for file in ["gains.txt", "loses.txt", "neither.txt"] {
        assert_eq!(text_diff(&repo, Some(at(first)), at(second), file, None), git_hunks(&repo, &[&a, &b, "--", file]), "{file}");
    }
    let flagged = |file: &str| -> Vec<(LineKind, bool)> {
        let hunks = text_diff(&repo, Some(at(first)), at(second), file, None);
        hunks[0].lines.iter().map(|line| (line.kind, line.no_final_newline)).filter(|(_, flag)| *flag).collect()
    };
    assert_eq!(flagged("gains.txt"), [(LineKind::Removed, true)]);
    assert_eq!(flagged("loses.txt"), [(LineKind::Added, true)]);
    assert_eq!(flagged("neither.txt"), [(LineKind::Context, true)]);
}

// ---------------------------------------------------------------------------------------------
// Binary, too large, submodule, line endings

#[test]
fn binary_and_too_large_files_have_no_line_diff() {
    let fx = Fixture::new();
    let repo = fx.init("binary");
    write(&repo, "image.bin", b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR");
    let big = "a line of text\n".repeat(9 * 1024 * 1024 / 15 + 1);
    write(&repo, "big.txt", &big);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "first"]);
    let first = rev_parse(&repo, "HEAD");
    let opened = open(&repo);

    assert_eq!(opened.file_diff(None, at(first), "image.bin", None).unwrap(), FileDiff::Binary);
    assert_eq!(opened.file_diff(None, at(first), "big.txt", None).unwrap(), FileDiff::TooLarge);
    assert_eq!(
        opened.changes(None, at(first)).unwrap(),
        [change("big.txt", None, ChangeStatus::Added, None), change("image.bin", None, ChangeStatus::Added, None)]
    );

    write(&repo, "image.bin", b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR changed");
    write(&repo, "big.txt", format!("{big}one more\n"));
    assert_eq!(opened.file_diff(Some(INDEX), WORKTREE, "image.bin", None).unwrap(), FileDiff::Binary);
    assert_eq!(opened.file_diff(Some(INDEX), WORKTREE, "big.txt", None).unwrap(), FileDiff::TooLarge);
    assert_eq!(
        opened.changes(Some(INDEX), WORKTREE).unwrap(),
        [change("big.txt", None, ChangeStatus::Modified, None), change("image.bin", None, ChangeStatus::Modified, None)]
    );
}

/// A submodule is a commit id on every side; on the worktree side, the commit checked out in it.
#[test]
fn a_submodule_shows_the_commits_it_points_to() {
    let fx = Fixture::new();
    let library = fx.init("library");
    let library_first = commit(&library, "lib.txt", "v1\n", "library v1", T0);
    let repo = fx.init("super");
    commit(&repo, "readme.txt", "hi\n", "readme", T0);
    let library_url = library.to_str().unwrap().replace('\\', "/");
    git(&repo, &["-c", "protocol.file.allow=always", "submodule", "add", "--quiet", &library_url, "lib"]);
    git_at(&repo, &["commit", "--quiet", "-m", "add submodule"], T0 + MINUTE);
    let with_submodule = rev_parse(&repo, "HEAD");
    let opened = open(&repo);

    let added = opened.changes(Some(at(rev_parse(&repo, "HEAD~1"))), at(with_submodule)).unwrap();
    assert_eq!(
        added,
        [change(".gitmodules", None, ChangeStatus::Added, Some((3, 0))), change("lib", None, ChangeStatus::Added, None)]
    );
    assert_eq!(
        opened.file_diff(None, at(with_submodule), "lib", None).unwrap(),
        FileDiff::Submodule { old: None, new: Some(library_first) }
    );
    assert_eq!(opened.changes(Some(INDEX), WORKTREE).unwrap(), [], "checked out at the recorded commit");

    let checkout = repo.join("lib");
    let library_second = commit(&checkout, "lib.txt", "v2\n", "library v2", T0 + 2 * MINUTE);
    assert_eq!(opened.changes(Some(INDEX), WORKTREE).unwrap(), [change("lib", None, ChangeStatus::Modified, None)]);
    assert_eq!(
        opened.file_diff(Some(INDEX), WORKTREE, "lib", None).unwrap(),
        FileDiff::Submodule { old: Some(library_first), new: Some(library_second) }
    );
    assert_eq!(
        opened.changes(Some(at(with_submodule)), WORKTREE).unwrap(),
        [change("lib", None, ChangeStatus::Modified, None)]
    );
}

#[test]
fn line_ending_only_differences_are_not_changes_with_autocrlf() {
    let fx = Fixture::new();
    let repo = fx.init("crlf");
    let head = commit(&repo, "f.txt", "one\ntwo\nthree\n", "lf", T0);
    git(&repo, &["config", "core.autocrlf", "true"]);
    write(&repo, "f.txt", "one\r\ntwo\r\nthree\r\n");
    let opened = open(&repo);

    assert_eq!(opened.changes(Some(INDEX), WORKTREE).unwrap(), []);
    assert_eq!(opened.changes(Some(at(head)), WORKTREE).unwrap(), []);
    assert_eq!(opened.file_diff(Some(INDEX), WORKTREE, "f.txt", None).unwrap(), FileDiff::Text { hunks: vec![] });

    write(&repo, "f.txt", "one\r\n2\r\nthree\r\n");
    assert_eq!(opened.changes(Some(INDEX), WORKTREE).unwrap(), [change("f.txt", None, ChangeStatus::Modified, Some((1, 1)))]);
    let hunks = hunks_of(opened.file_diff(Some(INDEX), WORKTREE, "f.txt", None).unwrap());
    let texts: Vec<&str> = hunks[0].lines.iter().map(|line| line.text.as_str()).collect();
    assert_eq!(texts, ["one", "two", "2", "three"]);
}

// ---------------------------------------------------------------------------------------------
// changes between sides

#[test]
fn changes_from_nothing_to_a_commit_list_every_file_as_added() {
    let fx = Fixture::new();
    let repo = fx.init("from-nothing");
    write(&repo, "a.txt", "1\n");
    write(&repo, "dir/b.txt", "1\n2\n");
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "first"]);
    let head = rev_parse(&repo, "HEAD");

    assert_eq!(
        open(&repo).changes(None, at(head)).unwrap(),
        [change("a.txt", None, ChangeStatus::Added, Some((1, 0))), change("dir/b.txt", None, ChangeStatus::Added, Some((2, 0)))]
    );
}

#[test]
fn changes_between_commits_equal_the_files_of_commit_details() {
    let fx = Fixture::new();
    let repo = fx.init("between");
    write(&repo, "rename-me.txt", numbered_lines(10));
    write(&repo, "modify.txt", "a\nb\n");
    write(&repo, "delete.txt", "bye\n");
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "first"]);
    let parent = rev_parse(&repo, "HEAD");
    git(&repo, &["mv", "rename-me.txt", "renamed.txt"]);
    write(&repo, "modify.txt", "a\nB\nc\n");
    git(&repo, &["rm", "--quiet", "delete.txt"]);
    write(&repo, "add.txt", "new\n");
    git(&repo, &["add", "."]);
    git_at(&repo, &["commit", "--quiet", "-m", "second"], T0 + MINUTE);
    let child = rev_parse(&repo, "HEAD");
    let opened = open(&repo);

    let details = opened.commit_details(child).unwrap().expect("a commit").files;
    assert_eq!(opened.changes(Some(at(parent)), at(child)).unwrap(), details);
    assert_eq!(
        details,
        [
            change("add.txt", None, ChangeStatus::Added, Some((1, 0))),
            change("delete.txt", None, ChangeStatus::Deleted, Some((0, 1))),
            change("modify.txt", None, ChangeStatus::Modified, Some((2, 1))),
            change("renamed.txt", Some("rename-me.txt"), ChangeStatus::Renamed, Some((0, 0))),
        ]
    );
    assert_eq!(opened.changes(Some(at(child)), at(child)).unwrap(), []);
}

#[test]
fn changes_from_a_commit_to_the_index_list_staged_changes_only() {
    let fx = Fixture::new();
    let repo = fx.init("staged");
    write(&repo, "a.txt", "a\n");
    write(&repo, "b.txt", numbered_lines(10));
    write(&repo, "c.txt", "c\n");
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "first"]);
    let head = rev_parse(&repo, "HEAD");
    write(&repo, "a.txt", "a\nstaged\n");
    git(&repo, &["add", "a.txt"]);
    git(&repo, &["mv", "b.txt", "moved.txt"]);
    git(&repo, &["rm", "--quiet", "c.txt"]);
    write(&repo, "new.txt", "new\n");
    git(&repo, &["add", "new.txt"]);
    // Neither of these is staged.
    write(&repo, "a.txt", "a\nstaged\nand more on disk\n");
    write(&repo, "untracked.txt", "u\n");
    let opened = open(&repo);

    let staged = [
        change("a.txt", None, ChangeStatus::Modified, Some((1, 0))),
        change("c.txt", None, ChangeStatus::Deleted, Some((0, 1))),
        change("moved.txt", Some("b.txt"), ChangeStatus::Renamed, Some((0, 0))),
        change("new.txt", None, ChangeStatus::Added, Some((1, 0))),
    ];
    assert_eq!(opened.changes(Some(at(head)), INDEX).unwrap(), staged);
    assert_eq!(
        opened.changes(Some(INDEX), at(head)).unwrap(),
        [
            change("a.txt", None, ChangeStatus::Modified, Some((0, 1))),
            change("b.txt", Some("moved.txt"), ChangeStatus::Renamed, Some((0, 0))),
            change("c.txt", None, ChangeStatus::Added, Some((1, 0))),
            change("new.txt", None, ChangeStatus::Deleted, Some((0, 1))),
        ]
    );
    assert_eq!(opened.changes(Some(INDEX), INDEX).unwrap(), []);
}

#[test]
fn changes_from_the_index_to_the_worktree_list_unstaged_and_untracked_files() {
    let fx = Fixture::new();
    let repo = fx.init("unstaged");
    write(&repo, ".gitignore", "*.log\n");
    write(&repo, "a.txt", "a\n");
    write(&repo, "b.txt", "b\n");
    write(&repo, "staged.txt", "s\n");
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "first"]);
    write(&repo, "staged.txt", "s\nstaged\n");
    git(&repo, &["add", "staged.txt"]);
    write(&repo, "a.txt", "A\n");
    std::fs::remove_file(repo.join("b.txt")).unwrap();
    write(&repo, "new/deep/untracked.txt", "u\nv\n");
    write(&repo, "untracked.txt", "u\n");
    write(&repo, "debug.log", "ignored\n");
    let opened = open(&repo);

    let unstaged = [change("a.txt", None, ChangeStatus::Modified, Some((1, 1))), change("b.txt", None, ChangeStatus::Deleted, Some((0, 1)))];
    let untracked = [
        change("new/deep/untracked.txt", None, ChangeStatus::Added, Some((2, 0))),
        change("untracked.txt", None, ChangeStatus::Added, Some((1, 0))),
    ];
    let everything: Vec<_> = unstaged.iter().chain(&untracked).cloned().collect::<Vec<_>>();
    let mut everything = everything;
    everything.sort_by(|a, b| a.path.cmp(&b.path));
    assert_eq!(opened.changes(Some(INDEX), WORKTREE).unwrap(), everything);
    assert_eq!(
        text_diff(&repo, Some(INDEX), WORKTREE, "new/deep/untracked.txt", None),
        text_diff(&repo, None, WORKTREE, "new/deep/untracked.txt", None)
    );
    let backwards = opened.changes(Some(WORKTREE), INDEX).unwrap();
    assert_eq!(backwards[0], change("a.txt", None, ChangeStatus::Modified, Some((1, 1))));
    assert_eq!(backwards[1], change("b.txt", None, ChangeStatus::Added, Some((1, 0))));

    git(&repo, &["config", "status.showUntrackedFiles", "no"]);
    assert_eq!(open(&repo).changes(Some(INDEX), WORKTREE).unwrap(), unstaged);
}

#[test]
fn changes_from_a_commit_to_the_worktree_combine_staged_and_unstaged_changes() {
    let fx = Fixture::new();
    let repo = fx.init("combined");
    write(&repo, "reverted.txt", "r\n");
    write(&repo, "rename-me.txt", numbered_lines(10));
    write(&repo, "twice.txt", "1\n2\n3\n");
    write(&repo, "deleted.txt", "d\n");
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "first"]);
    let head = rev_parse(&repo, "HEAD");

    // Staged, then undone on disk: no change from the commit.
    write(&repo, "reverted.txt", "r\nstaged\n");
    git(&repo, &["add", "reverted.txt"]);
    write(&repo, "reverted.txt", "r\n");
    // A staged rename, then edited on disk.
    git(&repo, &["mv", "rename-me.txt", "renamed.txt"]);
    write(&repo, "renamed.txt", numbered_lines(10).replace("line 2\n", "line two\n"));
    // Changed in the index and again on disk.
    write(&repo, "twice.txt", "1\ntwo\n3\n");
    git(&repo, &["add", "twice.txt"]);
    write(&repo, "twice.txt", "1\ntwo\nthree\n4\n");
    // Deleted on disk only.
    std::fs::remove_file(repo.join("deleted.txt")).unwrap();
    // Staged as new, then deleted from disk: nothing.
    write(&repo, "fleeting.txt", "f\n");
    git(&repo, &["add", "fleeting.txt"]);
    std::fs::remove_file(repo.join("fleeting.txt")).unwrap();
    write(&repo, "untracked.txt", "u\n");
    let opened = open(&repo);

    let expected = [
        change("deleted.txt", None, ChangeStatus::Deleted, Some((0, 1))),
        change("renamed.txt", Some("rename-me.txt"), ChangeStatus::Renamed, Some((1, 1))),
        change("twice.txt", None, ChangeStatus::Modified, Some((3, 2))),
        change("untracked.txt", None, ChangeStatus::Added, Some((1, 0))),
    ];
    assert_eq!(opened.changes(Some(at(head)), WORKTREE).unwrap(), expected);
    let head_hex = head.to_string();
    assert_eq!(text_diff(&repo, Some(at(head)), WORKTREE, "twice.txt", None), git_hunks(&repo, &[&head_hex, "--", "twice.txt"]));
    assert_eq!(
        text_diff(&repo, Some(at(head)), WORKTREE, "renamed.txt", Some("rename-me.txt")),
        git_hunks(&repo, &["-M", &head_hex, "--", "rename-me.txt", "renamed.txt"])
    );
    let backwards = opened.changes(Some(WORKTREE), at(head)).unwrap();
    assert_eq!(backwards.len(), expected.len());
    assert!(backwards.contains(&change("rename-me.txt", Some("renamed.txt"), ChangeStatus::Renamed, Some((1, 1)))));
}

/// During a merge conflict the index side holds "ours" (stage 2) for the unmerged path.
#[test]
fn an_unmerged_path_shows_our_version_on_the_index_side() {
    let fx = Fixture::new();
    let repo = fx.init("conflict");
    write(&repo, "conflict.txt", "base\n");
    write(&repo, "clean.txt", "clean\n");
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "base"]);
    git(&repo, &["checkout", "--quiet", "-b", "other"]);
    write(&repo, "conflict.txt", "theirs\n");
    write(&repo, "clean.txt", "clean\nfrom other\n");
    git(&repo, &["commit", "--quiet", "-am", "theirs"]);
    git(&repo, &["checkout", "--quiet", "main"]);
    write(&repo, "conflict.txt", "ours\n");
    git_at(&repo, &["commit", "--quiet", "-am", "ours"], T0 + MINUTE);
    let head = rev_parse(&repo, "HEAD");
    assert!(!git_succeeds(&repo, &["merge", "--quiet", "--no-edit", "other"]), "the merge must stop with a conflict");
    assert!(git(&repo, &["ls-files", "--unmerged"]).contains("conflict.txt"));
    let opened = open(&repo);

    assert_eq!(
        opened.changes(Some(at(head)), INDEX).unwrap(),
        [change("clean.txt", None, ChangeStatus::Modified, Some((1, 0)))],
        "the merged file is staged; the unmerged one matches HEAD"
    );
    assert_eq!(opened.file_diff(Some(at(head)), INDEX, "conflict.txt", None).unwrap(), FileDiff::Text { hunks: vec![] });
    assert_eq!(opened.changes(Some(INDEX), WORKTREE).unwrap(), [change("conflict.txt", None, ChangeStatus::Modified, Some((4, 0)))]);
    let hunks = text_diff(&repo, Some(INDEX), WORKTREE, "conflict.txt", None);
    let added: Vec<&str> = hunks[0].lines.iter().filter(|line| line.kind == LineKind::Added).map(|line| line.text.as_str()).collect();
    assert_eq!(added, ["<<<<<<< HEAD", "=======", "theirs", ">>>>>>> other"]);
}

#[test]
fn commit_sides_must_name_commits() {
    let fx = Fixture::new();
    let repo = fx.init("not-a-commit");
    commit(&repo, "a.txt", "a\n", "first", T0);
    let tree = rev_parse(&repo, "HEAD^{tree}");
    let missing: Oid = "0123456789abcdef0123456789abcdef01234567".parse().unwrap();
    let opened = open(&repo);

    for id in [tree, missing] {
        let is_not_a_commit = |err: RepoError| matches!(&err, RepoError::Git(message) if message.contains(&id.to_string()) && message.contains("not a commit"));
        assert!(is_not_a_commit(opened.changes(Some(at(id)), INDEX).unwrap_err()));
        assert!(is_not_a_commit(opened.changes(None, at(id)).unwrap_err()));
        assert!(is_not_a_commit(opened.file_diff(Some(at(id)), WORKTREE, "a.txt", None).unwrap_err()));
    }
}

// ---------------------------------------------------------------------------------------------
// Performance

/// Run with `cargo test -p fergit-core --release --test diff -- --ignored --nocapture`.
#[test]
#[ignore = "a benchmark; builds a 20,000-file worktree"]
fn worktree_changes_on_a_large_worktree_are_stat_based() {
    let fx = Fixture::new();
    let repo = fx.init("large");
    let started = Instant::now();
    // Stat data older than the index keeps git and gix from treating the files as racily clean.
    let past = SystemTime::now() - Duration::from_secs(3600);
    for dir in 0..200 {
        for file in 0..100 {
            let path = repo.join(format!("dir{dir}/file{file}.txt"));
            write(&repo, &format!("dir{dir}/file{file}.txt"), format!("directory {dir}\nfile {file}\n"));
            std::fs::File::options().write(true).open(&path).unwrap().set_modified(past).unwrap();
        }
    }
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "20k files"]);
    let head = rev_parse(&repo, "HEAD");
    for n in 0..20 {
        write(&repo, &format!("dir{n}/file{n}.txt"), format!("changed {n}\n"));
        write(&repo, &format!("untracked/new{n}.txt"), "new\n");
    }
    println!("setup: {:.1} s", started.elapsed().as_secs_f64());

    let opened = open(&repo);
    let time = |label: &str, run: &dyn Fn() -> usize| {
        for attempt in 1..=3 {
            let started = Instant::now();
            let count = run();
            println!("{label} #{attempt}: {:.1} ms, {count} changes", started.elapsed().as_secs_f64() * 1e3);
            assert_eq!(count, 40);
        }
    };
    time("changes(Index, Worktree)", &|| opened.changes(Some(INDEX), WORKTREE).unwrap().len());
    time("changes(Commit, Worktree)", &|| opened.changes(Some(at(head)), WORKTREE).unwrap().len());
    let started = Instant::now();
    let porcelain = git(&repo, &["status", "--porcelain", "--untracked-files=all"]);
    println!("git status --porcelain: {:.1} ms, {} lines", started.elapsed().as_secs_f64() * 1e3, porcelain.lines().count());
}
