//! Finding commits and filtering the history, over repositories built with the git CLI.

mod common;

use std::path::Path;

use common::{Fixture, T0, commit, git, git_at, rev_parse, write};
use fergit_core::session::Session;
use fergit_core::{Filter, Half, Oid, Row, RowKind};

const MINUTE: i64 = 60;

fn all_rows(session: &Session) -> Vec<Row> {
    session.rows(0, session.info().row_count).unwrap().rows
}

fn ids(session: &Session) -> Vec<Oid> {
    all_rows(session).into_iter().map(|row| row.id).collect()
}

/// The ids of the rows `query` finds.
fn found(session: &Session, query: &str) -> Vec<Oid> {
    let result = session.search(query).unwrap();
    assert_eq!(result.generation, session.info().generation);
    assert_eq!(result.total as usize, result.rows.len());
    let rows = all_rows(session);
    result.rows.iter().map(|&row| rows[row as usize].id).collect()
}

/// Commits `file` as Bob rather than the default author.
fn commit_as_bob(repo: &Path, file: &str, message: &str, time: i64) -> Oid {
    write(repo, file, message);
    git(repo, &["add", "--", file]);
    git_at(repo, &["commit", "--quiet", "--author", "Bob Builder <bob@EXAMPLE.org>", "-m", message], time);
    rev_parse(repo, "HEAD")
}

fn refs_filter(refs: &[&str]) -> Filter {
    Filter { refs: refs.iter().map(|name| name.to_string()).collect(), path: None }
}

fn path_filter(path: &str) -> Filter {
    Filter { refs: Vec::new(), path: Some(path.to_owned()) }
}

/// Lanes leaving each row's node downwards: how many parents each row's commit is drawn with.
fn parent_lines(rows: &[Row]) -> Vec<usize> {
    rows.iter()
        .map(|row| row.graph.edges.iter().filter(|e| e.half == Half::Lower && e.from == row.graph.column).count())
        .collect()
}

/// ```text
/// merge   Merge branch 'feature'         (main)
/// |  \
/// fix  |  Fix Parser crash: src/parser.rs
/// |    docs  Write docs: docs/guide.md, by Bob (feature)
/// |  /
/// parser  Add parser: src/parser.rs
/// init    Initial commit: README
/// ```
struct Repo {
    path: std::path::PathBuf,
    init: Oid,
    parser: Oid,
    docs: Oid,
    fix: Oid,
    merge: Oid,
}

fn history(fx: &Fixture) -> Repo {
    let path = fx.init("find");
    let init = commit(&path, "README", "hello\n", "Initial commit", T0);
    let parser = commit(&path, "src/parser.rs", "fn parse() {}\n", "Add parser", T0 + MINUTE);
    git(&path, &["switch", "--quiet", "-c", "feature"]);
    let docs = commit_as_bob(&path, "docs/guide.md", "Write docs", T0 + 2 * MINUTE);
    git(&path, &["switch", "--quiet", "main"]);
    let fix = commit(&path, "src/parser.rs", "fn parse() { fixed() }\n", "Fix Parser crash", T0 + 3 * MINUTE);
    git_at(&path, &["merge", "--quiet", "--no-ff", "-m", "Merge branch 'feature'", "feature"], T0 + 4 * MINUTE);
    let merge = rev_parse(&path, "HEAD");
    Repo { path, init, parser, docs, fix, merge }
}

#[test]
fn search_matches_messages_ignoring_case() {
    let fx = Fixture::new();
    let repo = history(&fx);
    let session = Session::open(&repo.path).unwrap();

    assert_eq!(found(&session, "PARSER"), [repo.fix, repo.parser]);
    assert_eq!(found(&session, "  fix parser "), [repo.fix]);
    assert_eq!(found(&session, "merge branch 'FEATURE'"), [repo.merge]);
    assert_eq!(found(&session, "nothing like this"), []);
}

#[test]
fn search_matches_author_names_and_emails() {
    let fx = Fixture::new();
    let repo = history(&fx);
    let session = Session::open(&repo.path).unwrap();

    assert_eq!(found(&session, "bob builder"), [repo.docs]);
    assert_eq!(found(&session, "bob@example.ORG"), [repo.docs]);
    assert_eq!(found(&session, "ada author").len(), 4, "every commit but Bob's");
}

#[test]
fn search_matches_id_prefixes() {
    let fx = Fixture::new();
    let repo = history(&fx);
    let session = Session::open(&repo.path).unwrap();

    let hex = repo.init.to_string();
    assert!(found(&session, &hex[..7].to_uppercase()).contains(&repo.init));
    assert_eq!(found(&session, &hex), [repo.init]);
}

