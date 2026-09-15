//! File contents: loading a version, classifying it (text, binary, too large, submodule), counting
//! changed lines and building `git diff`-style hunks.

use gix::bstr::BStr;
use gix::diff::blob::{Algorithm, Diff, InternedInput};

use super::{Sides, Version};
use crate::repo::{RepoError, git_error, lossy, to_oid};
use crate::types::{DiffLine, FileDiff, Hunk, LineKind};

/// Versions larger than this aren't line-diffed: [`FileDiff::TooLarge`], and no line counts. A diff
/// this size takes tens of milliseconds, and a commit may touch many such files.
pub(in crate::repo) const MAX_DIFF_BYTES: u64 = 8 * 1024 * 1024;

/// Git's binary heuristic: a NUL byte within the first 8000 bytes.
pub(in crate::repo) const BINARY_PROBE_BYTES: usize = 8000;

/// Lines of unchanged context around each change, as in `git diff`.
const CONTEXT_LINES: u32 = 3;

pub(super) fn is_binary(data: &[u8]) -> bool {
    data[..data.len().min(BINARY_PROBE_BYTES)].contains(&0)
}

/// A version's content, classified.
pub(super) enum Content {
    Text(Vec<u8>),
    Binary,
    TooLarge,
}

/// The content of `version` found at `path`; a missing version is empty text.
fn load(sides: &mut Sides<'_>, version: Option<Version>, path: &BStr) -> Result<Content, RepoError> {
    match version {
        None => Ok(Content::Text(Vec::new())),
        Some(Version::Object { id, .. }) => load_object(sides.repo, id),
        Some(Version::Worktree { mode }) => sides.worktree()?.content(path, mode),
    }
}

fn load_object(repo: &gix::Repository, id: gix::ObjectId) -> Result<Content, RepoError> {
    // `git add --intent-to-add` records the empty blob without writing it.
    if id.is_empty_blob() {
        return Ok(Content::Text(Vec::new()));
    }
    let read_error = |err| git_error(format!("Can't read blob {id}"), err);
    if repo.find_header(id).map_err(read_error)?.size() > MAX_DIFF_BYTES {
        return Ok(Content::TooLarge);
    }
    Ok(classify(repo.find_object(id).map_err(read_error)?.detach().data))
}

pub(super) fn classify(data: Vec<u8>) -> Content {
    if is_binary(&data) { Content::Binary } else { Content::Text(data) }
}

fn is_submodule(version: Option<Version>) -> bool {
    version.is_some_and(|version| version.mode().is_commit())
}

/// Added and deleted lines between two versions (absent versions count as empty), or `None` for
/// submodules, binary files and files too large to diff.
pub(super) fn line_counts(
    sides: &mut Sides<'_>,
    old: Option<Version>,
    old_path: &BStr,
    new: Option<Version>,
    new_path: &BStr,
) -> Result<(Option<u32>, Option<u32>), RepoError> {
    // A submodule entry names a commit in another repository; there are no lines to count.
    if is_submodule(old) || is_submodule(new) {
        return Ok((None, None));
    }
    if let (Some(Version::Object { id: a, .. }), Some(Version::Object { id: b, .. })) = (old, new) {
        if a == b {
            return Ok((Some(0), Some(0)));
        }
    }
    let (Content::Text(before), Content::Text(after)) = (load(sides, old, old_path)?, load(sides, new, new_path)?)
    else {
        return Ok((None, None));
    };
    let (_, diff) = line_diff(&before, &after);
    Ok((Some(diff.count_additions()), Some(diff.count_removals())))
}

pub(super) fn file_diff(
    sides: &mut Sides<'_>,
    old: Option<Version>,
    old_path: &BStr,
    new: Option<Version>,
    new_path: &BStr,
) -> Result<FileDiff, RepoError> {
    if is_submodule(old) || is_submodule(new) {
        let commit = |version: Option<Version>| match version {
            Some(Version::Object { mode, id }) if mode.is_commit() => Some(to_oid(&id)),
            _ => None,
        };
        return Ok(FileDiff::Submodule { old: commit(old), new: commit(new) });
    }
    // Too large wins over binary: a side over the limit is never read, so it can't be probed.
    Ok(match (load(sides, old, old_path)?, load(sides, new, new_path)?) {
        (Content::TooLarge, _) | (_, Content::TooLarge) => FileDiff::TooLarge,
        (Content::Binary, _) | (_, Content::Binary) => FileDiff::Binary,
        (Content::Text(before), Content::Text(after)) => FileDiff::Text { hunks: hunks(&before, &after) },
    })
}

/// A line diff like git's default: Myers, then sliding ambiguous hunks with git's indent heuristic
/// (`diff.indentHeuristic`, on by default), so hunk boundaries match `git diff`.
fn line_diff<'a>(before: &'a [u8], after: &'a [u8]) -> (InternedInput<&'a [u8]>, Diff) {
    let input = InternedInput::new(before, after);
    let mut diff = Diff::compute(Algorithm::Myers, &input);
    diff.postprocess_lines(&input);
    (input, diff)
}

