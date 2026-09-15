//! Branch names in merge commit messages.
//!
//! A merge commit doesn't record which branches it joined, but its message usually says, in one of
//! a few fixed forms written by `git merge`, GitHub or GitLab. Only the summary line is parsed.

use super::{RepoError, details, git_error, to_object_id};
use crate::types::Oid;

/// The branches a merge commit's message names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MergeNames {
    /// The merged branches, in the order of the commit's second, third, … parents.
    pub sources: Vec<String>,
    /// The branch merged into, when the message says (`… into main`).
    pub target: Option<String>,
}

pub(super) fn read(repo: &gix::Repository, ids: &[Oid]) -> Result<Vec<Option<MergeNames>>, RepoError> {
    ids.iter()
        .map(|&id| {
            let Some(object) = repo
                .try_find_object(to_object_id(id))
                .map_err(|err| git_error(format!("Can't read commit {id}"), err))?
                .filter(|object| object.kind == gix::objs::Kind::Commit)
            else {
                return Ok(None);
            };
            let commit = gix::objs::CommitRef::from_bytes(&object.data, gix::hash::Kind::Sha1)
                .map_err(|err| git_error(format!("Can't parse commit {id}"), err))?;
            Ok(parse(&details::summary_line(commit.message)))
        })
        .collect()
}

/// The branch names in a merge commit's summary line, or `None` if it names no branch. Recognized:
///
/// - `git merge`: `Merge branch 'x'`, `Merge branches 'a', 'b' and 'c'`, `Merge remote-tracking
///   branch 'origin/x'`, each optionally followed by ` of <url>` and then ` into <target>`
/// - GitLab: the same with a quoted target, `Merge branch 'x' into 'main'`
/// - GitHub: `Merge pull request #12 from owner/x`, where `owner` is the account the branch lives
///   under and not part of the branch name
///
/// Tags (`Merge tag 'v1'`), commits and anything else name no branch.
pub(super) fn parse(summary: &str) -> Option<MergeNames> {
    let summary = summary.trim();
    if let Some(rest) = summary.strip_prefix("Merge pull request #") {
        let (number, rest) = rest.split_once(' ')?;
        let head = rest.strip_prefix("from ")?;
        if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) || !is_name(head) {
            return None;
        }
        let branch = head.split_once('/').map_or(head, |(_, branch)| branch);
        return is_name(branch).then(|| MergeNames { sources: vec![branch.to_owned()], target: None });
    }

    let rest = summary.strip_prefix("Merge ")?;
    let rest = ["remote-tracking branches ", "remote-tracking branch ", "branches ", "branch "]
        .iter()
        .find_map(|kind| rest.strip_prefix(kind))?;
    let (sources, mut rest) = quoted_list(rest)?;
    if let Some(location) = rest.strip_prefix(" of ") {
        // A URL or path, which contains no spaces.
        let end = location.find(' ').unwrap_or(location.len());
        if end == 0 {
            return None;
        }
        rest = &location[end..];
    }
    let target = match rest.strip_prefix(" into ") {
        Some(target) => {
            let target = target.strip_prefix('\'').and_then(|t| t.strip_suffix('\'')).unwrap_or(target);
            Some(is_name(target).then(|| target.to_owned())?)
        }
        None if rest.is_empty() => None,
        None => return None,
    };
    Some(MergeNames { sources, target })
}

/// `'a'`, `'a' and 'b'` or `'a', 'b' and 'c'`, and what follows the list.
fn quoted_list(mut text: &str) -> Option<(Vec<String>, &str)> {
    let mut names = Vec::new();
    loop {
        let quoted = text.strip_prefix('\'')?;
        let end = quoted.find('\'')?;
        let name = &quoted[..end];
        if !is_name(name) {
            return None;
        }
        names.push(name.to_owned());
        text = &quoted[end + 1..];
        match text.strip_prefix(", ").or_else(|| text.strip_prefix(" and ")) {
            Some(next) if next.starts_with('\'') => text = next,
            _ => return Some((names, text)),
        }
    }
}

fn is_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_summaries_name_their_branches() {
        let names = |sources: &[&str], target: Option<&str>| {
            Some(MergeNames { sources: sources.iter().map(|s| s.to_string()).collect(), target: target.map(str::to_owned) })
        };
        let cases = [
            ("Merge branch 'x'", names(&["x"], None)),
            ("Merge branch 'feature/x' into y", names(&["feature/x"], Some("y"))),
            ("Merge remote-tracking branch 'origin/x'", names(&["origin/x"], None)),
            ("Merge remote-tracking branch 'origin/x' into release/2.0", names(&["origin/x"], Some("release/2.0"))),
            ("Merge branch 'x' of https://github.com/acme/widget", names(&["x"], None)),
            ("Merge branch 'x' of github.com:acme/widget into main", names(&["x"], Some("main"))),
            ("Merge branches 'a', 'b' and 'c'", names(&["a", "b", "c"], None)),
            ("Merge branches 'a' and 'b' into dev", names(&["a", "b"], Some("dev"))),
            ("Merge remote-tracking branches 'origin/a' and 'origin/b'", names(&["origin/a", "origin/b"], None)),
            ("Merge pull request #12 from user/x", names(&["x"], None)),
            ("Merge pull request #7 from acme/feature/login", names(&["feature/login"], None)),
            ("Merge pull request #7 from x", names(&["x"], None)),
            ("Merge branch 'x' into 'main'", names(&["x"], Some("main"))),
            ("  Merge branch 'x'  ", names(&["x"], None)),
            ("Merge tag 'v1'", None),
            ("Merge tag 'v1' into main", None),
            ("Merge commit 'abc1234'", None),
            ("Merge branch 'x' and tag 'v1'", None),
            ("Merge branch 'x", None),
            ("Merge branch ''", None),
            ("Merge branch 'x' into", None),
            ("Merge branch 'x' of ", None),
            ("Merge pull request #12 from", None),
            ("Merge pull request #abc from user/x", None),
            ("merge branch 'x'", None),
            ("Revert \"Merge branch 'x'\"", None),
            ("Fix the parser", None),
            ("", None),
        ];
        for (summary, expected) in cases {
            assert_eq!(parse(summary), expected, "{summary:?}");
        }
    }
}
