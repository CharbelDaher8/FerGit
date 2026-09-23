//! Finding commits by message, author or id.

use super::{RepoError, git_error, to_object_id};
use crate::types::Oid;

/// Shortest id prefix matched against commit ids, as for `git rev-parse --short`. Shorter hex
/// strings still match messages and authors.
const MIN_ID_PREFIX: usize = 4;

/// The positions in `ids` of the commits `query` matches; see [`super::Repo::search`].
pub(super) fn search(repo: &gix::Repository, ids: &[Oid], query: &str) -> Result<Vec<usize>, RepoError> {
    let Some(matcher) = Matcher::new(query) else {
        return Ok(Vec::new());
    };
    let mut found = Vec::new();
    for (i, &id) in ids.iter().enumerate() {
        if matcher.matches_id(id) {
            found.push(i);
            continue;
        }
        let Some(object) = repo
            .try_find_object(to_object_id(id))
            .map_err(|err| git_error(format!("Can't read object {id}"), err))?
        else {
            continue;
        };
        if object.kind != gix::objs::Kind::Commit {
            continue;
        }
        let commit = gix::objs::CommitRef::from_bytes(&object.data, gix::hash::Kind::Sha1)
            .map_err(|err| git_error(format!("Can't parse commit {id}"), err))?;
        // A commit whose author line doesn't parse can still match by message.
        let author = commit.author().ok().map(|author| author.trim());
        if matcher.matches_text(commit.message)
            || author.is_some_and(|author| matcher.matches_text(author.name) || matcher.matches_text(author.email))
        {
            found.push(i);
        }
    }
    Ok(found)
}

/// A query, prepared for matching many commits.
struct Matcher {
    /// The query, lowercased.
    text: String,
    /// The query as lowercase hex, if it is long enough to be an id prefix.
    id_prefix: Option<String>,
}

impl Matcher {
    /// `None` for a query that is empty or only whitespace, which matches nothing.
    fn new(query: &str) -> Option<Matcher> {
        let query = query.trim();
        if query.is_empty() {
            return None;
        }
        let text = query.to_lowercase();
        let is_id_prefix =
            (MIN_ID_PREFIX..=40).contains(&query.len()) && query.bytes().all(|b| b.is_ascii_hexdigit());
        let id_prefix = is_id_prefix.then(|| text.clone());
        Some(Matcher { text, id_prefix })
    }

    fn matches_id(&self, id: Oid) -> bool {
        self.id_prefix.as_ref().is_some_and(|prefix| id.to_string().starts_with(prefix.as_str()))
    }

    /// Whether `haystack` (repository text: arbitrary bytes, usually UTF-8) contains the query,
    /// ignoring case.
    fn matches_text(&self, haystack: &[u8]) -> bool {
        let needle = self.text.as_bytes();
        if self.text.is_ascii() {
            // No allocation on the common path: compare bytes, folding ASCII letters only. Every
            // byte of a multi-byte UTF-8 character is non-ASCII, so none of them matches the query.
            return haystack.len() >= needle.len()
                && haystack.windows(needle.len()).any(|window| window.eq_ignore_ascii_case(needle));
        }
        String::from_utf8_lossy(haystack).to_lowercase().contains(&self.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matcher(query: &str) -> Matcher {
        Matcher::new(query).expect("a non-empty query")
    }

    #[test]
    fn blank_queries_match_nothing() {
        assert!(Matcher::new("").is_none());
        assert!(Matcher::new("  \t").is_none());
    }

    #[test]
    fn text_matches_ignore_case_and_surrounding_whitespace() {
        let m = matcher("  Fix Parser ");
        assert!(m.matches_text(b"Quickly fix parser bugs"));
        assert!(m.matches_text(b"FIX PARSER"));
        assert!(!m.matches_text(b"fix the parser"));
        assert!(!m.matches_text(b"fix"));
    }

    #[test]
    fn non_ascii_queries_fold_unicode_case() {
        let m = matcher("ÉCOLE");
        assert!(m.matches_text("à l'école".as_bytes()));
        assert!(!m.matches_text(b"ecole"));
        // Invalid UTF-8 in the haystack doesn't stop a match elsewhere in it.
        assert!(m.matches_text(b"\xff \xc3\x89cole"));
    }

    #[test]
    fn id_prefixes_need_four_hex_digits() {
        let id: Oid = "abcdef0123456789abcdef0123456789abcdef01".parse().unwrap();
        assert!(matcher("ABCD").matches_id(id));
        assert!(matcher("abcdef0123456789abcdef0123456789abcdef01").matches_id(id));
        assert!(!matcher("abc").matches_id(id), "too short to be an id prefix");
        assert!(!matcher("bcde").matches_id(id), "a prefix, not a substring");
        assert!(!matcher("abcg").matches_id(id));
    }
}