#[test]
fn blank_searches_find_nothing() {
    let fx = Fixture::new();
    let repo = history(&fx);
    let session = Session::open(&repo.path).unwrap();

    let result = session.search("   ").unwrap();
    assert!(result.rows.is_empty());
    assert_eq!(result.total, 0);
}

#[test]
fn search_finds_stashes_but_not_the_working_tree() {
    let fx = Fixture::new();
    let repo = history(&fx);
    write(&repo.path, "README", "changed\n");
    git(&repo.path, &["stash", "push", "--quiet", "-m", "parked parser idea"]);
    write(&repo.path, "README", "dirty again\n");
    let session = Session::open(&repo.path).unwrap();
    let rows = all_rows(&session);
    assert_eq!(rows[0].kind, RowKind::WorkingTree);

    let result = session.search("parked").unwrap();
    assert_eq!(result.rows.len(), 1);
    assert_eq!(rows[result.rows[0] as usize].kind, RowKind::Stash);
    // "Uncommitted changes" is the working-tree row's text, not a commit's.
    assert_eq!(found(&session, "uncommitted"), []);
}

#[test]
fn filtering_by_branch_shows_only_what_it_reaches() {
    let fx = Fixture::new();
    let repo = history(&fx);
    let session = Session::open(&repo.path).unwrap();
    let full = session.info();

    let info = session.set_filter(refs_filter(&["refs/heads/feature"])).unwrap();
    assert!(info.generation > full.generation);
    assert_eq!(info.filter, refs_filter(&["refs/heads/feature"]));
    assert_eq!(ids(&session), [repo.docs, repo.parser, repo.init]);
    assert_eq!(parent_lines(&all_rows(&session)), [1, 1, 0], "one straight line");
    assert_eq!(session.locate(repo.fix).row, None, "hidden commits aren't located");
    assert_eq!(session.locate(repo.parser).row, Some(1));

    let both = session.set_filter(refs_filter(&["refs/heads/feature", "refs/heads/main"])).unwrap();
    assert_eq!(both.row_count, full.row_count);

    let none = session.set_filter(refs_filter(&["refs/heads/no-such-branch"])).unwrap();
    assert_eq!(none.row_count, 0);
}

#[test]
fn filtering_by_detached_head() {
    let fx = Fixture::new();
    let repo = history(&fx);
    git(&repo.path, &["switch", "--quiet", "--detach", &repo.parser.to_string()]);
    let session = Session::open(&repo.path).unwrap();

    session.set_filter(refs_filter(&["HEAD"])).unwrap();
    assert_eq!(ids(&session), [repo.parser, repo.init]);
}

#[test]
fn clearing_the_filter_restores_the_full_view() {
    let fx = Fixture::new();
    let repo = history(&fx);
    let session = Session::open(&repo.path).unwrap();
    let full = ids(&session);

    let filtered = session.set_filter(refs_filter(&["refs/heads/feature"])).unwrap();
    let cleared = session.set_filter(Filter::default()).unwrap();
    assert!(cleared.generation > filtered.generation, "a new snapshot, so the UI re-anchors");
    assert_eq!(cleared.filter, Filter::default());
    assert_eq!(ids(&session), full);
    assert_eq!(session.locate(repo.docs).row, Some(full.iter().position(|&id| id == repo.docs).unwrap() as u32));
}

#[test]
fn setting_the_same_filter_keeps_the_generation() {
    let fx = Fixture::new();
    let repo = history(&fx);
    let session = Session::open(&repo.path).unwrap();

    let opened = session.info();
    assert_eq!(session.set_filter(Filter::default()).unwrap(), opened);
    let filtered = session.set_filter(path_filter("src/")).unwrap();
    assert_eq!(session.set_filter(path_filter(" ./src")).unwrap(), filtered, "the same path, written differently");
}

#[test]
fn filtering_by_path_follows_the_side_a_merge_took_it_from() {
    let fx = Fixture::new();
    let repo = history(&fx);
    let session = Session::open(&repo.path).unwrap();

    // The merge took src/ unchanged from main, and the docs commit didn't touch it.
    session.set_filter(path_filter("src")).unwrap();
    assert_eq!(ids(&session), [repo.fix, repo.parser]);
    assert_eq!(parent_lines(&all_rows(&session)), [1, 0], "the fix leads to the parser commit");

    session.set_filter(path_filter("src/parser.rs")).unwrap();
    assert_eq!(ids(&session), [repo.fix, repo.parser]);

    // The merge took docs/ from the feature branch.
    session.set_filter(path_filter("docs/guide.md")).unwrap();
    assert_eq!(ids(&session), [repo.docs]);

    session.set_filter(path_filter("README")).unwrap();
    assert_eq!(ids(&session), [repo.init], "a root commit adding the path changes it");

    session.set_filter(path_filter("no/such/file")).unwrap();
    assert_eq!(session.info().row_count, 0);
}