/// Changes grouped into hunks with [`CONTEXT_LINES`] of context, merging changes separated by at
/// most twice that many unchanged lines, as git's xdiff does.
fn hunks(before: &[u8], after: &[u8]) -> Vec<Hunk> {
    let (input, diff) = line_diff(before, after);
    let (old_len, new_len) = (input.before.len() as u32, input.after.len() as u32);
    let line = |kind, token, is_last_line: bool| {
        let text: &[u8] = input.interner[token];
        let without_newline = text.strip_suffix(b"\n");
        let text = without_newline.map_or(text, |text| text.strip_suffix(b"\r").unwrap_or(text));
        DiffLine { kind, text: lossy(text), no_final_newline: is_last_line && without_newline.is_none() }
    };

    let changes: Vec<_> = diff.hunks().collect();
    let mut hunks = Vec::new();
    let mut group_start = 0;
    while group_start < changes.len() {
        let mut group_end = group_start + 1;
        while group_end < changes.len()
            && changes[group_end].before.start - changes[group_end - 1].before.end <= 2 * CONTEXT_LINES
        {
            group_end += 1;
        }
        let group = &changes[group_start..group_end];
        group_start = group_end;

        let (first, last) = (&group[0], &group[group.len() - 1]);
        let leading = first.before.start.min(CONTEXT_LINES);
        let trailing = (old_len - last.before.end).min(CONTEXT_LINES);
        let (old_first, new_first) = (first.before.start - leading, first.after.start - leading);
        let (old_end, new_end) = (last.before.end + trailing, last.after.end + trailing);

        let mut lines = Vec::new();
        let context = |lines: &mut Vec<DiffLine>, from: u32, to: u32| {
            for i in from..to {
                lines.push(line(LineKind::Context, input.before[i as usize], i + 1 == old_len));
            }
        };
        let mut position = old_first;
        for change in group {
            context(&mut lines, position, change.before.start);
            for i in change.before.clone() {
                lines.push(line(LineKind::Removed, input.before[i as usize], i + 1 == old_len));
            }
            for i in change.after.clone() {
                lines.push(line(LineKind::Added, input.after[i as usize], i + 1 == new_len));
            }
            position = change.before.end;
        }
        context(&mut lines, position, old_end);

        // Git numbers lines from 1, but a range with no lines starts at the line before it (0 for
        // an empty file): `@@ -0,0 +1,3 @@`.
        let start = |first: u32, count: u32| if count == 0 { first } else { first + 1 };
        let (old_lines, new_lines) = (old_end - old_first, new_end - new_first);
        hunks.push(Hunk {
            old_start: start(old_first, old_lines),
            old_lines,
            new_start: start(new_first, new_lines),
            new_lines,
            lines,
        });
    }
    hunks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(hunks: &[Hunk]) -> String {
        let mut out = String::new();
        for hunk in hunks {
            out += &format!("@@ -{},{} +{},{} @@\n", hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines);
            for line in &hunk.lines {
                let prefix = match line.kind {
                    LineKind::Context => ' ',
                    LineKind::Added => '+',
                    LineKind::Removed => '-',
                };
                out += &format!("{prefix}{}\n", line.text);
                if line.no_final_newline {
                    out += "\\ No newline at end of file\n";
                }
            }
        }
        out
    }

    #[test]
    fn identical_contents_have_no_hunks() {
        assert!(hunks(b"a\nb\n", b"a\nb\n").is_empty());
        assert!(hunks(b"", b"").is_empty());
    }

    #[test]
    fn an_added_file_starts_at_line_zero_on_the_old_side() {
        assert_eq!(render(&hunks(b"", b"a\nb")), "@@ -0,0 +1,2 @@\n+a\n+b\n\\ No newline at end of file\n");
    }

    #[test]
    fn changes_six_lines_apart_share_a_hunk_and_seven_apart_do_not() {
        let lines = |changed: [u32; 2]| -> String {
            (1..=30).map(|n| if changed.contains(&n) { format!("changed {n}\n") } else { format!("{n}\n") }).collect()
        };
        let old = lines([0, 0]);
        // Lines 6 to 11 are unchanged between 5 and 12; lines 6 to 12 between 5 and 13.
        assert_eq!(hunks(old.as_bytes(), lines([5, 12]).as_bytes()).len(), 1);
        assert_eq!(hunks(old.as_bytes(), lines([5, 13]).as_bytes()).len(), 2);
    }

    #[test]
    fn crlf_terminators_are_stripped_from_line_text() {
        let hunks = hunks(b"a\r\nb\r\n", b"a\r\nc\r\n");
        let texts: Vec<_> = hunks[0].lines.iter().map(|line| line.text.as_str()).collect();
        assert_eq!(texts, ["a", "b", "c"]);
    }
}
