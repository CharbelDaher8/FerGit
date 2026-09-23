//! Removing credentials from git's output before anything shows, stores or journals it.
//!
//! Git prints remote URLs as configured, and a URL can carry a user name and token
//! (`https://user:ghp_…@github.com/…`). Credentials typed into the askpass dialog never pass
//! through git's output, so URLs (and, defensively, HTTP authorization headers from a trace) are
//! what needs scrubbing.

/// Replaces the user-info part of every URL in `text` (`https://user:token@host/…` becomes
/// `https://***@host/…`), and the value of any `Authorization:` header, with `***`.
pub(crate) fn scrub(text: &str) -> String {
    text.split_inclusive('\n').map(scrub_line).collect()
}

fn scrub_line(line: &str) -> String {
    if let Some(at) = line.to_ascii_lowercase().find("authorization:") {
        let end = at + "authorization:".len();
        let newline = if line.ends_with('\n') { "\n" } else { "" };
        return format!("{} ***{newline}", &line[..end]);
    }
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(scheme_end) = rest.find("://") {
        let authority_start = scheme_end + 3;
        out.push_str(&rest[..authority_start]);
        rest = &rest[authority_start..];
        let authority_len = rest
            .find(|c: char| c == '/' || c.is_whitespace() || matches!(c, '\'' | '"' | '<' | '>' | '`'))
            .unwrap_or(rest.len());
        let authority = &rest[..authority_len];
        match authority.rfind('@') {
            Some(at) => {
                out.push_str("***");
                out.push_str(&authority[at..]);
            }
            None => out.push_str(authority),
        }
        rest = &rest[authority_len..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::scrub;

    #[test]
    fn removes_user_info_from_urls() {
        assert_eq!(
            scrub("To https://me:ghp_secret@github.com/o/r.git\n ! [rejected] main -> main\n"),
            "To https://***@github.com/o/r.git\n ! [rejected] main -> main\n"
        );
        assert_eq!(scrub("fatal: 'https://token@host/x' failed"), "fatal: 'https://***@host/x' failed");
        assert_eq!(scrub("a http://u:p@h b http://u2:p2@h2/c"), "a http://***@h b http://***@h2/c");
    }

    #[test]
    fn leaves_urls_without_credentials_and_other_text_alone() {
        let text = "From https://github.com/o/r\n * branch main -> FETCH_HEAD\nuser@host is fine\n";
        assert_eq!(scrub(text), text);
        assert_eq!(scrub("ssh://git@github.com/o/r"), "ssh://***@github.com/o/r");
        assert_eq!(scrub(""), "");
    }

    #[test]
    fn removes_authorization_headers() {
        assert_eq!(scrub("> Authorization: Basic dXNlcjpwYXNz\nok\n"), "> Authorization: ***\nok\n");
    }
}