#[test]
fn a_merge_combining_changes_to_a_path_is_shown_with_both_sides() {
    let fx = Fixture::new();
    let path = fx.init("combined");
    let base = commit(&path, "src/a.rs", "a\n", "base", T0);
    git(&path, &["switch", "--quiet", "-c", "side"]);
    let side = commit(&path, "src/b.rs", "b\n", "side", T0 + MINUTE);
    commit(&path, "notes.txt", "n\n", "side notes", T0 + 2 * MINUTE);
    git(&path, &["switch", "--quiet", "main"]);
    let main = commit(&path, "src/a.rs", "a2\n", "main", T0 + 3 * MINUTE);
    git_at(&path, &["merge", "--quiet", "--no-ff", "-m", "merge side", "side"], T0 + 4 * MINUTE);
    let merge = rev_parse(&path, "HEAD");
    let session = Session::open(&path).unwrap();

    session.set_filter(path_filter("src")).unwrap();
    let rows = all_rows(&session);
    assert_eq!(rows.iter().map(|row| row.id).collect::<Vec<_>>(), [merge, main, side, base]);
    // The merge's second line skips the hidden notes commit and reaches the side commit.
    assert_eq!(parent_lines(&rows), [2, 1, 1, 0]);
}

#[test]
fn filters_combine_branches_and_paths() {
    let fx = Fixture::new();
    let repo = history(&fx);
    let session = Session::open(&repo.path).unwrap();

    session.set_filter(Filter { refs: vec!["refs/heads/feature".into()], path: Some("src".into()) }).unwrap();
    assert_eq!(ids(&session), [repo.parser]);
}

#[test]
fn refreshes_keep_the_filter() {
    let fx = Fixture::new();
    let repo = history(&fx);
    let session = Session::open(&repo.path).unwrap();
    session.set_filter(path_filter("src")).unwrap();

    let docs = commit(&repo.path, "docs/more.md", "more\n", "More docs", T0 + 5 * MINUTE);
    let refreshed = session.refresh().unwrap();
    assert_eq!(refreshed.filter, path_filter("src"));
    assert!(!ids(&session).contains(&docs));

    let tweak = commit(&repo.path, "src/lexer.rs", "lex\n", "Add lexer", T0 + 6 * MINUTE);
    session.refresh().unwrap();
    assert_eq!(ids(&session), [tweak, repo.fix, repo.parser]);
}

#[test]
fn search_reports_rows_of_the_filtered_view() {
    let fx = Fixture::new();
    let repo = history(&fx);
    let session = Session::open(&repo.path).unwrap();
    session.set_filter(refs_filter(&["refs/heads/feature"])).unwrap();

    assert_eq!(session.search("parser").unwrap().rows, [1]);
    assert_eq!(found(&session, "parser"), [repo.parser]);
}

#[test]
fn uncommitted_changes_show_only_while_head_does() {
    let fx = Fixture::new();
    let repo = history(&fx);
    write(&repo.path, "README", "dirty\n");
    let session = Session::open(&repo.path).unwrap();
    assert_eq!(all_rows(&session)[0].kind, RowKind::WorkingTree);

    session.set_filter(refs_filter(&["refs/heads/feature"])).unwrap();
    assert!(all_rows(&session).iter().all(|row| row.kind != RowKind::WorkingTree), "HEAD is on main");

    session.set_filter(refs_filter(&["refs/heads/main"])).unwrap();
    let rows = all_rows(&session);
    assert_eq!(rows[0].kind, RowKind::WorkingTree);
    assert_eq!(rows[1].id, repo.merge);
}

#[test]
fn refs_lists_every_ref_whatever_the_filter() {
    let fx = Fixture::new();
    let repo = history(&fx);
    git(&repo.path, &["tag", "v1", &repo.init.to_string()]);
    let session = Session::open(&repo.path).unwrap();
    session.set_filter(path_filter("src")).unwrap();

    let names: Vec<String> = session.refs().into_iter().map(|label| label.full_name).collect();
    assert_eq!(names, ["refs/heads/feature", "refs/heads/main", "refs/tags/v1"]);
}
